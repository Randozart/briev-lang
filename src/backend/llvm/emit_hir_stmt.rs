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
use crate::hir::expr::HirExprKind;
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
pub(crate) fn hir_sourced_body(
    backend: &mut LlvmBackend,
    hir_body: &[HirStmt],
    ast_body: &[Statement],
    name: &str,
) -> Option<Vec<Statement>> {
    if hir_body.len() != ast_body.len() {
        return None;
    }
    let mut out = Vec::with_capacity(hir_body.len());
    for (hir_stmt, ast_stmt) in hir_body.iter().zip(ast_body.iter()) {
        backend.hir_stmts_seen += 1;
        match convert_hir_statement(hir_stmt) {
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
        let Some(ast_target) = target.as_ref().and_then(crate::hir::to_ast::expr) else {
            return None;
        };
        let Some(ast_value) = crate::hir::to_ast::expr(value) else {
            return None;
        };
        let boxed_target = Box::new(ast_target);
        let boxed_value = Box::new(ast_value);
        return Some(super::emit_stmt::emit_arrow_assign_statement(
            backend,
            out,
            indent,
            &Some(boxed_target),
            &super::emit_stmt::ArrowAssignValue::Hir(value, &boxed_value),
            consume,
        ));
    }
    // 2026-11-05 (H4 slice D): native Let — the binding dispatch reads the
    // reconstructed shapes (spawn handles, bare-let slots, collection
    // scaffolds); covered initializer leaves emit natively through the
    // dual-fueled value path. Reconstructibility checked BEFORE any
    // emission (task-handle registration and slot binding come first).
    if let HirStmtKind::Let { name, names, ty, expr, modifiers, .. } = &stmt.kind {
        // HIR keeps modifier NAMES; the AST arm reads names only (`vol`),
        // so name-only Annotations are lossless for emission.
        let ast_modifiers: Vec<crate::ast::top::Annotation> = modifiers
            .iter()
            .map(|m| crate::ast::top::Annotation { name: m.clone(), value: None })
            .collect();
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
                &ast_modifiers,
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
            &ast_modifiers,
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
    if let HirStmtKind::Foreach { item, list, body } = &stmt.kind {
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
