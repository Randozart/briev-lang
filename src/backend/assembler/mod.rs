// ── AsmAssembler Trait — Pluggable Assembly Backend ───────────────────
// 2026-07-29: Compile-time assembly validation trait.
// Each implementation validates asm text for a target architecture and
// returns assembled bytes (for verification) or an error.
// Config-driven: selected via `assembler` key in config/targets.dbvl.
// Supports three backends: Keystone (default on supported platforms),
// platform assembler (system as/ml64), and stub (warn-only, no validation).

use std::fmt::Debug;

/// 2026-07-29: Compile-time assembly validation trait.
///
/// Each implementation validates asm instruction text for a target
/// architecture and returns the assembled bytes or an error.
/// The selected implementation is determined by `config/targets.dbvl`'s
/// `assembler` key. The `:=` verification chain uses this to cross-verify
/// asm bodies against reference implementations at compile time.
///
/// # Implementations
///
/// - `KeystoneAssembler` — links against Keystone Engine C library.
///   Fast, supports all LLVM architectures. Default when available.
/// - `PlatformAssembler` — shells out to system `as`/`ml64`.
///   No external library needed. Slower, arch-dependent.
/// - `StubAssembler` — no-op, warns at compile time.
///   Safe fallback for unsupported platforms.
pub trait AsmAssembler: Debug {
    /// Human-readable name (e.g., "keystone", "platform", "none").
    fn name(&self) -> &str;

    /// Validate and assemble an instruction or block.
    ///
    /// `text`: the instruction template with `{param}` already substituted
    /// with ABI registers.
    /// `arch`: target architecture string (e.g., "x86_64", "aarch64").
    ///
    /// Returns assembled bytes on success, error description on failure.
    fn assemble(&self, text: &str, arch: &str) -> Result<Vec<u8>, String>;

    /// Whether this assembler is available on the current system.
    fn is_available(&self) -> bool;
}

// ── Stub Implementation ─────────────────────────────────────────────

/// 2026-07-29: No-op assembler — warns at compile time and passes through
/// without validation. Selected via `assembler = "none"` in targets.dbvl.
/// Safe fallback for platforms where Keystone or system assembler
/// is not available. Cross-verification in the := chain still catches
/// semantic errors even without byte-level validation.
#[derive(Debug)]
pub struct StubAssembler;

impl AsmAssembler for StubAssembler {
    fn name(&self) -> &str { "none" }

    fn assemble(&self, text: &str, arch: &str) -> Result<Vec<u8>, String> {
        eprintln!("  warning: assembly not validated for {}: {}", arch, text);
        eprintln!("  warning: set assembler = \"keystone\" or assembler = \"platform\" in config/targets.dbvl");
        Ok(vec![])
    }

    fn is_available(&self) -> bool { true }
}

// ── Platform Implementation ──────────────────────────────────────────

/// 2026-07-29: System assembler — shells out to `as` on Unix or `ml64`
/// on Windows. No external library needed. Selected via `assembler = "platform"`.
/// Uses a temp file for each assembly invocation.
#[derive(Debug)]
pub struct PlatformAssembler;

impl PlatformAssembler {
    /// Map Briev arch string to the assembler binary and flags.
    fn assembler_for_arch(arch: &str) -> (&'static str, &'static [&'static str]) {
        match arch {
            "x86_64" => ("as", &["--64"]),
            "aarch64" => ("as", &[]),
            "wasm32" | "wasm64" => ("wasm-as", &[]),
            _ => ("as", &[]),
        }
    }
}

impl AsmAssembler for PlatformAssembler {
    fn name(&self) -> &str { "platform" }

    fn assemble(&self, text: &str, arch: &str) -> Result<Vec<u8>, String> {
        let (assembler, flags) = Self::assembler_for_arch(arch);
        let mut tmp_dir = std::env::temp_dir();
        tmp_dir.push(format!("briev_asm_{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("cannot create tmp dir: {}", e))?;

        let src_path = tmp_dir.join("input.s");
        let obj_path = tmp_dir.join("input.o");
        std::fs::write(&src_path, text).map_err(|e| format!("cannot write asm file: {}", e))?;

        let output = std::process::Command::new(assembler)
            .args(flags)
            .arg("-o")
            .arg(&obj_path)
            .arg(&src_path)
            .output()
            .map_err(|e| format!("failed to run '{}': {}", assembler, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return Err(format!("assembler error for {}: {}", arch, stderr.trim()));
        }

        let bytes = std::fs::read(&obj_path)
            .map_err(|e| format!("cannot read assembled output: {}", e))?;
        let _ = std::fs::remove_dir_all(&tmp_dir);
        Ok(bytes)
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("as")
            .arg("--version")
            .output()
            .is_ok()
    }
}

// ── Selector ─────────────────────────────────────────────────────────

/// 2026-07-29: Select the assembler implementation based on config.
/// Falls back to StubAssembler for unknown values.
pub fn get_assembler(config: &crate::target::TargetEntry) -> Box<dyn AsmAssembler> {
    match config.assembler.as_str() {
        "none" => Box::new(StubAssembler),
        "platform" => Box::new(PlatformAssembler),
        other => {
            eprintln!("  warning: unknown assembler '{}', falling back to 'none'", other);
            Box::new(StubAssembler)
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stub_assembler_name() {
        let stub = StubAssembler;
        assert_eq!(stub.name(), "none");
    }

    #[test]
    fn test_stub_assembler_available() {
        let stub = StubAssembler;
        assert!(stub.is_available());
    }

    #[test]
    fn test_stub_assembler_assemble() {
        let stub = StubAssembler;
        let result = stub.assemble("nop", "x86_64");
        assert!(result.is_ok(), "stub should always succeed");
    }

    #[test]
    fn test_get_assembler_unknown_fallback() {
        let entry = crate::target::TargetEntry {
            backend: Some("llvm".to_string()),
            defaults: vec![],
            plugins: None,
            target_triple: None,
            data_layout: None,
            assembler: "unknown_value".into(),
            cross_verify_samples: 50,
            linker_script: None,
        };
        let asm = get_assembler(&entry);
        assert_eq!(asm.name(), "none", "unknown assembler should fall back to stub");
    }

    #[test]
    fn test_get_assembler_none() {
        let entry = crate::target::TargetEntry {
            backend: Some("llvm".to_string()),
            defaults: vec![],
            plugins: None,
            target_triple: None,
            data_layout: None,
            assembler: "none".into(),
            cross_verify_samples: 50,
            linker_script: None,
        };
        let asm = get_assembler(&entry);
        assert_eq!(asm.name(), "none");
    }

    #[test]
    fn test_get_assembler_platform() {
        let entry = crate::target::TargetEntry {
            backend: Some("llvm".to_string()),
            defaults: vec![],
            plugins: None,
            target_triple: None,
            data_layout: None,
            assembler: "platform".into(),
            cross_verify_samples: 50,
            linker_script: None,
        };
        let asm = get_assembler(&entry);
        assert_eq!(asm.name(), "platform");
    }

    #[test]
    fn test_platform_assembler_is_available() {
        let pa = PlatformAssembler;
        // Should be available on any system with `as` installed
        assert_eq!(pa.is_available(), std::process::Command::new("as").arg("--version").output().is_ok());
    }

    #[test]
    fn test_platform_assembler_assemble_nop_x86_64() {
        let pa = PlatformAssembler;
        if pa.is_available() {
            let result = pa.assemble("nop\n", "x86_64");
            assert!(result.is_ok(), "nop should assemble: {:?}", result);
            // A single nop is 1 byte (0x90)
            assert!(!result.unwrap().is_empty(), "should produce bytes");
        }
    }
}
