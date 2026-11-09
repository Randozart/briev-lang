// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! P5 pass-level HIR snapshots. Any unintended change to lowering, typing, or
//! specialization shows up as a reviewable text diff. Refresh intentionally
//! with `UPDATE_SNAPSHOTS=1 cargo test --test hir_snapshots`.

use briev_compiler::casting::graph::CastingGraph;
use briev_compiler::hir::{lower_program, render_callable, render_program, HirProgram};
use briev_compiler::type_universe::TypeUniverse;
use std::path::{Path, PathBuf};

const FIXTURES: &[(&str, &str)] = &[
    (
        "guards",
        r#"
let x: Int = 0;
let done: Bool = false;
node n [done == false][done == true] {
    when x > 0 { x = 1; };
    [x == 0] x = 2;
    done = true;
    term;
};
"#,
    ),
    (
        "foreach_match",
        r#"
defn f(xs: List<Int>) -> Int {
    let acc: Int = 0;
    foreach (v in xs) { acc = acc + v; };
    term match acc { 0 => 0, _ => acc };
};
"#,
    ),
    (
        "collections",
        r#"
struct Point { x: Int; y: Int; };
defn f(p: Point, xs: List<Int>) -> Int { term p.x + xs[0]; };
"#,
    ),
    (
        "generic",
        r#"
defn id<T>(x: T) -> T { term x; };
defn use_it() -> Int { term id(7); };
"#,
    ),
    (
        "defer",
        r#"
let done: Bool = false;
node main [done == false][done == true] {
    defer { println!(7); };
    println!(1);
    done = true;
    term;
};
"#,
    ),
    (
        "laws",
        r#"
let attached: Bool = true;
hold attached { attached = true; }
node m [true][true] { hold true { term; }; };
"#,
    ),
];

fn parse(src: &str) -> Vec<briev_compiler::ast::TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn check(kind: &str, name: &str, text: &str) {
    let path = root().join("tests/snapshots/hir").join(format!("{kind}__{name}.snap"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("snapshot dir");
        std::fs::write(&path, text).expect("write snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing snapshot {}: {e}", path.display()));
    assert_eq!(text, expected, "snapshot drifted: {}", path.display());
}

fn annotate(program: &mut HirProgram) {
    let universe = TypeUniverse::new();
    let graph = CastingGraph::new();
    briev_compiler::hir::typing::annotate_types(program, &universe, &graph);
}

#[test]
fn lowered_program_snapshots() {
    let mut previous = Vec::new();
    for (name, src) in FIXTURES {
        let program = lower_program(&parse(src));
        let text = render_program(&program);
        assert_eq!(text, render_program(&program), "{name}: render is not stable");
        previous.push(text.clone());
        check("lower", name, &text);
    }
    let mut sorted = previous.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), previous.len(), "fixtures collapsed to identical snapshots");
}

#[test]
fn typed_program_snapshots() {
    for (name, src) in FIXTURES {
        let mut program = lower_program(&parse(src));
        annotate(&mut program);
        check("typed", name, &render_program(&program));
    }
}

#[test]
fn specialized_callable_snapshot() {
    let src = "defn id<T>(x: T) -> T { term x; };";
    let program = lower_program(&parse(src));
    let base = program
        .callables
        .iter()
        .find(|c| !c.type_params.is_empty())
        .expect("generic callable");
    let mut subst = std::collections::HashMap::new();
    subst.insert("T".to_string(), briev_compiler::ast::Type::int());
    let mut ids = briev_compiler::hir::ids::IdSupply::default();
    let specialized = briev_compiler::hir::specialize::specialize_with_substitution(
        base,
        &subst,
        "id$Int",
        &std::collections::HashMap::new(),
        &mut ids,
    )
    .expect("specialization");
    let mut out = String::new();
    render_callable(&mut out, &specialized);
    check("specialized", "id_int", &out);
}

#[test]
fn snapshot_directory_is_under_tests() {
    assert!(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/hir").is_dir()
        || std::env::var_os("UPDATE_SNAPSHOTS").is_some());
}
