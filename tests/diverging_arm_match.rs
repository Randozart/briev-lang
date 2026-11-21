//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! A value `match` whose arm diverges must still terminate its merge block.
//!
//! 2026-11-20 (issue #148): `emit_match` reset `fun.terminated` at the top of
//! every arm but never restored it, so a match whose LAST arm diverged
//! returned `terminated = true`. The enclosing body loop took that as "stop
//! emitting" and broke out — leaving the merge block holding a phi with no
//! terminator and no continuation. LLVM rejected the module with "expected
//! instruction opcode" and the whole program failed to build.
//!
//! These tests build and RUN each shape. Compilation alone is the primary
//! assertion (the defect was an invalid module), but the printed value pins
//! that the code after the match actually survives.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A scratch directory unique to this process and test.
fn scratch(tag: &str) -> PathBuf {
    let unique = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "briev_diverging_arm_{}_{}_{}",
        std::process::id(),
        unique,
        tag
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Build a source string with `brievc` and return the produced binary path.
///
/// `BRIEV_HOME`/`CARGO_MANIFEST_DIR` point at the crate root so the stdlib and
/// config resolve exactly as they do for the tier1 corpus.
fn build(tag: &str, source: &str) -> PathBuf {
    let dir = scratch(tag);
    let source_path = dir.join(format!("{tag}.bv"));
    fs::write(&source_path, source).expect("write fixture");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .current_dir(root)
        .env("BRIEV_HOME", root)
        .args([
            "build",
            source_path.to_str().expect("utf-8 path"),
            "--stdlib-path",
            "lib",
            "--config-dir",
            "config",
            "--out",
        ])
        .arg(&dir)
        .output()
        .expect("start brievc build");
    assert!(
        output.status.success(),
        "build of {tag} failed (a diverging arm must not leave a dangling phi):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    dir.join(tag)
}

/// Run a freshly built binary and return its stdout.
fn run(tag: &str, source: &str, seed: i64) -> String {
    let binary = build(tag, source);
    let output = Command::new(&binary)
        .env("SEED", seed.to_string())
        .output()
        .expect("run fixture binary");
    assert!(
        output.status.success(),
        "{tag} exited {}:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

const RESULT_PRELUDE: &str = r#"
import "std/result.bv";

defn make(seed: Int) -> Result<Int, Int> {
    when seed > 0 {
        term Ok(seed);
    };
    term Err(0 - seed);
};
"#;

#[test]
fn diverging_last_arm_keeps_continuation() {
    // The reported shape: the LAST arm diverges. Before the fix the phi-merge
    // block was emitted without a terminator and the continuation was lost.
    let src = format!(
        "{RESULT_PRELUDE}\n\
         node entry [beginprogram][true] {{\n\
             let r: Result<Int, Int> = make(get_env_int!(\"SEED\"));\n\
             let value: Int = match r {{\n\
                 Ok(n) => n,\n\
                 Err(e) => {{ Print#(\"err\"); endprogram Print#(\"\"); }},\n\
             }};\n\
             Print#(value + 1);\n\
             endprogram Print#(\"\");\n\
         }};\n"
    );
    assert_eq!(run("diverging_last_arm", &src, 7), "8");
}

#[test]
fn diverging_first_arm_keeps_continuation() {
    // Arm order flipped: the diverging arm is emitted FIRST, and a later arm
    // resets `terminated`. This shape already worked; it pins that the fix did
    // not make it worse.
    let src = format!(
        "{RESULT_PRELUDE}\n\
         node entry [beginprogram][true] {{\n\
             let r: Result<Int, Int> = make(get_env_int!(\"SEED\"));\n\
             let value: Int = match r {{\n\
                 Err(e) => {{ Print#(\"err\"); endprogram Print#(\"\"); }},\n\
                 Ok(n) => n,\n\
             }};\n\
             Print#(value + 1);\n\
             endprogram Print#(\"\");\n\
         }};\n"
    );
    assert_eq!(run("diverging_first_arm", &src, 7), "8");
}

#[test]
fn diverging_middle_arm_keeps_continuation() {
    // Three arms, the diverging one in the middle: the arm after it resets the
    // flag as well, so the merge is reached from two sides.
    let src = r#"
node entry [beginprogram][true] {
    let pick: Int = get_env_int!("SEED");
    let value: Int = match pick {
        0 => { Print#("zero"); endprogram Print#(""); },
        1 => 11,
        _ => 22,
    };
    Print#(value);
    endprogram Print#("");
};
"#;
    assert_eq!(run("diverging_middle_arm", src, 2), "22");
}

#[test]
fn all_arms_diverging_emits_unreachable() {
    // Every arm diverges: the match itself never falls through, so the merge
    // block must be `unreachable` (no phi). This path was already correct and
    // must stay correct — it is the case the pre-fix code handled explicitly.
    let src = format!(
        "{RESULT_PRELUDE}\n\
         node entry [beginprogram][true] {{\n\
             let r: Result<Int, Int> = make(get_env_int!(\"SEED\"));\n\
             let _value: Void = match r {{\n\
                 Ok(n) => {{ Print#(\"ok\"); endprogram Print#(\"\"); }},\n\
                 Err(e) => {{ Print#(\"err\"); endprogram Print#(\"\"); }},\n\
             }};\n\
             Print#(\"unreachable\");\n\
             endprogram Print#(\"\");\n\
         }};\n"
    );
    assert_eq!(run("all_arms_diverging", &src, 5), "ok");
}

#[test]
fn diverging_arm_then_term_emits_once() {
    // A `term` after the match must be emitted exactly once, not swallowed by
    // a stale flag and not duplicated by the restore.
    let src = format!(
        "{RESULT_PRELUDE}\n\
         defn pick(seed: Int) -> Int {{\n\
             let r: Result<Int, Int> = make(seed);\n\
             let value: Int = match r {{\n\
                 Ok(n) => n,\n\
                 Err(e) => {{ Print#(\"err\"); endprogram Print#(\"\"); }},\n\
             }};\n\
             term value + 5;\n\
         }};\n\
         node entry [beginprogram][true] {{\n\
             Print#(pick(get_env_int!(\"SEED\")));\n\
             endprogram Print#(\"\");\n\
         }};\n"
    );
    assert_eq!(run("diverging_then_term", &src, 3), "8");
}

#[test]
fn diverging_arm_inside_guard_body_keeps_outer_continuation() {
    // Nested: the match sits inside a `when` body, so the same flag has two
    // owners. A stale value there breaks out of the OUTER body, silently
    // dropping the statements after the guard - hence the trailing `end` in
    // the expected output, which is the part the old code lost.
    let src = format!(
        "{RESULT_PRELUDE}\n\
         node entry [beginprogram][true] {{\n\
             let seed: Int = get_env_int!(\"SEED\");\n\
             when seed > 0 {{\n\
                 let r: Result<Int, Int> = make(seed);\n\
                 let value: Int = match r {{\n\
                     Ok(n) => n,\n\
                     Err(e) => {{ endprogram Print#(\"\"); }},\n\
                 }};\n\
                 Print#(value + 2);\n\
             }};\n\
             Print#(\"end\");\n\
             endprogram Print#(\"\");\n\
         }};\n"
    );
    assert_eq!(run("diverging_inside_guard", &src, 4), "6end");
}
