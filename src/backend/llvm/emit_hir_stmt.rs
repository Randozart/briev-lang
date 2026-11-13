// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 phase C statement seam: per-statement HIR-native body emission.
//!
//! The body loops walk the (HIR, source AST) pair: a statement whose native
//! arm emits is emitted from HIR (counted `native`); a statement whose native
//! arm declines (uncovered expression leaves, `Other` nodes) is spliced from
//! the SOURCE AST statement at the same pair slot (counted `fallback`) —
//! never guessed, never reconstructed. 2026-11-12 (H5 phase C): the H4.2
//! convert-then-substitute seam is gone — no statement is rebuilt through
//! `hir::to_ast` any more. Compiler-inserted statements (`ReleaseOwned`,
//! `TempTransfer`) have no source slot in the walk (`ReleaseOwned` is a pure
//! insert; `TempTransfer` consumed the return statement it replaced) and
//! always emit through their native arms. 2026-11-13 (H5 phase C): the
//! value-binding and term/match statement arms take their shape and fallback
//! expressions from the SOURCE statement at the same pair slot too, so this
//! module no longer touches `hir::to_ast` at all.

use super::{emit_stmt::emit_statement, LlvmBackend, TypedRegister};
use std::fmt::Write;
use crate::ast::Statement;
use crate::hir::expr::{HirExpr, HirExprKind};
use crate::ast::top::StmtMatchArm;
use crate::backend::llvm::AllocStrategy;
use crate::hir::stmt::{HirStmt, HirStmtKind};

/// True for a pure compiler insert: a statement with no source syntax that
/// consumes no AST slot in a pair walk. (`TempTransfer` is NOT one — it
/// replaced a source return statement, so it consumes that slot.)
pub(crate) fn is_pure_insert(kind: &HirStmtKind) -> bool {
    matches!(kind, HirStmtKind::ReleaseOwned { .. })
}

/// Pair alignment: HIR statements that consume an AST slot (every kind
/// except pure inserts) must equal the AST body length. A mismatch is
/// lowering drift — callers emit the plain AST body.
fn pair_counts_ok(hir_body: &[HirStmt], ast_body: &[Statement]) -> bool {
    hir_body
        .iter()
        .filter(|s| !is_pure_insert(&s.kind))
        .count()
        == ast_body.len()
}

/// 2026-11-12 (H5 phase C): validate that (hir_body, ast_body) can emit as a
/// pair — slot alignment between the HIR walk and the source body. The body
/// loops then emit HIR natively and splice the SOURCE statement whenever a
/// native arm declines. Replaces the H4.2 convert-then-substitute seam.
pub(crate) fn hir_sourced_body(hir_body: &[HirStmt], ast_body: &[Statement]) -> bool {
    pair_counts_ok(hir_body, ast_body)
}

fn coll_destroy_available(backend: &LlvmBackend) -> bool {
    // stdlib-extension gate: coll_destroy lives in the stdlib prelude. Bodies
    // compiled without stdlib (bare-compiler corpus tests) have no definition
    // to call — List releases are skipped (conservative leak, the pre-slice
    // fallback behavior) rather than emitting an undefined symbol.
    backend.ctx.defn_params.contains_key("coll_destroy")
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
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    let HirStmtKind::TempTransfer { name, ty, expr, releases, ret } = &stmt.kind else {
        return None;
    };
    // 2026-11-13 (H5 phase C): the transfer replaced a source return
    // statement at this pair slot; its expression is the source shape for the
    // synthetic binding — never reconstructed from HIR.
    let ast_expr = match ast {
        Some(Statement::Term(Some(e)))
        | Some(Statement::Tail(e))
        | Some(Statement::EndProgram(Some(e))) => Some(e),
        _ => None,
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
    let bound = emit_let_pair(backend, out, &binding, ast_expr, indent)?;
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
    // The synthetic return reads the temp binding, so its source expression is
    // the identifier — never the replaced statement's original expression.
    let ret_ast = Statement::Term(Some(crate::ast::Expr::Identifier(name.clone())));
    let _ = bound;
    emit_hir_statement(backend, out, &ret_stmt, Some(&ret_ast), indent)
}

/// Advance a pair-walk HIR cursor over pure inserts scheduled before the
/// next source statement, emitting each through its native arm (pure inserts
/// have no AST slot — native emission is their only path). The List gate
/// mirrors the native arm: without the stdlib prelude the release is skipped
/// (conservative leak, the pre-slice behavior), consuming no slot either way.
/// Returns the last native register when anything emitted, so the caller can
/// refresh `last_expr_reg` exactly like a native body slot.
pub(crate) fn emit_pending_inserts(
    backend: &mut LlvmBackend,
    out: &mut String,
    hir_body: &[HirStmt],
    hi: &mut usize,
    indent: &str,
) -> Option<TypedRegister> {
    let mut emitted = None;
    while let Some(hir_stmt) = hir_body.get(*hi) {
        let HirStmtKind::ReleaseOwned { storage, .. } = &hir_stmt.kind else {
            break;
        };
        backend.hir_stmts_seen += 1;
        if *storage == crate::hir::stmt::StorageKind::List && !coll_destroy_available(backend) {
            *hi += 1;
            continue;
        }
        if let Some(reg) = emit_hir_statement(backend, out, hir_stmt, None, indent) {
            emitted = Some(reg);
        }
        *hi += 1;
    }
    emitted
}

/// Native-first emission of one pair slot: the HIR statement emits through
/// its native arm; a declining native arm (`None` — uncovered expression
/// leaves, `Other` nodes) splices the SOURCE AST statement at the same slot.
/// Returns `Some` only for a NATIVE register — the caller refreshes
/// `last_expr_reg` exactly for native slots, the AST splice leaves it alone
/// (the AST walk's contract). Without a paired HIR statement this is the
/// plain AST emitter, which keeps the AST-fallback body loop one shape.
pub(crate) fn emit_body_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    hir_stmt: Option<&HirStmt>,
    ast: &Statement,
    indent: &str,
) -> Option<TypedRegister> {
    if let Some(hir_stmt) = hir_stmt {
        backend.hir_stmts_seen += 1;
        if let Some(reg) = emit_hir_statement(backend, out, hir_stmt, Some(ast), indent) {
            backend.hir_stmts_native += 1;
            return Some(reg);
        }
        backend.hir_stmts_fallback += 1;
    }
    emit_statement(backend, out, ast, indent);
    None
}

/// Knobs for one nested pair walk — each scope arm's exact parity contract
/// with its AST arm. `break_on_terminated` mirrors the match-arm body loop
/// (the only AST body loop that breaks on a terminated child);
/// `thread_last` keeps the value-block register contract (Block/serial arms
/// return the last child's register, initialized even for an empty body);
/// scopes whose AST arm drops child registers must pass `false` — a
/// register allocated here would shift every later %tN (register parity).
#[derive(Clone, Copy)]
pub(crate) struct ChildPairWalk<'a> {
    pub indent: &'a str,
    pub break_on_terminated: bool,
    pub thread_last: bool,
}

/// Emit a nested (HIR, AST) child pair — guard/block/serial bodies, match
/// arm bodies, foreach bodies. Same contract as the top-level body loops:
/// pure inserts flush natively before each source child; a declining native
/// child splices its source statement. Entry checks run BEFORE any emission
/// so a drift or unpairable scope falls back wholesale (no partial IR).
pub(crate) fn emit_children_pair(
    backend: &mut LlvmBackend,
    out: &mut String,
    hir_body: &[HirStmt],
    ast_body: &[Statement],
    walk: ChildPairWalk<'_>,
) -> Option<TypedRegister> {
    let ChildPairWalk { indent, break_on_terminated, thread_last } = walk;
    if !pair_counts_ok(hir_body, ast_body) {
        return None;
    }
    let mut last = thread_last.then(|| TypedRegister {
        name: backend.fun.gen_reg(),
        ty: crate::ast::Type::void(),
    });
    let mut hi = 0usize;
    let mut ai = 0usize;
    while hi < hir_body.len() {
        if break_on_terminated && backend.fun.terminated {
            break;
        }
        if let Some(reg) = emit_pending_inserts(backend, out, hir_body, &mut hi, indent) {
            last = Some(reg);
            continue;
        }
        let hir_stmt = &hir_body[hi];
        let ast_stmt = &ast_body[ai];
        hi += 1;
        ai += 1;
        backend.hir_stmts_seen += 1;
        match emit_hir_statement(backend, out, hir_stmt, Some(ast_stmt), indent) {
            Some(reg) => {
                backend.hir_stmts_native += 1;
                last = Some(reg);
            }
            None => {
                backend.hir_stmts_fallback += 1;
                let reg = emit_statement(backend, out, ast_stmt, indent);
                if thread_last {
                    last = Some(reg);
                }
            }
        }
    }
    last
}

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
fn emit_scoped_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    match &stmt.kind {
        HirStmtKind::Guarded(..) => {
            let Some(Statement::Guarded(_, ast_children)) = ast else {
                return None;
            };
            emit_guarded_statement(backend, out, stmt, ast_children, indent)
        }
        HirStmtKind::Block(children) => {
            let Some(Statement::Block(ast_children)) = ast else {
                return None;
            };
            emit_block_children(backend, out, children, ast_children, indent)
        }
        _ => None,
    }
}

/// Guarded (when) emission: condition through operand recursion, children
/// through the pair walk; bookkeeping verbatim from the AST arm. The child
/// pair is checked before the branch prelude emits, so an unpairable scope
/// declines BEFORE any IR (the caller splices the source statement).
fn emit_guarded_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast_children: &[Statement],
    indent: &str,
) -> Option<TypedRegister> {
    let HirStmtKind::Guarded(cond, body) = &stmt.kind else {
        return None;
    };
    let Some(cond_reg) = backend.emit_hir_operand(out, cond, indent) else {
        return None;
    };
    if !pair_counts_ok(body, ast_children) {
        return None;
    }
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
    let _ = emit_children_pair(
        backend,
        out,
        body,
        ast_children,
        ChildPairWalk { indent, break_on_terminated: false, thread_last: false },
    );
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

/// Block/serial-child emission: children through the pair walk, last
/// register wins (the AST arm's contract — Block, `mutex {}`, and the
/// legacy `sync {}` spelling share this exact shape).
fn emit_block_children(
    backend: &mut LlvmBackend,
    out: &mut String,
    children: &[HirStmt],
    ast_children: &[Statement],
    indent: &str,
) -> Option<TypedRegister> {
    if !pair_counts_ok(children, ast_children) {
        return None;
    }
    emit_children_pair(
        backend,
        out,
        children,
        ast_children,
        ChildPairWalk { indent, break_on_terminated: false, thread_last: true },
    )
}

/// H4 slice D dispatch group: value-binding statements (ArrowAssign, Let, Assign).
fn emit_value_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    // 2026-11-13 (H5 phase C): shape dispatch and fallback come from the
    // SOURCE AST at the same pair slot — never a reconstruction. HIR still
    // drives native value emission through the dual-fueled helpers.
    if let HirStmtKind::ArrowAssign { target, value, consume } = &stmt.kind {
        let Some(Statement::ArrowAssign { target: ast_target, value: ast_value, .. }) = ast else {
            return None;
        };
        let target = match (target.as_ref(), ast_target.as_deref()) {
            (Some(h), Some(a)) => Some(super::emit_stmt::ArrowOperand::HirSourced(h, a)),
            (None, None) => None,
            _ => return None,
        };
        let ctx = super::emit_stmt::ArrowAssignCtx {
            target,
            value: super::emit_stmt::ArrowOperand::HirSourced(value, ast_value),
            consume: *consume,
        };
        return Some(super::emit_stmt::emit_arrow_assign_statement(
            backend, out, indent, &ctx,
        ));
    }
    if let HirStmtKind::Let { .. } = &stmt.kind {
        let Some(Statement::Let { expr: ast_expr, .. }) = ast else {
            return None;
        };
        return emit_let_pair(backend, out, stmt, ast_expr.as_ref(), indent);
    }
    if let HirStmtKind::Assign(lhs, rhs) = &stmt.kind {
        let Some(Statement::Assign(ast_lhs, ast_rhs)) = ast else {
            return None;
        };
        return Some(super::emit_stmt::emit_assign_statement(
            backend,
            out,
            indent,
            ast_lhs,
            &super::emit_stmt::AssignValue::Hir(rhs, ast_rhs),
        ));
    }
    None
}

/// 2026-11-13 (H5 phase C): pair a Let HIR statement with its SOURCE AST
/// initializer (both `None` = bare let). The value dispatch reads the source
/// shape; a HIR initializer without a source expression cannot dispatch and
/// declines so the caller splices the source statement.
fn emit_let_pair(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast_expr: Option<&crate::ast::Expr>,
    indent: &str,
) -> Option<TypedRegister> {
    let HirStmtKind::Let { name, names, ty, expr, modifiers, .. } = &stmt.kind else {
        return None;
    };
    let value = match (expr.as_ref(), ast_expr) {
        (Some(h), Some(a)) => super::emit_stmt::LetValue::Hir(Some(h), a),
        (None, None) => super::emit_stmt::LetValue::Ast(None),
        _ => return None,
    };
    Some(super::emit_stmt::emit_let_statement(
        backend, out, indent, name, names, ty.as_ref(), modifiers, &value,
    ))
}

/// H4 slice D dispatch group: Foreach (shared loop-engine helper).
fn emit_foreach_statement_hir(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    // 2026-11-05 (H4 slice D): native Foreach — the iteration-source dispatch
    // reads the list's shape, the loop engine is the shared extracted helper,
    // and body children route through the pair walk. 2026-11-12 (H5 phase
    // C): the list expression and arm bodies come from the SOURCE statement
    // (no reconstruction); the pair check runs BEFORE the helper emits
    // (carrier allocas come first), so None falls back cleanly.
    if let HirStmtKind::Foreach { item, body, .. } = &stmt.kind {
        let Some(Statement::Foreach { list: ast_list, body: ast_body, .. }) = ast else {
            return None;
        };
        if !pair_counts_ok(body, ast_body) {
            return None;
        }
        super::emit_stmt::emit_foreach_statement(
            backend,
            out,
            indent,
            item,
            ast_list,
            super::emit_stmt::ForeachBody::Hir { hir: body, ast: ast_body },
        );
        return Some(TypedRegister {
            name: backend.fun.gen_reg(),
            ty: crate::ast::Type::void(),
        });
    }
    None
}

/// H4 slice D dispatch group: Term + Tail (the P-gated transaction exit).
fn emit_term_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    match &stmt.kind {
        HirStmtKind::Term(val) => {
            emit_term_value(backend, out, val, term_source_value(ast), indent)
        }
        HirStmtKind::Tail(e) => {
            let term = HirStmt {
                id: stmt.id,
                span: stmt.span,
                kind: HirStmtKind::Term(Some(e.clone())),
            };
            emit_hir_statement(backend, out, &term, ast, indent)
        }
        _ => None,
    }
}

/// The SOURCE value expression a paired Term/Tail statement carries.
fn term_source_value(ast: Option<&Statement>) -> Option<&crate::ast::Expr> {
    match ast? {
        Statement::Term(Some(e)) | Statement::Tail(e) => Some(e),
        _ => None,
    }
}

/// Term value emission + the P-gated exit. The value emits natively through
/// the operand path, falling back to the SOURCE AST expression at the pair
/// slot (no reconstruction).
fn emit_term_value(
    backend: &mut LlvmBackend,
    out: &mut String,
    val: &Option<crate::hir::expr::HirExpr>,
    ast_v: Option<&crate::ast::Expr>,
    indent: &str,
) -> Option<TypedRegister> {
    // 2026-11-13 (H5 phase C): a value term needs its source expression for
    // the collection-witness construction and the uncovered-leaf fallback.
    // Decline BEFORE the web flush so an unpaired term can never leave
    // partial IR (the caller then splices the source statement).
    if val.is_some() && ast_v.is_none() {
        return None;
    }
    if val.is_some() && backend.fun.inline_member_exit.is_none() {
        super::emit_stmt::emit_web_flush_batch(backend, out, indent);
        backend.flush_defer_cleanup(out, indent);
    }
    if let Some(v) = val {
        let saved = super::emit_stmt::begin_term_return_context(backend);
        let ast_v = ast_v.expect("guarded above");
        let returned_collection = if matches!(v.kind, HirExprKind::List(_)) {
            backend.fun.item_return_type.clone().and_then(|ty| {
                backend
                    .construct_local_collection(out, indent, &ty, ast_v)
                    .map(|(value, _)| value)
            })
        } else {
            None
        };
        let reg = returned_collection.unwrap_or_else(|| {
            match backend.emit_hir_operand(out, v, indent) {
                Some(r) => r,
                // The source AST covers every leaf the HIR operand path
                // declines; no reconstruction, no double emission.
                None => backend.emit_expr(out, ast_v, indent),
            }
        });
        super::emit_stmt::emit_term_value_tail(backend, out, indent, reg, saved);
    } else {
        super::emit_stmt::emit_bare_term_gate(backend, out, indent);
    }
    Some(TypedRegister {
        name: backend.fun.gen_reg(),
        ty: crate::ast::Type::void(),
    })
}

/// H4 slice D dispatch group: statement Match (phi chain).
fn emit_match_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    let HirStmtKind::Match { expr: scrutinee, arms } = &stmt.kind else {
        return None;
    };
    // 2026-11-13 (H5 phase C): scrutinee, arm patterns and bodies all come
    // from the SOURCE statement (no reconstruction). Arm count and per-arm
    // body pairing are checked before the label chain emits; the scrutinee
    // emits natively through the operand path with the source AST as its
    // uncovered-leaf fallback.
    let Some(Statement::Match { expr: ast_scrutinee, arms: ast_arms }) = ast else {
        return None;
    };
    if ast_arms.len() != arms.len() {
        return None;
    }
    for (arm, ast_arm) in arms.iter().zip(ast_arms) {
        if !pair_counts_ok(&arm.body, &ast_arm.body) {
            return None;
        }
    }
    let scrut = match backend.emit_hir_operand(out, scrutinee, indent) {
        Some(reg) => reg,
        None => backend.emit_expr(out, ast_scrutinee, indent),
    };
    let counter = backend.fun.txn_counter;
    backend.fun.txn_counter += 1;
    let merge = format!(".smt_end_{}", counter);
    let ctx = MatchCtx { scrut: &scrut, merge: &merge, counter, indent };
    emit_match_conditions(backend, out, arms, ast_arms, &ctx);
    emit_match_bodies(backend, out, arms, ast_arms, &ctx);
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

/// One arm's emission view (HIR body + source AST arm).
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
    ast_arms: &[StmtMatchArm],
    ctx: &MatchCtx<'_>,
) {
    for (i, _arm) in arms.iter().enumerate() {
        emit_match_condition_arm(backend, out, &ast_arms[i].patterns, ctx, i);
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

/// Phase 2: per-arm bodies (children through the pair walk) and merge edges.
fn emit_match_bodies(
    backend: &mut LlvmBackend,
    out: &mut String,
    arms: &[crate::hir::stmt::HirStmtMatchArm],
    ast_arms: &[StmtMatchArm],
    ctx: &MatchCtx<'_>,
) {
    for (i, arm) in arms.iter().enumerate() {
        let view = MatchArmView {
            hir: arm,
            patterns: &ast_arms[i].patterns,
            ast_children: &ast_arms[i].body,
        };
        emit_match_body_arm(backend, out, &view, ctx, i);
    }
}

/// One arm's body: bind patterns, children through the pair walk, merge edge.
fn emit_match_body_arm(
    backend: &mut LlvmBackend,
    out: &mut String,
    arm: &MatchArmView<'_>,
    ctx: &MatchCtx<'_>,
    i: usize,
) {
    let patterns = arm.patterns;
    let body_label = format!(".smt_body_{}_{}", ctx.counter, i);
    writeln!(out, "{}:", body_label).ok();
    backend.fun.cur_block = Some(body_label);
    backend.fun.terminated = false;
    for p in patterns {
        backend.bind_pattern(p, &ctx.scrut.name, &ctx.scrut.ty, out, ctx.indent);
    }
    let _ = emit_children_pair(
        backend,
        out,
        &arm.hir.body,
        arm.ast_children,
        ChildPairWalk {
            indent: ctx.indent,
            break_on_terminated: true,
            thread_last: false,
        },
    );
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
/// Defer registration: queue the cleanup body pair; flush runs it LIFO at
/// every exit (term/rollback/fallthrough). No code at the registration
/// point. The source AST body rides along so the flush can splice a
/// declining native child (no reconstruction at flush time).
fn emit_defer_statement(
    backend: &mut LlvmBackend,
    children: &[crate::hir::stmt::HirStmt],
    ast: Option<&Statement>,
) -> Option<TypedRegister> {
    let ast_children = match ast {
        Some(Statement::Defer(b)) => b.clone(),
        // Unpaired registration (should not occur — every Defer emission is
        // paired): the flush then emits HIR natively and skips a declining
        // child, the same silent skip the reconstruction seam had.
        _ => Vec::new(),
    };
    backend.fun.defer_bodies.push(super::context::DeferCleanupBody {
        hir: children.to_vec(),
        ast: ast_children,
    });
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

/// Terminal helper: SyncBlock/Mutex — a serial section emits inline (a
/// modifier must never be a speedup); children through the pair walk, the
/// exact Block shape.
fn emit_serial_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    children: &[HirStmt],
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    let ast_children = match ast {
        Some(Statement::SyncBlock(b)) | Some(Statement::Mutex(b)) => b.as_slice(),
        _ => return None,
    };
    emit_block_children(backend, out, children, ast_children, indent)
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
/// (Paired scope kinds — SyncBlock/Mutex/Defer — dispatch separately: they
/// need their source children for the pair walk.)
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
        HirStmtKind::EndProgram(val) => emit_end_program_statement(backend, out, val, indent),
        _ => None,
    }
}

/// H4 slice D: statement seam entry, now fully native (2026-11-12, H5 phase
/// C). Per-kind bodies live in the group functions above (Praetor split).
/// `ast` is the SOURCE statement at the same pair slot — scope kinds pull
/// their child pairs from it; every other kind ignores it. A `None` from the
/// owning group means "uncovered expression leaf / `Other` node": the CALLER
/// splices the source statement — there is no statement-level reconstruction
/// any more.
pub(crate) fn emit_hir_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    ast: Option<&Statement>,
    indent: &str,
) -> Option<TypedRegister> {
    match &stmt.kind {
        HirStmtKind::ReleaseOwned { storage, name } => {
            emit_release_owned_statement(backend, out, *storage, name, indent)
        }
        HirStmtKind::TempTransfer { .. } => {
            emit_temp_transfer_statement(backend, out, stmt, ast, indent)
        }
        HirStmtKind::Expression(_) => emit_expr_statement(backend, out, stmt, indent),
        HirStmtKind::Guarded(..) | HirStmtKind::Block(_) => {
            emit_scoped_statement(backend, out, stmt, ast, indent)
        }
        HirStmtKind::ArrowAssign { .. } | HirStmtKind::Let { .. } | HirStmtKind::Assign(..) => {
            emit_value_statement(backend, out, stmt, ast, indent)
        }
        HirStmtKind::Foreach { .. } => emit_foreach_statement_hir(backend, out, stmt, ast, indent),
        HirStmtKind::Term(_) | HirStmtKind::Tail(_) => {
            emit_term_statement(backend, out, stmt, ast, indent)
        }
        HirStmtKind::Match { .. } => emit_match_statement(backend, out, stmt, ast, indent),
        HirStmtKind::SyncBlock(children) | HirStmtKind::Mutex(children) => {
            emit_serial_statement(backend, out, children, ast, indent)
        }
        HirStmtKind::Defer(children) => emit_defer_statement(backend, children, ast),
        HirStmtKind::Break
        | HirStmtKind::Trap
        | HirStmtKind::Halt
        | HirStmtKind::Yield
        | HirStmtKind::Check(_)
        | HirStmtKind::Gate(_)
        | HirStmtKind::Rollback(_)
        | HirStmtKind::KeepHint(_)
        | HirStmtKind::FreeHint(_)
        | HirStmtKind::EndProgram(_) => emit_terminal_statement(backend, out, stmt, indent),
        // 2026-11-12 (H5 phase C): compile-time / analysis-only forms — the
        // AST arm emits only a void register and no IR (display, desugar,
        // trigger wiring, electronics disconnection; LLVM rejects InlineAsm
        // at the capability gate). Mirror that directly instead of
        // round-tripping through a reconstruction.
        HirStmtKind::Open(..)
        | HirStmtKind::TrgBinding { .. }
        | HirStmtKind::MetadataAssignment(..)
        | HirStmtKind::InlineAsm { .. } => {
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        }
        HirStmtKind::Other(_) => None,
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
    fn nested_storage_inserts_keep_the_pair_aligned() {
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
        // The pass releases the owned local inside the guard body.
        let guard_hir = use_fn
            .body
            .statements
            .iter()
            .find_map(|st| match &st.kind {
                HirStmtKind::Guarded(_, b) => Some(b.clone()),
                _ => None,
            })
            .expect("guard");
        let guard_inserts = guard_hir
            .iter()
            .filter(|st| {
                matches!(
                    &st.kind,
                    HirStmtKind::TempTransfer { .. } | HirStmtKind::ReleaseOwned { .. }
                )
            })
            .count();
        assert!(guard_inserts > 0, "release expected inside the guard body");

        let ast_body: Vec<Statement> = items
            .iter()
            .find_map(|i| match i {
                crate::ast::TopLevel::Definition(d) if d.name == "use" => Some(d.body.clone()),
                _ => None,
            })
            .expect("ast defn");
        let guard_ast_children: Vec<Statement> = ast_body
            .iter()
            .find_map(|st| match st {
                Statement::Guarded(_, b) => Some(b.clone()),
                _ => None,
            })
            .expect("ast guard");

        // 2026-11-12 (H5 phase C): inserts consume no AST slot, so the pair
        // stays aligned at the top level AND inside the nested scope — the
        // whole reason the old convert-then-substitute seam existed.
        assert!(hir_sourced_body(&use_fn.body.statements, &ast_body));
        assert!(super::pair_counts_ok(&guard_hir, &guard_ast_children));
        assert_eq!(guard_hir.len() - guard_inserts, guard_ast_children.len());
    }

    #[test]
    fn per_statement_accounting_native_and_fallback() {
        // Native: a covered expression statement emits from HIR.
        let lit = HirExpr {
            id: NodeId(0),
            span: None,
            ty: None,
            kind: HirExprKind::Literal(crate::hir::expr::HirLiteral::Int(1)),
        };
        let native_stmt = HirStmt {
            id: NodeId(1),
            span: None,
            kind: HirStmtKind::Expression(lit.clone()),
        };
        let ast_native = Statement::Expression(crate::ast::Expr::Decimal(1));
        let mut backend = LlvmBackend::new();
        let mut out = String::new();
        emit_body_statement(&mut backend, &mut out, Some(&native_stmt), &ast_native, "  ");
        assert_eq!(backend.hir_stmts_seen, 1);
        assert_eq!(backend.hir_stmts_native, 1);
        assert_eq!(backend.hir_stmts_fallback, 0);

        // Fallback: an Other statement splices the source statement.
        let other = HirStmt {
            id: NodeId(2),
            span: None,
            kind: HirStmtKind::Other("plugin_intercept"),
        };
        let mut backend = LlvmBackend::new();
        let mut out = String::new();
        emit_body_statement(&mut backend, &mut out, Some(&other), &ast_native, "  ");
        assert_eq!(backend.hir_stmts_seen, 1);
        assert_eq!(backend.hir_stmts_native, 0);
        assert_eq!(backend.hir_stmts_fallback, 1);

        // AST-only slot (no HIR pairing): plain emitter, no counting.
        let mut backend = LlvmBackend::new();
        let mut out = String::new();
        emit_body_statement(&mut backend, &mut out, None, &ast_native, "  ");
        assert_eq!(backend.hir_stmts_seen, 0);
        assert_eq!(backend.hir_stmts_native, 0);
        assert_eq!(backend.hir_stmts_fallback, 0);
    }

    #[test]
    fn length_mismatch_falls_back() {
        let hir_body: Vec<HirStmt> = Vec::new();
        let ast_body = vec![Statement::Break];
        assert!(!hir_sourced_body(&hir_body, &ast_body));
    }
}
