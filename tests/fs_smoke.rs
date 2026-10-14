// File I/O E2E — Increment 3 of docs/plans/2026-10-12-daily-driver-quick-wins.md.
//
// lib/std/os/fs.bv round-trip through the compiled surface: write_file →
// read_file → read_lines → file_exists, plus the Err branch (missing file).
// Golden stdout asserts the data survived the syscall round trip byte-for-
// byte, and the exit code pins the Result dispatch. The scratch file lives
// under target/ (unique per test slot — cargo runs tests in parallel).

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn build_link_run(name: &str, src: &str, scratch: &str) -> (String, i32) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT)
        .join("target/briev_fs_smoke")
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
        .arg(scratch)
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to run: {e}"));
    (
        String::from_utf8_lossy(&run.stdout).to_string(),
        run.status.code().unwrap_or(-1),
    )
}

/// write → read → read_lines → file_exists round trip, plus the Err branch.
/// The program takes the scratch path as argv[1] (cli.bv arg(1)).
#[test]
fn fs_round_trip_write_read_lines() {
    let src = r#"
import "std/cli.bv";
import "std/os/fs.bv";

let n: Int = 0;
node main [n == 0][n == 1] {
    let path: String = arg(1);
    let payload: String = "alpha\nbeta\ngamma";

    // Result access is SHAPE-SENSITIVE in the backend today (BUGS.md —
    // enum payload bugs, 2026-10-12): Int payloads take predicates+unwrap,
    // String payloads take match. Each form here is probe-verified.
    let wres: Result<Int, Int> = write_file(path, payload);
    when is_err(wres) {
        println!(unwrap_err(wres));
    };
    when is_ok(wres) {
        println!(unwrap(wres));
    };

    let content: Result<String, Int> = read_file(path);
    match content {
        Ok(data) => println!(data),
        Err(e) => println!(e),
    };

    let lres: Result<List<String>, Int> = read_lines(path);
    when is_err(lres) {
        println!(unwrap_err(lres));
    };
    // The count goes through read_line_count — consuming a List-payload
    // Result via a let binding mis-lowers in node bodies (BUGS.md,
    // 2026-10-12); the count stays on the proven Int paths.
    let cnt: Result<Int, Int> = read_line_count(path);
    when is_ok(cnt) {
        println!(unwrap(cnt));
    };

    when file_exists(path) {
        println!("exists");
    };
    when !file_exists("/nonexistent/briev_fs_probe") {
        println!("absent");
    };

    let missing: Result<String, Int> = read_file("/nonexistent/briev_fs_probe");
    match missing {
        Ok(d) => println!(d),
        Err(e) => {
            println!("err");
            println!(e);
        }
    };

    n = 1;
    term;
};
"#;
    let scratch = Path::new(PROJECT_ROOT)
        .join("target/briev_fs_smoke/scratch_roundtrip.txt");
    let _ = std::fs::remove_file(&scratch);
    let (out, code) = build_link_run("roundtrip", src, &scratch.to_string_lossy());
    assert_eq!(code, 0, "round-trip program exit code");
    assert_eq!(
        out,
        "16\n\
         alpha\nbeta\ngamma\n\
         3\n\
         exists\n\
         absent\n\
         err\n\
         -2\n",
        "golden stdout: bytes written, file content, line count, \
         existence pair, missing-file Err(-ENOENT)"
    );
}

/// Overwrite semantics: O_TRUNC — the second write replaces, not appends.
#[test]
fn fs_write_truncates() {
    let src = r#"
import "std/cli.bv";
import "std/os/fs.bv";

let n: Int = 0;
node main [n == 0][n == 1] {
    let path: String = arg(1);
    write_file(path, "first-version-longer");
    let wres: Result<Int, Int> = write_file(path, "short");
    when is_ok(wres) {
        println!(unwrap(wres));
    };
    let content: Result<String, Int> = read_file(path);
    match content {
        Ok(d) => println!(d),
        Err(e) => println!(e),
    };
    n = 1;
    term;
};
"#;
    let scratch = Path::new(PROJECT_ROOT)
        .join("target/briev_fs_smoke/scratch_trunc.txt");
    let _ = std::fs::remove_file(&scratch);
    let (out, code) = build_link_run("trunc", src, &scratch.to_string_lossy());
    assert_eq!(code, 0, "truncate program exit code");
    assert_eq!(
        out,
        "5\nshort\n",
        "O_TRUNC: second write replaces content; read returns only it"
    );
}

/// The variadic `syscall(nr, ...args)` retirement (2026-10-14): the
/// ZERO-rest instantiation (SYS_GETPID = 39) runs E2E — POSIX guarantees a
/// pid > 0, so the assertion is real, not a smoke.
#[test]
fn fs_syscall_variadic_getpid() {
    let src = r#"
import "std/posix/syscall.bv";

let n: Int = 0;
node main [n == 0][n == 1] {
    let pid: Int = syscall(39);
    when pid > 0 {
        println!(pid);
    };
    n = 1;
    term;
};
"#;
    let (out, code) = build_link_run("getpid", src, "unused-scratch");
    assert_eq!(code, 0, "getpid program exit code");
    let pid: i64 = out
        .trim()
        .parse()
        .expect("getpid printed a positive integer");
    assert!(pid > 0, "getpid returned {pid}");
}
