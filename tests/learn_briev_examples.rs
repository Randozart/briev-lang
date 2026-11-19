// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! learn-briev companion examples (2026-11-19, plan F5).
//!
//! Every companion in `learn-briev/examples/` is a COMPLETE program that
//! teaches the chapter's features, compiled, linked, run, and pinned against
//! its `<name>.golden` stdout. The tutorial chapters' inline blocks are
//! fragments (verified: none of the 92 chapter blocks parse as programs), so
//! these companions are the machine-checked face of the tutorial: doc rot in
//! the covered feature set fails CI here.
//!
//! Coverage contract: one companion per core chapter (01 basics, 04
//! functions, 05 data types, 06 strings) — each names its chapter in the
//! header and the chapter links to it. Add a companion when a chapter gains
//! runnable surface; this test scales by directory, not by list.

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");
const EXAMPLES: &str = "learn-briev/examples";

fn has(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Compile one companion with the shipped stdlib, link it, run it, and
/// compare stdout against its sidecar `.golden` (byte-exact).
fn build_link_run_pin(name: &str) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_learn_examples")
        .join(name);
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let fixture = Path::new(PROJECT_ROOT)
        .join(EXAMPLES)
        .join(format!("{name}.bv"));
    let build = Command::new(brievc)
        .args([
            "build",
            &fixture.to_string_lossy(),
            "--llvm",
            "--out",
            &out_dir.to_string_lossy(),
            "--stdlib-path",
            &format!("{PROJECT_ROOT}/lib/std"),
        ])
        .output()
        .expect("failed to spawn brievc build");
    assert!(
        build.status.success(),
        "{name}: build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join(format!("{name}.ll"));
    let exe = out_dir.join(name);
    let link = Command::new("clang")
        .args(["-O2", "-march=x86-64-v3"])
        .arg(&ll)
        .arg(format!("{PROJECT_ROOT}/lib/runtime/briev_rt.c"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to spawn clang");
    assert!(
        link.status.success(),
        "{name}: link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&exe)
        .output()
        .expect("failed to run the companion");
    assert!(
        run.status.success(),
        "{name}: run failed ({}): {}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let out = String::from_utf8_lossy(&run.stdout);
    let expect = std::fs::read_to_string(
        Path::new(PROJECT_ROOT)
            .join(EXAMPLES)
            .join(format!("{name}.golden")),
    )
    .unwrap_or_else(|_| panic!("{name}: missing {EXAMPLES}/{name}.golden"));
    assert_eq!(
        out, expect,
        "{name}: stdout drifted from {EXAMPLES}/{name}.golden — the tutorial \
         companion's documented behavior changed; fix the program AND the \
         chapter prose together"
    );
}

#[test]
fn learn_briev_companions_compile_run_and_match_goldens() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let dir = Path::new(PROJECT_ROOT).join(EXAMPLES);
    let mut companions: Vec<String> = std::fs::read_dir(&dir)
        .expect("learn-briev/examples must exist")
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension()? == "bv" {
                p.file_stem()?.to_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();
    companions.sort();
    assert!(
        companions.len() >= 4,
        "companion corpus shrank unexpectedly: {companions:?}"
    );
    for name in &companions {
        build_link_run_pin(name);
    }
    eprintln!(
        "learn-briev companions: {} ran and matched goldens",
        companions.len()
    );
}
