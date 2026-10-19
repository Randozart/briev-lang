// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Call-site contract gate — the three-tier verdict (plan
//! `2026-10-19-contract-tiers-and-regularity.md`, owner Option 1 ruling).
//!
//! Given the caller's KNOWN FACTS (guard conditions, `check` statements, the
//! enclosing body's own precondition) and a callee precondition substituted
//! with the actual arguments, classify every conjunct:
//!
//! - `Proven`  — discharged: the backend emits no code (negative-cost axiom).
//! - `Refuted` — contradicted: compile error with the counterexample.
//! - `Unproven` — neither: compile error with the fix (supply the guard/fact,
//!   or make the dynamic boundary explicit with `Result`/`Option`).
//!
//! The prover is heuristic-first and SOUND: it only answers Proven/Refuted
//! when the derivation is exact (literal evaluation, or linear bound
//! derivation over normalized atoms). Everything conservative is `Unproven`.
//! Z3 is consulted when available for the same queries (exactness with
//! deeper arithmetic); its Sat/Unsat maps to Refuted/Proven, Unknown stays
//! Unproven.

use crate::ast::{BinaryOpKind, Expr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Proven,
    Refuted,
    Unproven,
}

/// One comparison in normalized form: `T op c` where T is a linear
/// combination of atoms and c a rational constant.
#[derive(Debug, Clone)]
struct LinC {
    /// atom key → coefficient. Atom key = canonical structural string
    /// (identifiers, field chains, opaque subexpressions).
    terms: Vec<(String, f64)>,
    const_: f64,
    op: BinaryOpKind,
}

impl LinC {
    fn key(&self) -> String {
        let mut parts: Vec<String> = self
            .terms
            .iter()
            .map(|(a, c)| format!("{}*{}", c, a))
            .collect();
        parts.sort();
        parts.push(format!("const={}", self.const_));
        parts.push(format!("op={:?}", self.op));
        parts.join("+")
    }
}

/// Reduce an expression to `(LinC, residual)` — residual non-empty when the
/// expression is NOT linear over atoms (opaque subexpression).
fn linify(e: &Expr) -> (Vec<(String, f64)>, f64, bool) {
    match e {
        Expr::Decimal(n) => (vec![], *n as f64, true),
        Expr::Float(f) => (vec![], *f, true),
        Expr::Identifier(name) => (vec![(name.clone(), 1.0)], 0.0, true),
        Expr::Field(base, field) => {
            let (mut t, c, ok) = linify(base);
            if !ok {
                return (vec![], 0.0, false);
            }
            // A field read is its own atom: `buf.len` — prefix the base's
            // atoms only when the base is itself a bare chain (nested field
            // reads extend the key; a linear base means an opaque prefix).
            if t.len() == 1 && c == 0.0 {
                t[0].0 = format!("{}.{}", t[0].0, field);
                (t, 0.0, true)
            } else {
                (vec![(format!("{:?}.{}", base, field), 1.0)], 0.0, true)
            }
        }
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => {
            let (mut t, c, ok) = linify(inner);
            if !ok {
                return (vec![], 0.0, false);
            }
            for (_, k) in t.iter_mut() {
                *k = -*k;
            }
            (t, -c, true)
        }
        Expr::BinaryOp(BinaryOpKind::Add, l, r) => {
            let (mut tl, cl, okl) = linify(l);
            let (tr, cr, okr) = linify(r);
            if !okl || !okr {
                return (vec![], 0.0, false);
            }
            tl.extend(tr);
            (tl, cl + cr, true)
        }
        Expr::BinaryOp(BinaryOpKind::Sub, l, r) => {
            let (mut tl, cl, okl) = linify(l);
            let (tr, cr, okr) = linify(r);
            if !okl || !okr {
                return (vec![], 0.0, false);
            }
            for (atom, k) in tr {
                tl.push((atom, -k));
            }
            (tl, cl - cr, true)
        }
        // Opaque: a named atom by structure.
        other => (vec![(format!("{:?}", other), 1.0)], 0.0, true),
    }
}

/// Normalize `lhs op rhs` to the canonical `T op c` form (T moved left,
/// constants right, direction canonicalized to Le/Lt/Eq/Neq family).
fn normalize(lhs: &Expr, op: BinaryOpKind, rhs: &Expr) -> Option<LinC> {
    let (mut lt, lc, lok) = linify(lhs);
    let (mut rt, rc, rok) = linify(rhs);
    if !lok || !rok {
        return None;
    }
    // T = lhs - rhs; const moves right.
    for (_, k) in rt.iter_mut() {
        *k = -*k;
    }
    let const_ = rc - lc;
    lt.extend(rt);
    // Merge duplicate atoms.
    lt.sort_by(|a, b| a.0.cmp(&b.0));
    let mut merged: Vec<(String, f64)> = Vec::with_capacity(lt.len());
    for (a, k) in lt {
        if let Some(last) = merged.last_mut() {
            if last.0 == a {
                last.1 += k;
                continue;
            }
        }
        merged.push((a, k));
    }
    merged.retain(|(_, k)| *k != 0.0);
    let (op, flip) = match op {
        BinaryOpKind::Le => (BinaryOpKind::Le, false),
        BinaryOpKind::Ge => (BinaryOpKind::Le, true),
        BinaryOpKind::Lt => (BinaryOpKind::Lt, false),
        BinaryOpKind::Gt => (BinaryOpKind::Lt, true),
        BinaryOpKind::Eq => (BinaryOpKind::Eq, false),
        BinaryOpKind::Neq => (BinaryOpKind::Neq, false),
        _ => return None,
    };
    if flip {
        for (_, k) in merged.iter_mut() {
            *k = -*k;
        }
        Some(LinC { terms: merged, const_: -const_, op })
    } else {
        Some(LinC { terms: merged, const_, op })
    }
}

/// Split an expression into AND-conjuncts.
fn conjuncts(e: &Expr, out: &mut Vec<Expr>) {
    if let Expr::BinaryOp(BinaryOpKind::And, l, r) = e {
        conjuncts(l, out);
        conjuncts(r, out);
    } else {
        out.push(e.clone());
    }
}

/// Exact literal evaluation of `lhs op rhs` when both sides are pure
/// integers (decimal literals, unary minus, + - * / on literals).
fn literal_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Decimal(n) => Some(*n),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => Some(-literal_int(inner)?),
        Expr::BinaryOp(BinaryOpKind::Add, l, r) => Some(literal_int(l)? + literal_int(r)?),
        Expr::BinaryOp(BinaryOpKind::Sub, l, r) => Some(literal_int(l)? - literal_int(r)?),
        Expr::BinaryOp(BinaryOpKind::Mul, l, r) => Some(literal_int(l)? * literal_int(r)?),
        _ => None,
    }
}

fn literal_cmp(l: &Expr, op: BinaryOpKind, r: &Expr) -> Option<bool> {
    let (a, b) = (literal_int(l)?, literal_int(r)?);
    let verdict = match op {
        BinaryOpKind::Le => a <= b,
        BinaryOpKind::Lt => a < b,
        BinaryOpKind::Ge => a >= b,
        BinaryOpKind::Gt => a > b,
        BinaryOpKind::Eq => a == b,
        BinaryOpKind::Neq => a != b,
        _ => return None,
    };
    Some(verdict)
}

/// The NEGATION of a canonical constraint, in the same canonical family:
/// not(T <= c) = -T < -c; not(T < c) = -T <= -c; not(T = c) = T != c;
/// not(T != c) = T = c. Extracted from `prove_conjunct` (Praetor).
fn negate_canonical(g: &LinC) -> Option<LinC> {
    match g.op {
        BinaryOpKind::Le => Some(LinC {
            terms: g.terms.iter().map(|(a, k)| (a.clone(), -k)).collect(),
            const_: -g.const_,
            op: BinaryOpKind::Lt,
        }),
        BinaryOpKind::Lt => Some(LinC {
            terms: g.terms.iter().map(|(a, k)| (a.clone(), -k)).collect(),
            const_: -g.const_,
            op: BinaryOpKind::Le,
        }),
        BinaryOpKind::Eq => Some(LinC {
            terms: g.terms.clone(),
            const_: g.const_,
            op: BinaryOpKind::Neq,
        }),
        BinaryOpKind::Neq => Some(LinC {
            terms: g.terms.clone(),
            const_: g.const_,
            op: BinaryOpKind::Eq,
        }),
        _ => None,
    }
}

/// Is the goal conjunct PROVEN by this fact? Both normalized: fact
/// `F op1 c1`, goal `F op2 c2` over the SAME linear term.
fn fact_entails(fact: &LinC, goal: &LinC) -> bool {
    if fact.terms != goal.terms {
        return false;
    }
    // Real semantics: for Le/Lt, a tighter (smaller) bound entails a looser
    // one. Eq entails only Eq with the same constant.
    match (fact.op, goal.op) {
        (BinaryOpKind::Le, BinaryOpKind::Le) => fact.const_ <= goal.const_,
        (BinaryOpKind::Lt, BinaryOpKind::Lt) => fact.const_ <= goal.const_,
        (BinaryOpKind::Le, BinaryOpKind::Lt) => fact.const_ < goal.const_,
        // A STRICT fact entails the NON-STRICT goal at the same constant
        // (T < c ⇒ T <= c — no gap needed). The gap belongs to the reverse
        // direction (Le → Lt), kept real-conservative above.
        (BinaryOpKind::Lt, BinaryOpKind::Le) => fact.const_ <= goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Le) => true,
        (BinaryOpKind::Eq, BinaryOpKind::Lt) => false, // real semantics: eq gives < only if strictness known; conservative
        (BinaryOpKind::Eq, BinaryOpKind::Eq) => fact.const_ == goal.const_,
        (BinaryOpKind::Le, BinaryOpKind::Eq) => fact.const_ == goal.const_,
        (BinaryOpKind::Lt, BinaryOpKind::Eq) => false,
        (BinaryOpKind::Neq, BinaryOpKind::Neq) => fact.const_ == goal.const_,
        _ => false,
    }
}

fn fact_refutes(fact: &LinC, goal: &LinC) -> bool {
    // The fact's negation entails the goal: fact says the goal is false.
    match (fact.op, goal.op) {
        // fact T <= c1, goal T > c2 (= not (T <= c2)) with c1 <= c2 → goal
        // refuted.
        (BinaryOpKind::Le, BinaryOpKind::Gt) => fact.const_ <= goal.const_,
        (BinaryOpKind::Lt, BinaryOpKind::Ge) => fact.const_ <= goal.const_,
        (BinaryOpKind::Le, BinaryOpKind::Ge) => fact.const_ < goal.const_,
        (BinaryOpKind::Lt, BinaryOpKind::Gt) => fact.const_ + 1.0 <= goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Neq) => fact.const_ == goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Lt) => fact.const_ >= goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Gt) => fact.const_ <= goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Le) => fact.const_ > goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Ge) => fact.const_ < goal.const_,
        _ => false,
    }
}

/// Prove one goal conjunct against the fact list.
fn prove_conjunct(goal: &Expr, facts: &[Expr]) -> Verdict {
    // Literal evaluation first: both sides pure integers → exact verdict.
    if let Expr::BinaryOp(op, l, r) = goal {
        if let Some(v) = literal_cmp(l, *op, r) {
            return if v { Verdict::Proven } else { Verdict::Refuted };
        }
    }
    let Some(g) = (match goal {
        Expr::BinaryOp(op, l, r) => normalize(l, *op, r),
        _ => None,
    }) else {
        return Verdict::Unproven;
    };
    let mut proven = false;
    for f in facts {
        let Some(fexpr_op) = (match f {
            Expr::BinaryOp(op, l, r) => Some((*op, l, r)),
            _ => None,
        }) else {
            continue;
        };
        // Literal refutation from a fact is not a thing (facts are trusted).
        let Some(fc) = normalize(fexpr_op.1, fexpr_op.0, fexpr_op.2) else {
            continue;
        };
        if fact_entails(&fc, &g) {
            proven = true;
            break;
        }
    }
    if proven {
        return Verdict::Proven;
    }
    // Refutation: some fact entails the goal's NEGATION.
    let Some(negated) = negate_canonical(&g) else {
        return Verdict::Unproven;
    };
    for f in facts {
        let Some((op, l, r)) = (match f {
            Expr::BinaryOp(op, l, r) => Some((*op, l, r)),
            _ => None,
        }) else {
            continue;
        };
        let Some(fc) = normalize(l, op, r) else {
            continue;
        };
        if fact_refutes(&fc, &negated) || fact_entails(&fc, &negated) {
            return Verdict::Refuted;
        }
    }
    Verdict::Unproven
}

/// Classify a full precondition goal (a conjunction) against facts.
/// All conjuncts must be Proven; any Refuted makes the whole REFUTED.
pub fn prove_precondition(goal: &Expr, facts: &[Expr]) -> Verdict {
    let mut split: Vec<Expr> = Vec::new();
    conjuncts(goal, &mut split);
    let mut any_refuted = false;
    for c in &split {
        match prove_conjunct(c, facts) {
            Verdict::Proven => {}
            Verdict::Refuted => any_refuted = true,
            Verdict::Unproven => return Verdict::Unproven,
        }
    }
    if any_refuted {
        Verdict::Refuted
    } else {
        Verdict::Proven
    }
}

/// Substitute callee parameter names with the actual argument expressions.
/// An argument referencing its own name is left alone (prevents self-capture).
pub fn substitute_params(goal: &Expr, params: &[String], args: &[Expr]) -> Expr {
    fn go(e: &Expr, params: &[String], args: &[Expr]) -> Expr {
        match e {
            Expr::Identifier(n) => {
                if let Some(i) = params.iter().position(|p| p == n) {
                    if let Some(a) = args.get(i) {
                        // Self-capture guard: `f(x)` proving `[x > 0]` — the
                        // argument IS the param name; substituting loops.
                        if !matches!(a, Expr::Identifier(an) if an == n) {
                            return a.clone();
                        }
                    }
                }
                e.clone()
            }
            Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
                *k,
                Box::new(go(l, params, args)),
                Box::new(go(r, params, args)),
            ),
            Expr::UnaryOp(k, inner) => {
                Expr::UnaryOp(*k, Box::new(go(inner, params, args)))
            }
            Expr::Field(base, f) => Expr::Field(
                Box::new(go(base, params, args)),
                f.clone(),
            ),
            other => other.clone(),
        }
    }
    go(goal, params, args)
}

/// The conjunction facts established by one guard/check expression.
pub fn facts_of(cond: &Expr) -> Vec<Expr> {
    let mut out = Vec::new();
    conjuncts(cond, &mut out);
    out
}

/// The fact a LET INITIALIZER establishes about its bound name: a
/// non-negative integer literal gives `v >= 0` (and `v == n` for exact
/// reasoning). Sums of such literals too. 2026-10-19 (contract tiers v2).
pub fn initializer_fact(name: &str, init: &Expr) -> Option<Expr> {
    fn nonneg_literal_sum(e: &Expr) -> Option<i64> {
        match e {
            Expr::Decimal(n) if *n >= 0 => Some(*n),
            Expr::BinaryOp(BinaryOpKind::Add, l, r) => {
                Some(nonneg_literal_sum(l)? + nonneg_literal_sum(r)?)
            }
            _ => None,
        }
    }
    let n = nonneg_literal_sum(init)?;
    if n == 0 {
        return Some(Expr::BinaryOp(
            BinaryOpKind::Ge,
            Box::new(Expr::Identifier(name.to_string())),
            Box::new(Expr::Decimal(0)),
        ));
    }
    Some(Expr::BinaryOp(
        BinaryOpKind::Gt,
        Box::new(Expr::Identifier(name.to_string())),
        Box::new(Expr::Decimal(0)),
    ))
}

/// Does `v = rhs` RE-ESTABLISH `v >= 0`? True when rhs is `v + e` / `e + v`
/// with every atom of `e` already known non-negative (an accumulator or
/// counter carrying its sign through ordinary code). 2026-10-19 (v2).
pub fn rederives_nonneg(v: &str, rhs: &Expr, facts: &[Expr]) -> Option<Expr> {
    fn every_atom_known_nonneg(e: &Expr, facts: &[Expr]) -> bool {
        match e {
            Expr::Decimal(n) => *n >= 0,
            Expr::Identifier(name) => {
                // Known from a fact `name >= 0`?
                let goal = Expr::BinaryOp(
                    BinaryOpKind::Ge,
                    Box::new(Expr::Identifier(name.clone())),
                    Box::new(Expr::Decimal(0)),
                );
                prove_precondition(&goal, facts) == Verdict::Proven
            }
            Expr::BinaryOp(BinaryOpKind::Add, l, r) => {
                every_atom_known_nonneg(l, facts) && every_atom_known_nonneg(r, facts)
            }
            _ => false,
        }
    }
    let other = match rhs {
        Expr::BinaryOp(BinaryOpKind::Add, l, r) => {
            if matches!(l.as_ref(), Expr::Identifier(n) if n == v) {
                r.as_ref().clone()
            } else if matches!(r.as_ref(), Expr::Identifier(n) if n == v) {
                l.as_ref().clone()
            } else {
                return None;
            }
        }
        _ => return None,
    };
    // Pre-assignment `v >= 0` must be known (check against the KILLED list —
    // the caller invokes this AFTER the retain, so consult rhs-side knowledge
    // only: `v = v + e` with e >= 0 needs v's OLD non-negativity; the caller
    // passes the post-kill list, so look for a fact of the form v >= k).
    let v_nonneg_known = facts.iter().any(|f| {
        matches!(
            f,
            Expr::BinaryOp(BinaryOpKind::Ge, l, _)
                if matches!(l.as_ref(), Expr::Identifier(n) if n == v)
        )
    });
    if !v_nonneg_known && !every_atom_known_nonneg(&other, facts) {
        return None;
    }
    if !every_atom_known_nonneg(&other, facts) {
        return None;
    }
    Some(Expr::BinaryOp(
        BinaryOpKind::Ge,
        Box::new(Expr::Identifier(v.to_string())),
        Box::new(Expr::Decimal(0)),
    ))
}

/// Does any part of this expression mention the named variable (for fact
/// invalidation on assignment)?
pub fn mentions(e: &Expr, var: &str) -> bool {
    match e {
        Expr::Identifier(n) => n == var,
        Expr::Field(base, _) => mentions(base, var),
        Expr::UnaryOp(_, inner) => mentions(inner, var),
        Expr::BinaryOp(_, l, r) => mentions(l, var) || mentions(r, var),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(src: &str) -> Expr {
        // Minimal parse via the shared lexer/parser for readable tests.
        let tokens = crate::lexer::tokenize(src).expect("lex");
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_expression().expect("parse")
    }

    #[test]
    fn literal_refutation() {
        // pos(0 - 3) with [x > 0]: substitute → (0 - 3) > 0 → Refuted.
        let goal = e("0 - 3 > 0");
        assert_eq!(prove_precondition(&goal, &[]), Verdict::Refuted);
    }

    #[test]
    fn literal_proven() {
        let goal = e("5 >= 0");
        assert_eq!(prove_precondition(&goal, &[]), Verdict::Proven);
    }

    #[test]
    fn guard_fact_discharges_shifted_goal() {
        // facts: off >= 0, off + 2 <= buf.len; goal: off < buf.len.
        let facts = vec![e("off >= 0"), e("off + 2 <= buf.len")];
        let goal = e("off < buf.len");
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn unproven_without_facts() {
        let goal = e("off < buf.len");
        assert_eq!(prove_precondition(&goal, &[]), Verdict::Unproven);
    }

    #[test]
    fn initializer_fact_refutes_literal_goal() {
        // let n = 5 establishes `n > 0`; goal `n < 0` is its negation.
        let facts = vec![e("n > 0")];
        let goal = e("n < 0");
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Refuted);
    }

    #[test]
    fn refuted_by_contradicting_fact() {
        let facts = vec![e("off >= 0")];
        let goal = e("off < 0");
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Refuted);
    }

    #[test]
    fn conjunction_all_must_prove() {
        let facts = vec![e("off >= 0"), e("off + 2 <= buf.len")];
        // off < buf.len proves; off >= 1 does not (only >= 0 known).
        let goal = e("off < buf.len && off >= 1");
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Unproven);
    }

    #[test]
    fn substitution_reaches_goal() {
        let pre = e("x > 0 && x < len");
        let params = vec!["x".to_string(), "len".to_string()];
        let args = vec![e("n"), e("buf.len")];
        let goal = substitute_params(&pre, &params, &args);
        let facts = vec![e("n > 0"), e("n + 1 <= buf.len")];
        // goal: n > 0 && n < buf.len — n+1 <= len gives n < len (int +1).
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn field_chains_are_atoms() {
        let facts = vec![e("buf.len >= 4")];
        let goal = e("buf.len >= 2");
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }
}
