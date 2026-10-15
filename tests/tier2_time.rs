// time.bv — clock + sleep E2E — tier2 phase 4
// (docs/plans/2026-10-14-daily-driver-tier2.md).
//
// Pure stdlib over the variadic syscall: SYS_CLOCK_GETTIME (228,
// CLOCK_MONOTONIC) + SYS_NANOSLEEP (35), a 16-byte heap timespec built
// with the raw-memory intrinsics and freed on every path. The assertions
// are real: now_ns is non-decreasing (monotonic), and a sleep_ms(30)
// measures >= 25 ms of wall time.

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn time_clock_monotonic_and_sleep_measures_real() {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT).join("target/briev_tier2/time");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();
    let bv = out_dir.join("time.bv");
    std::fs::write(
        &bv,
        r#"
import "std/os/time.bv";

let done: Bool = false;
node main [done == false][done == true] {
    let t0: Int = now_ns();
    let slept: Int = sleep_ms(30);
    let t1: Int = now_ns();
    let elapsed_ms: Int = (t1 - t0) / 1000000;
    println!(slept);
    println!(elapsed_ms);
    println!(t1 >= t0);
    done = true;
    term;
};
"#,
    )
    .unwrap();

    let build = Command::new(brievc)
        .args(["build", &bv.to_string_lossy(), "--llvm", "--out", &out_dir.to_string_lossy()])
        .output()
        .expect("spawn brievc build");
    assert!(
        build.status.success(),
        "build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let ll = out_dir.join("time.ll");
    let exe = out_dir.join("time");
    let rt = Path::new(PROJECT_ROOT).join("lib/runtime/briev_rt.c");
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
            &exe.to_string_lossy(),
        ])
        .output()
        .expect("spawn clang");
    assert!(
        link.status.success(),
        "link failed:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&exe).output().expect("run time probe");
    assert!(run.status.success(), "exit code {:?}", run.status.code());
    let lines: Vec<String> = String::from_utf8_lossy(&run.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(lines[0], "0", "sleep_ms returns 0 on completion");
    let elapsed: i64 = lines[1].parse().expect("elapsed ms printed");
    assert!(elapsed >= 25, "sleep_ms(30) measured only {elapsed} ms");
    assert!(elapsed < 5000, "sleep_ms(30) measured {elapsed} ms — runaway");
    assert_eq!(lines[2], "true", "monotonic clock never goes backwards");
}
