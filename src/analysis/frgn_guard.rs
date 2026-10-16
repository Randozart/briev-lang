// ── frgn? Guard Safety Pass ──────────────────────────────────
// 2026-07-25: Checks that every frgn? call is guarded by fn?
// before the call site. If a function body calls a frgn? symbol
// without first checking fn? on all paths, compilation fails.
//
// Integration: called after typechecking, before codegen.

use std::collections::HashSet;

use std::collections::HashMap;

use crate::ast::*;

/// Check all definitions and transactions for unguarded frgn? calls.
pub fn check_frgn_guards(items: &[TopLevel]) -> Result<(), String> {
    // Build a lookup table of frgn?/frgn!/frgn?! bindings
    let mut optional_frgns: HashMap<String, bool> = HashMap::new();
    for item in items {
        if let TopLevel::ForeignBinding(fb) = item {
            if fb.is_optional || fb.is_fire_forget || fb.is_delivery {
                let name = fb.briev_name.clone().unwrap_or(fb.foreign_name.clone());
                optional_frgns.insert(name, true);
            }
        }
    }

    for item in items {
        match item {
            TopLevel::Definition(d) => check_body(&d.body, &d.name, &optional_frgns)?,
            TopLevel::Transaction(t) => check_body(&t.body, &t.name, &optional_frgns)?,
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    check_body(&d.body, &d.name, &optional_frgns)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Walk a function body and check all Expr::Call sites.
fn check_body(
    body: &[Statement],
    fn_name: &str,
    optional_frgns: &HashMap<String, bool>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let guarded = collect_guarded_fns(body);
    let calls: Vec<String> = collect_calls(body);

    for name in &calls {
        if optional_frgns.contains_key(name) && !guarded.contains(name) {
            errors.push(format!(
                "frgn? '{}' called in '{}' but not guarded by {}?",
                name, fn_name, name
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// Collect all function names guarded by Expr::Exists checks.
fn collect_guarded_fns(body: &[Statement]) -> std::collections::HashSet<String> {
    let mut guarded = HashSet::new();
    for stmt in body {
        match stmt {
            Statement::Guarded(cond, inner) => {
                // Check if the guard is Expr::Exists("fn_name")
                if let crate::ast::Expr::Exists(name) = cond {
                    guarded.insert(name.clone());
                }
                // Also recurse into inner body for nested guards
                guarded.extend(collect_guarded_fns(inner));
            }
            Statement::Block(stmts) => {
                guarded.extend(collect_guarded_fns(stmts));
            }
            _ => {}
        }
    }
    guarded
}

/// Collect all function names called via Expr::Call.
fn collect_calls(body: &[Statement]) -> Vec<String> {
    let mut calls = Vec::new();
    for stmt in body {
        match stmt {
            Statement::Expression(e) => collect_calls_from_expr(e, &mut calls),
            Statement::Let { expr, .. } => {
                if let Some(e) = expr {
                    collect_calls_from_expr(e, &mut calls);
                }
            }
            Statement::Term(Some(e)) | Statement::Tail(e) => collect_calls_from_expr(e, &mut calls),
            Statement::Guarded(_, inner) => calls.extend(collect_calls(inner)),
            Statement::Block(stmts) => calls.extend(collect_calls(stmts)),
            _ => {}
        }
    }
    calls
}

fn collect_calls_from_expr(expr: &Expr, calls: &mut Vec<String>) {
    match expr {
        Expr::Call(name, args, _) => {
            calls.push(name.clone());
            for arg in args {
                collect_calls_from_expr(arg, calls);
            }
        }
        Expr::BinaryOp(_, lhs, rhs) => {
            collect_calls_from_expr(lhs, calls);
            collect_calls_from_expr(rhs, calls);
        }
        Expr::UnaryOp(_, inner) => collect_calls_from_expr(inner, calls),
        Expr::Field(inner, _) => collect_calls_from_expr(inner, calls),
        Expr::Index(inner, idx) => {
            collect_calls_from_expr(inner, calls);
            collect_calls_from_expr(idx, calls);
        }
        Expr::List(elems) => {
            for e in elems {
                collect_calls_from_expr(e, calls);
            }
        }
        Expr::Tuple(elems) => {
            for e in elems {
                collect_calls_from_expr(e, calls);
            }
        }
        Expr::If(cond, then, else_) => {
            collect_calls_from_expr(cond, calls);
            collect_calls_from_expr(then, calls);
            if let Some(el) = else_ {
                collect_calls_from_expr(el, calls);
            }
        }
        Expr::Match(expr, arms) => {
            collect_calls_from_expr(expr, calls);
            for arm in arms {
                collect_calls_from_expr(&arm.body, calls);
            }
        }
        Expr::Block(body) => {
            calls.extend(collect_calls(body));
        }
        _ => {}
    }
}
