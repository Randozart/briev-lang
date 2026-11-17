//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! ABI cores — the only place a width, alignment or aggregate-passing rule is
//! written down.
//!
//! 2026-11-16 (FFI contract plan S2): the FFI surface splits into a stable axis
//! (how the machine passes arguments) and a volatile one (how a language
//! represents its objects). This module owns the stable axis: one descriptor per
//! target family under `config/abi/`, named by a language adapter with
//! `abi: "<name>"`.
//!
//! Facts only. A core may never state a preference, a cost, or a strategy — a
//! fast path is derived (representations coincide *and* the prover licenses it),
//! never declared. That rule is what keeps "add a language" a data change.

use crate::dbriev::config_db::ConfigDb;
use crate::dbriev::v2::DataValue;
use std::path::{Path, PathBuf};

/// One target family's argument-passing facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiCore {
    /// The core's name, as a descriptor refers to it (`"sysv-x86_64"`).
    pub name: String,
    pub pointer_bits: u32,
    pub int_bits: u32,
    pub float_bits: u32,
    /// Stack alignment in bytes at a call boundary.
    pub stack_align: u32,
    /// Aggregate passing: largest size still passed in registers, the
    /// classification used, and the size above which a hidden pointer is used.
    pub aggregate_small_max: u32,
    pub aggregate_in_regs: String,
    pub aggregate_hidden_ptr_threshold: u32,
    /// Symbol decoration applied by the toolchain ("" = none).
    pub symbol_prefix: String,
    pub symbol_mangling: String,
    /// Whether the caller releases memory the callee returned.
    pub temporaries_caller_frees: bool,
}

/// Keys a core entry may carry. Closed, like the language descriptors: an
/// unrecognised key would silently change what the compiler believes about the
/// machine.
const KNOWN_KEYS: &[&str] = &[
    "name",
    "pointer_bits",
    "int_bits",
    "float_bits",
    "stack_align",
    "aggregate_small_max",
    "aggregate_in_regs",
    "aggregate_hidden_ptr_threshold",
    "symbol_prefix",
    "symbol_mangling",
    "temporaries_caller_frees",
];

/// Directory holding the cores. Resolved like `lib/glue`: relative to the
/// project root that owns the compiler build.
///
/// 2026-11-16 (found building the kiosk daemon from its own repository): the
/// cores are COMPILER-SHIPPED data, so resolution must not depend on where the
/// caller happens to stand. The old version only walked up from the current
/// directory, so `brievc build` from any tree outside the compiler repo failed
/// with "Known cores: <none found>" even though the data sits beside the
/// binary. Order is now: explicit `BRIEV_HOME` (the documented install root),
/// then the tree the binary itself lives in, then the caller's cwd (a
/// development build run from the repo root).
fn abi_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("BRIEV_HOME") {
        let candidate = PathBuf::from(dir).join("config").join("abi");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = walk_up_for_abi_dir(exe.parent()?) {
            return Some(dir);
        }
    }
    walk_up_for_abi_dir(&std::env::current_dir().ok()?)
}

/// Nearest `config/abi/` at or above `start`.
fn walk_up_for_abi_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("config").join("abi");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Load one core by name. The name is also the file stem, so a typo is a refusal
/// rather than a silent default.
pub fn load_core(name: &str) -> Result<AbiCore, String> {
    let dir = abi_dir().ok_or_else(|| {
        "config/abi/ not found — ABI cores are compiler-shipped data; run from the \
         repository root or set the project layout"
            .to_string()
    })?;
    let path = dir.join(format!("{name}.dbvl"));
    load_core_from(&path)
}

/// Load and validate one core from an explicit path (tests and future
/// per-project overrides).
pub fn load_core_from(path: &Path) -> Result<AbiCore, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read ABI core '{}': {e}", path.display()))?;
    parse_core(&source, &path.display().to_string())
}

/// Names of every shipped core, sorted. Used by diagnostics that must list the
/// alternatives ("known cores: aapcs64, ptx, riscv64, …").
pub fn core_names() -> Vec<String> {
    let Some(dir) = abi_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("dbvl"))
                .then(|| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

/// Parse and validate one core document.
///
/// The core uses the flat `.dbvl` line-table form (`key: value;` at top level,
/// the form `config/ir-lowering.dbvl` uses) because it is the one the config DB
/// reads as typed integers; nested quoted-mode maps only carry strings.
pub fn parse_core(source: &str, origin: &str) -> Result<AbiCore, String> {
    let db = ConfigDb::from_quoted_str(source)?;
    for key in db.keys() {
        reject_unknown(origin, "core", &key, KNOWN_KEYS)?;
    }
    let name = db
        .field_string("name", 0)
        .ok_or_else(|| format!("ABI core '{origin}' has no `name`"))?
        .to_string();
    Ok(AbiCore {
        name,
        pointer_bits: int_of(&db, origin, "pointer_bits")?,
        int_bits: int_of(&db, origin, "int_bits")?,
        float_bits: int_of(&db, origin, "float_bits")?,
        stack_align: int_of(&db, origin, "stack_align")?,
        aggregate_small_max: int_of(&db, origin, "aggregate_small_max")?,
        aggregate_in_regs: db
            .field_string("aggregate_in_regs", 0)
            .unwrap_or("none")
            .to_string(),
        aggregate_hidden_ptr_threshold: int_of(&db, origin, "aggregate_hidden_ptr_threshold")?,
        symbol_prefix: db
            .field_string("symbol_prefix", 0)
            .unwrap_or("")
            .to_string(),
        symbol_mangling: db
            .field_string("symbol_mangling", 0)
            .unwrap_or("none")
            .to_string(),
        temporaries_caller_frees: matches!(
            db.field("temporaries_caller_frees", 0),
            Some(DataValue::Bool(true))
        ),
    })
}

/// Refuse an unrecognised key, naming the section and the acceptable set.
fn reject_unknown(origin: &str, section: &str, key: &str, known: &[&str]) -> Result<(), String> {
    if known.contains(&key) {
        return Ok(());
    }
    let mut sorted: Vec<&str> = known.to_vec();
    sorted.sort_unstable();
    Err(format!(
        "unknown key `{key}` in ABI core '{origin}' at `{section}` — known keys: {}. \
         ABI facts are closed: a key the compiler does not understand would leave \
         it believing something about the machine that the data does not say.",
        sorted.join(", ")
    ))
}

/// A required integer field, refused by name when missing or out of range.
fn int_of(db: &ConfigDb, origin: &str, field: &str) -> Result<u32, String> {
    match db.field(field, 0) {
        Some(DataValue::Int(n)) if *n >= 0 && *n <= u32::MAX as i64 => Ok(*n as u32),
        _ => Err(format!(
            "ABI core '{origin}' needs an integer `{field}` (0..={})",
            u32::MAX
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped core parses and names itself after its file.
    #[test]
    fn shipped_cores_parse() {
        let names = core_names();
        assert!(
            names.len() >= 6,
            "expected the six shipped cores, found {names:?}"
        );
        for name in names {
            let core = load_core(&name).unwrap_or_else(|e| panic!("core '{name}': {e}"));
            assert_eq!(core.name, name, "a core's `name` must match its file stem");
            assert!(core.pointer_bits == 32 || core.pointer_bits == 64);
            assert!(core.stack_align > 0);
        }
    }

    /// The two facts the generator will lean on: wasm has no aggregate register
    /// passing, and SysV passes <= 16 bytes in registers.
    #[test]
    fn aggregate_rules_differ_per_family() {
        let wasm = load_core("wasm32").expect("wasm32 core");
        assert_eq!(wasm.aggregate_small_max, 0);
        assert_eq!(wasm.aggregate_in_regs, "none");
        assert_eq!(wasm.pointer_bits, 32);
        let sysv = load_core("sysv-x86_64").expect("sysv core");
        assert_eq!(sysv.aggregate_small_max, 16);
        assert_eq!(sysv.aggregate_in_regs, "eightbyte");
        assert!(sysv.temporaries_caller_frees);
    }

    /// An unknown key is refused, and the message names the section.
    #[test]
    fn unknown_key_is_refused() {
        let err = parse_core(
            "name: \"x\";\npointer_bits: 64;\nint_bits: 64;\nfloat_bits: 64;\nstack_align: 16;\naggregate_small_max: 16;\naggregate_in_regs: \"eightbyte\";\naggregate_hidden_ptr_threshold: 16;\naggregate_extra: 1;\nsymbol_prefix: \"\";\nsymbol_mangling: \"none\";\ntemporaries_caller_frees: true;\n",
            "test",
        )
        .expect_err("unknown key must be refused");
        assert!(err.contains("aggregate_extra"), "got: {err}");
        assert!(err.contains("unknown key"), "got: {err}");
    }

    /// A missing integer field is refused with its path, not defaulted.
    #[test]
    fn missing_field_is_refused() {
        let err = parse_core(
            "name: \"x\";\nint_bits: 64;\nfloat_bits: 64;\nstack_align: 16;\naggregate_small_max: 0;\naggregate_in_regs: \"none\";\naggregate_hidden_ptr_threshold: 0;\nsymbol_prefix: \"\";\nsymbol_mangling: \"none\";\ntemporaries_caller_frees: true;\n",
            "test",
        )
        .expect_err("missing pointer_bits must be refused");
        assert!(err.contains("pointer_bits"), "got: {err}");
    }

    /// The cores are compiler-shipped data, so they must resolve from where the
    /// BINARY lives, not only from where the caller happens to stand. Found
    /// 2026-11-16: building the kiosk daemon from its own repository reported
    /// "Known cores: <none found>" while the data sat beside the compiler.
    #[test]
    fn abi_dir_resolves_from_the_binary_tree() {
        let root = std::env::temp_dir().join(format!("briev-abi-dir-{}", std::process::id()));
        let abi = root.join("config").join("abi");
        std::fs::create_dir_all(&abi).expect("create synthetic tree");
        std::fs::write(abi.join("x.dbvl"), "name: \"x\";").expect("write core");
        let deep_bin = root.join("target").join("release");
        std::fs::create_dir_all(&deep_bin).expect("create bin dir");
        assert_eq!(walk_up_for_abi_dir(&deep_bin), Some(abi.clone()));
        // A nested directory deeper than the tree still resolves to the same
        // root, and the walk is bounded by the filesystem root.
        let deeper = deep_bin.join("deps").join("nested");
        std::fs::create_dir_all(&deeper).expect("create nested dir");
        assert_eq!(walk_up_for_abi_dir(&deeper), Some(abi));
        let _ = std::fs::remove_dir_all(&root);
    }
}
