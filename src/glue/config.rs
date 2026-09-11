// ── GLUE Configuration (Data Briev) ───────────────────────────────────
// 2026-08-03 (plan 2026-08-03-glue-folders-node-bridge): the GLUE registry is
// a per-language folder tree — lib/glue/<lang>/glue.dbv. The compiler finds
// the relevant folder BY NAME on invocation (`briev export|bindings|extension
// <bridge> <lang>`) and loads that file; the full registry (all languages) is
// the merged scan, used by extension routing (find_language_by_extension) and
// ConfigGet$. Replaced the monolithic config/glue.dbv (itself migrated from
// lib/glue.toml). The compiler carries zero language knowledge — the glue
// folder is data.
//
// Format (one entry per language, quoted mode):
//   <lang>: { types_module: "…"; extension: "…"; bridge_kind: "…";
//             calling_convention: "…"; module_init: true;
//             protocols: { "#String": { native: "…"; c_abi: "…"; }; };
//             templates: { "file": "…\n…"; "fn_template": "…"; }; };

use crate::dbriev::config_db::ConfigDb;
use crate::dbriev::v2::DataValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A language target entry from the GLUE registry.
///
/// 2026-07-22: Each target describes how to bridge with one foreign language.
/// Protocol mapping replaces old type_map/c_type_map/conversions —
/// the config only knows about protocol categories (#String, #Int, #Float),
/// not about Briev-internal type names.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GlueTarget {
    /// Language identifier (e.g., "python", "rust", "node")
    pub language: String,
    /// Path to .bv file declaring foreign type representations
    pub types_module: PathBuf,
    /// Native source file extension (without dot)
    pub extension: String,
    /// Bridge strategy: "native_module", "esm_module", "extern_c_crate", etc.
    pub bridge_kind: String,
    /// Calling convention: "c_abi", "lto", etc.
    pub calling_convention: String,
    /// 2026-07-23: Emit native C extension module init metadata
    /// (PyMethodDef/PyModuleDef for Python, NAPI for Node, etc.).
    /// When true, the LLVM backend emits module init at codegen time.
    pub module_init: bool,
    /// Protocol category → native/C ABI type mapping.
    /// Keys like "#String", "#Int", "#Float" — protocol categories only,
    /// never Briev-internal type names.
    pub protocols: HashMap<String, ProtocolEntry>,
    /// Output path → template content. Special keys:
    ///   "fn_template" — per-function safe wrapper (rendered into {{exports}})
    ///   "ffi_template" — per-function FFI declaration (rendered into {{ffi_decls}})
    pub templates: HashMap<String, String>,
    /// 2026-08-03: Per-protocol ABI conversion expressions. `to_abi` renders
    /// a native argument value to its boundary form (placeholder `{name}`);
    /// `from_abi` renders the boundary result back to a native value
    /// (the raw result is `result_abi`). Defaults to identity when absent —
    /// ctypes/ffi-napi/LTO handle typing on their side. No compiler-side
    /// language knowledge.
    pub conversions: Conversions,
    /// 2026-08-03: How the leading `%state` handle is represented on this
    /// language's side of the boundary (body-dependent ABI). Pure exports
    /// omit it entirely.
    pub state: StateAbi,
    /// 2026-08-03: Parameter declaration format for FFI signatures.
    /// `{name}` / `{type}` placeholders; default `{name}: {type}`.
    /// C-family targets use `{type} {name}`.
    pub param_decl: String,
    /// 2026-08-03: Declaration format for function-pointer (callback) params.
    /// `{ret}` / `{name}` / `{params}` placeholders. C: `{ret} (*{name})({params})`.
    pub fn_param_decl: String,
    /// 2026-08-03 (P2, plan glue-folders-node-bridge): the native-extension
    /// build recipe — how to compile + link the generated shim for this
    /// language. Each command emits its value on stdout; None = absent.
    /// Keeps ALL toolchain knowledge out of the compiler (config-driven).
    pub native_include_cmd: Option<String>,
    /// Literal output suffix for the built extension (e.g. node: ".node").
    pub native_suffix: Option<String>,
    /// Command whose stdout is the output suffix (e.g. python's
    /// `python3-config --extension-suffix`).
    pub native_suffix_cmd: Option<String>,
    /// Command whose stdout is the link flags (python's `python3-config
    /// --ldflags`; node needs none — symbols resolve at load).
    pub native_link_cmd: Option<String>,
    /// The C compiler to invoke (default "cc").
    pub native_cc: Option<String>,
    /// 2026-08-04 (ship common languages): filename prefix for the built
    /// extension — the JVM's `System.loadLibrary("x")` loads `libx.so`, so the
    /// java target sets "lib"; python/node default to "".
    pub native_prefix: Option<String>,
}

/// How the state handle crosses the boundary for one language.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StateAbi {
    /// C-ABI parameter declaration (e.g. `BrievState* state`), used in
    /// prototypes / extern blocks when an export needs state.
    pub decl: String,
    /// The state value expression at the call site (e.g. `STATE`, `_STATE`).
    pub arg: String,
    /// Host FFI arg-type list entry (e.g. `ctypes.c_void_p`, `'pointer'`).
    pub ffi_type: String,
}

/// Boundary conversion expressions per protocol category.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conversions {
    /// Protocol → expression turning `{name}` into the ABI argument form.
    pub to_abi: HashMap<String, String>,
    /// Protocol → expression turning the raw `result_abi` into the native
    /// return value.
    pub from_abi: HashMap<String, String>,
}

/// A protocol category mapping for a single language.
///
/// 2026-07-22: Each protocol category (#String, #Int, #Float) maps to
/// the language's native type and its C ABI representation. The compiler
/// uses this when the BFS finds a path through that protocol category.
///
/// 2026-07-26: Added wasm_abi for the web target (calling_convention = "wasm_import").
/// When present, the target prefers wasm_abi over c_abi for FFI marshalling.
/// wasm_abi values are WebAssembly value types: i32, i64, f32, f64.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ProtocolEntry {
    /// Language-native type name (e.g., "str", "String", "int", "number", "Element")
    pub native: String,
    /// C ABI type name (e.g., "i64", "ctypes.c_int64", "cstring")
    #[serde(default)]
    pub c_abi: Option<String>,
    /// WASM ABI type name (e.g., "i32", "i64", "f32", "f64").
    /// 2026-07-26: Used when calling_convention is "wasm_import".
    /// Maps to WebAssembly import/export value types.
    #[serde(default)]
    pub wasm_abi: Option<String>,
}

/// Load the GLUE registry (Data Briev). `None` scans the per-language glue
/// folders (lib/glue/<lang>/glue.dbv) and merges by folder name; `Some(path)`
/// loads a single config file (the `--glue-config` override / tests).
pub fn load_glue_config(path: Option<&Path>) -> Result<HashMap<String, GlueTarget>, String> {
    match path {
        Some(p) => {
            let source = std::fs::read_to_string(p)
                .map_err(|e| format!("Failed to read GLUE config '{}': {}", p.display(), e))?;
            parse_glue_dbvl(&source)
        }
        None => load_glue_folders(),
    }
}

/// Load one language's glue config BY NAME — `lib/glue/<lang>/glue.dbv`.
/// Used by `briev export|bindings|extension <bridge> <lang>` after the
/// invocation resolves the language folder.
pub fn load_glue_language(lang: &str) -> Result<GlueTarget, String> {
    let glue_root = resolve_glue_root().ok_or_else(|| {
        format!("lib/glue/ not found — cannot load glue config for '{}'", lang)
    })?;
    let cfg = glue_root.join(lang).join("glue.dbv");
    if !cfg.exists() {
        return Err(format!(
            "no glue config for language '{}' (wanted lib/glue/{}/glue.dbv)",
            lang, lang
        ));
    }
    let source = std::fs::read_to_string(&cfg)
        .map_err(|e| format!("Failed to read '{}': {}", cfg.display(), e))?;
    let parsed = parse_glue_dbvl(&source)?;
    parsed.get(lang).cloned().ok_or_else(|| {
        format!("'{}' has no '{}' target entry", cfg.display(), lang)
    })
}

/// Scan lib/glue/*/glue.dbv and merge every language into the registry,
/// keyed by folder name.
fn load_glue_folders() -> Result<HashMap<String, GlueTarget>, String> {
    let glue_root = resolve_glue_root().ok_or_else(|| {
        "lib/glue/ not found — per-language glue configs unavailable".to_string()
    })?;
    let mut targets: HashMap<String, GlueTarget> = HashMap::new();
    let mut found = false;
    let entries = std::fs::read_dir(&glue_root)
        .map_err(|e| format!("cannot read glue dir '{}': {}", glue_root.display(), e))?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let cfg = dir.join("glue.dbv");
        if !cfg.exists() {
            continue;
        }
        let source = std::fs::read_to_string(&cfg)
            .map_err(|e| format!("Failed to read '{}': {}", cfg.display(), e))?;
        found = true;
        targets.extend(parse_glue_dbvl(&source)?);
    }
    if !found {
        return Err(format!(
            "no lib/glue/*/glue.dbv found under '{}'",
            glue_root.display()
        ));
    }
    Ok(targets)
}

/// Locate the per-language glue folder root: project-local lib/glue, then the
/// executable-relative dev layout, then the compiler-shipped lib/glue.
fn resolve_glue_root() -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        let local = cwd.join("lib/glue");
        if local.is_dir() {
            return Some(local);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let dev = dir.join("../../lib/glue");
            if dev.is_dir() {
                return Some(dev);
            }
        }
    }
    let baked = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lib/glue");
    if baked.is_dir() {
        return Some(baked);
    }
    None
}

/// Parse Data Briev glue config into GlueTargets.
///
/// Layout: one `<lang>: { … }` entry per language (scalars + protocols map),
/// plus positional template lines `<lang>.templates.<n>: "<output path>"
/// "<content>";` (flat line form — long `\n`-escaped values and `/` in
/// output paths trip the nested named-fields-map parser).
fn parse_glue_dbvl(source: &str) -> Result<HashMap<String, GlueTarget>, String> {
    let db = ConfigDb::from_quoted_str(source)?;

    let templates_by_lang = collect_templates(&db);

    let mut targets: HashMap<String, GlueTarget> = HashMap::new();
    for key in db.keys() {
        if key.contains(".templates.") || key.contains(".bindings.") {
            continue;
        }
        let Some(DataValue::Map(entry)) = db.field(&key, 0) else { continue };
        let Some(target) = glue_target_from_entry(&key, entry, &templates_by_lang) else { continue };
        targets.insert(key, target);
    }
    Ok(targets)
}

/// Collect `<lang>.templates.<n>` (output path, content) and
/// `<lang>.bindings.<file>` (filename in key, content in field 0) into a
/// per-language templates map. `bindings.*` keys are prefixed so `briev
/// bindings` can find them.
fn collect_templates(db: &ConfigDb) -> HashMap<String, HashMap<String, String>> {
    let mut templates_by_lang: HashMap<String, HashMap<String, String>> = HashMap::new();
    for key in db.keys() {
        if let Some((lang, path, content)) = template_line(db, &key) {
            templates_by_lang.entry(lang).or_default().insert(path, content);
        }
    }
    templates_by_lang
}

/// Parse one flat template line into `(lang, template_key, content)`.
/// Templates: `<lang>.templates.<n>` with field 0 = output path, field 1 =
/// content. Bindings: `<lang>.bindings.<file>` with the filename in the key
/// and field 0 = content (stored under `bindings.<file>`).
fn template_line(db: &ConfigDb, key: &str) -> Option<(String, String, String)> {
    if let Some(pos) = key.find(".templates.") {
        let lang = key[..pos].to_string();
        let path = db.field_string(key, 0)?;
        let content = db.field_string(key, 1)?;
        Some((lang, path.to_string(), content.to_string()))
    } else if let Some(pos) = key.find(".bindings.") {
        let lang = key[..pos].to_string();
        let content = db.field_string(key, 0)?;
        let suffix = &key[pos + ".bindings.".len()..];
        Some((lang, format!("bindings.{}", suffix), content.to_string()))
    } else {
        None
    }
}

/// Build one GlueTarget from a `<lang>: { … }` entry map.
fn glue_target_from_entry(
    key: &str,
    entry: &HashMap<String, DataValue>,
    templates_by_lang: &HashMap<String, HashMap<String, String>>,
) -> Option<GlueTarget> {
    let str_field = |name: &str| -> Option<String> {
        entry.get(name).and_then(|v| match v {
            DataValue::String(s) => Some(s.clone()),
            _ => None,
        })
    };
    let string_map = |name: &str| -> HashMap<String, String> {
        match entry.get(name) {
            Some(DataValue::Map(map)) => map.iter()
                .filter_map(|(k, v)| match v {
                    DataValue::String(s) => Some((k.clone(), s.clone())),
                    _ => None,
                })
                .collect(),
            _ => HashMap::new(),
        }
    };
    let types_module = str_field("types_module")?;
    let extension = str_field("extension")?;
    let bridge_kind = str_field("bridge_kind")?;
    let calling_convention = str_field("calling_convention")?;
    let module_init = matches!(entry.get("module_init"), Some(DataValue::Bool(true)));
    let protocols = match entry.get("protocols") {
        Some(DataValue::Map(map)) => map.iter()
            .map(|(proto, v)| {
                let proto_entry = match v {
                    DataValue::Map(fields) => ProtocolEntry {
                        native: str_from(fields.get("native")).unwrap_or_default(),
                        c_abi: str_from(fields.get("c_abi")),
                        wasm_abi: str_from(fields.get("wasm_abi")),
                    },
                    _ => ProtocolEntry { native: String::new(), c_abi: None, wasm_abi: None },
                };
                (proto.clone(), proto_entry)
            })
            .collect(),
        _ => HashMap::new(),
    };
    let conversions = match entry.get("conversions") {
        Some(DataValue::Map(conv)) => Conversions {
            to_abi: string_map_from(conv.get("to_abi")),
            from_abi: string_map_from(conv.get("from_abi")),
        },
        _ => Conversions::default(),
    };
    let state = match entry.get("state") {
        Some(DataValue::Map(m)) => StateAbi {
            decl: str_from(m.get("decl")).unwrap_or_default(),
            arg: str_from(m.get("arg")).unwrap_or_default(),
            ffi_type: str_from(m.get("ffi_type")).unwrap_or_default(),
        },
        _ => StateAbi::default(),
    };
    let param_decl = str_field("param_decl").unwrap_or_else(|| "{name}: {type}".to_string());
    let fn_param_decl = str_field("fn_param_decl").unwrap_or_else(|| "{name}: {type}".to_string());
    Some(GlueTarget {
        language: key.to_string(),
        types_module: PathBuf::from(types_module),
        extension,
        bridge_kind,
        calling_convention,
        module_init,
        protocols,
        templates: templates_by_lang.get(key).cloned().unwrap_or_default(),
        conversions,
        state,
        param_decl,
        fn_param_decl,
        native_include_cmd: str_field("native_include_cmd"),
        native_suffix: str_field("native_suffix"),
        native_suffix_cmd: str_field("native_suffix_cmd"),
        native_link_cmd: str_field("native_link_cmd"),
        native_cc: str_field("native_cc"),
        native_prefix: str_field("native_prefix"),
    })
}

fn str_from(v: Option<&DataValue>) -> Option<String> {
    match v {
        Some(DataValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn string_map_from(v: Option<&DataValue>) -> HashMap<String, String> {
    match v {
        Some(DataValue::Map(map)) => map.iter()
            .filter_map(|(k, v)| match v {
                DataValue::String(s) => Some((k.clone(), s.clone())),
                _ => None,
            })
            .collect(),
        _ => HashMap::new(),
    }
}
/// Find a language target by extension.
///
/// 2026-07-22: Matches the file extension (without dot) against known
/// language targets. Returns the language name and its GlueTarget.
/// This is the primary lookup used during frgn dispatch resolution.
pub fn find_language_by_extension<'a>(
    targets: &'a HashMap<String, GlueTarget>,
    ext: &str,
) -> Option<&'a GlueTarget> {
    let ext = ext.trim_start_matches('.');
    targets.values().find(|t| t.extension == ext)
}

/// Map a file extension to a language identifier using the loaded targets.
/// Returns Some(language_name) if the extension is recognized, None otherwise.
pub fn extension_to_language<'a>(ext: &str, targets: &'a HashMap<String, GlueTarget>) -> Option<&'a str> {
    let ext = ext.trim_start_matches('.');
    targets.values().find(|t| t.extension == ext).map(|t| t.language.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_to_language_via_targets() {
        let mut targets = HashMap::new();
        targets.insert("python".to_string(), GlueTarget {
            language: "python".to_string(),
            types_module: PathBuf::from("glue/python/types.bv"),
            extension: "py".to_string(),
            bridge_kind: "native_module".to_string(),
            calling_convention: "c_abi".to_string(),
            module_init: false,
            protocols: HashMap::new(),
            templates: HashMap::new(),
            conversions: Conversions::default(),
            state: crate::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            ..Default::default()
        });
        targets.insert("rust".to_string(), GlueTarget {
            language: "rust".to_string(),
            types_module: PathBuf::from("glue/rust/types.bv"),
            extension: "rs".to_string(),
            bridge_kind: "extern_c_crate".to_string(),
            calling_convention: "lto".to_string(),
            module_init: false,
            protocols: HashMap::new(),
            templates: HashMap::new(),
            conversions: Conversions::default(),
            state: crate::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            ..Default::default()
        });

        // Should work with or without leading dot
        assert_eq!(find_language_by_extension(&targets, ".py").unwrap().language, "python");
        assert_eq!(find_language_by_extension(&targets, "py").unwrap().language, "python");
    }

    #[test]
    fn test_find_language_by_extension() {
        let mut targets = HashMap::new();
        targets.insert("python".to_string(), GlueTarget {
            language: "python".to_string(),
            types_module: PathBuf::from("glue/python/types.bv"),
            extension: "py".to_string(),
            bridge_kind: "native_module".to_string(),
            calling_convention: "c_abi".to_string(),
            module_init: false,
            protocols: HashMap::new(),
            templates: HashMap::new(),
            conversions: Conversions::default(),
            state: crate::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            ..Default::default()
        });

        // Should work with or without leading dot
        assert_eq!(find_language_by_extension(&targets, ".py").unwrap().language, "python");
        assert_eq!(find_language_by_extension(&targets, "py").unwrap().language, "python");
    }

    #[test]
    fn test_find_language_by_extension_with_dot() {
        let mut targets = HashMap::new();
        targets.insert("python".to_string(), GlueTarget {
            language: "python".to_string(),
            types_module: PathBuf::from("glue/python/types.bv"),
            extension: "py".to_string(),
            bridge_kind: "native_module".to_string(),
            calling_convention: "c_abi".to_string(),
            module_init: false,
            protocols: HashMap::new(),
            templates: HashMap::new(),
            conversions: Conversions::default(),
            state: crate::glue::config::StateAbi::default(),
            param_decl: "{name}: {type}".to_string(),
            fn_param_decl: "{name}: {type}".to_string(),
            ..Default::default()
        });

        // Should work with or without leading dot
        assert_eq!(find_language_by_extension(&targets, ".py").unwrap().language, "python");
        assert_eq!(find_language_by_extension(&targets, "py").unwrap().language, "python");
    }

    #[test]
    fn test_load_glue_config_custom_path() {
        let dir = std::env::temp_dir();
        let config_path = dir.join("test_glue_config.dbvl");
        let content = r##"python: { types_module: "glue/python/types.bv"; extension: "py"; bridge_kind: "native_module"; calling_convention: "c_abi"; module_init: false; protocols: { "#String": { native: "str"; c_abi: "ctypes.c_void_p" }; "#Int": { native: "int"; c_abi: "ctypes.c_int64" } } };
python.templates.0: "fn_template" "def {{name}}({{params}}):\n    return {{name}};\n";
rust: { types_module: "glue/rust/types.bv"; extension: "rs"; bridge_kind: "extern_c_crate"; calling_convention: "lto"; module_init: false };"##;
        std::fs::write(&config_path, content).unwrap();

        let result = load_glue_config(Some(&config_path));
        assert!(result.is_ok(), "load_glue_config failed: {:?}", result.err());
        let targets = result.unwrap();

        assert!(targets.contains_key("python"));
        let py = &targets["python"];
        assert_eq!(py.language, "python");
        assert_eq!(py.extension, "py");
        assert_eq!(py.bridge_kind, "native_module");
        assert_eq!(py.calling_convention, "c_abi");
        assert!(py.protocols.contains_key("#String"));
        assert_eq!(py.protocols.get("#String").unwrap().native, "str");
        assert_eq!(py.protocols.get("#String").unwrap().c_abi.as_deref(), Some("ctypes.c_void_p"));
        assert_eq!(py.templates.get("fn_template").unwrap(), "def {{name}}({{params}}):\n    return {{name}};\n");

        assert!(targets.contains_key("rust"));
        let rust = &targets["rust"];
        assert_eq!(rust.language, "rust");
        assert_eq!(rust.extension, "rs");
        assert_eq!(rust.bridge_kind, "extern_c_crate");
        assert_eq!(rust.calling_convention, "lto");

        let _ = std::fs::remove_file(&config_path);
    }

    #[test]
    fn test_load_glue_config_not_found() {
        let dir = std::env::temp_dir();
        let missing = dir.join("nonexistent_glue.dbv");
        let result = load_glue_config(Some(&missing));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_glue_config_default_path_exists() {
        // The compiler-shipped default (config/glue.dbv) should exist.
        let result = load_glue_config(None);
        assert!(result.is_ok(), "Default config/glue.dbv not found: {:?}", result.err());
    }

    /// Baked config golden: the compiler-shipped config/glue.dbv loads with
    /// all four languages and the expected protocol/template shape. Replaces
    /// the TOML parity gate after lib/glue.toml was deleted.
    #[test]
    fn baked_glue_dbvl_shape() {
        let config = load_glue_config(None).expect("config/glue.dbv should load");
        for lang in ["python", "rust", "node", "web"] {
            assert!(config.contains_key(lang), "missing '{}' target", lang);
        }
        let python = &config["python"];
        assert_eq!(python.calling_convention, "c_abi");
        assert!(python.module_init);
        assert_eq!(
            // 2026-09-11 (Phase A6): protocol keys are the BARE category.
            python.protocols.get("String").unwrap().c_abi.as_deref(),
            Some("ctypes.c_void_p")
        );
        assert!(python.templates.contains_key("__init__.py"), "python __init__.py template");
        assert!(python.templates.contains_key("fn_template"), "python fn_template template");
        let rust = &config["rust"];
        assert_eq!(rust.calling_convention, "lto");
        assert!(rust.templates.contains_key("src/lib.rs"), "rust src/lib.rs template");
        let node = &config["node"];
        assert!(node.templates.contains_key("index.mjs"), "node index.mjs template");
        let web = &config["web"];
        assert_eq!(web.calling_convention, "wasm_import");
        assert!(web.templates.contains_key("dom-shim.mjs"), "web dom-shim.mjs template");
    }
}
