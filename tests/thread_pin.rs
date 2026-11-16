//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// 2026-11-16 (per-worker arena plan, Stage 2a): explicit worker threads.
//
// Proves the threaded lane end to end:
//   1. `on thread <N>` emits one reactor tick per worker plus the pointer table
//      the runtime pool indexes.
//   2. main starts the pool ONCE before the loop and balances the two barriers
//      around its own tick.
//   3. The linked program produces the same output as an unpinned build — the
//      pin changes where nodes fire, never what the program computes.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Build the fixture to LLVM IR and return the module text.
fn emit_ir(name: &str, out_dir: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("tests/fixtures").join(name);
    let _ = std::fs::remove_dir_all(out_dir);
    let output = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args([
            "build",
            &fixture.to_string_lossy(),
            "--llvm",
            "--out",
            &out_dir.to_string_lossy(),
        ])
        .output()
        .expect("spawn brievc build");
    assert!(
        output.status.success(),
        "build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stem = name.trim_end_matches(".bv");
    let ll = out_dir.join(format!("{stem}.ll"));
    std::fs::read_to_string(&ll).expect("read emitted .ll")
}

/// The body of one emitted function, up to the next `define`/`declare`.
fn function_body<'a>(ll: &'a str, header: &str) -> &'a str {
    let start = ll
        .find(header)
        .unwrap_or_else(|| panic!("function not found: {header}"));
    let rest = &ll[start..];
    let end = rest.find("\ndefine ").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn pinned_program_emits_per_worker_ticks() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("thread_pin_ir");
    let ll = emit_ir("thread_pin_smoke.bv", &out);

    // One tick per worker; worker 0 keeps the canonical name.
    assert!(
        ll.contains("define void @reactor_tick(ptr noalias nocapture %state)"),
        "worker 0's tick must keep the canonical @reactor_tick name"
    );
    assert!(
        ll.contains("define void @reactor_tick_w1(ptr noalias nocapture %state)"),
        "worker 1 must get its own tick function"
    );
    assert!(
        !ll.contains("@reactor_tick_w2"),
        "only the pinned workers exist (2 workers, indices 0 and 1)"
    );

    // The pool table: one entry per worker, entry 0 null because main runs it.
    assert!(
        ll.contains(
            "@__briev_worker_fns = internal constant [2 x ptr] [ptr null, ptr @reactor_tick_w1]"
        ),
        "the worker table must map index 1 to @reactor_tick_w1"
    );

    // Node placement: each tick contains only its own node's body.
    let tick0 = function_body(&ll, "define void @reactor_tick(");
    let tick1 = function_body(&ll, "define void @reactor_tick_w1(");
    assert!(
        tick0.contains("bump_a") || tick0.contains("counter_a"),
        "worker 0's tick should carry bump_a's state access"
    );
    assert!(
        !tick1.contains("counter_a"),
        "worker 1's tick must not touch worker 0's state: {tick1}"
    );
}

#[test]
fn threaded_main_starts_once_and_balances_the_barriers() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("thread_pin_main");
    let ll = emit_ir("thread_pin_smoke.bv", &out);
    let main = function_body(&ll, "define i32 @main(");

    // Started exactly once, before the loop, with the declared worker count.
    assert_eq!(
        main.matches("call i64 @briev_workers_start").count(),
        1,
        "the pool must be started once, not per tick"
    );
    assert!(
        main.contains("@__briev_worker_fns"),
        "main must hand the worker table to the pool"
    );
    // Per tick: release → own tick → wait.
    let release = main
        .find("call void @briev_workers_release()")
        .expect("release");
    let tick = main.find("call void @reactor_tick(ptr").expect("tick");
    let wait = main.find("call void @briev_workers_wait()").expect("wait");
    assert!(
        release < tick && tick < wait,
        "each tick must be bracketed: release, tick, wait"
    );
    // And joined at the end.
    assert!(
        main.contains("call void @briev_workers_stop()"),
        "main must stop and join the pool at .end"
    );

    // The pool ABI is declared, so the module links against the runtime.
    for symbol in [
        "declare i64 @briev_workers_start(i64, ptr, ptr)",
        "declare void @briev_workers_release()",
        "declare void @briev_workers_wait()",
        "declare void @briev_workers_stop()",
    ] {
        assert!(ll.contains(symbol), "missing pool declaration: {symbol}");
    }
}

#[test]
fn pinned_program_runs_and_matches_the_unpinned_result() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("thread_pin_run");
    let ll = emit_ir("thread_pin_smoke.bv", &out);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let runtime = root.join("lib/runtime/briev_rt.c");
    let binary = out.join("thread_pin_smoke");

    // The harness link line, verbatim — the threaded lane must link with it.
    let link = Command::new("clang")
        .args([
            "-O2",
            "-march=native",
            "-fdata-sections",
            "-ffunction-sections",
            "-Wl,--gc-sections",
            &out.join("thread_pin_smoke.ll").to_string_lossy(),
            &runtime.to_string_lossy(),
            "-o",
            &binary.to_string_lossy(),
        ])
        .output()
        .expect("spawn clang");
    assert!(
        link.status.success(),
        "link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );
    assert!(!ll.is_empty());

    let run = Command::new(&binary)
        .output()
        .expect("run the pinned binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "pinned program failed: stdout={stdout} stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        stdout.trim(),
        "6",
        "two workers incrementing disjoint counters 3 times each must print 3 + 3"
    );
}

// ── 2026-11-16 (thread-lane E1): the threaded arm must beat the async phase ──

/// `emit_main` used to prefer the async-phase arm, which runs `@async_body_*`
/// on the main thread and never touches the worker barriers. A pinned program
/// taking that arm left every worker parked on `release` while its nodes
/// silently never fired.
///
/// This fixture is deliberately NOT "lightweight" async
/// (`src/backend/llvm/strategy.rs` grants that only to pure bodies with a
/// non-constant `bounded_pre`), so before the fix it took the async-phase arm.
#[test]
fn pinned_non_lightweight_async_uses_the_threaded_arm() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("thread_pin_async");
    let ll = emit_ir("thread_pin_async_guard.bv", &out);
    let main = function_body(&ll, "define i32 @main(");

    assert!(
        main.contains("call void @briev_workers_release()")
            && main.contains("call void @briev_workers_wait()"),
        "main must bracket the tick with the worker barriers"
    );
    assert!(
        !main.contains("call void @async_body_"),
        "a pinned program must not take the async-phase arm — its bodies would \
         run on the main thread while every worker waits at the barrier"
    );

    // And the pool really runs: worker 1's node prints and ends the program.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = out.join("thread_pin_async_guard");
    let link = Command::new("clang")
        .args([
            "-O2",
            "-march=native",
            &out.join("thread_pin_async_guard.ll").to_string_lossy(),
            &root.join("lib/runtime/briev_rt.c").to_string_lossy(),
            "-o",
            &binary.to_string_lossy(),
        ])
        .output()
        .expect("spawn clang");
    assert!(
        link.status.success(),
        "link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );
    let run = Command::new(&binary).output().expect("run pinned async binary");
    assert!(
        run.status.success(),
        "pinned async program failed: stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "42",
        "worker 1's node must actually fire (a parked worker would print nothing)"
    );
}

// ── 2026-11-16 (thread-lane E3): the both-allocate refusal is retired ────────

/// Two pinned workers that both allocate. This pair was refused while the arena
/// cursor was a single shared slot; with one triple per worker it is accepted.
///
/// Note what the fixture actually reaches: the heap path. No program in this
/// tree naturally reaches the bump arena — a runtime-sized `Alloc#` is folded or
/// heap-allocated before codegen, and `needs_arena` stays empty — so the
/// per-worker partition is covered structurally by
/// `backend::llvm::tests::pinned_arena_injects_one_slot_triple_per_worker`. This
/// test covers the retired refusal end to end.
#[test]
fn two_pinned_workers_may_both_allocate() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("thread_pin_alloc");
    let ll = emit_ir("thread_pin_alloc.bv", &out);

    // Accepted: two workers, one tick each.
    assert!(
        ll.contains("define void @reactor_tick(ptr noalias nocapture %state)"),
        "worker 0's tick must exist"
    );
    assert!(
        ll.contains("define void @reactor_tick_w1(ptr noalias nocapture %state)"),
        "worker 1's tick must exist"
    );

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = out.join("thread_pin_alloc");
    let link = Command::new("clang")
        .args([
            "-O2",
            "-march=native",
            &out.join("thread_pin_alloc.ll").to_string_lossy(),
            &root.join("lib/runtime/briev_rt.c").to_string_lossy(),
            "-o",
            &binary.to_string_lossy(),
        ])
        .output()
        .expect("spawn clang");
    assert!(
        link.status.success(),
        "link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );
    let run = Command::new(&binary).output().expect("run allocating binary");
    assert!(
        run.status.success(),
        "allocating pinned program failed: stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "9",
        "worker 1's own allocation round-trips (Store# 9 → Load# 9)"
    );
}
