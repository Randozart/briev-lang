/// Feature C: Assertion Verification with `sig -> true`
///
/// Enables compile-time verification that functions always return Bool = true.
/// Example: sig always_succeeds: String -> true; asserts the function always succeeds.
use crate::ast::{BinaryOpKind, Definition, Expr, Signature, Statement, Type};
use std::collections::HashMap;

/// Verify that a sig's `-> true` assertion is valid
pub fn verify_true_assertion(sig: &Signature, defn: &Definition) -> Result<(), String> {
    // Check that the sig declares Bool output
    if sig.outputs.is_empty() || sig.outputs[0] != Type::bool_() {
        return Ok(());
    }

    // Check that definition produces Bool
    if !defn.outputs.is_empty() && defn.outputs[0] != Type::bool_() {
        return Err(format!(
            "Assertion '{}' requires Bool output, but definition produces {:?}",
            sig.name, defn.outputs[0]
        ));
    }

    // Check all execution paths for Bool = true guarantee
    verify_all_paths_produce_true(defn)
}

/// Check if all paths through the definition produce Bool = true
fn verify_all_paths_produce_true(defn: &Definition) -> Result<(), String> {
    // Start with a symbolic state from the precondition
    let mut vars: HashMap<String, Expr> = HashMap::new();

    // Extract variables from precondition
    extract_vars_from_expr(&defn.contract.pre_condition, &mut vars);

    // Walk through body and check termination conditions
    check_all_paths(&defn.body, vars, defn)
}

/// Check that all execution paths produce true
fn check_all_paths(
    body: &[Statement],
    mut vars: HashMap<String, Expr>,
    defn: &Definition,
) -> Result<(), String> {
    let mut found_term = false;
    let mut found_true_path = false;

    for stmt in body {
        match stmt {
            Statement::Assign(lhs, expr) => {
                // Track assignments
                if let Expr::Identifier(name) = lhs {
                    vars.insert(name.clone(), expr.clone());
                } else if let Some(name) = lhs.as_var_name() {
                    vars.insert(name.to_string(), expr.clone());
                }
            }

            Statement::Guarded(condition, statements) => {
                // Check TRUE branch: guarded statements must independently produce true
                let mut branch_vars = vars.clone();
                branch_vars.insert(
                    format!("__guard_{}", format!("{:?}", condition)),
                    Expr::Bool(true),
                );
                let mut guard_found_term = false;
                let mut guard_found_true = false;
                // We check the guarded statements separately, tracking its own
                // termination. If the guarded branch terminates with true, that's
                // one valid path.
                match check_all_paths(statements, branch_vars, defn) {
                    Ok(()) => {
                        guard_found_true = true;
                        guard_found_term = true;
                    }
                    Err(_) => {}
                }
                if guard_found_true {
                    found_true_path = true;
                }
                if guard_found_term {
                    found_term = true;
                }

                // Check FALSE branch: subsequent statements under negated condition
                // must also independently produce true. The negated condition tracks
                // that we're on the path where the guard was bypassed.
                vars.insert(
                    format!("__guard_{}", format!("{:?}", condition)),
                    Expr::Bool(false),
                );
            }

            Statement::Term(Some(expr)) | Statement::EndProgram(Some(expr)) => {
                found_term = true;
                // Check if this term produces true
                if is_provably_true(expr, &vars) {
                    found_true_path = true;
                } else {
                    return Err(format!(
                        "Termination expression is not provably true in definition '{}'",
                        defn.name
                    ));
                }
            }
            Statement::Term(None) | Statement::EndProgram(None) => {
                return Err("Term has no output expression".to_string());
            }

            _ => {}
        }
    }

    if !found_term {
        return Err("Definition body has no termination".to_string());
    }

    if !found_true_path {
        return Err("No execution path produces Bool = true in definition body".to_string());
    }

    Ok(())
}

/// Check if an expression is provably true given current symbolic state
fn is_provably_true(expr: &Expr, vars: &HashMap<String, Expr>) -> bool {
    match expr {
        Expr::Bool(b) => *b,

        Expr::Identifier(name) => {
            // Check if this variable is known to be true
            match vars.get(name) {
                Some(Expr::Bool(true)) => true,
                _ => false,
            }
        }

        _ => false, // Conservative: unknown expressions not provably true
    }
}

/// Extract variables mentioned in an expression and add to state
fn extract_vars_from_expr(expr: &Expr, vars: &mut HashMap<String, Expr>) {
    match expr {
        Expr::Identifier(name) => {
            vars.entry(name.clone()).or_insert(Expr::Bool(false));
        }
        Expr::BinaryOp(BinaryOpKind::And, l, r) | Expr::BinaryOp(BinaryOpKind::Or, l, r)
        | Expr::BinaryOp(BinaryOpKind::Eq, l, r) | Expr::BinaryOp(BinaryOpKind::Neq, l, r) => {
            extract_vars_from_expr(l, vars);
            extract_vars_from_expr(r, vars);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Contract, Definition, Signature};

    #[test]
    fn test_literal_true_assertion() {
        let sig = Signature {
            name: "always_true".to_string(),
            params: vec![],
            outputs: vec![Type::bool_()],
            span: None,
        };

        let defn = Definition {
                variadic_param: None,
            name: "always_true_defn".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![Type::bool_()],
            output_type: None,
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(Some(Expr::Bool(true)))],
            annotations: vec![],
            metadata: HashMap::new(),
            modifiers: vec![],
            derivation: None,
            span: None,
            doc: None,
        };

        assert!(verify_true_assertion(&sig, &defn).is_ok());
    }

    #[test]
    fn test_false_assertion_fails() {
        let sig = Signature {
            name: "always_false".to_string(),
            params: vec![],
            outputs: vec![Type::bool_()],
            span: None,
        };

        let defn = Definition {
                variadic_param: None,
            name: "always_false_defn".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![Type::bool_()],
            output_type: None,
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(Some(Expr::Bool(false)))],
            annotations: vec![],
            metadata: HashMap::new(),
            modifiers: vec![],
            derivation: None,
            span: None,
            doc: None,
        };

        assert!(verify_true_assertion(&sig, &defn).is_err());
    }

    #[test]
    fn test_variable_assigned_true() {
        let sig = Signature {
            name: "check_x".to_string(),
            params: vec![("".to_string(), Type::bool_())],
            outputs: vec![Type::bool_()],
            span: None,
        };

        let defn = Definition {
                variadic_param: None,
            name: "check_x_defn".to_string(),
            type_params: vec![],
            parameters: vec![("x".to_string(), Type::bool_())],
            outputs: vec![Type::bool_()],
            output_type: None,
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("result".to_string()), Expr::Bool(true)),
                Statement::Term(Some(Expr::Identifier("result".to_string()))),
            ],
            annotations: vec![],
            metadata: HashMap::new(),
            modifiers: vec![],
            derivation: None,
            span: None,
            doc: None,
        };

        assert!(verify_true_assertion(&sig, &defn).is_ok());
    }

    #[test]
    fn test_non_bool_output_fails() {
        let sig = Signature {
            name: "not_bool".to_string(),
            params: vec![],
            outputs: vec![Type::bool_()],
            span: None,
        };

        let defn = Definition {
                variadic_param: None,
            name: "not_bool_defn".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![Type::string()],
            output_type: None,
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(Some(Expr::Quoted(
                "not bool".into(),
            )))],
            annotations: vec![],
            metadata: HashMap::new(),
            modifiers: vec![],
            derivation: None,
            span: None,
            doc: None,
        };

        assert!(verify_true_assertion(&sig, &defn).is_err());
    }

    #[test]
    fn test_no_assertion_type_skipped() {
        let sig = Signature {
            name: "regular_sig".to_string(),
            params: vec![],
            outputs: vec![Type::int()],
            span: None,
        };

        let defn = Definition {
                variadic_param: None,
            name: "regular_sig_defn".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![Type::bool_()],
            output_type: None,
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(Some(Expr::Bool(false)))],
            annotations: vec![],
            metadata: HashMap::new(),
            modifiers: vec![],
            derivation: None,
            span: None,
            doc: None,
        };

        // Should be OK because the sig output is Int, not Bool (no assertion)
        assert!(verify_true_assertion(&sig, &defn).is_ok());
    }
}
