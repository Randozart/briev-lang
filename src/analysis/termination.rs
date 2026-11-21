// ── Term Termination Diagnostics ─────────────────────────────────────
//
// 2026-08-04 (term-termination-diagnostics): missing-compiler-error closure.
// The interpreter is the reference (AGENTS.md Rule 4). Reference semantics
// (src/interpreter/eval.rs):
//
//   - `term <val>` / `term! <val>`  → Err(TermReturn(val)): the whole
//     transaction unwinds (mod.rs:128-139 breaks; Guarded uses `?`), so every
//     later sibling statement in the same list never runs.
//   - bare `term;` / `term!;`        → **post-gated**: `lower_bare_terms`
//     (`interpreter/eval.rs:2485`) rewrites it at registration into
//     `when P { term <void>; }`, so it exits when the postcondition holds and
//     continues when it does not. A vacuous post makes it unconditional.
//     2026-11-21: this comment previously said bare `term;` always continues,
//     which read the un-lowered arm and cost a wrong BUGS.md entry.
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
/// 2026-11-09 (H5 Leg B): HIR-first surface for the pipeline/call-site flips.
/// The AST result stays the fallback and the debug oracle; under the flag a
/// fresh HIR is lowered at this exact point (the analyses run before the
/// plugin stages, so the backend HIR cannot be reused yet).
pub fn analyze_dual(items: &[TopLevel]) -> (Vec<String>, Vec<String>) {
    let ast = analyze(items);
    if !crate::backend::hir_emit_enabled() {
        return ast;
    }
    let program = crate::hir::lower_program(items);
    let hir = crate::hir::termination::analyze(&program);
    debug_assert_eq!(hir, ast, "HIR/AST termination drift");
    hir
}

/// Which kind of body the walker is inside — it decides what a bare `term;`
/// means. A defn is linear: its `term` is a post-gated exit, unconditional when
/// the post is vacuous. A node/txn is cyclic: its `term` is a convergence
/// checkpoint that legitimately continues until the post holds.
#[derive(Clone, Copy)]
enum BodyKind {
    Defn { post_vacuous: bool },
    Cyclic,
}

pub fn analyze(items: &[TopLevel]) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for item in items {
        match item {
            TopLevel::Transaction(t) => check_list(
                &t.name,
                &t.body,
                BodyKind::Cyclic,
                &mut errors,
                &mut warnings,
            ),
            TopLevel::Definition(d) => check_list(
                &d.name,
                &d.body,
                BodyKind::Defn {
                    post_vacuous: post_is_vacuous(d),
                },
                &mut errors,
                &mut warnings,
            ),
            TopLevel::Statement(stmt) => {
                check_list(
                    "top-level",
                    std::slice::from_ref(stmt.as_ref()),
                    BodyKind::Cyclic,
                    &mut errors,
                    &mut warnings,
                );
            }
            _ => {}
        }
    }
    (errors, warnings)
}

/// Does this statement unconditionally terminate its enclosing list, per the
/// interpreter? Straight-line flow reaches an always-terminating statement no
/// matter what, so it makes every later sibling unreachable.
pub(crate) fn statement_always_terminates(s: &Statement) -> bool {
    match s {
        Statement::Term(Some(_))
        | Statement::EndProgram(Some(_))
        | Statement::Tail(_)
        | Statement::Trap
        | Statement::Halt => true,
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

// ── Linear-exit totality (2026-11-21, wave-8) ─────────────────────────
//
// The owner's rule: "If any valid input would not lead to a valid term/output,
// compilation fails." For a defn that returns a value, that means every path
// through the body must reach an exit that PRODUCES the value. This is a
// different question from `statement_always_terminates` above, which asks
// whether a later sibling is unreachable — a node's bare `term;` may legitimately
// continue (its gate is unmet), while a defn's bare `term;` exits whenever the
// defn's postcondition is vacuous, because `lower_bare_terms` makes it
// unconditional in that case (`interpreter/eval.rs:2485`).

/// Does this statement list guarantee the enclosing defn produced its value?
///
/// Public so the typechecker can ask the same question when it records a
/// guard's negation as a fact: a guard body that unconditionally exits makes
/// `!cond` true for the code after it. `post_vacuous` decides whether a bare
/// `term;` is such an exit (see `statement_ends_defn`).
pub fn stmts_end_defn(stmts: &[Statement], post_vacuous: bool) -> bool {
    list_ends_defn(stmts, post_vacuous)
}

/// Errors for every non-void defn whose body can fall off the end.
pub fn defn_totality_errors(items: &[TopLevel]) -> Vec<String> {
    let mut errors = Vec::new();
    for item in items {
        let TopLevel::Definition(d) = item else {
            continue;
        };
        // An EMPTY body is a body-less signature (a staged declaration — the
        // variadic specializer leaves one behind, and `export defn` without a
        // body is the same shape). There is no path to be incomplete.
        if d.body.is_empty() || returns_nothing(d) || list_ends_defn(&d.body, post_is_vacuous(d)) {
            continue;
        }
        errors.push(format!(
            "`{}`: this defn returns a value, but a path through the body reaches the \
             end without producing one. Every path out of a value-returning defn must \
             end in the result — a trailing expression, `term <value>`, `endprogram`, \
             or `trap`. Add the return the missing path needs, or give the defn no \
             return type if it produces nothing.",
            d.name
        ));
    }
    errors
}

/// Does this defn produce no value (`-> Void`, or no return type at all)?
///
/// Falling off the end of such a body is the implicit return: there is exactly
/// one value of the type and it is the absence of one.
fn returns_nothing(d: &crate::ast::Definition) -> bool {
    match &d.output_type {
        None => true,
        Some(crate::ast::OutputType::Single(ty)) => *ty == crate::ast::Type::Void,
        _ => false,
    }
}

/// Is the defn's postcondition vacuous, so that a bare `term;` is an
/// unconditional exit rather than a gated one?
///
/// A contract that was never written defaults to `[true][true]`
/// (`Contract::explicit == false`), and `post_authority` means the author took
/// the post on trust instead of proof — either way nothing gates the term at
/// runtime, because `lower_bare_terms` folds a `Bool(true)` post into an
/// unconditional return.
fn post_is_vacuous(d: &crate::ast::Definition) -> bool {
    !d.contract.explicit
        || d.contract.post_authority
        || matches!(d.contract.post_condition, crate::ast::Expr::Bool(true))
}

/// Does this statement list guarantee the defn produced its value?
fn list_ends_defn(stmts: &[Statement], post_vacuous: bool) -> bool {
    stmts
        .iter()
        .any(|stmt| statement_ends_defn(stmt, post_vacuous))
}

/// Does this statement end the enclosing defn with a produced value (or a
/// deliberate close of the program)?
fn statement_ends_defn(stmt: &Statement, post_vacuous: bool) -> bool {
    match stmt {
        Statement::Term(Some(_)) | Statement::EndProgram(_) | Statement::Trap | Statement::Halt => {
            true
        }
        // A tail expression is the return, but its own shape can still be
        // incomplete: `match x { 0 => 1, _ => { let y: Int = 2; } }` as a tail
        // produces a value on one arm and nothing on the other.
        Statement::Tail(expr) => tail_ends_defn(expr, post_vacuous),
        // A defn's bare `term;` produces no value; it only exits when the gate
        // is vacuous.
        Statement::Term(None) => post_vacuous,
        Statement::Block(body) => list_ends_defn(body, post_vacuous),
        Statement::Match { arms, .. } => arms
            .iter()
            .all(|arm| list_ends_defn(&arm.body, post_vacuous)),
        // A statement-position `match` reaches here as a bare expression when
        // the arm bodies are blocks (`match kind { "Add" => { term "+"; }; … }`).
        // It is an exit only if every arm is; any OTHER expression statement is
        // discarded, so the body continues past it.
        Statement::Expression(expr @ (crate::ast::Expr::Match(..) | crate::ast::Expr::Block(_))) => {
            tail_ends_defn(expr, post_vacuous)
        }
        _ => false,
    }
}

/// Does this TAIL expression produce the defn's value?
///
/// A leaf expression is the value. A block is judged by its statements — a
/// block whose last statement is a bare `let` yields nothing. A match is judged
/// by its arms, which is how `match x { 0 => 1, _ => { let y: Int = 2; } }`
/// fails: the second arm produces no value even though it is an arm.
fn tail_ends_defn(expr: &crate::ast::Expr, post_vacuous: bool) -> bool {
    match expr {
        crate::ast::Expr::Block(stmts) => list_ends_defn(stmts, post_vacuous),
        crate::ast::Expr::Match(_, arms) => arms
            .iter()
            .all(|arm| tail_ends_defn(&arm.body, post_vacuous)),
        _ => true,
    }
}

/// Everything the termination family reports: unreachable code, the bare-term
/// guard hint, and the linear-exit totality rule.
pub fn analyze_all(items: &[TopLevel]) -> (Vec<String>, Vec<String>) {
    let (mut errors, warnings) = analyze_dual(items);
    errors.extend(defn_totality_errors(items));
    (errors, warnings)
}

/// Walk one statement list in order. After an always-terminating statement,
/// every later sibling is unreachable — report the first and stop. Recurse
/// into nested lists to catch the same rule inside guards, branches, blocks,
/// loops, sync blocks, and match arms.
fn check_list(
    name: &str,
    stmts: &[Statement],
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
        // 2026-08-04: hint for the bare-term-guard confusion — a `when` guard
        // ending in bare `term;` with more code after it. The author expected
        // early exit; the language continues. Only warn when code actually
        // follows the guard (a trailing checkpoint is harmless).
        if let Statement::Guarded(_, body) = stmt {
            let ends_in_void_term = matches!(
                body.last(),
                Some(Statement::Term(None)) | Some(Statement::EndProgram(None))
            );
            // 2026-11-21 (wave-8): the hint is about a GATED term. In a defn
            // whose postcondition is vacuous the term always exits, so there is
            // nothing to warn about; the old text taught node semantics to defn
            // authors and cited `term!`, which no longer exists.
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

/// Recurse into nested statement lists owned by this statement.
fn recurse(
    name: &str,
    stmt: &Statement,
    kind: BodyKind,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    match stmt {
        Statement::Guarded(_, body) => check_list(name, body, kind, errors, warnings),
        Statement::Block(body) => check_list(name, body, kind, errors, warnings),
        Statement::Foreach { body, .. } => check_list(name, body, kind, errors, warnings),
        Statement::SyncBlock(body) => check_list(name, body, kind, errors, warnings),
        Statement::Match { arms, .. } => {
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
            contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: Vec::new(),
            span: None,
            doc: None,
        })
    }

    use crate::ast::Expr;

    /// Parse a source string into top-level items (no imports, no typecheck).
    fn parse_items(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("lex");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    /// Totality errors for a source string.
    fn totality(src: &str) -> Vec<String> {
        defn_totality_errors(&parse_items(src))
    }

    #[test]
    fn non_void_defn_fallthrough_is_error() {
        // The design's obligation: a value-returning defn must produce a value
        // on every path. Before this rule the compiler accepted a body that
        // simply ended.
        let errs = totality("defn f(x: Int) -> Int { let y: Int = x + 1; };");
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("`f`"), "names the defn: {}", errs[0]);
        assert!(
            errs[0].contains("returns a value"),
            "names the obligation: {}",
            errs[0]
        );
    }

    #[test]
    fn defn_exit_forms_are_accepted() {
        // Every documented way out of a value-returning body, one per case.
        for src in [
            "defn f(x: Int) -> Int { term x; };",
            "defn f(x: Int) -> Int { x }",
            "defn f(x: Int) -> Int { let y: Int = x; y }",
            "defn f(x: Int) -> Int { when x > 0 { term x; }; term 0; };",
            "defn f(x: Int) -> Int { endprogram 0; };",
            "defn f(x: Int) -> Int { trap; };",
            "defn f(x: Int) -> Int { { term x; } };",
        ] {
            assert!(totality(src).is_empty(), "should be total: {src}");
        }
    }

    #[test]
    fn defn_match_all_arms_exit_is_total() {
        // A statement-position match is an exit when every arm is one, whether
        // the arms are blocks or bare values.
        for src in [
            "defn f(x: Int) -> Int { match x { 0 => { term 1; }, _ => { term 2; } } };",
            "defn f(x: Int) -> Int { match x { 0 => 1, _ => 2 } };",
        ] {
            assert!(totality(src).is_empty(), "should be total: {src}");
        }
    }

    #[test]
    fn defn_match_one_fallthrough_arm_is_error() {
        // One arm that only assigns leaves a path with no produced value.
        let errs = totality(
            "defn f(x: Int) -> Int { match x { 0 => { term 1; }, _ => { let y: Int = 2; } } };",
        );
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("`f`"), "{}", errs[0]);
    }

    #[test]
    fn void_defn_implicit_return_allowed() {
        // Falling off the end of a `-> Void` body IS the return: the type has
        // exactly one value and it is the absence of one.
        for src in [
            "defn f(x: Int) -> Void { Print#(x); };",
            "defn f(x: Int) { Print#(x); };",
        ] {
            assert!(totality(src).is_empty(), "should be exempt: {src}");
        }
    }

    #[test]
    fn bodyless_signature_is_not_flagged() {
        // An empty body is a staged declaration (the variadic specializer and
        // `export defn` both leave one behind), not an incomplete path.
        assert!(totality("export defn f(x: Int) -> Int;").is_empty());
    }

    #[test]
    fn trailing_expression_with_semicolon_is_not_a_return() {
        // SPEC §11.3: the return is a trailing expression WITHOUT `;`. With the
        // semicolon it is an expression statement, so the body falls off its
        // end — the emitter used to return it anyway, which hid the hole.
        let errs = totality("defn f(a: Int, b: Int) -> Int { a + b; };");
        assert_eq!(errs.len(), 1, "{errs:?}");
    }

    fn expr_call(name: &str) -> Expr {
        Expr::Call(name.to_string(), Vec::new(), None)
    }

    #[test]
    fn always_terminates_value_forms_only() {
        assert!(statement_always_terminates(&Statement::Term(Some(
            Expr::Decimal(1)
        ))));
        assert!(statement_always_terminates(&Statement::EndProgram(Some(
            Expr::Decimal(1)
        ))));
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
        let (errors, warnings) = analyze(&[txn(
            "n",
            vec![
                Statement::Expression(expr_call("work")),
                Statement::EndProgram(Some(expr_call("Print#"))),
                Statement::Expression(expr_call("dead")),
            ],
        )]);
        assert_eq!(
            errors.len(),
            1,
            "dead statement must be flagged: {errors:?}"
        );
        assert!(errors[0].contains("unreachable"));
        assert!(errors[0].contains("n"), "error must name the node");
        assert!(warnings.is_empty());
    }

    #[test]
    fn unreachable_inside_guard_body() {
        let (errors, _) = analyze(&[txn(
            "n",
            vec![Statement::Guarded(
                Expr::Bool(true),
                vec![
                    Statement::Expression(expr_call("print")),
                    Statement::EndProgram(Some(expr_call("Print#"))),
                    Statement::Expression(expr_call("dead")),
                ],
            )],
        )]);
        assert_eq!(
            errors.len(),
            1,
            "sibling after term! inside a guard is unreachable: {errors:?}"
        );
    }

    #[test]
    fn bare_term_guard_hint_when_code_follows() {
        let (errors, warnings) = analyze(&[txn(
            "n",
            vec![
                Statement::Guarded(
                    Expr::Bool(true),
                    vec![
                        Statement::Expression(expr_call("print")),
                        Statement::Term(None),
                    ],
                ),
                Statement::Expression(expr_call("more")),
            ],
        )]);
        assert!(
            errors.is_empty(),
            "bare term; continues — nothing is unreachable"
        );
        assert_eq!(
            warnings.len(),
            1,
            "the confusion needs a hint: {warnings:?}"
        );
        assert!(warnings[0].contains("checkpoint"));
    }

    #[test]
    fn trailing_bare_term_guard_no_hint() {
        let (errors, warnings) = analyze(&[txn(
            "n",
            vec![Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::Term(None)],
            )],
        )]);
        assert!(errors.is_empty());
        assert!(warnings.is_empty(), "trailing checkpoint guard is harmless");
    }

    // 2026-11-19 (wave-4 D): pin the G2 diagnostic's exact condition — it
    // fires when a guard-final bare `term;` has statements after it in the
    // SAME body (the author may have meant an exit), and stays silent when
    // the guard is block-final (a convergence checkpoint, harmless). The
    // census gate (tests/g2_census_gate.rs) pins the corpus counts; these
    // two pin the diagnostic semantics the census classifies against.
    #[test]
    fn g2_checkpoint_with_following_statement_warns() {
        let (errors, warnings) = analyze(&[txn(
            "n",
            vec![
                Statement::Guarded(Expr::Bool(true), vec![Statement::Term(None)]),
                Statement::Assign(
                    Expr::Identifier("x".into()),
                    Expr::Decimal(1),
                ),
            ],
        )]);
        assert!(errors.is_empty());
        assert_eq!(
            warnings.len(),
            1,
            "a guard-final bare `term;` with a following statement must warn: {warnings:?}"
        );
        assert!(
            warnings[0].contains("convergence checkpoint"),
            "the warning should explain the checkpoint semantics: {warnings:?}"
        );
    }

    #[test]
    fn g2_block_final_checkpoint_is_silent() {
        let (errors, warnings) = analyze(&[txn(
            "n",
            vec![Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::Term(None)],
            )],
        )]);
        assert!(errors.is_empty());
        assert!(
            warnings.is_empty(),
            "a block-final guard checkpoint is harmless and must not warn: {warnings:?}"
        );
    }

    #[test]
    fn valid_swan_song_passes() {
        let (errors, warnings) = analyze(&[txn(
            "n",
            vec![Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::EndProgram(Some(expr_call("PrintLn#")))],
            )],
        )]);
        assert!(errors.is_empty());
        assert!(
            warnings.is_empty(),
            "the canonical swan song must not warn: {warnings:?}"
        );
    }
}
