// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// The body prover lives in the `briev-verify` crate so it can be used without
// the compiler. Its tests stay here because they parse Briev source, which needs
// the parser this workspace's compiler crate owns.

pub use briev_verify::verify::*;

/// The prover's tests construct AST nodes directly.
#[cfg(test)]
use crate::ast::TopLevel;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TopLevel;

    /// Parse one defn through the real frontend so the AST shapes stay
    /// honest (no hand-rolled Definition construction to drift).
    fn parse_defn(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items
            .into_iter()
            .find(|i| matches!(i, TopLevel::Definition(_)))
            .expect("expected a Definition top-level")
    }

    #[test]
    fn correct_body_holds() {
        let d = parse_defn("defn f(a: Int) [a >= 0][term == a + 1] -> Int { term a + 1; };");
        let r = verify_bodies(&[d]);
        assert!(r.violated.is_empty(), "violated: {:?}", r.violated);
        assert!(r.unproven.is_empty(), "unproven: {:?}", r.unproven);
        assert_eq!(r.proven.len(), 1);
    }

    #[test]
    fn json_report_splits_names_and_reasons() {
        let d = parse_defn("defn f(a: Int) [a >= 0][term == 0] -> Int { term 5; };");
        let r = verify_bodies(&[d]);
        let json = r.to_json();
        // `proven` lists names, not a count: a bare total cannot tell "proved"
        // apart from "never examined", and consumers need the difference.
        let proven = json["proven"].as_array().expect("proven array");
        assert!(proven.is_empty(), "a violated body is not proven: {json}");
        let violated = json["violated"].as_array().expect("violated array");
        assert_eq!(violated.len(), 1);
        assert_eq!(violated[0]["name"], "f");
        assert!(
            violated[0]["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "{json}"
        );
        // A finding without the "name: reason" separator still yields a name.
        let bare = findings_json(&["solo".to_string()]);
        assert_eq!(bare[0]["name"], "solo");
        assert_eq!(bare[0]["reason"], "");
    }

    #[test]
    fn lying_body_is_violated() {
        let d = parse_defn("defn f(a: Int) [a >= 0][term == 0] -> Int { term 5; };");
        let r = verify_bodies(&[d]);
        assert_eq!(r.violated.len(), 1, "violated: {:?}", r.violated);
    }

    #[test]
    fn path_split_both_hold() {
        // Guarded: a > 5 path returns a (bounds prove a >= 0); fall-through
        // path returns the literal 0. Both must hold.
        let d = parse_defn(
            "defn f(a: Int) [a >= 0][term >= 0] -> Int { when a > 5 { term a; }; term 0; };",
        );
        let r = verify_bodies(&[d]);
        assert!(r.violated.is_empty(), "violated: {:?}", r.violated);
        assert!(r.unproven.is_empty(), "unproven: {:?}", r.unproven);
    }

    #[test]
    fn ge_holds_via_lower_bound() {
        // `term >= 0` with term = a and only the fact `a >= 0`: the interval
        // lower bound must prove it (this was undecided before bounds).
        let d = parse_defn("defn f(a: Int) [a >= 0][term >= 0] -> Int { term a; };");
        let r = verify_bodies(&[d]);
        assert!(r.violated.is_empty(), "violated: {:?}", r.violated);
        assert!(r.unproven.is_empty(), "unproven: {:?}", r.unproven);
    }

    #[test]
    fn unprovable_is_warning_never_error() {
        // The postcondition `term == f(a)` is outside the decidable surface;
        // the body itself is supported. Must be advisory, never a violation.
        let d = parse_defn("defn f(a: Int) [true][term == f(a)] -> Int { term a; };");
        let r = verify_bodies(&[d]);
        assert!(r.violated.is_empty(), "violated: {:?}", r.violated);
        assert_eq!(r.unproven.len(), 1, "unproven: {:?}", r.unproven);
    }

    #[test]
    fn unsupported_body_is_skipped() {
        // A call in the BODY puts it outside the prover's surface: skipped
        // silently — never proven, never flagged.
        let d = parse_defn("defn f(a: Int) [a >= 0][term >= 0] -> Int { term helper(a); };");
        let r = verify_bodies(&[d]);
        assert!(r.violated.is_empty() && r.unproven.is_empty() && r.proven.is_empty());
    }
}

#[cfg(test)]
mod point_bound_tests {
    use super::*;
    use crate::ast::TopLevel;

    /// Parse one defn through the real frontend.
    fn parse_defn(src: &str) -> TopLevel {
        let (items, _universe) = crate::library::parse_and_check("<test>", src).expect("parse");
        items
            .into_iter()
            .find(|item| matches!(item, TopLevel::Definition(_)))
            .expect("definition")
    }

    /// `a == 5` is a value fact, so `a + a` folds and the goal is decided.
    #[test]
    fn point_bound_decides_arithmetic() {
        let d = parse_defn("defn f(a: Int) [a == 5][term == a + a] -> Int { term a + a; };");
        let report = verify_bodies(&[d]);
        assert!(
            report.violated.is_empty(),
            "violated: {:?}",
            report.violated
        );
        assert!(
            report.unproven.is_empty(),
            "unproven: {:?}",
            report.unproven
        );
        assert_eq!(report.proven.len(), 1);
    }

    /// Pinned to 5 the body yields 10: `2 * a` holds, `11` does not.
    #[test]
    fn point_bound_proves_and_refutes() {
        let ok = parse_defn("defn f(a: Int) [a == 5][term == 2 * a] -> Int { term a + a; };");
        assert!(verify_bodies(&[ok]).violated.is_empty());
        let bad = parse_defn("defn f(a: Int) [a == 5][term == 11] -> Int { term a + a; };");
        assert_eq!(verify_bodies(&[bad]).violated.len(), 1);
    }

    /// A range is not a value: without a solver, arithmetic over it stays
    /// undecided. With a solver (C4) the identity is discharged instead —
    /// either way the verdict must never be a refutation.
    #[test]
    fn range_bound_does_not_pin() {
        // `term == 2 * a` is NOT structurally equal to the body's `a + a`, so
        // the symbolic engine alone could never decide it.
        let d = parse_defn("defn f(a: Int) [a >= 0][term == 2 * a] -> Int { term a + a; };");
        let report = verify_bodies(&[d]);
        assert!(
            report.violated.is_empty(),
            "a true identity must never be refuted: {:?}",
            report.violated
        );
        if crate::proof_engine::smt::is_z3_available() {
            assert_eq!(
                report.proven.len(),
                1,
                "the solver should discharge a+a == 2*a: unproven {:?}",
                report.unproven
            );
        } else {
            assert_eq!(
                report.proven.len(),
                0,
                "a range must not fabricate a concrete value"
            );
            assert_eq!(report.unproven.len(), 1, "unproven: {:?}", report.unproven);
        }
    }

    /// C4: the solver decides what pure symbolic tracking cannot.
    #[test]
    fn smt_discharges_arithmetic_identity() {
        if !crate::proof_engine::smt::is_z3_available() {
            return; // No solver installed: `unproven` is the honest answer.
        }
        let d = parse_defn("defn f(a: Int) [a >= 0][term == 3 * a] -> Int { term a + a + a; };");
        let report = verify_bodies(&[d]);
        assert_eq!(report.proven.len(), 1, "unproven: {:?}", report.unproven);
        assert!(report.violated.is_empty());
    }

    /// C4: a parameter-dependent refutation stays advisory (the stdlib relies
    /// on optimistic contracts like `[true][term >= 0]`).
    #[test]
    fn smt_parameter_dependent_refutation_is_advisory() {
        if !crate::proof_engine::smt::is_z3_available() {
            return;
        }
        let d = parse_defn("defn f(a: Int) [a >= 0][term >= 0] -> Int { term 0 - a; };");
        let report = verify_bodies(&[d]);
        assert!(
            report.violated.is_empty(),
            "input-dependent failures must not fail a build: {:?}",
            report.violated
        );
    }

    /// Parse every top-level item — the interprocedural tests need several.
    fn parse_program(src: &str) -> Vec<TopLevel> {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items
    }

    /// C3: a caller is proved through the callee's value summary.
    #[test]
    fn summary_proves_through_a_call() {
        let items = parse_program(
            "defn inc(a: Int) [a >= 0][term == a + 1] -> Int { term a + 1; }\n\
             defn uses(a: Int) [a >= 0][term == a + 2] -> Int { term inc(a) + 1; }",
        );
        let report = verify_bodies(&items);
        assert!(
            report.violated.is_empty(),
            "violated: {:?}",
            report.violated
        );
        assert!(
            report.unproven.is_empty(),
            "unproven: {:?}",
            report.unproven
        );
        assert_eq!(report.proven.len(), 2, "both should be proved");
    }

    /// C3: summaries close over callees, so a chain proves end to end.
    #[test]
    fn summary_chain_closes_to_a_fixpoint() {
        let items = parse_program(
            "defn base(a: Int) [a >= 0][term == a + 1] -> Int { term a + 1; }\n\
             defn mid(a: Int) [a >= 0][term == a + 2] -> Int { term base(a) + 1; }\n\
             defn top(a: Int) [a >= 0][term == a + 3] -> Int { term mid(a) + 1; }",
        );
        let report = verify_bodies(&items);
        assert_eq!(
            report.proven.len(),
            3,
            "proven {:?} unproven {:?}",
            report.proven,
            report.unproven
        );
    }

    /// C3: substituting a callee's value still refutes a lying caller.
    #[test]
    fn summary_refutes_a_lying_caller() {
        let items = parse_program(
            "defn five(a: Int) [a >= 0][term == 5] -> Int { term 5; }\n\
             defn lies(a: Int) [a >= 0][term == 0] -> Int { term five(a); }",
        );
        let report = verify_bodies(&items);
        assert_eq!(
            report.violated.len(),
            1,
            "proven {:?} unproven {:?}",
            report.proven,
            report.unproven
        );
        assert!(report.violated[0].contains("lies"));
    }

    /// C3: recursion never earns a summary — skipped, never guessed at.
    #[test]
    fn recursion_never_earns_a_summary() {
        let items = parse_program(
            "defn ping(a: Int) [a >= 0][term == a] -> Int { term pong(a); }\n\
             defn pong(a: Int) [a >= 0][term == a] -> Int { term ping(a); }",
        );
        let report = verify_bodies(&items);
        assert!(report.proven.is_empty(), "proven: {:?}", report.proven);
        assert!(
            report.violated.is_empty(),
            "violated: {:?}",
            report.violated
        );
    }
}

#[cfg(test)]
mod cycle_tests {
    use super::*;

    /// Parse one defn through the real frontend.
    fn parse_defn(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items
            .into_iter()
            .find(|item| matches!(item, TopLevel::Definition(_)))
            .expect("expected a Definition")
    }

    /// A self-referential assignment must still produce a verification condition.
    ///
    /// Before the cycle guard, `acc = acc + 1` bound `acc -> acc + 1`, and
    /// substitution followed that chain to the depth cap and returned `None` —
    /// so `substitute_all` bailed and **no VC was ever built**. Every
    /// accumulating body was therefore undecidable, silently.
    #[test]
    fn self_referential_assignment_is_decided_not_skipped() {
        let d = parse_defn(
            "defn f(n: Int, acc: Int) [n >= 0 && n <= 1000 && acc == n][acc == n] -> Int {\n\
                 acc = acc + 1;\n\
                 acc = acc - 1;\n\
                 term 0;\n\
             };",
        );
        let report = verify_bodies(&[d]);
        assert!(
            report.violated.is_empty(),
            "violated: {:?}",
            report.violated
        );
        assert_eq!(
            report.proven.len(),
            1,
            "an accumulating body must be DECIDED, not skipped: unproven {:?}",
            report.unproven
        );
    }
}

#[cfg(test)]
mod loop_invariant_tests {
    use super::*;

    /// Parse one `defn` through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A `foreach` carrying an invariant is proved by its three obligations.
    #[test]
    fn foreach_invariant_yields_three_obligations() {
        let defn = parse_one(
            "defn count(n: Int) [n >= 0 && n <= 1000][term >= 0] -> Int {\n\
                 let acc: Int = 0;\n\
                 foreach i [acc == i && i <= n] in 0..n { acc = acc + 1; };\n\
                 term acc;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        for obligation in ["count_init", "count_preserve", "count_exit"] {
            assert!(
                report.proven.iter().any(|name| name == obligation),
                "{obligation} not proven: unproven={:?} skipped={:?}",
                report.unproven,
                report.skipped
            );
        }
    }

    /// The obligations discriminate: a wrong invariant is refuted at entry
    /// rather than accepted because the other two happen to hold.
    #[test]
    fn a_wrong_foreach_invariant_is_refuted_at_entry() {
        let defn = parse_one(
            "defn bad(n: Int) [n >= 0 && n <= 1000][term >= 0] -> Int {\n\
                 let acc: Int = 0;\n\
                 foreach i [acc == i + 1 && i <= n] in 0..n { acc = acc + 1; };\n\
                 term acc;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(
            report.violated.iter().any(|line| line.starts_with("bad_init")),
            "entry must be refuted: {:?}",
            report.violated
        );
    }
}
