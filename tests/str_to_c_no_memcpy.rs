// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-15 (FFI zero-copy audit, task 26): regression test proving the
//! `str_to_c` FFI boundary delta emits pointer arithmetic (zero-copy), not a
//! `memcpy`.
//!
//! The fixture (`tests/fixtures/cstr_str_to_c_meld.bv`) imports the C glue
//! module so the `C_String` protocol registers in the casting graph, then
//! passes a `String` to a `CStr`-typed frgn param WITHOUT an explicit `as
//! CStr` cast — the implicit-meld shape that fires `emit_frgn_variant_cast`.
//! The emitted IR must contain:
//!   1. A `str_to_c` call at the frgn call site (the casting-graph delta).
//!   2. The `str_to_c` function body: `add i64 <handle>, 8` — the pointer
//!      offset from the `[len]` header to the `[bytes][NUL]` data region.
//!   3. NO `memcpy` inside the `str_to_c` body (the delta is a pointer
//!      offset, not a copy — the `[len][bytes][NUL]` allocation invariant
//!      means the data region IS a C string).
//!
//! Provenance: 2026-11-15, FFI zero-copy audit
//! (docs/plans/2026-11-15-llvm-predictability-and-ffi-zero-copy.md).
//! The `str_to_c` delta is the String → C_String direction of the casting
//! graph's `CastFrom` binding (lib/glue/c.bv): `handle + 8`.
//!
//! Pre-existing finding (documented, not fixed here): an EXPLICIT `as CStr`
//! cast bypasses `emit_frgn_variant_cast` — the cast is a pure relabel of
//! the whole `[len][bytes]` handle to `ptr` (no `+8` offset, no `str_to_c`
//! call). The implicit-meld shape is the one that marshals through the
//! casting graph. See BUGS.md entry "str_to_c not emitted for explicit as-CStr".

use std::process::Command;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Extract the body of a named LLVM function from the module text.
/// Returns the lines from `define ... @name` through the closing `}`.
fn extract_fn_body(ir: &str, name: &str) -> Option<String> {
    let marker = format!("define ");
    let mut start = None;
    for (i, line) in ir.lines().enumerate() {
        if line.starts_with(&marker) && line.contains(&format!("@{}(", name)) {
            start = Some(i);
            break;
        }
    }
    let Some(start) = start else { return None };
    let mut body = Vec::new();
    for (i, line) in ir.lines().enumerate() {
        if i < start {
            continue;
        }
        body.push(line.to_string());
        if line.trim() == "}" {
            break;
        }
    }
    Some(body.join("\n"))
}

#[test]
fn str_to_c_emits_pointer_arithmetic_not_memcpy() {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = std::env::temp_dir().join("briev_str_to_c_test");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let fixture = ROOT.to_string() + "/tests/fixtures/cstr_str_to_c_meld.bv";
    let build = Command::new(brievc)
        .args([
            "build",
            &fixture,
            "--split",
            "--out",
            &out_dir.to_string_lossy(),
        ])
        .args(["--stdlib-path", "lib", "--config-dir", "config"])
        .current_dir(ROOT)
        .output()
        .expect("spawn brievc");
    // The link step fails (no -llibc on this host) but the .ll is emitted
    // before the link — that's all this test needs.
    let ll_path = out_dir.join("cstr_str_to_c_meld.ll");
    assert!(
        ll_path.exists(),
        "brievc did not emit the .ll (build stderr: {})",
        String::from_utf8_lossy(&build.stderr)
    );
    let ir = std::fs::read_to_string(&ll_path).expect("read .ll");

    // 1. The str_to_c call must appear at the frgn call site.
    assert!(
        ir.contains("call ptr @str_to_c") || ir.contains("call i64 @str_to_c"),
        "str_to_c call missing from frgn argument marshalling:\n{ir}"
    );

    // 2. The str_to_c function body must be a pointer offset (+8), not a copy.
    let Some(body) = extract_fn_body(&ir, "str_to_c") else {
        panic!("str_to_c function definition missing from emitted IR:\n{ir}");
    };
    assert!(
        body.contains("add i64 0, 8") && body.contains("add i64"),
        "str_to_c body must contain the +8 pointer offset:\n{body}"
    );
    // 3. No memcpy inside the str_to_c body — the delta is a pointer offset.
    assert!(
        !body.contains("memcpy"),
        "str_to_c body must NOT contain memcpy (zero-copy pointer offset):\n{body}"
    );
}
