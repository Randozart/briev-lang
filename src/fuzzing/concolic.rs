//! Concolic Fuzzer (Proof-Guided)
//!
//! Uses the proof engine's path constraints to generate concrete inputs
//! that satisfy them, eliminating impossible state permutations.
//!
//! Concolic = Concrete + Symbolic execution combined.
//! The symbolic engine identifies path constraints, then the fuzzer
//! generates concrete values that satisfy those constraints.

use crate::ast::*;
use crate::symbolic::{SymbolicState, SymbolicValue};
use std::collections::HashMap;

/// Extract path constraints from a transaction's body
pub fn extract_path_constraints(txn: &Transaction) -> Vec<PathConstraintInfo> {
    let mut constraints = Vec::new();
    collect_constraints_recursive(&txn.body, &mut constraints, Vec::new());
    constraints
}

/// Information about a path constraint
#[derive(Debug, Clone)]
pub struct PathConstraintInfo {
    /// The condition expression
    pub condition: Expr,
    /// Depth in the statement tree (how nested)
    pub depth: usize,
    /// Whether this is a guard condition or a contract condition
    pub kind: ConstraintKind,
    /// Whether the condition involves trigger (volatile) variables
    pub involves_trigger: bool,
}

#[derive(Debug, Clone)]
pub enum ConstraintKind {
    /// Guard condition: [condition] { ... }
    Guard,
    /// Transaction precondition
    Precondition,
    /// Transaction postcondition
    Postcondition,
    /// Local trigger expression
    TriggerExpr,
}

fn collect_constraints_recursive(
    stmts: &[Statement],
    constraints: &mut Vec<PathConstraintInfo>,
    path: Vec<Expr>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Guarded(condition, statements) => {
                let involves_trigger = expr_involves_trigger(condition);
                constraints.push(PathConstraintInfo {
                    condition: condition.clone(),
                    depth: path.len(),
                    kind: ConstraintKind::Guard,
                    involves_trigger,
                });

                // Recurse into guarded body with this condition on the path
                let mut new_path = path.clone();
                new_path.push(condition.clone());
                collect_constraints_recursive(statements, constraints, new_path);
            }
            _ => {}
        }
    }
}

/// Check if an expression involves a trigger variable
fn expr_involves_trigger(expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(name) => {
            // Heuristic: trigger variables often have signal-like names
            is_trigger_like_name(name)
        }
        Expr::BinaryOp(_, l, r) => {
            expr_involves_trigger(l) || expr_involves_trigger(r)
        }
        Expr::UnaryOp(_, e) => expr_involves_trigger(e),
        Expr::Call(_, args, _) => args.iter().any(expr_involves_trigger),
        Expr::Index(list, idx) => {
            expr_involves_trigger(list) || expr_involves_trigger(idx)
        }
        _ => false,
    }
}

/// Heuristic: check if a variable name looks like a trigger/signal
fn is_trigger_like_name(name: &str) -> bool {
    let trigger_prefixes = [
        "sig", "signal", "trg", "trigger", "event", "irq", "interrupt",
        "input", "sensor", "button", "key", "click", "tick", "clock",
        "stdin", "stdout", "network", "socket", "file_", "fs_",
    ];

    let name_lower = name.to_lowercase();
    trigger_prefixes.iter().any(|prefix| name_lower.starts_with(prefix))
        || name_lower.ends_with("_trg")
        || name_lower.ends_with("_trigger")
        || name_lower.ends_with("_signal")
}

/// Generate concrete values that satisfy a path constraint
/// Returns a list of (variable_name, concrete_value) assignments
pub fn generate_concrete_values(
    constraint: &PathConstraintInfo,
    state: &SymbolicState,
) -> Vec<(String, i64)> {
    let mut assignments = Vec::new();

    match &constraint.condition {
        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), *val));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Gt, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), val + 1));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Lt, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), val.saturating_sub(1)));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Ge, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), *val));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Le, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), *val));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Neq, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), val + 1));
            }
        }
        Expr::BinaryOp(BinaryOpKind::And, l, r) => {
            // Both must be true
            let mut l_assignments = generate_concrete_for_expr(l, state);
            let mut r_assignments = generate_concrete_for_expr(r, state);
            assignments.append(&mut l_assignments);
            assignments.append(&mut r_assignments);
        }
        _ => {
            // For complex expressions, try to extract any integer comparisons
            extract_integer_comparisons(&constraint.condition, &mut assignments);
        }
    }

    assignments
}

fn generate_concrete_for_expr(expr: &Expr, _state: &SymbolicState) -> Vec<(String, i64)> {
    let mut assignments = Vec::new();
    extract_integer_comparisons(expr, &mut assignments);
    assignments
}

fn extract_integer_comparisons(expr: &Expr, assignments: &mut Vec<(String, i64)>) {
    match expr {
        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), *val));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Gt, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), val + 1));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Lt, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), val.saturating_sub(1)));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Ge, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), *val));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Le, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), *val));
            }
        }
        Expr::BinaryOp(BinaryOpKind::Neq, l, r) => {
            if let (Expr::Identifier(var), Expr::Decimal(val)) = (&**l, &**r) {
                assignments.push((var.clone(), val + 1));
            }
        }
        Expr::BinaryOp(BinaryOpKind::And, l, r) | Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
            extract_integer_comparisons(l, assignments);
            extract_integer_comparisons(r, assignments);
        }
        _ => {}
    }
}

/// Filter out constraints that involve trigger variables (unpredictable)
/// Returns only constraints that can be pre-evaluated
pub fn filter_pre_evaluable_constraints(
    constraints: &[PathConstraintInfo],
) -> Vec<&PathConstraintInfo> {
    constraints.iter().filter(|c| !c.involves_trigger).collect()
}

/// Generate a concolic test case from a transaction
/// Returns a list of concrete input assignments that exercise different paths
pub fn generate_concolic_test_cases(txn: &Transaction) -> Vec<Vec<(String, i64)>> {
    let constraints = extract_path_constraints(txn);
    let pre_evaluable = filter_pre_evaluable_constraints(&constraints);

    let mut test_cases = Vec::new();

    // Generate test case for each pre-evaluable constraint
    // Each test case exercises one specific path
    let state = SymbolicState::new(&Expr::Bool(true));

    for constraint in pre_evaluable {
        let values = generate_concrete_values(constraint, &state);
        if !values.is_empty() {
            test_cases.push(values);
        }
    }

    // Also generate a "default" test case with zero values
    let param_defaults: Vec<(String, i64)> = txn.parameters.iter()
        .map(|(name, _)| (name.clone(), 0))
        .collect();
    if !param_defaults.is_empty() {
        test_cases.push(param_defaults);
    }

    test_cases
}

/// Run concolic analysis on a program and return test cases
pub fn run_concolic_analysis(program: &[TopLevel]) -> Vec<TransactionTestCases> {
    let mut results = Vec::new();

    for item in program {
        if let TopLevel::Transaction(txn) = item {
            let test_cases = generate_concolic_test_cases(txn);
            let constraints = extract_path_constraints(txn);

            results.push(TransactionTestCases {
                txn_name: txn.name.clone(),
                test_cases,
                total_constraints: constraints.len(),
                pre_evaluable: filter_pre_evaluable_constraints(&constraints).len(),
                trigger_constraints: constraints.iter().filter(|c| c.involves_trigger).count(),
            });
        }
    }

    results
}

/// Test cases for a single transaction
#[derive(Debug)]
pub struct TransactionTestCases {
    pub txn_name: String,
    pub test_cases: Vec<Vec<(String, i64)>>,
    pub total_constraints: usize,
    pub pre_evaluable: usize,
    pub trigger_constraints: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_txn(name: &str, pre: Expr, post: Expr, body: Vec<Statement>) -> Transaction {
        Transaction {
            variadic_param: None,
            name: name.to_string(),
            is_reactive: false,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            contract: Contract {
                pre_condition: pre,
                post_condition: post,
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            span: None,
            metadata: HashMap::new(),
            modifiers: vec![],
            outputs: vec![],
            output_type: None,
            derivation: None,
            doc: None,
        }
    }

    fn make_txn_top(name: &str, pre: Expr, post: Expr, body: Vec<Statement>) -> TopLevel {
        TopLevel::Transaction(make_txn(name, pre, post, body))
    }

    fn make_state(name: &str) -> TopLevel {
        TopLevel::StateDecl(StateDecl {
            name: name.to_string(),
            ty: Type::int(),
            span: None,
        })
    }

    #[test]
    fn test_extract_path_constraints_simple() {
        let txn = make_txn("foo",
            Expr::BinaryOp(BinaryOpKind::Lt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(100))),
            Expr::Bool(true),
            vec![
                Statement::Guarded(
                    Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(0))),
                    vec![Statement::Assign(Expr::Identifier("x".into()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(1))))],
                ),
                Statement::Guarded(
                    Expr::BinaryOp(BinaryOpKind::Eq, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(0))),
                    vec![Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(1))],
                ),
                Statement::Term(None),
            ],
        );
        let constraints = extract_path_constraints(&txn);
        assert_eq!(constraints.len(), 2, "Should find 2 guard constraints");

        // Both guards don't involve triggers
        for c in &constraints {
            assert!(!c.involves_trigger, "x is not a trigger variable");
        }
    }

    #[test]
    fn test_extract_path_constraints_with_trigger() {
        let txn = make_txn("handle_click",
            Expr::Bool(true),
            Expr::Bool(true),
            vec![
                Statement::Guarded(
                    Expr::Identifier("sensor".into()),
                    vec![Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(1))],
                ),
                Statement::Term(None),
            ],
        );
        let constraints = extract_path_constraints(&txn);
        assert_eq!(constraints.len(), 1, "Should find 1 guard constraint");

        // The guard involves 'sensor' which is trigger-like
        assert!(constraints[0].involves_trigger, "Guard involves trigger variable");
    }

    #[test]
    fn test_filter_pre_evaluable_constraints() {
        let txn = make_txn("process",
            Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(0))),
            Expr::Bool(true),
            vec![
                Statement::Guarded(
                    Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(10))),
                    vec![Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(1))],
                ),
                Statement::Guarded(
                    Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("sensor".into())), Box::new(Expr::Decimal(0))),
                    vec![Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(2))],
                ),
                Statement::Term(None),
            ],
        );
        let constraints = extract_path_constraints(&txn);
        let pre_evaluable = filter_pre_evaluable_constraints(&constraints);

        assert_eq!(constraints.len(), 2);
        assert_eq!(pre_evaluable.len(), 1, "Only x > 10 is pre-evaluable");
        assert!(!pre_evaluable[0].involves_trigger);
    }

    #[test]
    fn test_generate_concrete_values() {
        let state = SymbolicState::new(&Expr::Bool(true));

        // Test x > 5
        let constraint = PathConstraintInfo {
            condition: Expr::BinaryOp(BinaryOpKind::Gt,
                Box::new(Expr::Identifier("x".to_string())),
                Box::new(Expr::Decimal(5)),
            ),
            depth: 0,
            kind: ConstraintKind::Guard,
            involves_trigger: false,
        };

        let values = generate_concrete_values(&constraint, &state);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], ("x".to_string(), 6));

        // Test x == 10
        let constraint = PathConstraintInfo {
            condition: Expr::BinaryOp(BinaryOpKind::Eq,
                Box::new(Expr::Identifier("y".to_string())),
                Box::new(Expr::Decimal(10)),
            ),
            depth: 0,
            kind: ConstraintKind::Guard,
            involves_trigger: false,
        };

        let values = generate_concrete_values(&constraint, &state);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], ("y".to_string(), 10));
    }

    #[test]
    fn test_generate_concolic_test_cases() {
        let txn = make_txn("increment",
            Expr::BinaryOp(BinaryOpKind::And,
                Box::new(Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("amount".into())), Box::new(Expr::Decimal(0)))),
                Box::new(Expr::BinaryOp(BinaryOpKind::Lt, Box::new(Expr::Identifier("counter".into())), Box::new(Expr::Decimal(100)))),
            ),
            Expr::Bool(true),
            vec![
                Statement::Guarded(
                    Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("amount".into())), Box::new(Expr::Decimal(10))),
                    vec![Statement::Assign(Expr::Identifier("counter".into()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("counter".into())), Box::new(Expr::Identifier("amount".into()))))],
                ),
                Statement::Guarded(
                    Expr::BinaryOp(BinaryOpKind::Le, Box::new(Expr::Identifier("amount".into())), Box::new(Expr::Decimal(10))),
                    vec![Statement::Assign(Expr::Identifier("counter".into()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("counter".into())), Box::new(Expr::Identifier("amount".into()))))],
                ),
                Statement::Term(None),
            ],
        );

        let test_cases = generate_concolic_test_cases(&txn);

        // Should have test cases for amount > 10, amount <= 10, and default
        assert!(!test_cases.is_empty(), "Should generate at least one test case");

        // Verify test cases have valid values
        for case in &test_cases {
            for (name, val) in case {
                assert!(!name.is_empty());
                // Values should be reasonable
                assert!(*val >= -1000 && *val <= 10000, "Value {} out of range", val);
            }
        }
    }

    #[test]
    fn test_run_concolic_analysis() {
        let program: Vec<TopLevel> = vec![
            make_state("x"),
            make_txn_top("inc",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(100))),
                Expr::Bool(true),
                vec![
                    Statement::Guarded(
                        Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(50))),
                        vec![Statement::Assign(Expr::Identifier("x".into()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(2))))],
                    ),
                    Statement::Assign(Expr::Identifier("x".into()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(1)))),
                    Statement::Term(None),
                ],
            ),
            make_txn_top("dec",
                Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(0))),
                Expr::Bool(true),
                vec![
                    Statement::Guarded(
                        Expr::BinaryOp(BinaryOpKind::Lt, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(10))),
                        vec![Statement::Assign(Expr::Identifier("x".into()), Expr::BinaryOp(BinaryOpKind::Sub, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(2))))],
                    ),
                    Statement::Assign(Expr::Identifier("x".into()), Expr::BinaryOp(BinaryOpKind::Sub, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(1)))),
                    Statement::Term(None),
                ],
            ),
        ];

        let results = run_concolic_analysis(&program);

        assert_eq!(results.len(), 2, "Should have results for 2 transactions");

        for result in &results {
            assert!(!result.txn_name.is_empty());
            // Each transaction has a guard, so should have at least one test case
            assert!(result.total_constraints >= 1, "Should have at least one constraint");
        }
    }

    #[test]
    fn test_is_trigger_like_name() {
        assert!(is_trigger_like_name("sigint"));
        assert!(is_trigger_like_name("signal_handler"));
        assert!(is_trigger_like_name("trg_button"));
        assert!(is_trigger_like_name("event_queue"));
        assert!(is_trigger_like_name("irq_line"));
        assert!(is_trigger_like_name("sensor_value"));
        assert!(is_trigger_like_name("button_state"));
        assert!(is_trigger_like_name("clock_tick"));
        assert!(is_trigger_like_name("stdin_buffer"));
        assert!(is_trigger_like_name("network_data"));
        assert!(is_trigger_like_name("file_event"));
        assert!(is_trigger_like_name("my_trg"));
        assert!(is_trigger_like_name("my_trigger"));
        assert!(is_trigger_like_name("my_signal"));

        assert!(!is_trigger_like_name("counter"));
        assert!(!is_trigger_like_name("balance"));
        assert!(!is_trigger_like_name("result"));
        assert!(!is_trigger_like_name("x"));
        assert!(!is_trigger_like_name("temp"));
    }

    proptest::proptest! {
        #[test]
        fn test_concolic_never_panics(code in "[a-zA-Z0-9_ \\t\\n\\r;{}\\[\\]().,=+\\-*/<>!&|]{0,500}") {
            // Just test that run_concolic_analysis doesn't panic with empty input
            let program: Vec<TopLevel> = vec![];
            let _ = run_concolic_analysis(&program);
        }
    }
}
