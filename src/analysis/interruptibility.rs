// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//!
//! Interruptibility of declarations that waived their termination obligation.
//!
//! SPEC 10.9: an `unbounded` declaration owes the property that every cycle in its
//! body reaches a `yield`, or that the loop is provably bounded. A node body is
//! already interruptible *between ticks* — the reactor is the scheduler — so the
//! hazard is an **inner** loop.
//!
//! The check is structural, never a timing analysis, and it lives here rather than
//! in the pipeline so it can be unit-tested directly. It reports; it inserts
//! nothing, which is why satisfying it costs nothing at runtime.

use crate::ast::{Expr, Statement};

/// Inner loops that cannot be shown to yield, in source order.
pub fn loops_without_yield(body: &[Statement]) -> Vec<String> {
    let mut out = Vec::new();
    scan(body, &mut out);
    out
}

/// Walk the statements, reporting each unyielding inner loop.
fn scan(stmts: &[Statement], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Statement::Foreach {
                list, body: inner, ..
            } => {
                // A literal range is bounded, so it returns to the scheduler by
                // construction and needs no `yield`.
                if !literal_range(list) && !reaches_yield(inner) {
                    out.push(
                        "an inner `foreach` over non-literal bounds reaches no `yield` — \
                         add one, or bound the range"
                            .to_string(),
                    );
                }
                scan(inner, out);
            }
            Statement::Guarded(_, inner) => scan(inner, out),
            _ => {}
        }
    }
}

/// Whether a loop's bounds are both integer literals, and so bounded.
fn literal_range(list: &Expr) -> bool {
    let Expr::Range { start, end, .. } = list else {
        return false;
    };
    matches!(start.as_ref(), Expr::Decimal(_)) && matches!(end.as_ref(), Expr::Decimal(_))
}

/// Whether a statement list reaches a `yield` at any depth.
fn reaches_yield(stmts: &[Statement]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Statement::Yield => true,
        Statement::Guarded(_, inner) => reaches_yield(inner),
        Statement::Foreach { body, .. } => reaches_yield(body),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `foreach` over a dynamic range with no `yield` is reported.
    #[test]
    fn dynamic_range_without_yield_is_reported() {
        let body = vec![Statement::Foreach {
            item: "i".into(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Identifier("n".into())),
                inclusive: false,
            }),
            body: vec![Statement::Term(None)],
            modifiers: Vec::new(),
        }];
        assert_eq!(loops_without_yield(&body).len(), 1);
    }

    /// The same loop with a `yield` is accepted.
    #[test]
    fn dynamic_range_with_yield_is_accepted() {
        let body = vec![Statement::Foreach {
            item: "i".into(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Identifier("n".into())),
                inclusive: false,
            }),
            body: vec![Statement::Yield, Statement::Term(None)],
            modifiers: Vec::new(),
        }];
        assert!(loops_without_yield(&body).is_empty());
    }

    /// A literal range is bounded, so it needs no `yield`.
    #[test]
    fn literal_range_needs_no_yield() {
        let body = vec![Statement::Foreach {
            item: "i".into(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Decimal(8)),
                inclusive: false,
            }),
            body: vec![Statement::Term(None)],
            modifiers: Vec::new(),
        }];
        assert!(loops_without_yield(&body).is_empty());
    }

    /// A guarded `yield` counts: reachability is the requirement, not execution.
    #[test]
    fn a_guarded_yield_counts() {
        let body = vec![Statement::Foreach {
            item: "i".into(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Identifier("n".into())),
                inclusive: false,
            }),
            body: vec![Statement::Guarded(
                Expr::Identifier("due".into()),
                vec![Statement::Yield],
            )],
            modifiers: Vec::new(),
        }];
        assert!(
            loops_without_yield(&body).is_empty(),
            "a conditional yield still gives the scheduler an opportunity"
        );
    }
}
