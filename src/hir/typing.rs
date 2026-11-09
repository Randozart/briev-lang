// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H1 typing pass (A1, plan 2026-11-09-cast-via-typing-scoping.md).
//!
//! Best-effort HIR expression typing from facts the HIR already carries:
//! declared binding types, callable output lists, and intrinsic signatures.
//! Annotations are advisory — an unknown shape stays `None`, and codegen
//! continues to derive register types during emission. The first consumer is
//! explicit cast-path resolution (`Cast.via`), which needs the source type.

use super::expr::{HirExpr, HirExprKind};
use super::program::HirProgram;
use super::stmt::{HirStmt, HirStmtKind};
use crate::ast::{BinaryOpKind, Type, UnaryOpKind};
use crate::casting::graph::CastingGraph;
use crate::type_universe::TypeUniverse;
use std::collections::HashMap;

type Env = HashMap<String, Type>;
type Returns = HashMap<String, Vec<Type>>;

/// Annotate expression types, then resolve explicit cast paths. Returns the
/// number of cast paths resolved.
pub fn annotate_types(
    program: &mut HirProgram,
    universe: &TypeUniverse,
    graph: &CastingGraph,
) -> usize {
    let returns = collect_returns(program);
    let base = collect_binding_types(program);
    for callable in &mut program.callables {
        let mut env = seeded_env(&base, &callable.params);
        annotate_stmts(&mut callable.body.statements, &mut env, &returns);
    }
    super::cast_paths::resolve_cast_paths(program, graph, universe)
}

/// Callable name → declared output types (single-output empty list handled
/// by the caller as unknown).
fn collect_returns(program: &HirProgram) -> Returns {
    program
        .callables
        .iter()
        .map(|c| (c.name.clone(), declared_outputs(c)))
        .collect()
}

/// The callable's result types: named outputs when present, else a single
/// arrow return. Unions/tuples stay untyped (no single result type).
fn declared_outputs(c: &super::program::HirCallable) -> Vec<Type> {
    if !c.output_list.is_empty() {
        return c.output_list.clone();
    }
    c.result.iter().cloned().collect()
}

fn seeded_env(base: &Env, params: &[super::program::HirParam]) -> Env {
    let mut env = base.clone();
    for param in params {
        env.insert(param.name.clone(), param.ty.clone());
    }
    env
}

/// Program-scope bindings: globals, constants, and inits.
fn collect_binding_types(program: &HirProgram) -> Env {
    let mut env = Env::new();
    for g in &program.globals {
        if let Some(ty) = &g.ty {
            env.insert(g.name.clone(), ty.clone());
        }
    }
    for (name, ty) in &program.const_types {
        env.insert(name.clone(), ty.clone());
    }
    env
}

fn annotate_stmts(stmts: &mut [HirStmt], env: &mut Env, returns: &Returns) {
    for stmt in stmts {
        annotate_stmt(stmt, env, returns);
    }
}

/// Annotate a nested scope with a copy of the enclosing bindings (shadowing
/// stays local to the block).
fn annotate_scoped(stmts: &mut [HirStmt], env: &Env, returns: &Returns) {
    let mut scoped = env.clone();
    annotate_stmts(stmts, &mut scoped, returns);
}

fn annotate_stmt(stmt: &mut HirStmt, env: &mut Env, returns: &Returns) {
    match &mut stmt.kind {
        HirStmtKind::Let { name, ty, expr, .. } => {
            if let Some(expr) = expr {
                annotate_expr(expr, env, returns);
            }
            if let Some(ty) = ty {
                env.insert(name.clone(), ty.clone());
            }
        }
        HirStmtKind::Assign(lhs, rhs) | HirStmtKind::Open(lhs, rhs) => {
            annotate_expr(lhs, env, returns);
            annotate_expr(rhs, env, returns);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(target) = target {
                annotate_expr(target, env, returns);
            }
            annotate_expr(value, env, returns);
        }
        HirStmtKind::Term(e) | HirStmtKind::EndProgram(e) | HirStmtKind::Rollback(e) => {
            if let Some(e) = e {
                annotate_expr(e, env, returns);
            }
        }
        HirStmtKind::Tail(e)
        | HirStmtKind::Expression(e)
        | HirStmtKind::Gate(e)
        | HirStmtKind::Check(e) => annotate_expr(e, env, returns),
        HirStmtKind::Guarded(cond, body) => {
            annotate_expr(cond, env, returns);
            annotate_scoped(body, env, returns);
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => annotate_stmts(body, &mut env.clone(), returns),
        HirStmtKind::Foreach { list, body, .. } => {
            annotate_expr(list, env, returns);
            annotate_scoped(body, env, returns);
        }
        HirStmtKind::Match { expr, arms } => {
            annotate_expr(expr, env, returns);
            for arm in arms {
                annotate_scoped(&mut arm.body, env, returns);
            }
        }
        HirStmtKind::TrgBinding { instance, .. } => annotate_expr(instance, env, returns),
        _ => {}
    }
}

fn annotate_expr(expr: &mut HirExpr, env: &Env, returns: &Returns) {
    let ty = match &mut expr.kind {
        HirExprKind::Identifier(name) => env.get(name).cloned(),
        HirExprKind::Binary { op, left, right } => {
            annotate_expr(left, env, returns);
            annotate_expr(right, env, returns);
            binary_type(*op, left.ty.as_ref(), right.ty.as_ref())
        }
        HirExprKind::Unary { op, inner } => {
            annotate_expr(inner, env, returns);
            unary_type(*op, inner.ty.as_ref())
        }
        HirExprKind::Call { callee, args, .. } => {
            for arg in args {
                annotate_expr(arg, env, returns);
            }
            call_type(callee, returns)
        }
        HirExprKind::Cast { inner, to, .. } => {
            annotate_expr(inner, env, returns);
            Some(to.clone())
        }
        HirExprKind::Field { receiver, .. } => {
            annotate_expr(receiver, env, returns);
            None
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            annotate_expr(receiver, env, returns);
            for arg in args {
                annotate_expr(arg, env, returns);
            }
            None
        }
        HirExprKind::Reflect { receiver, .. } => {
            annotate_expr(receiver, env, returns);
            None
        }
        HirExprKind::Index(a, b) => {
            annotate_expr(a, env, returns);
            annotate_expr(b, env, returns);
            None
        }
        other => annotate_container(other, env, returns),
        HirExprKind::UnitLiteral { .. } => None,
        HirExprKind::Literal(_) | HirExprKind::Other(_) => expr.ty.clone(),
    };
    if let Some(ty) = ty {
        expr.ty = Some(ty);
    }
}


/// Container/structural expression kinds: annotate children, derive a type
/// only where it is unambiguous.
fn annotate_container(kind: &mut HirExprKind, env: &Env, returns: &Returns) -> Option<Type> {
    match kind {
        HirExprKind::Slice { array, start, end, stride } => {
            annotate_expr(array, env, returns);
            for opt in [start, end, stride].into_iter().flatten() {
                annotate_expr(opt, env, returns);
            }
            None
        }
        HirExprKind::Range { start, end, .. } => {
            annotate_expr(start, env, returns);
            annotate_expr(end, env, returns);
            None
        }
        HirExprKind::Spawn { args, .. } => {
            for arg in args {
                annotate_expr(arg, env, returns);
            }
            None
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            annotate_expr(condition, env, returns);
            annotate_expr(then_branch, env, returns);
            if let Some(e) = else_branch {
                annotate_expr(e, env, returns);
            }
            then_branch.ty.clone()
        }
        HirExprKind::Lambda { body, .. } => {
            annotate_expr(body, env, returns);
            None
        }
        HirExprKind::IsType { inner, .. } => {
            annotate_expr(inner, env, returns);
            Some(Type::bool_())
        }
        HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => {
            annotate_expr(inner, env, returns);
            None
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            for item in items {
                annotate_expr(item, env, returns);
            }
            None
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            if let Some(receiver) = receiver {
                annotate_expr(receiver, env, returns);
            }
            for arg in args {
                annotate_expr(arg, env, returns);
            }
            None
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, e) in fields.iter_mut().chain(specs.iter_mut()) {
                annotate_expr(e, env, returns);
            }
            None
        }
        HirExprKind::Match { scrutinee, arms } => {
            annotate_expr(scrutinee, env, returns);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    annotate_expr(guard, env, returns);
                }
                annotate_expr(&mut arm.body, env, returns);
            }
            None
        }
        HirExprKind::Block(stmts) => {
            annotate_scoped(stmts, env, returns);
            None
        }
        _ => None,
    }
}

fn binary_type(op: BinaryOpKind, left: Option<&Type>, right: Option<&Type>) -> Option<Type> {
    match op {
        BinaryOpKind::Eq
        | BinaryOpKind::Neq
        | BinaryOpKind::Lt
        | BinaryOpKind::Gt
        | BinaryOpKind::Le
        | BinaryOpKind::Ge
        | BinaryOpKind::And
        | BinaryOpKind::Or => Some(Type::bool_()),
        BinaryOpKind::Add
        | BinaryOpKind::Sub
        | BinaryOpKind::Mul
        | BinaryOpKind::Div
        | BinaryOpKind::Mod => arithmetic_type(left, right),
        _ => None,
    }
}

/// Numeric result: Float if either side is Float, Int only when both are Int.
fn arithmetic_type(left: Option<&Type>, right: Option<&Type>) -> Option<Type> {
    if left == Some(&Type::float()) || right == Some(&Type::float()) {
        return Some(Type::float());
    }
    if left == Some(&Type::int()) && right == Some(&Type::int()) {
        return Some(Type::int());
    }
    None
}

fn unary_type(op: UnaryOpKind, inner: Option<&Type>) -> Option<Type> {
    match op {
        UnaryOpKind::Not => Some(Type::bool_()),
        UnaryOpKind::Neg | UnaryOpKind::BitNot => match inner {
            Some(t) if *t == Type::int() || *t == Type::float() => Some(t.clone()),
            _ => None,
        },
    }
}

fn call_type(callee: &str, returns: &Returns) -> Option<Type> {
    if let Some(outputs) = returns.get(callee) {
        if let [single] = outputs.as_slice() {
            return Some(single.clone());
        }
    }
    match crate::intrinsic_signatures::get_intrinsic_signature(callee) {
        Some(sig) => match sig.return_kind {
            crate::intrinsic_signatures::ReturnKind::Native(name) if name != "Void" => {
                Some(Type::Custom(name.to_string()))
            }
            crate::intrinsic_signatures::ReturnKind::Exact(ty) => Some(ty),
            _ => None,
        },
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::expr::HirExprKind;
    use crate::hir::lower_program;
    use crate::hir::program::HirCallable;

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    fn annotated(src: &str) -> (HirProgram, usize) {
        let mut program = lower_program(&parse(src));
        let universe = TypeUniverse::new();
        let graph = CastingGraph::new();
        let resolved = annotate_types(&mut program, &universe, &graph);
        (program, resolved)
    }

    fn body_of<'a>(program: &'a HirProgram, name: &str) -> &'a HirCallable {
        program
            .callables
            .iter()
            .find(|c| c.name == name)
            .expect("callable")
    }

    #[test]
    fn identifiers_get_declared_types_and_cast_paths_resolve() {
        let (program, resolved) = annotated(
            "defn f(x: Int) -> Float { let y: Int = x; term y as Float; };",
        );
        assert_eq!(resolved, 1, "identifier-source cast must resolve");
        let body = body_of(&program, "f");
        let HirStmtKind::Term(Some(cast)) = &body.body.statements[1].kind else {
            panic!("term cast expected");
        };
        let HirExprKind::Cast { inner, via, .. } = &cast.kind else {
            panic!("cast expected");
        };
        assert_eq!(inner.ty, Some(Type::int()));
        assert!(via.as_ref().is_some_and(|p| !p.is_empty()));
    }

    #[test]
    fn binary_and_call_types_are_derived() {
        let (program, _) = annotated(
            "defn g(a: Int, b: Int) -> Int { term a + b; };\n\
             defn h(a: Int, b: Int) -> Int { term g(a, b) + 1; };",
        );
        let g = body_of(&program, "g");
        let HirStmtKind::Term(Some(add)) = &g.body.statements[0].kind else {
            panic!("term expected");
        };
        assert_eq!(add.ty, Some(Type::int()));
        let h = body_of(&program, "h");
        let HirStmtKind::Term(Some(add)) = &h.body.statements[0].kind else {
            panic!("term expected");
        };
        let HirExprKind::Binary { left, .. } = &add.kind else {
            panic!("binary expected");
        };
        assert_eq!(left.ty, Some(Type::int()), "call result type from output_list");
    }

    #[test]
    fn comparisons_are_bool() {
        let (program, _) = annotated("defn f(a: Int, b: Int) -> Bool { term a < b; };");
        let f = body_of(&program, "f");
        let HirStmtKind::Term(Some(cmp)) = &f.body.statements[0].kind else {
            panic!("term expected");
        };
        assert_eq!(cmp.ty, Some(Type::bool_()));
    }

    #[test]
    fn unknown_shapes_stay_untyped() {
        let (program, _) = annotated("defn f(a: Int) -> Int { term a.^Length; };");
        let f = body_of(&program, "f");
        let HirStmtKind::Term(Some(reflect)) = &f.body.statements[0].kind else {
            panic!("term expected");
        };
        assert_eq!(reflect.ty, None, "reflect typing lands with A2");
    }
}
