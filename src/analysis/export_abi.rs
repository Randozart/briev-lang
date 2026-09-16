// ── Export ABI Analysis ────────────────────────────────────────────────
// 2026-08-03: Computes, per exported defn, whether its C-ABI signature
// carries the leading `ptr %state` parameter (body-dependent, non-fragile
// ABI — plan 2026-08-03-host-callable-glue-export).
//
// The result is the single source of truth for both the LLVM backend
// (which emits the signature) and GLUE export wrapper generation (which
// renders the per-function state argument). Wrappers/bindings derive their
// ABI from the per-export metadata, never by re-analyzing the AST.
//
// Rule: a defn needs state if its body (transitively through called Briev
// defns) uses the runtime state. This fixes the prior non-transitive check
// in the LLVM backend, which emitted `call @f(ptr %state, ...)` from a
// "pure" export that had no `%state` parameter → undefined-value IR.
//
// Undo: if the state parameter is ever removed from all defn signatures
// (uniform stateless ABI), delete this module and always emit no state.

use crate::ast::{Definition, Expr, Statement, TopLevel};
use std::collections::{HashMap, HashSet};

/// Compute `needs_state` for every exported defn in a program.
///
/// Indexes regular defns, transactions, and exports, then runs a memoized
/// DFS over the call graph. Regular defns and transactions are ALWAYS
/// emitted with a `%state` parameter today, so any call to one forces the
/// caller to carry state too.
pub fn compute_export_needs_state(items: &[TopLevel]) -> HashMap<String, bool> {
    let mut regular: HashMap<String, &Definition> = HashMap::new();
    let mut txns: HashSet<String> = HashSet::new();
    let mut exports: HashMap<String, &Definition> = HashMap::new();
    // 2026-08-03 (node bridge): bare reads/writes of a state field
    // (`term saved;`, `saved = name;`) need the `%state` handle even though no
    // intrinsic/call is involved. Collect the top-level state field names so
    // identifier access to them is detected (a pure local/param is not).
    let mut state_fields: HashSet<String> = HashSet::new();
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                regular.insert(d.name.clone(), d);
            }
            // 2026-08-03: transactions always carry `ptr %state` (the reactor
            // threads state through them) — a caller must supply it.
            TopLevel::Transaction(t) => {
                txns.insert(t.name.clone());
            }
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    exports.insert(d.name.clone(), d);
                }
            }
            TopLevel::Statement(stmt) => {
                if let crate::ast::Statement::Let { name, .. } = stmt.as_ref() {
                    state_fields.insert(name.clone());
                }
            }
            TopLevel::Constant(c) => {
                state_fields.insert(c.name.clone());
            }
            TopLevel::StateDecl(s) => {
                state_fields.insert(s.name.clone());
            }
            _ => {}
        }
    }

    let mut memo: HashMap<String, bool> = HashMap::new();
    for name in exports.keys() {
        defn_needs_state(name, &regular, &txns, &exports, &state_fields, &mut memo, &mut Vec::new());
    }
    memo
}

/// Memoized transitive needs-state for a defn name.
/// `visiting` breaks call cycles conservatively (in-progress ⇒ true).
fn defn_needs_state(
    name: &str,
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    exports: &HashMap<String, &Definition>,
    state_fields: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut Vec<String>,
) -> bool {
    if let Some(v) = memo.get(name) {
        return *v;
    }
    if visiting.iter().any(|n| n == name) {
        return true;
    }
    let d = exports.get(name).copied();
    let Some(d) = d else {
        // Not an exported Briev defn — no state on its own. (Regular defns
        // and transactions short-circuit to `true` at the call site.)
        memo.insert(name.to_string(), false);
        return false;
    };
    visiting.push(name.to_string());
    let result = body_needs_state(&d.body, regular, txns, exports, state_fields, memo, visiting);
    visiting.pop();
    memo.insert(name.to_string(), result);
    result
}

fn body_needs_state(
    body: &[Statement],
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    exports: &HashMap<String, &Definition>,
    state_fields: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut Vec<String>,
) -> bool {
    body.iter().any(|s| stmt_needs_state(s, regular, txns, exports, state_fields, memo, visiting))
}

fn stmt_needs_state(
    stmt: &Statement,
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    exports: &HashMap<String, &Definition>,
    state_fields: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut Vec<String>,
) -> bool {
    match stmt {
        Statement::Term(opt)
        | Statement::EndProgram(opt)
        | Statement::Rollback(opt) => {
            opt.as_ref().is_some_and(|e| expr_needs_state(e, regular, txns, exports, state_fields, memo, visiting))
        }
        Statement::Expression(expr) => expr_needs_state(expr, regular, txns, exports, state_fields, memo, visiting),
        Statement::Let { expr, .. } => {
            expr.as_ref().is_some_and(|e| expr_needs_state(e, regular, txns, exports, state_fields, memo, visiting))
        }
        // 2026-08-03 (node bridge): the assignment TARGET may be a state field
        // (`saved = name;`) — check it too, not just the RHS.
        Statement::Assign(lhs, expr) => {
            expr_needs_state(lhs, regular, txns, exports, state_fields, memo, visiting)
                || expr_needs_state(expr, regular, txns, exports, state_fields, memo, visiting)
        }
        Statement::Guarded(_, body) => {
            body.iter().any(|s| stmt_needs_state(s, regular, txns, exports, state_fields, memo, visiting))
        }
        Statement::Foreach { body, .. } => {
            body.iter().any(|s| stmt_needs_state(s, regular, txns, exports, state_fields, memo, visiting))
        }
        Statement::Block(body) => {
            body.iter().any(|s| stmt_needs_state(s, regular, txns, exports, state_fields, memo, visiting))
        }
        // MetadataAssignment is compile-time only, no state needed
        Statement::MetadataAssignment(..) => false,
        // Conservative: non-exhaustive match assumes needs state
        _ => true,
    }
}

fn expr_needs_state(
    expr: &Expr,
    regular: &HashMap<String, &Definition>,
    txns: &HashSet<String>,
    exports: &HashMap<String, &Definition>,
    state_fields: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
    visiting: &mut Vec<String>,
) -> bool {
    match expr {
        // Field access always needs state (reads struct metadata)
        Expr::Field(_, _) => true,
        Expr::Call(name, args, _) => {
            // Observable/stateful intrinsics need state (unchanged from the
            // backend's original list).
            let name_needs = if matches!(name.as_str(),
                "Malloc#" | "Memcpy#" | "Memmove#" | "Memset#"
                | "Print#"
                | "FileRead#" | "FileWrite#" | "ShellCmd#"
                | "SysQuery#" | "EnvGet#" | "HttpFetch#"
                | "AllocArray#" | "AllocInitArray#" | "StringNew#"
                | "StringFromPtr#" | "StringConcat#"
            ) {
                true
            } else if regular.contains_key(name.as_str()) || txns.contains(name.as_str()) {
                // Regular (non-exported) defns and transactions are ALWAYS
                // emitted with a %state parameter, so a caller must carry state.
                true
            } else if exports.contains_key(name.as_str()) {
                // Export-to-export calls: the callee may be pure (no state) or
                // stateful — resolve transitively.
                defn_needs_state(name, regular, txns, exports, state_fields, memo, visiting)
            } else {
                // frgn / other external calls do not need state at the boundary
                // — BUT their arguments may read/write state fields. The
                // marshalling rewrites `term saved;` → `term str_to_c(saved);`
                // (the CStr<->String meld), so a bare state-field read becomes
                // a frgn call ARG. Check the args too.
                false
            };
            name_needs
                || args.iter().any(|a| {
                    expr_needs_state(a, regular, txns, exports, state_fields, memo, visiting)
                })
        }
        Expr::BinaryOp(_, lhs, rhs) => {
            expr_needs_state(lhs, regular, txns, exports, state_fields, memo, visiting)
                || expr_needs_state(rhs, regular, txns, exports, state_fields, memo, visiting)
        }
        Expr::UnaryOp(_, inner) => expr_needs_state(inner, regular, txns, exports, state_fields, memo, visiting),
        Expr::List(items) => {
            items.iter().any(|e| expr_needs_state(e, regular, txns, exports, state_fields, memo, visiting))
        }
        // 2026-08-04 (compiler-in-Briev): wrapping expression kinds that can
        // HIDE a stateful inner — a cast-wrapped call (`token_at(t, 1) as Int`),
        // a method call on a state-field receiver, an index/slice/addr-of of a
        // state field. Previously the `_ => false` arm made these invisible, so
        // an export calling a regular defn through a cast got a STATELESS shim
        // that referenced `%state` (opt: "use of undefined value '%state'").
        Expr::Cast(inner, _) => expr_needs_state(inner, regular, txns, exports, state_fields, memo, visiting),
        Expr::MethodCall(recv, _, args, _, _) => {
            expr_needs_state(recv, regular, txns, exports, state_fields, memo, visiting)
                || args.iter().any(|a| expr_needs_state(a, regular, txns, exports, state_fields, memo, visiting))
        }
        Expr::Reflect(recv, _, _) => expr_needs_state(recv, regular, txns, exports, state_fields, memo, visiting),
        Expr::Index(arr, idx) => {
            expr_needs_state(arr, regular, txns, exports, state_fields, memo, visiting)
                || expr_needs_state(idx, regular, txns, exports, state_fields, memo, visiting)
        }
        Expr::Slice { array, start, end, .. } => {
            expr_needs_state(array, regular, txns, exports, state_fields, memo, visiting)
                || start.as_ref().is_some_and(|e| expr_needs_state(e, regular, txns, exports, state_fields, memo, visiting))
                || end.as_ref().is_some_and(|e| expr_needs_state(e, regular, txns, exports, state_fields, memo, visiting))
        }
        Expr::AddrOf(inner) => expr_needs_state(inner, regular, txns, exports, state_fields, memo, visiting),
        // 2026-08-03 (node bridge): a bare read of a state field needs the
        // `%state` handle even though no intrinsic/call is involved
        // (`term saved;`). Params/locals (not in state_fields) stay pure.
        Expr::Identifier(name) => state_fields.contains(name),
        // other literals are pure
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defn(name: &str, body: Vec<Statement>) -> Definition {
        use crate::ast::{Contract, OutputType, TypeParam};
        Definition {
            name: name.to_string(),
            type_params: Vec::<TypeParam>::new(),
            parameters: vec![],
            output_type: Some(OutputType::Single(crate::ast::Type::Custom("Int".to_string()))),
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body,
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        }
    }

    fn exported(d: Definition) -> TopLevel {
        use crate::ast::Export;
        TopLevel::Export(Export {
            inner: Box::new(TopLevel::Definition(d)),
            export_name: None,
        })
    }

    #[test]
    fn pure_export_needs_no_state() {
        let d = defn("add", vec![Statement::Term(Some(Expr::Decimal(5)))]);
        let items = vec![exported(d)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("add"), Some(&false));
    }

    #[test]
    fn export_calling_stateful_intrinsic_needs_state() {
        let d = defn("f", vec![Statement::Term(Some(
            Expr::Call("StringNew#".to_string(), vec![], None),
        ))]);
        let items = vec![exported(d)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&true));
    }

    #[test]
    fn export_calling_regular_defn_needs_state() {
        // helper is a regular defn (always carries %state) → caller must too.
        let helper = defn("helper", vec![Statement::Term(Some(Expr::Decimal(1)))]);
        let caller = defn("f", vec![Statement::Term(Some(
            Expr::Call("helper".to_string(), vec![], None),
        ))]);
        let items = vec![TopLevel::Definition(helper), exported(caller)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&true));
    }

    #[test]
    fn pure_export_calling_pure_export_needs_no_state() {
        let inner = defn("inner", vec![Statement::Term(Some(Expr::Decimal(1)))]);
        let outer = defn("outer", vec![Statement::Term(Some(
            Expr::Call("inner".to_string(), vec![], None),
        ))]);
        let items = vec![exported(inner), exported(outer)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("inner"), Some(&false));
        assert_eq!(map.get("outer"), Some(&false));
    }

    #[test]
    fn export_calling_frgn_needs_no_state() {
        // cstr_to_briev is a frgn, not a Briev defn → no state.
        let d = defn("f", vec![Statement::Term(Some(
            Expr::Call("cstr_to_briev".to_string(), vec![], None),
        ))]);
        let items = vec![exported(d)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&false));
    }

    #[test]
    fn mutual_recursion_is_conservative() {
        let a = defn("a", vec![Statement::Term(Some(
            Expr::Call("b".to_string(), vec![], None),
        ))]);
        let b = defn("b", vec![Statement::Term(Some(
            Expr::Call("a".to_string(), vec![], None),
        ))]);
        let items = vec![exported(a), exported(b)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("a"), Some(&true));
        assert_eq!(map.get("b"), Some(&true));
    }

    #[test]
    fn export_calling_txn_needs_state() {
        // Transactions always carry %state — a caller must supply it.
        use crate::ast::{Contract, PropertyValue, Transaction, TypeParam};
        use crate::errors::Span;
        let txn = Transaction {
            name: "loop_txn".to_string(),
            is_reactive: false,
            is_async: false,
            type_params: Vec::<TypeParam>::new(),
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body: vec![],
            metadata: std::collections::HashMap::<String, PropertyValue>::new(),
            derivation: None,
            modifiers: vec![],
            span: Option::<Span>::None,
            doc: None,
        };
        let caller = defn("f", vec![Statement::Term(Some(
            Expr::Call("loop_txn".to_string(), vec![], None),
        ))]);
        let items = vec![TopLevel::Transaction(txn), exported(caller)];
        let map = compute_export_needs_state(&items);
        assert_eq!(map.get("f"), Some(&true));
    }
}
