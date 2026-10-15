// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

// xxHash parity — core-usable purge Phase 2 (plan
// 2026-10-15-core-usable-purge.md §Phase 2).
//
// lib/std/ffi/xxhash.bv is now PURE BRIEV (the two `frgn` bindings to the
// vendored C were removed). This test pins the rewrite byte-for-byte
// against the authoritative reference — the vendored C itself
// (lib/std/c/xxhash/xxhash.c, compiled by tests/vendored_c_libs.rs), whose
// one-shot XXH64/XXH32 values are the ground truth below.
//
// Method: compile a .bv program that hashes a fixed vector set, link with
// the harness-exact clang line, run, convert each printed signed decimal to
// its u64/u32 hex, and assert equality with the C-derived vector. A
// mismatch means the pure-Briev mixing/rotate/avalanche drifted from the
// spec — never "fix" a vector to match a bug.

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

// The .bv vector program: prints one hash per line, in VECTOR order.
const XXH_SRC: &str = r#"
import { xxh64, xxh32 } from "std/ffi/xxhash.bv";
let done: Int = 0;
node main [done == 0][done == 1] {
    println!(xxh64("", 0));
    println!(xxh64("a", 0));
    println!(xxh64("abc", 0));
    println!(xxh64("hello", 0));
    println!(xxh64("The quick brown fox jumps over the lazy dog", 0));
    println!(xxh64("", 1));
    println!(xxh32("", 0));
    println!(xxh32("a", 0));
    println!(xxh32("abc", 0));
    println!(xxh32("hello", 0));
    done = 1;
    term;
};
"#;

// (label, expected hex, bit-width) — values from the vendored C reference.
const VECTORS: &[(&str, &str, u32)] = &[
    ("XXH64(\"\",0)", "EF46DB3751D8E999", 64),
    ("XXH64(\"a\",0)", "D24EC4F1A98C6E5B", 64),
    ("XXH64(\"abc\",0)", "44BC2CF5AD770999", 64),
    ("XXH64(\"hello\",0)", "26C7827D889F6DA3", 64),
    ("XXH64(fox,0)", "0B242D361FDA71BC", 64),
    ("XXH64(\"\",1)", "D5AFBA1336A3BE4B", 64),
    ("XXH32(\"\",0)", "02CC5D05", 32),
    ("XXH32(\"a\",0)", "550D7456", 32),
    ("XXH32(\"abc\",0)", "32D153FF", 32),
    ("XXH32(\"hello\",0)", "FB0077F9", 32),
];

fn build_link_run() -> String {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT).join("target/xxhash_parity");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let src_path = out_dir.join("xxh_vectors.bv");
    std::fs::write(&src_path, XXH_SRC).unwrap();

    let build = Command::new(brievc)
        .args([
            "build",
            &src_path.to_string_lossy(),
            "--llvm",
            "--out",
            &out_dir.to_string_lossy(),
            "--stdlib-path",
            &format!("{PROJECT_ROOT}/lib/std"),
        ])
        .output()
        .expect("spawn brievc build");
    assert!(
        build.status.success(),
        "build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join("xxh_vectors.ll");
    let exe = out_dir.join("xxh_vectors");
    let link = Command::new("clang")
        .args([
            "-O3",
            "-flto",
            "-march=native",
            "-ffast-math",
            "-fdata-sections",
            "-ffunction-sections",
            "-Wl,--gc-sections",
            &ll.to_string_lossy(),
            &format!("{PROJECT_ROOT}/lib/runtime/briev_rt.c"),
            "-o",
            &exe.to_string_lossy(),
        ])
        .output()
        .expect("spawn clang");
    assert!(
        link.status.success(),
        "link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&exe).output().expect("run exe");
    String::from_utf8_lossy(&run.stdout).to_string()
}

#[test]
fn pure_briev_xxhash_matches_vendored_c() {
    let out = build_link_run();
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        VECTORS.len(),
        "expected {} hash lines, got {}: {out}",
        VECTORS.len(),
        lines.len()
    );

    for (i, (label, expected_hex, bits)) in VECTORS.iter().enumerate() {
        let got: i64 = lines[i].trim().parse().expect("line is an integer");
        let mask: u64 = if *bits == 32 { 0xFFFF_FFFF } else { u64::MAX };
        let got_hex = format!("{:0width$X}", (got as u64) & mask, width = (*bits / 4) as usize);
        assert_eq!(
            got_hex, *expected_hex,
            "{label}: pure-Briev xxhash drifted from the C reference"
        );
    }
}
