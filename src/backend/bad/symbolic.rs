// ── .bad symbolic regions — assembly+ lowering (2026-10-19) ───────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// Symbolic labels and callable `fn` bodies lower together through ONE
// in-memory BILLD program, so `fn`s call each other and labels call
// `fn`s with the existing signature/call machinery:
//
//   .bad symbolic bodies
//     → BldProgram AST (consts + one Defn per unit)
//     → BILLD Lowerer → BadProgram with virtuals
//     → M4 allocator (loops, ABI params, frames)
//     → bodies extracted and spliced back
//
// No synthetic `.bld` source, no re-parse. Phase 1 keeps arithmetic as
// Let/Assign (paragraph parity) and uses `BldStmt::Instr` for future raw
// symbolic instructions. Raw physical-only labels never enter this pass.

use super::registry::{BadIsa, BadRegisters};
use crate::ast::bad::{BadFn, BadInstr, BadLabel, BadOperand, BadTopLevel};
use crate::ast::bld::{
    BldConst, BldDefn, BldParam, BldProgram, BldStmt, BldTopLevel,
};
use crate::ast::{BinaryOpKind, Expr, Type};
use crate::backend::bld::registry::BldIntrinsics;
use crate::errors::Span;
use crate::parser::bld::parse_bld_expr;
use std::collections::{HashMap, HashSet};

pub struct SymCtx<'a> {
    pub isa: &'a BadIsa,
    pub regs: &'a BadRegisters,
    pub family: &'a str,
    pub aliases: &'a HashMap<String, String>,
    pub consts: &'a HashMap<String, String>,
    pub const_values: &'a [(String, i64)],
    pub label_names: &'a HashSet<String>,
    pub defns: &'a HashSet<String>,
    pub intrinsics: &'a BldIntrinsics,
    /// Inline symbolic definitions (`defn name(params) { … }`), expanded
    /// at each call site.
    pub symdefs: &'a HashMap<String, BadFn>,
    /// Raw `.bad` defns (inline expansion material). Used to keep a raw
    /// arity-matching call on the raw path when a same-named engine verb
    /// exists (e.g. `ReadControlReg 0, r1` vs `ReadControlReg(0)`).
    pub bad_defns: &'a HashMap<String, crate::ast::bad::BadDefn>,
    /// File-scoped pins from `declarations { name: rN; }`.
    pub global_pins: &'a [(String, String)],
}

const PREFIX: &str = "__bad_sym_";

const RAW_ZERO: &[&str] = &[
    "ret", "halt", "nop", "cli", "sti", "wfi", "fence", "tlbflush", "mret",
];

const SYMBOLIC_OPS: &[&str] = &[
    "mov", "add", "sub", "mul", "div", "mod", "and", "or", "xor", "shl", "shr",
    "store", "load", "stb", "sth", "addr", "readcr", "writecr",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Label,
    Fn,
}

struct Unit {
    idx: usize,
    synth: String,
    kind: Kind,
    params: Vec<crate::ast::bad::BadFnParam>,
    ret: Option<String>,
    body: Vec<crate::ast::bad::BadBodyItem>,
}

pub fn lower_symbolic_labels(
    items: &mut [BadTopLevel], ctx: &SymCtx<'_>,
) -> Result<(), String> {
    let mut units: Vec<Unit> = Vec::new();
    let mut serial = 0usize;
    for (idx, item) in items.iter().enumerate() {
        match item {
            BadTopLevel::Label(l) => {
                if !body_has_symbolic(&l.body, ctx) {
                    continue;
                }
                let mut synth = format!("{PREFIX}{serial}");
                while ctx.label_names.contains(&synth) || ctx.defns.contains(&synth) {
                    synth.push('_');
                }
                serial += 1;
                units.push(Unit {
                    idx,
                    synth,
                    kind: Kind::Label,
                    params: Vec::new(),
                    ret: None,
                    body: l.body.clone(),
                });
            }
            BadTopLevel::Fn(f) => {
                if f.ret.is_some() && !body_has_return(&f.body) {
                    return Err(format!(
                        "`call defn {}` (line {}) returns a value but has no `return` - add \
                         `return <expr>;` on the value path",
                        f.name, f.span.line
                    ));
                }
                units.push(Unit {
                    idx,
                    synth: f.name.clone(),
                    kind: Kind::Fn,
                    params: f.params.clone(),
                    ret: f.ret.clone(),
                    body: f.body.clone(),
                });
            }
            BadTopLevel::SymDefn(_) => {}
            _ => {}
        }
    }
    if units.is_empty() {
        return Ok(());
    }

    let mut program_items: Vec<BldTopLevel> = Vec::new();
    for (cname, value) in ctx.const_values {
        program_items.push(BldTopLevel::Const(BldConst {
            name: cname.clone(),
            value: Expr::Decimal(*value),
            span: Span::dummy(),
        }));
    }
    for u in &units {
        let params: HashSet<String> = u.params.iter().map(|p| p.name.clone()).collect();
        let mut body: Vec<BldStmt> = Vec::new();
        for (name, reg) in ctx.global_pins {
            body.push(BldStmt::Bind {
                name: name.clone(),
                register: reg.clone(),
                span: Span::dummy(),
            });
        }
        let mut bound = params.clone();
        for (name, _) in ctx.global_pins {
            bound.insert(name.clone());
        }
        let mut b = Builder {
            ctx,
            stmts: &mut body,
            bound,
            params,
            is_fn: u.kind == Kind::Fn,
            depth: 0,
        };
        b.emit_body(&u.body)?;
        if u.kind == Kind::Fn && u.ret.is_none() {
            body.push(BldStmt::Return {
                value: None,
                span: Span::dummy(),
            });
        }
        let bparams = u
            .params
            .iter()
            .map(|p| BldParam {
                name: p.name.clone(),
                ty: Some(parse_type(&p.ty)),
                span: Span::dummy(),
            })
            .collect();
        program_items.push(BldTopLevel::Defn(BldDefn {
            name: u.synth.clone(),
            params: bparams,
            ret: u.ret.as_deref().map(parse_type),
            body,
            span: Span::dummy(),
        }));
    }

    let mut lowerer = crate::backend::bld::lower::BldLowerer::new(
        ctx.isa,
        ctx.regs,
        ctx.family,
        None,
        None,
    );
    lowerer.load_program(BldProgram {
        items: program_items,
        span: Span::dummy(),
    })?;
    let prog = lowerer.compile()?;
    let prog = crate::backend::bld::alloc::allocate(prog, ctx.regs, ctx.family)
        .map_err(|e| format!("symbolic allocation failed: {e}"))?;
    let mut bodies: HashMap<String, Vec<crate::ast::bad::BadBodyItem>> = HashMap::new();
    for item in prog.items {
        if let BadTopLevel::Label(l) = item {
            bodies.insert(l.name, l.body);
        }
    }

    for u in units {
        let body = bodies.remove(&u.synth).ok_or_else(|| {
            format!(
                "internal: synthetic label `{}` missing after symbolic lowering",
                u.synth
            )
        })?;
        match u.kind {
            Kind::Label => {
                if let BadTopLevel::Label(l) = &mut items[u.idx] {
                    l.body = body;
                    l.symbolic = true;
                }
            }
            Kind::Fn => {
                let span = match &items[u.idx] {
                    BadTopLevel::Fn(f) => f.span.clone(),
                    _ => Span::dummy(),
                };
                items[u.idx] = BadTopLevel::Label(BadLabel {
                    name: u.synth,
                    local: false,
                    contracts: Vec::new(),
                    body,
                    reserved_regs: Vec::new(),
                    symbolic: true,
                    span,
                });
            }
        }
    }
    Ok(())
}

fn parse_type(name: &str) -> Type {
    match name.trim() {
        "Int" => Type::Custom("Int".to_string()),
        "Float" => Type::Custom("Float".to_string()),
        "Bool" => Type::Custom("Bool".to_string()),
        "Ptr" => Type::Ptr(Box::new(Type::Custom("UInt8".to_string()))),
        other => Type::Custom(other.to_string()),
    }
}

fn body_has_symbolic(body: &[crate::ast::bad::BadBodyItem], ctx: &SymCtx<'_>) -> bool {
    use crate::ast::bad::BadBodyItem;
    body.iter().any(|item| match item {
        BadBodyItem::Let(_)
        | BadBodyItem::Assign(_)
        | BadBodyItem::When(_)
        | BadBodyItem::Loop(_)
        | BadBodyItem::While(_)
        | BadBodyItem::Break(_)
        | BadBodyItem::Continue(_)
        | BadBodyItem::CallStmt { .. }
        | BadBodyItem::Return { .. }
        | BadBodyItem::RawRegion { .. } => true,
        BadBodyItem::Instr(i) => instr_has_symbolic(i, ctx),
        BadBodyItem::Local(_) | BadBodyItem::Site(_) => false,
    })
}

fn body_has_return(body: &[crate::ast::bad::BadBodyItem]) -> bool {
    use crate::ast::bad::BadBodyItem;
    body.iter().any(|item| match item {
        BadBodyItem::Return { .. } => true,
        BadBodyItem::When(x) => body_has_return(&x.then),
        BadBodyItem::Loop(x) => body_has_return(&x.body),
        BadBodyItem::While(x) => body_has_return(&x.body),
        _ => false,
    })
}

fn instr_has_symbolic(instr: &BadInstr, ctx: &SymCtx<'_>) -> bool {
    // A raw defn call with matching arity stays raw, even when an engine
    // verb shares the name (`ReadControlReg n, dst` vs `ReadControlReg(n)`).
    if let Some(d) = ctx.bad_defns.get(&instr.mnemonic) {
        let arity = match &d.shape {
            crate::ast::bad::BadDefnShape::Sequence(_) => d.params.len(),
            crate::ast::bad::BadDefnShape::Branch(rows) => rows.first().map(|r| r.body.len()).unwrap_or(0),
        };
        if arity == instr.operands.len() {
            return false;
        }
    }
    // Engine verbs are symbolic statements even with zero operands
    // (`Park`, `DisableInterrupts`).
    if ctx.intrinsics.is_intrinsic(&instr.mnemonic) {
        return true;
    }
    if !SYMBOLIC_OPS.contains(&instr.mnemonic.as_str()) {
        return false;
    }
    if ctx.isa.is_sym(&instr.mnemonic) {
        return false;
    }
    instr.operands.iter().any(|op| match op {
        BadOperand::Int(_) | BadOperand::Float(_) => false,
        BadOperand::Expr(t) => !expr_is_constant(t, ctx),
        BadOperand::Name(n) => !is_known_value(n, ctx),
    })
}

fn expr_is_constant(text: &str, ctx: &SymCtx<'_>) -> bool {
    for tok in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')) {
        if tok.is_empty() {
            continue;
        }
        if tok.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            continue;
        }
        if ctx.consts.contains_key(tok) {
            continue;
        }
        return false;
    }
    true
}

fn is_portable_name(name: &str) -> bool {
    if name == "sp" || name == "pc" {
        return true;
    }
    let Some(rest) = name.strip_prefix('r').or_else(|| name.strip_prefix('f')) else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn is_known_value(name: &str, ctx: &SymCtx<'_>) -> bool {
    is_portable_name(name)
        || ctx.regs.resolve(name, ctx.family).is_some()
        || ctx.regs.resolve_real(name, ctx.family).is_some()
        || !ctx.regs.alias_targets(name).is_empty()
        || ctx.aliases.contains_key(name)
        || ctx.consts.contains_key(name)
        || ctx.label_names.contains(name)
        || ctx.defns.contains(name)
}

fn normalize_call(text: &str) -> String {
    text.trim()
        .strip_prefix("call ")
        .unwrap_or(text.trim())
        .trim()
        .to_string()
}

struct Builder<'a, 'b> {
    ctx: &'a SymCtx<'b>,
    stmts: &'a mut Vec<BldStmt>,
    bound: HashSet<String>,
    params: HashSet<String>,
    is_fn: bool,
    depth: usize,
}

impl Builder<'_, '_> {
    fn emit_body(&mut self, body: &[crate::ast::bad::BadBodyItem]) -> Result<(), String> {
        for item in body {
            self.emit_item(item)?;
        }
        Ok(())
    }

    fn known(&self, name: &str) -> bool {
        self.params.contains(name) || is_known_value(name, self.ctx)
    }

    fn is_phys_reg(&self, name: &str) -> bool {
        is_portable_name(name)
            || self.ctx.regs.resolve(name, self.ctx.family).is_some()
            || self.ctx.regs.resolve_real(name, self.ctx.family).is_some()
            || !self.ctx.regs.alias_targets(name).is_empty()
    }

    fn parse_expr(&self, text: &str, line: usize) -> Result<Expr, String> {
        let text = normalize_call(text);
        let expr = parse_bld_expr(&text)
            .map_err(|e| format!("line {line}: expression `{text}`: {e}"))?;
        Ok(rewrite_labels(expr, self.ctx))
    }

    fn operand_expr(&self, op: &BadOperand, line: usize) -> Result<Expr, String> {
        match op {
            BadOperand::Int(n) => Ok(Expr::Decimal(*n)),
            BadOperand::Float(t) => self.parse_expr(t, line),
            BadOperand::Expr(t) => self.parse_expr(t, line),
            BadOperand::Name(n) => {
                if self.params.contains(n) {
                    return Ok(Expr::Identifier(n.clone()));
                }
                if self.ctx.regs.resolve(n, self.ctx.family).is_some()
                    || self.ctx.regs.resolve_real(n, self.ctx.family).is_some()
                {
                    return Err(format!(
                        "physical register `{n}` (line {line}) in a symbolic expression - \
                         copy it into a symbolic value first, or keep the instruction raw"
                    ));
                }
                if self.ctx.label_names.contains(n) {
                    return Ok(Expr::Call(
                        "addr".to_string(),
                        vec![Expr::Identifier(n.clone())],
                        None,
                    ));
                }
                Ok(Expr::Identifier(n.clone()))
            }
        }
    }

    /// A call statement: inline symbolic `defn` expands here, everything
    /// else becomes a real `call` (callable `defn`, engine verb, or fn).
    fn emit_call(
        &mut self, callee: String, args: Vec<Expr>, span: Span,
    ) -> Result<(), String> {
        if let Some(d) = self.ctx.symdefs.get(&callee).cloned() {
            if args.len() != d.params.len() {
                return Err(format!(
                    "inline defn `{}` (line {}) takes {} argument(s), got {}",
                    callee,
                    span.line,
                    d.params.len(),
                    args.len()
                ));
            }
            if self.depth >= 64 {
                return Err(format!(
                    "inline defn expansion exceeds 64 frames at `{callee}` (line {}) -                      use `call defn` for recursion",
                    span.line
                ));
            }
            let mut body = Vec::new();
            let mut inner_bound = HashSet::new();
            let mut inner_params = HashSet::new();
            for (name, _) in self.ctx.global_pins {
                inner_bound.insert(name.clone());
            }
            for (p, a) in d.params.iter().zip(args) {
                body.push(BldStmt::Let {
                    name: p.name.clone(),
                    ty: Some(parse_type(&p.ty)),
                    value: a,
                    span: span.clone(),
                });
                inner_bound.insert(p.name.clone());
                inner_params.insert(p.name.clone());
            }
            {
                let mut inner = Builder {
                    ctx: self.ctx,
                    stmts: &mut body,
                    bound: inner_bound,
                    params: inner_params,
                    is_fn: false,
                    depth: self.depth + 1,
                };
                inner.emit_body(&d.body)?;
            }
            self.stmts.push(BldStmt::Block { body, span });
            return Ok(());
        }
        self.stmts.push(BldStmt::Call {
            callee,
            args,
            span,
        });
        Ok(())
    }

    fn emit_item(&mut self, item: &crate::ast::bad::BadBodyItem) -> Result<(), String> {
        use crate::ast::bad::BadBodyItem;
        match item {
            BadBodyItem::Let(x) => {
                self.check_new_name(&x.name, x.span.line)?;
                let raw = x.value.trim().trim_end_matches(';').trim();
                if let Some(reg) = raw.strip_prefix("pin ") {
                    let reg = reg.trim();
                    if !self.is_phys_reg(reg) {
                        return Err(format!(
                            "`pin {reg}` (line {}) needs a physical register",
                            x.span.line
                        ));
                    }
                    self.stmts.push(BldStmt::Bind {
                        name: x.name.clone(),
                        register: reg.to_string(),
                        span: x.span.clone(),
                    });
                    self.bound.insert(x.name.clone());
                    return Ok(());
                }
                let value = if self.is_phys_reg(raw) {
                    Expr::Call(
                        "copyreg".to_string(),
                        vec![Expr::Identifier(raw.to_string())],
                        None,
                    )
                } else {
                    self.parse_expr(&x.value, x.span.line)?
                };
                self.stmts.push(BldStmt::Let {
                    name: x.name.clone(),
                    ty: x.ty.as_deref().map(parse_type),
                    value,
                    span: x.span.clone(),
                });
                self.bound.insert(x.name.clone());
            }
            BadBodyItem::Assign(x) => {
                if self.is_phys_reg(&x.name) {
                    let rhs = self.parse_expr(&x.value, x.span.line)?;
                    let value = if x.op.is_empty() {
                        rhs
                    } else {
                        let kind = compound_kind(&x.op).ok_or_else(|| {
                            format!(
                                "unknown compound operator `{}=` at line {}",
                                x.op, x.span.line
                            )
                        })?;
                        Expr::BinaryOp(
                            kind,
                            Box::new(Expr::Call(
                                "copyreg".to_string(),
                                vec![Expr::Identifier(x.name.clone())],
                                None,
                            )),
                            Box::new(rhs),
                        )
                    };
                    self.stmts.push(BldStmt::WriteReg {
                        register: x.name.clone(),
                        value,
                        span: x.span.clone(),
                    });
                    return Ok(());
                }
                self.check_assign_name(&x.name, x.span.line)?;
                if !self.bound.contains(&x.name) {
                    return Err(format!(
                        "assignment to `{}` (line {}) before it is defined - use `let {} = ...` \
                         or `mov {name}, ...` first",
                        x.name,
                        x.span.line,
                        x.name,
                        name = x.name
                    ));
                }
                let rhs = self.parse_expr(&x.value, x.span.line)?;
                let value = if x.op.is_empty() {
                    rhs
                } else {
                    let kind = compound_kind(&x.op).ok_or_else(|| {
                        format!("unknown compound operator `{}=` at line {}", x.op, x.span.line)
                    })?;
                    Expr::BinaryOp(
                        kind,
                        Box::new(Expr::Identifier(x.name.clone())),
                        Box::new(rhs),
                    )
                };
                self.stmts.push(BldStmt::Assign {
                    target: Expr::Identifier(x.name.clone()),
                    value,
                    span: x.span.clone(),
                });
            }
            BadBodyItem::When(x) => {
                let cond = self.parse_expr(&x.cond, x.span.line)?;
                let mut then = Vec::new();
                {
                    let mut inner = Builder {
                        ctx: self.ctx,
                        stmts: &mut then,
                        bound: self.bound.clone(),
                        params: self.params.clone(),
                        is_fn: self.is_fn,
                        depth: self.depth,
                    };
                    inner.emit_body(&x.then)?;
                }
                self.stmts.push(BldStmt::When {
                    cond,
                    then,
                    otherwise: None,
                    span: x.span.clone(),
                });
            }
            BadBodyItem::Loop(x) => {
                let mut body = Vec::new();
                {
                    let mut inner = Builder {
                        ctx: self.ctx,
                        stmts: &mut body,
                        bound: self.bound.clone(),
                        params: self.params.clone(),
                        is_fn: self.is_fn,
                        depth: self.depth,
                    };
                    inner.emit_body(&x.body)?;
                }
                self.stmts.push(BldStmt::Loop {
                    body,
                    span: x.span.clone(),
                });
            }
            BadBodyItem::While(x) => {
                let cond = self.parse_expr(&x.cond, x.span.line)?;
                let mut body = Vec::new();
                {
                    let mut inner = Builder {
                        ctx: self.ctx,
                        stmts: &mut body,
                        bound: self.bound.clone(),
                        params: self.params.clone(),
                        is_fn: self.is_fn,
                        depth: self.depth,
                    };
                    inner.emit_body(&x.body)?;
                }
                self.stmts.push(BldStmt::While {
                    cond,
                    body,
                    span: x.span.clone(),
                });
            }
            BadBodyItem::Break(span) => self.stmts.push(BldStmt::Break { span: span.clone() }),
            BadBodyItem::Continue(span) => {
                self.stmts.push(BldStmt::Continue { span: span.clone() })
            }
            BadBodyItem::CallStmt { text, span } => {
                let expr = self.parse_expr(text, span.line)?;
                let Expr::Call(callee, args, _) = expr else {
                    return Err(format!(
                        "line {}: `{}` is not a call statement",
                        span.line, text
                    ));
                };
                self.emit_call(callee, args, span.clone())?;
            }
            BadBodyItem::Return { value, span } => {
                if !self.is_fn {
                    return Err(format!(
                        "`return` (line {}) is only legal inside a `call defn` - a label falls \
                         through or uses raw `ret`",
                        span.line
                    ));
                }
                let value = match value {
                    Some(v) => Some(self.parse_expr(v, span.line)?),
                    None => None,
                };
                self.stmts.push(BldStmt::Return {
                    value,
                    span: span.clone(),
                });
            }
            BadBodyItem::Instr(i) => self.emit_instr(i)?,
            BadBodyItem::RawRegion { text, clobbers, span } => {
                self.stmts.push(BldStmt::RawClobber {
                    text: text.clone(),
                    clobbers: clobbers.clone(),
                    span: span.clone(),
                });
            }
            BadBodyItem::Local(l) => {
                return Err(format!(
                    "manual label `{}` inside a symbolic body (line {}) - symbolic bodies use \
                     structured control; put the raw label in its own label",
                    l.name, l.span.line
                ))
            }
            BadBodyItem::Site(_) => {
                return Err(
                    "`site` inside a symbolic body - put the target-dispatch block in a raw \
                     label, or use structured control"
                        .to_string(),
                )
            }
        }
        Ok(())
    }

    fn check_new_name(&self, name: &str, line: usize) -> Result<(), String> {
        if self.params.contains(name) {
            return Err(format!(
                "`let {name}` (line {line}) redeclares a parameter - assign it with `{name} = ...`"
            ));
        }
        if self.known(name) {
            return Err(format!(
                "symbolic value `{name}` (line {line}) collides with a register, const, label, \
                 or defn - use a different name"
            ));
        }
        Ok(())
    }

    fn check_assign_name(&self, name: &str, line: usize) -> Result<(), String> {
        if self.ctx.regs.resolve(name, self.ctx.family).is_some()
            || self.ctx.regs.resolve_real(name, self.ctx.family).is_some()
        {
            return Err(format!(
                "assignment to physical register `{name}` (line {line}) in a symbolic body - \
                 assign to a symbolic value, or keep the instruction raw"
            ));
        }
        if self.ctx.consts.contains_key(name)
            || self.ctx.label_names.contains(name)
            || self.ctx.defns.contains(name)
        {
            return Err(format!(
                "assignment to `{name}` (line {line}) - the name is a const, label, or defn, \
                 not a mutable symbolic value"
            ));
        }
        Ok(())
    }

    fn emit_instr(&mut self, i: &BadInstr) -> Result<(), String> {
        if i.operands.is_empty() && RAW_ZERO.contains(&i.mnemonic.as_str()) {
            self.stmts.push(BldStmt::Bad {
                text: i.mnemonic.clone(),
                span: i.span.clone(),
            });
            return Ok(());
        }
        if self.ctx.intrinsics.is_intrinsic(&i.mnemonic) {
            let arity = self.ctx.intrinsics.arity(&i.mnemonic).unwrap_or(0);
            if i.operands.len() != arity {
                return Err(format!(
                    "engine verb `{}` (line {}) takes {arity} argument(s), got {}",
                    i.mnemonic,
                    i.span.line,
                    i.operands.len()
                ));
            }
            if self.ctx.intrinsics.returns(&i.mnemonic) == Some(true) {
                return Err(format!(
                    "value-returning verb `{}` (line {}) cannot stand as a statement - use \
                     `let x = {}(…)` or `x = {}(…)`",
                    i.mnemonic, i.span.line, i.mnemonic, i.mnemonic
                ));
            }
            let mut args = Vec::with_capacity(i.operands.len());
            for op in &i.operands {
                args.push(self.operand_expr(op, i.span.line)?);
            }
            self.stmts.push(BldStmt::Call {
                callee: i.mnemonic.clone(),
                args,
                span: i.span.clone(),
            });
            return Ok(());
        }
        if self.ctx.isa.is_sym(&i.mnemonic) {
            return Err(format!(
                "`{}` (line {}) is a branch/symbol operation in a symbolic body - use \
                 `when`/`loop`/`while`, or move the raw jump into its own label",
                i.mnemonic, i.span.line
            ));
        }
        let arity = i.operands.len();
        match (i.mnemonic.as_str(), arity) {
            ("mov", 2) => {
                let dst = self.dest(&i.operands[0], i.span.line)?;
                let value = self.operand_expr(&i.operands[1], i.span.line)?;
                self.write_dst(&dst, value, i.span.clone())?;
            }
            ("add" | "sub" | "mul" | "div" | "mod" | "and" | "or" | "xor" | "shl"
                | "shr", 3) => {
                let kind = match i.mnemonic.as_str() {
                    "add" => BinaryOpKind::Add,
                    "sub" => BinaryOpKind::Sub,
                    "mul" => BinaryOpKind::Mul,
                    "div" => BinaryOpKind::Div,
                    "mod" => BinaryOpKind::Mod,
                    "and" => BinaryOpKind::BitAnd,
                    "or" => BinaryOpKind::BitOr,
                    "xor" => BinaryOpKind::BitXor,
                    "shl" => BinaryOpKind::Shl,
                    "shr" => BinaryOpKind::Shr,
                    _ => unreachable!(),
                };
                let dst = self.dest(&i.operands[0], i.span.line)?;
                let a = self.operand_expr(&i.operands[1], i.span.line)?;
                let b = self.operand_expr(&i.operands[2], i.span.line)?;
                self.write_dst(
                    &dst,
                    Expr::BinaryOp(kind, Box::new(a), Box::new(b)),
                    i.span.clone(),
                )?;
            }
            ("addr", 2) => {
                let dst = self.dest(&i.operands[0], i.span.line)?;
                let label = match &i.operands[1] {
                    BadOperand::Name(n) => n.clone(),
                    _ => {
                        return Err(format!(
                            "`addr` (line {}) needs a bare label operand",
                            i.span.line
                        ))
                    }
                };
                self.write_dst(
                    &dst,
                    Expr::Call("addr".to_string(), vec![Expr::Identifier(label)], None),
                    i.span.clone(),
                )?;
            }
            ("stb" | "sth", 2) => {
                // Sub-width stores: keep the operand tokens; the ISA row
                // picks reg/imm form and the allocator resolves virtuals.
                // Hex/const expression operands become ordinary immediates.
                let mut ops = i.operands.clone();
                for op in &mut ops {
                    if let BadOperand::Expr(t) = op {
                        match parse_bld_expr(t) {
                            Ok(Expr::Decimal(n)) | Ok(Expr::TaggedLiteral(n, _)) => {
                                *op = BadOperand::Int(n)
                            }
                            Ok(Expr::Float(_)) => {}
                            _ => {
                                return Err(format!(
                                    "`{}` (line {}) takes simple value/address operands - \
                                     assign expressions to a symbolic value first",
                                    i.mnemonic, i.span.line
                                ))
                            }
                        }
                    }
                }
                self.stmts.push(BldStmt::Instr(BadInstr {
                    mnemonic: i.mnemonic.clone(),
                    operands: ops,
                    contract: None,
                    ack: None,
                    span: i.span.clone(),
                }));
            }
            ("store", 2) => {
                let value = self.operand_expr(&i.operands[0], i.span.line)?;
                let addr = self.operand_expr(&i.operands[1], i.span.line)?;
                self.stmts.push(BldStmt::Call {
                    callee: "Store".to_string(),
                    args: vec![value, addr],
                    span: i.span.clone(),
                });
            }
            ("load", 2) => {
                let dst = self.dest(&i.operands[0], i.span.line)?;
                let addr = self.operand_expr(&i.operands[1], i.span.line)?;
                self.write_dst(
                    &dst,
                    Expr::Call("Load".to_string(), vec![addr], None),
                    i.span.clone(),
                )?;
            }
            ("readcr", 2) => {
                let dst = self.dest(&i.operands[0], i.span.line)?;
                let n = self.operand_expr(&i.operands[1], i.span.line)?;
                self.write_dst(
                    &dst,
                    Expr::Call("ReadControlReg".to_string(), vec![n], None),
                    i.span.clone(),
                )?;
            }
            ("writecr", 2) => {
                let n = self.operand_expr(&i.operands[0], i.span.line)?;
                let value = self.operand_expr(&i.operands[1], i.span.line)?;
                self.stmts.push(BldStmt::Call {
                    callee: "WriteControlReg".to_string(),
                    args: vec![n, value],
                    span: i.span.clone(),
                });
            }
            _ => {
                return Err(format!(
                    "`{}` (line {}) is not supported in a symbolic body yet - use one of \
                     mov/add/sub/mul/div/mod/and/or/xor/shl/shr/store/load/addr/readcr/writecr, \
                     or put the raw instruction in its own label",
                    i.mnemonic, i.span.line
                ))
            }
        }
        Ok(())
    }

    fn dest(&self, op: &BadOperand, line: usize) -> Result<String, String> {
        let BadOperand::Name(name) = op else {
            return Err(format!(
                "destination at line {line} must be a symbolic value, not an immediate/expression"
            ));
        };
        if self.ctx.regs.resolve(name, self.ctx.family).is_some()
            || self.ctx.regs.resolve_real(name, self.ctx.family).is_some()
        {
            return Err(format!(
                "physical register destination `{name}` (line {line}) in a symbolic body - \
                 assign to a symbolic value, or keep the instruction raw"
            ));
        }
        if self.ctx.consts.contains_key(name)
            || self.ctx.label_names.contains(name)
            || self.ctx.defns.contains(name)
        {
            return Err(format!(
                "`{name}` (line {line}) is a const/label/defn, not a writable symbolic value"
            ));
        }
        Ok(name.clone())
    }

    fn write_dst(
        &mut self, dst: &str, value: Expr, span: Span,
    ) -> Result<(), String> {
        if self.bound.contains(dst) {
            self.stmts.push(BldStmt::Assign {
                target: Expr::Identifier(dst.to_string()),
                value,
                span,
            });
        } else {
            self.check_new_name(dst, span.line)?;
            self.stmts.push(BldStmt::Let {
                name: dst.to_string(),
                ty: None,
                value,
                span,
            });
            self.bound.insert(dst.to_string());
        }
        Ok(())
    }
}

/// A known data/code label used as a value means its address in a
/// symbolic expression. `addr(label)` keeps its bare argument.
fn rewrite_labels(expr: Expr, ctx: &SymCtx<'_>) -> Expr {
    match expr {
        Expr::Identifier(n) if ctx.label_names.contains(&n) => Expr::Call(
            "addr".to_string(),
            vec![Expr::Identifier(n)],
            None,
        ),
        Expr::Call(callee, args, id) => {
            let args = if callee == "addr" {
                args
            } else {
                args.into_iter().map(|a| rewrite_labels(a, ctx)).collect()
            };
            Expr::Call(callee, args, id)
        }
        Expr::BinaryOp(kind, l, r) => Expr::BinaryOp(
            kind,
            Box::new(rewrite_labels(*l, ctx)),
            Box::new(rewrite_labels(*r, ctx)),
        ),
        Expr::UnaryOp(kind, v) => Expr::UnaryOp(kind, Box::new(rewrite_labels(*v, ctx))),
        Expr::Field(base, name) => Expr::Field(Box::new(rewrite_labels(*base, ctx)), name),
        Expr::MethodCall(base, name, args, id, refs) => Expr::MethodCall(
            Box::new(rewrite_labels(*base, ctx)),
            name,
            args.into_iter().map(|a| rewrite_labels(a, ctx)).collect(),
            id,
            refs,
        ),
        Expr::Index(base, idx) => Expr::Index(
            Box::new(rewrite_labels(*base, ctx)),
            Box::new(rewrite_labels(*idx, ctx)),
        ),
        other => other,
    }
}

fn compound_kind(op: &str) -> Option<BinaryOpKind> {
    Some(match op {
        "+" => BinaryOpKind::Add,
        "-" => BinaryOpKind::Sub,
        "*" => BinaryOpKind::Mul,
        "/" => BinaryOpKind::Div,
        "%" => BinaryOpKind::Mod,
        "&" => BinaryOpKind::BitAnd,
        "|" => BinaryOpKind::BitOr,
        "^" => BinaryOpKind::BitXor,
        "<<" => BinaryOpKind::Shl,
        ">>" => BinaryOpKind::Shr,
        _ => return None,
    })
}
