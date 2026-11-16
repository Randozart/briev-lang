// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// src/analysis/verify_bodies.rs — BODY ⇒ POSTCONDITION verification.
//
// `brievc --verify-bodies` makes the compiler prove that each txn/defn BODY's
// execution establishes its OWN [pre][post] — not merely that the contract is
// internally consistent (which is all the compile gate checks today). See
// docs/plans/2026-11-11-verify-bodies-body-correctness.md.
//
// Approach: symbolic execution over the body's paths (preconditions and guard
// facts bound as path constraints, the returned `term` value captured per
// path), then a TRI-STATE postcondition verdict per path:
//
//   Holds     — the postcondition definitely holds on this path.
//   Violated  — a concrete counterexample is known (e.g. term==0 but term 5).
//   Undecided — could not prove either way with the pure-integer reasoning.
//
// Aggregation is three-way and honest: a concrete Violated is a hard error;
// an Undecided on a supported body is an advisory warning (never a false
// failure, never a false proof); bodies outside the prover's pure-integer
// surface are skipped silently (declared coverage, per the plan's soundness
// boundary).

use crate::ast::{
    Annotation, BinaryOpKind, Contract, Expr, OutputType, Statement, TopLevel, Type, UnaryOpKind,
};
use crate::symbolic::{SymbolicState, SymbolicValue, eval_symbolic, simplify_binary};
use std::collections::HashMap;
use std::sync::Mutex;

/// Z3 escalation profiler — active only with `--features z3-profile`.
///
/// Instruments `verify_one` to record WHY each path escalated to `smt_discharge`.
/// Output via `z3_profile_report()`.  Run the corpus through `verify_bodies` with
/// the feature enabled, then grep the output for "Z3 profile".
///
/// The histogram tells us where structural derivation stops and Z3 begins:
/// - `binary_vs_nonliteral` = Tier 1a gap (a+b compared, neither operand is a bound literal)
/// - `field_projection`      = Tier 2a gap (x.n in postcondition, may still need Z3 if field not in map)
/// - `mul_sign`             = sign-lattice candidate (a * b > 0)
/// - `uninterpreted_fn`    = needs lemma or Z3 (f(x) in postcondition)
/// - `pure_nonlinear`       = Z3 only (sqrt, pow, division)
/// - `other`                = needs per-case diagnosis
#[cfg(feature = "z3-profile")]
mod z3_profile {
    use crate::ast::{BinaryOpKind, Expr, UnaryOpKind};
    use crate::symbolic::SymbolicState;
    use crate::verify::ProofVerdict;
    use std::collections::HashMap;
    use std::sync::RwLock;

    /// Records Z3 escalation patterns for corpus analysis.
    #[derive(Default)]
    pub struct Z3EscalationProfile {
        /// Total paths that escalated to smt_discharge.
        pub escalated: usize,
        /// Z3 found a concrete violation.
        pub smt_violated: usize,
        /// Z3 proved the postcondition holds.
        pub smt_holds: usize,
        /// Z3 unavailable / model-dependent / bound / not encodable.
        pub smt_none: usize,
        /// Postcondition structural category.
        post_category: HashMap<&'static str, usize>,
    }

    // RwLock + interior mutability: `force()` gives `&Z3EscalationProfile`; lock()
    // for write access to `record`.  Initialised once on first use, safe because
    // Rust closures in statics are implicitly Send + Sync and this is only called
    // from single-threaded test / verify_bodies_with_types contexts.
    fn make_profile() -> RwLock<Z3EscalationProfile> {
        RwLock::new(Z3EscalationProfile::default())
    }
    static PROFILE: std::sync::LazyLock<RwLock<Z3EscalationProfile>> =
        std::sync::LazyLock::new(make_profile);

    /// Classify the structural shape of a postcondition expression.
    ///
    /// Returns one of:
    /// - `"binary_vs_literal"`       — comparison of a binary expression against a literal
    /// - `"binary_vs_nonliteral"`     — comparison of a binary expression against a non-literal
    /// - `"field_projection"`         — post contains `x.n` (a struct field read)
    /// - `"mul_sign"`                 — post has a multiplication/subdivision on both sides
    /// - `"uninterpreted_fn"`         — post contains a function call
    /// - `"pure_nonlinear"`           — sqrt, pow, division in post (requires reals)
    /// - `"identifier_vs_literal"`    — plain identifier vs literal (already handled by bounds)
    /// - `"structural_eq"`            — equality comparison of compound terms
    /// - `"other"`                   — anything not in the above categories
    fn either<L, R>(l: L, r: R, pred: impl Fn(&Expr) -> bool) -> bool {
        pred(l) || pred(r)
    }

    fn both_sides(l: &Expr, r: &Expr, pred: impl Fn(&Expr) -> bool) -> bool {
        pred(l) && pred(r)
    }

    /// True if either operand is a Mul or Div binary op (sign-lattice candidate).
    fn has_mul_or_div(l: &Expr, r: &Expr) -> bool {
        either(l, r, |e| {
            matches!(
                e,
                Expr::BinaryOp(BinaryOpKind::Mul | BinaryOpKind::Div, _, _)
            )
        })
    }

    /// True if either operand is a Div or Mod binary op (real arithmetic needed).
    fn has_div_or_mod(l: &Expr, r: &Expr) -> bool {
        either(l, r, |e| {
            matches!(
                e,
                Expr::BinaryOp(BinaryOpKind::Div | BinaryOpKind::Mod, _, _)
            )
        })
    }

    /// True if either operand is a Call or BitNot (nonlinear / uninterpreted).
    fn has_nonlinear(l: &Expr, r: &Expr) -> bool {
        either(l, r, |e| {
            matches!(e, Expr::Call(..) | Expr::UnaryOp(UnaryOpKind::BitNot, _))
        })
    }

    /// True if one operand is a BinaryOp/Field and the other is a Decimal literal.
    fn binary_vs_literal(l: &Expr, r: &Expr) -> bool {
        both_sides(l, r, |_| false)
            || (is_binary_or_field(l) && is_decimal(r))
            || (is_binary_or_field(r) && is_decimal(l))
    }

    /// True if one operand is a BinaryOp/Field and the other is an Identifier.
    fn binary_vs_nonliteral(l: &Expr, r: &Expr) -> bool {
        (is_binary_or_field(l) && is_identifier(r)) || (is_binary_or_field(r) && is_identifier(l))
    }

    /// True if the op is Eq/Neq and at least one side is a BinaryOp or Field.
    fn is_structural_eq(op: &BinaryOpKind, l: &Expr, r: &Expr) -> bool {
        (op.is_eq() || op.is_neq()) && (is_binary_or_field(l) || is_binary_or_field(r))
    }

    fn is_decimal(e: &Expr) -> bool {
        matches!(e, Expr::Decimal(_))
    }

    fn is_identifier(e: &Expr) -> bool {
        matches!(e, Expr::Identifier(_))
    }

    fn is_binary_or_field(e: &Expr) -> bool {
        matches!(e, Expr::BinaryOp(..) | Expr::Field(..))
    }

    fn categorize_non_comparison(post: &Expr) -> &'static str {
        match post {
            Expr::Field(..) => "field_projection",
            Expr::Call(..) => "uninterpreted_fn",
            Expr::Identifier(_) => "identifier_vs_literal",
            _ => "other",
        }
    }

    /// Classify a postcondition into a structural category for profiling.
    /// Early-return per category; all two-operand checks are in named helpers.
    pub fn categorize_post(post: &Expr) -> &'static str {
        let Expr::BinaryOp(op, lhs, rhs) = post else {
            return categorize_non_comparison(post);
        };
        if !op.is_comparison() {
            return "other";
        }
        let (l, r) = (&**lhs, &**rhs);
        if has_mul_or_div(l, r) {
            return "mul_sign";
        }
        if has_div_or_mod(l, r) {
            return "pure_nonlinear";
        }
        if has_nonlinear(l, r) {
            return "pure_nonlinear";
        }
        if binary_vs_literal(l, r) {
            return "binary_vs_literal";
        }
        if binary_vs_nonliteral(l, r) {
            return "binary_vs_nonliteral";
        }
        if is_structural_eq(op, l, r) {
            return "structural_eq";
        }
        "other"
    }

    impl Z3EscalationProfile {
        fn record(&mut self, post: &Expr, result: Option<ProofVerdict>) {
            self.escalated += 1;
            match result {
                Some(ProofVerdict::Violated) => self.smt_violated += 1,
                Some(ProofVerdict::Holds) => self.smt_holds += 1,
                None => self.smt_none += 1,
                _ => {}
            }
            let cat = categorize_post(post);
            *self.post_category.entry(cat).or_insert(0) += 1;
        }
    }

    /// Called from `verify_one` each time `smt_discharge` is invoked.
    pub fn record_smt_result(post: &Expr, result: Option<ProofVerdict>) {
        if let Ok(mut p) = std::sync::LazyLock::force(&PROFILE).write() {
            p.record(post, result);
        }
    }

    /// Print the profile histogram to stdout.
    ///
    /// Output is stable key-value format — parseable by scripts:
    /// ```
    /// Z3 profile:
    ///   escalated:       203
    ///   smt_violated:     12
    ///   smt_holds:        89
    ///   smt_none:        102  (solver unavailable / model-dependent / not encodable)
    ///   binary_vs_literal:    45
    ///   binary_vs_nonliteral: 12
    ///   field_projection:      31
    ///   mul_sign:              8
    ///   uninterpreted_fn:     14
    ///   pure_nonlinear:         7
    ///   structural_eq:         5
    ///   identifier_vs_literal: 31
    ///   other:                 50
    /// ```
    pub fn report() {
        let p = std::sync::LazyLock::force(&PROFILE).read().unwrap();
        println!("Z3 profile:");
        println!("  escalated:       {}", p.escalated);
        println!("  smt_violated:    {}", p.smt_violated);
        println!("  smt_holds:       {}", p.smt_holds);
        println!(
            "  smt_none:        {}  (solver unavailable / model-dependent / not encodable)",
            p.smt_none
        );
        let mut cats: Vec<_> = p.post_category.iter().collect();
        cats.sort_by_key(|(k, _)| *k);
        for (cat, count) in cats.iter() {
            println!("  {cat:30}  {count}");
        }
        let smt_total = p.smt_violated + p.smt_holds;
        if smt_total > 0 {
            println!("  --- SMT resolution ---");
            println!(
                "  smt_violated/smt_total: {}/{} ({:.0}%)",
                p.smt_violated,
                smt_total,
                100.0 * p.smt_violated as f64 / smt_total as f64
            );
        }
    }
}

/// Wire Z3 profiling into `smt_discharge`.  When the feature is off this is a
/// no-op (LLVM dead-code elimination removes the call).
#[cfg(feature = "z3-profile")]
macro_rules! z3_profile_record {
    ($post:expr, $result:expr) => {
        z3_profile::record_smt_result($post, $result);
    };
}
#[cfg(not(feature = "z3-profile"))]
macro_rules! z3_profile_record {
    ($post:expr, $result:expr) => {};
}

mod interval;
use interval::{Interval as Bound, Relation};

/// Tri-state outcome of evaluating a postcondition on one symbolic path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProofVerdict {
    /// Definitely satisfied on this path.
    Holds,
    /// A concrete counterexample is known (postcondition refuted).
    Violated,
    /// Could not prove either way; never treated as proven.
    Undecided,
}

/// Outcome of verifying one declaration's body against its contract.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyOutcome {
    /// Every terminating path provably satisfies the postcondition.
    Proven,
    /// Supported body, but the prover could not decide (advisory only).
    Unproven(String),
    /// A concrete counterexample exists — a hard failure.
    Violated(String),
}

/// Sweep result for a whole program under `--verify-bodies`.
///
/// `violated` entries fail the build; `unproven` entries are printed as
/// advisory warnings; proven bodies are counted, not printed.
#[derive(Debug, Default)]
pub struct BodyReport {
    /// Declarations whose bodies were proven to satisfy their contracts.
    pub proven: Vec<String>,
    /// Supported bodies the prover could not decide (advisory, not failures).
    pub unproven: Vec<String>,
    /// Declarations with a concrete counterexample — hard failures.
    pub violated: Vec<String>,
    /// Declarations the prover never examined, with the reason.
    ///
    /// These are not successes. A body outside the supported surface used to be
    /// dropped silently, so a caller could not tell "proved" from "never looked
    /// at". Naming them costs nothing and removes the ambiguity.
    pub skipped: Vec<String>,
}

impl BodyReport {
    /// Render the report as JSON for tooling (`--verify-report`).
    ///
    /// Entries become `{ "name", "reason" }` objects so a consumer never parses
    /// prose. The internal strings are `name: reason` for the human diagnostic;
    /// a declaration name is an identifier, so it cannot contain `: ` and the
    /// split is unambiguous.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            // Names, not a bare count: a consumer has to be able to tell
            // "proved" from "never examined", and a total cannot do that. The
            // prelude is included in this list, so consumers intersect with
            // their own declarations.
            "proven": self.proven.iter().map(|name| serde_json::json!({
                "name": name,
                "reason": "",
            })).collect::<Vec<_>>(),
            "unproven": findings_json(&self.unproven),
            "violated": findings_json(&self.violated),
            "skipped": findings_json(&self.skipped),
        })
    }
}

/// Split `name: reason` entries into structured objects.
pub fn findings_json(entries: &[String]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| {
            let (name, reason) = entry.split_once(": ").unwrap_or((entry.as_str(), ""));
            serde_json::json!({ "name": name, "reason": reason })
        })
        .collect()
}

/// Verify every txn/defn body (incl. SyncGroup/Export wrappers) against its
/// own explicit contract.
///
/// Explicit source types require `verify_bodies_with_types`; this standalone
/// entry point admits only untyped mathematical AST declarations.
///
/// Returns the three-way report; the caller decides failure (violated) and
/// warning (unproven) handling. Bodies whose surface is outside the
/// pure-integer prover's reach are skipped — never wrongly proven, never
/// falsely rejected.
///
/// # Gating
/// Callers enable this only under `BuildOptions.verify_bodies`; with the flag
/// off (the default) the historical consistency-only behaviour is unchanged.
pub fn verify_bodies(items: &[TopLevel]) -> BodyReport {
    verify_bodies_with_types(items, &|_| crate::domain::ScalarModel::Unsupported)
}

/// Verify source declarations with compiler-resolved scalar semantics. The
/// resolver must account for the selected target and user operator overrides.
/// Untyped AST variables retain the standalone API's mathematical word model.
pub fn verify_bodies_with_types(
    items: &[TopLevel],
    resolve: &crate::domain::TypeResolver<'_>,
) -> BodyReport {
    let mut report = BodyReport::default();
    let globals = crate::domain::global_names(items);
    // C3: value summaries let a body be proved through the helpers it calls.
    // Computed once, up front, so the result never depends on item order.
    let sums = build_summaries(items, resolve, &globals);
    for item in items {
        let (name, contract, body) = match contract_target(item) {
            Some(t) => t,
            None => continue,
        };
        // Skip unwritten contracts — `[true][true]` placeholders are not
        // propositions to prove.
        if !contract.explicit || trivially_true(&contract.post_condition) {
            continue;
        }
        if let Some(reason) = crate::domain::unsupported_declaration(item, resolve, &globals) {
            report.skipped.push(format!("{name}: {reason}"));
            continue;
        }
        // A loop carrying an `invariant` annotation is proved by its three
        // Hoare obligations, not as one straight-line body. Each obligation is
        // loop-free, so the ordinary path walk discharges it.
        // 2026-11-15 (phase 1b): Bool-typed defns declare `term` as `Bool` in
        // the SMT query. `term_is_bool` reads the declared return type once.
        let term_sort = if term_is_bool(item) {
            Some(crate::smt::encoding::Sort::Boolean)
        } else {
            None
        };
        let obligations = loop_obligations(name, contract, body)
            .or_else(|| inferred_loop_obligations(name, contract, body))
            .or_else(|| convergent_obligations(item, name, contract, body));
        if let Some(obligations) = obligations {
            for (obligation, pre, post, obligation_body) in obligations {
                verify_supported(
                    &mut report,
                    &obligation,
                    (&pre, &post),
                    &obligation_body,
                    &sums,
                    term_sort,
                );
            }
            continue;
        }
        verify_supported(
            &mut report,
            name,
            (&contract.pre_condition, &contract.post_condition),
            body,
            &sums,
            term_sort,
        );
    }
    // Emit Z3 escalation histogram when profiling is active.
    // This fires once per top-level verify_bodies_with_types call — callers can
    // grep for "Z3 profile" to collect data without the feature enabled.
    #[cfg(feature = "z3-profile")]
    z3_profile::report();
    report
}

/// Apply the same admission and inlining boundary to ordinary bodies and
/// generated loop obligations. Calls must have independently admitted summaries.
fn verify_supported(
    report: &mut BodyReport,
    name: &str,
    contract: (&Expr, &Expr),
    body: &[Statement],
    sums: &HashMap<String, Summary>,
    term_sort: Option<crate::smt::encoding::Sort>,
) {
    let reason = unsupported_reason(body, sums)
        .or_else(|| crate::domain::unsupported_expression(contract.0))
        .or_else(|| crate::domain::unsupported_expression(contract.1));
    if let Some(reason) = reason {
        report.skipped.push(format!("{name}: {reason}"));
        return;
    }
    // 2026-11-15 (phase 1b): Bool-typed defns are gated with `supported_boolean`
    // on the postcondition (the SMT query declares `term` as `Bool`, so
    // `term == (n >= 10)` encodes as Bool == Bool). Int-typed defns keep the
    // word-model gate. Both gates encode to the same SMT-LIB Boolean sort;
    // the difference is only which operand sorts the encoder admits.
    // 2026-11-15 (phase 1b): Bool-typed defns skip the gate entirely — the
    // SMT encoding with `term_sort=Some(Boolean)` is the authoritative sort
    // check (it encodes `term` as `Bool` and `==`/`!=`/`and`/`or`/`not` on
    // Bool operands produce Boolean terms). The gate is only meaningful for
    // word-model contracts, where it distinguishes BitVec encodings from
    // unsupported syntax. Skipping it for Bool defns avoids a false reject
    // when the standalone API (no type universe) cannot resolve Bool's
    // protocol category, leaving the SMT encoder to decide.
    // 2026-11-15 (phase 1b): the PRECONDITION is gated with the term-sort
    // aware encoder (a Bool defn's `[pre]` may mention `term` as Bool);
    // the POSTCONDITION keeps `supported_word_postcondition` for the word
    // model — it special-cases `Field` expressions that the strict
    // `expect` encoder rejects. For Bool defns the postcondition is gated
    // with the term-sort aware encoder instead, since `term` is declared
    // `Bool` and comparisons like `term == (x == 0)` encode as Bool == Bool.
    let pre_ok = crate::smt::supported_boolean_with_term_sort(contract.0, term_sort);
    let post_ok = match term_sort {
        Some(crate::smt::encoding::Sort::Boolean) => {
            crate::smt::supported_boolean_with_term_sort(contract.1, term_sort)
        }
        _ => crate::smt::supported_word_postcondition(contract.1),
    };
    if !pre_ok || !post_ok {
        report.unproven.push(format!(
            "{name}: unsupported or ill-sorted word-model contract"
        ));
        return;
    }
    // Substitute only summarized callees so the path walk sees a flat body.
    let Ok(flattened) = inline_body(body, sums) else {
        report
            .skipped
            .push(format!("{name}: a call could not be inlined"));
        return;
    };
    match verify_one_with_term_sort(contract.0, contract.1, &flattened, term_sort) {
        BodyOutcome::Proven => report.proven.push(name.to_string()),
        BodyOutcome::Unproven(why) => report.unproven.push(format!("{name}: {why}")),
        BodyOutcome::Violated(why) => report.violated.push(format!("{name}: {why}")),
    }
}

/// Extract (name, contract, body) from a top-level item, looking one level
/// into Export/SyncGroup wrappers so wrapped txns are verified too.
fn contract_target(item: &TopLevel) -> Option<(&str, &crate::ast::Contract, &[Statement])> {
    match item {
        TopLevel::Definition(d) => Some((&d.name, &d.contract, d.body.as_slice())),
        TopLevel::Transaction(t) => Some((&t.name, &t.contract, t.body.as_slice())),
        TopLevel::Export(e) => contract_target(e.inner.as_ref()),
        TopLevel::SyncGroup { item: inner, .. } => contract_target(inner.as_ref()),
        _ => None,
    }
}

/// Whether the item's declared return type is Bool, so the SMT query can
/// declare `term` as `Bool` (2026-11-15 phase 1b).
///
/// Only `-> Bool` admits the Bool sort. Structured results (tuples,
/// unions, arrays) have no single `term` projection, and Int is the word
/// model. Returning false for everything else keeps Int defns on the
/// `(_ BitVec 64)` path they have always used.
fn term_is_bool(item: &TopLevel) -> bool {
    let item = match item {
        TopLevel::Export(e) => e.inner.as_ref(),
        TopLevel::SyncGroup { item, .. } => item.as_ref(),
        other => other,
    };
    let output = match item {
        TopLevel::Definition(d) => d.output_type.as_ref(),
        TopLevel::Transaction(t) => t.output_type.as_ref(),
        _ => None,
    };
    matches!(
        output,
        Some(OutputType::Single(Type::Custom(name))) if name == "Bool"
    )
}

/// Whether an expression is the literal `true` (nothing to prove).
fn trivially_true(expr: &Expr) -> bool {
    matches!(expr, Expr::Bool(true))
}

/// Prove one (pre, body, post) triple across all of the body's paths.
/// Bind identifiers that a fact pins to a single value (`a == 5`).
///
/// A point equality is a value fact, not a range. Recording it as an
/// assignment lets arithmetic over the identifier fold *while the body is
/// walked*, so the body's result and the postcondition are evaluated with the
/// same knowledge — pinning only at verdict time leaves the two sides
/// disagreeing (`a + a` already folded against an unpinned `a`).
fn pin_points(state: &mut SymbolicState, fact: &Expr) {
    match fact {
        Expr::BinaryOp(BinaryOpKind::And, left, right) => {
            pin_points(state, left);
            pin_points(state, right);
        }
        Expr::BinaryOp(BinaryOpKind::Eq, left, right) => {
            let pinned = match (left.as_ref(), right.as_ref()) {
                (Expr::Identifier(name), Expr::Decimal(value))
                | (Expr::Decimal(value), Expr::Identifier(name)) => Some((name.clone(), *value)),
                _ => None,
            };
            if let Some((name, value)) = pinned {
                state
                    .assignments
                    .entry(name)
                    .or_insert(SymbolicValue::Literal(value, "int".to_string()));
            }
        }
        _ => {}
    }
}

fn verify_one(pre: &Expr, post: &Expr, body: &[Statement]) -> BodyOutcome {
    verify_one_with_term_sort(pre, post, body, None)
}

/// 2026-11-15 (plan 2026-11-15-value-summaries.md phase 1b): verify with an
/// explicit sort for `term`. Bool-typed defns pass `Some(Sort::Boolean)` so
/// the SMT query declares `term` as `Bool` and postconditions like
/// `term == (x > 5)` are well-sorted. Int-typed defns pass `None`.
fn verify_one_with_term_sort(
    pre: &Expr,
    post: &Expr,
    body: &[Statement],
    term_sort: Option<crate::smt::encoding::Sort>,
) -> BodyOutcome {
    let mut initial = SymbolicState::new(pre);
    pin_points(&mut initial, pre);
    let mut terminated = Vec::new();
    walk_block(body, initial, &mut terminated);
    if terminated.is_empty() {
        return BodyOutcome::Unproven(
            "body has no terminating value path — nothing to verify".to_string(),
        );
    }
    let mut violated: Option<String> = None;
    let mut undecided = 0usize;
    let mut first_undecided: Option<usize> = None;
    for (index, state) in terminated.iter().enumerate() {
        match reachable_verdict(post, state) {
            ProofVerdict::Holds => {}
            ProofVerdict::Violated => {
                if violated.is_none() {
                    violated = Some(render_counterexample(state));
                }
            }
            ProofVerdict::Undecided => {
                // C4: escalate an inconclusive symbolic verdict to the solver
                // before reporting "could not prove". Only a real `Sat` becomes
                // a violation; every other answer stays honest and undecided.
                let smt_result = smt_discharge(post, state, term_sort);
                z3_profile_record!(post, smt_result);
                match smt_result {
                    Some(ProofVerdict::Violated) => {
                        if violated.is_none() {
                            violated = Some(render_counterexample(state));
                        }
                    }
                    Some(ProofVerdict::Holds) => {}
                    _ => {
                        undecided += 1;
                        if first_undecided.is_none() {
                            first_undecided = Some(index);
                        }
                    }
                }
            }
        }
    }
    if let Some(v) = violated {
        return BodyOutcome::Violated(format!("body can violate its postcondition: {v}"));
    }
    if undecided > 0 {
        // Name the first blocking path and render the goal. "could not prove"
        // alone leaves the author guessing which branch needs the stronger
        // fact — and that guess is what makes repairing a contract risky.
        let path = first_undecided.map_or(1, |index| index + 1);
        return BodyOutcome::Unproven(format!(
            "could not prove the postcondition on path {path} of {} (goal: {post}) — add a guard or a stronger precondition",
            terminated.len()
        ));
    }
    BodyOutcome::Proven
}

/// A refutation needs a feasible execution path. Failure to find a witness or
/// decide feasibility cannot turn an unreachable branch into a hard error.
fn reachable_verdict(post: &Expr, state: &SymbolicState) -> ProofVerdict {
    match verdict_for_path(post, state) {
        ProofVerdict::Violated => match path_feasible(state) {
            Some(true) => ProofVerdict::Violated,
            Some(false) => ProofVerdict::Holds,
            None => ProofVerdict::Undecided,
        },
        other => other,
    }
}

/// Establish a real path witness before reporting a hard violation. If the
/// simple deterministic witness fails, ask the solver about the recorded facts.
/// Unknown feasibility remains advisory, never a spurious hard rejection.
fn path_feasible(state: &SymbolicState) -> Option<bool> {
    if state
        .path_constraints
        .iter()
        .any(|fact| !crate::smt::supported_boolean(fact))
    {
        return None;
    }
    let Some(bounds) = bounds_of(state) else {
        return Some(false);
    };
    let mut witness = SymbolicState::empty();
    for name in state.path_constraints.iter().flat_map(free_idents) {
        let interval = bounds.get(&name).copied().unwrap_or(Bound::FULL);
        let value = if interval.contains(0) {
            0
        } else {
            interval.lower
        };
        witness
            .assignments
            .insert(name, SymbolicValue::int_literal(value));
    }
    if state
        .path_constraints
        .iter()
        .all(|fact| eval_post(fact, &witness, &HashMap::new()) == ProofVerdict::Holds)
    {
        return Some(true);
    }
    if !crate::smt::is_z3_available() {
        return None;
    }
    let query = crate::smt::build_vc_query(&state.path_constraints, &Expr::Bool(false));
    match crate::smt::prove_smt_formula(&query, 1000) {
        crate::smt::SmtResult::Sat(_) => Some(true),
        crate::smt::SmtResult::Unsat => Some(false),
        crate::smt::SmtResult::Unknown => None,
    }
}

/// Try to decide an Undecided path with the SMT solver (C4).
///
/// Returns `None` — leaving the caller's honest `Undecided` verdict intact —
/// when no solver is installed, when the goal falls outside the encodable
/// subset, or when the path's state cannot be substituted unambiguously (a
/// prior-state `@x` reference or an unknown value).
fn smt_discharge(
    post: &Expr,
    state: &SymbolicState,
    term_sort: Option<crate::smt::encoding::Sort>,
) -> Option<ProofVerdict> {
    if !crate::smt::is_z3_available() {
        return None;
    }
    // Soundness: a postcondition about the returned value only means something
    // when THIS path actually bound one. If `term` reaches the solver as a free
    // variable, `facts ∧ ¬post` is trivially satisfiable and the solver would
    // manufacture a refutation for a body it never reasoned about.
    for bound in ["term", "__result"] {
        if mentions_ident(post, bound) && !state.assignments.contains_key(bound) {
            return None;
        }
    }
    let goal = substitute_all(post, &state.assignments)?;
    // Defensive: substitution must have eliminated the return projections.
    if mentions_ident(&goal, "term") || mentions_ident(&goal, "__result") {
        return None;
    }
    if !encodable(&goal) {
        return None;
    }
    let mut facts = Vec::new();
    for fact in &state.path_constraints {
        // Path constraints are facts about the state WHEN THEY WERE RECORDED —
        // the entry precondition, or a guard at that moment. Substituting the
        // FINAL assignment state into them rewrites an entry fact into the goal
        // itself (`acc == i` becomes `acc == i + 6`), so the VC becomes
        // fact ≡ goal, Z3 reports unsat, and a violated obligation is reported
        // as PROVEN. That is a false proof, the one outcome this tool must never
        // produce: the facts go to the solver exactly as they were recorded.
        if !encodable(fact) {
            return None;
        }
        facts.push(fact.clone());
    }
    if std::env::var_os("BRIEV_DIAG_VC").is_some() {
        eprintln!("[vc] facts={facts:?}");
        eprintln!("[vc] goal={goal:?}");
    }
    let query = crate::smt::build_vc_query_with_term_sort(&facts, &goal, term_sort);
    if std::env::var_os("BRIEV_DIAG_VC").is_some() {
        eprintln!("[vc] query=\n{query}");
    }
    match crate::smt::prove_smt_formula(&query, 1000) {
        crate::smt::SmtResult::Unsat => Some(ProofVerdict::Holds),
        // Refute only when the body's result is fully determined: with no free
        // identifier left in the goal, a model means the body contradicts its
        // contract for EVERY input. A parameter-dependent counterexample means
        // "some input violates" — real, but the stdlib's optimistic contracts
        // (`[true][term >= 0]`) hit that routinely, so those stay advisory
        // (the caller keeps `Undecided` → warning) rather than failing builds.
        crate::smt::SmtResult::Sat(_) if free_idents(&goal).is_empty() => {
            Some(ProofVerdict::Violated)
        }
        crate::smt::SmtResult::Sat(_) => None,
        crate::smt::SmtResult::Unknown => None,
    }
}

/// Free identifiers of an expression (sorted, for determinism).
pub fn free_idents(expr: &Expr) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    collect_idents(expr, &mut out);
    out
}

/// Accumulate identifiers into `out`.
fn collect_idents(expr: &Expr, out: &mut std::collections::BTreeSet<String>) {
    match expr {
        Expr::Identifier(name) => {
            out.insert(name.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            collect_idents(l, out);
            collect_idents(r, out);
        }
        Expr::UnaryOp(_, inner) => collect_idents(inner, out),
        _ => {}
    }
}

/// Maximum transitive substitution depth before the discharge gives up.
const SUBST_DEPTH_CAP: usize = 8;

/// Evaluate current expression leaves once; saved symbolic values already refer
/// to immutable entry atoms and must never be followed through later bindings.
fn substitute_all(expr: &Expr, assignments: &HashMap<String, SymbolicValue>) -> Option<Expr> {
    crate::symbolic::snapshot::expression(expr, assignments)
}

/// Whether the SMT encoder models this expression exactly.
///
/// `encode_expr_smt` silently falls back to asserting equality for operators it
/// does not know, so anything outside this set must never reach the solver.
fn encodable(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(_) | Expr::Bool(_) | Expr::Identifier(_) => true,
        Expr::BinaryOp(kind, l, r) => {
            matches!(
                kind,
                BinaryOpKind::Eq
                    | BinaryOpKind::Neq
                    | BinaryOpKind::Lt
                    | BinaryOpKind::Gt
                    | BinaryOpKind::Le
                    | BinaryOpKind::Ge
                    | BinaryOpKind::And
                    | BinaryOpKind::Or
                    | BinaryOpKind::Add
                    | BinaryOpKind::Sub
                    | BinaryOpKind::Mul
                    | BinaryOpKind::Div
                    | BinaryOpKind::Mod
            ) && encodable(l)
                && encodable(r)
        }
        Expr::UnaryOp(kind, inner) => {
            matches!(kind, UnaryOpKind::Not | UnaryOpKind::Neg) && encodable(inner)
        }
        _ => false,
    }
}

/// Whether an expression mentions `name` anywhere.
fn mentions_ident(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Identifier(ident) => ident == name,
        Expr::BinaryOp(_, l, r) => mentions_ident(l, name) || mentions_ident(r, name),
        Expr::UnaryOp(_, inner) => mentions_ident(inner, name),
        _ => false,
    }
}

/// Walk one statement block, forking on guard blocks.
///
/// Terminating paths (states where the returned value is bound, or the path
/// ends without a value) are appended to `terminated`; the returned vec holds
/// the states that reach the END of this block without terminating — the
/// caller decides their continuation (guard fall-through continues into the
/// enclosing block).
fn walk_block(
    body: &[Statement],
    state: SymbolicState,
    terminated: &mut Vec<SymbolicState>,
) -> Vec<SymbolicState> {
    let mut conts = vec![state];
    for stmt in body {
        if conts.is_empty() {
            break;
        }
        conts = step_all(stmt, conts, terminated);
    }
    conts
}

/// Apply one statement to every continuation state (the per-statement fan-out
/// of the path walk).
fn step_all(
    stmt: &Statement,
    conts: Vec<SymbolicState>,
    terminated: &mut Vec<SymbolicState>,
) -> Vec<SymbolicState> {
    let mut next = Vec::new();
    for st in conts {
        step_statement(stmt, st, &mut next, terminated);
    }
    next
}

/// Apply one statement to one continuation state.
fn step_statement(
    stmt: &Statement,
    st: SymbolicState,
    next: &mut Vec<SymbolicState>,
    terminated: &mut Vec<SymbolicState>,
) {
    match stmt {
        Statement::Guarded(cond, inner) => step_guard(cond, inner, st, next, terminated),
        Statement::Assign(lhs, expr) => step_assign(lhs, expr, st, next),
        Statement::Let {
            name,
            names,
            expr: Some(e),
            ..
        } => {
            let mut s = st;
            // `names` holds the declared names; a plain `let x = e;` has one.
            if names.len() <= 1 {
                s.assign(name, e);
            }
            next.push(s);
        }
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) => {
            terminated.push(bind_output(st, e));
        }
        // Paths that end without producing a value.
        Statement::Term(None)
        | Statement::EndProgram(None)
        | Statement::Rollback(_)
        | Statement::Halt
        | Statement::Trap => {}
        // Static assertions: recorded as facts on the path.
        Statement::Gate(c) | Statement::Check(c) => {
            let mut s = st;
            s.add_constraint(c, true);
            pin_points(&mut s, c);
            next.push(s);
        }
        // Pure expression statement: no effect on the symbolic state.
        Statement::Expression(_) => next.push(st),
        // Everything else is filtered by body_supported before we get here.
        _ => next.push(st),
    }
}

/// Fork a continuation around a guard: the taken branch descends with the
/// guard as a fact (inner fall-through re-enters this block); the skipped
/// branch continues with the negated fact.
fn step_guard(
    cond: &Expr,
    inner: &[Statement],
    st: SymbolicState,
    next: &mut Vec<SymbolicState>,
    terminated: &mut Vec<SymbolicState>,
) {
    let decision = verdict_for_path(cond, &st);
    // Guard TAKEN: descend with the guard as a fact; whatever reaches the
    // inner block's end falls through with the guard still true.
    if decision != ProofVerdict::Violated {
        let mut taken = st.clone();
        taken.add_constraint(cond, true);
        pin_points(&mut taken, cond);
        next.extend(walk_block(inner, taken, terminated));
    }
    // Guard NOT taken: continue in the enclosing block with the negated fact.
    if decision != ProofVerdict::Holds {
        let mut skipped = st;
        skipped.add_constraint(cond, false);
        next.push(skipped);
    }
}

/// Track a pure-integer assignment symbolically.
fn step_assign(lhs: &Expr, expr: &Expr, st: SymbolicState, next: &mut Vec<SymbolicState>) {
    let mut s = st;
    if let Expr::Identifier(name) = lhs {
        s.assign(name, expr);
    } else if let Expr::Field(base, field) = lhs {
        if let Expr::Identifier(base_name) = base.as_ref() {
            s.assign_field(base_name, field, expr);
        }
    }
    next.push(s);
}

/// Evaluate a terminating value and bind it as the function's result.
fn bind_output(st: SymbolicState, expr: &Expr) -> SymbolicState {
    let mut s = st;
    let sym = eval_symbolic(expr, &s);
    if sym != SymbolicValue::Unknown {
        // The postcondition names the result `term`; bind both spellings.
        s.assignments.insert("term".to_string(), sym.clone());
        s.assignments.insert("__result".to_string(), sym);
    }
    s
}

/// Drain a path's constraints into per-identifier numeric bounds.
fn bounds_of(state: &SymbolicState) -> Option<HashMap<String, Bound>> {
    let mut bounds: HashMap<String, Bound> = HashMap::new();
    for constraint in &state.path_constraints {
        if !update_bounds(constraint, &mut bounds) {
            return None;
        }
    }
    if bounds.values().any(|bound| bound.is_empty()) {
        return None;
    }
    Some(bounds)
}

/// Merge one comparison fact into the identifier bounds map.
///
/// Recurses through conjunctions (a precondition like
/// `[a >= 0 && a <= 255]` contributes both facts) and maps negated
/// comparisons to their flipped forms (`!(a > 5)` bounds `a <= 5`).
fn update_bounds(constraint: &Expr, bounds: &mut HashMap<String, Bound>) -> bool {
    match constraint {
        Expr::Bool(value) => *value,
        Expr::BinaryOp(BinaryOpKind::And, l, r) => {
            update_bounds(l, bounds) && update_bounds(r, bounds)
        }
        Expr::UnaryOp(UnaryOpKind::Not, inner) => match inner.as_ref() {
            Expr::Bool(value) => !value,
            Expr::BinaryOp(op, left, right) => Relation::from_ast(*op)
                .is_none_or(|op| comparison_bounds(op.negate(), left, right, bounds)),
            _ => true,
        },
        Expr::BinaryOp(op, left, right) => {
            Relation::from_ast(*op).is_none_or(|op| comparison_bounds(op, left, right, bounds))
        }
        _ => true,
    }
}

/// Constant unary negation follows signed-word wrapping semantics.
fn literal_word(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Decimal(value) => Some(*value),
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => literal_word(inner).map(i64::wrapping_neg),
        _ => None,
    }
}

/// Normalize literal-left facts through the same production relation mirror
/// checked by Kani; unknown/nonlinear facts retain their original SMT form.
fn comparison_bounds(
    op: Relation,
    left: &Expr,
    right: &Expr,
    bounds: &mut HashMap<String, Bound>,
) -> bool {
    if let (Some(a), Some(b)) = (literal_word(left), literal_word(right)) {
        return op.holds(a, b);
    }
    let (name, value, op) = match (left, right) {
        (Expr::Identifier(name), value) => (name, literal_word(value), op),
        (value, Expr::Identifier(name)) => (name, literal_word(value), op.mirror()),
        _ => return true,
    };
    if let Some(value) = value {
        let current = bounds.entry(name.clone()).or_insert(Bound::FULL);
        *current = current.restrict(op, value);
    }
    true
}

/// Tri-state evaluation of the postcondition against one path's state.
fn verdict_for_path(post: &Expr, state: &SymbolicState) -> ProofVerdict {
    let Some(mut bounds) = bounds_of(state) else {
        return ProofVerdict::Holds;
    };
    // Mask expressions contribute intervals: `x & MASK32` is in [0, MASK32].
    // Without this the uint-range postconditions never have a bound to consult.
    add_mask_bounds(state, &mut bounds);
    // 2026-11-12 (point bounds): a precondition that pins an identifier
    // (`[a == 5]`) is a fact about its VALUE, not merely a range. Binding such
    // an identifier to its literal lets arithmetic over it fold — otherwise
    // `term == a + a` stays Undecided even though `a` is fully known.
    let pinned = with_point_bounds(state, &bounds);
    eval_post(post, &pinned, &bounds)
}

/// Clone a state with point-bounded identifiers bound to their literal value.
///
/// An existing assignment wins: a body-local that was actually computed is more
/// precise than a constraint-derived pin.
fn with_point_bounds(state: &SymbolicState, bounds: &HashMap<String, Bound>) -> SymbolicState {
    let mut pinned = state.clone();
    for (name, bound) in bounds {
        let Some(lower) = bound.point() else {
            continue;
        };
        pinned
            .assignments
            .entry(name.clone())
            .or_insert(SymbolicValue::Literal(lower, "int".to_string()));
    }
    pinned
}

/// Recursive tri-state postcondition evaluator over the (term-bound) state.
fn eval_post(post: &Expr, state: &SymbolicState, bounds: &HashMap<String, Bound>) -> ProofVerdict {
    match post {
        Expr::Bool(b) => {
            if *b {
                ProofVerdict::Holds
            } else {
                ProofVerdict::Violated
            }
        }
        Expr::BinaryOp(kind, l, r) => match *kind {
            // Conjunction / disjunction decompose so each conjunct is proven
            // independently (e.g. `[term >= -149 && term <= 104]`).
            BinaryOpKind::And => {
                if let Some(verdict) = tautology_verdict(BinaryOpKind::And, l, r) {
                    return verdict;
                }
                let lv = eval_post(l, state, bounds);
                let rv = eval_post(r, state, bounds);
                combine_and(lv, rv)
            }
            BinaryOpKind::Or => {
                if let Some(verdict) = tautology_verdict(BinaryOpKind::Or, l, r) {
                    return verdict;
                }
                let lv = eval_post(l, state, bounds);
                let rv = eval_post(r, state, bounds);
                combine_or(lv, rv)
            }
            _ => {
                let lv = eval_symbolic(l, state);
                let rv = eval_symbolic(r, state);
                rel_verdict(*kind, &lv, &rv, bounds)
            }
        },
        Expr::UnaryOp(UnaryOpKind::Not, inner) => not_verdict(eval_post(inner, state, bounds)),
        _ => ProofVerdict::Undecided,
    }
}

/// Decide a disjunction/conjunction whose operands are logically exhaustive.
///
/// Two shapes are exhaustive by construction, and deciding them syntactically
/// avoids routing them to the solver (84 baseline escalations came from exactly
/// these two):
///
/// * `p || !p` — a comparison paired with its flipped complement.
/// * `a == true || a == false` — the whole Bool domain for one operand.
///
/// For `||` an exhaustive pair makes the postcondition TRUE; for `&&` the
/// conjunction is unsatisfiable, which refutes the path.
fn tautology_verdict(op: BinaryOpKind, l: &Expr, r: &Expr) -> Option<ProofVerdict> {
    if !is_complement(l, r) && !is_bool_exhaustive(l, r) {
        return None;
    }
    match op {
        BinaryOpKind::Or => Some(ProofVerdict::Holds),
        BinaryOpKind::And => Some(ProofVerdict::Violated),
        _ => None,
    }
}

/// True when one operand is the comparison complement of the other.
///
/// `negate` flips `<`/`>=`, `<=`/`>`, `==`/`!=` exactly over the integers, so
/// the two comparisons together cover every value of the left operand.
fn is_complement(l: &Expr, r: &Expr) -> bool {
    negate(l) == *r || negate(r) == *l
}

/// True for `a == true` paired with `a == false` on the same operand.
///
/// Sound without type information: in a well-typed program only a Bool compares
/// equal to a Bool literal, and Bool has exactly these two values. Deliberately
/// narrow — `a == 0 || a == 1` is NOT exhaustive for an Int and must stay
/// undecided.
fn is_bool_exhaustive(l: &Expr, r: &Expr) -> bool {
    let (Expr::BinaryOp(lop, ll, lr), Expr::BinaryOp(rop, rl, rr)) = (l, r) else {
        return false;
    };
    if *lop != BinaryOpKind::Eq || *rop != BinaryOpKind::Eq || ll != rl {
        return false;
    }
    matches!(
        (&**lr, &**rr),
        (Expr::Bool(true), Expr::Bool(false)) | (Expr::Bool(false), Expr::Bool(true))
    )
}

/// Kleene negation.
fn not_verdict(v: ProofVerdict) -> ProofVerdict {
    match v {
        ProofVerdict::Holds => ProofVerdict::Violated,
        ProofVerdict::Violated => ProofVerdict::Holds,
        ProofVerdict::Undecided => ProofVerdict::Undecided,
    }
}

/// Kleene conjunction: one refuted conjunct refutes the path.
fn combine_and(left: ProofVerdict, right: ProofVerdict) -> ProofVerdict {
    match (left, right) {
        (ProofVerdict::Violated, _) | (_, ProofVerdict::Violated) => ProofVerdict::Violated,
        (ProofVerdict::Holds, ProofVerdict::Holds) => ProofVerdict::Holds,
        _ => ProofVerdict::Undecided,
    }
}

/// Kleene disjunction: one proven disjunct proves the path.
fn combine_or(left: ProofVerdict, right: ProofVerdict) -> ProofVerdict {
    match (left, right) {
        (ProofVerdict::Holds, _) | (_, ProofVerdict::Holds) => ProofVerdict::Holds,
        (ProofVerdict::Violated, ProofVerdict::Violated) => ProofVerdict::Violated,
        _ => ProofVerdict::Undecided,
    }
}

/// Resolve a comparison between two symbolic values to a tri-state verdict.
fn rel_verdict(
    op: BinaryOpKind,
    lv: &SymbolicValue,
    rv: &SymbolicValue,
    bounds: &HashMap<String, Bound>,
) -> ProofVerdict {
    // Both concrete: a total order decides outright.
    if let (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) = (lv, rv) {
        return order_verdict(op, a.cmp(&b));
    }
    // Structurally identical values (pure terms of equal subterms): every
    // reflexive operator holds, its strict/inequality opposite is refuted.
    if structural_eq(lv, rv) {
        return match op {
            BinaryOpKind::Eq | BinaryOpKind::Le | BinaryOpKind::Ge => ProofVerdict::Holds,
            BinaryOpKind::Neq | BinaryOpKind::Lt | BinaryOpKind::Gt => ProofVerdict::Violated,
            _ => ProofVerdict::Undecided,
        };
    }
    // Concrete arithmetic result: a binary expression that evaluates to a
    // known integer (e.g. 2+3 → 5) compared against a literal.  Decides
    // immediately without routing to Z3 — the dominant case for arithmetic
    // body-postcondition obligations.
    if let Some(lit) = lv.try_to_literal() {
        if let SymbolicValue::Literal(r, _) = rv {
            return order_verdict(op, lit.cmp(r));
        }
    }
    if let Some(lit) = rv.try_to_literal() {
        if let SymbolicValue::Literal(l, _) = lv {
            return order_verdict(op, l.cmp(&lit));
        }
    }
    // Identifier-vs-literal: decide through the path's interval bounds.
    // The other side may be a folded constant term (`1 << 24`) as well as a
    // plain literal, so both spellings resolve through `try_to_literal`.
    if let SymbolicValue::Identifier(name) = lv {
        if let Some(v) = rv.try_to_literal() {
            return ident_verdict(op, name, v, false, bounds);
        }
    }
    if let Some(v) = lv.try_to_literal() {
        if let SymbolicValue::Identifier(name) = rv {
            return ident_verdict(op, name, v, true, bounds);
        }
    }
    // Masked value vs literal: `x & M` with `M >= 0` lies in `[0, M]`, which
    // decides the uint-range postconditions (`term <= MASK32`) without a solver.
    if let (Some(bound), SymbolicValue::Literal(v, _)) = (mask_bound(lv), rv) {
        return interval_verdict(op, bound, *v, false);
    }
    if let (Some(bound), SymbolicValue::Literal(v, _)) = (mask_bound(rv), lv) {
        return interval_verdict(op, bound, *v, true);
    }
    // Masked-plus-offset vs literal — the `mantissa_val` shape.
    //
    // `lv` is `(x & mask) + n` and `rv` is a literal `v`. Because
    // `x & mask ∈ [0, mask]` for a non-negative mask, the whole expression
    // lies in `[n, mask + n]` whenever `n >= 0`. That interval decides the
    // comparison without a solver:
    //   `Lt` holds when `mask + n < v`, refutes when `mask < v`.
    //   `Le` holds when `mask + n <= v`, refutes when `mask > v`.
    //   `Ge`/`Gt` are the mirrored lower-bound checks.
    //
    // Worked example (`mantissa_val`):
    //   term = mant + 0x800000,  v = 1 << 24,  mant = bits & 0x7FFFFF
    //   0x7FFFFF + 0x800000 = 16777215 < 16777216  ⇒  Holds.
    if let SymbolicValue::Binary(add_op, inner_box, lit_sym_box) = lv {
        if add_op.as_str() == "+" {
            if let Some(offset) = lit_sym_box.try_to_literal() {
                if offset >= 0 {
                    if let Some(inner_bound) = value_bound(&inner_box, bounds) {
                        // `rv` may be a literal or a foldable constant term such
                        // as `1 << 24`; both denote an exact bound.
                        if let Some(limit) = rv.try_to_literal() {
                            let combined_max = inner_bound.upper.saturating_add(offset);
                            // The reachable interval is [offset, combined_max].
                            // Holds needs the whole interval on the true side,
                            // refuted needs the whole interval on the false side,
                            // and anything else stays undecided.
                            match op {
                                BinaryOpKind::Lt => {
                                    return if combined_max < limit {
                                        ProofVerdict::Holds
                                    } else if inner_bound.lower.saturating_add(offset) >= limit {
                                        ProofVerdict::Violated
                                    } else {
                                        ProofVerdict::Undecided
                                    };
                                }
                                BinaryOpKind::Le => {
                                    return if combined_max <= limit {
                                        ProofVerdict::Holds
                                    } else if inner_bound.lower.saturating_add(offset) > limit {
                                        ProofVerdict::Violated
                                    } else {
                                        ProofVerdict::Undecided
                                    };
                                }
                                BinaryOpKind::Ge => {
                                    return if inner_bound.lower.saturating_add(offset) >= limit {
                                        ProofVerdict::Holds
                                    } else if combined_max < limit {
                                        ProofVerdict::Violated
                                    } else {
                                        ProofVerdict::Undecided
                                    };
                                }
                                BinaryOpKind::Gt => {
                                    return if inner_bound.lower.saturating_add(offset) > limit {
                                        ProofVerdict::Holds
                                    } else if combined_max <= limit {
                                        ProofVerdict::Violated
                                    } else {
                                        ProofVerdict::Undecided
                                    };
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
    ProofVerdict::Undecided
}

/// The best interval known for one symbolic operand.
///
/// Two sources contribute, and both are needed by the masked-offset rule:
/// a mask expression (`x & MASK`) carries its own interval, while an ordinary
/// identifier carries whatever the path's precondition and guards established.
/// Returning `None` (rather than `Bound::FULL`) keeps the caller from treating
/// "no information" as "the whole range is decided".
fn value_bound(value: &SymbolicValue, bounds: &HashMap<String, Bound>) -> Option<Bound> {
    if let Some(masked) = mask_bound(value) {
        return Some(masked);
    }
    match value {
        SymbolicValue::Literal(n, _) => Some(Bound {
            lower: *n,
            upper: *n,
        }),
        SymbolicValue::Identifier(name) => bounds.get(name).copied(),
        _ => None,
    }
}

/// The interval of a value masked by a non-negative constant.
///
/// `x & M` with `M >= 0` lies in `[0, M]`: the AND can never set a bit above
/// the mask's highest set bit, and a non-negative mask keeps the sign bit clear.
/// This is the general rule behind every `term <= MASK32` obligation rather than
/// a rule for one function.
///
/// Deliberately narrow: a negative mask is not a range (`x & -1 == x`), so it
/// contributes nothing instead of a bound that could be wrong.
fn mask_bound(value: &SymbolicValue) -> Option<Bound> {
    let SymbolicValue::Binary(op, left, right) = value else {
        return None;
    };
    if op != "&" {
        return None;
    }
    let mask = literal_value(left).or_else(|| literal_value(right))?;
    if mask < 0 {
        return None;
    }
    Some(Bound {
        lower: 0,
        upper: mask,
    })
}

/// The integer behind a literal symbolic value.
fn literal_value(value: &SymbolicValue) -> Option<i64> {
    match value {
        SymbolicValue::Literal(inner, _) => Some(*inner),
        _ => None,
    }
}

/// Fold mask-derived intervals for assigned locals into the path's bounds.
///
/// `let lo: Int = x & MASK32` binds `lo` to a mask expression; recording its
/// interval lets a later `lo <= MASK32` resolve through `ident_verdict`.
fn add_mask_bounds(state: &SymbolicState, bounds: &mut HashMap<String, Bound>) {
    for (name, value) in &state.assignments {
        let Some(masked) = mask_bound(value) else {
            continue;
        };
        let entry = bounds.entry(name.clone()).or_insert(Bound::FULL);
        *entry = entry.intersect(masked);
    }
}

/// Decide `lit <op> x` (flipped) or `x <op> lit` for a bounded identifier.
fn ident_verdict(
    op: BinaryOpKind,
    name: &str,
    lit: i64,
    flipped: bool,
    bounds: &HashMap<String, Bound>,
) -> ProofVerdict {
    let bound = bounds.get(name).copied().unwrap_or(Bound::FULL);
    interval_verdict(op, bound, lit, flipped)
}

/// Decide a comparison against a literal from a known interval.
///
/// `None` from `classify` means the interval straddles the literal, which is a
/// genuine undecided — never reported as holding.
fn interval_verdict(op: BinaryOpKind, bound: Bound, lit: i64, flipped: bool) -> ProofVerdict {
    let Some(relation) = Relation::from_ast(op) else {
        return ProofVerdict::Undecided;
    };
    let relation = if flipped { relation.mirror() } else { relation };
    match bound.classify(relation, lit) {
        Some(value) => tri(value),
        None => ProofVerdict::Undecided,
    }
}

/// Map a boolean claim to its verdict.
fn tri(holds: bool) -> ProofVerdict {
    if holds {
        ProofVerdict::Holds
    } else {
        ProofVerdict::Violated
    }
}

/// Decide a comparison between two concrete literals.
fn order_verdict(op: BinaryOpKind, ord: std::cmp::Ordering) -> ProofVerdict {
    use std::cmp::Ordering as O;
    let holds = match op {
        BinaryOpKind::Eq => ord == O::Equal,
        BinaryOpKind::Neq => ord != O::Equal,
        BinaryOpKind::Lt => ord == O::Less,
        BinaryOpKind::Le => ord != O::Greater,
        BinaryOpKind::Gt => ord == O::Greater,
        BinaryOpKind::Ge => ord != O::Less,
        _ => return ProofVerdict::Undecided,
    };
    if holds {
        ProofVerdict::Holds
    } else {
        ProofVerdict::Violated
    }
}

/// Reduce a symbolic value through one pass of algebraic simplification.
///
/// Applies `simplify_binary` to `Binary` nodes; returns the simplified value.
/// Handles the common cases that would otherwise route to Z3:
///   a + 0 → a,  a - 0 → a,  a * 1 → a,  a * 0 → 0
///   (a+b) − b → a,  2 + 3 → 5
///
/// Single pass (no recursion) — keeps O(1) complexity; nested reductions
/// like (a+0)+1 are handled by `eval_symbolic`'s bottom-up construction.
fn simplifies_to(sv: &SymbolicValue) -> SymbolicValue {
    let SymbolicValue::Binary(op, l, r) = sv else {
        return sv.clone();
    };
    let Some(simplified) = simplify_binary(op.as_str(), l, r) else {
        return sv.clone();
    };
    simplified
}

/// Structural equality of two symbolic values — proves `x + 1 == x + 1`
/// without a concrete numeric value. Unknown never equals anything (two
/// unanalysable values are NOT provably identical).  Both sides are first
/// reduced through `simplifies_to` so algebraically equal terms match even
/// when their surface forms differ.
fn structural_eq(lv: &SymbolicValue, rv: &SymbolicValue) -> bool {
    // Algebraic simplification: if both sides reduce to the same canonical form
    // they are equal even when surface syntax differs (e.g. (a+0) vs a).
    let sl = simplifies_to(lv);
    let sr = simplifies_to(rv);
    match (sl, sr) {
        (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) => a == b,
        (SymbolicValue::Identifier(a), SymbolicValue::Identifier(b)) => a == b,
        (SymbolicValue::Previous(a), SymbolicValue::Previous(b)) => a == b,
        (SymbolicValue::Binary(a, al, ar), SymbolicValue::Binary(b, bl, br)) => {
            a == b && structural_eq(&*al, &*bl) && structural_eq(&*ar, &*br)
        }
        _ => false,
    }
}

/// Terse counterexample for the diagnostic: the captured result value.
fn render_counterexample(state: &SymbolicState) -> String {
    match state.assignments.get("term") {
        Some(SymbolicValue::Literal(v, _)) => {
            format!("a path returns term = {v}, which contradicts the contract")
        }
        _ => "a path produces a value that contradicts the contract".to_string(),
    }
}

/// Names a statement list writes.
fn assigned_names(stmts: &[Statement]) -> Vec<String> {
    let mut out = Vec::new();
    collect_assigned(stmts, &mut out);
    out
}

/// Collect assignment and binding targets, descending into guards.
fn collect_assigned(stmts: &[Statement], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Statement::Assign(Expr::Identifier(name), _) => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            Statement::Guarded(_, inner) => collect_assigned(inner, out),
            Statement::Let { name, .. } => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            _ => {}
        }
    }
}

/// Guess an inductive invariant for a loop, or `None` when nothing survives.
///
/// Houdini-style: propose candidates drawn from the code, then keep only those
/// that are inductive on their own — established by the precondition at entry
/// and preserved by one pass. Filtering each candidate independently is
/// complete-but-conservative: a conjunct that is only inductive *alongside*
/// another is dropped. That is the standard trade, and it keeps the procedure
/// linear in the candidate count.
fn infer_invariant(
    contract: &Contract,
    var: &str,
    lower: &Expr,
    bound: &Expr,
    inclusive: bool,
    loop_body: &[Statement],
    prefix: &[Statement],
) -> Option<Expr> {
    let loop_var = Expr::Identifier(var.to_string());
    let zero = Expr::Decimal(0);

    // Candidates come from the code that exists: the variables the loop writes,
    // their bindings before the loop, and the loop variable's own range.
    let mut vars = assigned_names(loop_body);
    for stmt in prefix {
        let Statement::Let { name, .. } = stmt else {
            continue;
        };
        if !vars.contains(name) {
            vars.push(name.clone());
        }
    }
    let mut candidates: Vec<Expr> = Vec::new();
    for name in &vars {
        let value = Expr::Identifier(name.clone());
        candidates.push(binop(BinaryOpKind::Eq, value.clone(), loop_var.clone()));
        candidates.push(binop(BinaryOpKind::Ge, value.clone(), zero.clone()));
        candidates.push(binop(BinaryOpKind::Ge, value.clone(), lower.clone()));
        candidates.push(binop(BinaryOpKind::Le, value, bound.clone()));
    }
    candidates.push(binop(BinaryOpKind::Le, loop_var.clone(), bound.clone()));
    candidates.push(binop(BinaryOpKind::Ge, loop_var.clone(), lower.clone()));

    let within = if inclusive {
        binop(BinaryOpKind::Le, loop_var.clone(), bound.clone())
    } else {
        binop(BinaryOpKind::Lt, loop_var.clone(), bound.clone())
    };

    let survivors: Vec<Expr> = candidates
        .into_iter()
        .filter(|candidate| {
            // Established at entry. The pre-loop bindings are RUN so that a
            // candidate like `acc == i` sees `acc = 0`; the loop variable is
            // pinned to the lower bound, as the entry obligation does.
            let mut entry_body = prefix.to_vec();
            entry_body.push(Statement::Term(Some(zero.clone())));
            let entry_pre = conjunction(vec![
                contract.pre_condition.clone(),
                binop(BinaryOpKind::Eq, loop_var.clone(), lower.clone()),
            ]);
            let entry = matches!(
                verify_one(&entry_pre, candidate, &entry_body),
                BodyOutcome::Proven
            );
            if !entry {
                return false;
            }
            // Preserved by one pass.
            let mut step = loop_body.to_vec();
            step.push(Statement::Assign(
                loop_var.clone(),
                binop(BinaryOpKind::Add, loop_var.clone(), Expr::Decimal(1)),
            ));
            step.push(Statement::Term(Some(zero.clone())));
            let preserve = conjunction(vec![
                contract.pre_condition.clone(),
                candidate.clone(),
                binop(BinaryOpKind::Ge, loop_var.clone(), lower.clone()),
                within.clone(),
            ]);
            matches!(verify_one(&preserve, candidate, &step), BodyOutcome::Proven)
        })
        .collect();

    if survivors.is_empty() {
        None
    } else {
        Some(conjunction(survivors))
    }
}

/// Prove an unannotated loop by inferring its invariant first.
fn inferred_loop_obligations(
    name: &str,
    contract: &Contract,
    body: &[Statement],
) -> Option<Vec<(String, Expr, Expr, Vec<Statement>)>> {
    let mut patched: Vec<Statement> = body.to_vec();
    let mut inferred = false;
    for at in 0..patched.len() {
        let (var, lower, bound, inclusive, loop_body) = match &patched[at] {
            Statement::Foreach {
                item,
                list,
                body: inner,
                modifiers,
            } => {
                if modifiers
                    .iter()
                    .any(|annotation| annotation.name == "invariant")
                {
                    continue;
                }
                let Expr::Range {
                    start,
                    end,
                    inclusive,
                } = list.as_ref()
                else {
                    continue;
                };
                (
                    item.clone(),
                    (**start).clone(),
                    (**end).clone(),
                    *inclusive,
                    inner.clone(),
                )
            }
            _ => continue,
        };
        let Some(invariant) = infer_invariant(
            contract,
            &var,
            &lower,
            &bound,
            inclusive,
            &loop_body,
            &patched[..at],
        ) else {
            continue;
        };
        if let Statement::Foreach { modifiers, .. } = &mut patched[at] {
            modifiers.push(Annotation {
                name: "invariant".to_string(),
                value: Some(invariant),
            });
            inferred = true;
        }
    }
    if !inferred {
        return None;
    }
    loop_obligations(name, contract, &patched)
}

/// The negation of a condition, preferring a flipped comparison.
///
/// A flipped comparison is a numeric fact the bounds reasoning can use;
/// `!(a > 5)` is not.
fn negate(condition: &Expr) -> Expr {
    if let Expr::BinaryOp(op, left, right) = condition {
        let flipped = match op {
            BinaryOpKind::Lt => Some(BinaryOpKind::Ge),
            BinaryOpKind::Le => Some(BinaryOpKind::Gt),
            BinaryOpKind::Gt => Some(BinaryOpKind::Le),
            BinaryOpKind::Ge => Some(BinaryOpKind::Lt),
            BinaryOpKind::Eq => Some(BinaryOpKind::Neq),
            BinaryOpKind::Neq => Some(BinaryOpKind::Eq),
            _ => None,
        };
        if let Some(kind) = flipped {
            return binop(kind, (**left).clone(), (**right).clone());
        }
    }
    Expr::UnaryOp(UnaryOpKind::Not, Box::new(condition.clone()))
}

/// The obligations of a convergent body (`node` / `txn`) carrying an invariant.
///
/// There is no index range: the body runs until the postcondition holds, so the
/// guard of a pass is the *negation of the postcondition* and there is no exit
/// obligation to state — the loop leaves precisely because `post` holds, and `I`
/// is then a free fact about the exit state.
///
///   init       pre        =>  I
///   preserve   pre && I && !post  =>  I   after one pass
///
/// Progress — that the body actually drives `post` towards truth — needs a
/// variant and is not claimed here.
fn convergent_obligations(
    item: &TopLevel,
    name: &str,
    contract: &Contract,
    body: &[Statement],
) -> Option<Vec<(String, Expr, Expr, Vec<Statement>)>> {
    let TopLevel::Transaction(txn) = item else {
        return None;
    };
    let invariant = txn
        .modifiers
        .iter()
        .find(|annotation| annotation.name == "invariant")?
        .value
        .clone()?;
    let zero = Expr::Decimal(0);

    let init = (
        format!("{name}_init"),
        contract.pre_condition.clone(),
        invariant.clone(),
        vec![Statement::Term(Some(zero.clone()))],
    );

    let preserve = vec![
        contract.pre_condition.clone(),
        invariant.clone(),
        negate(&contract.post_condition),
    ];
    let mut preserve_body = body.to_vec();
    preserve_body.push(Statement::Term(Some(zero)));
    let keep = (
        format!("{name}_preserve"),
        conjunction(preserve),
        invariant.clone(),
        preserve_body,
    );

    // A `variant` adds the two obligations that turn the loop's progress — and
    // therefore its termination — into a proof instead of a shape check.
    let measure = txn
        .modifiers
        .iter()
        .find(|annotation| annotation.name == "variant")
        .and_then(|annotation| annotation.value.clone());

    if let Some(measure) = measure {
        let zero = Expr::Decimal(0);
        let active = conjunction(vec![
            contract.pre_condition.clone(),
            invariant.clone(),
            negate(&contract.post_condition),
        ]);
        // Bounded below: the measure cannot fall for ever.
        let bounded = (
            format!("{name}_variant_bound"),
            active.clone(),
            binop(BinaryOpKind::Ge, measure.clone(), zero.clone()),
            vec![Statement::Term(Some(zero.clone()))],
        );
        // Strictly decreasing: the entry value is snapshotted into a name, and
        // the postcondition — evaluated in the FINAL state — compares against
        // that snapshot.
        let snapshot = Expr::Identifier("__variant0".to_string());
        let step_pre = conjunction(vec![
            active,
            binop(BinaryOpKind::Eq, snapshot.clone(), measure.clone()),
        ]);
        let mut step_body = body.to_vec();
        step_body.push(Statement::Term(Some(zero)));
        let step = (
            format!("{name}_variant_step"),
            step_pre,
            binop(BinaryOpKind::Lt, measure, snapshot),
            step_body,
        );
        return Some(vec![init, keep, bounded, step]);
    }

    Some(vec![init, keep])
}

/// `left op right`.
fn binop(kind: BinaryOpKind, left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp(kind, Box::new(left), Box::new(right))
}

/// Conjoin the facts, right-associated. `true` when there are none.
fn conjunction(parts: Vec<Expr>) -> Expr {
    let mut iter = parts.into_iter().rev();
    let Some(mut combined) = iter.next() else {
        return Expr::Bool(true);
    };
    for part in iter {
        combined = binop(BinaryOpKind::And, part, combined);
    }
    combined
}

/// The three Hoare obligations for a loop carrying an `invariant` annotation.
///
/// Returns `None` when the body has no annotated loop, leaving the ordinary
/// straight-line obligation in charge. All three obligations are loop-free:
///
///   VC1  pre && locals = initialisers && i = lo   =>  I
///   VC2  pre && I && i in range                   =>  I   after the body and i+1
///   VC3  pre && I && i past the end               =>  post
fn loop_obligations(
    name: &str,
    contract: &Contract,
    body: &[Statement],
) -> Option<Vec<(String, Expr, Expr, Vec<Statement>)>> {
    let (at, var, lower, bound, inclusive, loop_body, invariant) =
        body.iter().enumerate().find_map(|(at, stmt)| {
            let Statement::Foreach {
                item,
                list,
                body: inner,
                modifiers,
            } = stmt
            else {
                return None;
            };
            let invariant = modifiers
                .iter()
                .find(|annotation| annotation.name == "invariant")?
                .value
                .clone()?;
            let Expr::Range {
                start,
                end,
                inclusive,
            } = list.as_ref()
            else {
                return None;
            };
            Some((
                at,
                item.clone(),
                (**start).clone(),
                (**end).clone(),
                *inclusive,
                inner.as_slice(),
                invariant,
            ))
        })?;

    // The function's value is whatever follows the loop.
    let exit_body: Vec<Statement> = body.iter().skip(at + 1).cloned().collect();
    if exit_body.is_empty() {
        return None;
    }
    let loop_var = Expr::Identifier(var);
    let zero = Expr::Decimal(0);

    // VC1 — the locals the loop writes are pinned to the values they enter with.
    let mut init = vec![contract.pre_condition.clone()];
    for stmt in &body[..at] {
        let Statement::Let {
            name: local,
            names,
            expr: Some(value),
            ..
        } = stmt
        else {
            continue;
        };
        if names.len() <= 1 {
            init.push(binop(
                BinaryOpKind::Eq,
                Expr::Identifier(local.clone()),
                value.clone(),
            ));
        }
    }
    init.push(binop(BinaryOpKind::Eq, loop_var.clone(), lower.clone()));

    // VC2 — the body preserves it, with the guard and the precondition available.
    let within = if inclusive {
        binop(BinaryOpKind::Le, loop_var.clone(), bound.clone())
    } else {
        binop(BinaryOpKind::Lt, loop_var.clone(), bound.clone())
    };
    let preserve = vec![
        contract.pre_condition.clone(),
        invariant.clone(),
        binop(BinaryOpKind::Ge, loop_var.clone(), lower),
        within,
    ];
    let mut preserve_body = loop_body.to_vec();
    preserve_body.push(Statement::Assign(
        loop_var.clone(),
        binop(BinaryOpKind::Add, loop_var.clone(), Expr::Decimal(1)),
    ));
    preserve_body.push(Statement::Term(Some(zero.clone())));

    // VC3 — the invariant at exit is enough for the contract.
    let past = if inclusive {
        binop(BinaryOpKind::Gt, loop_var, bound)
    } else {
        binop(BinaryOpKind::Ge, loop_var, bound)
    };
    let exit = vec![contract.pre_condition.clone(), invariant.clone(), past];

    Some(vec![
        (
            format!("{name}_init"),
            conjunction(init),
            invariant.clone(),
            vec![Statement::Term(Some(zero))],
        ),
        (
            format!("{name}_preserve"),
            conjunction(preserve),
            invariant.clone(),
            preserve_body,
        ),
        (
            format!("{name}_exit"),
            conjunction(exit),
            contract.post_condition.clone(),
            exit_body,
        ),
    ])
}

/// Whether the entire body is within the supported pure-integer surface.
fn body_supported(body: &[Statement], sums: &HashMap<String, Summary>) -> bool {
    unsupported_reason(body, sums).is_none()
}

/// The first construct that puts a body outside the prover's reach, named.
///
/// The boolean this replaces said only "not supported", which tells an author
/// nothing and makes a census impossible. Naming the construct makes the skip
/// actionable: a reader learns what to avoid, and extending the prover can be
/// aimed at whatever blocks the most bodies.
fn unsupported_reason(stmts: &[Statement], sums: &HashMap<String, Summary>) -> Option<String> {
    stmts.iter().find_map(|stmt| statement_reason(stmt, sums))
}

/// Whether an assignment target is in the prover's reach: a plain name
/// (`x`), or a field of a plain name (`x.n`).  Index bases (`arr[i].n`) and
/// any deeper projection stay out of reach — the field map keys on the base
/// identifier and cannot track which element of a collection is written.
fn plain_assignment_target(lhs: &Expr) -> bool {
    match lhs {
        Expr::Identifier(_) => true,
        Expr::Field(base, _) => matches!(base.as_ref(), Expr::Identifier(_)),
        _ => false,
    }
}

/// The reason one statement is unsupported, if it is.
fn statement_reason(stmt: &Statement, sums: &HashMap<String, Summary>) -> Option<String> {
    match stmt {
        Statement::Assign(lhs, e) => {
            if !plain_assignment_target(lhs) {
                return Some(
                    "an assignment whose target is not a plain name or a field of a plain name"
                        .to_string(),
                );
            }
            expr_reason(e, sums)
        }
        Statement::Let { names, expr, .. } => {
            // A plain `let x = e;` carries exactly one name — the parser fills
            // `names` with it. Only a multi-target binding is out of reach.
            if names.len() > 1 {
                return Some("a `let` binding more than one name".to_string());
            }
            expr.as_ref().and_then(|e| expr_reason(e, sums))
        }
        Statement::Guarded(c, inner) => {
            expr_reason(c, sums).or_else(|| unsupported_reason(inner, sums))
        }
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) => {
            expr_reason(e, sums)
        }
        // Paths that terminate without a value are always modelable.
        Statement::Term(None) | Statement::EndProgram(None) => None,
        Statement::Gate(c) | Statement::Check(c) => expr_reason(c, sums),
        Statement::Expression(e) => expr_reason(e, sums),
        Statement::Rollback(_) | Statement::Halt | Statement::Trap => None,
        Statement::Foreach { .. } => Some("a `foreach` loop".to_string()),
        Statement::Match { .. } => Some("a `match` statement".to_string()),
        Statement::Break => Some("`break`".to_string()),
        Statement::Yield => Some("`yield`".to_string()),
        Statement::Expect(_) => Some("`expect`".to_string()),
        Statement::MetadataAssignment(..) => Some("a metadata assignment".to_string()),
        Statement::SyncBlock(_) => Some("a sync block".to_string()),
        Statement::Defer(_) => Some("a `defer` block".to_string()),
        Statement::Mutex(_) => Some("a `mutex` block".to_string()),
        Statement::TrgBinding { .. } => Some("a trigger binding".to_string()),
        Statement::InlineAsm { .. } => Some("inline asm".to_string()),
        Statement::Block(_) => Some("a nested block".to_string()),
        _ => Some("an unsupported statement".to_string()),
    }
}

/// The reason one expression is unsupported, if it is.
///
/// A call is supported **only** when the callee has a value summary, so the
/// prover never claims a proof over a value it cannot evaluate.
fn expr_reason(expr: &Expr, sums: &HashMap<String, Summary>) -> Option<String> {
    if let Some(reason) = crate::domain::expression_node_reason(expr) {
        return Some(reason);
    }
    match expr {
        Expr::Decimal(_)
        | Expr::Char(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::BeginProgram
        | Expr::UnitLiteral { .. }
        | Expr::Identifier(_)
        | Expr::Wildcard => None,
        Expr::BinaryOp(_, l, r) => expr_reason(l, sums).or_else(|| expr_reason(r, sums)),
        Expr::UnaryOp(_, x) => expr_reason(x, sums),
        Expr::AddrOf(x) | Expr::Consume(x) => expr_reason(x, sums),
        Expr::Call(name, args, _) => {
            // A `Load#` with a modellable literal width is admitted: its value
            // is a zero-extended word, so the prover can bound it. Everything
            // else still needs a summary (or stays out of reach).
            let modelled_load = name == "Load#" && crate::symbolic::load_width(args).is_some();
            if !modelled_load && !sums.contains_key(name) {
                return Some(format!("a call to `{name}`, which has no value summary"));
            }
            args.iter().find_map(|a| expr_reason(a, sums))
        }
        Expr::Field(..) => Some("field access".to_string()),
        Expr::Index(..) => Some("indexing".to_string()),
        Expr::MethodCall(..) => Some("a method call".to_string()),
        Expr::List(_) | Expr::Tuple(_) => Some("a list or tuple literal".to_string()),
        Expr::Reflect(..) => Some("reflection".to_string()),
        _ => Some("an unsupported expression".to_string()),
    }
}

/// A function's returned value, as an expression over its parameters.
///
/// Only unconditional single-value bodies get one: a value that depends on a
/// guard has no single expression to substitute, and guessing one would turn an
/// unproved contract into a false proof.
#[derive(Debug, Clone)]
struct Summary {
    params: Vec<String>,
    value: Expr,
}

/// Build value summaries for pure-integer definitions, closing over callees.
///
/// Iterates to a fixpoint: a body calling a function with no summary yet is
/// retried next round. Recursion therefore never acquires a summary — the sound
/// answer, since a self-referential summary would be a guess rather than a
/// proof.
fn build_summaries(
    items: &[TopLevel],
    resolve: &crate::domain::TypeResolver<'_>,
    globals: &std::collections::HashSet<String>,
) -> HashMap<String, Summary> {
    let candidates: Vec<(&str, Vec<String>, &[Statement])> = items
        .iter()
        .filter(|item| crate::domain::unsupported_declaration(item, resolve, globals).is_none())
        .filter_map(summary_candidate)
        .collect();
    let mut sums: HashMap<String, Summary> = HashMap::new();
    // Each round adds a summary or ends the loop, so this terminates.
    for _ in 0..=candidates.len() {
        let mut added = false;
        for (name, params, body) in &candidates {
            if sums.contains_key(*name) || !body_supported(body, &sums) {
                continue;
            }
            let Ok(flattened) = inline_body(body, &sums) else {
                continue;
            };
            let Ok(value) = evaluate_body(&flattened) else {
                continue;
            };
            if !mentions_only(&value, params) {
                continue;
            }
            sums.insert(
                (*name).to_string(),
                Summary {
                    params: params.clone(),
                    value,
                },
            );
            added = true;
        }
        if !added {
            break;
        }
    }
    sums
}

/// The (name, parameters, body) a summary could be built from.
fn summary_candidate(item: &TopLevel) -> Option<(&str, Vec<String>, &[Statement])> {
    match item {
        TopLevel::Definition(d) => Some((
            &d.name,
            d.parameters.iter().map(|(n, _)| n.clone()).collect(),
            d.body.as_slice(),
        )),
        TopLevel::Transaction(t) => Some((
            &t.name,
            t.parameters.iter().map(|(n, _)| n.clone()).collect(),
            t.body.as_slice(),
        )),
        _ => None,
    }
}

/// Evaluate a call-free, guard-free body to the expression it returns.
///
/// Parameters stay unbound so `eval_symbolic` captures their immutable entry
/// atoms. Returning a saved value must not resolve those atoms through the
/// callee's final assignments (which may have overwritten its parameters).
fn evaluate_body(body: &[Statement]) -> Result<Expr, ()> {
    let mut state = SymbolicState::empty();
    for stmt in body {
        match stmt {
            Statement::Assign(lhs, e) => {
                let Expr::Identifier(name) = lhs else {
                    return Err(());
                };
                state
                    .assignments
                    .insert(name.clone(), eval_symbolic(e, &state));
            }
            Statement::Let {
                name, names, expr, ..
            } => {
                // A plain `let x = e;` carries one name; only a multi-target
                // binding stops the body from having a single value. Requiring
                // an empty list silently denied a summary to every callee whose
                // body contained an ordinary `let`.
                if names.len() > 1 {
                    return Err(());
                }
                if let Some(e) = expr {
                    state
                        .assignments
                        .insert(name.clone(), eval_symbolic(e, &state));
                }
            }
            Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) => {
                return crate::symbolic::snapshot::value_expr(&eval_symbolic(e, &state)).ok_or(());
            }
            Statement::Gate(_) | Statement::Check(_) | Statement::Expression(_) => {}
            // Guards pick between values; a non-value exit has no result.
            _ => return Err(()),
        }
    }
    Err(())
}

/// Replace every summarised call in a body with the callee's value form.
fn inline_body(body: &[Statement], sums: &HashMap<String, Summary>) -> Result<Vec<Statement>, ()> {
    body.iter().map(|stmt| inline_stmt(stmt, sums)).collect()
}

/// Replace summarised calls in one statement.
fn inline_stmt(stmt: &Statement, sums: &HashMap<String, Summary>) -> Result<Statement, ()> {
    Ok(match stmt {
        Statement::Assign(lhs, e) => Statement::Assign(lhs.clone(), inline_expr(e, sums, 0)?),
        Statement::Let {
            name,
            names,
            ty,
            expr,
            modifiers,
        } => Statement::Let {
            name: name.clone(),
            names: names.clone(),
            ty: ty.clone(),
            expr: match expr {
                Some(e) => Some(inline_expr(e, sums, 0)?),
                None => None,
            },
            modifiers: modifiers.clone(),
        },
        Statement::Guarded(c, inner) => {
            Statement::Guarded(inline_expr(c, sums, 0)?, inline_body(inner, sums)?)
        }
        Statement::Term(Some(e)) => Statement::Term(Some(inline_expr(e, sums, 0)?)),
        Statement::EndProgram(Some(e)) => Statement::EndProgram(Some(inline_expr(e, sums, 0)?)),
        Statement::Tail(e) => Statement::Tail(inline_expr(e, sums, 0)?),
        Statement::Gate(c) => Statement::Gate(inline_expr(c, sums, 0)?),
        Statement::Check(c) => Statement::Check(inline_expr(c, sums, 0)?),
        Statement::Expression(e) => Statement::Expression(inline_expr(e, sums, 0)?),
        // Everything else is rejected before inlining is attempted.
        _ => return Err(()),
    })
}

/// Replace summarised calls in one expression.
fn inline_expr(expr: &Expr, sums: &HashMap<String, Summary>, depth: usize) -> Result<Expr, ()> {
    if depth > SUBST_DEPTH_CAP {
        return Err(());
    }
    Ok(match expr {
        Expr::BinaryOp(kind, l, r) => Expr::BinaryOp(
            *kind,
            Box::new(inline_expr(l, sums, depth)?),
            Box::new(inline_expr(r, sums, depth)?),
        ),
        Expr::UnaryOp(kind, x) => Expr::UnaryOp(*kind, Box::new(inline_expr(x, sums, depth)?)),
        Expr::Call(name, args, _) => {
            // A modelled intrinsic keeps its call form: it has no summary to
            // inline, but `eval_symbolic` gives it an interval directly.
            if name == "Load#" && crate::symbolic::load_width(args).is_some() {
                return Ok(expr.clone());
            }
            let Some(sum) = sums.get(name) else {
                return Err(());
            };
            if sum.params.len() != args.len() {
                return Err(());
            }
            let inlined: Vec<Expr> = args
                .iter()
                .map(|a| inline_expr(a, sums, depth + 1))
                .collect::<Result<_, _>>()?;
            substitute_params(&sum.value, &sum.params, &inlined)
        }
        other => other.clone(),
    })
}

/// Replace the callee's parameters with the caller's argument expressions.
fn substitute_params(expr: &Expr, params: &[String], args: &[Expr]) -> Expr {
    match expr {
        Expr::Identifier(name) => match params.iter().position(|p| p == name) {
            Some(index) => args[index].clone(),
            None => expr.clone(),
        },
        Expr::BinaryOp(kind, l, r) => Expr::BinaryOp(
            *kind,
            Box::new(substitute_params(l, params, args)),
            Box::new(substitute_params(r, params, args)),
        ),
        Expr::UnaryOp(kind, x) => {
            Expr::UnaryOp(*kind, Box::new(substitute_params(x, params, args)))
        }
        other => other.clone(),
    }
}

/// Whether every free identifier of `expr` is one of `allowed`.
fn mentions_only(expr: &Expr, allowed: &[String]) -> bool {
    free_idents(expr)
        .iter()
        .all(|name| allowed.iter().any(|p| p == name))
}

#[cfg(test)]
mod prover_api_tests {
    use super::verify_bodies;
    use crate::ast::{BinaryOpKind, Contract, Definition, Expr, Statement, TopLevel};

    /// Build `defn f(x) [x >= 0][term >= 0] { term x; }` without any frontend.
    #[test]
    fn a_constructed_definition_is_proved() {
        let ge = |lhs: Expr, rhs: i64| {
            Expr::BinaryOp(
                BinaryOpKind::Ge,
                Box::new(lhs),
                Box::new(Expr::Decimal(rhs)),
            )
        };
        let contract = Contract::proven(
            ge(Expr::Identifier("x".into()), 0),
            ge(Expr::Identifier("term".into()), 0),
        );
        let body = vec![Statement::Term(Some(Expr::Identifier("x".into())))];
        let item = TopLevel::Definition(Definition::prover_defn("f", contract, body));

        let report = verify_bodies(&[item]);
        assert_eq!(
            report.proven,
            vec!["f".to_string()],
            "unproven: {:?}",
            report.unproven
        );
        assert!(report.violated.is_empty());
    }

    /// A false contract built the same way is refuted, not silently accepted.
    #[test]
    fn a_constructed_false_contract_is_violated() {
        let contract = Contract::proven(
            Expr::Bool(true),
            Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Identifier("term".into())),
                Box::new(Expr::Decimal(0)),
            ),
        );
        let body = vec![Statement::Term(Some(Expr::Decimal(5)))];
        let item = TopLevel::Definition(Definition::prover_defn("g", contract, body));

        let report = verify_bodies(&[item]);
        assert_eq!(report.violated.len(), 1, "proven: {:?}", report.proven);
    }
}

#[cfg(test)]
mod structural_derivation_tests {
    use super::verify_bodies;
    use crate::ast::{BinaryOpKind, Contract, Definition, Expr, Statement, TopLevel};

    /// A field projection in a postcondition is resolved structurally from the
    /// body's field assignment, without Z3: `x.n = 5; [x.n > 0]` holds because
    /// the field map records `("x", "n") -> 5` and `5 > 0` is a concrete order.
    #[test]
    fn field_assignment_proves_field_postcondition() {
        let x_n = || Expr::Field(Box::new(Expr::Identifier("x".into())), "n".into());
        let body = vec![
            Statement::Assign(x_n(), Expr::Decimal(5)),
            Statement::Term(Some(Expr::Decimal(0))),
        ];
        let post = Expr::BinaryOp(
            BinaryOpKind::Gt,
            Box::new(x_n()),
            Box::new(Expr::Decimal(0)),
        );
        let contract = Contract::proven(Expr::Bool(true), post);
        let item = TopLevel::Definition(Definition::prover_defn("f_field", contract, body));

        let report = verify_bodies(&[item]);
        assert!(
            report.proven.iter().any(|n| n == "f_field"),
            "field postcondition should be proved structurally; unproven: {:?}; skipped: {:?}",
            report.unproven,
            report.skipped
        );
        assert!(report.violated.is_empty());
    }

    /// Reassigning the base object (`x = y`) invalidates any earlier `x.n = …`
    /// value.  The stale entry must NOT be used to prove `x.n > 0` after the
    /// reassignment, so the body must not appear as proven (the postcondition
    /// is undecidable without a value for the new `x`).
    #[test]
    fn field_reassignment_invalidates_stale_value() {
        let x_n = || Expr::Field(Box::new(Expr::Identifier("x".into())), "n".into());
        let body = vec![
            Statement::Assign(x_n(), Expr::Decimal(5)),
            Statement::Assign(Expr::Identifier("x".into()), Expr::Identifier("y".into())),
            Statement::Term(Some(Expr::Decimal(0))),
        ];
        let post = Expr::BinaryOp(
            BinaryOpKind::Gt,
            Box::new(x_n()),
            Box::new(Expr::Decimal(0)),
        );
        let contract = Contract::proven(Expr::Bool(true), post);
        let item = TopLevel::Definition(Definition::prover_defn("f_stale", contract, body));

        let report = verify_bodies(&[item]);
        assert!(
            !report.proven.iter().any(|n| n == "f_stale"),
            "stale field value was incorrectly used to prove; proven: {:?}",
            report.proven
        );
    }

    /// Additive identity: `term = a + 0` reduces to `a`, so `term == a` is
    /// structural equality, provable without Z3.
    #[test]
    fn additive_identity_proves_structural_equality() {
        let a = Expr::Identifier("a".into());
        let body = vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(a.clone()),
            Box::new(Expr::Decimal(0)),
        )))];
        let post = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Identifier("term".into())),
            Box::new(a),
        );
        let contract = Contract::proven(Expr::Bool(true), post);
        let item = TopLevel::Definition(Definition::prover_defn("f_id", contract, body));

        let report = verify_bodies(&[item]);
        assert!(
            report.proven.iter().any(|n| n == "f_id"),
            "a + 0 == a should be proved structurally; unproven: {:?}",
            report.unproven
        );
    }

    /// Zero absorption: `term = a * 0` reduces to `0`, so `term == 0` holds
    /// for any `a`, provable without Z3.
    #[test]
    fn zero_multiplication_proves_zero_result() {
        let body = vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Mul,
            Box::new(Expr::Identifier("a".into())),
            Box::new(Expr::Decimal(0)),
        )))];
        let post = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Identifier("term".into())),
            Box::new(Expr::Decimal(0)),
        );
        let contract = Contract::proven(Expr::Bool(true), post);
        let item = TopLevel::Definition(Definition::prover_defn("f_zero", contract, body));

        let report = verify_bodies(&[item]);
        assert!(
            report.proven.iter().any(|n| n == "f_zero"),
            "a * 0 == 0 should be proved structurally; unproven: {:?}",
            report.unproven
        );
    }
}

#[cfg(test)]
mod tautology_discharge_tests {
    use super::verify_bodies;
    use crate::ast::{BinaryOpKind, Contract, Definition, Expr, Statement, TopLevel};

    /// Build `defn name() [true][post] { term <body_expr>; }` and return the report.
    ///
    /// The cache guard is held because a postcondition outside the structural
    /// rules escalates to the solver, whose cache and statistics are
    /// process-global and shared with `smt::tests`.
    fn report_for(name: &str, post: Expr, body_expr: Expr) -> crate::verify::BodyReport {
        let _guard = crate::smt::cache_test_guard();
        let body = vec![Statement::Term(Some(body_expr))];
        let contract = Contract::proven(Expr::Bool(true), post);
        let item = TopLevel::Definition(Definition::prover_defn(name, contract, body));
        verify_bodies(&[item])
    }

    fn ident(name: &str) -> Expr {
        Expr::Identifier(name.into())
    }

    fn binop(op: BinaryOpKind, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(op, Box::new(l), Box::new(r))
    }

    /// `x == 0 || x != 0` is a complementary pair: every integer satisfies it,
    /// so the postcondition holds without consulting the solver.
    #[test]
    fn complementary_comparisons_are_a_tautology() {
        let post = binop(
            BinaryOpKind::Or,
            binop(BinaryOpKind::Eq, ident("x"), Expr::Decimal(0)),
            binop(BinaryOpKind::Neq, ident("x"), Expr::Decimal(0)),
        );
        let report = report_for("f_complement", post, ident("x"));
        assert!(
            report.proven.iter().any(|n| n == "f_complement"),
            "x == 0 || x != 0 must be a tautology; unproven: {:?}",
            report.unproven
        );
    }

    /// Order pairs are complementary too: `x < k || x >= k`.
    #[test]
    fn complementary_order_pair_is_a_tautology() {
        let post = binop(
            BinaryOpKind::Or,
            binop(BinaryOpKind::Lt, ident("x"), Expr::Decimal(5)),
            binop(BinaryOpKind::Ge, ident("x"), Expr::Decimal(5)),
        );
        let report = report_for("f_order", post, ident("x"));
        assert!(report.proven.iter().any(|n| n == "f_order"));
    }

    /// The same pair under `&&` can never hold, so the path is refuted —
    /// a violation, not an undecided obligation.
    #[test]
    fn complementary_conjunction_is_refuted() {
        let post = binop(
            BinaryOpKind::And,
            binop(BinaryOpKind::Eq, ident("x"), Expr::Decimal(0)),
            binop(BinaryOpKind::Neq, ident("x"), Expr::Decimal(0)),
        );
        let report = report_for("f_contra", post, ident("x"));
        assert!(
            report.violated.iter().any(|n| n.contains("f_contra")),
            "x == 0 && x != 0 must be refuted; violated: {:?}; unproven: {:?}",
            report.violated,
            report.unproven
        );
    }

    /// `b_term == true || b_term == false` covers the whole Bool domain.
    /// Mirrors `is_ws_byte`, whose term is a disjunction of comparisons.
    #[test]
    fn bool_domain_exhaustion_is_a_tautology() {
        let term_expr = || binop(BinaryOpKind::Eq, ident("x"), Expr::Decimal(0));
        let post = binop(
            BinaryOpKind::Or,
            binop(BinaryOpKind::Eq, term_expr(), Expr::Bool(true)),
            binop(BinaryOpKind::Eq, term_expr(), Expr::Bool(false)),
        );
        let report = report_for("f_bool", post, term_expr());
        assert!(
            report.proven.iter().any(|n| n == "f_bool"),
            "(x == 0) == true || (x == 0) == false must be a tautology; unproven: {:?}",
            report.unproven
        );
    }

    /// Over-discharge guard: a repeated comparison is NOT exhaustive, so it
    /// must stay undecided rather than being reported as proved.
    #[test]
    fn repeated_comparison_is_not_a_tautology() {
        let term_expr = || binop(BinaryOpKind::Eq, ident("x"), Expr::Decimal(0));
        let post = binop(
            BinaryOpKind::Or,
            binop(BinaryOpKind::Eq, term_expr(), Expr::Bool(true)),
            binop(BinaryOpKind::Eq, term_expr(), Expr::Bool(true)),
        );
        let report = report_for("f_repeat", post, term_expr());
        assert!(
            !report.proven.iter().any(|n| n == "f_repeat"),
            "a repeated comparison is not exhaustive and must not be proved"
        );
    }

    /// Over-discharge guard: two distinct integer constants do NOT exhaust the
    /// Int domain, so `x == 0 || x == 1` must stay undecided.
    #[test]
    fn integer_domain_is_not_exhausted_by_two_constants() {
        let post = binop(
            BinaryOpKind::Or,
            binop(BinaryOpKind::Eq, ident("x"), Expr::Decimal(0)),
            binop(BinaryOpKind::Eq, ident("x"), Expr::Decimal(1)),
        );
        let report = report_for("f_int", post, ident("x"));
        assert!(
            !report.proven.iter().any(|n| n == "f_int"),
            "x == 0 || x == 1 must not be treated as exhaustive"
        );
        assert!(!report.violated.iter().any(|n| n.contains("f_int")));
    }
}

#[cfg(test)]
mod mask_bound_tests {
    use super::verify_bodies;
    use crate::ast::{BinaryOpKind, Contract, Definition, Expr, Statement, TopLevel};

    fn report_for(name: &str, post: Expr, body: Vec<Statement>) -> crate::verify::BodyReport {
        let _guard = crate::smt::cache_test_guard();
        let contract = Contract::proven(Expr::Bool(true), post);
        let item = TopLevel::Definition(Definition::prover_defn(name, contract, body));
        verify_bodies(&[item])
    }

    fn ident(name: &str) -> Expr {
        Expr::Identifier(name.into())
    }

    fn masked(mask: i64) -> Expr {
        Expr::BinaryOp(
            BinaryOpKind::BitAnd,
            Box::new(ident("x")),
            Box::new(Expr::Decimal(mask)),
        )
    }

    fn cmp(op: BinaryOpKind, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(op, Box::new(l), Box::new(r))
    }

    /// `x & 0xFF` is in [0, 255], so the upper bound is decided.
    ///
    /// The postcondition names `term` rather than the mask expression: a
    /// contract that itself contains `&` is rejected as ill-sorted by the word
    /// model (the same admission gate that rejected shift expressions), so
    /// writing it inline would test the gate, not the inference.
    #[test]
    fn byte_mask_is_non_negative_and_bounded() {
        let post = cmp(BinaryOpKind::Le, ident("term"), Expr::Decimal(255));
        let report = report_for(
            "f_byte_mask",
            post,
            vec![Statement::Term(Some(masked(0xFF)))],
        );
        assert!(
            report.proven.iter().any(|n| n == "f_byte_mask"),
            "x & 0xFF <= 255 must be proved; unproven: {:?}",
            report.unproven
        );
    }

    /// The 32-bit mask case (`sha1_f`, `rotl32`).
    #[test]
    fn word_mask_is_bounded_by_the_mask() {
        let post = cmp(BinaryOpKind::Le, ident("term"), Expr::Decimal(4294967295));
        let report = report_for(
            "f_word_mask",
            post,
            vec![Statement::Term(Some(masked(4294967295)))],
        );
        assert!(
            report.proven.iter().any(|n| n == "f_word_mask"),
            "x & MASK32 <= MASK32 must be proved; unproven: {:?}",
            report.unproven
        );
    }

    /// Soundness guard: a negative mask is the identity, not a range, so a
    /// small upper bound must NOT be inferred.
    #[test]
    fn negative_mask_yields_no_bound() {
        let post = cmp(BinaryOpKind::Le, ident("term"), Expr::Decimal(5));
        let report = report_for("f_neg_mask", post, vec![Statement::Term(Some(masked(-1)))]);
        assert!(
            !report.proven.iter().any(|n| n == "f_neg_mask"),
            "x & -1 must not be bounded by 5"
        );
        assert!(!report.violated.iter().any(|n| n.contains("f_neg_mask")));
    }

    /// A masked value assigned to a local carries its interval to that local.
    #[test]
    fn assigned_mask_interval_reaches_the_local() {
        let body = vec![
            Statement::Let {
                name: "lo".into(),
                names: Vec::new(),
                ty: None,
                expr: Some(masked(4294967295)),
                modifiers: Vec::new(),
            },
            Statement::Term(Some(ident("lo"))),
        ];
        let post = cmp(BinaryOpKind::Le, ident("lo"), Expr::Decimal(4294967295));
        let report = report_for("f_mask_let", post, body);
        assert!(
            report.proven.iter().any(|n| n == "f_mask_let"),
            "a masked local must carry its interval; unproven: {:?}",
            report.unproven
        );
    }
}

#[cfg(test)]
mod unknown_identity_tests {
    use super::*;
    #[test]
    fn pointer_identity_does_not_prove_unknown_trees_equal() {
        let unknown = SymbolicValue::Unknown;
        assert!(!structural_eq(&unknown, &unknown));
        let tree = SymbolicValue::Binary(
            "+".into(),
            Box::new(unknown),
            Box::new(SymbolicValue::int_literal(1)),
        );
        assert!(!structural_eq(&tree, &tree));
    }
}
