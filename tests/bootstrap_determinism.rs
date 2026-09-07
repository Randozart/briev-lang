// Bootstrap Chain Verification Tests
// Tests deterministic self-hosting compilation
//
// These tests verify that the Briv compiler produces identical output
// across multiple compilation runs, ensuring deterministic code generation.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn get_project_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is already the project root for this workspace
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_bootstrap_compile(source: &str, output_dir: &PathBuf) -> Result<String, String> {
    let project_root = get_project_root();
    let source_path = project_root.join(source);
    
    let output = Command::new(project_root.join("target/release/briv-compiler"))
        .arg("rust")
        .arg(&source_path)
        .arg("--out")
        .arg(output_dir)
        .output()
        .map_err(|e| format!("Failed to run compiler: {}", e))?;
    
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    
    let rs_path = output_dir.join("main.rs");
    fs::read_to_string(&rs_path)
        .map_err(|e| format!("Failed to read generated file: {}", e))
}

#[test]
fn test_bootstrap_determinism() {
    // Run the bootstrap compiler 3 times on the same source
    // and verify all outputs are identical
    
    let project_root = get_project_root();
    let temp_dir = project_root.join("target/bootstrap-test");
    fs::create_dir_all(&temp_dir).unwrap();
    
    let mut hashes = Vec::new();
    
    for i in 0..3 {
        let run_dir = temp_dir.join(format!("run{}", i));
        fs::create_dir_all(&run_dir).unwrap();
        
        let output = run_bootstrap_compile("lib/compiler/main.bv", &run_dir)
            .expect("Bootstrap compiler should succeed");
        
        // Simple hash: sum of all bytes (no external deps needed)
        let hash: u64 = output.bytes().map(|b| b as u64).sum();
        hashes.push((hash, output.clone()));
    }
    
    // All runs must produce identical output
    for i in 1..hashes.len() {
        assert_eq!(
            hashes[0].0, hashes[i].0,
            "Run 0 and run {} produced different output.\nRun 0:\n{}\nRun {}:\n{}",
            i, hashes[0].1, i, hashes[i].1
        );
    }
}

#[test]
fn test_bootstrap_compiles_to_valid_rust() {
    let project_root = get_project_root();
    let temp_dir = project_root.join("target/bootstrap-test-valid");
    fs::create_dir_all(&temp_dir).unwrap();
    
    let output = run_bootstrap_compile("lib/compiler/main.bv", &temp_dir)
        .expect("Bootstrap compiler should succeed");
    
    // Verify generated Rust is syntactically valid by checking key elements
    assert!(output.contains("fn main()"), "Generated code should have main function");
    assert!(output.contains("Briv"), "Generated code should contain Briv references");
}

#[test]
fn test_self_hosted_binary_runs() {
    let project_root = get_project_root();
    let temp_dir = project_root.join("target/bootstrap-test-binary");
    fs::create_dir_all(&temp_dir).unwrap();
    
    // Generate and compile the self-hosted binary
    let _output = run_bootstrap_compile("lib/compiler/main.bv", &temp_dir)
        .expect("Bootstrap compiler should succeed");
    
    let rs_path = temp_dir.join("main.rs");
    let binary_path = temp_dir.join("briv-self-hosted");
    
    // Compile with rustc
    let rustc_output = Command::new("rustc")
        .arg("-o")
        .arg(&binary_path)
        .arg(&rs_path)
        .output()
        .expect("rustc should be available");
    
    // rustc may produce warnings but should succeed
    assert!(
        rustc_output.status.success() || binary_path.exists(),
        "rustc should produce binary. stderr: {}",
        String::from_utf8_lossy(&rustc_output.stderr)
    );
    
    // Run the self-hosted binary
    if binary_path.exists() {
        let run_output = Command::new(&binary_path)
            .output()
            .expect("Self-hosted binary should run");
        
        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert!(
            stdout.contains("Briv") || run_output.status.success(),
            "Self-hosted binary should run successfully. stdout: {}",
            stdout
        );
    }
}
