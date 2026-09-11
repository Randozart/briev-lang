// ── Slice Narrowing Pass ──────────────────────────────────────────────
// 2026-07-26: Walks expressions and narrows constant-bounds Expr::Slice
// to direct array access. Contiguous slices (stride 1, constant bounds)
// get replaced with the base array expression. Strided or dynamic slices
// remain for runtime evaluation.
//
// Flat control flow: max 2 levels, guard clauses, early returns.

use crate::ast::{Expr, TopLevel, Statement};

/// Narrow all constant-bounds Expr::Slice nodes in a program.
pub fn narrow_slices(items: &mut [TopLevel]) {
    for item in items.iter_mut() {
        match item {
            TopLevel::Definition(defn) => {
                walk_stmts(&mut defn.body);
            }
            TopLevel::Transaction(txn) => {
                walk_stmts(&mut txn.body);
            }
            _ => {}
        }
    }
}

fn walk_stmts(stmts: &mut [Statement]) {
    for stmt in stmts.iter_mut() {
        match stmt {
            Statement::Expression(expr)
            | Statement::Term(Some(expr))
            | Statement::EndProgram(Some(expr))
            | Statement::Rollback(Some(expr)) => {
                walk_expr(expr);
            }
            Statement::Let { expr: Some(e), .. } => walk_expr(e),
            Statement::Assign(_, e) => walk_expr(e),
            Statement::Guarded(cond, body) => {
                walk_expr(cond);
                walk_stmts(body);
            }
            Statement::Gate(cond) => walk_expr(cond),
            Statement::Block(body) => walk_stmts(body),
            _ => {}
        }
    }
}

fn walk_expr(expr: &mut Expr) {
    match expr {
        Expr::Slice { array, start, end, stride } => {
            walk_expr(array);
            if let Some(s) = start { walk_expr(s); }
            if let Some(e) = end { walk_expr(e); }
            if let Some(s) = stride { walk_expr(s); }

            let stride_val = stride.as_ref().and_then(|e| expr_as_i64(e)).unwrap_or(1i64);
            if stride_val != 1 {
                return;
            }
            // 2026-08-13 (String slices): narrowing a slice to its base
            // array is only valid for Vector offset-views. A String slice
            // `s[0:idx]` IS a substring — replacing it with `s` silently
            // returns the whole string. This pass has NO type info, so it
            // cannot prove the base is a Vector: identifier-based slices
            // were exempted then, but 2026-08-28 proved non-identifier
            // bases are equally unsafe (`mk()[1:3]` on a String-returning
            // fn narrowed to `mk()` — whole string again). The backend's
            // Slice arm is type-aware (briev_str_substr for String, the
            // base-array offset view for Vectors), so EVERY slice is left
            // to it. This pass is now a pure walk — kept only so nested
            // slices inside compound expressions are visited.
            let _ = (array, start, end, stride);
            return;
        }
        Expr::BinaryOp(_, a, b) => { walk_expr(a); walk_expr(b); }
        Expr::UnaryOp(_, a) => { walk_expr(a); }
        Expr::Call(_, args, _) => { for a in args { walk_expr(a); } }
        Expr::Index(arr, idx) => { walk_expr(arr); walk_expr(idx); }
        Expr::Field(obj, _) => { walk_expr(obj); }
        Expr::If(c, t, e) => {
            walk_expr(c);
            walk_expr(t);
            if let Some(el) = e { walk_expr(el); }
        }
        Expr::Match(val, arms) => {
            walk_expr(val);
            for arm in arms { walk_expr(&mut arm.body); }
        }
        Expr::Tuple(items) | Expr::List(items) => {
            for item in items { walk_expr(item); }
        }
        Expr::Block(stmts) => walk_stmts(stmts),
        Expr::Lambda(_, body) => walk_expr(body),
        Expr::StructLiteral { fields, .. } => {
            for (_, f) in fields { walk_expr(f); }
        }
        Expr::PluginIntercept { args, .. } => {
            for a in args { walk_expr(a); }
        }
        _ => {}
    }
}

/// Try to extract a constant integer from an expression.
fn expr_as_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Decimal(n) => Some(*n),
        Expr::UnaryOp(kind, inner) => {
            let v = expr_as_i64(inner)?;
            match kind {
                crate::ast::UnaryOpKind::Neg => Some(-v),
                _ => None,
            }
        }
        _ => None,
    }
}
