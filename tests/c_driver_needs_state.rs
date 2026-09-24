// ── Compiler-in-Briv: needs_state transition test ─────────────────────
// 2026-08-04 (plan 2026-08-04-compiler-in-briev-dogfood-ffi): the Briv pass
// lib/compiler/needs_state.bv must produce the SAME needs_state bitmask as the
// Rust reference (compute_export_needs_state) on a corpus of bridges. This is
// the P4 behavioral gate: Rust serializes the projection, the linked Briv
// library computes the mask, the test asserts equality with the reference.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn needs_state_pass_matches_reference() {
    for tool in ["cc"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_needs_state_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    // Build the Briv pass as a library (the compiler-in-Briv payload).
    let pass_bv = format!("{}/lib/compiler/needs_state.bv", PROJECT_ROOT);
    let build = Command::new(brievc)
        .args(["build", &pass_bv, "--library", "--out", &out_dir.to_string_lossy()])
        .output().expect("failed brievc build --library");
    assert!(build.status.success(), "pass build failed: {}", String::from_utf8_lossy(&build.stderr));

    // Corpus: bridges with and without state. Each entry is (file, expected
    // bitmask where bit i = export[i] (sorted by name) needs state).
    let corpus: Vec<(&str, i64)> = vec![
        ("examples/glue-host/boundary.bv", 0),
        ("examples/glue-host/node_bridge.bv", 31),
        ("examples/glue-host/cancel.bv", 1),
        ("examples/glue-host/rank.bv", 2),
        ("examples/glue-host/bench.bv", 2),
    ];

    // Serialize each bridge, embed the projection in a C driver, link against
    // the pass library, run, and compare to the reference mask.
    for (rel, expect) in &corpus {
        let src = format!("{}/{}", PROJECT_ROOT, rel);
        let source = std::fs::read_to_string(&src).unwrap();
        let (items, _u) = briev_compiler::library::parse_and_check(&src, &source).unwrap();
        let proj = briev_compiler::analysis::needs_state_projection::serialize_needs_state_projection(&items);
        let needs = briev_compiler::analysis::export_abi::compute_export_needs_state(&items);
        let mut exports: Vec<String> = needs.keys().cloned().collect();
        exports.sort();
        let mut ref_mask = 0i64;
        for (i, name) in exports.iter().enumerate() {
            if *needs.get(name).unwrap() { ref_mask |= 1 << i; }
        }
        assert_eq!(ref_mask, *expect, "reference mask drifted for {}", rel);

        // Embed the projection as a C string literal.
        let esc = proj.replace('\\', "\\\\").replace('"', "\\\"")
            .replace('\n', "\\n").replace('\r', "\\r");
        let driver_c = out_dir.join(format!("{}.c", rel.replace('/', "_")));
        std::fs::write(&driver_c, format!(r#"
#include <stdio.h>
long __briev_init_state(void);
long needs_state_compute(const char* s);
int main(void) {{
    long st = __briev_init_state();
    const char* p = "{}";
    printf("%ld\n", needs_state_compute(p));
    return 0;
}}
"#, esc)).unwrap();

        let driver = out_dir.join(format!("{}.bin", rel.replace('/', "_")));
        let cc = Command::new("cc")
            .arg("-o").arg(&driver).arg(&driver_c)
            .arg(out_dir.join("libneeds_state.a"))
            .output().expect("failed cc");
        assert!(cc.status.success(), "cc failed: {}", String::from_utf8_lossy(&cc.stderr));

        let run = Command::new(&driver).output().expect("failed to run driver");
        assert!(run.status.success(), "driver failed: {}", String::from_utf8_lossy(&run.stderr));
        let got: i64 = String::from_utf8_lossy(&run.stdout).trim().parse().unwrap();
        assert_eq!(got, *expect,
            "Briv needs_state pass mismatch for {} (got {}, expect {})", rel, got, expect);
    }

    let _ = std::fs::remove_dir_all(&out_dir);
}
