// ── Cell / Thread Logic ────────────────────────────────────────────────
// 2026-07-12: Phase 3.3 — Cell execution and VirtualHeap.
// No Intrinsic references — all intrinsic dispatch goes through
// execute_intrinsic() in intrinsics.rs.

use crate::ast::{Expr, Statement, TopLevel, Transaction};
use crate::errors::RuntimeError;
use crate::interpreter::{Value, VirtualHeap, eval_expr, eval_statement};
use std::collections::HashMap;

/// Evaluate a cell transaction body.
pub fn eval_cell_txn(
    txn: &Transaction,
    state: &HashMap<String, Value>,
    heap: &mut VirtualHeap,
) -> Result<Value, RuntimeError> {
    let mut bindings = state.clone();
    let mut result = Value::Void;
    for stmt in &txn.body {
        result = eval_statement(stmt, heap, &mut bindings, &HashMap::new())?;
    }
    Ok(result)
}
