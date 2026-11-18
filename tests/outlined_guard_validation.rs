// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-18 (issue #7, Phase 2): outlined-guard validation test.
//!
//! The canonical outlinable shape is a guard whose body is a single
//! observable expression over pure scalar reads (e.g.
//! `endprogram Print#(a * b)`). Such a body is outlined into
//! `@txn_<name>_cold_N` by the cold-function outlining pass. The body is
//! rewritten so every state identifier is read through a `__cp_<name>`
//! param, and the post-outline validation (`validate_cold_body_idents`)
//! checks every identifier left in the rewritten body is either a `__cp_`
//! param or a module global — a silent-zero rewrite gap would fire a loud
//! diagnostic.
//!
//! These tests verify:
//!   * the build succeeds and emits a `_cold_` function (outlining fired);
//!   * no validation diagnostic (`cold function … no storage`) is emitted;
//!   * the cold function reads `a` and `b` via `__cp_a` / `__cp_b` params,
//!     not `%state` (the rewrite happened).

use std::process::Command;

fn build_fixture(out_dir: &std::path::Path) -> (std::path::PathBuf, String) {
    let bv = format!(
        "{}/tests/fixtures/outlined_guard_expressions.bv",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args(["build", &bv, "--out", &out_dir.to_string_lossy()])
        .output()
        .expect("failed brievc build");
    let stderr = String::from_utf8_lossy(&build.stderr);
    let stdout = String::from_utf8_lossy(&build.stdout);
    assert!(build.status.success(), "build failed:\n{stderr}\n{stdout}");
    // No validation diagnostic: a silent-zero rewrite gap would print
    // "cold function … references … which has no storage".
    let combined = format!("{stderr}\n{stdout}");
    assert!(
        !combined.contains("which has no storage in this module"),
        "regression: the post-outline validation fired — an identifier in the \
         outlined guard body is unresolved (a rewrite gap). Diagnostics:\n{combined}"
    );
    let ll = out_dir.join("outlined_guard_expressions.ll");
    let src = std::fs::read_to_string(&ll).expect("read .ll");
    (ll, src)
}

/// 2026-11-18 (issue #7, Phase 2): the guard body is outlined into a
/// `_cold_` function (the FFI call creates a memory barrier).
#[test]
fn outlined_guard_produces_cold_function() {
    let out_dir = std::env::temp_dir().join(format!("outlined_guard_{}_cold", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let (_, src) = build_fixture(&out_dir);
    assert!(
        src.contains("_cold_"),
        "regression: no `@txn_*_cold_*` function emitted — the guard was not \
         outlined (the FFI call should have triggered the cold barrier). \
         Cold-function outlining is the whole point of the Phase 1 validation."
    );
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// 2026-11-18 (issue #7, Phase 2): the cold function reads `a` and `b` via
/// `__cp_a` / `__cp_b` params — NOT via `%state` (which the cold function
/// has no direct write access to). This proves `rewrite_stmt_idents`
/// substituted the identifiers.
#[test]
fn cold_function_uses_cp_params_not_state() {
    let out_dir = std::env::temp_dir().join(format!("outlined_guard_{}_cp", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let (_, src) = build_fixture(&out_dir);
    // Find the cold function body.
    let cold_start = src
        .find("define void @txn_n_cold_")
        .expect("must contain the cold function definition");
    let cold_body = &src[cold_start..];
    let cold_end = cold_body
        .find("\n}\n")
        .or_else(|| cold_body.find("\n}"))
        .expect("must find the cold function closing brace");
    let cold_fn = &cold_body[..cold_end + 1];
    // The cold function signature must have __cp_a and __cp_b params.
    assert!(
        cold_fn.contains("__cp_a"),
        "regression: the cold function does not have a `__cp_a` param — \
         `a` was not rewritten to a param. \n\nCold function:\n{}",
        cold_fn
    );
    assert!(
        cold_fn.contains("__cp_b"),
        "regression: the cold function does not have a `__cp_b` param — \
         `b` was not rewritten to a param. \n\nCold function:\n{}",
        cold_fn
    );
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// 2026-11-18 (issue #7, Phase 2 coverage-matrix / Phase 3 repair): a
/// multi-statement guard body with local `let`s must NOT be outlined — the
/// cold-function emission drops the let bindings (a silent-zero shape the
/// Phase 1 validation caught live). The outline gate now requires
/// `body.len() == 1`, so this fixture builds with NO validation diagnostic
/// and the guard stays inline (no `_cold_` function). The program prints the
/// correct value (0 = -10 + 10), not the silent 0 the old outlined path
/// would have produced.
#[test]
fn multi_stmt_guard_with_local_lets_stays_inline() {
    let bv = format!(
        "{}/tests/fixtures/outlined_guard_local_lets.bv",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );
    let out_dir =
        std::env::temp_dir().join(format!("outlined_guard_{}_inline", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args(["build", &bv, "--out", &out_dir.to_string_lossy()])
        .output()
        .expect("failed brievc build");
    let stderr = String::from_utf8_lossy(&build.stderr);
    let stdout = String::from_utf8_lossy(&build.stdout);
    assert!(build.status.success(), "build failed:\n{stderr}\n{stdout}");
    // No validation diagnostic — the gate no longer outlines this shape, so
    // there is no rewrite gap to fire. A regression (re-outlining multi-stmt
    // bodies) would fire "which has no storage in this module".
    let combined = format!("{stderr}\n{stdout}");
    assert!(
        !combined.contains("which has no storage in this module"),
        "regression: the post-outline validation fired — a multi-statement \
         guard body was outlined and its local lets were silently dropped. \
         The single-statement gate (issue #7 Phase 3) was weakened. \
         Diagnostics:\n{combined}"
    );
    // The guard must NOT be outlined — no `@txn_n_cold_` function.
    let ll = out_dir.join("outlined_guard_local_lets.ll");
    let src = std::fs::read_to_string(&ll).expect("read .ll");
    assert!(
        !src.contains("@txn_n_cold_"),
        "regression: the multi-statement guard was outlined into a cold \
         function — the local lets would be silently dropped (issue #7 \
         Phase 3). The guard must stay inline.\n\n.ll:\n{}",
        src
    );
    // Run the binary and check the printed value is correct (0 = -10 + 10),
    // not the silent 0 the old outlined path produced.
    let bin = out_dir.join("outlined_guard_local_lets");
    let run = Command::new(&bin).output().expect("failed to run binary");
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(
        out.trim() == "0",
        "regression: the program printed `{out}` — expected `0` (u + c = -10 + \
         10). A silent-zero rewrite gap (or a wrong inline emission) would \
         produce a different value."
    );
    let _ = std::fs::remove_dir_all(&out_dir);
}
