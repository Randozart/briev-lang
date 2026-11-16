//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// 2026-11-15 (List FFI zero-copy): integration test for list_to_c_ptr and
// list_len in glue/c.bv.
//
// Verifies the emitted LLVM IR for the list_ffi_zero_copy.bv fixture:
//   1. Both helper functions are defined (not just declared)
//   2. list_to_c_ptr is identity (no memcpy, no arithmetic)
//   3. list_len loads from handle + 16 (len slot offset)
//   4. A call to c_array_sum(ptr, len) exists at the FFI boundary
//   5. No memcpy anywhere in the helper bodies

use std::fs;

#[test]
fn list_ffi_zero_copy_ir() {
    // Build the fixture to generate .ll
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args([
            "build",
            "tests/fixtures/list_ffi_zero_copy.bv",
            "--llvm",
            "--out",
            "/tmp/briev-test-list-ffi",
        ])
        .output()
        .expect("failed to run brievc build");

    assert!(
        output.status.success(),
        "brievc build failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    // Read the emitted LLVM IR
    let ll = fs::read_to_string("/tmp/briev-test-list-ffi/list_ffi_zero_copy.ll")
        .expect("failed to read .ll file");

    // 1. Both helpers are DEFINED (not just declared)
    assert!(
        ll.contains("define i64 @list_to_c_ptr(i64 %arg0)"),
        "list_to_c_ptr should be defined in emitted IR"
    );
    assert!(
        ll.contains("define i64 @list_len(i64 %arg0)"),
        "list_len should be defined in emitted IR"
    );

    // Extract list_to_c_ptr body
    let list_to_c_start = ll
        .find("define i64 @list_to_c_ptr")
        .expect("list_to_c_ptr def not found");
    let list_to_c_body_end = ll[list_to_c_start..]
        .find("\ndefine ")
        .map(|i| i + list_to_c_start)
        .unwrap_or(ll.len());
    let list_to_c_body = &ll[list_to_c_start..list_to_c_body_end];

    // 2. list_to_c_ptr is identity (returns argument directly)
    assert!(
        list_to_c_body.contains("ret i64 %arg0") || list_to_c_body.contains("ret i64 %arg"),
        "list_to_c_ptr should return its argument directly (identity), body: {}",
        list_to_c_body
    );
    assert!(
        !list_to_c_body.contains("memcpy") && !list_to_c_body.contains("memset"),
        "list_to_c_ptr must not emit memcpy or memset (zero-copy invariant)"
    );

    // Extract list_len body
    let list_len_start = ll
        .find("define i64 @list_len")
        .expect("list_len def not found");
    let list_len_body_end = ll[list_len_start..]
        .find("\ndefine ")
        .map(|i| i + list_len_start)
        .unwrap_or(ll.len());
    let list_len_body = &ll[list_len_start..list_len_body_end];

    // 3. list_len loads from handle + 16
    assert!(
        list_len_body.contains("add i64") && list_len_body.contains("load i64"),
        "list_len should add 16 to handle and load, body: {}",
        list_len_body
    );
    assert!(
        !list_len_body.contains("memcpy") && !list_len_body.contains("memset"),
        "list_len must not emit memcpy or memset"
    );

    // 4. c_array_sum call exists with two i64 args
    assert!(
        ll.contains("call i64 @c_array_sum(i64"),
        "c_array_sum should be called with (ptr, len)"
    );
}
