// ── Three-Tier Overfitting Prevention ──────────────────────────────────
// 2026-07-28: Tier 1 — Identity-op pruning (generation-time).
//              Tier 2 — Random-input contract verification (post-synthesis).
//              Tier 3 — Boundary testing + constant-output detection.
//
// Architecture: verify_candidate() is called from synthesize() after a
// candidate matches all examples. If verification fails, the candidate is
// rejected and search continues for a different expression.

use crate::ast::{Expr, Type};
use crate::derive::SynthesizeError;
use crate::derive::engine::SynthesisEvalContext;
use crate::derive::engine::evaluate_synthesized;
use crate::interpreter::Atom;

/// Result of candidate verification.
#[derive(Debug)]
pub enum VerifyResult {
    /// Candidate passed all verification checks.
    Pass,
    /// Candidate failed verification.
    /// Fields: failing inputs (one Vec<Expr> per param row), optional correct output
    /// (for re-synthesis counterexample injection), human-readable reason.
    /// 2026-07-29: Added Option<Expr> for correct output. The random verifier knows
    /// the correct output for reference mismatches (it's the reference's result) and
    /// for @result = expr postconditions (evaluated from the RHS). For evaluation
    /// errors and general postconditions, the correct output is None.
    Fail(Vec<Vec<Expr>>, Option<Expr>, String),
}

/// Generate test inputs for a function with given parameters.
/// Returns `sample_count` input vectors, each with one Expr per parameter.
/// Mix: boundary values (0, 1, -1, MAX, MIN) + random + example-adjacent.
pub fn generate_test_inputs(params: &[(String, Type)], sample_count: usize) -> Vec<Vec<Expr>> {
    let mut inputs = Vec::new();

    // Boundary values for Int params
    let boundaries: Vec<i64> = vec![0, 1, -1, i64::MAX, i64::MIN, 2, -2, 64, -64, 1024];

    for (_, ty) in params {
        if ty == &Type::int() || ty == &Type::bits(64) {
            for &b in &boundaries {
                let mut row = Vec::new();
                for (_, other_ty) in params {
                    if other_ty == &Type::int() || other_ty == &Type::bits(64) {
                        row.push(Expr::Decimal(b));
                    } else if other_ty == &Type::bool_() {
                        row.push(Expr::Bool(b != 0));
                    } else {
                        row.push(Expr::Decimal(0));
                    }
                }
                inputs.push(row);
            }
        }
    }

    // Fill up to sample_count with random-looking deterministic inputs
    let mut rng_state: i64 = 12345;
    while inputs.len() < sample_count {
        let mut row = Vec::new();
        for (_, ty) in params {
            if ty == &Type::int() || ty == &Type::bits(64) {
                rng_state = rng_state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                row.push(Expr::Decimal(rng_state.wrapping_abs()));
            } else if ty == &Type::bool_() {
                rng_state = rng_state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                row.push(Expr::Bool(rng_state & 1 == 1));
            } else {
                row.push(Expr::Decimal(0));
            }
        }
        inputs.push(row);
    }

    inputs
}

/// Verify a synthesized candidate against random inputs, boundary values,
/// and optional postcondition.
///
/// Returns `Pass` if the candidate produces no evaluation errors and satisfies
/// all checks. Returns `Fail` with the first failing input if any check fails.
///
/// Checks performed:
/// 1. No evaluation errors (division by zero, overflow) for ANY tested input
/// 2. If postcondition provided: evaluate post(result) and check it's true
/// 3. Constant-output detection: if candidate returns same value for ALL tested
///    inputs (while examples have varying outputs), reject.
/// 2026-07-29: ref_fn changed from Option<&Expr> to Option<(&Expr, &[String])>
/// to carry the reference function's own parameter names. The verifier binds
/// reference params by position (first ref param gets first candidate input),
/// so ref and candidate can use different parameter names.
pub fn verify_candidate(
    candidate: &Expr,
    params: &[(String, Type)],
    postcondition: Option<&Expr>,
    sample_count: usize,
    ref_fn: Option<(&Expr, &[String])>,
) -> VerifyResult {
    let inputs = generate_test_inputs(params, sample_count);

    // Track outputs for constant-output detection
    let mut seen_outputs: Vec<i64> = Vec::new();

    for input_row in &inputs {
        // Bind params to input values
        let mut ctx = SynthesisEvalContext::new();
        for (i, (name, _)) in params.iter().enumerate() {
            if let Some(val_expr) = input_row.get(i) {
                let val = expr_to_decimal(val_expr);
                ctx.bind(name, val);
            }
        }

        // Evaluate candidate
        let result = match evaluate_synthesized(candidate, &mut ctx) {
            Ok(v) => v,
            Err(e) => {
                return VerifyResult::Fail(
                    vec![input_row.clone()],
                    None,
                    format!("evaluation error: {:?}", e),
                );
            }
        };

        // Record output for constant detection
        match &result {
            crate::interpreter::Value::Atom(Atom::Int(n)) => seen_outputs.push(*n),
            _ => seen_outputs.push(0),
        }

        // Check postcondition if provided
        if let Some(post) = postcondition {
            let post_ctx = &mut SynthesisEvalContext::new();
            if let crate::interpreter::Value::Atom(Atom::Int(n)) = &result {
                post_ctx.bind("#Term", crate::interpreter::Value::Atom(Atom::Int(*n)));
            }
            let post_result = evaluate_synthesized(post, post_ctx);
            match post_result {
                Ok(crate::interpreter::Value::Bits(b)) => {
                    if b.is_empty() || b.iter().all(|&x| x == 0) {
                        return VerifyResult::Fail(
                            vec![input_row.clone()],
                            None,
                            format!("postcondition failed for output {:?}", result),
                        );
                    }
                }
                Ok(crate::interpreter::Value::Atom(Atom::Int(n))) => {
                    if n == 0 {
                        return VerifyResult::Fail(
                            vec![input_row.clone()],
                            None,
                            format!("postcondition failed for output {:?}", result),
                        );
                    }
                }
                _ => {}
            }
        }

        // 2026-07-29: Check reference function if provided.
        // Use the interpreter's eval_expr (not evaluate_synthesized) because
        // the reference body is an Expr::Block containing let statements and
        // a term statement. evaluate_synthesized does not handle blocks or
        // statements — only the interpreter does.
        // The interpreter signals normal return via RuntimeError::TermReturn(val).
        // Reference params are bound by position (first ref param gets first input),
        // so ref and candidate can use different parameter names.
        if let Some((ref_expr, ref_param_names)) = ref_fn {
            let mut ref_ctx = SynthesisEvalContext::new();
            for (i, ref_pname) in ref_param_names.iter().enumerate() {
                if let Some(val_expr) = input_row.get(i) {
                    let val = expr_to_decimal(val_expr);
                    ref_ctx.bind(ref_pname, val);
                }
            }
            let mut heap = crate::interpreter::VirtualHeap::new();
            let ref_result = crate::interpreter::eval_expr(
                ref_expr,
                &mut heap,
                &mut ref_ctx.bindings,
                &std::collections::HashMap::new(),
            );
            let ref_val = match ref_result {
                Ok(v) => v,
                // 2026-07-29: Term with value signals early return — this is
                // the normal termination path for a defn body.
                Err(crate::interpreter::RuntimeError::TermReturn(v)) => v,
                Err(e) => {
                    return VerifyResult::Fail(
                        vec![input_row.clone()],
                        None,
                        format!("reference evaluation error: {:?}", e),
                    );
                }
            };
            let tol = 0.0; // exact match for now
            if !crate::interpreter::values_within_tolerance(&result, &ref_val, tol) {
                let correct_output = val_to_expr_output(&ref_val);
                return VerifyResult::Fail(
                    vec![input_row.clone()],
                    Some(correct_output),
                    format!(
                        "reference mismatch: candidate={:?} ref={:?}",
                        result, ref_val
                    ),
                );
            }
        }
    }

    // Constant-output detection: if the candidate produces the same value
    // for ALL tested inputs, it's likely overfitted (unless there's only
    // one test input, which can't happen — sample_count >= 10).
    if seen_outputs.len() >= 2 {
        let first = seen_outputs[0];
        if seen_outputs.iter().all(|&v| v == first) {
            return VerifyResult::Fail(
                vec![],
                None,
                format!(
                    "constant output {} for all {} test inputs",
                    first,
                    seen_outputs.len()
                ),
            );
        }
    }

    VerifyResult::Pass
}

/// 2026-07-29: Convert an interpreter Value to an Expr for use as a
/// counterexample's correct output in DerivationExample.
fn val_to_expr_output(val: &crate::interpreter::Value) -> Expr {
    match val {
        crate::interpreter::Value::Atom(Atom::Int(n)) => Expr::Decimal(*n),
        crate::interpreter::Value::Atom(Atom::Float(f)) => Expr::Float(*f),
        crate::interpreter::Value::Bits(b) => {
            if b.len() == 1 && (b[0] == 0 || b[0] == 1) {
                Expr::Bool(b[0] == 1)
            } else {
                Expr::Decimal(
                    b.iter()
                        .fold(0i64, |acc, &x| acc.wrapping_shl(8) | x as i64),
                )
            }
        }
        _ => Expr::Decimal(0),
    }
}

/// Extract an i64 value from a constant expression (for test input generation).
fn expr_to_decimal(expr: &Expr) -> crate::interpreter::Value {
    match expr {
        Expr::Decimal(n) => crate::interpreter::Value::Atom(Atom::Int(*n)),
        Expr::Bool(b) => crate::interpreter::Value::Bits(vec![if *b { 1 } else { 0 }]),
        _ => crate::interpreter::Value::Atom(Atom::Int(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int_params(n: usize) -> Vec<(String, Type)> {
        (0..n).map(|i| (format!("x{}", i), Type::int())).collect()
    }

    #[test]
    fn test_generate_test_inputs_count() {
        let params = int_params(1);
        let inputs = generate_test_inputs(&params, 100);
        assert!(inputs.len() >= 10, "should generate at least 10 inputs");
        assert!(inputs.len() <= 200, "should not over-generate");
    }

    #[test]
    fn test_generate_test_inputs_includes_boundaries() {
        let params = int_params(1);
        let inputs = generate_test_inputs(&params, 10);
        let all_vals: Vec<i64> = inputs
            .iter()
            .filter_map(|row| row.first())
            .filter_map(|e| match e {
                Expr::Decimal(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert!(all_vals.contains(&0), "should contain 0");
        assert!(all_vals.contains(&1), "should contain 1");
        assert!(all_vals.contains(&(-1)), "should contain -1");
        assert!(all_vals.contains(&i64::MAX), "should contain MAX");
        assert!(all_vals.contains(&i64::MIN), "should contain MIN");
    }

    #[test]
    fn test_verify_trivial_identity() {
        let params = int_params(1);
        // x0 always passes — it's the identity function
        let candidate = Expr::Identifier("x0".into());
        let result = verify_candidate(&candidate, &params, None, 20, None);
        assert!(matches!(result, VerifyResult::Pass), "identity should pass");
    }

    #[test]
    fn test_verify_constant_detected() {
        let params = int_params(1);
        // Constant 42 — should be detected as constant-output
        let candidate = Expr::Decimal(42);
        let result = verify_candidate(&candidate, &params, None, 20, None);
        match result {
            VerifyResult::Fail(_, _, ref msg) => {
                assert!(
                    msg.contains("constant output"),
                    "expected constant output message, got: {}",
                    msg
                );
            }
            _ => panic!("constant should be detected: {:?}", result),
        }
    }

    #[test]
    fn test_verify_with_postcondition() {
        let params = int_params(1);
        // x0 (identity) — postcondition @result >= -1 is always true
        // Use a simple postcondition expression constructed as AST
        let candidate = Expr::Identifier("x0".into());
        // Postcondition: just `true` (always passes)
        let post = Expr::Bool(true);
        let result = verify_candidate(&candidate, &params, Some(&post), 100, None);
        assert!(
            matches!(result, VerifyResult::Pass),
            "should pass postcondition: {:?}",
            result
        );
    }

    #[test]
    fn test_verify_two_params() {
        let params = int_params(2);
        // x0 + x1 — should pass verification
        let candidate = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Add,
            Box::new(Expr::Identifier("x0".into())),
            Box::new(Expr::Identifier("x1".into())),
        );
        let result = verify_candidate(&candidate, &params, None, 20, None);
        assert!(matches!(result, VerifyResult::Pass), "addition should pass");
    }
}
