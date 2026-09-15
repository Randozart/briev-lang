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

//! Canonical Briev source formatting.
//!
//! 2026-08-05 (normative spec Phase 2): this is the canonical formatter. Its
//! contract is **round-trip AST equivalence**: `parse(format(parse(source)))`
//! must equal `parse(source)`, and formatting must be idempotent
//! (`format(parse(format(parse(source)))) == format(parse(source))`).
//!
//! The formatter is separate from the debug `Display` impls (`src/ast/display.rs`).
//! Every syntax-family migration (Phases 3+) must be accompanied by formatter
//! support so macros, `briev fmt`, and the repository sweep emit canonical text.

use crate::ast::*;
use std::fmt::Write;

/// 2026-08-05: format a whole program (top-level items) canonically.
/// Each item is separated by a blank line; bodies are indented 4 spaces.
pub fn format_program(items: &[TopLevel]) -> String {
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_item_into(item, &mut out, 0);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("    ");
    }
}

/// 2026-08-05: format one top-level item.
pub fn format_item(item: &TopLevel) -> String {
    let mut out = String::new();
    format_item_into(item, &mut out, 0);
    out
}

fn format_item_into(item: &TopLevel, out: &mut String, level: usize) {
    match item {
        TopLevel::Import(import) => format_import_into(import, out, level),
        TopLevel::Export(export) => {
            indent(out, level);
            out.push_str("export ");
            format_item_into(&export.inner, out, 0);
        }
        TopLevel::Constant(c) => {
            indent(out, level);
            let _ = write!(out, "const {}: {} = {};", c.name, c.ty, c.expr);
        }
        TopLevel::StateDecl(s) => {
            indent(out, level);
            let _ = write!(out, "let {}: {};", s.name, s.ty);
        }
        TopLevel::Definition(defn) => format_callable(
            out,
            level,
            "defn",
            &Callable {
                name: &defn.name,
                parameters: &defn.parameters,
                output_type: defn.output_type.as_ref(),
                contract: &defn.contract,
                derivation: defn.derivation.as_ref(),
                body: &defn.body,
            },
        ),
        TopLevel::TypeDefOperator(defn) => format_callable(
            out,
            level,
            "op",
            &Callable {
                name: &defn.name,
                parameters: &defn.parameters,
                output_type: defn.output_type.as_ref(),
                contract: &defn.contract,
                derivation: defn.derivation.as_ref(),
                body: &defn.body,
            },
        ),
        TopLevel::CompileTimeDefn(defn) => format_callable(
            out,
            level,
            "$defn",
            &Callable {
                name: &defn.name,
                parameters: &defn.parameters,
                output_type: defn.output_type.as_ref(),
                contract: &defn.contract,
                derivation: defn.derivation.as_ref(),
                body: &defn.body,
            },
        ),
        TopLevel::Transaction(txn) => {
            let prefix = if txn.is_reactive { "node" } else { "txn" };
            let async_prefix = if txn.is_async { "async " } else { "" };
            format_callable(
                out,
                level,
                &format!("{}{}", async_prefix, prefix),
                &Callable {
                    name: &txn.name,
                    parameters: &txn.parameters,
                    output_type: txn.output_type.as_ref(),
                    contract: &txn.contract,
                    derivation: txn.derivation.as_ref(),
                    body: &txn.body,
                },
            )
        }
        TopLevel::CompileTimeTxn(txn) => format_callable(
            out,
            level,
            "$txn",
            &Callable {
                name: &txn.name,
                parameters: &txn.parameters,
                output_type: txn.output_type.as_ref(),
                contract: &txn.contract,
                derivation: txn.derivation.as_ref(),
                body: &txn.body,
            },
        ),
        TopLevel::StaticStruct(s) => {
            format_struct_into(out, level, s);
        }
        TopLevel::Enum(e) => format_enum_into(e, out, level),
        TopLevel::TypeDef(t) => format_typedef_into(t, out, level),
        TopLevel::Obj(obj) => {
            indent(out, level);
            let _ = write!(out, "obj {}", obj.name);
            if !obj.type_params.is_empty() {
                let _ = write!(out, "<{}>", obj.type_params.join(", "));
            }
            let _ = write!(out, " {{ ... }};");
        }
        TopLevel::Cell(cell) => {
            indent(out, level);
            let _ = write!(out, "cell {} {{ ... }};", cell.name);
        }
        TopLevel::Trigger(trg) => {
            indent(out, level);
            let _ = write!(out, "trg {} @ {};", trg.name, trg.instance);
        }
        TopLevel::TriggerBinding { name, instance, .. } => {
            indent(out, level);
            let _ = write!(out, "trg {} @ {};", name, instance);
        }
        TopLevel::AsmFn(f) => {
            indent(out, level);
            let _ = write!(out, "asm<{}> {}(", f.target, f.name);
            for (i, (name, ty)) in f.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{}: {}", name, ty);
            }
            let _ = write!(out, ")");
            let _ = write!(out, " -> {}", f.ret_type);
            let _ = write!(out, " {{ ");
            for instr in &f.body {
                let _ = write!(out, "\"{}\"; ", instr);
            }
            let _ = write!(out, "}};");
        }
        // 2026-09-06 (ISR plan): the canonical form re-renders the explicit
        // mechanism when present; the vector prints literal-or-name as
        // written (round-trip fidelity — the resolution happens at
        // typecheck, not in the printer).
        TopLevel::IsrHandler(h) => {
            indent(out, level);
            if let Some(mech) = &h.mechanism {
                let _ = write!(out, "isr<{}> handler @ ", mech);
            } else {
                out.push_str("isr handler @ ");
            }
            let _ = write!(out, "{}: {}(", h.vector, h.name);
            for (i, (name, ty)) in h.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{}: {}", name, ty);
            }
            let _ = write!(out, ")");
            let _ = write!(out, " {{ ");
            for stmt in &h.body {
                format_stmt_into(stmt, out, 0);
                out.push(' ');
            }
            let _ = write!(out, "}};");
        }
        TopLevel::Trait(t) => {
            indent(out, level);
            let _ = write!(out, "trait {}", t.name);
            if !t.type_params.is_empty() {
                let params: Vec<String> = t.type_params.iter().map(|p| p.name.clone()).collect();
                let _ = write!(out, "<{}>", params.join(", "));
            }
            let _ = write!(out, " {{ ");
            for (i, (fname, fty)) in t.fields.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                let _ = write!(out, "{}: {};", fname, fty);
            }
            for f in &t.functions {
                let _ = write!(out, " {};", TopLevel::Definition(f.clone()));
            }
            let _ = write!(out, " }};");
        }
        TopLevel::Impl(i) => {
            indent(out, level);
            let _ = write!(out, "impl {}", i.target);
            if !i.type_params.is_empty() {
                let params: Vec<String> = i.type_params.iter().map(|p| p.name.clone()).collect();
                let _ = write!(out, "<{}>", params.join(", "));
            }
            let _ = write!(out, " {{ ");
            for f in &i.functions {
                let _ = write!(out, " {};", TopLevel::Definition(f.clone()));
            }
            let _ = write!(out, " }};");
        }
        TopLevel::RenderBlock(r) => {
            indent(out, level);
            let _ = write!(out, "render {} {{ {} }};", r.struct_name, r.view_html);
        }
        TopLevel::ProtocolDef(p) => {
            indent(out, level);
            let _ = write!(out, "proto {}: {} {{ ... }};", p.name, p.category);
        }
        TopLevel::CompileTimeLet(name, expr) => {
            indent(out, level);
            let _ = write!(out, "$let {} = {};", name, expr);
        }
        TopLevel::CompileTimeConst(name, expr) => {
            indent(out, level);
            let _ = write!(out, "$const {} = {};", name, expr);
        }
        TopLevel::StageBlock(block) => {
            indent(out, level);
            let _ = write!(out, "$({:?}) {{ ", block.stage);
            for stmt in &block.body {
                let _ = write!(out, "{} ", stmt);
            }
            let _ = write!(out, "}};");
        }
        TopLevel::Statement(stmt) => {
            format_stmt_into(stmt, out, level);
        }
        TopLevel::Fuzzed { item, .. } => format_item_into(item, out, level),
        TopLevel::SyncGroup { domains, item } => {
            indent(out, level);
            let _ = write!(out, "sync<{}> {{ ", domains.join(","));
            format_item_into(item, out, level + 1);
            let _ = write!(out, "}};");
        }
        TopLevel::Signature(s) => {
            indent(out, level);
            let _ = write!(out, "sig {}(", s.name);
            for (i, (name, ty)) in s.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{}: {}", name, ty);
            }
            let _ = write!(out, ");");
        }
        TopLevel::LinkDependency(l) => {
            indent(out, level);
            let _ = write!(out, "link \"{}\";", l.path);
        }
        TopLevel::ModuleMetadata(meta) => {
            // Deterministic output: sort keys (HashMap iteration order varies).
            let mut keys: Vec<&String> = meta.keys().collect();
            keys.sort();
            for key in keys {
                indent(out, level);
                let _ = write!(out, "!> {}: {};", key, meta[key]);
            }
        }
        TopLevel::Init(init) => {
            indent(out, level);
            let _ = write!(out, "init {}", init.name);
            let _ = write!(out, ":");
            if let Some(bound) = &init.bound {
                let _ = write!(out, " [{}]", crate::ast::display::display_bound_set(bound));
            }
            let _ = write!(out, " {}", init.ty);
            if let Some(value) = &init.value {
                let _ = write!(out, " = {};", value);
            } else {
                let _ = write!(out, " {{ ");
                for stmt in &init.body {
                    let _ = write!(out, "{} ", stmt);
                }
                out.push_str("};");
            }
        }
        TopLevel::ResourceDecl(_)
        | TopLevel::ForeignBinding(_)
        | TopLevel::Codec(_)
        | TopLevel::Assertion { .. }
        | TopLevel::Stylesheet(_)
        | TopLevel::SvgComponent { .. }
        | TopLevel::Cfg(_) => {
            // These legacy/rare forms use the debug Display as a fallback.
            let _ = write!(out, "{}", item);
        }
    }
}

/// 2026-08-13 (layout-keywords plan Phase 5): the set of field names declared
/// `atomic`, from the structured `atomic_fields` metadata List. Shared by the
/// struct and TypeDef formatters so the `atomic` prefix prints for both.
/// 2026-09-06: Returns a map of field name → ordering ("seq", "relaxed",
/// "acquire", "release", "bartered") from the `atomic_fields` metadata.
/// Entries are stored as `"field_name:ordering"`.
fn atomic_field_map(metadata: &std::collections::HashMap<String, crate::ast::PropertyValue>) -> std::collections::HashMap<&str, &str> {
    match metadata.get("atomic_fields") {
        Some(crate::ast::PropertyValue::List(entries)) => entries
            .iter()
            .filter_map(|e| match e {
                crate::ast::PropertyValue::String(s) => {
                    let (name, ordering) = s.split_once(':').unwrap_or((s.as_str(), "seq"));
                    Some((name, ordering))
                }
                _ => None,
            })
            .collect(),
        _ => std::collections::HashMap::new(),
    }
}

fn format_struct_into(
    out: &mut String,
    level: usize,
    def: &crate::ast::top::StructDef,
) {
    indent(out, level);
    if def.seq {
        out.push_str("seq ");
    }
    // 2026-08-13 (layout-keywords plan): `pack struct` — bit-contiguous.
    // Packed layouts are order-independent of `seq`, so both may be present.
    if def.pack {
        out.push_str("pack ");
    }
    // 2026-08-13 (layout-keywords plan Phase 6): `union` is standalone —
    // no `struct` keyword, no seq/pack prefixes.
    if def.union {
        let _ = write!(out, "union {}", def.name);
    } else {
        let _ = write!(out, "struct {}", def.name);
    }
    if !def.type_params.is_empty() {
        let params: Vec<String> = def.type_params.iter().map(|p| p.name.clone()).collect();
        let _ = write!(out, "<{}>", params.join(", "));
    }
    let _ = write!(out, " {{ ");
    let atomic = atomic_field_map(&def.metadata);
    for (i, (fname, fty)) in def.fields.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if let Some(ordering) = atomic.get(fname.as_str()) {
            if *ordering != "seq" {
                out.push_str(ordering);
                out.push(' ');
            }
            out.push_str("atomic ");
        }
        let _ = write!(out, "{}: {};", fname, fty);
    }
    // 2026-08-13 (layout-keywords plan): struct physical-layout metadata prints
    // in the declared form. Deterministic: sorted keys.
    print_metadata_clauses(out, &def.metadata);
    let _ = write!(out, " }};");
}

/// 2026-08-13 (layout-keywords plan): print a type/struct body's metadata —
/// physical-layout keys in the declared `spec <PascalCase>` form, everything
/// else as `!> <key>: <value>`. Deterministic: sorted keys. The internal
/// `atomic_fields` carrier is skipped (the `atomic` field prefix encodes it).
fn print_metadata_clauses(out: &mut String, metadata: &std::collections::HashMap<String, crate::ast::PropertyValue>) {
    let mut meta_keys: Vec<&String> = metadata.keys().collect();
    meta_keys.sort();
    for key in meta_keys {
        if key == "atomic_fields" {
            continue;
        }
        out.push(' ');
        if let Some(spec_name) = spec_display_key(key) {
            let _ = write!(out, "spec {}: {};", spec_name, metadata[key]);
        } else {
            let _ = write!(out, "!> {}: {};", key, metadata[key]);
        }
    }
}

/// 2026-08-13 (layout-keywords plan): lowercase metadata key → PascalCase spec
/// name (the canonical formatter's half of `spec_name_to_key` in the parser —
/// keep in sync). Non-layout keys return None → printed as `!>`.
fn spec_display_key(key: &str) -> Option<&'static str> {
    match key {
        "alignment" => Some("Alignment"),
        "bits" => Some("Bits"),
        "maxbits" => Some("MaxBits"),
        "bytes" => Some("Bytes"),
        "endian" => Some("Endian"),
        // 2026-09-14 (Matrix type plan): shape keys (the parser's
        // spec_name_to_key half — keep in sync).
        "rows" => Some("Rows"),
        "cols" => Some("Cols"),
        "depth" => Some("Depth"),
        _ => None,
    }
}

// 2026-08-22 (Phase 4a): statement-match patterns are the unified
// `ast::expr::Pattern`; Display already renders each, `|` joins here.

/// 2026-08-05: format an enum variant in canonical form.
fn format_enum_variant(v: &EnumVariant) -> String {
    match v {
        EnumVariant::Unit(name) => name.clone(),
        EnumVariant::Tuple(name, types) => {
            let tys: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            format!("{}({})", name, tys.join(", "))
        }
        EnumVariant::Struct(name, fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("{}: {}", n, t))
                .collect();
            format!("{}({})", name, fields.join(", "))
        }
    }
}

/// 2026-08-05: the parts of a callable declaration that the canonical
/// formatter needs. Bundled to keep `format_callable` under the parameter
/// limit (Praetor Rule 4).
struct Callable<'a> {
    name: &'a str,
    parameters: &'a [(String, Type)],
    output_type: Option<&'a OutputType>,
    contract: &'a Contract,
    derivation: Option<&'a DerivationBlock>,
    body: &'a [Statement],
}

/// 2026-08-05: format a callable declaration (`defn`, `txn`, `node`, `$defn`).
fn format_callable(
    out: &mut String,
    level: usize,
    keyword: &str,
    c: &Callable,
) {
    indent(out, level);
    let _ = write!(out, "{} {}", keyword, c.name);
    if !c.parameters.is_empty() {
        out.push('(');
        for (i, (pname, pty)) in c.parameters.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "{}: {}", pname, pty);
        }
        out.push(')');
    }
    if let Some(oty) = c.output_type {
        let _ = write!(out, " -> {}", oty);
    }
    let _ = write!(out, " {}", format_contract(c.contract));
    if let Some(deriv) = c.derivation {
        let _ = write!(out, " {}", deriv);
    }
    let _ = write!(out, " {{");
    if !c.body.is_empty() {
        out.push('\n');
        format_block(c.body, level + 1, out);
        indent(out, level);
    } else {
        out.push(' ');
    }
    let _ = write!(out, "}};");
}

/// 2026-08-05: format a braced block statement: `<header> { body };`.
/// `header` already includes the opening brace.
fn format_braced(out: &mut String, level: usize, header: &str, body: &[Statement]) {
    indent(out, level);
    out.push_str(header);
    if !body.is_empty() {
        out.push('\n');
        format_block(body, level + 1, out);
        indent(out, level);
    } else {
        out.push(' ');
    }
    let _ = write!(out, "}};");
}

/// 2026-08-05: format a statement block with indentation. Leaf statements use
/// the debug `Display`; block-like statements recurse.
fn format_block(stmts: &[Statement], level: usize, out: &mut String) {
    for stmt in stmts {
        format_stmt_into(stmt, out, level);
    }
}

fn format_stmt_into(stmt: &Statement, out: &mut String, level: usize) {
    match stmt {
        Statement::Guarded(cond, body) => {
            format_braced(out, level, &format!("when {} {{", cond), body);
        }
        Statement::Foreach { item, list, body } => {
            format_braced(out, level, &format!("foreach({} in {}) {{", item, list), body);
        }
        Statement::Block(body) => format_braced(out, level, "{", body),
        Statement::SyncBlock(body) => format_braced(out, level, "sync {", body),
        Statement::Match { expr, arms } => {
            indent(out, level);
            let _ = write!(out, "match {} {{", expr);
            for arm in arms {
                out.push('\n');
                indent(out, level + 1);
                let _ = write!(
                    out,
                    "{} =>",
                    arm
                        .patterns
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
                if !arm.body.is_empty() {
                    out.push('\n');
                    format_block(&arm.body, level + 2, out);
                    indent(out, level + 1);
                } else {
                    out.push_str(" {}");
                }
            }
            out.push('\n');
            indent(out, level);
            let _ = write!(out, "}};");
        }
        Statement::InlineDefn(defn) => format_callable(
            out,
            level,
            "$defn",
            &Callable {
                name: &defn.name,
                parameters: &defn.parameters,
                output_type: defn.output_type.as_ref(),
                contract: &defn.contract,
                derivation: defn.derivation.as_ref(),
                body: &defn.body,
            },
        ),
        Statement::InlineTxn(txn) => format_callable(
            out,
            level,
            "$txn",
            &Callable {
                name: &txn.name,
                parameters: &txn.parameters,
                output_type: txn.output_type.as_ref(),
                contract: &txn.contract,
                derivation: txn.derivation.as_ref(),
                body: &txn.body,
            },
        ),
        _ => {
            indent(out, level);
            let _ = write!(out, "{}", stmt);
        }
    }
    out.push('\n');
}

/// 2026-08-05: canonical contract rendering. Always emits `[pre][post]` (an
/// omitted contract becomes `[true][true]` — still idempotent because the
/// parser sets `explicit=false` but the formatter output always carries the
/// brackets). Watchdogs are emitted in a deterministic unit form.
pub fn format_contract(c: &Contract) -> String {
    let mut out = String::new();
    let _ = write!(out, "[{}][{}]", c.pre_condition, c.post_condition);
    if let Some(w) = &c.watchdog {
        format_watchdog(&mut out, w);
    }
    out
}

/// 2026-08-05: format a watchdog clause after the pre/post brackets.
fn format_watchdog(out: &mut String, w: &WatchdogSpec) {
    out.push(if w.is_required { '!' } else { '?' });
    let _ = write!(out, "[{}]", w.condition);
    if let Some(cyc) = w.cycles_bound {
        let _ = write!(out, " within {} cyc", cyc);
    } else if let Some(ns) = w.deadline_ns {
        format_deadline(out, ns);
    }
    if let Some(on_fire) = &w.on_fire {
        let _ = write!(out, " -> {}", on_fire.handler);
        if let Some(arg) = &on_fire.arg {
            let _ = write!(out, "({})", arg);
        } else {
            out.push_str("()");
        }
    }
}

/// 2026-08-05: deterministic unit selection for a deadline in nanoseconds.
/// Reparse of the emitted unit yields the same `deadline_ns`, so formatting
/// is idempotent.
fn format_deadline(out: &mut String, ns: u64) {
    if ns % 60_000_000_000 == 0 {
        let _ = write!(out, " within {} min", ns / 60_000_000_000);
        return;
    }
    if ns % 1_000_000_000 == 0 {
        let _ = write!(out, " within {} s", ns / 1_000_000_000);
        return;
    }
    if ns % 1_000_000 == 0 {
        let _ = write!(out, " within {} ms", ns / 1_000_000);
        return;
    }
    let _ = write!(out, " within {} ns", ns);
}

/// 2026-08-05: format an `import` item.
fn format_import_into(import: &Import, out: &mut String, level: usize) {
    indent(out, level);
    let symbols: Vec<String> = import
        .symbols
        .iter()
        .map(|(l, e)| if l == e { l.clone() } else { format!("{}: {}", l, e) })
        .collect();
    let symbols = symbols.join(", ");
    match &import.kind {
        ImportKind::Literal(path) => {
            if symbols.is_empty() {
                let _ = write!(out, "import \"{}\";", path);
            } else {
                let _ = write!(out, "import {{ {} }} from \"{}\";", symbols, path);
            }
        }
        ImportKind::Registry(name) => {
            if symbols.is_empty() {
                let _ = write!(out, "import <{}>;", name);
            } else {
                let _ = write!(out, "import {{ {} }} from <{}>;", symbols, name);
            }
        }
    }
}

/// 2026-08-05: format an `enum` item.
fn format_enum_into(e: &EnumDefinition, out: &mut String, level: usize) {
    indent(out, level);
    let _ = write!(out, "enum {}", e.name);
    if !e.type_params.is_empty() {
        let params: Vec<String> = e.type_params.iter().map(|p| p.name.clone()).collect();
        let _ = write!(out, "<{}>", params.join(", "));
    }
    let _ = write!(out, " {{ ");
    for (i, v) in e.variants.iter().enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        let _ = write!(out, "{}", format_enum_variant(v));
    }
    let _ = write!(out, " }};");
}

/// 2026-08-05: format a `type` item.
fn format_typedef_into(t: &TypeDef, out: &mut String, level: usize) {
    indent(out, level);
    let _ = write!(out, "type {}", t.name);
    if !t.type_params.is_empty() {
        let params: Vec<String> = t.type_params.iter().map(|p| p.name.clone()).collect();
        let _ = write!(out, "<{}>", params.join(", "));
    }
    if let Some(protocol) = &t.protocol {
        let _ = write!(out, ": {}", protocol);
    }
    let _ = write!(out, " {{ ");
    let atomic = atomic_field_map(&t.body.metadata);
    for (i, slot) in t.body.slots.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if let Some(ordering) = atomic.get(slot.name.as_str()) {
            if *ordering != "seq" {
                out.push_str(ordering);
                out.push(' ');
            }
            out.push_str("atomic ");
        }
        let _ = write!(out, "{}: {};", slot.name, slot.ty);
    }
    // 2026-08-13 (layout-keywords plan): TypeDef metadata round-trips via the
    // shared printer (spec form for physical-layout keys, `!>` otherwise).
    print_metadata_clauses(out, &t.body.metadata);
    let _ = write!(out, " }};");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    /// 2026-08-05 (Phase 2): parse a Briev source string into top-level items.
    fn parse(source: &str) -> Result<Vec<TopLevel>, String> {
        let tokens = tokenize(source).map_err(|e| format!("lex: {}", e))?;
        let mut parser = crate::parser::Parser::new(tokens, source);
        parser.parse_program().map_err(|e| format!("parse: {}", e))
    }

    /// 2026-08-05 (Phase 2): the round-trip contract.
    /// `format(parse(format(parse(source)))) == format(parse(source))`.
    fn assert_idempotent(source: &str) {
        let items = parse(source).unwrap_or_else(|e| panic!("first parse failed: {e}\n{source}"));
        let first = format_program(&items);
        let reparsed =
            parse(&first).unwrap_or_else(|e| panic!("reparse failed: {e}\n--- output:\n{first}"));
        let second = format_program(&reparsed);
        assert_eq!(
            first, second,
            "formatter is not idempotent for:\n{source}\n--- output:\n{first}\n--- reformat:\n{second}"
        );
    }

    const FIXTURES: &[&str] = &[
        "import <std/collections>;\n",
        "import \"std/string\";\n",
        "import { Map, Set } from \"std/collections\";\n",
        "const Max: Int = 10;\n",
        "defn add(a: Int, b: Int) -> Int [a >= 0][b >= 0] {\n  term a + b;\n};\n",
        "defn empty() [true][true] {};\n",
        "txn increment()[count < Max][count == count] {\n  count = count + 1;\n  term;\n};\n",
        "node update [ready][!ready] {\n  ready = false;\n  term;\n};\n",
        "async node tick [pending][!pending] {\n  pending = false;\n  term;\n};\n",
        "struct Point {\n  x: Float;\n  y: Float;\n};\n",
        "type Meters: Int {\n  value: Int;\n};\n",
        "trg input_ready @ device;\n",
        "$const Limit = 32;\n",
        "$let current = 0;\n",
        "defn guarded(x: Int) -> Int [true][x >= 0] {\n  when x > 0 {\n    term x;\n  };\n  term 0;\n};\n",
        "defn looped(items: List<Int>) -> Int [true][true] {\n  let acc: Int = 0;\n  foreach(item in items) {\n    acc = acc + item;\n  };\n  term acc;\n};\n",
        "defn lifetime(x: Int) -> Int [true][true] {\n  let buf: Ptr<Int> = Malloc#(4);\n  keep buf;\n  term x;\n};\n",
        "defn watchdog(a: Int) -> Int [a >= 0][a >= 0] ?[progress] within 10ms {\n  term a;\n};\n",
        // 2026-08-06 (accel plan): module-level `!>` metadata round-trips.
        "!> accel: try_all;\n",
        "!> accel: force;\n!> target: spirv;\n",
        "!> flags: [fast, contract];\n",
        // 2026-08-13 (layout-keywords plan): physical-layout metadata round-trips.
        "type W8: Int {\n  spec Bits: 8;\n};\n",
        "struct Flags {\n  spec Bytes: 1;\n  spec Alignment: 1;\n  a: Bool;\n};\n",
        "type Frame: Bit {\n  spec Alignment: 2;\n  spec Bits: 12;\n  spec MaxBits: 16;\n  spec Bytes: 4;\n  spec Endian: Big;\n};\n",
        // 2026-08-13 (layout-keywords plan): `pack struct` round-trips with the
        // prefix preserved, alongside its spec metadata.
        "pack struct Eth {\n  spec Endian: Big;\n  dst: Bit<48>;\n  src: Bit<48>;\n  etype: Bit<16>;\n};\n",
        "seq pack struct Mix {\n  spec Alignment: 1;\n  a: Bit<12>;\n  b: Bit<4>;\n};\n",
        // 2026-08-13 (layout-keywords plan Phase 5): the `atomic` field
        // modifier round-trips through the prefix (not `!> atomic_fields`).
        "struct Counter {\n  atomic count: Int;\n  other: Int;\n};\n",
        // 2026-08-13 (layout-keywords plan Phase 6): `union` round-trips.
        "union Word {\n  i: Int;\n  f: Float;\n};\n",
        // 2026-08-09 (init kind): runtime-seeded invariant round-trips.
        "init BufSize: Int = get_env_int!(\"BUFSIZE\");\n",
        "init BufferSize: [64 | lo..hi] Int = 64;\n",
        "init BitLayout: [16 | 32 | 64] Int = 16;\n",
        "init Layout: [16 | 32 | 64] Int {\n  term pick(target);\n};\n",
    ];

    #[test]
    fn formatter_is_idempotent_on_canonical_fixtures() {
        for fixture in FIXTURES {
            assert_idempotent(fixture);
        }
    }

    #[test]
    fn formatter_handles_blocky_statements() {
        assert_idempotent(
            "defn nested(x: Int) -> Int [true][true] {\n  when x > 0 {\n    term x;\n  };\n  term 0;\n};\n",
        );
    }

    /// 2026-08-13 (layout-keywords plan): a type body containing mixed `spec`
    /// and `!>` metadata prints both forms and round-trips.
    #[test]
    fn formatter_preserves_spec_and_exclaim_metadata() {
        let src = "type W: Int {\n  !> ctd: Add;\n  spec Bits: 8;\n  spec Endian: Little;\n};\n";
        let items = parse(src).expect("parse");
        let out = format_program(&items);
        assert!(out.contains("spec Bits: 8;"), "output: {out}");
        assert!(out.contains("spec Endian: Little;"), "output: {out}");
        assert!(out.contains("!> ctd: Add;"), "output: {out}");
        assert_idempotent(src);
    }
}

