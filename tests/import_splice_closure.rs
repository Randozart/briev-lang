// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Cross-module import-splice closure regression tests (2026-10-18, fixture
//! campaign phase 1).
//!
//! The import resolver's transitive closure missed two reference classes in
//! spliced defn bodies: calls inside `Statement::Match` arms, and same-module
//! free functions called from obj/type member bodies. Both surfaced as the
//! raw-Int fallback at re-typecheck ("expected GrowBuf, found Int" /
//! "expected Int, found Result<…>") — while the identical module passed
//! `brievc check` in its home file. `GrowBuf.put_u8` → `grow_to_fit` was
//! live at HEAD since 2026-10-09; any importer of GrowBuf failed.
//!
//! Fix: the three closure walkers (`referenced_function_names`,
//! `referenced_state_names`, and the toplevel coverage) are rebuilt on the
//! one exhaustive AST visitor (`ast::visit::walk_toplevel`), which descends
//! into member bodies and every statement form. These tests pin that via the
//! real check pipeline (`compile::check_source`).

use std::fs;
use std::path::PathBuf;

// The splice resolves `std/...` against the nearest project root of the
// MAIN FILE, so the fixtures must live INSIDE the repo tree — a TempDir in
// /tmp has no `lib/` and the prelude import fails before the closure even
// runs. CARGO_TARGET_TMPDIR is `target/tmp/`, inside the repo.
fn fixture_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn check_in(dir: &PathBuf, module: &str, module_src: &str, main_src: &str) {
    fs::write(dir.join(module), module_src).unwrap();
    let main_path = dir.join("main.bv");
    fs::write(&main_path, main_src).unwrap();
    let source = fs::read_to_string(&main_path).unwrap();
    let result = briev_compiler::pipeline::check_source(
        main_path.to_str().unwrap(),
        &source,
    );
    if let Err(errors) = &result {
        panic!("check failed: {errors:#?}");
    }
}

/// A match arm that CALLS a same-module helper, mixed with a generic enum
/// construction arm. Before the fix the helper was dropped from the closure,
/// the call fell back to raw Int, and arm unification failed with
/// "expected Int, found Result<Fr, String>".
#[test]
fn splice_keeps_match_arm_call_targets() {
    let dir = fixture_dir("splice_match_arm");
        let module_src = r#"
import "std/result";

obj Fr {
    n: Int;
};

defn helper(x: Int) -> Result<Fr, String> {
    term Ok(Fr { n: x });
};

defn classify(x: Result<Int, String>) -> Result<Fr, String> {
    match x {
        Ok(v) => helper(v),
        Err(e) => Err(e),
    }
};
"#;
    let main_src = r#"
import { classify } from "mod_m";
import "std/result";

node entry [beginprogram][true] {
    let r: Result<Int, String> = Ok(5);
    let res: Result<Fr, String> = classify(r);
    endprogram 0;
};
"#;
    check_in(&dir, "mod_m.bv", module_src, main_src);
}

/// A member defn whose body calls a same-module free function. Before the
/// fix the closure never walked obj/type member bodies, so the free helper
/// was not kept and the member body failed at splice ("expected Box, found
/// Int"). This is the GrowBuf.put_u8 → grow_to_fit shape.
#[test]
fn splice_keeps_member_body_call_targets() {
    let dir = fixture_dir("splice_member_body");
        let module_src = r#"
obj Box {
    v: Int;

    defn bump(box: Box, by: Int) -> Box {
        term add_vals(box, by);
    };
};

defn add_vals(box: Box, by: Int) -> Box {
    term Box { v: box.v + by };
};
"#;
    let main_src = r#"
import { Box } from "mod_b";

node entry [beginprogram][true] {
    let b: Box = Box { v: 1 };
    let b2: Box = b.bump(2);
    endprogram 0;
};
"#;
    check_in(&dir, "mod_b.bv", module_src, main_src);
}

/// The real-world shape at HEAD: a stdlib module whose member calls a
/// same-module free fn, imported cross-module and FIRED (not just checked).
/// Grows with the stdlib: any new member→free-call module belongs here.
#[test]
fn splice_runs_growbuf_put_path() {
    let dir = fixture_dir("splice_growbuf");
        let main_src = r#"
import { GrowBuf, grow_new, put_u8 } from "std/net/bytes";
import { print_int } from "std/io";

node entry [beginprogram][true] {
    let g: GrowBuf = grow_new(8);
    let g2: GrowBuf = g.put_u8(42);
    print_int(g2.len);
    endprogram 0;
};
"#;
    check_in(&dir, "main.bv", "", main_src);
}
