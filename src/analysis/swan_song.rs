// ── Swan-Song Hoisting ─────────────────────────────────────────────
//
// 2026-07-31: Frontend-driven dispatch (Phase 1). The terminating-guard
// hoist moved from src/backend/llvm/mod.rs:136-204 into a backend-agnostic
// analysis pass so webstack/circt can reuse it and the LLVM backend consumes
// it via AnalysisResults.swan_songs.
//
// A "swan song" is the terminal observable side effect of a convergent loop:
//
//     node mb [count < N][count == N] {
//         ...
//         when count == N { term! -> PrintLn!(nesc); };   // ← hoisted
//         term;
//     };
//
// The hoist removes the terminating guard from the loop body (so the hot
// loop has no extra branch for a block that runs exactly once at the end)
// and returns the guard body for re-emission in the post-loop block.
// The `let_to_field` remap is required because the hoisted body may reference
// a let binding (e.g. `nesc` in mandelbrot) whose register is only valid
// inside the loop body; the value lives in a state field, so identifiers are
// rewritten to the field name.

use crate::ast::{Expr, Statement};
use std::collections::{HashMap, HashSet};

/// Detect whether a transaction body has a hoistable swan song.
///
/// Returns true exactly when `hoist_swan_song` would remove a terminating
/// guard: after discarding trailing `term`/`term!` statements, the last
/// remaining statement is a `Guarded` block whose body contains a `term!`.
/// This is the *correct* trigger for the loop-dispatch's purity gate — a
/// hoisted swan song would be silently dropped by the pure counter fold, so
/// the fold must be blocked exactly when a hoist fires.
pub fn has_swan_song(body: &[Statement]) -> bool {
    let stmts: Vec<&Statement> = body.iter()
        .filter(|s| !matches!(s, Statement::Term(..) | Statement::EndProgram(..) | Statement::Tail(_)))
        .collect();
    match stmts.last() {
        Some(Statement::Guarded(_, statements)) => {
            statements.iter().any(|s| matches!(s, Statement::EndProgram(..)))
        }
        _ => false,
    }
}

/// Hoist terminating guard(s) at the end of a body.
///
/// Returns `(body_without_guard, vec_of_guard_bodies)` where each guard body
/// is the re-emitted swan song (with let references remapped to state fields).
///
/// `state_fields` is the set of state field names (formerly the keys of the
/// backend's `field_index_map`). It gates the `let_to_field` remap: a let
/// binding is only remapped when the field it was assigned from is a real
/// state field.
///
/// The hoist runs in a loop so ALL trailing terminating guards are removed,
/// not just the last one. A non-terminating trailing guard (e.g. a plain
/// `when done { store }`) breaks the loop and is left in the body.
pub fn hoist_swan_song(
    body: &[Statement],
    state_fields: &HashSet<String>,
) -> (Vec<Statement>, Vec<Vec<Statement>>) {
    let mut stmts: Vec<&Statement> = body.iter()
        .filter(|s| !matches!(s, Statement::EndProgram(..) | Statement::Term(None)))
        .collect();
    // 2026-07-05: Build let-to-state-field mapping from body assignments.
    // When the hoisted swan song references a let binding (like nesc in
    // mandelbrot), the done: block can't use the body's register. We remap
    // the let binding to the state field that stores its value.
    // Pattern: &field_name = let_name  →  map[let_name] = field_name
    let mut let_to_field: HashMap<String, String> = HashMap::new();
    for s in body {
        if let Statement::Assign(lhs, Expr::Identifier(let_name)) = s {
            if let Some(field_name) = lhs.as_var_name() {
                if state_fields.contains(field_name) {
                    let_to_field.insert(let_name.clone(), field_name.to_string());
                }
            }
        }
    }
    let mut hoist: Vec<Vec<Statement>> = Vec::new();
    while let Some(last_idx) = stmts.len().checked_sub(1) {
        if let Statement::Guarded(_, statements) = &stmts[last_idx] {
            let is_terminating = statements.iter().any(|s| matches!(s, Statement::EndProgram(..)));
            if !is_terminating {
                break;
            }
            // Hoist the entire guard body (all statements before the term!)
            // into a Vec<Statement> that the post-loop block can re-emit.
            // This handles both simple field-print patterns (original hoisting)
            // and let-binding-based patterns (nbody: energy computation + print).
            let mut body_stmts: Vec<Statement> = statements.iter()
                .filter(|s| !matches!(s, Statement::EndProgram(..)))
                .cloned()
                .collect();
            // Remap let binding references to state field names in hoisted body.
            for s in &mut body_stmts {
                remap_stmt_identifiers(s, &let_to_field);
            }
            let swan_song_stmt = statements.iter().find_map(|s| {
                if let Statement::EndProgram(Some(ss)) = s {
                    Some(ss.clone())
                } else {
                    None
                }
            });
            // Remap swan song identifiers too.
            let swan_song_stmt = swan_song_stmt.map(|mut ss| {
                remap_expr_into(&mut ss, &let_to_field);
                ss
            });
            // 2026-07-04: Hoist even when body_stmts is empty — the
            // guard may be just `term! -> print_int#(result)` with no
            // preceding statements. Previously we only hoisted when
            // body_stmts was non-empty, leaving the swan song in the
            // body and blocking Path A (no-dead-stores) because
            // pending_post_hoist was empty.
            if !body_stmts.is_empty() || swan_song_stmt.is_some() {
                let mut full_body = body_stmts;
                if let Some(sw) = swan_song_stmt {
                    full_body.push(Statement::Expression(sw));
                }
                hoist.push(full_body);
                stmts.pop();
            }
            break;
        } else {
            break;
        }
    }
    let body_vec: Vec<Statement> = stmts.into_iter().cloned().collect();
    (body_vec, hoist)
}

/// Recursively remap identifiers in a statement using the let-to-field map.
/// 2026-07-31: pub(crate) — the batch-loop emission (emit_countable_batched_main)
/// remaps guard-body identifiers to state fields at the boundary.
pub(crate) fn remap_stmt_identifiers(s: &mut Statement, map: &HashMap<String, String>) {
    match s {
        Statement::Assign(_, expr) => {
            remap_expr_into(expr, map);
        }
        Statement::Expression(e) => {
            remap_expr_into(e, map);
        }
        Statement::EndProgram(Some(ss)) => {
            remap_expr_into(ss, map);
        }
        Statement::Guarded(condition, statements) => {
            remap_expr_into(condition, map);
            for stmt in statements.iter_mut() {
                remap_stmt_identifiers(stmt, map);
            }
        }
        Statement::Let { expr: Some(e), .. } => {
            remap_expr_into(e, map);
        }
        Statement::Let { expr: None, .. } => {}
        _ => {}
    }
}

/// Recursively remap identifiers in an expression.
fn remap_expr_into(e: &mut Expr, map: &HashMap<String, String>) {
    match e {
        Expr::Identifier(name) => {
            if let Some(field) = map.get(name) {
                *name = field.clone();
            }
        }
        Expr::Call(_, args, _) => {
            for arg in args.iter_mut() {
                remap_expr_into(arg, map);
            }
        }
        Expr::PluginIntercept { args, .. } => {
            for arg in args.iter_mut() {
                remap_expr_into(arg, map);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            remap_expr_into(l, map);
            remap_expr_into(r, map);
        }
        Expr::UnaryOp(_, inner) | Expr::Cast(inner, _) | Expr::IsType(inner, _) => {
            remap_expr_into(inner, map);
        }
        Expr::Field(target, _) | Expr::Index(target, _) => {
            remap_expr_into(target, map);
        }
        Expr::Block(stmts) => {
            for s in stmts.iter_mut() {
                remap_stmt_identifiers(s, map);
            }
        }
        Expr::If(cond, then_b, else_b) => {
            remap_expr_into(cond, map);
            remap_expr_into(then_b, map);
            if let Some(eb) = else_b {
                remap_expr_into(eb, map);
            }
        }
        Expr::Tuple(elems) | Expr::List(elems) => {
            for e in elems.iter_mut() {
                remap_expr_into(e, map);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_fields(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_hoist_simple_field_print() {
        let fields = state_fields(&["count", "N", "result"]);
        // node n [count < N][count == N] { count = count + 1; when count == N { term! -> PrintLn!(result); }; }
        let body = vec![
            Statement::Assign(
                Expr::Identifier("count".into()),
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Add,
                    Box::new(Expr::Identifier("count".into())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Guarded(
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Eq,
                    Box::new(Expr::Identifier("count".into())),
                    Box::new(Expr::Identifier("N".into())),
                ),
                vec![Statement::EndProgram(Some(Expr::Call(
                    "PrintLn!".into(),
                    vec![Expr::Identifier("result".into())],
                    None,
                )))],
            ),
            Statement::Term(None),
        ];
        let (stripped, hoist) = hoist_swan_song(&body, &fields);
        assert_eq!(stripped.len(), 1, "terminating guard must be hoisted");
        assert_eq!(hoist.len(), 1, "exactly one swan song hoisted");
        let song = &hoist[0];
        assert_eq!(song.len(), 1);
        assert!(matches!(song[0], Statement::Expression(_)));
    }

    #[test]
    fn test_hoist_let_to_field_remap() {
        // mandelbrot pattern: `escapes = nesc` binds the let to a state field,
        // so the hoisted swan song `PrintLn!(nesc)` is remapped to `escapes`.
        let fields = state_fields(&["count", "N", "escapes"]);
        let body = vec![
            Statement::Assign(
                Expr::Identifier("escapes".into()),
                Expr::Identifier("nesc".into()),
            ),
            Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::EndProgram(Some(Expr::Call(
                    "PrintLn!".into(),
                    vec![Expr::Identifier("nesc".into())],
                    None,
                )))],
            ),
        ];
        let (_, hoist) = hoist_swan_song(&body, &fields);
        let song = &hoist[0];
        assert_eq!(song.len(), 1);
        let Statement::Expression(Expr::Call(_, args, _)) = &song[0] else {
            panic!("hoisted swan song should be an Expression call");
        };
        // nesc must be remapped to the state field escapes.
        assert!(matches!(&args[0], Expr::Identifier(n) if n == "escapes"),
            "let-to-field remap failed: {:?}", args[0]);
    }

    #[test]
    fn test_let_to_field_remap_requires_state_field() {
        // When the field assigned from is NOT a state field, no remap happens.
        let fields = state_fields(&["count", "N"]);
        let body = vec![
            Statement::Assign(
                Expr::Identifier("local_tmp".into()),
                Expr::Identifier("nesc".into()),
            ),
            Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::EndProgram(Some(Expr::Call(
                    "PrintLn!".into(),
                    vec![Expr::Identifier("nesc".into())],
                    None,
                )))],
            ),
        ];
        let (_, hoist) = hoist_swan_song(&body, &fields);
        let song = &hoist[0];
        let Statement::Expression(Expr::Call(_, args, _)) = &song[0] else {
            panic!("hoisted swan song should be an Expression call");
        };
        assert!(matches!(&args[0], Expr::Identifier(n) if n == "nesc"),
            "non-state-field assignment must not remap: {:?}", args[0]);
    }

    #[test]
    fn test_hoist_empty_guard_body_prints_only() {
        let fields = state_fields(&["count", "N"]);
        // term! -> print_int#(result) with no preceding statements in guard.
        let body = vec![Statement::Guarded(
            Expr::Bool(true),
            vec![Statement::EndProgram(Some(Expr::Call(
                "PrintInt#".into(),
                vec![Expr::Decimal(7)],
                None,
            )))],
        )];
        let (stripped, hoist) = hoist_swan_song(&body, &fields);
        assert!(stripped.is_empty(), "guard body is the only statement");
        assert_eq!(hoist.len(), 1, "swan song hoisted even with empty body");
    }

    #[test]
    fn test_non_terminating_trailing_guard_not_hoisted() {
        let fields = state_fields(&["count", "N"]);
        let body = vec![Statement::Guarded(
            Expr::BinaryOp(
                crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Identifier("N".into())),
            ),
            vec![Statement::Assign(
                Expr::Identifier("count".into()),
                Expr::Decimal(0),
            )],
        )];
        let (stripped, hoist) = hoist_swan_song(&body, &fields);
        assert_eq!(stripped.len(), 1, "non-terminating guard stays in body");
        assert!(hoist.is_empty());
    }

    #[test]
    fn test_has_swan_song_detection() {
        let plain = vec![Statement::Assign(
            Expr::Identifier("x".into()),
            Expr::Decimal(1),
        )];
        assert!(!has_swan_song(&plain));

        // A plain `term 1` is not a swan song — it is discarded, not hoisted.
        let with_term = vec![Statement::Term(Some(Expr::Decimal(1)))];
        assert!(!has_swan_song(&with_term));

        // A trailing guard containing term! IS hoistable.
        let guarded = vec![Statement::Guarded(
            Expr::BinaryOp(
                crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Identifier("N".into())),
            ),
            vec![Statement::EndProgram(Some(Expr::Call(
                "PrintLn!".into(),
                vec![Expr::Identifier("result".into())],
                None,
            )))],
        )];
        assert!(has_swan_song(&guarded));

        // A trailing guard WITHOUT term! is not hoistable.
        let plain_guard = vec![Statement::Guarded(
            Expr::Bool(true),
            vec![Statement::Assign(
                Expr::Identifier("count".into()),
                Expr::Decimal(0),
            )],
        )];
        assert!(!has_swan_song(&plain_guard));
    }
}
