// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P3 baseline gate: the saltpetre scalar Gemm at the g0 geometry.
//!
//! Plan 2026-11-22-saltpetre-gemm-graduation.md, invariants 1 and 2. The
//! fixture `gemm_baseline.bv` builds a one-Gemm ONNX graph in source and
//! drives it through the real session path (`sp_run`), so what is measured is
//! saltpetre's `gemm` op (exec.bv), not a hand-copied kernel. The geometry is
//! g0_gemm's (512^3 f32, same closed-form inputs) so the three parties are
//! comparable. This test pins: it compiles, runs, and its spot-check against
//! an independent CPU dot reports BAD=0 and prints the wall-clock baseline.
//!
//! The fixture lives under `tests/saltpetre/` on purpose: that root is the
//! recorded exemption from the `deny` call-site gate in
//! config/contracts.dbvl (the saltpetre call sites are hardened by their own
//! campaign), and moving the fixture anywhere else would force widening the
//! exemption — which this plan forbids.

use std::process::Command;
use std::sync::OnceLock;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The fixture is compiled and run ONCE and shared *within a process*: it is
/// a timing probe, so two concurrent invocations would both corrupt the same
/// output directory and each pay the multi-second compile.
static FIXTURE: OnceLock<String> = OnceLock::new();

/// Cross-process exclusion around the fixture build, released on drop.
struct FixtureLock {
    dir: std::path::PathBuf,
}

impl Drop for FixtureLock {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Claim `dir` by atomic `create_dir`, waiting while another process holds it.
///
/// `OnceLock` memoises per PROCESS. That was enough locally but not in CI: the
/// project's gate runner is cargo-nextest, which executes every test in its own
/// process, so the two tests in this file each built into the same `--out`
/// directory and one `remove_dir_all`'d the other's output between writing the
/// `.ll` and linking it (`ld: cannot open output file .../gemm_baseline: No such
/// file or directory`, CI run 45050623812). A directory is the portable atomic
/// claim; the deadline turns a leaked claim into a loud failure, not a hang.
fn claim(dir: &std::path::Path) -> FixtureLock {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if std::fs::create_dir(dir).is_ok() {
            return FixtureLock {
                dir: dir.to_path_buf(),
            };
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fixture lock {} held for over 300 s; remove it if no build is running",
            dir.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// True when `bin` exists and is at least as new as `src`, so a binary left
/// behind by an older fixture is never the thing measured.
fn is_fresh(bin: &std::path::Path, src: &std::path::Path) -> bool {
    let (Ok(bin_md), Ok(src_md)) = (std::fs::metadata(bin), std::fs::metadata(src)) else {
        return false;
    };
    match (bin_md.modified(), src_md.modified()) {
        (Ok(built), Ok(changed)) => built >= changed,
        _ => false,
    }
}

/// Compile-and-run the fixture: the measurement is the point, so the release
/// brievc is required and the debug binary is not acceptable.
fn run_fixture() -> &'static str {
    FIXTURE.get_or_init(|| {
        let root = root();
        let out_dir = root.join("target/saltpetre-gemm-baseline");
        let bin = out_dir.join("gemm_baseline");
        let src = root.join("tests/saltpetre/gemm_baseline.bv");
        let _held = claim(&root.join("target/saltpetre-gemm-baseline.lock"));
        if is_fresh(&bin, &src) {
            return finish(
                Command::new(&bin)
                    .output()
                    .expect("run the built gemm_baseline"),
            );
        }
        std::fs::remove_dir_all(&out_dir).ok();
        finish(
            Command::new(root.join("target/release/brievc"))
                .args([
                    "run",
                    "tests/saltpetre/gemm_baseline.bv",
                    "--stdlib-path",
                    "lib",
                    "--config-dir",
                    "config",
                    "--out",
                ])
                .arg(&out_dir)
                .env("BRIEV_HOME", &root)
                .current_dir(&root)
                .output()
                .expect("compile and run gemm_baseline.bv"),
        )
    })
}

/// Merge the two streams and refuse a non-zero exit, quoting the output.
fn finish(out: std::process::Output) -> String {
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "fixture failed:\n{text}");
    text
}

/// Invariants 1 and 2: the fixture runs clean, its sampled spot-check against
/// an independent CPU dot is zero, and it prints the wall-clock baseline.
#[test]
fn salt_gemm_cpu_runs_and_checks() {
    let text = run_fixture();
    let bad = line_value(text, "SALT_GEMM_BAD=");
    assert_eq!(bad, "0", "sampled spot-check mismatches (BAD): {text}");
    assert!(
        text.contains("SALT_GEMM_CPU_US="),
        "missing wall-clock baseline line:\n{text}"
    );
    assert!(
        text.contains("SALT_GEMM_SUM="),
        "missing spot-check sum line:\n{text}"
    );
}

/// The baseline is a plausible f32 scalar-loop time: positive and below a
/// hard ceiling so a silent hang or a folded-away graph cannot pass. The
/// ceiling is generous (single-threaded f32 at 512^3 is ~0.1-0.3 s => ~1e5-3e5
/// microseconds); 5,000,000 µs (5 s) is an order of magnitude above that,
/// far below a hang.
#[test]
fn salt_gemm_cpu_baseline_is_plausible() {
    let text = run_fixture();
    let us = line_value(text, "SALT_GEMM_CPU_US=")
        .parse::<f64>()
        .expect("SALT_GEMM_CPU_US must be numeric");
    assert!(us > 0.0, "baseline must be positive, got {us} us");
    assert!(
        us < 5_000_000.0,
        "baseline {us} us implausibly large (hang or folded graph)"
    );
}

fn line_value(text: &str, prefix: &str) -> String {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest.trim().to_string();
        }
    }
    panic!("no line starting with {prefix:?} in output");
}
