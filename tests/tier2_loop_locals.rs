// Loop-carried locals across nested blocks — tier2 phase 2
// (docs/plans/2026-10-14-daily-driver-tier2.md, BUGS.md 9303).
//
// A node-body local ASSIGNED inside a `when` block and READ in a sibling:
// the assign-site alloca demotion used to land INSIDE the block, which does
// not dominate the sibling read (clang: "Instruction does not dominate all
// uses!"). The SSA main now pre-declares entry allocas for reassigned lets
// (the emit_definition pattern), so the let's seed store and all later
// stores target a slot that dominates everything. Tests linked at -O0 (the
// IR-validity gate).

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn build_link_run(name: &str, src: &str) -> (String, i32) {
    let brievc = env!("CARGO_BIN_EXE_brievc");
    let out_dir = Path::new(PROJECT_ROOT).join("target/briev_tier2").join(name);
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
            "-O0",
            "-fdata-sections",
            "-ffunction-sections",
            &ll.to_string_lossy(),
            &format!("{PROJECT_ROOT}/lib/runtime/briev_rt.c"),
            "-o",
            &exe.to_string_lossy(),
        ])
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to spawn clang: {e}"));
    assert!(
        link.status.success(),
        "{name}: link failed (-O0 IR gate):\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("{name}: failed to run: {e}"));
    (
        String::from_utf8_lossy(&run.stdout).to_string(),
        run.status.code().unwrap_or(-1),
    )
}

/// Both `when` blocks fire: i = 0 → 1 (first when) → 2 (second when).
#[test]
fn loop_local_carried_across_whens() {
    let (out, code) = build_link_run(
        "loop_local_both",
        r#"
let done: Bool = false;
node main [done == false][done == true] {
    let i: Int = 0;
    let up: Int = 1;
    when up > 0 {
        i = 1;
    };
    when i < 5 {
        i = i + 1;
    };
    println!(i);
    done = true;
    term;
};
"#,
    );
    assert_eq!(code, 0, "exit code (the dominance gate — clang -O0)");
    assert_eq!(out.trim(), "2", "carried through both blocks");
}

/// The FIRST when never fires — the sibling read must see the LET's
/// initial value (0), then the second when adds 2. Seeding semantics.
#[test]
fn loop_local_false_branch_sees_the_seed() {
    let (out, code) = build_link_run(
        "loop_local_seed",
        r#"
let done: Bool = false;
node main [done == false][done == true] {
    let i: Int = 0;
    let up: Int = 0;
    when up > 0 {
        i = 9;
    };
    when i < 5 {
        i = i + 2;
    };
    println!(i);
    done = true;
    term;
};
"#,
    );
    assert_eq!(code, 0, "exit code");
    assert_eq!(out.trim(), "2", "0 (seed, when skipped) + 2");
}
