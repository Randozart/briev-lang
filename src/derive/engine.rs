// ── Phase C — Full Enumerative Synthesis Engine ─────────────────────────
// 2026-07-12: Phase 6.1 — Original depth-bounded enumerative search.
// 2026-07-28: Phase C — Rewritten: type-aware generation, interpreter-backed
// evaluation, Occam cost model with constant burden, cost-pruned search.
// Flat code: each function max 2 levels of nesting.

use crate::ast::{BinaryOpKind, DerivationExample, Expr, Pattern, Type, UnaryOpKind};
use crate::derive::SynthesizeError;
use crate::interpreter::values_within_tolerance;
use crate::interpreter::{Atom, Value};

// ── C.2 — Occam Cost Model ────────────────────────────────────────────

/// 2026-07-28: Phase C.2 — Cost model for enumerative synthesis.
/// Occam's razor: simpler programs (lower cost) are preferred.
/// Constants beyond trivial values incur a per-bit burden.
#[derive(Debug, Clone)]
pub struct CostModel {
    pub constant: u64,
    pub variable: u64,
    pub unary_op: u64,
    pub binary_op: u64,
    pub branch: u64,
    pub constant_burden: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        CostModel {
            constant: 1,
            variable: 1,
            unary_op: 2,
            binary_op: 3,
            branch: 5,
            constant_burden: 0.1,
        }
    }
}

impl CostModel {
    #[allow(dead_code)]
    pub fn cost_of_constant(&self, val: &Value) -> u64 {
        let bits = match val {
            Value::Atom(Atom::Int(n)) => {
                n.checked_abs().map(|a| 64 - a.leading_zeros()).unwrap_or(0) as u64
            }
            Value::Atom(Atom::Float(_)) => 64,
            Value::Bits(bytes) if bytes.len() <= 1 => 1,
            _ => 8,
        };
        self.constant + (bits as f64 * self.constant_burden).ceil() as u64
    }

    pub fn cost_of_expr(&self, expr: &Expr) -> u64 {
        match expr {
            Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) => self.constant,
            Expr::Identifier(_) => self.variable,
            Expr::UnaryOp(_, inner) => self.unary_op + self.cost_of_expr(inner),
            Expr::BinaryOp(op, lhs, rhs) => {
                let base = self.binary_op + self.cost_of_expr(lhs) + self.cost_of_expr(rhs);
                // 2026-07-28: Equality penalty — Eq/Neq add +2 cost to discourage
                // table-lookup ite chains (x0 == 5, x0 == -3) in favor of general
                // formulas using comparison operators (<, >, <=, >=).
                if matches!(op, BinaryOpKind::Eq | BinaryOpKind::Neq) {
                    base + 2
                } else {
                    base
                }
            }
            Expr::If(cond, then_, else_) => {
                let else_cost = else_.as_ref().map(|e| self.cost_of_expr(e)).unwrap_or(0);
                self.branch + self.cost_of_expr(cond) + self.cost_of_expr(then_) + else_cost
            }
            // 2026-07-28: Phase 5 — Compound type expression costs
            Expr::Call(_, args, _) => 3 + args.iter().map(|a| self.cost_of_expr(a)).sum::<u64>(),
            Expr::Field(inner, _) => 2 + self.cost_of_expr(inner),
            Expr::Match(scrut, arms) => {
                5 + self.cost_of_expr(scrut)
                    + arms.len() as u64
                    + arms.iter().map(|a| self.cost_of_expr(&a.body)).sum::<u64>()
            }
            _ => 10,
        }
    }
}

// ── C.1 — Synthesis Evaluation Context ─────────────────────────────────

/// 2026-07-28: Phase C.1 — Lightweight evaluation context for synthesized
/// expressions during enumerative search. Maps variable names to Values.
#[derive(Debug, Clone)]
pub struct SynthesisEvalContext {
    pub bindings: std::collections::HashMap<String, Value>,
}

impl SynthesisEvalContext {
    pub fn new() -> Self {
        SynthesisEvalContext {
            bindings: std::collections::HashMap::new(),
        }
    }

    pub fn bind(&mut self, name: &str, val: Value) {
        self.bindings.insert(name.to_string(), val);
    }
}

/// 2026-07-28: Phase C.1 — Error type for synthesis expression evaluation.
#[derive(Debug, Clone)]
pub enum SynthesisEvalError {
    UndefinedVariable(String),
    DivisionByZero,
    TypeMismatch(String),
}

/// Evaluate a synthesized expression against concrete input values.
/// 2026-07-28: Phase C.1 — replaces the old matches_pattern stub.
pub fn evaluate_synthesized(
    expr: &Expr,
    ctx: &mut SynthesisEvalContext,
) -> Result<Value, SynthesisEvalError> {
    match expr {
        Expr::Decimal(n) => Ok(Value::Atom(Atom::Int(*n))),
        Expr::Float(f) => Ok(Value::Atom(Atom::Float(*f))),
        Expr::Bool(b) => Ok(Value::Bits(vec![if *b { 1u8 } else { 0u8 }])),
        Expr::Identifier(name) => ctx
            .bindings
            .get(name)
            .cloned()
            .ok_or(SynthesisEvalError::UndefinedVariable(name.clone())),
        Expr::UnaryOp(kind, inner) => {
            let val = evaluate_synthesized(inner, ctx)?;
            eval_unary(kind, val)
        }
        Expr::BinaryOp(kind, lhs, rhs) => {
            let l = evaluate_synthesized(lhs, ctx)?;
            let r = evaluate_synthesized(rhs, ctx)?;
            eval_binary(kind, l, r)
        }
        Expr::If(cond, then_, else_) => {
            let c = evaluate_synthesized(cond, ctx)?;
            let cbool = match c {
                Value::Bits(ref bytes) => bytes.iter().any(|b| *b != 0),
                Value::Atom(Atom::Int(n)) => n != 0,
                Value::Atom(Atom::Float(f)) => f != 0.0,
                _ => {
                    return Err(SynthesisEvalError::TypeMismatch(
                        "non-boolean condition".into(),
                    ));
                }
            };
            if cbool {
                evaluate_synthesized(then_, ctx)
            } else if let Some(e) = else_ {
                evaluate_synthesized(e, ctx)
            } else {
                Ok(Value::Bits(vec![0]))
            }
        }
        Expr::Call(name, args, _) => {
            // Treat calls as sums for compound types.
            // A constructor call like Add(Const(5), Const(3)) is represented
            // as Call("Add", [Call("Const", [Decimal(5)]), ...], None) — the
            // applied name is the sum's variant tag.
            let mut values = Vec::new();
            for arg in args {
                values.push(evaluate_synthesized(arg, ctx)?);
            }
            Ok(Value::Sum {
                name: name.clone(),
                payload: values,
            })
        }
        Expr::Field(inner, field_name) => {
            let val = evaluate_synthesized(inner, ctx)?;
            match val {
                Value::Sum {
                    payload: fields, ..
                } => {
                    if let Ok(idx) = field_name.parse::<usize>() {
                        fields
                            .get(idx)
                            .cloned()
                            .ok_or(SynthesisEvalError::TypeMismatch(format!(
                                "field '{}' not found (index {})",
                                field_name, idx
                            )))
                    } else {
                        Err(SynthesisEvalError::TypeMismatch(format!(
                            "named fields not yet supported in synthesis: '{}'",
                            field_name
                        )))
                    }
                }
                _ => Err(SynthesisEvalError::TypeMismatch(format!(
                    "field access on non-sum: {:?}",
                    val
                ))),
            }
        }
        Expr::Match(expr, arms) => {
            let val = evaluate_synthesized(expr, ctx)?;
            match val {
                Value::Sum {
                    name,
                    payload: fields,
                } => {
                    for arm in arms {
                        if let Pattern::EnumVariant(ref arm_name, ref pat_fields) = arm.pattern {
                            if *arm_name == name {
                                for (i, pat) in pat_fields.iter().enumerate() {
                                    match pat {
                                        Pattern::Binding(binding_name) => {
                                            if let Some(f) = fields.get(i) {
                                                let val: crate::interpreter::Value = f.clone();
                                                ctx.bind(binding_name, val);
                                            }
                                        }
                                        Pattern::Wildcard => {}
                                        _ => {}
                                    }
                                }
                                return evaluate_synthesized(&arm.body, ctx);
                            }
                        }
                    }
                    Err(SynthesisEvalError::TypeMismatch(format!(
                        "no matching arm for sum variant '{}'",
                        name
                    )))
                }
                _ => Err(SynthesisEvalError::TypeMismatch(format!(
                    "match on non-sum: {:?}",
                    val
                ))),
            }
        }
        _ => Err(SynthesisEvalError::TypeMismatch(format!(
            "unsupported expression in synthesis: {:?}",
            expr
        ))),
    }
}

fn eval_unary(kind: &UnaryOpKind, val: Value) -> Result<Value, SynthesisEvalError> {
    match kind {
        UnaryOpKind::Neg => match val {
            Value::Atom(Atom::Int(n)) => Ok(Value::Atom(Atom::Int(n.wrapping_neg()))),
            Value::Atom(Atom::Float(f)) => Ok(Value::Atom(Atom::Float(-f))),
            _ => Err(SynthesisEvalError::TypeMismatch(
                "negation requires numeric type".into(),
            )),
        },
        UnaryOpKind::Not => match val {
            Value::Bits(ref bytes) => Ok(Value::Bits(vec![if bytes.iter().all(|b| *b == 0) {
                1
            } else {
                0
            }])),
            Value::Atom(Atom::Int(n)) => Ok(Value::Atom(Atom::Int(if n == 0 { 1 } else { 0 }))),
            _ => Err(SynthesisEvalError::TypeMismatch(
                "not requires boolean type".into(),
            )),
        },
        UnaryOpKind::BitNot => Err(SynthesisEvalError::TypeMismatch(
            "bitwise not not supported in synthesis".into(),
        )),
    }
}

fn eval_binary(kind: &BinaryOpKind, l: Value, r: Value) -> Result<Value, SynthesisEvalError> {
    match (l, r) {
        (Value::Atom(Atom::Int(a)), Value::Atom(Atom::Int(b))) => eval_binary_int(kind, a, b),
        (Value::Atom(Atom::Float(a)), Value::Atom(Atom::Float(b))) => eval_binary_float(kind, a, b),
        (Value::Atom(Atom::Int(a)), Value::Atom(Atom::Float(b))) => {
            eval_binary_float(kind, a as f64, b)
        }
        (Value::Atom(Atom::Float(a)), Value::Atom(Atom::Int(b))) => {
            eval_binary_float(kind, a, b as f64)
        }
        _ => match kind {
            BinaryOpKind::Eq => Ok(Value::Bits(vec![0])),
            BinaryOpKind::Neq => Ok(Value::Bits(vec![1])),
            _ => Err(SynthesisEvalError::TypeMismatch(
                "incompatible types for binary op".into(),
            )),
        },
    }
}

fn eval_binary_int(kind: &BinaryOpKind, a: i64, b: i64) -> Result<Value, SynthesisEvalError> {
    match kind {
        BinaryOpKind::Add => Ok(Value::Atom(Atom::Int(a.wrapping_add(b)))),
        BinaryOpKind::Sub => Ok(Value::Atom(Atom::Int(a.wrapping_sub(b)))),
        BinaryOpKind::Mul => Ok(Value::Atom(Atom::Int(a.wrapping_mul(b)))),
        BinaryOpKind::Div => {
            if b == 0 {
                return Err(SynthesisEvalError::DivisionByZero);
            }
            Ok(Value::Atom(Atom::Int(a.wrapping_div(b))))
        }
        BinaryOpKind::Mod => {
            if b == 0 {
                return Err(SynthesisEvalError::DivisionByZero);
            }
            Ok(Value::Atom(Atom::Int(a.wrapping_rem(b))))
        }
        BinaryOpKind::Eq => Ok(Value::Bits(vec![if a == b { 1 } else { 0 }])),
        BinaryOpKind::Neq => Ok(Value::Bits(vec![if a != b { 1 } else { 0 }])),
        BinaryOpKind::Lt => Ok(Value::Bits(vec![if a < b { 1 } else { 0 }])),
        BinaryOpKind::Gt => Ok(Value::Bits(vec![if a > b { 1 } else { 0 }])),
        BinaryOpKind::Le => Ok(Value::Bits(vec![if a <= b { 1 } else { 0 }])),
        BinaryOpKind::Ge => Ok(Value::Bits(vec![if a >= b { 1 } else { 0 }])),
        BinaryOpKind::And => Ok(Value::Atom(Atom::Int(if a != 0 && b != 0 { 1 } else { 0 }))),
        BinaryOpKind::Or => Ok(Value::Atom(Atom::Int(if a != 0 || b != 0 { 1 } else { 0 }))),
        BinaryOpKind::BitXor => Ok(Value::Atom(Atom::Int(a ^ b))),
        BinaryOpKind::Shl => Ok(Value::Atom(Atom::Int(a.wrapping_shl(b as u32)))),
        BinaryOpKind::Shr => Ok(Value::Atom(Atom::Int(a.wrapping_shr(b as u32)))),
        BinaryOpKind::BitAnd => Ok(Value::Atom(Atom::Int(a & b))),
        BinaryOpKind::BitOr => Ok(Value::Atom(Atom::Int(a | b))),
        BinaryOpKind::Concat => Err(SynthesisEvalError::TypeMismatch(
            "concat not supported on int".into(),
        )),
    }
}

fn eval_binary_float(kind: &BinaryOpKind, a: f64, b: f64) -> Result<Value, SynthesisEvalError> {
    match kind {
        BinaryOpKind::Add => Ok(Value::Atom(Atom::Float(a + b))),
        BinaryOpKind::Sub => Ok(Value::Atom(Atom::Float(a - b))),
        BinaryOpKind::Mul => Ok(Value::Atom(Atom::Float(a * b))),
        BinaryOpKind::Div => Ok(Value::Atom(Atom::Float(a / b))),
        BinaryOpKind::Mod => Ok(Value::Atom(Atom::Float(a % b))),
        BinaryOpKind::Eq => Ok(Value::Bits(vec![if a == b { 1 } else { 0 }])),
        BinaryOpKind::Neq => Ok(Value::Bits(vec![if a != b { 1 } else { 0 }])),
        BinaryOpKind::Lt => Ok(Value::Bits(vec![if a < b { 1 } else { 0 }])),
        BinaryOpKind::Gt => Ok(Value::Bits(vec![if a > b { 1 } else { 0 }])),
        BinaryOpKind::Le => Ok(Value::Bits(vec![if a <= b { 1 } else { 0 }])),
        BinaryOpKind::Ge => Ok(Value::Bits(vec![if a >= b { 1 } else { 0 }])),
        BinaryOpKind::And
        | BinaryOpKind::Or
        | BinaryOpKind::BitAnd
        | BinaryOpKind::BitOr
        | BinaryOpKind::BitXor
        | BinaryOpKind::Shl
        | BinaryOpKind::Shr
        | BinaryOpKind::Concat => Err(SynthesisEvalError::TypeMismatch(
            "integer-only binary op on float".into(),
        )),
    }
}

// ── C.0 — Type-Aware Expression Generation ────────────────────────────

/// 2026-07-28: Phase C.0 — Operator type compatibility for synthesis.
/// Returns the result type if the operator is valid for the given operand types.
fn op_result_type(op: &BinaryOpKind, lhs_ty: &str, rhs_ty: &str) -> Option<&'static str> {
    if lhs_ty != rhs_ty {
        return None;
    }
    match op {
        BinaryOpKind::Add
        | BinaryOpKind::Sub
        | BinaryOpKind::Mul
        | BinaryOpKind::Div
        | BinaryOpKind::Mod => {
            if lhs_ty == "Int" || lhs_ty == "Float" {
                if lhs_ty == "Int" {
                    Some("Int")
                } else {
                    Some("Float")
                }
            } else {
                None
            }
        }
        BinaryOpKind::Eq
        | BinaryOpKind::Neq
        | BinaryOpKind::Lt
        | BinaryOpKind::Gt
        | BinaryOpKind::Le
        | BinaryOpKind::Ge => Some("Bool"),
        BinaryOpKind::And | BinaryOpKind::Or => {
            if lhs_ty == "Bool" {
                Some("Bool")
            } else {
                None
            }
        }
        BinaryOpKind::BitXor => {
            if lhs_ty == "Int" {
                Some("Int")
            } else if lhs_ty == "Bool" {
                Some("Bool")
            } else {
                None
            }
        }
        BinaryOpKind::Shl | BinaryOpKind::Shr => {
            if lhs_ty == "Int" {
                Some("Int")
            } else {
                None
            }
        }
        BinaryOpKind::BitAnd | BinaryOpKind::BitOr => {
            if lhs_ty == "Int" {
                Some("Int")
            } else if lhs_ty == "Bool" {
                Some("Bool")
            } else {
                None
            }
        }
        BinaryOpKind::Concat => None,
        _ => None,
    }
}

/// 2026-07-28: Phase C.0 — Type-aware expression generator.
/// Generates all well-typed expressions up to a given depth.
fn generate_typed_expressions(
    param_names: &[String],
    param_types: &[String],
    ret_type: &str,
    depth: u8,
) -> Vec<Expr> {
    if depth == 0 || param_names.len() != param_types.len() {
        return Vec::new();
    }

    let mut result = Vec::new();

    // Constants at any depth (but prefer shallow)
    push_typed_constants(ret_type, &mut result);

    // Variables that match the return type
    for (name, ty) in param_names.iter().zip(param_types.iter()) {
        if ty == ret_type {
            result.push(Expr::Identifier(name.clone()));
        }
    }

    if depth <= 1 {
        return result;
    }

    let sub = generate_typed_expressions(param_names, param_types, ret_type, depth - 1);

    // Unary ops
    if ret_type == "Int" || ret_type == "Float" {
        for e in &sub {
            if let Some(neg_ty) = unary_result_type(&UnaryOpKind::Neg, ret_type) {
                if neg_ty == ret_type {
                    result.push(Expr::UnaryOp(UnaryOpKind::Neg, Box::new(e.clone())));
                }
            }
        }
    }
    if ret_type == "Bool" {
        for e in &sub {
            result.push(Expr::UnaryOp(UnaryOpKind::Not, Box::new(e.clone())));
        }
    }

    // Binary ops
    for lhs_ty_str in &["Int", "Float", "Bool"] {
        let lhs_candidates =
            generate_typed_expressions(param_names, param_types, lhs_ty_str, depth - 1);
        if lhs_candidates.is_empty() {
            continue;
        }
        let rhs_candidates =
            generate_typed_expressions(param_names, param_types, lhs_ty_str, depth - 1);
        if rhs_candidates.is_empty() {
            continue;
        }
        for op in &[
            BinaryOpKind::Add,
            BinaryOpKind::Sub,
            BinaryOpKind::Mul,
            BinaryOpKind::Div,
            BinaryOpKind::Mod,
            BinaryOpKind::Eq,
            BinaryOpKind::Neq,
            BinaryOpKind::Lt,
            BinaryOpKind::Gt,
            BinaryOpKind::Le,
            BinaryOpKind::Ge,
            BinaryOpKind::BitAnd,
            BinaryOpKind::BitOr,
            BinaryOpKind::BitXor,
            BinaryOpKind::Shl,
            BinaryOpKind::Shr,
        ] {
            if let Some(ty) = op_result_type(op, lhs_ty_str, lhs_ty_str) {
                if ty != ret_type {
                    continue;
                }
                for lhs_expr in &lhs_candidates {
                    for rhs_expr in &rhs_candidates {
                        result.push(Expr::BinaryOp(
                            *op,
                            Box::new(lhs_expr.clone()),
                            Box::new(rhs_expr.clone()),
                        ));
                    }
                }
            }
        }
    }

    // Bool-specific: logical ops only when ret_type == Bool
    if ret_type == "Bool" {
        let bool_candidates =
            generate_typed_expressions(param_names, param_types, "Bool", depth - 1);
        if !bool_candidates.is_empty() {
            for op in &[BinaryOpKind::And, BinaryOpKind::Or, BinaryOpKind::BitXor] {
                if op_result_type(op, "Bool", "Bool") == Some("Bool") {
                    for lhs_expr in &bool_candidates {
                        for rhs_expr in &bool_candidates {
                            result.push(Expr::BinaryOp(
                                *op,
                                Box::new(lhs_expr.clone()),
                                Box::new(rhs_expr.clone()),
                            ));
                        }
                    }
                }
            }
        }
    }

    result
}

/// 2026-07-28: Lazy version — yields each candidate to `callback` as generated,
/// stops early if callback returns true. Reduces memory from O(candidates) to O(depth).
fn generate_typed_expressions_lazy(
    param_names: &[String],
    param_types: &[String],
    ret_type: &str,
    depth: u8,
    callback: &mut dyn FnMut(&Expr) -> bool,
) {
    if depth == 0 || param_names.len() != param_types.len() {
        return;
    }

    // Constants at any depth (but prefer shallow)
    let mut temp = Vec::new();
    push_typed_constants(ret_type, &mut temp);
    for c in &temp {
        if callback(c) {
            return;
        }
    }

    // Variables that match the return type
    for (name, ty) in param_names.iter().zip(param_types.iter()) {
        if ty == ret_type {
            if callback(&Expr::Identifier(name.clone())) {
                return;
            }
        }
    }

    if depth <= 1 {
        return;
    }

    // Unary ops
    if ret_type == "Int" || ret_type == "Float" {
        generate_typed_expressions_lazy(param_names, param_types, ret_type, depth - 1, &mut |e| {
            if let Some(neg_ty) = unary_result_type(&UnaryOpKind::Neg, ret_type) {
                if neg_ty == ret_type {
                    if callback(&Expr::UnaryOp(UnaryOpKind::Neg, Box::new(e.clone()))) {
                        return true;
                    }
                }
            }
            false
        });
    }
    if ret_type == "Bool" {
        generate_typed_expressions_lazy(param_names, param_types, ret_type, depth - 1, &mut |e| {
            if callback(&Expr::UnaryOp(UnaryOpKind::Not, Box::new(e.clone()))) {
                return true;
            }
            false
        });
    }

    // Binary ops
    for lhs_ty_str in &["Int", "Float", "Bool"] {
        let mut lhs_collected = Vec::new();
        generate_typed_expressions_lazy(
            param_names,
            param_types,
            lhs_ty_str,
            depth - 1,
            &mut |e| {
                lhs_collected.push(e.clone());
                false
            },
        );
        if lhs_collected.is_empty() {
            continue;
        }
        let mut rhs_collected = Vec::new();
        generate_typed_expressions_lazy(
            param_names,
            param_types,
            lhs_ty_str,
            depth - 1,
            &mut |e| {
                rhs_collected.push(e.clone());
                false
            },
        );
        if rhs_collected.is_empty() {
            continue;
        }
        // 2026-07-28: Collect candidates with div-by-zero pruning.
        // Skip Div/Mod/Shl where RHS is constant zero (always fails evaluation).
        for op in &[
            BinaryOpKind::Add,
            BinaryOpKind::Sub,
            BinaryOpKind::Mul,
            BinaryOpKind::Div,
            BinaryOpKind::Mod,
            BinaryOpKind::Eq,
            BinaryOpKind::Neq,
            BinaryOpKind::Lt,
            BinaryOpKind::Gt,
            BinaryOpKind::Le,
            BinaryOpKind::Ge,
            BinaryOpKind::BitAnd,
            BinaryOpKind::BitOr,
            BinaryOpKind::BitXor,
            BinaryOpKind::Shl,
            BinaryOpKind::Shr,
        ] {
            if let Some(ty) = op_result_type(op, lhs_ty_str, lhs_ty_str) {
                if ty != ret_type {
                    continue;
                }
                for lhs_expr in &lhs_collected {
                    for rhs_expr in &rhs_collected {
                        // Div/Mod/Shl with constant-zero RHS always fail evaluation
                        if matches!(
                            op,
                            BinaryOpKind::Div | BinaryOpKind::Mod | BinaryOpKind::Shl
                        ) {
                            if is_constant_zero(rhs_expr) {
                                continue;
                            }
                        }
                        if callback(&Expr::BinaryOp(
                            *op,
                            Box::new(lhs_expr.clone()),
                            Box::new(rhs_expr.clone()),
                        )) {
                            return;
                        }
                    }
                }
            }
        }
    }

    // Bool-specific: logical ops only when ret_type == Bool
    if ret_type == "Bool" {
        let mut bool_candidates = Vec::new();
        generate_typed_expressions_lazy(param_names, param_types, "Bool", depth - 1, &mut |e| {
            bool_candidates.push(e.clone());
            false
        });
        if !bool_candidates.is_empty() {
            for op in &[BinaryOpKind::And, BinaryOpKind::Or, BinaryOpKind::BitXor] {
                if op_result_type(op, "Bool", "Bool") == Some("Bool") {
                    for lhs_expr in &bool_candidates {
                        for rhs_expr in &bool_candidates {
                            if callback(&Expr::BinaryOp(
                                *op,
                                Box::new(lhs_expr.clone()),
                                Box::new(rhs_expr.clone()),
                            )) {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 2026-07-28: Per-type pruned expression cache for checkpointed depth search.
/// After each depth, expressions are evaluated against all examples and pruned
/// to keep only unique, non-constant output vectors. The next depth generates
/// candidates only from the pruned set, dramatically reducing the cross product.
/// 2026-07-29: Made pub(crate) for sibling module library.rs (abstraction discovery).
/// Added helper_names for tracked helper functions. See abstraction-discovery plan.
pub(crate) struct LevelCache {
    pub(crate) int_exprs: Vec<Expr>,
    pub(crate) float_exprs: Vec<Expr>,
    pub(crate) bool_exprs: Vec<Expr>,
    /// 2026-07-28: Phase 5 — Compound type expressions, keyed by type name (e.g., "Expr").
    pub(crate) compound_exprs: std::collections::HashMap<String, Vec<Expr>>,
    /// 2026-07-29: Names of registered helper functions (abstraction discovery).
    /// Stored separately from per-type exprs because helper calls are generated
    /// from name-to-type mapping, not from raw exprs. Iterated in
    /// generate_next_level() to find matching-return-type helpers.
    pub(crate) helper_names: Vec<String>,
    /// 2026-07-29: Helper metadata mapping name → (param_names, ret_type).
    /// Used by generate_next_level() to emit standalone helper calls with
    /// the correct arity and return type. Populated by register_helpers().
    pub(crate) helper_info: std::collections::HashMap<String, (Vec<String>, String)>,
}

impl LevelCache {
    fn empty() -> Self {
        LevelCache {
            int_exprs: vec![],
            float_exprs: vec![],
            bool_exprs: vec![],
            compound_exprs: std::collections::HashMap::new(),
            helper_names: vec![],
            helper_info: std::collections::HashMap::new(),
        }
    }

    /// Push an expression with its inferred type into the appropriate bucket.
    fn push(&mut self, expr: Expr, ty: &str) {
        match ty {
            "Int" => self.int_exprs.push(expr),
            "Float" => self.float_exprs.push(expr),
            "Bool" => self.bool_exprs.push(expr),
            // 2026-07-28: Phase 5 — Compound types stored in HashMap
            ty if is_compound_type(ty) => {
                self.compound_exprs
                    .entry(ty.to_string())
                    .or_default()
                    .push(expr);
            }
            _ => {}
        }
    }
}

/// 2026-07-28: Incremental evaluation cache — stores evaluation results for each
/// expression from the pruning step, avoiding recursive re-evaluation at the next
/// depth. The LevelCache holds the pruned expression sets; the results HashMap
/// provides O(1) lookup for candidate_matches_all_examples sub-expression evaluation.
struct EvalCache {
    level: LevelCache,
    /// 2026-07-28: Evaluation results stored as (expr, results_for_each_example) pairs.
    /// Linear lookup is fine: n ≤ 500 at any depth.
    results: Vec<(Expr, Vec<crate::interpreter::Value>)>,
}

impl EvalCache {
    fn empty() -> Self {
        EvalCache {
            level: LevelCache::empty(),
            results: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.results.len()
    }

    /// Look up cached evaluation results for an expression.
    fn get(&self, expr: &Expr) -> Option<&Vec<crate::interpreter::Value>> {
        self.results
            .iter()
            .find(|(e, _)| std::ptr::eq(e, expr))
            .map(|(_, v)| v)
    }
}

/// 2026-07-28: Check if an expression is constant zero (for div-by-zero pruning).
fn is_constant_zero(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(n) => *n == 0,
        Expr::Float(f) => *f == 0.0,
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => is_constant_zero(inner),
        _ => false,
    }
}

/// 2026-07-28: Check if an expression is constant one (for identity pruning).
fn is_constant_one(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(n) => *n == 1,
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => match inner.as_ref() {
            Expr::Decimal(n) => *n == -1,
            _ => false,
        },
        _ => false,
    }
}

/// 2026-07-28: Check if an expression is all-ones (0xFFFF...FFFF = -1 for i64).
fn is_all_ones(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(n) => *n == -1,
        _ => false,
    }
}

/// 2026-07-28: Phase 5 — Check if a type name is compound (not a primitive).
pub fn is_compound_type(name: &str) -> bool {
    matches!(name, "Expr")
}

/// 2026-07-28: Symmetry breaking — check if an operator is commutative.
/// For commutative ops, we only generate lhs ≤ rhs to avoid duplicates.
/// 2026-07-29: Made pub(crate) for sibling module library.rs (abstraction discovery).
pub(crate) fn is_commutative_op(op: BinaryOpKind) -> bool {
    matches!(
        op,
        BinaryOpKind::Add
            | BinaryOpKind::Mul
            | BinaryOpKind::BitAnd
            | BinaryOpKind::BitOr
            | BinaryOpKind::BitXor
            | BinaryOpKind::Eq
    )
}

/// 2026-07-28: Symmetry breaking — check if lhs ≤ rhs in canonical ordering.
/// Uses expression index in the Vec as position indicator.
fn expr_pair_ordered(lhs: &Expr, rhs: &Expr, exprs: &[Expr]) -> bool {
    let lhs_pos = exprs.iter().position(|e| std::ptr::eq(e, lhs));
    let rhs_pos = exprs.iter().position(|e| std::ptr::eq(e, rhs));
    match (lhs_pos, rhs_pos) {
        (Some(l), Some(r)) => l <= r,
        _ => true, // fallback: generate both
    }
}

/// 2026-07-28: Tier 1 — Check if a binary operation is semantically redundant.
/// Identity operations like 0+X, X*1, X>>0 produce the same result as the
/// non-identity operand. Pruning them reduces search space and overfitting.
/// 2026-07-29: Made pub(crate) for sibling module library.rs (abstraction discovery).
pub(crate) fn is_identity_op(op: BinaryOpKind, lhs: &Expr, rhs: &Expr) -> bool {
    match op {
        BinaryOpKind::Add => is_constant_zero(lhs) || is_constant_zero(rhs),
        BinaryOpKind::Sub => is_constant_zero(rhs),
        BinaryOpKind::Mul => is_constant_one(lhs) || is_constant_one(rhs),
        BinaryOpKind::Div => is_constant_one(rhs),
        BinaryOpKind::Shl | BinaryOpKind::Shr => is_constant_zero(rhs),
        BinaryOpKind::BitAnd => is_all_ones(rhs),
        BinaryOpKind::BitOr | BinaryOpKind::BitXor => is_constant_zero(rhs),
        _ => false,
    }
}

/// 2026-07-28: Generate the next depth's candidates from the pruned previous level.
/// Uses LevelCache (per-type pruned sets from the previous depth) instead of
/// recursively generating all sub-expressions. This enables checkpoint pruning.
fn generate_next_level(
    param_names: &[String],
    param_types: &[String],
    ret_type: &str,
    prev: &EvalCache,
) -> Vec<Expr> {
    let mut result = Vec::new();

    // Constants at any depth
    push_typed_constants(ret_type, &mut result);

    // 2026-07-28: Phase 5 — Add ALL parameter names, regardless of type.
    // Compound-type params (e.g., Expr) are needed for cross-type extraction
    // via Match expressions (Expr → Int).
    for (name, _ty) in param_names.iter().zip(param_types.iter()) {
        result.push(Expr::Identifier(name.clone()));
    }

    // 2026-07-29: Helper call generation — emit Expr::Call for each
    // registered helper whose return type matches the current target type.
    // This is the key scaling improvement: instead of regenerating the
    // helper's entire subtree at each use, the search references it by name.
    // Helper calls are emitted as standalone candidates (like variables) and
    // also participate in binary ops via their presence in the per-type buckets.
    // Adapted from Koza's ADFs [GP'92] §6.3.
    for helper_name in &prev.level.helper_names {
        if let Some((params, h_ret_type)) = prev.level.helper_info.get(helper_name) {
            if *h_ret_type == ret_type && !params.is_empty() {
                let args: Vec<Expr> = params.iter().map(|p| Expr::Identifier(p.clone())).collect();
                result.push(Expr::Call(helper_name.clone(), args, None));
            }
        }
    }

    // Unary ops on the previous level's expressions
    // Merge int_exprs and float_exprs for arithmetic unary (Neg works on both)
    let mut unary_sources: Vec<Expr> = Vec::new();
    if ret_type == "Int" || ret_type == "Float" {
        unary_sources.extend(prev.level.int_exprs.clone());
        unary_sources.extend(prev.level.float_exprs.clone());
    } else if ret_type == "Bool" {
        unary_sources.extend(prev.level.bool_exprs.clone());
    }
    for e in &unary_sources {
        if ret_type == "Int" || ret_type == "Float" {
            result.push(Expr::UnaryOp(UnaryOpKind::Neg, Box::new(e.clone())));
        } else {
            result.push(Expr::UnaryOp(UnaryOpKind::Not, Box::new(e.clone())));
        }
    }

    // Binary ops: cross product of prev per type
    let cache_types: [(&str, &Vec<Expr>); 3] = [
        ("Int", &prev.level.int_exprs),
        ("Float", &prev.level.float_exprs),
        ("Bool", &prev.level.bool_exprs),
    ];
    for (ty_str, exprs) in &cache_types {
        if exprs.is_empty() {
            continue;
        }
        for op in &[
            BinaryOpKind::Add,
            BinaryOpKind::Sub,
            BinaryOpKind::Mul,
            BinaryOpKind::Div,
            BinaryOpKind::Mod,
            BinaryOpKind::Lt,
            BinaryOpKind::Gt,
            BinaryOpKind::Le,
            BinaryOpKind::Ge,
            BinaryOpKind::Eq,
            BinaryOpKind::Neq,
            BinaryOpKind::BitAnd,
            BinaryOpKind::BitOr,
            BinaryOpKind::BitXor,
            BinaryOpKind::Shl,
            BinaryOpKind::Shr,
        ] {
            if let Some(ty) = op_result_type(op, ty_str, ty_str) {
                // 2026-07-28: Keep Bool-returning operators even when ret_type
                // is Int. Comparison ops (Lt, Gt, Le, Ge, Eq, Neq) return Bool;
                // these are needed as IF conditions at the next depth.
                if ty != ret_type && ty != "Bool" {
                    continue;
                }
                for lhs in *exprs {
                    for rhs in *exprs {
                        if matches!(
                            op,
                            BinaryOpKind::Div | BinaryOpKind::Mod | BinaryOpKind::Shl
                        ) {
                            if is_constant_zero(rhs) {
                                continue;
                            }
                        }
                        // 2026-07-28: Tier 1 — Skip identity operations that add
                        // no semantic value (0+X, X*1, X>>0, etc.). These inflate
                        // the search space and produce overfitted formulas.
                        if is_identity_op(*op, lhs, rhs) {
                            continue;
                        }
                        // 2026-07-28: Symmetry breaking — for commutative ops, only
                        // generate when lhs <= rhs (by expression position) to avoid
                        // generating both Add(A,B) and Add(B,A).
                        if is_commutative_op(*op) && !expr_pair_ordered(lhs, rhs, exprs) {
                            continue;
                        }
                        result.push(Expr::BinaryOp(
                            *op,
                            Box::new(lhs.clone()),
                            Box::new(rhs.clone()),
                        ));
                    }
                }
            }
        }
    }

    // Bool-specific logical ops
    if ret_type == "Bool" && !prev.level.bool_exprs.is_empty() {
        for op in &[BinaryOpKind::And, BinaryOpKind::Or, BinaryOpKind::BitXor] {
            if op_result_type(op, "Bool", "Bool") == Some("Bool") {
                for lhs in &prev.level.bool_exprs {
                    for rhs in &prev.level.bool_exprs {
                        result.push(Expr::BinaryOp(
                            *op,
                            Box::new(lhs.clone()),
                            Box::new(rhs.clone()),
                        ));
                    }
                }
            }
        }
    }

    // 2026-07-28: If generation — conditional expressions (if cond then then else else)
    // Generate If(Bool, T, T) → T where T = ret_type. For Int return type, this enables
    // min = if x0 < x1 then x0 else x1 and abs = if x0 < 0 then -x0 else x0.
    let bool_exprs_exist = !prev.level.bool_exprs.is_empty();
    let int_exprs_exist = !prev.level.int_exprs.is_empty() || !prev.level.float_exprs.is_empty();
    if bool_exprs_exist && (ret_type == "Int" || ret_type == "Float" || ret_type == "Bool") {
        let then_else_sources: Vec<Expr> = {
            let mut s = Vec::new();
            s.extend(prev.level.int_exprs.iter().cloned());
            s.extend(prev.level.float_exprs.iter().cloned());
            if ret_type == "Bool" {
                s.extend(prev.level.bool_exprs.iter().cloned());
            }
            s
        };
        if !then_else_sources.is_empty() {
            for cond in &prev.level.bool_exprs {
                for then_expr in &then_else_sources {
                    for else_expr in &then_else_sources {
                        result.push(Expr::If(
                            Box::new(cond.clone()),
                            Box::new(then_expr.clone()),
                            Some(Box::new(else_expr.clone())),
                        ));
                    }
                }
            }
        }
    }

    // 2026-07-28: Phase 5 — Call and Match generation for compound types
    if is_compound_type(ret_type) {
        // Call generation: Const(Int), Add(Expr, Expr), Sub(Expr, Expr), Mul(Expr, Expr)
        // Const takes Int arg
        for int_expr in &prev.level.int_exprs {
            result.push(Expr::Call("Const".into(), vec![int_expr.clone()], None));
        }
        // Add/Sub/Mul take two Expr args — use prev level's compound expressions
        let compound_exprs = ret_type.to_string();
        if let Some(exprs) = prev.level.compound_exprs.get(&compound_exprs) {
            for lhs in exprs {
                for rhs in exprs {
                    result.push(Expr::Call(
                        "Add".into(),
                        vec![lhs.clone(), rhs.clone()],
                        None,
                    ));
                    result.push(Expr::Call(
                        "Sub".into(),
                        vec![lhs.clone(), rhs.clone()],
                        None,
                    ));
                    result.push(Expr::Call(
                        "Mul".into(),
                        vec![lhs.clone(), rhs.clone()],
                        None,
                    ));
                }
            }
        }
    }

    // Match generation: produce Match expressions over compound type expressions.
    // Works for both same-type (Expr → Expr) and cross-type (Expr → Int) extraction.
    // For each compound type expression in prev level, generate a Match with
    // EnumVariant arms. Arm bodies use the bound field variables — for cross-type
    // extraction, the first field is assumed to be the value (correct for Const).
    for (type_name, exprs) in &prev.level.compound_exprs {
        if type_name == ret_type {
            // Same-type match: arms return the same type (reconstruct)
            for scrutinee in exprs {
                let arms = vec![
                    crate::ast::MatchArm {
                        pattern: crate::ast::Pattern::EnumVariant(
                            "Const".into(),
                            vec![crate::ast::Pattern::Binding("_v".into())],
                        ),
                        guard: None,
                        body: Box::new(Expr::Call(
                            "Const".into(),
                            vec![Expr::Identifier("_v".into())],
                            None,
                        )),
                    },
                    crate::ast::MatchArm {
                        pattern: crate::ast::Pattern::EnumVariant(
                            "Add".into(),
                            vec![
                                crate::ast::Pattern::Binding("_l".into()),
                                crate::ast::Pattern::Binding("_r".into()),
                            ],
                        ),
                        guard: None,
                        body: Box::new(Expr::Call(
                            "Add".into(),
                            vec![Expr::Identifier("_l".into()), Expr::Identifier("_r".into())],
                            None,
                        )),
                    },
                    crate::ast::MatchArm {
                        pattern: crate::ast::Pattern::Wildcard,
                        guard: None,
                        body: Box::new(scrutinee.clone()),
                    },
                ];
                result.push(Expr::Match(Box::new(scrutinee.clone()), arms));
            }
        }
        if ret_type == "Int" {
            // Cross-type extraction: Expr → Int.
            // Arm bodies return the first field (assumed Int for Const).
            for scrutinee in exprs {
                let arms = vec![
                    // Const(val) → val (extract Int)
                    crate::ast::MatchArm {
                        pattern: crate::ast::Pattern::EnumVariant(
                            "Const".into(),
                            vec![crate::ast::Pattern::Binding("_v".into())],
                        ),
                        guard: None,
                        body: Box::new(Expr::Identifier("_v".into())),
                    },
                    // Add(l, r) → 0 (fallback — first field is Expr, return 0)
                    crate::ast::MatchArm {
                        pattern: crate::ast::Pattern::EnumVariant(
                            "Add".into(),
                            vec![
                                crate::ast::Pattern::Binding("_l".into()),
                                crate::ast::Pattern::Binding("_r".into()),
                            ],
                        ),
                        guard: None,
                        body: Box::new(Expr::Decimal(0)),
                    },
                    // Wildcard → 0
                    crate::ast::MatchArm {
                        pattern: crate::ast::Pattern::Wildcard,
                        guard: None,
                        body: Box::new(Expr::Decimal(0)),
                    },
                ];
                result.push(Expr::Match(Box::new(scrutinee.clone()), arms));
            }
        }
    }

    result
}

/// 2026-07-28: Evaluate candidates against examples and prune to useful set.
/// Keeps expressions that produce unique, non-constant output vectors across the
/// examples. This is the key checkpoint: depth 3's ~500 candidates reduce to ~50.
fn prune_level(
    candidates: Vec<Expr>,
    param_names: &[String],
    param_types: &[String],
    examples: &[DerivationExample],
) -> EvalCache {
    use std::collections::HashSet;
    let mut level_cache = LevelCache::empty();
    let mut eval_results: Vec<(Expr, Vec<crate::interpreter::Value>)> = Vec::new();
    let mut seen: HashSet<Vec<i64>> = HashSet::new();

    for expr in &candidates {
        // Evaluate against all examples — store raw results for caching
        let eval_outputs: Vec<crate::interpreter::Value> = examples
            .iter()
            .map(|ex| {
                let input_values = example_inputs_to_values(ex, param_names);
                let mut ctx = SynthesisEvalContext::new();
                for (name, val) in param_names.iter().zip(input_values.iter()) {
                    ctx.bind(name, val.clone());
                }
                evaluate_synthesized(expr, &mut ctx)
                    .unwrap_or(crate::interpreter::Value::Atom(Atom::Int(0)))
            })
            .collect();
        // Compute fingerprint from already-evaluated results
        let outputs: Vec<(i64, i64)> = eval_outputs
            .iter()
            .map(|v| match v {
                Value::Atom(Atom::Int(n)) => (*n, 1),
                Value::Atom(Atom::Float(f)) => (f.to_bits() as i64, 2),
                Value::Bits(b) => (b.iter().fold(0i64, |acc, &x| (acc << 1) | x as i64), 3),
                Value::Sum {
                    name,
                    payload: fields,
                } => {
                    let name_hash = name
                        .bytes()
                        .fold(0i64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as i64));
                    let fields_hash = fields.iter().fold(0i64, |acc, f| match f {
                        Value::Atom(Atom::Int(n)) => acc.wrapping_mul(31).wrapping_add(*n),
                        Value::Atom(Atom::Float(fv)) => {
                            acc.wrapping_mul(31).wrapping_add(fv.to_bits() as i64)
                        }
                        Value::Bits(b) => acc
                            .wrapping_mul(31)
                            .wrapping_add(b.first().copied().unwrap_or(0) as i64),
                        Value::Sum {
                            name: n,
                            payload: fs,
                        } => {
                            let h = n
                                .bytes()
                                .fold(0i64, |a, b| a.wrapping_mul(31).wrapping_add(b as i64));
                            acc.wrapping_mul(31)
                                .wrapping_add(h)
                                .wrapping_add(fs.len() as i64)
                        }
                        _ => acc.wrapping_mul(31),
                    });
                    (name_hash, 10 + fields_hash)
                }
                _ => (0, 0),
            })
            .collect();

        // Prune constant outputs (same value for all inputs) — only if 2+ examples.
        // With a single example, every expression appears constant.
        // 2026-07-28: Keep small integer constants [-128, 255] — they're essential
        // building blocks for shift amounts and bit masks (e.g., `1 + 1 = 2` for
        // `x0 >> 2`). Prune only large garbage constants (e.g., `x0 * 0 + 9999999`).
        if examples.len() >= 2 && outputs.iter().all(|&v| v == outputs[0]) {
            let val = outputs[0].0; // first element's value
            if val < -128 || val > 255 {
                // Only prune if this is an Int-type expression (kind 1)
                if outputs[0].1 == 1 {
                    continue;
                }
            }
        }

        // Prune redundant output signatures — use the raw tuple as hash key
        let seen_key: Vec<i64> = outputs.iter().flat_map(|(v, k)| vec![*v, *k]).collect();
        if !seen.insert(seen_key) {
            continue;
        }

        // 2026-07-28: Phase 5 — Determine type and add to cache.
        // Check if the expression is an identifier whose parameter is a compound type.
        let is_compound_param = matches!(expr, Expr::Identifier(name) if {
            param_names.iter().position(|n| n == name)
                .and_then(|i| param_types.get(i))
                .map(|s| crate::derive::engine::is_compound_type(s.as_str()))
                .unwrap_or(false)
        });
        // Store eval results in the cache (Expr as Vec key, pointer-based lookup)
        eval_results.push((expr.clone(), eval_outputs));

        let expr_ty = expr_type_hint_with_params(expr, param_names, param_types);
        if is_compound_param {
            level_cache
                .compound_exprs
                .entry("Expr".to_string())
                .or_default()
                .push(expr.clone());
        } else if let Some(ty) = expr_ty {
            level_cache.push(expr.clone(), ty);
        } else {
            level_cache
                .compound_exprs
                .entry("Expr".to_string())
                .or_default()
                .push(expr.clone());
        }
    }

    EvalCache {
        level: level_cache,
        results: eval_results,
    }
}

/// 2026-07-28: Infer the return type of an expression (best-effort for pruning).
/// Uses param_types to resolve identifier types at depth 1 (bare variables).
fn expr_type_hint_with_params(
    expr: &Expr,
    param_names: &[String],
    param_types: &[String],
) -> Option<&'static str> {
    match expr {
        Expr::Decimal(_) => Some("Int"),
        Expr::Float(_) => Some("Float"),
        Expr::Bool(_) => Some("Bool"),
        Expr::Identifier(name) => {
            let idx = param_names.iter().position(|n| n == name);
            match idx.and_then(|i| param_types.get(i).map(|s| s.as_str())) {
                Some("Float") => Some("Float"),
                Some("Bool") => Some("Bool"),
                _ => Some("Int"),
            }
        }
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => {
            expr_type_hint_with_params(inner, param_names, param_types).or(Some("Int"))
        }
        Expr::UnaryOp(UnaryOpKind::Not, _) => Some("Bool"),
        Expr::BinaryOp(op, lhs, _) => {
            let lhs_ty = expr_type_hint_with_params(lhs, param_names, param_types).unwrap_or("Int");
            op_result_type(op, lhs_ty, lhs_ty)
        }
        // 2026-07-28: Phase 5 — Compound type inference
        Expr::Call(name, _, _) => {
            // Constructor calls: Const → Expr, Add → Expr, etc.
            if is_compound_type(name) {
                Some("Expr") // All current compound types map to "Expr"
            } else {
                Some("Int") // Fallback for unknown constructors
            }
        }
        Expr::Field(inner, _) => {
            // Field access on a compound expression returns the field type.
            // For Expr with Const-val → Int, Add-left/right → Expr.
            // For now, assume field access on compound returns the compound's type.
            expr_type_hint_with_params(inner, param_names, param_types)
        }
        Expr::Match(scrut, _) => {
            // Match returns the type of its arms, which is the return type
            // of the expression. Default to the scrutinee's type.
            expr_type_hint_with_params(scrut, param_names, param_types)
        }
        _ => None,
    }
}

fn unary_result_type(kind: &UnaryOpKind, ty: &str) -> Option<&'static str> {
    match kind {
        UnaryOpKind::Neg => {
            if ty == "Int" || ty == "Float" {
                Some("Int")
            } else {
                None
            }
        }
        UnaryOpKind::Not => {
            if ty == "Bool" {
                Some("Bool")
            } else {
                None
            }
        }
        UnaryOpKind::BitNot => None,
    }
}

fn push_typed_constants(ty: &str, result: &mut Vec<Expr>) {
    match ty {
        "Int" => {
            result.push(Expr::Decimal(0));
            result.push(Expr::Decimal(1));
            result.push(Expr::Decimal(-1));
        }
        "Float" => {
            result.push(Expr::Float(0.0));
            result.push(Expr::Float(1.0));
        }
        "Bool" => {
            result.push(Expr::Bool(true));
            result.push(Expr::Bool(false));
        }
        _ => {}
    }
}

// ── C.3 — Depth-Bounded Search with Cost Pruning ──────────────────────

/// 2026-07-28: Phase C.3 — A successfully synthesized program with its cost.
/// 2026-07-29: Added helpers (abstraction discovery). Helper functions
/// consumed by the synthesized body are emitted as defn blocks in the
/// doppelganger output. See abstraction-discovery plan §5.
#[derive(Debug, Clone)]
pub struct SynthesizedProgram {
    pub body: Vec<Expr>,
    pub cost: u64,
    pub depth: u8,
    /// 2026-07-29: Helper functions consumed by the synthesized body.
    /// Only helpers with use_count > 0 are included. Emitted as defn
    /// blocks before the main function in doppelganger output.
    pub helpers: Vec<crate::derive::library::HelperFunction>,
}

/// 2026-07-28: Phase C.3 — Enumerate all programs up to max_depth,
/// evaluate against all examples, return lowest-cost match.
/// 2026-07-28: Phase C.3 — Enumerate all programs up to max_depth,
/// evaluate against all examples, return lowest-cost match.
/// 2026-07-28: Refactored — extract evaluation helper, use lazy callback generation.
pub fn synthesize_enumerative(
    param_types: &[String],
    ret_type: &str,
    param_names: &[String],
    examples: &[DerivationExample],
    cost_model: &CostModel,
    max_depth: u8,
    beam_width: usize,
) -> Result<SynthesizedProgram, SynthesizeError> {
    if examples.is_empty() {
        return Err(SynthesizeError::NoExamples("synthesize_enumerative".into()));
    }

    let mut best: Option<SynthesizedProgram> = None;
    // 2026-07-29: Abstraction discovery helper registry. Collects all
    // discovered helpers across depth levels. After solution found, only
    // helpers with use_count > 0 are attached to SynthesizedProgram.
    use crate::derive::HelperFunction;
    let mut helper_registry: Vec<HelperFunction> = Vec::new();
    // 2026-07-29: Counter for unique helper names (_h0, _h1, ...).
    // Persisted across depth levels to avoid name collisions.
    let mut helper_name_counter: usize = 0;
    // 2026-07-28: Checkpointed depth search — start with empty level cache,
    // generate each depth from the pruned set of the previous depth.
    let mut prev_cache = EvalCache::empty();

    for depth in 1..=max_depth {
        // Generate candidates for this depth from the pruned previous level
        let candidates = generate_next_level(param_names, param_types, ret_type, &prev_cache);
        if candidates.is_empty() {
            continue;
        }

        // 2026-07-28: Evaluate in cost order — cheapest first.
        // Sorting by cost ensures general formulas (e.g., If(x0<0, -x0, x0)
        // at cost ~14) are evaluated before table-lookup ite chains (cost ~56).
        let mut sorted: Vec<(u64, &Expr)> = candidates
            .iter()
            .map(|c| (cost_model.cost_of_expr(c), c))
            .collect();
        sorted.sort_by_key(|(cost, _)| *cost);

        // 2026-07-28: Adaptive beam — scales with depth.
        // Effective beam = base_beam * depth. At depth 3 with base 5000,
        // effective beam = 15000 (enough for 2-param IF expressions).
        // At depth 4, effective beam = 20000 (up from 15000).
        let effective_beam = beam_width * depth as usize;
        if sorted.len() > effective_beam {
            sorted.truncate(effective_beam);
        }
        // 2026-07-28: Ensure at least 200 IF expressions are in the beam.
        // IF expressions enable conditional logic (abs, min, max) but start at
        // cost 14+, so they may be excluded by cheaper binary ops.
        let if_count = sorted
            .iter()
            .filter(|(_, c)| matches!(c, Expr::If(_, _, _)))
            .count();
        if if_count < 200 {
            // Find position of 200th IF in the full sorted list and expand beam
            let mut if_seen = 0;
            for (idx, (_, c)) in sorted.iter().enumerate() {
                if matches!(c, Expr::If(_, _, _)) {
                    if_seen += 1;
                }
            }
            if if_seen < 200 {
                // Need wider beam — re-sort and expand from scratch
                sorted = candidates
                    .iter()
                    .map(|c| (cost_model.cost_of_expr(c), c))
                    .collect();
                sorted.sort_by_key(|(cost, _)| *cost);
                // Find position of 200th IF
                let mut pos = sorted.len();
                let mut count = 0;
                for (i, (_, c)) in sorted.iter().enumerate() {
                    if matches!(c, Expr::If(_, _, _)) {
                        count += 1;
                        if count >= 200 {
                            pos = i + 1;
                            break;
                        }
                    }
                }
                // Ensure beam + 5000 margin for IF safety
                let expanded = pos.max(effective_beam).min(candidates.len());
                sorted.truncate(expanded);
            }
        }

        let mut next_level: Vec<Expr> = Vec::new();
        for (cost, candidate) in &sorted {
            if best.as_ref().map_or(false, |b| *cost >= b.cost) {
                next_level.push((*candidate).clone());
                continue;
            }

            if candidate_matches_all_examples(*candidate, param_names, examples, &prev_cache) {
                best = Some(SynthesizedProgram {
                    body: vec![(*candidate).clone()],
                    cost: *cost,
                    depth,
                    helpers: vec![],
                });
                if depth >= 3 {
                    return Ok(best.unwrap());
                }
            }
            next_level.push((*candidate).clone());
        }

        // Checkpoint: prune the candidates to unique, non-constant outputs
        prev_cache = prune_level(next_level, param_names, param_types, examples);

        // 2026-07-29: Abstraction discovery — extract reusable sub-expressions
        // from the pruned LevelCache and promote to helper functions.
        // Adapted from Koza ADFs [GP'92] and Feser et al. λ² [PLDI'15].
        // 2026-07-29: Anti-unification abstraction discovery — find common
        // sub-structure between pairs of LevelCache expressions and REPLACE
        // originals with helper calls. Adapted from Feser et al. λ² (PLDI 2015) §4.
        // Only activate at depth >= 2.
        if depth >= 2 {
            let helpers = crate::derive::library::discover_and_register_helpers(
                &mut prev_cache.level,
                param_names,
                param_types,
                &crate::derive::library::DISCOVER_CONFIG,
                &mut helper_name_counter,
            );
            if !helpers.is_empty() {
                eprintln!(
                    "  anti-unify: discovered {} helper(s) at depth {}",
                    helpers.len(),
                    depth
                );
                helper_registry.extend(helpers);
            }
        }

        // 2026-07-29: Garbage-collect unused helpers after registration.
        if !helper_registry.is_empty() {
            crate::derive::library::gc_helpers(&mut prev_cache.level, &mut helper_registry);
        }
    }

    // 2026-07-29: If a solution was found, attach the consumed helpers.
    // Only helpers with use_count > 0 are included in the output.
    if let Some(ref mut best) = best {
        best.helpers = helper_registry;
    }

    best.ok_or_else(|| SynthesizeError::NoSolution("enumerative search failed".into()))
}

/// 2026-07-28: Evaluate a candidate expression against all derivation examples.
fn candidate_matches_all_examples(
    candidate: &Expr,
    param_names: &[String],
    examples: &[DerivationExample],
    eval_cache: &EvalCache,
) -> bool {
    // 2026-07-28: Check eval cache first — avoids re-evaluating expressions
    // that were already evaluated during pruning.
    if let Some(cached) = eval_cache.get(candidate) {
        return cached
            .iter()
            .enumerate()
            .all(|(i, val): (usize, &crate::interpreter::Value)| {
                let expected_input_ctx = || -> SynthesisEvalContext {
                    let mut c = SynthesisEvalContext::new();
                    c
                };
                let expected = evaluate_synthesized(&examples[i].output, &mut expected_input_ctx());
                let tol = examples[i].tolerance.unwrap_or(0.0);
                let ok_val: Result<
                    crate::interpreter::Value,
                    crate::derive::engine::SynthesisEvalError,
                > = Ok(val.clone());
                match (ok_val, expected) {
                    (Ok(ref actual), Ok(ref exp)) => values_within_tolerance(actual, exp, tol),
                    _ => false,
                }
            });
    }
    // 2026-07-28: Pre-screen — check first example first, short-circuit on failure.
    // Most candidates (~90%) fail on the first example alone, so this avoids 3-5
    // unnecessary evaluations per rejected candidate.
    let first_ex = &examples[0];
    let first_ok = {
        let input_values = example_inputs_to_values(first_ex, param_names);
        let mut ctx = SynthesisEvalContext::new();
        for (name, val) in param_names.iter().zip(input_values.iter()) {
            ctx.bind(name, val.clone());
        }
        match evaluate_synthesized(candidate, &mut ctx) {
            Ok(actual) => {
                let expected_input_ctx = || -> SynthesisEvalContext {
                    let mut c = SynthesisEvalContext::new();
                    c
                };
                match evaluate_synthesized(&first_ex.output, &mut expected_input_ctx()) {
                    Ok(expected) => {
                        let tol = first_ex.tolerance.unwrap_or(0.0);
                        values_within_tolerance(&actual, &expected, tol)
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    };
    if !first_ok {
        return false;
    }
    // Full check on remaining examples (skip index 0)
    examples.iter().enumerate().skip(1).all(|(i, ex)| {
        let input_values = example_inputs_to_values(ex, param_names);
        let mut ctx = SynthesisEvalContext::new();
        for (name, val) in param_names.iter().zip(input_values.iter()) {
            ctx.bind(name, val.clone());
        }
        let result = evaluate_synthesized(candidate, &mut ctx);
        let expected_input_ctx = || -> SynthesisEvalContext {
            let mut c = SynthesisEvalContext::new();
            c
        };
        let expected = evaluate_synthesized(&ex.output, &mut expected_input_ctx());
        match (result, expected) {
            (Ok(actual), Ok(exp)) => {
                let tol = ex.tolerance.unwrap_or(0.0);
                values_within_tolerance(&actual, &exp, tol)
            }
            _ => false,
        }
    })
}

/// Convert the input expressions in a DerivationExample to Values.
/// Uses `expr_to_value` to evaluate each input expression.
fn example_inputs_to_values(ex: &DerivationExample, param_names: &[String]) -> Vec<Value> {
    // 2026-07-28: Phase 5 — Use evaluate_synthesized for compound types.
    // The old expr_to_value only handles constants (Decimal, Float, Bool)
    // and returns Value::Atom(Atom::Int(0)) for constructors like Call("Const", ...).
    // evaluate_synthesized handles all expression types including constructors.
    ex.inputs
        .iter()
        .map(|e| {
            if matches!(e, Expr::Call(_, _, _)) {
                let mut ctx = SynthesisEvalContext::new();
                evaluate_synthesized(e, &mut ctx).unwrap_or(Value::Atom(Atom::Int(0)))
            } else {
                expr_to_value(e)
            }
        })
        .collect()
}

/// Convert an expression to a Value (for constant inputs from examples).
fn expr_to_value(expr: &Expr) -> Value {
    match expr {
        Expr::Decimal(n) => Value::Atom(Atom::Int(*n)),
        Expr::Float(f) => Value::Atom(Atom::Float(*f)),
        Expr::Bool(b) => Value::Bits(vec![if *b { 1 } else { 0 }]),
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => match expr_to_value(inner) {
            Value::Atom(Atom::Int(n)) => Value::Atom(Atom::Int(-n)),
            Value::Atom(Atom::Float(f)) => Value::Atom(Atom::Float(-f)),
            _ => Value::Void,
        },
        Expr::Identifier(_) => Value::Atom(Atom::Int(0)),
        _ => Value::Atom(Atom::Int(0)),
    }
}

// ── Legacy Compatibility ──────────────────────────────────────────────

/// Enumerative search using actual parameter names and types.
/// 2026-07-12: Original entry point. 2026-07-28: Accepts params instead of hardcoding.
/// 2026-07-28: Default beam width for candidate pruning at each depth.
/// 2026-07-29: Capped at 4000 — depth scaling (beam * depth) would exceed
/// 15000 at depth 4, letting too many candidates through.
/// Depth 3: beam = 4000 * 3 = 12000 (enough for IF expressions + binary ops)
/// Depth 4: beam = 4000 * 4 = 16000 (capped at 16000 — manageable <60s)
const DEFAULT_BEAM_WIDTH: usize = 4000;

pub fn enumerative_search(
    name: &str,
    params: &[(String, Type)],
    ret_type: &Type,
    examples: &[DerivationExample],
    max_depth: usize,
) -> Result<Option<Expr>, SynthesizeError> {
    if examples.is_empty() {
        return Err(SynthesizeError::NoExamples(name.to_string()));
    }
    let param_names: Vec<String> = params
        .iter()
        .map(|(n, _): &(String, Type)| n.clone())
        .collect();
    let param_types: Vec<String> = params
        .iter()
        .map(|(_, t): &(String, Type)| t.to_string())
        .collect();
    let ret_type_str = if ret_type == &Type::int() {
        "Int".to_string()
    } else {
        ret_type.to_string()
    };
    let max_depth_u8 = max_depth.min(8) as u8;
    let cost_model = CostModel::default();
    // 2026-07-28: Use proportional beam for multi-param functions.
    // Each additional param multiplies the candidate space at depth 3.
    // 5000 keeps depth 3 completion under 30s for 2-param functions.
    let beam = if params.len() >= 2 {
        5000
    } else {
        DEFAULT_BEAM_WIDTH
    };

    match synthesize_enumerative(
        &param_types,
        &ret_type_str,
        &param_names,
        examples,
        &cost_model,
        max_depth_u8,
        beam,
    ) {
        Ok(prog) => Ok(prog.body.into_iter().next()),
        Err(SynthesizeError::NoSolution(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::Span;

    fn dummy_span() -> Span {
        Span::dummy()
    }

    fn example(inputs: Vec<Expr>, output: Expr, tolerance: Option<f64>) -> DerivationExample {
        DerivationExample {
            inputs,
            output: Box::new(output),
            tolerance,
            span: dummy_span(),
        }
    }

    // ── C.0 — Type-aware expression generation ─────────────────────

    #[test]
    fn test_generate_type_int() {
        let names: Vec<String> = vec!["x".into(), "y".into()];
        let types: Vec<String> = vec!["Int".into(), "Int".into()];
        let exprs = generate_typed_expressions(&names, &types, "Int", 2);
        assert!(!exprs.is_empty(), "should generate Int expressions");
        // Should include x, y, 0, 1, -1
        assert!(
            exprs
                .iter()
                .any(|e| matches!(e, Expr::Identifier(n) if n == "x"))
        );
        assert!(exprs.iter().any(|e| matches!(e, Expr::Decimal(0))));
        assert!(exprs.iter().any(|e| matches!(e, Expr::Decimal(1))));
        assert!(exprs.iter().any(|e| matches!(e, Expr::Decimal(-1))));
    }

    #[test]
    fn test_generate_type_bool() {
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Int".into()];
        let exprs = generate_typed_expressions(&names, &types, "Bool", 2);
        assert!(!exprs.is_empty(), "should generate Bool expressions");
        // Should include true, false, comparisons
        assert!(exprs.iter().any(|e| matches!(e, Expr::Bool(true))));
        assert!(exprs.iter().any(|e| matches!(e, Expr::Bool(false))));
    }

    #[test]
    fn test_generate_type_float() {
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Float".into()];
        let exprs = generate_typed_expressions(&names, &types, "Float", 2);
        assert!(!exprs.is_empty(), "should generate Float expressions");
        assert!(
            exprs
                .iter()
                .any(|e| matches!(e, Expr::Float(f) if (*f - 0.0).abs() < 1e-10))
        );
        assert!(
            exprs
                .iter()
                .any(|e| matches!(e, Expr::Float(f) if (*f - 1.0).abs() < 1e-10))
        );
    }

    #[test]
    fn test_generate_type_mismatch_rejected() {
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Int".into()];
        // x + true should not be generated (type mismatch)
        let exprs = generate_typed_expressions(&names, &types, "Int", 3);
        for e in &exprs {
            if let Expr::BinaryOp(_, lhs, rhs) = e {
                let lhs_is_bool = matches!(
                    lhs.as_ref(),
                    Expr::Bool(_) | Expr::UnaryOp(UnaryOpKind::Not, _)
                );
                let rhs_is_bool = matches!(
                    rhs.as_ref(),
                    Expr::Bool(_) | Expr::UnaryOp(UnaryOpKind::Not, _)
                );
                assert!(
                    !(lhs_is_bool || rhs_is_bool),
                    "Int-typed expression should not contain Bool operands: {:?}",
                    e
                );
            }
        }
    }

    #[test]
    fn test_generate_with_typed_constants() {
        let mut result = Vec::new();
        push_typed_constants("Float", &mut result);
        assert!(result.iter().any(|e| matches!(e, Expr::Float(_))));
        assert!(!result.iter().any(|e| matches!(e, Expr::Decimal(_))));
    }

    // ── C.1 — Interpreter-based evaluation ─────────────────────────

    #[test]
    fn test_evaluate_add_expr() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Identifier("y".into())),
        );
        let mut ctx = SynthesisEvalContext::new();
        ctx.bind("x", Value::Atom(Atom::Int(2)));
        ctx.bind("y", Value::Atom(Atom::Int(3)));
        let result = evaluate_synthesized(&expr, &mut ctx).unwrap();
        assert_eq!(result, Value::Atom(Atom::Int(5)));
    }

    #[test]
    fn test_evaluate_cond_expr() {
        let expr = Expr::If(
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Gt,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Decimal(0)),
            )),
            Box::new(Expr::Identifier("x".into())),
            Some(Box::new(Expr::UnaryOp(
                UnaryOpKind::Neg,
                Box::new(Expr::Identifier("x".into())),
            ))),
        );
        let mut ctx = SynthesisEvalContext::new();
        ctx.bind("x", Value::Atom(Atom::Int(-3)));
        let result = evaluate_synthesized(&expr, &mut ctx).unwrap();
        assert_eq!(result, Value::Atom(Atom::Int(3)));
    }

    #[test]
    fn test_evaluate_nested_expr() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Mul,
            Box::new(Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Identifier("y".into())),
            )),
            Box::new(Expr::Identifier("z".into())),
        );
        let mut ctx = SynthesisEvalContext::new();
        ctx.bind("x", Value::Atom(Atom::Int(2)));
        ctx.bind("y", Value::Atom(Atom::Int(3)));
        ctx.bind("z", Value::Atom(Atom::Int(4)));
        let result = evaluate_synthesized(&expr, &mut ctx).unwrap();
        assert_eq!(result, Value::Atom(Atom::Int(20)));
    }

    #[test]
    fn test_evaluate_undefined_var() {
        let expr = Expr::Identifier("undefined".into());
        let mut ctx = SynthesisEvalContext::new();
        let result = evaluate_synthesized(&expr, &mut ctx);
        assert!(matches!(
            result,
            Err(SynthesisEvalError::UndefinedVariable(_))
        ));
    }

    // ── C.2 — Cost Model ───────────────────────────────────────────

    #[test]
    fn test_cost_constant_small() {
        let model = CostModel::default();
        let small = model.cost_of_expr(&Expr::Decimal(0));
        let large = model.cost_of_expr(&Expr::Decimal(65536));
        assert!(
            small <= large,
            "small constant should cost <= large constant"
        );
    }

    #[test]
    fn test_cost_constant_defaults() {
        let model = CostModel::default();
        assert_eq!(model.constant, 1);
        assert_eq!(model.variable, 1);
        assert_eq!(model.unary_op, 2);
        assert_eq!(model.binary_op, 3);
    }

    #[test]
    fn test_cost_binary_op_greater_than_variable() {
        let model = CostModel::default();
        let var_cost = model.cost_of_expr(&Expr::Identifier("x".into()));
        let binop = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Identifier("y".into())),
        );
        let binop_cost = model.cost_of_expr(&binop);
        assert!(binop_cost > var_cost, "x + y should cost more than x");
    }

    #[test]
    fn test_cost_prefers_simple() {
        let model = CostModel::default();
        let simple = Expr::Identifier("x".into());
        let complex = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".into())),
            Box::new(Expr::Decimal(0)),
        );
        assert!(
            model.cost_of_expr(&simple) < model.cost_of_expr(&complex),
            "simpler expression should have lower cost"
        );
    }

    // ── C.3 — Depth-bounded search ─────────────────────────────────

    #[test]
    fn test_enumerative_simple_add() {
        let examples = vec![
            example(
                vec![Expr::Decimal(2), Expr::Decimal(3)],
                Expr::Decimal(5),
                None,
            ),
            example(
                vec![Expr::Decimal(0), Expr::Decimal(5)],
                Expr::Decimal(5),
                None,
            ),
        ];
        let names: Vec<String> = vec!["x".into(), "y".into()];
        let types: Vec<String> = vec!["Int".into(), "Int".into()];
        let model = CostModel::default();
        let result =
            synthesize_enumerative(&types, "Int", &names, &examples, &model, 3, 2000).unwrap();
        assert_eq!(
            result.cost,
            model.cost_of_expr(&Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Identifier("y".into())),
            ))
        );
    }

    #[test]
    fn test_enumerative_identity() {
        let examples = vec![
            example(vec![Expr::Decimal(0)], Expr::Decimal(0), None),
            example(vec![Expr::Decimal(42)], Expr::Decimal(42), None),
        ];
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Int".into()];
        let model = CostModel::default();
        let result =
            synthesize_enumerative(&types, "Int", &names, &examples, &model, 2, 2000).unwrap();
        assert_eq!(
            result.cost,
            model.cost_of_expr(&Expr::Identifier("x".into())),
            "identity should be cheaper than adding 0"
        );
    }

    #[test]
    fn test_enumerative_constant() {
        let examples = vec![
            example(vec![Expr::Decimal(0)], Expr::Decimal(1), None),
            example(vec![Expr::Decimal(100)], Expr::Decimal(1), None),
        ];
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Int".into()];
        let model = CostModel::default();
        // The constant 1 (in the default pool) should be found
        let result = synthesize_enumerative(&types, "Int", &names, &examples, &model, 2, 2000);
        assert!(result.is_ok(), "should find constant 1: {:?}", result.err());
    }

    #[test]
    fn test_enumerative_no_solution() {
        let examples = vec![example(vec![Expr::Decimal(1)], Expr::Decimal(999), None)];
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Int".into()];
        let model = CostModel::default();
        let result = synthesize_enumerative(&types, "Int", &names, &examples, &model, 2, 2000);
        assert!(result.is_err(), "999 should not be found at depth 2");
    }

    #[test]
    fn test_enumerative_with_tolerance() {
        let examples = vec![example(
            vec![Expr::Float(1.0), Expr::Float(2.0)],
            Expr::Float(3.0),
            Some(0.01),
        )];
        let names: Vec<String> = vec!["x".into(), "y".into()];
        let types: Vec<String> = vec!["Float".into(), "Float".into()];
        let model = CostModel::default();
        let result = synthesize_enumerative(&types, "Float", &names, &examples, &model, 3, 2000);
        assert!(
            result.is_ok(),
            "should find x + y with tolerance: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_enumerative_empty_examples() {
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Int".into()];
        let model = CostModel::default();
        let result = synthesize_enumerative(&types, "Int", &names, &[], &model, 2, 2000);
        assert!(matches!(result, Err(SynthesizeError::NoExamples(_))));
    }

    #[test]
    fn test_enumerative_cost_pruning() {
        // With many examples, expensive candidates should be skipped
        // if a cheap one already matches.
        let examples = vec![
            example(vec![Expr::Decimal(0)], Expr::Decimal(0), None),
            example(vec![Expr::Decimal(42)], Expr::Decimal(42), None),
        ];
        let names: Vec<String> = vec!["x".into()];
        let types: Vec<String> = vec!["Int".into()];
        let model = CostModel::default();
        // identity (x) should be found before x + 0
        let result =
            synthesize_enumerative(&types, "Int", &names, &examples, &model, 3, 2000).unwrap();
        assert!(
            result.body.iter().any(|e| matches!(e, Expr::Identifier(_))),
            "should prefer identity over x + 0: {:?}",
            result.body
        );
    }

    // ── Legacy compatibility ───────────────────────────────────────

    #[test]
    fn test_enumerative_search_identity() {
        let examples = vec![example(vec![Expr::Decimal(42)], Expr::Decimal(42), None)];
        let params = vec![("x0".into(), Type::int())];
        let result = enumerative_search("id", &params, &Type::int(), &examples, 3).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_empty_examples() {
        let params = vec![("x".into(), Type::int())];
        let result = enumerative_search("f", &params, &Type::int(), &[], 3);
        assert!(result.is_err());
    }

    #[test]
    fn test_no_solution() {
        let examples = vec![example(vec![Expr::Decimal(1)], Expr::Decimal(999), None)];
        let params = vec![("x0".into(), Type::int())];
        let result = enumerative_search("f", &params, &Type::int(), &examples, 2).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_enumerative_search_expr_field() {
        // Test that compound type (Expr) synthesis works.
        // Input: Expr value Const(5), expected output: Int(5)
        // The engine needs to find a Match that extracts the field from Const.
        let const5 = Expr::Call("Const".into(), vec![Expr::Decimal(5)], None);
        let const42 = Expr::Call("Const".into(), vec![Expr::Decimal(42)], None);
        fn make_ex(inputs: Vec<Expr>, output: Expr) -> DerivationExample {
            DerivationExample {
                inputs,
                output: Box::new(output),
                tolerance: None,
                span: crate::errors::Span::dummy(),
            }
        }
        let examples = vec![
            make_ex(vec![const5.clone()], Expr::Decimal(5)),
            make_ex(vec![const42.clone()], Expr::Decimal(42)),
        ];
        let params = vec![("e".into(), Type::Custom("Expr".into()))];
        // Int return type, Expr param — the engine should find Const extraction
        let result = enumerative_search("eval", &params, &Type::int(), &examples, 3);
        match &result {
            Ok(Some(expr)) => eprintln!("  DIAG: found expr: {:?}", expr),
            Ok(None) => eprintln!("  DIAG: no solution found at depth 3"),
            Err(e) => eprintln!("  DIAG: error: {:?}", e),
        }
        assert!(
            result.is_ok(),
            "compound type search should succeed: {:?}",
            result
        );
        assert!(result.unwrap().is_some(), "should find a solution");
    }

    #[test]
    fn test_evaluate_call_produces_sum_with_field_access() {
        // 2026-08-06 (Slice H): a Call evaluates to Value::Sum; numeric field
        // access extracts a payload element.
        let call = Expr::Call(
            "Pair".into(),
            vec![Expr::Decimal(3), Expr::Decimal(9)],
            None,
        );
        let mut ctx = SynthesisEvalContext::new();
        let val = evaluate_synthesized(&call, &mut ctx).unwrap();
        match &val {
            crate::interpreter::Value::Sum { name, payload } => {
                assert_eq!(name, "Pair");
                assert_eq!(
                    payload,
                    &vec![
                        crate::interpreter::Value::int(3),
                        crate::interpreter::Value::int(9)
                    ]
                );
            }
            other => panic!("expected Sum, got {other:?}"),
        }
        let field = Expr::Field(Box::new(call), "1".into());
        let v = evaluate_synthesized(&field, &mut ctx).unwrap();
        assert_eq!(v.as_i64(), Some(9));
    }
}
