// ── Term Termination Diagnostics ─────────────────────────────────────
//
// 2026-08-04 (term-termination-diagnostics): missing-compiler-error closure.
// The interpreter is the reference (AGENTS.md Rule 4). Reference semantics
// (src/interpreter/eval.rs):
//
//   - `term <val>` / `term! <val>`  → Err(TermReturn(val)): the whole
//     transaction unwinds (mod.rs:128-139 breaks; Guarded uses `?`), so every
//     later sibling statement in the same list never runs.
//   - bare `term;` / `term!;`        → Ok(Void): a convergence checkpoint,
//     execution continues to the next statement.
//
// This pass emits a hard ERROR for unreachable statements after an
// always-terminating statement, and a WARNING for the bare-term-guard-ending
// confusion (the 2026-08-03 test-project-otto cascade: 404/422/409/200 all
// printed because bare `term;` really does continue).
//
// NOTE: `Guarded` is deliberately NOT always-terminating. `when c { term! ... }`
// exits only when `c` is true — later siblings are conditionally reachable, so
// flagging them would be a false positive. Only statements AFTER an
// unconditional terminator within the SAME statement list are unreachable.
// `return <val>` is also NOT always-terminating: the interpreter's `Return` is
// Ok (it continues the list), so flagging it would reject code the interpreter
// executes. The backend's `ret` there is a separate, tracked divergence.
//
// The rule is fully general: it does not special-case any program shape. A
// `term!` inside a guard is exactly as diagnosable as one at top level, and an
// `If` whose every branch always terminates is as much an unconditional exit
// as a bare `term <val>`.

use crate::ast::{Statement, TopLevel};

/// Detect unreachable statements and bare-term-guard confusions.
///
/// Returns `(errors, warnings)`. Errors fail both `brievc check` and
/// `brievc build`; warnings print to stderr.
pub fn analyze(items: &[TopLevel]) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for item in items {
        match item {
            TopLevel::Transaction(t) => check_list(&t.name, &t.body, &mut errors, &mut warnings),
            TopLevel::Definition(d) => check_list(&d.name, &d.body, &mut errors, &mut warnings),
            TopLevel::Statement(stmt) => {
                check_list("top-level", std::slice::from_ref(stmt.as_ref()), &mut errors, &mut warnings);
            }
            _ => {}
        }
    }
    (errors, warnings)
}

/// Does this statement unconditionally terminate its enclosing list, per the
/// interpreter? Straight-line flow reaches an always-terminating statement no
/// matter what, so it makes every later sibling unreachable.
fn statement_always_terminates(s: &Statement) -> bool {
    match s {
        Statement::Term(Some(_)) | Statement::EndProgram(Some(_)) => true,
        Statement::Block(body) => list_always_terminates(body),
        _ => false,
    }
}

/// A list always terminates when at least one of its statements does: earlier
/// statements complete normally (none of them always terminates), so the first
/// always-terminating one is always reached.
fn list_always_terminates(stmts: &[Statement]) -> bool {
    stmts.iter().any(statement_always_terminates)
}

/// Walk one statement list in order. After an always-terminating statement,
/// every later sibling is unreachable — report the first and stop. Recurse
/// into nested lists to catch the same rule inside guards, branches, blocks,
/// loops, sync blocks, and match arms.
fn check_list(name: &str, stmts: &[Statement], errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    let mut alive = true;
    for (i, stmt) in stmts.iter().enumerate() {
        if !alive {
            errors.push(format!(
                "{name}: unreachable code: this statement follows a `term <value>` / `term! <value>` \
                 that always terminates the transaction, so it can never run (interpreter reference). \
                 Remove it or move it before the term."
            ));
            return;
        }
        if statement_always_terminates(stmt) {
            alive = false;
        }
        recurse(name, stmt, errors, warnings);
        // 2026-08-04: hint for the bare-term-guard confusion — a `when` guard
        // ending in bare `term;` with more code after it. The author expected
        // early exit; the language continues. Only warn when code actually
        // follows the guard (a trailing checkpoint is harmless).
        if let Statement::Guarded(_, body) = stmt {
            let ends_in_void_term = matches!(
                body.last(),
                Some(Statement::Term(None)) | Some(Statement::EndProgram(None))
            );
            if ends_in_void_term && i + 1 < stmts.len() {
                warnings.push(format!(
                    "{name}: a `when` guard ending in bare `term;` is a convergence checkpoint — \
                     execution continues to the statement after the guard. To close the program here \
                     use `term! ->`, to return a value early use `term <value>`, or restructure so \
                     the postcondition excludes later guards."
                ));
            }
        }
    }
}

/// Recurse into nested statement lists owned by this statement.
fn recurse(name: &str, stmt: &Statement, errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    match stmt {
        Statement::Guarded(_, body) => check_list(name, body, errors, warnings),
        Statement::Block(body) => check_list(name, body, errors, warnings),
        Statement::Foreach { body, .. } => check_list(name, body, errors, warnings),
        Statement::SyncBlock(body) => check_list(name, body, errors, warnings),
        Statement::Match { arms, .. } => {
            for arm in arms {
                check_list(name, &arm.body, errors, warnings);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txn(name: &str, body: Vec<Statement>) -> TopLevel {
        TopLevel::Transaction(crate::ast::Transaction {
            variadic_param: None,
            name: name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: Vec::new(),
            parameters: Vec::new(),
            output_type: None,
            outputs: Vec::new(),
            contract: crate::ast::Contract::new(
                Expr::Bool(true),
                Expr::Bool(true),
            ),
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: Vec::new(),
            span: None,
            doc: None,
        })
    }

    use crate::ast::Expr;

    fn expr_call(name: &str) -> Expr {
        Expr::Call(name.to_string(), Vec::new(), None)
    }

    #[test]
    fn always_terminates_value_forms_only() {
        assert!(statement_always_terminates(&Statement::Term(Some(Expr::Decimal(1)))));
        assert!(statement_always_terminates(&Statement::EndProgram(Some(Expr::Decimal(1)))));
        assert!(!statement_always_terminates(&Statement::Term(None)));
        assert!(!statement_always_terminates(&Statement::EndProgram(None)));
        // A guard is conditional — NOT unconditional.
        assert!(!statement_always_terminates(&Statement::Guarded(
            Expr::Bool(true),
            vec![Statement::EndProgram(Some(Expr::Decimal(1)))]
        )));
    }

    #[test]
    fn always_terminates_blocks() {
        let term = Statement::EndProgram(Some(Expr::Decimal(1)));
        assert!(statement_always_terminates(&Statement::Block(vec![term])));
        assert!(!statement_always_terminates(&Statement::Block(Vec::new())));
    }

    #[test]
    fn unreachable_after_top_level_terminating_term() {
        let (errors, warnings) = analyze(&[txn("n", vec![
            Statement::Expression(expr_call("work")),
            Statement::EndProgram(Some(expr_call("Print#"))),
            Statement::Expression(expr_call("dead")),
        ])]);
        assert_eq!(errors.len(), 1, "dead statement must be flagged: {errors:?}");
        assert!(errors[0].contains("unreachable"));
        assert!(errors[0].contains("n"), "error must name the node");
        assert!(warnings.is_empty());
    }

    #[test]
    fn unreachable_inside_guard_body() {
        let (errors, _) = analyze(&[txn("n", vec![
            Statement::Guarded(
                Expr::Bool(true),
                vec![
                    Statement::Expression(expr_call("print")),
                    Statement::EndProgram(Some(expr_call("Print#"))),
                    Statement::Expression(expr_call("dead")),
                ],
            ),
        ])]);
        assert_eq!(errors.len(), 1, "sibling after term! inside a guard is unreachable: {errors:?}");
    }

    #[test]
    fn bare_term_guard_hint_when_code_follows() {
        let (errors, warnings) = analyze(&[txn("n", vec![
            Statement::Guarded(
                Expr::Bool(true),
                vec![
                    Statement::Expression(expr_call("print")),
                    Statement::Term(None),
                ],
            ),
            Statement::Expression(expr_call("more")),
        ])]);
        assert!(errors.is_empty(), "bare term; continues — nothing is unreachable");
        assert_eq!(warnings.len(), 1, "the confusion needs a hint: {warnings:?}");
        assert!(warnings[0].contains("checkpoint"));
    }

    #[test]
    fn trailing_bare_term_guard_no_hint() {
        let (errors, warnings) = analyze(&[txn("n", vec![
            Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::Term(None)],
            ),
        ])]);
        assert!(errors.is_empty());
        assert!(warnings.is_empty(), "trailing checkpoint guard is harmless");
    }

    #[test]
    fn valid_swan_song_passes() {
        let (errors, warnings) = analyze(&[txn("n", vec![
            Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::EndProgram(Some(expr_call("PrintLn#")))],
            ),
        ])]);
        assert!(errors.is_empty());
        assert!(warnings.is_empty(), "the canonical swan song must not warn: {warnings:?}");
    }
}
