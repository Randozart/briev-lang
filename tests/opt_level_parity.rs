// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P3 differential gate (opt levels): one emitted `.ll`, linked/run at
//! `-O0`, `-O2`, `-O3` — stdout and exit code must agree. The program is the
//! same source in every arm, so no stored golden (float drift vs a golden is
//! not this gate's concern).
//!
//! Quick set per commit; `OPT_MATRIX_FULL=1` runs every program-surface
//! tier1 fixture. Library-surface fixtures (export defns, no entry) are
//! skipped by construction, not by a list.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const QUICK_SET: &[&str] = &[
    "daily_use_smoke",
    "fsm_smoke",
    "iter_chain_multi",
    "json_empty_object_lookup",
];

/// Fixtures known to miscompile or diverge across optimization levels. The
/// list may only shrink; each entry names the reason and the BUGS.md id.
const SKIP: &[(&str, &str)] = &[];

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A program fixture declares a `node`/`txn` entry; library fixtures export
/// defns only.
fn is_program_surface(src: &str) -> bool {
    let code: String = src
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    code.contains("node ") || code.contains("txn ")
}

/// Run a command with a hard deadline; a hung fixture must fail the gate, not
/// stall CI (reactive programs can idle forever).
fn run_with_timeout(mut cmd: Command, secs: u64) -> Result<Output, String> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|e| format!("wait: {e}")),
            Ok(None) if start.elapsed() > Duration::from_secs(secs) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {secs}s"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
}

fn build_ll(brievc: &Path, src: &Path, out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("mkdir {}: {e}", out.display()))?;
    let mut cmd = Command::new(brievc);
    cmd.args([
        "build",
        &src.to_string_lossy(),
        "--llvm",
        "--out",
        &out.to_string_lossy(),
    ]);
    let status = run_with_timeout(cmd, 120).map_err(|e| format!("brievc: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&status.stderr).into_owned())
    }
}

fn link(ll: &Path, bin: &Path, level: &str, lto: bool) -> Result<(), String> {
    let rt = project_root().join("lib/runtime/briev_rt.c");
    let mut cmd = Command::new("clang");
    cmd.arg(level).arg("-march=native");
    if lto {
        for flag in [
            "-flto",
            "-ffast-math",
            "-fdata-sections",
            "-ffunction-sections",
            "-Wl,--gc-sections",
        ] {
            cmd.arg(flag);
        }
    }
    cmd.arg(ll).arg(rt).arg("-o").arg(bin);
    let out = run_with_timeout(cmd, 300).map_err(|e| format!("clang: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn run(bin: &Path) -> (String, i32) {
    let mut cmd = Command::new(bin);
    cmd.env("BOUND", "1000");
    let out = run_with_timeout(cmd, 10)
        .unwrap_or_else(|e| panic!("run {}: {e}", bin.display()));
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn check_fixture(brievc: &Path, src: &Path, scratch: &Path) {
    let name = src.file_stem().unwrap().to_string_lossy().into_owned();
    let dir = scratch.join(&name);
    let _ = std::fs::remove_dir_all(&dir);
    build_ll(brievc, src, &dir).unwrap_or_else(|e| panic!("{name}: build failed:\n{e}"));
    let ll = dir.join(format!("{name}.ll"));

    let mut reference: Option<(String, i32)> = None;
    for (level, lto) in [("-O0", false), ("-O2", true), ("-O3", true)] {
        let bin = dir.join(format!("{name}{level}"));
        link(&ll, &bin, level, lto).unwrap_or_else(|e| panic!("{name}: {level} link failed:\n{e}"));
        let observed = run(&bin);
        match &reference {
            None => reference = Some(observed),
            Some(expected) => assert_eq!(
                observed, *expected,
                "{name}: {level} diverges from -O0 (stdout/exit)"
            ),
        }
    }
}

fn program_fixtures() -> Vec<String> {
    let root = project_root().join("tests/tier1");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("tests/tier1")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "bv").unwrap_or(false))
        .filter(|p| {
            let src = std::fs::read_to_string(p).unwrap_or_default();
            is_program_surface(&src)
        })
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

#[test]
fn optimization_levels_agree() {
    let brievc = PathBuf::from(env!("CARGO_BIN_EXE_brievc"));
    let scratch =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("opt_level_parity");
    let full = std::env::var("OPT_MATRIX_FULL").is_ok();
    let names: Vec<String> = if full {
        program_fixtures()
    } else {
        QUICK_SET.iter().map(|s| s.to_string()).collect()
    };
    let root = project_root().join("tests/tier1");
    for name in names {
        if SKIP.iter().any(|(skip, _)| *skip == name) {
            continue;
        }
        check_fixture(&brievc, &root.join(format!("{name}.bv")), &scratch);
    }
}
