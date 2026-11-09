//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Memory-defect campaign: compile generated ownership-heavy programs natively
//! and run them under ASan/LSan/UBSan (optionally Valgrind). See
//! docs/guides/memory-defect-campaign.md. This harness hunts *memory safety*,
//! not wrong values (that is deterministic_transitions.rs).
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use proptest::{
    prelude::*,
    strategy::BoxedStrategy,
    test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process::Command};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Case {
    family: u8,
    size: i64,
    depth: i64,
    iterations: i64,
}

fn cases(family: u8) -> BoxedStrategy<Case> {
    (1i64..512, 0i64..8, 1i64..64)
        .prop_map(move |(size, depth, iterations)| Case {
            family,
            size,
            depth,
            iterations,
        })
        .boxed()
}

/// Complete program per family. Every program ends `endprogram;` (exit 0) and
/// observables go to stdout, so a nonzero exit or a signal is a real anomaly.
///
/// Family 1 (`publish_into_parameter`) is a **standing finding** (2026-11-09):
/// it detects the open publish-into-incoming-parameter leak (BUGS.md). It is
/// kept out of the default family so `cargo test --all` stays green; the
/// nightly lane runs it with Valgrind and tolerates the known failure.
fn program(case: &Case) -> String {
    match case.family {
        0 => escape_from_callee(case.size, case.depth, demo_fault()),
        1 => publish_into_parameter(case.size, case.depth),
        2 => list_return_alias_mutate(case.size),
        3 => defn_image_into_state(case.size),
        4 => per_iteration_allocation(case.size, case.iterations),
        _ => panic!("unknown memory family {}", case.family),
    }
}

/// 2026-11-09: opt-in planted use-after-free. Proves the sanitizer stack
/// actually detects a memory defect, independent of real findings. Never set
/// in normal runs or CI.
fn demo_fault() -> bool {
    std::env::var_os("MEM_DEMO_FAULT").is_some()
}

fn escape_from_callee(size: i64, depth: i64, fault: bool) -> String {
    // A fault injects Free# before the read: a genuine heap-use-after-free.
    let injected = if fault { "    Free#(p);\n" } else { "" };
    format!(
        "defn make(depth: Int, size: Int) -> Ptr<Bit<8>> {{\n    when depth == 0 {{ term Malloc#(size + 8); }};\n    term make(depth-1, size);\n}};\n\
defn exercise(depth: Int, size: Int) -> Int {{\n    let p: Ptr<Bit<8>> = make(depth, size);\n    Store#(p, 7, 8);\n{injected}    term Load#(p, 8);\n}};\n\
let done: Bool = false;\nnode main [!done][done] {{\n    Print#(exercise({depth}, {size}));\n    done = true;\n    endprogram;\n}};\n"
    )
}

fn publish_into_parameter(size: i64, depth: i64) -> String {
    format!(
        "obj Holder {{ addr: Ptr<Bit<8>>; }};\n\
defn make(depth: Int, size: Int) -> Ptr<Bit<8>> {{\n    when depth == 0 {{ term Malloc#(size + 8); }};\n    term make(depth-1, size);\n}};\n\
defn publish(h: Holder, depth: Int, size: Int) -> Bool {{\n    h.addr = make(depth, size);\n    term true;\n}};\n\
defn exercise(depth: Int, size: Int) -> Int {{\n    let h: Holder = Holder {{ addr: 0 as Ptr<Bit<8>> }};\n    let ok: Bool = publish(h, depth, size);\n    Store#(h.addr, 9, 8);\n    term Load#(h.addr, 8);\n}};\n\
let done: Bool = false;\nnode main [!done][done] {{\n    Print#(exercise({depth}, {size}));\n    done = true;\n    endprogram;\n}};\n"
    )
}

fn list_return_alias_mutate(size: i64) -> String {
    format!(
        "defn build(n: Int) -> List<Int> {{\n    term [n, n+1, n+2];\n}};\n\
defn exercise(n: Int) -> Int {{\n    let xs: List<Int> = build(n);\n    xs[0] = n + 5;\n    let ys: List<Int> = xs;\n    ys[1] = n + 6;\n    term xs[0] + xs[1];\n}};\n\
let done: Bool = false;\nnode main [!done][done] {{\n    Print#(exercise({size}));\n    done = true;\n    endprogram;\n}};\n"
    )
}

fn defn_image_into_state(size: i64) -> String {
    format!(
        "obj Box {{ value: Int; extra: Int; }};\n\
defn make(n: Int) -> Box {{\n    let b: Box = Box {{ value: n, extra: n * 2 }};\n    term b;\n}};\n\
let stored: Box = make({size});\n\
let done: Bool = false;\nnode main [!done][done] {{\n    when stored.value + stored.extra != {} {{ endprogram 1; }};\n    Print#(stored.value + stored.extra);\n    done = true;\n    endprogram;\n}};\n",
        size * 3
    )
}

fn per_iteration_allocation(size: i64, iterations: i64) -> String {
    format!(
        "defn tick(i: Int, size: Int) -> Int {{\n    let buf: Ptr<Bit<8>> = Malloc#(size + 8);\n    Store#(buf, i, 8);\n    term Load#(buf, 8);\n}};\n\
let total: Int = 0;\nlet done: Bool = false;\nnode main [!done][done] {{\n    foreach index in 0..{iterations} {{ total = total + tick(index, {size}); }};\n    Print#(total);\n    done = true;\n    endprogram;\n}};\n"
    )
}

/// Detection profiles.
///
/// Methodology note (2026-11-09): clang does **not** instrument a `.ll` input
/// under `-fsanitize=*` — it only links the runtime — so an ASan-linked Briev
/// binary catches `free`-family errors and leaks (interceptor-based) but NOT
/// use-after-free *reads* or overflows. `opt -passes=asan` would add those, but
/// on the emitted module it only registered globals and set a
/// `nosanitize_address` module flag, leaving accesses unchecked. Valgrind needs
/// no instrumentation and catches access errors directly, so it is the primary
/// oracle here; ASan adds leak/free-error cover via interceptors.
#[derive(Clone, Copy, Debug)]
enum Profile {
    AsanO0,
    AsanO1,
    PlainO1,
}

impl Profile {
    fn name(self) -> &'static str {
        match self {
            Profile::AsanO0 => "asan-O0",
            Profile::AsanO1 => "asan-O1",
            Profile::PlainO1 => "plain-O1",
        }
    }

    fn cflags(self) -> &'static [&'static str] {
        match self {
            Profile::AsanO0 => &[
                "-O0", "-g", "-fsanitize=address", "-fno-omit-frame-pointer", "-fno-inline",
            ],
            Profile::AsanO1 => &[
                "-O1", "-g", "-fsanitize=address", "-fno-omit-frame-pointer", "-march=native",
            ],
            Profile::PlainO1 => &["-O1", "-g"],
        }
    }
}

fn checked(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|e| format!("{command:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{command:?}: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn link(profile: Profile, root: &Path, ll: &Path, binary: &Path) -> Result<(), String> {
    checked(
        Command::new("timeout")
            .arg("180")
            .arg("clang")
            .args(profile.cflags())
            .arg(ll)
            .arg(root.join("lib/runtime/briev_rt.c"))
            .args(["-lm", "-ldl", "-o"])
            .arg(binary),
    )
    .map(|_| ())
}

/// Valgrind is on by default for the small generated programs; set
/// `MEM_VALGRIND=0` to skip it.
fn valgrind_enabled() -> bool {
    !matches!(std::env::var("MEM_VALGRIND").as_deref(), Ok("0") | Ok("false"))
}

/// The corpus sweep opts into Valgrind only when asked (`MEM_VALGRIND=1`), so
/// `cargo test --all` stays bounded on the larger fixtures.
fn valgrind_explicit() -> bool {
    matches!(std::env::var("MEM_VALGRIND").as_deref(), Ok("1") | Ok("true"))
}

/// Markers that mean a memory/UB defect, not a normal result.
const MARKERS: &[&str] = &[
    "AddressSanitizer",
    "LeakSanitizer",
    "runtime error",
    "SUMMARY: UndefinedBehaviorSanitizer",
    "ERROR: MemorySanitizer",
];

fn run_profile(profile: Profile, binary: &Path, require_zero_exit: bool) -> Result<(), String> {
    let output = Command::new("timeout")
        .arg("60")
        .arg(binary)
        .env("ASAN_OPTIONS", "detect_leaks=1:detect_stack_use_after_return=1:abort_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .env("LSAN_OPTIONS", "exitcode=23")
        .output()
        .map_err(|e| format!("run {}: {e}", profile.name()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    for marker in MARKERS {
        if stderr.contains(marker) {
            return Err(format!(
                "{}: memory defect ({marker})\nstatus={}\n{stderr}",
                profile.name(),
                output.status
            ));
        }
    }
    if let Some(signal) = signal_of(&output.status) {
        return Err(format!("{}: killed by signal {signal}\n{stderr}", profile.name()));
    }
    if require_zero_exit && !output.status.success() {
        return Err(format!("{}: exit {}\n{stderr}", profile.name(), output.status));
    }
    Ok(())
}

fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

fn valgrind(binary: &Path) -> Result<(), String> {
    if Command::new("valgrind").arg("--version").output().is_err() {
        return Ok(());
    }
    let output = Command::new("timeout")
        .arg("180")
        .arg("valgrind")
        .args([
            "--error-exitcode=91",
            "--leak-check=full",
            "--errors-for-leak-kinds=definite,indirect",
            "--track-origins=yes",
        ])
        .arg(binary)
        .output()
        .map_err(|e| format!("valgrind: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "valgrind: exit {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn compare(case: &Case, directory: &Path) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = program(case);
    let path = directory.join("memory.bv");
    fs::write(&path, &source).map_err(|e| e.to_string())?;
    checked(
        Command::new("timeout")
            .arg("60")
            .arg(env!("CARGO_BIN_EXE_brievc"))
            .current_dir(root)
            .env("BRIEV_HOME", root)
            .arg("build")
            .arg(&path)
            .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
            .arg(directory),
    )?;
    let ll = directory.join("memory.ll");
    let mut profiles = vec![Profile::AsanO0, Profile::AsanO1];
    if valgrind_enabled() {
        profiles.push(Profile::PlainO1);
    }
    for profile in profiles {
        let binary = directory.join(format!("memory-{}", profile.name()));
        link(profile, root, &ll, &binary)?;
        if matches!(profile, Profile::PlainO1) {
            valgrind(&binary)?;
            continue;
        }
        run_profile(profile, &binary, true)?;
    }
    Ok(())
}

fn exercise(case: &Case, artifact: &Path) -> Result<(), TestCaseError> {
    let directory = tempfile::tempdir().map_err(|e| TestCaseError::fail(e.to_string()))?;
    if let Err(error) = compare(case, directory.path()) {
        save_artifact(case, directory.path(), artifact, &error);
        return Err(TestCaseError::fail(error));
    }
    Ok(())
}

fn save_artifact(case: &Case, directory: &Path, artifact: &Path, error: &str) {
    fs::create_dir_all(artifact).expect("create memory artifact directory");
    fs::write(artifact.join("case.json"), serde_json::to_vec_pretty(case).unwrap()).unwrap();
    fs::write(artifact.join("failure.txt"), error).unwrap();
    for name in ["memory.bv", "memory.ll"] {
        let source = directory.join(name);
        if source.exists() {
            let _ = fs::copy(source, artifact.join(name));
        }
    }
}

fn artifact_dir() -> std::path::PathBuf {
    std::env::var_os("MEM_ARTIFACTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/memory-failure"))
}

fn replay(path: &std::path::PathBuf, artifact: &Path) {
    let case: Case = serde_json::from_slice(&fs::read(path).expect("read replay case"))
        .expect("decode replay case");
    assert!((1..512).contains(&case.size), "replay size 1..=511");
    assert!((0..8).contains(&case.depth), "replay depth 0..=7");
    assert!((1..64).contains(&case.iterations), "replay iterations 1..=63");
    assert!(case.family <= 4, "family 0..=4");
    exercise(&case, artifact).unwrap();
}

#[test]
fn generated_memory_programs_are_memory_safe() {
    let artifact = artifact_dir();
    if let Some(path) = std::env::var_os("MEM_REPLAY") {
        replay(&std::path::PathBuf::from(path), &artifact);
        return;
    }
    let seed: u64 = std::env::var("MEM_SEED").unwrap_or_else(|_| "1".into()).parse().expect("seed");
    let family: u8 = std::env::var("MEM_FAMILY").unwrap_or_else(|_| "0".into()).parse().expect("family");
    assert!(family <= 4, "family 0..=4");
    let count: u32 = std::env::var("MEM_CASES").unwrap_or_else(|_| "4".into()).parse().expect("count");
    assert!(count > 0, "count must be positive");
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let config = Config {
        cases: count,
        max_shrink_iters: 24,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner = TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    eprintln!("memory family={family} seed={seed} cases={count}; failures: {}", artifact.display());
    let result = runner.run(&cases(family), |case| exercise(&case, &artifact));
    if let Err(proptest::test_runner::TestError::Fail(_, case)) = &result {
        let _ = exercise(case, &artifact);
    }
    result.unwrap();
}

#[test]
fn memory_campaign_families_generate_terminating_programs() {
    for family in 0..=4 {
        let case = Case { family, size: 16, depth: 2, iterations: 3 };
        let source = program(&case);
        assert!(source.contains("endprogram;"), "family {family} terminates");
        assert!(!source.contains("todo"), "family {family} is concrete");
    }
}

/// Breadth: existing ownership-heavy fixtures under the same sanitizer stack.
/// Exit codes are ignored here (fixtures use them as result channels); only
/// sanitizer markers and signals count.
const MEMORY_FIXTURES: &[&str] = &[
    "tests/fixtures/load/reclamation_aliases.bv",
    "tests/fixtures/load/reclamation_bounded.bv",
    "tests/fixtures/load/structured_lifetimes.bv",
    "tests/fixtures/load/tuple_values.bv",
    "tests/fixtures/load/file_descriptor_lifetime.bv",
];

#[test]
fn existing_memory_fixtures_are_sanitizer_clean() {
    let artifact = artifact_dir();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for fixture in MEMORY_FIXTURES {
        let directory = tempfile::tempdir().expect("tempdir");
        checked(
            Command::new("timeout")
                .arg("120")
                .arg(env!("CARGO_BIN_EXE_brievc"))
                .current_dir(root)
                .env("BRIEV_HOME", root)
                .arg("build")
                .arg(root.join(fixture))
                .args(["--stdlib-path", "lib", "--config-dir", "config", "--out"])
                .arg(directory.path()),
        )
        .unwrap_or_else(|e| panic!("{fixture}: build failed:\n{e}"));
        let stem = Path::new(fixture).file_stem().unwrap().to_string_lossy().into_owned();
        let ll = directory.path().join(format!("{stem}.ll"));
        let mut profiles = vec![Profile::AsanO0, Profile::AsanO1];
        if valgrind_explicit() {
            profiles.push(Profile::PlainO1);
        }
        for profile in profiles {
            let binary = directory.path().join(format!("{stem}-{}", profile.name()));
            link(profile, root, &ll, &binary).unwrap_or_else(|e| panic!("{fixture}: link failed:\n{e}"));
            let outcome = if matches!(profile, Profile::PlainO1) {
                valgrind(&binary)
            } else {
                run_profile(profile, &binary, false)
            };
            if let Err(error) = outcome {
                fs::create_dir_all(&artifact).ok();
                fs::write(artifact.join(format!("{stem}.failure.txt")), &error).ok();
                let _ = fs::copy(root.join(fixture), artifact.join(format!("{stem}.bv")));
                panic!("{fixture}: {error}");
            }
        }
    }
}
