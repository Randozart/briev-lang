// ── Derivation Assertion Verification ──────────────────────────────────
// 2026-07-28: Phase B.0 — Assertion build gate.
// Every definition/txn with both body and derivation block is verified
// by evaluating each example through the interpreter and comparing to
// expected output. A mismatch aborts the build.
// Flat code: max 2 levels of nesting.

use crate::ast::{DerivationExample, TopLevel};
use crate::errors::RuntimeError;
use crate::interpreter::{Atom, values_within_tolerance, Interpreter, Value};

/// 2026-07-28: Phase B.0 — Verify all derivation examples against their
/// function bodies. Called after type-check, before codegen.
/// Errors are fatal — the build is aborted with exit code 64.
pub fn verify_derivation_assertions(
    program: &[TopLevel],
    interpreter: &mut Interpreter,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    for item in program {
        let result = verify_item(item, interpreter);
        if let Err(item_errors) = result {
            errors.extend(item_errors);
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Verify a single top-level item's derivation block against its body.
fn verify_item(item: &TopLevel, interp: &mut Interpreter) -> Result<(), Vec<String>> {
    let (name, body, derivation) = match item {
        TopLevel::Definition(d) => (&d.name, &d.body, d.derivation.as_ref()),
        TopLevel::Transaction(t) => (&t.name, &t.body, t.derivation.as_ref()),
        _ => return Ok(()),
    };

    let Some(derivation) = derivation else {
        return Ok(()); // No derivation block — skip
    };

    if body.is_empty() {
        return Ok(()); // No body (draft) — skip
    }

    let mut errors = Vec::new();
    for (i, example) in derivation.examples.iter().enumerate() {
        if let Err(msg) = verify_example(name, i, example, interp) {
            errors.push(msg);
        }
    }

    // 2026-07-28: Check [[postcondition]] for each example.
    if let Some(ref post) = derivation.postcondition {
        for (i, example) in derivation.examples.iter().enumerate() {
            if let Err(msg) = verify_postcondition(name, i, post, example, interp) {
                errors.push(msg);
            }
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Verify a single derivation example against the function body.
fn verify_example(
    name: &str,
    index: usize,
    example: &DerivationExample,
    interp: &mut Interpreter,
) -> Result<(), String> {
    // Evaluate inputs
    let args: Result<Vec<Value>, RuntimeError> = example.inputs
        .iter()
        .map(|input| interp.eval_expr(input))
        .collect();
    let args = match args {
        Ok(a) => a,
        Err(e) => return Err(format!(
            "{} example {}: input evaluation failed: {}",
            name, index + 1, e
        )),
    };

    // Evaluate body with those arguments
    let result = match interp.call_function(name, &args) {
        Ok(r) => r,
        Err(e) => return Err(format!(
            "{} example {}: body execution failed: {}",
            name, index + 1, e
        )),
    };

    // Evaluate expected output
    let expected = match interp.eval_expr(&example.output) {
        Ok(v) => v,
        Err(e) => return Err(format!(
            "{} example {}: expected output evaluation failed: {}",
            name, index + 1, e
        )),
    };

    // Compare with tolerance if applicable
    let match_ok = match example.tolerance {
        Some(tol) => values_within_tolerance(&result, &expected, tol),
        None => result == expected,
    };
    if !match_ok {
        return Err(format!(
            "{} example {}: expected {:?}, got {:?}",
            name, index + 1, expected, result
        ));
    }

    Ok(())
}

/// Verify a [[postcondition]] for a given example.
/// Evaluates the postcondition expression with #Term bound to the function's
/// actual output for this example's inputs.
fn verify_postcondition(
    name: &str,
    index: usize,
    post: &crate::ast::Expr,
    example: &DerivationExample,
    interp: &mut Interpreter,
) -> Result<(), String> {
    let args: Result<Vec<Value>, _> = example.inputs
        .iter()
        .map(|input| interp.eval_expr(input))
        .collect();
    let args = match args {
        Ok(a) => a,
        Err(e) => return Err(format!(
            "{} example {}: input evaluation failed: {}",
            name, index + 1, e
        )),
    };
    let result = match interp.call_function(name, &args) {
        Ok(r) => r,
        Err(e) => return Err(format!(
            "{} example {}: body execution failed: {}",
            name, index + 1, e
        )),
    };
    interp.state.insert("#Term".into(), result.clone());
    let post_result = match interp.eval_expr(post) {
        Ok(v) => v,
        Err(e) => {
            interp.state.remove("#Term");
            return Err(format!(
                "{} example {}: postcondition evaluation failed: {}",
                name, index + 1, e
            ));
        }
    };
    interp.state.remove("#Term");
    let pass = match &post_result {
        Value::Atom(Atom::Int(n)) => *n != 0,
        Value::Bits(b) => b.iter().any(|x| *x != 0),
        _ => false,
    };
    if pass {
        Ok(())
    } else {
        Err(format!(
            "{} example {}: postcondition violated (result={:?})",
            name, index + 1, result
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, DerivationBlock, Expr, Statement};
    use crate::interpreter::{Atom, Value};

    /// Build a simple definition with body and derivation for testing.
    fn make_defn(
        name: &str,
        body: Vec<Statement>,
        derivation: DerivationBlock,
    ) -> TopLevel {
        TopLevel::Definition(crate::ast::Definition {
            variadic_param: None,
            name: name.to_string(),
            type_params: vec![],
            parameters: vec![
                ("x".to_string(), crate::ast::Type::int()),
                ("y".to_string(), crate::ast::Type::int()),
            ],
            output_type: Some(crate::ast::OutputType::Single(crate::ast::Type::int())),
            outputs: vec![],
            contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body,
            metadata: std::collections::HashMap::new(),
            derivation: Some(derivation),
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })
    }

    fn dummy_span() -> crate::errors::Span { crate::errors::Span::dummy() }

    fn make_example(inputs: Vec<Expr>, output: Expr) -> DerivationExample {
        DerivationExample {
            inputs,
            output: Box::new(output),
            tolerance: None,
            span: dummy_span(),
        }
    }

    fn make_example_with_tol(inputs: Vec<Expr>, output: Expr, tol: f64) -> DerivationExample {
        DerivationExample {
            inputs,
            output: Box::new(output),
            tolerance: Some(tol),
            span: dummy_span(),
        }
    }

    #[test]
    fn test_assertion_passes() {
        // defn add(x, y) -> Int { term x + y; } := { 2, 3 -> 5; };
        let body = vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".to_string())),
            Box::new(Expr::Identifier("y".to_string())),
        )))];
        let derivation = DerivationBlock {
            examples: vec![make_example(
                vec![Expr::Decimal(2), Expr::Decimal(3)],
                Expr::Decimal(5),
            )],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        let program = vec![make_defn("add", body, derivation)];
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let result = verify_derivation_assertions(&program, &mut interp);
        assert!(result.is_ok(), "assertion should pass: {:?}", result);
    }

    #[test]
    fn test_assertion_fails() {
        // defn add(x, y) -> Int { term x - y; } := { 2, 3 -> 5; };
        let body = vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Sub,
            Box::new(Expr::Identifier("x".to_string())),
            Box::new(Expr::Identifier("y".to_string())),
        )))];
        let derivation = DerivationBlock {
            examples: vec![make_example(
                vec![Expr::Decimal(2), Expr::Decimal(3)],
                Expr::Decimal(5),
            )],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        let program = vec![make_defn("add", body, derivation)];
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let result = verify_derivation_assertions(&program, &mut interp);
        assert!(result.is_err(), "assertion should fail");
    }

    #[test]
    fn test_assertion_tolerance_passes() {
        // defn f(x: Float) -> Float { term x; } := { 3.0 -> [0.1] 3.05; };
        let body = vec![Statement::Term(Some(Expr::Identifier("x".to_string())))];
        let derivation = DerivationBlock {
            examples: vec![make_example_with_tol(
                vec![Expr::Float(3.0)],
                Expr::Float(3.05),
                0.1,
            )],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        // Use float-aware definition
        let program = vec![TopLevel::Definition(crate::ast::Definition {
            variadic_param: None,
            name: "f".to_string(),
            type_params: vec![],
            parameters: vec![("x".to_string(), crate::ast::Type::float())],
            output_type: Some(crate::ast::OutputType::Single(crate::ast::Type::float())),
            outputs: vec![],
            contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body,
            metadata: std::collections::HashMap::new(),
            derivation: Some(derivation),
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })];
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let result = verify_derivation_assertions(&program, &mut interp);
        assert!(result.is_ok(), "tolerance assertion should pass: {:?}", result);
    }

    #[test]
    fn test_assertion_tolerance_fails() {
        // defn f(x: Float) -> Float { term x; } := { 3.0 -> [0.01] 3.5; };
        let body = vec![Statement::Term(Some(Expr::Identifier("x".to_string())))];
        let derivation = DerivationBlock {
            examples: vec![make_example_with_tol(
                vec![Expr::Float(3.0)],
                Expr::Float(3.5),
                0.01,
            )],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        let program = vec![TopLevel::Definition(crate::ast::Definition {
            variadic_param: None,
            name: "f".to_string(),
            type_params: vec![],
            parameters: vec![("x".to_string(), crate::ast::Type::float())],
            output_type: Some(crate::ast::OutputType::Single(crate::ast::Type::float())),
            outputs: vec![],
            contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body,
            metadata: std::collections::HashMap::new(),
            derivation: Some(derivation),
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })];
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let result = verify_derivation_assertions(&program, &mut interp);
        assert!(result.is_err(), "tolerance assertion should fail");
    }

    #[test]
    fn test_assertion_skipped_no_body() {
        // defn add(x, y) -> Int := { 2, 3 -> 5; }; — no body (draft)
        let derivation = DerivationBlock {
            examples: vec![make_example(
                vec![Expr::Decimal(2), Expr::Decimal(3)],
                Expr::Decimal(5),
            )],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        let program = vec![TopLevel::Definition(crate::ast::Definition {
            variadic_param: None,
            name: "add".to_string(),
            type_params: vec![],
            parameters: vec![
                ("x".to_string(), crate::ast::Type::int()),
                ("y".to_string(), crate::ast::Type::int()),
            ],
            output_type: Some(crate::ast::OutputType::Single(crate::ast::Type::int())),
            outputs: vec![],
            contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body: vec![], // empty body = draft
            metadata: std::collections::HashMap::new(),
            derivation: Some(derivation),
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })];
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let result = verify_derivation_assertions(&program, &mut interp);
        assert!(result.is_ok(), "draft should be skipped: {:?}", result);
    }

    #[test]
    fn test_assertion_skipped_no_derivation() {
        // defn add(x, y) -> Int { term x + y; }; — no derivation block
        let body = vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".to_string())),
            Box::new(Expr::Identifier("y".to_string())),
        )))];
        let program = vec![TopLevel::Definition(crate::ast::Definition {
            variadic_param: None,
            name: "add".to_string(),
            type_params: vec![],
            parameters: vec![
                ("x".to_string(), crate::ast::Type::int()),
                ("y".to_string(), crate::ast::Type::int()),
            ],
            output_type: Some(crate::ast::OutputType::Single(crate::ast::Type::int())),
            outputs: vec![],
            contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None, // no derivation
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })];
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let result = verify_derivation_assertions(&program, &mut interp);
        assert!(result.is_ok(), "no derivation should be skipped: {:?}", result);
    }

    #[test]
    fn test_assertion_multi_example() {
        // defn add(x, y) -> Int { term x + y; }
        // := { 2, 3 -> 5; 0, 0 -> 0; 10, 20 -> 30; };
        let body = vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".to_string())),
            Box::new(Expr::Identifier("y".to_string())),
        )))];
        let derivation = DerivationBlock {
            examples: vec![
                make_example(vec![Expr::Decimal(2), Expr::Decimal(3)], Expr::Decimal(5)),
                make_example(vec![Expr::Decimal(0), Expr::Decimal(0)], Expr::Decimal(0)),
                make_example(vec![Expr::Decimal(10), Expr::Decimal(20)], Expr::Decimal(30)),
            ],
            synthesized: None,
            postcondition: None,
            precondition: None,
            ref_name: None,
            ref_tolerance: None,
            chain: vec![],
            span: dummy_span(),
        };
        let program = vec![make_defn("add", body, derivation)];
        let mut interp = Interpreter::new();
        interp.load_program(&program);
        let result = verify_derivation_assertions(&program, &mut interp);
        assert!(result.is_ok(), "multi example should pass: {:?}", result);
    }
}
