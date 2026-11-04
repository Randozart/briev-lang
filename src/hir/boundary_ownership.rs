// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H3.3b — boundary ownership inference over typed HIR.
//!
//! Mirror of `analysis::boundary_ownership` (AST, still authoritative until
//! parity): seed each export/frgn from its protocol variant + direction +
//! convention, then propagate export returns transitively through the call
//! graph. Reuses the AST module's `BoundaryOwnership`/`seed_from_protocol`/
//! meeting helpers so the two cannot drift; only the walk is HIR-based.

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirOutput, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::boundary_ownership::{
    fold_ownership, meet_option, seed_from_protocol, BoundaryEntry, BoundaryOwnership,
    BoundaryOwnershipResult, Direction,
};
use crate::ast::Type;
use crate::type_universe::TypeUniverse;
use std::collections::{HashMap, HashSet};

/// Compute boundary ownership for every export and frgn in the program.
pub fn compute_boundary_ownership(
    program: &HirProgram,
    universe: Option<&TypeUniverse>,
    convention_of: &dyn Fn(&str) -> Option<String>,
) -> BoundaryOwnershipResult {
    let exports: HashMap<String, &HirCallable> = program
        .callables
        .iter()
        .filter(|c| c.exported && matches!(c.kind, HirCallableKind::Defn))
        .map(|c| (c.name.clone(), c))
        .collect();
    let state_fields: HashSet<String> = program
        .globals
        .iter()
        .map(|g| g.name.clone())
        .chain(program.constants.iter().cloned())
        .collect();
    let declared_protocols: HashMap<String, String> = program
        .type_defs
        .iter()
        .filter_map(|t| t.protocol.clone().map(|p| (t.name.clone(), p)))
        .collect();

    let mut result = BoundaryOwnershipResult::default();
    for f in &program.foreign {
        let convention = convention_of(&f.name).unwrap_or_else(|| "c_abi".to_string());
        let params = f
            .params
            .iter()
            .map(|(_, t)| {
                seed_from_protocol(t, &convention, Direction::Param, universe, &declared_protocols)
            })
            .collect();
        let ret = f.ret.as_ref().map(|t| {
            seed_from_protocol(t, &convention, Direction::Return, universe, &declared_protocols)
        });
        result.frgns.insert(f.name.clone(), BoundaryEntry { params, ret });
    }

    let mut ctx = FlowCtx {
        exports: &exports,
        frgn_owns: &result.frgns,
        state_fields: &state_fields,
        universe,
        declared_protocols: &declared_protocols,
        memo: HashMap::new(),
        visiting: Vec::new(),
    };
    for name in exports.keys() {
        ctx.defn_ownership(name);
    }
    result.exports = ctx.memo;
    result
}

struct FlowCtx<'a> {
    exports: &'a HashMap<String, &'a HirCallable>,
    frgn_owns: &'a HashMap<String, BoundaryEntry>,
    state_fields: &'a HashSet<String>,
    universe: Option<&'a TypeUniverse>,
    declared_protocols: &'a HashMap<String, String>,
    memo: HashMap<String, BoundaryEntry>,
    visiting: Vec<String>,
}

impl<'a> FlowCtx<'a> {
    fn defn_ownership(&mut self, name: &str) -> BoundaryEntry {
        if let Some(v) = self.memo.get(name) {
            return v.clone();
        }
        if self.visiting.iter().any(|n| n == name) {
            return BoundaryEntry {
                params: vec![BoundaryOwnership::Borrowed],
                ret: Some(BoundaryOwnership::Borrowed),
            };
        }
        let Some(d) = self.exports.get(name).copied() else {
            self.memo.insert(name.to_string(), BoundaryEntry::default());
            return BoundaryEntry::default();
        };

        let params: Vec<BoundaryOwnership> = d
            .params
            .iter()
            .map(|p| {
                seed_from_protocol(
                    &p.ty,
                    "c_abi",
                    Direction::Param,
                    self.universe,
                    self.declared_protocols,
                )
            })
            .collect();

        self.visiting.push(name.to_string());
        let decl_ret = output_ownership(d.outputs.as_ref(), self.universe, self.declared_protocols);
        let flow_ret = self.body_ownership(&d.body.statements);
        self.visiting.pop();

        let ret = match (decl_ret, flow_ret) {
            (Some(d), Some(f)) => Some(d.meet_with_boundary(f)),
            (Some(d), None) => Some(d),
            (None, f) => f,
        };
        let entry = BoundaryEntry { params, ret };
        self.memo.insert(name.to_string(), entry.clone());
        entry
    }

    fn body_ownership(&mut self, body: &[HirStmt]) -> Option<BoundaryOwnership> {
        fold_ownership(body.iter().filter_map(|s| self.stmt_ownership(s)))
    }

    fn stmt_ownership(&mut self, stmt: &HirStmt) -> Option<BoundaryOwnership> {
        match &stmt.kind {
            HirStmtKind::Term(opt) | HirStmtKind::EndProgram(opt) | HirStmtKind::Rollback(opt) => {
                opt.as_ref().and_then(|e| self.expr_ownership(e))
            }
            HirStmtKind::Tail(e) | HirStmtKind::Expression(e) => self.expr_ownership(e),
            HirStmtKind::Let { expr, .. } => expr.as_ref().and_then(|e| self.expr_ownership(e)),
            HirStmtKind::Assign(_, e) => self.expr_ownership(e),
            HirStmtKind::Guarded(_, body)
            | HirStmtKind::Foreach { body, .. }
            | HirStmtKind::Block(body) => {
                fold_ownership(body.iter().filter_map(|s| self.stmt_ownership(s)))
            }
            HirStmtKind::MetadataAssignment(..) => None,
            _ => None,
        }
    }

    fn call_ownership(&mut self, name: &str, args: &[HirExpr]) -> Option<BoundaryOwnership> {
        let from_call = if let Some(e) = self.frgn_owns.get(name) {
            e.ret
        } else if self.exports.contains_key(name) {
            Some(
                self.defn_ownership(name)
                    .ret
                    .unwrap_or(BoundaryOwnership::Owned),
            )
        } else {
            None
        };
        let args_own = fold_ownership(args.iter().filter_map(|a| self.expr_ownership(a)));
        meet_option(from_call, args_own)
    }

    fn expr_ownership(&mut self, expr: &HirExpr) -> Option<BoundaryOwnership> {
        match &expr.kind {
            HirExprKind::Identifier(name) => {
                if self.state_fields.contains(name) {
                    Some(BoundaryOwnership::Owned)
                } else {
                    None
                }
            }
            HirExprKind::Call { callee, args, .. } => self.call_ownership(callee, args),
            HirExprKind::Binary { left, right, .. } => {
                let l = self.expr_ownership(left);
                let r = self.expr_ownership(right);
                meet_option(l, r)
            }
            HirExprKind::Unary { inner, .. } => self.expr_ownership(inner),
            HirExprKind::List(items) => {
                fold_ownership(items.iter().filter_map(|e| self.expr_ownership(e)))
            }
            HirExprKind::Cast { inner, .. } => self.expr_ownership(inner),
            HirExprKind::MethodCall { receiver, args, .. } => {
                let r = self.expr_ownership(receiver);
                let args_own =
                    fold_ownership(args.iter().filter_map(|a| self.expr_ownership(a)));
                meet_option(r, args_own)
            }
            HirExprKind::Reflect { receiver, .. } => self.expr_ownership(receiver),
            HirExprKind::Index(arr, idx) => {
                let a = self.expr_ownership(arr);
                let i = self.expr_ownership(idx);
                meet_option(a, i)
            }
            HirExprKind::Slice { array, start, end, .. } => {
                let mut acc = self.expr_ownership(array);
                for e in [start.as_ref(), end.as_ref()] {
                    if let Some(e) = e {
                        acc = meet_option(acc, self.expr_ownership(e));
                    }
                }
                acc
            }
            HirExprKind::AddrOf(inner) => self.expr_ownership(inner),
            _ => None,
        }
    }
}

fn output_ownership(
    output: Option<&HirOutput>,
    universe: Option<&TypeUniverse>,
    declared_protocols: &HashMap<String, String>,
) -> Option<BoundaryOwnership> {
    let o = output?;
    match o {
        HirOutput::Single(t) => Some(seed_from_protocol(
            t,
            "c_abi",
            Direction::Return,
            universe,
            declared_protocols,
        )),
        HirOutput::Union(v) | HirOutput::Tuple(v) => v
            .iter()
            .filter_map(|x| output_ownership(Some(x), universe, declared_protocols))
            .reduce(BoundaryOwnership::meet),
        HirOutput::Array(inner) | HirOutput::Named(_, inner) => {
            output_ownership(Some(inner), universe, declared_protocols)
        }
    }
}
