// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H1e explicit cast-path resolution. A `Cast` node records the actual
//! protocol lane sequence when both endpoints are known; unknown endpoints stay
//! unresolved for the type phase rather than being guessed.

use super::expr::HirExprKind;
use super::program::{HirContract, HirProgram};
use super::walk::{walk_body_mut, walk_expr_mut};
use crate::casting::graph::CastingGraph;
use crate::type_universe::TypeUniverse;

pub fn resolve_cast_paths(
    program: &mut HirProgram,
    graph: &CastingGraph,
    universe: &TypeUniverse,
) -> usize {
    let mut resolved = 0usize;
    let mut resolve = |expr: &mut super::expr::HirExpr| {
        resolved += resolve_one(expr, graph, universe);
    };
    for callable in &mut program.callables {
        if let Some(contract) = &mut callable.contract {
            resolve_contract(contract, &mut resolve);
        }
        walk_body_mut(&mut callable.body.statements, &mut resolve);
    }
    resolve_program_tails(program, &mut resolve);
    resolved
}

/// Contract expressions are emitted code; casts there resolve like body casts.
fn resolve_contract<F: FnMut(&mut super::expr::HirExpr)>(contract: &mut HirContract, resolve: &mut F) {
    walk_expr_mut(&mut contract.pre, resolve);
    walk_expr_mut(&mut contract.post, resolve);
    let Some(wd) = &mut contract.watchdog else {
        return;
    };
    walk_expr_mut(&mut wd.condition, resolve);
    if let Some(fb) = &mut wd.fallback {
        walk_expr_mut(fb, resolve);
    }
}

/// Top-level initializers, constants, and `init` bodies are not callable
/// bodies; they carry casts too (state seeding, global setup).
fn resolve_program_tails<F: FnMut(&mut super::expr::HirExpr)>(
    program: &mut HirProgram,
    resolve: &mut F,
) {
    for global in &mut program.globals {
        if let Some(expr) = &mut global.expr {
            walk_expr_mut(expr, resolve);
        }
    }
    for (_, expr) in &mut program.const_exprs {
        walk_expr_mut(expr, resolve);
    }
    for init in &mut program.inits {
        if let Some(expr) = &mut init.value {
            walk_expr_mut(expr, resolve);
        }
        walk_body_mut(&mut init.body, resolve);
    }
}

fn resolve_one(
    expr: &mut super::expr::HirExpr,
    graph: &CastingGraph,
    universe: &TypeUniverse,
) -> usize {
    let HirExprKind::Cast { inner, to, via } = &mut expr.kind else {
        return 0;
    };
    if via.is_some() {
        return 0;
    }
    let Some(source) = inner.ty.as_ref() else {
        return 0;
    };
    let (src_cat, src_var) = graph.type_to_protocol(universe, source);
    let (dst_cat, dst_var) = graph.type_to_protocol(universe, to);
    match graph.find_path(&src_cat, &src_var, &dst_cat, &dst_var) {
        Some(path) => {
            *via = Some(path);
            1
        }
        None => 0,
    }
}
