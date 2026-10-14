// ── Callable Txn Inline Cost ────────────────────────────────────────
//
// 2026-07-31: Phase 2 (plan §7.3) — weighted instruction count for the
// callable-txn auto-inline decision, computed ONCE in the frontend.
// Replaces the backend's `params < 8 && body < 20 && !ffi` heuristic at
// emit_toplevel.rs:2196-2204.
//
// The weight model (call=10, binop=1, load/store=2) approximates the LLVM
// instruction cost of inlining the body. A small pure helper (e.g.
// memcmp_loop) is alwaysinline so LLVM can optimize cross-function; a large
// or FFI-calling body stays a real call.
//
// TEMP: 2026-07-31 — the threshold 40 is a constant here; plan §8.2 moves
// it to config/ir-lowering.toml `callable_inline_weight_threshold` in
// Phase 3. The `params < 8` gate is intentionally dropped: no stdlib
// callable txn exceeds 6 params and the weight bound caps body size, so the
// param gate adds no signal (verified against lib/std in Phase 2).

use crate::ast::{Expr, Statement, Transaction};
use crate::analysis::region::has_ffi_or_trigger_stmt_in_chain;

/// Whether a callable txn should be emitted with `alwaysinline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineDecision {
    /// Emit `alwaysinline` — small, pure body.
    AlwaysInline,
    /// Emit no inline attribute — body too large or contains FFI.
    No,
}

/// Decide the auto-inline attribute for a callable txn.
///
/// 2026-07-31: Inline when the weighted body cost is ≤ the threshold AND the
/// body has no FFI/trigger statements (a call inside the body defeats the
/// inline benefit and could hoist an external side effect into every caller).
/// The threshold is config-driven (plan §8.2 `callable_inline_weight_threshold`,
/// default 40) — see config/ir-lowering.toml.
pub fn callable_inline_decision(txn: &Transaction) -> InlineDecision {
    if has_ffi_or_trigger_stmt_in_chain(&txn.body) {
        return InlineDecision::No;
    }
    if weight_of_body(&txn.body) <= crate::config_tuning::ir_lowering().callable_inline_weight_threshold {
        InlineDecision::AlwaysInline
    } else {
        InlineDecision::No
    }
}

/// Weighted instruction cost of a txn body.
///
/// 2026-07-31: Statement base cost 1, plus expression weights
/// (call=10, binop=1, operand use=1). Guarded/Block/Foreach recurse.
/// The load/store weight (2) is folded into statement + identifier costs —
/// a field read is one identifier use, a state store is one Assign.
fn weight_of_body(body: &[Statement]) -> u32 {
    body.iter().map(weight_of_stmt).sum()
}

fn weight_of_stmt(stmt: &Statement) -> u32 {
    match stmt {
        Statement::Let { expr, .. } => 1 + expr.as_ref().map_or(0, weight_of_expr),
        Statement::Assign(lhs, rhs) => 1 + weight_of_expr(lhs) + weight_of_expr(rhs),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
            1 + weight_of_expr(e)
        }
        Statement::Term(None) | Statement::EndProgram(None) => 1,
        Statement::Expression(e) => 1 + weight_of_expr(e),
        Statement::Guarded(cond, body) => 1 + weight_of_expr(cond) + weight_of_body(body),
        Statement::Gate(e) => 1 + weight_of_expr(e),
        Statement::Block(b) => 1 + weight_of_body(b),
        Statement::Foreach { list, body, .. } => 1 + weight_of_expr(list) + weight_of_body(body),
        Statement::MetadataAssignment(_, _) => 1,
        Statement::Rollback(Some(e)) => 1 + weight_of_expr(e),
        Statement::Rollback(None) => 1,
        _ => 1,
    }
}

fn weight_of_expr(expr: &Expr) -> u32 {
    match expr {
        Expr::Call(_, args, _) => 10 + args.iter().map(weight_of_expr).sum::<u32>(),
        Expr::BinaryOp(_, l, r) => 1 + weight_of_expr(l) + weight_of_expr(r),
        Expr::UnaryOp(_, e) => 1 + weight_of_expr(e),
        Expr::Identifier(_) => 1,
        Expr::Index(a, i) => 1 + weight_of_expr(a) + weight_of_expr(i),
        Expr::Field(o, _) => 1 + weight_of_expr(o),
        Expr::Slice { array, start, end, stride } => {
            1 + weight_of_expr(array)
                + start.as_ref().map_or(0, |b| weight_of_expr(b))
                + end.as_ref().map_or(0, |b| weight_of_expr(b))
                + stride.as_ref().map_or(0, |b| weight_of_expr(b))
        }
        Expr::Cast(e, _) | Expr::Deref(e) | Expr::AddrOf(e) | Expr::Within(e, _) => {
            1 + weight_of_expr(e)
        }
        Expr::Tuple(ts) | Expr::List(ts) => 1 + ts.iter().map(weight_of_expr).sum::<u32>(),
        Expr::Block(stmts) => weight_of_body(stmts),
        Expr::If(c, t, e) => {
            1 + weight_of_expr(c) + weight_of_expr(t) + e.as_ref().map_or(0, |b| weight_of_expr(b))
        }
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Contract};

    fn id(name: &str) -> Expr {
        Expr::Identifier(name.to_string())
    }

    fn add(l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Add, Box::new(l), Box::new(r))
    }

    fn call(name: &str) -> Expr {
        Expr::Call(name.to_string(), vec![], None)
    }

    fn stmt_let(name: &str, e: Expr) -> Statement {
        Statement::Let {
            name: name.to_string(),
            names: Vec::new(),
            ty: None,
            expr: Some(e),
            modifiers: Vec::new(),
        }
    }

    fn txn(name: &str, body: Vec<Statement>) -> Transaction {
        Transaction {
            variadic_param: None,
            name: name.to_string(),
            is_reactive: false,
            is_async: false,
            type_params: Vec::new(),
            parameters: Vec::new(),
            output_type: None,
            outputs: Vec::new(),
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: Vec::new(),
            span: None,
            doc: None,
        }
    }

    /// A small pure helper (memcmp_loop pattern WITHOUT a term — term is
    /// treated as ffi-like by has_ffi_or_trigger_stmt_in_chain) is inlined.
    #[test]
    fn small_pure_body_inlines() {
        let body = vec![
            stmt_let("a", add(id("x"), id("y"))),
            stmt_let("b", add(id("a"), id("z"))),
            Statement::Assign(id("out"), id("b")),
        ];
        let t = txn("small", body);
        assert_eq!(callable_inline_decision(&t), InlineDecision::AlwaysInline);
    }

    /// A body with many `#`-calls exceeds the weight threshold and is NOT
    /// inlined (Load# is a `#` intrinsic, so the ffi gate does not fire —
    /// the weight model makes the decision).
    #[test]
    fn heavy_body_not_inlined() {
        let mut body = Vec::new();
        for _ in 0..10 {
            body.push(stmt_let("v", call("Load#")));
        }
        let t = txn("heavy", body);
        assert_eq!(callable_inline_decision(&t), InlineDecision::No);
    }

    /// An FFI-calling body (non-# call) is never inlined, even if tiny.
    #[test]
    fn ffi_body_never_inlined() {
        let body = vec![stmt_let("v", call("frgn_fn"))];
        let t = txn("ffi", body);
        assert_eq!(callable_inline_decision(&t), InlineDecision::No);
    }

    /// A `term` statement triggers the ffi/terminator gate → never inlined
    /// (matches the old backend: callable txns that return via `term` are
    /// never auto-inlined because has_ffi_or_terminator_stmt treats Term as
    /// ffi-like, emit_toplevel.rs:2199).
    #[test]
    fn term_statement_blocks_inline() {
        let body = vec![stmt_let("v", add(id("i"), id("j"))), Statement::Term(Some(id("v")))];
        let t = txn("term", body);
        assert_eq!(callable_inline_decision(&t), InlineDecision::No);
    }

    /// Guards contribute to the weight.
    #[test]
    fn guarded_body_weights_contribute() {
        let guard = Statement::Guarded(
            add(id("i"), id("n")),
            vec![stmt_let("inner", add(id("i"), id("j")))],
        );
        let body = vec![guard];
        let t = txn("guarded", body);
        // weight: Guarded(1 + 1 + (1+1+1)) = 6 ≤ 40
        assert_eq!(callable_inline_decision(&t), InlineDecision::AlwaysInline);
    }

    /// An empty body is trivially inlined.
    #[test]
    fn empty_body_inlines() {
        let t = txn("empty", vec![]);
        assert_eq!(callable_inline_decision(&t), InlineDecision::AlwaysInline);
    }
}
