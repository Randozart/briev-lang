// ── .bad lowerer — defn expansion, exception resolution, register map ──
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-09-21 (bad-dialect plan): lowers a parsed .bad program to target
// assembly text. All target knowledge comes from the two config
// registries; this module only interprets rows (Rule 15 — no type or
// vocabulary knowledge; the ISA table IS the data).
//
// Lowering order per instruction:
//   1. defn call → inline expansion (cycle-guarded, depth-capped)
//   2. attached exception matching the target family → raw emission
//   3. core ISA universal lowering (reg/imm form pick) → substitution
//   4. otherwise → loud what/why/fix error
//
// To undo: delete this module with the rest of src/backend/bad/.

use super::contracts;
use super::registry::{BadIsa, BadRegisters, ImmHandling};
use crate::ast::bad::*;
use std::collections::HashMap;

/// Max defn expansion depth (mutual-recursion guard). 64 matches the
/// `--optimize-budget` spirit: deep expansion is an authoring error.
const MAX_EXPANSION_DEPTH: usize = 64;

pub struct Lowerer<'a> {
    isa: &'a BadIsa,
    regs: &'a BadRegisters,
    family: String,
    defns: HashMap<String, &'a BadDefn>,
    aliases: HashMap<String, String>,
    out: String,
    errors: Vec<String>,
}

/// A resolved operand inside a defn expansion: either a final token
/// (register/label text) or an immediate value (gets the target prefix).
#[derive(Debug, Clone)]
enum Bound {
    Token(String),
    Imm(i64),
}

impl<'a> Lowerer<'a> {
    pub fn new(isa: &'a BadIsa, regs: &'a BadRegisters, family: &str) -> Self {
        Lowerer {
            isa,
            regs,
            family: family.to_string(),
            defns: HashMap::new(),
            aliases: HashMap::new(),
            out: String::new(),
            errors: Vec::new(),
        }
    }

    /// Lower the whole program. First pass collects defns + aliases
    /// (forward references allowed); second pass emits.
    pub fn run(mut self, program: &'a BadProgram) -> Result<String, String> {
        for item in &program.items {
            match item {
                BadTopLevel::Defn(d) => {
                    if self.defns.insert(d.name.clone(), d).is_some() {
                        self.errors.push(format!(
                            "defn `{}` is declared twice - remove one of the declarations",
                            d.name
                        ));
                    }
                }
                BadTopLevel::Alias(a) => {
                    if self.aliases.insert(a.name.clone(), a.register.clone()).is_some() {
                        self.errors.push(format!(
                            "alias `{}` is declared twice - remove one of the declarations",
                            a.name
                        ));
                    }
                }
                _ => {}
            }
        }

        for item in &program.items {
            match item {
                BadTopLevel::Directive(d) => self.emit_directive(d),
                BadTopLevel::Data(d) => {
                    contracts::check_data_label(d, &mut self.errors);
                    self.push_line(&format!("{}: {} {}", d.name, d.directive.name, d.directive.args));
                }
                BadTopLevel::Alias(_) | BadTopLevel::Defn(_) => {}
                BadTopLevel::Label(l) => self.emit_label(l),
            }
        }

        if self.errors.is_empty() {
            Ok(self.out)
        } else {
            Err(self.errors.join("\n"))
        }
    }

    fn emit_directive(&mut self, d: &BadDirective) {
        match d.name.as_str() {
            "section" => self.push_line(&format!(".section {}", d.args)),
            "global" => self.push_line(&format!(".global {}", d.args)),
            _ => self.push_line(&format!("{} {}", d.name, d.args)),
        }
    }

    fn emit_label(&mut self, l: &BadLabel) {
        self.push_line(&format!("{}:", l.name));
        let proven = contracts::check_label_contracts(l, self.regs, &self.family, &mut self.errors);
        for note in &proven {
            self.push_line(&format!("{} {}", self.regs.comment_prefix(&self.family), note));
        }
        for instr in &l.body {
            if let Err(e) = self.emit_instr(instr, &HashMap::new(), 0) {
                self.errors.push(e);
            }
        }
    }

    /// Emit one instruction under `env` (defn param bindings). `depth`
    /// guards defn recursion.
    fn emit_instr(
        &mut self, instr: &BadInstr, env: &HashMap<String, Bound>, depth: usize,
    ) -> Result<(), String> {
        contracts::check_inline_contract(instr, self.regs, &self.family, &mut self.errors);
        // 1. Defn call → inline expansion.
        if !self.isa.is_core(&instr.mnemonic) {
            if let Some(d) = self.defns.get(&instr.mnemonic).copied() {
                return self.expand_defn(d, instr, env, depth);
            }
            return Err(format!(
                "instruction `{}` is unknown at line {} - it is not a core op ({}) \
                 and no defn with that name exists",
                instr.mnemonic,
                instr.span.line,
                self.isa.known_ops().join(", ")
            ));
        }

        // 2. Attached exception matching this target → raw emission.
        if let Some(branch) = instr.exceptions.iter().find(|b| b.target == self.family) {
            for raw in &branch.body {
                self.emit_raw_instr(raw, env);
            }
            return Ok(());
        }
        // A `default =>` row on an inline exception is a category error —
        // the instruction itself IS the default.
        if let Some(d) = instr.exceptions.iter().find(|b| b.is_default) {
            return Err(format!(
                "`default =>` rows belong in branch defns - instruction `{}` at line {} \
                 already lowers universally; write `{} => ...` for a target override",
                instr.mnemonic, instr.span.line, self.family
            ));
        }

        // 3. Universal core lowering.
        let arity = self.isa.arity(&instr.mnemonic).unwrap_or(0);
        if instr.operands.len() != arity {
            return Err(format!(
                "core op `{}` takes {} operand(s), got {} at line {} - fix the call",
                instr.mnemonic,
                arity,
                instr.operands.len(),
                instr.span.line
            ));
        }
        let lowering = match self.isa.lookup(&instr.mnemonic, &self.family) {
            Some(l) => l,
            None => {
                return Err(format!(
                    "core op `{}` has no `{}` lowering - add a row to config/bad-isa.dbvl \
                     or gate the use site with an exception ({} => ...)",
                    instr.mnemonic, self.family, self.family
                ));
            }
        };
        let has_imm = instr.operands.iter().any(|o| matches!(o, BadOperand::Int(_)));
        let template = match (&lowering.imm, has_imm) {
            (ImmHandling::Form(t), true) => t,
            (ImmHandling::Illegal, true) => {
                return Err(format!(
                    "core op `{}` has no immediate form on `{}` - load the value into \
                     a register first (the hardware has no immediate encoding)",
                    instr.mnemonic, self.family
                ));
            }
            _ => &lowering.reg_form,
        };

        let text = self.substitute(template, instr, env, self.isa.is_sym(&instr.mnemonic))?;
        self.emit_mapped(&text, env);
        Ok(())
    }

    /// Expand a defn call: bind params positionally, emit its body/rows.
    fn expand_defn(
        &mut self, d: &'a BadDefn, call: &BadInstr, outer: &HashMap<String, Bound>, depth: usize,
    ) -> Result<(), String> {
        if depth >= MAX_EXPANSION_DEPTH {
            return Err(format!(
                "defn `{}` exceeded the expansion depth of {} - the defn graph is \
                 recursive; break the cycle",
                d.name, MAX_EXPANSION_DEPTH
            ));
        }
        if call.operands.len() != d.params.len() {
            return Err(format!(
                "defn `{}` takes {} param(s), got {} at line {} - fix the call",
                d.name,
                d.params.len(),
                call.operands.len(),
                call.span.line
            ));
        }
        let mut env: HashMap<String, Bound> = outer.clone();
        for (param, op) in d.params.iter().zip(&call.operands) {
            let bound = match op {
                BadOperand::Int(n) => Bound::Imm(*n),
                BadOperand::Name(name) => {
                    // An operand that names an outer param inherits its
                    // binding; anything else is a token resolved at
                    // emission (register, label, or symbol).
                    outer.get(name).cloned().unwrap_or(Bound::Token(name.to_string()))
                }
            };
            env.insert(param.clone(), bound);
        }

        match &d.shape {
            BadDefnShape::Sequence(body) => {
                for instr in body {
                    self.emit_instr(instr, &env, depth + 1)?;
                }
            }
            BadDefnShape::Branch(rows) => {
                let row = rows.iter().find(|r| r.target == self.family)
                    .or_else(|| rows.iter().find(|r| r.is_default));
                let Some(row) = row else {
                    return Err(format!(
                        "defn `{}` has neither a `{} =>` row nor a `default =>` row - \
                         add the missing row",
                        d.name, self.family
                    ));
                };
                // A chosen target row emits raw; the default row re-enters
                // the core pipeline so it validates like ordinary code.
                for instr in &row.body {
                    if row.is_default {
                        self.emit_instr(instr, &env, depth + 1)?;
                    } else {
                        self.emit_raw_instr(instr, &env);
                    }
                }
            }
        }
        Ok(())
    }

    /// Emit a target-owned (raw) instruction: mnemonic verbatim, operands
    /// resolved through the env/register table with boundary replacement
    /// inside composite operand text.
    fn emit_raw_instr(&mut self, instr: &BadInstr, env: &HashMap<String, Bound>) {
        let operands: Vec<String> = instr
            .operands
            .iter()
            .map(|op| self.resolve_raw_operand(op, env, instr.span.line))
            .collect();
        let mut line = instr.mnemonic.clone();
        for op in &operands {
            line.push(' ');
            line.push_str(op);
            line.push(',');
        }
        if line.ends_with(',') {
            line.pop();
        }
        self.push_line(&line);
    }

    /// Resolve one raw-instruction operand: env bindings first, then the
    /// register table, then boundary-replaced composite text.
    fn resolve_raw_operand(
        &mut self, op: &BadOperand, env: &HashMap<String, Bound>, line: usize,
    ) -> String {
        match op {
            BadOperand::Int(n) => format!("{}{}", self.regs.imm_prefix(&self.family), n),
            BadOperand::Name(name) => self.resolve_raw_name(name, env, line),
        }
    }

    fn resolve_raw_name(
        &mut self, name: &str, env: &HashMap<String, Bound>, line: usize,
    ) -> String {
        let imm = self.regs.imm_prefix(&self.family);
        match env.get(name) {
            Some(Bound::Imm(n)) => return format!("{imm}{n}"),
            Some(Bound::Token(t)) => {
                if let Some(tok) = self.resolve_register(t) {
                    return tok.to_string();
                }
                self.check_portable(t, line);
                return self.replace_names(t, env);
            }
            None => {}
        }
        if let Some(tok) = self.resolve_register(name) {
            return tok.to_string();
        }
        // Portable-register SYNTAX check: `r14` on x86_64 must be a loud
        // error, never a silent symbol.
        self.check_portable(name, line);
        // Symbol/label or composite text (e.g. `[sp, #-16]!`).
        self.replace_names(name, env)
    }

    /// Substitute `$N` operand refs in an ISA template. `allow_symbols`
    /// (branch ops) lets the last operand be a verbatim label/symbol;
    /// elsewhere an unresolvable name is a loud error.
    fn substitute(
        &self, template: &str, instr: &BadInstr, env: &HashMap<String, Bound>,
        allow_symbols: bool,
    ) -> Result<String, String> {
        let mut out = String::with_capacity(template.len());
        let mut i = 0;
        while i < template.len() {
            match take_operand_ref(template, i) {
                Some((r, next)) => {
                    out.push_str(&self.operand_text(instr, r, env, allow_symbols)?);
                    i = next;
                }
                None => {
                    out.push(template[i..].chars().next().unwrap());
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    fn operand_text(
        &self, instr: &BadInstr, r: Ref, env: &HashMap<String, Bound>,
        allow_symbols: bool,
    ) -> Result<String, String> {
        if r.n == 0 || r.n > instr.operands.len() {
            return Ok(format!("$<{}>", r.n)); // unreachable: arity checked first
        }
        let imm = self.regs.imm_prefix(&self.family);
        match &instr.operands[r.n - 1] {
            BadOperand::Int(v) => {
                if r.bare {
                    return Ok(v.to_string());
                }
                Ok(format!("{imm}{v}"))
            }
            BadOperand::Name(name) => {
                if let Some(Bound::Imm(v)) = env.get(name) {
                    return Ok(format!("{imm}{v}"));
                }
                let (bound_name, param) = match env.get(name) {
                    Some(Bound::Token(t)) => (t.as_str(), Some(name.as_str())),
                    _ => (name.as_str(), None),
                };
                let req = TokReq {
                    name: bound_name,
                    width: r.width,
                    allow_symbols,
                    instr,
                    param,
                };
                let token = self.token_for(&req)?;
                if r.bare && !imm.is_empty() && token.starts_with(imm) {
                    // A bare ref to an immediate-bound param strips the
                    // prefix; a register token is already prefix-free.
                    return Ok(token[imm.len()..].to_string());
                }
                Ok(token)
            }
        }
    }

    fn token_for(&self, req: &TokReq) -> Result<String, String> {
        let TokReq { name, width, allow_symbols, instr, param } = req;
        let name = *name;
        let width = *width;
        let allow_symbols = *allow_symbols;
        let tok = match width {
            Some(w) => self.regs.resolve_w(name, &self.family, w),
            None => self.resolve_register(name),
        };
        if let Some(t) = tok {
            return Ok(t.to_string());
        }
        if allow_symbols {
            return Ok(name.to_string());
        }
        let bound = param
            .map(|p| format!(" (bound from param `{p}`)"))
            .unwrap_or_default();
        Err(format!(
            "operand `{name}`{bound} in `{mn}` (line {line}) is not a register on \
             `{fam}` and `{mn}` does not take a branch target - use `addr d, {name}` \
             to take an address, or load the value into a register first",
            name = name,
            mn = instr.mnemonic,
            line = instr.span.line,
            fam = self.family
        ))
    }

    /// Emit a fully substituted template line. NO name re-scan here: the
    /// output already holds final tokens, and boundary-replacing them
    /// would corrupt them (`%r8` contains the word `r8`, which would
    /// re-map to r8's own token). Raw-operand text is resolved earlier,
    /// per operand, in emit_raw_instr.
    fn emit_mapped(&mut self, text: &str, _env: &HashMap<String, Bound>) {
        self.push_line(text);
    }

    /// Identifier-boundary replacement of aliases, params, and portable
    /// register names within arbitrary operand text.
    fn replace_names(&self, text: &str, env: &HashMap<String, Bound>) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = find_word_start(rest) {
            let after = &rest[start..];
            let end = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(after.len());
            let word = &after[..end];
            out.push_str(&rest[..start]);
            out.push_str(&self.resolve_word(word, env));
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }

    fn resolve_word(&self, word: &str, env: &HashMap<String, Bound>) -> String {
        match env.get(word) {
            Some(Bound::Imm(v)) => return format!("{}{}", self.regs.imm_prefix(&self.family), v),
            Some(Bound::Token(t)) => {
                if let Some(tok) = self.resolve_register(t) {
                    return tok.to_string();
                }
                return t.clone();
            }
            None => {}
        }
        if let Some(tok) = self.resolve_register(word) {
            return tok.to_string();
        }
        word.to_string()
    }

    /// Resolve a name through aliases then the register table.
    fn resolve_register(&self, name: &str) -> Option<&'a str> {
        let canonical = self.aliases.get(name).map(|s| s.as_str()).unwrap_or(name);
        self.regs.resolve(canonical, &self.family)
    }

    /// Portable register SYNTAX check for raw operands: `r14` on x86_64
    /// must be a loud error, not a silent symbol pass-through.
    fn check_portable(&mut self, name: &str, line: usize) {
        let is_portable_shape = name == "sp" || name == "pc"
            || (name.len() >= 2 && name.starts_with('r')
                && name[1..].chars().all(|c| c.is_ascii_digit()));
        if is_portable_shape && !self.regs.exists(name, &self.family)
            && !self.aliases.contains_key(name)
        {
            let targets = self.regs.known_targets(name).join(", ");
            self.errors.push(format!(
                "register `{}` (line {}) does not exist on `{}` - it maps only to [{}] \
                 on other targets; reduce register pressure or restructure the code",
                name, line, self.family, targets
            ));
        }
    }

    fn push_line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }
}

/// Everything `token_for` needs to resolve one name — keeps the helper
/// at two parameters.
struct TokReq<'a> {
    name: &'a str,
    width: Option<u8>,
    allow_symbols: bool,
    instr: &'a BadInstr,
    /// Set when `name` came from a defn param binding (for diagnostics).
    param: Option<&'a str>,
}

/// A parsed `$N` template reference: operand index, optional width
/// qualifier (`.w8/.w16/.w32`), and the bare flag (`$N!` = no imm prefix).
#[derive(Debug, Clone, Copy)]
struct Ref {
    n: usize,
    width: Option<u8>,
    bare: bool,
}

/// `$N` template ref at byte `i` — `$N`, `$N.w8/.w16/.w32`, `$N!`.
/// Returns (Ref, next byte index).
fn take_operand_ref(s: &str, i: usize) -> Option<(Ref, usize)> {
    let bytes = s.as_bytes();
    if bytes.get(i) != Some(&b'$') {
        return None;
    }
    let mut j = i + 1;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    if j == i + 1 {
        return None;
    }
    let r = Ref { n: s[i + 1..j].parse().unwrap_or(0), width: None, bare: false };
    let (r, j) = take_width_suffix(s, j, r);
    if bytes.get(j) == Some(&b'!') {
        return Some((Ref { bare: true, ..r }, j + 1));
    }
    Some((r, j))
}

/// `.w8` / `.w16` / `.w32` suffix scan.
fn take_width_suffix(s: &str, j: usize, r: Ref) -> (Ref, usize) {
    let bytes = s.as_bytes();
    if bytes.get(j) != Some(&b'.') || bytes.get(j + 1) != Some(&b'w') {
        return (r, j);
    }
    let mut k = j + 2;
    while k < bytes.len() && bytes[k].is_ascii_digit() {
        k += 1;
    }
    match s[j + 2..k].parse::<u8>() {
        Ok(w) if matches!(w, 8 | 16 | 32) => (Ref { width: Some(w), ..r }, k),
        _ => (r, j),
    }
}

/// Start of the next identifier-ish word at or after byte 0 of `s`.
fn find_word_start(s: &str) -> Option<usize> {
    s.char_indices()
        .find(|(_, c)| c.is_alphabetic() || *c == '_' || *c == '.')
        .map(|(i, _)| i)
}