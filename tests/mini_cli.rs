// Core-usable mini-CLI E2E — Phase 4 close-out of
// docs/plans/2026-10-15-lifecycle-laws-termination-warning.md.
//
// One program exercising the whole daily-driver os surface: cli args, fs
// write/read round trip, mktemp_file (rand + O_EXCL), directory listing
// count, and the wall clock (cast_lanes rt_now_ns). Golden stdout pins the
// data flow; the exit code pins the dispatch. Scratch lives under target/
// (unique per test slot — cargo runs tests in parallel). The zombie
// warning (no terminator in the driver) is expected on stderr and NOT part
// of the contract.

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn mini_cli_uses_the_daily_driver_surface() {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let slot = "mini_cli";
    let out_dir = Path::new(PROJECT_ROOT).join("target/briev_mini_cli").join(slot);
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    // The scratch directory the program will list: one file → 3 entries
    // (".", "..", "note.txt"), in any readdir order (the count is the
    // order-independent contract).
    let scratch = out_dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("note.txt"), "x").unwrap();

    let bv = out_dir.join(format!("{slot}.bv"));
    let src = r#"
import "std/cli.bv";
import "std/os/fs.bv";
import "std/os/dir.bv";
import "std/os/rand.bv";
import "std/os/temp.bv";

let n: Int = 0;
node main [n == 0][n == 1] {
    seed_rand(7);
    let who: String = arg(1);
    let scratch: String = arg(2);
    let tmpbase: String = arg(3);

    // fs round trip
    let path: String = scratch + "/note.txt";
    let w: Result<Int, Int> = write_file(path, "hi " + who);
    when is_ok(w) {
        println!(unwrap(w));
    };
    let c: Result<String, Int> = read_file(path);
    when is_ok(c) {
        println!(unwrap(c));
    };

    // temp: unique file via rand + O_EXCL, then gone
    let t: Result<String, Int> = mktemp_file(tmpbase);
    when is_ok(t) {
        let p: String = unwrap(t);
        println!(file_exists(p));
        let d: Result<Int, Int> = remove_file(p);
        println!(is_ok(d));
        println!(file_exists(p));
    };

    // dir: order-independent count
    let list: Result<List<String>, Int> = entries(scratch);
    when is_ok(list) {
        println!(unwrap(list).Count#());
    };

    // clock: monotonic-ish wall time — two reads never agree exactly, but
    // the second is never SMALLER.
    let a: Int = rt_now_ns();
    let b: Int = rt_now_ns();
    when b >= a {
        println!("clock-ok");
    };
    n = 1;
    term;
};
"#;
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
        .unwrap_or_else(|e| panic!("{slot}: failed to spawn brievc build: {e}"));
    assert!(
        build.status.success(),
        "{slot}: build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join(format!("{slot}.ll"));
    let exe = out_dir.join(slot);
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
        .unwrap_or_else(|e| panic!("{slot}: failed to spawn clang: {e}"));
    assert!(
        link.status.success(),
        "{slot}: link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let tmpbase = out_dir.join("tmpfile");
    let run = Command::new(&exe)
        .args([
            "mini",
            &scratch.to_string_lossy(),
            &tmpbase.to_string_lossy(),
        ])
        .output()
        .unwrap_or_else(|e| panic!("{slot}: failed to run: {e}"));
    let out = String::from_utf8_lossy(&run.stdout).to_string();
    assert_eq!(
        run.status.code(),
        Some(0),
        "{slot}: exit code drifted"
    );
    assert_eq!(
        out, "7\nhi mini\ntrue\ntrue\nfalse\n3\nclock-ok\n",
        "{slot}: golden drifted: {out:?}"
    );
}
