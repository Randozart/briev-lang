// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.2 parity: HIR `defn_liveness` must equal the AST analysis (still
//! authoritative) for roots, closure, construction-rooted members, intrinsic
//! helpers, and the `endprogram` provider.

use briev_compiler::analysis::defn_liveness::DefnLiveness as AstLiveness;
use briev_compiler::ast::TopLevel;
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");
use briev_compiler::hir::defn_liveness::DefnLiveness as HirLiveness;
use briev_compiler::hir::{lower_program, unsupported_counts};

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn assert_parity(src: &str, label: &str) {
    let items = parse(src);
    let ast = AstLiveness::build(&items);
    let hir = HirLiveness::build(&lower_program(&items));
    assert_eq!(ast.keep_all, hir.keep_all, "{label}: keep_all parity");
    assert_eq!(ast.live, hir.live, "{label}: live-set parity");
}

#[test]
fn curated_defn_liveness_parity() {
    // A dead defn is emitted by neither pass.
    assert_parity("defn dead() -> Int { term 1; };", "dead");

    // Exports are roots.
    assert_parity("export defn live() -> Int { term 1; };", "export");

    // Reactive txns are reactor-dispatched roots.
    assert_parity(
        "defn helper() -> Int { term 1; };\nnode n [true][true] { let x: Int = helper(); term; };",
        "reactive-txn",
    );

    // Explicit call closure from a rooted export.
    assert_parity(
        "defn a() -> Int { term b(); };\ndefn b() -> Int { term 1; };\nexport defn c() -> Int { term a(); };",
        "closure",
    );

    // `endprogram` roots the exit provider.
    assert_parity(
        "defn exit_prog(x: Int) -> Int { term x; };\nnode n [true][true] { endprogram 0; };",
        "endprogram",
    );

    // Intrinsic helpers are derived from the shared table.
    assert_parity(
        "defn print_int(x: Int) -> Int { term x; };\nexport defn show() -> Int { Print#(1); term 0; };",
        "print-helper",
    );

    // Construction roots a type's behavioral members.
    assert_parity(
        "obj C { v: Int; defn m(this: C) -> Int { term 1; }; };\nexport defn make() -> C { term C { v: 1 }; };",
        "construction",
    );

    // 2026-11-04 (PR #62): runtime `.^Length` evaluates its receiver, so a
    // call under it is live (the pre-fix AST/HIR both dropped it).
    assert_parity(
        "defn receiver() -> String { term \"x\"; };\nnode n [true][true] { let len: Int = receiver().^Length; Print#(len); term; };",
        "runtime-reflect",
    );

    // 2026-11-05: `proto` cast-edge/cross-op bindings (`op Concat = ...`)
    // are emission roots; HIR must carry them or a constructed boundary type
    // loses its binding fns (caught on pp-types by the debug cross-check).
    assert_parity(include_str!("../lib/glue/c.bv"), "protocol-bindings");

    // 2026-11-05: trait defaults are never bare-name live (the backend emits
    // specialized copies); calls to them must not root them (caught by the
    // full-corpus debug sweep on byte_sink/net_stack/trait_generics).
    assert_parity(
        include_str!("../tests/tier1/byte_sink_trait_smoke.bv"),
        "trait-defaults",
    );

    // Binary `==` roots the string-equality helper when present.
    assert_parity(
        "defn briev_str_eq(a: Int, b: Int) -> Int { term 1; };\nexport defn cmp(x: Int, y: Int) -> Bool { term x == y; };",
        "eq-helper",
    );
}

/// Walk `traceable.bv`-bearing directories for the corpus parity test.
fn bv_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            bv_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("bv") {
            out.push(path);
        }
    }
}

/// H3.2 over the CORPUS, not just the curated shapes above.
///
/// 2026-11-18: the curated cases all agreed while `lib/std/os/dir.bv` did not
/// (`BUGS.md` "HIR/AST defn-liveness drift"), because the shapes that reproduce
/// it are not among them. A gate that only tests shapes chosen by the same
/// reasoning that produced the bug cannot find the bug — this one walks the real
/// library and the tier1 corpus.
///
/// Files whose HIR lowering is incomplete are skipped and counted, exactly like
/// `hir_needs_state_parity`: the parity question is only meaningful once both
/// passes can see the same program.
#[test]
fn corpus_defn_liveness_parity() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    files.sort();

    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(items) = std::panic::catch_unwind(|| parse(&source)) else {
            continue;
        };
        let program = lower_program(&items);
        if unsupported_counts(&program).total() != 0 {
            skipped += 1;
            continue;
        }
        compared += 1;
        let ast = AstLiveness::build(&items);
        let hir = HirLiveness::build(&program);
        if ast.keep_all != hir.keep_all || ast.live != hir.live {
            let rel = path.strip_prefix(root).unwrap_or(path).display();
            let mut ast_only: Vec<String> = ast.live.difference(&hir.live).cloned().collect();
            let mut hir_only: Vec<String> = hir.live.difference(&ast.live).cloned().collect();
            ast_only.sort();
            hir_only.sort();
            mismatches.push(format!(
                "{rel}\n  keep_all: ast={} hir={}\n  ast-only: {ast_only:?}\n  hir-only: {hir_only:?}",
                ast.keep_all, hir.keep_all
            ));
        }
    }

    eprintln!(
        "CORPUS_DEFN_LIVENESS_PARITY compared={compared} skipped={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "HIR/AST defn-liveness mismatches:\n{}",
        mismatches.join("\n")
    );
}

/// The drift's *actual* reproduction: `dir.bv` compiled as the ROOT file.
///
/// 2026-11-18: `corpus_defn_liveness_parity` above compares the RAW parse and is
/// green (154 files, 0 mismatches) — the drift is not visible there. It appears
/// only on the RESOLVED program: with the prelude merged in, the HIR live set is
/// 24 names against the AST set's 83, losing `dir_open`/`dir_close`/`dir_loop`
/// and the whole cast/format family. That is why this test drives the real
/// binary instead of calling the analysis directly.
///
/// Ignored rather than deleted: it is the repro for a live bug recorded in
/// `BUGS.md` ("2026-11-16 — HIR/AST defn-liveness drift on lib/std/os/dir.bv").
/// Run it with `--ignored` to see the failure; it should be un-ignored by
/// whoever fixes the drift.
#[test]
#[ignore = "reproduces BUGS.md 2026-11-16 defn-liveness drift; run with --ignored"]
fn dir_bv_as_root_does_not_trip_the_liveness_assert() {
    let root = Path::new(PROJECT_ROOT);
    let out = std::env::temp_dir().join("briev_dir_liveness_repro");
    let _ = std::fs::create_dir_all(&out);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_brievc"))
        .args([
            "build",
            "lib/std/os/dir.bv",
            "--no-link",
            "--out",
            &out.to_string_lossy(),
        ])
        .env("BRIEV_STDLIB_PATH", root.join("lib"))
        .current_dir(root)
        .output()
        .expect("run brievc");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("defn-liveness drift"),
        "the HIR/AST defn-liveness drift is still here:\n{stderr}"
    );
}
