// ── .bad symbolic unused-value lint (2026-10-20) ──────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-10-20 (bad assembly+ plan): `--warn-unused-values`. A symbolic
// value that is defined (`let x = …` / the first `x = …`) but never read
// is almost always a typo (`vga` vs `vgaa`) — the allocator happily
// assigns a register and the code silently reads the wrong value. The
// lint runs over the symbolic program BEFORE the engine lowerer resolves
// names to `vN`s, while the author's names still exist.
//
// Non-fatal and opt-in: this is an advisory prediction, like the W-tier
// notices, not a contract. A name starting with `_` is intentionally
// unused and is skipped. Pins (`let p = pin r5`) and defn parameters are
// declarations, not inferred values, so they are not flagged either.
//
// To undo: delete this file, the `warn_unused` plumbing in
// src/backend/bad/{mod,lower,symbolic}.rs, and the CLI flag.

use crate::ast::bld::{BldStmt, BldTopLevel};
use crate::ast::Expr;
use std::collections::HashSet;

/// One defined-but-never-read symbolic value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedValue {
    pub name: String,
    pub line: usize,
}

/// Scan every recipe body for symbolic values that are defined but never
/// read. Returns one entry per (first) definition of an unused name.
pub fn unused_values(items: &[BldTopLevel]) -> Vec<UnusedValue> {
    let mut out = Vec::new();
    for item in items {
        match item {
            BldTopLevel::Defn(d) => collect_unit(&d.body, &mut out),
            BldTopLevel::Bootstrap(b) => collect_unit(&b.body, &mut out),
            _ => {}
        }
    }
    out
}

fn collect_unit(body: &[BldStmt], out: &mut Vec<UnusedValue>) {
    let mut defs: Vec<(String, usize)> = Vec::new();
    let mut uses: HashSet<String> = HashSet::new();
    walk_body(body, &mut defs, &mut uses);
    let mut reported: HashSet<String> = HashSet::new();
    for (name, line) in defs {
        if name.starts_with('_') {
            continue;
        }
        if !uses.contains(&name) && reported.insert(name.clone()) {
            out.push(UnusedValue { name, line });
        }
    }
}

fn walk_body(body: &[BldStmt], defs: &mut Vec<(String, usize)>, uses: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            BldStmt::Let { name, value, span, .. } => {
                defs.push((name.clone(), span.line));
                collect_uses(value, uses);
            }
            BldStmt::Assign { target, value, .. } => {
                // A bare identifier target is a pure write; a field/index
                // target reads its base and index.
                if !matches!(target, Expr::Identifier(_)) {
                    collect_uses(target, uses);
                }
                collect_uses(value, uses);
            }
            BldStmt::Call { args, .. } => {
                for a in args {
                    collect_uses(a, uses);
                }
            }
            BldStmt::Block { body, .. } => walk_body(body, defs, uses),
            BldStmt::When { cond, then, otherwise, .. } => {
                collect_uses(cond, uses);
                walk_body(then, defs, uses);
                if let Some(o) = otherwise {
                    walk_body(o, defs, uses);
                }
            }
            BldStmt::Loop { body, .. } => walk_body(body, defs, uses),
            BldStmt::While { cond, body, .. } => {
                collect_uses(cond, uses);
                walk_body(body, defs, uses);
            }
            BldStmt::Return { value: Some(v), .. } => collect_uses(v, uses),
            BldStmt::WriteReg { value, .. } => collect_uses(value, uses),
            BldStmt::Instr(instr) => {
                for op in &instr.operands {
                    match op {
                        crate::ast::bad::BadOperand::Name(n) => {
                            uses.insert(n.clone());
                        }
                        crate::ast::bad::BadOperand::Expr(t) => {
                            if let Ok(e) = crate::parser::bld::parse_bld_expr(t) {
                                collect_uses(&e, uses);
                            }
                        }
                        crate::ast::bad::BadOperand::Int(_)
                        | crate::ast::bad::BadOperand::Float(_) => {}
                    }
                }
            }
            BldStmt::Break { .. }
            | BldStmt::Continue { .. }
            | BldStmt::Return { value: None, .. }
            | BldStmt::Bad { .. }
            | BldStmt::Bind { .. }
            | BldStmt::RawClobber { .. } => {}
        }
    }
}

fn collect_uses(expr: &Expr, uses: &mut HashSet<String>) {
    match expr {
        Expr::Identifier(n) => {
            uses.insert(n.clone());
        }
        Expr::Call(_, args, _) => {
            for a in args {
                collect_uses(a, uses);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_uses(l, uses);
            collect_uses(r, uses);
        }
        Expr::UnaryOp(_, v) => collect_uses(v, uses),
        Expr::Field(base, _) => collect_uses(base, uses),
        Expr::MethodCall(base, _, args, _, _) => {
            collect_uses(base, uses);
            for a in args {
                collect_uses(a, uses);
            }
        }
        Expr::Index(base, idx) => {
            collect_uses(base, uses);
            collect_uses(idx, uses);
        }
        _ => {}
    }
}
