// Daily-use E2E gate — compile+link+run for the everyday-features corpus.
//
// 2026-10-12 (Increment 1, docs/plans/2026-10-12-daily-driver-quick-wins.md):
// the conformance sweep (src/conformance.rs) is parse+check only — nothing
// compiled AND ran tests/tier1/*.bv in CI (the fixtures were orphaned; two
// still carried pre-`when` syntax that never parsed). This is the missing
// day-one net (plan 2026-09-28 step 1): every fixture must either
//   (a) PROGRAM surface  — build (--llvm) + harness-exact clang link + run
//       to exit 0, or
//   (b) LIBRARY surface  — `export defn` and no node/txn entry: build via
//       `brievc lib` (the export surface, e.g. strexp/strexp2).
// The flagship (daily_use_smoke.bv) additionally pins golden stdout.
//
// Link command matches the benchmark harness exactly (the LTO lesson —
// -O3 -flto is the real pipeline; -O0 stays the IR-validity companion in
// backend unit tests, not here).

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

/// Program surface = the fixture declares a `node`/`txn` entry (comment
/// lines stripped so a mention in prose does not misclassify).
fn is_library_surface(src: &str) -> bool {
    let code: String = src
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    code.contains("export defn") && !code.contains("node ") && !code.contains("txn ")
}

/// Build + link + run one program-surface fixture. Returns stdout and asserts
/// every stage; `env` pairs are set on the run (not the build). `slot` makes
/// the out dir unique per caller — cargo runs tests in parallel, and two
/// callers sharing one exe path race (one hits "Text file busy").
fn build_link_run(name: &str, slot: &str, env: &[(&str, &str)]) -> String {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_daily_use_smoke")
        .join(format!("{name}__{slot}"));
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    let fixture = Path::new(PROJECT_ROOT)
        .join("tests/tier1")
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
        .unwrap_or_else(|e| panic!("{name}: failed to spawn brievc build: {e}"));
    assert!(
        build.status.success(),
        "{name}: build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join(format!("{name}.ll"));
    let exe = out_dir.join(name);
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
        .unwrap_or_else(|e| panic!("{name}: failed to spawn clang: {e}"));
    assert!(
        link.status.success(),
        "{name}: link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let mut run = Command::new(&exe);
    for (k, v) in env {
        run.env(k, v);
    }
    let run = run.output().unwrap_or_else(|e| panic!("{name}: failed to run: {e}"));
    assert!(
        run.status.success(),
        "{name}: run failed (exit {:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).to_string()
}

/// Library-surface fixture: `brievc lib` must produce the archive.
fn build_library(name: &str) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_daily_use_smoke")
        .join(format!("{name}_lib"));
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();
    let fixture = Path::new(PROJECT_ROOT)
        .join("tests/tier1")
        .join(format!("{name}.bv"));
    let build = Command::new(brievc)
        .current_dir(&out_dir)
        .args(["lib", &fixture.to_string_lossy()])
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to spawn brievc lib: {e}"));
    assert!(
        build.status.success(),
        "{name}: library build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        out_dir.join(format!("lib{name}.a")).exists(),
        "{name}: library archive not produced"
    );
}

#[test]
fn tier1_corpus_compiles_links_and_runs() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let dir = Path::new(PROJECT_ROOT).join("tests/tier1");
    let mut fixtures: Vec<String> = std::fs::read_dir(&dir)
        .expect("tests/tier1 must exist")
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension()? == "bv" {
                p.file_stem()?.to_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();
    fixtures.sort();
    assert!(
        fixtures.len() >= 12,
        "tier1 corpus shrank unexpectedly: {fixtures:?}"
    );

    let mut ran = 0;
    let mut lib = 0;
    for name in &fixtures {
        let src = std::fs::read_to_string(dir.join(format!("{name}.bv"))).unwrap();
        if is_library_surface(&src) {
            build_library(name);
            lib += 1;
        } else {
            // BOUND exercises the env lane (get_env_int!) where used.
            build_link_run(name, "corpus", &[("BOUND", "5")]);
            ran += 1;
        }
    }
    eprintln!("tier1 E2E: {ran} program fixtures ran, {lib} library fixtures built");
}

#[test]
fn daily_use_smoke_golden_output() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    // __print_str("world") (no newline) + println!("ready") — the flagship
    // exercises string ops, Stack inserts, env, arithmetic, when+println.
    let out = build_link_run("daily_use_smoke", "golden", &[("BOUND", "5")]);
    assert_eq!(out, "worldready\n", "flagship golden stdout drifted: {out:?}");
}

/// 2026-10-13 (trait-system branch, SPEC §8.6): static trait-constrained
/// generics, end to end through the monomorphizing backend. The fixture runs
/// all three resolution forms — a `<T: Counter>` generic, a trait default
/// called as a method, and the same default called as a free function — each
/// a tally of `bump_twice(_, 5)` = 10, summed = 30. Pins the printed result so
/// a codegen regression in trait-generic monomorphization is caught in CI.
/// 2026-10-19 (PR #23 pin): a module-level `const Name: String = "lit";`
/// was emitted as `@Name = constant ptr null` — every read dereferenced null
/// and segfaulted. The fix interns the literal into the @str.N table. The
/// fixture concats two const Strings and compares one against a literal, so
/// the golden output proves both reads dereference real storage.
#[test]
fn const_string_global_golden_output() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    // NOTE: this pin covers #23's const-global READ path only. String CONCAT
    // at -O2 is a separate, pre-existing miscompile (BUGS.md 2026-10-20) —
    // exercising it here would mask #23's verdict behind that bug.
    let out = build_link_run("const_string_global", "golden", &[]);
    assert_eq!(
        out, "hello worldhello!",
        "const String global stdout drifted: {out:?}"
    );
}

/// 2026-10-20 (BUGS.md "string concat miscompiles at -O2/-O3"): REGRESSION
/// GATE for the fixed bug. Any String concat was EMPTY under the pipeline
/// flags (-O2/-O3; -O0 correct): defns reaching a syscall through a callee
/// (`print_str` -> `write_all` -> `SysCall#`) were emitted argmemonly, so
/// LLVM deleted the buffer-filling stores. Fixed by making
/// `definition_touches_raw_memory` transitive (ctx.defn_touches_raw_memory).
#[test]
fn string_concat_at_o2_is_correct() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let out = build_link_run("string_concat_probe", "concat", &[]);
    assert_eq!(out, "abcd|abcd", "string concat at -O2: {out:?}");
}

/// 2026-10-20 (PR #26 reconciliation): canonical net-stack answers through
/// the compiled pipeline — Base64, the RFC 6455 accept key (SHA-1 + Base64),
/// incremental HTTP parse, and the websocket conveniences over the unified
/// codec. Base64 is compiled-path only (interpreter callable-txn gap).
/// 2026-10-20 (workstream C): pooled object globals — runtime initializer
/// seeding, per-instance pool rows, and boxed argument passing.
#[test]
fn obj_globals_golden_output() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let out = build_link_run("obj_globals", "golden", &[]);
    assert_eq!(out, "3|8|9|1|4", "pooled object globals drifted: {out:?}");
}

#[test]
fn net_stack_helpers_golden_output() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let out = build_link_run("net_stack_helpers", "golden", &[]);
    assert_eq!(
        out, "aGVsbG8=|s3pPLMBiTxaQ9kYGzzhZRbK+xOo=|OK",
        "net-stack helper golden drifted: {out:?}"
    );
}

#[test]
fn trait_generics_golden_output() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let out = build_link_run("trait_generics_smoke", "golden", &[]);
    assert_eq!(out, "30", "trait-generics golden stdout drifted: {out:?}");
}

/// 2026-10-15 (fullstack-webapp net stack, design D): the ByteSink trait end
/// to end. lib/std/net/bytes.bv shares its derived network-byte-order writers
/// (put_u16_be/put_cstr) as trait DEFAULTS over required primitives (put_u8/
/// put_bytes), implemented by a FIXED ByteBuf and a GROWING GrowBuf. A single
/// `write_tagged<S: ByteSink>` writes a u16-BE tag 0x1234 (18, 52) then a
/// NUL-terminated "Hi" (72, 105, 0) into BOTH sinks and reads the bytes back —
/// the GrowBuf starting at cap 2 so the 5-byte frame forces a mid-write
/// realloc. Both sinks yield identical bytes, so the digit-by-digit print is
/// "18527210501852721050". Pins that trait-default monomorphization onto a
/// stateful growing buffer (reached through a generic) stays correct — a codegen
/// regression in the member liveness or type-dependent needs-state paths is
/// caught in CI.
#[test]
fn byte_sink_trait_golden_output() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let out = build_link_run("byte_sink_trait_smoke", "golden", &[]);
    assert_eq!(
        out, "18527210501852721050",
        "ByteSink trait golden stdout drifted: {out:?}"
    );
}

/// 2026-10-18 (fullstack-webapp net stack, L2): the pure-Briev TCP transport
/// (lib/std/net/socket.bv — raw syscalls, no libc) end to end over loopback:
/// bind+listen (SO_REUSEADDR) → connect → accept → send one byte through the
/// MSG_NOSIGNAL partial-transfer loop → recv → close. Golden = five ok-flags
/// (bind_listen, connect, accept, send, recv), the received byte (65 = 'A'),
/// then the whole-trip verdict: "11111651". Pins the client AND server paths
/// the Postgres client (L3) and HTTP server (L4) will sit on; a codegen
/// regression in the syscall wrapper, the sockaddr_in byte layout, or the
/// send/recv loops is caught in CI instead of as a hung connection.
#[test]
fn socket_loopback_golden_output() {
    if !has("clang") {
        eprintln!("SKIP: clang not available");
        return;
    }
    let out = build_link_run("socket_loopback_smoke", "golden", &[]);
    assert_eq!(
        out, "11111651",
        "socket loopback golden stdout drifted: {out:?}"
    );
}
