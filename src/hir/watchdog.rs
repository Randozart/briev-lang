// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H5 inventory port: watchdog diagnostics over HIR.
//! Mirror of `analysis::watchdog` (AST). PORT-ONLY: consumers
//! (`compile.rs`, `pipeline.rs`) run before the backend builds HIR — the
//! flip belongs to the pipeline-HIR batch. Parity tests pin HIR == AST on
//! the AST fixtures.
//!
//! Coverage mirrors the AST walker: bare top-level reactive transactions
//! (raw items, not effective), trigger names from `TopLevel::Trigger`, the
//! on-fire callable set = bare txns/defns + type-member txns/defns (impl
//! functions distinguished via `is_impl_fn` and excluded, exactly like the
//! AST walker which never unwraps impl blocks).

use super::expr::{HirExpr, HirExprKind};
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::HirStmtKind;
use crate::analysis::watchdog::WatchdogError;
use crate::ast::{BinaryOpKind, UnaryOpKind};
use std::collections::{BTreeSet, HashSet};

pub fn analyze(program: &HirProgram) -> Vec<WatchdogError> {
    let trigger_names: HashSet<&str> = program
        .trigger_names
        .iter()
        .map(|n| n.as_str())
        .collect();
    let mut errors = Vec::new();
    for c in bare_reactive_txns(program) {
        let Some(wd) = c.contract.as_ref().and_then(|ct| ct.watchdog.as_ref()) else {
            continue;
        };
        let HirExprKind::Identifier(cond_name) = &wd.condition.kind else {
            continue;
        };
        let txn_name = c.name.clone();
        if !trigger_names.contains(cond_name.as_str()) {
            errors.push(WatchdogError::UnknownTrigger(txn_name, cond_name.clone()));
            continue;
        }
        let handlers = find_handlers(program, cond_name);
        if handlers.is_empty() {
            errors.push(WatchdogError::NoHandler(txn_name, cond_name.clone()));
            continue;
        }
        let handler_writes = collect_handler_writes(program, &handlers);
        let contract = c.contract.as_ref().unwrap();
        let pre_vars = extract_variables(&contract.pre);
        let intersecting: Vec<String> = handler_writes.intersection(&pre_vars).cloned().collect();
        if intersecting.is_empty() {
            if !wd.is_required
                && !matches!(&contract.pre.kind, HirExprKind::Literal(HirLiteral::Bool(true)))
            {
                let has_convergence = check_convergent_loop(contract);
                if has_convergence {
                    continue;
                }
            }
            let any_pre_var_written = pre_vars.iter().any(|v| handler_writes.contains(v));
            if any_pre_var_written {
                for var in pre_vars.iter() {
                    if handler_writes.contains(var) {
                        errors.push(WatchdogError::NoConflict(
                            txn_name.clone(),
                            cond_name.clone(),
                            var.clone(),
                        ));
                    }
                }
            } else if wd.is_required {
                let mut vars_list: Vec<String> = pre_vars.iter().cloned().collect();
                vars_list.sort();
                let vars_str = vars_list.join(", ");
                errors.push(WatchdogError::NoConflict(txn_name, cond_name.clone(), vars_str));
            }
            continue;
        }
        for var in &intersecting {
            let falsifies = check_falsifies(program, &handlers, contract, var);
            if !falsifies {
                errors.push(WatchdogError::HandlerDoesNotFalsify(
                    txn_name.clone(),
                    cond_name.clone(),
                    var.clone(),
                ));
                continue;
            }
            let restores = check_restores(program, &handlers, contract, var);
            if restores {
                errors.push(WatchdogError::HandlerRestoresPrecondition(
                    txn_name.clone(),
                    cond_name.clone(),
                    var.clone(),
                ));
            }
        }
    }
    errors
}

fn bare_reactive_txns<'a>(program: &'a HirProgram) -> Vec<&'a HirCallable> {
    program
        .callables
        .iter()
        .filter(|c| {
            matches!(c.kind, HirCallableKind::Transaction)
                && c.owner.is_none()
                && !c.exported
                && c.sync_domains.is_empty()
        })
        .collect()
}

pub fn check_on_fire_handlers(program: &HirProgram) -> Result<(), String> {
    // Exact AST mirror: bare txns/defns + type-member txns/defns. Impl
    // functions (`is_impl_fn`) and trait defaults are NOT declared
    // callable names in the AST walker.
    let mut callables: HashSet<String> = HashSet::new();
    for c in program.callables.iter() {
        let is_declared = match c.kind {
            HirCallableKind::Transaction | HirCallableKind::Defn => {
                c.owner.is_none() && !c.is_impl_fn && !c.trait_default
            }
            HirCallableKind::MemberDefn => !c.is_impl_fn && !c.trait_default && c.owner.is_some(),
            _ => false,
        };
        if is_declared {
            callables.insert(c.name.clone());
        }
    }
    for c in bare_reactive_txns(program) {
        let Some(wd) = c.contract.as_ref().and_then(|ct| ct.watchdog.as_ref()) else {
            continue;
        };
        if let Some(on_fire) = &wd.on_fire {
            if !callables.contains(&on_fire.handler) {
                return Err(format!(
                    "watchdog in '{}': on-fire handler '{}' is not a declared txn/defn/node",
                    c.name, on_fire.handler
                ));
            }
        }
    }
    Ok(())
}

fn find_handlers(program: &HirProgram, trigger_name: &str) -> Vec<String> {
    let mut handlers = Vec::new();
    for c in bare_reactive_txns(program) {
        let Some(contract) = &c.contract else { continue };
        if contains_trigger_in_guard(&contract.pre, trigger_name) {
            handlers.push(c.name.clone());
        }
    }
    handlers
}

fn contains_trigger_in_guard(expr: &HirExpr, trigger_name: &str) -> bool {
    match &expr.kind {
        HirExprKind::Identifier(name) => name == trigger_name,
        HirExprKind::Binary { op: BinaryOpKind::And | BinaryOpKind::Or, left, right } => {
            contains_trigger_in_guard(left, trigger_name)
                || contains_trigger_in_guard(right, trigger_name)
        }
        _ => false,
    }
}

fn collect_handler_writes(
    program: &HirProgram,
    handler_names: &[String],
) -> HashSet<String> {
    let handler_set: HashSet<&str> = handler_names.iter().map(|n| n.as_str()).collect();
    let mut writes = HashSet::new();
    for c in bare_reactive_txns(program) {
        if handler_set.contains(c.name.as_str()) {
            collect_writes_from_body(&c.body.statements, &mut writes);
        }
    }
    writes
}

fn collect_writes_from_body(body: &[super::stmt::HirStmt], writes: &mut HashSet<String>) {
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::Assign(lhs, _) => {
                if let HirExprKind::Identifier(name) = &lhs.kind {
                    writes.insert(name.clone());
                }
            }
            HirStmtKind::Guarded(_, statements) => collect_writes_from_body(statements, writes),
            HirStmtKind::SyncBlock(inner) => collect_writes_from_body(inner, writes),
            _ => {}
        }
    }
}

fn extract_variables(expr: &HirExpr) -> HashSet<String> {
    let mut vars = HashSet::new();
    collect_vars(expr, &mut vars);
    vars
}

fn collect_vars(expr: &HirExpr, vars: &mut HashSet<String>) {
    match &expr.kind {
        HirExprKind::Identifier(name) => {
            vars.insert(name.clone());
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_vars(left, vars);
            collect_vars(right, vars);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. } => collect_vars(inner, vars),
        HirExprKind::Call { args, .. } => {
            for arg in args {
                collect_vars(arg, vars);
            }
        }
        HirExprKind::Field { receiver, .. } => collect_vars(receiver, vars),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            for elem in items {
                collect_vars(elem, vars);
            }
        }
        HirExprKind::Index(list, idx) => {
            collect_vars(list, vars);
            collect_vars(idx, vars);
        }
        HirExprKind::If { condition, then_branch, else_branch } => {
            collect_vars(condition, vars);
            collect_vars(then_branch, vars);
            if let Some(else_) = else_branch {
                collect_vars(else_, vars);
            }
        }
        HirExprKind::Match { arms, .. } => {
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_vars(guard, vars);
                }
                collect_vars(&arm.body, vars);
            }
        }
        HirExprKind::Block(stmts) => {
            for stmt in stmts {
                if let HirStmtKind::Expression(e) = &stmt.kind {
                    collect_vars(e, vars);
                }
            }
        }
        HirExprKind::Lambda { body, .. } => collect_vars(body, vars),
        _ => {}
    }
}

fn check_falsifies(
    program: &HirProgram,
    handler_names: &[String],
    contract: &super::program::HirContract,
    var: &str,
) -> bool {
    let handler_set: HashSet<&str> = handler_names.iter().map(|n| n.as_str()).collect();
    for c in bare_reactive_txns(program) {
        if handler_set.contains(c.name.as_str()) {
            if check_falsifies_in_body(&c.body.statements, contract, var) {
                return true;
            }
        }
    }
    false
}

fn check_falsifies_in_body(
    body: &[super::stmt::HirStmt],
    contract: &super::program::HirContract,
    var: &str,
) -> bool {
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::Assign(lhs, expr) => {
                if let HirExprKind::Identifier(name) = &lhs.kind {
                    if name == var {
                        match &contract.pre.kind {
                            HirExprKind::Binary {
                                op: BinaryOpKind::Eq | BinaryOpKind::Neq,
                                ..
                            } => {
                                if evaluate_literal(expr).is_some() {
                                    return true;
                                }
                            }
                            HirExprKind::Identifier(ident) if ident == var => {
                                if let HirExprKind::Literal(HirLiteral::Bool(false)) = &expr.kind {
                                    return true;
                                }
                            }
                            HirExprKind::Unary { op: UnaryOpKind::Not, inner } => {
                                if let HirExprKind::Identifier(ident) = &inner.kind {
                                    if ident == var {
                                        if let HirExprKind::Literal(HirLiteral::Bool(true)) =
                                            &expr.kind
                                        {
                                            return true;
                                        }
                                    }
                                }
                            }
                            _ => return true,
                        }
                    }
                }
            }
            HirStmtKind::Guarded(_, statements) => {
                if check_falsifies_in_body(statements, contract, var) {
                    return true;
                }
            }
            HirStmtKind::SyncBlock(inner) => {
                if check_falsifies_in_body(inner, contract, var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn check_restores(
    program: &HirProgram,
    handler_names: &[String],
    contract: &super::program::HirContract,
    var: &str,
) -> bool {
    let handler_set: HashSet<&str> = handler_names.iter().map(|n| n.as_str()).collect();
    for c in bare_reactive_txns(program) {
        if handler_set.contains(c.name.as_str()) {
            if check_restores_in_body(&c.body.statements, contract, var) {
                return true;
            }
        }
    }
    false
}

fn check_restores_in_body(
    body: &[super::stmt::HirStmt],
    contract: &super::program::HirContract,
    var: &str,
) -> bool {
    let mut found_falsify = false;
    for stmt in body {
        match &stmt.kind {
            HirStmtKind::Assign(lhs, expr) => {
                if let HirExprKind::Identifier(name) = &lhs.kind {
                    if name == var {
                        if !found_falsify {
                            match &contract.pre.kind {
                                HirExprKind::Identifier(ident) if ident == var => {
                                    if let HirExprKind::Literal(HirLiteral::Bool(false)) = &expr.kind {
                                        found_falsify = true;
                                        continue;
                                    }
                                }
                                _ => {
                                    found_falsify = true;
                                    continue;
                                }
                            }
                        } else {
                            match &contract.pre.kind {
                                HirExprKind::Identifier(ident) if ident == var => {
                                    if let HirExprKind::Literal(HirLiteral::Bool(true)) = &expr.kind {
                                        return true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            HirStmtKind::Guarded(_, statements) => {
                if check_restores_in_body(statements, contract, var) {
                    return true;
                }
            }
            HirStmtKind::SyncBlock(inner) => {
                if check_restores_in_body(inner, contract, var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn evaluate_literal(expr: &HirExpr) -> Option<bool> {
    match &expr.kind {
        HirExprKind::Literal(HirLiteral::Bool(b)) => Some(*b),
        HirExprKind::Literal(HirLiteral::Int(n)) => Some(*n != 0),
        HirExprKind::Unary { op: UnaryOpKind::Not, inner } => {
            evaluate_literal(inner).map(|v| !v)
        }
        _ => None,
    }
}

fn check_convergent_loop(contract: &super::program::HirContract) -> bool {
    use HirExprKind::Binary;
    let pre_matches = matches!(
        &contract.pre.kind,
        Binary {
            op: BinaryOpKind::Lt
                | BinaryOpKind::Le
                | BinaryOpKind::Gt
                | BinaryOpKind::Ge
                | BinaryOpKind::Neq,
            ..
        }
    );
    let post_matches = matches!(
        &contract.post.kind,
        Binary { op: BinaryOpKind::Eq, .. }
    );
    pre_matches && post_matches
}

// Re-exported literal/kind imports used only in the patterns above.
use super::expr::HirLiteral;


#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::lower_program;

    const TRG: &str = "trg btn @ io;";

    fn compare(src: &str) -> Vec<WatchdogError> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let items = crate::parser::Parser::new(tokens, src).parse_program().unwrap();
        let ast = crate::analysis::watchdog::analyze(&items);
        let hir = analyze(&lower_program(&items));
        assert_eq!(hir, ast, "HIR/AST watchdog drift");
        hir
    }

    #[test]
    fn unknown_trigger() {
        let errors = compare(&format!(
            "{TRG}\nnode main [true][true] ![ghost] {{ term; }};"
        ));
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], WatchdogError::UnknownTrigger(..)));
    }

    #[test]
    fn no_handler() {
        let errors = compare(&format!(
            "{TRG}\nnode main [true][true] ![btn] {{ term; }};"
        ));
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], WatchdogError::NoHandler(..)));
    }

    #[test]
    fn handler_writes_unrelated_var_required_no_conflict() {
        let errors = compare(&format!(
            "{TRG}\nnode handler [btn][true] {{ unrelated = 42; term; }};\n\
             node main [ready][true] ![btn] {{ term; }};\n\
             let ready: Bool = false; let unrelated: Int = 0;"
        ));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(matches!(&errors[0], WatchdogError::NoConflict(..)));
    }

    #[test]
    fn handler_does_not_falsify() {
        let errors = compare(&format!(
            "{TRG}\nnode handler [btn][true] {{ ready = true; term; }};\n\
             node main [ready][true] ![btn] {{ term; }};\n\
             let ready: Bool = false;"
        ));
        assert_eq!(errors.len(), 1);
        assert!(matches!(&errors[0], WatchdogError::HandlerDoesNotFalsify(..)));
    }

    #[test]
    fn handler_falsifies_ok() {
        let errors = compare(&format!(
            "{TRG}\nnode handler [btn][true] {{ ready = false; term; }};\n\
             node main [ready][true] ![btn] {{ term; }};\n\
             let ready: Bool = false;"
        ));
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn on_fire_undeclared_handler_rejected() {
        let src = format!(
            "{TRG}\nnode handler [btn][true] {{ ready = false; term; }};\n\
             node main [ready][true] ![btn] -> ghost_handler(1) {{ term; }};\n\
             let ready: Bool = false;"
        );
        assert!(src.contains("ghost_handler"));
        let tokens = crate::lexer::tokenize(&src).unwrap();
        let items = crate::parser::Parser::new(tokens, &src).parse_program().unwrap();
        let ast = crate::analysis::watchdog::check_on_fire_handlers(&items);
        let hir = check_on_fire_handlers(&lower_program(&items));
        assert_eq!(hir, ast, "on-fire parity drift");
        assert!(hir.is_err(), "{hir:?}");
    }

    #[test]
    fn no_watchdogs_no_errors() {
        let errors = compare("node plain [true][true] { term; };");
        assert!(errors.is_empty());
    }
}
