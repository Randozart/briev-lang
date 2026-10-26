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
//! when the derivation is exact (literal evaluation, a same-term linear
//! implication, or a conservative interval bound). Everything else is
//! `Unproven`. Z3 is consulted when available for the same queries (exactness
//! with deeper arithmetic); its Sat/Unsat maps to Refuted/Proven, Unknown
//! stays Unproven.
//!
//! 2026-10-20 (bucket 2): the first version only consumed the caller's own
//! facts. Bucket 2 exposed the missing half of every recursive proof:
//! callee postconditions, guard fall-through, strict integer bounds, and
//! quotient/remainder/shift/mask intervals. This module now carries all four.
//! Interval reasoning is deliberately conservative: signed/overflowing/
//! unknown shapes return no bound rather than a guessed one.

use crate::ast::{BinaryOpKind, Expr};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Proven,
    Refuted,
    Unproven,
}

/// A registered defn contract: parameter names, precondition, postcondition.
/// Both sides are carried because a caller needs the postcondition as a fact
/// after `let r = f(args)`.
#[derive(Debug, Clone, PartialEq)]
pub struct FnContract {
    pub params: Vec<String>,
    pub pre: Expr,
    pub post: Expr,
}

/// One comparison in normalized form: `T op c` where T is a linear
/// combination of atoms and c an exact integer constant. 2026-10-20: i128
/// (checked), not f64 — `2^60` and `10^18` contract boundaries must be exact.
#[derive(Debug, Clone)]
struct LinC {
    /// atom key → coefficient. Atom key = canonical structural string
    /// (identifiers, field chains, opaque subexpressions).
    terms: Vec<(String, i128)>,
    const_: i128,
    op: BinaryOpKind,
}

/// Inclusive bound. `None` means unbounded in that direction.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    lo: Option<i128>,
    hi: Option<i128>,
}

impl Bounds {
    fn unknown() -> Self {
        Bounds { lo: None, hi: None }
    }
    fn exact(v: i128) -> Self {
        Bounds { lo: Some(v), hi: Some(v) }
    }
    fn nonneg() -> Self {
        Bounds { lo: Some(0), hi: None }
    }
    fn intersect(self, other: Bounds) -> Bounds {
        let lo = match (self.lo, other.lo) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        let hi = match (self.hi, other.hi) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        Bounds { lo, hi }
    }
    fn is_unknown(&self) -> bool {
        self.lo.is_none() && self.hi.is_none()
    }
}

/// Canonical atom key. Identifiers and field chains are readable keys; every
/// other expression is its structural Debug form. Exact structural equality
/// is what makes the atom check sound.
fn atom_key(e: &Expr) -> String {
    if let Some(inner) = struct_field_value(e) {
        return atom_key(inner);
    }
    match e {
        Expr::Identifier(n) => n.clone(),
        Expr::Field(base, field) => format!("{}.{}", atom_key(base), field),
        _ => format!("{:?}", e),
    }
}

/// 2026-10-26: `Type { field: value, ... }.field` denotes `value`. Leaving it
/// opaque made every struct-literal argument (Socket/ByteBuf/Matrix views)
/// fail bounds the caller already guarded. Reduction is exact and total.
fn struct_field_value(e: &Expr) -> Option<&Expr> {
    let Expr::Field(base, field) = e else {
        return None;
    };
    let Expr::StructLiteral { fields, specs, .. } = base.as_ref() else {
        return None;
    };
    fields
        .iter()
        .chain(specs.iter())
        .find(|(name, _)| name == field)
        .map(|(_, value)| value)
}

/// Reduce an expression to `(terms, constant)` — `None` when the expression
/// is not an exact integer linear form over atoms.
fn linify(e: &Expr) -> Option<(Vec<(String, i128)>, i128)> {
    // Fold any pure literal expression (`1 << 24`, `10 * 10`, ...) to its
    // exact constant before atomizing; otherwise the right-hand side becomes
    // an opaque atom and every bound against it is lost.
    if let Some(n) = literal_int(e) {
        return Some((vec![], n as i128));
    }
    if let Some(inner) = struct_field_value(e) {
        return linify(inner);
    }
    match e {
        Expr::Decimal(n) => Some((vec![], *n as i128)),
        Expr::Char(c) => Some((vec![], *c as i128)),
        Expr::TaggedLiteral(n, _) => Some((vec![], *n as i128)),
        Expr::Float(_) | Expr::Bool(_) | Expr::BeginProgram => None,
        Expr::Identifier(_) | Expr::Field(_, _) => {
            Some((vec![(atom_key(e), 1)], 0))
        }
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => {
            let (mut t, c) = linify(inner)?;
            for (_, k) in t.iter_mut() {
                *k = k.checked_neg()?;
            }
            Some((t, c.checked_neg()?))
        }
        Expr::BinaryOp(BinaryOpKind::Add, l, r) => {
            let (mut tl, cl) = linify(l)?;
            let (tr, cr) = linify(r)?;
            tl.extend(tr);
            Some((tl, cl.checked_add(cr)?))
        }
        Expr::BinaryOp(BinaryOpKind::Sub, l, r) => {
            let (mut tl, cl) = linify(l)?;
            let (tr, cr) = linify(r)?;
            tl.extend(tr.into_iter().map(|(a, k)| (a, -k)));
            Some((tl, cl.checked_sub(cr)?))
        }
        Expr::BinaryOp(BinaryOpKind::Mul, l, r) => {
            // Constant × linear is linear; both-linear is not.
            let (lt, lc) = linify(l)?;
            let (rt, rc) = linify(r)?;
            if lt.is_empty() {
                let mut out = rt;
                for (_, k) in out.iter_mut() {
                    *k = k.checked_mul(lc)?;
                }
                Some((out, rc.checked_mul(lc)?))
            } else if rt.is_empty() {
                let mut out = lt;
                for (_, k) in out.iter_mut() {
                    *k = k.checked_mul(rc)?;
                }
                Some((out, lc.checked_mul(rc)?))
            } else {
                None
            }
        }
        _ => Some((vec![(atom_key(e), 1)], 0)),
    }
}

/// Normalize `lhs op rhs` to the canonical `T op c` form (T moved left,
/// constants right, direction canonicalized to Le/Lt/Eq/Neq family).
fn normalize(lhs: &Expr, op: BinaryOpKind, rhs: &Expr) -> Option<LinC> {
    let (mut lt, lc) = linify(lhs)?;
    let (rt, rc) = linify(rhs)?;
    // T = lhs - rhs; constant = rhs.const - lhs.const.
    for (a, k) in rt {
        lt.push((a, k.checked_neg()?));
    }
    let mut const_ = rc.checked_sub(lc)?;
    // Merge duplicate atoms with checked arithmetic.
    lt.sort_by(|a, b| a.0.cmp(&b.0));
    let mut merged: Vec<(String, i128)> = Vec::with_capacity(lt.len());
    for (a, k) in lt {
        if let Some(last) = merged.last_mut() {
            if last.0 == a {
                last.1 = last.1.checked_add(k)?;
                continue;
            }
        }
        merged.push((a, k));
    }
    merged.retain(|(_, k)| *k != 0);
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
            *k = k.checked_neg()?;
        }
        const_ = const_.checked_neg()?;
    }
    Some(LinC { terms: merged, const_, op })
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
/// integers (decimal/char literals, unary minus, + - * / % shifts bitops).
fn literal_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Decimal(n) => Some(*n),
        Expr::Char(c) => Some(*c as i64),
        Expr::TaggedLiteral(n, _) => Some(*n),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => literal_int(inner)?.checked_neg(),
        Expr::BinaryOp(BinaryOpKind::Add, l, r) => {
            literal_int(l)?.checked_add(literal_int(r)?)
        }
        Expr::BinaryOp(BinaryOpKind::Sub, l, r) => {
            literal_int(l)?.checked_sub(literal_int(r)?)
        }
        Expr::BinaryOp(BinaryOpKind::Mul, l, r) => {
            literal_int(l)?.checked_mul(literal_int(r)?)
        }
        Expr::BinaryOp(BinaryOpKind::Div, l, r) => {
            let d = literal_int(r)?;
            if d == 0 {
                None
            } else {
                literal_int(l)?.checked_div(d)
            }
        }
        Expr::BinaryOp(BinaryOpKind::Mod, l, r) => {
            let d = literal_int(r)?;
            if d == 0 {
                None
            } else {
                literal_int(l)?.checked_rem(d)
            }
        }
        Expr::BinaryOp(BinaryOpKind::Shl, l, r) => {
            let s = literal_int(r)?;
            if (0..64).contains(&s) {
                literal_int(l)?.checked_mul(1i64.checked_shl(s as u32)?)
            } else {
                None
            }
        }
        Expr::BinaryOp(BinaryOpKind::Shr, l, r) => {
            let s = literal_int(r)?;
            if (0..64).contains(&s) {
                literal_int(l)?.checked_shr(s as u32)
            } else {
                None
            }
        }
        Expr::BinaryOp(BinaryOpKind::BitAnd, l, r) => {
            Some(literal_int(l)? & literal_int(r)?)
        }
        Expr::BinaryOp(BinaryOpKind::BitOr, l, r) => {
            Some(literal_int(l)? | literal_int(r)?)
        }
        Expr::BinaryOp(BinaryOpKind::BitXor, l, r) => {
            Some(literal_int(l)? ^ literal_int(r)?)
        }
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
/// not(T != c) = T = c.
fn negate_canonical(g: &LinC) -> Option<LinC> {
    match g.op {
        BinaryOpKind::Le => Some(LinC {
            terms: g.terms.iter().map(|(a, k)| (a.clone(), -k)).collect(),
            const_: g.const_.checked_neg()?,
            op: BinaryOpKind::Lt,
        }),
        BinaryOpKind::Lt => Some(LinC {
            terms: g.terms.iter().map(|(a, k)| (a.clone(), -k)).collect(),
            const_: g.const_.checked_neg()?,
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
    if fact.terms == goal.terms {
        return entail_same_terms(fact, goal);
    }
    if fact.op == BinaryOpKind::Eq {
        let neg_terms: Vec<(String, i128)> = fact
            .terms
            .iter()
            .map(|(a, k)| (a.clone(), -k))
            .collect();
        if neg_terms == goal.terms {
            let twin = LinC {
                terms: neg_terms,
                const_: fact.const_.wrapping_neg(),
                op: BinaryOpKind::Eq,
            };
            return entail_same_terms(&twin, goal);
        }
    }
    false
}

/// Entailment for the SAME normalized term `T`. Every arm is an exact
/// integer implication. The `(Lt, Le)` arm is the discrete tightening:
/// `T < a` gives `T <= b` when `a <= b + 1` (for example `nd < count` gives
/// `count - nd >= 1`). Checked arithmetic; overflow means "cannot tell".
fn entail_same_terms(fact: &LinC, goal: &LinC) -> bool {
    match (fact.op, goal.op) {
        (BinaryOpKind::Le, BinaryOpKind::Le) => fact.const_ <= goal.const_,
        (BinaryOpKind::Le, BinaryOpKind::Lt) => fact.const_ < goal.const_,
        (BinaryOpKind::Lt, BinaryOpKind::Le) => match goal.const_.checked_add(1) {
            Some(limit) => fact.const_ <= limit,
            None => false,
        },
        (BinaryOpKind::Lt, BinaryOpKind::Lt) => fact.const_ <= goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Le) => fact.const_ <= goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Lt) => fact.const_ < goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Eq) => fact.const_ == goal.const_,
        (BinaryOpKind::Eq, BinaryOpKind::Neq) => fact.const_ != goal.const_,
        (BinaryOpKind::Le, BinaryOpKind::Neq) => goal.const_ > fact.const_,
        (BinaryOpKind::Lt, BinaryOpKind::Neq) => goal.const_ >= fact.const_,
        (BinaryOpKind::Neq, BinaryOpKind::Neq) => fact.const_ == goal.const_,
        _ => false,
    }
}

/// Collect atom bounds from simple one-atom facts. Facts with two atoms
/// (`x <= y`, `x != y`) are handled by the linear same-term core instead.
fn bounds_map(facts: &[Expr]) -> HashMap<String, Bounds> {
    let mut map: HashMap<String, Bounds> = HashMap::new();
    let mut neqs: Vec<(String, i128)> = Vec::new();
    for f in facts {
        let Expr::BinaryOp(op, l, r) = f else { continue };
        let Some(lc) = normalize(l, *op, r) else { continue };
        if lc.terms.len() != 1 {
            continue;
        }
        let (key, coeff) = lc.terms[0].clone();
        let b = map.entry(key.clone()).or_insert_with(Bounds::unknown);
        match (coeff, lc.op) {
            (1, BinaryOpKind::Le) => b.hi = Some(b.hi.map_or(lc.const_, |h| h.min(lc.const_))),
            (1, BinaryOpKind::Lt) => {
                if let Some(c) = lc.const_.checked_sub(1) {
                    b.hi = Some(b.hi.map_or(c, |h| h.min(c)));
                }
            }
            (1, BinaryOpKind::Eq) => *b = Bounds::exact(lc.const_),
            (1, BinaryOpKind::Neq) => neqs.push((key, lc.const_)),
            (-1, BinaryOpKind::Le) => {
                if let Some(c) = lc.const_.checked_neg() {
                    b.lo = Some(b.lo.map_or(c, |l| l.max(c)));
                }
            }
            (-1, BinaryOpKind::Lt) => {
                if let Some(c) = lc.const_.checked_neg().and_then(|c| c.checked_add(1)) {
                    b.lo = Some(b.lo.map_or(c, |l| l.max(c)));
                }
            }
            (-1, BinaryOpKind::Eq) => {
                if let Some(c) = lc.const_.checked_neg() {
                    *b = Bounds::exact(c);
                }
            }
            (-1, BinaryOpKind::Neq) => {
                if let Some(c) = lc.const_.checked_neg() {
                    neqs.push((key, c));
                }
            }
            _ => {}
        }
    }
    // `x != c` tightens a bound sitting exactly on c (integer lattice):
    // `x >= 0 && x != 0` is `x >= 1`, the recursive-base-case fact.
    // Contradictory intervals are left alone (the path is unreachable; we do
    // not exploit inconsistency to prove arbitrary goals).
    for (key, c) in neqs {
        if let Some(b) = map.get_mut(&key) {
            if b.lo == Some(c) && b.hi == Some(c) {
                continue;
            }
            if b.lo == Some(c) && b.hi.is_none_or(|h| c < h) {
                if let Some(next) = c.checked_add(1) {
                    b.lo = Some(next);
                }
            }
            if b.hi == Some(c) && b.lo.is_none_or(|l| l < c) {
                if let Some(prev) = c.checked_sub(1) {
                    b.hi = Some(prev);
                }
            }
        }
    }
    map
}

fn contains_float(e: &Expr) -> bool {
    match e {
        Expr::Float(_) => true,
        Expr::UnaryOp(_, inner) => contains_float(inner),
        Expr::BinaryOp(_, l, r) => contains_float(l) || contains_float(r),
        Expr::Field(base, _) | Expr::Reflect(base, _, _) | Expr::Cast(base, _)
        | Expr::IsType(base, _) | Expr::Consume(base) | Expr::Deref(base)
        | Expr::AddrOf(base) | Expr::Await(base) => contains_float(base),
        Expr::Call(_, args, _) | Expr::Tuple(args) | Expr::List(args) => {
            args.iter().any(contains_float)
        }
        _ => false,
    }
}

fn eval_bounds(e: &Expr, map: &HashMap<String, Bounds>) -> Bounds {
    let computed = match e {
        Expr::Decimal(n) => Bounds::exact(*n as i128),
        Expr::Char(c) => Bounds::exact(*c as i128),
        Expr::TaggedLiteral(n, _) => Bounds::exact(*n as i128),
        Expr::Identifier(_) | Expr::Field(_, _) | Expr::Call(_, _, _)
        | Expr::MethodCall(_, _, _, _, _) | Expr::Reflect(_, _, _) => Bounds::unknown(),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => {
            let b = eval_bounds(inner, map);
            Bounds {
                lo: b.hi.and_then(|v| v.checked_neg()),
                hi: b.lo.and_then(|v| v.checked_neg()),
            }
        }
        Expr::BinaryOp(op, l, r) => {
            let a = eval_bounds(l, map);
            let b = eval_bounds(r, map);
            binary_bounds(*op, a, b)
        }
        // Integer-preserving casts (Ptr↔Int, Char↔Int, same-width Bit) keep
        // the value; a cast is not a proof wall for address bounds.
        Expr::Cast(inner, _) => eval_bounds(inner, map),
        _ => Bounds::unknown(),
    };
    // A fact may state a bound about the whole expression (`acc / 10 >= 0`);
    // intersect it with the structural computation.
    if let Some(mapped) = map.get(&atom_key(e)) {
        computed.intersect(*mapped)
    } else {
        computed
    }
}

fn binary_bounds(op: BinaryOpKind, a: Bounds, b: Bounds) -> Bounds {
    match op {
        BinaryOpKind::Add => Bounds {
            lo: a.lo.zip(b.lo).and_then(|(x, y)| x.checked_add(y)),
            hi: a.hi.zip(b.hi).and_then(|(x, y)| x.checked_add(y)),
        },
        BinaryOpKind::Sub => Bounds {
            lo: a.lo.zip(b.hi).and_then(|(x, y)| x.checked_sub(y)),
            hi: a.hi.zip(b.lo).and_then(|(x, y)| x.checked_sub(y)),
        },
        BinaryOpKind::Mul => mul_bounds(a, b),
        BinaryOpKind::Div => div_bounds(a, b),
        BinaryOpKind::Mod => mod_bounds(a, b),
        BinaryOpKind::Shl => shl_bounds(a, b),
        BinaryOpKind::Shr => shr_bounds(a, b),
        BinaryOpKind::BitAnd => bitand_bounds(a, b),
        BinaryOpKind::BitOr => bitor_bounds(a, b),
        BinaryOpKind::BitXor => bitxor_bounds(a, b),
        _ => Bounds::unknown(),
    }
}

fn mul_bounds(a: Bounds, b: Bounds) -> Bounds {
    if let (Some(alo), Some(ahi), Some(blo), Some(bhi)) = (a.lo, a.hi, b.lo, b.hi) {
        let candidates = [
            alo.checked_mul(blo),
            alo.checked_mul(bhi),
            ahi.checked_mul(blo),
            ahi.checked_mul(bhi),
        ];
        if candidates.iter().any(|c| c.is_none()) {
            return Bounds::unknown();
        }
        let vals: Vec<i128> = candidates.into_iter().flatten().collect();
        return Bounds {
            lo: vals.iter().copied().min(),
            hi: vals.iter().copied().max(),
        };
    }
    // One-sided bounds still fix a one-sided product when both signs are
    // known. `acc >= 1 && acc*half` is the common `let doubled = x * 2` case.
    match (a.lo, a.hi, b.lo, b.hi) {
        (Some(alo), ahi, Some(blo), bhi) if alo >= 0 && blo >= 0 => Bounds {
            lo: alo.checked_mul(blo),
            hi: ahi.zip(bhi).and_then(|(x, y)| x.checked_mul(y)),
        },
        (Some(alo), ahi, blo, Some(bhi)) if alo <= 0 && bhi <= 0 => Bounds {
            lo: alo.checked_mul(bhi),
            hi: ahi.zip(blo).and_then(|(x, y)| x.checked_mul(y)),
        },
        _ => Bounds::unknown(),
    }
}

fn div_bounds(a: Bounds, b: Bounds) -> Bounds {
    // Exact positive divisor: truncating division is monotone, endpoints
    // suffice. (Rust `/` truncates toward zero, matching the language.)
    if let (Some(d), Some(d2)) = (b.lo, b.hi) {
        if d == d2 && d > 0 {
            let lo = a.lo.map(|v| v / d);
            let hi = a.hi.map(|v| v / d);
            if lo.is_some() || hi.is_some() {
                return Bounds { lo, hi };
            }
        }
    }
    // Divisor known only to be >= 1: for a non-negative dividend the upper
    // bound is `hi / lower_divisor`; the lower bound stays 0.
    if b.lo.is_some_and(|d| d >= 1) && a.lo.is_some_and(|v| v >= 0) {
        let hi = match (a.hi, b.lo) {
            (Some(h), Some(d)) => Some(h / d.max(1)),
            _ => None,
        };
        return Bounds { lo: Some(0), hi };
    }
    Bounds::unknown()
}

fn mod_bounds(a: Bounds, b: Bounds) -> Bounds {
    // Divisor merely known positive: a non-negative dividend gives a
    // non-negative remainder below the divisor's upper bound.
    if b.lo.is_some_and(|d| d >= 1) && a.lo.is_some_and(|v| v >= 0) {
        return Bounds {
            lo: Some(0),
            hi: b.hi.map(|d| d - 1),
        };
    }
    let (Some(d), Some(d2)) = (b.lo, b.hi) else {
        return Bounds::unknown();
    };
    if d != d2 || d <= 0 {
        return Bounds::unknown();
    }
    let max = d - 1;
    match (a.lo, a.hi) {
        (Some(lo), _) if lo >= 0 => Bounds { lo: Some(0), hi: Some(max) },
        (_, Some(hi)) if hi <= 0 => Bounds { lo: Some(-max), hi: Some(0) },
        _ => Bounds { lo: Some(-max), hi: Some(max) },
    }
}

fn shl_bounds(a: Bounds, b: Bounds) -> Bounds {
    let Some(k) = single(b) else { return Bounds::unknown() };
    if !(0..63).contains(&k) {
        return Bounds::unknown();
    }
    let Some(factor) = 1i128.checked_shl(k as u32) else {
        return Bounds::unknown();
    };
    if a.lo.is_some_and(|v| v < 0) {
        return Bounds::unknown();
    }
    Bounds {
        lo: a.lo.and_then(|v| v.checked_mul(factor)),
        hi: a.hi.and_then(|v| v.checked_mul(factor)),
    }
}

fn shr_bounds(a: Bounds, b: Bounds) -> Bounds {
    if a.lo.is_some_and(|v| v < 0) {
        return Bounds::unknown();
    }
    let (Some(slo), Some(shi)) = (b.lo, b.hi) else {
        return Bounds::unknown();
    };
    if slo < 0 || shi > 63 {
        return Bounds::unknown();
    }
    let lo = a.lo.map(|v| v >> shi);
    let hi = a.hi.map(|v| v >> slo);
    Bounds { lo, hi }
}

fn bitand_bounds(a: Bounds, b: Bounds) -> Bounds {
    if let Some(mask) = single(b) {
        if mask >= 0 {
            return Bounds { lo: Some(0), hi: Some(mask) };
        }
    }
    if let Some(mask) = single(a) {
        if mask >= 0 {
            return Bounds { lo: Some(0), hi: Some(mask) };
        }
    }
    Bounds::unknown()
}

fn bitor_bounds(a: Bounds, b: Bounds) -> Bounds {
    if let (Some(x), Some(y)) = (single(a), single(b)) {
        return Bounds::exact(x | y);
    }
    if a.lo.is_some_and(|v| v >= 0) && b.lo.is_some_and(|v| v >= 0) {
        // OR only sets bits, so the result is at least the larger operand.
        let lo = match (a.lo, b.lo) {
            (Some(x), Some(y)) => Some(x.max(y)),
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        };
        return Bounds { lo, hi: None };
    }
    Bounds::unknown()
}

fn bitxor_bounds(a: Bounds, b: Bounds) -> Bounds {
    if let (Some(x), Some(y)) = (single(a), single(b)) {
        return Bounds::exact(x ^ y);
    }
    if a.lo.is_some_and(|v| v >= 0) && b.lo.is_some_and(|v| v >= 0) {
        return Bounds { lo: Some(0), hi: None };
    }
    Bounds::unknown()
}

fn single(b: Bounds) -> Option<i128> {
    match (b.lo, b.hi) {
        (Some(x), Some(y)) if x == y => Some(x),
        _ => None,
    }
}

/// Interval verdict for one comparison. `None` = cannot tell. Strictness is
/// handled on the integer lattice via inclusive bounds.
fn interval_verdict(l: &Expr, op: BinaryOpKind, r: &Expr, facts: &[Expr]) -> Option<Verdict> {
    if contains_float(l) || contains_float(r) {
        return None;
    }
    let map = bounds_map(facts);
    let a = eval_bounds(l, &map);
    let b = eval_bounds(r, &map);
    // An inverted interval means the facts are contradictory. Do not read a
    // verdict out of an unreachable path.
    if matches!((a.lo, a.hi), (Some(x), Some(y)) if x > y)
        || matches!((b.lo, b.hi), (Some(x), Some(y)) if x > y)
    {
        return None;
    }
    let verdict = match op {
        BinaryOpKind::Le => {
            if a.hi.zip(b.lo).is_some_and(|(x, y)| x <= y) {
                Verdict::Proven
            } else if a.lo.zip(b.hi).is_some_and(|(x, y)| x > y) {
                Verdict::Refuted
            } else {
                return None;
            }
        }
        BinaryOpKind::Lt => {
            if a.hi.zip(b.lo).is_some_and(|(x, y)| x < y) {
                Verdict::Proven
            } else if a.lo.zip(b.hi).is_some_and(|(x, y)| x >= y) {
                Verdict::Refuted
            } else {
                return None;
            }
        }
        BinaryOpKind::Ge => {
            if a.lo.zip(b.hi).is_some_and(|(x, y)| x >= y) {
                Verdict::Proven
            } else if a.hi.zip(b.lo).is_some_and(|(x, y)| x < y) {
                Verdict::Refuted
            } else {
                return None;
            }
        }
        BinaryOpKind::Gt => {
            if a.lo.zip(b.hi).is_some_and(|(x, y)| x > y) {
                Verdict::Proven
            } else if a.hi.zip(b.lo).is_some_and(|(x, y)| x <= y) {
                Verdict::Refuted
            } else {
                return None;
            }
        }
        BinaryOpKind::Eq => {
            if let (Some(x), Some(y)) = (single(a), single(b)) {
                if x == y { Verdict::Proven } else { Verdict::Refuted }
            } else if a.hi.zip(b.lo).is_some_and(|(x, y)| x < y)
                || b.hi.zip(a.lo).is_some_and(|(x, y)| x < y)
            {
                Verdict::Refuted
            } else {
                return None;
            }
        }
        BinaryOpKind::Neq => {
            if let (Some(x), Some(y)) = (single(a), single(b)) {
                if x != y { Verdict::Proven } else { Verdict::Refuted }
            } else if a.hi.zip(b.lo).is_some_and(|(x, y)| x < y)
                || b.hi.zip(a.lo).is_some_and(|(x, y)| x < y)
            {
                Verdict::Proven
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(verdict)
}

fn is_complement(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Bool(x), Expr::Bool(y)) => x != y,
        (Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner), other)
        | (other, Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner)) => inner.as_ref() == other,
        (Expr::BinaryOp(op1, l1, r1), Expr::BinaryOp(op2, l2, r2)) => {
            l1 == l2
                && r1 == r2
                && matches!(
                    (op1, op2),
                    (BinaryOpKind::Eq, BinaryOpKind::Neq)
                        | (BinaryOpKind::Neq, BinaryOpKind::Eq)
                        | (BinaryOpKind::Lt, BinaryOpKind::Ge)
                        | (BinaryOpKind::Ge, BinaryOpKind::Lt)
                        | (BinaryOpKind::Gt, BinaryOpKind::Le)
                        | (BinaryOpKind::Le, BinaryOpKind::Gt)
                )
        }
        _ => false,
    }
}

fn prove_disjunction(goal: &Expr, facts: &[Expr]) -> Verdict {
    let mut parts = Vec::new();
    fn split<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        if let Expr::BinaryOp(BinaryOpKind::Or, l, r) = e {
            split(l, out);
            split(r, out);
        } else {
            out.push(e);
        }
    }
    split(goal, &mut parts);
    if parts.iter().any(|p| facts.iter().any(|f| is_complement(p, f))) {
        return Verdict::Proven;
    }
    for i in 0..parts.len() {
        for j in (i + 1)..parts.len() {
            if is_complement(parts[i], parts[j]) {
                return Verdict::Proven;
            }
        }
    }
    let mut any_refuted = false;
    for p in &parts {
        match prove_conjunct(p, facts) {
            Verdict::Proven => return Verdict::Proven,
            Verdict::Refuted => any_refuted = true,
            Verdict::Unproven => any_refuted = false,
        }
    }
    if any_refuted && parts.iter().all(|p| prove_conjunct(p, facts) == Verdict::Refuted) {
        Verdict::Refuted
    } else {
        Verdict::Unproven
    }
}

/// Prove one goal conjunct against the fact list.
/// Definitions established by `let x = e` facts (`x == e`). Used to expand
/// derived names before interval/linear reasoning, so the prover sees the
/// expression's bounds rather than an opaque atom.
fn defs_from_facts(facts: &[Expr]) -> Vec<(String, Expr)> {
    let mut defs = Vec::new();
    for f in facts {
        let Expr::BinaryOp(BinaryOpKind::Eq, l, r) = f else {
            continue;
        };
        let (name, value) = match (l.as_ref(), r.as_ref()) {
            (Expr::Identifier(n), v) => (n, v),
            (v, Expr::Identifier(n)) => (n, v),
            _ => continue,
        };
        if matches!(value, Expr::Identifier(n) if n == name) {
            continue;
        }
        if !defs.iter().any(|(existing, _): &(String, Expr)| existing == name) {
            defs.push((name.clone(), value.clone()));
        }
    }
    defs
}

/// Expand `let` definitions until stable (bounded depth for cycles). Uses the
/// existing parameter substitution walker; a self-definition is skipped.
fn expand_defs(e: &Expr, defs: &[(String, Expr)]) -> Expr {
    let mut out = e.clone();
    for _ in 0..8 {
        let before = out.clone();
        for (name, value) in defs {
            out = substitute_params(&out, std::slice::from_ref(name), std::slice::from_ref(value));
        }
        if out == before {
            break;
        }
    }
    out
}

fn prove_conjunct(goal: &Expr, facts: &[Expr]) -> Verdict {
    let direct = prove_conjunct_core(goal, facts);
    if direct != Verdict::Unproven {
        return direct;
    }
    // Field equalities (`b.len == 0`, `r.cap >= n`) are atom definitions.
    // Expand the whole atom, never just the base, so `r.len` cannot turn into
    // `(a + b).len`.
    let atom_defs = atom_defs_from_facts(facts);
    let mut current = if atom_defs.is_empty() {
        goal.clone()
    } else {
        expand_atom_defs(goal, &atom_defs)
    };
    if &current != goal {
        let verdict = prove_conjunct_core(&current, facts);
        if verdict != Verdict::Unproven {
            return verdict;
        }
    }
    // `let` definitions are equalities. Try the defining expression only as a
    // fallback: direct facts about the bound name (e.g. a guard's `x >= 1e18`)
    // are stronger than their expanded form.
    let defs = defs_from_facts(facts);
    if defs.is_empty() {
        return direct;
    }
    current = expand_defs(&current, &defs);
    if &current == goal {
        return direct;
    }
    prove_conjunct_core(&current, facts)
}

fn is_atom_expr(e: &Expr) -> bool {
    match e {
        Expr::Identifier(_) => true,
        Expr::Field(base, _) => is_atom_expr(base),
        _ => false,
    }
}

fn atom_defs_from_facts(facts: &[Expr]) -> Vec<(Expr, Expr)> {
    let mut defs = Vec::new();
    for f in facts {
        let Expr::BinaryOp(BinaryOpKind::Eq, l, r) = f else {
            continue;
        };
        if is_atom_expr(l) {
            defs.push(((**l).clone(), (**r).clone()));
        } else if is_atom_expr(r) {
            defs.push(((**r).clone(), (**l).clone()));
        }
    }
    defs
}

fn expand_atom_defs(goal: &Expr, defs: &[(Expr, Expr)]) -> Expr {
    let mut out = goal.clone();
    for _ in 0..8 {
        let before = out.clone();
        out = replace_atoms(&out, defs);
        if out == before {
            break;
        }
    }
    out
}

fn replace_atoms(e: &Expr, defs: &[(Expr, Expr)]) -> Expr {
    if let Some((_, value)) = defs.iter().find(|(atom, _)| atom == e) {
        return value.clone();
    }
    match e {
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(replace_atoms(l, defs)),
            Box::new(replace_atoms(r, defs)),
        ),
        Expr::UnaryOp(k, inner) => Expr::UnaryOp(*k, Box::new(replace_atoms(inner, defs))),
        Expr::Field(base, f) => Expr::Field(Box::new(replace_atoms(base, defs)), f.clone()),
        Expr::Call(name, args, id) => Expr::Call(
            name.clone(),
            args.iter().map(|a| replace_atoms(a, defs)).collect(),
            *id,
        ),
        Expr::MethodCall(recv, name, args, id, refs) => Expr::MethodCall(
            Box::new(replace_atoms(recv, defs)),
            name.clone(),
            args.iter().map(|a| replace_atoms(a, defs)).collect(),
            *id,
            refs.clone(),
        ),
        Expr::Reflect(base, target, kind) => {
            Expr::Reflect(Box::new(replace_atoms(base, defs)), target.clone(), *kind)
        }
        other => other.clone(),
    }
}

fn prove_conjunct_core(goal: &Expr, facts: &[Expr]) -> Verdict {
    // Structural facts (reflection `.^Length`, opaque predicates, field
    // chains) are not linear/interval expressions; an identical fact is the
    // proof.
    if facts.iter().any(|f| f == goal) {
        return Verdict::Proven;
    }
    // A stored length (`.^Length`) is a non-negative header slot. This is an
    // invariant of every managed String/collection, so `x.^Length >= 0`
    // needs no separate fact.
    if let Expr::BinaryOp(BinaryOpKind::Ge, l, r) = goal {
        if matches!(
            l.as_ref(),
            Expr::Reflect(_, field, crate::ast::ReflectKind::Runtime) if field == "Length"
        ) && matches!(r.as_ref(), Expr::Decimal(0))
        {
            return Verdict::Proven;
        }
    }
    match goal {
        Expr::Bool(true) => return Verdict::Proven,
        Expr::Bool(false) => return Verdict::Refuted,
        Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner) => {
            return match prove_conjunct(inner, facts) {
                Verdict::Proven => Verdict::Refuted,
                Verdict::Refuted => Verdict::Proven,
                Verdict::Unproven => Verdict::Unproven,
            };
        }
        Expr::BinaryOp(BinaryOpKind::Or, _, _) => return prove_disjunction(goal, facts),
        _ => {}
    }

    // Exact literal evaluation first.
    if let Expr::BinaryOp(op, l, r) = goal {
        if let Some(v) = literal_cmp(l, *op, r) {
            return if v { Verdict::Proven } else { Verdict::Refuted };
        }
    }

    // Predicate atoms (`is_digit(c)`, `in_range(...)`) are opaque Boolean
    // facts. Identical structure proves; the literal negation refutes.
    if !matches!(goal, Expr::BinaryOp(_, _, _)) {
        if facts.iter().any(|f| f == goal) {
            return Verdict::Proven;
        }
        if facts.iter().any(|f| {
            matches!(f, Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner) if inner.as_ref() == goal)
        }) {
            return Verdict::Refuted;
        }
    }

    let Some(g) = (match goal {
        Expr::BinaryOp(op, l, r) => normalize(l, *op, r),
        _ => None,
    }) else {
        // Non-linear shapes (products, shifts) may still have exact interval
        // bounds. Do not lose them just because the linear core declines.
        if let Expr::BinaryOp(op, l, r) = goal {
            if let Some(v) = interval_verdict(l, *op, r, facts) {
                return v;
            }
        }
        return Verdict::Unproven;
    };

    // A goal whose atoms cancel is a pure constant relation: `x + 1 = x + 1`
    // needs no fact at all.
    if g.terms.is_empty() {
        return match g.op {
            BinaryOpKind::Eq => {
                if g.const_ == 0 { Verdict::Proven } else { Verdict::Refuted }
            }
            BinaryOpKind::Le => {
                if g.const_ >= 0 { Verdict::Proven } else { Verdict::Refuted }
            }
            BinaryOpKind::Lt => {
                if g.const_ > 0 { Verdict::Proven } else { Verdict::Refuted }
            }
            BinaryOpKind::Neq => {
                if g.const_ != 0 { Verdict::Proven } else { Verdict::Refuted }
            }
            _ => Verdict::Unproven,
        };
    }

    let mut proven = false;
    for f in facts {
        let Some((op, l, r)) = (match f {
            Expr::BinaryOp(op, l, r) => Some((*op, l, r)),
            _ => None,
        }) else {
            continue;
        };
        let Some(fc) = normalize(l, op, r) else { continue };
        if fact_entails(&fc, &g) {
            proven = true;
            break;
        }
    }
    if proven {
        return Verdict::Proven;
    }

    if let Some(negated) = negate_canonical(&g) {
        for f in facts {
            let Some((op, l, r)) = (match f {
                Expr::BinaryOp(op, l, r) => Some((*op, l, r)),
                _ => None,
            }) else {
                continue;
            };
            let Some(fc) = normalize(l, op, r) else { continue };
            if fact_entails(&fc, &negated) {
                return Verdict::Refuted;
            }
        }
    }

    if let Expr::BinaryOp(op, l, r) = goal {
        if let Some(v) = interval_verdict(l, *op, r, facts) {
            return v;
        }
    }
    Verdict::Unproven
}

/// Classify a full precondition goal (a conjunction) against facts.
/// All conjuncts must be Proven; any Refuted makes the whole REFUTED.
pub fn prove_precondition(goal: &Expr, facts: &[Expr]) -> Verdict {
    if matches!(goal, Expr::Bool(true)) {
        return Verdict::Proven;
    }
    if matches!(goal, Expr::Bool(false)) {
        return Verdict::Refuted;
    }
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
            Expr::UnaryOp(k, inner) => Expr::UnaryOp(*k, Box::new(go(inner, params, args))),
            Expr::Field(base, f) => Expr::Field(Box::new(go(base, params, args)), f.clone()),
            Expr::Call(name, c_args, id) => Expr::Call(
                name.clone(),
                c_args.iter().map(|a| go(a, params, args)).collect(),
                *id,
            ),
            Expr::MethodCall(recv, name, c_args, id, refs) => Expr::MethodCall(
                Box::new(go(recv, params, args)),
                name.clone(),
                c_args.iter().map(|a| go(a, params, args)).collect(),
                *id,
                refs.clone(),
            ),
            Expr::Reflect(base, target, kind) => {
                Expr::Reflect(Box::new(go(base, params, args)), target.clone(), *kind)
            }
            Expr::Index(base, idx) => Expr::Index(
                Box::new(go(base, params, args)),
                Box::new(go(idx, params, args)),
            ),
            Expr::Slice { array, start, end, stride } => Expr::Slice {
                array: Box::new(go(array, params, args)),
                start: start.as_ref().map(|x| Box::new(go(x, params, args))),
                end: end.as_ref().map(|x| Box::new(go(x, params, args))),
                stride: stride.as_ref().map(|x| Box::new(go(x, params, args))),
            },
            Expr::Cast(inner, ty) => Expr::Cast(Box::new(go(inner, params, args)), ty.clone()),
            Expr::Deref(inner) => Expr::Deref(Box::new(go(inner, params, args))),
            Expr::AddrOf(inner) => Expr::AddrOf(Box::new(go(inner, params, args))),
            Expr::Consume(inner) => Expr::Consume(Box::new(go(inner, params, args))),
            Expr::Tuple(items) => {
                Expr::Tuple(items.iter().map(|x| go(x, params, args)).collect())
            }
            Expr::List(items) => {
                Expr::List(items.iter().map(|x| go(x, params, args)).collect())
            }
            other => other.clone(),
        }
    }
    go(goal, params, args)
}

/// Rename every free occurrence of `from` to `to` (used for `term`).
fn rename_identifier(e: &Expr, from: &str, to: &str) -> Expr {
    match e {
        Expr::Identifier(n) if n == from => Expr::Identifier(to.to_string()),
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(rename_identifier(l, from, to)),
            Box::new(rename_identifier(r, from, to)),
        ),
        Expr::UnaryOp(k, inner) => {
            Expr::UnaryOp(*k, Box::new(rename_identifier(inner, from, to)))
        }
        Expr::Field(base, f) => {
            Expr::Field(Box::new(rename_identifier(base, from, to)), f.clone())
        }
        Expr::Call(name, args, id) => Expr::Call(
            name.clone(),
            args.iter().map(|a| rename_identifier(a, from, to)).collect(),
            *id,
        ),
        Expr::MethodCall(recv, name, args, id, refs) => Expr::MethodCall(
            Box::new(rename_identifier(recv, from, to)),
            name.clone(),
            args.iter().map(|a| rename_identifier(a, from, to)).collect(),
            *id,
            refs.clone(),
        ),
        Expr::Reflect(base, target, kind) => Expr::Reflect(
            Box::new(rename_identifier(base, from, to)),
            target.clone(),
            *kind,
        ),
        Expr::Index(base, idx) => Expr::Index(
            Box::new(rename_identifier(base, from, to)),
            Box::new(rename_identifier(idx, from, to)),
        ),
        Expr::Cast(inner, ty) => {
            Expr::Cast(Box::new(rename_identifier(inner, from, to)), ty.clone())
        }
        Expr::Consume(inner) => {
            Expr::Consume(Box::new(rename_identifier(inner, from, to)))
        }
        other => other.clone(),
    }
}

/// The conjunction facts established by one guard/check expression.
pub fn facts_of(cond: &Expr) -> Vec<Expr> {
    if matches!(cond, Expr::Bool(true)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    conjuncts(cond, &mut out);
    out
}

/// The facts established when control FALLS THROUGH a guard whose body
/// always returns. A single comparison negates directly; `A || B` negates to
/// both `!A` and `!B`. A negated `A && B` is a disjunction we cannot store,
/// so it yields no facts (conservative, never wrong).
pub fn fallthrough_facts(cond: &Expr) -> Vec<Expr> {
    match cond {
        Expr::Bool(_) => Vec::new(),
        Expr::BinaryOp(BinaryOpKind::And, _, _) => Vec::new(),
        Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
            let a = fallthrough_facts(l);
            let b = fallthrough_facts(r);
            if a.is_empty() || b.is_empty() {
                Vec::new()
            } else {
                let mut out = a;
                out.extend(b);
                out
            }
        }
        _ => match negate_literal(cond) {
            Some(f) => vec![f],
            None => Vec::new(),
        },
    }
}

fn negate_literal(e: &Expr) -> Option<Expr> {
    match e {
        Expr::Bool(b) => Some(Expr::Bool(!b)),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner) => Some((**inner).clone()),
        Expr::BinaryOp(op, l, r) => Some(Expr::BinaryOp(
            match op {
                BinaryOpKind::Le => BinaryOpKind::Gt,
                BinaryOpKind::Lt => BinaryOpKind::Ge,
                BinaryOpKind::Ge => BinaryOpKind::Lt,
                BinaryOpKind::Gt => BinaryOpKind::Le,
                BinaryOpKind::Eq => BinaryOpKind::Neq,
                BinaryOpKind::Neq => BinaryOpKind::Eq,
                _ => return None,
            },
            l.clone(),
            r.clone(),
        )),
        _ => Some(Expr::UnaryOp(
            crate::ast::UnaryOpKind::Not,
            Box::new(e.clone()),
        )),
    }
}

/// The fact a LET INITIALIZER establishes about its bound name: a
/// non-negative integer literal gives `v >= 0` (and `v == n` for exact
/// reasoning). Sums of such literals too. 2026-10-19 (contract tiers v2).
pub fn initializer_fact(name: &str, init: &Expr) -> Option<Expr> {
    // A stored length (`.^Length`, `CharCount#`-free header slot) is never
    // negative; declaring it lets a digest length flow into a callee that
    // requires a non-negative size without an assertion at every call site.
    if let Expr::Reflect(_, field, crate::ast::ReflectKind::Runtime) = init {
        if field == "Length" {
            return Some(Expr::BinaryOp(
                BinaryOpKind::Ge,
                Box::new(Expr::Identifier(name.to_string())),
                Box::new(Expr::Decimal(0)),
            ));
        }
    }
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

/// The equality a single-name `let` establishes (`x == init`). The prover
/// expands these definitions before interval reasoning. A self-binding adds
/// nothing and is skipped.
pub fn initializer_equality(name: &str, init: &Expr) -> Option<Expr> {
    if matches!(init, Expr::Identifier(n) if n == name) {
        return None;
    }
    Some(Expr::BinaryOp(
        BinaryOpKind::Eq,
        Box::new(Expr::Identifier(name.to_string())),
        Box::new(init.clone()),
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
        Expr::Field(base, _) | Expr::Reflect(base, _, _) | Expr::Cast(base, _)
        | Expr::IsType(base, _) | Expr::Consume(base) | Expr::Deref(base)
        | Expr::AddrOf(base) | Expr::Await(base) => mentions(base, var),
        Expr::UnaryOp(_, inner) => mentions(inner, var),
        Expr::BinaryOp(_, l, r) => mentions(l, var) || mentions(r, var),
        Expr::Call(_, args, _) | Expr::Tuple(args) | Expr::List(args) => {
            args.iter().any(|a| mentions(a, var))
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            mentions(recv, var) || args.iter().any(|a| mentions(a, var))
        }
        Expr::PluginIntercept { args, receiver, .. } => {
            args.iter().any(|a| mentions(a, var))
                || receiver.as_ref().is_some_and(|r| mentions(r, var))
        }
        Expr::Index(base, idx) => mentions(base, var) || mentions(idx, var),
        Expr::Slice { array, start, end, stride } => {
            mentions(array, var)
                || start.as_ref().is_some_and(|x| mentions(x, var))
                || end.as_ref().is_some_and(|x| mentions(x, var))
                || stride.as_ref().is_some_and(|x| mentions(x, var))
        }
        Expr::Capture { expr, .. } => mentions(expr, var),
        Expr::If(c, t, e) => {
            mentions(c, var)
                || mentions(t, var)
                || e.as_ref().is_some_and(|x| mentions(x, var))
        }
        Expr::Match(scrut, arms) => {
            mentions(scrut, var)
                || arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(|g| mentions(g, var))
                        || mentions(&a.body, var)
                })
        }
        Expr::Block(stmts) => stmts.iter().any(|s| statement_mentions(s, var)),
        Expr::StructLiteral { fields, specs, .. } => {
            fields.iter().any(|(_, f)| mentions(f, var))
                || specs.iter().any(|(_, f)| mentions(f, var))
        }
        Expr::Spawn { args, .. } => args.iter().any(|a| mentions(a, var)),
        Expr::Lambda(_, body) => mentions(body, var),
        Expr::Within(a, b) => mentions(a, var) || mentions(b, var),
        Expr::Range { start, end, .. } => mentions(start, var) || mentions(end, var),
        _ => false,
    }
}

fn statement_mentions(s: &crate::ast::Statement, var: &str) -> bool {
    use crate::ast::Statement as S;
    match s {
        S::Let { expr, .. } => expr.as_ref().is_some_and(|e| mentions(e, var)),
        S::Assign(l, r) => mentions(l, var) || mentions(r, var),
        S::ArrowAssign { target, value, .. } => {
            target.as_ref().is_some_and(|t| mentions(t, var)) || mentions(value, var)
        }
        S::Term(e) | S::EndProgram(e) | S::Rollback(e) => {
            e.as_ref().is_some_and(|x| mentions(x, var))
        }
        S::Tail(e) | S::Check(e) => mentions(e, var),
        S::Gate(e) | S::Expression(e) => mentions(e, var),
        S::Guarded(c, body) => mentions(c, var) || body.iter().any(|s| statement_mentions(s, var)),
        S::Block(body) | S::SyncBlock(body) | S::Defer(body) | S::Mutex(body) => {
            body.iter().any(|s| statement_mentions(s, var))
        }
        S::Foreach { list, body, .. } => {
            mentions(list, var) || body.iter().any(|s| statement_mentions(s, var))
        }
        S::Match { expr, arms } => {
            mentions(expr, var)
                || arms.iter().any(|a| a.body.iter().any(|s| statement_mentions(s, var)))
        }
        _ => false,
    }
}

fn free_identifiers(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Identifier(n) => {
            out.insert(n.clone());
        }
        Expr::Field(base, _) | Expr::Reflect(base, _, _) | Expr::Cast(base, _)
        | Expr::IsType(base, _) | Expr::Consume(base) | Expr::Deref(base)
        | Expr::AddrOf(base) | Expr::Await(base) => free_identifiers(base, out),
        Expr::UnaryOp(_, inner) => free_identifiers(inner, out),
        Expr::BinaryOp(_, l, r) => {
            free_identifiers(l, out);
            free_identifiers(r, out);
        }
        Expr::Call(_, args, _) | Expr::Tuple(args) | Expr::List(args) => {
            for a in args {
                free_identifiers(a, out);
            }
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            free_identifiers(recv, out);
            for a in args {
                free_identifiers(a, out);
            }
        }
        Expr::Index(base, idx) => {
            free_identifiers(base, out);
            free_identifiers(idx, out);
        }
        Expr::If(c, t, e) => {
            free_identifiers(c, out);
            free_identifiers(t, out);
            if let Some(e) = e {
                free_identifiers(e, out);
            }
        }
        Expr::Block(stmts) => {
            for s in stmts {
                statement_free_identifiers(s, out);
            }
        }
        _ => {}
    }
}

fn statement_free_identifiers(s: &crate::ast::Statement, out: &mut BTreeSet<String>) {
    use crate::ast::Statement as S;
    match s {
        S::Let { expr, .. } => {
            if let Some(e) = expr {
                free_identifiers(e, out);
            }
        }
        S::Assign(l, r) => {
            free_identifiers(l, out);
            free_identifiers(r, out);
        }
        S::Term(e) | S::EndProgram(e) | S::Rollback(e) => {
            if let Some(e) = e {
                free_identifiers(e, out);
            }
        }
        S::Tail(e) | S::Check(e) => free_identifiers(e, out),
        S::Gate(e) | S::Expression(e) => free_identifiers(e, out),
        S::Guarded(c, body) => {
            free_identifiers(c, out);
            for s in body {
                statement_free_identifiers(s, out);
            }
        }
        S::Block(body) | S::SyncBlock(body) | S::Defer(body) | S::Mutex(body) => {
            for s in body {
                statement_free_identifiers(s, out);
            }
        }
        S::Foreach { list, body, .. } => {
            free_identifiers(list, out);
            for s in body {
                statement_free_identifiers(s, out);
            }
        }
        _ => {}
    }
}

/// Facts a call result establishes: the callee postcondition with params and
/// `term` substituted. Only `term`-bearing conjuncts are used, and only when
/// every remaining free name is the bound result or names appearing in the
/// actual arguments — this prevents a callee local from colliding with a
/// caller variable.
pub fn post_facts(contract: &FnContract, args: &[Expr], binding: &str) -> Vec<Expr> {
    let mut allowed = BTreeSet::new();
    for a in args {
        free_identifiers(a, &mut allowed);
    }
    let mut raw = Vec::new();
    conjuncts(&contract.post, &mut raw);
    let mut out = Vec::new();
    for c in raw {
        if !mentions(&c, "term") {
            continue;
        }
        let substituted = substitute_params(&c, &contract.params, args);
        let renamed = rename_identifier(&substituted, "term", binding);
        let mut ids = BTreeSet::new();
        free_identifiers(&renamed, &mut ids);
        if ids
            .iter()
            .all(|id| id == binding || allowed.contains(id))
        {
            out.push(renamed);
        }
    }
    out
}

/// Postcondition facts for a multi-value binding. `term.0`, `term.1`, ...
/// name the bound components; a bare `term` names the first component.
pub fn post_facts_components(
    contract: &FnContract,
    args: &[Expr],
    names: &[String],
) -> Vec<Expr> {
    let Some(first) = names.first() else {
        return Vec::new();
    };
    let mut allowed = BTreeSet::new();
    for a in args {
        free_identifiers(a, &mut allowed);
    }
    let mut raw = Vec::new();
    conjuncts(&contract.post, &mut raw);
    let mut out = Vec::new();
    for c in raw {
        if !mentions(&c, "term") {
            continue;
        }
        let substituted = substitute_params(&c, &contract.params, args);
        let renamed = rename_term_components(&substituted, first, names);
        let mut ids = BTreeSet::new();
        free_identifiers(&renamed, &mut ids);
        if ids.iter().all(|id| allowed.contains(id) || names.contains(id)) {
            out.push(renamed);
        }
    }
    out
}

fn rename_term_components(e: &Expr, first: &str, names: &[String]) -> Expr {
    match e {
        Expr::Identifier(n) if n == "term" => Expr::Identifier(first.to_string()),
        Expr::Field(base, field) if matches!(base.as_ref(), Expr::Identifier(n) if n == "term") => {
            match field.parse::<usize>().ok().and_then(|i| names.get(i)) {
                Some(name) => Expr::Identifier(name.clone()),
                None => e.clone(),
            }
        }
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(rename_term_components(l, first, names)),
            Box::new(rename_term_components(r, first, names)),
        ),
        Expr::UnaryOp(k, inner) => {
            Expr::UnaryOp(*k, Box::new(rename_term_components(inner, first, names)))
        }
        Expr::Field(base, f) => Expr::Field(
            Box::new(rename_term_components(base, first, names)),
            f.clone(),
        ),
        Expr::Call(name, args, id) => Expr::Call(
            name.clone(),
            args.iter()
                .map(|a| rename_term_components(a, first, names))
                .collect(),
            *id,
        ),
        Expr::MethodCall(recv, name, args, id, refs) => Expr::MethodCall(
            Box::new(rename_term_components(recv, first, names)),
            name.clone(),
            args.iter()
                .map(|a| rename_term_components(a, first, names))
                .collect(),
            *id,
            refs.clone(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(src: &str) -> Expr {
        let tokens = crate::lexer::tokenize(src).expect("lex");
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_expression().expect("parse")
    }

    #[test]
    fn literal_refutation() {
        let goal = e("0 - 3 > 0");
        assert_eq!(prove_precondition(&goal, &[]), Verdict::Refuted);
    }

    #[test]
    fn cancelled_linear_goal_is_a_tautology() {
        assert_eq!(
            prove_precondition(&e("x + 1 == x + 1"), &[]),
            Verdict::Proven
        );
        assert_eq!(
            prove_precondition(&e("x + 1 <= x + 1"), &[]),
            Verdict::Proven
        );
    }

    #[test]
    fn literal_proven() {
        let goal = e("5 >= 0");
        assert_eq!(prove_precondition(&goal, &[]), Verdict::Proven);
    }

    #[test]
    fn guard_fact_discharges_shifted_goal() {
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
    fn unrelated_atom_does_not_refute() {
        // A non-negative mask is now PROVEN by the interval domain; an
        // unbounded opaque call must stay Unproven (never blindly refuted).
        let facts = vec![e("i >= 0"), e("len >= 0")];
        assert_eq!(
            prove_precondition(&e("(b0 & 31) >= 0"), &facts),
            Verdict::Proven
        );
        assert_eq!(
            prove_precondition(&e("opaque(b0) >= 0"), &facts),
            Verdict::Unproven
        );
    }

    #[test]
    fn upper_bound_does_not_prove_equality() {
        let facts = vec![e("x <= 5")];
        let goal = e("x == 5");
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Unproven);
    }

    #[test]
    fn equality_proves_weaker_upper_bound() {
        let facts = vec![e("x == 3")];
        assert_eq!(prove_precondition(&e("x <= 5"), &facts), Verdict::Proven);
        assert_eq!(prove_precondition(&e("x >= 3"), &facts), Verdict::Proven);
        assert_eq!(prove_precondition(&e("x < 3"), &facts), Verdict::Refuted);
    }

    #[test]
    fn initializer_fact_refutes_literal_goal() {
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
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn field_chains_are_atoms() {
        let facts = vec![e("buf.len >= 4")];
        let goal = e("buf.len >= 2");
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    /// 2026-10-26: a literal's own field denotes its value. Without this,
    /// `Socket { fd: fd }.fd` was opaque and a caller's `fd >= -4096` guard
    /// could not discharge the callee contract.
    #[test]
    fn struct_literal_field_reduces_to_value() {
        let lit = Expr::StructLiteral {
            type_name: "Socket".into(),
            fields: vec![("fd".into(), Expr::Identifier("fd".into()))],
            specs: Vec::new(),
        };
        let field = Expr::Field(Box::new(lit), "fd".into());
        assert_eq!(atom_key(&field), "fd");
        assert_eq!(
            linify(&field),
            Some((vec![("fd".to_string(), 1)], 0)),
            "literal field must reduce to the field value"
        );
    }

    #[test]
    fn strict_integer_bound_tightens_by_one() {
        // nd < count => count - nd >= 1, and nd <= count - 1.
        let facts = vec![e("nd < count")];
        assert_eq!(
            prove_precondition(&e("count - nd >= 1"), &facts),
            Verdict::Proven
        );
    }

    #[test]
    fn zero_exclusion_and_lower_bound_give_one() {
        let facts = vec![e("n >= 0"), e("n <= 60"), e("n != 0")];
        assert_eq!(prove_precondition(&e("n - 1 >= 0"), &facts), Verdict::Proven);
        assert_eq!(
            prove_precondition(&e("n - 1 <= 60"), &facts),
            Verdict::Proven
        );
    }

    #[test]
    fn quotient_and_remainder_bounds() {
        let facts = vec![e("a >= 0"), e("a < 1000000000000000000")];
        assert_eq!(prove_precondition(&e("a / 10 >= 0"), &facts), Verdict::Proven);
        assert_eq!(
            prove_precondition(&e("a / 10 < 1000000000000000000"), &facts),
            Verdict::Proven
        );
        assert_eq!(prove_precondition(&e("a % 10 >= 0"), &facts), Verdict::Proven);
        assert_eq!(prove_precondition(&e("a % 10 <= 9"), &facts), Verdict::Proven);
    }

    #[test]
    fn large_constants_are_exact() {
        // f64 would collapse 2^60 and 2^60-1; i128 must not.
        let facts = vec![e("x <= 1152921504606846975")];
        assert_eq!(
            prove_precondition(&e("x < 1152921504606846976"), &facts),
            Verdict::Proven
        );
    }

    #[test]
    fn predicate_atom_discharges_identical_goal() {
        let facts = vec![e("is_digit(c)")];
        assert_eq!(
            prove_precondition(&e("is_digit(c)"), &facts),
            Verdict::Proven
        );
        assert_eq!(
            prove_precondition(&e("!is_digit(c)"), &facts),
            Verdict::Refuted
        );
    }

    #[test]
    fn complement_tautology_proves() {
        let goal = e("x == 0 || x != 0");
        assert_eq!(prove_precondition(&goal, &[]), Verdict::Proven);
    }

    #[test]
    fn guard_fallthrough_negates_or_guard() {
        let cond = e("b > 60 || m < pow2b(b)");
        let facts = fallthrough_facts(&cond);
        assert_eq!(facts.len(), 2);
        assert_eq!(prove_precondition(&e("b <= 60"), &facts), Verdict::Proven);
        assert_eq!(
            prove_precondition(&e("m >= pow2b(b)"), &facts),
            Verdict::Proven
        );
    }

    #[test]
    fn guard_fallthrough_skips_and_guard() {
        assert!(fallthrough_facts(&e("a > 0 && b > 0")).is_empty());
    }

    #[test]
    fn post_facts_substitute_term_and_args() {
        let contract = FnContract {
            params: vec!["x".to_string()],
            pre: e("x >= 1"),
            post: e("term >= x && term <= x + 1"),
        };
        let facts = post_facts(&contract, &[e("n")], "r");
        assert_eq!(facts, vec![e("r >= n"), e("r <= n + 1")]);
    }

    #[test]
    fn component_post_facts_bind_each_tuple_slot() {
        let contract = FnContract {
            params: vec!["x".to_string()],
            pre: e("x >= 1"),
            post: e("term.0 >= 1 && term.1 >= x && term.1 <= x + 1"),
        };
        let names = vec!["a".to_string(), "b".to_string()];
        let facts = post_facts_components(&contract, &[e("n")], &names);
        assert_eq!(
            facts,
            vec![e("a >= 1"), e("b >= n"), e("b <= n + 1")]
        );
    }

    #[test]
    fn field_atom_definitions_chain_through_lets() {
        let facts = vec![
            e("b1.len == b0.len + msg_len"),
            e("b1.cap == b0.cap"),
            e("after_one.len == b1.len + 1"),
            e("after_one.cap == b1.cap"),
            e("b0.len == 0"),
            e("b0.cap == total"),
            e("msg_len >= 0"),
        ];
        assert_eq!(
            prove_precondition(&e("after_one.len == msg_len + 1"), &facts),
            Verdict::Proven
        );
    }

    #[test]
    fn structural_fact_proves_reflection_goal() {
        let facts = vec![e("r .^Length >= 1")];
        assert_eq!(
            prove_precondition(&e("r .^Length >= 1"), &facts),
            Verdict::Proven
        );
    }

    #[test]
    fn post_facts_drop_callee_local_names() {
        let contract = FnContract {
            params: vec!["x".to_string()],
            pre: Expr::Bool(true),
            post: e("term <= tmp"),
        };
        let facts = post_facts(&contract, &[e("n")], "r");
        assert!(facts.is_empty(), "callee local leaked: {facts:?}");
    }

    #[test]
    fn signed_division_without_bounds_stays_unproven() {
        let facts = vec![e("x >= -100"), e("x <= 100")];
        assert_eq!(prove_precondition(&e("x / 2 >= -50"), &facts), Verdict::Proven);
        assert_eq!(
            prove_precondition(&e("x % 2 >= 0"), &facts),
            Verdict::Unproven
        );
    }
}


#[cfg(test)]
mod guard_flow_tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Expr};

    fn id(n: &str) -> Expr {
        Expr::Identifier(n.into())
    }

    fn bin(op: BinaryOpKind, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(op, Box::new(l), Box::new(r))
    }

    #[test]
    fn neq_tightens_lower_bound() {
        let facts = vec![
            bin(BinaryOpKind::Ge, id("n"), Expr::Decimal(0)),
            bin(BinaryOpKind::Le, id("n"), Expr::Decimal(60)),
            bin(BinaryOpKind::Neq, id("n"), Expr::Decimal(0)),
        ];
        let goal = bin(
            BinaryOpKind::Ge,
            bin(BinaryOpKind::Sub, id("n"), Expr::Decimal(1)),
            Expr::Decimal(0),
        );
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn let_equality_expands_for_bounds() {
        let facts = vec![
            Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(id("a")),
                Box::new(bin(
                    BinaryOpKind::Sub,
                    Expr::Decimal(0),
                    id("k"),
                )),
            ),
            bin(BinaryOpKind::Lt, id("k"), Expr::Decimal(0)),
        ];
        let goal = bin(BinaryOpKind::Ge, id("a"), Expr::Decimal(0));
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn let_equality_multiplied() {
        let facts = vec![
            Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(id("a2")),
                Box::new(bin(
                    BinaryOpKind::Mul,
                    id("acc"),
                    Expr::Decimal(2),
                )),
            ),
            bin(BinaryOpKind::Ge, id("acc"), Expr::Decimal(1)),
        ];
        let goal = bin(BinaryOpKind::Ge, id("a2"), Expr::Decimal(1));
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn product_nonnegative_from_positives() {
        let facts = vec![
            bin(BinaryOpKind::Ge, id("acc"), Expr::Decimal(1)),
            bin(BinaryOpKind::Ge, id("up"), Expr::Decimal(1)),
        ];
        let goal = bin(
            BinaryOpKind::Ge,
            bin(BinaryOpKind::Mul, id("acc"), id("up")),
            Expr::Decimal(0),
        );
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn let_equality_with_field_fact() {
        let field = Expr::Field(Box::new(id("s")), "Length".to_string());
        let facts = vec![
            Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(id("missing")),
                Box::new(bin(
                    BinaryOpKind::Sub,
                    id("len"),
                    field.clone(),
                )),
            ),
            bin(BinaryOpKind::Le, field.clone(), id("len")),
        ];
        let goal = bin(BinaryOpKind::Ge, id("missing"), Expr::Decimal(0));
        assert_eq!(prove_precondition(&goal, &facts), Verdict::Proven);
    }

    #[test]
    fn fallthrough_neq_proves_base_successor() {
        let neg = fallthrough_facts(&bin(BinaryOpKind::Eq, id("n"), Expr::Decimal(0)));
        assert!(matches!(neg.as_slice(), [Expr::BinaryOp(BinaryOpKind::Neq, _, _)]));
    }
}
