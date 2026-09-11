// ── Target Config — Backend Selection ─────────────────────────────────
// 2026-07-14: Reads config/targets.dbvl at compile time.
// Maps file extension → (backend, default CLI flags).
// --backend flag overrides the config.

use std::collections::HashMap;

/// Backend kinds that the compiler can dispatch to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Llvm,
    Circt,
    Webstack,
    Gpu,
    Spirv,
    Vm,
    Ptx,
}

/// One entry from config/targets.dbvl.
///
/// 2026-07-31: `backend`/`defaults` are optional so the `[target.<prefix>]`
/// tuning tables (plan §8.1) coexist in the same file without failing the
/// flatten parse — extension entries always set them; target-tuning entries
/// do not (they carry float_registers/dense_compute_density/vector_min_width,
/// read by config_tuning.rs, which serde ignores here).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TargetEntry {
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub defaults: Vec<String>,
    /// System plugins enabled for this extension. None = default set.
    pub plugins: Option<Vec<String>>,
    /// Override LLVM target triple (e.g. "wasm32-unknown-wasi").
    /// 2026-07-15: Phase 7 — optional, defaults to x86_64-unknown-linux-gnu.
    pub target_triple: Option<String>,
    /// Override LLVM data layout string.
    /// 2026-07-15: Phase 7 — optional, auto-derived from target_triple if not set.
    pub data_layout: Option<String>,
    /// 2026-07-29: Assembler backend for inline assembly validation.
    /// "keystone" — Keystone Engine (default, requires libkeystone).
    /// "platform" — system assembler (as / ml64).
    /// "none" — no validation, warn at compile time.
    #[serde(default = "default_assembler")]
    pub assembler: String,
    /// 2026-07-29: Number of random samples for cross-verification
    /// in the := verification chain. Default 50.
    #[serde(default = "default_cross_verify_samples")]
    pub cross_verify_samples: u32,
}

fn default_assembler() -> String { "none".to_string() }
fn default_cross_verify_samples() -> u32 { 50 }

/// Split a whitespace-separated list field into words (parser has no array
/// grammar — space-separated values round-trip as one String field).
/// 2026-08-03 (Phase 3, data-briev-config plan).
fn split_words(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| w.to_string()).collect()
}

/// Loaded config/targets.dbvl.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TargetConfig {
    #[serde(flatten)]
    entries: HashMap<String, TargetEntry>,
}

// ── Protocol Map ─────────────────────────────────────────────────────────
// 2026-07-26: Phase 1 — Protocol-to-library resolution for from #System etc.
// Maps a target triple → { protocol_name → library_or_none }.
// None means the protocol is unavailable on that target.

/// Loaded config/protocols.dbvl.
///
/// 2026-07-26: Maps protocol names to linker library names per target triple.
/// `#System` abstracts "the platform's standard system library" (libc on Linux,
/// libSystem on macOS, WASI preview1 on wasm). `#Web` routes through the GLUE
/// wasm_runtime bridge (WASM targets only, no linker flag needed).
/// Any other protocol hashword produces a compile error.
/// Loaded by ProtocolConfig::load() and consulted during frgn dispatch resolution.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProtocolConfig {
    /// Key = target triple (e.g. "x86_64-linux"),
    /// Value = { protocol_name → library_name_or_none }.
    #[serde(flatten)]
    per_target: HashMap<String, HashMap<String, Option<String>>>,
}

impl ProtocolConfig {
    /// Load the compiled-in protocol config (baked at compile time).
    ///
    /// 2026-08-03 (Phase 3, data-briev-config plan): reads config/protocols.dbvl
    /// (quoted mode — the `#System`/`#Web` map keys are quoted because `#` is
    /// not an identifier char). Shape is `<triple>: { "<protocol>": "<lib>"; }`.
    pub fn load() -> Self {
        let content = include_str!("../config/protocols.dbvl");
        let db = crate::dbriev::config_db::ConfigDb::from_quoted_str(content)
            .unwrap_or_else(|e| panic!("config/protocols.dbvl parse error: {}", e));
        let mut per_target = HashMap::new();
        for key in db.keys() {
            let map = match db.field(&key, 0) {
                Some(crate::dbriev::v2::DataValue::Map(entries)) => entries
                    .iter()
                    .map(|(protocol, lib)| {
                        let lib = match lib {
                            crate::dbriev::v2::DataValue::String(s) => Some(s.clone()),
                            _ => None,
                        };
                        (protocol.clone(), lib)
                    })
                    .collect(),
                _ => HashMap::new(),
            };
            per_target.insert(key, map);
        }
        ProtocolConfig { per_target }
    }

    /// Resolve a protocol name to a library name for the given target.
    ///
    /// `#System` and `#Web` are the two recognized protocols.
    /// `#System` links against the platform's system library (libc, WASI).
    /// `#Web` routes through the GLUE wasm_runtime bridge (valid on WASM targets only).
    ///
    /// Returns:
    /// - `Ok(Some(lib))` — protocol maps to library `lib`, link with `-l<lib>`.
    /// - `Ok(None)` — protocol is available but needs no extra linker flag
    ///   (e.g., libc is linked by default with clang).
    /// - `Err(msg)` — protocol is unrecognized or unavailable on target.
    pub fn resolve(&self, target_triple: &str, protocol: &str) -> Result<Option<&str>, String> {
        if protocol != "#System" && protocol != "#Web" {
            return Err(format!(
                "'{}' is not a valid protocol hashword. \
                 #System and #Web are the supported protocols",
                protocol
            ));
        }
        let target_map = self.per_target.get(target_triple).ok_or_else(|| {
            format!(
                "target '{}' not found in config/protocols.dbvl. \
                 Add an entry for this target to configure protocol support",
                target_triple
            )
        })?;
        match target_map.get(protocol) {
            Some(Some(lib)) => Ok(Some(lib.as_str())),
            Some(None) => Ok(None),
            None => Err(format!(
                "target '{}' has no '{}' entry in config/protocols.dbvl",
                target_triple, protocol
            )),
        }
    }

    /// Check if a protocol is available on a given target.
    pub fn is_available(&self, target_triple: &str, protocol: &str) -> bool {
        self.per_target
            .get(target_triple)
            .and_then(|m| m.get(protocol))
            .map(|v| v.is_some())
            .unwrap_or(false)
    }
}

// ── ISR Mechanism Registry ──────────────────────────────────────────────
// 2026-09-06 (plan 2026-09-06-isr-handlers-and-sections.md): the target
// knowledge behind `isr[<mechanism>] handler @ vector: ...` — vector table
// layout + calling convention, per config/isr-targets.dbvl. The mechanism
// resolves explicit-in-`<>` → target profile → compile error (what/why/fix);
// the compiler never invents a layout ("inventing hardware layout is silent
// wrongness" — the asm<target> dead-data gap must not repeat).

/// One row of config/isr-targets.dbvl.
#[derive(Debug, Clone, PartialEq)]
pub struct IsrMechanism {
    /// Bytes per vector table slot.
    pub entry_stride: u64,
    /// Slot 0 is the initial stack pointer (ARM Cortex-M), not a handler.
    pub sp_slot: bool,
    /// OR 1 into handler addresses (ARM Thumb state bit).
    pub thumb_bit: bool,
    /// The epilogue return instruction (validation anchor + emission ref).
    pub return_insn: String,
    /// How the mechanism stacks FP context.
    pub fpu_context: IsrFpuContext,
    /// Stack frame bound in bytes; a body frame above it is a compile error.
    pub max_frame: u64,
    /// Name for undeclared vectors (emitted as a spin loop when the board
    /// file does not provide one).
    pub default_handler: String,
    /// ELF section for the emitted vector table (empty = no section
    /// attribute — x86 IDT / RISC-V mtvec tables are runtime-built).
    pub table_section: String,
    /// The wrapper's calling convention (config row field 8).
    pub convention: IsrConv,
}

/// The calling convention the emitted ISR wrapper carries (config row
/// field 8 — the backend consumes the mechanism's decision, never
/// name-matches it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsrConv {
    /// LLVM "interrupt"="IRQ" function attribute (ARM targets).
    ArmIrq,
    /// LLVM "interrupt"="machine" attribute (RISC-V targets).
    RiscvInterrupt,
    /// x86_intrcc calling convention.
    X86Intr,
}

/// How the mechanism handles floating-point context in ISRs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsrFpuContext {
    /// Float in an ISR body is a compile error.
    None,
    /// FP context stacked lazily (FPCCR ASPEN/LSPEN on Cortex-M4F).
    Lazy,
    /// FP context saved/restored on every entry.
    Eager,
}

/// Loaded config/isr-targets.dbvl.
#[derive(Debug, Clone)]
pub struct IsrMechanismConfig {
    mechanisms: HashMap<String, IsrMechanism>,
}

impl IsrMechanismConfig {
    /// Load the compiled-in ISR mechanism registry (baked at compile time).
    pub fn load() -> Self {
        let content = include_str!("../config/isr-targets.dbvl");
        let db = crate::dbriev::config_db::ConfigDb::from_str(content)
            .unwrap_or_else(|e| panic!("config/isr-targets.dbvl parse error: {}", e));
        let mut mechanisms = HashMap::new();
        for key in db.keys() {
            let entry_stride = db.field_int(&key, 0).unwrap_or(4) as u64;
            let sp_slot = db.field_string(&key, 1) == Some("sp");
            let thumb_bit = db.field_int(&key, 2).map(|v| v != 0).unwrap_or(false);
            let return_insn = db
                .field_string(&key, 3)
                .unwrap_or("bx lr")
                .to_string();
            let fpu_context = match db.field_string(&key, 4) {
                Some("lazy") => IsrFpuContext::Lazy,
                Some("eager") => IsrFpuContext::Eager,
                _ => IsrFpuContext::None,
            };
            let max_frame = db.field_int(&key, 5).unwrap_or(512) as u64;
            let default_handler = db
                .field_string(&key, 6)
                .unwrap_or("Default_Handler")
                .to_string();
            let table_section = db
                .field_string(&key, 7)
                .unwrap_or("")
                .to_string();
            let convention = match db.field_string(&key, 8) {
                Some("riscv_machine") => IsrConv::RiscvInterrupt,
                Some("x86_intr") => IsrConv::X86Intr,
                _ => IsrConv::ArmIrq,
            };
            mechanisms.insert(
                key.clone(),
                IsrMechanism {
                    entry_stride,
                    sp_slot,
                    thumb_bit,
                    return_insn,
                    fpu_context,
                    max_frame,
                    default_handler,
                    table_section,
                    convention,
                },
            );
        }
        IsrMechanismConfig { mechanisms }
    }

    /// Registry lookup — None means the mechanism name is not a row of
    /// config/isr-targets.dbvl (a typo'd `isr<arm_cortexm>` must fail at
    /// the typecheck, never silently).
    pub fn get(&self, mechanism: &str) -> Option<&IsrMechanism> {
        self.mechanisms.get(mechanism)
    }

    /// The known mechanism names, sorted — for error messages listing the
    /// valid choices.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.mechanisms.keys().cloned().collect();
        names.sort();
        names
    }
}

impl TargetConfig {
    /// Load the compiled-in target config (fallback).
    ///
    /// 2026-08-03 (Phase 3, data-briev-config plan): reads config/targets.dbvl.
    /// Extension entries are `<.ext>: <backend>; <defaults space-sep>;
    /// <plugins space-sep>; [assembler]; [cross_verify_samples];
    /// [target_triple]; [data_layout];` — the two overrides are optional and
    /// never set in the shipped file, so they sit AFTER the common fields.
    /// The `target.*` tuning rows are consumed by config_tuning, not here.
    pub fn load() -> Self {
        let content = include_str!("../config/targets.dbvl");
        let db = crate::dbriev::config_db::ConfigDb::from_str(content)
            .unwrap_or_else(|e| panic!("config/targets.dbvl parse error: {}", e));
        let mut entries = HashMap::new();
        for key in db.keys() {
            if key.starts_with("target.") {
                continue; // tuning rows — config_tuning's table
            }
            let entry = TargetEntry {
                backend: db.field_string(&key, 0).map(|s| s.to_string()),
                defaults: db.field_string(&key, 1).map(split_words).unwrap_or_default(),
                plugins: db.field_string(&key, 2).map(split_words),
                assembler: db.field_string(&key, 3).map(|s| s.to_string()).unwrap_or_else(default_assembler),
                cross_verify_samples: db.field_int(&key, 4).map(|v| v as u32)
                    .unwrap_or_else(default_cross_verify_samples),
                target_triple: db.field_string(&key, 5).map(|s| s.to_string()),
                data_layout: db.field_string(&key, 6).map(|s| s.to_string()),
            };
            entries.insert(key, entry);
        }
        TargetConfig { entries }
    }

    /// 2026-07-16: P1 — load from a concrete file path (TOML or .dbvl).
    pub fn load_from(path: &std::path::Path) -> Result<Self, String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "dbvl" || ext == "dbv" {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
            let db = crate::dbriev::config_db::ConfigDb::from_str(&content)
                .map_err(|e| format!("parse error in '{}': {}", path.display(), e))?;
            let mut entries = HashMap::new();
            for key in db.keys() {
                if key.starts_with("target.") {
                    continue;
                }
                let entry = TargetEntry {
                    backend: db.field_string(&key, 0).map(|s| s.to_string()),
                    defaults: db.field_string(&key, 1).map(split_words).unwrap_or_default(),
                    plugins: db.field_string(&key, 2).map(split_words),
                    assembler: db.field_string(&key, 3).map(|s| s.to_string()).unwrap_or_else(default_assembler),
                    cross_verify_samples: db.field_int(&key, 4).map(|v| v as u32)
                        .unwrap_or_else(default_cross_verify_samples),
                    target_triple: db.field_string(&key, 5).map(|s| s.to_string()),
                    data_layout: db.field_string(&key, 6).map(|s| s.to_string()),
                };
                entries.insert(key, entry);
            }
            return Ok(TargetConfig { entries });
        }
        // Legacy TOML path (pre-migration profiles).
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
        toml::from_str(&content)
            .map_err(|e| format!("parse error in '{}': {}", path.display(), e))
    }

    /// Look up a target entry by file extension (e.g. ".bv").
    pub fn lookup(&self, extension: &str) -> Option<&TargetEntry> {
        let key = if extension.starts_with('.') { extension.to_string() } else { format!(".{}", extension) };
        self.entries.get(&key)
    }

    /// Resolve a backend name string to a BackendKind.
    pub fn resolve(name: &str) -> Result<BackendKind, String> {
        match name {
            "llvm" => Ok(BackendKind::Llvm),
            "circt" => Ok(BackendKind::Circt),
            "webstack" => Ok(BackendKind::Webstack),
            "gpu" => Ok(BackendKind::Gpu),
            "spirv" => Ok(BackendKind::Spirv),
            "vm" => Ok(BackendKind::Vm),
            "ptx" => Ok(BackendKind::Ptx),
            _ => Err(format!("unknown backend '{}'. Supported: llvm, circt, webstack, vm, spirv, ptx", name)),
        }
    }
}

/// Get the file extension from a path, including the dot.
pub fn get_extension(file_path: &str) -> String {
    let p = std::path::Path::new(file_path);
    match p.extension().and_then(|s| s.to_str()) {
        Some(ext) => format!(".{}", ext),
        None => ".bv".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-migration config/targets.toml, frozen as the golden reference for
    /// parity_targets_dbvl_matches_toml. 2026-08-03: the .toml file is deleted;
    /// edits to config/targets.dbvl must keep this test green.
    const TARGETS_TOML_GOLDEN: &str = r#"
# Extension → backend routing.

[".bv"]
backend = "llvm"
defaults = ["--budget", "256"]
plugins = ["prelude-native", "env", "print", "inline-frgn", "entry", "script"]
assembler = "none"
cross_verify_samples = 50

[".cbv"]
backend = "circt"
defaults = []
plugins = ["prelude-hw"]

[".rbv"]
backend = "webstack"
defaults = ["--target", "wasm"]
plugins = ["prelude"]

[".abv"]
backend = "spirv"
defaults = []
plugins = ["prelude"]

[target.x86_64]
float_registers = 16
dense_compute_density = 4.0
vector_min_width = 4

[target.aarch64]
float_registers = 32
dense_compute_density = 4.0
vector_min_width = 4

[target.arm64]
float_registers = 32
dense_compute_density = 4.0
vector_min_width = 4

[target.wasm32]
float_registers = 4294967295
dense_compute_density = 4.0
vector_min_width = 4

[target.wasm64]
float_registers = 4294967295
dense_compute_density = 4.0
vector_min_width = 4

[target.spirv64]
float_registers = 32
dense_compute_density = 4.0
vector_min_width = 4
"#;

    /// Pre-migration config/protocols.toml, frozen as the golden reference for
    /// parity_protocols_dbvl_matches_toml. 2026-08-03: the .toml file is
    /// deleted; edits to config/protocols.dbvl must keep this test green.
    /// Uses `r##"..."##` because the TOML values contain `"#System` (`"#`
    /// would close a `r#"` raw string).
    const PROTOCOLS_TOML_GOLDEN: &str = r##"
[x86_64-linux]
"#System" = "c"

[aarch64-linux]
"#System" = "c"

[x86_64-macos]
"#System" = "System"

[aarch64-macos]
"#System" = "System"

[wasm32-wasi]
"#System" = "wasi_snapshot_preview1"
"#Web" = "wasm_runtime"
"##;

    #[test]
    fn test_target_config_loads() {
        let config = TargetConfig::load();
        assert!(config.lookup(".bv").is_some(), "should have .bv entry");
    }

    #[test]
    fn test_target_config_has_extensions() {
        let config = TargetConfig::load();
        for ext in &[".bv", ".cbv", ".rbv", ".abv"] {
            assert!(config.lookup(ext).is_some(), "missing entry for {}", ext);
        }
    }

    #[test]
    fn test_resolve_backend() {
        assert_eq!(TargetConfig::resolve("llvm").unwrap(), BackendKind::Llvm);
        assert_eq!(TargetConfig::resolve("circt").unwrap(), BackendKind::Circt);
        assert_eq!(TargetConfig::resolve("webstack").unwrap(), BackendKind::Webstack);
        assert_eq!(TargetConfig::resolve("spirv").unwrap(), BackendKind::Spirv);
        assert!(TargetConfig::resolve("unknown").is_err());
    }

    #[test]
    fn test_get_extension() {
        assert_eq!(get_extension("foo.bv"), ".bv");
        assert_eq!(get_extension("foo.cbv"), ".cbv");
        assert_eq!(get_extension("foo"), ".bv");
    }

    #[test]
    fn test_protocol_config_loads() {
        let config = ProtocolConfig::load();
        assert!(config.is_available("x86_64-linux", "#System"),
            "x86_64-linux should support #System");
        assert!(!config.is_available("x86_64-linux", "#NonExistent"),
            "non-existent protocol should be unavailable");
    }

    #[test]
    fn test_protocol_config_resolve_system() {
        let config = ProtocolConfig::load();
        let lib = config.resolve("x86_64-linux", "#System").unwrap();
        assert_eq!(lib, Some("c"), "#System should resolve to 'c' on linux");
    }

    #[test]
    fn test_protocol_config_resolve_system_wasi() {
        let config = ProtocolConfig::load();
        let lib = config.resolve("wasm32-wasi", "#System").unwrap();
        assert_eq!(lib, Some("wasi_snapshot_preview1"),
            "#System on wasm32-wasi should resolve to wasi_snapshot_preview1");
    }

    #[test]
    fn test_protocol_config_resolve_unknown_protocol() {
        let config = ProtocolConfig::load();
        let err = config.resolve("x86_64-linux", "#SomethingElse")
            .unwrap_err();
        assert!(err.contains("supported protocols"),
            "error should mention supported protocols (got: '{}')", err);
    }

    #[test]
    fn test_protocol_config_resolve_unknown_target() {
        let config = ProtocolConfig::load();
        let result = config.resolve("nonexistent-target", "#System");
        assert!(result.is_err(), "unknown target should error");
    }

    #[test]
    fn parity_targets_dbvl_matches_toml() {
        // Phase 3 migration gate: config/targets.dbvl must produce exactly the
        // extension→TargetEntry map the targets.toml it replaced produced. The
        // `target.*` tuning rows are skipped here — they feed config_tuning,
        // whose own parity test covers them. 2026-08-03: the .toml is deleted;
        // this is now a GOLDEN test — the pre-migration TOML is baked below and
        // re-parsed, so the exact-value comparison stays without the file.
        let db = TargetConfig::load();
        let toml_entries: HashMap<String, TargetEntry> =
            toml::from_str(TARGETS_TOML_GOLDEN).unwrap();

        let toml_exts: Vec<String> = toml_entries
            .keys()
            // The `[target.<prefix>]` tuning tables flatten to a single `target`
            // key (nested table), not `target.*` — exclude it.
            .filter(|k| !k.starts_with("target.") && *k != "target")
            .cloned()
            .collect();
        assert!(!toml_exts.is_empty(), "targets.toml should have extension entries");

        assert_eq!(db.entries.len(), toml_exts.len(),
            "extension-entry count diverges between .dbvl and .toml");
        for ext in &toml_exts {
            let db_entry = db.entries.get(ext)
                .unwrap_or_else(|| panic!("extension '{}' missing from targets.dbvl", ext));
            let toml_entry = &toml_entries[ext];
            assert_eq!(db_entry.backend, toml_entry.backend,
                "backend for '{}' diverges", ext);
            assert_eq!(db_entry.defaults, toml_entry.defaults,
                "defaults for '{}' diverge", ext);
            assert_eq!(db_entry.plugins, toml_entry.plugins,
                "plugins for '{}' diverge", ext);
            assert_eq!(db_entry.assembler, toml_entry.assembler,
                "assembler for '{}' diverges", ext);
            assert_eq!(db_entry.cross_verify_samples, toml_entry.cross_verify_samples,
                "cross_verify_samples for '{}' diverges", ext);
        }
    }

    #[test]
    fn parity_protocols_dbvl_matches_toml() {
        // Phase 3 migration gate: config/protocols.dbvl must produce exactly
        // the target→protocol→library map the protocols.toml it replaced
        // produced. 2026-08-03: the .toml is deleted; this is now a GOLDEN
        // test — the pre-migration TOML is baked below and re-parsed.
        let db = ProtocolConfig::load();
        let toml_map: HashMap<String, HashMap<String, Option<String>>> =
            toml::from_str(PROTOCOLS_TOML_GOLDEN).unwrap();

        assert_eq!(db.per_target.len(), toml_map.len());
        // Flatten to (triple, protocol, lib) and iterate once (avoids nested
        // loops for the Praetor complexity gate).
        let flat: Vec<(String, String, Option<String>)> = toml_map
            .iter()
            .flat_map(|(triple, protos)| {
                protos
                    .iter()
                    .map(move |(protocol, lib)| (triple.clone(), protocol.clone(), lib.clone()))
            })
            .collect();
        for (triple, protocol, lib) in flat {
            let db_map = db.per_target.get(&triple)
                .unwrap_or_else(|| panic!("target '{}' missing from protocols.dbvl", triple));
            assert_eq!(
                db_map.get(&protocol),
                Some(&lib),
                "protocol '{}' on '{}' diverges between .dbvl and .toml",
                protocol, triple
            );
        }
    }
}
