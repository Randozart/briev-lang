// CLI argv E2E + interpreter parity — Increment 2 of
// docs/plans/2026-10-12-daily-driver-quick-wins.md.
//
// Compiled surface: build+link+run with known argv, golden stdout. The
// entry-plugin surface (`entry!`/`args!`) and the `Argc#`/`Args#` intrinsics
// (lib/std/cli.bv walkers) must agree on the same process argv.
//
// Interpreter surface: `Argc#()` is the materialized `std::env::args()`
// length (interpreter/intrinsics.rs) — the exit value must equal the test
// process's own argc (Rule 5: the interpreter is the reference the backend
// reproduces; here both engines read the same OS argv).

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Build + link the given source (written to a scratch .bv) and run it with
/// `argv`. Returns (stdout, exit_code).
fn build_link_run_argv(name: &str, src: &str, argv: &[&str]) -> (String, i32) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_cli_args")
        .join(name);
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();
    let bv = out_dir.join(format!("{name}.bv"));
    std::fs::write(&bv, src).unwrap();

    let build = Command::new(brievc)
        .args([
            "build",
            &bv.to_string_lossy(),
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

    let run = Command::new(&exe)
        .args(argv)
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to run: {e}"));
    (
        String::from_utf8_lossy(&run.stdout).to_string(),
        run.status.code().unwrap_or(-1),
    )
}

const GREET: &str = r#"
let n: Int = 0;
import "std/cli.bv";

node main [entry!("greet")][n == 1] {
    when args!("--verbose") {
        println!("verbose");
    };
    println!(args!("--name", String));
    n = 1;
    term;
};
"#;

const COUNTER: &str = r#"
import "std/cli.bv";
let n: Int = 0;
node main [n == 0][n == 1] {
    n = 1;
    endprogram arg_count();
};
"#;

/// entry_cmd picks the first non-flag argv token — the documented semantics
/// of the revived cli.bv (`entry_cmd_walk`): `greet --verbose --name world`
/// → "greet"; the flag-unaware walk means a VALUE that precedes the command
/// wins, which is why `build --verbose --name world` → "build".
#[test]
fn entry_cmd_greet_verbose_name() {
    let (out, code) = build_link_run_argv(
        "greet",
        GREET,
        &["greet", "--verbose", "--name", "world"],
    );
    assert_eq!(code, 0, "greet probe exit code");
    assert_eq!(out, "verbose\nworld\n", "golden stdout");
}

#[test]
fn entry_cmd_greet_quiet() {
    let (out, code) = build_link_run_argv(
        "greet_quiet",
        GREET,
        &["greet", "--name", "world"],
    );
    assert_eq!(code, 0, "greet probe (no --verbose) exit code");
    assert_eq!(out, "world\n", "verbose branch silent, name value printed");
}

#[test]
fn entry_cmd_wrong_command_skips_node() {
    let (out, code) = build_link_run_argv(
        "wrong_cmd",
        GREET,
        &["run", "--name", "world"],
    );
    assert_eq!(code, 0, "non-matching entry_cmd must exit cleanly");
    assert_eq!(out, "", "greet body never fires");
}

/// Argc# via the compiled reactor: exit code = process argc (the swan song
/// crosses the exit boundary, so the value is observable). Note the runtime
/// truncates to the OS exit-code width (0-255) — argc is always small.
#[test]
fn argc_intrinsic_exit_code_matches_process_argc() {
    let argv = ["x", "y", "z"];
    let (_, code) = build_link_run_argv("argc_exit", COUNTER, &argv);
    assert_eq!(
        code,
        (argv.len() + 1) as i32,
        "exit code = argc (program name + {} args)",
        argv.len()
    );
}

// ── Interpreter parity (Rule 5) ─────────────────────────────────────────

/// Argc# in the interpreter materializes `std::env::args()` — the test
/// process's own argv. The exit value must equal that count (the same OS
/// argv the compiled surface reads via `@__briev_argc`).
#[test]
fn interpreter_argc_matches_process_argc() {
    use briev_compiler::errors::RuntimeError;
    use briev_compiler::interpreter::{Interpreter, Value};
    use briev_compiler::lexer::tokenize;
    use briev_compiler::parser::Parser;
    use briev_compiler::reactor::Reactor;

    let src = r#"
let n: Int = 0;
node run [n == 0][n == 1] {
    n = 1;
    endprogram Argc#();
};
"#;
    let tokens = tokenize(src).expect("lex");
    let mut p = Parser::new(tokens, src);
    let items = p.parse_program().expect("parse");
    let mut interp = Interpreter::new();
    interp.load_program(&items);
    interp.seed_inits(&items).expect("seed inits");
    for item in &items {
        if let briev_compiler::ast::TopLevel::Statement(s) = item {
            interp.exec_stmt(s).expect("seed stmt");
        }
    }
    let mut reactor = Reactor::new();
    reactor.build_from_program(&items);
    let exit = loop {
        match reactor.run(&mut interp) {
            Ok(true) => {}
            Ok(false) => panic!("reactor reached equilibrium without endprogram"),
            Err(RuntimeError::ProgramExit(v)) => break v,
            Err(e) => panic!("runtime: {e:?}"),
        }
    };
    let got = match exit {
        Value::Atom(briev_compiler::interpreter::Atom::Int(v)) => Some(v),
        _ => None,
    };
    assert_eq!(
        got,
        Some(std::env::args().count() as i64),
        "interpreter Argc#() must equal the process argc"
    );
}
