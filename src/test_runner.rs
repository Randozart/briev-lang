// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

// `brievc test` — the Briev-level test runner (tier2 phase 3,
// docs/plans/2026-10-14-daily-driver-tier2.md).
//
// Each `*.bv` file under the path is one test: compile (the same `brievc
// build --llvm` path users run, via the running executable), link with
// lib/runtime/briev_rt.c (the benchmark harness's exact clang line), run.
// PASS = exit 0, plus a byte-equal stdout match when a sibling `<name>.golden`
// file exists (the daily_use_smoke golden convention, generalized).
//
// v2 (2026-11-09, verification program P2): comment directives add
// compile-fail tests and per-test options without new language surface:
//
//   // RUN: --backend llvm --optimize-budget 0   extra build arguments
//   // EXPECT-ERROR: <substring>                 build MUST fail; every
//                                               listed substring must appear
//   // EXPECT-STDOUT: <text>                     inline stdout pin (one line
//                                               per directive, newline-joined)
//   // EXPECT-EXIT: <n>                         expected exit code (default 0)
//
// A compile-fail test never links/runs and cannot also pin stdout.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct TestOutcome {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

struct BuildOutcome {
    success: bool,
    output: String,
}

fn spawn_build(
    exe: &Path,
    bv: &Path,
    scratch: &Path,
    extra: &[String],
) -> Result<BuildOutcome, String> {
    let out = Command::new(exe)
        .args([
            "build",
            &bv.to_string_lossy(),
            "--llvm",
            "--out",
            &scratch.to_string_lossy(),
        ])
        .args(extra)
        .output()
        .map_err(|e| format!("spawn brievc build: {e}"))?;
    let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
    output.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(BuildOutcome {
        success: out.status.success(),
        output,
    })
}

#[derive(Default)]
struct Directives {
    run_args: Vec<String>,
    expect_errors: Vec<String>,
    expect_stdout: Option<String>,
    expect_exit: i32,
}

fn parse_directives(src: &str) -> Directives {
    let mut d = Directives::default();
    let mut stdout = String::new();
    for line in src.lines() {
        apply_directive(line, &mut d, &mut stdout);
    }
    if !stdout.is_empty() {
        d.expect_stdout = Some(stdout);
    }
    d
}

fn apply_directive(line: &str, d: &mut Directives, stdout: &mut String) {
    let Some(rest) = line.trim().strip_prefix("//") else {
        return;
    };
    let rest = rest.trim();
    if let Some(args) = rest.strip_prefix("RUN:") {
        d.run_args.extend(args.split_whitespace().map(str::to_string));
        return;
    }
    if let Some(needle) = rest.strip_prefix("EXPECT-ERROR:") {
        d.expect_errors.push(needle.trim().to_string());
        return;
    }
    if let Some(text) = rest.strip_prefix("EXPECT-STDOUT:") {
        stdout.push_str(text.trim_start());
        stdout.push('\n');
        return;
    }
    if let Some(code) = rest.strip_prefix("EXPECT-EXIT:") {
        if let Ok(code) = code.trim().parse() {
            d.expect_exit = code;
        }
    }
}

fn link_and_run(scratch: &Path, name: &str) -> Result<(String, i32), String> {
    let ll = scratch.join(format!("{name}.ll"));
    let bin = scratch.join(name);
    let rt = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib/runtime/briev_rt.c");
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
            &rt.to_string_lossy(),
            "-o",
            &bin.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("spawn clang: {e}"))?;
    if !link.status.success() {
        return Err(format!(
            "link failed:\n{}",
            String::from_utf8_lossy(&link.stderr)
        ));
    }
    let run = Command::new(&bin)
        .output()
        .map_err(|e| format!("run {name}: {e}"))?;
    Ok((
        String::from_utf8_lossy(&run.stdout).into_owned(),
        run.status.code().unwrap_or(-1),
    ))
}

fn run_one(exe: &Path, bv: &Path, root: &Path) -> TestOutcome {
    let name = bv
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| bv.to_string_lossy().into_owned());
    let scratch = root.join(&name);
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        return TestOutcome { name, passed: false, detail: format!("scratch dir: {e}") };
    }
    let src = match std::fs::read_to_string(bv) {
        Ok(s) => s,
        Err(e) => return TestOutcome { name, passed: false, detail: format!("read: {e}") },
    };
    let dirs = parse_directives(&src);
    let build = match spawn_build(exe, bv, &scratch, &dirs.run_args) {
        Ok(b) => b,
        Err(e) => return TestOutcome { name, passed: false, detail: e },
    };
    if !dirs.expect_errors.is_empty() {
        return check_compile_fail(name, &dirs, &build);
    }
    if !build.success {
        return TestOutcome { name, passed: false, detail: format!("build failed:\n{}", build.output) };
    }
    run_program(name, bv, &dirs, &scratch)
}

/// A compile-fail case: the build must fail and every expected substring must
/// appear in the combined compiler output.
fn check_compile_fail(name: String, dirs: &Directives, build: &BuildOutcome) -> TestOutcome {
    if build.success {
        return TestOutcome {
            name,
            passed: false,
            detail: "expected a compile failure, but the build succeeded".to_string(),
        };
    }
    let missing: Vec<&String> = dirs
        .expect_errors
        .iter()
        .filter(|needle| !build.output.contains(needle.as_str()))
        .collect();
    if !missing.is_empty() {
        return TestOutcome {
            name,
            passed: false,
            detail: format!(
                "missing expected diagnostic(s) {missing:?}; compiler output:\n{}",
                build.output.trim()
            ),
        };
    }
    TestOutcome { name, passed: true, detail: String::new() }
}

/// A run-pass case: link, run, then check exit code and stdout (inline
/// directive or sibling `.golden`).
fn run_program(name: String, bv: &Path, dirs: &Directives, scratch: &Path) -> TestOutcome {
    let (stdout, code) = match link_and_run(scratch, &name) {
        Ok(r) => r,
        Err(e) => return TestOutcome { name, passed: false, detail: e },
    };
    if code != dirs.expect_exit {
        return TestOutcome {
            name,
            passed: false,
            detail: format!("exit code {code} (expected {})", dirs.expect_exit),
        };
    }
    let golden_path = bv.with_extension("golden");
    let expected = match &dirs.expect_stdout {
        Some(inline) => {
            if golden_path.exists() {
                return TestOutcome {
                    name,
                    passed: false,
                    detail: "both EXPECT-STDOUT and a .golden file are present; pick one".to_string(),
                };
            }
            inline.clone()
        }
        None if golden_path.exists() => match std::fs::read_to_string(&golden_path) {
            Ok(text) => text,
            Err(e) => {
                return TestOutcome { name, passed: false, detail: format!("golden read: {e}") }
            }
        },
        None => return TestOutcome { name, passed: true, detail: String::new() },
    };
    if stdout != expected {
        return TestOutcome {
            name,
            passed: false,
            detail: format!("stdout mismatch:\n  expected: {expected:?}\n  got:      {stdout:?}"),
        };
    }
    TestOutcome { name, passed: true, detail: String::new() }
}

struct TestArgs {
    target: PathBuf,
    out_dir: PathBuf,
}

fn parse_test_args(args: &[String]) -> Result<TestArgs, String> {
    let mut target: Option<PathBuf> = None;
    let mut out_dir = PathBuf::from("target/briev_test");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                out_dir = PathBuf::from(
                    it.next().ok_or("--out needs a directory argument")?,
                );
            }
            other if target.is_none() => target = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    let target = target.ok_or(
        "specify a test file or a directory of *.bv tests — e.g. `brievc test tests/`",
    )?;
    Ok(TestArgs { target, out_dir })
}

fn collect_test_files(target: &Path) -> Result<Vec<PathBuf>, String> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    if !target.is_dir() {
        return Err(format!("{} does not exist", target.display()));
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(target)
        .map_err(|e| format!("read dir {}: {e}", target.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map(|x| x == "bv").unwrap_or(false)
                && !p
                    .file_name()
                    .map(|n| n.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
        })
        .collect();
    entries.sort();
    Ok(entries)
}

/// `brievc test <file.bv | dir> [--out <dir>]`. Returns the process exit
/// code (0 = all passed).
pub fn run_test_mode(args: &[String]) -> Result<(), String> {
    let TestArgs { target, out_dir } = parse_test_args(args)?;
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let files = collect_test_files(&target)?;
    if files.is_empty() {
        return Err(format!("no *.bv tests under {}", target.display()));
    }

    std::fs::create_dir_all(&out_dir).map_err(|e| format!("out dir: {e}"))?;
    let mut failed = 0usize;
    for bv in &files {
        let outcome = run_one(&exe, bv, &out_dir);
        if outcome.passed {
            println!("PASS {}", outcome.name);
        } else {
            failed += 1;
            println!("FAIL {} — {}", outcome.name, outcome.detail);
        }
    }
    println!("{}/{} passed", files.len() - failed, files.len());
    if failed > 0 {
        Err(format!("{failed} test(s) failed"))
    } else {
        Ok(())
    }
}
