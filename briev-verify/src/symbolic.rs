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
//
// Runtime Exception for Use as a Language:
// When the Work or any Derivative Work thereof is used to generate code
// ("generated code"), such generated code shall not be subject to the
// terms of this License, provided that the generated code itself is not
// a Derivative Work of the Work. This exception does not apply to code
// that is itself a compiler, interpreter, or similar tool that incorporates
// or embeds the Work.

/// Symbolic Executor for Assignment Tracking and Postcondition Verification
///
/// This module provides Level 2 symbolic execution capabilities:
/// - Tracks variable assignments symbolically (literals, identifiers, arithmetic)
/// - Handles prior-state comparisons with @ operator
/// - Evaluates postconditions against symbolic state
/// - Enumerates execution paths through guard blocks
///
/// Coverage: ~90% of real Briev contracts
use crate::ast::{BinaryOpKind, Expr, ReflectKind, Statement, UnaryOpKind};
use std::collections::HashMap;

pub(crate) mod snapshot;

/// Symbolic representation of a value
/// Represents what a variable could be given current state
#[derive(Debug, Clone, PartialEq)]
pub enum SymbolicValue {
    /// Literal constant (5, true, "hello", etc.)
    Literal(i64, String), // (value, type_hint: "int", "bool", "float", etc.)

    /// Immutable entry atom captured when this value was evaluated. Never
    /// resolve it again through the mutable assignment map.
    Identifier(String),

    /// Prior state value (before execution)
    Previous(String),

    /// Binary operation: op(left, right)
    Binary(String, Box<SymbolicValue>, Box<SymbolicValue>), // (op: "+", "-", "*", etc.)

    /// Unknown value (can't track)
    Unknown,
}

impl SymbolicValue {
    /// Create a symbolic value from an integer literal
    pub fn int_literal(n: i64) -> Self {
        SymbolicValue::Literal(n, "int".to_string())
    }

    /// Create a symbolic value from a boolean literal
    pub fn bool_literal(b: bool) -> Self {
        SymbolicValue::Literal(if b { 1 } else { 0 }, "bool".to_string())
    }

    /// Check if this value is definitely true (for boolean simplification)
    pub fn is_definitely_true(&self) -> bool {
        matches!(self, SymbolicValue::Literal(1, _))
    }

    /// Check if this value is definitely false
    pub fn is_definitely_false(&self) -> bool {
        matches!(self, SymbolicValue::Literal(0, _))
    }

    /// Try to evaluate this symbolic value to a concrete integer literal.
    ///
    /// Returns `Some(i64)` when the value is or simplifies to a concrete integer,
    /// allowing `rel_verdict` to decide `Binary <op> Literal` comparisons
    /// structurally without routing to Z3. Handles the common nested case
    /// `(x+0)+1` directly by binding inner literals and folding immediately.
    pub fn try_to_literal(&self) -> Option<i64> {
        match self {
            SymbolicValue::Literal(n, _) => Some(*n),
            SymbolicValue::Binary(op, l, r) => {
                // Fast path: both operands are concrete literals.
                if let (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) = (&**l, &**r) {
                    return fold_integer(op, *a, *b);
                }
                // Nested path: (x ± c1) ± c2  →  evaluate inner first, then fold outer.
                // Covers expressions like `(a+0)+1` where the inner result is still a Binary.
                if let Some(ref a) = l.try_to_literal() {
                    if let SymbolicValue::Literal(b, _) = &**r {
                        return fold_integer(op, *a, *b);
                    }
                }
                if let Some(ref b) = r.try_to_literal() {
                    if let SymbolicValue::Literal(a, _) = &**l {
                        return fold_integer(op, *a, *b);
                    }
                }
                // Fallback: let simplify_binary handle any other algebraic identity.
                simplify_binary(op, l, r)?.try_to_literal()
            }
            _ => None,
        }
    }
}

/// State during symbolic execution of a path
#[derive(Debug, Clone)]
pub struct SymbolicState {
    /// Mapping of variable -> its symbolic value
    pub assignments: HashMap<String, SymbolicValue>,

    /// Constraints (guards) from this path
    pub path_constraints: Vec<Expr>,

    /// Field assignments: (base-identifier, field-name) -> symbolic value.
    /// Populated when the body assigns to `x.field = expr`.  Allows
    /// `eval_symbolic(x.field)` to resolve the stored value structurally
    /// rather than routing to Z3 for field-projection postconditions.
    field_assignments: HashMap<(String, String), SymbolicValue>,
}

impl SymbolicState {
    /// Create new state from precondition
    pub fn new(precondition: &Expr) -> Self {
        SymbolicState {
            assignments: HashMap::new(),
            path_constraints: vec![precondition.clone()],
            field_assignments: HashMap::new(),
        }
    }

    /// Create an empty state (for initialization)
    pub fn empty() -> Self {
        SymbolicState {
            assignments: HashMap::new(),
            path_constraints: Vec::new(),
            field_assignments: HashMap::new(),
        }
    }

    /// Record an assignment
    pub fn assign(&mut self, target: &str, value_expr: &Expr) {
        let symbolic_val = eval_symbolic(value_expr, self);
        self.invalidate_fields_for(target);
        self.assignments.insert(target.to_string(), symbolic_val);
    }

    /// Add a guard constraint (from [condition] guard block)
    pub fn add_constraint(&mut self, condition: &Expr, taken: bool) {
        let Some(fact) = snapshot::expression(condition, &self.assignments) else {
            // Unknown values cannot supply facts; omitting one only weakens a VC.
            return;
        };
        let fact = if taken {
            fact
        } else {
            Expr::UnaryOp(UnaryOpKind::Not, Box::new(fact))
        };
        self.path_constraints.push(fact);
    }

    /// Get the symbolic value for a variable, or None if unknown
    pub fn get_value(&self, name: &str) -> Option<SymbolicValue> {
        self.assignments.get(name).cloned()
    }

    /// Record an assignment to a struct field (x.field = expr).
    pub fn assign_field(&mut self, base: &str, field: &str, value_expr: &Expr) {
        let sym_val = eval_symbolic(value_expr, self);
        self.field_assignments
            .insert((base.to_string(), field.to_string()), sym_val);
    }

    /// Get the symbolic value for a field, or None if not tracked.
    pub fn get_field(&self, base: &str, field: &str) -> Option<SymbolicValue> {
        self.field_assignments
            .get(&(base.to_string(), field.to_string()))
            .cloned()
    }

    /// Drop all field entries whose base identifier was reassigned.
    ///
    /// Reassigning `x` (e.g. `x = y`) changes which object `x` points to, so any
    /// earlier `x.f = …` value is no longer valid for the new `x`.  Clearing the
    /// entries keeps the field map sound: a stale read falls back to `Unknown` and
    /// escalates to Z3 rather than silently using a wrong value.
    pub fn invalidate_fields_for(&mut self, base: &str) {
        self.field_assignments.retain(|(b, _), _| b != base);
    }
}

/// Evaluate an expression to a symbolic value
/// Returns Unknown if expression is too complex to track
pub fn eval_symbolic(expr: &Expr, state: &SymbolicState) -> SymbolicValue {
    match expr {
        // Literal values
        Expr::Char(c) => SymbolicValue::Literal(*c as i64, "int".to_string()),
        Expr::Decimal(n) | Expr::TaggedLiteral(n, _) => {
            SymbolicValue::Literal(*n, "int".to_string())
        }
        Expr::Float(_) | Expr::UnitLiteral { .. } => SymbolicValue::Unknown,
        Expr::Bool(b) => SymbolicValue::bool_literal(*b),
        Expr::BeginProgram => SymbolicValue::bool_literal(true),
        Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _) => SymbolicValue::Unknown,
        // 2026-09-28 (E3): the `[*]` selector is a leaf — no symbolic value.
        Expr::Wildcard => SymbolicValue::Unknown,

        // Variable references
        Expr::Identifier(name) => {
            if let Some(sym_val) = state.assignments.get(name) {
                sym_val.clone()
            } else {
                SymbolicValue::Identifier(name.clone())
            }
        }

        // A tracked field belongs to its current lexical variable. Following
        // the base's saved entry atom would read another variable's older field.
        Expr::Field(base, field) => {
            if let Expr::Identifier(name) = base.as_ref() {
                return state
                    .get_field(name, field)
                    .unwrap_or(SymbolicValue::Unknown);
            }
            SymbolicValue::Unknown
        }

        // Binary operations
        Expr::BinaryOp(BinaryOpKind::Add, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);

            if let Some(simplified) = simplify_binary("+", &left_sym, &right_sym) {
                simplified
            } else {
                SymbolicValue::Binary("+".to_string(), Box::new(left_sym), Box::new(right_sym))
            }
        }

        Expr::BinaryOp(BinaryOpKind::Sub, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);

            if let Some(simplified) = simplify_binary("-", &left_sym, &right_sym) {
                simplified
            } else {
                SymbolicValue::Binary("-".to_string(), Box::new(left_sym), Box::new(right_sym))
            }
        }

        Expr::BinaryOp(BinaryOpKind::Mul, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);

            if let Some(simplified) = simplify_binary("*", &left_sym, &right_sym) {
                simplified
            } else {
                SymbolicValue::Binary("*".to_string(), Box::new(left_sym), Box::new(right_sym))
            }
        }

        Expr::BinaryOp(BinaryOpKind::Div, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);

            if let Some(simplified) = simplify_binary("/", &left_sym, &right_sym) {
                simplified
            } else {
                SymbolicValue::Binary("/".to_string(), Box::new(left_sym), Box::new(right_sym))
            }
        }

        Expr::BinaryOp(BinaryOpKind::Mod, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);

            if let Some(simplified) = simplify_binary("%", &left_sym, &right_sym) {
                simplified
            } else {
                SymbolicValue::Binary("%".to_string(), Box::new(left_sym), Box::new(right_sym))
            }
        }

        Expr::BinaryOp(BinaryOpKind::BitAnd, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            SymbolicValue::Binary("&".to_string(), Box::new(left_sym), Box::new(right_sym))
        }

        Expr::BinaryOp(BinaryOpKind::BitOr, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            SymbolicValue::Binary("|".to_string(), Box::new(left_sym), Box::new(right_sym))
        }

        Expr::BinaryOp(BinaryOpKind::BitXor, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            SymbolicValue::Binary("^".to_string(), Box::new(left_sym), Box::new(right_sym))
        }

        Expr::BinaryOp(BinaryOpKind::Shl, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            SymbolicValue::Binary("<<".to_string(), Box::new(left_sym), Box::new(right_sym))
        }

        Expr::BinaryOp(BinaryOpKind::Shr, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            SymbolicValue::Binary(">>".to_string(), Box::new(left_sym), Box::new(right_sym))
        }

        Expr::BinaryOp(BinaryOpKind::Concat, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            SymbolicValue::Binary("++".to_string(), Box::new(left_sym), Box::new(right_sym))
        }

        // A handle's stored length (`s.^Length`, `CharCount#(s)`) is an integer
        // atom, distinct per handle and always non-negative. Modelled as
        // `atom & i64::MAX` so the existing mask range supplies `>= 0` — the same
        // machinery that bounds `x & MASK`, reused rather than duplicated.
        Expr::Reflect(base, target, ReflectKind::Runtime) if target == "Length" => {
            length_atom(base, state)
        }
        Expr::Call(name, args, _) if name == "CharCount#" && args.len() == 1 => {
            length_atom(&args[0], state)
        }

        // A whole-word load of `w` bytes is `zext i{8w}` at the backend (see
        // emit_load), so its value lies in `[0, 2^(8w) - 1]`. Modelling that
        // interval is what lets the stdlib's own byte contracts prove:
        // `byte_at` declares `[term >= 0 && term <= 255]` over `Load#(..., 1)`.
        //
        // The mask restriction is deliberate. `8w < 32` holds on every target
        // this compiler emits (wasm32 included); a 4-byte load is exactly the
        // word width there, so zero-extension is the identity and the value can
        // read as negative — claiming `[0, 2^32-1]` for it would be a false
        // proof. Wider or non-literal widths stay Unknown.
        Expr::Call(name, args, _) if name == "Load#" => load_range_value(expr, args),

        // Function calls - can't track
        Expr::Call(_, _, _) => SymbolicValue::Unknown,

        // Unary operations
        Expr::UnaryOp(UnaryOpKind::Neg, _)
        | Expr::UnaryOp(UnaryOpKind::Not, _)
        | Expr::UnaryOp(UnaryOpKind::BitNot, _) => SymbolicValue::Unknown,

        // Compound and complex expressions
        Expr::List(_)
        | Expr::Index(_, _)
        | Expr::Tuple(_)
        | Expr::If(_, _, _)
        | Expr::Match(_, _)
        | Expr::Block(_)
        | Expr::Lambda(_, _) => SymbolicValue::Unknown,

        // Comparison operators don't produce symbolic values (boolean expressions)
        Expr::BinaryOp(BinaryOpKind::Eq, _, _)
        | Expr::BinaryOp(BinaryOpKind::Neq, _, _)
        | Expr::BinaryOp(BinaryOpKind::Lt, _, _)
        | Expr::BinaryOp(BinaryOpKind::Le, _, _)
        | Expr::BinaryOp(BinaryOpKind::Gt, _, _)
        | Expr::BinaryOp(BinaryOpKind::Ge, _, _)
        | Expr::BinaryOp(BinaryOpKind::And, _, _)
        | Expr::BinaryOp(BinaryOpKind::Or, _, _) => SymbolicValue::Unknown,

        // Type-level operations
        Expr::Cast(_, _) | Expr::IsType(_, _) | Expr::Within(_, _) => SymbolicValue::Unknown,

        // Derivation and metadata
        Expr::DerivationBlock(_)
        | Expr::StructLiteral { .. }
        | Expr::Reflect(_, _, _)
        | Expr::MethodCall(..)
        | Expr::FormattingAnnotation(_) => SymbolicValue::Unknown,

        // Pointer dereference
        Expr::Deref(inner) => eval_symbolic(inner, state),
        Expr::Await(inner) => eval_symbolic(inner, state),
        // Address-of
        Expr::AddrOf(inner) => eval_symbolic(inner, state),
        Expr::Consume(inner) => eval_symbolic(inner, state),
        // 2026-07-19: Plugin-intercept calls — unknown at symbolic eval
        Expr::PluginIntercept { .. } => SymbolicValue::Unknown,
        Expr::Exists(_) => {
            unreachable!("fn? only in stage eval")
        }
        Expr::Slice { .. } => SymbolicValue::Unknown,
        Expr::Range { .. } => SymbolicValue::Unknown,
        Expr::Spawn { .. } => SymbolicValue::Unknown,
    }
}

/// Try to simplify a binary operation on symbolic values
pub(crate) fn simplify_binary(
    op: &str,
    left: &SymbolicValue,
    right: &SymbolicValue,
) -> Option<SymbolicValue> {
    if let (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) = (left, right) {
        return fold_integer(op, *a, *b).map(SymbolicValue::int_literal);
    }
    match (op, left, right) {
        // Identity and absorption rules for addition
        ("+", SymbolicValue::Literal(0, _), x) => Some(x.clone()),
        ("+", x, SymbolicValue::Literal(0, _)) => Some(x.clone()),
        ("-", x, SymbolicValue::Literal(0, _)) => Some(x.clone()),
        // 2026-08-28 (P1.5): additive cancellation — (x+c)−c → x, (x−c)+c → x,
        // x−x → 0. These make add/sub CastTo/CastFrom pairs provably
        // 1-to-1 (the P1.5 delta-collapse), which previously required z3
        // (absent from PATH → always failed).
        ("-", SymbolicValue::Binary(op, inner, r2), r1) if op == "+" => match (r1, r2.as_ref()) {
            (SymbolicValue::Literal(c1, _), SymbolicValue::Literal(c2, _)) if c1 == c2 => {
                Some((**inner).clone())
            }
            _ => None,
        },
        ("+", SymbolicValue::Binary(op, inner, r2), r1) if op == "-" => match (r1, r2.as_ref()) {
            (SymbolicValue::Literal(c1, _), SymbolicValue::Literal(c2, _)) if c1 == c2 => {
                Some((**inner).clone())
            }
            _ => None,
        },
        ("-", l, r) if symbolic_equals(l, r) => Some(SymbolicValue::int_literal(0)),

        // Identity and absorption rules for multiplication
        ("*", SymbolicValue::Literal(1, _), x) => Some(x.clone()),
        ("*", x, SymbolicValue::Literal(1, _)) => Some(x.clone()),
        ("*", SymbolicValue::Literal(0, _), _) => Some(SymbolicValue::int_literal(0)),
        ("*", _, SymbolicValue::Literal(0, _)) => Some(SymbolicValue::int_literal(0)),

        // Can't simplify further
        _ => None,
    }
}

/// Match interpreter/SMT wrapping arithmetic without host-profile overflow panics.
/// Exceptional division stays symbolic: native division is not a total operation.
/// The stored-length atom of a handle, as a non-negative integer.
///
/// Keyed on the base's **symbolic value**, not its text: `s.^Length` in two
/// places denotes one quantity, while `a.^Length` and `b.^Length` stay distinct —
/// and after a rebinding the value differs, so the old length does not carry
/// over. Keying on the identifier name alone would make those two mistakes
/// possible, and a length claim is exactly where such a mistake becomes a false
/// proof.
///
/// An unknown base has no length to speak of, so the caller gets `Unknown` rather
/// than an atom that would compare equal to itself for the wrong reason.
fn length_atom(base: &Expr, state: &SymbolicState) -> SymbolicValue {
    let value = eval_symbolic(base, state);
    if value == SymbolicValue::Unknown {
        return SymbolicValue::Unknown;
    }
    SymbolicValue::Binary(
        "&".to_string(),
        Box::new(SymbolicValue::Identifier(format!("len#{value:?}"))),
        Box::new(SymbolicValue::Literal(i64::MAX, "int".to_string())),
    )
}

/// The byte width a `Load#` call loads, when it is a literal in the modellable
/// range.
///
/// Returns `None` for a missing width (the backend then loads a full word), for
/// a non-literal width, and for widths whose bits reach 32 — see the call site in
/// `eval_symbolic` for why that boundary is the sound one.
pub(crate) fn load_width(args: &[Expr]) -> Option<i64> {
    let width = match args.get(1)? {
        Expr::Decimal(value) => *value,
        _ => return None,
    };
    (1..=3).contains(&width).then_some(width)
}

/// The symbolic value of a `Load#(addr, w)` call: an opaque atom masked to the
/// loaded width.
///
/// The atom is keyed by the node's identity rather than by its text so that two
/// textually identical loads stay distinct. Collapsing them would let `a - b`
/// over two separate loads fold to zero — a false proof, since a store between
/// them can change the value.
fn load_range_value(node: &Expr, args: &[Expr]) -> SymbolicValue {
    let Some(width) = load_width(args) else {
        return SymbolicValue::Unknown;
    };
    let mask = (1_i64 << (8 * width)) - 1;
    let atom = SymbolicValue::Identifier(format!("load#{:p}", node));
    SymbolicValue::Binary(
        "&".to_string(),
        Box::new(atom),
        Box::new(SymbolicValue::Literal(mask, "int".to_string())),
    )
}

fn fold_integer(op: &str, a: i64, b: i64) -> Option<i64> {
    match op {
        "+" => Some(a.wrapping_add(b)),
        "-" => Some(a.wrapping_sub(b)),
        "*" => Some(a.wrapping_mul(b)),
        "/" => a.checked_div(b),
        // Contract bounds are routinely written as shifts (`1 << 24`). Only an
        // in-range shift folds: an out-of-range one has no agreed word value,
        // so it stays symbolic rather than inventing a constant.
        "<<" => u32::try_from(b)
            .ok()
            .filter(|shift| *shift < 64)
            .and_then(|shift| a.checked_shl(shift)),
        ">>" => u32::try_from(b)
            .ok()
            .filter(|shift| *shift < 64)
            .and_then(|shift| a.checked_shr(shift)),
        _ => None,
    }
}

#[cfg(kani)]
mod arithmetic_proofs {
    use super::fold_integer;

    /// The production folds equal exact widened arithmetic reduced to 64 bits.
    #[kani::proof]
    fn integer_add_sub_folds_are_modular() {
        let a: i64 = kani::any();
        let b: i64 = kani::any();
        let wide_a = i128::from(a);
        let wide_b = i128::from(b);
        assert_eq!(fold_integer("+", a, b), Some((wide_a + wide_b) as i64));
        assert_eq!(fold_integer("-", a, b), Some((wide_a - wide_b) as i64));
        kani::cover!(a == i64::MAX && b == 1);
        kani::cover!(a == i64::MIN && b == -1);
    }

    /// Multiplication has its own solver budget; a timeout is not a proof.
    #[kani::proof]
    fn integer_mul_fold_is_modular() {
        let a: i64 = kani::any();
        let b: i64 = kani::any();
        assert_eq!(
            fold_integer("*", a, b),
            Some((i128::from(a) * i128::from(b)) as i64)
        );
    }

    /// Exceptional native divisions must never become invented literal results.
    #[kani::proof]
    fn exceptional_division_is_not_folded() {
        let a: i64 = kani::any();
        assert_eq!(fold_integer("/", a, 0), None);
        assert_eq!(fold_integer("/", i64::MIN, -1), None);
    }
}

/// Check if a postcondition is satisfied given symbolic state
pub fn satisfies_postcondition(post: &Expr, state: &SymbolicState) -> bool {
    match post {
        Expr::BinaryOp(BinaryOpKind::Eq, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            symbolic_equals(&left_sym, &right_sym)
        }

        Expr::BinaryOp(BinaryOpKind::Neq, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            !symbolic_equals(&left_sym, &right_sym)
        }

        Expr::BinaryOp(BinaryOpKind::Lt, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            symbolic_less_than(&left_sym, &right_sym)
        }

        Expr::BinaryOp(BinaryOpKind::Le, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            symbolic_less_than(&left_sym, &right_sym) || symbolic_equals(&left_sym, &right_sym)
        }

        Expr::BinaryOp(BinaryOpKind::Gt, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            symbolic_less_than(&right_sym, &left_sym)
        }

        Expr::BinaryOp(BinaryOpKind::Ge, left, right) => {
            let left_sym = eval_symbolic(left, state);
            let right_sym = eval_symbolic(right, state);
            symbolic_less_than(&right_sym, &left_sym) || symbolic_equals(&left_sym, &right_sym)
        }

        Expr::BinaryOp(BinaryOpKind::And, left, right) => {
            satisfies_postcondition(left, state) && satisfies_postcondition(right, state)
        }

        Expr::BinaryOp(BinaryOpKind::Or, left, right) => {
            satisfies_postcondition(left, state) || satisfies_postcondition(right, state)
        }

        Expr::Bool(b) => *b,

        Expr::UnaryOp(UnaryOpKind::Not, expr) => !satisfies_postcondition(expr, state),

        Expr::IsType(_, _) => false,

        Expr::UnitLiteral { .. } => false,
        _ => false,
    }
}

/// Check symbolic equality between two values
fn symbolic_equals(left: &SymbolicValue, right: &SymbolicValue) -> bool {
    match (left, right) {
        // Literal equality
        (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) => a == b,

        // Identical identifiers
        (SymbolicValue::Identifier(a), SymbolicValue::Identifier(b)) => a == b,

        // Identical prior-state references
        (SymbolicValue::Previous(a), SymbolicValue::Previous(b)) => a == b,

        // Identical binary expressions
        (SymbolicValue::Binary(op1, l1, r1), SymbolicValue::Binary(op2, l2, r2)) => {
            op1 == op2 && symbolic_equals(l1, l2) && symbolic_equals(r1, r2)
        }

        // Different types; not equal
        _ => false,
    }
}

/// Check symbolic less-than with basic numeric reasoning
fn symbolic_less_than(left: &SymbolicValue, right: &SymbolicValue) -> bool {
    match (left, right) {
        // Literal comparison
        (SymbolicValue::Literal(a, _), SymbolicValue::Literal(b, _)) => a < b,

        // Conservative for unknowns
        _ => false,
    }
}

/// Enumerate all possible execution paths through a statement block
/// Each path represents a sequence of statements with guards either taken or not taken
pub fn enumerate_paths(body: &[Statement]) -> Vec<SymbolicState> {
    let mut paths = vec![SymbolicState::empty()];

    for stmt in body {
        let mut new_paths = Vec::new();

        for mut state in paths {
            match stmt {
                Statement::Guarded(condition, statements) => {
                    // Path 1: Guard taken - execute the statements
                    let mut true_state = state.clone();
                    true_state.add_constraint(condition, true);

                    // Process all inner statements
                    for stmt in statements {
                        execute_statement(stmt, &mut true_state);
                    }
                    new_paths.push(true_state);

                    // Path 2: Guard not taken - skip the statement
                    let mut false_state = state;
                    false_state.add_constraint(condition, false);
                    new_paths.push(false_state);
                }

                Statement::Term(..)
                | Statement::EndProgram(..)
                | Statement::Rollback(_)
                | Statement::Tail(_) => {
                    new_paths.push(state);
                }

                _ => {
                    // Regular statement: execute on all paths
                    execute_statement(stmt, &mut state);
                    new_paths.push(state);
                }
            }
        }

        paths = new_paths;
    }

    paths
}

/// Execute a single statement on a symbolic state
fn execute_statement(stmt: &Statement, state: &mut SymbolicState) {
    match stmt {
        Statement::Assign(lhs, expr) => {
            if let Expr::Identifier(name) = lhs {
                state.assign(name, expr);
            }
        }

        Statement::Let { name, expr, .. } => {
            if let Some(e) = expr {
                state.assign(name, e);
            }
        }

        _ => {}
    }
}

/// 2026-07-16: P6 — Evaluate an expression symbolically given explicit input bindings.
/// Unlike eval_symbolic, this does NOT require a full SymbolicState — just a
/// HashMap of input bindings. Used by meld validation Layer 4.
/// 2026-08-28 (P1.5): normalize the op to the canonical "+"/"-"/"*"/"/"/"%"
/// symbols (matching simplify_binary) and SIMPLIFY at every node — the old
/// form produced `Binary("Sub", Binary("Add", x, 1), 1)` with no
/// cancellation, so `prove_composition_inverse` never collapsed (x+1)−1 to x
/// and the P1.5 delta-collapse never fired for add/sub inverse pairs.
pub fn eval_symbolic_expr(
    expr: &Expr,
    inputs: &std::collections::HashMap<String, SymbolicValue>,
) -> SymbolicValue {
    fn op_sym(kind: &BinaryOpKind) -> &'static str {
        match kind {
            BinaryOpKind::Add => "+",
            BinaryOpKind::Sub => "-",
            BinaryOpKind::Mul => "*",
            BinaryOpKind::Div => "/",
            BinaryOpKind::Mod => "%",
            _ => "?",
        }
    }
    match expr {
        Expr::Identifier(name) => inputs.get(name).cloned().unwrap_or(SymbolicValue::Unknown),
        Expr::Field(_, _) => SymbolicValue::Unknown,
        Expr::BinaryOp(kind, l, r) => {
            let lv = eval_symbolic_expr(l, inputs);
            let rv = eval_symbolic_expr(r, inputs);
            let sym = op_sym(kind);
            if let Some(simplified) = simplify_binary(sym, &lv, &rv) {
                simplified
            } else {
                SymbolicValue::Binary(sym.to_string(), Box::new(lv), Box::new(rv))
            }
        }
        Expr::Decimal(n) | Expr::TaggedLiteral(n, _) => {
            SymbolicValue::Literal(*n, "i64".to_string())
        }
        _ => SymbolicValue::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_creation() {
        let val = SymbolicValue::int_literal(5);
        assert_eq!(val, SymbolicValue::Literal(5, "int".to_string()));
    }

    #[test]
    fn test_literal_addition() {
        let left = SymbolicValue::int_literal(3);
        let right = SymbolicValue::int_literal(2);
        let result = simplify_binary("+", &left, &right);
        assert_eq!(result, Some(SymbolicValue::int_literal(5)));
    }

    #[test]
    fn test_literal_multiplication() {
        let left = SymbolicValue::int_literal(3);
        let right = SymbolicValue::int_literal(4);
        let result = simplify_binary("*", &left, &right);
        assert_eq!(result, Some(SymbolicValue::int_literal(12)));
    }

    #[test]
    fn test_identity_addition_zero() {
        let left = SymbolicValue::int_literal(0);
        let right = SymbolicValue::int_literal(5);
        let result = simplify_binary("+", &left, &right);
        assert_eq!(result, Some(SymbolicValue::int_literal(5)));
    }

    #[test]
    fn test_absorption_multiplication_zero() {
        let left = SymbolicValue::int_literal(0);
        let right = SymbolicValue::int_literal(999);
        let result = simplify_binary("*", &left, &right);
        assert_eq!(result, Some(SymbolicValue::int_literal(0)));
    }

    #[test]
    fn test_symbolic_equals_literals() {
        let left = SymbolicValue::int_literal(5);
        let right = SymbolicValue::int_literal(5);
        assert!(symbolic_equals(&left, &right));

        let right_diff = SymbolicValue::int_literal(3);
        assert!(!symbolic_equals(&left, &right_diff));
    }

    #[test]
    fn test_symbolic_less_than_literals() {
        let left = SymbolicValue::int_literal(3);
        let right = SymbolicValue::int_literal(5);
        assert!(symbolic_less_than(&left, &right));

        let reverse = SymbolicValue::int_literal(5);
        let base = SymbolicValue::int_literal(3);
        assert!(!symbolic_less_than(&reverse, &base));
    }

    #[test]
    fn test_state_assign_literal() {
        let mut state = SymbolicState::empty();
        state.assign("x", &Expr::Decimal(5));

        let val = state.get_value("x");
        assert_eq!(val, Some(SymbolicValue::int_literal(5)));
    }

    #[test]
    fn test_satisfies_postcondition_literal_equality() {
        let mut state = SymbolicState::empty();
        state.assign("x", &Expr::Decimal(5));

        let postcond = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Identifier("x".to_string())),
            Box::new(Expr::Decimal(5)),
        );

        assert!(satisfies_postcondition(&postcond, &state));
    }

    #[test]
    fn test_satisfies_postcondition_literal_inequality() {
        let mut state = SymbolicState::empty();
        state.assign("x", &Expr::Decimal(5));

        let postcond = Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Identifier("x".to_string())),
            Box::new(Expr::Decimal(3)),
        );

        assert!(!satisfies_postcondition(&postcond, &state));
    }

    #[test]
    fn test_satisfies_postcondition_conjunction() {
        let mut state = SymbolicState::empty();
        state.assign("x", &Expr::Decimal(5));
        state.assign("y", &Expr::Decimal(10));

        let postcond = Expr::BinaryOp(
            BinaryOpKind::And,
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(5)),
            )),
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Identifier("y".to_string())),
                Box::new(Expr::Decimal(10)),
            )),
        );

        assert!(satisfies_postcondition(&postcond, &state));
    }

    #[test]
    fn test_satisfies_postcondition_disjunction() {
        let mut state = SymbolicState::empty();
        state.assign("x", &Expr::Decimal(5));

        let postcond = Expr::BinaryOp(
            BinaryOpKind::Or,
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(5)),
            )),
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(3)),
            )),
        );

        assert!(satisfies_postcondition(&postcond, &state));
    }
}

#[cfg(all(feature = "kani", feature = "kani_full"))]
mod kani_full_tests {
    use super::*;

    #[kani::proof]
    fn verify_eval_symbolic_literal_integer() {
        let state = SymbolicState::empty();
        let expr = Expr::Decimal(42);
        let result = eval_symbolic(&expr, &state);
        assert!(matches!(result, SymbolicValue::Literal(42, _)));
    }

    #[kani::proof]
    fn verify_eval_symbolic_literal_bool_true() {
        let state = SymbolicState::empty();
        let expr = Expr::Bool(true);
        let result = eval_symbolic(&expr, &state);
        assert!(result.is_definitely_true());
    }

    #[kani::proof]
    fn verify_eval_symbolic_literal_bool_false() {
        let state = SymbolicState::empty();
        let expr = Expr::Bool(false);
        let result = eval_symbolic(&expr, &state);
        assert!(result.is_definitely_false());
    }

    #[kani::proof]
    fn verify_eval_symbolic_literal_float_is_unknown() {
        let state = SymbolicState::empty();
        let expr = Expr::Float(1.0);
        let result = eval_symbolic(&expr, &state);
        assert!(matches!(result, SymbolicValue::Unknown));
    }

    #[kani::proof]
    fn verify_eval_symbolic_literal_string_is_unknown() {
        let state = SymbolicState::empty();
        let expr = Expr::Quoted("x".into());
        let result = eval_symbolic(&expr, &state);
        assert!(matches!(result, SymbolicValue::Unknown));
    }
}
