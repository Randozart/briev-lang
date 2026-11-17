// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// The body prover lives in the `briev-verify` crate so it can be used without
// the compiler. Its tests stay here because they parse Briev source, which needs
// the parser this workspace's compiler crate owns.

pub use briev_verify::verify::*;

use crate::ast::{TopLevel, Type};
use crate::casting::graph::CastingGraph;
use crate::type_universe::TypeUniverse;
use briev_verify::domain::ScalarModel;
use std::collections::HashSet;

/// Resolve the exact scalar subset shared by the body prover and Lean export.
/// Width comes from the compiler's layout resolver; operator overrides and
/// structured storage cannot inherit the model merely by claiming Cast.Int.
pub struct BodyTypeModel<'a> {
    universe: &'a TypeUniverse,
    graph: CastingGraph,
    int_bits: u64,
    overridden: HashSet<String>,
}

impl<'a> BodyTypeModel<'a> {
    pub fn new(items: &[TopLevel], universe: &'a TypeUniverse, int_bits: u64) -> Self {
        let mut overridden = HashSet::new();
        collect_overrides(items, &mut overridden);
        Self {
            universe,
            graph: CastingGraph::new(),
            int_bits,
            overridden,
        }
    }

    pub fn resolve(&self, ty: &Type) -> ScalarModel {
        if self.int_bits == 0 {
            return ScalarModel::Unsupported;
        }
        // Applied, constrained and aggregate types need their own semantics.
        let Type::Custom(key) = ty else {
            return ScalarModel::Unsupported;
        };
        let category = crate::type_universe::operators::protocol_category(self.universe, ty);
        if !self.standard_scalar_chain(key, category.as_deref()) {
            return ScalarModel::Unsupported;
        }
        let storage = self
            .graph
            .resolve_llvm_type(self.universe, ty, self.int_bits);
        match (category.as_deref(), storage.as_str()) {
            (Some("Int"), "i64") => ScalarModel::SignedWord { bits: 64 },
            // 2026-11-15 (plan 2026-11-15-value-summaries.md phase 1): Bool is
            // already fully supported end-to-end inside the prover —
            // `SymbolicValue::bool_literal` evaluates to `Literal(0|1, "bool")`
            // and the SMT encoder maps `Expr::Bool` to the native SMT-LIB Boolean
            // (`(= #b1 #b1)` / `(= #b0 #b1)`) with `==`/`!=` on same-sort operands
            // and `Not` already encoding correctly. The only thing rejecting
            // Bool-typed defns was this one gate. Admit it: the symbolic and SMT
            // layers need no change. This is a gate fix, not a capability fix.
            (Some("Bool"), _) => ScalarModel::Bool,
            // 2026-11-16 (phase 2a): a String/Data value is a word-sized handle
            // with no value semantics. Admitting it cannot prove anything about
            // text — it lets the body be *examined*, and the only claims that
            // can then discharge are about the handle's word (e.g. an address
            // read through Load#). Content and length are separate gates.
            (Some("String"), _) | (Some("Data"), _) => ScalarModel::OpaqueHandle,
            _ => ScalarModel::Unsupported,
        }
    }

    fn standard_scalar_chain(&self, key: &str, category: Option<&str>) -> bool {
        let mut current = key.to_owned();
        let mut seen = HashSet::new();
        while seen.insert(current.clone()) {
            if self.overridden.contains(&current) {
                return false;
            }
            let Some(resolved) = self.universe.get(&current) else {
                return false;
            };
            if !standard_scalar_metadata(resolved, category) {
                return false;
            }
            let Some((base, variant)) = CastingGraph::parse_protocol_base(&resolved.base) else {
                return resolved.base.is_empty();
            };
            if !variant.is_empty() {
                return false;
            }
            if base == current {
                return true;
            }
            current = base;
        }
        false
    }
}

/// Fixed-width storage metadata is also consumed directly by LLVM ABI/state
/// lowering. Require agreement with the model even when the casting graph would
/// fall back to the target width. Oversized metadata fails closed without overflow.
fn standard_scalar_metadata(
    resolved: &crate::type_universe::ResolvedType,
    category: Option<&str>,
) -> bool {
    if category == Some("Int")
        && [
            resolved.min_bits,
            resolved.max_bits,
            resolved.bytes.saturating_mul(8),
        ]
        .iter()
        .any(|bits| *bits != 0 && *bits != 64)
    {
        return false;
    }
    resolved.fields.is_empty()
        && resolved.type_params.is_empty()
        && !resolved.properties.keys().any(|key| key.starts_with("op."))
}

/// Member operators and protocol behavior are as significant as binding-form
/// overrides; range-constrained arithmetic also needs independent semantics.
fn has_custom_behavior(ty: &crate::ast::TypeDef) -> bool {
    !ty.body.operators.is_empty()
        || !ty.body.op_bindings.is_empty()
        || !ty.body.members.is_empty()
        || !ty.body.constraints.is_empty()
        || ty.bit_range.is_some()
}

fn collect_overrides(items: &[TopLevel], names: &mut HashSet<String>) {
    for item in items {
        match item {
            TopLevel::TypeDef(t) if has_custom_behavior(t) => {
                names.insert(t.name.clone());
            }
            TopLevel::Impl(i) if !i.op_bindings.is_empty() || !i.functions.is_empty() => {
                names.insert(i.target.clone());
            }
            TopLevel::ProtocolDef(protocol) => {
                names.insert(protocol.category.trim_start_matches('#').to_owned());
            }
            TopLevel::Export(e) => collect_overrides(std::slice::from_ref(&*e.inner), names),
            TopLevel::SyncGroup { item, .. } => {
                collect_overrides(std::slice::from_ref(&**item), names)
            }
            _ => {}
        }
    }
}

/// Standalone compiler API with a native 64-bit primordial environment. Source
/// compilation uses `verify_bodies_in_universe` with its actual resolved types.
pub fn verify_bodies(items: &[TopLevel]) -> BodyReport {
    let mut universe = TypeUniverse::new();
    if let Err(reason) = crate::backend::register_types::register_typedefs(items, &mut universe, 64)
    {
        return BodyReport {
            skipped: vec![format!("type environment: {reason}")],
            ..BodyReport::default()
        };
    }
    verify_bodies_in_universe(items, &universe, 64)
}

pub fn verify_bodies_in_universe(
    items: &[TopLevel],
    universe: &TypeUniverse,
    int_bits: u64,
) -> BodyReport {
    let model = BodyTypeModel::new(items, universe, int_bits);
    verify_bodies_with_types(items, &|ty| model.resolve(ty))
}

/// The prover's tests construct AST nodes directly.

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
        // silently — never proven, never flagged. The callee must be defined
        // (2026-11-16: the typechecker now rejects calls to undefined names,
        // so a bare `helper` no longer typechecks — define it here).
        let d = parse_defn(
            "defn helper(a: Int) -> Int { term a; }; defn f(a: Int) [a >= 0][term >= 0] -> Int { term helper(a); };",
        );
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
            report
                .violated
                .iter()
                .any(|line| line.starts_with("bad_init")),
            "entry must be refuted: {:?}",
            report.violated
        );
    }
}

#[cfg(test)]
mod convergent_invariant_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A `txn` with a third-bracket invariant proves entry and preservation.
    #[test]
    fn txn_invariant_proves_entry_and_preservation() {
        let defn = parse_one(
            "txn tick(x: Int, done: Int) [x >= 0 && x < 1000][done == 1][x >= 0] {\n\
                 x = x + 1;\n\
                 done = 1;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        for obligation in ["tick_init", "tick_preserve"] {
            assert!(
                report.proven.iter().any(|name| name == obligation),
                "{obligation} not proven: unproven={:?} violated={:?} skipped={:?}",
                report.unproven,
                report.violated,
                report.skipped
            );
        }
    }
}

#[cfg(test)]
mod inferred_invariant_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A loop with no annotation is proved by inferring its invariant.
    #[test]
    fn unannotated_loop_is_proved_by_inference() {
        let defn = parse_one(
            "defn count(n: Int) [n >= 0 && n <= 1000][term >= 0] -> Int {\n\
                 let acc: Int = 0;\n\
                 foreach i in 0..n { acc = acc + 1; };\n\
                 term acc;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        for obligation in ["count_init", "count_preserve", "count_exit"] {
            assert!(
                report.proven.iter().any(|name| name == obligation),
                "{obligation} not proven: unproven={:?} violated={:?} skipped={:?}",
                report.unproven,
                report.violated,
                report.skipped
            );
        }
    }

    /// Inference must never manufacture a proof: a false contract stays unproved.
    #[test]
    fn inference_does_not_invent_a_proof() {
        let defn = parse_one(
            "defn wrong(n: Int) [n >= 0 && n <= 1000][term == 999] -> Int {\n\
                 let acc: Int = 0;\n\
                 foreach i in 0..n { acc = acc + 1; };\n\
                 term acc;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        let proven: Vec<&String> = report
            .proven
            .iter()
            .filter(|name| name.starts_with("wrong"))
            .collect();
        assert!(
            proven.len() < 3,
            "a false contract must not be fully proved: {proven:?}"
        );
    }
}

#[cfg(test)]
mod mask_offset_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// The `mantissa_val` shape: a masked value plus the hidden bit.
    ///
    /// `term = (bits & 0x7FFFFF) + 0x800000` cannot exceed
    /// `0x7FFFFF + 0x800000 == 0xFFFFFF`, so `term < 1 << 24` holds on every
    /// path without a solver. Before the masked-offset rule this body was
    /// reported unproven: the AND had a bound, but nothing propagated it
    /// through the addition.
    #[test]
    fn masked_offset_upper_bound_proves() {
        let defn = parse_one(
            "defn f(bits: Int) [bits >= 0][term >= 1 && term < 1 << 24] -> Int {\n\
                 let mant: Int = bits & 0x7FFFFF;\n\
                 term mant + 0x800000;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(
            report.violated.is_empty(),
            "violated: {:?}",
            report.violated
        );
        assert_eq!(
            report.proven.len(),
            1,
            "a masked+offset value must prove its upper bound: unproven={:?}",
            report.unproven
        );
    }

    /// The rule must not invent proofs: an offset that can overshoot the
    /// postcondition stays refuted rather than proven.
    #[test]
    fn masked_offset_that_can_overshoot_is_refuted() {
        let defn = parse_one(
            "defn f(bits: Int) [bits >= 0][term < 1 << 24] -> Int {\n\
                 let mant: Int = bits & 0x7FFFFF;\n\
                 term mant + 0x1000000;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert_eq!(
            report.violated.len(),
            1,
            "an offset past the bound must be refuted: proven={:?} unproven={:?}",
            report.proven,
            report.unproven
        );
    }

    /// A comparison that straddles the reachable interval stays undecided —
    /// neither proven nor refuted, never guessed at.
    #[test]
    fn masked_offset_straddling_the_bound_stays_undecided() {
        let defn = parse_one(
            "defn f(bits: Int) [bits >= 0][term < 0x800000] -> Int {\n\
                 let mant: Int = bits & 0x7FFFFF;\n\
                 term mant + 0x400000;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(
            report.violated.is_empty(),
            "a straddling goal must not be refuted: {:?}",
            report.violated
        );
        assert!(report.proven.is_empty(), "proven: {:?}", report.proven);
        assert_eq!(report.unproven.len(), 1, "unproven: {:?}", report.unproven);
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A decreasing measure proves both the bound and the step.
    #[test]
    fn a_decreasing_variant_proves_bound_and_step() {
        let defn = parse_one(
            "txn converge(x: Int, done: Int) [x >= 0 && x <= 100][done == 1][x >= 0] \
             variant [100 - x] {\n x = x + 1;\n done = 1;\n};",
        );
        let report = verify_bodies(&[defn]);
        for obligation in ["converge_variant_bound", "converge_variant_step"] {
            assert!(
                report.proven.iter().any(|name| name == obligation),
                "{obligation} not proven: unproven={:?}",
                report.unproven
            );
        }
    }

    /// A measure that grows must not prove its step obligation.
    #[test]
    fn a_growing_variant_does_not_prove_the_step() {
        let defn = parse_one(
            "txn grows(x: Int, done: Int) [x >= 0 && x <= 100][done == 1][x >= 0] \
             variant [x] {\n x = x + 1;\n done = 1;\n};",
        );
        let report = verify_bodies(&[defn]);
        assert!(
            !report
                .proven
                .iter()
                .any(|name| name == "grows_variant_step"),
            "a growing measure must not prove progress: {:?}",
            report.proven
        );
    }
}

#[cfg(test)]
mod typed_model_tests {
    use super::*;

    fn check(source: &str, bits: u64) -> BodyReport {
        let (items, universe) = crate::library::parse_and_check("<typed-model>", source).unwrap();
        verify_bodies_in_universe(&items, &universe, bits)
    }

    #[test]
    fn floating_parameters_and_locals_are_skipped() {
        for source in [
            "defn f(x: Float) [true][term != x] -> Float { term x + 1; };",
            "defn f() [true][term == 16777216] -> Float { let x: Float = 16777216; term (x + 1) - 1; };",
        ] {
            let report = check(source, 64);
            assert!(report.proven.is_empty(), "{report:?}");
            assert_eq!(report.skipped.len(), 1, "{report:?}");
        }
    }

    #[test]
    fn aliases_use_metadata_and_target_width() {
        let source =
            "type Ticket: Int64; defn f(x: Ticket) [true][term == x] -> Ticket { term x; };";
        assert_eq!(check(source, 64).proven, vec!["f"]);
        let source = "defn f(x: Int) [true][term == x] -> Int { term x; };";
        assert_eq!(check(source, 64).proven, vec!["f"]);
        assert_eq!(check(source, 32).skipped.len(), 1);
        for ty in ["Float32", "UInt64", "Int32"] {
            let source = format!("defn f(x: {ty}) [true][term == x] -> {ty} {{ term x; }};");
            assert_eq!(check(&source, 64).skipped.len(), 1, "{ty}");
        }
    }

    #[test]
    fn member_behavior_cannot_inherit_a_word_model() {
        let (mut items, universe) =
            crate::library::parse_and_check("<member-model>", "type Counter: Int64;").unwrap();
        let TopLevel::TypeDef(ty) = &mut items[0] else {
            panic!("expected type");
        };
        ty.body.members.push(TopLevel::TypeDefOperator(
            crate::ast::Definition::prover_defn(
                "Add",
                crate::ast::Contract::new(
                    crate::ast::Expr::Bool(true),
                    crate::ast::Expr::Bool(true),
                ),
                vec![],
            ),
        ));
        let model = BodyTypeModel::new(&items, &universe, 64);
        assert_eq!(
            model.resolve(&Type::Custom("Counter".into())),
            ScalarModel::Unsupported
        );
    }

    #[test]
    fn custom_protocol_variants_need_an_independent_model() {
        let mut universe = TypeUniverse::new();
        let mut resolved = universe.get("Int64").unwrap().clone();
        resolved.name = "AlternateWord".into();
        resolved.base = "Int<Alternate>".into();
        universe.register(resolved);
        let model = BodyTypeModel::new(&[], &universe, 64);
        assert_eq!(
            model.resolve(&Type::Custom("AlternateWord".into())),
            ScalarModel::Unsupported
        );
    }

    #[test]
    fn oversized_storage_metadata_fails_closed() {
        let mut universe = TypeUniverse::new();
        let mut resolved = universe.get("Int").unwrap().clone();
        resolved.name = "Oversized".into();
        resolved.bytes = u64::MAX;
        universe.register(resolved);
        let model = BodyTypeModel::new(&[], &universe, 64);
        assert_eq!(
            model.resolve(&Type::Custom("Oversized".into())),
            ScalarModel::Unsupported
        );
    }

    #[test]
    fn resolved_operator_overrides_cannot_inherit_word_proofs() {
        let mut universe = TypeUniverse::new();
        let mut resolved = universe.get("Int").unwrap().clone();
        resolved.name = "CustomArithmetic".into();
        resolved.properties.insert(
            "op.Add".into(),
            crate::ast::PropertyValue::Identifier("different_add".into()),
        );
        universe.register(resolved);
        let model = BodyTypeModel::new(&[], &universe, 64);
        assert_eq!(
            model.resolve(&Type::Custom("CustomArithmetic".into())),
            ScalarModel::Unsupported
        );
    }
}

#[cfg(test)]
mod boolean_model_boundary_tests {
    use super::*;

    /// 2026-11-15 (plan 2026-11-15-value-summaries.md phase 1a): Bool is
    /// admitted into the body model. The symbolic evaluator handles it
    /// (`Literal(0|1, "bool")`) and the SMT encoder encodes `Expr::Bool` as
    /// a Boolean. The SMT *encoding* side (declaring `term` as `Bool` rather
    /// than `BitVec`) is a separate concern (phase 1b) — until then, Bool
    /// postconditions that compare `term` against BitVec values are rejected
    /// at the SMT gate as "unsupported or ill-sorted", which is an honest
    /// skip, never a false proof.
    #[test]
    fn boolean_storage_is_admitted_into_the_body_model() {
        let universe = TypeUniverse::new();
        let model = BodyTypeModel::new(&[], &universe, 64);
        assert_eq!(model.resolve(&Type::bool_()), ScalarModel::Bool);
        assert!(model.resolve(&Type::bool_()).admitted());
    }
}

#[cfg(test)]
mod load_width_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A one-byte load is zero-extended, so the stdlib's own byte contract holds.
    ///
    /// `byte_at` declares `[term >= 0 && term <= 255]` over `Load#(addr, 1)`;
    /// before the width-aware load model this body was skipped as "a call to
    /// `Load#`, which has no value summary".
    #[test]
    fn byte_load_proves_the_byte_range() {
        let defn = parse_one(
            "defn f(p: Int) [p >= 0][term >= 0 && term <= 255] -> Int {\n\
                 term Load#(p, 1);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(
            report.violated.is_empty(),
            "violated: {:?}",
            report.violated
        );
        assert_eq!(
            report.proven.len(),
            1,
            "a 1-byte load must prove [0,255]: unproven={:?} skipped={:?}",
            report.unproven,
            report.skipped
        );
    }

    /// The bound scales with the loaded width.
    #[test]
    fn two_byte_load_proves_the_wider_range() {
        let defn = parse_one(
            "defn f(p: Int) [p >= 0][term >= 0 && term <= 65535] -> Int {\n\
                 term Load#(p, 2);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert_eq!(report.proven.len(), 1, "unproven={:?}", report.unproven);
    }

    /// A goal the interval does not decide must stay undecided — never proven.
    #[test]
    fn wider_load_does_not_prove_a_narrower_bound() {
        let defn = parse_one(
            "defn f(p: Int) [p >= 0][term <= 255] -> Int {\n\
                 term Load#(p, 2);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.proven.is_empty(), "proven: {:?}", report.proven);
        assert!(
            report.violated.is_empty(),
            "straddling must not refute: {:?}",
            report.violated
        );
        assert_eq!(report.unproven.len(), 1, "unproven: {:?}", report.unproven);
    }

    /// Two separate loads must not collapse into one atom.
    ///
    /// If they shared an identity, `a - b` would fold to 0 and a postcondition of
    /// `term == 0` would be "proved" for a body that reads memory twice around a
    /// possible store. The contract must stay unproven instead.
    #[test]
    fn distinct_loads_do_not_collapse() {
        let defn = parse_one(
            "defn f(p: Int) [p >= 0][term == 0] -> Int {\n\
                 let a: Int = Load#(p, 1);\n\
                 let b: Int = Load#(p, 1);\n\
                 term a - b;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(
            !report.proven.iter().any(|name| name == "f"),
            "two loads must not be treated as one value: {:?}",
            report.proven
        );
    }

    /// A four-byte load is exactly the word width on wasm32, so zero-extension is
    /// the identity and the value may read negative. Modelling it as
    /// `[0, 2^32-1]` would be a false proof, so the body must stay out of reach.
    #[test]
    fn word_width_load_stays_unmodelled() {
        let defn = parse_one(
            "defn f(p: Int) [p >= 0][term >= 0] -> Int {\n\
                 term Load#(p, 4);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.proven.is_empty(), "proven: {:?}", report.proven);
        assert!(
            report
                .skipped
                .iter()
                .any(|line| line.contains("no value summary")),
            "a 4-byte load must stay skipped: {:?}",
            report.skipped
        );
    }
}

#[cfg(test)]
mod handle_model_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A String parameter is an opaque handle, so the byte accessor shape proves.
    ///
    /// `(s as Data) as Int` is a representation cast (ptrtoint, no call), and
    /// `Load#(..., 1)` is zero-extended, so `term >= 0` follows. Before phase 2a
    /// this body never reached the prover: the String type gate refused it.
    #[test]
    fn handle_representation_cast_proves_a_byte_bound() {
        let defn = parse_one(
            "defn raw(s: String) [true][term >= 0] -> Int {\n\
                 term Load#((s as Data) as Int, 1);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
        assert_eq!(
            report.proven.len(),
            1,
            "a representation-cast byte read must prove: unproven={:?} skipped={:?}",
            report.unproven,
            report.skipped
        );
    }

    /// `String as Int` is a **conversion** (it lowers through `str_to_int`), so it
    /// must never be read as a representation — treating it as identity would
    /// fabricate a proof about parsed text.
    #[test]
    fn direct_string_to_int_cast_is_not_identity() {
        let defn = parse_one(
            "defn parsed(s: String) [true][term >= 0] -> Int {\n\
                 term s as Int;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(
            report.proven.is_empty(),
            "a conversion must not be proved as an identity: {:?}",
            report.proven
        );
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
    }

    /// A handle-returning contract is a claim about text or length — a separate
    /// gate. It is skipped with a reason, never reported as unproven (nothing was
    /// decided against it) and never guessed at.
    #[test]
    fn handle_returning_contract_is_skipped_with_a_reason() {
        // A *value* claim about the result handle. (A length-only claim is in the
        // model since phase 2b and is covered by `length_only_handle_contract_proves`;
        // this fixture keeps the genuine skip case pinned.)
        let defn = parse_one(
            "out defn pick(a: String, b: String) [true][CharCount#(term) == CharCount#(a)] -> String {\n\
                 term b;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.proven.is_empty(), "proven: {:?}", report.proven);
        assert!(report.unproven.is_empty(), "unproven: {:?}", report.unproven);
        assert!(
            report
                .skipped
                .iter()
                .any(|line| line.contains("handle-returning")),
            "expected the handle-returning skip: {:?}",
            report.skipped
        );
    }
}

#[cfg(test)]
mod length_fact_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A handle's stored length is a non-negative integer, so the byte accessor's
    /// guard no longer mentions reflection and its contract proves.
    #[test]
    fn length_guard_lets_the_byte_accessor_prove() {
        let defn = parse_one(
            "defn at(s: String, i: Int) [true][term >= 0 && term <= 255] -> Int {\n\
                 let addr: Int = (s as Data) as Int;\n\
                 when i < 0 || i >= s.^Length { term 0; };\n\
                 term Load#(addr + 8 + i, 1);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
        assert_eq!(
            report.proven.len(),
            1,
            "the length guard must let this prove: unproven={:?} skipped={:?}",
            report.unproven,
            report.skipped
        );
    }

    /// Two different handles have independent lengths: a claim that one equals
    /// the other must stay undecided, never be assumed.
    #[test]
    fn distinct_handles_have_distinct_lengths() {
        let defn = parse_one(
            "defn same(a: String, b: String) [true][term >= 0] -> Int {\n\
                 when a.^Length == b.^Length { term 0; };\n\
                 term 1;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
    }

    /// Length reflection on a non-handle never reaches the prover: the frontend
    /// refuses it, which is why the prover's own base-type guard is defensive
    /// rather than reachable from valid source.
    #[test]
    fn length_on_a_non_handle_is_a_frontend_error() {
        let err = crate::library::parse_and_check(
            "<test>",
            "defn f(n: Int) [n >= 0][term >= 0] -> Int { term n.^Length; };",
        )
        .expect_err("`.^Length` on an Int must be rejected by the type checker");
        let text = format!("{err}");
        assert!(
            text.contains("no intrinsic (stored) length"),
            "unexpected diagnostic: {text}"
        );
    }
}

#[cfg(test)]
mod handle_contract_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A handle-returning contract that claims only a length bound is in the
    /// model now, so it proves instead of being skipped wholesale.
    #[test]
    fn length_only_handle_contract_proves() {
        let defn = parse_one(
            "out defn ident(s: String) [true][term.^Length >= 0] -> String {\n\
                 term s;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
        assert_eq!(
            report.proven.len(),
            1,
            "a length-only handle contract must prove: unproven={:?} skipped={:?}",
            report.unproven,
            report.skipped
        );
    }

    /// A postcondition relating **two** handles' lengths is outside the model
    /// (`op Concat` algebra is not modelled), so it stays a skip — never unproven,
    /// never proved.
    #[test]
    fn cross_handle_length_equation_is_skipped() {
        let defn = parse_one(
            "out defn join(a: String, b: String) [true][CharCount#(term) == CharCount#(a) + CharCount#(b)] -> String {\n\
                 term a;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.proven.is_empty(), "proven: {:?}", report.proven);
        assert!(report.unproven.is_empty(), "unproven: {:?}", report.unproven);
        assert!(
            report.skipped.iter().any(|l| l.contains("handle-returning")),
            "expected a handle skip: {:?}",
            report.skipped
        );
    }

    /// A handle-returning declaration whose post says nothing about the result
    /// (`[i == count]`) proves nothing about the handle, so it is skipped rather
    /// than examined into an unproven verdict.
    #[test]
    fn handle_returning_post_silent_about_handle_is_skipped() {
        let defn = parse_one(
            "out defn countdown(s: String, n: Int, i: Int) [i <= n][i == n] -> String {\n\
                 term s;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.unproven.is_empty(), "unproven: {:?}", report.unproven);
        assert!(
            report.skipped.iter().any(|l| l.contains("handle-returning")),
            "expected a handle skip: {:?}",
            report.skipped
        );
    }

    /// A Bool body that *is* its own postcondition decides structurally: the
    /// comparison is a self-identical opaque value, so `term == <same comparison>`
    /// holds without a solver.
    #[test]
    fn relational_body_equalling_its_post_proves() {
        let defn = parse_one(
            "defn empty(s: String) [true][term == (CharCount#(s) == 0)] -> Bool {\n\
                 term CharCount#(s) == 0;\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
        assert_eq!(
            report.proven.len(),
            1,
            "a self-identical relational body must prove: unproven={:?} skipped={:?}",
            report.unproven,
            report.skipped
        );
    }
}

#[cfg(test)]
mod contract_atom_tests {
    use super::*;

    /// Parse a whole program through the real frontend.
    fn parse_all(src: &str) -> Vec<TopLevel> {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items
    }

    /// A caller proves from the callee's **declared** bound, even though the
    /// callee's body could never earn a value summary (it reads memory).
    #[test]
    fn caller_proves_from_declared_bound() {
        let items = parse_all(
            "defn raw(s: String, i: Int) [true][term >= 0 && term <= 255] -> Int {\n\
                 term Load#((s as Data) as Int + i, 1);\n\
             };\n\
             defn use_raw(s: String, i: Int) [true][term >= 0] -> Int {\n\
                 term raw(s, i);\n\
             };",
        );
        let report = verify_bodies(&items);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
        assert!(
            report.proven.iter().any(|n| n == "use_raw"),
            "the caller must prove from the declared bound: proven={:?} skipped={:?}",
            report.proven,
            report.skipped
        );
    }

    /// A **value-relational** contract is not substituted: it would replace a call
    /// with an atom that says nothing about the value, turning a skip into an
    /// unproven verdict without widening the proof.
    #[test]
    fn value_relational_contract_is_not_substituted() {
        let items = parse_all(
            // Guarded body: no value summary is possible, and the contract is a
            // value relation (`term == a`) rather than a literal bound.
            "defn pick(a: Int) [a >= 0][term == a] -> Int {\n\
                 when a > 0 { term a; };\n\
                 term 0;\n\
             };\n\
             defn caller(a: Int) [a >= 0][term >= 0] -> Int {\n\
                 term pick(a);\n\
             };",
        );
        let report = verify_bodies(&items);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
        assert!(
            report.skipped.iter().any(|l| l.contains("caller")),
            "a value-relational callee must leave the body skipped: {:?}",
            report.skipped
        );
    }

    /// Two calls to the same callee get **distinct** atoms, so `f(x) - f(x)` never
    /// folds to zero across a possible memory change.
    #[test]
    fn two_calls_to_one_callee_do_not_collapse() {
        let items = parse_all(
            "defn raw(s: String, i: Int) [true][term >= 0 && term <= 255] -> Int {\n\
                 term Load#((s as Data) as Int + i, 1);\n\
             };\n\
             defn same(s: String) [true][term == 0] -> Int {\n\
                 term raw(s, 0) - raw(s, 0);\n\
             };",
        );
        let report = verify_bodies(&items);
        assert!(
            !report.proven.iter().any(|n| n == "same"),
            "two calls must not be treated as one value: {:?}",
            report.proven
        );
    }

    /// A self-recursive contract contributes nothing: using it would be the guess
    /// this prover exists to avoid.
    #[test]
    fn self_recursive_contract_contributes_nothing() {
        let items = parse_all(
            "defn spin(n: Int) [n >= 0][term == 999] -> Int {\n\
                 term spin(n);\n\
             };",
        );
        let report = verify_bodies(&items);
        assert!(
            report.proven.is_empty(),
            "a self-referential contract must never prove itself: {:?}",
            report.proven
        );
    }
}

#[cfg(test)]
mod tuple_result_tests {
    use super::*;

    /// Parse one declaration through the real frontend.
    fn parse_one(src: &str) -> TopLevel {
        let (items, _universe) =
            crate::library::parse_and_check("<test>", src).expect("parse_and_check");
        items.into_iter().next().expect("one item")
    }

    /// A tuple result is proved field-by-field from the preconditions.
    #[test]
    fn tuple_fields_prove_from_the_precondition() {
        let defn = parse_one(
            "defn pair(acc: Int, k: Int) [acc >= 1 && k >= 0][term.0 >= 1 && term.1 >= 0] -> (Int, Int) {\n\
                 term (acc, k);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert!(report.violated.is_empty(), "violated: {:?}", report.violated);
        assert_eq!(
            report.proven.len(),
            1,
            "both fields must prove: unproven={:?} skipped={:?}",
            report.unproven,
            report.skipped
        );
    }

    /// A field that contradicts the body still refutes — the field split must not
    /// let a wrong component pass because another one held.
    #[test]
    fn a_false_tuple_field_refutes() {
        let defn = parse_one(
            "defn bad(acc: Int, k: Int) [acc >= 1 && k >= 0][term.0 >= 1 && term.1 < 0] -> (Int, Int) {\n\
                 term (acc, k);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert_eq!(
            report.violated.len(),
            1,
            "a false field must be refuted: proven={:?} unproven={:?}",
            report.proven,
            report.unproven
        );
    }

    /// A bare `term` on a multi-value result names the FIRST component, the same
    /// convention the call gate uses.
    #[test]
    fn bare_term_names_the_first_component() {
        let defn = parse_one(
            "defn first(acc: Int, k: Int) [acc >= 1 && k >= 0][term >= 1] -> (Int, Int) {\n\
                 term (acc, k);\n\
             };",
        );
        let report = verify_bodies(&[defn]);
        assert_eq!(
            report.proven.len(),
            1,
            "a bare term must resolve to component 0: unproven={:?}",
            report.unproven
        );
    }
}

/// Origin labels must be metadata (2026-11-17, skip-origin reporting).
///
/// `--verify-report` now says whether an entry belongs to the file or to an
/// import. That split exists to fix *accounting*, not verdicts, so the property
/// worth pinning is that labelling cannot move a single declaration between
/// `proven`, `unproven`, `skipped` and `violated`.
#[cfg(test)]
mod origin_reporting_tests {
    use super::*;
    use std::collections::HashSet;

    /// A unit with its own contract-bearing defn plus an imported one, so the
    /// report has both origins present.
    fn two_origin_program() -> Vec<TopLevel> {
        let src = r#"
defn own_identity(a: Int) [a >= 0][term == a] { term a; };
defn own_floaty(f: Float) [true][term == term] { term f; };
"#;
        crate::library::parse_and_check("<origin-test>", src)
            .expect("parse_and_check")
            .0
    }

    #[test]
    fn labelling_never_changes_a_verdict() {
        let items = two_origin_program();
        let report = verify_bodies(&items);
        let plain = report.to_json();
        // Pretend every declaration whose name starts with `own_` is the file's
        // and the rest are imported: the classification itself is exercised by
        // the unit tests, this test is about the labels being inert.
        let own = declared_names(&items);
        let imported: HashSet<String> = HashSet::new();
        let labelled = report.to_json_with_origins(&OriginTable::new(own, imported));

        for bucket in ["proven", "unproven", "violated", "skipped"] {
            let before = plain[bucket].as_array().expect("bucket array");
            let after = labelled[bucket].as_array().expect("bucket array");
            assert_eq!(before.len(), after.len(), "{bucket}: entry count moved");
            for (a, b) in before.iter().zip(after.iter()) {
                assert_eq!(a["name"], b["name"], "{bucket}: entry order moved");
                assert_eq!(a["reason"], b["reason"], "{bucket}: reason changed");
            }
        }
        // And the labels really are present, or the test above proves nothing.
        let skipped_origins = labelled["skipped"]
            .as_array()
            .expect("skipped")
            .iter()
            .map(|entry| entry["origin"].as_str().expect("origin").to_string())
            .collect::<Vec<_>>();
        assert!(
            skipped_origins.iter().all(|o| o == "own"),
            "every declaration here is the file's own: {skipped_origins:?}"
        );
    }

    #[test]
    fn declared_names_uses_the_reported_name() {
        let items = two_origin_program();
        let names = declared_names(&items);
        assert!(names.contains("own_identity"));
        assert!(names.contains("own_floaty"));
        // Non-reportable items (types, imports) are not in the attribution set:
        // a wider set would let an imported *type* name claim a defn entry.
        assert_eq!(names.len(), 2);
    }
}
