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
