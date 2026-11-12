// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H4.2 statement seam: per-statement HIR-driven body construction.
//!
//! The choke point (`emit_definition`/`emit_transaction`) walks the HIR body
//! statement-by-statement: a statement whose HIR can be reconstructed
//! losslessly is emitted from HIR (counted `native`); a statement containing
//! an unsupported node (`Other`) is spliced from the AST body (counted
//! `fallback`) — never guessed. Native HIR statement arms land here
//! kind-by-kind; expressions cross `hir::to_ast::expr` until the expression
//! families (slice D) replace them.

use super::{emit_stmt::emit_statement, LlvmBackend, TypedRegister};
use std::fmt::Write;
use crate::ast::Statement;
use crate::hir::expr::{HirExpr, HirExprKind};
use crate::ast::top::StmtMatchArm;
use crate::backend::llvm::AllocStrategy;
use crate::hir::stmt::{HirStmt, HirStmtKind};

/// Reconstruct one HIR statement for emission. `None` = unsupported node;
/// the caller splices the AST statement at the same index.
pub(crate) fn convert_hir_statement(stmt: &HirStmt) -> Option<Statement> {
    match &stmt.kind {
        HirStmtKind::Other(_) => None,
        _ => crate::hir::to_ast::stmt(stmt),
    }
}

/// Build a HIR-sourced body: per-statement conversion with AST splice and
/// coverage accounting. Returns `None` when the HIR body length does not
/// match the AST body (lowering drift) — callers fall back to the AST body.
/// 2026-11-12 (owned-storage seam): comparison view that strips the
/// compiler-inserted storage releases and temp-transfers back to the source
/// shape. Emission keeps the full body; only the H4.2 drift assert looks
/// through this view. Top-level only — nested inserts fall back to AST and
/// are lost (leak, never unsound).
pub(crate) fn storage_release_view(body: &[Statement]) -> Vec<Statement> {
    strip_view(body)
}

fn strip_view(body: &[Statement]) -> Vec<Statement> {
    let mut stash: Vec<(String, crate::ast::Expr)> = Vec::new();
    let mut out = Vec::with_capacity(body.len());
    for stmt in body {
        match stmt {
            Statement::Expression(crate::ast::Expr::Call(callee, args, _))
                if (callee == "Free#" || callee == "coll_destroy")
                    && args.len() == 1
                    && matches!(&args[0], crate::ast::Expr::Identifier(_)) =>
            {
                continue;
            }
            Statement::Let { name, expr: Some(e), .. } if name.starts_with("__ot") => {
                stash.push((name.clone(), e.clone()));
                continue;
            }
            Statement::Term(Some(crate::ast::Expr::Identifier(n)))
            | Statement::Tail(crate::ast::Expr::Identifier(n))
            | Statement::EndProgram(Some(crate::ast::Expr::Identifier(n))) => {
                if let Some((_, e)) = stash.iter().rev().find(|(sn, _)| sn == n) {
                    let e = e.clone();
                    out.push(match stmt {
                        Statement::Tail(_) => Statement::Tail(e),
                        Statement::EndProgram(_) => Statement::EndProgram(Some(e)),
                        _ => Statement::Term(Some(e)),
                    });
                    continue;
                }
                out.push(stmt.clone());
            }
            Statement::Guarded(c, b) => {
                out.push(Statement::Guarded(c.clone(), strip_view(b)));
            }
            Statement::Block(b) => out.push(Statement::Block(strip_view(b))),
            Statement::SyncBlock(b) => out.push(Statement::SyncBlock(strip_view(b))),
            Statement::Mutex(b) => out.push(Statement::Mutex(strip_view(b))),
            Statement::Defer(b) => out.push(Statement::Defer(strip_view(b))),
            Statement::Foreach { item, list, body, modifiers } => {
                out.push(Statement::Foreach {
                    item: item.clone(),
                    list: list.clone(),
                    body: strip_view(body),
                    modifiers: modifiers.clone(),
                });
            }
            Statement::Match { expr, arms } => {
                out.push(Statement::Match {
                    expr: expr.clone(),
                    arms: arms
                        .iter()
                        .map(|a| StmtMatchArm { patterns: a.patterns.clone(), body: strip_view(&a.body) })
                        .collect(),
                });
            }
            _ => out.push(stmt.clone()),
        }
    }
    out
}

fn coll_destroy_available(backend: &LlvmBackend) -> bool {
    // stdlib-extension gate: coll_destroy lives in the stdlib prelude. Bodies
    // compiled without stdlib (bare-compiler corpus tests) have no definition
    // to call — List releases are skipped (conservative leak, the pre-slice
    // fallback behavior) rather than emitting an undefined symbol.
    backend.ctx.defn_params.contains_key("coll_destroy")
}

fn release_ast_stmt(storage: crate::hir::stmt::StorageKind, name: &str) -> Option<Statement> {
    let callee = match storage {
        crate::hir::stmt::StorageKind::Str => "Free#",
        crate::hir::stmt::StorageKind::List => "coll_destroy",
    };
    let hir = HirStmt {
        id: crate::hir::ids::NodeId(0),
        span: None,
        kind: HirStmtKind::Expression(crate::hir::expr::HirExpr {
            id: crate::hir::ids::NodeId(0),
            span: None,
            ty: None,
            kind: HirExprKind::Call {
                callee: callee.to_string(),
                args: vec![crate::hir::expr::HirExpr {
                    id: crate::hir::ids::NodeId(0),
                    span: None,
                    ty: None,
                    kind: HirExprKind::Identifier(name.to_string()),
                }],
                analysis_id: None,
            },
        }),
    };
    crate::hir::to_ast::stmt(&hir)
}

/// Native emission for a compiler-inserted storage release. Str reuses the
/// strategy-aware destroy (arena no-ops honored); List calls the stdlib
/// coll_destroy handle-destroy.
fn emit_release_owned_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    storage: crate::hir::stmt::StorageKind,
    name: &str,
    indent: &str,
) -> Option<TypedRegister> {
    match storage {
        crate::hir::stmt::StorageKind::Str => emit_release_owned_str(backend, out, name, indent),
        crate::hir::stmt::StorageKind::List => {
            // stdlib-extension gate: coll_destroy lives in the stdlib prelude.
            // Bodies compiled without stdlib (bare-compiler corpus tests) have
            // no definition to call — skip the release (conservative leak,
            // previous fallback behavior) rather than emit an undefined sym.
            if !backend.ctx.defn_params.contains_key("coll_destroy") {
                return Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() });
            }
            // Handle width follows the target's pointer-int width; the arg
            // goes through the normal expression/adapter path so ptr-typed
            // bindings materialize correctly (same lesson as Free#).
            let ity = backend.ptr_int_ty();
            let reg = backend.emit_expr(
                out,
                &crate::ast::Expr::Identifier(name.to_string()),
                indent,
            );
            let h = super::intrinsics::adapt_to_ptr_width(backend, out, &reg, indent);
            let v = backend.fun.gen_reg();
            writeln!(out, "{indent}{v} = call {ity} @coll_destroy({ity} {h})").ok();
            Some(TypedRegister { name: v, ty: crate::ast::Type::void() })
        }
    }
}

/// String lane of ReleaseOwned. String bindings materialize as ACTUAL
/// pointers on both targets, but their stamped Briev type is Custom while
/// some producers stamp Ptr — the shared adapter cannot tell them apart.
/// This lane owns the decision: materialize via emit_expr, honor the arena
/// strategy (no-op), then ptrtoint the pointer directly.
fn emit_release_owned_str(
    backend: &mut LlvmBackend,
    out: &mut String,
    name: &str,
    indent: &str,
) -> Option<TypedRegister> {
    let reg = backend.emit_expr(out, &crate::ast::Expr::Identifier(name.to_string()), indent);
    if matches!(
        backend.fun.alloc_strategies.get(&reg.name),
        Some(AllocStrategy::Arena) | Some(AllocStrategy::Alloca)
    ) {
        return Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() });
    }
    let ity = backend.ptr_int_ty();
    let h = super::intrinsics::adapt_handle_to_ptr_int(backend, out, &reg, indent);
    let p = backend.fun.gen_reg();
    writeln!(out, "{indent}{p} = inttoptr {ity} {h} to ptr").ok();
    writeln!(out, "{indent}call void @free(ptr {p})").ok();
    Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
}

/// Native emission for a temp-transfer: bind, release, then the original
/// return form (Term/Tail/EndProgram) on the temp binding.
fn emit_temp_transfer_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    indent: &str,
) -> Option<TypedRegister> {
    let HirStmtKind::TempTransfer { name, ty, expr, releases, ret } = &stmt.kind else {
        return None;
    };
    let binding = HirStmt {
        id: crate::hir::ids::NodeId(0),
        span: None,
        kind: HirStmtKind::Let {
            name: name.clone(),
            names: vec![name.clone()],
            ty: ty.clone(),
            expr: Some((**expr).clone()),
            modifiers: vec![],
        },
    };
    let bound = emit_value_statement(backend, out, &binding, indent);
    for (storage, n) in releases {
        emit_release_owned_statement(backend, out, *storage, n, indent);
    }
    let ret_expr = HirExpr {
        id: crate::hir::ids::NodeId(1),
        span: None,
        ty: ty.clone(),
        kind: HirExprKind::Identifier(name.clone()),
    };
    let ret_stmt = HirStmt {
        id: crate::hir::ids::NodeId(2),
        span: None,
        kind: match ret {
            crate::hir::stmt::RetKind::Term => HirStmtKind::Term(Some(ret_expr)),
            crate::hir::stmt::RetKind::Tail => HirStmtKind::Tail(ret_expr),
            crate::hir::stmt::RetKind::EndProgram => HirStmtKind::EndProgram(Some(ret_expr)),
        },
    };
    let _ = bound;
    emit_hir_statement(backend, out, &ret_stmt, indent)
}

/// 2026-11-12 (owned-storage seam, nested): scoped-statement conversion that
/// preserves inserted storage releases inside guard/block/loop/match bodies.
/// Without this, any scope containing an insert failed conversion wholesale
/// and the whole scope fell back to source AST — the release was silently
/// lost (leak). Recursion makes arbitrary nesting seam-clean.
fn convert_children_seam(
    backend: &LlvmBackend,
    body: &[HirStmt],
) -> Option<Vec<Statement>> {
    let mut out = Vec::with_capacity(body.len() + 2);
    for child in body {
        match &child.kind {
            HirStmtKind::ReleaseOwned { storage, name } => {
                if *storage == crate::hir::stmt::StorageKind::List
                    && !coll_destroy_available(backend)
                {
                    continue;
                }
                out.push(release_ast_stmt(*storage, name)?);
            }
            HirStmtKind::TempTransfer { name, ty, expr, ret, releases } => {
                let releases: Vec<(crate::hir::stmt::StorageKind, String)> = releases
                    .iter()
                    .filter(|(st, _)| {
                        *st == crate::hir::stmt::StorageKind::Str
                            || coll_destroy_available(backend)
                    })
                    .cloned()
                    .collect();
                let binding = HirStmt {
                    id: crate::hir::ids::NodeId(0),
                    span: None,
                    kind: HirStmtKind::Let {
                        name: name.clone(),
                        names: vec![name.clone()],
                        ty: ty.clone(),
                        expr: Some((**expr).clone()),
                        modifiers: vec![],
                    },
                };
                out.push(crate::hir::to_ast::stmt(&binding)?);
                let ret_expr = crate::hir::to_ast::expr(expr)?;
                out.push(match ret {
                    crate::hir::stmt::RetKind::Term => Statement::Term(Some(ret_expr)),
                    crate::hir::stmt::RetKind::Tail => Statement::Tail(ret_expr),
                    crate::hir::stmt::RetKind::EndProgram => Statement::EndProgram(Some(ret_expr)),
                });
            }
            _ => out.push(convert_scoped_seam(backend, child)?),
        }
    }
    Some(out)
}

fn convert_scoped_seam(backend: &LlvmBackend, stmt: &HirStmt) -> Option<Statement> {
    match &stmt.kind {
        HirStmtKind::Guarded(c, b) => Some(Statement::Guarded(
            crate::hir::to_ast::expr(c)?,
            convert_children_seam(backend, b)?,
        )),
        HirStmtKind::Block(b) => Some(Statement::Block(convert_children_seam(backend, b)?)),
        HirStmtKind::SyncBlock(b) => Some(Statement::SyncBlock(convert_children_seam(backend, b)?)),
        HirStmtKind::Mutex(b) => Some(Statement::Mutex(convert_children_seam(backend, b)?)),
        HirStmtKind::Defer(b) => Some(Statement::Defer(convert_children_seam(backend, b)?)),
        HirStmtKind::Foreach { item, list, body, modifiers } => {
            Some(Statement::Foreach {
                item: item.clone(),
                list: Box::new(crate::hir::to_ast::expr(list)?),
                body: convert_children_seam(backend, body)?,
                modifiers: modifiers
                    .iter()
                    .map(|name| crate::ast::Annotation { name: name.clone(), value: None })
                    .collect(),
            })
        }
        HirStmtKind::Match { expr, arms } => Some(Statement::Match {
            expr: Box::new(crate::hir::to_ast::expr(expr)?),
            arms: arms
                .iter()
                .map(|arm| {
                    Some(StmtMatchArm {
                        patterns: arm.patterns.iter().map(crate::hir::to_ast::pattern).collect::<Option<Vec<_>>>()?,
                        body: convert_children_seam(backend, &arm.body)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        }),
        _ => crate::hir::to_ast::stmt(stmt),
    }
}

pub(crate) fn hir_sourced_body(
    backend: &mut LlvmBackend,
    hir_body: &[HirStmt],
    ast_body: &[Statement],
    name: &str,
) -> Option<Vec<Statement>> {
    // Two-cursor walk: compiler-inserted storage statements consume no AST
    // statement (they have no source syntax); everything else zips 1:1.
    let mut out = Vec::with_capacity(hir_body.len() + 2);
    let mut ai = 0usize;
    for hir_stmt in hir_body {
        backend.hir_stmts_seen += 1;
        match &hir_stmt.kind {
            HirStmtKind::ReleaseOwned { storage, name } => {
                if *storage == crate::hir::stmt::StorageKind::List
                    && !coll_destroy_available(backend)
                {
                    continue;
                }
                let s = release_ast_stmt(*storage, name)?;
                backend.hir_stmts_native += 1;
                out.push(s);
            }
            HirStmtKind::TempTransfer { name, ty, expr, ret, .. } => {
                let binding = HirStmt {
                    id: hir_stmt.id,
                    span: None,
                    kind: HirStmtKind::Let {
                        name: name.clone(),
                        names: vec![name.clone()],
                        ty: ty.clone(),
                        expr: Some((**expr).clone()),
                        modifiers: vec![],
                    },
                };
                let let_ast = crate::hir::to_ast::stmt(&binding)?;
                let term_expr = crate::hir::to_ast::expr(expr)?;
                backend.hir_stmts_native += 2;
                out.push(let_ast);
                out.push(match ret {
                    crate::hir::stmt::RetKind::Term => Statement::Term(Some(term_expr)),
                    crate::hir::stmt::RetKind::Tail => Statement::Tail(term_expr),
                    crate::hir::stmt::RetKind::EndProgram => Statement::EndProgram(Some(term_expr)),
                });
            }
            _ => {
                let Some(ast_stmt) = ast_body.get(ai) else { return None };
                ai += 1;
                match convert_scoped_seam(backend, hir_stmt) {
                    Some(s) => {
                        backend.hir_stmts_native += 1;
                        out.push(s);
                    }
                    None => {
                        backend.hir_stmts_fallback += 1;
                        out.push(ast_stmt.clone());
                    }
                }
            }
        }
    }
    if ai != ast_body.len() {
        return None;
    }
    let _ = name;
    Some(out)
}

/// Entry point for future native statement arms (slice C/D). Today every
/// supported kind converts and delegates; native arms replace the conversion
/// for their kind only, keeping the AST emitter for everything else.
/// H4 slice D dispatch group: Expression statements (native family fast path).
fn emit_expr_statement(backend: &mut LlvmBackend, out: &mut String, stmt: &HirStmt, indent: &str) -> Option<TypedRegister> {
    if let HirStmtKind::Expression(e) = &stmt.kind {
        // Family-covered expressions AND calls: the Call arm guards its own
        // argument reconstructibility before emitting anything, so a None
        // here falls through to conversion with no partial IR.
        let call_native = matches!(
            &e.kind,
            HirExprKind::Call { .. } | HirExprKind::Index { .. }
        );
        if call_native || super::emit_expr::hir_expr_family_covered(e) {
            if let Some(reg) = backend.emit_hir_expr(out, e, indent) {
                backend.fun.last_expr_reg = Some(reg.name.clone());
                backend.fun.last_expr_ty = Some(reg.ty.clone());
                return Some(reg);
            }
        }
    }
    None
}

/// H4 slice D dispatch group: scoped control flow (Guarded, Block).
/// H4 slice D dispatch group: scoped control flow (Guarded, Block).
fn emit_scoped_statement(backend: &mut LlvmBackend, out: &mut String, stmt: &HirStmt, indent: &str) -> Option<TypedRegister> {
    match &stmt.kind {
        HirStmtKind::Guarded(cond, body) => {
            emit_guarded_statement(backend, out, cond, body, indent)
        }
        HirStmtKind::Block(children) => emit_block_children(backend, out, children, indent),
        _ => None,
    }
}

/// Guarded (when) emission: condition through operand recursion, children
/// through the seam; bookkeeping verbatim from the AST arm.
fn emit_guarded_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    cond: &crate::hir::expr::HirExpr,
    body: &[HirStmt],
    indent: &str,
) -> Option<TypedRegister> {
    let Some(cond_reg) = backend.emit_hir_operand(out, cond, indent) else {
        return None;
    };
    let saved_lvt = backend.fun.last_val_temps.clone();
    let saved_lvt_types = backend.fun.last_val_types.clone();
    let label_n = backend.fun.txn_counter;
    backend.fun.txn_counter += 1;
    let then_lbl = format!("guard.then{}", label_n);
    let end_lbl = format!("guard.end{}", label_n);
    let cond_i1 = super::emit_stmt::guard_cond_i1(backend, out, indent, &cond_reg);
    writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, cond_i1, then_lbl, end_lbl).ok();
    writeln!(out, "{}{}:", indent, then_lbl).ok();
    backend.fun.terminated = false;
    for child in body {
        if emit_hir_statement(backend, out, child, indent).is_none() {
            let Some(ast) = convert_hir_statement(child) else {
                return None;
            };
            emit_statement(backend, out, &ast, indent);
        }
    }
    backend.fun.last_val_temps = saved_lvt;
    backend.fun.last_val_types = saved_lvt_types;
    if !backend.fun.terminated {
        writeln!(out, "{}br label %{}", indent, end_lbl).ok();
    }
    writeln!(out, "{}{}:", indent, end_lbl).ok();
    backend.fun.terminated = false;
    backend.fun.tail_loop_backedge = false;
    backend.fun.cur_block = Some(end_lbl.clone());
    Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
}

/// Block/serial-child emission: children through the seam, last register wins.
fn emit_block_children(
    backend: &mut LlvmBackend,
    out: &mut String,
    children: &[HirStmt],
    indent: &str,
) -> Option<TypedRegister> {
    let mut last = TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() };
    for child in children {
        if let Some(reg) = emit_hir_statement(backend, out, child, indent) {
            last = reg;
        } else {
            let Some(ast) = convert_hir_statement(child) else {
                return None;
            };
            last = emit_statement(backend, out, &ast, indent);
        }
    }
    Some(last)
}

/// H4 slice D dispatch group: value-binding statements (ArrowAssign, Let, Assign).
fn emit_value_statement(backend: &mut LlvmBackend, out: &mut String, stmt: &HirStmt, indent: &str) -> Option<TypedRegister> {
    // 2026-11-05 (H4 slice D): native ArrowAssign — the dispatch reads the
    // reconstructed target/value shapes (wire-fire, streams, insert/extract
    // strategies, provenance); covered value leaves emit natively through
    // the dual-fueled value path. Reconstructibility checked BEFORE any
    // emission (wire-fire and stream paths emit immediately).
    if let HirStmtKind::ArrowAssign { target, value, consume } = &stmt.kind {
        // 2026-11-09 (H5 seam refactor): dispatch by name/type; the AST is only
        // a lossless fallback for an uncovered value leaf. Check that fallback
        // exists before any emission (wire/stream paths emit immediately).
        if crate::hir::to_ast::expr(value).is_none() {
            return None;
        }
        let ctx = super::emit_stmt::ArrowAssignCtx {
            target: target.as_ref().map(super::emit_stmt::ArrowOperand::Hir),
            value: super::emit_stmt::ArrowOperand::Hir(value),
            consume: *consume,
        };
        return Some(super::emit_stmt::emit_arrow_assign_statement(
            backend, out, indent, &ctx,
        ));
    }
    // 2026-11-05 (H4 slice D): native Let — the binding dispatch reads the
    // reconstructed shapes (spawn handles, bare-let slots, collection
    // scaffolds); covered initializer leaves emit natively through the
    // dual-fueled value path. Reconstructibility checked BEFORE any
    // emission (task-handle registration and slot binding come first).
    if let HirStmtKind::Let { name, names, ty, expr, modifiers, .. } = &stmt.kind {
        let Some(ast_expr) = expr.as_ref().and_then(crate::hir::to_ast::expr) else {
            if expr.is_some() {
                return None;
            }
            return Some(super::emit_stmt::emit_let_statement(
                backend,
                out,
                indent,
                name,
                names,
                ty.as_ref(),
                modifiers,
                &super::emit_stmt::LetValue::Ast(None),
            ));
        };
        return Some(super::emit_stmt::emit_let_statement(
            backend,
            out,
            indent,
            name,
            names,
            ty.as_ref(),
            modifiers,
            &super::emit_stmt::LetValue::Hir(Some(expr.as_ref().unwrap()), &ast_expr),
        ));
    }
    // 2026-11-05 (H4 slice D): native Assign — the dispatch reads the
    // reconstructed LHS/RHS shapes (provenance/collection assignment), so
    // both cross the boundary; covered RHS leaves emit natively through the
    // dual-fueled value path. Reconstructibility is checked BEFORE any
    // emission (the AddrOf-pop path early-returns emit no IR, but the
    // collection-construction path starts emitting immediately).
    if let HirStmtKind::Assign(lhs, rhs) = &stmt.kind {
        let Some(ast_lhs) = crate::hir::to_ast::expr(lhs) else {
            return None;
        };
        let Some(ast_rhs) = crate::hir::to_ast::expr(rhs) else {
            return None;
        };
        return Some(super::emit_stmt::emit_assign_statement(
            backend,
            out,
            indent,
            &ast_lhs,
            &super::emit_stmt::AssignValue::Hir(rhs, &ast_rhs),
        ));
    }
    None
}

/// H4 slice D dispatch group: Foreach (shared loop-engine helper).
fn emit_foreach_statement_hir(backend: &mut LlvmBackend, out: &mut String, stmt: &HirStmt, indent: &str) -> Option<TypedRegister> {
    // 2026-11-05 (H4 slice D): native Foreach — the iteration-source dispatch
    // reads the reconstructed list's shape (identical), the loop engine is
    // the shared extracted helper, and body children route through the seam
    // (native or per-child conversion). The list converts BEFORE any
    // emission (carrier allocas come first in the helper), so None falls
    // back cleanly.
    if let HirStmtKind::Foreach { item, list, body, .. } = &stmt.kind {
        let Some(ast_list) = crate::hir::to_ast::expr(list) else {
            return None;
        };
        super::emit_stmt::emit_foreach_statement(
            backend,
            out,
            indent,
            item,
            &ast_list,
            super::emit_stmt::ForeachBody::Hir(body),
        );
        return Some(TypedRegister {
            name: backend.fun.gen_reg(),
            ty: crate::ast::Type::void(),
        });
    }
    None
}

/// H4 slice D dispatch group: Term + Tail (the P-gated transaction exit).
fn emit_term_statement(backend: &mut LlvmBackend, out: &mut String, stmt: &HirStmt, indent: &str) -> Option<TypedRegister> {
    match &stmt.kind {
        HirStmtKind::Term(val) => emit_term_value(backend, out, val, indent),
        HirStmtKind::Tail(e) => {
            let term = HirStmt {
                id: stmt.id,
                span: stmt.span,
                kind: HirStmtKind::Term(Some(e.clone())),
            };
            emit_hir_statement(backend, out, &term, indent)
        }
        _ => None,
    }
}

/// Term value emission + the P-gated exit (body verbatim from the arm).
fn emit_term_value(
    backend: &mut LlvmBackend,
    out: &mut String,
    val: &Option<crate::hir::expr::HirExpr>,
    indent: &str,
) -> Option<TypedRegister> {
        if let Some(v) = val {
            if crate::hir::to_ast::expr(v).is_none() {
                return None;
            }
        }
        if val.is_some() && backend.fun.inline_member_exit.is_none() {
            super::emit_stmt::emit_web_flush_batch(backend, out, indent);
            backend.flush_defer_cleanup(out, indent);
        }
        if let Some(v) = val {
            let saved = super::emit_stmt::begin_term_return_context(backend);
            let ast_v = crate::hir::to_ast::expr(v).expect("guarded above");
            let returned_collection = if matches!(v.kind, HirExprKind::List(_)) {
                backend.fun.item_return_type.clone().and_then(|ty| {
                    backend
                        .construct_local_collection(out, indent, &ty, &ast_v)
                        .map(|(value, _)| value)
                })
            } else {
                None
            };
            let reg = returned_collection.unwrap_or_else(|| {
                match backend.emit_hir_operand(out, v, indent) {
                    Some(r) => r,
                    // Unreachable under the reconstructibility guard; kept
                    // total so no path can double-emit.
                    None => backend.emit_expr(out, &ast_v, indent),
                }
            });
            super::emit_stmt::emit_term_value_tail(backend, out, indent, reg, saved);
        } else {
            super::emit_stmt::emit_bare_term_gate(backend, out, indent);
        }
        return Some(TypedRegister {
            name: backend.fun.gen_reg(),
            ty: crate::ast::Type::void(),
        });
    
}

/// H4 slice D dispatch group: statement Match (phi chain).
fn emit_match_statement(backend: &mut LlvmBackend, out: &mut String, stmt: &HirStmt, indent: &str) -> Option<TypedRegister> {
    let HirStmtKind::Match { expr: scrutinee, arms } = &stmt.kind else {
        return None;
    };
    // Reconstructibility gate BEFORE any emission (scrutinee included).
    crate::hir::to_ast::expr(scrutinee)?;
    let ast_arms = reconstruct_match_arms(arms)?;
    let scrut = backend.emit_hir_operand(out, scrutinee, indent)?;
    let counter = backend.fun.txn_counter;
    backend.fun.txn_counter += 1;
    let merge = format!(".smt_end_{}", counter);
    let ctx = MatchCtx { scrut: &scrut, merge: &merge, counter, indent };
    emit_match_conditions(backend, out, arms, &ast_arms, &ctx);
    emit_match_bodies(backend, out, arms, &ast_arms, &ctx);
        // 2026-11-05 (#69 mirror): the merge stays open — the no-match edge
        // and continuing arms always reach it; only explicit
        // predecessor/exhaustiveness tracking may close it.
        writeln!(out, "{}:", merge).ok();
        backend.fun.terminated = false;
        backend.fun.cur_block = Some(merge);
        return Some(TypedRegister {
            name: backend.fun.gen_reg(),
            ty: crate::ast::Type::void(),
        });
}

/// Reconstruct patterns and arm bodies (all-or-nothing, before any emission).
fn reconstruct_match_arms(
    arms: &[crate::hir::stmt::HirStmtMatchArm],
) -> Option<Vec<(Vec<crate::ast::Pattern>, Vec<Statement>)>> {
    let mut ast_arms = Vec::with_capacity(arms.len());
    for arm in arms {
        ast_arms.push(reconstruct_match_arm(arm)?);
    }
    Some(ast_arms)
}

/// One arm's patterns + reconstructed body (all-or-nothing).
fn reconstruct_match_arm(
    arm: &crate::hir::stmt::HirStmtMatchArm,
) -> Option<(Vec<crate::ast::Pattern>, Vec<Statement>)> {
    let mut pats = Vec::with_capacity(arm.patterns.len());
    for p in &arm.patterns {
        pats.push(crate::hir::to_ast::pattern(p)?);
    }
    let body = crate::hir::to_ast::stmt_body(&arm.body)?;
    Some((pats, body))
}

/// One arm's emission view (HIR body + reconstructed AST pair).
struct MatchArmView<'a> {
    hir: &'a crate::hir::stmt::HirStmtMatchArm,
    patterns: &'a [crate::ast::Pattern],
    ast_children: &'a [Statement],
}

/// Shared label context for the match phases.
struct MatchCtx<'a> {
    scrut: &'a TypedRegister,
    merge: &'a str,
    counter: usize,
    indent: &'a str,
}

/// Phase 1: per-arm pattern-condition chain and body/next labels.
fn emit_match_conditions(
    backend: &mut LlvmBackend,
    out: &mut String,
    arms: &[crate::hir::stmt::HirStmtMatchArm],
    ast_arms: &[(Vec<crate::ast::Pattern>, Vec<Statement>)],
    ctx: &MatchCtx<'_>,
) {
    for (i, _arm) in arms.iter().enumerate() {
        emit_match_condition_arm(backend, out, &ast_arms[i].0, ctx, i);
    }
    writeln!(out, "  br label %{}", ctx.merge).ok();
}

/// One arm's condition chain (pattern OR-fold, wildcard/binding short-circuit).
fn emit_match_condition_arm(
    backend: &mut LlvmBackend,
    out: &mut String,
    patterns: &[crate::ast::Pattern],
    ctx: &MatchCtx<'_>,
    i: usize,
) {
    let next = format!(".smt_next_{}_{}", ctx.counter, i);
    let mut cond: Option<String> = None;
    for pat in patterns {
        if matches!(
            pat,
            crate::ast::Pattern::Wildcard | crate::ast::Pattern::Binding(_)
        ) {
            cond = Some(format!("{}", backend.fun.gen_reg()));
            writeln!(out, "  {} = icmp eq i64 0, 0", cond.as_deref().unwrap_or("")).ok();
            break;
        }
        let c = backend.emit_pattern_condition(pat, &ctx.scrut.name, &ctx.scrut.ty, out, ctx.indent);
        cond = Some(match cond {
            None => c,
            Some(prev) => {
                let r = backend.fun.gen_reg();
                writeln!(out, "  {} = or i1 {}, {}", r, prev, c).ok();
                format!("{}", r)
            }
        });
    }
    let body_label = format!(".smt_body_{}_{}", ctx.counter, i);
    let cond = cond.unwrap_or_else(|| {
        let r = backend.fun.gen_reg();
        writeln!(out, "  {} = icmp eq i64 0, 0", r).ok();
        r
    });
    writeln!(out, "  br i1 {}, label %{}, label %{}", cond, body_label, next).ok();
    writeln!(out, "{}:", next).ok();
    backend.fun.cur_block = Some(next);
}

/// Phase 2: per-arm bodies (children through the seam) and merge edges.
fn emit_match_bodies(
    backend: &mut LlvmBackend,
    out: &mut String,
    arms: &[crate::hir::stmt::HirStmtMatchArm],
    ast_arms: &[(Vec<crate::ast::Pattern>, Vec<Statement>)],
    ctx: &MatchCtx<'_>,
) {
    for (i, arm) in arms.iter().enumerate() {
        let view = MatchArmView {
            hir: arm,
            patterns: &ast_arms[i].0,
            ast_children: &ast_arms[i].1,
        };
        emit_match_body_arm(backend, out, &view, ctx, i);
    }
}

/// One arm's body: bind patterns, children through the seam, merge edge.
fn emit_match_body_arm(
    backend: &mut LlvmBackend,
    out: &mut String,
    arm: &MatchArmView<'_>,
    ctx: &MatchCtx<'_>,
    i: usize,
) {
    let patterns = arm.patterns;
    let ast_children = arm.ast_children;
    let body_label = format!(".smt_body_{}_{}", ctx.counter, i);
    writeln!(out, "{}:", body_label).ok();
    backend.fun.cur_block = Some(body_label);
    backend.fun.terminated = false;
    for p in patterns {
        backend.bind_pattern(p, &ctx.scrut.name, &ctx.scrut.ty, out, ctx.indent);
    }
    for (j, child) in arm.hir.body.iter().enumerate() {
        if backend.fun.terminated {
            break;
        }
        if emit_hir_statement(backend, out, child, ctx.indent).is_none() {
            emit_statement(backend, out, &ast_children[j], ctx.indent);
        }
    }
    if !backend.fun.terminated {
        writeln!(out, "  br label %{}", ctx.merge).ok();
    }
}

/// Terminal helper: Halt.
fn emit_halt_statement(backend: &mut LlvmBackend, out: &mut String, indent: &str) -> Option<TypedRegister> {
            let t = &backend.ctx.target_triple;
            let wait_family = ["arm", "thumb", "aarch64", "cortex", "riscv"]
                .iter()
                .any(|fam| t.contains(fam));
            if wait_family {
                let uid = backend.fun.txn_counter;
                backend.fun.txn_counter += 1;
                writeln!(out, "{}br label %halt_wait_{}", indent, uid).ok();
                writeln!(out, "halt_wait_{}:", uid).ok();
                writeln!(out, "  call void asm sideeffect \"wfi\", \"\"()").ok();
                writeln!(out, "  br label %halt_wait_{}", uid).ok();
            } else {
                writeln!(out, "{}call void @llvm.trap()", indent).ok();
            }
            writeln!(out, "{}unreachable", indent).ok();
            backend.fun.terminated = true;
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        
}

/// Terminal helper: Gate.
fn emit_gate_statement(backend: &mut LlvmBackend, out: &mut String, cond: &crate::hir::expr::HirExpr, indent: &str) -> Option<TypedRegister> {
            // 2026-07-26: convergence gate — mirror of the AST arm.
            let Some(cond_reg) = backend.emit_hir_operand(out, cond, indent) else {
                return None;
            };
            let label_n = backend.fun.txn_counter;
            backend.fun.txn_counter += 1;
            let pass_lbl = format!("gate.pass{}", label_n);
            let has_convergence = backend.fun.convergence_target.is_some();
            let fail_target = if has_convergence {
                backend.fun.convergence_target.as_ref().unwrap().clone()
            } else {
                format!("gate.fail{}", label_n)
            };
            let cond_i1 = if cond_reg.ty == crate::ast::Type::bool_() {
                let b = backend.fun.gen_reg();
                writeln!(out, "{}{} = trunc i8 {} to i1", indent, b, cond_reg.name).ok();
                b
            } else {
                let i1_name = format!("%gi1_{}", label_n);
                writeln!(out, "{}{} = icmp ne i64 {}, 0", indent, i1_name, cond_reg.name).ok();
                i1_name
            };
            writeln!(out, "{0}br i1 {1}, label %{2}, label %{3}",
                indent, cond_i1, pass_lbl, fail_target).ok();
            if !has_convergence {
                writeln!(out, "{}{}:", indent, fail_target).ok();
                writeln!(out, "{}  unreachable", indent).ok();
            }
            writeln!(out, "{}{}:", indent, pass_lbl).ok();
            backend.fun.terminated = false;
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        
}

/// Terminal helper: Rollback.
/// Zero literal for a return type (rollback's fabricated return).
fn ret_zero_literal(ret_ty: &str) -> &'static str {
    match ret_ty {
        "float" | "double" => "0.0",
        "ptr" => "null",
        _ => "0",
    }
}

/// Terminal helper: Rollback (deferred cleanup + ret-ty return).
/// Defer registration: queue the HIR cleanup body; flush runs it LIFO at every
/// exit (term/rollback/fallthrough). No code at the registration point.
fn emit_defer_statement(
    backend: &mut LlvmBackend,
    children: &[crate::hir::stmt::HirStmt],
) -> Option<TypedRegister> {
    backend.fun.defer_bodies.push(children.to_vec());
    Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
}

fn emit_rollback_statement(backend: &mut LlvmBackend, out: &mut String, indent: &str) -> Option<TypedRegister> {
    // 2026-08-09 (Phase 10): deferred cleanup runs on rollback too.
    backend.flush_defer_cleanup(out, indent);
    if backend.fun.fn_ret_ty == "void" {
        writeln!(out, "{}ret void", indent).ok();
    } else {
        let zero = ret_zero_literal(&backend.fun.fn_ret_ty);
        writeln!(out, "{}ret {} {}", indent, backend.fun.fn_ret_ty, zero).ok();
    }
    backend.fun.terminated = true;
    Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
}

/// Terminal helper: FreeHint.
fn emit_free_hint_statement(backend: &mut LlvmBackend, out: &mut String, name: &String, indent: &str) -> Option<TypedRegister> {
            // 2026-08-26 (async Phase C): a freed TASK HANDLE cancels its
            // scheduler entry. Ordinary frees keep the Phase 5 path.
            if backend.fun.task_handle_names.contains(name) {
                if let Some(h) = backend.fun.let_bindings.get(name).cloned() {
                    let st = if backend
                        .ctx
                        .defn_takes_state("briev_task_cancel_impl")
                    {
                        "ptr %state, "
                    } else {
                        ""
                    };
                    writeln!(out, "{indent}call i64 @briev_task_cancel_impl({st}i64 {h})").ok();
                    let _ = backend.fun.gen_reg();
                    return Some(TypedRegister {
                        name: backend.fun.gen_reg(),
                        ty: crate::ast::Type::void(),
                    });
                }
            }
            super::emit_stmt::emit_destroy_consumed(
                backend,
                out,
                indent,
                &crate::ast::Expr::Identifier(name.clone()),
            );
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        
}

/// Terminal helper: SyncBlock/Mutex.
fn emit_serial_statement(backend: &mut LlvmBackend, out: &mut String, children: &Vec<HirStmt>, indent: &str) -> Option<TypedRegister> {
            // 2026-09-08 / 2026-08-09: serial sections emit inline (a modifier
            // must never be a speedup); recursion through the seam.
            let mut last = TypedRegister {
                name: backend.fun.gen_reg(),
                ty: crate::ast::Type::void(),
            };
            for child in children {
                if let Some(reg) = emit_hir_statement(backend, out, child, indent) {
                    last = reg;
                } else {
                    let Some(ast) = convert_hir_statement(child) else {
                        return None;
                    };
                    last = emit_statement(backend, out, &ast, indent);
                }
            }
            Some(last)
        
}

/// Terminal helper: EndProgram.
fn emit_end_program_statement(backend: &mut LlvmBackend, out: &mut String, val: &Option<crate::hir::expr::HirExpr>, indent: &str) -> Option<TypedRegister> {
            // 2026-08-06 (endprogram plan): genuine process exit; the value's
            // side effects emit first, then the runtime exit provider.
            if backend.ctx.webstack_enabled {
                super::emit_stmt::emit_web_flush_batch(backend, out, indent);
            }
            let code = if let Some(v) = val {
                let Some(reg) = backend.emit_hir_operand(out, v, indent) else {
                    return None;
                };
                backend.adapt_to_i64(out, indent, &reg)
            } else {
                "0".to_string()
            };
            writeln!(out, "{}; briev.program_end", indent).ok();
            if backend.ctx.defn_params.contains_key("exit_prog") {
                let st = if backend.ctx.defn_takes_state("exit_prog") {
                    "ptr %state, "
                } else {
                    ""
                };
                writeln!(out, "{}call i64 @exit_prog({}i64 {})", indent, st, code).ok();
            } else {
                writeln!(out, "{}call void @__exit(i64 {})", indent, code).ok();
            }
            writeln!(out, "{}unreachable", indent).ok();
            backend.fun.terminated = true;
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        
}

/// H4 slice D dispatch group: small terminal/directive statements.
fn emit_terminal_statement(backend: &mut LlvmBackend, out: &mut String, stmt: &HirStmt, indent: &str) -> Option<TypedRegister> {
    match &stmt.kind {
        HirStmtKind::Break => match backend.fun.foreach_break_labels.last() {
            Some(label) => {
                writeln!(out, "{}br label %{}", indent, label).ok();
                backend.fun.terminated = true;
                Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
            }
            None => None,
        },
        HirStmtKind::Trap => {
            writeln!(out, "{}call void @llvm.trap()", indent).ok();
            writeln!(out, "{}unreachable", indent).ok();
            backend.fun.terminated = true;
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        }
        HirStmtKind::Halt => emit_halt_statement(backend, out, indent),
        HirStmtKind::Yield => Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() }),
        HirStmtKind::Check(cond) => match backend.emit_hir_operand(out, cond, indent) {
            Some(cond_reg) => {
                let cond_i1 = super::emit_stmt::guard_cond_i1(backend, out, indent, &cond_reg);
                let noop = backend.fun.gen_reg();
                writeln!(out, "{}{} = add i64 0, 0", indent, noop).ok();
                Some(TypedRegister { name: noop, ty: crate::ast::Type::void() })
            }
            None => None,
        },
        HirStmtKind::Gate(cond) => emit_gate_statement(backend, out, cond, indent),
        HirStmtKind::Rollback(_) => emit_rollback_statement(backend, out, indent),
        HirStmtKind::KeepHint(_) => Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() }),
        HirStmtKind::FreeHint(name) => emit_free_hint_statement(backend, out, name, indent),
        HirStmtKind::SyncBlock(children) | HirStmtKind::Mutex(children) => emit_serial_statement(backend, out, children, indent),
        HirStmtKind::Defer(children) => emit_defer_statement(backend, children),
        HirStmtKind::EndProgram(val) => emit_end_program_statement(backend, out, val, indent),
        _ => None,
    }
}

/// H4 slice D: statement seam entry. Native group dispatch (kinds are
/// disjoint, so a `None` from the owning group means "fall back to the
/// lossless AST conversion" exactly as the original grouped `if let` chain
/// did). Per-kind bodies live in the group functions above (Praetor split;
/// behavior verbatim).
pub(crate) fn emit_hir_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    indent: &str,
) -> Option<TypedRegister> {
    let native = match &stmt.kind {
        HirStmtKind::ReleaseOwned { storage, name } => {
            emit_release_owned_statement(backend, out, *storage, name, indent)
        }
        HirStmtKind::TempTransfer { .. } => emit_temp_transfer_statement(backend, out, stmt, indent),
        HirStmtKind::Expression(_) => emit_expr_statement(backend, out, stmt, indent),
        HirStmtKind::Guarded(..) | HirStmtKind::Block(_) => {
            emit_scoped_statement(backend, out, stmt, indent)
        }
        HirStmtKind::ArrowAssign { .. } | HirStmtKind::Let { .. } | HirStmtKind::Assign(..) => {
            emit_value_statement(backend, out, stmt, indent)
        }
        HirStmtKind::Foreach { .. } => emit_foreach_statement_hir(backend, out, stmt, indent),
        HirStmtKind::Term(_) | HirStmtKind::Tail(_) => {
            emit_term_statement(backend, out, stmt, indent)
        }
        HirStmtKind::Match { .. } => emit_match_statement(backend, out, stmt, indent),
        HirStmtKind::Break
        | HirStmtKind::Trap
        | HirStmtKind::Halt
        | HirStmtKind::Yield
        | HirStmtKind::Check(_)
        | HirStmtKind::Gate(_)
        | HirStmtKind::Rollback(_)
        | HirStmtKind::KeepHint(_)
        | HirStmtKind::FreeHint(_)
        | HirStmtKind::SyncBlock(_)
        | HirStmtKind::Mutex(_)
        | HirStmtKind::EndProgram(_) => emit_terminal_statement(backend, out, stmt, indent),
        _ => None,
    };
    if native.is_some() {
        return native;
    }
    match convert_hir_statement(stmt) {
        Some(ast) => Some(emit_statement(backend, out, &ast, indent)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral};
    use crate::hir::ids::NodeId;
    use crate::hir::program::{HirCallableKind, HirProgram};

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    #[test]
    fn nested_storage_inserts_survive_the_body_seam() {
        let src = r#"
defn mk() -> String { term "a" + "b"; };
defn use(s: String) -> Int {
    when s .^Length > 0 {
        let y: String = mk();
        let n: Int = CharCount#(y);
        term n;
    };
    term 0;
};
"#;
        let items = parse(src);
        let mut program = crate::hir::lower_program(&items);
        crate::hir::owned_storage::insert_releases(&mut program, &std::collections::HashMap::new());
        let use_fn = program
            .callables
            .iter()
            .find(|c| c.name == "use")
            .expect("use fn");
        // The pass releases x inside the guard body (early-return path).
        let guard = use_fn
            .body
            .statements
            .iter()
            .find_map(|st| match &st.kind {
                HirStmtKind::Guarded(_, b) => Some(b),
                _ => None,
            })
            .expect("guard");
        assert!(
            guard.iter().any(|st| matches!(&st.kind, HirStmtKind::TempTransfer { .. } | HirStmtKind::ReleaseOwned { .. })),
            "release expected inside the guard body"
        );

        let ast_body: Vec<Statement> = items
            .iter()
            .find_map(|i| match i {
                crate::ast::TopLevel::Definition(d) if d.name == "use" => Some(d.body.clone()),
                _ => None,
            })
            .expect("ast defn");

        let mut backend = LlvmBackend::new();
        let out = hir_sourced_body(&mut backend, &use_fn.body.statements, &ast_body, "use")
            .expect("nested inserts must not force a whole-body fallback");
        // Comparison view strips the inserts back to the source shape.
        assert_eq!(storage_release_view(&out), ast_body);
        // Emission view keeps them: a Free#/coll_destroy call exists inside
        // the emitted guard.
        let emitted_guard = out.iter().find_map(|st| match st {
            Statement::Guarded(_, b) => Some(b),
            _ => None,
        })
        .expect("emitted guard");
        let has_release = emitted_guard.iter().any(|st| matches!(st, Statement::Expression(crate::ast::Expr::Call(c, _, _)) if c == "Free#" || c == "coll_destroy"));
        assert!(has_release, "release must survive emission inside the guard");
    }

    #[test]
    fn per_statement_accounting_native_and_fallback() {
        let src = "node n [true][true] { let a: Int = 1; a = a + 1; term; };";
        let items = parse(src);
        let program = crate::hir::lower_program(&items);
        let hir_body = program
            .callables
            .iter()
            .find(|c| matches!(c.kind, HirCallableKind::Transaction))
            .expect("txn")
            .body
            .statements
            .clone();
        let ast_body: Vec<Statement> = items
            .iter()
            .find_map(|i| match i {
                crate::ast::TopLevel::Transaction(t) => Some(t.body.clone()),
                _ => None,
            })
            .expect("ast txn");

        let mut backend = LlvmBackend::new();
        let out = hir_sourced_body(&mut backend, &hir_body, &ast_body, "n")
            .expect("supported body converts");
        assert_eq!(out, ast_body);
        assert_eq!(backend.hir_stmts_native, 3);
        assert_eq!(backend.hir_stmts_fallback, 0);

        // An unsupported statement splices the AST statement and counts.
        let other = HirStmt {
            id: NodeId(0),
            span: None,
            kind: HirStmtKind::Other("plugin_intercept"),
        };
        let mut mixed = hir_body.clone();
        mixed[1] = other;
        let mut backend = LlvmBackend::new();
        let out = hir_sourced_body(&mut backend, &mixed, &ast_body, "n")
            .expect("mixed body falls back per statement");
        assert_eq!(out, ast_body, "spliced statement equals AST");
        assert_eq!(backend.hir_stmts_native, 2);
        assert_eq!(backend.hir_stmts_fallback, 1);
    }

    #[test]
    fn length_mismatch_falls_back() {
        let mut backend = LlvmBackend::new();
        let hir_body: Vec<HirStmt> = Vec::new();
        let ast_body = vec![Statement::Break];
        assert!(hir_sourced_body(&mut backend, &hir_body, &ast_body, "x").is_none());
    }
}
