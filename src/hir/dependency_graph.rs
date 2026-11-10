// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10 (H5 port): variable-level dependency graph over typed HIR.
//!
//! Mirror of `analysis::dependency_graph` (AST remains authoritative until the
//! consumer flip is fully retired): state declarations + trigger bindings seed
//! the variable set, bare reactive transaction bodies contribute read edges,
//! and Kahn's algorithm assigns the deterministic topo order / bit indices the
//! `trg` dirty-flag system consumes.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::dependency_graph::{DependencyError, DependencyGraph};
use std::collections::{HashMap, HashSet, VecDeque};

pub fn build(program: &HirProgram) -> Result<DependencyGraph, DependencyError> {
    let mut graph = DependencyGraph {
        topo_order: Vec::new(),
        bit_index: HashMap::new(),
        dependencies: HashMap::new(),
        dependents: HashMap::new(),
        is_trg: HashSet::new(),
        all_vars: HashSet::new(),
    };

    for global in &program.globals {
        if global.is_state_decl {
            graph.all_vars.insert(global.name.clone());
        }
    }
    for name in program
        .trigger_names
        .iter()
        .chain(program.trigger_bindings.iter())
    {
        graph.all_vars.insert(name.clone());
        graph.is_trg.insert(name.clone());
    }

    // AST parity: only BARE top-level transactions contribute read edges;
    // member/ISR/compile-time callables are not TopLevel::Transaction items.
    for callable in &program.callables {
        if callable.kind != HirCallableKind::Transaction
            || callable.owner.is_some()
            || callable.is_isr
        {
            continue;
        }
        for stmt in &callable.body.statements {
            collect_stmt(stmt, &mut graph);
        }
    }

    let topo = topological_sort(&graph)?;
    graph.topo_order = topo;
    for (i, var) in graph.topo_order.iter().enumerate() {
        graph.bit_index.insert(var.clone(), i);
    }
    Ok(graph)
}

fn collect_stmt(stmt: &HirStmt, graph: &mut DependencyGraph) {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, rhs) => {
            let lhs_name = match &lhs.kind {
                HirExprKind::Identifier(name) => Some(name.clone()),
                _ => None,
            };
            let rhs_ids: Vec<String> = collect_expr_identifiers(rhs)
                .into_iter()
                .filter(|r| graph.all_vars.contains(r))
                .collect();
            let Some(lhs) = lhs_name else { return };
            if !graph.all_vars.contains(&lhs) || rhs_ids.is_empty() {
                return;
            }
            graph.dependencies.entry(lhs.clone()).or_default().extend(rhs_ids.clone());
            for dep in &rhs_ids {
                graph.dependents.entry(dep.clone()).or_default().push(lhs.clone());
            }
        }
        HirStmtKind::Let { name, expr: Some(e), .. } => {
            let rhs_ids: Vec<String> = collect_expr_identifiers(e)
                .into_iter()
                .filter(|r| graph.all_vars.contains(r))
                .collect();
            if !graph.all_vars.contains(name) || rhs_ids.is_empty() {
                return;
            }
            graph.dependencies.entry(name.clone()).or_default().extend(rhs_ids.clone());
            for dep in &rhs_ids {
                graph.dependents.entry(dep.clone()).or_default().push(name.clone());
            }
        }
        HirStmtKind::Guarded(_, body) => {
            for s in body {
                collect_stmt(s, graph);
            }
        }
        // AST parity: Expression statements are reads tracked passively
        // (no edge); other statement kinds contribute nothing.
        _ => {}
    }
}

pub fn collect_expr_identifiers(expr: &HirExpr) -> Vec<String> {
    let mut ids = Vec::new();
    collect_expr_ids_inner(expr, &mut ids);
    ids
}

fn collect_expr_ids_inner(expr: &HirExpr, ids: &mut Vec<String>) {
    match &expr.kind {
        HirExprKind::Identifier(n) => ids.push(n.clone()),
        HirExprKind::Binary { left, right, .. } => {
            collect_expr_ids_inner(left, ids);
            collect_expr_ids_inner(right, ids);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. } => collect_expr_ids_inner(inner, ids),
        HirExprKind::Call { args, .. }
        | HirExprKind::List(args)
        | HirExprKind::Tuple(args)
        | HirExprKind::Spawn { args, .. } => collect_exprs(args, ids),
        HirExprKind::Index(a, b) => {
            collect_expr_ids_inner(a, ids);
            collect_expr_ids_inner(b, ids);
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_expr_ids_inner(scrutinee, ids);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_ids_inner(guard, ids);
                }
                collect_expr_ids_inner(&arm.body, ids);
            }
        }
        HirExprKind::Block(stmts) => {
            for stmt in stmts {
                collect_stmt_ids(stmt, ids);
            }
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_expr_ids_inner(condition, ids);
            collect_expr_ids_inner(then_branch, ids);
            if let Some(other) = else_branch {
                collect_expr_ids_inner(other, ids);
            }
        }
        HirExprKind::Lambda { body, .. } => collect_expr_ids_inner(body, ids),
        HirExprKind::MethodCall { receiver, args, .. } => {
            collect_expr_ids_inner(receiver, ids);
            collect_exprs(args, ids);
        }
        HirExprKind::PluginIntercept { args, .. } => collect_exprs(args, ids),
        HirExprKind::Slice { array, start, end, stride } => {
            collect_expr_ids_inner(array, ids);
            for part in [start, end, stride].into_iter().flatten() {
                collect_expr_ids_inner(part, ids);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            collect_expr_ids_inner(start, ids);
            collect_expr_ids_inner(end, ids);
        }
        // Literal leaves and StructLiteral (AST parity) contribute no ids.
        _ => {}
    }
}

fn collect_exprs(exprs: &[HirExpr], ids: &mut Vec<String>) {
    for e in exprs {
        collect_expr_ids_inner(e, ids);
    }
}

fn collect_stmt_ids(stmt: &HirStmt, ids: &mut Vec<String>) {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, rhs) => {
            if let HirExprKind::Identifier(name) = &lhs.kind {
                ids.push(name.clone());
            }
            collect_expr_ids_inner(rhs, ids);
        }
        HirStmtKind::Let { name, expr, .. } => {
            ids.push(name.clone());
            if let Some(e) = expr {
                collect_expr_ids_inner(e, ids);
            }
        }
        HirStmtKind::Guarded(_, body) => {
            for s in body {
                collect_stmt_ids(s, ids);
            }
        }
        HirStmtKind::Expression(e) => collect_expr_ids_inner(e, ids),
        HirStmtKind::TrgBinding { name, .. } => ids.push(name.clone()),
        _ => {}
    }
}

fn topological_sort(graph: &DependencyGraph) -> Result<Vec<String>, DependencyError> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for var in &graph.all_vars {
        in_degree.entry(var.as_str()).or_insert(0);
    }
    for var in &graph.all_vars {
        in_degree.entry(var.as_str()).or_insert(0);
    }
    for (var, deps) in &graph.dependencies {
        in_degree.entry(var.as_str()).or_insert(0);
        *in_degree.get_mut(var.as_str()).expect("var") += deps.len();
    }
    let mut vars: Vec<&str> = graph.all_vars.iter().map(String::as_str).collect();
    vars.sort_unstable();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for var in vars {
        if *in_degree.get(var).unwrap_or(&0) == 0 {
            queue.push_back(var);
        }
    }
    let mut sorted: Vec<String> = Vec::new();
    while let Some(var) = queue.pop_front() {
        sorted.push(var.to_string());
        if let Some(deps) = graph.dependents.get(var) {
            let mut ordered: Vec<&String> = deps.iter().collect();
            ordered.sort();
            for dep in ordered {
                if let Some(deg) = in_degree.get_mut(dep.as_str()) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push_back(dep.as_str());
                    }
                }
            }
        }
    }
    if sorted.len() != graph.all_vars.len() {
        let mut unsorted: Vec<String> = graph
            .all_vars
            .difference(&sorted.iter().cloned().collect())
            .cloned()
            .collect();
        unsorted.sort();
        return Err(DependencyError { cycle: unsorted });
    }
    Ok(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::dependency_graph::DependencyGraph as AstGraph;

    fn graphs(src: &str) -> (Result<DependencyGraph, DependencyError>, Result<AstGraph, DependencyError>) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (build(&program), AstGraph::build(&items))
    }

    fn assert_parity(src: &str) {
        let (hir, ast) = graphs(src);
        match (hir, ast) {
            (Ok(h), Ok(a)) => assert_eq!(h, a, "graph drift for:\n{src}"),
            (Err(h), Err(a)) => assert_eq!(h.cycle, a.cycle, "cycle drift for:\n{src}"),
            (h, a) => panic!("status drift: hir={h:?} ast={a:?}"),
        }
    }

    #[test]
    fn empty_program_matches_ast() {
        assert_parity("");
    }

    #[test]
    fn triggers_and_bindings_match_ast() {
        assert_parity("trg alpha @ 0x1000;\ntrg zeta @ 0x1004;\n");
    }

    #[test]
    fn dependency_chain_matches_ast() {
        let src = "trg in_a @ 0x1000;\ntrg in_b @ 0x1004;\n\
                   let c: Int = 0;\nlet d: Int = 0;\n\
                   node main [c == 0][d == 1] { c = in_a + in_b; d = c; term; };\n";
        assert_parity(src);
    }

    #[test]
    fn cycle_matches_ast() {
        let src = "let a: Int = 0;\nlet b: Int = 0;\n\
                   node main [a == 0][b == 1] { a = b; b = a; term; };\n";
        assert_parity(src);
    }
}
