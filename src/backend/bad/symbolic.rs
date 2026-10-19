// ── .bad symbolic regions — assembly+ lowering (2026-10-19) ───────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// A label body that uses `let` / assignment / `when` / `loop` / `while` /
// `break` / `continue` (or an instruction with a symbolic value operand)
// is lowered through the existing BILLD expression engine:
//
//   .bad symbolic body
//     → synthetic `.bld` defn (same statements, Store/Load verbs)
//     → BILLD Lowerer → BadProgram with virtuals
//     → M4 allocator → physical instructions
//     → spliced back into the .bad label body
//
// Raw physical-only labels never enter this pass. A symbolic body cannot
// yet mix raw physical instructions or manual labels; those need their
// own label. `defn` symbolic bodies are a follow-up (inline expansion).

use super::registry::{BadIsa, BadRegisters};
use crate::backend::bld::registry::BldIntrinsics;
use crate::ast::bad::*;
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

/// Operandless raw instructions safe to splice into a symbolic body:
/// they can neither name nor clobber a symbolic value register.
const RAW_ZERO: &[&str] = &[
    "ret", "halt", "nop", "cli", "sti", "wfi", "fence", "tlbflush", "mret",
];

pub fn lower_symbolic_labels(
    items: &mut [BadTopLevel], ctx: &SymCtx<'_>,
) -> Result<(), String> {
    let mut serial = 0usize;
    for item in items.iter_mut() {
        match item {
            BadTopLevel::Label(label) => {
                if !body_has_symbolic(&label.body, ctx) {
                    continue;
                }
                let body = lower_label(&label.body, ctx, serial)
                    .map_err(|e| format!("symbolic body `{}` (line {}): {e}", label.name, label.span.line))?;
                label.body = body;
                serial += 1;
            }
            // Symbolic `defn` bodies are a follow-up: defn params are
            // bound only during inline expansion, so this pass cannot
            // resolve them. Raw defns keep working unchanged.
            BadTopLevel::Defn(_) => {}
            _ => {}
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
        | BadBodyItem::Continue(_) => true,
        BadBodyItem::Instr(i) => instr_has_symbolic(i, ctx),
        BadBodyItem::Local(_) | BadBodyItem::Site(_) => false,
    })
}

/// Only these mnemonics can carry symbolic value operands. Everything
/// else (syscall, arg, branches, raw opcodes) belongs to the raw path;
/// treating an unknown name there as a virtual would hijack raw code.
const SYMBOLIC_OPS: &[&str] = &[
    "mov", "add", "sub", "mul", "div", "mod", "and", "or", "xor", "shl", "shr",
    "store", "load",
];

fn instr_has_symbolic(instr: &BadInstr, ctx: &SymCtx<'_>) -> bool {
    if !SYMBOLIC_OPS.contains(&instr.mnemonic.as_str()) {
        return false;
    }
    if ctx.isa.is_sym(&instr.mnemonic) {
        return false;
    }
    instr.operands.iter().any(|op| match op {
        BadOperand::Int(_) | BadOperand::Float(_) => false,
        // Any expression operand is treated as symbolic; the BILLD
        // expression parser decides whether it is constant-only.
        BadOperand::Expr(t) => !expr_is_constant(t, ctx),
        BadOperand::Name(n) => !is_known_value(n, ctx),
    })
}

/// A pure numeric / `.const` expression is raw (immediate), not symbolic.
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

/// Portable-shaped names (`rN`, `fN`, `sp`, `pc`) are known even when the
/// target lacks them — the raw path owns the loud "register missing"
/// diagnostic.
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

fn lower_label(
    body: &[BadBodyItem], ctx: &SymCtx<'_>, serial: usize,
) -> Result<Vec<BadBodyItem>, String> {
    let mut name = format!("{PREFIX}{serial}");
    while ctx.label_names.contains(&name) || ctx.defns.contains(&name) {
        name.push('_');
    }
    let mut src = String::new();
    for (cname, value) in ctx.const_values {
        src.push_str(&format!("const {cname} = {value};\n"));
    }
    src.push_str(&format!("defn {name}() {{\n"));
    let mut b = Builder { ctx, src: &mut src, bound: HashSet::new() };
    b.emit_body(body)?;
    src.push_str("}\n");

    let prog = crate::backend::bld::lower_to_bad(&src, ctx.family, None, None)
        .map_err(|e| format!("symbolic lowering failed: {e}"))?;
    let prog = crate::backend::bld::alloc::allocate(prog, ctx.regs, ctx.family)
        .map_err(|e| format!("symbolic allocation failed: {e}"))?;
    let mut found = None;
    for item in prog.items {
        if let BadTopLevel::Label(l) = item {
            if l.name == name {
                found = Some(l.body);
                break;
            }
        }
    }
    found.ok_or_else(|| format!("internal: synthetic label `{name}` missing after lowering"))
}

struct Builder<'a, 'b> {
    ctx: &'a SymCtx<'b>,
    src: &'a mut String,
    bound: HashSet<String>,
}

impl Builder<'_, '_> {
    fn emit_body(&mut self, body: &[BadBodyItem]) -> Result<(), String> {
        for item in body {
            self.emit_item(item)?;
        }
        Ok(())
    }

    fn emit_item(&mut self, item: &BadBodyItem) -> Result<(), String> {
        match item {
            BadBodyItem::Let(x) => {
                self.check_new_name(&x.name, x.span.line)?;
                let ty = x.ty.as_ref().map(|t| format!(": {t}")).unwrap_or_default();
                self.src.push_str(&format!("let {}{ty} = {};\n", x.name, x.value));
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
                if x.op.is_empty() {
                    self.src.push_str(&format!("{} = {};\n", x.name, x.value));
                } else {
                    self.src.push_str(&format!("{} {}= {};\n", x.name, x.op, x.value));
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
        if is_known_value(name, self.ctx) {
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
        // Operandless control/state instructions are safe raw splices.
        if i.operands.is_empty() && RAW_ZERO.contains(&i.mnemonic.as_str()) {
            self.src.push_str(&format!("bad {{ {} }};\n", i.mnemonic));
            return Ok(());
        }
        // Statement-position engine verbs (BILLD intrinsic table, the
        // transitional shared source of truth with std/bad/engine.bad).
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
                    "value-returning verb `{}` (line {}) cannot stand as a statement - use                      `let x = {}(…)` or `x = {}(…)`",
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
            BadOperand::Expr(t) => Ok(t.clone()),
            BadOperand::Name(n) => {
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
