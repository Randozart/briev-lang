//! Cross-thread load-stride analysis (M3.5, plan 2026-09-18-coalesced-kv-memory-path).
//!
//! Doctrine (user directive, 2026-09-18): the compiler must, under all
//! circumstances, SHOW THROUGH ANALYSIS what the fastest code would be.
//! The programmer's lever is writing better algorithms — never strategy
//! keywords or pragma trickery. This pass is the analysis half of that
//! promise for memory coalescing: when a kernel's work items read a state
//! array with a proven cross-thread stride, the compiler says so, names
//! the stride, and states the mechanical fix (store the field so
//! consecutive work items read consecutive elements). It never rewrites
//! anything: the layout is part of the program's host ABI (field tables,
//! seed path, append ranges), so the decision stays with the author.
//!
//! The stride model: at a FIXED position of every inner loop, the address
//! a work item reads must advance by one element for consecutive work
//! items to coalesce. We extract the integer coefficient of the kernel's
//! index variable in each load's index expression, treating
//! division/modulo-protected terms as run-constant (within a run of N
//! consecutive work items, `t / N` is fixed — the local-affine view that
//! makes q look like a broadcast and k look like a stride-D read, which
//! is exactly what the device does). Anything unprovable suppresses the
//! warning: this pass only speaks when it can prove.

use crate::analysis::accel::AccelEntry;
use crate::ast::{Expr, Statement, TopLevel};
use crate::errors::{Diagnostic, Severity};
use std::collections::HashMap;

/// Affine classification of an index expression in the work-item variable.
#[derive(Debug, Clone, PartialEq)]
enum Coeff {
    /// address advances by this many elements per work item
    Stride(i64),
    /// no dependence on the work item; carries the folded value so a
    /// multiplication can scale a stride (index j*D needs D's value)
    Constant(i64),
    /// unprovable — carries WHICH term defeated the affine proof (the
    /// limits-of-proof report, G002: the honest boundary is visible)
    Unknown(&'static str),
}

pub fn analyze_coalescing(
    items: &[TopLevel],
    entries: &HashMap<String, AccelEntry>,
    consts: &HashMap<String, Expr>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut reported: std::collections::HashSet<(String, String)> = Default::default();
    for (name, entry) in entries {
        let shape = &entry.shape;
        if shape.index_var.is_empty() {
            continue;
        }
        // Cooperative row kernels map the LANES from tid.x, not from the
        // work-item counter — the work-item stride is the row stride
        // there, which is not a coalescing defect (the softmax's s reads
        // coalesce across lanes). The stride metric is only sound for
        // flat 1D work-item == thread mappings.
        if crate::analysis::accel::is_cooperative_shape(shape) {
            continue;
        }
        // Straight-line local environment: name -> defining expression
        // (lets and assigns; last one wins — kernel bodies are
        // straight-line by the shape prover's contract).
        let mut locals: HashMap<String, Expr> = HashMap::new();
        collect_locals_into(&shape.kernel_stmts, &mut locals);
        let txn_span = items.iter().find_map(|i| match i {
            TopLevel::Transaction(t) if t.name == *name => t.span,
            _ => None,
        });
        let mut loads: Vec<(String, Expr)> = Vec::new();
        collect_array_loads(&shape.kernel_stmts, &mut loads);
        let mut unproven: HashMap<String, &'static str> = HashMap::new();
        for (field, idx) in &loads {
            let elems = match stride_coeff(idx, shape, consts, &locals) {
                Coeff::Stride(e) => e,
                Coeff::Unknown(why) => {
                    // Limits-of-proof report (F2, doctrine point 1): the
                    // boundary of the analysis is visible, always on. One
                    // reason per (kernel, field).
                    unproven.entry(field.clone()).or_insert(why);
                    continue;
                }
                Coeff::Constant(_) => continue,
            };
            if elems.abs() < 2 {
                continue;
            }
            if !reported.insert((name.clone(), field.clone())) {
                continue;
            }
            let mut d = Diagnostic::new(
                "G001",
                Severity::Warning,
                &format!(
                    "kernel '{name}' reads '{field}' with a cross-thread stride of {elems} element{}",
                    if elems.abs() == 1 { "" } else { "s" }
                ),
            );
            if let Some(s) = txn_span {
                d = d.with_span(s);
            }
            d = d
                .with_explanation(&format!(
                    "consecutive work items index '{field}' {elems} elements apart, so each 32-byte device memory transaction carries {elems} used bytes when the hardware can coalesce 8"
                ))
                .with_proof_step(&format!(
                    "at a fixed inner-loop position the load index reduces to a linear form advancing {elems} elements per unit of the work-item counter"
                ))
                .with_hint(&format!(
                    "store '{field}' so consecutive work items read consecutive elements (transpose the storage order for this kernel's mapping)"
                ))
                .with_note(
                    "layout is part of the program's host contract; the compiler reports, the author decides",
                );
            out.push(d);
        }
        // Flush the limits-of-proof reports for this kernel (G002, Info):
        // the analysis names what it could NOT prove, per field, with the
        // defeating construct. Always on — the honest boundary is part of
        // the output, not a debug mode.
        for (field, why) in &unproven {
            let mut d = Diagnostic::new(
                "G002",
                Severity::Info,
                &format!("coalescing of field '{field}' in kernel '{name}' unproven"),
            );
            if let Some(s) = txn_span {
                d = d.with_span(s);
            }
            d = d
                .with_explanation(&format!(
                    "the load index is not provably affine in the work-item counter: {why}"
                ))
                .with_note(
                    "the compiler can neither confirm coalescing nor prove a defect here; this is the analysis limit, not a verified-clean bill",
                );
            out.push(d);
        }
    }
    out
}

fn collect_locals_into(stmts: &[Statement], locals: &mut HashMap<String, Expr>) {
    for s in stmts {
        match s {
            Statement::Let {
                name, expr: Some(e), ..
            } => {
                locals.insert(name.clone(), e.clone());
            }
            Statement::Assign(lhs, rhs) => {
                if let Expr::Identifier(n) = lhs {
                    locals.insert(n.clone(), rhs.clone());
                }
            }
            Statement::Foreach { body, .. } => collect_locals_into(body, locals),
            _ => {}
        }
    }
}

fn collect_array_loads(stmts: &[Statement], out: &mut Vec<(String, Expr)>) {
    fn walk_expr(e: &Expr, out: &mut Vec<(String, Expr)>) {
        match e {
            Expr::Index(obj, idx) => {
                if let Expr::Identifier(f) = obj.as_ref() {
                    out.push((f.clone(), (**idx).clone()));
                }
                walk_expr(idx, out);
            }
            Expr::BinaryOp(_, l, r) => {
                walk_expr(l, out);
                walk_expr(r, out);
            }
            Expr::UnaryOp(_, x) => walk_expr(x, out),
            Expr::Call(_, args, _) => {
                for a in args {
                    walk_expr(a, out);
                }
            }
            Expr::Cast(x, _) => walk_expr(x, out),
            _ => {}
        }
    }
    for s in stmts {
        match s {
            Statement::Assign(_, rhs) => walk_expr(rhs, out),
            Statement::Let {
                expr: Some(e), ..
            } => walk_expr(e, out),
            Statement::Foreach { body, .. } => collect_array_loads(body, out),
            _ => {}
        }
    }
}

/// The loop variables held at a fixed position: the kernel's inner foreach
/// items. The shape's own loop_var field IS the outer counter; inner items
/// come from the foreach statements.
fn stride_coeff(
    idx: &Expr,
    shape: &crate::analysis::accel::KernelShape,
    consts: &HashMap<String, Expr>,
    locals: &HashMap<String, Expr>,
) -> Coeff {
    // Collect inner foreach items so they can be held at zero.
    let mut inner_items: Vec<String> = Vec::new();
    fn find_items(stmts: &[Statement], out: &mut Vec<String>) {
        for s in stmts {
            if let Statement::Foreach { item, body, .. } = s {
                out.push(item.clone());
                find_items(body, out);
            }
        }
    }
    find_items(&shape.kernel_stmts, &mut inner_items);
    // The contract counter (shape.index_var) flows through the lets; every
    // foreach item is an inner serial loop variable held at a fixed
    // position for the stride question. Div-protected derived locals
    // (h = t / NKV) resolve to run-constant inside CoeffEnv.
    let mut env = CoeffEnv {
        item: shape.index_var.clone(),
        held_at_zero: inner_items,
        consts,
        locals,
        depth: 0,
    };
    env.coeff(idx)
}

struct CoeffEnv<'a> {
    item: String,
    held_at_zero: Vec<String>,
    consts: &'a HashMap<String, Expr>,
    locals: &'a HashMap<String, Expr>,
    depth: u32,
}

impl CoeffEnv<'_> {
    /// Fold an identifier through consts/locals to an integer, when
    /// provably constant.
    fn const_value(&self, n: &str) -> Option<i64> {
        fn ev(
            e: &Expr,
            consts: &HashMap<String, Expr>,
            locals: &HashMap<String, Expr>,
            depth: u32,
        ) -> Option<i64> {
            if depth > 8 {
                return None;
            }
            match e {
                Expr::Decimal(v) => Some(*v),
                Expr::Identifier(n) => consts
                    .get(n)
                    .and_then(|x| ev(x, consts, locals, depth + 1))
                    .or_else(|| locals.get(n).and_then(|x| ev(x, consts, locals, depth + 1))),
                Expr::BinaryOp(kind, l, r) => {
                    let a = ev(l, consts, locals, depth + 1)?;
                    let b = ev(r, consts, locals, depth + 1)?;
                    use crate::ast::BinaryOpKind::{Add, Div, Mod, Mul, Sub};
                    match kind {
                        Add => Some(a + b),
                        Sub => Some(a - b),
                        Mul => Some(a * b),
                        Div if b != 0 => Some(a / b),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        self.consts
            .get(n)
            .and_then(|x| ev(x, self.consts, self.locals, 0))
            .or_else(|| self.locals.get(n).and_then(|x| ev(x, self.consts, self.locals, 0)))
    }

    fn coeff(&mut self, e: &Expr) -> Coeff {
        self.depth += 1;
        if self.depth > 16 {
            self.depth -= 1;
            return Coeff::Unknown("analysis depth limit");
        }
        let out = self.coeff_inner(e);
        self.depth -= 1;
        out
    }

    fn coeff_inner(&mut self, e: &Expr) -> Coeff {
        match e {
            Expr::Decimal(v) => Coeff::Constant(*v),
            Expr::Identifier(n) => {
                if *n == self.item {
                    Coeff::Stride(1)
                } else if self.held_at_zero.iter().any(|h| h == n) {
                    Coeff::Constant(0)
                } else if let Some(v) = self.const_value(n) {
                    Coeff::Constant(v)
                } else if let Some(def) = self.locals.get(n) {
                    if self.depth < 12 {
                        self.coeff(def)
                    } else {
                        Coeff::Unknown("local inlining depth limit")
                    }
                } else {
                    // scalar state read or unknown name: uniform across
                    // threads either way; value unknown → 0 (safe for
                    // multiplication: contributes no stride)
                    Coeff::Constant(0)
                }
            }
            Expr::BinaryOp(kind, l, r) => {
                let lc = self.coeff(l);
                let rc = self.coeff(r);
                use crate::ast::BinaryOpKind::{Add, Div, Mod, Mul, Sub};
                match (kind, &lc, &rc) {
                    (_, Coeff::Unknown(w), _) => Coeff::Unknown(w),
                    (_, _, Coeff::Unknown(w)) => Coeff::Unknown(w),
                    (Add, a, b) => match (a, b) {
                        (Coeff::Stride(x), Coeff::Stride(y)) => Coeff::Stride(x + y),
                        (Coeff::Stride(x), Coeff::Constant(c)) => Coeff::Stride(x + c),
                        (Coeff::Constant(c), Coeff::Stride(x)) => Coeff::Stride(x + c),
                        (Coeff::Constant(a), Coeff::Constant(b)) => Coeff::Constant(a + b),
                        _ => Coeff::Constant(0),
                    },
                    (Sub, a, b) => match (a, b) {
                        (Coeff::Stride(x), Coeff::Stride(y)) => Coeff::Stride(x - y),
                        (Coeff::Stride(x), Coeff::Constant(c)) => Coeff::Stride(x - c),
                        (Coeff::Constant(c), Coeff::Stride(y)) => Coeff::Stride(c - y),
                        (Coeff::Constant(a), Coeff::Constant(b)) => Coeff::Constant(a - b),
                        _ => Coeff::Constant(0),
                    },
                    (Mul, a, b) => match (a, b) {
                        (Coeff::Stride(x), Coeff::Constant(c)) => Coeff::Stride(x * c),
                        (Coeff::Constant(c), Coeff::Stride(x)) => Coeff::Stride(x * c),
                        (Coeff::Constant(a), Coeff::Constant(b)) => Coeff::Constant(a * b),
                        _ => Coeff::Constant(0),
                    },
                    // Division/modulo by anything: the quotient is fixed
                    // over runs of the divisor — run-constant in the
                    // local-affine view (this is what makes `h = t / NKV`
                    // a broadcast term rather than a stride term). Value 0
                    // is the safe identity: it contributes no stride.
                    (Div, _, _) => Coeff::Constant(0),
                    // Modulo is locally affine (consecutive work items
                    // advance the remainder by one until the wrap), so the
                    // dividend's coefficient is the honest local stride.
                    (Mod, l, _) => l.clone(),
                    _ => Coeff::Unknown("unsupported index operator"),
                }
            }
            Expr::UnaryOp(_, x) => self.coeff(x),
            Expr::Call(_, _, _) => Coeff::Unknown("intrinsic call result"),
            Expr::Index(_, _) => Coeff::Unknown("nested buffer indexing"),
            _ => Coeff::Unknown("unmodeled expression form"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Statement};

    fn b(op: BinaryOpKind, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(op, Box::new(l), Box::new(r))
    }
    fn id(n: &str) -> Expr {
        Expr::Identifier(n.into())
    }
    fn num(v: i64) -> Expr {
        Expr::Decimal(v)
    }

    fn shape(stmts: Vec<Statement>, index_var: &str) -> crate::analysis::accel::KernelShape {
        // Minimal shape: the analysis reads index_var + kernel_stmts only.
        crate::analysis::accel::KernelShape {
            index_var: index_var.into(),
            count_expr: None,
            read_buffers: vec![],
            write_buffers: vec![],
            scalar_ins: vec![],
            work_cols: None,
            reduction: None,
            kernel_stmts: stmts,
            host_stmts: vec![],
            eligible: true,
            reasons: vec![],
        }
    }

    fn consts() -> HashMap<String, i64> {
        [("D".to_string(), 128), ("NKV".to_string(), 4096), ("G".to_string(), 4)]
            .into_iter()
            .collect()
    }

    fn const_exprs() -> HashMap<String, Expr> {
        [
            ("D".to_string(), Expr::Decimal(128)),
            ("NKV".to_string(), Expr::Decimal(4096)),
            ("G".to_string(), Expr::Decimal(4)),
        ]
        .into_iter()
        .collect()
    }

    /// The qk decomposition shape: h = t / NKV; j = t - h * NKV;
    /// kh = h / G; foreach d { acc += q[h*D+d] * k[kh*D*NKV + j*D + d] }.
    #[test]
    fn qk_k_load_is_proven_scattered() {
        let stmts = vec![
            Statement::Let {
                name: "h".into(),
                names: vec![],
                ty: None,
                expr: Some(b(BinaryOpKind::Div, id("t"), id("NKV"))),
                modifiers: vec![],
            },
            Statement::Let {
                name: "j".into(),
                names: vec![],
                ty: None,
                expr: Some(b(
                    BinaryOpKind::Sub,
                    id("t"),
                    b(BinaryOpKind::Mul, id("h"), id("NKV")),
                )),
                modifiers: vec![],
            },
            Statement::Let {
                name: "kh".into(),
                names: vec![],
                ty: None,
                expr: Some(b(BinaryOpKind::Div, id("h"), id("G"))),
                modifiers: vec![],
            },
            Statement::Foreach {
                item: "d".into(),
                list: Box::new(Expr::Range {
                    start: Box::new(num(0)),
                    end: Box::new(id("D")),
                    inclusive: false,
                }),
                body: vec![Statement::Assign(
                    id("acc"),
                    b(
                        BinaryOpKind::Add,
                        id("acc"),
                        b(
                            BinaryOpKind::Mul,
                            Expr::Index(Box::new(id("q")), Box::new(b(
                                BinaryOpKind::Add,
                                b(BinaryOpKind::Mul, id("h"), id("D")),
                                id("d"),
                            ))),
                            Expr::Cast(
                                Box::new(Expr::Index(
                                    Box::new(id("k")),
                                    Box::new(b(
                                        BinaryOpKind::Add,
                                        b(
                                            BinaryOpKind::Add,
                                            b(BinaryOpKind::Mul, id("kh"), b(BinaryOpKind::Mul, id("D"), id("NKV"))),
                                            b(BinaryOpKind::Mul, id("j"), id("D")),
                                        ),
                                        id("d"),
                                    )),
                                )),
                                crate::ast::Type::Bits(32),
                            ),
                        ),
                    ),
                )],
            },
        ];
        let s = shape(stmts, "t");
        let locals = {
            let mut m = HashMap::new();
            collect_locals_into(&s.kernel_stmts, &mut m);
            m
        };
        let c = stride_coeff(
            &b(
                BinaryOpKind::Add,
                b(
                    BinaryOpKind::Add,
                    b(BinaryOpKind::Mul, id("kh"), b(BinaryOpKind::Mul, id("D"), id("NKV"))),
                    b(BinaryOpKind::Mul, id("j"), id("D")),
                ),
                id("d"),
            ),
            &s,
            &const_exprs(),
            &locals,
        );
        assert_eq!(c, Coeff::Stride(128), "k must prove a stride-D scattered read");
    }

    /// The q load in the same kernel: h = t / NKV is div-protected —
    /// run-constant, i.e. a broadcast within a run. No warning.
    #[test]
    fn qk_q_load_is_broadcast() {
        let stmts = vec![Statement::Let {
            name: "h".into(),
            names: vec![],
            ty: None,
            expr: Some(b(BinaryOpKind::Div, id("t"), id("NKV"))),
            modifiers: vec![],
        }];
        let s = shape(stmts, "t");
        let locals = {
            let mut m = HashMap::new();
            collect_locals_into(&s.kernel_stmts, &mut m);
            m
        };
        let c = stride_coeff(
            &b(
                BinaryOpKind::Add,
                b(BinaryOpKind::Mul, id("h"), id("D")),
                id("d_held"),
            ),
            &s,
            &const_exprs(),
            &locals,
        );
        assert_eq!(c, Coeff::Constant(0), "div-protected h must read as broadcast");
    }

    /// F2 limits-of-proof (plan coalesced-kv-memory-path): a load indexed
    /// through ANOTHER buffer (`k[ii[j]]`) is not affine in the work item —
    /// the analysis must NAME the defeat (G002) instead of staying silent.
    /// Silence on the unprovable would overstate the analysis.
    #[test]
    fn nested_index_load_is_reported_unproven() {
        let stmts = vec![
            Statement::Foreach {
                item: "j".into(),
                list: Box::new(Expr::Range {
                    start: Box::new(num(0)),
                    end: Box::new(id("D")),
                    inclusive: false,
                }),
                body: vec![Statement::Assign(
                    id("acc"),
                    b(
                        BinaryOpKind::Add,
                        id("acc"),
                        Expr::Index(
                            Box::new(id("k")),
                            Box::new(Expr::Index(Box::new(id("ii")), Box::new(id("j")))),
                        ),
                    ),
                )],
            },
        ];
        let s = shape(stmts, "t");
        let locals = {
            let mut m = HashMap::new();
            collect_locals_into(&s.kernel_stmts, &mut m);
            m
        };
        // The k load's index is `ii[j]` — a nested buffer read.
        let c = stride_coeff(
            &Expr::Index(Box::new(id("ii")), Box::new(id("j"))),
            &s,
            &const_exprs(),
            &locals,
        );
        assert_eq!(
            c,
            Coeff::Unknown("nested buffer indexing"),
            "nested indexing must be reported as an analysis limit"
        );
    }
}
