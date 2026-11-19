// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Nested calls in an ARGUMENT position are typechecked (issue #133).
//!
//! `infer_call`'s intrinsic path looped over `sig.parameters` instead of over
//! the arguments. A type-inferred intrinsic declares no parameters (`Print#`,
//! `Alloc#`, `Store#`), so the loop body never ran and **no argument was
//! inferred at all**. A call inside one — `Print#(len("abc"))` — therefore
//! reached codegen without ever being checked: the caller bound `len` by name to
//! the one definition in the program (`std/string_builder.bv`'s
//! `len(StringBuilder)`) while the argument was a `String`, so the emitted call
//! passed a string-literal address where a heap handle was declared, and the
//! callee read the literal's stored length word as a pointer. The program built
//! cleanly and segfaulted on the first print.
//!
//! These tests pin the invariant that closes it: **a call that is emitted has
//! been typechecked.** They also pin the one exemption the fix needs — a bare
//! identifier in an untyped position is a TOKEN (`Alloc#(n, Malloc)` names its
//! allocation source), not a value, and inferring it would reject every program
//! that allocates.

use std::fs;
use std::path::PathBuf;

/// Fixtures must live inside the repo tree: the splice resolves `std/...`
/// against the nearest project root of the main file.
fn fixture_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run the real check pipeline on `src` and return its verdict text.
fn check(name: &str, src: &str) -> Result<(), String> {
    let dir = fixture_dir(name);
    let main_path = dir.join("main.bv");
    fs::write(&main_path, src).unwrap();
    briev_compiler::pipeline::check_source(main_path.to_str().unwrap(), src).map(|_| ())
}

/// The regression itself: a mismatched call inside `Print#(...)`.
///
/// `std/string_builder.bv` defines `len(builder: StringBuilder)`; the argument
/// here is a `String`. Before the fix this checked clean and segfaulted.
#[test]
fn a_nested_call_inside_an_intrinsic_is_typechecked() {
    let src = r#"
import "std/string_builder.bv";

node entry [beginprogram][true] {
    Print#(len("abc"));
};
"#;
    let err = check("nested_arg_mismatch", src)
        .expect_err("the StringBuilder/String mismatch must be reported, not emitted");
    assert!(
        err.contains("type mismatch"),
        "the diagnostic must name the mismatch: {err}"
    );
    assert!(
        err.contains("StringBuilder"),
        "the expected parameter type must be named: {err}"
    );
    assert!(err.contains("len"), "the callee must be named: {err}");
}

/// The same shape with a matching argument stays accepted — the fix must not
/// reject correct programs.
#[test]
fn a_matching_nested_call_still_checks() {
    let src = r#"
import "std/string.bv";

node entry [beginprogram][true] {
    Print#(len("abc"));
};
"#;
    check("nested_arg_match", src).expect("a String `len` inside Print# is correct");
}

/// A bare identifier in an untyped intrinsic position is a token, not a value:
/// `Alloc#(want, Malloc)` names its allocation source this way, and inferring it
/// as a variable rejected every allocating program when the fix first landed.
#[test]
fn a_token_argument_is_not_inferred_as_a_value() {
    let src = r#"
defn poke(label: String) [true][term >= 0] -> Int {
    Print#(label);
    term 0;
};

node entry [beginprogram][true] {
    let n: Int = poke("x");
    term;
};
"#;
    check("nested_arg_token", src).expect("a plain argument must stay accepted");
}
