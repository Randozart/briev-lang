use crate::ast::{BinaryOpKind, Contract, Expr, Statement, TopLevel, Transaction, Type};
use std::collections::HashMap;

/// Parameter range analysis for transaction parameters.
///
/// Infers the possible value ranges of parameters by analyzing
/// preconditions. Uses constraints like `x > 0`, `x < 10`, `x >= 0`
/// to determine bounds.
///
/// Backends use this to:
/// - Prove termination of structural recursion
/// - Determine memory allocation sizes
/// - Generate bounded loop unrolling
/// - Optimize away runtime bounds checks
#[derive(Debug, Clone)]
pub struct ParameterRanges {
    /// For each transaction, a map from parameter name to its inferred range
    pub ranges: HashMap<String, HashMap<String, Range>>,
}

/// A numeric range with optional lower and upper bounds
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub lower: Bound,
    pub upper: Bound,
}

/// A bound value (inclusive or exclusive)
#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    Unbounded,
    Inclusive(i64),
    Exclusive(i64),
}

impl ParameterRanges {
    pub fn new() -> Self {
        ParameterRanges {
            ranges: HashMap::new(),
        }
    }

    /// Analyze a program to infer parameter ranges from preconditions
    pub fn analyze(&mut self, program: &[TopLevel]) {
        self.ranges.clear();
        for item in program {
            if let TopLevel::Transaction(txn) = item {
                let mut param_ranges = HashMap::new();

                // Initialize all params as unbounded
                for (name, _) in &txn.parameters {
                    param_ranges.insert(
                        name.clone(),
                        Range {
                            lower: Bound::Unbounded,
                            upper: Bound::Unbounded,
                        },
                    );
                }

                // Extract bounds from precondition
                self.extract_bounds_from_expr(&txn.contract.pre_condition, &mut param_ranges);

                self.ranges.insert(txn.name.clone(), param_ranges);
            }
        }
    }

    /// Recursively extract bounds from precondition expressions
    fn extract_bounds_from_expr(
        &self,
        expr: &Expr,
        ranges: &mut HashMap<String, Range>,
    ) {
        match expr {
            Expr::BinaryOp(BinaryOpKind::And, l, r) => {
                self.extract_bounds_from_expr(l, ranges);
                self.extract_bounds_from_expr(r, ranges);
            }
            Expr::BinaryOp(BinaryOpKind::Gt, l, r) => {
                self.apply_comparison(l, r, ranges, true, false);
            }
            Expr::BinaryOp(BinaryOpKind::Ge, l, r) => {
                self.apply_comparison(l, r, ranges, true, true);
            }
            Expr::BinaryOp(BinaryOpKind::Lt, l, r) => {
                self.apply_comparison(l, r, ranges, false, false);
            }
            Expr::BinaryOp(BinaryOpKind::Le, l, r) => {
                self.apply_comparison(l, r, ranges, false, true);
            }
            _ => {}
        }
    }

    /// Apply a comparison constraint to infer a bound
    ///
    /// `op_is_gt`: true if the operator is Gt/Ge (greater-than family),
    ///             false if Lt/Le (less-than family).
    /// `inclusive`: true if the operator includes equality (Ge, Le).
    fn apply_comparison(
        &self,
        left: &Expr,
        right: &Expr,
        ranges: &mut HashMap<String, Range>,
        op_is_gt: bool,
        inclusive: bool,
    ) {
        match (left, right) {
            // var <op> lit
            (Expr::Identifier(name), Expr::Decimal(val)) => {
                if let Some(range) = ranges.get_mut(name) {
                    if op_is_gt {
                        // var > lit or var >= lit → LOWER bound
                        match inclusive {
                            true => self.set_lower_bound(range, Bound::Inclusive(*val)),
                            false => self.set_lower_bound(range, Bound::Exclusive(*val)),
                        }
                    } else {
                        // var < lit or var <= lit → UPPER bound
                        match inclusive {
                            true => self.set_upper_bound(range, Bound::Inclusive(*val)),
                            false => self.set_upper_bound(range, Bound::Exclusive(*val)),
                        }
                    }
                }
            }
            // lit <op> var
            (Expr::Decimal(val), Expr::Identifier(name)) => {
                if let Some(range) = ranges.get_mut(name) {
                    if op_is_gt {
                        // lit > var → var < lit → UPPER bound
                        match inclusive {
                            true => self.set_upper_bound(range, Bound::Inclusive(*val)),
                            false => self.set_upper_bound(range, Bound::Exclusive(*val)),
                        }
                    } else {
                        // lit < var → var > lit → LOWER bound
                        match inclusive {
                            true => self.set_lower_bound(range, Bound::Inclusive(*val)),
                            false => self.set_lower_bound(range, Bound::Exclusive(*val)),
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn set_lower_bound(&self, range: &mut Range, bound: Bound) {
        range.lower = match (&range.lower, &bound) {
            (Bound::Unbounded, _) => bound,
            (Bound::Inclusive(a), Bound::Inclusive(b)) => Bound::Inclusive((*a).max(*b)),
            (Bound::Inclusive(a), Bound::Exclusive(b)) => {
                if *a > *b { range.lower.clone() } else { bound }
            }
            _ => bound,
        };
    }

    fn set_upper_bound(&self, range: &mut Range, bound: Bound) {
        range.upper = match (&range.upper, &bound) {
            (Bound::Unbounded, _) => bound,
            (Bound::Inclusive(a), Bound::Inclusive(b)) => Bound::Inclusive((*a).min(*b)),
            (Bound::Inclusive(a), Bound::Exclusive(b)) => {
                if *a < *b { range.upper.clone() } else { bound }
            }
            _ => bound,
        };
    }

    /// Check if a parameter has a known lower bound
    pub fn has_lower_bound(&self, txn_name: &str, param_name: &str) -> bool {
        self.ranges
            .get(txn_name)
            .and_then(|r| r.get(param_name))
            .map(|r| r.lower != Bound::Unbounded)
            .unwrap_or(false)
    }

    /// Check if a parameter has a known upper bound
    pub fn has_upper_bound(&self, txn_name: &str, param_name: &str) -> bool {
        self.ranges
            .get(txn_name)
            .and_then(|r| r.get(param_name))
            .map(|r| r.upper != Bound::Unbounded)
            .unwrap_or(false)
    }

    /// Get the minimum value of a parameter (if bounded)
    pub fn min_value(&self, txn_name: &str, param_name: &str) -> Option<i64> {
        self.ranges.get(txn_name).and_then(|r| r.get(param_name)).and_then(|r| match r.lower {
            Bound::Inclusive(v) => Some(v),
            Bound::Exclusive(v) => Some(v + 1),
            Bound::Unbounded => None,
        })
    }

    /// Get the maximum value of a parameter (if bounded)
    pub fn max_value(&self, txn_name: &str, param_name: &str) -> Option<i64> {
        self.ranges.get(txn_name).and_then(|r| r.get(param_name)).and_then(|r| match r.upper {
            Bound::Inclusive(v) => Some(v),
            Bound::Exclusive(v) => Some(v - 1),
            Bound::Unbounded => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_txn_with_precondition(
        name: &str,
        param_names: &[&str],
        pre: Expr,
    ) -> TopLevel {
        TopLevel::Transaction(Transaction {
            variadic_param: None,
            name: name.to_string(),
            type_params: vec![],
            parameters: param_names.iter().map(|n| (n.to_string(), Type::int())).collect(),
            contract: Contract {
                pre_condition: pre,
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![],
            is_async: false,
            is_reactive: false,
            outputs: Vec::new(),
            output_type: None,
            metadata: HashMap::new(),
            modifiers: vec![],
            derivation: None,
            span: None,
            doc: None,
        })
    }

    #[test]
    fn test_empty_program_no_ranges() {
        let program: Vec<TopLevel> = vec![];
        let mut pr = ParameterRanges::new();
        pr.analyze(&program);
        assert!(pr.ranges.is_empty());
    }

    #[test]
    fn test_single_unbounded_param() {
        let program: Vec<TopLevel> = vec![make_txn_with_precondition("f", &["x"], Expr::Bool(true))];
        let mut pr = ParameterRanges::new();
        pr.analyze(&program);
        assert!(!pr.has_lower_bound("f", "x"));
        assert!(!pr.has_upper_bound("f", "x"));
    }

    #[test]
    fn test_lower_bound_x_gt_0() {
        let program: Vec<TopLevel> = vec![make_txn_with_precondition(
            "f",
            &["x"],
            Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(0))),
        )];
        let mut pr = ParameterRanges::new();
        pr.analyze(&program);
        assert!(pr.has_lower_bound("f", "x"));
        assert_eq!(pr.min_value("f", "x"), Some(1)); // x > 0 => x >= 1
        assert!(!pr.has_upper_bound("f", "x"));
    }

    #[test]
    fn test_upper_bound_x_le_10() {
        let program: Vec<TopLevel> = vec![make_txn_with_precondition(
            "f",
            &["x"],
            Expr::BinaryOp(BinaryOpKind::Le, Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(10))),
        )];
        let mut pr = ParameterRanges::new();
        pr.analyze(&program);
        assert!(pr.has_upper_bound("f", "x"));
        assert_eq!(pr.max_value("f", "x"), Some(10));
        assert!(!pr.has_lower_bound("f", "x"));
    }

    #[test]
    fn test_both_bounds_from_and() {
        let program: Vec<TopLevel> = vec![make_txn_with_precondition(
            "f",
            &["x"],
            Expr::BinaryOp(BinaryOpKind::And,
                Box::new(Expr::BinaryOp(BinaryOpKind::Gt, Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(0)))),
                Box::new(Expr::BinaryOp(BinaryOpKind::Lt, Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(100)))),
            ),
        )];
        let mut pr = ParameterRanges::new();
        pr.analyze(&program);
        assert!(pr.has_lower_bound("f", "x"));
        assert!(pr.has_upper_bound("f", "x"));
        assert_eq!(pr.min_value("f", "x"), Some(1));
        assert_eq!(pr.max_value("f", "x"), Some(99)); // x < 100 => x <= 99
    }
}
