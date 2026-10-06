// ── ConfigResolver — Runtime Config File Resolution ────────────────────
// 2026-07-16: P1 — Centralizes config loading so profiles can be swapped
// at runtime via `briev-compiler config set <profile>`.
// Resolution chain: --config-dir CLI flag → BRIEV_CONFIG_DIR env var →
// ./.briev/config/ (project) → ~/.config/briev-compiler/active_profile →
// compile-time baked fallback.

use crate::target::TargetConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 2026-07-16: Resolved configuration for a single compilation session.
/// 2026-07-20: Simplified for hashword protocol. No type/op config.
pub struct ConfigResolver {
    /// 2026-07-16: The config directory path.
    pub config_dir: PathBuf,
    /// 2026-07-16: The active target configuration.
    pub target_config: TargetConfig,
    /// 2026-07-16: The module registry (import paths).
    pub module_registry: crate::config_resolver::ModuleRegistry,
}

impl ConfigResolver {
    /// Follow the resolution chain to find config files and load them.
    /// Priority:
    ///   1. --config-dir CLI flag override
    ///   2. BRIEV_CONFIG_DIR env var
    ///   3. ./.briev/config/ (project-local)
    ///   4. ~/.config/briev-compiler/active_profile symlink/text
    ///   5. Compile-time baked fallback (path = "__baked__")
    pub fn resolve(config_dir_override: Option<&Path>) -> Self {
        let config_dir = Self::resolve_config_dir(config_dir_override);

        let target_config = if config_dir.to_string_lossy() == "__baked__" {
            TargetConfig::load()
        } else {
            match crate::dbriev::config_db::resolve_config_file(&config_dir, "targets") {
                Some(path) => TargetConfig::load_from(&path).unwrap_or_else(|e| {
                    eprintln!("warning: cannot load '{}': {} — using baked fallback", path.display(), e);
                    TargetConfig::load()
                }),
                None => {
                    eprintln!("warning: no targets config found in '{}' — using baked fallback", config_dir.display());
                    TargetConfig::load()
                }
            }
        };

        let module_registry = Self::load_module_registry(&config_dir);

        ConfigResolver {
            config_dir,
            target_config,
            module_registry: ModuleRegistry { modules: module_registry },
        }
    }

    /// Resolve the config directory path through the priority chain.
    fn resolve_config_dir(override_dir: Option<&Path>) -> PathBuf {
        // 1. CLI override
        if let Some(dir) = override_dir {
            return dir.to_path_buf();
        }
        // 2. Environment variable
        if let Ok(env) = std::env::var("BRIEV_CONFIG_DIR") {
            return PathBuf::from(env);
        }
        // 3. Project-local
        if Path::new(".briev/config").exists() {
            return PathBuf::from(".briev/config");
        }
        // 4. User-global active profile
        if let Some(user_config) = dirs::config_dir() {
            let briev_config = user_config.join("briev-compiler");
            let active = briev_config.join("active_profile");
            // Try symlink first
            if let Ok(target) = std::fs::read_link(&active) {
                let profile_dir = if target.is_absolute() {
                    target
                } else {
                    briev_config.join(target)
                };
                if profile_dir.exists() {
                    return profile_dir;
                }
            }
            // Fallback: read as text file
            if let Ok(name) = std::fs::read_to_string(&active) {
                let profile_dir = briev_config.join("profiles").join(name.trim());
                if profile_dir.exists() {
                    return profile_dir;
                }
            }
        }
        // 5. Compile-time baked — marker path triggers fallback loading
        PathBuf::from("__baked__")
    }

    /// Load the module registry (config/module-registry.dbvl).
    ///
    /// 2026-08-03 (Phase 3): the Data Briev form is the only form now — the
    /// pre-migration .toml is deleted and the TOML fallback is removed.
    fn load_module_registry(config_dir: &Path) -> HashMap<String, String> {
        crate::dbriev::config_db::load_string_registry(config_dir, "module-registry")
    }

    /// Resolve a logical config name against this session's resolved config
    /// dir, as a Data Briev file (`.dbvl`/`.dbv`).
    ///
    /// 2026-08-03 (Phase 1a → 3, plan docs/plans/2026-08-03-data-briev-config-
    /// and-board-hardware-map.md): the migration seam for Phase 3. Existing
    /// `--config-dir`/profile users keep working because only the resolved
    /// extension changed during the migration.
    pub fn resolve_config(&self, name: &str) -> Option<PathBuf> {
        crate::dbriev::config_db::resolve_config_file(&self.config_dir, name)
    }
}

/// Pre-migration TOML structure for config/module-registry.toml (retained for
/// the golden-test reference; the file itself is deleted).
#[derive(serde::Deserialize)]
pub struct ModuleRegistry {
    modules: HashMap<String, String>,
}

/// Manage config profiles in ~/.config/briev-compiler/
pub fn list_profiles() -> Result<Vec<String>, String> {
    let base = config_base_dir()?;
    let profiles_dir = base.join("profiles");
    if !profiles_dir.exists() {
        return Ok(vec![]);
    }
    let mut profiles = Vec::new();
    for entry in std::fs::read_dir(&profiles_dir)
        .map_err(|e| format!("cannot read '{}': {}", profiles_dir.display(), e))?
    {
        let entry = entry.map_err(|e| format!("read error: {}", e))?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                profiles.push(name.to_string());
            }
        }
    }
    profiles.sort();
    Ok(profiles)
}

pub fn set_active_profile(name: &str) -> Result<(), String> {
    let base = config_base_dir()?;
    let profile_path = base.join("profiles").join(name);
    if !profile_path.exists() {
        return Err(format!("profile '{}' not found", name));
    }
    let active = base.join("active_profile");
    // Try symlink first
    if std::fs::remove_file(&active).is_err() {
        // File may not exist — ignore
    }
    // On platforms without symlink support, write a text file
    #[cfg(unix)]
    {
        let target = format!("profiles/{}", name);
        std::os::unix::fs::symlink(&target, &active)
            .map_err(|e| format!("cannot create symlink '{}': {}", active.display(), e))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&active, name)
            .map_err(|e| format!("cannot write '{}': {}", active.display(), e))?;
    }
    Ok(())
}

pub fn show_active_profile() -> Result<(), String> {
    let base = config_base_dir()?;
    let active = base.join("active_profile");
    let profile_dir = if let Ok(target) = std::fs::read_link(&active) {
        if target.is_absolute() { target } else { base.join(target) }
    } else if let Ok(name) = std::fs::read_to_string(&active) {
        base.join("profiles").join(name.trim())
    } else {
        return Err("no active profile set".to_string());
    };

    println!("Active profile: {}", profile_dir.display());
    println!();
    if profile_dir.exists() {
        for entry in std::fs::read_dir(&profile_dir)
            .map_err(|e| format!("cannot read '{}': {}", profile_dir.display(), e))?
        {
            let entry = entry.map_err(|e| format!("read error: {}", e))?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    println!("  {}:", name);
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        for line in content.lines().take(10) {
                            println!("    {}", line);
                        }
                        if content.lines().count() > 10 {
                            println!("    ... ({} total lines)", content.lines().count());
                        }
                    }
                }
            }
        }
    } else {
        println!("  (profile directory does not exist)");
    }
    Ok(())
}

pub fn init_profile(name: &str) -> Result<(), String> {
    let base = config_base_dir()?;
    let profile_dir = base.join("profiles").join(name);
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("cannot create '{}': {}", profile_dir.display(), e))?;

    let baked_dir = crate::resource_root().join("config");
    // 2026-08-03 (Phase 3): all six configs are Data Briev now — seed the
    // profile with the .dbvl forms so created profiles load through the DB
    // path. (The pre-migration TOMLs were deleted; nothing consumes them.)
    for file in &[
        "targets.dbvl", "module-registry.dbvl", "ir-lowering.dbvl",
        "protocols.dbvl", "encodings.dbvl", "alloc-strategies.dbvl",
        "address-map.dbvl",
    ] {
        let src = baked_dir.join(file);
        if src.exists() {
            let content = std::fs::read_to_string(&src)
                .map_err(|e| format!("cannot read '{}': {}", src.display(), e))?;
            let dst = profile_dir.join(file);
            std::fs::write(&dst, &content)
                .map_err(|e| format!("cannot write '{}': {}", dst.display(), e))?;
            println!("wrote {}", dst.display());
        }
    }

    // Set as active
    set_active_profile(name)?;
    println!("Profile '{}' initialized and set as active.", name);
    Ok(())
}

/// Get the base config directory (~/.config/briev-compiler/).
fn config_base_dir() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|d| d.join("briev-compiler"))
        .ok_or_else(|| "cannot determine config directory (no $HOME?)".to_string())
}
