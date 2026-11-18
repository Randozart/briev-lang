// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-17 (issue #8): a value-form `term <val>` inside a `when` guard
//! in the SSA reactor main (a `node` body) must branch to the next-txn
//! label (`.ssn_<name>`) to skip the rest of this firing's body — it must
//! NOT emit `ret i64` into `define i32 @main`.
//!
//! Pre-fix, the `fn_ret_ty != "void"` arm in `emit_term_value_tail` fired
//! before the `void_txn_abort_label` arm (the reactor main carries
//! `fn_ret_ty = "i64"`), emitting `ret i64 <val>` into `define i32 @main`
//! — invalid IR (`opt -verify`: "value doesn't match function result type
//! 'i32'"). Post-fix, the `void_txn_abort_label` arm fires first, emitting
//! `br label %.ssn_<name>`.
//!
//! These tests verify the IR structure (not runtime output — the reactor
//! main's `ret i32 0` is only reachable when the node converges, and a
//! `term` that doesn't converge the node will re-fire, which is correct
//! TermReturn semantics):
//!
//!   * the build succeeds (pre-fix it failed at `opt -passes=verify`);
//!   * the emitted IR contains no `ret i64` in `define i32 @main`;
//!   * the guard.then block branches to `.ssn_main` (the next-txn label).

use std::process::Command;

/// Build the fixture to a temp dir and return the `.ll` path.
fn build_fixture(out_dir: &std::path::Path) -> (std::path::PathBuf, String) {
    let bv = format!(
        "{}/tests/fixtures/term_guard_value_reactor.bv",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args(["build", &bv, "--out", &out_dir.to_string_lossy()])
        .output()
        .expect("failed brievc build");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        build.status.success(),
        "build failed (pre-fix this fails at opt -passes=verify):\n{stderr}"
    );
    let ll = out_dir.join("term_guard_value_reactor.ll");
    let src = std::fs::read_to_string(&ll).expect("read .ll");
    (ll, src)
}

/// 2026-11-17 (issue #8): the emitted IR must not contain a `ret i64`
/// inside `define i32 @main` — that was the pre-fix invalid IR. The
/// reactor main's value-term now emits `br label %.ssn_<name>` instead.
#[test]
fn no_ret_i64_in_reactor_main() {
    // Unique per-test dir (the pid is shared across parallel tests in one
    // process; a thread-id suffix avoids a temp-dir race).
    let out_dir =
        std::env::temp_dir().join(format!("term_guard_reactor_{}_no_ret", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let (_, src) = build_fixture(&out_dir);
    // Extract the @main function body.
    let main_start = src
        .find("define i32 @main")
        .expect("must contain define i32 @main");
    let main_body = &src[main_start..];
    let main_end = main_body
        .find("\n}\n")
        .or_else(|| main_body.find("\n}"))
        .expect("must find @main closing brace");
    let main_fn = &main_body[..main_end + 1];
    // No `ret i64` anywhere in @main. (The stdlib helper defns elsewhere in
    // the module do emit `ret i64` — they are separate functions; only the
    // reactor main body is checked here.)
    assert!(
        !main_fn
            .lines()
            .any(|l| l.trim_start().starts_with("ret i64")),
        "regression: `ret i64` found in `define i32 @main` — the value-form \
         term in a node body must branch to .ssn_<name>, not emit `ret i64`. \n\n\
         @main body:\n{}",
        main_fn
    );
    let _ = std::fs::remove_dir_all(&out_dir);
}

/// 2026-11-17 (issue #8): the guard.then block in @main must branch to
/// `.ssn_main` (the next-txn label) — this is the TermReturn unwind that
/// skips the rest of the firing's body.
#[test]
fn guard_then_branches_to_next_txn() {
    let out_dir = std::env::temp_dir().join(format!(
        "term_guard_reactor_{}_guard_then",
        std::process::id()
    ));
    std::fs::create_dir_all(&out_dir).unwrap();
    let (_, src) = build_fixture(&out_dir);
    let main_start = src.find("define i32 @main").unwrap();
    let main_body = &src[main_start..];
    let main_end = main_body
        .find("\n}\n")
        .or_else(|| main_body.find("\n}"))
        .expect("must find @main closing brace");
    let main_fn = &main_body[..main_end + 1];
    // The guard.then block must contain `br label %.ssn_main`.
    assert!(
        main_fn.contains("br label %.ssn_main"),
        "regression: guard.then in @main does not branch to %.ssn_main — \
         the value-term must unwind to the next-txn label. \n\n@main body:\n{}",
        main_fn
    );
    let _ = std::fs::remove_dir_all(&out_dir);
}
