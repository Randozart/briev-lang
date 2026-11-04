// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.5 parity: HIR task segmentation / spawn targets / task ABI must match.

use briev_compiler::analysis::task_segments::{
    collect_spawn_targets as ast_targets, collect_task_abi_errors as ast_abi,
    mentions_param_field as ast_mentions, split_task_body as ast_split,
};
use briev_compiler::ast::{Statement, TopLevel};
use briev_compiler::hir::lower_program;
use briev_compiler::hir::stmt::{HirStmt, HirStmtKind};
use briev_compiler::hir::task_segments::{
    collect_spawn_targets as hir_targets, collect_task_abi_errors as hir_abi,
    mentions_param_field as hir_mentions, split_task_body as hir_split,
};
use briev_compiler::hir::HirCallableKind;
use briev_compiler::type_universe::TypeUniverse;
use std::path::{Path, PathBuf};

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = briev_compiler::lexer::tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn bv_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            bv_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("bv") {
            out.push(path);
        }
    }
}

fn ast_tag(s: &Statement) -> &'static str {
    match s {
        Statement::Let { .. } => "let",
        Statement::Assign(..) => "assign",
        Statement::ArrowAssign { .. } => "arrow",
        Statement::Term(_) => "term",
        Statement::Tail(_) => "tail",
        Statement::Check(_) => "check",
        Statement::Expression(_) => "expr",
        Statement::Guarded(..) => "guarded",
        Statement::Block(_) => "block",
        Statement::Foreach { .. } => "foreach",
        Statement::Match { .. } => "match",
        Statement::Yield => "yield",
        Statement::EndProgram(_) => "endprogram",
        Statement::Gate(_) => "gate",
        Statement::Rollback(_) => "rollback",
        Statement::SyncBlock(_) => "sync",
        Statement::Mutex(_) => "mutex",
        Statement::Defer(_) => "defer",
        Statement::Open(..) => "open",
        _ => "other",
    }
}

fn hir_tag(s: &HirStmt) -> &'static str {
    match &s.kind {
        HirStmtKind::Let { .. } => "let",
        HirStmtKind::Assign(..) => "assign",
        HirStmtKind::ArrowAssign { .. } => "arrow",
        HirStmtKind::Term(_) => "term",
        HirStmtKind::Tail(_) => "tail",
        HirStmtKind::Check(_) => "check",
        HirStmtKind::Expression(_) => "expr",
        HirStmtKind::Guarded(..) => "guarded",
        HirStmtKind::Block(_) => "block",
        HirStmtKind::Foreach { .. } => "foreach",
        HirStmtKind::Match { .. } => "match",
        HirStmtKind::Yield => "yield",
        HirStmtKind::EndProgram(_) => "endprogram",
        HirStmtKind::Gate(_) => "gate",
        HirStmtKind::Rollback(_) => "rollback",
        HirStmtKind::SyncBlock(_) => "sync",
        HirStmtKind::Mutex(_) => "mutex",
        HirStmtKind::Defer(_) => "defer",
        HirStmtKind::Open(..) => "open",
        _ => "other",
    }
}

fn compare(src: &str) -> Vec<String> {
    let items = parse(src);
    let program = lower_program(&items);
    let mut bad = Vec::new();

    let a_targets = ast_targets(&items);
    let h_targets = hir_targets(&program);
    if a_targets != h_targets {
        bad.push(format!("spawn targets ast={a_targets:?} hir={h_targets:?}"));
    }

    let universe = TypeUniverse::new();
    let a_abi = ast_abi(&items, &universe);
    let h_abi = hir_abi(&program, &universe);
    if a_abi != h_abi {
        bad.push(format!("task ABI ast={a_abi:?} hir={h_abi:?}"));
    }

    // Duplicate defn names exist in the corpus; match the Nth occurrence.
    let mut by_name: std::collections::HashMap<&str, Vec<&briev_compiler::hir::HirCallable>> =
        std::collections::HashMap::new();
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Defn) && c.owner.is_none()
    }) {
        by_name.entry(c.name.as_str()).or_default().push(c);
    }
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for item in &items {
        let TopLevel::Definition(d) = item else { continue };
        let occurrence = seen.entry(d.name.as_str()).or_insert(0);
        let Some(h) = by_name.get(d.name.as_str()).and_then(|v| v.get(*occurrence)) else {
            *occurrence += 1;
            continue;
        };
        *occurrence += 1;
        let params: Vec<String> = d.parameters.iter().map(|(n, _)| n.clone()).collect();
        let a_segs = ast_split(&d.body, &params);
        let h_segs = hir_split(&h.body.statements, &params);
        let a_shape: Vec<Vec<&str>> = a_segs
            .iter()
            .map(|s| s.iter().map(ast_tag).collect())
            .collect();
        let h_shape: Vec<Vec<&str>> = h_segs
            .iter()
            .map(|s| s.iter().map(hir_tag).collect())
            .collect();
        if a_shape != h_shape {
            bad.push(format!("{}: segments ast={a_shape:?} hir={h_shape:?}", d.name));
        }
        if a_segs.len() == h_segs.len() {
            for (i, (asg, hsg)) in a_segs.iter().zip(h_segs.iter()).enumerate() {
                for (j, (ast, hst)) in asg.iter().zip(hsg.iter()).enumerate() {
                    let am = ast_mentions(ast, &params);
                    let hm = hir_mentions(hst, &params);
                    if am != hm {
                        bad.push(format!("{}: seg {i} stmt {j} mentions ast={am} hir={hm}", d.name));
                    }
                }
            }
        }
    }
    bad
}

#[test]
fn curated_task_segments_parity() {
    let src = "defn job(n: Int, w: Evt) -> Int { let a: Int = n; w <- a; yield; term n; };\n\
               defn spinner(ch: Chan) -> Int { side(); yield; term ch.wire.v; };\n\
               defn spawner() -> Int { let t: Int = spawn job(1, w); term t; };";
    let bad = compare(src);
    assert!(bad.is_empty(), "HIR task-segment parity:\n{}", bad.join("\n"));
}

#[test]
fn corpus_task_segments_parity() {
    let root = Path::new(PROJECT_ROOT);
    let mut files = Vec::new();
    bv_files(&root.join("benchmarks"), &mut files);
    bv_files(&root.join("tests/tier1"), &mut files);
    bv_files(&root.join("lib/std"), &mut files);
    files.sort();
    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut mismatches = Vec::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else { continue };
        let Ok(items) = std::panic::catch_unwind(|| parse(&source)) else { continue };
        let program = lower_program(&items);
        if briev_compiler::hir::unsupported_counts(&program).total() != 0 {
            skipped += 1;
            continue;
        }
        compared += 1;
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        for b in compare(&source) {
            mismatches.push(format!("{rel}: {b}"));
        }
    }
    println!(
        "HIR_TASK_SEGMENTS_PARITY compared={compared} skipped_unsupported={skipped} mismatches={}",
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "HIR task-segment mismatches:\n{}", mismatches.join("\n"));
}
