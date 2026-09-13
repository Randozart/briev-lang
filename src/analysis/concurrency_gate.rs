// ── Concurrency Gate ──────────────────────────────────────────────────
// 2026-08-01 (Phase 3c): Frontend-computed (rule: frontend-driven dispatch)
// check that no pair of reactive txns can fire together implicitly.
//
// For every unordered pair of reactive txns (A, B):
//   1. sat = check_satisfiable(pre_A, pre_B)
//   2. xor_overlap = (A.writes ∩ (B.reads ∪ B.writes)) ≠ ∅ OR
//                    (B.writes ∩ A.reads) ≠ ∅
//   3. If !sat OR xor_overlap → safe without classification (mutually
//      exclusive, or sequential-by-dependency).
//   4. Else (eligible to fire together): the pair must be classified — both
//      `async` (explicit simultaneous firing) or both `sync<group>` (same
//      group barrier). Otherwise → hard compile error (rule #21: no implicit
//      concurrency).
//
// Generated entry/script nodes are NEVER async and NEVER sync<group>, so a
// generated node overlapping a user node with no XOR dependency is DENIED
// (the intended behavior). Two entry! nodes with mutually exclusive commands
// are UNSAT (check_satisfiable detects `cmd == "a"` vs `cmd == "b"`) → legal
// subcommand dispatch.

use crate::ast::{Expr, TopLevel};
use crate::backend::collect_assigned_identifiers;
use crate::backend::collect_read_identifiers;
use crate::proof_engine::check_satisfiable;
use std::collections::HashSet;

/// A reactive transaction and its classification context.
struct ReactiveTxn<'a> {
    name: &'a str,
    pre: &'a Expr,
    body: &'a [crate::ast::Statement],
    is_async: bool,
    /// sync<group> domains the node belongs to (empty = not group-classified).
    sync_groups: Vec<String>,
}

/// Run the gate over all reactive txns. Returns a list of error messages
/// (one per unclassified eligible pair), empty when the program is legal.
pub fn run_concurrency_gate(items: &[TopLevel]) -> Vec<String> {
    let txns = collect_reactive(items);
    let mut errors = Vec::new();
    for i in 0..txns.len() {
        for j in (i + 1)..txns.len() {
            let a = &txns[i];
            let b = &txns[j];
            if let Some(msg) = check_pair(a, b) {
                errors.push(msg);
            }
        }
    }
    errors
}

/// Collect all reactive transactions (direct + sync<group>-wrapped).
fn collect_reactive(items: &[TopLevel]) -> Vec<ReactiveTxn<'_>> {
    let mut out = Vec::new();
    for item in items {
        match item {
            TopLevel::Transaction(t) => {
                // 2026-09-14 (machine-entry plan): bootstrap nodes run once at
                // the machine's beginning — they are not reactor-dispatched,
                // so the pairwise eligibility gate does not apply.
                if t.is_reactive && !has_bootstrap_modifier(t) {
                    out.push(ReactiveTxn {
                        name: &t.name,
                        pre: &t.contract.pre_condition,
                        body: &t.body,
                        is_async: t.is_async,
                        sync_groups: vec![],
                    });
                }
            }
            TopLevel::SyncGroup { domains, item: inner } => {
                if let TopLevel::Transaction(t) = inner.as_ref() {
                    if t.is_reactive && !has_bootstrap_modifier(t) {
                        out.push(ReactiveTxn {
                            name: &t.name,
                            pre: &t.contract.pre_condition,
                            body: &t.body,
                            is_async: t.is_async,
                            sync_groups: domains.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// 2026-09-14 (machine-entry plan): the `bootstrap` modifier marks the
/// authored program entry (pre-reactor machine beginning).
fn has_bootstrap_modifier(t: &crate::ast::Transaction) -> bool {
    t.modifiers.iter().any(|m| m.name == "bootstrap")
}

/// Check one unordered pair. Returns Some(error) if the pair is eligible to
/// fire together but unclassified.
fn check_pair(a: &ReactiveTxn<'_>, b: &ReactiveTxn<'_>) -> Option<String> {
    let sat = check_satisfiable(a.pre, b.pre);
    if !sat {
        // Mutually exclusive preconditions → can never both fire.
        return None;
    }
    if xor_overlap(a, b) {
        // Read-write overlap → sequential-by-dependency (one depends on the
        // other's writes), no simultaneous firing.
        return None;
    }
    // Eligible to fire together — must be classified.
    let both_async = a.is_async && b.is_async;
    let same_group = a.sync_groups.iter().any(|g| b.sync_groups.contains(g));
    classify_eligible_pair(both_async, same_group, a, b)
}

/// 2026-08-09 (Phase 10, Slice D): the pure classification decision for an
/// ELIGIBLE pair (preconditions satisfiable + XOR overlap resolved). Extracted
/// so Kani can prove the gate is total and sound: an eligible pair is ACCEPTED
/// exactly when it is classified (both async or a shared sync group), and
/// REJECTED (an error) exactly when it is not — no unclassified eligible pair
/// ever reaches execution.
fn classify_eligible_pair(
    both_async: bool,
    same_group: bool,
    a: &ReactiveTxn<'_>,
    b: &ReactiveTxn<'_>,
) -> Option<String> {
    if both_async || same_group {
        return None;
    }
    Some(format!(
        "nodes '{}' and '{}' can fire together; declare 'async' on both or \
         'sync<group>' on both (no implicit concurrency)",
        a.name, b.name
    ))
}

/// XOR read-write overlap: A writes what B reads/writes, or B writes what A
/// reads. Uses the full body write/read sets (rule: frontend-computed).
fn xor_overlap(a: &ReactiveTxn<'_>, b: &ReactiveTxn<'_>) -> bool {
    let a_writes: HashSet<String> =
        collect_assigned_identifiers(a.body).into_iter().collect();
    let a_reads = collect_read_identifiers(a.body);
    let b_writes: HashSet<String> =
        collect_assigned_identifiers(b.body).into_iter().collect();
    let b_reads = collect_read_identifiers(b.body);

    !a_writes.is_disjoint(&b_reads)
        || !a_writes.is_disjoint(&b_writes)
        || !b_writes.is_disjoint(&a_reads)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Contract, Statement, Transaction};
    use crate::lexer::tokenize;
    use crate::parser::Parser;

    fn txn(name: &str, pre: &str, body: &[&str]) -> TopLevel {
        let body: Vec<Statement> = body
            .iter()
            .map(|s| {
                let tokens = tokenize(s).unwrap();
                let mut p = Parser::new(tokens, s);
                p.parse_statement().unwrap()
            })
            .collect();
        let pre_tokens = tokenize(pre).unwrap();
        let mut p = Parser::new(pre_tokens, pre);
        let pre_expr = p.parse_expression().unwrap();
        TopLevel::Transaction(Transaction {
            name: name.into(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: pre_expr,
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: true,
            post_authority: false},
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })
    }

    #[test]
    fn test_mutually_exclusive_commands_are_legal() {
        // Two entry!-shaped nodes with different commands: pre_A ∧ pre_B is
        // UNSAT (`cmd == "build"` vs `cmd == "run"`) → no classification
        // needed (legal subcommand dispatch).
        let items = vec![
            txn("build", r#"entry_cmd() == "build""#, &["term;"]),
            txn("run", r#"entry_cmd() == "run""#, &["term;"]),
        ];
        let errors = run_concurrency_gate(&items);
        assert!(
            errors.is_empty(),
            "mutually exclusive commands must be legal; got: {errors:?}"
        );
    }

    #[test]
    fn test_xor_overlap_is_legal_without_classification() {
        // A writes x, B reads x → sequential-by-dependency (no simultaneous
        // firing) → no classification needed.
        let items = vec![
            txn("produce", "true", &["x = 1;", "term;"]),
            txn("consume", "true", &["term x;"]),
        ];
        let errors = run_concurrency_gate(&items);
        assert!(
            errors.is_empty(),
            "read-write overlap must be legal without classification; got: {errors:?}"
        );
    }

    #[test]
    fn test_unclassified_eligible_pair_errors() {
        // Two disjoint-write nodes with `[true]` preconditions can fire
        // together and neither is async nor sync<group> → denied.
        let items = vec![
            txn("a", "true", &["x = 1;", "term;"]),
            txn("b", "true", &["y = 1;", "term;"]),
        ];
        let errors = run_concurrency_gate(&items);
        assert_eq!(errors.len(), 1, "unclassified eligible pair must error");
        assert!(
            errors[0].contains("can fire together"),
            "error must mention the pair; got: {}",
            errors[0]
        );
    }

    #[test]
    fn test_both_async_is_classified() {
        let mut a = txn("a", "true", &["x = 1;", "term;"]);
        let mut b = txn("b", "true", &["y = 1;", "term;"]);
        if let TopLevel::Transaction(t) = &mut a {
            t.is_async = true;
        }
        if let TopLevel::Transaction(t) = &mut b {
            t.is_async = true;
        }
        let errors = run_concurrency_gate(&[a, b]);
        assert!(
            errors.is_empty(),
            "both async must be classified; got: {errors:?}"
        );
    }
}

/// 2026-08-09 (Phase 10, Slice D): Kani proof of the concurrency gate's
/// classification decision. The gate is TOTAL and SOUND: for every eligible
/// pair, it accepts (None) exactly when the pair is classified (both async or
/// a shared sync group) and rejects (Some) exactly when it is not. Therefore
/// no unclassified eligible pair can reach execution — every program the
/// compiler accepts has every co-firable pair explicitly classified.
#[cfg(all(feature = "kani", feature = "kani_full"))]
mod kani_full_tests {
    use super::*;

    #[kani::proof]
    fn verify_classified_pair_is_accepted() {
        // For any booleans (any eligible pair's classification state): if the
        // pair is classified (both async OR same group), the gate returns None
        // (accepted) — a classified pair never errors.
        let both_async: bool = kani::any();
        let same_group: bool = kani::any();
        kani::assume(both_async || same_group);
        let a = ReactiveTxn {
            name: "a",
            pre: &Expr::Bool(true),
            body: &[],
            is_async: both_async,
            sync_groups: vec![],
        };
        let b = ReactiveTxn {
            name: "b",
            pre: &Expr::Bool(true),
            body: &[],
            is_async: both_async,
            sync_groups: vec![],
        };
        let result = classify_eligible_pair(both_async, same_group, &a, &b);
        assert!(result.is_none(), "a classified pair must be accepted");
    }

    #[kani::proof]
    fn verify_unclassified_pair_is_rejected() {
        // An eligible pair that is NEITHER both-async NOR same-group is
        // rejected — the error is produced, so the program does not compile.
        let a = ReactiveTxn {
            name: "a",
            pre: &Expr::Bool(true),
            body: &[],
            is_async: false,
            sync_groups: vec![],
        };
        let b = ReactiveTxn {
            name: "b",
            pre: &Expr::Bool(true),
            body: &[],
            is_async: false,
            sync_groups: vec![],
        };
        let result = classify_eligible_pair(false, false, &a, &b);
        assert!(result.is_some(), "an unclassified eligible pair must be rejected");
    }
}
