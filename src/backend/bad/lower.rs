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

use super::comptime;
use super::contracts;
use super::registry::{BadIsa, BadRegisters, ImmHandling};
use crate::ast::bad::*;
use std::collections::HashMap;

/// Max defn expansion depth (mutual-recursion guard). 64 matches the
/// `--optimize-budget` spirit: deep expansion is an authoring error.
const MAX_EXPANSION_DEPTH: usize = 64;

/// The FP literal pool: insertion-ordered entries + a bits index.
#[derive(Default)]
struct FloatPool {
    entries: Vec<(String, String)>,
    index: HashMap<u64, String>,
}

impl FloatPool {
    fn intern(&mut self, text: &str) -> String {
        let bits = text.parse::<f64>().unwrap_or(0.0).to_bits();
        if let Some(label) = self.index.get(&bits) {
            return label.clone();
        }
        let label = format!(".Lfloat_{}", self.entries.len());
        self.entries.push((label.clone(), text.to_string()));
        self.index.insert(bits, label.clone());
        label
    }
}

pub struct Lowerer<'a> {
    isa: &'a BadIsa,
    regs: &'a BadRegisters,
    family: String,
    defns: HashMap<String, BadDefn>,
    aliases: HashMap<String, String>,
    /// `.const NAME expr` raw expressions — evaluated lazily (forward
    /// refs OK), cycle-guarded.
    consts: HashMap<String, String>,
    /// Sequential `.struct` field layout, computed after pass 1.
    structs: Vec<StructField>,
    /// The open `.struct` block, for `.field` context.
    current_struct: Option<String>,
    /// Enclosing global label — scopes local labels (`.loop:`).
    current_label: Option<String>,
    /// Per-defn expansion counters (hygienic local-label gensym).
    defn_calls: HashMap<String, usize>,
    /// The active defn expansion scope, when inside one.
    local_scope: Option<String>,
    /// Global label names — `.export` targets must exist.
    label_names: std::collections::HashSet<String>,
    /// Import resolution root (the importing file's directory).
    base_dir: Option<std::path::PathBuf>,
    /// Canonicalized import paths already expanded (cycle guard).
    visited: std::collections::HashSet<std::path::PathBuf>,
    /// Depth-first flattened items: imported files expand at their
    /// `import` line, so pass 1 / pass 2 see them in source order.
    emit_items: Vec<BadTopLevel>,
    /// FP literal pool: value bits → (label, source text). Registration
    /// happens during substitution (&self path), so the map is a RefCell;
    /// the pool block is appended after pass 2.
    float_pool: std::cell::RefCell<FloatPool>,
    out: String,
    errors: Vec<String>,
    trace: bool,
    /// The friendly alias sheet loads by default; `--raw` skips it.
    friendly: bool,
    /// Names the sheet provided — user redeclarations overwrite these
    /// silently (the sheet is a default), user-vs-user dups still error.
    sheet_aliases: std::collections::HashSet<String>,
    /// 2026-09-21: When compiling a `bad fn` body from `.bv`, param names
    /// are pre-bound to physical registers. Merged into every env lookup.
    param_env: HashMap<String, Bound>,
    /// 2026-09-22: W-tier probable-error notices (never restrictive —
    /// acknowledged with `^`/`^^`/`^^^`, recorded, never silent).
    notices: Vec<crate::backend::bad::notices::Notice>,
}

/// One `.field name, size[, align]` inside a `.struct` block.
struct StructField {
    struct_name: String,
    field: String,
    size_expr: String,
    align_expr: Option<String>,
}

/// A resolved operand inside a defn expansion: either a final token
/// (register/label text) or an immediate value (gets the target prefix).
#[derive(Debug, Clone)]
pub enum Bound {
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
            consts: HashMap::new(),
            structs: Vec::new(),
            current_struct: None,
            current_label: None,
            defn_calls: HashMap::new(),
            local_scope: None,
            label_names: std::collections::HashSet::new(),
            base_dir: None,
            visited: std::collections::HashSet::new(),
            emit_items: Vec::new(),
            float_pool: std::cell::RefCell::new(FloatPool::default()),
            out: String::new(),
            errors: Vec::new(),
            trace: false,
            friendly: true,
            sheet_aliases: std::collections::HashSet::new(),
            param_env: HashMap::new(),
            notices: Vec::new(),
        }
    }

    /// Enable per-instruction lowering traces on stderr.
    pub fn with_trace(mut self, trace: bool) -> Self {
        self.trace = trace;
        self
    }

    /// `--raw`: skip the default friendly alias sheet.
    pub fn with_friendly(mut self, friendly: bool) -> Self {
        self.friendly = friendly;
        self
    }

    /// Root directory for resolving `import` paths.
    pub fn with_base_dir(mut self, dir: Option<std::path::PathBuf>) -> Self {
        self.base_dir = dir;
        self
    }

    /// 2026-09-21: Inject parameter bindings for `bad fn` bodies compiled
    /// from `.bv`. The map is param-name → Bound::Token(register).
    pub fn with_param_env(mut self, env: HashMap<String, Bound>) -> Self {
        self.param_env = env;
        self
    }

    /// Lower the whole program. First pass collects defns + aliases
    /// (forward references allowed); second pass emits.
    pub fn run(&mut self, program: &'a BadProgram) -> Result<String, String> {
        // Depth-first import expansion: imported files clone in at their
        // `import` line, so both passes see source order. Root items are
        // cloned once — a one-shot compile can afford it.
        let root: Vec<BadTopLevel> = program.items.clone();
        if self.friendly {
            self.load_friendly_sheet()?;
        }
        self.expand_imports(root, 0)?;

        // Pass 1: names — defns, aliases, consts, struct fields, labels.
        let pass1 = std::mem::take(&mut self.emit_items);
        for item in &pass1 {
            self.collect_names(item);
        }
        self.emit_items = pass1;
        self.layout_structs();

        // Pass 2: emission (take the vec — borrow split vs self.errors).
        let emit_items = std::mem::take(&mut self.emit_items);
        // 2026-09-22: W-tier analysis — predicted probable errors,
        // acknowledged with `^`/`^^`/`^^^`. Recorded, never silent; they
        // do not block emission.
        self.collect_notices(&emit_items);
        for item in &emit_items {
            match item {
                BadTopLevel::Directive(d) => self.emit_directive(d),
                BadTopLevel::Data(d) => {
                    contracts::check_data_label(d, &mut self.errors);
                    let args = self.resolve_data_args(&d.directive.args);
                    self.push_line(&format!("{}: {} {}", d.name, d.directive.name, args));
                }
                BadTopLevel::Alias(_) | BadTopLevel::Defn(_) => {}
                BadTopLevel::Label(l) => self.emit_label(l),
                // 2026-09-22: raw <target> ... end — verbatim for the
                // active family, skipped otherwise. A named block emits
                // its callable label first.
                BadTopLevel::RawBlock(b) => {
                    if self.family.starts_with(&b.target) {
                        if let Some(name) = &b.name {
                            self.push_line(&format!("{name}:"));
                        }
                        for line in &b.lines {
                            self.push_line(line);
                        }
                    }
                }
            }
        }

        self.flush_float_pool();

        // 2026-09-22: Surface the W-tier notices — warnings are
        // informative, never restrictive. Unacknowledged probable errors
        // print to stderr; acknowledged ones are recorded as info under
        // --trace-lowering (never silent).
        self.surface_notices();
        // A stale ack — a named warning that never fired — is a loud
        // error so markers can't rot.
        self.errors.extend(self.stale_acks(program));

        if self.errors.is_empty() {
            Ok(std::mem::take(&mut self.out))
        } else {
            Err(self.errors.join("\n"))
        }
    }

    /// 2026-09-22: Run the W-tier analysis over every label and defn.
    fn collect_notices(&mut self, items: &[BadTopLevel]) {
        for item in items {
            match item {
                BadTopLevel::Label(l) => {
                    let n = crate::backend::bad::notices::check_label(l, self.regs, &self.family);
                    self.notices.extend(n);
                }
                BadTopLevel::Defn(d) => {
                    let n = crate::backend::bad::notices::check_defn(d);
                    self.notices.extend(n);
                }
                _ => {}
            }
        }
    }

    /// 2026-09-22: Print the W-tier notices. Never silent: acknowledged
    /// ones still surface under --trace-lowering.
    fn surface_notices(&self) {
        for n in &self.notices {
            if n.acknowledged {
                if self.trace {
                    eprintln!("{}", crate::backend::bad::notices::format_acknowledged(n));
                }
            } else {
                eprintln!("{}", crate::backend::bad::notices::format(n));
            }
        }
    }

    /// 2026-09-22: The W-tier notices collected during lowering — the
    /// predicted probable errors, acknowledged and unacknowledged.
    pub fn notices(&self) -> &[crate::backend::bad::notices::Notice] {
        &self.notices
    }

    /// 2026-09-22: Stale acknowledge markers — a named warning that never
    /// fired is a loud error, so markers can't rot. Returns the errors.
    pub fn stale_acks(&self, program: &BadProgram) -> Vec<String> {
        crate::backend::bad::notices::stale_acks(program, &self.notices)
    }

    /// Pass 1 name collection for one item.
    fn collect_names(&mut self, item: &BadTopLevel) {
        match item {
            BadTopLevel::Defn(d) => {
                if self.defns.insert(d.name.clone(), d.clone()).is_some() {
                    self.errors.push(format!(
                        "defn `{}` is declared twice - remove one of the declarations",
                        d.name
                    ));
                }
            }
            BadTopLevel::Alias(a) => {
                // Sheet defaults are silently overridable; user-vs-user
                // duplicates remain a loud error.
                if self.aliases.insert(a.name.clone(), a.register.clone()).is_some()
                    && !self.sheet_aliases.contains(&a.name)
                {
                    self.errors.push(format!(
                        "alias `{}` is declared twice - remove one of the declarations",
                        a.name
                    ));
                }
            }
            BadTopLevel::Directive(d) => self.collect_directive(d),
            BadTopLevel::Label(l) if !l.local => {
                if !self.label_names.insert(l.name.clone()) {
                    self.errors.push(format!(
                        "label `{}` is declared twice - labels share one global \
                         namespace; rename one or use a local label (.name:)",
                        l.name
                    ));
                }
            }
            // 2026-09-22 (per-arch stdlib boot entries): a NAMED raw block
            // registers its callable label only for the matching family —
            // `raw riscv64 uart_init` + `raw thumbv7m uart_init` share the
            // name but never both register (one build = one target).
            BadTopLevel::RawBlock(b) => {
                if let Some(name) = &b.name {
                    if self.family.starts_with(&b.target) {
                        if !self.label_names.insert(name.clone()) {
                            self.errors.push(format!(
                                "named raw block `{name}` collides with an existing label - \
                                 labels share one global namespace"
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The shipped friendly alias sheet (std/bad/friendly.bad, baked in)
    /// — aliases only, user declarations override.
    fn load_friendly_sheet(&mut self) -> Result<(), String> {
        let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/std/bad/friendly.bad"));
        let sheet = crate::parser::bad::parse_bad(src)
            .map_err(|e| format!("friendly sheet: line {}: {}", e.line, e.message))?;
        for item in &sheet.items {
            if let BadTopLevel::Alias(a) = item {
                self.sheet_aliases.insert(a.name.clone());
                self.aliases.insert(a.name.clone(), a.register.clone());
            }
        }
        Ok(())
    }

    /// Label aliases: `alias Name = real` where the value is neither a
    /// register nor a core op. Bounded chain, like mnemonics.
    fn canonical_label(&self, name: &str) -> String {
        let mut cur = name.to_string();
        for _ in 0..8 {
            match self.aliases.get(&cur) {
                Some(t) if !self.isa.is_core(t) && !self.regs.exists(t, &self.family) => {
                    cur = t.clone();
                }
                _ => break,
            }
        }
        cur
    }

    /// Canonicalize a mnemonic through the alias table (bounded chain).
    fn canonical_mnemonic(&self, mnemonic: &str) -> String {
        let mut cur = mnemonic.to_string();
        for _ in 0..8 {
            match self.aliases.get(&cur) {
                Some(t) if self.isa.is_core(t) || self.defns.contains_key(t) => {
                    cur = t.clone();
                }
                _ => break,
            }
        }
        cur
    }

    /// Recursively inline `import "path.bad"` files at their position.
    /// `depth` bounds pathological import graphs.
    fn expand_imports(&mut self, items: Vec<BadTopLevel>, depth: usize) -> Result<(), String> {
        if depth > 16 {
            return Err("import graph deeper than 16 - break the import cycle".to_string());
        }
        for item in items {
            if let BadTopLevel::Directive(d) = &item {
                if d.name == "import" {
                    let raw = d.args.trim().trim_matches('"').to_string();
                    self.load_import(&raw, depth)?;
                    continue;
                }
            }
            self.emit_items.push(item);
        }
        Ok(())
    }

    fn load_import(&mut self, path: &str, depth: usize) -> Result<(), String> {
        let mut candidates = Vec::new();
        if path.starts_with('/') {
            candidates.push(std::path::PathBuf::from(path));
        } else {
            if let Some(base) = &self.base_dir {
                candidates.push(base.join(path));
            }
            candidates.push(std::path::PathBuf::from(path));
        }
        let resolved = candidates
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| {
                format!(
                    "import `{path}` not found - resolve it against the importing file's \
                     directory or the working directory"
                )
            })?;
        let canonical = resolved
            .canonicalize()
            .map_err(|e| format!("import `{path}` cannot be canonicalized: {e}"))?;
        if !self.visited.insert(canonical.clone()) {
            return Ok(()); // already expanded — imports are idempotent
        }
        let src = std::fs::read_to_string(&resolved)
            .map_err(|e| format!("import `{path}`: cannot read: {e}"))?;
        let imported = crate::parser::bad::parse_bad(&src)
            .map_err(|e| format!("import `{path}`: line {}: {}", e.line, e.message))?;
        self.expand_imports(imported.items, depth + 1)
    }
    /// Data-directive args: comptime consts resolve to values
    /// (`.zero Guest.size` → `.zero 24`); string literals pass verbatim;
    /// unknown identifiers stay (GAS resolves labels).
    fn resolve_data_args(&self, args: &str) -> String {
        if args.starts_with('"') {
            return args.to_string();
        }
        let mut out = String::new();
        let mut rest = args;
        while let Some(start) = find_word_start(rest) {
            let after = &rest[start..];
            let end = after
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(after.len());
            let word = &after[..end];
            out.push_str(&rest[..start]);
            match self.const_value(word, &[]) {
                Ok(v) => out.push_str(&v.to_string()),
                Err(_) => out.push_str(word),
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }

    fn emit_directive(&mut self, d: &BadDirective) {
        match d.name.as_str() {
            // Consumed in pass 1 — never emitted.
            ".const" | ".struct" | ".field" | ".end" => {}
            "section" => self.push_line(&format!(".section {}", d.args)),
            "global" => self.push_line(&format!(".global {}", d.args.trim())),
            "export" => {
                let name = d.args.trim();
                if !self.label_names.contains(name) {
                    self.errors.push(format!(
                        "`export {name}` (line {}) names no label - an export must point \
                         at a label declared in this file",
                        d.span.line
                    ));
                }
                self.push_line(&format!(".global {name}"));
            }
            _ => self.push_line(&format!("{} {}", d.name, d.args)),
        }
    }

    fn collect_directive(&mut self, d: &BadDirective) {
        let bare_name: &str = d.name.trim_start_matches('.');
        match bare_name {
            "const" => {
                let (name, expr) = split_ws(&d.args);
                if name.is_empty() || expr.is_empty() {
                    self.errors.push(format!(
                        ".const needs `const NAME expr` form (line {})",
                        d.span.line
                    ));
                    return;
                }
                if self.consts.insert(name.to_string(), expr.to_string()).is_some() {
                    self.errors.push(format!(
                        "const `{}` is declared twice (line {}) - remove one declaration",
                        name, d.span.line
                    ));
                }
            }
            "struct" => {
                let (name, _) = split_ws(&d.args);
                if name.is_empty() {
                    self.errors.push(format!(
                        ".struct needs a name (line {})",
                        d.span.line
                    ));
                    return;
                }
                self.current_struct = Some(name.to_string());
            }
            "field" => {
                // `.field name, size[, align]` — context is the open
                // `.struct` block (directives are sequential).
                let Some(struct_name) = self.current_struct.clone() else {
                    self.errors.push(format!(
                        ".field outside a .struct block (line {})",
                        d.span.line
                    ));
                    return;
                };
                let mut parts = d.args.split(',');
                let field = parts.next().unwrap_or("").trim().to_string();
                let size_expr = parts.next().unwrap_or("").trim().to_string();
                let align_expr = parts.next().map(|a| a.trim().to_string());
                if field.is_empty() || size_expr.is_empty() {
                    self.errors.push(format!(
                        ".field needs `.field name, size[, align]` (line {})",
                        d.span.line
                    ));
                    return;
                }
                self.structs.push(StructField { struct_name, field, size_expr, align_expr });
            }
            "end" => {
                self.current_struct = None;
            }
            _ => {}
        }
    }

    /// Sequential natural layout: each field's offset is its align
    /// (default: the field size clamped to 8) rounded up to; `.size`
    /// lands as the struct-name const.
    fn layout_structs(&mut self) {
        let mut offset: i64 = 0;
        let mut current = String::new();
        let fields = std::mem::take(&mut self.structs);
        for f in &fields {
            if f.struct_name != current {
                // New struct: close the previous one, reset the offset.
                if !current.is_empty() {
                    self.consts.insert(format!("{current}.size"), offset.to_string());
                }
                current = f.struct_name.clone();
                offset = 0;
                self.consts.insert(current.clone(), "0".to_string());
            }
            let (size, align) = match self.eval_two(&f.size_expr, f.align_expr.as_deref()) {
                Ok(pair) => pair,
                Err(e) => {
                    self.errors.push(e);
                    continue;
                }
            };
            let natural = match size.clamp(1, 8) {
                1 | 2 => size.clamp(1, 8),
                3..=4 => 4,
                _ => 8,
            };
            let align = align.unwrap_or(natural).max(1);
            offset = (offset + align - 1) / align * align;
            self.consts.insert(format!("{current}.{}", f.field), offset.to_string());
            offset += size;
        }
        if !current.is_empty() {
            self.consts.insert(format!("{current}.size"), offset.to_string());
        }
    }

    /// Evaluate up to two const expressions (size, optional align).
    fn eval_two(&self, a: &str, b: Option<&str>) -> Result<(i64, Option<i64>), String> {
        let va = comptime::eval(a, &|n: &str| self.const_value(n, &[]))
            .map_err(|e| format!("struct field: {e}"))?;
        let vb = match b {
            Some(e) => Some(
                comptime::eval(e, &|n: &str| self.const_value(n, &[]))
                    .map_err(|e| format!("struct field: {e}"))?,
            ),
            None => None,
        };
        Ok((va, vb))
    }

    /// Const value with a cycle-guarded path.
    fn const_value(&self, name: &str, path: &[String]) -> Result<i64, String> {
        if path.iter().any(|p| p == name) {
            let mut cycle = path.join(" -> ");
            cycle.push_str(&format!(" -> {name}"));
            return Err(format!("const cycle: {cycle}"));
        }
        let expr = self
            .consts
            .get(name)
            .ok_or_else(|| format!("unknown constant `{name}`"))?;
        let mut next = path.to_vec();
        next.push(name.to_string());
        comptime::eval(expr, &|n: &str| self.const_value(n, &next))
    }

    /// Evaluate an operand expression: consts + defn-param immediates.
    fn eval_operand_expr(
        &self, expr: &str, env: &HashMap<String, Bound>, instr: &BadInstr,
    ) -> Result<i64, String> {
        comptime::eval(expr, &|n: &str| {
            if let Some(v) = self.const_value(n, &[]).ok() {
                return Ok(v);
            }
            if let Some(Bound::Imm(v)) = env.get(n) {
                return Ok(*v);
            }
            if env.contains_key(n) {
                return Err(format!(
                    "expression `{expr}` (line {}) uses `{n}`, which is bound to a \
                     REGISTER - register values are not compile-time constants",
                    instr.span.line
                ));
            }
            self.const_value(n, &[])
        })
        .map_err(|e| format!("{e} (line {})", instr.span.line))
    }

    fn emit_label(&mut self, l: &BadLabel) {
        // Local labels never open a new scope — the parser only nests
        // them inside a body, so current_label is already the parent.
        if !l.local {
            self.current_label = Some(l.name.clone());
        }
        self.push_line(&format!("{}:", self.canonical_label(&l.name)));
        let proven = contracts::check_label_contracts(l, self.regs, &self.family, &mut self.errors);
        for note in &proven {
            self.push_line(&format!("{} {}", self.regs.comment_prefix(&self.family), note));
        }
        for item in &l.body {
            match item {
                BadBodyItem::Instr(instr) => {
                    if let Err(e) = self.emit_instr(instr, &HashMap::new(), 0) {
                        self.errors.push(e);
                    }
                }
                BadBodyItem::Local(local) => {
                    self.push_line(&self.local_label_name(&l.name, &local.name));
                }
            }
        }
    }

    /// Local label `.name` inside global label `parent` → unique symbol.
    /// The same mapping applies at REFERENCE sites (sym operands starting
    /// with `.` inside the same parent), so `.loop` and `jnz .loop` meet.
    fn local_label_name(&self, parent: &str, name: &str) -> String {
        format!("L{parent}__{name}:")
    }

    /// Step 2 of the dispatch: a target-matched exception emits raw.
    fn emit_exception(
        &mut self, instr: &BadInstr, branch: &BadBranch, env: &HashMap<String, Bound>,
    ) {
        if self.trace {
            eprintln!(
                "trace: `{mn}` (line {ln}) -> {fam} EXCEPTION `{tgt}`",
                mn = instr.mnemonic,
                ln = instr.span.line,
                fam = self.family,
                tgt = branch.target
            );
        }
        for raw in &branch.body {
            self.emit_raw_instr(raw, env);
        }
    }

    /// The `Arg d, n` routine — uniform across targets, driven entirely
    /// by the abi_* config rows.
    fn emit_arg(&mut self, instr: &BadInstr, env: &HashMap<String, Bound>) -> Result<(), String> {
        if instr.operands.len() != 2 {
            return Err(format!(
                "`arg` takes 2 operands (dst, n), got {} at line {}",
                instr.operands.len(),
                instr.span.line
            ));
        }
        let n = match instr.operands[1].clone() {
            BadOperand::Int(n) => n,
            BadOperand::Name(name) => match self.eval_operand_expr(&name, env, instr) {
                Ok(v) => v,
                Err(_) => {
                    return Err(format!(
                        "`arg` index (line {}) must be a constant - the ABI position is \
                         compile-time data",
                        instr.span.line
                    ));
                }
            },
            BadOperand::Float(_) => {
                return Err(self.bad_arg_index(instr));
            }
            BadOperand::Expr(e) => self.eval_operand_expr(&e, env, instr)?,
        };
        if n < 1 {
            return Err(format!(
                "`arg` index {} (line {}) is out of range - arguments number from 1",
                n, instr.span.line
            ));
        }
        let reg_args = self.regs.abi_reg_args(&self.family);
        let route = if (n as usize) <= reg_args {
            // Register window: a move from the mapped arg register.
            let src = self.regs.abi_args(&self.family).get(n as usize - 1).cloned();
            match src {
                Some(src) => BadInstr {
                    mnemonic: "mov".to_string(),
                    operands: vec![instr.operands[0].clone(), BadOperand::Name(src)],
                    contract: None,
                    exceptions: Vec::new(),
                    ack: None,
                    span: instr.span,
                },
                None => {
                    return Err(self.bad_arg_index(instr));
                }
            }
        } else {
            // Stack window: base + (n - reg_args - 1) * 8.
            let off = self.regs.abi_stack_arg_base(&self.family)
                + (n as i64 - reg_args as i64 - 1) * 8;
            BadInstr {
                mnemonic: "loadoff".to_string(),
                operands: vec![
                    instr.operands[0].clone(),
                    BadOperand::Name("sp".to_string()),
                    BadOperand::Int(off),
                ],
                contract: None,
                exceptions: Vec::new(),
                ack: None,
                span: instr.span,
            }
        };
        self.emit_instr(&route, env, 0)
    }

    fn bad_arg_index(&self, instr: &BadInstr) -> String {
        format!(
            "`arg` index (line {}) must be an integer constant",
            instr.span.line
        )
    }

    /// Step 1 of the dispatch: non-core mnemonics are defn calls.
    fn route_non_core(
        &mut self, instr: &BadInstr, env: &HashMap<String, Bound>, depth: usize,
    ) -> Result<(), String> {
        if self.defns.contains_key(&instr.mnemonic) {
            let d = self.defns.get(&instr.mnemonic).cloned().unwrap();
            return self.expand_defn(&d, instr, env, depth);
        }
        Err(self.unknown_mnemonic(instr))
    }

    /// Arity + sym-position checks for a core-op call.
    fn check_call_shape(&self, instr: &BadInstr) -> Result<(), String> {
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
        // A sym op's LAST operand is the label/symbol — an immediate
        // there is always a mistake (`addr r5, 1` wants `mov r5, 1`).
        if self.isa.is_sym(&instr.mnemonic)
            && instr.operands.last().is_some_and(|o| matches!(o, BadOperand::Int(_)))
        {
            return Err(format!(
                "`{mn}` (line {ln}) takes a label or symbol as its last operand - an \
                 immediate was supplied; use `mov` for values, `addr d, label` for \
                 addresses",
                mn = instr.mnemonic,
                ln = instr.span.line
            ));
        }
        Ok(())
    }

    fn unknown_mnemonic(&self, instr: &BadInstr) -> String {
        format!(
            "instruction `{}` is unknown at line {} - it is not a core op ({}) \
             and no defn with that name exists",
            instr.mnemonic,
            instr.span.line,
            self.isa.known_ops().join(", ")
        )
    }

    /// Emit one instruction under `env` (defn param bindings). `depth`
    /// guards defn recursion.
    fn emit_instr(
        &mut self, instr: &BadInstr, env: &HashMap<String, Bound>, depth: usize,
    ) -> Result<(), String> {
        contracts::check_inline_contract(instr, self.regs, &self.family, &mut self.errors);

        // 0. Mnemonic aliases (`Move` → `mov`): canonicalize once, then
        // every later stage sees the core name.
        let canonical = self.canonical_mnemonic(&instr.mnemonic);
        let instr: BadInstr = if canonical == instr.mnemonic {
            instr.clone()
        } else {
            let mut owned = instr.clone();
            owned.mnemonic = canonical;
            owned
        };
        let instr = &instr;

        // 1. Defn call → inline expansion.
        if !self.isa.is_core(&instr.mnemonic) {
            return self.route_non_core(instr, env, depth);
        }

        // 1b. `Arg d, n` — ABI argument materialization. A general
        // routine over the config rows: within the register window it is
        // a move from the mapped arg register; beyond it, a stack
        // loadoff at abi_stack_arg_base + (n - reg_args - 1) * 8.
        if instr.mnemonic == "arg" {
            return self.emit_arg(instr, env);
        }

        // 2. Attached exception matching this target → raw emission.
        if let Some(branch) = instr.exceptions.iter().find(|b| b.target == self.family) {
            self.emit_exception(instr, branch, env);
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
        self.check_call_shape(instr)?;

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
        let has_imm = instr.operands.iter().any(|o| match o {
            BadOperand::Int(_) | BadOperand::Float(_) => true,
            // Const names resolve to immediates at substitution — they
            // must pick the imm form too (aarch64 mul takes no #imm).
            BadOperand::Name(name) => self.consts.contains_key(name),
            // An Expr that EVALUATES to a constant (a bare hex literal
            // `0x40004008`) is an immediate; one that resolves to a label
            // (`addr + 8`) is not.
            BadOperand::Expr(e) => self
                .eval_operand_expr(e, env, instr)
                .map(|_| true)
                .unwrap_or(false),
        });
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
        if self.trace {
            let form = if has_imm { "imm" } else { "reg" };
            eprintln!(
                "trace: `{mn}` (line {ln}) -> {fam} universal {form}-form",
                mn = instr.mnemonic,
                ln = instr.span.line,
                fam = self.family
            );
        }
        self.emit_mapped(&text, env);
        Ok(())
    }

    /// Expand a defn call: bind params positionally, emit its body/rows.
    fn expand_defn(
        &mut self, d: &BadDefn, call: &BadInstr, outer: &HashMap<String, Bound>, depth: usize,
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
                BadOperand::Float(_) => {
                    return Err(format!(
                        "defn param `{}` bound to a float literal (line {}) - floats ride \
                         the FP register class (f0-f15) via fmov/fload, not integer params",
                        op_debug_name(op),
                        call.span.line
                    ));
                }
                BadOperand::Expr(e) => match self.eval_operand_expr(e, outer, call) {
                    Ok(v) => Bound::Imm(v),
                    Err(err) => {
                        self.errors.push(err);
                        return Ok(());
                    }
                },
                BadOperand::Name(name) => {
                    // An operand that names an outer param inherits its
                    // binding; anything else is a token resolved at
                    // emission (register, label, or symbol).
                    outer.get(name).cloned().unwrap_or(Bound::Token(name.to_string()))
                }
            };
            env.insert(param.clone(), bound);
        }

        // Hygiene scope: every expansion renames its local labels
        // (`L<defn>__<local>__<n>`) and rewrites branch references
        // through the same scope — double invocation cannot collide.
        let call_n = self.defn_calls.entry(d.name.clone()).or_insert(0);
        *call_n += 1;
        let scope = format!("L{}__{}", d.name, call_n);
        let prev_scope = self.local_scope.take();
        self.local_scope = Some(scope);

        match &d.shape {
            BadDefnShape::Sequence(items) => {
                for item in items {
                    match item {
                        BadBodyItem::Instr(instr) => {
                            self.emit_instr(instr, &env, depth + 1)?;
                        }
                        BadBodyItem::Local(local) => {
                            let s = self.local_scope.clone().unwrap_or_default();
                            self.push_line(&format!("{}__{}:", s, local.name));
                        }
                    }
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
        self.local_scope = prev_scope;
        Ok(())
    }

    /// Emit a target-owned (raw) instruction: mnemonic verbatim, operands
    /// resolved through the env/register table with boundary replacement
    /// inside composite operand text.
    fn emit_raw_instr(&mut self, instr: &BadInstr, env: &HashMap<String, Bound>) {
        // Raw bodies are target-owned, but friendly mnemonics still
        // canonicalize (the sheet is active grammar, not decoration).
        let mnemonic = self.canonical_mnemonic(&instr.mnemonic);
        let operands: Vec<String> = instr
            .operands
            .iter()
            .map(|op| self.resolve_raw_operand(op, env, instr.span.line, instr))
            .collect();
        let mut line = mnemonic;
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
        instr: &BadInstr,
    ) -> String {
        match op {
            BadOperand::Int(n) => format!("{}{}", self.regs.imm_prefix(&self.family), n),
            BadOperand::Float(t) => self.float_operand(t, instr),
            // Raw-row Expr operands are TARGET-OWNED text (`lsl #1`) —
            // verbatim with name substitution, never comptime.
            BadOperand::Expr(e) => self.resolve_raw_name(e, env, line),
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
            BadOperand::Float(t) => Ok(self.float_operand(t, instr)),
            BadOperand::Expr(e) => match self.eval_operand_expr(e, env, instr) {
                Ok(v) if r.bare => Ok(v.to_string()),
                Ok(v) => Ok(format!("{imm}{v}")),
                Err(err) => Err(err),
            },
            BadOperand::Name(name) => {
                if r.named {
                    return self.syscall_number_token(name, instr);
                }
                // Check local env first, then fn-level param_env.
                if let Some(Bound::Imm(v)) = env.get(name).or_else(|| self.param_env.get(name)) {
                    return Ok(format!("{imm}{v}"));
                }
                let (bound_name, param) = match env.get(name).or_else(|| self.param_env.get(name)) {
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

    /// `$N#` — a kernel-call number by name, per the syscall_nums rows.
    fn syscall_number_token(&self, name: &str, instr: &BadInstr) -> Result<String, String> {
        match self.regs.syscall_number(&self.family, name) {
            Some(v) => Ok(format!("{}{}", self.regs.imm_prefix(&self.family), v)),
            None => {
                let known = self.regs.known_syscalls(&self.family).join(", ");
                Err(format!(
                    "syscall `{}` (line {}) names `{}`, which is not in the \
                     syscall_nums row for `{}` - add it to config/bad-registers.dbvl \
                     (known: {known})",
                    instr.mnemonic,
                    instr.span.line,
                    name,
                    self.family
                ))
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
        // A `.const` name resolves to an immediate token. Known key =
        // committed: cycle errors are loud, never a silent symbol.
        if self.consts.contains_key(name) {
            let v = self.const_value(name, &[])?;
            return Ok(format!("{}{}", self.regs.imm_prefix(&self.family), v));
        }
        if allow_symbols {
            // Local-label reference: `.loop` inside label `foo` →
            // `Lfoo__loop`. A missing parent is a hard error (the
            // parser already rejects orphan local labels).
            if let Some(rest) = name.strip_prefix('.') {
                if let Some(scope) = &self.local_scope {
                    return Ok(format!("{scope}__{rest}"));
                }
                return match &self.current_label {
                    Some(parent) => Ok(format!("L{parent}__{rest}")),
                    None => Err(format!(
                        "reference `.{rest}` (line {}) is outside any label - local \
                         labels resolve within the global label that contains them",
                        instr.span.line
                    )),
                };
            }
            // Label aliases: `alias entry = _start` — references rewrite
            // to the canonical name (values naming registers or core ops
            // are register/mnemonic aliases, handled earlier).
            return Ok(self.canonical_label(name));
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
        // `;` inside a template splits into separate emitted lines
        // (documented row semantics — riscv push/pop, x86 push2, ...).
        for part in text.split(';') {
            let part = part.trim();
            if !part.is_empty() {
                self.push_line(part);
            }
        }
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
        match env.get(word).or_else(|| self.param_env.get(word)) {
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

    /// A Float operand's final text: a pool label (x86_64/riscv64 ride
    /// `.rodata` pool entries) or the literal itself (aarch64 `ldr =1.5`
    /// GAS literal pools), per the `float_literal` config row.
    fn float_operand(&self, text: &str, instr: &BadInstr) -> String {
        let mode = self.regs.float_literal(&self.family);
        match mode {
            "literal" => text.to_string(),
            // The parser validates the f64 parse; re-parse failure here
            // cannot occur. The instr parameter documents the call site.
            _ => {
                let _ = instr;
                self.float_pool.borrow_mut().intern(text)
            }
        }
    }

    /// Append the FP literal pool (`.rodata`) when the target rides the
    /// pool and any float was interned.
    fn flush_float_pool(&mut self) {
        if self.regs.float_literal(&self.family) != "pool" {
            return;
        }
        let entries = self.float_pool.borrow().entries.clone();
        if entries.is_empty() {
            return;
        }
        self.push_line("");
        self.push_line(".section .rodata");
        self.push_line(".balign 8");
        for (label, text) in &entries {
            self.push_line(&format!("{label}:"));
            self.push_line(&format!(".double {text}"));
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
/// qualifier (`.w8/.w16/.w32`), the bare flag (`$N!` = no imm prefix),
/// and the named flag (`$N#` = kernel-call number by NAME, resolved
/// through the syscall_nums rows).
#[derive(Debug, Clone, Copy)]
struct Ref {
    n: usize,
    width: Option<u8>,
    bare: bool,
    named: bool,
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
    let r = Ref { n: s[i + 1..j].parse().unwrap_or(0), width: None, bare: false, named: false };
    let (r, j) = take_width_suffix(s, j, r);
    if bytes.get(j) == Some(&b'!') {
        return Some((Ref { bare: true, ..r }, j + 1));
    }
    if bytes.get(j) == Some(&b'#') {
        return Some((Ref { named: true, ..r }, j + 1));
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
fn op_debug_name(op: &BadOperand) -> String {
    match op {
        BadOperand::Int(n) => n.to_string(),
        BadOperand::Float(t) => t.clone(),
        BadOperand::Name(n) => n.clone(),
        BadOperand::Expr(e) => e.clone(),
    }
}

fn split_ws(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
}

fn find_word_start(s: &str) -> Option<usize> {
    s.char_indices()
        .find(|(_, c)| c.is_alphabetic() || *c == '_' || *c == '.')
        .map(|(i, _)| i)
}