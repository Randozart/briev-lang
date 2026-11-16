// ── The zero-friction FFI gate (regression canary) ─────────────────────
// 2026-08-04 (plan 2026-08-04-zero-friction-ffi-gate): runs
// benchmarks/bridge/gate/run_gate.sh and asserts the gate ratios stay sane.
// These are GENEROUS canaries (catching gross regressions, not flaky under
// load), not tight perf assertions:
//   - every present host: Briv feature_hash / native feature_hash < 1.6
//     (Briv's real work must never be 60%+ slower than the host itself)
//   - Python/Lua/Node: that ratio < 0.6 (Briv must still win big for
//     interpreted hosts — its compute is native machine code)
//   - Python add ratio < 2.0 (the METH_FASTCALL dispatch stays tight)
// Toolchain-guarded: needs cc+clang (C baseline); other hosts run if present.

use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn has(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[test]
fn zero_friction_gate() {
    // Opt-in: this gate builds every host's bridge + runs 3 interleaved rounds
    // (~3 min). Run explicitly with BRIV_RUN_GATE=1 before perf-affecting
    // commits; the default suite stays fast.
    if std::env::var("BRIV_RUN_GATE").unwrap_or_default() != "1" {
        eprintln!("SKIP: set BRIV_RUN_GATE=1 to run the zero-friction gate");
        return;
    }
    for tool in ["cc", "clang"] {
        if !has(tool) {
            eprintln!("SKIP: {} not available", tool);
            return;
        }
    }
    let gate = format!("{}/benchmarks/bridge/gate/run_gate.sh", PROJECT_ROOT);
    let run = Command::new("bash")
        .arg(&gate)
        .output()
        .expect("failed run_gate.sh");
    assert!(
        run.status.success(),
        "gate failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let out = String::from_utf8_lossy(&run.stdout);

    // Parse host rows: "C  1100.3  1095.7  1.00  1.99  1.87  1.06"
    let known = ["C", "C++", "Go", "Java", "Lua", "Py", "Node"];
    let mut hosts = 0;
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 7 || !known.contains(&f[0]) {
            continue;
        }
        hosts += 1;
        let host = f[0];
        let fh_ratio: f64 = f[3].parse().unwrap_or(f64::MAX);
        let add_ratio: f64 = f[6].parse().unwrap_or(f64::MAX);
        assert!(
            fh_ratio < 1.6,
            "{}: Briv feature_hash {:.2}x native — real work regressed",
            host,
            fh_ratio
        );
        match host {
            "Py" | "Lua" | "Node" => {
                assert!(
                    fh_ratio < 0.6,
                    "{}: Briv must beat interpreted-native by >1.7x, got {:.2}x",
                    host,
                    fh_ratio
                );
            }
            "Py" => {
                assert!(
                    add_ratio < 2.0,
                    "{}: METH_FASTCALL dispatch regressed (add {:.2}x native)",
                    host,
                    add_ratio
                );
            }
            _ => {}
        }
    }
    assert!(hosts > 0, "no gate rows parsed");
    eprintln!(
        "zero-friction gate: {} hosts, all ratios within bounds",
        hosts
    );
}
