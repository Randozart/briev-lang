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
//             protocols: { "String": { native: "…"; c_abi: "…"; }; };
//             templates: { "file": "…\n…"; "fn_template": "…"; }; };

use crate::dbriev::config_db::ConfigDb;
use crate::dbriev::v2::DataValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A language target entry from the GLUE registry.
///
/// 2026-07-22: Each target describes how to bridge with one foreign language.
/// Protocol mapping replaces old type_map/c_type_map/conversions —
/// the config only knows about protocol categories (String, Int, Float),
/// not about Briev-internal type names.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GlueTarget {
    /// 2026-11-16 (S3/G1): boundary value classes — how a Briev type maps onto a
    /// representation crossing this boundary. Keyed by the Briev protocol name
    /// (`String`). Only classes with a non-`scalar` class and a `to_op`/`from_op`
    /// pair render a boundary type + protocol; scalars are the base types and
    /// need no generated declaration.
    pub values: HashMap<String, ValueClass>,
    /// 2026-11-16 (FFI contract plan S2): the ABI core this adapter targets
    /// (`config/abi/<name>.dbvl`). Optional: an adapter whose boundary is not a
    /// machine ABI (e.g. a JS host module) omits it. When present it must name a
    /// shipped core — validated at load, so a typo cannot silently fall back.
    pub abi: Option<String>,
    /// 2026-11-16 (FFI contract plan D3): descriptor schema version. A file
    /// written for a schema the compiler does not implement is refused rather
    /// than silently read with defaults — see `validate_target_entry`.
    pub version: i64,
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
    /// Keys like "String", "Int", "Float" — protocol categories only,
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
    /// 2026-10-06 (web host-JS shipping): host function JS bodies for the
    /// `wasm_runtime`/web target — `name -> JS function expression` (e.g.
    /// `"log": "(msg) => console.log(msg)"`). The generated shim emits each at
    /// MODULE SCOPE as `const <name> = <body>;` so a web host frgn's import
    /// stub (which calls `<name>(...)`) resolves. Retained for the legacy
    /// config-driven path; the current browser host module is a `.js` file
    /// (`lib/glue/web/web.js`, host-boundary decision record D7).
    pub host_fns: HashMap<String, String>,
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
/// 2026-07-22: Each protocol category (String, Int, Float) maps to
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
        format!(
            "lib/glue/ not found — cannot load glue config for '{}'",
            lang
        )
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
    parsed
        .get(lang)
        .cloned()
        .ok_or_else(|| format!("'{}' has no '{}' target entry", cfg.display(), lang))
}

/// Scan lib/glue/*/glue.dbv and merge every language into the registry,
/// keyed by folder name.
fn load_glue_folders() -> Result<HashMap<String, GlueTarget>, String> {
    let glue_root = resolve_glue_root()
        .ok_or_else(|| "lib/glue/ not found — per-language glue configs unavailable".to_string())?;
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
    let baked = crate::resource_root().join("lib/glue");
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
        let Some(DataValue::Map(entry)) = db.field(&key, 0) else {
            continue;
        };
        validate_target_entry(&key, entry)?;
        let Some(target) = glue_target_from_entry(&key, entry, &templates_by_lang) else {
            continue;
        };
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
            templates_by_lang
                .entry(lang)
                .or_default()
                .insert(path, content);
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

/// The descriptor schema version this compiler implements.
///
/// 2026-11-16 (FFI contract plan D3): `glue.dbv` now carries semantics
/// (ownership conventions, discipline, capability list), so an unversioned file
/// is a silent-wrong-answer generator in both directions — a newer descriptor
/// read by an older compiler loses sections, and an older one read by a newer
/// compiler takes defaults for them. Version + unknown-key rejection turns both
/// into a loud error, which is the only acceptable failure mode for a file that
/// decides how a program talks to the outside world.
pub const GLUE_SCHEMA_VERSION: i64 = 1;

/// Keys a `<lang>: { … }` target entry may carry.
const KNOWN_TARGET_KEYS: &[&str] = &[
    "version",
    "types_module",
    "extension",
    "bridge_kind",
    "calling_convention",
    "module_init",
    "protocols",
    "conversions",
    "state",
    "param_decl",
    "fn_param_decl",
    "templates",
    "native_include_cmd",
    "host_fns",
    "native_suffix",
    "native_suffix_cmd",
    "native_link_cmd",
    "native_cc",
    "native_prefix",
    // 2026-11-16: the boundary-contract sections (FFI plan §4.3). Declared here
    // so a descriptor may state them before their consumers land; each is
    // additive and defaults to "absent".
    "values",
    "frees",
    "discipline",
    "errors",
    "supports",
    "abi",
];

/// Nested key sets, so a typo inside a section is caught as well as at the top.
fn known_keys_for(section: &str) -> Option<&'static [&'static str]> {
    match section {
        "values" => Some(&[
            "class",
            "zero_copy",
            "layout",
            "to",
            "from",
            "len",
            "retain_op",
            "release_op",
            "kind",
            "copy",
        ]),
        "discipline" => Some(&["blocking", "reentrant", "needs_main_thread"]),
        "errors" => Some(&["class", "success"]),
        "state" => Some(&["decl", "arg", "ffi_type"]),
        // `conversions` children are protocol names (handled like the others).
        "conversions" => Some(&["to_abi", "from_abi"]),
        _ => None,
    }
}

/// Reject an unknown key with what/why/fix, the way every other descriptor
/// diagnostic reads. `path` is the key path (`c : values.String`) so the author
/// can find it without a span — `.dbv` values carry none (`dbriev/v2.rs:88`).
fn reject_unknown_key(path: &str, key: &str, known: &[&str]) -> Result<(), String> {
    if known.contains(&key) {
        return Ok(());
    }
    let mut sorted: Vec<&str> = known.to_vec();
    sorted.sort_unstable();
    Err(format!(
        "unknown key `{key}` in glue descriptor at `{path}` — known keys: {}. \
         A key the compiler does not recognise is refused rather than ignored: \
         a silently-dropped section would change how the boundary is marshalled \
         without any diagnostic.",
        sorted.join(", ")
    ))
}

/// Validate one `<lang>` entry: schema version present and supported, and no
/// unknown key at any level this compiler reads.
fn validate_target_entry(lang: &str, entry: &HashMap<String, DataValue>) -> Result<(), String> {
    validate_version(lang, entry)?;
    validate_abi_core(lang, entry)?;
    for (key, value) in entry {
        reject_unknown_key(lang, key, KNOWN_TARGET_KEYS)?;
        validate_section(lang, key, value)?;
    }
    Ok(())
}

/// The `abi:` gate: when an adapter names an ABI core, that core must be shipped.
/// A typo is refused rather than silently leaving the adapter without ABI facts,
/// because "no core" and "core I did not find" would behave differently later.
fn validate_abi_core(lang: &str, entry: &HashMap<String, DataValue>) -> Result<(), String> {
    let Some(DataValue::String(name)) = entry.get("abi") else {
        return Ok(());
    };
    let Err(load_err) = crate::glue::abi::load_core(name) else {
        return Ok(());
    };
    // 2026-11-17: "cannot find" and "found but cannot read it" are different
    // repairs. The old message reported both as "not shipped" and listed the
    // existing cores — which says the core is absent while the file sits right
    // there. Found by pointing `--config-dir` at a deliberately broken core.
    if let Some(path) = crate::glue::abi::core_path(name) {
        return Err(format!(
            "ABI core `{name}` at `{}` cannot be used: {load_err}. Fix the core, \
             or point the glue descriptor at a core that loads.",
            path.display()
        ));
    }
    let known = crate::glue::abi::core_names();
    Err(format!(
        "glue descriptor for `{lang}` names ABI core `{name}`, which is not \
         shipped. Known cores: {}. Each core is `config/abi/<name>.dbvl`; write \
         the descriptor against one of them or add the core.",
        if known.is_empty() {
            "<none found>".to_string()
        } else {
            known.join(", ")
        }
    ))
}

/// The version gate: present, integral, and within what this compiler reads.
fn validate_version(lang: &str, entry: &HashMap<String, DataValue>) -> Result<(), String> {
    match entry.get("version") {
        Some(DataValue::Int(n)) if *n >= 1 && *n <= GLUE_SCHEMA_VERSION => Ok(()),
        Some(DataValue::Int(n)) => Err(format!(
            "glue descriptor for `{lang}` declares version {n} but this compiler \
             implements {GLUE_SCHEMA_VERSION}. Upgrade the compiler, or write the \
             descriptor for version {GLUE_SCHEMA_VERSION}."
        )),
        Some(_) => Err(format!(
            "glue descriptor for `{lang}`: `version` must be an integer (this \
             compiler implements {GLUE_SCHEMA_VERSION})"
        )),
        None => Err(format!(
            "glue descriptor for `{lang}` has no `version`. Add `version: \
             {GLUE_SCHEMA_VERSION};` — an unversioned descriptor cannot be read \
             safely once the schema carries semantics."
        )),
    }
}

/// Validate one top-level section whose children are fixed keys (`discipline`,
/// `errors`, `state`, `conversions`) or named entries with fixed fields
/// (`protocols.<Name>`, `values.<Class>[.to|.from]`).
fn validate_section(lang: &str, key: &str, value: &DataValue) -> Result<(), String> {
    let Some(nested) = known_keys_for(key) else {
        return Ok(());
    };
    let DataValue::Map(fields) = value else {
        return Ok(());
    };
    if key == "protocols" || key == "values" {
        return validate_named_entries(lang, key, fields, nested);
    }
    for field in fields.keys() {
        reject_unknown_key(lang, field, nested)?;
    }
    Ok(())
}

/// `protocols.<Name>` / `values.<Class>`: the child names are user-chosen, so
/// only their fields are checked — plus one more level for `values.<Class>.to`.
fn validate_named_entries(
    lang: &str,
    section: &str,
    entries: &HashMap<String, DataValue>,
    _known: &[&str],
) -> Result<(), String> {
    let entry_keys: &[&str] = if section == "protocols" {
        &["native", "c_abi", "wasm_abi"]
    } else {
        // 2026-11-16: flat by necessity — the .dbv quoted-mode parser nests two
        // levels, and `values.<Class>.to` would be a third. The `_op` suffix
        // makes it obvious the value names an operation, not a nested record.
        &[
            "class",
            "zero_copy",
            "layout",
            "len",
            "len_op",
            "to_op",
            "from_op",
            "from_copy",
            "retain_op",
            "release_op",
            "kind",
            // 2026-11-16 (S3/G1): the names the generator emits. They are
            // language facts (what this boundary is called here), so they belong
            // in data rather than in a naming convention inside the compiler.
            "boundary",
            "protocol",
            "base",
        ]
    };
    for (name, body) in entries {
        let path = format!("{lang} : {section}.{name}");
        let DataValue::Map(fields) = body else {
            continue;
        };
        for field in fields.keys() {
            reject_unknown_key(&path, field, entry_keys)?;
        }
    }
    Ok(())
}

/// Build one GlueTarget from a `<lang>: { … }` entry map./// Build one GlueTarget from a `<lang>: { … }` entry map.
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
            Some(DataValue::Map(map)) => map
                .iter()
                .filter_map(|(k, v)| match v {
                    DataValue::String(s) => Some((k.clone(), s.clone())),
                    _ => None,
                })
                .collect(),
            _ => HashMap::new(),
        }
    };
    let values = match entry.get("values") {
        Some(DataValue::Map(map)) => map
            .iter()
            .map(|(name, v)| (name.clone(), ValueClass::from_entry(v)))
            .collect(),
        _ => HashMap::new(),
    };
    let abi = match entry.get("abi") {
        Some(DataValue::String(name)) => Some(name.clone()),
        _ => None,
    };
    let version = match entry.get("version") {
        Some(DataValue::Int(n)) => *n,
        _ => 0,
    };
    let types_module = str_field("types_module")?;
    let extension = str_field("extension")?;
    let bridge_kind = str_field("bridge_kind")?;
    let calling_convention = str_field("calling_convention")?;
    let module_init = matches!(entry.get("module_init"), Some(DataValue::Bool(true)));
    let protocols = match entry.get("protocols") {
        Some(DataValue::Map(map)) => map
            .iter()
            .map(|(proto, v)| {
                let proto_entry = match v {
                    DataValue::Map(fields) => ProtocolEntry {
                        native: str_from(fields.get("native")).unwrap_or_default(),
                        c_abi: str_from(fields.get("c_abi")),
                        wasm_abi: str_from(fields.get("wasm_abi")),
                    },
                    _ => ProtocolEntry {
                        native: String::new(),
                        c_abi: None,
                        wasm_abi: None,
                    },
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
        values,
        abi,
        version,
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
        host_fns: string_map("host_fns"),
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
        Some(DataValue::Map(map)) => map
            .iter()
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
///
/// 2026-10-07 (host-boundary decision record): DETERMINISTIC. A HashMap's
/// iteration order is seed-dependent; `node` and `web` both claimed `.mjs`,
/// so `from "x.mjs"` resolved by hash order (a determinism-rule violation).
/// Two fixes: iterate in sorted key order, and exclude host runtimes
/// (`bridge_kind == "wasm_runtime"`) — a host is reached by provenance
/// (`#System`/host marker), never by a source-file extension, so it must not
/// compete for one.
pub fn find_language_by_extension<'a>(
    targets: &'a HashMap<String, GlueTarget>,
    ext: &str,
) -> Option<&'a GlueTarget> {
    let ext = ext.trim_start_matches('.');
    let mut keys: Vec<&String> = targets.keys().collect();
    keys.sort();
    keys.into_iter()
        .map(|k| &targets[k])
        .find(|t| t.extension == ext && t.bridge_kind != "wasm_runtime")
}

/// Map a file extension to a language identifier using the loaded targets.
/// Returns Some(language_name) if the extension is recognized, None otherwise.
pub fn extension_to_language<'a>(
    ext: &str,
    targets: &'a HashMap<String, GlueTarget>,
) -> Option<&'a str> {
    find_language_by_extension(targets, ext).map(|t| t.language.as_str())
}

/// One boundary value class of a language adapter.
///
/// 2026-11-16 (S3/G1): the *representation* half of the contract. Ownership of a
/// specific call is declared per function (D1), so nothing here states who frees
/// what — only how a class is represented and which operations realise the lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValueClass {
    /// `scalar`, `borrowed_bytes`, `owned_buffer`, `opaque_handle`, `aggregate`,
    /// `callback`, `error_channel`.
    pub class: String,
    /// Outbound lane is a representation view, not a copy.
    pub zero_copy: bool,
    /// Layout hint for a scalar class (a C type name such as `i8`).
    pub layout: Option<String>,
    /// Length accessor operation, for `borrowed_bytes` with a split (ptr, len).
    pub len_op: Option<String>,
    /// Outbound conversion operation (Briev → boundary).
    pub to_op: Option<String>,
    /// Inbound conversion operation (boundary → Briev).
    pub from_op: Option<String>,
    /// The inbound lane must copy (mandatory for representations that differ).
    pub from_copy: bool,
    pub retain_op: Option<String>,
    pub release_op: Option<String>,
    /// Language-native kind marker for `opaque_handle` (`pyobj`, `jni_ref`, …).
    pub kind: Option<String>,
    /// The boundary type name this class declares (`CStr`).
    pub boundary: Option<String>,
    /// The protocol name it belongs to (`C_String`).
    pub protocol: Option<String>,
    /// The base protocol it specialises (`String`).
    pub base: Option<String>,
}

impl ValueClass {
    /// Read one `values.<Name>: { … }` entry.
    fn from_entry(value: &DataValue) -> Self {
        let DataValue::Map(fields) = value else {
            return Self::default();
        };
        let text = |k: &str| match fields.get(k) {
            Some(DataValue::String(s)) => Some(s.clone()),
            _ => None,
        };
        Self {
            class: text("class").unwrap_or_else(|| "scalar".to_string()),
            zero_copy: matches!(fields.get("zero_copy"), Some(DataValue::Bool(true))),
            layout: text("layout"),
            len_op: text("len_op"),
            to_op: text("to_op"),
            from_op: text("from_op"),
            from_copy: matches!(fields.get("from_copy"), Some(DataValue::Bool(true))),
            retain_op: text("retain_op"),
            release_op: text("release_op"),
            kind: text("kind"),
            boundary: text("boundary"),
            protocol: text("protocol"),
            base: text("base"),
        }
    }

    /// Does this class produce a generated boundary type + protocol?
    ///
    /// Only a non-`scalar` class that names a boundary type, a protocol and both
    /// conversion operations can: the scalars are the base types themselves, and
    /// a class without ops has no lane to generate.
    pub fn renders(&self) -> bool {
        self.class != "scalar"
            && self.boundary.is_some()
            && self.protocol.is_some()
            && self.to_op.is_some()
            && self.from_op.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_to_language_via_targets() {
        let mut targets = HashMap::new();
        targets.insert(
            "python".to_string(),
            GlueTarget {
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
            },
        );
        targets.insert(
            "rust".to_string(),
            GlueTarget {
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
            },
        );

        // Should work with or without leading dot
        assert_eq!(
            find_language_by_extension(&targets, ".py")
                .unwrap()
                .language,
            "python"
        );
        assert_eq!(
            find_language_by_extension(&targets, "py").unwrap().language,
            "python"
        );
    }

    #[test]
    fn test_find_language_by_extension() {
        let mut targets = HashMap::new();
        targets.insert(
            "python".to_string(),
            GlueTarget {
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
            },
        );

        // Should work with or without leading dot
        assert_eq!(
            find_language_by_extension(&targets, ".py")
                .unwrap()
                .language,
            "python"
        );
        assert_eq!(
            find_language_by_extension(&targets, "py").unwrap().language,
            "python"
        );
    }

    #[test]
    fn mjs_extension_excludes_host_runtime_and_is_deterministic() {
        // 2026-10-07 (host-boundary decision record): node (esm_module) and
        // web (wasm_runtime) both claimed ".mjs". A host runtime is reached
        // by provenance, never by a source extension, so it must not win
        // extension routing — and the result must not depend on HashMap
        // iteration order.
        let mut targets = HashMap::new();
        for (name, bridge_kind) in [("web", "wasm_runtime"), ("node", "esm_module")] {
            targets.insert(
                name.to_string(),
                GlueTarget {
                    language: name.to_string(),
                    extension: "mjs".to_string(),
                    bridge_kind: bridge_kind.to_string(),
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            find_language_by_extension(&targets, ".mjs")
                .unwrap()
                .language,
            "node",
            "the host runtime (wasm_runtime) must not be reachable by extension"
        );
        assert_eq!(extension_to_language("mjs", &targets), Some("node"));
    }

    #[test]
    fn test_find_language_by_extension_with_dot() {
        let mut targets = HashMap::new();
        targets.insert(
            "python".to_string(),
            GlueTarget {
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
            },
        );

        // Should work with or without leading dot
        assert_eq!(
            find_language_by_extension(&targets, ".py")
                .unwrap()
                .language,
            "python"
        );
        assert_eq!(
            find_language_by_extension(&targets, "py").unwrap().language,
            "python"
        );
    }

    #[test]
    fn test_load_glue_config_custom_path() {
        let dir = std::env::temp_dir();
        let config_path = dir.join("test_glue_config.dbvl");
        let content = r##"python: { version: 1; types_module: "glue/python/types.bv"; extension: "py"; bridge_kind: "native_module"; calling_convention: "c_abi"; module_init: false; protocols: { "String": { native: "str"; c_abi: "ctypes.c_void_p" }; "Int": { native: "int"; c_abi: "ctypes.c_int64" } } };
python.templates.0: "fn_template" "def {{name}}({{params}}):\n    return {{name}};\n";
rust: { version: 1; types_module: "glue/rust/types.bv"; extension: "rs"; bridge_kind: "extern_c_crate"; calling_convention: "lto"; module_init: false };"##;
        std::fs::write(&config_path, content).unwrap();

        let result = load_glue_config(Some(&config_path));
        assert!(
            result.is_ok(),
            "load_glue_config failed: {:?}",
            result.err()
        );
        let targets = result.unwrap();

        assert!(targets.contains_key("python"));
        let py = &targets["python"];
        assert_eq!(py.language, "python");
        assert_eq!(py.extension, "py");
        assert_eq!(py.bridge_kind, "native_module");
        assert_eq!(py.calling_convention, "c_abi");
        assert!(py.protocols.contains_key("String"));
        assert_eq!(py.protocols.get("String").unwrap().native, "str");
        assert_eq!(
            py.protocols.get("String").unwrap().c_abi.as_deref(),
            Some("ctypes.c_void_p")
        );
        assert_eq!(
            py.templates.get("fn_template").unwrap(),
            "def {{name}}({{params}}):\n    return {{name}};\n"
        );

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
        assert!(
            result.is_ok(),
            "Default config/glue.dbv not found: {:?}",
            result.err()
        );
    }

    // ── 2026-11-16 (FFI contract plan D3): schema version + closed keys ─────

    /// Write one descriptor to a temp file and parse it.
    fn parse_one(name: &str, content: &str) -> Result<GlueTarget, String> {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, content).expect("write temp descriptor");
        let map = load_glue_config(Some(&path))?;
        map.into_values()
            .next()
            .ok_or_else(|| "no target parsed".to_string())
    }

    #[test]
    fn descriptor_without_version_is_refused() {
        // An unversioned descriptor cannot be read safely once the schema
        // carries semantics: a missing section would silently take defaults.
        let err = parse_one(
            "glue_no_version.dbvl",
            r#"c: { types_module: "t.bv"; extension: "h"; bridge_kind: "extern_c_crate"; calling_convention: "c_abi" };"#,
        )
        .expect_err("missing version must be refused");
        assert!(err.contains("has no `version`"), "got: {err}");
    }

    #[test]
    fn descriptor_with_a_future_version_is_refused() {
        let err = parse_one(
            "glue_future_version.dbvl",
            r#"c: { version: 99; types_module: "t.bv"; extension: "h"; bridge_kind: "extern_c_crate"; calling_convention: "c_abi" };"#,
        )
        .expect_err("future version must be refused");
        assert!(err.contains("declares version 99"), "got: {err}");
        assert!(
            err.contains(&format!("implements {}", GLUE_SCHEMA_VERSION)),
            "the error must name the supported version, got: {err}"
        );
    }

    #[test]
    fn unknown_key_is_refused_at_target_and_section_level() {
        // Target level — the classic typo.
        let err = parse_one(
            "glue_unknown_target.dbvl",
            r#"c: { version: 1; types_module: "t.bv"; extension: "h"; bridge_kind: "extern_c_crate"; calling_convention: "c_abi"; discpline: { blocking: false } };"#,
        )
        .expect_err("unknown target key must be refused");
        assert!(err.contains("unknown key `discpline`"), "got: {err}");

        // Section level — a typo inside `values` is just as dangerous.
        let err = parse_one(
            "glue_unknown_section.dbvl",
            r#"c: { version: 1; types_module: "t.bv"; extension: "h"; bridge_kind: "extern_c_crate"; calling_convention: "c_abi"; values: { String: { class: borrowed_bytes; zero_copy: true; zero_copied: true } } };"#,
        )
        .expect_err("unknown section key must be refused");
        assert!(err.contains("unknown key `zero_copied`"), "got: {err}");
        assert!(
            err.contains("values.String"),
            "the error must name the key path, got: {err}"
        );
    }

    #[test]
    fn unknown_abi_core_is_refused() {
        let err = parse_one(
            "glue_unknown_abi.dbvl",
            r#"c: { version: 1; abi: "sysv-x9999"; types_module: "t.bv"; extension: "h"; bridge_kind: "extern_c_crate"; calling_convention: "c_abi" };"#,
        )
        .expect_err("unknown ABI core must be refused");
        assert!(err.contains("names ABI core `sysv-x9999`"), "got: {err}");
        assert!(
            err.contains("sysv-x86_64"),
            "the error must list the known cores, got: {err}"
        );
    }

    /// The C adapter's boundary value class, in the flat schema form. Only
    /// `String` needs an entry: the scalar classes are already determined by the
    /// protocol plus the ABI core (S2), so naming them here would restate a fact
    /// the compiler derives — the thing the descriptor must never do. `String` is
    /// the case with genuine asymmetry: outbound is zero-copy (the `[len][bytes]`
    /// data region IS a C string), inbound must copy (a C string has no length
    /// prefix).
    #[test]
    fn the_c_adapter_declares_its_string_value_class() {
        let source = std::fs::read_to_string("lib/glue/c/glue.dbv").expect("read c adapter");
        let target = parse_glue_dbvl(&source)
            .expect("c adapter parses")
            .remove("c")
            .expect("c target");
        assert_eq!(target.abi.as_deref(), Some("sysv-x86_64"));
        assert!(
            source.contains("to_op: \"str_to_c\""),
            "the outbound C-string lane must name its zero-copy op"
        );
        assert!(
            source.contains("from_op: \"cstr_to_briev\"; from_copy: true"),
            "the inbound C-string lane must be declared as a copy"
        );
    }

    /// Every shipped adapter that names a `types_module` must name a file that
    /// exists. `glue/c/types.bv` did not: the descriptor pointed at nothing, and
    /// nothing read the path, so the claim was never checked. A dangling path in
    /// data is a lie the compiler tells itself — this pins it shut.
    #[test]
    fn every_named_types_module_exists() {
        let mut checked = 0;
        for entry in std::fs::read_dir("lib/glue").expect("lib/glue") {
            let dir = entry.expect("dir entry").path();
            let cfg = dir.join("glue.dbv");
            if !cfg.exists() {
                continue;
            }
            let lang = dir.file_name().and_then(|s| s.to_str()).unwrap_or("?");
            let source = std::fs::read_to_string(&cfg).expect("read descriptor");
            let Some(target) = parse_glue_dbvl(&source)
                .expect("descriptor parses")
                .remove(lang)
            else {
                continue;
            };
            let path = target.types_module.to_string_lossy().to_string();
            if path.is_empty() {
                continue;
            }
            let resolved = std::path::Path::new("lib").join(&path);
            assert!(
                resolved.exists(),
                "adapter `{lang}` names types_module `{path}` but {resolved:?} does not exist"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "expected at least one adapter with a types_module"
        );
    }

    #[test]
    fn the_c_adapter_declares_its_abi_core() {
        // Only where `c_abi` values are genuine C types; the managed-language
        // adapters name host-FFI types, not a machine ABI.
        let core = load_glue_language("c").expect("c adapter loads");
        assert_eq!(core.abi.as_deref(), Some("sysv-x86_64"));
        let loaded = crate::glue::abi::load_core("sysv-x86_64").expect("core loads");
        assert_eq!(loaded.aggregate_small_max, 16);
    }

    #[test]
    fn boundary_sections_parse_when_well_formed() {
        // The additive sections are declared but not yet consumed (S1/S3); they
        // must parse now so a descriptor can state the contract before its
        // consumers land.
        let target = parse_one(
            "glue_boundary_sections.dbvl",
            r#"c: { version: 1; types_module: "t.bv"; extension: "h"; bridge_kind: "extern_c_crate"; calling_convention: "c_abi"; values: { "String": { class: "borrowed_bytes"; zero_copy: true; to_op: "str_to_c"; from_op: "cstr_to_briev"; from_copy: true } }; frees: { owned_buffer: "caller" }; discipline: { blocking: false; reentrant: false; needs_main_thread: false }; errors: { class: "status"; success: 0 } };"#,
        )
        .expect("well-formed boundary sections must parse");
        assert_eq!(target.version, 1);
    }

    /// Baked config golden: the compiler-shipped config/glue.dbv loads with
    /// all four languages and the expected protocol/template shape. Replaces
    /// the TOML parity gate after lib/glue.toml was deleted.
    #[test]
    fn baked_glue_dbvl_shape() {
        let config = load_glue_config(None).expect("config/glue.dbv should load");
        // 2026-10-07 (host-boundary decision record): `web` is no longer a
        // GLUE language target — the browser is a host-module path
        // (`lib/glue/web/web.js`), not a registry entry.
        for lang in ["python", "rust", "node"] {
            assert!(config.contains_key(lang), "missing '{}' target", lang);
        }
        assert!(!config.contains_key("web"), "web must not be a GLUE target");
        let python = &config["python"];
        assert_eq!(python.calling_convention, "c_abi");
        assert!(python.module_init);
        assert_eq!(
            // 2026-09-11 (Phase A6): protocol keys are the BARE category.
            python.protocols.get("String").unwrap().c_abi.as_deref(),
            Some("ctypes.c_void_p")
        );
        assert!(
            python.templates.contains_key("__init__.py"),
            "python __init__.py template"
        );
        assert!(
            python.templates.contains_key("fn_template"),
            "python fn_template template"
        );
        let rust = &config["rust"];
        assert_eq!(rust.calling_convention, "lto");
        assert!(
            rust.templates.contains_key("src/lib.rs"),
            "rust src/lib.rs template"
        );
        let node = &config["node"];
        assert!(
            node.templates.contains_key("index.mjs"),
            "node index.mjs template"
        );
    }
}
