// ── Optional Dependency Manager ────────────────────────────────────
// 2026-07-25: Downloads and installs optional external deps
// (z3, dwarfdump) from pinned GitHub release URLs.
// Called via `brievc install-deps`.
//
// Uses curl/wget for transport — no external Rust dependencies.
// Binaries go to ~/.local/share/briev-compiler/bin/.

use std::path::PathBuf;
use std::process::Command;

const Z3_VERSION: &str = "4.13.0";
const Z3_URL: &str =
    "https://github.com/Z3Prover/z3/releases/download/z3-4.13.0/z3-4.13.0-x64-glibc-2.31.zip";

const DWARFDUMP_URL: &str = "https://github.com/llvm/llvm-project/releases/download/llvmorg-19.1.0/llvm-dwarfdump-19.1.0-x86_64-unknown-linux-gnu.tar.xz";

/// Get the binary directory for compiler-managed dependencies.
/// Returns ~/.local/share/briev-compiler/bin/
pub fn dep_bin_dir() -> PathBuf {
    let base = if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(dir)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".local").join("share")
    };
    base.join("briev-compiler").join("bin")
}

/// Check if z3 is available on PATH or in the managed binary directory.
pub fn is_z3_installed() -> bool {
    find_in_path("z3").is_some() || dep_bin_dir().join("z3").exists()
}

/// Check if dwarfdump is available on PATH or in the managed binary directory.
pub fn is_dwarfdump_installed() -> bool {
    find_in_path("llvm-dwarfdump").is_some() || dep_bin_dir().join("llvm-dwarfdump").exists()
}

/// Install all optional dependencies (z3, dwarfdump).
pub fn install_all() -> Result<(), String> {
    let bin_dir = dep_bin_dir();
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create {}: {}", bin_dir.display(), e))?;

    if !is_z3_installed() {
        println!("z3 not found — downloading v{}...", Z3_VERSION);
        install_z3(&bin_dir)?;
    } else {
        println!("z3 already installed");
    }

    if !is_dwarfdump_installed() {
        println!("llvm-dwarfdump not found — downloading...");
        install_dwarfdump(&bin_dir)?;
    } else {
        println!("llvm-dwarfdump already installed");
    }

    println!("\nDependencies installed to: {}", bin_dir.display());
    println!(
        "Make sure this directory is on your PATH, or the compiler will find it automatically."
    );
    Ok(())
}

/// Download z3, extract, and place the binary in the target directory.
fn install_z3(target_dir: &PathBuf) -> Result<(), String> {
    let archive = target_dir.join("z3.zip");
    download_file(Z3_URL, &archive)?;

    // Extract — z3 archive contains a directory with the binary inside
    let extract_dir = target_dir.join("z3_extract");
    std::fs::create_dir_all(&extract_dir).map_err(|e| format!("mkdir: {}", e))?;
    run_command(
        "unzip",
        &[
            "-o",
            &archive.to_string_lossy(),
            "-d",
            &extract_dir.to_string_lossy(),
        ],
    )?;

    // Find the z3 binary within the extracted directory tree
    let z3_binary = find_file_recursive(&extract_dir, "z3")
        .ok_or_else(|| "z3 binary not found after extraction".to_string())?;

    std::fs::rename(&z3_binary, target_dir.join("z3")).map_err(|e| format!("rename z3: {}", e))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            target_dir.join("z3"),
            std::fs::Permissions::from_mode(0o755),
        )
        .map_err(|e| format!("chmod z3: {}", e))?;
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&extract_dir);
    let _ = std::fs::remove_file(&archive);

    println!("  z3 installed");
    Ok(())
}

/// Download dwarfdump and place it in the target directory.
fn install_dwarfdump(target_dir: &PathBuf) -> Result<(), String> {
    let archive = target_dir.join("dwarfdump.tar.xz");
    download_file(DWARFDUMP_URL, &archive)?;

    // Extract
    run_command(
        "tar",
        &[
            "-xf",
            &archive.to_string_lossy(),
            "-C",
            &target_dir.to_string_lossy(),
        ],
    )?;

    // Find the binary
    let dd_binary = find_file_recursive(target_dir, "llvm-dwarfdump")
        .ok_or_else(|| "llvm-dwarfdump binary not found after extraction".to_string())?;

    let target = target_dir.join("llvm-dwarfdump");
    if dd_binary != target {
        std::fs::rename(&dd_binary, &target).map_err(|e| format!("rename dwarfdump: {}", e))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod dwarfdump: {}", e))?;
    }

    let _ = std::fs::remove_file(&archive);

    println!("  llvm-dwarfdump installed");
    Ok(())
}

/// Download a file from a URL to a local path using curl or wget.
fn download_file(url: &str, dest: &PathBuf) -> Result<(), String> {
    if find_in_path("curl").is_some() {
        run_command("curl", &["-fsSL", "-o", &dest.to_string_lossy(), url])
    } else if find_in_path("wget").is_some() {
        run_command("wget", &["-q", "-O", &dest.to_string_lossy(), url])
    } else {
        return Err(format!(
            "no download tool found (curl or wget required).\n\
             Install manually:\n  wget {} -O {}\n  unzip -o {} -d {}/",
            url,
            dest.display(),
            dest.display(),
            dest.parent().unwrap().display()
        ));
    }
}

/// Run a command and return Ok/Err with stderr on failure.
fn run_command(cmd: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|c| c.wait_with_output())
        .map_err(|e| format!("failed to run '{}': {}", cmd, e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("'{}' failed: {}", cmd, stderr.trim()))
    }
}

/// Check if a program exists on PATH.
fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Find a file by name recursively under a directory.
fn find_file_recursive(dir: &PathBuf, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries {
        let path = entry.ok()?.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}
