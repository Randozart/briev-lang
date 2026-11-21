// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 inventory port: term-termination diagnostics over HIR.
//! Mirror of `analysis::termination` (AST). PORT-ONLY for now: the AST
//! pass's consumers (`compile.rs`, `pipeline.rs`) run BEFORE the backend
//! builds the HIR, so the flip needs pipeline-side HIR availability and is
//! deferred. Parity tests pin HIR == AST on the AST fixtures.
//!
//! Coverage mirrors the AST walker: BARE top-level transactions and
//! definitions only (raw items, not effective; exports and sync wrappers
//! excluded), and — same as the AST — recursion into Guarded/Block/Foreach/
//! SyncBlock/Match arms but NOT Mutex/Defer. Top-level non-let statements
//! cannot be mirrored (HIR carries only globals); they are inert for this
//! rule in practice (a top-level `term` is not valid Briev).

use super::expr::{HirExprKind, HirLiteral};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};

/// Mirror of the AST pass's `BodyKind`: what a bare `term;` means depends on
/// whether the body is linear (a defn) or cyclic (a node/txn), and — for a
/// defn — on whether its postcondition gates the term.
#[derive(Clone, Copy)]
enum BodyKind {
    Defn { post_vacuous: bool },
    Cyclic,
}

pub fn analyze(program: &HirProgram) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for c in program.callables.iter().filter(|c| {
        matches!(c.kind, HirCallableKind::Transaction | HirCallableKind::Defn)
            && c.owner.is_none()
            && !c.exported
            && c.sync_domains.is_empty()
    }) {
        let kind = match c.kind {
            HirCallableKind::Defn => BodyKind::Defn {
                post_vacuous: c
                    .contract
                    .as_ref()
                    .map(|contract| matches!(&contract.post.kind, HirExprKind::Literal(HirLiteral::Bool(true))))
                    .unwrap_or(true),
            },
            _ => BodyKind::Cyclic,
        };
        check_list(&c.name, &c.body.statements, kind, &mut errors, &mut warnings);
    }
    (errors, warnings)
}

/// Mirror of `analysis::termination::statement_always_terminates` over HIR.
pub(crate) fn statement_always_terminates(stmt: &HirStmt) -> bool {
    match &stmt.kind {
        HirStmtKind::Term(Some(_))
        | HirStmtKind::EndProgram(Some(_))
        | HirStmtKind::Tail(_)
        | HirStmtKind::Trap
        | HirStmtKind::Halt => true,
        HirStmtKind::Block(body) => list_always_terminates(body),
        _ => false,
    }
}

fn list_always_terminates(stmts: &[HirStmt]) -> bool {
    stmts.iter().any(statement_always_terminates)
}

fn check_list(
    name: &str,
    stmts: &[HirStmt],
    kind: BodyKind,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
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
        recurse(name, stmt, kind, errors, warnings);
        if let HirStmtKind::Guarded(_, body) = &stmt.kind {
            let ends_in_void_term = matches!(
                body.last().map(|s| &s.kind),
                Some(HirStmtKind::Term(None)) | Some(HirStmtKind::EndProgram(None))
            );
            let gated = match kind {
                BodyKind::Defn { post_vacuous } => !post_vacuous,
                BodyKind::Cyclic => true,
            };
            if ends_in_void_term && gated && i + 1 < stmts.len() {
                warnings.push(match kind {
                    BodyKind::Defn { .. } => format!(
                        "{name}: a `when` guard ending in bare `term;` exits only when the \
                         postcondition holds — with a non-vacuous post, execution continues to \
                         the statement after the guard. Return a value with `term <value>` if the \
                         early exit must be unconditional."
                    ),
                    BodyKind::Cyclic => format!(
                        "{name}: a `when` guard ending in bare `term;` is a convergence \
                         checkpoint — the node fires again rather than continuing, so the \
                         statement after the guard runs on the next pass only if the goal was \
                         unmet. Use `endprogram` to close the program here."
                    ),
                });
            }
        }
    }
}

fn recurse(
    name: &str,
    stmt: &HirStmt,
    kind: BodyKind,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    match &stmt.kind {
        HirStmtKind::Guarded(_, body)
        | HirStmtKind::Block(body)
        | HirStmtKind::Foreach { body, .. }
        | HirStmtKind::SyncBlock(body) => check_list(name, body, kind, errors, warnings),
        HirStmtKind::Match { arms, .. } => {
            for arm in arms {
                check_list(name, &arm.body, kind, errors, warnings);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_program;

    fn compare(src: &str) -> (Vec<String>, Vec<String>) {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let items = crate::parser::Parser::new(tokens, src)
            .parse_program()
            .unwrap();
        let ast = crate::analysis::termination::analyze(&items);
        let hir = analyze(&lower_program(&items));
        assert_eq!(hir, ast, "HIR/AST termination drift");
        hir
    }

    #[test]
    fn unreachable_after_value_term() {
        let (errors, warnings) =
            compare("txn n [true][true] { work(); endprogram 1; dead(); term; };");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("unreachable"));
        assert!(errors[0].contains("n"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn unreachable_inside_guard_body() {
        let (errors, _) =
            compare("txn n [true][true] { when true { print(); endprogram 1; dead(); }; term; };");
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn bare_term_guard_hint_and_trailing_guard() {
        let (errors, warnings) =
            compare("txn n [true][true] { when true { print(); term; }; more(); term; };");
        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("checkpoint"));
        let (errors, warnings) = compare("txn n [true][true] { when true { term; }; };");
        assert!(errors.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn valid_swan_song_passes() {
        let (errors, warnings) =
            compare("txn n [true][true] { when true { endprogram PrintLn#(1); }; term; };");
        assert!(errors.is_empty());
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
