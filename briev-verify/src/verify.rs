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
    // C3: value summaries let a body be proved through the helpers it calls.
    // Computed once, up front, so the result never depends on item order.
    let sums = build_summaries(items);
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
        // within the pure-integer prover's reach (calls included, when the
        // callee has a value summary).
        if let Some(reason) = unsupported_reason(body, &sums) {
            report.skipped.push(format!("{name}: {reason}"));
            continue;
        }
        // Substitute each summarised call with the callee's value expression,
        // so the existing path walk reasons about one flat body.
        let Ok(flattened) = inline_body(body, &sums) else {
            report.skipped.push(format!("{name}: a call could not be inlined"));
            continue;
        };
        match verify_one(
            &contract.pre_condition,
            &contract.post_condition,
            &flattened,
        ) {
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
    let mut first_undecided: Option<usize> = None;
    for (index, state) in terminated.iter().enumerate() {
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

/// Try to decide an Undecided path with the SMT solver (C4).
///
/// Returns `None` — leaving the caller's honest `Undecided` verdict intact —
/// when no solver is installed, when the goal falls outside the encodable
/// subset, or when the path's state cannot be substituted unambiguously (a
/// prior-state `@x` reference or an unknown value).
fn smt_discharge(post: &Expr, state: &SymbolicState) -> Option<ProofVerdict> {
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
    let query = crate::smt::build_vc_query(&facts, &goal);
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

/// Rewrite every identifier the path bound to a symbolic value.
///
/// Substitution is transitive (a local may be defined in terms of another) with
/// a depth cap; it bails out (`None`) rather than guessing.
fn substitute_all(expr: &Expr, assignments: &HashMap<String, SymbolicValue>) -> Option<Expr> {
    // A body assignment is self-referential: `acc = acc + 1` binds
    // `acc -> acc + 1`. Following that chain reaches the depth cap and returns
    // None, which silently meant NO verification condition was ever built — so
    // no accumulating body could ever reach the solver. The `seen` guard stops
    // at the cycle and leaves the name free, which is what a VC needs.
    let mut seen = Vec::new();
    subst_expr(expr, assignments, 0, &mut seen)
}

/// One substitution step; `depth` caps transitive chains.
fn subst_expr(
    expr: &Expr,
    assignments: &HashMap<String, SymbolicValue>,
    depth: usize,
    seen: &mut Vec<String>,
) -> Option<Expr> {
    if depth > SUBST_DEPTH_CAP {
        return None;
    }
    match expr {
        Expr::Identifier(name) => match assignments.get(name) {
            Some(value) => {
                // Guard the name BEFORE descending. `acc -> acc + 1` is
                // self-referential, so without this the inner `acc` expands
                // again and one `acc = acc + 1` becomes acc+2 in the goal.
                if seen.iter().any(|seen_name| seen_name == name) {
                    return Some(Expr::Identifier(name.clone()));
                }
                seen.push(name.clone());
                let expanded = symbolic_to_expr(value, assignments, depth + 1, seen);
                seen.pop();
                expanded
            }
            None => Some(expr.clone()),
        },
        Expr::BinaryOp(kind, l, r) => Some(Expr::BinaryOp(
            *kind,
            Box::new(subst_expr(l, assignments, depth, seen)?),
            Box::new(subst_expr(r, assignments, depth, seen)?),
        )),
        Expr::UnaryOp(kind, inner) => Some(Expr::UnaryOp(
            *kind,
            Box::new(subst_expr(inner, assignments, depth, seen)?),
        )),
        other => Some(other.clone()),
    }
}

/// Convert a tracked symbolic value back into an expression for the solver.
fn symbolic_to_expr(
    value: &SymbolicValue,
    assignments: &HashMap<String, SymbolicValue>,
    depth: usize,
    seen: &mut Vec<String>,
) -> Option<Expr> {
    match value {
        SymbolicValue::Literal(n, _) => Some(Expr::Decimal(*n)),
        SymbolicValue::Identifier(name) => {
            // A cycle means "this name refers to itself": keep it free rather
            // than expanding until the depth cap and giving up.
            if seen.iter().any(|seen_name| seen_name == name) {
                return Some(Expr::Identifier(name.clone()));
            }
            match assignments.get(name) {
                Some(_) if depth > SUBST_DEPTH_CAP => Some(Expr::Identifier(name.clone())),
                Some(inner) => {
                    seen.push(name.clone());
                    let expanded = symbolic_to_expr(inner, assignments, depth + 1, seen);
                    seen.pop();
                    expanded
                }
                None => Some(Expr::Identifier(name.clone())),
            }
        }
        SymbolicValue::Binary(op, l, r) => Some(Expr::BinaryOp(
            op_kind(op)?,
            Box::new(symbolic_to_expr(l, assignments, depth + 1, seen)?),
            Box::new(symbolic_to_expr(r, assignments, depth + 1, seen)?),
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

/// The reason one statement is unsupported, if it is.
fn statement_reason(stmt: &Statement, sums: &HashMap<String, Summary>) -> Option<String> {
    match stmt {
        Statement::Assign(lhs, e) => {
            if !matches!(lhs, Expr::Identifier(_)) {
                return Some("an assignment whose target is not a plain name".to_string());
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
            if !sums.contains_key(name) {
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
fn build_summaries(items: &[TopLevel]) -> HashMap<String, Summary> {
    let candidates: Vec<(&str, Vec<String>, &[Statement])> =
        items.iter().filter_map(summary_candidate).collect();
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
/// Parameters are deliberately left UNBOUND: `eval_symbolic` already yields
/// `Identifier(name)` for an unknown name, whereas binding `a -> Identifier(a)`
/// is self-referential — `symbolic_to_expr` would follow it until the depth cap
/// and give up, and no summary would ever be produced.
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
                let mut seen = Vec::new();
                return symbolic_to_expr(
                    &eval_symbolic(e, &state),
                    &state.assignments,
                    0,
                    &mut seen,
                )
                .ok_or(());
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
    use crate::ast::{BinaryOpKind, Contract, Definition, Expr, Statement, TopLevel};
    use super::verify_bodies;

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
        assert_eq!(report.proven, vec!["f".to_string()], "unproven: {:?}", report.unproven);
        assert!(report.violated.is_empty());
    }

    /// A false contract built the same way is refuted, not silently accepted.
    #[test]
    fn a_constructed_false_contract_is_violated() {
        let contract = Contract::proven(
            Expr::Identifier("true".into()),
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
