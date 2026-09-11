// GLUE Export Pipeline
//
// `briev export <bridge.bv> <language>` — reads a Briev bridge file,
// extracts #export, frgn, and meld declarations, then writes a
// bridge-exports.dbvl metadata file alongside the compiled LLVM IR module.
//
// Pipeline:
//   1. Parse the bridge .bv to collect #export/frgn/meld declarations.
//   2. Read glue.dbv to find the language target entry (types_module,
//      llvm_triple, c_type_map).
//   3. Write bridge-exports.dbvl with tagged entries:
//      - export lines: function signatures crossing the boundary
//      - meld lines: type-layout compatibility proofs for the boundary
//      - ctype lines: Briev type → C ABI type mappings (from glue.dbv)
//   4. (Future) Compile bridge to .ll via `briev build --library`.
//
// 2026-07-10: GLUE v2. Replaced the $!macro adapter pipeline (which
// generated C ABI wrapper crates) with direct bridge-exports.dbvl output
// consumed by foreign build systems. The foreign build.rs or script reads
// the .dbvl to generate bindings. Two interop paths exist:
//   Path A (LLVM LTO) — for LLVM targets (Rust, C, Swift, Zig). Briev's
//     .ll merges with the host's .ll before optimization.
//   Path B (Meld) — for managed runtimes (Python, Node). C ABI transport
//     + meld projections for zero-copy data access.

use crate::ast::{Annotation, OutputType, TopLevel};
use crate::glue::config::GlueTarget;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;

/// Information extracted from a bridge .bv file for adapter consumption.
/// This is the structured representation of everything a language adapter
/// needs to know to generate wrapper code.
#[derive(Debug, Clone)]
pub struct BridgeInfo {
    /// Name of the bridge (derived from filename)
    pub name: String,
    /// Functions exported to the foreign language (via #export pragma)
    pub exports: Vec<ExportDecl>,
    /// Foreign functions called by the bridge (via frgn declarations)
    pub frgns: Vec<FrgnDecl>,
}

/// A function exported to the foreign language via `#export` pragma.
#[derive(Debug, Clone)]
pub struct ExportDecl {
    pub name: String,
    pub params: Vec<(String, String)>,  // (name, type string)
    pub return_type: String,
    /// 2026-08-03: Whether the emitted C-ABI signature carries a leading
    /// `ptr %state` parameter (body-dependent ABI, export_abi analysis).
    /// Wrappers/bindings must pass/omit the state handle to match.
    pub needs_state: bool,
    /// 2026-08-31 (boundary ownership plan): per-parameter ownership class
    /// (value / owned / zero-copy / borrowed / zero-cost) from the frontend
    /// inference. Wrappers render ownership-aware marshalling; the audit
    /// report reads it back.
    pub param_ownership: Vec<String>,
    /// 2026-08-31 (boundary ownership plan): return ownership class.
    pub return_ownership: Option<String>,
}

/// A foreign function declared via `frgn` in the bridge.
#[derive(Debug, Clone)]
pub struct FrgnDecl {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub return_type: String,
    /// If the frgn name matches an Intrinsic variant, list it here
    pub intrinsic_match: Option<String>,
}

/// Registry entry for a GLUE target language, parsed from glue.dbv.
///
/// 2026-07-10: GLUE v2. Removed macro_path (adapter $!macro system is
/// gone). Added types_module (path to .bv with foreign type declarations)
/// and llvm_triple (target triple for LLVM compilation, "any" for non-LLVM).
/// Renamed type_map to c_type_map — now maps Briev types to C ABI types
/// (int64_t, double, etc.) instead of language-specific type names.
#[derive(Debug, Clone)]
pub struct AdapterEntry {
    /// Target language name (rust, python, etc.)
    pub language: String,
    /// Path to .bv file declaring foreign type names for Briev's type universe
    pub types_module: String,
    /// Native source file extension without dot (rs, py, js)
    pub file_extension: String,
    /// LLVM target triple ("any" for non-LLVM targets like Python/Node)
    pub llvm_triple: String,
    /// Briev type name → C ABI type name mapping (e.g., Int → int64_t)
    pub c_type_map: HashMap<String, String>,
}

/// Extract bridge information from a parsed program.
/// Walks the AST to find #export pragmas, frgn declarations, and meld routes.
/// `universe` feeds the boundary-ownership inference (CStr vs String classes);
/// None disables ownership tagging (the contract degrades to needs_state only).
pub fn extract_bridge_info(
    items: &[TopLevel],
    name: &str,
    universe: Option<&crate::type_universe::TypeUniverse>,
) -> BridgeInfo {
    BridgeInfo {
        name: name.to_string(),
        exports: extract_exports(items, universe),
        frgns: extract_frgns(items),
    }
}

fn has_export_modifier(modifiers: &[Annotation]) -> bool {
    modifiers.iter().any(|m| m.name == "export")
}

fn extract_exports(
    items: &[TopLevel],
    universe: Option<&crate::type_universe::TypeUniverse>,
) -> Vec<ExportDecl> {
    // 2026-08-03: Body-dependent ABI — whether each export carries the
    // leading state param. Shared with the backend (src/analysis/export_abi.rs).
    // 2026-08-04 (compiler-in-Briev, P4): the Briev pass (briev_pass.rs)
    // computes this through the GLUE C ABI when its library is present;
    // otherwise the Rust reference runs.
    let needs_state = crate::glue::briev_pass::compute_export_needs_state(items);
    // 2026-08-31 (boundary ownership plan): per-export param/return ownership
    // classes. The convention is defaulted to c_abi (the GLUE target config
    // is not threaded here); lto/wasm refinements are a later wiring step.
    let ownership = crate::analysis::boundary_ownership::compute_boundary_ownership(
        items,
        universe,
        &|_| None,
    );
    let mut exports = Vec::new();
    for item in items {
        match item {
            // 2026-07-22: Form A — `export defn name(...) { ... }` keyword form
            TopLevel::Export(export) => {
                if let TopLevel::Definition(defn) = export.inner.as_ref() {
                    let params: Vec<(String, String)> = defn.parameters.iter()
                        .map(|p| (p.0.clone(), format_type(&p.1)))
                        .collect();
                    let return_type = defn.output_type.as_ref()
                        .map(|ot| format_output_type(ot))
                        .or_else(|| {
                            if !defn.outputs.is_empty() {
                                Some(defn.outputs.iter().map(|t| format_type(t)).collect::<Vec<_>>().join("|"))
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| "Void".to_string());
                    let entry = ownership.exports.get(&defn.name);
                    let param_ownership = entry
                        .map(|e| e.params.iter().map(|o| o.as_str().to_string()).collect())
                        .unwrap_or_default();
                    let return_ownership = entry
                        .and_then(|e| e.ret.map(|o| o.as_str().to_string()));
                    exports.push(ExportDecl {
                        name: defn.name.clone(),
                        params,
                        return_type,
                        needs_state: needs_state.get(&defn.name).copied().unwrap_or(false),
                        param_ownership,
                        return_ownership,
                    });
                }
            }
            // 2026-07-22: Form B — `#export` modifier on defn
            TopLevel::Definition(defn) if has_export_modifier(&defn.modifiers) => {
                let params: Vec<(String, String)> = defn.parameters.iter()
                    .map(|p| (p.0.clone(), format_type(&p.1)))
                    .collect();
                let return_type = defn.output_type.as_ref()
                    .map(|ot| format_output_type(ot))
                    .or_else(|| {
                        if !defn.outputs.is_empty() {
                            Some(defn.outputs.iter().map(|t| format_type(t)).collect::<Vec<_>>().join("|"))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "Void".to_string());
                let entry = ownership.exports.get(&defn.name);
                let param_ownership = entry
                    .map(|e| e.params.iter().map(|o| o.as_str().to_string()).collect())
                    .unwrap_or_default();
                let return_ownership = entry
                    .and_then(|e| e.ret.map(|o| o.as_str().to_string()));
                exports.push(ExportDecl {
                    name: defn.name.clone(),
                    params,
                    return_type,
                    needs_state: needs_state.get(&defn.name).copied().unwrap_or(false),
                    param_ownership,
                    return_ownership,
                });
            }
            _ => {}
        }
    }
    // 2026-07-22: Deduplicate by name — if both forms declare the same function, keep one.
    exports.sort_by_key(|e| e.name.clone());
    exports.dedup_by_key(|e| e.name.clone());
    exports
}

fn extract_frgns(items: &[TopLevel]) -> Vec<FrgnDecl> {
    let mut frgns = Vec::new();
    for item in items {
        if let TopLevel::Signature(sig) = item {
            let return_type = format_result_type(&sig.outputs);
            frgns.push(FrgnDecl {
                name: sig.name.clone(),
                params: sig.params.iter()
                    .map(|p| (p.0.clone(), format_type(&p.1)))
                    .collect(),
                return_type,
                intrinsic_match: None,
            });
        }
    }
    frgns
}

fn format_type(ty: &crate::ast::Type) -> String {
    match ty {
        crate::ast::Type::Custom(name) => name.clone(),
        // 2026-08-03: fn(P1,P2)->R — stable text form consumed by
        // resolve_protocol to build the per-language function-pointer ABI.
        crate::ast::Type::Function(params, ret) => {
            let ps: Vec<String> = params.iter().map(format_type).collect();
            format!("fn({})->{}", ps.join(","), format_type(ret))
        }
        _ => format!("{:?}", ty),
    }
}

fn format_output_type(ot: &OutputType) -> String {
    match ot {
        OutputType::Single(ty) => format_type(ty),
        OutputType::Tuple(types) => {
            let parts: Vec<String> = types.iter().map(|t| format_output_type(t)).collect();
            parts.join("|")
        }
        OutputType::Union(types) => {
            let parts: Vec<String> = types.iter().map(|t| format_output_type(t)).collect();
            parts.join("|")
        }
        OutputType::Array(ty) => format!("{}[]", format_output_type(ty)),
        OutputType::Named(name, inner) => format!("{}:{}", name, format_output_type(inner)),
    }
}

fn format_result_type(outputs: &[crate::ast::Type]) -> String {
    if outputs.is_empty() {
        "Void".to_string()
    } else {
        let parts: Vec<String> = outputs.iter().map(|t| format_type(t)).collect();
        parts.join("|")
    }
}

// =========================================================================
// DBVL Serialization — bridge-exports.dbvl output format
//
// Architecture: Briev export produces a .dbvl metadata file alongside the
// compiled .ll module. Each line is tagged with a discriminator field so
// the consumer (build.rs, Python script) can dispatch on entry type:
//
//   export:  export, name, param_types|pipe|separated, return_type
//   meld:    meld, from_type, to_type, route
//   ctype:   ctype, briev_type, c_type
//
// No quoting needed — none of our field values contain commas.
// The consumer splits by "\n" then by "," and switches on field[0].
//
// 2026-07-10: Tagged format replaces the old bare-entries format that was
// passed to $!macro adapters. Tagged lines allow a single file to carry
// multiple entry types without schema switching.
// =========================================================================

fn serialize_exports_tagged(exports: &[ExportDecl]) -> String {
    let mut lines = Vec::new();
    for e in exports {
        let params: Vec<String> = e.params.iter().map(|(_, t)| t.clone()).collect();
        // 2026-08-03: 5th field = needs_state (leading state param in the
        // emitted C ABI). Consumers use it to pass/omit the state handle.
        let ns = if e.needs_state { "state" } else { "pure" };
        // 2026-08-31 (boundary ownership plan): 6th field = per-param
        // ownership classes (value/owned/zero-copy/borrowed/zero-cost);
        // 7th field = return ownership. A consumer that sees zero-copy can
        // pass the pointer without copying.
        let pown = e.param_ownership.join("|");
        let rown = e.return_ownership.clone().unwrap_or_else(|| "".to_string());
        lines.push(format!("export,{},{},{},{},{},{}", e.name, params.join("|"), e.return_type, ns, pown, rown));
    }
    lines.join("\n")
}

fn serialize_ctypes_dbvl(c_type_map: &HashMap<String, String>) -> String {
    let mut lines: Vec<String> = c_type_map.iter()
        .map(|(briev_type, c_type)| format!("ctype,{},{}", briev_type, c_type))
        .collect();
    lines.sort();
    lines.join("\n")
}

/// Per-export template variables for wrapper/bindings rendering.
///
/// 2026-08-03: The state handle (s_param/s_ffi_param/s_ffi_type) is computed
/// from the body-dependent ABI (needs_state) and the target's config-driven
/// state representation. Boundary conversions come from target.conversions
/// (config), never hardcoded in Rust.
fn export_template_vars(export: &ExportDecl, target: &GlueTarget, type_protocols: &HashMap<String, String>) -> HashMap<String, String> {
    let mut fn_vars: HashMap<String, String> = HashMap::new();
    fn_vars.insert("name".to_string(), export.name.clone());
    // 2026-08-04 (ship common languages): camelCase the export name — Go/Java
    // wrappers must be exported and camelCase (`FeatureHash` from
    // `feature_hash`); the source names are snake_case.
    fn_vars.insert("name_upper".to_string(), to_camel_case(&export.name));

    let params: Vec<String> = export.params.iter()
        .map(|(name, ty)| {
            let (native, _) = resolve_protocol(ty, &target.protocols, type_protocols);
            // 2026-08-04 (ship common languages): `{{params}}` uses the
            // target's param_decl (C-family `{type} {name}`, Go `{name} {type}`,
            // python/rust `{name}: {type}`) with NATIVE types — previously the
            // `name: native` colon form was hardcoded, which is wrong for Go.
            target.param_decl
                .replace("{name}", name)
                .replace("{type}", &native)
        })
        .collect();
    let ffi_params: Vec<String> = export.params.iter()
        .map(|(name, ty)| {
            // 2026-08-03: function-pointer (callback) params use the target's
            // fn_param_decl (C embeds the name: `int64_t (*cb)(int64_t)`).
            if ty.starts_with("fn(") {
                let (ret, params) = fn_pointer_parts(ty, &target.protocols, type_protocols);
                return target.fn_param_decl
                    .replace("{ret}", &ret)
                    .replace("{name}", name)
                    .replace("{params}", &params);
            }
            let (_, c_abi) = resolve_protocol(ty, &target.protocols, type_protocols);
            // Config-driven param decl format (C uses `type name`,
            // python/rust use `name: type`).
            target.param_decl.replace("{name}", name).replace("{type}", &c_abi)
        })
        .collect();
    let args: Vec<String> = export.params.iter()
        .map(|(name, _)| name.clone())
        .collect();
    let c_types: Vec<String> = export.params.iter()
        .map(|(_, ty)| {
            let (_, c_abi) = resolve_protocol(ty, &target.protocols, type_protocols);
            c_abi
        })
        .collect();

    let args_abi = to_abi_args(export, target, type_protocols);
    let return_expr = from_abi_return(export, target, type_protocols);
    let (state_arg, state_ffi_param, state_ffi_type) =
        state_vars(export, target, !args_abi.is_empty(), !ffi_params.is_empty(), !c_types.is_empty());

    fn_vars.insert("s_param".to_string(), state_arg);
    fn_vars.insert("s_ffi_param".to_string(), state_ffi_param);
    fn_vars.insert("s_ffi_type".to_string(), state_ffi_type);
    let (native_ret, c_ret) = resolve_protocol(&export.return_type, &target.protocols, type_protocols);
    fn_vars.insert("return".to_string(), native_ret.clone());
    fn_vars.insert("c_return".to_string(), c_ret.clone());

    fn_vars.insert("params".to_string(), params.join(", "));
    fn_vars.insert("ffi_params".to_string(), ffi_params.join(", "));
    fn_vars.insert("c_types".to_string(), c_types.join(", "));
    fn_vars.insert("args".to_string(), args.join(", "));
    fn_vars.insert("args_abi".to_string(), args_abi.join(", "));
    fn_vars.insert("return_expr".to_string(), return_expr);
    fn_vars
}

/// `to_abi`: render each argument's boundary form from the config expression
/// (`{name}` placeholder); identity when the target has no conversion. The
/// lookup key is the type's protocol CATEGORY (`#String` for a CStr boundary
/// type), not the raw type name.
fn to_abi_args(
    export: &ExportDecl,
    target: &GlueTarget,
    type_protocols: &HashMap<String, String>,
) -> Vec<String> {
    export.params.iter()
        .map(|(name, ty)| {
            let key = protocol_category_of(ty, type_protocols);
            target.conversions.to_abi.get(&key)
                .map(|expr| expr.replace("{name}", name))
                .unwrap_or_else(|| name.clone())
        })
        .collect()
}

/// `from_abi`: render the raw boundary `result_abi` back to a native value.
/// Keyed by the return type's protocol category (boundary types resolve).
fn from_abi_return(
    export: &ExportDecl,
    target: &GlueTarget,
    type_protocols: &HashMap<String, String>,
) -> String {
    let key = protocol_category_of(&export.return_type, type_protocols);
    target.conversions.from_abi.get(&key)
        .cloned()
        .unwrap_or_else(|| "result_abi".to_string())
}

/// The protocol category of a type string — `CStr` → "String" (via its
/// declared `#String<C_String>` protocol), `Int` → "Int".
fn protocol_category_of(ty: &str, type_protocols: &HashMap<String, String>) -> String {
    type_protocols.get(ty)
        .map(|p| protocol_category(p).to_string())
        .unwrap_or_else(|| ty.to_string())
}

/// Per-export state-handle variables, joined WITHOUT a dangling separator
/// when there are no user params (C rejects `(BrievState* state, )`).
fn state_vars(
    export: &ExportDecl,
    target: &GlueTarget,
    has_args: bool,
    has_ffi_params: bool,
    has_c_types: bool,
) -> (String, String, String) {
    let needs = export.needs_state;
    (
        state_fragment(needs, has_args, &target.state.arg),
        state_fragment(needs, has_ffi_params, &target.state.decl),
        state_fragment(needs, has_c_types, &target.state.ffi_type),
    )
}

/// `needs` && more-fields → `"{s}, "`; `needs` only → `s`; else `""`.
fn state_fragment(needs: bool, has_more: bool, s: &str) -> String {
    if needs && has_more {
        format!("{}, ", s)
    } else if needs {
        s.to_string()
    } else {
        String::new()
    }
}

/// Render the per-export ffi declarations for a template set, joined by a
/// separator (newline). Shared by wrapper and bindings generation.
fn render_ffi_decls(
    exports: &[ExportDecl],
    target: &GlueTarget,
    template_vars: &HashMap<String, String>,
    ffi_template: &str,
    type_protocols: &HashMap<String, String>,
) -> String {
    let mut ffi_buf = String::new();
    for (i, export) in exports.iter().enumerate() {
        if i > 0 { ffi_buf.push('\n'); }
        let fn_vars = export_template_vars(export, target, type_protocols);
        let mut merged = template_vars.clone();
        merged.extend(fn_vars);
        ffi_buf.push_str(&render_template(ffi_template, &merged));
    }
    ffi_buf
}

/// `briev bindings <bridge.bv> <language> [--out <dir>]` — render only the
/// language's `bindings.*` templates (e.g. briev_types.h, briev_bindings.rs)
/// for the exported functions, without the full wrapper crate.
///
/// 2026-08-03: Config-driven — the bindings templates and per-export ABI
/// (state param from needs_state) live in lib/glue/<lang>/glue.dbv. No compiler-side
/// language knowledge.
pub fn run_bindings_cli(file_path: &str, language: &str, out_dir: &str) -> Result<(), String> {
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Cannot read '{}': {}", file_path, e))?;
    let bridge_name = std::path::Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bridge");
    let (items, universe) = crate::library::parse_and_check(file_path, &source)?;

    let target = crate::glue::config::load_glue_language(language)?;
    let info = extract_bridge_info(&items, bridge_name, Some(&universe));
    if std::env::var("BRIEV_DEBUG_BINDINGS").is_ok() {
        eprintln!("DBG target.templates keys: {:?}", target.templates.keys().collect::<Vec<_>>());
        eprintln!("DBG exports: {:?}", info.exports.iter().map(|e| e.name.clone()).collect::<Vec<_>>());
    }
    // 2026-08-03 (P3): type → declared protocol (`type CStr: #String<C_String>`)
    // so boundary types resolve to their category's ABI names in the header/
    // wrapper. The export runs before codegen's normalizer, so the universe
    // isn't populated — resolve from the type declarations.
    let type_protocols = build_type_protocols(&items);

    let mut template_vars: HashMap<String, String> = HashMap::new();
    template_vars.insert("bridge_name".to_string(), bridge_name.to_string());

    let bindings_ffi = target.templates.get("bindings.ffi_template")
        .or_else(|| target.templates.get("ffi_template"))
        .ok_or_else(|| format!("target '{}' has no bindings.ffi_template in lib/glue/<lang>/glue.dbv", language))?;
    let ffi_decls = render_ffi_decls(&info.exports, &target, &template_vars, bindings_ffi, &type_protocols);
    template_vars.insert("ffi_decls".to_string(), ffi_decls);

    let output_dir = std::path::Path::new(out_dir).join(format!("{}-bindings", bridge_name));
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("Failed to create output dir '{}': {}", output_dir.display(), e))?;

    let mut written = 0;
    for (filename, template) in &target.templates {
        if filename == "bindings.ffi_template" || filename == "ffi_template" || filename == "fn_template" {
            continue;
        }
        if !filename.starts_with("bindings.") {
            continue;
        }
        let rel = &filename["bindings.".len()..];
        let rendered = render_template(template, &template_vars);
        let output_path = output_dir.join(rel);
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir '{}': {}", parent.display(), e))?;
        }
        std::fs::write(&output_path, &rendered)
            .map_err(|e| format!("Failed to write '{}': {}", output_path.display(), e))?;
        println!("  Written: {}", output_path.display());
        written += 1;
    }
    if written == 0 {
        return Err(format!("target '{}' has no bindings.* templates in lib/glue/<lang>/glue.dbv", language));
    }
    Ok(())
}

/// `briev extension <bridge.bv> <language> [--out <dir>]` — build a native
/// host-language extension (no FFI marshalling layer). 2026-08-03 (plan
/// 2026-08-03-native-python-meld-composite): for Python this generates a
/// CPython C-extension module (`PyInit_<bridge>` + per-export methods) that
/// calls the Briev exports directly — the ~1.9µs ctypes overhead disappears.
/// Config-driven: the language's `native.*` templates + per-category
/// parse/build snippets live in lib/glue/<lang>/glue.dbv; the compiler only renders.
pub fn run_extension_cli(file_path: &str, language: &str, out_dir: &str) -> Result<(), String> {
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Cannot read '{}': {}", file_path, e))?;
    let bridge_name = std::path::Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bridge");
    let (items, universe) = crate::library::parse_and_check(file_path, &source)?;

    let target = crate::glue::config::load_glue_language(language)?;
    let info = extract_bridge_info(&items, bridge_name, Some(&universe));
    let type_protocols = build_type_protocols(&items);

    // Build the bridge static library (.a) with the current compiler, so the
    // extension is self-contained (links the real-ELF objects directly).
    let lib_dir = std::path::Path::new(out_dir);
    std::fs::create_dir_all(lib_dir).map_err(|e| format!("create {}: {}", lib_dir.display(), e))?;
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate brievc: {}", e))?;
    let build = std::process::Command::new(&exe)
        .args(["build", file_path, "--library", "--out", out_dir])
        .output()
        .map_err(|e| format!("failed to run brievc build --library: {}", e))?;
    if !build.status.success() {
        return Err(format!("brievc build --library failed:\n{}", String::from_utf8_lossy(&build.stderr)));
    }

    // Render the native shim (.c).
    let shim = render_native_shim(&info, &target, &type_protocols, bridge_name);
    let shim_path = lib_dir.join(format!("{}_module.c", bridge_name));
    std::fs::write(&shim_path, &shim).map_err(|e| format!("write shim: {}", e))?;

    // Compile + link via the language's toolchain recipe (config-driven —
    // the compiler only knows "compile C, link a shared library").
    let cc = target.native_cc.clone().unwrap_or_else(|| "cc".to_string());
    let include_flags = run_recipe_cmd(&target.native_include_cmd).unwrap_or_default();
    let shim_o = lib_dir.join(format!("{}_module.o", bridge_name));
    let cc_c = std::process::Command::new(&cc)
        .arg("-fPIC").arg("-c").arg(&shim_path).arg("-o").arg(&shim_o)
        .args(include_flags.split_whitespace())
        .output().map_err(|e| format!("failed to run {}: {}", cc, e))?;
    if !cc_c.status.success() {
        return Err(format!("{} failed:\n{}", cc, String::from_utf8_lossy(&cc_c.stderr)));
    }

    let suffix = target.native_suffix.clone()
        .or_else(|| run_recipe_cmd(&target.native_suffix_cmd))
        .unwrap_or_else(|| ".so".to_string());
    let prefix = target.native_prefix.clone().unwrap_or_default();
    let ext_path = lib_dir.join(format!("{}{}{}", prefix, bridge_name, suffix.trim()));
    let ldflags = run_recipe_cmd(&target.native_link_cmd).unwrap_or_default();
    let archive = lib_dir.join(format!("lib{}.a", bridge_name));
    // 2026-08-03 (node bridge): bind the addon's own exported symbols locally
    // (-Bsymbolic-functions) so the host cannot interpose them. A bridge export
    // named like a libc function (`read`, `open`, `exec`) otherwise resolves
    // through the PLT to the host's symbol and returns garbage (read → -1).
    let cc_link = std::process::Command::new(&cc)
        .arg("-shared").arg("-o").arg(&ext_path).arg(&shim_o).arg(&archive)
        .arg("-Wl,-Bsymbolic-functions")
        .args(ldflags.split_whitespace())
        .output().map_err(|e| format!("failed to link extension: {}", e))?;
    if !cc_link.status.success() {
        return Err(format!("link failed:\n{}", String::from_utf8_lossy(&cc_link.stderr)));
    }
    let _ = std::fs::remove_file(&shim_o);
    if std::env::var("BRIEV_KEEP_SHIM").is_ok() {
        println!("  Shim:    {}", shim_path.display());
    } else {
        let _ = std::fs::remove_file(&shim_path);
    }

    println!("  Extension: {}", ext_path.display());
    println!("  Usage:     import {}", bridge_name);
    Ok(())
}

/// Run a toolchain-recipe command from the language config; its stdout is the
/// value (include flags, link flags, suffix). None if no command is configured
/// or it fails (e.g. node needs no link flags).
fn run_recipe_cmd(cmd: &Option<String>) -> Option<String> {
    let cmd = cmd.as_ref()?;
    let out = std::process::Command::new("sh").arg("-c").arg(cmd).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// Render the native shim (.c) from the language's `native.*` templates.
fn render_native_shim(
    info: &BridgeInfo,
    target: &GlueTarget,
    type_protocols: &HashMap<String, String>,
    bridge_name: &str,
) -> String {
    let mut methods = String::new();
    let mut method_defs = String::new();
    let mut protos = String::new();
    let method_tpl = target.templates.get("native.method");
    let method_def_tpl = target.templates.get("native.method_def");
    for (i, export) in info.exports.iter().enumerate() {
        if i > 0 {
            methods.push('\n');
            method_defs.push('\n');
            protos.push('\n');
        }
        // Per-param native parse snippets + call args.
        let mut parse_code = String::new();
        let mut sig_params: Vec<String> = Vec::new();
        let mut call_args: Vec<String> = Vec::new();
        if export.needs_state {
            call_args.push("g_state".to_string());
        }
        for (name, ty) in &export.params {
            let key = native_key(ty, type_protocols);
            let parse_tpl = tpl_for(target, &format!("native.parse.{}", key), "");
            let mut pvars: HashMap<String, String> = HashMap::new();
            pvars.insert("name".to_string(), name.clone());
            parse_code.push_str(&render_template(&parse_tpl, &pvars));
            // 2026-08-04 (ship common languages): JNI-style native shims take
            // the host values DIRECTLY as signature params (`jlong name`,
            // `jstring jname`) rather than parsing a tuple. Each language
            // declares its signature form via `native.sig.<cat>`.
            let sig_tpl = tpl_for(target, &format!("native.sig.{}", key), "jlong {{name}}");
            let mut svars: HashMap<String, String> = HashMap::new();
            svars.insert("name".to_string(), name.clone());
            sig_params.push(render_template(&sig_tpl, &svars));
            call_args.push(name.clone());
        }
        let ret_key = native_key(&export.return_type, type_protocols);
        let ret_c = tpl_for(target, &format!("native.ret.{}", ret_key), "long long");
        let build = tpl_for(target, &format!("native.build.{}", ret_key), "return PyLong_FromLongLong(r);");
        let ret_jni = tpl_for(target, &format!("native.ret_jni.{}", ret_key), "jlong");
        // 2026-08-03 (node bridge): the shim calls each export under a unique
        // C identifier (`__briev_export_<name>`) aliased to the real symbol via
        // an `asm("...")` label. An export named like a libc/Python/Node header
        // function (`read`, `open`, `malloc`, …) would otherwise conflict with
        // the host's prototype at COMPILE time (conflicting types) and be
        // interposed at LINK time (read → libc read(2) → -1).
        let call_name = format!("__briev_export_{}", export.name);
        let mut vars: HashMap<String, String> = HashMap::new();
        vars.insert("name".to_string(), export.name.clone());
        vars.insert("name_upper".to_string(), to_camel_case(&export.name));
        vars.insert("bridge_name".to_string(), bridge_name.to_string());
        vars.insert("parse_code".to_string(), parse_code);
        vars.insert("ret_c".to_string(), ret_c.clone());
        vars.insert("call".to_string(), format!("{}({})", call_name, call_args.join(", ")));
        vars.insert("build_code".to_string(), build);
        vars.insert("sig_params".to_string(), sig_params.join(", "));
        // 2026-08-04 (JNI): a comma before the first host param in the
        // signature (`jclass cls, jlong count`) — absent for 0-param exports.
        vars.insert("sig_comma".to_string(), if export.params.is_empty() { String::new() } else { ", ".to_string() });
        vars.insert("ret_jni".to_string(), ret_jni);
        // Node native shim: argc is the real param count; array sizes must be
        // >= 1 (a 0-param export like `read()` must not emit `argv[0]`).
        let nargs = export.params.len();
        vars.insert("nargs".to_string(), nargs.to_string());
        vars.insert("nargs_arr".to_string(), nargs.max(1).to_string());
        if let Some(t) = method_tpl {
            methods.push_str(&render_template(t, &vars));
        }
        if let Some(t) = method_def_tpl {
            method_defs.push_str(&render_template(t, &vars));
        }
        // Extern prototype for the export (the shim calls it directly).
        let mut proto_params: Vec<String> = Vec::new();
        if export.needs_state {
            proto_params.push("BrievState* state".to_string());
        }
        for (name, ty) in &export.params {
            let key = native_key(ty, type_protocols);
            proto_params.push(format!(
                "{} {}",
                tpl_for(target, &format!("native.c_type.{}", key), "long long"),
                name
            ));
        }
        protos.push_str(&format!(
            "extern {} {}({}) asm(\"{}\");",
            ret_c, call_name, proto_params.join(", "), export.name
        ));
    }

    let module_tpl = target.templates.get("native.module");
    let mut vars: HashMap<String, String> = HashMap::new();
    vars.insert("bridge_name".to_string(), bridge_name.to_string());
    vars.insert("export_protos".to_string(), protos);
    vars.insert("methods".to_string(), methods);
    vars.insert("method_defs".to_string(), method_defs);
    vars.insert("nmethods".to_string(), info.exports.len().to_string());
    match module_tpl {
        Some(t) => render_template(t, &vars),
        None => String::new(),
    }
}

/// The native-template key for a type string: its protocol category (via the
/// declared protocol map), e.g. "CDouble" → "#Float", "Int" → "#Int".
fn native_key(ty: &str, type_protocols: &HashMap<String, String>) -> String {
    let cat = match type_protocols.get(ty) {
        Some(proto) => protocol_category(proto).to_string(),
        None => ty.to_string(),
    };
    cat
}

fn tpl_for(target: &GlueTarget, key: &str, default: &str) -> String {
    target.templates.get(key).cloned().unwrap_or_else(|| default.to_string())
}

/// camelCase a snake_case export name for host conventions: `feature_hash` →
/// `FeatureHash` (Go exported funcs, Java/JNI methods).
fn to_camel_case(name: &str) -> String {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut c = seg.to_string();
            if let Some(f) = c.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            c
        })
        .collect()
}

/// CLI entry point for `briev export <bridge.bv> <language> [--out <dir>]`.
///
/// 2026-07-22: Reads a Briev bridge file, extracts exports and foreign
/// declarations, generates LLVM IR with C-compatible wrappers, and writes
/// bridge metadata alongside the compiled output.
///
/// Steps:
///   1. Read and parse the .bv file
///   2. Extract bridge info (exports, frgns, melds)
///   3. Find the language target in the GLUE registry
///   4. Generate LLVM IR with C wrappers for exported functions
///   5. Compile to .o via llc
///   6. Generate native wrapper source files (Python, Rust, etc.)
///   7. Write bridge-exports.dbvl metadata
pub fn run_export_cli(file_path: &str, language: &str, out_dir: &str) -> Result<(), String> {
    // 2026-07-22: Step 1 — read and parse the bridge .bv file
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Cannot read '{}': {}", file_path, e))?;
    let bridge_name = std::path::Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bridge");
    let (items, universe) = crate::library::parse_and_check(file_path, &source)?;

    // 2026-07-22: Step 2 — find language target and extract bridge info
    let target = crate::glue::config::load_glue_language(language)?;
    // Full registry for frgn dispatch routing (extension → language).
    let glue_targets = crate::glue::config::load_glue_config(None)?;
    let info = extract_bridge_info(&items, bridge_name, Some(&universe));
    let type_protocols = build_type_protocols(&items);
    println!("  Bridge '{}': {} exports, {} frgns",
        info.name, info.exports.len(), info.frgns.len());
    println!("  Target: {} (types: {}, bridge: {})",
        target.language, target.types_module.display(), target.bridge_kind);

    // 2026-07-22: Step 3 — collect resolved frgns for dispatch
    let mut resolved_frgns = std::collections::HashMap::new();
    for item in &items {
        if let crate::ast::TopLevel::ForeignBinding(fb) = item {
            let ext = fb.from.extension().unwrap_or_default();
            if let Ok(dispatch) = crate::analysis::frgn_dispatch::resolve_single_frgn(
                fb, &ext, &glue_targets, crate::target::BackendKind::Llvm, Some(&universe),
            ) {
                resolved_frgns.insert(fb.effective_briev_name().to_string(), dispatch);
            }
        }
    }

    // 2026-07-22: Step 3 — generate LLVM IR with the full backend (real function bodies)
    let llvm_ir = {
        use crate::backend::llvm::LlvmBackend;
        let mut b = LlvmBackend::new()
            .with_type_universe(universe)
            .with_resolved_frgns(resolved_frgns);
        b.generate(&items, None)
    };

    // 2026-07-22: Step 5 — create output directory and write files
    let output_dir = std::path::Path::new(out_dir).join(format!("{}-bridge", bridge_name));
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("Failed to create output dir '{}': {}", output_dir.display(), e))?;

    // Write LLVM IR
    let ll_path = output_dir.join("bridge.ll");
    std::fs::write(&ll_path, &llvm_ir)
        .map_err(|e| format!("Failed to write '{}': {}", ll_path.display(), e))?;
    println!("  LLVM IR: {}", ll_path.display());

    // 2026-07-22: Step 6 — compile to .o via llc.
    // 2026-08-03: -relocation-model=pic — the .o must link into a shared
    // library for c_abi hosts (python/node); without it the linker rejects
    // R_X86_64_32 relocations against .rodata.
    let o_path = output_dir.join("bridge.o");
    let llc_status = std::process::Command::new("llc")
        .arg("-filetype=obj")
        .arg("-relocation-model=pic")
        .arg("-o")
        .arg(&o_path)
        .arg(&ll_path)
        .status()
        .map_err(|e| format!("Failed to run llc: {}", e))?;
    if !llc_status.success() {
        return Err("llc failed".to_string());
    }
    println!("  Object: {}", o_path.display());

    // 2026-07-22: Step 7 — generate language wrappers from config templates.
    let mut template_vars: HashMap<String, String> = HashMap::new();
    template_vars.insert("bridge_name".to_string(), bridge_name.to_string());
    // 2026-08-03: The global s_init differs by convention; the per-export
    // s_param/s_ffi_param/s_argtypes are computed per export from its
    // needs_state (body-dependent ABI) — pure exports get no state handle.
    if target.calling_convention == "c_abi" {
        template_vars.insert("s_init".to_string(), String::new());
    } else {
        // LTO path (Rust): init_state is declared as an FFI function
        template_vars.insert("s_init".to_string(), "    pub fn init_state(state: *mut c_void);\n".to_string());
    }

    // Render all exports + FFI declarations
    let fn_template = target.templates.get("fn_template");
    let ffi_template = target.templates.get("ffi_template");
    let mut exports_buf = String::new();
    let mut ffi_buf = String::new();

    for (i, export) in info.exports.iter().enumerate() {
        if i > 0 { exports_buf.push('\n'); }
        if i > 0 { ffi_buf.push('\n'); }

        let fn_vars = export_template_vars(export, &target, &type_protocols);

        if let Some(ft) = fn_template {
            // 2026-08-03: per-function render sees both fn_vars and the
            // global template_vars (s_init, bridge_name, …). The earlier
            // render passed only fn_vars, silently dropping {{s_param}}.
            let mut merged = template_vars.clone();
            merged.extend(fn_vars.clone());
            let rendered = render_template(ft, &merged);
            exports_buf.push_str(&rendered);
        }
        if let Some(ffit) = ffi_template {
            let mut merged = template_vars.clone();
            merged.extend(fn_vars.clone());
            let rendered = render_template(ffit, &merged);
            ffi_buf.push_str(&rendered);
        }
    }

    template_vars.insert("exports".to_string(), exports_buf.clone());
    template_vars.insert("ffi_decls".to_string(), ffi_buf.clone());
    if std::env::var("BRIEV_DEBUG_BINDINGS").is_ok() {
        eprintln!("DBG exports_buf=[{}]", exports_buf);
    }

    // Write each file template
    for (filename, template) in &target.templates {
        if filename == "fn_template" || filename == "ffi_template" || filename.starts_with("native.") {
            continue;
        }
        let rendered = render_template(template, &template_vars);
        let output_path = output_dir.join(filename);
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir '{}': {}", parent.display(), e))?;
        }
        std::fs::write(&output_path, &rendered)
            .map_err(|e| format!("Failed to write '{}': {}", output_path.display(), e))?;
        println!("  Written: {}", output_path.display());
    }

    // 2026-07-22: Step 8 — write bridge-exports.dbvl metadata
    let adapter = crate::glue::export::AdapterEntry {
        language: language.to_string(),
        types_module: target.types_module.to_string_lossy().to_string(),
        file_extension: target.extension.clone(),
        llvm_triple: "x86_64-unknown-linux-gnu".to_string(),
        // 2026-07-26: c_abi is optional — fall back to native type if absent
        c_type_map: target.protocols.iter()
            .map(|(k, v)| (k.clone(), v.c_abi.clone().unwrap_or_else(|| v.native.clone())))
            .collect(),
    };
    let mut dbvl_content = String::new();
    let exports_str = serialize_exports_tagged(&info.exports);
    if !exports_str.is_empty() {
        dbvl_content.push_str(&exports_str);
        dbvl_content.push('\n');
    }
    let ctypes_str = serialize_ctypes_dbvl(&adapter.c_type_map);
    if !ctypes_str.is_empty() {
        dbvl_content.push_str(&ctypes_str);
        dbvl_content.push('\n');
    }
    let dbvl_path = output_dir.join("bridge-exports.dbvl");
    std::fs::write(&dbvl_path, dbvl_content.trim_end())
        .map_err(|e| format!("Failed to write bridge-exports.dbvl: {}", e))?;
    println!("  Metadata: {}", dbvl_path.display());
    Ok(())
}

/// Look up a Briev type's protocol mapping in a target's protocols map.
/// Returns (native_type, c_abi_or_wasm_abi_type) or falls back to the input type name.
/// 2026-07-26: c_abi is optional — for wasm_import targets, use wasm_abi instead.
fn resolve_protocol(
    briev_type_name: &str,
    protocols: &HashMap<String, crate::glue::config::ProtocolEntry>,
    type_protocols: &HashMap<String, String>,
) -> (String, String) {
    // 2026-08-03: fn(P)->R — a function pointer IS the ABI. Both the native
    // and C forms are the C function-pointer type built from the inner
    // protocols (e.g. `fn(Int)->Int` → `i64 (*)(i64)`).
    if briev_type_name.starts_with("fn(") {
        let abi = fn_pointer_abi(briev_type_name, protocols, type_protocols);
        return (abi.clone(), abi);
    }
    // 2026-08-03 (P3): a boundary type (`CStr`, `CDouble`) resolves to its
    // protocol CATEGORY so the config's category-keyed c_abi applies. The
    // protocol string may carry a variant (`#String<C_String>`) — the ABI
    // name comes from the category entry.
    let category = type_protocols.get(briev_type_name)
        .map(|p| protocol_category(p).to_string())
        .unwrap_or_else(|| briev_type_name.to_string());
    let protocol_key = category;
    if let Some(entry) = protocols.get(&protocol_key) {
        let abi = entry.c_abi.clone()
            .or_else(|| entry.wasm_abi.clone())
            .unwrap_or_else(|| entry.native.clone());
        (entry.native.clone(), abi)
    } else {
        (briev_type_name.to_string(), briev_type_name.to_string())
    }
}

/// Type name → declared protocol (`type CStr: #String<C_String>` → CStr →
/// "#String<C_String>"). 2026-08-03 (P3): lets the wrapper/header resolve a
/// boundary type to its category's ABI names.
/// The type → protocol map used to resolve boundary ABI names — now the
/// SHARED AST-level derivation (2026-09-02, plan
/// fundamental-parent-membership): explicit declarations win, bare-parent
/// typedefs derive their category from the parent chain, so a
/// de-hashtagged `Float16 : Float` resolves at the FFI boundary too.
fn build_type_protocols(items: &[TopLevel]) -> HashMap<String, String> {
    crate::casting::graph::derive_type_protocols(items)
}

/// The bare category of a declared protocol string — `#String<C_String>` →
/// `"String"`, `#String` → `"String"`.
fn protocol_category(proto: &str) -> &str {
    let b = proto.trim_start_matches('#');
    match b.find('<') {
        Some(lt) => &b[..lt],
        None => b,
    }
}

/// Decompose a `fn(P1,P2)->R` text form into `(ret_c, params_c)` using each
/// inner type's c_abi mapping. `fn(Int)->Int` → `("i64", "i64")`.
fn fn_pointer_parts(
    fn_str: &str,
    protocols: &HashMap<String, crate::glue::config::ProtocolEntry>,
    type_protocols: &HashMap<String, String>,
) -> (String, String) {
    let inner = fn_str.strip_prefix("fn(").unwrap_or(fn_str);
    let (params_part, ret) = inner.split_once("->").unwrap_or((inner, ""));
    let params_part = params_part.trim_end_matches(')');
    let params: Vec<String> = params_part.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| resolve_protocol(p.trim(), protocols, type_protocols).1)
        .collect();
    let ret_c = if ret.trim().is_empty() {
        "void".to_string()
    } else {
        resolve_protocol(ret.trim(), protocols, type_protocols).1
    };
    (ret_c, params.join(", "))
}

/// Build the C function-pointer type for a `fn(P1,P2)->R` text form using
/// each inner type's c_abi mapping. `fn(Int)->Int` → `i64 (*)(i64)`.
fn fn_pointer_abi(
    fn_str: &str,
    protocols: &HashMap<String, crate::glue::config::ProtocolEntry>,
    type_protocols: &HashMap<String, String>,
) -> String {
    let (ret, params) = fn_pointer_parts(fn_str, protocols, type_protocols);
    format!("{} (*)({})", ret, params)
}


/// Simple mustache-like template substitution.
///
/// Replaces `{{variable}}` with values from `vars`.
/// No blocks, no loops — just single variable substitution.
/// Per-function repetition is handled by the caller (see `run_export_cli`).
fn render_template(template: &str, vars: &HashMap<String, String>) -> String {
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        // Push everything before the `{{`
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let var_name = &after_open[..end];
            let value = vars.get(var_name).map(|s| s.as_str()).unwrap_or("");
            result.push_str(value);
            rest = &after_open[end + 2..];
        } else {
            // Unclosed `{{` — push rest as-is
            result.push_str(&rest[start..]);
            break;
        }
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_exports_tagged_empty() {
        let result = serialize_exports_tagged(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_serialize_exports_tagged_single() {
        let exports = vec![ExportDecl {
            name: "print_int".to_string(),
            params: vec![("n".to_string(), "Int".to_string())],
            return_type: "Int".to_string(),
            needs_state: false,
            param_ownership: vec!["value".to_string()],
            return_ownership: Some("value".to_string()),
        }];
        let result = serialize_exports_tagged(&exports);
        assert_eq!(result, "export,print_int,Int,Int,pure,value,value");
    }

    #[test]
    fn test_serialize_exports_tagged_multiple_params() {
        let exports = vec![ExportDecl {
            name: "write_file".to_string(),
            params: vec![
                ("path".to_string(), "String".to_string()),
                ("data".to_string(), "Blob".to_string()),
            ],
            return_type: "Int".to_string(),
            needs_state: true,
            param_ownership: vec!["owned".to_string(), "owned".to_string()],
            return_ownership: Some("value".to_string()),
        }];
        let result = serialize_exports_tagged(&exports);
        assert_eq!(result, "export,write_file,String|Blob,Int,state,owned|owned,value");
    }

    #[test]
    fn test_serialize_ctypes_dbvl() {
        let mut map = HashMap::new();
        map.insert("Int".to_string(), "int64_t".to_string());
        map.insert("Float".to_string(), "double".to_string());
        let result = serialize_ctypes_dbvl(&map);
        assert!(result.contains("ctype,Int,int64_t"));
        assert!(result.contains("ctype,Float,double"));
    }

    #[test]
    fn test_serialize_ctypes_dbvl_empty() {
        let map = HashMap::new();
        let result = serialize_ctypes_dbvl(&map);
        assert_eq!(result, "");
    }

    #[test]
    fn test_serialize_exports_tagged_multiple_separate() {
        let exports = vec![
            ExportDecl {
                name: "add".to_string(),
                params: vec![("a".to_string(), "Int".to_string()), ("b".to_string(), "Int".to_string())],
                return_type: "Int".to_string(),
                needs_state: false,
                param_ownership: vec!["value".to_string(), "value".to_string()],
                return_ownership: Some("value".to_string()),
            },
            ExportDecl {
                name: "greet".to_string(),
                params: vec![("name".to_string(), "String".to_string())],
                return_type: "String".to_string(),
                needs_state: true,
                param_ownership: vec!["borrowed".to_string()],
                return_ownership: Some("owned".to_string()),
            },
        ];
        let result = serialize_exports_tagged(&exports);
        assert_eq!(result, "export,add,Int|Int,Int,pure,value|value,value\nexport,greet,String,String,state,borrowed,owned");
    }
}
