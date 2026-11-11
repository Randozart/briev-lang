// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-11 (H5 port): cross-thread load-stride analysis over typed HIR.
//!
//! Mirrors `analysis::coalescing`. The pass only speaks when it can prove:
//! at a fixed inner-loop position, the integer coefficient of the work-item
//! counter in a load's index expression is the cross-thread stride. Division
//! and modulo terms are run-constant in the local-affine view, so they
//! contribute no stride. Everything else is reported as an analysis limit
//! (G002), never silently clean.
//!
//! Result type: the public `errors::Diagnostic` is AST-free. Inputs are HIR
//! callables, HIR kernel statements, HIR consts, and the HIR accel result.

use crate::analysis::accel::AccelEntry;
use crate::errors::{Diagnostic, Severity};
use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral};
use crate::hir::program::{HirCallable, HirCallableKind, HirProgram};
use crate::hir::stmt::{HirStmt, HirStmtKind};
use std::collections::{BTreeMap, HashMap};

/// Affine classification of an index expression in the work-item variable.
#[derive(Debug, Clone, PartialEq)]
enum Coeff {
    Stride(i64),
    Constant(i64),
    Unknown(&'static str),
}

/// Cross-kernel report state (one reason per kernel/field).
#[derive(Default)]
struct Report {
    reported: std::collections::HashSet<(String, String)>,
    diagnostics: Vec<Diagnostic>,
}

/// Analyze every eligible HIR accel kernel in deterministic name order.
pub fn analyze(program: &HirProgram, entries: &HashMap<String, AccelEntry>) -> Vec<Diagnostic> {
    let consts = module_consts(program);
    let mut names: Vec<&String> = entries.keys().collect();
    names.sort();
    let mut report = Report::default();
    for name in names {
        report.kernel(program, name, &entries[name], &consts);
    }
    report.diagnostics
}

impl Report {
    fn kernel(
        &mut self,
        program: &HirProgram,
        name: &str,
        entry: &AccelEntry,
        consts: &HashMap<String, HirExpr>,
    ) {
        let shape = &entry.shape;
        if shape.index_var.is_empty() || crate::analysis::accel::is_cooperative_shape(shape) {
            return;
        }
        let Some(callable) = transaction(program, name) else {
            return;
        };
        let kernel = super::accel::kernel_statements(program, callable, &shape.index_var);
        let mut locals: HashMap<String, HirExpr> = HashMap::new();
        collect_locals_into(&kernel, &mut locals);
        let mut loads: Vec<(String, HirExpr)> = Vec::new();
        collect_array_loads(&kernel, &mut loads);
        let mut unproven: BTreeMap<String, &'static str> = BTreeMap::new();
        for (field, idx) in &loads {
            let elems = match stride_coeff(idx, &kernel, &shape.index_var, consts, &locals) {
                Coeff::Stride(e) => e,
                Coeff::Unknown(why) => {
                    unproven.entry(field.clone()).or_insert(why);
                    continue;
                }
                Coeff::Constant(_) => continue,
            };
            if elems.abs() < 2 || !self.reported.insert((name.to_string(), field.clone())) {
                continue;
            }
            self.diagnostics
                .push(stride_warning(name, field, elems, callable.span));
        }
        for (field, why) in &unproven {
            self.diagnostics
                .push(unproven_note(name, field, why, callable.span));
        }
    }
}

fn transaction<'a>(program: &'a HirProgram, name: &str) -> Option<&'a HirCallable> {
    program
        .callables
        .iter()
        .find(|c| c.name == name && c.kind == HirCallableKind::Transaction)
}

fn stride_warning(
    name: &str,
    field: &str,
    elems: i64,
    span: Option<crate::errors::Span>,
) -> Diagnostic {
    let mut d = Diagnostic::new(
        "G001",
        Severity::Warning,
        &format!(
            "kernel '{name}' reads '{field}' with a cross-thread stride of {elems} element{}",
            if elems.abs() == 1 { "" } else { "s" }
        ),
    );
    if let Some(s) = span {
        d = d.with_span(s);
    }
    d.with_explanation(&format!(
        "consecutive work items index '{field}' {elems} elements apart, so each 32-byte device memory transaction carries {elems} used bytes when the hardware can coalesce 8"
    ))
    .with_proof_step(&format!(
        "at a fixed inner-loop position the load index reduces to a linear form advancing {elems} elements per unit of the work-item counter"
    ))
    .with_hint(&format!(
        "store '{field}' so consecutive work items read consecutive elements (transpose the storage order for this kernel's mapping)"
    ))
    .with_note("layout is part of the program's host contract; the compiler reports, the author decides")
}

fn unproven_note(
    name: &str,
    field: &str,
    why: &'static str,
    span: Option<crate::errors::Span>,
) -> Diagnostic {
    let mut d = Diagnostic::new(
        "G002",
        Severity::Info,
        &format!("coalescing of field '{field}' in kernel '{name}' unproven"),
    );
    if let Some(s) = span {
        d = d.with_span(s);
    }
    d.with_explanation(&format!(
        "the load index is not provably affine in the work-item counter: {why}"
    ))
    .with_note(
        "the compiler can neither confirm coalescing nor prove a defect here; this is the analysis limit, not a verified-clean bill",
    )
}

/// Module consts (literal Int/Float only) — mirrors the AST const table the
/// coalescing consumer receives.
fn module_consts(program: &HirProgram) -> HashMap<String, HirExpr> {
    program
        .const_exprs
        .iter()
        .filter(|(_, e)| {
            matches!(
                e.kind,
                HirExprKind::Literal(HirLiteral::Int(_) | HirLiteral::Float(_))
            )
        })
        .cloned()
        .collect()
}

fn collect_locals_into(stmts: &[HirStmt], locals: &mut HashMap<String, HirExpr>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Let {
                name,
                expr: Some(e),
                ..
            } => {
                locals.insert(name.clone(), e.clone());
            }
            HirStmtKind::Assign(lhs, rhs) => {
                if let HirExprKind::Identifier(n) = &lhs.kind {
                    locals.insert(n.clone(), rhs.clone());
                }
            }
            HirStmtKind::Foreach { body, .. } => collect_locals_into(body, locals),
            _ => {}
        }
    }
}

fn collect_array_loads(stmts: &[HirStmt], out: &mut Vec<(String, HirExpr)>) {
    for s in stmts {
        match &s.kind {
            HirStmtKind::Assign(_, rhs) => walk_loads(rhs, out),
            HirStmtKind::Let { expr: Some(e), .. } => walk_loads(e, out),
            HirStmtKind::Foreach { body, .. } => collect_array_loads(body, out),
            _ => {}
        }
    }
}

fn walk_loads(e: &HirExpr, out: &mut Vec<(String, HirExpr)>) {
    match &e.kind {
        HirExprKind::Index(obj, idx) => {
            if let HirExprKind::Identifier(f) = &obj.kind {
                out.push((f.clone(), (**idx).clone()));
            }
            walk_loads(idx, out);
        }
        HirExprKind::Binary { left, right, .. } => {
            walk_loads(left, out);
            walk_loads(right, out);
        }
        HirExprKind::Unary { inner, .. } => walk_loads(inner, out),
        HirExprKind::Call { args, .. } => {
            for a in args {
                walk_loads(a, out);
            }
        }
        HirExprKind::Cast { inner, .. } => walk_loads(inner, out),
        _ => {}
    }
}

/// The loop variables held at a fixed position: the kernel's inner foreach
/// items. The shape's own loop_var field IS the outer counter.
fn stride_coeff(
    idx: &HirExpr,
    kernel: &[HirStmt],
    index_var: &str,
    consts: &HashMap<String, HirExpr>,
    locals: &HashMap<String, HirExpr>,
) -> Coeff {
    let mut inner_items: Vec<String> = Vec::new();
    find_items(kernel, &mut inner_items);
    let mut env = CoeffEnv {
        item: index_var.to_string(),
        held_at_zero: inner_items,
        consts,
        locals,
        depth: 0,
    };
    env.coeff(idx)
}

fn find_items(stmts: &[HirStmt], out: &mut Vec<String>) {
    for s in stmts {
        if let HirStmtKind::Foreach { item, body, .. } = &s.kind {
            out.push(item.clone());
            find_items(body, out);
        }
    }
}

struct CoeffEnv<'a> {
    item: String,
    held_at_zero: Vec<String>,
    consts: &'a HashMap<String, HirExpr>,
    locals: &'a HashMap<String, HirExpr>,
    depth: u32,
}

impl CoeffEnv<'_> {
    fn const_value(&self, n: &str) -> Option<i64> {
        self.consts
            .get(n)
            .and_then(|x| eval_const_expr(x, self.consts, self.locals, 0))
            .or_else(|| {
                self.locals
                    .get(n)
                    .and_then(|x| eval_const_expr(x, self.consts, self.locals, 0))
            })
    }

    fn coeff(&mut self, e: &HirExpr) -> Coeff {
        self.depth += 1;
        if self.depth > 16 {
            self.depth -= 1;
            return Coeff::Unknown("analysis depth limit");
        }
        let out = self.coeff_inner(e);
        self.depth -= 1;
        out
    }

    fn coeff_inner(&mut self, e: &HirExpr) -> Coeff {
        match &e.kind {
            HirExprKind::Literal(HirLiteral::Int(v)) => Coeff::Constant(*v),
            HirExprKind::Identifier(n) => self.identifier(n),
            HirExprKind::Binary { op, left, right } => self.binary(*op, left, right),
            HirExprKind::Unary { inner, .. } => self.coeff(inner),
            HirExprKind::Call { .. } => Coeff::Unknown("intrinsic call result"),
            HirExprKind::Index(_, _) => Coeff::Unknown("nested buffer indexing"),
            _ => Coeff::Unknown("unmodeled expression form"),
        }
    }

    fn identifier(&mut self, n: &str) -> Coeff {
        if n == self.item {
            return Coeff::Stride(1);
        }
        if self.held_at_zero.iter().any(|h| h == n) {
            return Coeff::Constant(0);
        }
        if let Some(v) = self.const_value(n) {
            return Coeff::Constant(v);
        }
        if let Some(def) = self.locals.get(n).cloned() {
            if self.depth < 12 {
                return self.coeff(&def);
            }
            return Coeff::Unknown("local inlining depth limit");
        }
        Coeff::Constant(0)
    }

    fn binary(&mut self, op: crate::ast::BinaryOpKind, left: &HirExpr, right: &HirExpr) -> Coeff {
        use crate::ast::BinaryOpKind::{Add, Div, Mod, Mul, Sub};
        let lc = self.coeff(left);
        let rc = self.coeff(right);
        if let Coeff::Unknown(w) = lc {
            return Coeff::Unknown(w);
        }
        if let Coeff::Unknown(w) = rc {
            return Coeff::Unknown(w);
        }
        match (op, &lc, &rc) {
            (Add, a, b) => add_coeff(a, b),
            (Sub, a, b) => sub_coeff(a, b),
            (Mul, a, b) => mul_coeff(a, b),
            (Div, _, _) => Coeff::Constant(0),
            (Mod, l, _) => l.clone(),
            _ => Coeff::Unknown("unsupported index operator"),
        }
    }
}

fn add_coeff(a: &Coeff, b: &Coeff) -> Coeff {
    match (a, b) {
        (Coeff::Stride(x), Coeff::Stride(y)) => Coeff::Stride(x + y),
        (Coeff::Stride(x), Coeff::Constant(c)) => Coeff::Stride(x + c),
        (Coeff::Constant(c), Coeff::Stride(x)) => Coeff::Stride(x + c),
        (Coeff::Constant(x), Coeff::Constant(y)) => Coeff::Constant(x + y),
        _ => Coeff::Constant(0),
    }
}

fn sub_coeff(a: &Coeff, b: &Coeff) -> Coeff {
    match (a, b) {
        (Coeff::Stride(x), Coeff::Stride(y)) => Coeff::Stride(x - y),
        (Coeff::Stride(x), Coeff::Constant(c)) => Coeff::Stride(x - c),
        (Coeff::Constant(c), Coeff::Stride(y)) => Coeff::Stride(c - y),
        (Coeff::Constant(x), Coeff::Constant(y)) => Coeff::Constant(x - y),
        _ => Coeff::Constant(0),
    }
}

fn mul_coeff(a: &Coeff, b: &Coeff) -> Coeff {
    match (a, b) {
        (Coeff::Stride(x), Coeff::Constant(c)) => Coeff::Stride(x * c),
        (Coeff::Constant(c), Coeff::Stride(x)) => Coeff::Stride(x * c),
        (Coeff::Constant(x), Coeff::Constant(y)) => Coeff::Constant(x * y),
        _ => Coeff::Constant(0),
    }
}

fn eval_const_expr(
    e: &HirExpr,
    consts: &HashMap<String, HirExpr>,
    locals: &HashMap<String, HirExpr>,
    depth: u32,
) -> Option<i64> {
    if depth > 8 {
        return None;
    }
    match &e.kind {
        HirExprKind::Literal(HirLiteral::Int(v)) => Some(*v),
        HirExprKind::Identifier(n) => consts
            .get(n)
            .and_then(|x| eval_const_expr(x, consts, locals, depth + 1))
            .or_else(|| {
                locals
                    .get(n)
                    .and_then(|x| eval_const_expr(x, consts, locals, depth + 1))
            }),
        HirExprKind::Binary { op, left, right } => {
            let a = eval_const_expr(left, consts, locals, depth + 1)?;
            let b = eval_const_expr(right, consts, locals, depth + 1)?;
            use crate::ast::BinaryOpKind::{Add, Div, Mul, Sub};
            match op {
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

/// Test-only HIR dump: analyze, then canonicalise diagnostics.
#[cfg(test)]
pub(crate) fn diagnostics_for_test(
    program: &HirProgram,
    entries: &HashMap<String, AccelEntry>,
) -> std::collections::BTreeMap<String, String> {
    crate::analysis::coalescing::canonical_diagnostics(&analyze(program, entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::accel::AccelMode;
    use crate::ast::TopLevel;
    use crate::type_universe::TypeUniverse;
    use std::collections::BTreeMap;

    fn parse_lower(src: &str) -> (Vec<TopLevel>, HirProgram) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (items, program)
    }

    fn parity(src: &str) -> BTreeMap<String, String> {
        let (items, program) = parse_lower(src);
        let universe = TypeUniverse::new();
        let ast_accel =
            crate::analysis::accel::analyze(&items, Some(&universe), Some(AccelMode::TryAll));
        let hir_accel =
            crate::hir::accel::analyze(&program, Some(&universe), Some(AccelMode::TryAll));
        let ast_consts = crate::backend::ptx::module_expr_consts(&items);
        let ast = crate::analysis::coalescing::analyze_coalescing(&items, &ast_accel, &ast_consts);
        let ast = crate::analysis::coalescing::canonical_diagnostics(&ast);
        let hir = diagnostics_for_test(&program, &hir_accel);
        assert_eq!(hir, ast, "HIR/AST coalescing drift:\n{src}");
        ast
    }

    const QK_SRC: &str = r#"
const D: Int = 128;
const NKV: Int = 4096;
let res: Float[64];
let q: Float[262144];
let k: Float[8192];
let t: Int = 0;
accel node qload [t < 64][t == 64] {
    res[t] = q[(t / NKV) * D];
    t = t + 1;
    term;
};
accel node kload [t < 64][t == 64] {
    res[t] = k[t * D];
    t = t + 1;
    term;
};
"#;

    #[test]
    fn qk_k_load_is_proven_scattered() {
        let dump = parity(QK_SRC);
        assert!(
            dump.values()
                .any(|d| d.contains("cross-thread stride of 128")),
            "expected stride-128 warning: {dump:?}"
        );
    }

    #[test]
    fn qk_q_load_is_broadcast() {
        let dump = parity(QK_SRC);
        assert!(
            !dump.values().any(|d| d.contains("reads 'q'")),
            "div-protected q load must not warn: {dump:?}"
        );
    }

    #[test]
    fn nested_index_load_is_reported_unproven() {
        let src = r#"
let res: Float[64];
let k: Float[8192];
let ii: Int[64];
let t: Int = 0;
accel node gather [t < 64][t == 64] {
    res[t] = k[ii[t]];
    t = t + 1;
    term;
};
"#;
        let dump = parity(src);
        assert!(
            dump.values()
                .any(|d| d.contains("not provably affine") && d.contains("nested buffer indexing")),
            "expected G002 nested-index limit: {dump:?}"
        );
    }

    #[test]
    fn empty_program_is_clean() {
        assert!(parity("").is_empty());
    }
}
