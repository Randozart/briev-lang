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

use crate::ast::{Expr, Import, ImportKind, TopLevel, Type};
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

pub struct ImportResolver {
    loaded_modules: HashMap<String, (Vec<TopLevel>, Vec<String>)>,
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

    pub fn resolve_imports(
        &mut self,
        items: Vec<TopLevel>,
        file_path: &PathBuf,
    ) -> Result<Vec<TopLevel>, String> {
        // Set root path from the main file's directory on first call
        if self.root_path == PathBuf::from(".") {
            self.root_path = file_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
        }

        let mut items = items;

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
            let resolved = self.resolve_import(&import, file_path)?;
            Self::record_imported_names(
                &mut imported_names,
                &mut imported_aliases,
                &resolved,
                import.path(),
                import.alias.as_deref(),
            )?;
            items.remove(index);
            items.splice(index..index, resolved);
        }

        // 2026-06-13: Dedup items
        items = dedup_items(items);

        Ok(items)
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
    ) -> Result<Vec<TopLevel>, String> {
        // Skip empty module paths
        if import.path().is_empty() {
            return Ok(vec![]);
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
            return self.resolve_target_import();
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
        if let Some((cached, sed_names)) = self.loaded_modules.get(import.path()) {
            return self.filter_items(cached, sed_names, &import.symbols);
        }

        // Check for CSS import
        if import.path().ends_with(".css") {
            let css_path = source_file
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
                .join(&import.path());

            if css_path.exists() {
                let css_content = std::fs::read_to_string(&css_path)
                    .map_err(|e| format!("Failed to read CSS '{}': {}", css_path.display(), e))?;
                let css_for_cache = css_content.clone();
                self.loaded_modules.insert(
                    import.path().to_string(),
                    (vec![TopLevel::Stylesheet(css_for_cache)], vec![]),
                );
                return Ok(vec![TopLevel::Stylesheet(css_content)]);
            }
        }

        // Check for SVG import
        if import.path().ends_with(".svg") {
            let svg_path = source_file
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
                .join(&import.path());

            if svg_path.exists() {
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
                self.loaded_modules.insert(
                    import.path().to_string(),
                    (vec![TopLevel::SvgComponent {
                        name: component_name.clone(),
                        content: svg_for_cache,
                    }], vec![]),
                );
                return Ok(vec![TopLevel::SvgComponent {
                    name: component_name,
                    content: svg_content,
                }]);
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

            self.loaded_modules.insert(
                import.path().to_string(),
                (program_for_cache, vec![]),
            );

            return Ok(dbriev_items);
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
                (vec![], vec![]),
            );
            return Ok(vec![]);
        }

        // Default: Briev module (.bv)
        let module_path = if import.path().ends_with(".bv") {
            import.path()[..import.path().len() - 3].replace('.', "/")
        } else {
            import.path().replace('.', "/")
        };
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

        // Search for .bv files in the search paths.
        let mut found_path = None;
        for search_dir in &self.search_paths {
            let bv_candidate = source_dir
                .join(search_dir)
                .join(format!("{}.bv", module_path));

            if bv_candidate.exists() {
                found_path = Some(bv_candidate);
                break;
            }
        }

        // Search from the project root's lib/ directory (findable from
        // anywhere) — std/*, glue/*, and any other lib/ module. The walk-up
        // stops at the first ancestor Cargo.toml (the compiler repo, or a
        // user project's root).
        if found_path.is_none() {
            let mut current = source_dir.clone();
            while let Some(parent) = current.parent() {
                if parent.join("Cargo.toml").exists() {
                    let std_path = parent.join("lib").join(format!("{}.bv", module_path));
                    if std_path.exists() {
                        found_path = Some(std_path);
                    }
                    break;
                }
                current = parent.to_path_buf();
            }
        }

        if found_path.is_none() {
            let direct_bv = source_dir.join(format!("{}.bv", module_path));
            if direct_bv.exists() {
                found_path = Some(direct_bv);
            }
        }

        let resolved_path = found_path.ok_or_else(|| {
            let dir = source_dir.display();
            format!(
                "Cannot find module '{}'. Searched in: \
                 {dir}/lib/{mp}.{{bv,ebv}}, \
                 {dir}/imports/{mp}.{{bv,ebv}}, \
                 {dir}/{mp}.{{bv,ebv}}",
                import.path(),
                mp = module_path,
            )
        })?;

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

        let tokens = lex_source(&source)?;
        let mut parser = crate::parser::Parser::new(tokens, &source);
        // 2026-07-14: Parse errors in imported files are non-fatal — the
        // imported file may use syntax (struct literals, etc.) that the
        // parser supports as AST but not yet as a fully parseable form.
        // 2026-08-04 (compiler-in-Briev): the error is NOT swallowed — it is
        // reported as a visible warning so a silently-empty import (which
        // drops a module's defns, e.g. std/string's `..` slices) is never
        // hidden again. The import still proceeds with the items that DID
        // parse (non-fatal, pre-merge behavior).
        let imported_program = match parser.parse_program() {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "warning: import '{}' at '{}' failed to fully parse: {}",
                    import.path(), resolved_path.display(), e
                );
                vec![]
            }
        };

        let resolved = self.resolve_imports(imported_program, &resolved_path)?;
        if import.path().contains("glue/c") {
        }

        // Cache the fully resolved program
        self.loaded_modules
            .insert(import.path().to_string(), (resolved.clone(), vec![]));

        let result = self.filter_items(&resolved, &[], &import.symbols);

        self.in_progress.remove(import.path());
        result
    }

    /// 2026-08-06 (Phase 11): filter imported items by the EXPORTED names and
    /// apply selective renames (`{ Local: Exported }`). Preserves the D3
    /// transitive-referenced-type closure and the file-private (sed) filter.
    fn filter_items(&self, items: &[TopLevel], sed_names: &[String], symbols: &[(String, String)]) -> Result<Vec<TopLevel>, String> {
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
        for item in items {
            if always(item) {
                out.push(item.clone());
                continue;
            }
            match Self::item_name(item) {
                Some(n) if keep.contains(n) => {
                    if let Some(local) = rename.get(n) {
                        out.push(Self::rename_item(item, local));
                    } else {
                        out.push(item.clone());
                    }
                }
                _ => {}
            }
        }
        Ok(out)
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
    ) -> Result<Vec<TopLevel>, String> {
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
        if let Some((cached, sed_names)) = self.loaded_modules.get(&cache_key) {
            return self.filter_items(cached, sed_names, &[]);
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

        let tokens = lex_source(&source)?;
        let mut parser = crate::parser::Parser::new(tokens, &source);
        let imported_program = parser.parse_program().unwrap_or_default();

        let resolved = self.resolve_imports(imported_program, &candidate)?;

        self.loaded_modules
            .insert(cache_key, (resolved.clone(), vec![]));

        self.filter_items(&resolved, &[], &[])
    }
}

/// Lex a source string into a token vector with span information.
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
fn dedup_items(items: Vec<TopLevel>) -> Vec<TopLevel> {
    use std::collections::HashMap;
    let mut last_indices: HashMap<(String, String), usize> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(key) = item_key(item) {
            last_indices.insert(key, i);
        }
    }
    let mut result = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        match item_key(&item) {
            Some(key) if last_indices[&key] == i => result.push(item),
            Some(_) => {}
            None => result.push(item),
        }
    }
    result
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
}
