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
    /// SPEC §8.11 test-block counts for this file (0 when it has no blocks).
    pub tests_run: usize,
    pub tests_failed: usize,
}

impl TestOutcome {
    fn new(name: String, passed: bool, detail: String) -> Self {
        TestOutcome { name, passed, detail, tests_run: 0, tests_failed: 0 }
    }
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

fn run_one(exe: &Path, bv: &Path, root: &Path, args: &TestArgs) -> TestOutcome {
    let name = bv
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| bv.to_string_lossy().into_owned());
    let scratch = root.join(&name);
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        return TestOutcome::new(name, false, format!("scratch dir: {e}"));
    }
    let src = match std::fs::read_to_string(bv) {
        Ok(s) => s,
        Err(e) => return TestOutcome::new(name, false, format!("read: {e}")),
    };
    let dirs = parse_directives(&src);

    // ── SPEC §8.11 test blocks (reference interpreter) ────────────────
    if briev_compiler::test_engine::source_has_tests(&src) {
        let run = match briev_compiler::test_engine::run_tests(
            &bv.to_string_lossy(),
            &src,
            args.domain.as_deref(),
        ) {
            Ok(Some(run)) => run,
            Ok(None) => {
                if args.tests_only {
                    return TestOutcome::new(name, true, "no test blocks".to_string());
                }
                return run_program_file(exe, bv, name, &dirs, &scratch);
            }
            Err(e) => {
                return TestOutcome::new(name, false, format!("test compile failed:\n{e}"))
            }
        };
        if run.reports.is_empty() {
            return TestOutcome::new(
                name,
                false,
                format!(
                    "no test blocks in domain '{}'; available domains: {}",
                    args.domain.as_deref().unwrap_or(""),
                    run.domains.join(", ")
                ),
            );
        }
        if args.list_tests {
            for report in &run.reports {
                println!("{}", report.label);
            }
            return TestOutcome::new(name, true, String::new());
        }
        let tests_run = run.reports.len();
        let tests_failed = run.reports.iter().filter(|r| !r.passed).count();
        let mut failures = Vec::new();
        for report in &run.reports {
            if report.passed {
                println!("TEST {} PASS", report.label);
            } else {
                println!("TEST {} FAIL — {}", report.label, report.detail);
                failures.push(format!("{}: {}", report.label, report.detail));
            }
        }
        if !failures.is_empty() {
            return TestOutcome {
                tests_run,
                tests_failed,
                name,
                passed: false,
                detail: failures.join("\n"),
            };
        }
        if args.tests_only {
            return TestOutcome { tests_run, tests_failed, name, passed: true, detail: String::new() };
        }
        let mut outcome = run_program_file(exe, bv, name, &dirs, &scratch);
        outcome.tests_run = tests_run;
        outcome.tests_failed = tests_failed;
        return outcome;
    } else if args.tests_only {
        return TestOutcome::new(name, true, "no test blocks".to_string());
    }

    run_program_file(exe, bv, name, &dirs, &scratch)
}

/// Build + link + run the file as a program (the pre-test-block path).
fn run_program_file(exe: &Path, bv: &Path, name: String, dirs: &Directives, scratch: &Path) -> TestOutcome {
    let build = match spawn_build(exe, bv, scratch, &dirs.run_args) {
        Ok(b) => b,
        Err(e) => return TestOutcome::new(name, false, e),
    };
    if !dirs.expect_errors.is_empty() {
        return check_compile_fail(name, dirs, &build);
    }
    if !build.success {
        return TestOutcome::new(name, false, format!("build failed:\n{}", build.output));
    }
    run_program(name, bv, dirs, scratch)
}

/// A compile-fail case: the build must fail and every expected substring must
/// appear in the combined compiler output.
fn check_compile_fail(name: String, dirs: &Directives, build: &BuildOutcome) -> TestOutcome {
    if build.success {
        return TestOutcome::new(
            name,
            false,
            "expected a compile failure, but the build succeeded".to_string(),
        );
    }
    let missing: Vec<&String> = dirs
        .expect_errors
        .iter()
        .filter(|needle| !build.output.contains(needle.as_str()))
        .collect();
    if !missing.is_empty() {
        return TestOutcome::new(
            name,
            false,
            format!(
                "missing expected diagnostic(s) {missing:?}; compiler output:\n{}",
                build.output.trim()
            ),
        );
    }
    TestOutcome::new(name, true, String::new())
}

/// A run-pass case: link, run, then check exit code and stdout (inline
/// directive or sibling `.golden`).
fn run_program(name: String, bv: &Path, dirs: &Directives, scratch: &Path) -> TestOutcome {
    let (stdout, code) = match link_and_run(scratch, &name) {
        Ok(r) => r,
        Err(e) => return TestOutcome::new(name, false, e),
    };
    if code != dirs.expect_exit {
        return TestOutcome::new(
            name,
            false,
            format!("exit code {code} (expected {})", dirs.expect_exit),
        );
    }
    let golden_path = bv.with_extension("golden");
    let expected = match &dirs.expect_stdout {
        Some(inline) => {
            if golden_path.exists() {
                return TestOutcome::new(
                    name,
                    false,
                    "both EXPECT-STDOUT and a .golden file are present; pick one".to_string(),
                );
            }
            inline.clone()
        }
        None if golden_path.exists() => match std::fs::read_to_string(&golden_path) {
            Ok(text) => text,
            Err(e) => return TestOutcome::new(name, false, format!("golden read: {e}")),
        },
        None => return TestOutcome::new(name, true, String::new()),
    };
    if stdout != expected {
        return TestOutcome::new(
            name,
            false,
            format!("stdout mismatch:\n  expected: {expected:?}\n  got:      {stdout:?}"),
        );
    }
    TestOutcome::new(name, true, String::new())
}

struct TestArgs {
    target: PathBuf,
    out_dir: PathBuf,
    /// 2026-11-10 (SPEC §8.11): run only `test` blocks (skip the file-as-
    /// program check).
    tests_only: bool,
    /// Print `domain#n` labels and run nothing else.
    list_tests: bool,
    /// Exact domain filter.
    domain: Option<String>,
}

fn parse_test_args(args: &[String]) -> Result<TestArgs, String> {
    let mut target: Option<PathBuf> = None;
    let mut out_dir = PathBuf::from("target/briev_test");
    let mut tests_only = false;
    let mut list_tests = false;
    let mut domain = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                out_dir = PathBuf::from(
                    it.next().ok_or("--out needs a directory argument")?,
                );
            }
            "--tests-only" => tests_only = true,
            "--list-tests" => list_tests = true,
            "--domain" => {
                domain = Some(it.next().ok_or("--domain needs a name")?.clone());
            }
            other if target.is_none() => target = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    let target = target.ok_or(
        "specify a test file or a directory of *.bv tests — e.g. `brievc test tests/`",
    )?;
    Ok(TestArgs { target, out_dir, tests_only, list_tests, domain })
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
    let parsed = parse_test_args(args)?;
    let target = parsed.target.clone();
    let out_dir = parsed.out_dir.clone();
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let files = collect_test_files(&target)?;
    if files.is_empty() {
        return Err(format!("no *.bv tests under {}", target.display()));
    }

    std::fs::create_dir_all(&out_dir).map_err(|e| format!("out dir: {e}"))?;
    let mut failed = 0usize;
    let mut tests_run = 0usize;
    let mut tests_failed = 0usize;
    for bv in &files {
        let outcome = run_one(&exe, bv, &out_dir, &parsed);
        tests_run += outcome.tests_run;
        tests_failed += outcome.tests_failed;
        if outcome.passed {
            println!("PASS {}", outcome.name);
        } else {
            failed += 1;
            println!("FAIL {} — {}", outcome.name, outcome.detail);
        }
    }
    if tests_run > 0 {
        println!(
            "{}/{} tests passed; {}/{} files passed",
            tests_run - tests_failed,
            tests_run,
            files.len() - failed,
            files.len()
        );
    } else {
        println!("{}/{} passed", files.len() - failed, files.len());
    }
    if failed > 0 {
        Err(format!("{failed} file(s) failed ({tests_failed} test(s))"))
    } else {
        Ok(())
    }
}
