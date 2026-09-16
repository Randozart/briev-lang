// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 2026-08-26 (async Phase C, docs/plans/2026-08-26-async-phase-c-
//! segmented-lowering.md): task-body segmentation — the ONE splitter shared
//! by the reference interpreter and the LLVM backend.
//!
//! A spawned callable's body splits into segments at its cancellation
//! points. The reference semantic is that ONLY parameters carry across a
//! boundary (probed: a `let` before `yield;` is undefined after it), so a
//! segment lowers to a plain function of the task's parameters — no state
//! structs, no fibers.
//!
//! Split rules (identical for every consumer):
//!   1. `yield;` ends the current segment;
//!   2. a statement whose expressions contain a `<param>.<field>` port read
//!      STARTS a new segment when the current one is non-empty (Phase B:
//!      the blocking read must head its segment so the post-wake re-run
//!      never repeats side effects that preceded it).
//!
//! Undo: inline back into interpreter::register_pending_task; drop the
//! AnalysisResults field.

use crate::ast::{Expr, Statement, TopLevel, Type};


/// Split `body` into segments per the rules above.
pub fn split_task_body(body: &[Statement], param_names: &[String]) -> Vec<Vec<Statement>> {
    let mut segments: Vec<Vec<Statement>> = vec![Vec::new()];
    for stmt in body {
        let is_port_read = mentions_param_field(stmt, param_names);
        let current_empty = segments.last().map(|s| s.is_empty()).unwrap_or(false);
        if matches!(stmt, Statement::Yield) || (is_port_read && !current_empty) {
            segments.push(Vec::new());
        }
        if !matches!(stmt, Statement::Yield) {
            segments.last_mut().unwrap().push(stmt.clone());
        }
    }
    segments
}

/// Defn names referenced by any `spawn defn(...)` anywhere in the program —
/// the task targets. Obj/cell spawns resolve to shapes, not callables; they
/// are excluded by the caller's defn lookup, but the walker still reports
/// them so the caller decides.
pub fn collect_spawn_targets(items: &[TopLevel]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => d.body.iter().for_each(|s| stmt_walk(s, &mut out)),
            TopLevel::Transaction(t) => t.body.iter().for_each(|s| stmt_walk(s, &mut out)),
            TopLevel::Statement(s) => stmt_walk(s, &mut out),
            _ => {}
        }
    }
    out
}

/// 2026-08-26 (async Phase C): compiled tasks carry args/results as i64
/// argv slots (the defn entry convention). Reject spawn targets whose
/// signature leaves that ABI — a float or aggregate slot would miscompile
/// SILENTLY otherwise. Protocol categories come from the casting graph over
/// the TypeUniverse (rule 19); Ptr/Bits are compiler constructs handled
/// directly.
pub fn collect_task_abi_errors(
    items: &[TopLevel],
    universe: &crate::type_universe::TypeUniverse,
) -> Vec<String> {
    let graph = crate::casting::graph::CastingGraph::new();
    let targets = collect_spawn_targets(items);
    let mut errors = Vec::new();
    for item in items {
        if let TopLevel::Definition(d) = item {
            check_task_signature(d, &targets, &graph, universe, &mut errors);
        }
    }
    errors.sort();
    errors.dedup();
    errors
}

const TASK_ABI_HINT: &str =
    "compiled tasks carry arguments/results as i64 slots; \
     use Int/Bool/Char/Ptr (v1 scope)";

fn task_type_is_i64_abi(
    graph: &crate::casting::graph::CastingGraph,
    universe: &crate::type_universe::TypeUniverse,
    ty: &Type,
) -> bool {
    match ty {
        Type::Ptr(_) | Type::Bits(_) | Type::Void => true,
        // 2026-08-26 (async Phase D): an event wire IS an i64 — the slot-id
        // handle. Legal as a task parameter since compiled ports landed.
        Type::Applied(base, _) if base == "Event" => true,
        _ => {
            let (cat, _) = graph.type_to_protocol(universe, ty);
            matches!(cat.as_str(), "Int" | "Bit" | "Char" | "UInt")
        }
    }
}

fn result_types_of(d: &crate::ast::top::Definition) -> Vec<Type> {
    if !d.outputs.is_empty() {
        return d.outputs.clone();
    }
    d.output_type
        .as_ref()
        .map(|ot| ot.all_types())
        .unwrap_or_default()
}

fn check_task_signature(
    d: &crate::ast::top::Definition,
    targets: &std::collections::HashSet<String>,
    graph: &crate::casting::graph::CastingGraph,
    universe: &crate::type_universe::TypeUniverse,
    errors: &mut Vec<String>,
) {
    if !targets.contains(&d.name) {
        return;
    }
    for (pname, pty) in &d.parameters {
        if !task_type_is_i64_abi(graph, universe, pty) {
            errors.push(format!(
                "task '{}': parameter '{}' has type {} — {TASK_ABI_HINT}",
                d.name, pname, pty
            ));
        }
    }
    for rty in result_types_of(d) {
        if !task_type_is_i64_abi(graph, universe, &rty) {
            errors.push(format!(
                "task '{}': result type {} is not i64-ABI — await returns it \
                 through an i64 slot; {TASK_ABI_HINT}",
                d.name, rty
            ));
        }
    }
}

fn stmt_walk(s: &Statement, out: &mut std::collections::HashSet<String>) {
    stmt_exprs(s).iter().for_each(|e| expr_walk(e, out));
    // Flatten the body lists first so the walk stays a single loop level.
    let nested: Vec<&Statement> = stmt_bodies(s).into_iter().flatten().collect();
    nested.iter().for_each(|inner| stmt_walk(inner, out));
}

fn expr_walk(e: &Expr, out: &mut std::collections::HashSet<String>) {
    if let Expr::Spawn { type_name, args, .. } = e {
        out.insert(type_name.clone());
    }
    for child in expr_children(e) {
        expr_walk(child, out);
    }
}

fn stmt_exprs(s: &Statement) -> Vec<&Expr> {
    match s {
        Statement::Let { expr: Some(e), .. } => vec![e],
        Statement::Assign(lhs, rhs) => vec![lhs, rhs],
        Statement::Expression(e)
        | Statement::Term(Some(e))
        | Statement::Rollback(Some(e))
        | Statement::EndProgram(Some(e))
        | Statement::Check(e)
        | Statement::Gate(e) => vec![e],
        Statement::Guarded(cond, _) => vec![cond],
        Statement::Foreach { list, .. } => vec![list],
        Statement::ArrowAssign { target, value, .. } => {
            target.iter().map(|t| t.as_ref()).chain(std::iter::once(value.as_ref())).collect()
        }
        Statement::Match { expr, .. } => vec![expr],
        _ => Vec::new(),
    }
}

fn stmt_bodies(s: &Statement) -> Vec<&[Statement]> {
    match s {
        Statement::Guarded(_, b)
        | Statement::Block(b)
        | Statement::SyncBlock(b)
        | Statement::Mutex(b)
        | Statement::Defer(b) => vec![b.as_slice()],
        Statement::Barrier { body, .. } => vec![body.as_slice()],
        Statement::Foreach { body, .. } => vec![body.as_slice()],
        Statement::Match { arms, .. } => arms.iter().map(|a| a.body.as_slice()).collect(),
        _ => Vec::new(),
    }
}

fn expr_children(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::Field(recv, _)
        | Expr::Await(recv)
        | Expr::Cast(recv, _)
        | Expr::IsType(recv, _)
        | Expr::Consume(recv)
        | Expr::Deref(recv)
        | Expr::AddrOf(recv) => vec![recv],
        Expr::Index(l, r) => vec![l, r],
        Expr::Range { start, end, .. } => vec![start, end],
        Expr::BinaryOp(_, l, r) => vec![l, r],
        Expr::Call(_, args, _) | Expr::MethodCall(_, _, args, _, _) => args.iter().collect(),
        _ => Vec::new(),
    }
}

/// Does this statement's expressions contain a field projection rooted at
/// one of `params`? Root-identifier rule: a nested port path (`ch.out.amount`)
/// roots at the parameter. Canonical home — the interpreter's registration
/// path calls THIS function after Phase C extraction.
pub fn mentions_param_field(stmt: &Statement, params: &[String]) -> bool {
    if params.is_empty() { return false; }
    fn root_is_param(e: &Expr, params: &[String]) -> bool {
        let mut root = e;
        loop {
            match root {
                Expr::Identifier(name) => return params.iter().any(|p| p == name),
                Expr::Field(inner, _) | Expr::Index(inner, _) => root = inner,
                _ => return false,
            }
        }
    }
    fn expr_walk(e: &Expr, params: &[String]) -> bool {
        match e {
            Expr::Field(recv, _) => root_is_param(recv, params) || expr_walk(recv, params),
            _ => false,
        }
    }
    fn stmt_exprs(stmt: &Statement, params: &[String]) -> bool {
        match stmt {
            Statement::Let { expr: Some(e), .. }
            | Statement::Assign(_, e)
            | Statement::Expression(e)
            | Statement::Term(Some(e))
            | Statement::Rollback(Some(e))
            | Statement::EndProgram(Some(e))
            | Statement::Check(e)
            | Statement::Gate(e) => expr_walk(e, params),
            Statement::Guarded(cond, body) => {
                expr_walk(cond, params) || body.iter().any(|s| stmt_exprs(s, params))
            }
            Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Mutex(body)
            | Statement::Defer(body) => body.iter().any(|s| stmt_exprs(s, params)),
            Statement::Barrier { body, .. } => body.iter().any(|s| stmt_exprs(s, params)),
            Statement::Foreach { list, body, .. } => {
                expr_walk(list, params) || body.iter().any(|s| stmt_exprs(s, params))
            }
            Statement::ArrowAssign { target: Some(t), value, .. } => {
                expr_walk(t, params) || expr_walk(value, params)
            }
            Statement::ArrowAssign { value, .. } => expr_walk(value, params),
            Statement::Match { expr, arms } => {
                expr_walk(expr, params)
                    || arms.iter().any(|a| a.body.iter().any(|s| stmt_exprs(s, params)))
            }
            _ => false,
        }
    }
    stmt_exprs(stmt, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_body(src: &str) -> Vec<Statement> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        match &items[0] {
            TopLevel::Definition(d) => d.body.clone(),
            _ => panic!("defn expected"),
        }
    }

    #[test]
    fn yield_boundaries_and_param_reads_head_segments() {
        // fire (bare target) | read: the port read must HEAD segment 1 so
        // post-wake re-runs never repeat the fire.
        let body = parse_body(
            "defn job(d: Dmg, o: Dmg) -> Int { o <- Dmg{x:1}; term d.x; };",
        );
        let segs = split_task_body(&body, &["d".into(), "o".into()]);
        assert_eq!(segs.len(), 2);
        assert!(matches!(segs[0][0], Statement::ArrowAssign { .. }));
        assert!(matches!(segs[1][0], Statement::Term(Some(_))));
    }

    #[test]
    fn nested_port_path_roots_at_parameter() {
        let body = parse_body(
            "defn job(ch: Chan) -> Int { side(); term ch.wire.v; };",
        );
        let segs = split_task_body(&body, &["ch".into()]);
        assert_eq!(segs.len(), 2, "nested path still heads its segment");
    }

    #[test]
    fn interpreter_registration_parity() {
        // The interpreter's registration must produce IDENTICAL segments —
        // same splitter, pinned here so a future divergence is loud.
        let src = "defn job(n: Int, w: Evt) -> Int { let a: Int = n; w <- a; yield; term n; };";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        let (body, params) = match &items[0] {
            TopLevel::Definition(d) => (
                d.body.clone(),
                d.parameters.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            ),
            _ => panic!(),
        };
        let shared = split_task_body(&body, &params);

        // Real registration path: seed the table via load_program, register,
        // read back.
        let mut interp = crate::interpreter::Interpreter::new();
        interp.load_program(&items);
        crate::interpreter::register_pending_task(
            1, "job".to_string(), Vec::new(), body, &params,
        );
        let registered = crate::interpreter::take_task_segments(1)
            .expect("registered task id 1");
        // Statement's hand-written PartialEq ignores some fields (spans,
        // inline wrappers), so structural equality is asserted via Debug —
        // behavioral pin of "same splitter output", not literal identity.
        assert_eq!(
            format!("{shared:?}"),
            format!("{:?}", registered.0),
            "splitter divergence between consumers"
        );
    }
}
