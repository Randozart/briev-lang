// ── Compiler Registry — `brievc registry` ──────────────────────────────
// 2026-07-26: Phase 1f — Per-user registry directory for installing
// Briev modules and foreign sources. Managed by `brievc registry {add,list,remove}`.
//
// Copy-only (version-locked, no symlinks). Project-local .briev/registry/
// overrides the user-wide dirs::data_dir() path.
//
// Lookup order for import <name> / from <name>:
//   1. Project-local .briev/registry/<name>
//   2. User-wide ~/.briev/registry/<name> (or platform equivalent)
//   3. config/module-registry.toml (for imports)
//   4. Stdlib path (for from <name> and import <name> fallback)

use std::path::{Path, PathBuf};

/// Resolve the primary registry directory (user-wide).
///
/// Uses dirs::data_dir() for cross-platform compatibility:
/// - Linux:   ~/.local/share/briev/registry/
/// - macOS:   ~/Library/Application Support/briev/registry/
/// - Windows: %APPDATA%/briev/registry/
///
/// 2026-07-26: Falls back to ~/.briev/registry/ if data_dir() fails.
pub fn user_registry_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| {
        // Fallback: ~/.briev/
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".briev")
    });
    base.join("briev").join("registry")
}

/// Resolve the project-local registry directory.
///
/// 2026-07-26: Returns .briev/registry/ relative to the given project root.
pub fn project_registry_path(project_root: &Path) -> PathBuf {
    project_root.join(".briev").join("registry")
}

/// Resolve the best available registry directory for lookups.
///
/// Returns (user_path, project_path) where project_path takes priority.
/// 2026-07-26: Used by ImportResolver and collect_extra_objects to find
/// registry-installed files.
pub fn registry_paths() -> (PathBuf, Option<PathBuf>) {
    let user = user_registry_path();
    let project = project_registry_path(&PathBuf::from("."));
    if project.exists() {
        (user, Some(project))
    } else {
        (user, None)
    }
}

/// Find a registry entry by name across all registry directories.
///
/// Checks project-local first, then user-wide. Returns the path if found.
/// 2026-07-26: Searches for both <name> (exact match) and <name>/<name>.bv
/// (multi-file package convention).
pub fn find_registry_entry(name: &str) -> Option<PathBuf> {
    let (user_path, project_path) = registry_paths();

    // Check project-local first
    if let Some(ref proj) = project_path {
        let exact = proj.join(name);
        if exact.exists() {
            return Some(exact);
        }
        let pkg = proj.join(name).join(format!("{}.bv", name));
        if pkg.exists() {
            return Some(pkg);
        }
    }

    // Then check user-wide
    let exact = user_path.join(name);
    if exact.exists() {
        return Some(exact);
    }
    let pkg = user_path.join(name).join(format!("{}.bv", name));
    if pkg.exists() {
        return Some(pkg);
    }

    None
}

/// Add a source path to the registry (copy, version-locked).
///
/// 2026-07-26: Copies the source file or directory to the user registry.
/// - Single file: registry/<name> (preserves extension)
/// - Directory: registry/<name>/ (recursive copy)
/// Returns the destination path.
pub fn add(source: &Path, name: &str) -> Result<PathBuf, String> {
    let dest = user_registry_path().join(name);
    if !source.exists() {
        return Err(format!("source path '{}' does not exist", source.display()));
    }
    // Create parent directory
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "cannot create registry directory '{}': {}",
                parent.display(),
                e
            )
        })?;
    }
    if source.is_dir() {
        // Recursive copy directory tree
        copy_dir_recursive(source, &dest)?;
    } else {
        std::fs::copy(source, &dest)
            .map_err(|e| format!("failed to copy '{}' to registry: {}", source.display(), e))?;
    }
    println!(
        "added '{}' → registry/{} (version-locked)",
        source.display(),
        name
    );
    Ok(dest)
}

/// List all entries in the registry.
///
/// 2026-07-26: Enumerates both project-local and user-wide registry dirs.
pub fn list() -> Result<Vec<(String, String, u64)>, String> {
    let mut entries: Vec<(String, String, u64)> = Vec::new();
    let (user_path, project_path) = registry_paths();

    // Helper to scan a single registry directory
    let mut scan = |dir: &Path, source_label: &str| -> Result<(), String> {
        if !dir.exists() {
            return Ok(());
        }
        let read_dir = std::fs::read_dir(dir)
            .map_err(|e| format!("cannot read registry '{}': {}", dir.display(), e))?;
        for entry in read_dir {
            let entry = entry.map_err(|e| format!("registry read error: {}", e))?;
            let file_type = entry
                .file_type()
                .map_err(|e| format!("registry entry type error: {}", e))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let kind = if file_type.is_dir() { "dir" } else { "file" };
            let size = if file_type.is_file() {
                std::fs::metadata(entry.path())
                    .map(|m| m.len())
                    .unwrap_or(0)
            } else {
                0
            };
            entries.push((name, format!("{} ({})", kind, source_label), size));
        }
        Ok(())
    };

    scan(&user_path, "user")?;
    if let Some(ref proj) = project_path {
        scan(proj, "project")?;
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

/// Remove a registry entry.
///
/// 2026-07-26: Deletes matching entries from both user and project registries.
/// Handles both files and directories. Returns count of removed entries.
pub fn remove(name: &str) -> Result<usize, String> {
    let (user_path, project_path) = registry_paths();
    let mut removed = 0;

    let targets = match project_path {
        Some(ref proj) => vec![user_path, proj.clone()],
        None => vec![user_path],
    };

    for dir in &targets {
        let exact = dir.join(name);
        if exact.exists() {
            if exact.is_dir() {
                std::fs::remove_dir_all(&exact).map_err(|e| {
                    format!("failed to remove directory '{}': {}", exact.display(), e)
                })?;
            } else {
                std::fs::remove_file(&exact)
                    .map_err(|e| format!("failed to remove '{}': {}", exact.display(), e))?;
            }
            println!("removed registry/{}", name);
            removed += 1;
        }
    }

    if removed == 0 {
        return Err(format!("registry entry '{}' not found", name));
    }
    Ok(removed)
}

/// Recursively copy a directory tree.
/// 2026-07-26: Used by add() for multi-file packages. Simple recursive copy
/// without symlink preservation or hardlink detection.
fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("cannot create directory '{}': {}", dest.display(), e))?;
    let read_dir = std::fs::read_dir(source)
        .map_err(|e| format!("cannot read source directory '{}': {}", source.display(), e))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| format!("readdir error: {}", e))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("file type error: {}", e))?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            std::fs::copy(&src_path, &dest_path)
                .map_err(|e| format!("failed to copy '{}': {}", src_path.display(), e))?;
        }
    }
    Ok(())
}
