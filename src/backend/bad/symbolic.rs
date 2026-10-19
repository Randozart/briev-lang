// ── .bad symbolic regions — assembly+ lowering (2026-10-19) ───────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// Symbolic labels and callable `fn` bodies lower together through ONE
// synthetic `.bld` program, so `fn`s can call each other and labels can
// call `fn`s with the existing BILLD signature/call machinery:
//
//   .bad symbolic bodies
//     → one synthetic `.bld` defn per unit
//     → BILLD Lowerer → BadProgram with virtuals
//     → M4 allocator (loops, ABI params, frames)
//     → bodies extracted and spliced back
//
// Raw physical-only labels never enter this pass. A symbolic body cannot
// mix raw instructions with operands; operandless control/state ops are
// allowed. Symbolic `defn` (inline) bodies are a follow-up; use `fn` for
// callable functions.

use super::registry::{BadIsa, BadRegisters};
use crate::ast::bad::*;
use crate::backend::bld::registry::BldIntrinsics;
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
}

const PREFIX: &str = "__bad_sym_";

const RAW_ZERO: &[&str] = &[
    "ret", "halt", "nop", "cli", "sti", "wfi", "fence", "tlbflush", "mret",
];

const SYMBOLIC_OPS: &[&str] = &[
    "mov", "add", "sub", "mul", "div", "mod", "and", "or", "xor", "shl", "shr",
    "store", "load",
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
    params: Vec<BadFnParam>,
    ret: Option<String>,
    body: Vec<BadBodyItem>,
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
                        "fn `{}` (line {}) returns a value but has no `return` - add \
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
            _ => {}
        }
    }
    if units.is_empty() {
        return Ok(());
    }

    let mut src = String::new();
    for (cname, value) in ctx.const_values {
        src.push_str(&format!("const {cname} = {value};\n"));
    }
    for u in &units {
        let params: HashSet<String> = u.params.iter().map(|p| p.name.clone()).collect();
        match u.kind {
            Kind::Label => src.push_str(&format!("defn {}() {{\n", u.synth)),
            Kind::Fn => {
                let ps: Vec<String> = u
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, p.ty))
                    .collect();
                let ret = u.ret.as_ref().map(|r| format!(" -> {r}")).unwrap_or_default();
                src.push_str(&format!("defn {}({}){ret} {{\n", u.synth, ps.join(", ")));
            }
        }
        let mut b = Builder {
            ctx,
            src: &mut src,
            bound: params.clone(),
            params,
            is_fn: u.kind == Kind::Fn,
        };
        b.emit_body(&u.body)?;
        if u.kind == Kind::Fn && u.ret.is_none() {
            src.push_str("return;\n");
        }
        src.push_str("}\n");
    }

    let prog = crate::backend::bld::lower_to_bad(&src, ctx.family, None, None)
        .map_err(|e| format!("symbolic lowering failed: {e}"))?;
    let prog = crate::backend::bld::alloc::allocate(prog, ctx.regs, ctx.family)
        .map_err(|e| format!("symbolic allocation failed: {e}"))?;
    let mut bodies: HashMap<String, Vec<BadBodyItem>> = HashMap::new();
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
                    _ => crate::errors::Span::dummy(),
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

fn body_has_symbolic(body: &[BadBodyItem], ctx: &SymCtx<'_>) -> bool {
    body.iter().any(|item| match item {
        BadBodyItem::Let(_)
        | BadBodyItem::Assign(_)
        | BadBodyItem::When(_)
        | BadBodyItem::Loop(_)
        | BadBodyItem::While(_)
        | BadBodyItem::Break(_)
        | BadBodyItem::Continue(_)
        | BadBodyItem::CallStmt { .. }
        | BadBodyItem::Return { .. } => true,
        BadBodyItem::Instr(i) => instr_has_symbolic(i, ctx),
        BadBodyItem::Local(_) | BadBodyItem::Site(_) => false,
    })
}

fn body_has_return(body: &[BadBodyItem]) -> bool {
    body.iter().any(|item| match item {
        BadBodyItem::Return { .. } => true,
        BadBodyItem::When(x) => body_has_return(&x.then),
        BadBodyItem::Loop(x) => body_has_return(&x.body),
        BadBodyItem::While(x) => body_has_return(&x.body),
        _ => false,
    })
}

fn instr_has_symbolic(instr: &BadInstr, ctx: &SymCtx<'_>) -> bool {
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
    src: &'a mut String,
    bound: HashSet<String>,
    params: HashSet<String>,
    is_fn: bool,
}

impl Builder<'_, '_> {
    fn emit_body(&mut self, body: &[BadBodyItem]) -> Result<(), String> {
        for item in body {
            self.emit_item(item)?;
        }
        Ok(())
    }

    fn known(&self, name: &str) -> bool {
        self.params.contains(name) || is_known_value(name, self.ctx)
    }

    fn emit_item(&mut self, item: &BadBodyItem) -> Result<(), String> {
        match item {
            BadBodyItem::Let(x) => {
                self.check_new_name(&x.name, x.span.line)?;
                let ty = x.ty.as_ref().map(|t| format!(": {t}")).unwrap_or_default();
                let value = normalize_call(&x.value);
                self.src.push_str(&format!("let {}{ty} = {};\n", x.name, value));
                self.bound.insert(x.name.clone());
            }
            BadBodyItem::Assign(x) => {
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
                let value = normalize_call(&x.value);
                if x.op.is_empty() {
                    self.src.push_str(&format!("{} = {};\n", x.name, value));
                } else {
                    self.src.push_str(&format!("{} {}= {};\n", x.name, x.op, value));
                }
            }
            BadBodyItem::When(x) => {
                self.src.push_str(&format!("when {} {{\n", x.cond));
                self.emit_body(&x.then)?;
                self.src.push_str("}\n");
            }
            BadBodyItem::Loop(x) => {
                self.src.push_str("loop {\n");
                self.emit_body(&x.body)?;
                self.src.push_str("}\n");
            }
            BadBodyItem::While(x) => {
                self.src.push_str(&format!("while {} {{\n", x.cond));
                self.emit_body(&x.body)?;
                self.src.push_str("}\n");
            }
            BadBodyItem::Break(_) => self.src.push_str("break;\n"),
            BadBodyItem::Continue(_) => self.src.push_str("continue;\n"),
            BadBodyItem::CallStmt { text, .. } => {
                self.src.push_str(&format!("{};\n", normalize_call(text)));
            }
            BadBodyItem::Return { value, span } => {
                if !self.is_fn {
                    return Err(format!(
                        "`return` (line {}) is only legal inside a `fn` - a label falls \
                         through or uses raw `ret`",
                        span.line
                    ));
                }
                match value {
                    Some(v) => self.src.push_str(&format!("return {};\n", normalize_call(v))),
                    None => self.src.push_str("return;\n"),
                }
            }
            BadBodyItem::Instr(i) => self.emit_instr(i)?,
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
            self.src.push_str(&format!("bad {{ {} }};\n", i.mnemonic));
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
                args.push(self.value(op, i.span.line)?);
            }
            self.src.push_str(&format!("{}({});\n", i.mnemonic, args.join(", ")));
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
                let src = self.value(&i.operands[1], i.span.line)?;
                self.write_dst(&dst, &src, i.span.line)?;
            }
            ("add" | "sub" | "mul" | "div" | "mod" | "and" | "or" | "xor" | "shl"
                | "shr", 3) => {
                let op = match i.mnemonic.as_str() {
                    "add" => "+",
                    "sub" => "-",
                    "mul" => "*",
                    "div" => "/",
                    "mod" => "%",
                    "and" => "&",
                    "or" => "|",
                    "xor" => "^",
                    "shl" => "<<",
                    "shr" => ">>",
                    _ => unreachable!(),
                };
                let dst = self.dest(&i.operands[0], i.span.line)?;
                let a = self.value(&i.operands[1], i.span.line)?;
                let b = self.value(&i.operands[2], i.span.line)?;
                self.write_dst(&dst, &format!("{a} {op} {b}"), i.span.line)?;
            }
            ("store", 2) => {
                let value = self.value(&i.operands[0], i.span.line)?;
                let addr = self.value(&i.operands[1], i.span.line)?;
                self.src.push_str(&format!("Store({value}, {addr});\n"));
            }
            ("load", 2) => {
                let dst = self.dest(&i.operands[0], i.span.line)?;
                let addr = self.value(&i.operands[1], i.span.line)?;
                self.write_dst(&dst, &format!("Load({addr})"), i.span.line)?;
            }
            _ => {
                return Err(format!(
                    "`{}` (line {}) is not supported in a symbolic body yet - use one of \
                     mov/add/sub/mul/div/mod/and/or/xor/shl/shr/store/load, or put the raw \
                     instruction in its own label",
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

    fn value(&self, op: &BadOperand, line: usize) -> Result<String, String> {
        match op {
            BadOperand::Int(n) => Ok(n.to_string()),
            BadOperand::Float(t) => Ok(t.clone()),
            BadOperand::Expr(t) => Ok(normalize_call(t)),
            BadOperand::Name(n) => {
                if self.params.contains(n) {
                    return Ok(n.clone());
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
                    return Err(format!(
                        "label `{n}` (line {line}) used as a value - address expressions are \
                         not supported in symbolic bodies yet; pass the address in a register \
                         or use `addr` in a raw label"
                    ));
                }
                Ok(n.clone())
            }
        }
    }

    fn write_dst(&mut self, dst: &str, value: &str, line: usize) -> Result<(), String> {
        if self.bound.contains(dst) {
            self.src.push_str(&format!("{dst} = {value};\n"));
        } else {
            self.check_new_name(dst, line)?;
            self.src.push_str(&format!("let {dst} = {value};\n"));
            self.bound.insert(dst.to_string());
        }
        Ok(())
    }
}
