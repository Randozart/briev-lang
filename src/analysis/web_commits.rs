//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Possible global writes at browser commit boundaries, including helper calls.
//! Authored by Otto Westerlund (@odecode); adopted with attribution, see
//! docs/contributors/otto-westerlund.md.
//! 2026-10-21: compute the closure once in the frontend; codegen consumes it.
//! These sets select DOM updates, not optimization or concurrency proofs.
use crate::ast::{Expr, Statement, TopLevel, Type};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
struct Summaries {
    writes: BTreeMap<String, HashSet<String>>,
    calls: BTreeMap<String, HashSet<String>>,
}

pub fn analyze(items: &[TopLevel]) -> HashMap<String, HashSet<String>> {
    let fields: HashSet<String> = items.iter().filter_map(|item| match item {
        TopLevel::StateDecl(state) => Some(state.name.clone()),
        TopLevel::Statement(stmt) => match stmt.as_ref() {
            Statement::Let { name, .. } => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }).collect();
    let mut summaries = Summaries::default();
    for txn in crate::analysis::effective_txns(items) {
        record(&txn.name, &txn.parameters, &txn.body, &fields, &mut summaries);
    }
    for item in items { record_definition(item, &fields, &mut summaries); }
    loop {
        let previous = summaries.writes.clone();
        propagate_round(&mut summaries, &previous);
        if summaries.writes == previous { break; }
    }
    summaries.writes.into_iter().collect()
}

fn propagate_round(summaries: &mut Summaries, previous: &BTreeMap<String, HashSet<String>>) {
    for (name, calls) in &summaries.calls {
        let writes = summaries.writes.entry(name.clone()).or_default();
        extend_callee_writes(writes, calls, previous);
    }
}

fn extend_callee_writes(writes: &mut HashSet<String>, calls: &HashSet<String>, previous: &BTreeMap<String, HashSet<String>>) {
    for callee in calls { writes.extend(previous.get(callee).into_iter().flatten().cloned()); }
}

fn record_definition(item: &TopLevel, fields: &HashSet<String>, summaries: &mut Summaries) {
    match item {
        TopLevel::Definition(defn) => record(&defn.name, &defn.parameters, &defn.body, fields, summaries),
        TopLevel::Export(export) => record_definition(&export.inner, fields, summaries),
        _ => {}
    }
}

fn record(name: &str, params: &[(String, Type)], body: &[Statement], fields: &HashSet<String>, summaries: &mut Summaries) {
    let shadowed: HashSet<&str> = params.iter().map(|(name, _)| name.as_str()).collect();
    let mut writes = HashSet::new();
    let mut calls = HashSet::new();
    let mut collect = |target: &Expr| {
        let Some(root) = root_name(target) else { return; };
        if fields.contains(root) && !shadowed.contains(root) { writes.insert(root.to_string()); }
    };
    crate::ast::visit::walk_assign_targets(body, &mut collect);
    crate::ast::visit::walk_stmts(body, &mut |expr| {
        if let Expr::Call(callee, _, _) = expr { calls.insert(callee.clone()); }
        if let Expr::Block(stmts) = expr { crate::ast::visit::walk_assign_targets(stmts, &mut collect); }
    });
    summaries.writes.insert(name.to_string(), writes);
    summaries.calls.insert(name.to_string(), calls);
}

fn root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Identifier(name) => Some(name),
        Expr::Field(base, _) | Expr::Index(base, _) => root_name(base),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_writes_cross_exports_guards_and_recursive_calls() {
        let source = "let visible: Bool = false; out defn helper(n: Int) -> Int { when n > 0 { visible = true; term helper(n-1); }; term 0; }; txn click [!visible][visible] { helper(2); term; };";
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source).parse_program().unwrap();
        let writes = analyze(&items);
        assert_eq!(writes["click"], HashSet::from(["visible".to_string()]));
        assert_eq!(writes["helper"], writes["click"]);
    }
}
