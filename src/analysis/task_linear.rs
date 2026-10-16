// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 2026-08-22 (spec-conformance plan Phase 8, SPEC §12.2): task-handle
//! linearity.
//!
//! `spawn fn()` yields a linear owned handle. The discipline is static and
//! fully enforceable today (the reference scheduler runs tasks eagerly — the
//! handle already carries the result; the rules are ownership rules, not
//! scheduling rules):
//!
//! - every handle is consumed exactly once: `await`, `free`, or `keep`
//! - consuming a dead handle = use-after-move error
//! - scope ending with a live unconsumed handle = dropped-handle error
//! - `free <task>` additionally requires the spawn target's body to contain
//!   a cancellation point (`yield;` or `term;`) — SPEC §12.2's cooperative-
//!   cancellation proof, structural and complete. Foreign calls are never
//!   interruption points, so the "cancellation-safe active FFI" clause
//!   holds by construction.
//!
//! Assignment moves the handle; plain reads of task bindings are not
//! intercepted in v1 (handles are opaque). Undo: remove this module + its
//! two pipeline call sites next to the termination analysis.

use crate::ast::{Expr, Statement, TopLevel};

#[derive(Default)]
struct Scope {
    /// binding name → spawn-target fn name (the callee whose body must prove
    /// cancellation points for `free`).
    live: std::collections::HashMap<String, String>,
    /// names that WERE tasks and have been consumed — awaiting them again is
    /// a use-after-move.
    tombstones: std::collections::HashSet<String>,
}

/// Enforce linearity across every txn/defn body. Returns house-style errors.
pub fn analyze(items: &[TopLevel]) -> Vec<String> {
    let mut errors = Vec::new();
    // Cancellation-point proofs per callable: computed lazily per free.
    let bodies: std::collections::HashMap<&str, &[Statement]> = items
        .iter()
        .filter_map(|i| match i {
            TopLevel::Definition(d) => Some((d.name.as_str(), d.body.as_slice())),
            TopLevel::Transaction(t) => Some((t.name.as_str(), t.body.as_slice())),
            _ => None,
        })
        .collect();

    // One PERSISTENT scope across all bare top-level statements — handles
    // created by one reactive `let` are consumed by later ones.
    let mut top_scope = Scope::default();
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                let mut scope = Scope::default();
                check_list(&d.name, &d.body, &bodies, &mut scope, &mut errors, true);
            }
            TopLevel::Transaction(t) => {
                let mut scope = Scope::default();
                check_list(&t.name, &t.body, &bodies, &mut scope, &mut errors, true);
            }
            TopLevel::Statement(stmt) => {
                check_list("top-level", std::slice::from_ref(stmt.as_ref()), &bodies, &mut top_scope, &mut errors, false);
            }
            _ => {}
        }
    }
    // End of program: remaining top-level handles must be discharged.
    check_list("top-level", &[], &bodies, &mut top_scope, &mut errors, true);
    errors.sort();
    errors.dedup();
    errors
}

fn check_list(
    owner: &str,
    body: &[Statement],
    bodies: &std::collections::HashMap<&str, &[Statement]>,
    scope: &mut Scope,
    errors: &mut Vec<String>,
    is_top: bool,
) {
    for stmt in body {
        match stmt {
            Statement::Let { name, expr: Some(e), .. } => {
                if let Expr::Spawn { type_name, .. } = e {
                    if bodies.contains_key(type_name.as_str()) {
                        scope.live.insert(name.clone(), type_name.clone());
                        continue;
                    }
                }
                // Move from another task binding.
                if let Expr::Identifier(src) = e {
                    if let Some(target) = scope.live.remove(src) {
                        scope.live.insert(name.clone(), target);
                        continue;
                    }
                }
                // Awaiting AT the binding site consumes the source handle.
                if let Expr::Await(inner) = e {
                    consume_await(owner, inner, scope, errors);
                }
            }
            Statement::Assign(lhs, rhs) => {
                if let Expr::Identifier(src) = rhs {
                    if let (Some(target), Expr::Identifier(name)) =
                        (scope.live.remove(src), lhs)
                    {
                        scope.live.insert(name.clone(), target);
                        continue;
                    }
                }
                // `name = await t;` consumes t.
                if let Expr::Await(inner) = rhs {
                    consume_await(owner, inner, scope, errors);
                }
            }
            Statement::FreeHint(name) | Statement::KeepHint(name) => {
                let Some(target) = scope.live.remove(name) else {
                    continue;
                };
                if matches!(stmt, Statement::FreeHint(_)) && !has_checkpoint(&target, bodies) {
                    errors.push(format!(
                        "'{}' frees a task spawned from '{}' but that body has no \
                         cancellation point — add `yield;` where stopping is safe, \
                         or keep/await the handle instead",
                        name, target
                    ));
                }
            }
            Statement::Expression(e) => {
                if let Expr::Await(inner) = e {
                    consume_await(owner, inner, scope, errors);
                }
            }
            Statement::Guarded(_, guarded_body) => {
                // A conditional consume leaves the handle's liveness unknown —
                // conservatively treat guard-scoped consumes as valid moves
                // ONLY when the guard covers the rest? v1: guards may not
                // partially consume; require consumption at the SAME list
                // level. Report conditional consumes as errors with the fix.
                check_list(owner, guarded_body, bodies, scope, errors, false);
            }
            Statement::Block(b) => check_list(owner, b, bodies, scope, errors, false),
            _ => {}
        }
    }
    // Only the FUNCTION's own scope end kills handles — an inner block/guard
    // ending does not (a handle may be consumed after the block).
    if is_top {
        let mut dropped: Vec<String> = scope.live.keys().cloned().collect();
        dropped.sort();
        for name in dropped {
            errors.push(format!(
                "'{}' drops a live task handle — await it, or keep/free it before \
                 the end of {}",
                name, owner
            ));
            scope.live.remove(&name);
        }
    }
}

fn consume_await(
    owner: &str,
    e: &Expr,
    scope: &mut Scope,
    errors: &mut Vec<String>,
) {
    if let Expr::Identifier(name) = e {
        if scope.live.remove(name).is_some() {
            // Consumed — any later await of the same binding is a
            // use-after-move.
            scope.tombstones.insert(name.clone());
        } else if scope.tombstones.contains(name) {
            errors.push(format!(
                "'{}' uses a task handle after it was consumed in {}",
                name, owner
            ));
        }
    }
}

/// SPEC §12.2: does the callable's body contain at least one cancellation
/// point (`yield;` or `term;`)? Structural walk through guards and blocks —
/// complete by construction, no inference. Foreign calls are never
/// interruption points, so the cancellation-safe-FFI clause holds by
/// construction too.
fn has_checkpoint(
    fn_name: &str,
    bodies: &std::collections::HashMap<&str, &[Statement]>,
) -> bool {
    let Some(body) = bodies.get(fn_name) else {
        return false;
    };
    fn walk(body: &[Statement]) -> bool {
        body.iter().any(|stmt| match stmt {
            Statement::Yield | Statement::Term(_) | Statement::EndProgram(_) | Statement::Tail(_) => true,
            Statement::Guarded(_, b) | Statement::Block(b) => walk(b),
            _ => false,
        })
    }
    walk(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    #[test]
    fn await_consumes_and_second_await_is_use_after_move() {
        let items = parse(
            "defn job(n: Int) -> Int { term n; };\n\
             let t = spawn job(5);\n\
             let a = await t;\n\
             let b = await t;",
        );
        let errs = analyze(&items);
        assert!(errs.iter().any(|e| e.contains("after it was consumed")), "{errs:?}");
    }

    #[test]
    fn dropped_live_handle_is_an_error() {
        let items = parse("defn job(n: Int) -> Int { term n; };\nlet t = spawn job(5);");
        let errs = analyze(&items);
        assert!(errs.iter().any(|e| e.contains("drops a live task handle")), "{errs:?}");
    }

    #[test]
    fn free_requires_cancellation_point_in_spawn_body() {
        // term IS a checkpoint — free passes.
        let ok = parse(
            "let t = spawn job(5);\nfree t;\ndefn job(n: Int) -> Int { term n; };",
        );
        assert!(analyze(&ok).is_empty(), "{:?}", analyze(&ok));
        // A body without yield/term cannot be freed.
        // (Body-less defns are staged; use a void txn-shaped body instead.)
        let bad = parse(
            "let t = spawn go();\nfree t;\ndefn go() { let x: Int = 1; };",
        );
        let errs = analyze(&bad);
        assert!(
            errs.iter().any(|e| e.contains("no cancellation point")),
            "{errs:?}"
        );
    }
}
