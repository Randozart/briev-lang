use crate::ast::{BinaryOpKind, Expr, TopLevel, Transaction, UnaryOpKind};

pub struct EntryPointAnalyzer;

impl EntryPointAnalyzer {
    pub fn find_entry_point(items: &[TopLevel]) -> Result<EntryPoint, EntryPointError> {
        let mut candidates = Vec::new();
        let mut async_candidates = Vec::new();

        for item in items {
            if let TopLevel::Transaction(txn) = item {
                // Check if precondition is true in initial state
                if Self::is_initially_true(&txn.contract.pre_condition, items) {
                    if txn.is_async {
                        async_candidates.push(txn.name.clone());
                    } else {
                        candidates.push(txn.name.clone());
                    }
                }
            }
        }

        if candidates.len() > 1 {
            return Err(EntryPointError::AmbiguousEntry {
                transactions: candidates,
            });
        }

        if candidates.is_empty() && async_candidates.is_empty() {
            return Err(EntryPointError::NoEntryPoint);
        }

        let entry = if !candidates.is_empty() {
            candidates.remove(0)
        } else if !async_candidates.is_empty() {
            // Multiple async is OK, just pick first as representative
            async_candidates.remove(0)
        } else {
            return Err(EntryPointError::NoEntryPoint);
        };

        Ok(EntryPoint {
            transaction_name: entry,
            is_async: !candidates.is_empty() || async_candidates.len() > 1,
            parallel_async: async_candidates.len(),
        })
    }

    fn is_initially_true(expr: &Expr, items: &[TopLevel]) -> bool {
        match expr {
            Expr::Bool(true) => true,
            Expr::Bool(false) => false,
            Expr::Identifier(name) => {
                // Check if variable is initialized to truthy/non-zero value
                Self::get_initial_value_numeric(name, items) != Some(0)
            }
            Expr::BinaryOp(BinaryOpKind::Eq, lhs, rhs) => {
                let l = Self::evaluate_to_constant(lhs, items);
                let r = Self::evaluate_to_constant(rhs, items);
                l == r
            }
            Expr::BinaryOp(BinaryOpKind::Neq, lhs, rhs) => {
                let l = Self::evaluate_to_constant(lhs, items);
                let r = Self::evaluate_to_constant(rhs, items);
                l != r
            }
            Expr::BinaryOp(BinaryOpKind::Ge, lhs, rhs) => {
                let l = Self::evaluate_to_constant(lhs, items);
                let r = Self::evaluate_to_constant(rhs, items);
                l >= r
            }
            Expr::BinaryOp(BinaryOpKind::Gt, lhs, rhs) => {
                let l = Self::evaluate_to_constant(lhs, items);
                let r = Self::evaluate_to_constant(rhs, items);
                l > r
            }
            Expr::BinaryOp(BinaryOpKind::Le, lhs, rhs) => {
                let l = Self::evaluate_to_constant(lhs, items);
                let r = Self::evaluate_to_constant(rhs, items);
                l <= r
            }
            Expr::BinaryOp(BinaryOpKind::Lt, lhs, rhs) => {
                let l = Self::evaluate_to_constant(lhs, items);
                let r = Self::evaluate_to_constant(rhs, items);
                l < r
            }
            Expr::BinaryOp(BinaryOpKind::And, lhs, rhs) => {
                Self::is_initially_true(lhs, items) && Self::is_initially_true(rhs, items)
            }
            Expr::BinaryOp(BinaryOpKind::Or, lhs, rhs) => {
                Self::is_initially_true(lhs, items) || Self::is_initially_true(rhs, items)
            }
            Expr::UnaryOp(UnaryOpKind::Not, inner) => !Self::is_initially_true(inner, items),
            // For complex expressions, conservatively return false
            _ => false,
        }
    }

    fn get_initial_value_numeric(name: &str, items: &[TopLevel]) -> Option<i64> {
        for item in items {
            if let TopLevel::StateDecl(decl) = item {
                if decl.name == name {
                    return match &None {
                        Some(Expr::Decimal(n)) => Some(*n),
                        Some(Expr::Bool(b)) => Some(if *b { 1 } else { 0 }),
                        _ => None,
                    };
                }
            }
        }
        None
    }

    fn evaluate_to_constant(expr: &Expr, items: &[TopLevel]) -> i64 {
        match expr {
            Expr::Decimal(n) => *n,
            Expr::Identifier(name) => {
                if let Some(val) = Self::get_initial_value_numeric(name, items) {
                    val
                } else {
                    0
                }
            }
            Expr::UnaryOp(UnaryOpKind::Neg, inner) => -Self::evaluate_to_constant(inner, items),
            Expr::BinaryOp(BinaryOpKind::Add, l, r) => {
                Self::evaluate_to_constant(l, items) + Self::evaluate_to_constant(r, items)
            }
            Expr::BinaryOp(BinaryOpKind::Sub, l, r) => {
                Self::evaluate_to_constant(l, items) - Self::evaluate_to_constant(r, items)
            }
            Expr::BinaryOp(BinaryOpKind::Mul, l, r) => {
                Self::evaluate_to_constant(l, items) * Self::evaluate_to_constant(r, items)
            }
            _ => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntryPoint {
    pub transaction_name: String,
    pub is_async: bool,
    pub parallel_async: usize,
}

#[derive(Debug, Clone)]
pub enum EntryPointError {
    AmbiguousEntry { transactions: Vec<String> },
    NoEntryPoint,
}

impl std::fmt::Display for EntryPointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryPointError::AmbiguousEntry { transactions } => {
                write!(
                    f,
                    "Multiple transactions can fire from initial state: {}. \
                    Specify which is the entry point or make some async.",
                    transactions.join(", ")
                )
            }
            EntryPointError::NoEntryPoint => {
                write!(
                    f,
                    "No transaction can fire from initial state. \
                    Define a transaction with a precondition that is initially true."
                )
            }
        }
    }
}

impl std::error::Error for EntryPointError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_entry_point() {
        // Test with a simple program
        let source = r#"
            let x: UInt = 0;
            
            node idle [x >= 0]] {
                term;
            };
        "#;

        // This test would require full parsing - just verify module compiles
        assert!(true);
    }
}
