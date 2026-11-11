use crate::ast::{BinaryOpKind, Expr, Statement};
use std::collections::HashSet;

/// Describes where a pointer value originates from — used to detect
/// dangling borrows and to refine parallel-txn write sets.
#[derive(Debug, Clone, PartialEq)]
pub enum Provenance {
    /// Points to a named variable or state field.
    Known(String),
    /// Points to a field of a known base: `base.field`.
    FieldAccess {
        base: Box<Provenance>,
        field: String,
    },
    /// Points to an index into a known base: `base[idx]`.
    Index {
        base: Box<Provenance>,
        index: Box<Provenance>,
    },
    /// Points to the target of a dereference: `*ptr`.
    Deref(Box<Provenance>),
    /// Provenance cannot be determined (opaque source, FFI, or complex expr).
    Unknown,
}

/// Infer the provenance of an expression. This is used alongside type
/// inference during typechecking to track where pointer values come from.
pub fn infer_provenance(expr: &Expr) -> Provenance {
    match expr {
        Expr::Identifier(name) => Provenance::Known(name.clone()),
        Expr::Field(base, field) => Provenance::FieldAccess {
            base: Box::new(infer_provenance(base)),
            field: field.clone(),
        },
        Expr::Index(base, _) => Provenance::Index {
            base: Box::new(infer_provenance(base)),
            index: Box::new(Provenance::Unknown),
        },
        // 2026-08-01 (D1): `&x` / `*p` — the provenance of the operand. The
        // dangling-pointer detector (`p = &local`) was silently missing every
        // case because AddrOf/Deref fell through to Unknown.
        Expr::AddrOf(inner) | Expr::Deref(inner) | Expr::Consume(inner) => infer_provenance(inner),
        Expr::Cast(inner, _) => infer_provenance(inner),
        _ => Provenance::Unknown,
    }
}

/// Compute the provenance of a dereference expression.
/// `*p` → deref(p's provenance)
/// `*(*p)` → just p's provenance (double deref collapses)
pub fn deref_provenance(ptr: &Expr) -> Provenance {
    let inner = infer_provenance(ptr);
    match inner {
        Provenance::Unknown => Provenance::Unknown,
        Provenance::Deref(in_inner) => *in_inner,
        _ => Provenance::Deref(Box::new(inner)),
    }
}

/// Check if a provenance refers to a local (non-state) variable.
/// 2026-07-18: Fixed stub — now checks `local_names` set instead of
/// always returning false. Used to detect dangling pointers: storing
/// the address of a local into a state field creates a dangling reference.
pub fn is_local_provenance(prov: &Provenance, local_names: &HashSet<String>) -> bool {
    match prov {
        Provenance::Known(name) => local_names.contains(name),
        Provenance::FieldAccess { base, .. } | Provenance::Index { base, .. } => {
            is_local_provenance(base, local_names)
        }
        Provenance::Deref(_) => false,
        Provenance::Unknown => false,
    }
}

/// Extract (target, source) from a pointer-to-pointer assignment.
/// Returns `Some((lhs_target, rhs_expr))` for statements like:
/// `&state_ptr = &local_var;`  —  `AddrOf(state_ptr) = AddrOf(local_var)`
pub fn extract_ptr_assign(stmt: &Statement) -> Option<(&Expr, &Expr)> {
    if let Statement::Assign(lhs, expr) = stmt {
        return Some((lhs, expr));
    }
    None
}

/// Build a diagnostic warning about a dangling pointer assignment.
pub fn build_dangling_warning(target: &Expr, source: &Expr) -> String {
    let target_name = match target {
        Expr::Identifier(n) => n.as_str(),
        _ => "<expression>",
    };
    let source_name = match source {
        Expr::Identifier(n) => n.as_str(),
        // 2026-08-01 (D1): `&local` — peel the address-of so the warning names
        // the escaping local, not "<expression>".
        Expr::AddrOf(inner) => match inner.as_ref() {
            Expr::Identifier(n) => n.as_str(),
            _ => "<expression>",
        },
        _ => "<expression>",
    };
    format!(
        "warning: storing pointer to local '{}' in state field '{}' may dangle. \
         Store the value, not the pointer, or ensure the local outlives the state reference",
        source_name, target_name
    )
}

/// Scan transaction body assignments for dangling pointer patterns.
/// 2026-07-18: Now takes `local_names` set to distinguish local vars
/// from state fields. Emits warnings when a pointer to a local variable
/// is stored in a state field via `&target = &source`.
pub fn check_dangling_ptrs(body: &[Statement], local_names: &HashSet<String>) -> Vec<String> {
    let mut warnings = Vec::new();
    for stmt in body {
        let Some((target, source)) = extract_ptr_assign(stmt) else {
            continue;
        };
        // 2026-08-01 (D1): only an ADDRESS (`&local`) can dangle. A plain
        // value assignment (`x0 = nx0` where nx0 is a local Float) copies the
        // value, not a pointer — flagging it would reject every value-assign
        // from a local (the kalman family). The pointer escape is `&local`.
        let Expr::AddrOf(inner) = source else {
            continue;
        };
        let source_prov = infer_provenance(inner);
        if is_local_provenance(&source_prov, local_names) {
            warnings.push(build_dangling_warning(target, source));
        }
    }
    warnings
}

/// Collect variable names referenced in an expression.
fn collect_var_names(expr: &Expr) -> Vec<String> {
    let mut vars = Vec::new();
    let mut work = vec![expr];
    while let Some(e) = work.pop() {
        match e {
            Expr::Identifier(n) => vars.push(n.clone()),
            Expr::UnaryOp(_, inner) | Expr::Cast(inner, _) | Expr::IsType(inner, _) => {
                work.push(inner);
            }
            Expr::BinaryOp(_, a, b) => {
                work.push(b);
                work.push(a);
            }
            Expr::Field(obj, _) | Expr::Index(obj, _) => {
                work.push(obj);
            }
            Expr::Call(_, args, _) => {
                work.extend(args.iter().rev());
            }
            Expr::If(cond, then, else_) => {
                work.push(then);
                work.push(cond);
                if let Some(else_) = else_ {
                    work.push(else_);
                }
            }
            Expr::Block(stmts) => {
                for stmt in stmts {
                    if let Statement::Expression(e) = stmt {
                        work.push(e);
                    }
                }
            }
            Expr::Lambda(_, body) => {
                work.push(body);
            }
            Expr::Within(inner, _) => {
                work.push(inner);
            }
            Expr::Tuple(elems) | Expr::List(elems) => {
                work.extend(elems.iter().rev());
            }
            Expr::Match(_, arms) => {
                for arm in arms {
                    work.push(&arm.body);
                    if let Some(ref guard) = arm.guard {
                        work.push(guard);
                    }
                }
            }
            _ => {}
        }
    }
    vars
}

/// Collect local variable names from a transaction's let-bindings and
/// parameters. Used by check_dangling_ptrs to distinguish local variables
/// from state fields.
pub fn collect_local_names(
    body: &[Statement],
    params: &[(String, crate::ast::Type)],
) -> HashSet<String> {
    let mut locals: HashSet<String> = params.iter().map(|(n, _)| n.clone()).collect();
    for stmt in body {
        if let Statement::Let { name, .. } = stmt {
            locals.insert(name.clone());
        }
        if let Statement::Block(inner) = stmt {
            locals.extend(collect_local_names(inner, &[]));
        }
    }
    locals
}

/// Collect variable names written to by assignment statements in a body.
fn collect_write_vars(body: &[Statement]) -> Vec<String> {
    let mut vars = Vec::new();
    for stmt in body {
        if let Statement::Assign(lhs, _) = stmt {
            if let Some(name) = lhs.as_var_name() {
                vars.push(name.to_string());
            }
        }
        if let Statement::Let { name, .. } = stmt {
            vars.push(name.clone());
        }
    }
    vars
}

/// Check that every reactive transaction can converge on its own.
/// A reactive txn must modify at least one variable in its pre-condition;
/// otherwise it depends entirely on other txns to unblock it, which is
/// a convergence footgun.
///
/// Returns (severity, message) pairs. When no other txn modifies any
/// pre-condition variable and no convergng txn depends on it, the
/// txn will run forever — this is a hard error, not a warning.
pub fn check_convergence_safety(
    txn_name: &str,
    is_reactive: bool,
    pre: &Expr,
    body: &[Statement],
    all_txns: &std::collections::HashMap<String, crate::ast::Transaction>,
) -> Vec<(&'static str, String)> {
    let mut results = Vec::new();
    if !is_reactive {
        return results;
    }
    if matches!(pre, Expr::Bool(true)) {
        return results;
    }

    let pre_vars = collect_var_names(pre);
    let write_vars = collect_write_vars(body);

    let modifies_own_pre = pre_vars.iter().any(|pv| write_vars.contains(pv));
    if !modifies_own_pre {
        let pre_str = pre_vars.join(", ");
        let write_str = if write_vars.is_empty() {
            "nothing".to_string()
        } else {
            write_vars.join(", ")
        };

        // Check if ANY other txn writes to any pre-condition variable.
        let other_modifies = pre_vars.iter().any(|pv| {
            all_txns.iter().any(|(other_name, other_txn)| {
                other_name != txn_name && {
                    let other_writes = collect_write_vars(&other_txn.body);
                    other_writes.contains(pv)
                }
            })
        });

        let msg = format!(
            "reactive txn '{}' has precondition [{}] but body writes to [{}]. \
             The txn does not modify any precondition variable and depends on other \
             txns to satisfy its convergence. If no other txn changes the precondition \
             to false, this txn will run forever.",
            txn_name, pre_str, write_str
        );

        if other_modifies {
            // Another txn may eventually unblock this one — soft warning.
            results.push(("warning", msg));
        } else {
            // No txn modifies any pre-condition variable — guaranteed infinite loop.
            results.push(("error", msg));
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provenance_identifier() {
        let prov = infer_provenance(&Expr::Identifier("x".to_string()));
        assert_eq!(prov, Provenance::Known("x".to_string()));
    }

    #[test]
    fn test_check_dangling_ptrs_detects_local_escape() {
        // 2026-08-01 (D1): `state_ptr = &local` is a dangling-pointer escape —
        // the provenance layer must flag it (the type system rejects it via
        // PtrConst; this is the defense-in-depth hard error).
        let body = vec![Statement::Assign(
            Expr::Identifier("p".to_string()),
            Expr::AddrOf(Box::new(Expr::Identifier("x".to_string()))),
        )];
        let locals: HashSet<String> = ["x".to_string()].into_iter().collect();
        let warnings = check_dangling_ptrs(&body, &locals);
        assert_eq!(warnings.len(), 1, "&local escape must be flagged");
        assert!(warnings[0].contains("p"), "warning names the target field");
    }

    #[test]
    fn test_check_dangling_ptrs_ignores_state_field() {
        // A pointer to a STATE field is not dangling (state lives for the
        // program duration).
        let body = vec![Statement::Assign(
            Expr::Identifier("q".to_string()),
            Expr::AddrOf(Box::new(Expr::Identifier("other".to_string()))),
        )];
        let locals: HashSet<String> = ["x".to_string()].into_iter().collect();
        let warnings = check_dangling_ptrs(&body, &locals);
        assert_eq!(warnings.len(), 0, "&state-field must not be flagged");
    }

    #[test]
    fn test_provenance_field() {
        let expr = Expr::Field(
            Box::new(Expr::Identifier("obj".to_string())),
            "field".to_string(),
        );
        let prov = infer_provenance(&expr);
        assert_eq!(
            prov,
            Provenance::FieldAccess {
                base: Box::new(Provenance::Known("obj".to_string())),
                field: "field".to_string(),
            }
        );
    }

    #[test]
    fn test_provenance_index() {
        let expr = Expr::Index(
            Box::new(Expr::Identifier("arr".to_string())),
            Box::new(Expr::Decimal(0)),
        );
        let prov = infer_provenance(&expr);
        assert_eq!(
            prov,
            Provenance::Index {
                base: Box::new(Provenance::Known("arr".to_string())),
                index: Box::new(Provenance::Unknown),
            }
        );
    }

    #[test]
    fn test_provenance_unknown() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x".to_string())),
            Box::new(Expr::Decimal(1)),
        );
        let prov = infer_provenance(&expr);
        assert_eq!(prov, Provenance::Unknown);
    }

    #[test]
    fn test_extract_ptr_assign_simple() {
        let stmt = Statement::Assign(
            Expr::Identifier("dst".to_string()),
            Expr::Identifier("src".to_string()),
        );
        let result = extract_ptr_assign(&stmt);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_ptr_assign_non_ptr_returns_some() {
        let stmt = Statement::Assign(Expr::Identifier("x".to_string()), Expr::Decimal(42));
        assert!(extract_ptr_assign(&stmt).is_some());
    }

    #[test]
    fn test_dangling_warning_message() {
        let target = Expr::Identifier("state_field".to_string());
        let source = Expr::Identifier("local_var".to_string());
        let msg = build_dangling_warning(&target, &source);
        assert!(msg.contains("local_var"));
        assert!(msg.contains("state_field"));
        assert!(msg.contains("may dangle"));
    }

    #[test]
    fn test_check_dangling_ptrs_no_warning_on_normal_assign() {
        let body = vec![Statement::Assign(
            Expr::Identifier("x".to_string()),
            Expr::Decimal(42),
        )];
        let local_names = HashSet::from(["x".to_string()]);
        let warnings = check_dangling_ptrs(&body, &local_names);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_is_local_provenance_with_locals() {
        let local_names = HashSet::from(["temp".to_string()]);
        assert!(is_local_provenance(
            &Provenance::Known("temp".to_string()),
            &local_names
        ));
        assert!(!is_local_provenance(
            &Provenance::Known("state_field".to_string()),
            &local_names
        ));
    }

    #[test]
    fn test_dangling_warning_fires() {
        // 2026-08-01 (D1): only an ADDRESS escape (`&local`) is a dangling
        // pointer — a plain value assign (`state_field = local_var`) copies
        // the value and is never flagged.
        let body = vec![Statement::Assign(
            Expr::Identifier("state_field".to_string()),
            Expr::AddrOf(Box::new(Expr::Identifier("local_var".to_string()))),
        )];
        let local_names = HashSet::from(["local_var".to_string()]);
        let warnings = check_dangling_ptrs(&body, &local_names);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("local_var"));
        assert!(warnings[0].contains("state_field"));
    }

    #[test]
    fn test_value_assign_from_local_not_flagged() {
        // `x0 = nx0` (nx0 a local value) is not a pointer escape.
        let body = vec![Statement::Assign(
            Expr::Identifier("x0".to_string()),
            Expr::Identifier("nx0".to_string()),
        )];
        let local_names = HashSet::from(["nx0".to_string()]);
        let warnings = check_dangling_ptrs(&body, &local_names);
        assert_eq!(warnings.len(), 0);
    }
}
