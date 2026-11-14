// ── Macro Capability Lockfile (macro-lock.toml) ──────────────────────
// 2026-07-23: Records SHA-256 hashes and approved capabilities for every
// system plugin (.bv file in plugins/). On subsequent builds, validates
// that no plugin has changed its capability requirements without re-approval.
//
// Flow:
//   1. User runs with --update-lockfile + --allow-* flags → lockfile generated
//   2. Lockfile records (plugin_name, hash, requested_caps) for each plugin
//   3. On plain `briev build`, lockfile is validated and applied:
//      - All plugins must be in lockfile (error if new plugin found)
//      - Hash changes with NEW capabilities → error with diff
//      - Hash changes with SAME capabilities → silently accept
//      - Lockfile's approved capabilities are granted to the sandbox
//
// Flat control flow: max 2 levels deep.

use std::collections::{BTreeSet, HashMap};

/// The macro capability lockfile — macro-lock.toml.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MacroLock {
    pub version: u32,
    #[serde(rename = "plugin")]
    pub plugins: HashMap<String, PluginLockEntry>,
}

/// A single plugin entry in the lockfile.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginLockEntry {
    pub hash: String,
    pub requested: Vec<String>,
}

/// All stage directory names for plugin discovery.
/// Must match the directories in discover_system_plugins.
const STAGE_DIRS: &[&str] = &[
    "prelex",
    "parsed",
    "resolved",
    "typed",
    "normalized",
    "verified",
    "allocated",
    "provenanced",
    "generated",
    "optimized",
    "linked",
];

/// Load macro-lock.toml from the given directory.
pub fn load_lockfile(dir: &str) -> Result<Option<MacroLock>, String> {
    let path = format!("{}/macro-lock.toml", dir);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("failed to read macro-lock.toml: {}", e)),
    };
    let lock: MacroLock =
        toml::from_str(&content).map_err(|e| format!("failed to parse macro-lock.toml: {}", e))?;
    if lock.version != 1 {
        return Err(format!(
            "macro-lock.toml: unsupported version {} (expected 1)",
            lock.version
        ));
    }
    Ok(Some(lock))
}

/// Write macro-lock.toml to the given directory.
pub fn save_lockfile(dir: &str, lock: &MacroLock) -> Result<(), String> {
    let content =
        toml::to_string_pretty(lock).map_err(|e| format!("failed to serialize lockfile: {}", e))?;
    let path = format!("{}/macro-lock.toml", dir);
    std::fs::write(&path, &content).map_err(|e| format!("failed to write macro-lock.toml: {}", e))
}

/// Compute SHA-256 hex digest.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Scan a plugin source for non-pure `$` intrinsic calls and return the
/// set of capabilities it requires.
pub fn scan_capabilities(source: &str) -> Vec<String> {
    let mut caps = BTreeSet::new();
    for line in source.lines() {
        if line.contains("FileRead$") || line.contains("ConfigGet$") {
            caps.insert("disk-read".to_string());
        }
        if line.contains("FileWrite$") {
            caps.insert("disk-write".to_string());
        }
        if line.contains("ShellCmd$") {
            caps.insert("shell".to_string());
        }
        if line.contains("SysQuery$") {
            caps.insert("sys-query".to_string());
        }
        if line.contains("HttpFetch$") {
            caps.insert("network".to_string());
        }
    }
    let mut result: Vec<String> = caps.into_iter().collect();
    result.sort();
    result
}

/// Discover all system plugin files in the plugins/ directory tree.
/// If `base_dir` is None, uses `plugins/` relative to current working directory.
/// Returns (plugin_name, file_path) pairs, sorted by name.
pub fn discover_plugin_files(base_dir: Option<&str>) -> Vec<(String, String)> {
    let base = match base_dir {
        Some(d) => std::path::Path::new(d).join("plugins"),
        None => std::path::PathBuf::from("plugins"),
    };
    let mut results = Vec::new();
    for dir_name in STAGE_DIRS {
        let dir = base.join(dir_name);
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bv") {
                continue;
            }
            let file_stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let path_str = path.to_string_lossy().to_string();
            results.push((file_stem, path_str));
        }
    }
    results.sort_by_key(|(name, _)| name.clone());
    results
}

/// Scan a single plugin file, returning (hash, capabilities).
fn scan_file(file_path: &str) -> Result<(String, Vec<String>), String> {
    let source = std::fs::read_to_string(file_path)
        .map_err(|e| format!("cannot read '{}': {}", file_path, e))?;
    let hash = sha256_hex(source.as_bytes());
    let caps = scan_capabilities(&source);
    Ok((hash, caps))
}

/// Map a capability string to its sandbox field setter.
fn apply_capability(sandbox: &mut crate::macros::eval::Sandbox, cap: &str) -> Result<(), String> {
    match cap {
        "disk-read" => sandbox.allow_read = true,
        "disk-write" => sandbox.allow_write = true,
        "shell" => sandbox.allow_run = true,
        "sys-query" => sandbox.allow_sys_query = true,
        "network" => sandbox.allow_net = true,
        other => return Err(format!("unknown capability '{}' in macro-lock.toml", other)),
    }
    Ok(())
}

/// Generate a macro-lock.toml from the current plugin files.
///
/// Only records capabilities that were both requested by the plugin AND
/// granted via the `granted` set (derived from --allow-* flags).
/// `base_dir` is the project root (None = current working directory).
pub fn generate_lockfile(
    granted: &BTreeSet<String>,
    base_dir: Option<&str>,
) -> Result<MacroLock, String> {
    let mut lock = MacroLock {
        version: 1,
        plugins: HashMap::new(),
    };
    for (name, file_path) in discover_plugin_files(base_dir) {
        let (hash, all_caps) = scan_file(&file_path)?;
        let approved: Vec<String> = all_caps
            .into_iter()
            .filter(|c| granted.contains(c))
            .collect();
        lock.plugins.insert(
            name,
            PluginLockEntry {
                hash,
                requested: approved,
            },
        );
    }
    Ok(lock)
}

/// Validate loaded plugins against the lockfile and apply approved
/// capabilities to the sandbox.
///
/// Errors if:
///   - A plugin exists on disk but not in the lockfile
///   - A plugin's hash changed AND it now requests capabilities not
///     previously approved
///
/// Silently accepts:
///   - Hash changes with no new capability requirements
///   - Pure plugins (no non-pure intrinsics) that aren't in lockfile
/// `base_dir` is the project root (None = current working directory).
pub fn validate_and_apply(
    lock: &MacroLock,
    pm: &mut crate::plugin::PluginManager,
    base_dir: Option<&str>,
) -> Result<(), String> {
    let approved: BTreeSet<String> = lock
        .plugins
        .values()
        .flat_map(|e| e.requested.iter().cloned())
        .collect();

    for (name, file_path) in discover_plugin_files(base_dir) {
        let (current_hash, current_caps) = scan_file(&file_path)?;

        let entry = lock.plugins.get(&name);
        let Some(entry) = entry else {
            // Plugin not in lockfile at all.
            if current_caps.is_empty() {
                // Pure plugin — no capabilities needed. Silently allow.
                continue;
            }
            let unapproved: Vec<&str> = current_caps
                .iter()
                .filter(|c| !approved.contains(*c))
                .map(|s| s.as_str())
                .collect();
            if unapproved.is_empty() {
                // All requested capabilities are already approved by other plugins.
                continue;
            }
            return Err(format!(
                "plugin '{}' is not in macro-lock.toml and requires {:?}\n\
                 Run with --update-lockfile and appropriate --allow-* flags to approve.",
                name, unapproved
            ));
        };

        if current_hash != entry.hash {
            let old_set: BTreeSet<&str> = entry.requested.iter().map(|s| s.as_str()).collect();
            let new_caps: Vec<&str> = current_caps
                .iter()
                .filter(|c| !old_set.contains(c.as_str()))
                .map(|s| s.as_str())
                .collect();
            if !new_caps.is_empty() {
                return Err(format!(
                    "plugin '{}' has changed and now requests new capabilities: {:?}\n\
                     Previously approved: {:?}\n\
                     Run with --update-lockfile and appropriate --allow-* flags to re-approve.",
                    name, new_caps, entry.requested
                ));
            }
        }
    }

    for sandbox_cap in &approved {
        apply_capability(&mut pm.sandbox, sandbox_cap)?;
    }

    Ok(())
}

/// Build the set of CLI-granted capability strings from BuildOptions.
pub fn cli_granted_set(
    allow_read: bool,
    allow_write: bool,
    allow_run: bool,
    allow_sys_query: bool,
    allow_net: bool,
) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if allow_read {
        set.insert("disk-read".to_string());
    }
    if allow_write {
        set.insert("disk-write".to_string());
    }
    if allow_run {
        set.insert("shell".to_string());
    }
    if allow_sys_query {
        set.insert("sys-query".to_string());
    }
    if allow_net {
        set.insert("network".to_string());
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Return the path of a temp dir as a String for use as base_dir.
    fn base(temp: &tempfile::TempDir) -> String {
        temp.path().to_string_lossy().to_string()
    }

    /// Create a temporary plugin file under base/plugins/subdir/.
    fn create_plugin(base: &str, subdir: &str, name: &str, content: &str) {
        let plugin_dir = std::path::Path::new(base).join("plugins").join(subdir);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let path = plugin_dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn test_scan_capabilities_empty() {
        let source = "Tag$(\"defn\").Count$()";
        let caps = scan_capabilities(source);
        assert!(caps.is_empty());
    }

    #[test]
    fn test_scan_capabilities_disk_read() {
        let source = "let content = FileRead$(\"data.txt\");";
        let caps = scan_capabilities(source);
        assert_eq!(caps, vec!["disk-read"]);
    }

    #[test]
    fn test_scan_capabilities_disk_write() {
        let source = "FileWrite$(\"out.txt\", content);";
        let caps = scan_capabilities(source);
        assert_eq!(caps, vec!["disk-write"]);
    }

    #[test]
    fn test_scan_capabilities_shell() {
        let source = "let out = ShellCmd$(\"curl\", url);";
        let caps = scan_capabilities(source);
        assert_eq!(caps, vec!["shell"]);
    }

    #[test]
    fn test_scan_capabilities_sys_query() {
        let source = "let cores = SysQuery$(\"cpu.cores\");";
        let caps = scan_capabilities(source);
        assert_eq!(caps, vec!["sys-query"]);
    }

    #[test]
    fn test_scan_capabilities_network() {
        let source = "let body = HttpFetch$(\"https://example.com\");";
        let caps = scan_capabilities(source);
        assert_eq!(caps, vec!["network"]);
    }

    #[test]
    fn test_scan_capabilities_multiple() {
        let source = r#"
            let data = FileRead$("input.txt");
            FileWrite$("output.txt", data);
            let body = HttpFetch$("https://api.example.com");
        "#;
        let caps = scan_capabilities(source);
        assert_eq!(caps, vec!["disk-read", "disk-write", "network"]);
    }

    #[test]
    fn test_discover_plugin_files_empty() {
        let dir = tempfile::tempdir().unwrap();
        let files = discover_plugin_files(Some(&base(&dir)));
        assert!(files.is_empty());
    }

    #[test]
    fn test_discover_plugin_files_finds_bv() {
        let dir = tempfile::tempdir().unwrap();
        let dir_str = base(&dir);
        create_plugin(
            &dir_str,
            "parsed",
            "test-plugin.bv",
            "$(Parsed) { EmitInfo$(\"hi\"); }",
        );
        let files = discover_plugin_files(Some(&dir_str));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "test-plugin");
        assert!(files[0].1.ends_with("test-plugin.bv"));
    }

    #[test]
    fn test_load_lockfile_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let lock = load_lockfile(&base(&dir)).unwrap();
        assert!(lock.is_none());
    }

    #[test]
    fn test_generate_and_validate_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        let dir_str = base(&dir);

        create_plugin(
            &dir_str,
            "parsed",
            "reader.bv",
            "$(Parsed) {\n    let data = FileRead$(\"input.txt\");\n};",
        );

        // Generate lockfile with disk-read granted, using temp dir as base
        let granted: BTreeSet<String> = ["disk-read"].iter().map(|s| s.to_string()).collect();
        let lock = generate_lockfile(&granted, Some(&dir_str)).unwrap();
        assert_eq!(lock.version, 1);
        assert_eq!(lock.plugins.len(), 1);

        let entry = lock.plugins.get("reader").unwrap();
        assert_eq!(entry.requested, vec!["disk-read"]);
        assert!(!entry.hash.is_empty());

        // Save and reload
        save_lockfile(&dir_str, &lock).unwrap();
        let loaded = load_lockfile(&dir_str).unwrap().unwrap();
        assert_eq!(loaded.plugins.len(), 1);

        // Validate against a PluginManager
        use crate::macros::eval::Sandbox;
        use crate::plugin::PluginManager;
        let mut pm = PluginManager::new();
        pm = pm.with_sandbox(Sandbox::default());
        assert!(!pm.sandbox.allow_read);

        validate_and_apply(&loaded, &mut pm, Some(&dir_str)).unwrap();
        assert!(pm.sandbox.allow_read);
    }

    #[test]
    fn test_validate_rejects_new_capability() {
        let dir = tempfile::tempdir().unwrap();
        let dir_str = base(&dir);

        create_plugin(
            &dir_str,
            "parsed",
            "writer.bv",
            "$(Parsed) {\n    let data = FileRead$(\"input.txt\");\n};",
        );

        let granted: BTreeSet<String> = ["disk-read"].iter().map(|s| s.to_string()).collect();
        let lock = generate_lockfile(&granted, Some(&dir_str)).unwrap();
        save_lockfile(&dir_str, &lock).unwrap();

        // Plugin changes to also write — same file path, new content
        create_plugin(
            &dir_str,
            "parsed",
            "writer.bv",
            "$(Parsed) {\n    let data = FileRead$(\"input.txt\");\n    FileWrite$(\"out.txt\", data);\n};",
        );

        let loaded = load_lockfile(&dir_str).unwrap().unwrap();
        let mut pm = crate::plugin::PluginManager::new();
        pm = pm.with_sandbox(crate::macros::eval::Sandbox::default());
        let result = validate_and_apply(&loaded, &mut pm, Some(&dir_str));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("writer"));
        assert!(err.contains("disk-write"));
    }

    #[test]
    fn test_validate_accepts_same_hash() {
        let dir = tempfile::tempdir().unwrap();
        let dir_str = base(&dir);

        create_plugin(
            &dir_str,
            "parsed",
            "stable.bv",
            "$(Parsed) {\n    let data = FileRead$(\"input.txt\");\n};",
        );

        let granted: BTreeSet<String> = ["disk-read"].iter().map(|s| s.to_string()).collect();
        let lock = generate_lockfile(&granted, Some(&dir_str)).unwrap();
        save_lockfile(&dir_str, &lock).unwrap();

        // Same content, same hash
        create_plugin(
            &dir_str,
            "parsed",
            "stable.bv",
            "$(Parsed) {\n    let data = FileRead$(\"input.txt\");\n};",
        );

        let loaded = load_lockfile(&dir_str).unwrap().unwrap();
        let mut pm = crate::plugin::PluginManager::new();
        pm = pm.with_sandbox(crate::macros::eval::Sandbox::default());
        assert!(validate_and_apply(&loaded, &mut pm, Some(&dir_str)).is_ok());
        assert!(pm.sandbox.allow_read);
    }

    #[test]
    fn test_cli_granted_set() {
        let set = cli_granted_set(true, false, true, false, false);
        let expected: BTreeSet<String> = ["disk-read", "shell"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(set, expected);
    }
}
