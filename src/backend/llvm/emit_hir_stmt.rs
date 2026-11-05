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
pub(crate) fn emit_hir_statement(
    backend: &mut LlvmBackend,
    out: &mut String,
    stmt: &HirStmt,
    indent: &str,
) -> Option<TypedRegister> {
    // 2026-11-05 (H4 slice D): native families first. An `Expression`
    // statement whose expression is family-covered (literal/identifier) is
    // emitted without any AST conversion; the plain fallthrough mirrors the
    // AST arm (the AddrOf/Match special cases cannot fire on these leaves).
    if let HirStmtKind::Expression(e) = &stmt.kind {
        // Family-covered expressions AND calls: the Call arm guards its own
        // argument reconstructibility before emitting anything, so a None
        // here falls through to conversion with no partial IR.
        let call_native = matches!(&e.kind, HirExprKind::Call { .. });
        if call_native || super::emit_expr::hir_expr_family_covered(e) {
            if let Some(reg) = backend.emit_hir_expr(out, e, indent) {
                backend.fun.last_expr_reg = Some(reg.name.clone());
                return Some(reg);
            }
        }
    }
    // 2026-11-05 (H4 slice D, control flow): native Guarded and Block arms —
    // the condition/children emit through the seam (native or per-node
    // conversion), the bookkeeping mirrors the AST arms verbatim. Children of
    // a seam-reachable Guarded/Block are reconstructible (the parent converted
    // in hir_sourced_body), so the per-child convert fallback never fires in
    // practice; it exists so no path can double-emit.
    if let HirStmtKind::Guarded(cond, body) = &stmt.kind {
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
        return Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() });
    }
    if let HirStmtKind::Block(children) = &stmt.kind {
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
        return Some(last);
    }
    // 2026-11-05 (H4 slice D): native Term — the P-gated transaction exit.
    // Reconstructibility is checked BEFORE the web-flush/defer prefix (which
    // emits IR), so an unsupported value falls back with no partial emission.
    // Value emission goes through operand recursion (covered leaves native);
    // the return-context setup, list-literal construction, and the whole
    // tail (backedge, float unbox, member capture, callable store, ret
    // paths) share the AST arm's extracted helpers verbatim.
    if let HirStmtKind::Term(val) = &stmt.kind {
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
    // 2026-11-05 (H4 slice D): `tail expr;` IS `term expr;` (the AST arm
    // delegates verbatim) — route through the native Term path; the Term
    // arm's reconstructibility guard keeps the legacy convert path for
    // unsupported values.
    if let HirStmtKind::Tail(e) = &stmt.kind {
        let term = HirStmt {
            id: stmt.id,
            span: stmt.span,
            kind: HirStmtKind::Term(Some(e.clone())),
        };
        return emit_hir_statement(backend, out, &term, indent);
    }
    // 2026-11-05 (H4 slice D): native statement match — scrutinee through
    // operand recursion, patterns reconstructed (lossless) for the shared
    // condition/binding helpers, arm children through the seam. Patterns and
    // scrutinee are reconstructed BEFORE the scrutinee emits, so a None here
    // falls back with no partial IR.
    if let HirStmtKind::Match { expr: scrutinee, arms } = &stmt.kind {
        let Some(ast_scrut) = crate::hir::to_ast::expr(scrutinee) else {
            return None;
        };
        let mut ast_arms = Vec::with_capacity(arms.len());
        for arm in arms {
            let mut pats = Vec::with_capacity(arm.patterns.len());
            for p in &arm.patterns {
                match crate::hir::to_ast::pattern(p) {
                    Some(p) => pats.push(p),
                    None => return None,
                }
            }
            let Some(body) = crate::hir::to_ast::stmt_body(&arm.body) else {
                return None;
            };
            ast_arms.push((pats, body));
        }
        let Some(scrut) = backend.emit_hir_operand(out, scrutinee, indent) else {
            return None;
        };
        let counter = backend.fun.txn_counter;
        backend.fun.txn_counter += 1;
        let merge = format!(".smt_end_{}", counter);
        for (i, arm) in arms.iter().enumerate() {
            let next = format!(".smt_next_{}_{}", counter, i);
            let mut cond: Option<String> = None;
            for pat in &ast_arms[i].0 {
                if matches!(
                    pat,
                    crate::ast::Pattern::Wildcard | crate::ast::Pattern::Binding(_)
                ) {
                    cond = Some(format!("{}", backend.fun.gen_reg()));
                    writeln!(out, "  {} = icmp eq i64 0, 0", cond.as_deref().unwrap_or("")).ok();
                    break;
                }
                let c =
                    backend.emit_pattern_condition(pat, &scrut.name, &scrut.ty, out, indent);
                cond = Some(match cond {
                    None => c,
                    Some(prev) => {
                        let r = backend.fun.gen_reg();
                        writeln!(out, "  {} = or i1 {}, {}", r, prev, c).ok();
                        format!("{}", r)
                    }
                });
            }
            let body_label = format!(".smt_body_{}_{}", counter, i);
            let cond = cond.unwrap_or_else(|| {
                let r = backend.fun.gen_reg();
                writeln!(out, "  {} = icmp eq i64 0, 0", r).ok();
                r
            });
            writeln!(out, "  br i1 {}, label %{}, label %{}", cond, body_label, next).ok();
            writeln!(out, "{}:", next).ok();
            backend.fun.cur_block = Some(next.clone());
        }
        writeln!(out, "  br label %{}", merge).ok();
        for (i, arm) in arms.iter().enumerate() {
            let body_label = format!(".smt_body_{}_{}", counter, i);
            writeln!(out, "{}:", body_label).ok();
            backend.fun.cur_block = Some(body_label.clone());
            backend.fun.terminated = false;
            for p in &ast_arms[i].0 {
                backend.bind_pattern(p, &scrut.name, &scrut.ty, out, indent);
            }
            let hir_children = &arm.body;
            let ast_children = &ast_arms[i].1;
            for (j, child) in hir_children.iter().enumerate() {
                if backend.fun.terminated {
                    break;
                }
                if emit_hir_statement(backend, out, child, indent).is_none() {
                    emit_statement(backend, out, &ast_children[j], indent);
                }
            }
            if !backend.fun.terminated {
                writeln!(out, "  br label %{}", merge).ok();
            }
        }
        writeln!(out, "{}:", merge).ok();
        if backend.fun.terminated {
            writeln!(out, "  unreachable").ok();
        } else {
            backend.fun.cur_block = Some(merge);
        }
        return Some(TypedRegister {
            name: backend.fun.gen_reg(),
            ty: crate::ast::Type::void(),
        });
    }
    // 2026-11-05 (H4 slice D): the small control/terminal statements,
    // mirrored verbatim from the AST arms (no emission before the match, so
    // no double-emit risk; each is total over its HIR kind).
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
        HirStmtKind::Halt => {
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
        HirStmtKind::Yield => {
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        }
        HirStmtKind::Check(cond) => {
            let Some(cond_reg) = backend.emit_hir_operand(out, cond, indent) else {
                return None;
            };
            let cond_i1 = super::emit_stmt::guard_cond_i1(backend, out, indent, &cond_reg);
            let noop = backend.fun.gen_reg();
            writeln!(out, "{}{} = add i64 0, 0", indent, noop).ok();
            Some(TypedRegister { name: noop, ty: crate::ast::Type::void() })
        }
        HirStmtKind::Gate(cond) => {
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
        HirStmtKind::Rollback(_) => {
            // 2026-08-09 (Phase 10): deferred cleanup runs on rollback too.
            backend.flush_defer_cleanup(out, indent);
            if backend.fun.fn_ret_ty == "void" {
                writeln!(out, "{}ret void", indent).ok();
            } else {
                let zero = if backend.fun.fn_ret_ty == "float" || backend.fun.fn_ret_ty == "double" {
                    "0.0".to_string()
                } else if backend.fun.fn_ret_ty == "ptr" {
                    "null".to_string()
                } else {
                    "0".to_string()
                };
                writeln!(out, "{}ret {} {}", indent, backend.fun.fn_ret_ty, zero).ok();
            }
            backend.fun.terminated = true;
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        }
        HirStmtKind::KeepHint(_) => {
            // 2026-08-01 (Phase 5): keep; is a scheduler directive — no
            // runtime emission.
            Some(TypedRegister { name: backend.fun.gen_reg(), ty: crate::ast::Type::void() })
        }
        HirStmtKind::FreeHint(name) => {
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
        HirStmtKind::SyncBlock(children) | HirStmtKind::Mutex(children) => {
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
        HirStmtKind::EndProgram(val) => {
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
        _ => None,
    }
    .or_else(|| {
        match convert_hir_statement(stmt) {
            Some(ast) => Some(emit_statement(backend, out, &ast, indent)),
            None => None,
        }
    })
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
