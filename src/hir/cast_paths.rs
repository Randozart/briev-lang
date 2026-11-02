// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H1e explicit cast-path resolution. A `Cast` node records the actual
//! protocol lane sequence when both endpoints are known; unknown endpoints stay
//! unresolved for the type phase rather than being guessed.

use super::expr::HirExprKind;
use super::program::HirProgram;
use super::walk::walk_body_mut;
use crate::casting::graph::CastingGraph;
use crate::type_universe::TypeUniverse;

pub fn resolve_cast_paths(
    program: &mut HirProgram,
    graph: &CastingGraph,
    universe: &TypeUniverse,
) -> usize {
    let mut resolved = 0usize;
    for callable in &mut program.callables {
        let mut resolve = |expr: &mut super::expr::HirExpr| {
            let HirExprKind::Cast { inner, to, via } = &mut expr.kind else {
                return;
            };
            if via.is_some() {
                return;
            }
            let Some(source) = inner.ty.as_ref() else {
                return;
            };
            let (src_cat, src_var) = graph.type_to_protocol(universe, source);
            let (dst_cat, dst_var) = graph.type_to_protocol(universe, to);
            if let Some(path) = graph.find_path(&src_cat, &src_var, &dst_cat, &dst_var) {
                *via = Some(path);
                resolved += 1;
            }
        };
        walk_body_mut(&mut callable.body.statements, &mut resolve);
    }
    resolved
}
