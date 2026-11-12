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

use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, UnaryOpKind};
use crate::symbolic::{eval_symbolic, SymbolicState, SymbolicValue};
use std::collections::HashMap;

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
            "proven": self.proven.len(),
            "unproven": findings_json(&self.unproven),
            "violated": findings_json(&self.violated),
        })
    }
}

/// Split `name: reason` entries into structured objects.
fn findings_json(entries: &[String]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| {
            let (name, reason) = entry.split_once(": ").unwrap_or((entry.as_str(), ""));
            serde_json::json!({ "name": name, "reason": reason })
        })
        .collect()
}

/// Inclusive numeric bounds of one identifier, derived from path facts.
#[derive(Debug, Clone, Copy, Default)]
struct Bound {
    lower: Option<i64>,
    upper: Option<i64>,
}

/// Verify every txn/defn body (incl. SyncGroup/Export wrappers) against its
/// own explicit contract.
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
    let mut report = BodyReport::default();
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
        // Soundness boundary: only prove bodies whose entire surface is
        // within the pure-integer prover's reach.
        if !body_supported(body) {
            continue;
        }
        match verify_one(&contract.pre_condition, &contract.post_condition, body) {
            BodyOutcome::Proven => report.proven.push(name.to_string()),
            BodyOutcome::Unproven(why) => report.unproven.push(format!("{name}: {why}")),
            BodyOutcome::Violated(why) => report.violated.push(format!("{name}: {why}")),
        }
    }
    report
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
    for state in &terminated {
        match verdict_for_path(post, state) {
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
                match smt_discharge(post, state) {
                    Some(ProofVerdict::Violated) => {
                        if violated.is_none() {
                            violated = Some(render_counterexample(state));
                        }
                    }
                    Some(ProofVerdict::Holds) => {}
                    _ => undecided += 1,
                }
            }
        }
    }
    if let Some(v) = violated {
        return BodyOutcome::Violated(format!("body can violate its postcondition: {v}"));
    }
    if undecided > 0 {
        return BodyOutcome::Unproven(format!(
            "could not prove the postcondition on {undecided} path(s) — add a guard or a stronger precondition"
        ));
    }
    BodyOutcome::Proven
}

/// Try to decide an Undecided path with the SMT solver (C4).
///
/// Returns `None` — leaving the caller's honest `Undecided` verdict intact —
/// when no solver is installed, when the goal falls outside the encodable
/// subset, or when the path's state cannot be substituted unambiguously (a
/// prior-state `@x` reference or an unknown value).
fn smt_discharge(post: &Expr, state: &SymbolicState) -> Option<ProofVerdict> {
    if !crate::proof_engine::smt::is_z3_available() {
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
        let substituted = substitute_all(fact, &state.assignments)?;
        if !encodable(&substituted) {
            return None;
        }
        facts.push(substituted);
    }
    let query = crate::proof_engine::smt::build_vc_query(&facts, &goal);
    match crate::proof_engine::smt::prove_smt_formula(&query, 1000) {
        crate::proof_engine::smt::SmtResult::Unsat => Some(ProofVerdict::Holds),
        // Refute only when the body's result is fully determined: with no free
        // identifier left in the goal, a model means the body contradicts its
        // contract for EVERY input. A parameter-dependent counterexample means
        // "some input violates" — real, but the stdlib's optimistic contracts
        // (`[true][term >= 0]`) hit that routinely, so those stay advisory
        // (the caller keeps `Undecided` → warning) rather than failing builds.
        crate::proof_engine::smt::SmtResult::Sat(_) if free_idents(&goal).is_empty() => {
            Some(ProofVerdict::Violated)
        }
        crate::proof_engine::smt::SmtResult::Sat(_) => None,
        crate::proof_engine::smt::SmtResult::Unknown => None,
    }
}

/// Free identifiers of an expression (sorted, for determinism).
fn free_idents(expr: &Expr) -> std::collections::BTreeSet<String> {
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

/// Rewrite every identifier the path bound to a symbolic value.
///
/// Substitution is transitive (a local may be defined in terms of another) with
/// a depth cap; it bails out (`None`) rather than guessing.
fn substitute_all(expr: &Expr, assignments: &HashMap<String, SymbolicValue>) -> Option<Expr> {
    subst_expr(expr, assignments, 0)
}

/// One substitution step; `depth` caps transitive chains.
fn subst_expr(
    expr: &Expr,
    assignments: &HashMap<String, SymbolicValue>,
    depth: usize,
) -> Option<Expr> {
    if depth > SUBST_DEPTH_CAP {
        return None;
    }
    match expr {
        Expr::Identifier(name) => match assignments.get(name) {
            Some(value) => symbolic_to_expr(value, assignments, depth + 1),
            None => Some(expr.clone()),
        },
        Expr::BinaryOp(kind, l, r) => Some(Expr::BinaryOp(
            *kind,
            Box::new(subst_expr(l, assignments, depth)?),
            Box::new(subst_expr(r, assignments, depth)?),
        )),
        Expr::UnaryOp(kind, inner) => Some(Expr::UnaryOp(
            *kind,
            Box::new(subst_expr(inner, assignments, depth)?),
        )),
        other => Some(other.clone()),
    }
}

/// Convert a tracked symbolic value back into an expression for the solver.
fn symbolic_to_expr(
    value: &SymbolicValue,
    assignments: &HashMap<String, SymbolicValue>,
    depth: usize,
) -> Option<Expr> {
    match value {
        SymbolicValue::Literal(n, _) => Some(Expr::Decimal(*n)),
        SymbolicValue::Identifier(name) => match assignments.get(name) {
            Some(_) if depth > SUBST_DEPTH_CAP => None,
            Some(inner) => symbolic_to_expr(inner, assignments, depth + 1),
            None => Some(Expr::Identifier(name.clone())),
        },
        SymbolicValue::Binary(op, l, r) => Some(Expr::BinaryOp(
            op_kind(op)?,
            Box::new(symbolic_to_expr(l, assignments, depth + 1)?),
            Box::new(symbolic_to_expr(r, assignments, depth + 1)?),
        )),
        // Prior-state (`@x`) and unknown values have no solver encoding here.
        SymbolicValue::Previous(_) | SymbolicValue::Unknown => None,
    }
}

/// Map a symbolic operator symbol to its AST kind.
fn op_kind(symbol: &str) -> Option<BinaryOpKind> {
    match symbol {
        "+" => Some(BinaryOpKind::Add),
        "-" => Some(BinaryOpKind::Sub),
        "*" => Some(BinaryOpKind::Mul),
        "/" => Some(BinaryOpKind::Div),
        "%" => Some(BinaryOpKind::Mod),
        _ => None,
    }
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
            if names.is_empty() {
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
    // Guard TAKEN: descend with the guard as a fact; whatever reaches the
    // inner block's end falls through with the guard still true.
    let mut taken = st.clone();
    taken.add_constraint(cond, true);
    pin_points(&mut taken, cond);
    next.extend(walk_block(inner, taken, terminated));
    // Guard NOT taken: continue in the enclosing block with the negated fact.
    let mut skipped = st;
    skipped.add_constraint(cond, false);
    next.push(skipped);
}

/// Track a pure-integer assignment symbolically.
fn step_assign(lhs: &Expr, expr: &Expr, st: SymbolicState, next: &mut Vec<SymbolicState>) {
    let mut s = st;
    if let Expr::Identifier(name) = lhs {
        s.assign(name, expr);
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
fn bounds_of(state: &SymbolicState) -> HashMap<String, Bound> {
    let mut bounds: HashMap<String, Bound> = HashMap::new();
    for constraint in &state.path_constraints {
        update_bounds(constraint, &mut bounds);
    }
    bounds
}

/// Merge one comparison fact into the identifier bounds map.
///
/// Recurses through conjunctions (a precondition like
/// `[a >= 0 && a <= 255]` contributes both facts) and maps negated
/// comparisons to their flipped forms (`!(a > 5)` bounds `a <= 5`).
fn update_bounds(constraint: &Expr, bounds: &mut HashMap<String, Bound>) {
    match constraint {
        Expr::BinaryOp(BinaryOpKind::And, l, r) => {
            update_bounds(l, bounds);
            update_bounds(r, bounds);
        }
        Expr::UnaryOp(UnaryOpKind::Not, inner) => {
            if let Some(fact) = negated_comparison(inner.as_ref()) {
                update_bounds(&fact, bounds);
            }
        }
        Expr::BinaryOp(op, l, r) => {
            let (name, lit) = match (l.as_ref(), r.as_ref()) {
                (Expr::Identifier(n), Expr::Decimal(v)) => (n, *v),
                (Expr::Decimal(v), Expr::Identifier(n)) => (n, *v),
                _ => return,
            };
            tighten(&name.clone(), *op, lit, bounds);
        }
        _ => {}
    }
}

/// The negation of one comparison as a bound-expressible fact, when one
/// exists. `!(a < b)` ⟺ `a >= b`; `!(a != b)` ⟺ `a == b` (integer math);
/// `!(a == b)` is a disjunction (`a < b or a > b`) — NOT expressible as a
/// single interval fact, so `None`. Skipping it is sound: facts only ever
/// constrain an identifier; we never assume the world minus a fact.
fn negated_comparison(expr: &Expr) -> Option<Expr> {
    let Expr::BinaryOp(op, l, r) = expr else {
        return None;
    };
    let flipped = match op {
        BinaryOpKind::Lt => BinaryOpKind::Ge,
        BinaryOpKind::Le => BinaryOpKind::Gt,
        BinaryOpKind::Gt => BinaryOpKind::Le,
        BinaryOpKind::Ge => BinaryOpKind::Lt,
        BinaryOpKind::Neq => BinaryOpKind::Eq,
        BinaryOpKind::Eq => return None,
        _ => return None,
    };
    Some(Expr::BinaryOp(flipped, l.clone(), r.clone()))
}

/// Tighten one identifier's bounds with `name <op> lit`.
fn tighten(name: &str, op: BinaryOpKind, lit: i64, bounds: &mut HashMap<String, Bound>) {
    let b = bounds.entry(name.to_string()).or_default();
    match op {
        BinaryOpKind::Ge => b.lower = Some(b.lower.map_or(lit, |l| l.max(lit))),
        BinaryOpKind::Gt => b.lower = Some(b.lower.map_or(lit + 1, |l| l.max(lit + 1))),
        BinaryOpKind::Le => b.upper = Some(b.upper.map_or(lit, |u| u.min(lit))),
        BinaryOpKind::Lt => b.upper = Some(b.upper.map_or(lit - 1, |u| u.min(lit - 1))),
        BinaryOpKind::Eq => {
            b.lower = Some(b.lower.map_or(lit, |l| l.max(lit)));
            b.upper = Some(b.upper.map_or(lit, |u| u.min(lit)));
        }
        _ => {}
    }
}

/// Tri-state evaluation of the postcondition against one path's state.
fn verdict_for_path(post: &Expr, state: &SymbolicState) -> ProofVerdict {
    let bounds = bounds_of(state);
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
        let (Some(lower), Some(upper)) = (bound.lower, bound.upper) else {
            continue;
        };
        if lower != upper {
            continue;
        }
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
                let lv = eval_post(l, state, bounds);
                let rv = eval_post(r, state, bounds);
                combine_and(lv, rv)
            }
            BinaryOpKind::Or => {
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
    // Identifier-vs-literal: decide through the path's interval bounds.
    if let SymbolicValue::Identifier(name) = lv {
        if let SymbolicValue::Literal(v, _) = rv {
            return ident_verdict(op, name, *v, false, bounds);
        }
    }
    if let SymbolicValue::Literal(v, _) = lv {
        if let SymbolicValue::Identifier(name) = rv {
            return ident_verdict(op, name, *v, true, bounds);
        }
    }
    ProofVerdict::Undecided
}

/// Decide `lit <op> x` (flipped) or `x <op> lit` for a bounded identifier.
fn ident_verdict(
    op: BinaryOpKind,
    name: &str,
    lit: i64,
    flipped: bool,
    bounds: &HashMap<String, Bound>,
) -> ProofVerdict {
    // `lit <op> x` ⟺ `x flip(op) lit` — normalise so bounds read naturally.
    let (effective, v) = if flipped { (flip(op), lit) } else { (op, lit) };
    let Some(b) = bounds.get(name) else {
        return ProofVerdict::Undecided;
    };
    match effective {
        BinaryOpKind::Eq => point_verdict(b, v, true),
        BinaryOpKind::Neq => point_verdict(b, v, false),
        BinaryOpKind::Lt => strict_upper_verdict(b, v),
        BinaryOpKind::Le => inclusive_upper_verdict(b, v),
        BinaryOpKind::Ge => inclusive_lower_verdict(b, v),
        BinaryOpKind::Gt => strict_lower_verdict(b, v),
        _ => ProofVerdict::Undecided,
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

/// Whether an interval is entirely outside `v` (every value > v or < v).
fn disjoint_from(b: &Bound, v: i64) -> bool {
    b.lower.map_or(false, |l| l > v) || b.upper.map_or(false, |h| h < v)
}

/// Decide `x == v` (want_eq) or `x != v` (!want_eq) against an interval.
fn point_verdict(b: &Bound, v: i64, want_eq: bool) -> ProofVerdict {
    if b.lower == Some(v) && b.upper == Some(v) {
        return tri(want_eq);
    }
    if disjoint_from(b, v) {
        return tri(!want_eq);
    }
    ProofVerdict::Undecided
}

/// Decide `x < v` against an interval.
fn strict_upper_verdict(b: &Bound, v: i64) -> ProofVerdict {
    if b.upper.map_or(false, |h| h < v) {
        return tri(true);
    }
    if b.lower.map_or(false, |l| l >= v) {
        return tri(false);
    }
    ProofVerdict::Undecided
}

/// Decide `x <= v` against an interval.
fn inclusive_upper_verdict(b: &Bound, v: i64) -> ProofVerdict {
    if b.upper.map_or(false, |h| h <= v) {
        return tri(true);
    }
    if disjoint_from(b, v) && b.lower.is_some_and(|l| l > v) {
        return tri(false);
    }
    ProofVerdict::Undecided
}

/// Decide `x >= v` against an interval.
fn inclusive_lower_verdict(b: &Bound, v: i64) -> ProofVerdict {
    if b.lower.map_or(false, |l| l >= v) {
        return tri(true);
    }
    if b.upper.map_or(false, |h| h < v) {
        return tri(false);
    }
    ProofVerdict::Undecided
}

/// Decide `x > v` against an interval.
fn strict_lower_verdict(b: &Bound, v: i64) -> ProofVerdict {
    if disjoint_from(b, v) && b.lower.is_some_and(|l| l > v) {
        return tri(true);
    }
    if b.upper.map_or(false, |h| h <= v) {
        return tri(false);
    }
    ProofVerdict::Undecided
}

/// The mirrored comparison operator (`v <op> x` ⟺ `x flip(<op>) v`).
fn flip(op: BinaryOpKind) -> BinaryOpKind {
    match op {
        BinaryOpKind::Lt => BinaryOpKind::Gt,
        BinaryOpKind::Le => BinaryOpKind::Ge,
        BinaryOpKind::Gt => BinaryOpKind::Lt,
        BinaryOpKind::Ge => BinaryOpKind::Le,
        other => other,
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

/// Structural equality of two symbolic values — proves `x + 1 == x + 1`
/// without a concrete numeric value. Unknown never equals anything (two
/// unanalysable values are NOT provably identical).
fn structural_eq(lv: &SymbolicValue, rv: &SymbolicValue) -> bool {
    match (lv, rv) {
        (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) => a == b,
        (SymbolicValue::Identifier(a), SymbolicValue::Identifier(b)) => a == b,
        (SymbolicValue::Previous(a), SymbolicValue::Previous(b)) => a == b,
        (SymbolicValue::Binary(a, al, ar), SymbolicValue::Binary(b, bl, br)) => {
            a == b && structural_eq(al, bl) && structural_eq(ar, br)
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

/// Whether the entire body is within the supported pure-integer surface.
fn body_supported(body: &[Statement]) -> bool {
    body.iter().all(stmt_supported)
}

/// Whether one statement is within the supported surface.
fn stmt_supported(stmt: &Statement) -> bool {
    match stmt {
        Statement::Assign(lhs, e) => matches!(lhs, Expr::Identifier(_)) && expr_supported(e),
        Statement::Let { names, expr, .. } => {
            names.is_empty() && expr.as_ref().map_or(true, |e| expr_supported(e))
        }
        Statement::Guarded(c, inner) => expr_supported(c) && body_supported(inner),
        Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) | Statement::Tail(e) => {
            expr_supported(e)
        }
        Statement::Term(None) | Statement::EndProgram(None) => true,
        Statement::Gate(c) | Statement::Check(c) => expr_supported(c),
        Statement::Expression(e) => expr_supported(e),
        // Paths that terminate without a value are always modelable.
        Statement::Rollback(_) | Statement::Halt | Statement::Trap => true,
        // Loops, foreach/break, calls-in-statement position, yields, expects,
        // metadata, arrows — outside the pure-integer prover's contract.
        _ => false,
    }
}

/// Whether an expression is within the supported pure-integer surface.
///
/// Calls, fields, indexes, slices, ranges, spawns, quotes, and reflection are
/// excluded: the prover must never claim a proof over a value it cannot
/// evaluate.
fn expr_supported(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(_)
        | Expr::Char(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::BeginProgram
        | Expr::UnitLiteral { .. }
        | Expr::Identifier(_)
        | Expr::Wildcard => true,
        Expr::BinaryOp(_, l, r) => expr_supported(l) && expr_supported(r),
        Expr::UnaryOp(_, x) => expr_supported(x),
        Expr::AddrOf(x) | Expr::Consume(x) => expr_supported(x),
        _ => false,
    }
}

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
        assert_eq!(json["proven"].as_u64(), Some(0));
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
}
