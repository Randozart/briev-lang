// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Runtime Exception for Use as a Language:
// When the Work or any Derivative Work thereof is used to generate code
// ("generated code"), such generated code shall not be subject to the
// terms of this License, provided that the generated code itself is not
// a Derivative Work of the Work. This exception does not apply to code
// that is itself a compiler, interpreter, or similar tool that incorporates
// or embeds the Work.

use crate::ast::{Expr, Import, ImportKind, StageKind, TopLevel, Type};
use crate::conformance::{classify, SourceKind};
use crate::dbriev::v2 as dbriev_v2;
use crate::lexer::Token;
use logos::Logos;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// 2026-07-21: Prelude is now a system plugin (plugins/parsed/prelude.bv) that
// runs at the $(Parsed) stage via the AST navigation DSL (Tag$ + Insert$).
// 2026-07-15: Removed hardcoded prelude injection.
// Removed fields: use_stdlib, core_imported. Removed method: with_use_stdlib.

/// Load the module registry from config/module-registry.toml (or its Data
/// Briev form module-registry.dbvl — Phase 3, 2026-08-03).
/// When the file doesn't exist or can't be parsed, returns an empty map
/// so that Registry imports fall back to literal filesystem resolution.
/// 2026-07-15: Phase 7i
fn load_module_registry() -> HashMap<String, String> {
    crate::dbriev::config_db::load_string_registry(Path::new("config"), "module-registry")
}

/// 2026-09-25 (interop Wave 1, plan `2026-09-25-interop-wave1.md` C1): the
/// provenance record for one loaded module — every file-backed load (Briev
/// `.bv`, electronics `.ebv`, CSS, SVG, DBriev `.dbv`) registers exactly one
/// record, appended once (cache hits reuse the record's id, so a diamond
/// import shares a single id across both splice sites).
///
/// `item_origins` on the resolver is the parallel shadow vector: index i
/// carries `Some(module id)` when items[i] came from a module and `None`
/// when it is a root-file item or generated (e.g. `import "target"` board
/// items are synthesized from board DBriev tables, not one source file).
///
/// Consumers: the cross-dialect collision rule (Wave 1 C4) needs to know
/// WHICH module a colliding name came from, and per-module dialect semantics
/// (C3) key off `kind`. Zero behavior change on its own — nothing reads the
/// fields until C3/C4 land.
///
/// How to undo: delete `ModuleRecord`, the two resolver fields,
/// `push_module`, and the origins threading (every mirror mutation pairs
/// with an items mutation — see `resolve_imports_inner`).
#[derive(Debug, Clone)]
pub struct ModuleRecord {
    pub specifier: String,
    pub source_path: PathBuf,
    pub kind: Option<SourceKind>,
}

pub struct ImportResolver {
    loaded_modules: HashMap<String, (Vec<TopLevel>, Vec<String>, Vec<Option<u32>>)>,
    search_paths: Vec<PathBuf>,
    root_path: PathBuf,
    stdlib_path: Option<PathBuf>,
    /// Board name for `import "target"` resolution (e.g., "stm32f407").
    board_name: Option<String>,
    // 2026-07-01: Cycle detection for import resolution.
    // Tracks path strings currently being resolved to detect A→B→A cycles.
    in_progress: HashSet<String>,
    /// Registry mapping from module names to filesystem paths.
    /// Loaded from config/module-registry.toml or hardcoded fallback.
    /// 2026-07-15: Phase 7i — import <name> resolution.
    registry: HashMap<String, String>,
    /// 2026-08-09 (Phase 11, Slice 2): the deterministic resolution record —
    /// (import specifier → canonical resolved path), in source order. SPEC
    /// §7.1 requires resolution to be deterministic AND to record the resolved
    /// path; this is the audit trail (reproducible builds, diagnostics).
    pub resolved_paths: Vec<(String, String)>,
    /// 2026-09-22 (per-arch stdlib boot entries): resolved absolute paths
    /// of `.bad` imports at the `.bv` top level. The resolver records them
    /// (a `.bad` file is never parsed as Briev); the bad backend inlines
    /// them when compiling `bootstrap bad` bodies.
    pub bad_imports: Vec<PathBuf>,
    /// 2026-09-25 (interop Wave 1 C1): append-only provenance table —
    /// index = module id. See `ModuleRecord` for the full rationale.
    pub modules: Vec<ModuleRecord>,
    /// 2026-09-25 (interop Wave 1 C1): parallel to the FINAL items vector
    /// returned by `resolve_imports` — `None` = root-file or generated item,
    /// `Some(id)` = `modules[id]`. Kept in lockstep with every items
    /// mutation (remove/splice/dedup/filter) inside `resolve_imports_inner`.
    pub item_origins: Vec<Option<u32>>,
    /// 2026-09-25 (interop Wave 1 C3): per-module dialect prelude. Given a
    /// resolved module path, build the scoped PluginManager whose
    /// Parsed-stage plugins run on the module AFTER parse and BEFORE nested
    /// resolution (`resolve_import`), so the `Import$` anchors its prelude
    /// splices are resolved by the inner walk — exactly the stage order a
    /// root file gets (`pipeline::compile_to_typed` runs Parsed, then
    /// resolves). Built from the ROOT's BuildOptions via
    /// `pipeline::module_plugin_factory`, so a module gets the per-extension
    /// treatment `config/targets.dbvl` gives a root file of its dialect
    /// (including `--no-std`'s prelude family and CLI enable/disable).
    /// `None` = modules parse plain (pre-C3 behavior; `library::parse_and_check`
    /// keeps it — that path runs no plugin stages even at the root).
    /// How to undo: drop this field, the `run_ast` block in `resolve_import`,
    /// and `pipeline::module_plugin_factory`.
    pub plugin_factory: Option<Box<dyn Fn(&str) -> Result<crate::plugin::PluginManager, String>>>,
}

/// The name of a top-level item, if it carries one.
fn item_name(item: &TopLevel) -> Option<&str> {
    match item {
        TopLevel::Definition(d) => Some(d.name.as_str()),
        TopLevel::Signature(s) => Some(s.name.as_str()),
        TopLevel::ForeignBinding(fb) => Some(fb.effective_briev_name()),
        TopLevel::Transaction(t) => Some(t.name.as_str()),
        TopLevel::Constant(c) => Some(c.name.as_str()),
        TopLevel::Obj(s) => Some(s.name.as_str()),
        TopLevel::TypeDef(t) => Some(t.name.as_str()),
        TopLevel::Trait(t) => Some(t.name.as_str()),
        TopLevel::Impl(i) => Some(i.target.as_str()),
        TopLevel::StaticStruct(s) => Some(s.name.as_str()),
        TopLevel::StateDecl(s) => Some(s.name.as_str()),
        TopLevel::Trigger(trg) => Some(trg.name.as_str()),
        TopLevel::TriggerBinding { name, .. } => Some(name.as_str()),
        TopLevel::Cell(c) => Some(c.name.as_str()),
        // 2026-09-11 (library globals): a top-level `let` is a named state
        // item — it imports (and lands in the program's state struct) like
        // any other named declaration. Previously dropped, which made
        // library-level mutable state impossible (the fasta buffered-stdout
        // gap, BUGS.md 2026-09-11).
        TopLevel::Statement(s) => match s.as_ref() {
            crate::ast::Statement::Let { name, .. } => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// The item's name as an owned String (for HashSet membership).
fn top_level_name(item: &TopLevel) -> Option<String> {
    item_name(item).map(|s| s.to_string())
}

/// The Custom/Applied type names referenced by an item's slots/fields.
/// 2026-08-01 (D3): used by the named-import dependency closure — `List`
/// references `ListBuffer<T>` in its slots, which must be imported too.
fn referenced_type_names(item: &TopLevel) -> Vec<String> {
    fn type_names(ty: &crate::ast::Type, acc: &mut Vec<String>) {
        match ty {
            crate::ast::Type::Custom(n) => acc.push(n.clone()),
            crate::ast::Type::Applied(n, args) => {
                acc.push(n.clone());
                for a in args {
                    type_names(a, acc);
                }
            }
            crate::ast::Type::Ptr(i) | crate::ast::Type::PtrConst(i) => type_names(i, acc),
            crate::ast::Type::Vector(i, _) => type_names(i, acc),
            crate::ast::Type::Tuple(elems) => {
                for e in elems {
                    type_names(e, acc);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    match item {
        TopLevel::TypeDef(td) => {
            for s in &td.body.slots {
                type_names(&s.ty, &mut out);
            }
            for m in &td.body.members {
                let params: Vec<&(String, crate::ast::Type)> = match m {
                    TopLevel::Transaction(t) => t.parameters.iter().collect(),
                    TopLevel::Definition(t) => t.parameters.iter().collect(),
                    _ => Vec::new(),
                };
                for (_, ty) in params {
                    type_names(ty, &mut out);
                }
                // 2026-08-18 (check/build divergence): a member's RETURN type is
                // a reference too. `import { HashMap }` from collections.bv
                // dropped `List` because HashMap's `keys()`/`values()`/`entries()`
                // return it — only their PARAMS were walked. The dropped type
                // then fell back to the typechecker's name-based `List`
                // special-case with NO members, so the generic scans
                // (`acc <- keys[i]`) failed `push_element_type` and `brievc
                // check` over-reported ("expected List<K> for arrow assignment,
                // found K"). `brievc build` masked it via a second resolution
                // pass. Walk output types so an imported collection brings its
                // returned collection types.
                let outputs: Vec<crate::ast::Type> = match m {
                    TopLevel::Definition(d) => {
                        let mut v: Vec<crate::ast::Type> = Vec::new();
                        if let Some(ot) = &d.output_type {
                            v.extend(ot.all_types());
                        }
                        v
                    }
                    TopLevel::Transaction(t) => {
                        let mut v: Vec<crate::ast::Type> = Vec::new();
                        if let Some(ot) = &t.output_type {
                            v.extend(ot.all_types());
                        }
                        v
                    }
                    _ => Vec::new(),
                };
                for ty in &outputs {
                    type_names(ty, &mut out);
                }
            }
        }
        TopLevel::StaticStruct(sd) => {
            for (_, ty) in &sd.fields {
                type_names(ty, &mut out);
            }
        }
        // 2026-08-23 (enum construction follow-up): top-level DEFINITIONS
        // and TRANSACTIONS were never walked — `import { is_ok } from
        // "std/result"` dropped the Result TYPEDEF because is_ok's PARAM
        // type (`r: Result<T,E>`) never entered refs; every constructor or
        // match on the dropped enum then failed open-scrutinee.
        TopLevel::Definition(d) => {
            for (_, ty) in &d.parameters {
                type_names(ty, &mut out);
            }
            if let Some(ot) = &d.output_type {
                for t in ot.all_types() {
                    type_names(&t, &mut out);
                }
            }
        }
        TopLevel::Transaction(t) => {
            for (_, ty) in &t.parameters {
                type_names(ty, &mut out);
            }
            if let Some(ot) = &t.output_type {
                for ty in ot.all_types() {
                    type_names(&ty, &mut out);
                }
            }
        }
        _ => {}
    }
    out
}

/// 2026-08-16 (Phase 3c): the NAMED functions/txns a kept item CALLS in its
/// body. A named import (`import { iter_map } from "std/iterator.bv"`) must
/// also bring `iter_map_loop` (the helper txn iter_map's body calls) — the
/// existing closure only pulled referenced TYPE names, so a generic adapter
/// body referencing its `_loop` sibling resolved to the raw-type fallback
/// (the call's return became Int, and the body failed to typecheck:
/// "expected Bool for term value ... found Int").
fn referenced_function_names(item: &TopLevel) -> Vec<String> {
    fn expr_calls(e: &crate::ast::Expr, acc: &mut Vec<String>) {
        match e {
            crate::ast::Expr::Call(name, args, _) => {
                acc.push(name.clone());
                for a in args {
                    expr_calls(a, acc);
                }
            }
            crate::ast::Expr::MethodCall(recv, _, args, _, _) => {
                expr_calls(recv, acc);
                for a in args {
                    expr_calls(a, acc);
                }
            }
            crate::ast::Expr::BinaryOp(_, l, r) => {
                expr_calls(l, acc);
                expr_calls(r, acc);
            }
            crate::ast::Expr::UnaryOp(_, inner) => expr_calls(inner, acc),
            crate::ast::Expr::Index(base, i) => {
                expr_calls(base, acc);
                expr_calls(i, acc);
            }
            crate::ast::Expr::Field(base, _) => expr_calls(base, acc),
            crate::ast::Expr::List(elems) => {
                for el in elems {
                    expr_calls(el, acc);
                }
            }
            crate::ast::Expr::Tuple(elems) => {
                for el in elems {
                    expr_calls(el, acc);
                }
            }
            crate::ast::Expr::Lambda(_, body) => expr_calls(body, acc),
            crate::ast::Expr::StructLiteral { fields, .. } => {
                for (_, v) in fields {
                    expr_calls(v, acc);
                }
            }
            _ => {}
        }
    }
    fn stmt_calls(s: &crate::ast::Statement, acc: &mut Vec<String>) {
        match s {
            crate::ast::Statement::Expression(e) => expr_calls(e, acc),
            crate::ast::Statement::Let { expr: Some(e), .. } => expr_calls(e, acc),
            crate::ast::Statement::Let { .. } => {}
            crate::ast::Statement::Assign(_, e) => expr_calls(e, acc),
            crate::ast::Statement::ArrowAssign { value, .. } => expr_calls(value, acc),
            crate::ast::Statement::Term(Some(e)) => expr_calls(e, acc),
            crate::ast::Statement::Term(None) => {}
            crate::ast::Statement::EndProgram(Some(e)) => expr_calls(e, acc),
            crate::ast::Statement::EndProgram(None) => {}
            crate::ast::Statement::Guarded(cond, b) => {
                // 2026-09-09 (parity spike): the guard CONDITION is an
                // expression too — `when value_ge_10(acc, b) { ... }`. Before
                // this fix the condition's calls were missed, so a helper
                // called ONLY from a guard position was dropped from the
                // transitive import closure and the backend emitted an
                // undefined `@value_ge_10`.
                expr_calls(cond, acc);
                for s in b {
                    stmt_calls(s, acc);
                }
            }
            crate::ast::Statement::Block(b) => {
                for s in b {
                    stmt_calls(s, acc);
                }
            }
            crate::ast::Statement::Foreach { list, body, .. } => {
                expr_calls(list, acc);
                for s in body {
                    stmt_calls(s, acc);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    match item {
        TopLevel::Definition(d) => {
            for s in &d.body {
                stmt_calls(s, &mut out);
            }
        }
        TopLevel::Transaction(t) => {
            for s in &t.body {
                stmt_calls(s, &mut out);
            }
        }
        _ => {}
    }
    out
}

impl ImportResolver {
    pub fn new() -> Self {
        ImportResolver {
            loaded_modules: HashMap::new(),
            search_paths: vec![PathBuf::from("lib"), PathBuf::from("imports"), PathBuf::from(".")],
            root_path: PathBuf::from("."),
            stdlib_path: None,
            board_name: None,
            in_progress: HashSet::new(),
            registry: load_module_registry(),
            resolved_paths: Vec::new(),
            bad_imports: Vec::new(),
            modules: Vec::new(),
            item_origins: Vec::new(),
            plugin_factory: None,
        }
    }

    /// Set the board name for `import "target"` resolution.
    pub fn with_board(mut self, board: &str) -> Self {
        self.board_name = Some(board.to_string());
        self
    }

    /// Set the stdlib root path for import# resolution
    pub fn with_stdlib_path(mut self, path: Option<PathBuf>) -> Self {
        self.stdlib_path = path;
        self
    }

    pub fn add_search_path(&mut self, path: PathBuf) {
        self.search_paths.push(path);
    }

    /// 2026-07-16: P3 — Resolve a path relative to the stdlib root.
    /// Searches the same paths as resolve_stdlib_root().
    pub fn resolve_stdlib_relative_path(&self, relative: &str) -> Option<PathBuf> {
        self.resolve_stdlib_root().map(|root| root.join(relative)).filter(|p| p.exists())
    }

    /// Resolve the stdlib root path, trying multiple sources in order:
    /// 1. Explicitly configured path (from --stdlib-path)
    /// 2. BRIEV_STDLIB_PATH env var
    /// 3. Executable-relative (dev layout: target/release/ -> ../../lib/)
    /// 4. root_path/lib/ (project-local)
    pub fn resolve_stdlib_root(&self) -> Option<PathBuf> {
        // 1. Explicitly configured
        if let Some(ref path) = self.stdlib_path {
            if path.exists() {
                return Some(path.clone());
            }
        }

        // 2. Environment variable
        if let Ok(env_path) = std::env::var("BRIEV_STDLIB_PATH") {
            let p = PathBuf::from(env_path);
            if p.exists() {
                return Some(p);
            }
        }

        // 3. Executable-relative (dev layout)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                // Development: briev-compiler/target/release/ -> ../../lib/
                let dev_p = exe_dir.join("../../lib/");
                if dev_p.exists() {
                    return Some(dev_p);
                }
                // Alternate: briev-compiler/target/debug/ -> ../../lib/
                let debug_p = exe_dir.join("../../lib/");
                if debug_p.exists() {
                    return Some(debug_p);
                }
                // Installed: ~/.local/bin/ -> ~/.local/share/briev/
                let installed_p = exe_dir.join("../share/briev/");
                if installed_p.join("std/core").exists() {
                    return Some(installed_p);
                }
            }
        }

        // 4. Project-local lib/
        let local = self.root_path.join("lib");
        if local.exists() {
            return Some(local);
        }

        None
    }

    /// Resolve every import in `items`, splicing resolved module items at
    /// the import sites. Public signature is unchanged since provenance
    /// landed (2026-09-25, Wave 1 C1): callers read the resolved origins
    /// afterwards from `self.item_origins` (parallel to the returned items).
    pub fn resolve_imports(
        &mut self,
        items: Vec<TopLevel>,
        file_path: &PathBuf,
    ) -> Result<Vec<TopLevel>, String> {
        let root_origins = vec![None; items.len()];
        let (items, origins) = self.resolve_imports_inner(items, root_origins, file_path)?;
        self.item_origins = origins;
        Ok(items)
    }

    /// The recursion core: identical to the old `resolve_imports`, except
    /// every mutation of `items` mirrors on `origins` at the SAME index
    /// (remove⇔remove, splice⇔splice) so the two vectors never desync —
    /// an origins/items desync would misattribute C4 collisions.
    /// `origins` enters aligned with `items` (root call: all `None`;
    /// module call from `resolve_import`: all `Some(module id)`).
    fn resolve_imports_inner(
        &mut self,
        items: Vec<TopLevel>,
        origins: Vec<Option<u32>>,
        file_path: &PathBuf,
    ) -> Result<(Vec<TopLevel>, Vec<Option<u32>>), String> {
        // Set root path from the main file's directory on first call
        if self.root_path == PathBuf::from(".") {
            self.root_path = file_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
        }

        let mut items = items;
        let mut origins = origins;

        // 2026-08-06 (Phase 11): track which module path each imported name
        // came from. Two DIFFERENT modules providing the same unqualified name
        // is a hard error (SPEC 7.2); the same path (diamond) is fine.
        // 2026-08-09 (Phase 11, Slice 2): a second map tracks the `:` module
        // alias per imported name — differing aliases resolve a collision.
        let mut imported_names: HashMap<String, (String, String)> = HashMap::new();
        let mut imported_aliases: HashMap<String, String> = HashMap::new();

        let mut index = 0;

        while index < items.len() {
            let import = match &items[index] {
                TopLevel::Import(import) => Some((import.clone(), false)),
                // `export import` — the only re-export form (SPEC 7.3). The
                // resolved names become module-level (imports are inlined), so
                // importers of this module see them.
                TopLevel::Export(e) => match e.inner.as_ref() {
                    TopLevel::Import(import) => Some((import.clone(), true)),
                    _ => None,
                },
                _ => None,
            };
            let Some((import, _is_reexport)) = import else {
                index += 1;
                continue;
            };
            let (resolved, resolved_origins) = self.resolve_import(&import, file_path)?;
            Self::record_imported_names(
                &mut imported_names,
                &mut imported_aliases,
                &resolved,
                import.path(),
                import.alias.as_deref(),
            )?;
            items.remove(index);
            origins.remove(index);
            items.splice(index..index, resolved);
            origins.splice(index..index, resolved_origins);
        }

        // 2026-06-13: Dedup items (2026-09-25: survivors keep their origins)
        dedup_items_with_origins(items, origins)
    }

    /// 2026-09-25 (interop Wave 1 C1): register a loaded file-backed module
    /// and return its id. Called once per ACTUAL load — cache hits reuse the
    /// cached origins, so a diamond import shares one record. Returns the
    /// index of the pushed record.
    fn push_module(&mut self, specifier: &str, source_path: &Path) -> u32 {
        let kind = classify(source_path);
        self.modules.push(ModuleRecord {
            specifier: specifier.to_string(),
            source_path: source_path.to_path_buf(),
            kind,
        });
        (self.modules.len() - 1) as u32
    }

    /// 2026-09-25 (Wave 1 C2): the three candidate rounds for ONE extension:
    /// (1) each search path (`lib/`, `imports/`, `.`) under `source_dir`,
    /// (2) `lib/` under the ancestor holding the nearest `Cargo.toml` —
    /// std/*, glue/*, and any lib/ module findable from anywhere; the walk-up
    /// stops at the first Cargo.toml (the compiler repo or a user project
    /// root), (3) `source_dir` directly. Extracted from `resolve_import` so
    /// implicit `.bv`/`.ebv` search and explicit-extension imports share one
    /// resolution order (Rule 17).
    fn search_module_file(
        &self,
        source_dir: &Path,
        module_path: &str,
        ext: &str,
    ) -> Option<PathBuf> {
        for search_dir in &self.search_paths {
            let candidate = source_dir
                .join(search_dir)
                .join(format!("{}{}", module_path, ext));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        let mut current = source_dir.to_path_buf();
        while let Some(parent) = current.parent() {
            if parent.join("Cargo.toml").exists() {
                let lib_candidate = parent
                    .join("lib")
                    .join(format!("{}{}", module_path, ext));
                if lib_candidate.exists() {
                    return Some(lib_candidate);
                }
                break;
            }
            current = parent.to_path_buf();
        }
        let direct = source_dir.join(format!("{}{}", module_path, ext));
        if direct.exists() {
            return Some(direct);
        }
        None
    }

    /// 2026-09-25 (Wave 1 C2): house-style not-found diagnostic — what was
    /// searched (truthful: exactly the extensions the search probes), why,
    /// and the concrete fix. If a DATA file sits at the same path it is
    /// called out: data dialects are not code imports — the explicit
    /// `x.dbv` specifier loads typed constants (learn-briev/11-triggers.md);
    /// an implicit specifier over a data file is this error case.
    fn not_found_diagnostic(
        &self,
        specifier: &str,
        module_path: &str,
        explicit_ext: Option<&str>,
        source_dir: &Path,
    ) -> String {
        let exts = match explicit_ext {
            Some(ext) => ext.to_string(),
            None => ".{bv,ebv}".to_string(),
        };
        let dir = source_dir.display();
        let mut msg = format!(
            "Cannot find module '{specifier}'. Searched: {dir}/lib/{mp}{exts}, \
             {dir}/imports/{mp}{exts}, {dir}/{mp}{exts}, and lib/ under the \
             nearest project root.",
            specifier = specifier,
            mp = module_path,
            exts = exts,
            dir = dir,
        );
        if let Some(data_path) = ["dbv", "dbvl"].iter().find_map(|extension| {
            self.search_module_file(source_dir, module_path, &format!(".{}", extension))
        }) {
            msg.push_str(&format!(
                " A data file exists at '{}' — data dialects are not code \
                 imports: load its constants with `import {{ name }} from \
                 \"{}\"`, or check it with `brievc check`.",
                data_path.display(),
                specifier,
            ));
        }
        msg.push_str(&format!(
            " Fix: create {mp}.bv (or {mp}.ebv) in a searched directory, correct \
             the import path, or — for a file in another dialect — import it with \
             its explicit extension (`import \"./{mp}.<ext>\"`).",
            mp = module_path,
        ));
        msg
    }

    /// Resolve `import "target"` — loads the board D-briev description and emits typed constants.
    /// 2026-08-06 (Phase 11): record which module path each imported name came
    /// from. Two DIFFERENT modules providing the same unqualified name is a
    /// hard error (SPEC 7.2) UNLESS the definitions are IDENTICAL (a benign
    /// duplicate, e.g. `SYS_WRITE` declared in both fs.bv and net.bv); the
    /// same path (diamond) is fine.
    fn record_imported_names(
        imported: &mut HashMap<String, (String, String)>,
        imported_aliases: &mut HashMap<String, String>,
        resolved: &[TopLevel],
        path: &str,
        alias: Option<&str>,
    ) -> Result<(), String> {
        for item in resolved {
            // 2026-08-09 (Phase 11, Slice 2): an `impl T` EXTENDS the type `T`
            // — it does not DECLARE it, so it must not participate in name
            // collision. The type declaration carries the name; an impl is a
            // coherence relationship (§17.2). Skipping impls here also fixes a
            // false collision: `type Point` in a.bv + `impl Point` in b.bv,
            // both imported, are a valid cross-module coherence pair.
            if matches!(item, TopLevel::Impl(_)) {
                continue;
            }
            if let Some(n) = Self::item_name(item) {
                if let Some((src, prior)) = imported.get(n) {
                    // 2026-08-09 (Phase 11, Slice 2): two imports providing the
                    // same exported name are legal when they carry DIFFERENT
                    // `:` module aliases — the alias is a collision-resolving
                    // local TAG (SPEC §7.2; no qualified access — Briev inlines).
                    // Same path (diamond) and identical definitions stay benign.
                    let same_alias = match (imported_aliases.get(n), alias) {
                        (Some(a), Some(b)) => a == b,
                        (None, None) => true,
                        // One side aliased, the other not: the aliased import
                        // is a distinct tag, so they coexist.
                        _ => false,
                    };
                    if src != path && *prior != format!("{:?}", item) && same_alias {
                        return Err(format!(
                            "import name '{}' conflicts: provided by both '{}' and '{}' — \
                             use a selective rename (`{{ Local: Exported }}`) or a module alias",
                            n, src, path
                        ));
                    }
                } else {
                    imported.insert(n.to_string(), (path.to_string(), format!("{:?}", item)));
                    if let Some(a) = alias {
                        imported_aliases.insert(n.to_string(), a.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_target_import(&mut self) -> Result<Vec<TopLevel>, String> {
        let board = self.board_name.as_deref().unwrap_or("stm32f407");

        // 2026-09-06 (ISR plan): activate the board UNCONDITIONALLY — the
        // address map (addresses.dbvl) and the named ISR vector table
        // (interrupts.dbvl) load through address_resolver's own search
        // (lib/boards/<board>/), independent of whether the D-briev device
        // description below resolves. Board activation must never depend on
        // the description being present.
        crate::address_resolver::set_active_board(board);

        // 2026-08-03 (Phase 2): the board map is now a directory:
        //   lib/boards/<board>/map.dbv          — schemas only
        //   lib/boards/<board>/addresses.dbvl   — flat KEY: addr; size; table
        //   lib/boards/<board>/registers.dbvl   — flat register detail
        // The old single-file `boards/<board>.dbvl` is obsolete. Look for the
        // addresses table first; fall back to the legacy single file.
        let addresses_path = self.search_paths.iter()
            .map(|p| p.join("boards").join(board).join("addresses.dbvl"))
            .chain(std::iter::once(PathBuf::from(board).join("addresses.dbvl")))
            .find(|p| p.exists());

        let mut doc = if let Some(path) = addresses_path {
            // Activate the board map so address_resolver agrees with this table.
            crate::address_resolver::set_active_board(board);

            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
            let mut doc = crate::dbriev::v2::parse_document(&content)
                .map_err(|e| format!("Failed to parse '{}': {}", path.display(), e))?;

            // Merge the schema carrier (map.dbv) and register detail table.
            let schemas_path = path.with_file_name("map.dbv");
            if schemas_path.exists() {
                if let Ok(schema_content) = std::fs::read_to_string(&schemas_path) {
                    if let Ok(schema_doc) = crate::dbriev::v2::parse_document(&schema_content) {
                        doc.schemas.extend(schema_doc.schemas);
                        // map.dbv is merged inline — drop it from doc.imports so
                        // the bridge does not re-emit it as a literal import.
                        doc.imports.retain(|i| i != "map.dbv");
                    }
                }
            }
            let registers_path = path.with_file_name("registers.dbvl");
            if registers_path.exists() {
                if let Ok(reg_content) = std::fs::read_to_string(&registers_path) {
                    if let Ok(reg_doc) = crate::dbriev::v2::parse_document(&reg_content) {
                        doc.data_groups.extend(reg_doc.data_groups);
                    }
                }
            }
            doc
        } else {
            // Legacy single-file board (pre-2026-08-03). Kept as a fallback
            // for out-of-tree board packs that still ship the old layout.
            let file_name = format!("{}.dbvl", board);
            let file_path = self.search_paths.iter()
                .map(|p| p.join("boards").join(&file_name))
                .chain(std::iter::once(PathBuf::from(&file_name)))
                .find(|p| p.exists());
            let path = match file_path {
                Some(p) => p,
                None => return Err(format!(
                    "Board file 'lib/boards/{}.dbvl' or 'lib/boards/{}/addresses.dbvl' not found. \
                     Use --board <name> or create a board directory.",
                    board, board
                )),
            };
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
            crate::dbriev::v2::parse_document(&content)
                .map_err(|e| format!("Failed to parse '{}': {}", path.display(), e))?
        };

        // Resolve schema imports (schema <path>; directives)
        let mut resolved_imports = Vec::new();
        for import_path in &doc.imports {
            let schema_path = self.search_paths.iter()
                .map(|p| p.join(&import_path))
                .chain(std::iter::once(PathBuf::from(&import_path)))
                .find(|p| p.exists());

            if let Some(sp) = schema_path {
                if let Ok(schema_content) = std::fs::read_to_string(&sp) {
                    if let Ok(schema_doc) = crate::dbriev::v2::parse_document(&schema_content) {
                        doc.schemas.extend(schema_doc.schemas);
                        resolved_imports.push(import_path.clone());
                    }
                }
            }
        }
        doc.imports.retain(|i| !resolved_imports.contains(i));

        let items = crate::dbriev::bridge::document_to_program(&doc, &board);

        Ok(items)
    }

    fn resolve_import(
        &mut self,
        import: &Import,
        source_file: &PathBuf,
    ) -> Result<(Vec<TopLevel>, Vec<Option<u32>>), String> {
        // Skip empty module paths
        if import.path().is_empty() {
            return Ok((vec![], vec![]));
        }

        // Handle Registry imports — look up name in registry dir first,
        // then fall back to the TOML module registry config.
        // 2026-07-15: Phase 7i
        // 2026-07-26: Check ~/.briev/registry/ before TOML registry.
        if let ImportKind::Registry(name) = &import.kind {
            // 2026-07-26: Check registry directory first (user-installed modules
            // take priority over baked config/module-registry.toml entries).
            if let Some(reg_path) = crate::registry::find_registry_entry(name) {
                // 2026-08-09 (Phase 11, Slice 2): record the registry name →
                // canonical path (SPEC §7.1 determinism record).
                self.resolved_paths
                    .push((import.path().to_string(), reg_path.to_string_lossy().to_string()));
                let literal_import = Import::literal(reg_path.to_string_lossy().to_string(), import.symbols.clone());
                return self.resolve_import(&literal_import, source_file);
            }
            let resolved_path = self.registry.get(name.as_str());
            let actual_path = match resolved_path {
                Some(p) => p.clone(),
                None => {
                    // Name not found in registry — fall back to using the name
                    // as a literal path (same as import "name").
                    name.clone()
                }
            };
            self.resolved_paths
                .push((import.path().to_string(), actual_path.clone()));
            let literal_import = Import::literal(actual_path, import.symbols.clone());
            return self.resolve_import(&literal_import, source_file);
        }

        // Handle `import "target"` — board-level device description
        if import.path() == "target" {
            // Generated items (assembled from board DBriev tables) — not a
            // single source file, so they carry no module origin (Wave 1 C1).
            let items = self.resolve_target_import()?;
            let n = items.len();
            return Ok((items, vec![None; n]));
        }

        // 2026-08-22 (spec-conformance plan Phase 1a): glob imports are
        // invalid (SPEC §7.2). Removed the directory-glob expansion that used
        // to live here (`resolve_glob`); a `*`/`**` path is now an error.
        // Undo: restore resolve_glob + its call site + the non-recursive test.
        if import.path().contains('*') {
            return Err(format!(
                "glob import '{}' is invalid — import each file explicitly (SPEC §7.2)",
                import.path()
            ));
        }

        // Cache check
        if let Some((cached, sed_names, cached_origins)) = self.loaded_modules.get(import.path()) {
            return self.filter_items_with_origins(cached, cached_origins, sed_names, &import.symbols);
        }

        // Check for CSS import (loader extracted 2026-09-25, C1: flat
        // control flow — the exists/load/cache/record body lives in the
        // helper; `None` means "not an existing file, fall through").
        if import.path().ends_with(".css") {
            if let Some(loaded) = self.load_css_import(import, source_file)? {
                return Ok(loaded);
            }
        }

        // Check for SVG import (same extraction as CSS above).
        if import.path().ends_with(".svg") {
            if let Some(loaded) = self.load_svg_import(import, source_file)? {
                return Ok(loaded);
            }
        }

        // Check for DBriev import (.dbv, .dbvl)
        if import.path().ends_with(".dbv") || import.path().ends_with(".dbvl") {
            let dbriev_src_dir = source_file
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));

            let dbriev_path = self
                .search_paths
                .iter()
                .map(|p| dbriev_src_dir.join(p).join(&import.path()))
                .chain(std::iter::once(dbriev_src_dir.join(&import.path())))
                .find(|p| p.exists())
                .ok_or_else(|| {
                    format!(
                        "DBriev file not found: {} (searched in lib/, imports/, ./ and source dir)",
                        import.path()
                    )
                })?;

            let content = std::fs::read_to_string(&dbriev_path)
                .map_err(|e| format!("Failed to read DBriev file '{}': {}", dbriev_path.display(), e))?;

            let is_dbvl = import.path().ends_with(".dbvl");

            // For .dbvl files, use offset-tracking parser for lazy loading
            let doc = if is_dbvl {
                dbriev_v2::parse_document_track_offsets(&content)
            } else {
                dbriev_v2::parse_document(&content)
            }.map_err(|e| format!("Failed to parse DBriev file '{}': {}", dbriev_path.display(), e))?;

            // Determine the constant name from import symbols
            let constant_name = import
                .symbols
                .first()
                .map(|(local, _)| local.clone())
                .unwrap_or_else(|| {
                    let fname = dbriev_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "data".to_string());
                    fname
                });

            let mut dbriev_items = crate::dbriev::bridge::document_to_program_flags(
                &doc, &constant_name, is_dbvl,
            );

            let program_for_cache = dbriev_items.clone();

            let m_id = self.push_module(import.path(), &dbriev_path);
            let origins = vec![Some(m_id); dbriev_items.len()];
            self.loaded_modules.insert(
                import.path().to_string(),
                (program_for_cache, vec![], origins.clone()),
            );

            return Ok((dbriev_items, origins));
        }

        // 2026-09-22 (per-arch stdlib boot entries): `import "*.bad"` —
        // a .bad source is NOT Briev; record its resolved path and hand it
        // to the bad backend. Returns no Briev items (the bootstrap body
        // references the imported named raw blocks by symbol).
        if import.path().ends_with(".bad") {
            let bad_src_dir = source_file
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            // `import "std/bad/arch.bad"`: the stdlib root is `lib/`, so
            // strip the leading `std/` and join — `std/bad/x.bad` →
            // `lib/bad/x.bad`. The `.bv` stdlib lives at `lib/std/`; the
            // `.bad` stdlib at `std/bad/` is the repo-root sibling of the
            // stdlib root's parent. Search local paths AND the stdlib root.
            let bad_path = self
                .search_paths
                .iter()
                .map(|p| bad_src_dir.join(p).join(&import.path()))
                .chain(std::iter::once(bad_src_dir.join(&import.path())))
                .chain(
                    std::env::current_dir()
                        .ok()
                        .into_iter()
                        .map(|cwd| cwd.join(&import.path())),
                )
                .chain(
                    self.resolve_stdlib_root()
                        .into_iter()
                        .flat_map(|root| {
                            // `.bad` stdlib lives at <repo>/std/bad/ — the
                            // repo root is one level up from the .bv stdlib
                            // root (`lib/`). Also try `lib/bad/`.
                            let repo = root.parent().map(|p| p.to_path_buf());
                            let mut candidates = Vec::new();
                            if let Some(r) = repo {
                                candidates.push(r.join(&import.path()));
                            }
                            let rel = import.path().strip_prefix("std/").unwrap_or(&import.path());
                            candidates.push(root.join("bad").join(rel));
                            candidates.into_iter()
                        }),
                )
                .find(|p| p.exists())
                .ok_or_else(|| {
                    format!(
                        "bad file not found: {} (searched in lib/, imports/, ./ and source dir)",
                        import.path()
                    )
                })?;
            self.resolved_paths
                .push((import.path().to_string(), bad_path.to_string_lossy().to_string()));
            if !self.bad_imports.contains(&bad_path) {
                self.bad_imports.push(bad_path);
            }
            self.loaded_modules.insert(
                import.path().to_string(),
                (vec![], vec![], vec![]),
            );
            return Ok((vec![], vec![]));
        }

        // Default: code-dialect module search (2026-09-25, interop Wave 1 C2).
        // An explicit known code extension (`.bv`, `.ebv`, `.rbv`, `.abv`,
        // `.sbv`) searches ONLY that extension; extension-less specifiers
        // search `.bv` then `.ebv` — the diagnostic's `{bv,ebv}` promise is
        // now true, and an electronics module becomes importable from `.bv`
        // (the real `.ebv` import candidate, item 3). `.abv`/`.sbv` are NOT
        // implicit candidates; their waves add them. Data (`.dbv`/`.dbvl`),
        // asset (`.css`/`.svg`), and `.bad` imports are handled by their own
        // arms above and never reach this search.
        let (raw_base, explicit_ext) = split_known_code_ext(import.path());
        let module_path = raw_base.replace('.', "/");
        // TypeScript-style import resolution:
        //   "./foo" or "../foo" → relative to importing file
        //   "foo/bar"           → relative to project root
        let is_relative = import.path().starts_with("./") || import.path().starts_with("../");
        let source_dir = if is_relative {
            source_file
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            self.root_path.clone()
        };

        let found_path = match explicit_ext {
            Some(ext) => self.search_module_file(&source_dir, &module_path, ext),
            None => ["bv", "ebv"].iter().find_map(|extension| {
                self.search_module_file(&source_dir, &module_path, &format!(".{}", extension))
            }),
        };

        let resolved_path = found_path
            .ok_or_else(|| self.not_found_diagnostic(import.path(), &module_path, explicit_ext, &source_dir))?;

        // 2026-08-09 (Phase 11, Slice 2): record the deterministic resolution
        // (specifier → canonical path) for reproducibility/diagnostics (SPEC
        // §7.1). The specifier is the ORIGINAL import path; a registry import
        // lands here after its literal re-entry, so the record uses the
        // import's current path (already rewritten to the resolved literal).
        self.resolved_paths
            .push((import.path().to_string(), resolved_path.to_string_lossy().to_string()));


        // 2026-07-01: Cycle detection
        if !self.in_progress.insert(import.path().to_string()) {
            return Err(format!(
                "Circular import detected: '{}' is already being resolved \
                 (direct or transitive self-import).",
                import.path()
            ));
        }

        let source = std::fs::read_to_string(&resolved_path)
            .map_err(|e| format!("Failed to read '{}': {}", resolved_path.display(), e))?;

        // Wave 1 C1: this module's id — EVERY item parsed below belongs to
        // it (including items its own nested imports splice in: their
        // inner origins are already `Some(id)` and pass through untouched).
        let m_id = self.push_module(import.path(), &resolved_path);

        // 2026-09-25 (Wave 1 C2): per-dialect source preparation after
        // `classify(&resolved_path)`. `.rbv` extracts its Briev remainder
        // (markup/style/view stay with the view pipeline; a logic-only
        // `.rbv` passes through unchanged). Briev and Electronics share the
        // parser — electronics syntax is lexer/AST-level on main. `.abv`/
        // `.sbv` (explicit extensions only) parse as Briev until their waves
        // land per-kind semantics; the shared parser accepts their
        // declarations today. Data kinds cannot reach here: the `.dbv`/
        // `.dbvl` arm above owns them (explicit data specifiers load typed
        // constants; implicit ones are the diagnostic-enrichment case).
        let resolved_str = resolved_path.to_string_lossy().to_string();
        let parse_source = match classify(&resolved_path) {
            Some(SourceKind::Rendered) => {
                crate::pipeline::preprocess_source_for_path(&resolved_str, &source)?.briev_source
            }
            _ => source,
        };

        // 2026-08-06 → 2026-09-25 (Wave 1 C2): imports now lex through the
        // shared `lex_for_path` (formatted `.f` profiles layout-process,
        // real token spans for error messages) — same entry as the root
        // compile path, so an imported module parses EXACTLY like a root
        // file of its dialect.
        let tokens = crate::pipeline::lex_for_path(&resolved_str, &parse_source)?;
        let mut parser = crate::parser::Parser::new(tokens, &parse_source);
        // 2026-07-14: Parse errors in imported files are non-fatal — the
        // imported file may use syntax (struct literals, etc.) that the
        // parser supports as AST but not yet as a fully parseable form.
        // 2026-08-04 (compiler-in-Briev): the error is NOT swallowed — it is
        // reported as a visible warning so a silently-empty import (which
        // drops a module's defns, e.g. std/string's `..` slices) is never
        // hidden again. The import still proceeds with the items that DID
        // parse (non-fatal, pre-merge behavior).
        let mut imported_program = match parser.parse_program() {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "warning: import '{}' at '{}' failed to fully parse: {}",
                    import.path(), resolved_path.display(), e
                );
                vec![]
            }
        };

        self.run_module_prelude(import.path(), &resolved_str, &mut imported_program)?;
        let module_len = imported_program.len();
        let (resolved, resolved_origins) =
            self.resolve_imports_inner(imported_program, vec![Some(m_id); module_len], &resolved_path)?;
        if import.path().contains("glue/c") {
        }

        // Cache the fully resolved program (origins ride along — a cache
        // hit must attribute items to the SAME module id: diamond test).
        self.loaded_modules.insert(
            import.path().to_string(),
            (resolved.clone(), vec![], resolved_origins.clone()),
        );

        let result = self.filter_items_with_origins(&resolved, &resolved_origins, &[], &import.symbols);

        self.in_progress.remove(import.path());
        result
    }

    /// 2026-09-25 (Wave 1 C3): run a module's dialect prelude after parse
    /// and before nested resolution. The module's dialect plugins (scoped by
    /// `config/targets.dbvl` through the factory) run at Parsed on the freshly
    /// parsed program — same stage/order as a root file — so prelude-inserted
    /// `Import$` anchors resolve in the inner walk.
    /// std-skip: stdlib modules do NOT run it. `std/…` files are the prelude's
    /// CONTENT, not its consumer — the root prelude plus flat inlining already
    /// puts their names in scope — and running it would self-cycle: an std
    /// file's own prelude re-inserts the std bundle while its `std→std`
    /// imports are still in `in_progress`, which the cycle guard (2026-07-01)
    /// errors on. `std/electronics.bv` remains reachable from any `.ebv`
    /// module; it just parses plain. Fresh universe per module mirrors the
    /// root call sites, whose Parsed-stage universes are block-local and
    /// discarded — threading one through would diverge from that parity (C3
    /// plan deviation note).
    /// How to undo: drop this helper with the `plugin_factory` field and
    /// `pipeline::module_plugin_factory`.
    fn run_module_prelude(
        &self,
        specifier: &str,
        module_path: &str,
        program: &mut Vec<TopLevel>,
    ) -> Result<(), String> {
        if specifier.starts_with("std/") {
            return Ok(());
        }
        if let Some(ref factory) = self.plugin_factory {
            let pm = factory(module_path)?;
            pm.run_ast(
                StageKind::Parsed,
                program,
                &mut crate::type_universe::TypeUniverse::new(),
            )?;
        }
        Ok(())
    }

    /// 2026-09-25 (interop Wave 1 C1): `.css` asset import body, extracted
    /// from `resolve_import` (flat control flow / Praetor line budget).
    /// `Ok(None)` = file does not exist (fall through to later arms);
    /// `Ok(Some(..))` = loaded, cached, and registered as one module record.
    fn load_css_import(
        &mut self,
        import: &Import,
        source_file: &PathBuf,
    ) -> Result<Option<(Vec<TopLevel>, Vec<Option<u32>>)>, String> {
        let css_path = source_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(&import.path());
        if !css_path.exists() {
            return Ok(None);
        }
        let css_content = std::fs::read_to_string(&css_path)
            .map_err(|e| format!("Failed to read CSS '{}': {}", css_path.display(), e))?;
        let css_for_cache = css_content.clone();
        let m_id = self.push_module(import.path(), &css_path);
        self.loaded_modules.insert(
            import.path().to_string(),
            (vec![TopLevel::Stylesheet(css_for_cache)], vec![], vec![Some(m_id)]),
        );
        Ok(Some((vec![TopLevel::Stylesheet(css_content)], vec![Some(m_id)])))
    }

    /// 2026-09-25 (interop Wave 1 C1): `.svg` component import body,
    /// extracted from `resolve_import` (same rationale as `load_css_import`).
    fn load_svg_import(
        &mut self,
        import: &Import,
        source_file: &PathBuf,
    ) -> Result<Option<(Vec<TopLevel>, Vec<Option<u32>>)>, String> {
        let svg_path = source_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(&import.path());
        if !svg_path.exists() {
            return Ok(None);
        }
        let svg_content = std::fs::read_to_string(&svg_path)
            .map_err(|e| format!("Failed to read SVG '{}': {}", svg_path.display(), e))?;
        let component_name = import
            .symbols
            .first()
            .map(|(local, _)| local.clone())
            .unwrap_or_else(|| {
                let file_name = if let Some(last_slash) = import.path().rfind('/') {
                    &import.path()[last_slash + 1..]
                } else {
                    &import.path()
                };
                let file_name = file_name.trim_end_matches(".svg");
                file_name
                    .split('-')
                    .map(|s| {
                        let mut chars = s.chars();
                        match chars.next() {
                            Some(c) => {
                                c.to_uppercase().collect::<String>() + chars.as_str()
                            }
                            None => String::new(),
                        }
                    })
                    .collect::<String>()
            });
        let svg_for_cache = svg_content.clone();
        let m_id = self.push_module(import.path(), &svg_path);
        self.loaded_modules.insert(
            import.path().to_string(),
            (vec![TopLevel::SvgComponent {
                name: component_name.clone(),
                content: svg_for_cache,
            }], vec![], vec![Some(m_id)]),
        );
        Ok(Some((vec![TopLevel::SvgComponent {
            name: component_name,
            content: svg_content,
        }], vec![Some(m_id)])))
    }

    /// 2026-08-06 (Phase 11): filter imported items by the EXPORTED names and
    /// apply selective renames (`{ Local: Exported }`). Preserves the D3
    /// transitive-referenced-type closure and the file-private (sed) filter.
    /// 2026-09-25 (interop Wave 1 C1): provenance split — origins ride in
    /// lockstep; the final pass is the only place items are kept or dropped,
    /// so it zips `origins` there; alignment is enforced by
    /// `assert_origins_aligned` (hard-error, never silent truncation).
    fn filter_items_with_origins(
        &self,
        items: &[TopLevel],
        origins: &[Option<u32>],
        sed_names: &[String],
        symbols: &[(String, String)],
    ) -> Result<(Vec<TopLevel>, Vec<Option<u32>>), String> {
        assert_origins_aligned(items.len(), origins.len(), "filtering module items")?;
        let rename: HashMap<String, String> = symbols
            .iter()
            .filter(|(l, e)| l != e)
            .map(|(l, e)| (e.clone(), l.clone()))
            .collect();
        let exported_names: std::collections::HashSet<String> =
            symbols.iter().map(|(_, e)| e.clone()).collect();
        let keep_all = symbols.is_empty();

        let always = |item: &TopLevel| {
            matches!(
                item,
                TopLevel::ForeignBinding { .. }
                    | TopLevel::LinkDependency(_)
                    | TopLevel::StageBlock(_)
                    | TopLevel::CompileTimeDefn(_)
                    | TopLevel::CompileTimeTxn(_)
                    | TopLevel::CompileTimeLet(_, _)
                    | TopLevel::CompileTimeConst(_, _)
            )
        };
        let wanted = |n: &str| keep_all || exported_names.contains(n);

        let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
        for item in items {
            if always(item) {
                continue;
            }
            if let Some(n) = Self::item_name(item) {
                if !sed_names.iter().any(|s| s == n) && wanted(n) {
                    keep.insert(n.to_string());
                }
            }
        }
        // 2026-08-01 (D3): a named import must ALSO bring the requested item's
        // transitive referenced types (List -> ListBuffer<T>).
        // 2026-08-16 (Phase 3c): and its referenced FUNCTIONS (iter_map ->
        // iter_map_loop) — a generic adapter body calls its `_loop` sibling,
        // which must be in scope or the call resolves to the raw-type
        // fallback and the body fails to typecheck.
        let mut changed = true;
        while changed {
            changed = false;
            let mut refs: std::collections::HashSet<String> = std::collections::HashSet::new();
            for item in items {
                if Self::item_name(item).map_or(false, |n| keep.contains(n)) {
                    for r in referenced_type_names(item) {
                        refs.insert(r);
                    }
                    for r in referenced_function_names(item) {
                        refs.insert(r);
                    }
                }
            }
            for item in items {
                if Self::item_name(item).map_or(false, |n| keep.contains(n)) {
                    continue;
                }
                if Self::item_name(item).map_or(false, |n| refs.contains(n)) {
                    if let Some(n) = Self::item_name(item) {
                        keep.insert(n.to_string());
                        changed = true;
                    }
                }
            }
        }
        let mut out: Vec<TopLevel> = Vec::new();
        let mut out_origins: Vec<Option<u32>> = Vec::new();
        for (item, origin) in items.iter().zip(origins) {
            if always(item) {
                out.push(item.clone());
                out_origins.push(*origin);
                continue;
            }
            match Self::item_name(item) {
                Some(n) if keep.contains(n) => {
                    if let Some(local) = rename.get(n) {
                        out.push(Self::rename_item(item, local));
                    } else {
                        out.push(item.clone());
                    }
                    out_origins.push(*origin);
                }
                _ => {}
            }
        }
        Ok((out, out_origins))
    }

    /// The unqualified name of a top-level item (import filtering + renames).
    fn item_name(item: &TopLevel) -> Option<&str> {
        match item {
            TopLevel::Definition(d) => Some(d.name.as_str()),
            TopLevel::Signature(s) => Some(s.name.as_str()),
            TopLevel::Statement(s) => match s.as_ref() {
                crate::ast::Statement::Let { name, .. } => Some(name.as_str()),
                _ => None,
            },
            TopLevel::ForeignBinding(fb) => Some(fb.effective_briev_name()),
            TopLevel::Transaction(t) => Some(t.name.as_str()),
            TopLevel::Constant(c) => Some(c.name.as_str()),
            TopLevel::Init(i) => Some(i.name.as_str()),
            TopLevel::Obj(s) => Some(s.name.as_str()),
            TopLevel::RenderBlock(rb) => Some(rb.struct_name.as_str()),
            TopLevel::Trigger(trg) => Some(trg.name.as_str()),
            TopLevel::TriggerBinding { name, .. } => Some(name.as_str()),
            TopLevel::Cell(c) => Some(c.name.as_str()),
            TopLevel::StateDecl(s) => Some(s.name.as_str()),
            TopLevel::TypeDef(t) => Some(t.name.as_str()),
            TopLevel::Trait(t) => Some(t.name.as_str()),
            TopLevel::Impl(i) => Some(i.target.as_str()),
            TopLevel::ProtocolDef(p) => Some(p.name.as_str()),
            TopLevel::StaticStruct(s) => Some(s.name.as_str()),
            _ => None,
        }
    }

    /// Apply a selective-import rename to a top-level item's name.
    fn rename_item(item: &TopLevel, local: &str) -> TopLevel {
        match item.clone() {
            TopLevel::Definition(mut d) => { d.name = local.to_string(); TopLevel::Definition(d) }
            TopLevel::Signature(mut s) => { s.name = local.to_string(); TopLevel::Signature(s) }
            TopLevel::Constant(mut c) => { c.name = local.to_string(); TopLevel::Constant(c) }
            TopLevel::Init(mut i) => { i.name = local.to_string(); TopLevel::Init(i) }
            TopLevel::Obj(mut s) => { s.name = local.to_string(); TopLevel::Obj(s) }
            TopLevel::Transaction(mut t) => { t.name = local.to_string(); TopLevel::Transaction(t) }
            TopLevel::Trigger(mut t) => { t.name = local.to_string(); TopLevel::Trigger(t) }
            TopLevel::Cell(mut c) => { c.name = local.to_string(); TopLevel::Cell(c) }
            TopLevel::StateDecl(mut s) => { s.name = local.to_string(); TopLevel::StateDecl(s) }
            TopLevel::TypeDef(mut t) => { t.name = local.to_string(); TopLevel::TypeDef(t) }
            TopLevel::Trait(mut t) => { t.name = local.to_string(); TopLevel::Trait(t) }
            other => other,
        }
    }

    /// Resolve an import from the stdlib path.
    fn resolve_stdlib_import(
        &mut self,
        module: &str,
    ) -> Result<(Vec<TopLevel>, Vec<Option<u32>>), String> {
        let stdlib_root = self.resolve_stdlib_root().ok_or_else(|| {
            format!(
                "Cannot resolve import '{}': no stdlib path configured. \
                 Use --stdlib-path or set BRIEV_STDLIB_PATH.",
                module
            )
        })?;

        let relative_path: PathBuf = module.split('/').collect();
        let full_path = stdlib_root.join(&relative_path);

        // Try with .bv extension if the path doesn't have one
        let candidate = if full_path.extension().is_some() {
            full_path.clone()
        } else {
            full_path.with_extension("bv")
        };

        // Use a distinct cache key for stdlib imports
        let cache_key = format!("stdlib:{}", module);
        if let Some((cached, sed_names, cached_origins)) = self.loaded_modules.get(&cache_key) {
            return self.filter_items_with_origins(cached, cached_origins, sed_names, &[]);
        }

        if !candidate.exists() {
            return Err(format!(
                "Cannot find module '{}' at stdlib path: {}",
                module,
                candidate.display()
            ));
        }

        let source = std::fs::read_to_string(&candidate)
            .map_err(|e| format!("Failed to read '{}': {}", candidate.display(), e))?;

        // Wave 1 C1: stdlib modules carry provenance like any other module.
        let m_id = self.push_module(module, &candidate);

        let tokens = lex_source(&source)?;
        let mut parser = crate::parser::Parser::new(tokens, &source);
        let imported_program = parser.parse_program().unwrap_or_default();
        let module_len = imported_program.len();

        let (resolved, resolved_origins) =
            self.resolve_imports_inner(imported_program, vec![Some(m_id); module_len], &candidate)?;

        self.loaded_modules.insert(
            cache_key,
            (resolved.clone(), vec![], resolved_origins.clone()),
        );

        self.filter_items_with_origins(&resolved, &resolved_origins, &[], &[])
    }
}

/// Lex a source string into a token vector with span information.
/// 2026-09-25 (Wave 1 C2): split a known CODE-dialect extension off an
/// import specifier — `(base, Some(".bv"))` for explicit extensions,
/// `(spec, None)` for extension-less. Callers map '.'→'/' over the base.
/// Data (`.dbv`/`.dbvl`), asset (`.css`/`.svg`), and `.bad` are not code
/// dialects: their dedicated arms in `resolve_import` run first, so they
/// never reach this split. `.bv` first in the list matters only for
/// documentation — the suffixes do not overlap (`x.ebv` does not end in
/// `.bv`).
fn split_known_code_ext(spec: &str) -> (&str, Option<&'static str>) {
    const CODE_EXTS: [&str; 5] = [".bv", ".ebv", ".rbv", ".abv", ".sbv"];
    for ext in CODE_EXTS {
        if spec.len() > ext.len() && spec.ends_with(ext) {
            return (&spec[..spec.len() - ext.len()], Some(ext));
        }
    }
    (spec, None)
}

fn lex_source(source: &str) -> Result<Vec<(Token, std::ops::Range<usize>)>, String> {
    let lexer = Token::lexer(source);
    let mut tokens = Vec::new();
    for result in lexer {
        let token = result.map_err(|_| "lex error".to_string())?;
        let range = 0..0;
        tokens.push((token, range));
    }
    Ok(tokens)
}

fn item_key(item: &TopLevel) -> Option<(String, String)> {
    // 2026-08-28: name-only keys — the typechecker holds ONE signature per
    // callable name (fn_param_types/fn_return_types are name-keyed), so
    // same-name defns cannot coexist in one module; keeping both would also
    // emit duplicate @symbols in LLVM. Combined with last-wins dedup below,
    // the LOCAL (later) definition shadows the imported one — lexical-scope
    // semantics. Overloads per se are a language-feature track (would need
    // call-site signature resolution + backend name mangling).
    match item {
        TopLevel::Definition(d) => Some(("def".into(), d.name.clone())),
        TopLevel::Transaction(t) => Some(("txn".into(), t.name.clone())),
        TopLevel::StateDecl(s) => Some(("state".into(), s.name.clone())),
        TopLevel::Trigger(trg) => Some(("trigger".into(), trg.name.clone())),
        TopLevel::TriggerBinding { name, .. } => Some(("trg_binding".into(), name.clone())),
        TopLevel::Cell(c) => Some(("cell".into(), c.name.clone())),
        TopLevel::Constant(c) => Some(("const".into(), c.name.clone())),
        TopLevel::Signature(s) => Some(("sig".into(), s.name.clone())),
        TopLevel::ForeignBinding(fb) => Some(("frgn".into(), fb.foreign_name.clone())),
        TopLevel::Obj(s) => Some(("struct".into(), s.name.clone())),
        TopLevel::Enum(e) => Some(("enum".into(), e.name.clone())),
        TopLevel::TypeDef(t) => Some(("typedef".into(), t.name.clone())),
        TopLevel::Trait(t) => Some(("trait".into(), t.name.clone())),
        TopLevel::Impl(i) => Some(("impl".into(), i.target.clone())),
        TopLevel::RenderBlock(r) => Some(("render".into(), r.struct_name.clone())),
        TopLevel::LinkDependency(l) => Some(("link".into(), l.path.clone())),
        TopLevel::ResourceDecl(r) => Some(("rsrc".into(), r.name.clone())),
        _ => None,
    }
}

/// Keep the LAST occurrence of each named top-level item — local/recent
/// definitions shadow imported ones, matching lexical-scope semantics.
/// Diamond-import dedup still works because identical items from the same
/// module collapse to one copy regardless of which is "last."
/// 2026-09-25 (interop Wave 1 C1): the origins/items alignment invariant,
/// shared by `filter_items_with_origins` and `dedup_items_with_origins` —
/// every caller aligns the vectors, so a length mismatch is an INTERNAL
/// invariant violation (an origins/items desync would misattribute C4
/// collisions) and hard-errors rather than silently truncating items.
fn assert_origins_aligned(
    items_len: usize,
    origins_len: usize,
    context: &str,
) -> Result<(), String> {
    if items_len == origins_len {
        return Ok(());
    }
    Err(format!(
        "internal: origins desync — {} items vs {} origins while {}",
        items_len, origins_len, context
    ))
}

/// 2026-06-13: dedup items (last occurrence wins — lexical shadowing).
/// 2026-09-25 (interop Wave 1 C1): survivors keep their origins; alignment
/// enforced by `assert_origins_aligned` (same contract as filtering).
fn dedup_items_with_origins(
    items: Vec<TopLevel>,
    origins: Vec<Option<u32>>,
) -> Result<(Vec<TopLevel>, Vec<Option<u32>>), String> {
    assert_origins_aligned(items.len(), origins.len(), "deduplicating")?;
    use std::collections::HashMap;
    let mut last_indices: HashMap<(String, String), usize> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(key) = item_key(item) {
            last_indices.insert(key, i);
        }
    }
    let mut result: Vec<TopLevel> = Vec::with_capacity(items.len());
    let mut result_origins: Vec<Option<u32>> = Vec::with_capacity(origins.len());
    for (i, (item, origin)) in items.into_iter().zip(origins).enumerate() {
        let keep = match item_key(&item) {
            Some(key) => last_indices[&key] == i,
            None => true,
        };
        if keep {
            result.push(item);
            result_origins.push(origin);
        }
    }
    Ok((result, result_origins))
}

impl Default for ImportResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use std::path::PathBuf;
    use std::fs;
    use tempfile::TempDir;

    fn import_program(path: &str, symbols: Vec<String>) -> Vec<TopLevel> {
        let symbols: Vec<(String, String)> = symbols.into_iter().map(|s| (s.clone(), s)).collect();
        vec![TopLevel::Import(Import::literal(path.to_string(), symbols))]
    }

    /// 2026-09-25 (Wave 1 C3): symlink the repo `lib/` into the fixture dir
    /// so prelude-inserted `std/…` imports resolve — round-1 search probes
    /// `lib/` under the source dir, which a bare TempDir lacks. Tests that
    /// never trigger a prelude don't call it.
    fn link_repo_lib(dir: &TempDir) {
        let repo_lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
        std::os::unix::fs::symlink(repo_lib, dir.path().join("lib"))
            .expect("symlink repo lib into fixture");
    }

    /// 2026-09-25 (Wave 1 C3): the production factory the compile/check
    /// call sites install (`pipeline::module_plugin_factory`), default opts.
    fn default_factory() -> Box<dyn Fn(&str) -> Result<crate::plugin::PluginManager, String>> {
        crate::pipeline::module_plugin_factory(&crate::pipeline::BuildOptions::default())
    }

    #[test]
    fn test_resolve_empty_import() {
        let items = import_program("", vec![]);
        let mut resolver = ImportResolver::new();
        let result = resolver.resolve_imports(items, &PathBuf::from("main.bv")).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_resolve_bv_file() {
        let dir = TempDir::new().unwrap();
        let bv_path = dir.path().join("test_module.bv");
        fs::write(&bv_path, "defn hello -> Int { term 42; };").unwrap();

        let items = import_program("test_module", vec![]);
        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();
        let result = resolver.resolve_imports(items, &src).unwrap();
        let defns: Vec<&TopLevel> = result.iter().filter(|i| matches!(i, TopLevel::Definition(_))).collect();
        assert_eq!(defns.len(), 1);
    }

    #[test]
    fn test_resolve_checked_cached_modules() {
        let dir = TempDir::new().unwrap();
        let bv_path = dir.path().join("cache_test.bv");
        fs::write(&bv_path, "defn cached -> Int { term 1; };").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        let items = import_program("cache_test", vec![]);
        resolver.resolve_imports(items, &src).unwrap();
        assert!(resolver.loaded_modules.contains_key("cache_test"));
    }

    #[test]
    fn test_import_css_file() {
        let dir = TempDir::new().unwrap();
        let css_path = dir.path().join("styles.m.css");
        fs::write(&css_path, "body { color: red; }").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let items = import_program("styles.m.css", vec![]);
        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        let result = resolver.resolve_imports(items, &src).unwrap();
        assert!(result.iter().any(|i| matches!(i, TopLevel::Stylesheet(_))));
    }

    #[test]
    fn test_filter_items_by_name() {
        let dir = TempDir::new().unwrap();
        let bv_path = dir.path().join("filter_mod.bv");
        fs::write(&bv_path, "defn keep -> Int { term 1; };\ndefn discard -> Int { term 2; };").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        let items = import_program("filter_mod", vec!["keep".into()]);
        let result = resolver.resolve_imports(items, &src).unwrap();
        let names: Vec<&str> = result.iter().filter_map(|i| match i {
            TopLevel::Definition(d) => Some(d.name.as_str()),
            _ => None,
        }).collect();
        assert_eq!(names, vec!["keep"]);
    }

    #[test]
    fn test_filter_items_empty() {
        let dir = TempDir::new().unwrap();
        let bv_path = dir.path().join("full_mod.bv");
        fs::write(&bv_path, "defn a -> Int { term 0; }; defn b -> Int { term 0; };").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        let items = import_program("full_mod", vec![]);
        let result = resolver.resolve_imports(items, &src).unwrap();
        let count = result.iter().filter(|i| matches!(i, TopLevel::Definition(_))).count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_resolve_module_not_found() {
        let items = import_program("nonexistent_mod", vec![]);
        let mut resolver = ImportResolver::new();
        let src = PathBuf::from("/tmp/main.bv");
        let result = resolver.resolve_imports(items, &src);
        assert!(result.is_err());
    }

    #[test]
    fn test_glob_path_import_is_rejected() {
        // 2026-08-22 (Phase 1a): directory-glob paths are invalid (SPEC §7.2).
        // The old resolver expanded "std/core/*" into per-file imports; the
        // spec forbids globs outright, so any `*` in the path is now an error.
        let dir = TempDir::new().unwrap();
        let stdlib_root = dir.path().join("lib");
        let core_dir = stdlib_root.join("std").join("core");
        fs::create_dir_all(&core_dir).unwrap();
        fs::write(core_dir.join("a.bv"), "defn a_fn -> Int { term 1; };").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        for pattern in ["std/core/*", "./**"] {
            let items = vec![TopLevel::Import(Import::literal(pattern, vec![]))];
            let mut resolver = ImportResolver::new();
            resolver.add_search_path(stdlib_root.clone());
            let result = resolver.resolve_imports(items, &src);
            assert!(result.is_err(), "glob path '{}' must be rejected", pattern);
            assert!(
                result.err().unwrap().contains("glob"),
                "rejection must name the glob rule"
            );
        }
    }

    #[test]
    fn test_auto_core_injection() {
        let dir = TempDir::new().unwrap();
        let stdlib_root = dir.path().join("lib");
        let core_dir = stdlib_root.join("std").join("core");
        let types_dir = stdlib_root.join("std").join("types");
        fs::create_dir_all(&core_dir).unwrap();
        fs::create_dir_all(&types_dir).unwrap();
        fs::write(core_dir.join("ptr.bv"), "defn p_fn -> Int { term 0; };").unwrap();
        fs::write(core_dir.join("string_builder.bv"), "defn s_fn -> Int { term 0; };").unwrap();
        // 2026-09-01: canonical spellings (BUGS.md "Bits tripwire") — `spec
// MaxBits: N;` metadata, not the legacy `maxbits <~ N;` grammar.
fs::write(types_dir.join("bootstrap.bv"), "type Int : Bits { spec MaxBits: 64; }; type Float : Bits { spec MaxBits: 32; };").unwrap();
        let os_dir = stdlib_root.join("std").join("os");
        fs::create_dir_all(&os_dir).unwrap();
        for module in &["fs.bv", "net.bv", "signal.bv", "ipc.bv", "thread.bv", "dir.bv",
                        "process.bv", "tty.bv", "user.bv", "time.bv", "mem.bv", "rand.bv",
                        "sched.bv", "resource.bv", "sysinfo.bv", "temp.bv", "dynlib.bv",
                        "debug.bv", "ring.bv", "atomic.bv", "io.bv"] {
            fs::write(os_dir.join(module), "").unwrap();
        }
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let items = import_program("", vec![]);
        let mut resolver = ImportResolver::new()
            .with_stdlib_path(Some(stdlib_root));
        let result = resolver.resolve_imports(items, &src).unwrap();
        let defns: Vec<&TopLevel> = result.iter().filter(|i| matches!(i, TopLevel::Definition(_))).collect();
        // Prelude injection is now handled by the plugin system, not the resolver.
        // Definitions come from explicit imports, not auto-injection.
        // This test is preserved as a smoke test that resolve_imports doesn't crash.
        assert!(true);
    }

    #[test]
    fn test_import_target_board_directory() {
        // 2026-08-03 (Phase 2): `import "target"` reads the board directory
        // (map.dbv + addresses.dbvl + registers.dbvl) and flattens constants.
        let dir = TempDir::new().unwrap();
        let board_dir = dir.path().join("lib").join("boards").join("stm32f407");
        fs::create_dir_all(&board_dir).unwrap();
        fs::write(
            board_dir.join("map.dbv"),
            "schema Device { base_addr: String; size: Int; };\n",
        )
        .unwrap();
        fs::write(
            board_dir.join("addresses.dbvl"),
            ">schema Device from \"map.dbv\"\nUART1: 0x40011000; 0x18;\nGPIOA: 0x40020000; 0x400;\n",
        )
        .unwrap();
        fs::write(
            board_dir.join("registers.dbvl"),
            ">schema Device from \"map.dbv\"\nUART1_DR: 0x00; 9; rw;\n",
        )
        .unwrap();

        let items = import_program("target", vec![]);
        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let result = resolver.resolve_imports(items, &src).unwrap();

        // The board directory loads without error and emits the address
        // constant; set_active_board ran (address resolver sees UART1).
        assert!(result.len() > 0);
        let constant_names: Vec<String> = result
            .iter()
            .filter_map(|i| match i {
                TopLevel::Constant(c) => Some(c.name.clone()),
                _ => None,
            })
            .collect();
        assert!(
            constant_names.iter().any(|n| n.contains("UART1")),
            "expected UART1-derived constants, got {constant_names:?}"
        );
        assert_eq!(crate::address_resolver::resolve_address("uart1"), 0x40011000);
    }


#[test]
fn test_selective_rename_binds_local_name() {
    // import { Local: Exported } — the module's `Exported` is bound as `Local`.
    let dir = TempDir::new().unwrap();
    let bv = dir.path().join("rename_mod.bv");
    fs::write(&bv, "defn Exported -> Int { term 7; };").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![TopLevel::Import(Import::literal(
        "rename_mod.bv".to_string(),
        vec![("Local".to_string(), "Exported".to_string())],
    ))];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let result = resolver.resolve_imports(items, &src).unwrap();
    assert!(
        result.iter().any(|i| matches!(i, TopLevel::Definition(d) if d.name == "Local")),
        "the imported defn must be renamed to Local; got: {:?}",
        result.iter().map(|i| item_name(i).unwrap_or("?")).collect::<Vec<_>>()
    );
    assert!(
        !result.iter().any(|i| matches!(i, TopLevel::Definition(d) if d.name == "Exported")),
        "the original exported name must not leak"
    );
}

#[test]
fn test_import_collision_is_an_error() {
    // Two different modules both exporting `foo` is a hard error (SPEC 7.2).
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("m1.bv"), "defn foo -> Int { term 1; };").unwrap();
    fs::write(dir.path().join("m2.bv"), "defn foo -> Int { term 2; };").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![
        TopLevel::Import(Import::literal("m1.bv".to_string(), vec![])),
        TopLevel::Import(Import::literal("m2.bv".to_string(), vec![])),
    ];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let err = resolver.resolve_imports(items, &src).unwrap_err();
    assert!(err.contains("conflicts"), "expected a collision error, got: {err}");
}

#[test]
fn test_rename_resolves_collision() {
    // Renaming one import resolves the collision.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("m1.bv"), "defn foo -> Int { term 1; };").unwrap();
    fs::write(dir.path().join("m2.bv"), "defn foo -> Int { term 2; };").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![
        TopLevel::Import(Import::literal("m1.bv".to_string(), vec![])),
        TopLevel::Import(Import::literal(
            "m2.bv".to_string(),
            vec![("renamed".to_string(), "foo".to_string())],
        )),
    ];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let result = resolver.resolve_imports(items, &src).unwrap();
    assert!(result.iter().any(|i| matches!(i, TopLevel::Definition(d) if d.name == "renamed")));
}

#[test]
fn test_glob_import_is_rejected() {
    // import * from "x" is invalid (SPEC 7.2).
    let src = r#"import * from "m.bv";"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    assert!(
        p.parse_program().is_err(),
        "glob imports must be rejected"
    );
}

#[test]
fn test_export_import_propagates() {
    // A module that re-exports (export import) provides the names to importers.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("internal.bv"), "defn pub -> Int { term 5; };").unwrap();
    fs::write(
        dir.path().join("facade.bv"),
        "export import { pub } from \"internal.bv\";",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![TopLevel::Import(Import::literal("facade.bv".to_string(), vec![]))];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let result = resolver.resolve_imports(items, &src).unwrap();
    assert!(
        result.iter().any(|i| matches!(i, TopLevel::Definition(d) if d.name == "pub")),
        "the re-exported defn must be visible to importers"
    );
}

#[test]
fn test_identical_duplicate_imports_do_not_conflict() {
    // Two modules declaring the SAME constant (e.g. SYS_WRITE in fs.bv +
    // net.bv) are a benign duplicate, not a collision.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("d1.bv"), "const SYS_WRITE: Int = 4;").unwrap();
    fs::write(dir.path().join("d2.bv"), "const SYS_WRITE: Int = 4;").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![
        TopLevel::Import(Import::literal("d1.bv".to_string(), vec![])),
        TopLevel::Import(Import::literal("d2.bv".to_string(), vec![])),
    ];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    assert!(
        resolver.resolve_imports(items, &src).is_ok(),
        "identical duplicate definitions must not conflict"
    );
}

/// 2026-08-16 (Phase 3c): a named import must ALSO bring the requested defn's
/// transitive referenced FUNCTIONS — `import { iter_map } from "iterator.bv"`
/// pulls `iter_map_loop` (the helper txn iter_map's body calls). Before this
/// fix the closure only pulled referenced TYPE names, so the helper was
/// dropped, the call resolved to the raw-type fallback (return became Int),
/// and the generic adapter body failed to typecheck.
#[test]
fn test_named_import_pulls_transitive_function_deps() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mod.bv"),
        "txn helper_loop(list: List<Int>, acc: Int, i: Int) [i < list.Count#()][i == list.Count#()] -> Int {\n\
             let _ = acc;\n\
             term i;\n\
         };\n\
         defn use_helper(list: List<Int>) -> Int {\n\
             term helper_loop(list, 0, 0);\n\
         };\n",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![TopLevel::Import(Import::literal(
        "mod.bv".to_string(),
        vec![("use_helper".to_string(), "use_helper".to_string())],
    ))];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let result = resolver.resolve_imports(items, &src).unwrap();
    assert!(
        result.iter().any(|i| matches!(i, TopLevel::Transaction(t) if t.name == "helper_loop")),
        "importing a defn must pull the helper txn it calls (transitive function dep); got: {:?}",
        result.iter().filter_map(|i| match i {
            TopLevel::Definition(d) => Some(d.name.clone()),
            TopLevel::Transaction(t) => Some(t.name.clone()),
            _ => None,
        }).collect::<Vec<_>>()
    );
}

/// 2026-09-09 (parity spike): a helper called ONLY from a `when` GUARD
/// position must still be pulled by the transitive import closure. Before
/// this fix `referenced_function_names` recursed into Guarded bodies but not
/// the guard CONDITION, so `when value_ge_10(acc, b) { ... }` dropped the
/// helper and the backend emitted an undefined `@value_ge_10`.
#[test]
fn test_named_import_pulls_guard_condition_deps() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mod.bv"),
        "defn helper(x: Int) [x >= 0][term == true || term == false] -> Bool {\n\
             term x > 0;\n\
         };\n\
         defn use_helper(x: Int) -> Int {\n\
             when helper(x) {\n\
                 term 1;\n\
             };\n\
             term 0;\n\
         };\n",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![TopLevel::Import(Import::literal(
        "mod.bv".to_string(),
        vec![("use_helper".to_string(), "use_helper".to_string())],
    ))];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let result = resolver.resolve_imports(items, &src).unwrap();
    assert!(
        result.iter().any(|i| matches!(i, TopLevel::Definition(d) if d.name == "helper")),
        "a defn called only from a when-guard condition must be pulled by the import closure; got: {:?}",
        result.iter().filter_map(|i| match i {
            TopLevel::Definition(d) => Some(d.name.clone()),
            _ => None,
        }).collect::<Vec<_>>()
    );
}

/// 2026-08-09 (Phase 11, Slice 2): an `impl T` extends the type `T` — it does
/// NOT declare it. `type Point` in a.bv + `impl Point` in b.bv, both imported,
/// is a VALID cross-module coherence pair (§17.2); the impl must not collide
/// with the type it targets.
#[test]
fn test_cross_module_impl_does_not_collide_with_target_type() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("ty.bv"), "type Point: Int;").unwrap();
    fs::write(dir.path().join("impl.bv"), "impl Point { defn origin() -> Int { term 0; }; };").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![
        TopLevel::Import(Import::literal("ty.bv".to_string(), vec![])),
        TopLevel::Import(Import::literal("impl.bv".to_string(), vec![])),
    ];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let result = resolver.resolve_imports(items, &src).unwrap();
    assert!(
        result.iter().any(|i| matches!(i, TopLevel::Impl(imp) if imp.target == "Point")),
        "the impl must survive import (coherence pair)"
    );
    assert!(
        result.iter().any(|i| matches!(i, TopLevel::TypeDef(t) if t.name == "Point")),
        "the type must survive import"
    );
}

/// 2026-08-09 (Phase 11, Slice 2): a `:` module alias resolves a name collision
/// between two DIFFERENT modules exporting the same symbol (SPEC §7.2).
#[test]
fn test_module_alias_resolves_collision() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("m1.bv"), "defn foo -> Int { term 1; };").unwrap();
    fs::write(dir.path().join("m2.bv"), "defn foo -> Int { term 2; };").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    // `import a: "m1.bv"; import b: "m2.bv";` — both export `foo` but carry
    // DIFFERENT aliases, so they coexist (no qualified access — inlined tags).
    let mut a = Import::literal("m1.bv".to_string(), vec![]);
    a.alias = Some("a".to_string());
    let mut b = Import::literal("m2.bv".to_string(), vec![]);
    b.alias = Some("b".to_string());
    let items = vec![
        TopLevel::Import(a),
        TopLevel::Import(b),
    ];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    assert!(
        resolver.resolve_imports(items, &src).is_ok(),
        "differing module aliases must resolve the collision"
    );
}

/// 2026-08-09 (Phase 11, Slice 2): same-alias imports of the same exported
/// name from DIFFERENT modules STILL collide (the alias is per-import).
#[test]
fn test_same_alias_still_collides() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("m1.bv"), "defn foo -> Int { term 1; };").unwrap();
    fs::write(dir.path().join("m2.bv"), "defn foo -> Int { term 2; };").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let mut a = Import::literal("m1.bv".to_string(), vec![]);
    a.alias = Some("a".to_string());
    let mut b = Import::literal("m2.bv".to_string(), vec![]);
    b.alias = Some("a".to_string()); // same tag → still a collision
    let items = vec![
        TopLevel::Import(a),
        TopLevel::Import(b),
    ];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    assert!(
        resolver.resolve_imports(items, &src).is_err(),
        "two imports with the SAME alias must still collide"
    );
}

/// 2026-08-09 (Phase 11, Slice 2): resolution records the deterministic
/// (specifier → resolved path) map (SPEC §7.1).
#[test]
fn test_resolved_paths_are_recorded() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("m.bv"), "defn foo -> Int { term 1; };").unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();
    let items = vec![
        TopLevel::Import(Import::literal("m.bv".to_string(), vec![])),
    ];
    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    resolver.resolve_imports(items, &src).unwrap();
    assert_eq!(resolver.resolved_paths.len(), 1);
    assert_eq!(resolver.resolved_paths[0].0, "m.bv");
    assert!(
        resolver.resolved_paths[0].1.ends_with("m.bv"),
        "the record must map the specifier to its canonical path: {:?}",
        resolver.resolved_paths
    );
}

// ── Wave 1 C1: provenance tests (2026-09-25) ─────────────────────────

/// Helper: the origin recorded for the named `defn` in the resolved items.
fn definition_origin(
    items: &[TopLevel],
    origins: &[Option<u32>],
    name: &str,
) -> Option<Option<u32>> {
    items
        .iter()
        .zip(origins)
        .find(|(item, _)| matches!(item, TopLevel::Definition(d) if d.name == name))
        .map(|(_, origin)| *origin)
}

/// A→B→C: C's items must carry C's module id, not B's — provenance
/// survives the two-level splice chain (each `resolve_imports_inner`
/// level returns its own aligned origins that the parent splices).
#[test]
fn test_provenance_survives_two_level_chain() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mod_c.bv"),
        "defn from_c -> Int { term 3; };",
    )
    .unwrap();
    fs::write(
        dir.path().join("mod_b.bv"),
        "import \"mod_c\";\ndefn from_b -> Int { term 2; };",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();

    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let items = import_program("mod_b", vec![]);
    let result = resolver.resolve_imports(items, &src).unwrap();
    let origins = &resolver.item_origins;

    assert_eq!(result.len(), origins.len(), "origins must align with items");

    let origin_c = definition_origin(&result, origins, "from_c")
        .expect("from_c must be spliced into the result");
    let origin_b = definition_origin(&result, origins, "from_b")
        .expect("from_b must be spliced into the result");
    let id_c = origin_c.expect("imported items must carry a module id");
    let id_b = origin_b.expect("imported items must carry a module id");
    assert_ne!(id_c, id_b, "C's items must not be attributed to B");
    assert_eq!(resolver.modules[id_c as usize].specifier, "mod_c");
    assert!(
        resolver.modules[id_c as usize]
            .source_path
            .file_name()
            .unwrap()
            .eq("mod_c.bv"),
        "record points at C's file: {:?}",
        resolver.modules[id_c as usize].source_path
    );
    assert_eq!(resolver.modules[id_b as usize].specifier, "mod_b");
    assert_eq!(
        resolver.modules.len(),
        2,
        "root file is not a module; exactly B and C register"
    );
}

/// Root-file items carry `None` — provenance distinguishes "written here"
/// from "imported from module id".
#[test]
fn test_provenance_root_items_are_none() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mod_m.bv"),
        "defn imported_fn -> Int { term 1; };",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();

    let root_src = "defn root_fn -> Int { term 0; };";
    let tokens = lex_source(root_src).unwrap();
    let mut parser = crate::parser::Parser::new(tokens, root_src);
    let mut root_items = parser.parse_program().unwrap();
    root_items.push(TopLevel::Import(Import::literal(
        "mod_m".to_string(),
        vec![],
    )));

    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let result = resolver.resolve_imports(root_items, &src).unwrap();
    let origins = &resolver.item_origins;

    assert_eq!(
        definition_origin(&result, origins, "root_fn"),
        Some(None),
        "root-file items carry no module id"
    );
    let imported = definition_origin(&result, origins, "imported_fn")
        .expect("imported_fn must be spliced")
        .expect("imported items must carry a module id");
    assert_eq!(resolver.modules[imported as usize].specifier, "mod_m");
}

/// Diamond (X and Y both import C): the cached C loads ONCE (one module
/// record) and both splice sites carry the SAME id. `let` statements have
/// no dedup key, so both site copies survive into the final items — the
/// only shape where "both sites, same id" is observable after dedup.
#[test]
fn test_provenance_cache_shared_origins() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mod_c.bv"),
        "let shared_val: Int = 7;\ndefn from_c -> Int { term 3; };",
    )
    .unwrap();
    fs::write(
        dir.path().join("mod_x.bv"),
        "import \"mod_c\";\ndefn from_x -> Int { term 1; };",
    )
    .unwrap();
    fs::write(
        dir.path().join("mod_y.bv"),
        "import \"mod_c\";\ndefn from_y -> Int { term 2; };",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();

    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let items = vec![
        TopLevel::Import(Import::literal("mod_x".to_string(), vec![])),
        TopLevel::Import(Import::literal("mod_y".to_string(), vec![])),
    ];
    let result = resolver.resolve_imports(items, &src).unwrap();
    let origins = &resolver.item_origins;
    assert_eq!(result.len(), origins.len(), "origins must align with items");

    let shared_ids: Vec<u32> = result
        .iter()
        .zip(origins)
        .filter(|(item, _)| {
            matches!(item, TopLevel::Statement(s) if matches!(s.as_ref(), crate::ast::Statement::Let { name, .. } if name == "shared_val"))
        })
        .map(|(_, origin)| origin.expect("imported let must carry a module id"))
        .collect();
    assert_eq!(
        shared_ids.len(),
        2,
        "both diamond splice sites survive dedup (no dedup key for lets)"
    );
    assert_eq!(
        shared_ids[0], shared_ids[1],
        "cache hit must reuse the SAME module id at both sites"
    );

    let c_records = resolver
        .modules
        .iter()
        .filter(|m| m.specifier == "mod_c")
        .count();
    assert_eq!(c_records, 1, "cached module must register exactly once");

    let origin_c = definition_origin(&result, origins, "from_c")
        .expect("from_c spliced")
        .expect("imported defn carries id");
    assert_eq!(
        resolver.modules[origin_c as usize].specifier,
        "mod_c",
        "shared id must point at C"
    );
}

// ── Wave 1 C2: extension dispatch tests (2026-09-25) ─────────────────

/// The real `.ebv` import candidate (item 3): an extension-less specifier
/// resolves an `.ebv` file when no `.bv` sibling exists, and the module
/// record classifies it as Electronics.
#[test]
fn test_ebv_module_imports() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("elec_mod.ebv"),
        "defn ebv_fn -> Int { term 1; };",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();

    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let items = import_program("elec_mod", vec![]);
    let result = resolver.resolve_imports(items, &src).unwrap();

    assert!(
        definition_origin(&result, &resolver.item_origins, "ebv_fn").is_some(),
        "the .ebv module's defn must be spliced: {:?}",
        result
    );
    let record = resolver
        .modules
        .iter()
        .find(|m| m.specifier == "elec_mod")
        .expect("elec_mod registers a module record");
    assert_eq!(
        record.kind,
        Some(SourceKind::Electronics),
        "record classifies by resolved path: {:?}",
        record.source_path
    );
    assert!(
        record.source_path.ends_with("elec_mod.ebv"),
        "record points at the .ebv file: {:?}",
        record.source_path
    );
}

/// The diagnostic's promise is truthful (item 1): it names ONLY the
/// extensions the search actually probes (`.bv`, `.ebv`) — no `.abv`,
/// `.rbv`, or `.sbv` — and carries the concrete fix (house style).
#[test]
fn test_diagnostic_names_only_searched_exts() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();

    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let items = import_program("ghost_mod", vec![]);
    let err = resolver.resolve_imports(items, &src).unwrap_err();

    assert!(
        err.contains(".{bv,ebv}") && err.contains("ghost_mod"),
        "error must name the searched extension set: {}",
        err
    );
    for unsearched in [".abv", ".rbv", ".sbv"] {
        assert!(
            !err.contains(unsearched),
            "error must not promise unsearched ext {}: {}",
            unsearched,
            err
        );
    }
    assert!(err.contains("Fix:"), "error must carry the fix: {}", err);
}

/// An explicit `.rbv` specifier imports the file, and only its Briev
/// remainder parses — markup and style stay with the view pipeline.
#[test]
fn test_rbv_module_imports_briev_remainder() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("views.rbv"),
        "<view><div>ok</div></view>\n<style>.a{color:red;}</style>\n\
         defn rbv_fn -> Int { term 2; };\n",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();

    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let items = vec![TopLevel::Import(Import::literal(
        "views.rbv".to_string(),
        vec![],
    ))];
    let result = resolver.resolve_imports(items, &src).unwrap();

    assert!(
        definition_origin(&result, &resolver.item_origins, "rbv_fn").is_some(),
        "the .rbv remainder's defn must be spliced: {:?}",
        result
    );
    let record = resolver
        .modules
        .iter()
        .find(|m| m.specifier == "views.rbv")
        .expect("views.rbv registers a module record");
    assert_eq!(record.kind, Some(SourceKind::Rendered));
}

/// Data dialects are not code imports (item 3's contract): an implicit
/// specifier over a `.dbv` file is rejected with the explanation and the
/// explicit-specifier fix (the `x.dbv` arm still loads data constants).
#[test]
fn test_dbv_import_rejected() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("registry_data.dbv"),
        "entry { kind: \"probe\" }",
    )
    .unwrap();
    let src = dir.path().join("main.bv");
    fs::write(&src, "").unwrap();

    let mut resolver = ImportResolver::new();
    resolver.add_search_path(dir.path().to_path_buf());
    let items = import_program("registry_data", vec![]);
    let err = resolver.resolve_imports(items, &src).unwrap_err();

    assert!(
        err.contains("data dialects are not code imports"),
        "error must state the data-dialect refusal: {}",
        err
    );
    assert!(
        err.contains("registry_data.dbv"),
        "error must point at the data file it found: {}",
        err
    );
    assert!(
        err.contains("import { name } from"),
        "error must carry the explicit data-import fix: {}",
        err
    );
}

    // ── Wave 1 C3: per-module dialect preludes ─────────────────────────

    /// An imported `.ebv` module runs the electronics prelude (the
    /// `config/targets.dbvl` row `.ebv → prelude-electronics`): its
    /// prelude-inserted `Import$("std/electronics.bv")` resolves and splices,
    /// proven by the stdlib module record, a spliced typedef, and the typedef's
    /// origin attributing to that record.
    #[test]
    fn test_ebv_import_gets_electronics_prelude() {
        let dir = TempDir::new().unwrap();
        link_repo_lib(&dir);
        // Typedef anchor — prelude-electronics' no-import fallback (E14a gate).
        fs::write(dir.path().join("elec_mod.ebv"), "type Probe { x: Int };").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        resolver.plugin_factory = Some(default_factory());
        let items = import_program("elec_mod", vec![]);
        let result = resolver.resolve_imports(items, &src).unwrap();

        let std_id = resolver
            .modules
            .iter()
            .position(|m| m.specifier == "std/electronics.bv")
            .unwrap_or_else(|| {
                panic!(
                    "module prelude must load std/electronics.bv: {:?}",
                    resolver.modules.iter().map(|m| &m.specifier).collect::<Vec<_>>()
                )
            }) as u32;
        assert!(
            resolver.modules[std_id as usize].kind.is_some(),
            "the stdlib record is file-backed and classifies: {:?}",
            resolver.modules[std_id as usize].source_path
        );
        let volt = result
            .iter()
            .position(|t| matches!(t, TopLevel::TypeDef(td) if td.name == "Volt"))
            .expect("std/electronics.bv items must splice into the program");
        assert_eq!(
            resolver.item_origins[volt],
            Some(std_id),
            "the spliced typedef carries the stdlib module's origin"
        );
    }

    /// An imported `.bv` module runs the native prelude (targets.dbvl
    /// `.bv → prelude-native …`): its import anchor triggers the std bundle
    /// splice — proven by the `std/io.bv` module record — while the module's
    /// OWN import still resolves.
    #[test]
    fn test_bv_import_gets_native_prelude() {
        let dir = TempDir::new().unwrap();
        link_repo_lib(&dir);
        fs::write(dir.path().join("dep.bv"), "defn dep_fn -> Int { term 1; };").unwrap();
        fs::write(dir.path().join("mod.bv"), "import \"dep\";").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        resolver.plugin_factory = Some(default_factory());
        let items = import_program("mod", vec![]);
        let result = resolver.resolve_imports(items, &src).unwrap();

        assert!(
            resolver.modules.iter().any(|m| m.specifier == "std/io.bv"),
            "native prelude's std bundle must load: {:?}",
            resolver.modules.iter().map(|m| &m.specifier).collect::<Vec<_>>()
        );
        assert!(
            definition_origin(&result, &resolver.item_origins, "dep_fn").is_some(),
            "the module's own import must still resolve: {:?}",
            result
        );
    }

    /// Diamond safety: two `.ebv` modules each trigger the electronics
    /// prelude — the stdlib module loads ONCE (one record, cache-shared) and
    /// its items survive dedup exactly once; both modules' own typedefs stay.
    #[test]
    fn test_prelude_not_double_spliced() {
        let dir = TempDir::new().unwrap();
        link_repo_lib(&dir);
        fs::write(dir.path().join("elec_a.ebv"), "type FooA { x: Int };").unwrap();
        fs::write(dir.path().join("elec_b.ebv"), "type FooB { x: Int };").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        resolver.plugin_factory = Some(default_factory());
        let items = vec![
            TopLevel::Import(Import::literal("elec_a", vec![])),
            TopLevel::Import(Import::literal("elec_b", vec![])),
        ];
        let result = resolver.resolve_imports(items, &src).unwrap();

        let std_recs = resolver
            .modules
            .iter()
            .filter(|m| m.specifier == "std/electronics.bv")
            .count();
        assert_eq!(std_recs, 1, "one load, one record: {:?}", resolver.modules);
        let volts = result
            .iter()
            .filter(|t| matches!(t, TopLevel::TypeDef(td) if td.name == "Volt"))
            .count();
        assert_eq!(volts, 1, "std items dedup to a single copy: {} copies", volts);
        for name in ["FooA", "FooB"] {
            assert!(
                result.iter().any(|t| matches!(t, TopLevel::TypeDef(td) if td.name == name)),
                "module {}'s own typedef must survive: {:?}",
                name,
                result
            );
        }
    }

    /// Regression guard for the C3 gate: with NO factory (library mode,
    /// pre-C3 callers) an `.ebv` module parses plain — no stdlib record, no
    /// spliced stdlib items — while its own items still splice.
    #[test]
    fn test_no_plugin_factory_is_todays_behavior() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("elec_mod.ebv"), "type Probe { x: Int };").unwrap();
        let src = dir.path().join("main.bv");
        fs::write(&src, "").unwrap();

        let mut resolver = ImportResolver::new();
        resolver.add_search_path(dir.path().to_path_buf());
        let items = import_program("elec_mod", vec![]);
        let result = resolver.resolve_imports(items, &src).unwrap();

        assert!(
            !resolver.modules.iter().any(|m| m.specifier.starts_with("std/")),
            "no prelude ran without a factory: {:?}",
            resolver.modules
        );
        assert!(
            result.iter().any(|t| matches!(t, TopLevel::TypeDef(td) if td.name == "Probe")),
            "the module's own typedef must splice: {:?}",
            result
        );
    }
}
