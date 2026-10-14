// Payload-ABI behavioral tests — tier2 phase 1
// (docs/plans/2026-10-14-daily-driver-tier2.md).
//
// Four symptoms, one family: a value crossing a boundary loses/gains a box.
// Each test compiles at -O0 (the IR-validity gate — the LTO lesson,
// inverted: -O0 rejects the undefined-symbol/dead-store class that -O3
// -flto eliminates) and pins the printed result at a print boundary.
//
//   probe C: let-bound List CONSTRUCT-then-INSERT in a node body — the
//            construction pre-pass fired on `~<-` extract and `= 0` init
//            but not on first `<-` insert (uninitialized handle slot).
//   probe D: a List-payload Result consumed through a let binding.
//   probe A: a CONCRETE pushed into a List<dyn Trait> — the push site must
//            box (tag + fat pointer), the gate must admit the dyn coercion
//            (coll-element-size ADDENDUM 3 hole 1).
//   probe B: a dyn list crossing a defn-call boundary (ADDENDUM 3 hole 2).

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
    // -O0: the IR-validity gate. -O3 -flto eliminates the dead-store /
    // undefined-symbol class this suite exists to catch.
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

/// `let xs: List<Int>;` + `<-` inserts in a node body: the construction
/// pre-pass must build the empty list at the declaration. Prints the count.
#[test]
fn tier2_let_bound_list_construct_then_insert() {
    let (out, code) = build_link_run(
        "let_list_insert",
        r#"
import { List } from "std/collections.bv";

let done: Bool = false;
node main [done == false][done == true] {
    let xs: List<Int>;
    xs <- 5;
    xs <- 7;
    let n: Int = xs.Count#();
    println!(n);
    done = true;
    term;
};
"#,
    );
    assert_eq!(code, 0, "exit code");
    assert_eq!(out.trim(), "2", "both inserts landed");
}

/// A `Result<List<Int>, Int>` consumed through a let binding + unwrap —
/// the shape that mis-lowered (and the Int-fallback typecheck hole under it).
#[test]
fn tier2_list_payload_result_through_let() {
    let (out, code) = build_link_run(
        "list_payload_result",
        r#"
import { List } from "std/collections.bv";
import { Result, Ok, is_ok, unwrap } from "std/result.bv";

defn make() -> Result<List<Int>, Int> [xs.Count#() == 2][term.Count#() == 2] {
    let xs: List<Int>;
    xs <- 1;
    xs <- 2;
    term Ok(xs);
};

let done: Bool = false;
node main [done == false][done == true] {
    let r: Result<List<Int>, Int> = make();
    when is_ok(r) {
        let data: List<Int> = unwrap(r);
        println!(data.Count#());
    };
    done = true;
    term;
};
"#,
    );
    assert_eq!(code, 0, "exit code");
    assert_eq!(out.trim(), "2", "payload list survived the Result box");
}

/// A CONCRETE pushed into a `List<dyn Trait>`: boxed at the push site
/// (tag + fat pointer); dispatch reads the right concrete per element.
/// Dog.greet = base*100 + t → 7*100+1 = 701; Cat.greet = lives*10 + t →
/// 9*10+2 = 92.
#[test]
fn tier2_dyn_push_boxes_the_concrete() {
    let (out, code) = build_link_run(
        "dyn_push",
        r#"
trait Greeter {
    defn greet(me: Self, times: Int) -> Int;
};
type Dog: Greeter { base: Int; };
type Cat: Greeter { lives: Int; };
impl Dog {
    defn greet(me: Dog, times: Int) -> Int { term me.base * 100 + times; }
};
impl Cat {
    defn greet(me: Cat, times: Int) -> Int { term me.lives * 10 + times; }
};
import { List } from "std/collections.bv";

let done: Bool = false;
node main [done == false][done == true] {
    let hs: List<dyn Greeter>;
    hs <- Dog { base: 7 };
    hs <- Cat { lives: 9 };
    let g0: Int = hs[0].greet(1);
    println!(g0);
    let g1: Int = hs[1].greet(2);
    println!(g1);
    done = true;
    term;
};
"#,
    );
    assert_eq!(code, 0, "exit code");
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines, vec!["701", "92"], "per-element dyn dispatch");
}

/// A dyn list crossing a defn-call boundary keeps its boxed elements
/// (ADDENDUM 3 hole 2). Dog.greet(3) = 703.
#[test]
fn tier2_dyn_list_across_call_boundary() {
    let (out, code) = build_link_run(
        "dyn_call_arg",
        r#"
trait Greeter {
    defn greet(me: Self, times: Int) -> Int;
};
type Dog: Greeter { base: Int; };
type Cat: Greeter { lives: Int; };
impl Dog {
    defn greet(me: Dog, times: Int) -> Int { term me.base * 100 + times; }
};
impl Cat {
    defn greet(me: Cat, times: Int) -> Int { term me.lives * 10 + times; }
};
import { List } from "std/collections.bv";

defn first_greet(hs: List<dyn Greeter>, t: Int) -> Int [hs.Count#() > 0][term >= 0] {
    term hs[0].greet(t);
};

let done: Bool = false;
node main [done == false][done == true] {
    let hs: List<dyn Greeter>;
    hs <- Dog { base: 7 };
    hs <- Cat { lives: 9 };
    let a: Int = first_greet(hs, 3);
    println!(a);
    done = true;
    term;
};
"#,
    );
    assert_eq!(code, 0, "exit code");
    assert_eq!(out.trim(), "703", "element dispatch through the call");
}
