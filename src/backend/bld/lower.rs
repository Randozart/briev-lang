// ── .bld lowering — BILLD AST → BadProgram ────────────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-10-08 (BILLD plan, docs/plans/2026-10-08-billd-intermediate-dialect.md
// M3): the lowering core — .bld execution recipes become a `BadProgram`
// the existing .bad backend emits unchanged.
//
// Division of labor:
// - STRUCTURED CONTROL FLOW (`when`/`while`/`loop`/`break`/`continue`)
//   lowers to local labels + fused compare-branches. Unbounded loops are
//   legal here by design (the physical world spins; no termination gates).
// - EXPRESSIONS lower to SSA-shaped values: constants fold at compile
//   time, everything else names a value (a bound ABI register, a `bad { }`
//   physical register, or a `vN` virtual). Virtual names resolve in M4
//   (register allocator) — they are intentional in the emitted BadProgram
//   until then; only const/physical recipes reach `.s` end to end.
// - `bad { … }` blocks parse through the .bad parser verbatim; instruction
//   lines splice into the recipe body, ownership items (sections, data,
//   defns, raw blocks) attach only at the first/last statement of a
//   recipe — .bad ownership is positional and a mid-recipe owner would
//   steal the following instructions.
// - IMPORTS: `.bad` files pass through as `import` directives for the
//   .bad backend (labels/defns harvested as external call signatures);
//   `.bld` files merge their items recursively (canonical-path dedup).
// - CONSTS: two-phase — collect every declaration (forward references
//   legal, duplicates loud), then evaluate with a cycle guard.
//
// Naked semantics: no prologue, no auto-`ret`, no hidden copies beyond
// what a recipe writes.
//
// To undo: delete src/backend/bld/, revert `pub mod bld;` in
// src/backend/mod.rs, and revert the `pub` on density::is_float_type.

use crate::analysis::density::is_float_type;
use crate::ast::bad::{
    BadBodyItem, BadDirective, BadInstr, BadLabel, BadLocal, BadOperand, BadProgram, BadTopLevel,
};
use crate::ast::bld::{BldBootstrap, BldConst, BldDefn, BldStmt, BldTopLevel};
use crate::ast::{BinaryOpKind, Expr, Type, UnaryOpKind};
use crate::backend::bad::registry::{BadIsa, BadIsaLowering, BadRegisters, ImmHandling};
use crate::errors::Span;
use crate::parser::bad::parse_bad;
use crate::parser::bld::parse_bld;
use std::collections::{HashMap, HashSet};

/// Import recursion bound (same bound the .bad backend enforces).
const MAX_IMPORT_DEPTH: usize = 16;

/// The value classes a .bld recipe manipulates. Every lowered value is
/// one of these; the class picks the mnemonic family (`add`/`fadd`) and
/// the argument/return register class at call boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VClass {
    Int,
    Float,
    Bool,
    Ptr,
}

impl VClass {
    pub fn name(self) -> &'static str {
        match self {
            VClass::Int => "Int",
            VClass::Float => "Float",
            VClass::Bool => "Bool",
            VClass::Ptr => "Ptr",
        }
    }
}

/// A folded compile-time constant. `Int` also carries Bool/Char/pointer
/// constants — they share the machine word; the class lives on the
/// binding/operand, not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstVal {
    Int(i64),
    Float(f64),
}

/// One recipe-scope binding: a value register (phys ABI register or a
/// `vN` virtual) or a folded constant (`reg == None`).
#[derive(Debug, Clone)]
struct Binding {
    reg: Option<String>,
    class: VClass,
    konst: Option<ConstVal>,
}

/// A lowered expression result: the operand to emit, its class, and its
/// constant value when known (constants stay symbolic until an instruction
/// actually needs a register). `temp` marks a freshly emitted single-def
/// value register — a `let` binding may adopt it as storage; a value read
/// from an existing binding never is (adopting would alias the two names).
#[derive(Debug, Clone)]
pub struct Lowered {
    op: BadOperand,
    class: VClass,
    konst: Option<ConstVal>,
    temp: bool,
}

impl Lowered {
    fn konst(v: ConstVal, class: VClass) -> Self {
        let op = match v {
            ConstVal::Int(n) => BadOperand::Int(n),
            ConstVal::Float(f) => BadOperand::Float(float_text(f)),
        };
        Lowered { op, class, konst: Some(v), temp: false }
    }

    fn reg(name: &str, class: VClass) -> Self {
        Lowered {
            op: BadOperand::Name(name.to_string()),
            class,
            konst: None,
            temp: false,
        }
    }

    /// A freshly emitted single-def value register: safe for a `let`
    /// binding to adopt as its storage (no second copy).
    fn temp_reg(name: String, class: VClass) -> Self {
        Lowered {
            op: BadOperand::Name(name),
            class,
            konst: None,
            temp: true,
        }
    }

    fn truthy(v: ConstVal) -> bool {
        match v {
            ConstVal::Int(n) => n != 0,
            ConstVal::Float(f) => f != 0.0,
        }
    }
}

/// A callable signature. `.bld` defns carry annotated classes; labels
/// harvested from imported `.bad` files are external (argument classes
/// come from the values themselves, arity unchecked for plain labels).
#[derive(Debug, Clone)]
struct Sig {
    params: Vec<VClass>,
    ret: Option<VClass>,
    /// External calls skip class coercion (the callee's parameter classes
    /// are unknown; values pass as their own class).
    external: bool,
    check_arity: bool,
    span: Span,
    file: String,
}

impl Sig {
    /// A `.bad` code label / named raw block: C-ABI calling convention,
    /// result in the return register, arity unchecked at the call site.
    fn external_label(span: Span, file: &str) -> Self {
        Sig {
            params: Vec::new(),
            ret: Some(VClass::Int),
            external: true,
            check_arity: false,
            span,
            file: file.to_string(),
        }
    }

    /// A `.bad` sequence defn: arity known, classes unknown (values pass
    /// as their own class), no result.
    fn external_defn(n: usize, span: Span, file: &str) -> Self {
        Sig {
            params: vec![VClass::Int; n],
            ret: None,
            external: true,
            check_arity: true,
            span,
            file: file.to_string(),
        }
    }
}

/// Where a statement sits in its recipe — .bad ownership items (sections,
/// data labels, defns) may attach only at the recipe's first or last
/// statement; anything else would be mid-recipe ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecipePos {
    First,
    Middle,
    Last,
}

/// The per-class argument-register lists for one target family — one
/// load, three consumers (parameter binding, call staging, staged copy).
struct ArgRegs {
    gp: Vec<String>,
    fp: Vec<String>,
    gp_budget: usize,
}

impl ArgRegs {
    fn load(regs: &BadRegisters, family: &str) -> Self {
        let gp = regs.abi_args(family);
        let gp_budget = regs.abi_reg_args(family).min(gp.len());
        ArgRegs {
            gp,
            fp: regs.abi_args_fp(family),
            gp_budget,
        }
    }

    /// The next argument register of `class`, or None when the class's
    /// register budget is exhausted.
    fn next_slot(&self, class: VClass, cur: ArgCursor) -> Option<String> {
        if class == VClass::Float {
            self.fp.get(cur.fp).cloned()
        } else {
            self.gp.get(cur.gp).cloned()
        }
    }

    fn budget(&self, class: VClass) -> usize {
        if class == VClass::Float {
            self.fp.len()
        } else {
            self.gp_budget
        }
    }
}

/// Consumption cursor over the two argument-register classes.
#[derive(Clone, Copy, Default)]
struct ArgCursor {
    gp: usize,
    fp: usize,
}

impl ArgCursor {
    fn take(&mut self, class: VClass) {
        if class == VClass::Float {
            self.fp += 1;
        } else {
            self.gp += 1;
        }
    }

    fn arg_no(&self) -> usize {
        self.gp + self.fp
    }
}

/// Everything a recipe needs, bundled so recipe_core stays under the
/// parameter budget.
struct RecipeHead<'s> {
    name: &'s str,
    params: &'s [(String, VClass)],
    ret: Option<VClass>,
    stmts: &'s [BldStmt],
    span: Span,
}

/// One recipe under construction: emitted body items, scope stack, loop
/// targets, and the ownership items that frame the label.
struct Body {
    items: Vec<BadBodyItem>,
    /// Ownership from a first-statement `bad { }` block — emitted before
    /// the recipe label so its directives do not land inside the recipe.
    own_first: Vec<BadTopLevel>,
    /// Ownership from a last-statement block — emitted after the label.
    trailing: Vec<BadTopLevel>,
    scopes: Vec<HashMap<String, Binding>>,
    /// (head, end) per enclosing `loop`/`while`, innermost last.
    loops: Vec<(String, String)>,
    seq: usize,
    vreg: usize,
    name: String,
    ret: Option<VClass>,
}

impl Body {
    fn new(name: &str, ret: Option<VClass>) -> Self {
        Body {
            items: Vec::new(),
            own_first: Vec::new(),
            trailing: Vec::new(),
            scopes: vec![HashMap::new()],
            loops: Vec::new(),
            seq: 0,
            vreg: 0,
            name: name.to_string(),
            ret,
        }
    }
}

/// A flattened stream item: source order across the root file and every
/// imported .bld module, each tagged with its file for diagnostics.
#[derive(Clone)]
enum Stream {
    Import(BadDirective),
    Const(BldConst),
    Defn(BldDefn),
    Bootstrap(BldBootstrap),
}

pub struct BldLowerer<'a> {
    isa: &'a BadIsa,
    regs: &'a BadRegisters,
    family: String,
    base_dir: Option<std::path::PathBuf>,
    root_path: Option<std::path::PathBuf>,
    stream: Vec<(Stream, String)>,
    /// Canonical paths of imported modules (dedup: diamonds and cycles).
    modules: HashSet<std::path::PathBuf>,
    /// Collected `const` declarations: name → (expr, span, file).
    const_exprs: HashMap<String, (Expr, Span, String)>,
    consts_done: HashMap<String, ConstVal>,
    resolving: HashSet<String>,
    sigs: HashMap<String, Sig>,
    bootstraps: usize,
    out: Vec<BadTopLevel>,
    body: Body,
    cur_span: Span,
    cur_file: String,
    root_span: Span,
}

impl<'a> BldLowerer<'a> {
    /// 2026-10-08 (BILLD M3): build a lowerer for one compilation.
    /// `base_dir` resolves relative imports; `root_path` (when the caller
    /// knows it) seeds module dedup so a cycle back to the root file is
    /// recognized instead of re-expanding the root's items.
    pub fn new(
        isa: &'a BadIsa,
        regs: &'a BadRegisters,
        family: &str,
        base_dir: Option<std::path::PathBuf>,
        root_path: Option<std::path::PathBuf>,
    ) -> Self {
        let empty = Body::new("", None);
        let span = Span { start: 0, end: 0, line: 1, column: 1 };
        BldLowerer {
            isa,
            regs,
            family: family.to_string(),
            base_dir,
            root_path,
            stream: Vec::new(),
            modules: HashSet::new(),
            const_exprs: HashMap::new(),
            consts_done: HashMap::new(),
            resolving: HashSet::new(),
            sigs: HashMap::new(),
            bootstraps: 0,
            out: Vec::new(),
            body: empty,
            cur_span: span,
            cur_file: "<input>".to_string(),
            root_span: span,
        }
    }

    /// Parse the root source and merge every import into `stream`.
    pub fn load(&mut self, source: &str) -> Result<(), String> {
        if let Some(root) = &self.root_path {
            if let Ok(canon) = root.canonicalize() {
                self.modules.insert(canon);
            }
        }
        let program = parse_bld(source)
            .map_err(|e| format!("bld: {e}"))?;
        self.root_span = program.span;
        self.cur_file = self
            .root_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<input>".to_string());
        let items = program.items;
        self.load_items(items, self.cur_file.clone(), 0)
    }

    /// Merge one file's top-level items, recursing through `.bld` imports.
    fn load_items(&mut self, items: Vec<BldTopLevel>, file: String, depth: usize) -> Result<(), String> {
        self.cur_file = file.clone();
        if depth > MAX_IMPORT_DEPTH {
            return Err(self.fail(
                "the import graph",
                &format!("it nests deeper than {MAX_IMPORT_DEPTH} files"),
                "break the import cycle — one module should not re-import the chain",
            ));
        }
        for item in items {
            match item {
                BldTopLevel::Import(imp) => {
                    self.cur_span = imp.span;
                    self.load_import(&imp.path, &file, depth, imp.span)?;
                }
                BldTopLevel::Const(c) => self.stream.push((Stream::Const(c), file.clone())),
                BldTopLevel::Defn(d) => self.stream.push((Stream::Defn(d), file.clone())),
                BldTopLevel::Bootstrap(b) => self.stream.push((Stream::Bootstrap(b), file.clone())),
            }
        }
        Ok(())
    }

    /// Resolve one import: `.bld` merges items, `.bad` passes through as
    /// a directive (with its labels harvested as external signatures).
    fn load_import(
        &mut self,
        path: &str,
        from_file: &str,
        depth: usize,
        span: Span,
    ) -> Result<(), String> {
        let resolved = self.resolve_import_path(path, from_file, span)?;
        let canonical = resolved
            .canonicalize()
            .map_err(|e| self.fail(&format!("import `{path}`"), &format!("it cannot be canonicalized: {e}"), "use a regular file path"))?;
        let ext = canonical.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "bld" => {
                if !self.modules.insert(canonical.clone()) {
                    return Ok(()); // diamond or cycle — merged once
                }
                let src = std::fs::read_to_string(&resolved).map_err(|e| {
                    self.fail(&format!("import `{path}`"), &format!("it cannot be read: {e}"), "check the path")
                })?;
                let nested = parse_bld(&src).map_err(|e| {
                    format!("bld `{}`: {e}", resolved.display())
                })?;
                self.load_items(nested.items, resolved.display().to_string(), depth + 1)
            }
            "bad" => {
                let first_visit = self.modules.insert(canonical.clone());
                if first_visit {
                    self.harvest_bad_signatures(&resolved, path, span)?;
                }
                let args = canonical.display().to_string();
                self.stream.push((
                    Stream::Import(BadDirective { name: "import".to_string(), args, span }),
                    from_file.to_string(),
                ));
                Ok(())
            }
            _ => Err(self.fail(
                &format!("import `{path}`"),
                "it is neither a `.bld` recipe file nor a `.bad` assembly file",
                "import a `.bld` or `.bad` file — other dialects do not participate in the ladder",
            )),
        }
    }

    /// Candidate resolution, mirroring the .bad backend: absolute paths
    /// as-is, otherwise the importing file's directory, then base_dir,
    /// then the working directory.
    fn resolve_import_path(
        &self,
        path: &str,
        from_file: &str,
        span: Span,
    ) -> Result<std::path::PathBuf, String> {
        if path.starts_with('/') {
            let p = std::path::PathBuf::from(path);
            return if p.is_file() {
                Ok(p)
            } else {
                Err(self.import_not_found(path))
            };
        }
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Some(dir) = std::path::Path::new(from_file).parent() {
            if dir != std::path::Path::new("") {
                candidates.push(dir.join(path));
            }
        }
        if let Some(base) = &self.base_dir {
            candidates.push(base.join(path));
        }
        candidates.push(std::path::PathBuf::from(path));
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| self.import_not_found(path))
    }

    /// The import site is already `cur_span`/`cur_file` (load_items and
    /// load_import set both before resolving), so the diagnostic points
    /// at the exact `import` line that failed.
    fn import_not_found(&self, path: &str) -> String {
        self.fail(
            &format!("import `{path}`"),
            "no such file relative to the importing file, the working directory, or base_dir",
            "check the path — imports resolve against the importing file's directory first",
        )
    }

    /// Parse an imported `.bad` file once and record its callable names:
    /// code labels and named raw blocks are external functions (return in
    /// r0/f0, arity unchecked), sequence defns keep their arity.
    fn harvest_bad_signatures(
        &mut self,
        resolved: &std::path::Path,
        path: &str,
        span: Span,
    ) -> Result<(), String> {
        let src = std::fs::read_to_string(resolved).map_err(|e| {
            self.fail(&format!("import `{path}`"), &format!("it cannot be read: {e}"), "check the path")
        })?;
        let prog = parse_bad(&src).map_err(|e| {
            format!("bad `{}`: line {}: {}", resolved.display(), e.line, e.message)
        })?;
        let file = resolved.display().to_string();
        for item in prog.items {
            let (name, sig) = match &item {
                BadTopLevel::Label(l) => {
                    (l.name.clone(), Sig::external_label(l.span, &file))
                }
                BadTopLevel::RawBlock(r) => match &r.name {
                    Some(n) => (n.clone(), Sig::external_label(r.span, &file)),
                    None => continue,
                },
                BadTopLevel::Defn(d) => {
                    (d.name.clone(), Sig::external_defn(d.params.len(), d.span, &file))
                }
                _ => continue,
            };
            self.cur_span = sig.span;
            self.cur_file = file.clone();
            self.declare_sig(&name, sig)?;
        }
        Ok(())
    }

    // ── collect: signatures and const declarations ────────────────────

    /// Pass 1 over the merged stream: `.bld` defns/bootstrap become
    /// signatures, consts become declarations (evaluated in pass 2).
    /// Duplicates and the bootstrap count are checked here.
    fn collect_signatures(&mut self) -> Result<(), String> {
        for (item, file) in self.stream.clone() {
            match item {
                Stream::Import(_) => {}
                Stream::Const(c) => {
                    self.cur_span = c.span;
                    self.cur_file = file.clone();
                    if self
                        .const_exprs
                        .insert(c.name.clone(), (c.value.clone(), c.span, file.clone()))
                        .is_some()
                    {
                        return Err(self.fail(
                            &format!("const `{}`", c.name),
                            "it is declared more than once across this file and its imports",
                            "rename one of the declarations — a .bld image has one value per name",
                        ));
                    }
                }
                Stream::Defn(d) => {
                    self.cur_span = d.span;
                    self.cur_file = file;
                    let sig = self.sig_for_defn(&d)?;
                    self.declare_sig(&d.name, sig)?;
                }
                Stream::Bootstrap(b) => {
                    self.cur_span = b.span;
                    self.cur_file = file.clone();
                    self.bootstraps += 1;
                    if self.bootstraps > 1 {
                        return Err(self.fail(
                            &format!("bootstrap `{}`", b.name),
                            "a .bld image has exactly one entry point, but this file and its imports declare more",
                            "keep one `bootstrap`; move the others into `defn`s",
                        ));
                    }
                    let sig = Sig {
                        params: Vec::new(),
                        ret: None,
                        external: false,
                        check_arity: true,
                        span: b.span,
                        file: file.clone(),
                    };
                    self.declare_sig(&b.name, sig)?;
                }
            }
        }
        Ok(())
    }

    fn declare_sig(&mut self, name: &str, sig: Sig) -> Result<(), String> {
        if self.sigs.contains_key(name) {
            let prior = self.sigs.get(name).cloned();
            let (pfile, pline) = prior
                .map(|s| (s.file, s.span.line))
                .unwrap_or_else(|| (sig.file.clone(), sig.span.line));
            return Err(self.fail(
                &format!("the name `{name}`"),
                &format!("it is already a callable or recipe name ({pfile} line {pline})"),
                "rename one of them — every callable name in a .bld image is unique",
            ));
        }
        self.sigs.insert(name.to_string(), sig);
        Ok(())
    }

    /// Build a defn's signature: every parameter must be annotated (.bld
    /// binds parameters straight to ABI registers, which need a class),
    /// the return type may be absent (a naked void recipe).
    fn sig_for_defn(&mut self, d: &BldDefn) -> Result<Sig, String> {
        let mut params = Vec::with_capacity(d.params.len());
        for p in &d.params {
            let ty = p.ty.as_ref().ok_or_else(|| {
                self.fail(
                    &format!("parameter `{}` of `defn {}`", p.name, d.name),
                    "it has no type, and .bld binds parameters directly to ABI registers",
                    &format!("annotate it — `{}: Int` (or Float/Bool/Ptr)", p.name),
                )
            })?;
            let class = self.annotation_class(ty).map_err(|why| {
                self.fail(
                    &format!("parameter `{}` of `defn {}`", p.name, d.name),
                    &why,
                    "use a scalar class: Int, Float, Bool, or Ptr",
                )
            })?;
            params.push(class);
        }
        let ret = match &d.ret {
            None => None,
            Some(Type::Void) => None,
            Some(ty) => Some(self.annotation_class(ty).map_err(|why| {
                self.fail(
                    &format!("the return type of `defn {}`", d.name),
                    &why,
                    "use a scalar class: Int, Float, Bool, or Ptr, or omit the return type",
                )
            })?),
        };
        Ok(Sig {
            params,
            ret,
            external: false,
            check_arity: true,
            span: d.span,
            file: self.cur_file.clone(),
        })
    }

    /// The closed scalar lattice a .bld annotation may name. Anything
    /// else is loud — recipes manipulate machine words, not structures.
    fn annotation_class(&self, ty: &Type) -> Result<VClass, String> {
        if is_float_type(ty) {
            return Ok(VClass::Float);
        }
        match ty {
            Type::Bits(_) => Ok(VClass::Int),
            Type::Ptr(_) | Type::PtrConst(_) => Ok(VClass::Ptr),
            Type::Constrained(inner, _) => self.annotation_class(inner),
            Type::Custom(n) => match n.as_str() {
                "Bool" => Ok(VClass::Bool),
                "Int" | "UInt" | "Char" | "Byte" | "Size" => Ok(VClass::Int),
                "Int8" | "Int16" | "Int32" | "Int64" => Ok(VClass::Int),
                "UInt8" | "UInt16" | "UInt32" | "UInt64" => Ok(VClass::Int),
                other => Err(format!(
                    "`{other}` is not a scalar class — a .bld recipe manipulates machine words"
                )),
            },
            _ => Err("this type has no .bld value class — recipes manipulate machine words"
                .to_string()),
        }
    }

    // ── pass 2: evaluate every const ──────────────────────────────────

    fn eval_all_consts(&mut self) -> Result<(), String> {
        let names: Vec<String> = self.const_exprs.keys().cloned().collect();
        for name in names {
            self.eval_const(&name)?;
        }
        Ok(())
    }

    fn eval_const(&mut self, name: &str) -> Result<ConstVal, String> {
        if let Some(v) = self.consts_done.get(name) {
            return Ok(*v);
        }
        let (expr, span, file) = match self.const_exprs.get(name) {
            Some(entry) => (entry.0.clone(), entry.1, entry.2.clone()),
            None => {
                return Err(self.fail(
                    &format!("constant `{name}`"),
                    "it is not defined in this file or its imports",
                    &format!("declare it — `const {name} = …;` — or check the spelling"),
                ));
            }
        };
        self.cur_span = span;
        self.cur_file = file;
        if !self.resolving.insert(name.to_string()) {
            return Err(self.fail(
                &format!("const `{name}`"),
                "it depends on itself — the constant chain loops back to this declaration",
                "break the cycle: give one constant an independent value",
            ));
        }
        let value = self.eval_const_expr(&expr)?;
        self.resolving.remove(name);
        self.consts_done.insert(name.to_string(), value);
        Ok(value)
    }

    /// Fold one constant expression: literals, other consts, and scalar
    /// arithmetic. Anything with runtime shape is loud here.
    fn eval_const_expr(&mut self, e: &Expr) -> Result<ConstVal, String> {
        match e {
            Expr::Decimal(n) => Ok(ConstVal::Int(*n)),
            Expr::Float(f) => Ok(ConstVal::Float(*f)),
            Expr::Bool(b) => Ok(ConstVal::Int(*b as i64)),
            Expr::Char(c) => Ok(ConstVal::Int(*c as i64)),
            Expr::TaggedLiteral(n, _) => Ok(ConstVal::Int(*n)),
            Expr::Identifier(name) => self.eval_const(name),
            Expr::UnaryOp(op, inner) => {
                let v = self.eval_const_expr(inner)?;
                self.fold_unary(*op, v)
            }
            Expr::BinaryOp(BinaryOpKind::And, l, r) => {
                let a = self.eval_const_expr(l)?;
                let b = self.eval_const_expr(r)?;
                Ok(ConstVal::Int((Lowered::truthy(a) && Lowered::truthy(b)) as i64))
            }
            Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
                let a = self.eval_const_expr(l)?;
                let b = self.eval_const_expr(r)?;
                Ok(ConstVal::Int((Lowered::truthy(a) || Lowered::truthy(b)) as i64))
            }
            Expr::BinaryOp(op, l, r) => {
                let a = self.eval_const_expr(l)?;
                let b = self.eval_const_expr(r)?;
                fold_bin(*op, a, b)
            }
            _ => Err("this is not a compile-time constant — a const holds literals, other consts, and scalar arithmetic over them".to_string()),
        }
    }

    fn fold_unary(&self, op: UnaryOpKind, v: ConstVal) -> Result<ConstVal, String> {
        match op {
            UnaryOpKind::Neg => match v {
                ConstVal::Int(n) => n.checked_neg().map(ConstVal::Int).ok_or_else(|| {
                    "negating this constant overflows Int — use a smaller value".to_string()
                }),
                ConstVal::Float(f) => Ok(ConstVal::Float(-f)),
            },
            UnaryOpKind::BitNot => match v {
                ConstVal::Int(n) => Ok(ConstVal::Int(!n)),
                ConstVal::Float(_) => Err(
                    "bitwise `~` is defined only for Int values — convert with `ftoi` in a `bad { }` block"
                        .to_string(),
                ),
            },
            UnaryOpKind::Not => Ok(ConstVal::Int((!Lowered::truthy(v)) as i64)),
        }
    }

    // ── diagnostics ───────────────────────────────────────────────────

    /// House-style error: what failed, where, why, and the concrete fix.
    fn fail(&self, what: &str, why: &str, fix: &str) -> String {
        format!(
            "{what} ({} line {}): {why} - {fix}",
            self.cur_file, self.cur_span.line
        )
    }

    // ── pass 3: emit ──────────────────────────────────────────────────

    /// The public lowering entry: collect signatures, evaluate consts,
    /// then emit every stream item in source order.
    pub fn compile(mut self) -> Result<BadProgram, String> {
        self.collect_signatures()?;
        self.eval_all_consts()?;
        self.emit_program()
    }

    fn emit_program(&mut self) -> Result<BadProgram, String> {
        self.out.push(BadTopLevel::Directive(BadDirective {
            name: "section".to_string(),
            args: ".text".to_string(),
            span: self.root_span,
        }));
        let stream = std::mem::take(&mut self.stream);
        for (item, file) in stream {
            self.cur_file = file;
            match item {
                Stream::Import(d) => self.out.push(BadTopLevel::Directive(d)),
                Stream::Const(c) => self.emit_const(&c)?,
                Stream::Defn(d) => {
                    self.cur_span = d.span;
                    let sig = match self.sigs.get(&d.name) {
                        Some(s) => s.clone(),
                        None => {
                            return Err(self.fail(
                                &format!("`defn {}`", d.name),
                                "it lost its collected signature",
                                "report this — every streamed defn is collected first",
                            ));
                        }
                    };
                    let pairs: Vec<(String, VClass)> = d
                        .params
                        .iter()
                        .zip(sig.params)
                        .map(|(p, c)| (p.name.clone(), c))
                        .collect();
                    let head = RecipeHead {
                        name: &d.name,
                        params: &pairs,
                        ret: sig.ret,
                        stmts: &d.body,
                        span: d.span,
                    };
                    self.recipe_core(head)?;
                }
                Stream::Bootstrap(b) => {
                    self.cur_span = b.span;
                    self.out.push(BadTopLevel::Directive(BadDirective {
                        name: "global".to_string(),
                        args: b.name.clone(),
                        span: b.span,
                    }));
                    let head = RecipeHead {
                        name: &b.name,
                        params: &[],
                        ret: None,
                        stmts: &b.body,
                        span: b.span,
                    };
                    self.recipe_core(head)?;
                }
            }
        }
        Ok(BadProgram {
            items: std::mem::take(&mut self.out),
            span: self.root_span,
        })
    }

    /// Every const is re-emitted as a `.const` directive so `bad { }`
    /// expression operands and data arguments can name it.
    fn emit_const(&mut self, c: &BldConst) -> Result<(), String> {
        let value = match self.consts_done.get(&c.name) {
            Some(v) => *v,
            None => {
                return Err(self.fail(
                    &format!("const `{}`", c.name),
                    "it was never evaluated",
                    "report this — collect evaluates every const before emission",
                ));
            }
        };
        let text = match value {
            ConstVal::Int(n) => n.to_string(),
            ConstVal::Float(f) => float_text(f),
        };
        self.out.push(BadTopLevel::Directive(BadDirective {
            name: ".const".to_string(),
            args: format!("{} {}", c.name, text),
            span: c.span,
        }));
        Ok(())
    }

    /// One recipe = ownership frame + exported label + lowered body.
    fn recipe_core(&mut self, head: RecipeHead<'_>) -> Result<(), String> {
        self.body = Body::new(head.name, head.ret);
        self.bind_params(head.name, head.params)?;
        let stmts = head.stmts;
        let last = stmts.len().saturating_sub(1);
        for (i, st) in stmts.iter().enumerate() {
            let pos = if i == 0 {
                // A single-statement recipe counts as First (the check
                // order makes First win over Last).
                RecipePos::First
            } else if i == last {
                RecipePos::Last
            } else {
                RecipePos::Middle
            };
            self.stmt(st, pos)?;
        }
        let b = std::mem::replace(&mut self.body, Body::new("", None));
        self.out.extend(b.own_first);
        self.out.push(BadTopLevel::Label(BadLabel {
            name: head.name.to_string(),
            local: false,
            contracts: Vec::new(),
            body: b.items,
            span: head.span,
        }));
        self.out.extend(b.trailing);
        Ok(())
    }

    /// Bind each parameter to its ABI register: integer-class parameters
    /// consume the `abi_args` order, float parameters the `abi_args_fp`
    /// order — the class-separate C-ABI convention the call site's
    /// staging mirrors. Exhausting the register budget is loud: stack
    /// parameters need a frame, and .bld stays frameless in v1.
    fn bind_params(&mut self, name: &str, params: &[(String, VClass)]) -> Result<(), String> {
        let abi = ArgRegs::load(self.regs, &self.family);
        let mut cur = ArgCursor::default();
        for (pname, class) in params {
            let reg = match abi.next_slot(*class, cur) {
                Some(r) => r,
                None => {
                    let budget = abi.budget(*class);
                    let arg_no = cur.arg_no() + 1;
                    return Err(self.fail(
                        &format!("`defn {name}`"),
                        &format!(
                            "parameter `{pname}` is {}-class argument #{arg_no}, but `{}` passes only {budget} of that class in registers",
                            class.name(),
                            self.family
                        ),
                        "reduce the parameters, or pack them into a pointer and unpack with a `bad { }` block",
                    ));
                }
            };
            cur.take(*class);
            let binding = Binding { reg: Some(reg), class: *class, konst: None };
            self.scopes_insert(pname, binding)?;
        }
        Ok(())
    }

    // ── statements ────────────────────────────────────────────────────

    fn stmt(&mut self, st: &BldStmt, pos: RecipePos) -> Result<(), String> {
        self.cur_span = stmt_span(st);
        match st {
            BldStmt::Let { name, ty, value, .. } => self.st_let(name, ty.as_ref(), value),
            BldStmt::Assign { target, value, .. } => self.st_assign(target, value),
            BldStmt::Call { callee, args, .. } => {
                let _ = self.lower_call(callee, args)?;
                Ok(())
            }
            BldStmt::Block { body, .. } => {
                self.body.scopes.push(HashMap::new());
                for s in body {
                    self.stmt(s, RecipePos::Middle)?;
                }
                self.body.scopes.pop();
                Ok(())
            }
            BldStmt::When { cond, then, otherwise, .. } => {
                self.st_when(cond, then, otherwise.as_deref())
            }
            BldStmt::Loop { body, .. } => self.st_loop(body),
            BldStmt::While { cond, body, .. } => self.st_while(cond, body),
            BldStmt::Break { .. } => self.st_break(),
            BldStmt::Continue { .. } => self.st_continue(),
            BldStmt::Return { value, .. } => self.st_return(value.as_ref()),
            BldStmt::Bad { text, .. } => self.st_bad(text, pos),
        }
    }

    fn st_let(&mut self, name: &str, ty: Option<&Type>, value: &Expr) -> Result<(), String> {
        let v = self.expr(value)?;
        let v = match ty {
            Some(t) => {
                let want = self.annotation_class(t).map_err(|why| {
                    self.fail(
                        &format!("the annotation on `let {name}`"),
                        &why,
                        "use a scalar class: Int, Float, Bool, or Ptr",
                    )
                })?;
                self.promote(v, want)?
            }
            None => v,
        };
        let class = v.class;
        let binding = match (v.konst, &v.op) {
            (Some(k), _) => Binding { reg: None, class, konst: Some(k) },
            // A freshly emitted single-def value register becomes the
            // binding's storage directly — no second copy.
            (None, BadOperand::Name(reg)) if v.temp => {
                Binding { reg: Some(reg.clone()), class, konst: None }
            }
            (None, _) => {
                let reg = self.fresh_vreg(class);
                self.emit_copy(&reg, v.op.clone(), class)?;
                Binding { reg: Some(reg), class, konst: None }
            }
        };
        self.scopes_insert(name, binding)
    }

    fn st_assign(&mut self, target: &Expr, value: &Expr) -> Result<(), String> {
        let name = match target {
            Expr::Identifier(n) => n.clone(),
            _ => {
                return Err(self.fail(
                    "this assignment",
                    "field and element targets have no .bld form in v1",
                    "compute the address in a `bad { }` block and `store` through it",
                ));
            }
        };
        let binding = match self.lookup(&name) {
            Some(b) => b,
            None => {
                return Err(if self.consts_done.contains_key(&name) {
                    self.fail(
                        &format!("`{name} = …`"),
                        "the name is a `const`, and constants do not change",
                        &format!("bind a fresh name — `let {name}_v = …;` — or drop the `const`"),
                    )
                } else {
                    self.unknown_name(&name)
                });
            }
        };
        let v = self.expr(value)?;
        let v = self.promote(v, binding.class)?;
        match binding.reg {
            Some(reg) => self.emit_copy(&reg, v.op.clone(), binding.class),
            None => {
                // A constant binding gaining storage for the first time.
                let reg = self.fresh_vreg(binding.class);
                self.emit_copy(&reg, v.op.clone(), binding.class)?;
                self.rebind(
                    &name,
                    Binding { reg: Some(reg), class: binding.class, konst: None },
                )
            }
        }
    }

    fn st_when(
        &mut self,
        cond: &Expr,
        then: &[BldStmt],
        otherwise: Option<&[BldStmt]>,
    ) -> Result<(), String> {
        let else_l = self.fresh_label("whe");
        let end = self.fresh_label("whend");
        self.lower_cond(cond, &else_l, false)?;
        for s in then {
            self.stmt(s, RecipePos::Middle)?;
        }
        if otherwise.is_some() {
            self.emit_jmp(&end)?;
        }
        self.push_local(&else_l);
        if let Some(els) = otherwise {
            for s in els {
                self.stmt(s, RecipePos::Middle)?;
            }
            self.push_local(&end);
        }
        Ok(())
    }

    fn st_while(&mut self, cond: &Expr, body: &[BldStmt]) -> Result<(), String> {
        let head = self.fresh_label("wlp");
        let end = self.fresh_label("wlpend");
        self.push_local(&head);
        self.lower_cond(cond, &end, false)?;
        self.body.loops.push((head.clone(), end.clone()));
        for s in body {
            self.stmt(s, RecipePos::Middle)?;
        }
        self.body.loops.pop();
        self.emit_jmp(&head)?;
        self.push_local(&end);
        Ok(())
    }

    fn st_loop(&mut self, body: &[BldStmt]) -> Result<(), String> {
        let head = self.fresh_label("lpe");
        let end = self.fresh_label("lpend");
        self.push_local(&head);
        self.body.loops.push((head.clone(), end.clone()));
        for s in body {
            self.stmt(s, RecipePos::Middle)?;
        }
        self.body.loops.pop();
        self.emit_jmp(&head)?;
        self.push_local(&end);
        Ok(())
    }

    fn st_break(&mut self) -> Result<(), String> {
        let end = match self.body.loops.last() {
            Some((_, end)) => end.clone(),
            None => {
                return Err(self.fail(
                    "`break`",
                    "it sits outside any `loop` or `while`",
                    "remove it, or wrap this code in a `loop { }`",
                ));
            }
        };
        self.emit_jmp(&end)
    }

    fn st_continue(&mut self) -> Result<(), String> {
        let head = match self.body.loops.last() {
            Some((head, _)) => head.clone(),
            None => {
                return Err(self.fail(
                    "`continue`",
                    "it sits outside any `loop` or `while`",
                    "remove it, or wrap this code in a `loop { }`",
                ));
            }
        };
        self.emit_jmp(&head)
    }

    fn st_return(&mut self, value: Option<&Expr>) -> Result<(), String> {
        match (value, self.body.ret) {
            (None, None) => self.emit("ret", Vec::new(), VClass::Int),
            (Some(e), Some(class)) => {
                let v = self.expr(e)?;
                let v = self.promote(v, class)?;
                self.emit_copy(ret_reg(class), v.op.clone(), class)?;
                self.emit("ret", Vec::new(), VClass::Int)
            }
            (Some(_), None) => Err(self.fail(
                &format!("`return …` in `{}`", self.body.name),
                "this recipe declares no return type, so it returns nothing",
                "drop the value, or add `-> <class>` to the signature",
            )),
            (None, Some(class)) => Err(self.fail(
                &format!("`return;` in `{}`", self.body.name),
                &format!(
                    "this recipe returns a {}, so a bare `return;` cannot end it",
                    class.name()
                ),
                &format!("return a {} value, or drop the return type from the signature", class.name()),
            )),
        }
    }

    /// `bad { … }` — verbatim .bad passthrough. The text parses inside a
    /// `_bldwrap:` label, so instruction lines can never orphan: they
    /// splice into the recipe body at any position. Ownership items
    /// (sections, data labels, defns, raw blocks) attach only at the
    /// recipe's first or last statement — .bad ownership is positional.
    fn st_bad(&mut self, text: &str, pos: RecipePos) -> Result<(), String> {
        let wrapped = format!("_bldwrap:\n{text}");
        let prog = parse_bad(&wrapped).map_err(|e| {
            self.fail(
                "this `bad { }` block",
                &format!("its assembly does not parse — inner line {}: {}", e.line, e.message),
                "keep instruction lines together (a directive line splits .bad ownership), or split the block in two",
            )
        })?;
        let mut it = prog.items.into_iter();
        let mut body: Vec<BadBodyItem> = Vec::new();
        let mut own: Vec<BadTopLevel> = Vec::new();
        match it.next() {
            Some(BadTopLevel::Label(l)) if l.name == "_bldwrap" => {
                body = l.body;
                own.extend(it);
            }
            Some(first) => {
                own.push(first);
                own.extend(it);
            }
            None => {}
        }
        if !body.is_empty() {
            self.body.items.extend(body);
        }
        if own.is_empty() {
            return Ok(());
        }
        match pos {
            RecipePos::First => {
                self.body.own_first.extend(own);
                Ok(())
            }
            RecipePos::Last => {
                self.body.trailing.extend(own);
                Ok(())
            }
            RecipePos::Middle => Err(self.fail(
                "this `bad { }` block",
                "it declares owners (sections, data labels, defns, raw blocks), and a mid-recipe owner would steal the instructions after it",
                "move it to the first or last statement of the recipe, or keep only instructions inside it",
            )),
        }
    }

    // ── name scopes ───────────────────────────────────────────────────

    fn lookup(&self, name: &str) -> Option<Binding> {
        for scope in self.body.scopes.iter().rev() {
            if let Some(b) = scope.get(name) {
                return Some(b.clone());
            }
        }
        None
    }

    fn scopes_insert(&mut self, name: &str, b: Binding) -> Result<(), String> {
        if self.body.scopes.last().map_or(false, |s| s.contains_key(name)) {
            return Err(self.fail(
                &format!("`let {name}`"),
                "the name is already bound in this scope",
                "rename the new binding, or drop the first one — shadowing needs a nested block",
            ));
        }
        match self.body.scopes.last_mut() {
            Some(scope) => {
                scope.insert(name.to_string(), b);
                Ok(())
            }
            None => Err(self.fail(
                &format!("`let {name}`"),
                "there is no scope to bind it in",
                "report this — a recipe always opens its parameter scope",
            )),
        }
    }

    /// Overwrite an existing binding in its own scope (assignment to a
    /// formerly-constant name gaining storage).
    fn rebind(&mut self, name: &str, b: Binding) -> Result<(), String> {
        for scope in self.body.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), b);
                return Ok(());
            }
        }
        Err(self.fail(
            &format!("`{name}`"),
            "it is not bound",
            "declare it with `let`",
        ))
    }

    fn unknown_name(&self, name: &str) -> String {
        self.fail(
            &format!("the name `{name}`"),
            "it is not bound in this recipe and names no `const`",
            &format!(
                "declare it with `let {name} = …;`, add a `const {name} = …;`, or compute it in a `bad {{ }}` block"
            ),
        )
    }

    // ── expressions ───────────────────────────────────────────────────

    fn expr(&mut self, e: &Expr) -> Result<Lowered, String> {
        match e {
            Expr::Decimal(n) => Ok(Lowered::konst(ConstVal::Int(*n), VClass::Int)),
            Expr::Float(f) => Ok(Lowered::konst(ConstVal::Float(*f), VClass::Float)),
            Expr::Bool(b) => Ok(Lowered::konst(ConstVal::Int(*b as i64), VClass::Bool)),
            Expr::Char(c) => Ok(Lowered::konst(ConstVal::Int(*c as i64), VClass::Int)),
            Expr::TaggedLiteral(n, _) => Ok(Lowered::konst(ConstVal::Int(*n), VClass::Int)),
            Expr::Identifier(name) => self.ident(name),
            Expr::BinaryOp(BinaryOpKind::And | BinaryOpKind::Or, ..) => self.bool_dance(e, false),
            Expr::BinaryOp(op, l, r) => self.binop(*op, l, r),
            Expr::UnaryOp(UnaryOpKind::Not, _) => self.bool_dance(e, true),
            Expr::UnaryOp(op, inner) => self.unary(*op, inner),
            Expr::Call(callee, args, _) => match self.lower_call(callee, args)? {
                Some(v) => Ok(v),
                None => Err(self.fail(
                    &format!("call to `{callee}`"),
                    "it returns no value, so the call cannot stand in an expression",
                    "call it as a statement, or give the callee a return type",
                )),
            },
            other => Err(self.unsupported_expr(other)),
        }
    }

    fn ident(&mut self, name: &str) -> Result<Lowered, String> {
        if let Some(b) = self.lookup(name) {
            return match (b.reg, b.konst) {
                (Some(reg), _) => Ok(Lowered::reg(&reg, b.class)),
                (None, Some(k)) => Ok(Lowered::konst(k, b.class)),
                (None, None) => Err(self.fail(
                    &format!("the name `{name}`"),
                    "its binding holds neither a register nor a constant",
                    "report this — every binding carries one of the two",
                )),
            };
        }
        match self.consts_done.get(name) {
            Some(k) => Ok(Lowered::konst(
                *k,
                match k {
                    ConstVal::Int(_) => VClass::Int,
                    ConstVal::Float(_) => VClass::Float,
                },
            )),
            None => Err(self.unknown_name(name)),
        }
    }

    fn unary(&mut self, op: UnaryOpKind, inner: &Expr) -> Result<Lowered, String> {
        if op == UnaryOpKind::Not {
            return self.bool_dance(inner, true);
        }
        let v = self.expr(inner)?;
        if let Some(k) = v.konst {
            return self.fold_unary(op, k).map(|r| Lowered::konst(r, result_class(r, v.class)));
        }
        let class = match (op, v.class) {
            (UnaryOpKind::Neg, VClass::Float) => VClass::Float,
            (UnaryOpKind::Neg, VClass::Ptr) => {
                return Err(self.fail(
                    "this negation",
                    "negating a pointer is not a meaningful operation",
                    "negate the integer offset instead",
                ));
            }
            (UnaryOpKind::Neg, _) => VClass::Int,
            (UnaryOpKind::BitNot, VClass::Float) => {
                return Err(self.fail(
                    "this bitwise complement",
                    "bitwise `~` is defined only for Int values",
                    "convert with `ftoi` in a `bad { }` block first",
                ));
            }
            (UnaryOpKind::BitNot | UnaryOpKind::Not, _) => VClass::Int,
        };
        let dst = self.fresh_vreg(class);
        let mn = if class == VClass::Float {
            "fneg"
        } else if op == UnaryOpKind::BitNot {
            "not"
        } else {
            "neg"
        };
        self.emit(mn, vec![BadOperand::Name(dst.clone()), v.op.clone()], class)?;
        Ok(Lowered::temp_reg(dst, class))
    }

    fn binop(&mut self, op: BinaryOpKind, le: &Expr, re: &Expr) -> Result<Lowered, String> {
        let l = self.expr(le)?;
        let r = self.expr(re)?;
        let class = common_class(l.class, r.class);
        let l = self.promote(l, class)?;
        let r = self.promote(r, class)?;
        if let (Some(a), Some(b)) = (l.konst, r.konst) {
            return fold_bin(op, a, b).map(|v| {
                let cls = if matches!(
                    op,
                    BinaryOpKind::Eq
                        | BinaryOpKind::Neq
                        | BinaryOpKind::Lt
                        | BinaryOpKind::Gt
                        | BinaryOpKind::Le
                        | BinaryOpKind::Ge
                ) {
                    VClass::Bool
                } else {
                    result_class(v, class)
                };
                Lowered::konst(v, cls)
            });
        }
        match op {
            BinaryOpKind::Eq
            | BinaryOpKind::Neq
            | BinaryOpKind::Lt
            | BinaryOpKind::Gt
            | BinaryOpKind::Le
            | BinaryOpKind::Ge => self.cmp_value(op, &l, &r),
            BinaryOpKind::And | BinaryOpKind::Or | BinaryOpKind::Concat => {
                Err(self.unsupported_binop(op))
            }
            _ => {
                let mn = self.binop_mnemonic(op, class)?;
                let dst = self.fresh_vreg(class);
                let ops = vec![BadOperand::Name(dst.clone()), l.op.clone(), r.op.clone()];
                self.emit(mn, ops, class)?;
                Ok(Lowered::temp_reg(dst, class))
            }
        }
    }

    fn binop_mnemonic(&self, op: BinaryOpKind, class: VClass) -> Result<&'static str, String> {
        let float = class == VClass::Float;
        let mn = match op {
            BinaryOpKind::Add if float => "fadd",
            BinaryOpKind::Add => "add",
            BinaryOpKind::Sub if float => "fsub",
            BinaryOpKind::Sub => "sub",
            BinaryOpKind::Mul if float => "fmul",
            BinaryOpKind::Mul => "mul",
            BinaryOpKind::Div if float => "fdiv",
            BinaryOpKind::Div => "div",
            BinaryOpKind::Mod if !float => "mod",
            BinaryOpKind::BitAnd if !float => "and",
            BinaryOpKind::BitOr if !float => "or",
            BinaryOpKind::BitXor if !float => "xor",
            BinaryOpKind::Shl if !float => "shl",
            BinaryOpKind::Shr if !float => "shr",
            _ => {
                return Err(self.fail(
                    "this operator",
                    &format!("it is not defined for {} values", class.name()),
                    if float {
                        "convert the operands with `ftoi` in a `bad { }` block, or keep the arithmetic in Int"
                    } else {
                        "check the operand classes — this needs a conversion or a `bad { }` sequence"
                    },
                ));
            }
        };
        Ok(mn)
    }

    fn unsupported_binop(&self, op: BinaryOpKind) -> String {
        let name = match op {
            BinaryOpKind::And => "`&&`",
            BinaryOpKind::Or => "`||`",
            _ => "concatenation",
        };
        self.fail(
            &format!("the operator {name}"),
            "it has no .bld value form here",
            "bind the boolean with `let ok = a && b;` (it lowers to a 0/1 value), and build concatenated data in a `bad { }` block",
        )
    }

    fn unsupported_expr(&self, e: &Expr) -> String {
        let (what, why) = match e {
            Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _) => (
                "a string literal",
                "a .bld value is a machine word — text has no value form",
            ),
            Expr::Field(..) => ("field access", "recipes manipulate machine words, not structures"),
            Expr::Index(..) => ("indexing", "recipes manipulate machine words, not collections"),
            Expr::MethodCall(..) => ("a method call", "methods belong to the .bv tier"),
            Expr::Reflect(..) => ("reflection", "reflection belongs to the .bv tier"),
            Expr::Slice { .. } => ("a slice", "slices belong to the .bv tier"),
            Expr::Cast(..) => (
                "a cast",
                "casts have no .bld form — convert with itof/ftoi in a `bad { }` block",
            ),
            _ => ("this expression", "it has no .bld value form"),
        };
        self.fail(
            what,
            why,
            "compute it in the .bv tier and pass the result in, or write the machine steps in a `bad { }` block",
        )
    }

    /// Bring one value to a target class: constants convert exactly
    /// (a fractional Float → Int is loud), integer-class values convert
    /// through itof/ftoi, and Int/Bool/Ptr interchange as the same
    /// machine word (a class relabel, no instruction).
    fn promote(&mut self, v: Lowered, to: VClass) -> Result<Lowered, String> {
        if v.class == to {
            return Ok(v);
        }
        match (v.class, to) {
            (_, VClass::Float) => match v.konst {
                Some(ConstVal::Int(n)) => {
                    Ok(Lowered::konst(ConstVal::Float(n as f64), VClass::Float))
                }
                _ => {
                    let dst = self.fresh_vreg(VClass::Float);
                    let ops = vec![BadOperand::Name(dst.clone()), v.op.clone()];
                    self.emit("itof", ops, VClass::Int)?;
                    Ok(Lowered::temp_reg(dst, VClass::Float))
                }
            },
            (VClass::Float, _) => match v.konst {
                Some(ConstVal::Float(f)) => {
                    if f.trunc() != f {
                        return Err(self.fail(
                            &format!("the constant {f}"),
                            "it has a fractional part where an integer value is required",
                            "use an integer constant, or keep the value Float",
                        ));
                    }
                    Ok(Lowered::konst(ConstVal::Int(f as i64), to))
                }
                _ => {
                    let dst = self.fresh_vreg(VClass::Int);
                    let ops = vec![BadOperand::Name(dst.clone()), v.op.clone()];
                    self.emit("ftoi", ops, VClass::Float)?;
                    Ok(Lowered::temp_reg(dst, to))
                }
            },
            _ => {
                let mut relabeled = v;
                relabeled.class = to;
                Ok(relabeled)
            }
        }
    }

    /// Materialize a boolean VALUE from a condition: 1 when it holds,
    /// 0 when it doesn't. `jump_when` is the polarity lower_cond jumps
    /// on — `let b = !a` dances on the whole Not node with jump_when
    /// = true, so the 0 branch fires when `a` holds.
    fn bool_dance(&mut self, e: &Expr, jump_when: bool) -> Result<Lowered, String> {
        let saved = (self.cur_span, self.cur_file.clone());
        let folded = self.eval_const_expr(e);
        self.cur_span = saved.0;
        self.cur_file = saved.1;
        if let Ok(k) = folded {
            return Ok(Lowered::konst(
                ConstVal::Int(Lowered::truthy(k) as i64),
                VClass::Bool,
            ));
        }
        let dst = self.fresh_vreg(VClass::Bool);
        let lf = self.fresh_label("bf");
        let le = self.fresh_label("be");
        self.emit_copy(&dst, BadOperand::Int(1), VClass::Bool)?;
        self.lower_cond(e, &lf, jump_when)?;
        self.emit_jmp(&le)?;
        self.push_local(&lf);
        self.emit_copy(&dst, BadOperand::Int(0), VClass::Bool)?;
        self.push_local(&le);
        Ok(Lowered::temp_reg(dst, VClass::Bool))
    }

    // ── conditions ────────────────────────────────────────────────────

    /// Branch to `target` exactly when `e ≡ when_true`. `when` and
    /// `while` always pass when_true = false (jump past me when false);
    /// the bool dance passes the dance polarity. Fall-through means
    /// "not taken".
    fn lower_cond(&mut self, e: &Expr, target: &str, when_true: bool) -> Result<(), String> {
        match e {
            Expr::BinaryOp(
                op @ (BinaryOpKind::Eq
                | BinaryOpKind::Neq
                | BinaryOpKind::Lt
                | BinaryOpKind::Gt
                | BinaryOpKind::Le
                | BinaryOpKind::Ge),
                l,
                r,
            ) => self.lower_cond_cmp(e, target, when_true),
            Expr::BinaryOp(BinaryOpKind::And, l, r) => {
                if when_true {
                    // Jump only when BOTH hold: a false left side skips
                    // straight past the right side's test.
                    let skip = self.fresh_label("and");
                    self.lower_cond(l, &skip, false)?;
                    self.lower_cond(r, target, true)?;
                    self.push_local(&skip);
                } else {
                    self.lower_cond(l, target, false)?;
                    self.lower_cond(r, target, false)?;
                }
                Ok(())
            }
            Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
                if when_true {
                    self.lower_cond(l, target, true)?;
                    self.lower_cond(r, target, true)?;
                } else {
                    // Both must fail before jumping: a true left side
                    // lands past the right side's test.
                    let skip = self.fresh_label("or");
                    self.lower_cond(l, &skip, true)?;
                    self.lower_cond(r, target, false)?;
                    self.push_local(&skip);
                }
                Ok(())
            }
            Expr::UnaryOp(UnaryOpKind::Not, inner) => self.lower_cond(inner, target, !when_true),
            _ => self.lower_cond_value(e, target, when_true),
        }
    }

    fn lower_cond_cmp(&mut self, e: &Expr, target: &str, when_true: bool) -> Result<(), String> {
        let Expr::BinaryOp(op, l, r) = e else {
            return Err(self.fail(
                "this condition",
                "it is not a comparison",
                "compare two values with ==, !=, <, >, <=, or >=",
            ));
        };
        let op = *op;
        let (lv, rv, class) = self.prep_cmp(l, r)?;
        if let (Some(a), Some(b)) = (lv.konst, rv.konst) {
            let truth = Lowered::truthy(fold_bin(op, a, b)?);
            return self.cond_const(truth, target, when_true);
        }
        let br = branch_op(op, when_true, class == VClass::Float)
            .ok_or_else(|| self.unsupported_binop(op))?;
        let lhs = self.branch_operand(&lv)?;
        let rhs = self.branch_operand(&rv)?;
        self.emit_branch(br, lhs, rhs, target)
    }

    /// Evaluate and class-unify the two sides of a comparison.
    fn prep_cmp(&mut self, l: &Expr, r: &Expr) -> Result<(Lowered, Lowered, VClass), String> {
        let lv = self.expr(l)?;
        let rv = self.expr(r)?;
        let class = common_class(lv.class, rv.class);
        let lv = self.promote(lv, class)?;
        let rv = self.promote(rv, class)?;
        Ok((lv, rv, class))
    }

    fn lower_cond_value(&mut self, e: &Expr, target: &str, when_true: bool) -> Result<(), String> {
        let v = self.expr(e)?;
        if let Some(k) = v.konst {
            return self.cond_const(Lowered::truthy(k), target, when_true);
        }
        let br = match (when_true, v.class == VClass::Float) {
            (true, false) => "jnz",
            (false, false) => "jz",
            (true, true) => "fjnz",
            (false, true) => "fjz",
        };
        // The fj* rows have no immediate form, so the compared-to zero
        // rides a float register on that class.
        let rhs = if v.class == VClass::Float {
            let zv = self.fresh_vreg(VClass::Float);
            self.emit_copy(&zv, BadOperand::Float("0.0".to_string()), VClass::Float)?;
            BadOperand::Name(zv)
        } else {
            BadOperand::Int(0)
        };
        self.emit_branch(br, v.op.clone(), rhs, target)
    }

    fn cond_const(&mut self, truth: bool, target: &str, when_true: bool) -> Result<(), String> {
        if truth == when_true {
            self.emit_jmp(target)
        } else {
            Ok(())
        }
    }

    /// A compare/branch operand. Float constants materialize into float
    /// registers first — the fj* rows have no immediate form, and
    /// substituting a pooled literal into a register slot mis-assembles.
    fn branch_operand(&mut self, v: &Lowered) -> Result<BadOperand, String> {
        match v {
            Lowered { class: VClass::Float, konst: Some(_), .. } => {
                let dst = self.fresh_vreg(VClass::Float);
                self.emit_copy(&dst, v.op.clone(), VClass::Float)?;
                Ok(BadOperand::Name(dst))
            }
            other => Ok(other.op.clone()),
        }
    }

    /// A comparison as a VALUE (0/1). Integer orderings lower to `slt`
    /// (operand swap / xor-1 for the flipped polarities); equality and
    /// every float comparison ride the branch form as a bool dance.
    fn cmp_value(&mut self, op: BinaryOpKind, l: &Lowered, r: &Lowered) -> Result<Lowered, String> {
        if l.class != VClass::Float {
            let dst = self.fresh_vreg(VClass::Bool);
            match op {
                BinaryOpKind::Lt | BinaryOpKind::Ge => {
                    self.emit("slt", vec![BadOperand::Name(dst.clone()), l.op.clone(), r.op.clone()], VClass::Int)?;
                }
                BinaryOpKind::Gt | BinaryOpKind::Le => {
                    self.emit("slt", vec![BadOperand::Name(dst.clone()), r.op.clone(), l.op.clone()], VClass::Int)?;
                }
                _ => return self.cmp_dance(op, l, r, false),
            }
            if matches!(op, BinaryOpKind::Le | BinaryOpKind::Ge) {
                let ops = vec![
                    BadOperand::Name(dst.clone()),
                    BadOperand::Name(dst.clone()),
                    BadOperand::Int(1),
                ];
                self.emit("xor", ops, VClass::Int)?;
            }
            return Ok(Lowered::temp_reg(dst, VClass::Bool));
        }
        self.cmp_dance(op, l, r, true)
    }

    fn cmp_dance(
        &mut self,
        op: BinaryOpKind,
        l: &Lowered,
        r: &Lowered,
        is_float: bool,
    ) -> Result<Lowered, String> {
        let dst = self.fresh_vreg(VClass::Bool);
        let lf = self.fresh_label("bf");
        let le = self.fresh_label("be");
        let br = branch_op(op, false, is_float).ok_or_else(|| self.unsupported_binop(op))?;
        let lhs = self.branch_operand(l)?;
        let rhs = self.branch_operand(r)?;
        self.emit_copy(&dst, BadOperand::Int(1), VClass::Bool)?;
        self.emit_branch(br, lhs, rhs, &lf)?;
        self.emit_jmp(&le)?;
        self.push_local(&lf);
        self.emit_copy(&dst, BadOperand::Int(0), VClass::Bool)?;
        self.push_local(&le);
        Ok(Lowered::temp_reg(dst, VClass::Bool))
    }

    // ── calls ─────────────────────────────────────────────────────────

    fn lower_call(&mut self, callee: &str, args: &[Expr]) -> Result<Option<Lowered>, String> {
        if callee.contains('.') {
            return Err(self.fail(
                &format!("call to `{callee}`"),
                "a .bld call names the function directly — there are no module paths in calls",
                "call the bare name; its `import \"…\";` already brought it in",
            ));
        }
        let sig = match self.sigs.get(callee) {
            Some(s) => s.clone(),
            None => {
                return Err(self.fail(
                    &format!("call to `{callee}`"),
                    "no `defn` or imported label provides it",
                    &format!(
                        "declare `defn {callee}(…) {{ … }}` in this file, or write the sequence in a `bad {{ }}` block"
                    ),
                ));
            }
        };
        if sig.check_arity && args.len() != sig.params.len() {
            return Err(self.fail(
                &format!("call to `{callee}`"),
                &format!("it takes {} argument(s), got {}", sig.params.len(), args.len()),
                &format!("match the `defn {callee}` signature"),
            ));
        }
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.expr(a)?);
        }
        if !sig.external {
            for i in 0..vals.len() {
                let want = sig.params[i];
                vals[i] = self.promote(vals[i].clone(), want)?;
            }
        }
        self.stage_args(&vals, callee)?;
        let call_ops = vec![BadOperand::Name(callee.to_string())];
        self.emit("call", call_ops, VClass::Int)?;
        Ok(match sig.ret {
            None => None,
            Some(class) => {
                let dst = self.fresh_vreg(class);
                let src = BadOperand::Name(ret_reg(class).to_string());
                self.emit_copy(&dst, src, class)?;
                Some(Lowered::temp_reg(dst, class))
            }
        })
    }

    /// Parallel-move-safe argument staging: every register-passed value
    /// first lands in a fresh virtual, THEN the virtuals copy into the
    /// ABI registers — a direct value→ABI copy could clobber a still
    /// needed source (`f(b, a)` with a and b already in argument regs).
    fn stage_args(&mut self, vals: &[Lowered], who: &str) -> Result<(), String> {
        let abi = ArgRegs::load(self.regs, &self.family);
        let staged = self.stage_each(vals, who, &abi)?;
        self.copy_staged_out(&staged, &abi)
    }

    /// Phase 1: land every register-passed value in a fresh virtual.
    fn stage_each(
        &mut self,
        vals: &[Lowered],
        who: &str,
        abi: &ArgRegs,
    ) -> Result<Vec<(VClass, String)>, String> {
        let mut staged: Vec<(VClass, String)> = Vec::with_capacity(vals.len());
        let mut cur = ArgCursor::default();
        for v in vals {
            let reg = match abi.next_slot(v.class, cur) {
                Some(r) => r,
                None => {
                    return Err(self.arg_budget_error(who, v.class, &cur, abi));
                }
            };
            let dst = self.fresh_vreg(v.class);
            self.emit_copy(&dst, v.op.clone(), v.class)?;
            staged.push((v.class, dst));
            cur.take(v.class);
        }
        Ok(staged)
    }

    fn arg_budget_error(&self, who: &str, class: VClass, cur: &ArgCursor, abi: &ArgRegs) -> String {
        self.fail(
            &format!("call to `{who}`"),
            &format!(
                "argument #{} needs a {}-class register, but `{}` passes only {} of that class",
                cur.arg_no() + 1,
                class.name(),
                self.family,
                abi.budget(class)
            ),
            "reduce the arguments, or pack them into a pointer and unpack with a `bad { }` block",
        )
    }

    /// Phase 2: copy the staged virtuals into the ABI argument registers.
    fn copy_staged_out(&mut self, staged: &[(VClass, String)], abi: &ArgRegs) -> Result<(), String> {
        let mut cur = ArgCursor::default();
        for (class, reg) in staged {
            let target = match abi.next_slot(*class, cur) {
                Some(t) => t,
                None => {
                    return Err(self.fail(
                        "this call",
                        "a staged argument lost its register slot",
                        "report this — staging checked the budget before staging",
                    ));
                }
            };
            cur.take(*class);
            let src = BadOperand::Name(reg.clone());
            self.emit_copy(&target, src, *class)?;
        }
        Ok(())
    }

    // ── emission infrastructure ───────────────────────────────────────

    /// Emit one instruction through the ISA registry: a missing target
    /// row is a loud capability error, arity is checked, and an
    /// immediate feeding a no-immediate op materializes into a fresh
    /// value register first (`mul` on aarch64, every float op).
    fn emit(&mut self, mn: &str, mut ops: Vec<BadOperand>, class: VClass) -> Result<(), String> {
        let row: &BadIsaLowering = match self.isa.lookup(mn, &self.family) {
            Some(r) => r,
            None => {
                return Err(self.fail(
                    &format!("the `{mn}` instruction"),
                    &format!("the `{}` target has no row for it", self.family),
                    &format!(
                        "write this step in a `bad {{ }}` block for `{}`, or drop it from the recipe",
                        self.family
                    ),
                ));
            }
        };
        let arity = self.isa.arity(mn).unwrap_or(ops.len());
        if ops.len() != arity {
            return Err(self.fail(
                &format!("the `{mn}` instruction"),
                &format!("it takes {arity} operand(s), got {}", ops.len()),
                "report this — the lowering emits registry-shaped operands",
            ));
        }
        if matches!(row.imm, ImmHandling::Illegal)
            && ops.iter().any(|o| matches!(o, BadOperand::Int(_) | BadOperand::Float(_)))
        {
            for slot in ops.iter_mut() {
                if matches!(slot, BadOperand::Int(_) | BadOperand::Float(_)) {
                    let reg = self.fresh_vreg(class);
                    self.emit_copy(&reg, slot.clone(), class)?;
                    *slot = BadOperand::Name(reg);
                }
            }
        }
        self.push_instr(mn, ops);
        Ok(())
    }

    fn push_instr(&mut self, mn: &str, ops: Vec<BadOperand>) {
        self.body.items.push(BadBodyItem::Instr(BadInstr {
            mnemonic: mn.to_string(),
            operands: ops,
            contract: None,
            ack: None,
            span: self.cur_span,
        }));
    }

    fn emit_copy(&mut self, dst: &str, src: BadOperand, class: VClass) -> Result<(), String> {
        let mn = if class == VClass::Float { "fmov" } else { "mov" };
        let ops = vec![BadOperand::Name(dst.to_string()), src];
        self.emit(mn, ops, class)
    }

    fn emit_jmp(&mut self, target: &str) -> Result<(), String> {
        let ops = vec![BadOperand::Name(target.to_string())];
        self.emit("jmp", ops, VClass::Int)
    }

    fn emit_branch(
        &mut self,
        br: &str,
        lhs: BadOperand,
        rhs: BadOperand,
        target: &str,
    ) -> Result<(), String> {
        let ops = vec![lhs, rhs, BadOperand::Name(target.to_string())];
        self.emit(br, ops, VClass::Int)
    }

    /// Plant a local label at the current position; the name carries the
    /// leading dot (the .bad local form).
    fn fresh_label(&mut self, prefix: &str) -> String {
        let n = self.body.seq;
        self.body.seq += 1;
        format!(".{prefix}_{n}")
    }

    fn push_local(&mut self, dotted: &str) {
        self.body.items.push(BadBodyItem::Local(BadLocal {
            name: dotted.trim_start_matches('.').to_string(),
            span: self.cur_span,
        }));
    }

    /// Allocate a fresh virtual value register. The class rides the name
    /// (`fv` = float) so the M4 allocator reads it straight off the
    /// emitted BadProgram without extra state.
    fn fresh_vreg(&mut self, class: VClass) -> String {
        let n = self.body.vreg;
        self.body.vreg += 1;
        if class == VClass::Float {
            format!("fv{n}")
        } else {
            format!("v{n}")
        }
    }
}

/// The class two operands unify to: any Float pulls both to Float;
/// otherwise any Ptr keeps pointer arithmetic; Bool folds into Int.
fn common_class(a: VClass, b: VClass) -> VClass {
    if a == VClass::Float || b == VClass::Float {
        return VClass::Float;
    }
    if a == VClass::Ptr || b == VClass::Ptr {
        return VClass::Ptr;
    }
    VClass::Int
}

/// The fused compare-branch mnemonic: jump when `a OP b` matches
/// `when_true`. Dual pairs keep the polarity table explicit — no
/// arithmetic inversion to misread.
fn branch_op(op: BinaryOpKind, when_true: bool, is_float: bool) -> Option<&'static str> {
    let (i, f) = match op {
        BinaryOpKind::Eq => ("jz", "fjz"),
        BinaryOpKind::Neq => ("jnz", "fjnz"),
        BinaryOpKind::Lt => ("jlt", "fjlt"),
        BinaryOpKind::Gt => ("jgt", "fjgt"),
        BinaryOpKind::Le => ("jle", "fjle"),
        BinaryOpKind::Ge => ("jge", "fjge"),
        _ => return None,
    };
    let chosen = if is_float { f } else { i };
    if when_true {
        Some(chosen)
    } else {
        dual(chosen)
    }
}

fn dual(br: &str) -> Option<&'static str> {
    match br {
        "jz" => Some("jnz"),
        "jnz" => Some("jz"),
        "jlt" => Some("jge"),
        "jge" => Some("jlt"),
        "jle" => Some("jgt"),
        "jgt" => Some("jle"),
        "fjz" => Some("fjnz"),
        "fjnz" => Some("fjz"),
        "fjlt" => Some("fjge"),
        "fjge" => Some("fjlt"),
        "fjle" => Some("fjgt"),
        "fjgt" => Some("fjle"),
        _ => None,
    }
}

/// The return-register convention of the .bad ABI: r0 (mapped to the
/// architecture's integer return register — %rax/x0/a0) or f0 for float.
fn ret_reg(class: VClass) -> &'static str {
    if class == VClass::Float {
        "f0"
    } else {
        "r0"
    }
}

/// The text form of a float constant — the .bad float pool keys on this
/// exact text, so one spelling per value keeps the pool deduped.
fn float_text(f: f64) -> String {
    format!("{f}")
}

/// Class of a folded result: floats stay float; integer results keep the
/// operand class (Bool/Ptr relabel to their machine word).
fn result_class(v: ConstVal, fallback: VClass) -> VClass {
    match v {
        ConstVal::Float(_) => VClass::Float,
        ConstVal::Int(_) => fallback,
    }
}

/// Fold a binary op over two same-class constants. Integer division by
/// zero, overflow, and out-of-range shifts are loud — a folded wrong
/// value is worse than a refused compile.
fn fold_bin(op: BinaryOpKind, a: ConstVal, b: ConstVal) -> Result<ConstVal, String> {
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => fold_int(op, x, y),
        (ConstVal::Float(x), ConstVal::Float(y)) => fold_float(op, x, y),
        _ => Err("this operation mixes a Float with an integer constant".to_string()),
    }
}

fn fold_int(op: BinaryOpKind, x: i64, y: i64) -> Result<ConstVal, String> {
    use BinaryOpKind::*;
    let overflow = |what: String| format!("{what} overflows Int - shrink the operands");
    let v = match op {
        Add => x.checked_add(y).map(ConstVal::Int).ok_or_else(|| overflow(format!("{x} + {y}")))?,
        Sub => x.checked_sub(y).map(ConstVal::Int).ok_or_else(|| overflow(format!("{x} - {y}")))?,
        Mul => x.checked_mul(y).map(ConstVal::Int).ok_or_else(|| overflow(format!("{x} * {y}")))?,
        Div => {
            if y == 0 {
                return Err(format!("{x} / 0 divides by zero - guard it at run time or fix the constant"));
            }
            x.checked_div(y)
                .map(ConstVal::Int)
                .ok_or_else(|| overflow(format!("{x} / {y}")))?
        }
        Mod => {
            if y == 0 {
                return Err(format!("{x} % 0 divides by zero - guard it at run time or fix the constant"));
            }
            x.checked_rem(y)
                .map(ConstVal::Int)
                .ok_or_else(|| overflow(format!("{x} % {y}")))?
        }
        BitAnd => ConstVal::Int(x & y),
        BitOr => ConstVal::Int(x | y),
        BitXor => ConstVal::Int(x ^ y),
        Shl => {
            if !(0..64).contains(&y) {
                return Err(format!("the shift amount {y} is outside 0..63"));
            }
            ConstVal::Int(x.wrapping_shl(y as u32))
        }
        Shr => {
            if !(0..64).contains(&y) {
                return Err(format!("the shift amount {y} is outside 0..63"));
            }
            // Logical shift right — the .bad `shr` is the logical form
            // on every target (shrq/lsr/srl).
            ConstVal::Int(((x as u64) >> (y as u32)) as i64)
        }
        Eq => ConstVal::Int((x == y) as i64),
        Neq => ConstVal::Int((x != y) as i64),
        Lt => ConstVal::Int((x < y) as i64),
        Gt => ConstVal::Int((x > y) as i64),
        Le => ConstVal::Int((x <= y) as i64),
        Ge => ConstVal::Int((x >= y) as i64),
        And => ConstVal::Int((x != 0 && y != 0) as i64),
        Or => ConstVal::Int((x != 0 || y != 0) as i64),
        Concat => {
            return Err("concatenation has no constant form - build the data in a `bad { }` block".to_string());
        }
    };
    Ok(v)
}

fn fold_float(op: BinaryOpKind, x: f64, y: f64) -> Result<ConstVal, String> {
    use BinaryOpKind::*;
    let v = match op {
        // y == 0.0 yields ±inf — the hardware behavior too.
        Add => ConstVal::Float(x + y),
        Sub => ConstVal::Float(x - y),
        Mul => ConstVal::Float(x * y),
        Div => ConstVal::Float(x / y),
        Eq => ConstVal::Int((x == y) as i64),
        Neq => ConstVal::Int((x != y) as i64),
        Lt => ConstVal::Int((x < y) as i64),
        Gt => ConstVal::Int((x > y) as i64),
        Le => ConstVal::Int((x <= y) as i64),
        Ge => ConstVal::Int((x >= y) as i64),
        _ => {
            return Err(
                "this operator is not defined for Float constants - it needs integer values or a `bad { }` sequence"
                    .to_string(),
            );
        }
    };
    Ok(v)
}

/// The head-token span of a statement — diagnostics point here.
fn stmt_span(st: &BldStmt) -> Span {
    match st {
        BldStmt::Let { span, .. }
        | BldStmt::Assign { span, .. }
        | BldStmt::Call { span, .. }
        | BldStmt::Block { span, .. }
        | BldStmt::When { span, .. }
        | BldStmt::Loop { span, .. }
        | BldStmt::While { span, .. }
        | BldStmt::Break { span }
        | BldStmt::Continue { span }
        | BldStmt::Return { span, .. }
        | BldStmt::Bad { span, .. } => *span,
    }
}
