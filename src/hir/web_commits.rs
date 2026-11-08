// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 inventory port: possible global writes at browser commit boundaries
//! over typed HIR. Mirror of `analysis::web_commits` (AST, still
//! authoritative until the consumer flip); output type shared so the
//! consumer flip needs no adapter.
//!
//! Coverage mirrors (authored by Otto Westerlund; adopted with attribution,
//! see docs/contributors/otto-westerlund.md): fields = state decls +
//! top-level lets (non-exported); txns = effective transactions (direct +
//! sync-wrapped + export-wrapped); defns = bare top-level definitions incl.
//! export-wrapped (sync-wrapped defns are NOT recorded — the AST
//! `record_definition` has no SyncGroup arm).

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::HirStmtKind;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
struct Summaries {
    writes: BTreeMap<String, HashSet<String>>,
    calls: BTreeMap<String, HashSet<String>>,
}

pub fn analyze(program: &HirProgram) -> HashMap<String, HashSet<String>> {
    let fields: HashSet<String> = program.globals.iter().map(|g| g.name.clone()).collect();
    let mut summaries = Summaries::default();
    for txn in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Transaction) && c.owner.is_none()
    }) {
        record(txn, &fields, &mut summaries);
    }
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Defn)
            && c.owner.is_none()
            && c.sync_domains.is_empty()
    }) {
        record(c, &fields, &mut summaries);
    }
    loop {
        let previous = summaries.writes.clone();
        propagate_round(&mut summaries, &previous);
        if summaries.writes == previous {
            break;
        }
    }
    summaries.writes.into_iter().collect()
}

fn propagate_round(summaries: &mut Summaries, previous: &BTreeMap<String, HashSet<String>>) {
    for (name, calls) in &summaries.calls {
        let writes = summaries.writes.entry(name.clone()).or_default();
        extend_from_callees(calls, previous, writes);
    }
}

fn extend_from_callees(
    calls: &HashSet<String>,
    previous: &BTreeMap<String, HashSet<String>>,
    writes: &mut HashSet<String>,
) {
    for callee in calls {
        if let Some(callee_writes) = previous.get(callee) {
            writes.extend(callee_writes.iter().cloned());
        }
    }
}

fn record(c: &HirCallable, fields: &HashSet<String>, summaries: &mut Summaries) {
    let shadowed: HashSet<&str> = c.params.iter().map(|p| p.name.as_str()).collect();
    let mut writes = HashSet::new();
    let mut calls = HashSet::new();
    let mut collect = |target: &HirExpr| {
        let Some(root) = root_name(target) else { return };
        if fields.contains(root) && !shadowed.contains(root) {
            writes.insert(root.to_string());
        }
    };
    walk_assign_targets(&c.body.statements, &mut collect);
    super::expr_walk::walk_stmts(&c.body.statements, &mut |expr| {
        record_expr(expr, &mut calls, &mut collect);
    });
    summaries.writes.insert(c.name.to_string(), writes);
    summaries.calls.insert(c.name.to_string(), calls);
}

fn record_expr(
    expr: &HirExpr,
    calls: &mut HashSet<String>,
    collect: &mut dyn FnMut(&HirExpr),
) {
    if let HirExprKind::Call { callee, .. } = &expr.kind {
        calls.insert(callee.clone());
    }
    if let HirExprKind::Block(stmts) = &expr.kind {
        walk_assign_targets(stmts, collect);
    }
}

fn root_name(expr: &HirExpr) -> Option<&str> {
    match &expr.kind {
        HirExprKind::Identifier(name) => Some(name),
        HirExprKind::Field { receiver: base, .. } | HirExprKind::Index(base, _) => root_name(base),
        _ => None,
    }
}

fn walk_assign_targets(body: &[super::stmt::HirStmt], f: &mut dyn FnMut(&HirExpr)) {
    for stmt in body {
        walk_one_assign_target(stmt, f);
    }
}

fn walk_one_assign_target(stmt: &super::stmt::HirStmt, f: &mut dyn FnMut(&HirExpr)) {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, _) => f(lhs),
        HirStmtKind::ArrowAssign { target: Some(t), .. } => f(t),
        HirStmtKind::Guarded(_, inner)
        | HirStmtKind::Block(inner)
        | HirStmtKind::SyncBlock(inner)
        | HirStmtKind::Defer(inner)
        | HirStmtKind::Mutex(inner)
        | HirStmtKind::Foreach { body: inner, .. } => walk_assign_targets(inner, f),
        HirStmtKind::Match { arms, .. } => {
            for arm in arms {
                walk_assign_targets(&arm.body, f);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_program;

    #[test]
    fn helper_writes_cross_exports_guards_and_recursive_calls() {
        // Mirrors the AST analysis unit test (analysis::web_commits).
        let source = "let visible: Bool = false; out defn helper(n: Int) -> Int { when n > 0 { visible = true; term helper(n-1); }; term 0; }; txn click [!visible][visible] { helper(2); term; };";
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source).parse_program().unwrap();
        let hir = lower_program(&items);
        let writes = analyze(&hir);
        assert_eq!(writes["click"], HashSet::from(["visible".to_string()]));
        assert_eq!(writes["helper"], writes["click"]);
        // Parity with the AST analysis on the same source.
        assert_eq!(writes, crate::analysis::web_commits::analyze(&items));
    }
}
