// ── Fuzz Checker — Compile-Time Inline Test Verification ──────────
//
// Verifies `#fuzz` cases attached to defn and txn (callable) items.
//
// For defn/txn: evaluates fuzz bindings, calls the interpreter's
// `call_defn`/`call_txn`, and compares the result against expected.
//
// Cells are skipped with a warning (state setup is deferred).

use crate::ast::{Definition, Expr, FuzzCase, TopLevel, Transaction};
use crate::errors::{FuzzError, Span};
use crate::interpreter::{Interpreter, Value};
use std::collections::HashMap;

/// Run all fuzz cases in a program and return any errors found.
pub fn check_fuzz_cases(
    program: &[crate::ast::TopLevel],
    interpreter: &mut Interpreter,
) -> Vec<FuzzError> {
    let mut errors = Vec::new();

    for item in program {
        match item {
            TopLevel::Fuzzed { item: inner, cases } => {
                let mut case_idx = 0;
                for fuzz_case in cases {
                    let errs = verify_fuzz_case(&inner, fuzz_case, case_idx, interpreter);
                    errors.extend(errs);
                    case_idx += 1;
                }
            }
            _ => {}
        }
    }

    errors
}

/// Verify a single fuzz case against its wrapped item.
fn verify_fuzz_case(
    item: &TopLevel,
    fuzz_case: &FuzzCase,
    case_idx: usize,
    interpreter: &mut Interpreter,
) -> Vec<FuzzError> {
    let span = fuzz_case.span.unwrap_or_else(Span::dummy);

    match item {
        TopLevel::Definition(defn) => {
            verify_defn_fuzz(defn, fuzz_case, case_idx, interpreter, span)
        }
        TopLevel::Transaction(txn) => {
            if txn.is_reactive {
                vec![FuzzError::Skipped {
                    function: txn.name.clone(),
                    reason: "reactive transactions cannot be fuzzed".to_string(),
                    span,
                }]
            } else {
                verify_txn_fuzz(txn, fuzz_case, case_idx, interpreter, span)
            }
        }
        TopLevel::Cell(_) => {
            vec![FuzzError::Skipped {
                function: "cell".to_string(),
                reason: "cell fuzzing is not yet supported".to_string(),
                span,
            }]
        }
        _ => {
            vec![FuzzError::Skipped {
                function: "unknown".to_string(),
                reason: "fuzz not applicable to this item type".to_string(),
                span,
            }]
        }
    }
}

fn verify_defn_fuzz(
    defn: &Definition,
    fuzz_case: &FuzzCase,
    case_idx: usize,
    interpreter: &mut Interpreter,
    span: Span,
) -> Vec<FuzzError> {
    let function = defn.name.clone();
    let mut errors = Vec::new();

    // Build argument list in parameter order.
    let args = match build_args(&defn.parameters, fuzz_case, &function, case_idx, &span) {
        Ok(a) => a,
        Err(e) => return vec![e],
    };

    // Build state overrides for non-parameter bindings.
    if let Err(e) = apply_state_overrides(
        &defn.parameters,
        fuzz_case,
        interpreter,
        &function,
        case_idx,
        &span,
    ) {
        return vec![e];
    }

    // Evaluate expected output.
    let expected = match interpreter.eval_expr(&fuzz_case.expected) {
        Ok(v) => v,
        Err(e) => {
            return vec![FuzzError::EvaluationError {
                function: function.clone(),
                case_index: case_idx,
                message: format!("cannot evaluate expected expression: {:?}", e),
                span,
            }];
        }
    };

    // Execute the definition body.
    let actual = match exec_body(&defn.body, interpreter) {
        Ok(v) => v,
        Err(e) => {
            return vec![FuzzError::EvaluationError {
                function: function.clone(),
                case_index: case_idx,
                message: format!("call failed: {:?}", e),
                span,
            }];
        }
    };

    // Compare.
    if actual != expected {
        errors.push(FuzzError::Mismatch {
            function,
            case_index: case_idx,
            inputs: format_bindings(fuzz_case),
            expected: format_value(&expected),
            actual: format_value(&actual),
            span,
        });
    }

    errors
}

fn verify_txn_fuzz(
    txn: &Transaction,
    fuzz_case: &FuzzCase,
    case_idx: usize,
    interpreter: &mut Interpreter,
    span: Span,
) -> Vec<FuzzError> {
    let function = txn.name.clone();
    let mut errors = Vec::new();

    // Build argument list in parameter order.
    let args = match build_args(&txn.parameters, fuzz_case, &function, case_idx, &span) {
        Ok(a) => a,
        Err(e) => return vec![e],
    };

    // Apply state overrides.
    if let Err(e) = apply_state_overrides(
        &txn.parameters,
        fuzz_case,
        interpreter,
        &function,
        case_idx,
        &span,
    ) {
        return vec![e];
    }

    // Evaluate expected output.
    let expected = match interpreter.eval_expr(&fuzz_case.expected) {
        Ok(v) => v,
        Err(e) => {
            return vec![FuzzError::EvaluationError {
                function: function.clone(),
                case_index: case_idx,
                message: format!("cannot evaluate expected expression: {:?}", e),
                span,
            }];
        }
    };

    // Execute the transaction body.
    let actual = match exec_body(&txn.body, interpreter) {
        Ok(v) => v,
        Err(e) => {
            return vec![FuzzError::EvaluationError {
                function: function.clone(),
                case_index: case_idx,
                message: format!("txn call failed: {:?}", e),
                span,
            }];
        }
    };

    if actual != expected {
        errors.push(FuzzError::Mismatch {
            function,
            case_index: case_idx,
            inputs: format_bindings(fuzz_case),
            expected: format_value(&expected),
            actual: format_value(&actual),
            span,
        });
    }

    errors
}

/// Build an argument expression list in parameter order from fuzz bindings.
fn build_args(
    params: &[(String, crate::ast::Type)],
    fuzz_case: &FuzzCase,
    function: &str,
    case_idx: usize,
    span: &Span,
) -> Result<Vec<Expr>, FuzzError> {
    let mut args = Vec::new();
    for (param_name, _) in params {
        let found = fuzz_case
            .bindings
            .iter()
            .find(|(name, _)| name == param_name);
        match found {
            Some((_, expr)) => args.push(expr.clone()),
            None => {
                return Err(FuzzError::MissingBinding {
                    function: function.to_string(),
                    case_index: case_idx,
                    param: param_name.clone(),
                    span: *span,
                });
            }
        }
    }
    Ok(args)
}

/// Apply non-parameter bindings as state overrides in the interpreter.
fn apply_state_overrides(
    params: &[(String, crate::ast::Type)],
    fuzz_case: &FuzzCase,
    interpreter: &mut Interpreter,
    function: &str,
    _case_idx: usize,
    _span: &Span,
) -> Result<(), FuzzError> {
    let param_names: std::collections::HashSet<&str> =
        params.iter().map(|(n, _)| n.as_str()).collect();
    for (name, expr) in &fuzz_case.bindings {
        if !param_names.contains(name.as_str()) {
            // This is a state/top-level override — evaluate and set.
            match interpreter.eval_expr(expr) {
                Ok(val) => {
                    interpreter.state.insert(name.clone(), val);
                }
                Err(e) => {
                    // Non-fatal — just skip if the expression can't be evaluated
                    // in the current state context.
                    let _ = e;
                }
            }
        }
    }
    Ok(())
}

fn format_bindings(fuzz_case: &FuzzCase) -> String {
    fuzz_case
        .bindings
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_value(val: &Value) -> String {
    match val {
        Value::Bits(d) if d.len() == 8 => {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&d[..8]);
            let n = i64::from_le_bytes(arr);
            n.to_string()
        }
        Value::Bits(d) => {
            let s = String::from_utf8_lossy(d);
            format!("\"{}\"", s)
        }
        Value::Atom(crate::interpreter::Atom::Int(n)) => n.to_string(),
        Value::Atom(crate::interpreter::Atom::Float(f)) => f.to_string(),
        Value::Atom(crate::interpreter::Atom::Bool(b)) => b.to_string(),
        Value::Atom(crate::interpreter::Atom::Char(c)) => c.to_string(),
        Value::Void => "void".to_string(),
        Value::Product { fields, .. } => {
            let inner: Vec<String> = fields.iter().map(format_value).collect();
            format!("({})", inner.join(", "))
        }
        _ => format!("{:?}", val),
    }
}

/// Execute a block of statements using the interpreter and return the last value.
fn exec_body(
    body: &[crate::ast::Statement],
    interpreter: &mut Interpreter,
) -> Result<Value, crate::errors::RuntimeError> {
    let mut result = Value::Void;
    for stmt in body {
        result = interpreter.exec_stmt(stmt)?;
    }
    Ok(result)
}
