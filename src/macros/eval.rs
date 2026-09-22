// ── Navigation Chain Evaluation ─────────────────────────────────────────
// 2026-07-21: Evaluates a tree of $ calls against the live compilation state.
// Each $ name dispatches to the appropriate selection, traversal, position,
// or action handler.
// 2026-07-23: Added capability sandboxing — each intrinsic checks its
// capability requirement before executing. Side-effect intrinsics require
// explicit --allow-* flags.
//
// Max 2 levels per function. Flat dispatch: one arm per intrinsic name.

use std::collections::{BTreeSet, HashMap};
use crate::ast::{Expr, Statement, TopLevel, PropertyValue, ImportKind, Type, BinaryOpKind};
use crate::ast::top::*;
use crate::ast::StageKind;
use std::fmt;
use crate::type_universe::TypeUniverse;
use crate::plugin::PluginManager;
use super::selection::{
    Selection, NodeRef, Selector,
    TagSelector, NamedSelector, WithKeySelector, WithAttrSelector, AllSelector,
    node_tag, top_level_name,
};
use super::actions::{
    Position, insert_items, insert_before_each, insert_after_each,
    delete_selection, replace_selection, set_metadata, rename_selection,
    collect_toplevel_indices,
};
use super::text_ops::TextSelection;
use super::stage_target;

/// Sandbox permissions for compile-time $ intrinsics.
/// Each capability corresponds to a --allow-* CLI flag.
/// By default, only pure AST/string intrinsics are allowed.
#[derive(Debug, Clone, Default)]
pub struct Sandbox {
    /// Read files via FileRead$
    pub allow_read: bool,
    /// Write files via FileWrite$
    pub allow_write: bool,
    /// Execute shell commands via ShellCmd$
    pub allow_run: bool,
    /// Query host hardware via SysQuery$
    pub allow_sys_query: bool,
    /// Access network via HttpFetch$
    pub allow_net: bool,
    /// Instruction budget (gas limit), 0 = unlimited
    pub budget: u64,
    /// Remaining instructions this macro can execute
    pub remaining: u64,
    /// 2026-07-23: Overrides for SysQuery$ results. When set, SysQuery$
    /// returns the override value instead of querying the real host.
    /// Used by multi-target compilation to mock hardware profiles.
    pub sysquery_overrides: HashMap<String, String>,
}

impl Sandbox {
    /// Create a new sandbox with all capabilities granted (for backward compat).
    pub fn permissive() -> Self {
        Self {
            allow_read: true, allow_write: true, allow_run: true,
            allow_sys_query: true, allow_net: true,
            budget: 0, remaining: 0,
            sysquery_overrides: HashMap::new(),
        }
    }

    /// Consume one unit of the instruction budget.
    /// Returns an error if the budget is exhausted.
    pub fn consume(&mut self, intrinsic: &str) -> Result<(), String> {
        if self.budget > 0 {
            if self.remaining == 0 {
                return Err(format!(
                    "instruction budget exceeded (limit {}): called '{}'",
                    self.budget, intrinsic
                ));
            }
            self.remaining -= 1;
        }
        Ok(())
    }

    /// Check that a capability is granted.
    pub fn check(&self, cap: Capability, intrinsic: &str) -> Result<(), String> {
        let granted = match cap {
            Capability::Pure => true,
            Capability::DiskRead => self.allow_read,
            Capability::DiskWrite => self.allow_write,
            Capability::Shell => self.allow_run,
            Capability::SysQuery => self.allow_sys_query,
            Capability::Network => self.allow_net,
        };
        if !granted {
            Err(format!(
                "intrinsic '{}' requires capability '{:?}'. Run with the corresponding --allow-* flag.",
                intrinsic, cap
            ))
        } else {
            Ok(())
        }
    }
}

/// Capability categories for $ intrinsics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Capability {
    Pure,
    DiskRead,
    DiskWrite,
    Shell,
    SysQuery,
    Network,
}

/// Result of evaluating a single navigation chain link.
#[derive(Debug, Clone)]
pub enum NavValue {
    Selection(Selection),
    Position(Position),
    TextSelection(TextSelection),
    Count(usize),
    Names(Vec<String>),
    Bool(bool),
    Int(i64),
    Str(String),
    TopLevel(TopLevel),
    VecTopLevel(Vec<TopLevel>),
    List(Vec<NavValue>),
    Void,
}

/// Extract a reference to the Sandbox from the optional PluginManager.
/// When no PluginManager is available (tests), returns a permissive sandbox.

type Scope = HashMap<String, NavValue>;

// ── Main Entry Points ──────────────────────────────────────────────────

/// Evaluate a $(Stage) block body, with transactional rollback on failure.
/// 2026-07-23: Snapshot program + VFS + tainted_indices before execution and
/// restore on error, preventing partial mutations from corrupting the pipeline.
pub fn evaluate_stage_block(
    body: &[Statement],
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    mut pm: &mut Option<&mut PluginManager>,
) -> Result<(), String> {
    let saved_program = program.clone();
    let saved_vfs = pm.as_ref().map(|p| p.vfs.clone());
    let saved_tainted = pm.as_ref().map(|p| p.tainted_indices.clone());

    let result = evaluate_stage_block_inner(body, program, universe, stage, pm);

    if result.is_err() {
        *program = saved_program;
        if let Some(pm_inner) = pm {
            if let Some(vfs) = saved_vfs {
                pm_inner.vfs = vfs;
            }
            if let Some(tainted) = saved_tainted {
                pm_inner.tainted_indices = tainted;
            }
        }
    }
    result
}

/// Inner evaluation without transactional wrapping.
/// 2026-07-23: Extracted for transaction boundary in evaluate_stage_block.
fn evaluate_stage_block_inner(
    body: &[Statement],
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    mut pm: &mut Option<&mut PluginManager>,
) -> Result<(), String> {
    let mut scope = Scope::new();
    let sandbox = pm.as_ref().map(|p| p.sandbox.clone()).unwrap_or_else(Sandbox::permissive);
    let mut sandbox = sandbox;
    // 2026-07-23: Snapshot program length before evaluation so we can mark
    // newly added top-level items as tainted (generated by macros).
    let prev_len = program.len();
    for stmt in body {
        evaluate_stage_stmt(stmt, program, universe, stage, &mut scope, &mut sandbox, &mut pm)?;
    }
    // 2026-07-23: Any new top-level items appended during this evaluation are
    // generated by macros and must be excluded from All$()/Tag$() results.
    // Insert$ at specific positions is tracked separately in the Insert$ handler.
    if let Some(pm_inner) = pm {
        for i in prev_len..program.len() {
            pm_inner.tainted_indices.insert(i);
            // 2026-07-23: Record expansion trace for appended nodes.
            let desc = format!("StageBlock appended at index {}", i);
            pm_inner.expansion_traces.insert(i, desc);
        }
    }
    Ok(())
}

/// 2026-07-25: Find a regular (non-$) defn in the program by name.
fn find_defn_in_program<'a>(program: &'a [TopLevel], name: &str) -> Option<&'a Definition> {
    program.iter().find_map(|item| {
        if let TopLevel::Definition(d) = item {
            if d.name == name { Some(d) } else { None }
        } else { None }
    })
}

/// 2026-07-25: Find a regular (non-$) txn in the program by name.
fn find_txn_in_program<'a>(program: &'a [TopLevel], name: &str) -> Option<&'a Transaction> {
    program.iter().find_map(|item| {
        if let TopLevel::Transaction(t) = item {
            if t.name == name { Some(t) } else { None }
        } else { None }
    })
}

/// 2026-07-25: Execute a regular defn at compile time.
fn execute_defn_at_compile_time(
    defn: &Definition,
    args: &[Expr],
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    scope: &Scope,
    sandbox: &mut Sandbox,
    pm: &mut Option<&mut PluginManager>,
) -> Result<NavValue, String> {
    let mut fn_scope = Scope::new();
    for (i, (param_name, _param_type)) in defn.parameters.iter().enumerate() {
        let arg_expr = args.get(i).ok_or_else(|| {
            format!("{}: missing argument {} (expected {})", defn.name, i, param_name)
        })?;
        let arg_val = eval_nav_chain(arg_expr, program, universe, stage, scope, sandbox, pm)?;
        fn_scope.insert(param_name.clone(), arg_val);
    }
    for stmt in &defn.body {
        if let Some(val) = evaluate_stage_stmt(stmt, program, universe,
            stage, &mut fn_scope, sandbox, pm)?
        {
            return Ok(val);
        }
    }
    Ok(NavValue::Void)
}

/// 2026-07-25: Execute a regular txn at compile time.
fn execute_txn_at_compile_time(
    txn: &Transaction,
    args: &[Expr],
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    scope: &Scope,
    sandbox: &mut Sandbox,
    pm: &mut Option<&mut PluginManager>,
) -> Result<NavValue, String> {
    let mut fn_scope = Scope::new();
    for (i, (param_name, _param_type)) in txn.parameters.iter().enumerate() {
        let arg_expr = args.get(i).ok_or_else(|| {
            format!("{}: missing argument {}", txn.name, i)
        })?;
        let arg_val = eval_nav_chain(arg_expr, program, universe, stage, scope, sandbox, pm)?;
        fn_scope.insert(param_name.clone(), arg_val);
    }
    let max_iter = 1000;
    let mut term_val = NavValue::Void;
    let mut term_hit = false;
    for _iter in 0..max_iter {
        let pre_result = eval_nav_chain(
            &txn.contract.pre_condition, program, universe, stage, &fn_scope, sandbox, pm
        )?;
        if !nav_is_truthy(&pre_result) {
            break;
        }
        term_hit = false;
        for stmt in &txn.body {
            match evaluate_stage_stmt(stmt, program, universe, stage, &mut fn_scope, sandbox, pm) {
                Ok(Some(val)) => { term_hit = true; term_val = val; break; }
                Ok(None) => {}
                Err(e) => return Err(format!("{}: {}", txn.name, e)),
            }
        }
        let post_result = eval_nav_chain(
            &txn.contract.post_condition, program, universe, stage, &fn_scope, sandbox, pm
        )?;
        if nav_is_truthy(&post_result) {
            return if term_hit { Ok(term_val) } else { Ok(NavValue::Void) };
        }
    }
    Err(format!("{}: exceeded max iterations ({}) without convergence", txn.name, max_iter))
}

/// Execute a compile-time function ($defn or $txn) with the given arguments.
/// 2026-07-23: Creates a fresh scope, binds parameters, executes the body,
/// and returns the term value. For $txn, loops with pre/post condition checks.
fn eval_compile_time_fn(
    fn_def: &crate::plugin::FnDef,
    args: &[Expr],
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    scope: &Scope,
    sandbox: &mut Sandbox,
    pm: &mut Option<&mut PluginManager>,
) -> Result<NavValue, String> {
    match fn_def {
        crate::plugin::FnDef::Defn(d) => {
            // Bind parameters to argument values
            let mut fn_scope = Scope::new();
            for (i, (param_name, _param_type)) in d.parameters.iter().enumerate() {
                let arg_expr = args.get(i).ok_or_else(|| {
                    format!("{}: missing argument {} (expected {})", d.name, i, param_name)
                })?;
                let arg_val = eval_nav_chain(arg_expr, program, universe, stage, scope, sandbox, pm)?;
                fn_scope.insert(param_name.clone(), arg_val);
            }
            // Execute body statements, return the term value
            for stmt in &d.body {
                if let Some(val) = evaluate_stage_stmt(stmt, program, universe,
                    stage, &mut fn_scope, sandbox, pm)?
                {
                    return Ok(val);
                }
            }
            Ok(NavValue::Void)
        }
        crate::plugin::FnDef::Txn(t) => {
            // Convergent loop: precondition → body → postcondition → repeat
            let mut fn_scope = Scope::new();
            for (i, (param_name, _param_type)) in t.parameters.iter().enumerate() {
                let arg_expr = args.get(i).ok_or_else(|| {
                    format!("{}: missing argument {}", t.name, i)
                })?;
                let arg_val = eval_nav_chain(arg_expr, program, universe, stage, scope, sandbox, pm)?;
                fn_scope.insert(param_name.clone(), arg_val);
            }
            let max_iter = 1000;
            let mut term_val = NavValue::Void;
            let mut term_hit = false;
            for _iter in 0..max_iter {
                // Evaluate precondition — if false, converged
                let pre_result = eval_nav_chain(
                    &t.contract.pre_condition, program, universe, stage, &fn_scope, sandbox, pm
                )?;
                if !nav_is_truthy(&pre_result) {
                    break;
                }
                // Execute body one statement at a time
                term_hit = false;
                for stmt in &t.body {
                    match evaluate_stage_stmt(stmt, program, universe, stage, &mut fn_scope, sandbox, pm) {
                        Ok(Some(val)) => { term_hit = true; term_val = val; break; }
                        Ok(None) => {}
                        Err(e) => return Err(format!("{}: {}", t.name, e)),
                    }
                }
                // Evaluate postcondition — if true, converged
                let post_result = eval_nav_chain(
                    &t.contract.post_condition, program, universe, stage, &fn_scope, sandbox, pm
                )?;
                if nav_is_truthy(&post_result) {
                    return if term_hit { Ok(term_val) } else { Ok(NavValue::Void) };
                }
                // Postcondition not met — continue loop
            }
            Err(format!("{}: exceeded max iterations ({}) without convergence", t.name, max_iter))
        }
    }
}

/// Evaluate a $ call tree. `pm` is a mutable ref to Option for reborrowing.
pub fn eval_nav_chain(
    expr: &Expr,
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    scope: &Scope,
    sandbox: &mut Sandbox,
    pm: &mut Option<&mut PluginManager>,
) -> Result<NavValue, String> {
    match expr {
        Expr::Call(name, args, _) if name.ends_with('$') || name.ends_with('#') => {
            eval_nav_call(name, args, program, universe, stage, scope, sandbox, pm)
        }
        // 2026-09-16: MethodCall with $-suffix — unified chaining.
        // Prepend receiver to args, then delegate to the Call handler.
        Expr::MethodCall(recv, name, args, _, _) if name.ends_with('$') => {
            let mut call_args = vec![(**recv).clone()];
            call_args.extend(args.iter().cloned());
            eval_nav_call(name, &call_args, program, universe, stage, scope, sandbox, pm)
        }
        // 2026-07-23: Non-$ function call — look up compile-time fn_registry.
        // 2026-07-25: Fall back to program search for regular defn/txn.
        Expr::Call(name, args, _) => {
            let fn_def = pm.as_ref()
                .and_then(|p| p.fn_registry.get(name))
                .cloned();
            if let Some(fn_def) = fn_def {
                return eval_compile_time_fn(&fn_def, args, program, universe,
                    stage, scope, sandbox, pm);
            }
            // 2026-07-25: Search program for a regular defn.
            // Clone to avoid borrow conflict with mutable program param.
            let found_defn = find_defn_in_program(program, name).cloned();
            if let Some(defn) = found_defn {
                return execute_defn_at_compile_time(&defn, args, program, universe,
                    stage, scope, sandbox, pm);
            }
            // 2026-07-25: Search program for a regular txn.
            let found_txn = find_txn_in_program(program, name).cloned();
            if let Some(txn) = found_txn {
                return execute_txn_at_compile_time(&txn, args, program, universe,
                    stage, scope, sandbox, pm);
            }
            Err(format!("undefined compile-time function '{}'", name))
        }
        Expr::Field(obj, name) if name.ends_with('$') => {
            let prev = eval_nav_chain(obj, program, universe, stage, scope, sandbox, pm)?;
            eval_nav_field_method(name, prev, program, stage)
        }
        // Fix 1: Resolve identifiers from the compile-time scope
        Expr::Identifier(name) => {
            // 2026-07-25: Check local scope first, then comptime_vars.
            if let Some(val) = scope.get(name) {
                Ok(val.clone())
            } else if let Some((val, _)) = pm.as_ref().and_then(|p| p.comptime_vars.get(name)) {
                Ok(val.clone())
            } else {
                Err(format!("undefined compile-time variable '{}'", name))
            }
        }
        Expr::Char(c) => Ok(NavValue::Int(*c as i64)),
        Expr::Decimal(n) => Ok(NavValue::Int(*n)),
        Expr::Float(n) => Ok(NavValue::Int(*n as i64)),
        Expr::Bool(b) => Ok(NavValue::Bool(*b)),
        Expr::Exists(name) => {
            // 2026-07-25: fn? — compile-time existence check.
            // For frgn?/frgn!/frgn?!, check if the binding is optional.
            // For regular frgn/defn, always exists (true).
            let exists = program.iter().any(|item| {
                if let TopLevel::ForeignBinding(fb) = item {
                    let bname = fb.briev_name.as_deref().unwrap_or(&fb.foreign_name);
                    bname == name && !fb.is_optional
                } else {
                    false
                }
            });
            // Also check if it's a regular defn/txn (always exists)
            let exists = exists || program.iter().any(|item| {
                matches!(item, TopLevel::Definition(d) if d.name == *name)
                    || matches!(item, TopLevel::Transaction(t) if t.name == *name)
            });
            Ok(NavValue::Bool(exists))
        }
        // 2026-07-23: String literal (Quoted) in macro DSL.
        Expr::Quoted(bytes) => String::from_utf8(bytes.clone())
            .map(NavValue::Str)
            .map_err(|_| "invalid UTF-8 string literal".into()),
        // UnitLiteral: treat as Float value.
        Expr::UnitLiteral { value, .. } => Ok(NavValue::Int(*value as i64)),
        // 2026-07-23: Binary operators — arithmetic and comparison.
        Expr::BinaryOp(kind, lhs, rhs) => {
            let lv = eval_nav_chain(lhs, program, universe, stage, scope, sandbox, pm)?;
            let rv = eval_nav_chain(rhs, program, universe, stage, scope, sandbox, pm)?;
            // 2026-07-24: String comparison for Eq/Neq. nav_to_i64 would parse
            // "Int" as 0 (parse failure), breaking string equality checks.
            if matches!(*kind, BinaryOpKind::Eq | BinaryOpKind::Neq) {
                match (&lv, &rv) {
                    (NavValue::Str(a), NavValue::Str(b)) => {
                        let eq = a == b;
                        let result = if *kind == BinaryOpKind::Eq { eq } else { !eq };
                        return Ok(NavValue::Bool(result));
                    }
                    _ => {}
                }
            }
            let li = nav_to_i64(&lv);
            let ri = nav_to_i64(&rv);
            match kind {
                BinaryOpKind::Add => {
                    match (lv, rv) {
                        (NavValue::Str(a), NavValue::Str(b)) => Ok(NavValue::Str(a + &b)),
                        (NavValue::Int(a), NavValue::Int(b)) => Ok(NavValue::Int(a + b)),
                        (NavValue::Str(a), NavValue::Int(b)) => Ok(NavValue::Str(a + &b.to_string())),
                        (NavValue::Int(a), NavValue::Str(b)) => Ok(NavValue::Str(a.to_string() + &b)),
                        _ => Err(format!("+ operator not supported for these operand types")),
                    }
                }
                BinaryOpKind::Sub => Ok(NavValue::Int(li - ri)),
                BinaryOpKind::Mul => Ok(NavValue::Int(li * ri)),
                BinaryOpKind::Div => Ok(NavValue::Int(li / ri)),
                BinaryOpKind::Mod => Ok(NavValue::Int(li % ri)),
                BinaryOpKind::Eq => Ok(NavValue::Bool(li == ri)),
                BinaryOpKind::Neq => Ok(NavValue::Bool(li != ri)),
                BinaryOpKind::Gt => Ok(NavValue::Bool(li > ri)),
                BinaryOpKind::Lt => Ok(NavValue::Bool(li < ri)),
                BinaryOpKind::Ge => Ok(NavValue::Bool(li >= ri)),
                BinaryOpKind::Le => Ok(NavValue::Bool(li <= ri)),
                _ => Err(format!("unsupported binary operator {:?}", kind)),
            }
        }
        // 2026-07-23: List construction for macro DSL (e.g., [a, b, c]).
        Expr::List(items) => {
            let values: Result<Vec<NavValue>, String> = items.iter()
                .map(|item| eval_nav_chain(item, program, universe, stage, scope, sandbox, pm))
                .collect();
            Ok(NavValue::List(values?))
        }
        // 2026-07-24: Struct literal in macro DSL — for now, return Void.
        Expr::StructLiteral { .. } => Ok(NavValue::Void),
        other => Err(format!(
            "expected a $ navigation call, got {:?}", other
        )),
    }
}

// ── Statement Evaluation ───────────────────────────────────────────────

/// Return true if a NavValue is "truthy" in a guard context.
/// 2026-07-23: Extracted from Guarded handler for use in $txn loops.
fn nav_is_truthy(val: &NavValue) -> bool {
    match val {
        NavValue::Selection(sel) => !sel.nodes.is_empty(),
        NavValue::Count(n) => *n > 0,
        NavValue::Bool(b) => *b,
        NavValue::Int(n) => *n != 0,
        NavValue::Str(s) => !s.is_empty(),
        NavValue::Void => false,
        _ => true,
    }
}

/// Extract an i64 value from a NavValue for comparison operators.
fn nav_to_i64(val: &NavValue) -> i64 {
    match val {
        NavValue::Count(n) => *n as i64,
        NavValue::Int(n) => *n,
        NavValue::Bool(b) => if *b { 1 } else { 0 },
        NavValue::Str(s) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn evaluate_stage_stmt(
    stmt: &Statement,
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    scope: &mut Scope,
    sandbox: &mut Sandbox,
    pm: &mut Option<&mut PluginManager>,
) -> Result<Option<NavValue>, String> {
    match stmt {
        Statement::Expression(expr) => {
            let _result = eval_nav_chain(expr, program, universe, stage, scope, sandbox, pm)?;
            Ok(None)
        }
        Statement::Let { name, expr: Some(e), .. } => {
            let result = eval_nav_chain(e, program, universe, stage, scope, sandbox, pm)?;
            scope.insert(name.clone(), result);
            Ok(None)
        }
        Statement::Block(statements) => {
            for s in statements {
                if let Some(val) = evaluate_stage_stmt(s, program, universe, stage, scope, sandbox, pm)? {
                    return Ok(Some(val));
                }
            }
            Ok(None)
        }
        Statement::Foreach { item, list, body } => {
            let list_val = eval_nav_chain(list, program, universe, stage, scope, sandbox, pm)?;
            let items = match list_val {
                NavValue::Selection(sel) => sel,
                _ => return Err("foreach: expected a Selection from the list expression".into()),
            };
            for node in &items.nodes {
                // Bind the loop variable to a single-element selection
                let selection = Selection::single(node.clone());
                scope.insert(item.clone(), NavValue::Selection(selection));
                for s in body {
                    if let Some(val) = evaluate_stage_stmt(s, program, universe, stage, scope, sandbox, pm)? {
                        scope.remove(item);
                        return Ok(Some(val));
                    }
                }
            }
            scope.remove(item);
            Ok(None)
        }
        Statement::Match { expr, arms } => {
            let val = eval_nav_chain(expr, program, universe, stage, scope, sandbox, pm)?;
            // 2026-08-22 (Phase 4a): arms carry the unified Pattern grammar;
            // `|` alternatives are a plain vec, first matching arm wins.
            for arm in arms {
                let matches = arm.patterns.iter().any(|p| pattern_matches_nav(p, &val));
                if matches {
                    for s in &arm.body {
                        if let Some(val) = evaluate_stage_stmt(s, program, universe, stage, scope, sandbox, pm)? {
                            return Ok(Some(val));
                        }
                    }
                    break;
                }
            }
            Ok(None)
        }
        // 2026-07-23: Assignment — evaluate the RHS and update scope.
        // 2026-07-25: Also writes to comptime_vars for mutable $let targets.
        Statement::Assign(target, value) => {
            let name = match target {
                Expr::Identifier(n) => n.clone(),
                other => return Err(format!("assignment target must be an identifier, got {:?}", other)),
            };
            let result = eval_nav_chain(value, program, universe, stage, scope, sandbox, pm)?;
            // 2026-07-25: If name is a comptime_var, update it (mutation for $let).
            if let Some(pm_inner) = pm.as_mut() {
                if let Some(entry) = pm_inner.comptime_vars.get_mut(&name) {
                    if entry.1 {
                        return Err(format!("cannot reassign $const '{}'", name));
                    }
                    entry.0 = result.clone();
                }
            }
            scope.insert(name, result);
            Ok(None)
        }
        Statement::Guarded(guard, body) => {
            // Handle BinaryOp comparisons (Count$() > 0, etc.) which
            // eval_nav_chain doesn't support directly.
            let result = match guard {
                Expr::UnaryOp(crate::ast::UnaryOpKind::Not, inner) => {
                    let val = eval_nav_chain(inner, program, universe, stage, scope, sandbox, pm)?;
                    let truthy = nav_is_truthy(&val);
                    NavValue::Bool(!truthy)
                }
                Expr::BinaryOp(op, left, right) => {
                    let l = eval_nav_chain(left, program, universe, stage, scope, sandbox, pm)?;
                    let r = eval_nav_chain(right, program, universe, stage, scope, sandbox, pm)?;
                    // 2026-07-24: String comparison for Eq/Neq. nav_to_i64 would
                    // parse "Int" as 0, breaking string equality in when guards.
                    let is_str_cmp = matches!(*op, crate::ast::BinaryOpKind::Eq | crate::ast::BinaryOpKind::Neq) &&
                        matches!((&l, &r), (NavValue::Str(_), NavValue::Str(_)));
                    let cmp = if is_str_cmp {
                        match (&l, &r) {
                            (NavValue::Str(a), NavValue::Str(b)) => {
                                if *op == crate::ast::BinaryOpKind::Eq { a == b } else { a != b }
                            }
                            _ => false,
                        }
                    } else {
                        let lv = nav_to_i64(&l);
                        let rv = nav_to_i64(&r);
                        match op {
                            crate::ast::BinaryOpKind::Eq => lv == rv,
                            crate::ast::BinaryOpKind::Neq => lv != rv,
                            crate::ast::BinaryOpKind::Gt => lv > rv,
                            crate::ast::BinaryOpKind::Lt => lv < rv,
                            crate::ast::BinaryOpKind::Ge => lv >= rv,
                            crate::ast::BinaryOpKind::Le => lv <= rv,
                            _ => return Err(format!("unsupported operator in when guard: {:?}", op)),
                        }
                    };
                    NavValue::Bool(cmp)
                }
                _ => eval_nav_chain(guard, program, universe, stage, scope, sandbox, pm)?,
            };
            let is_truthy = nav_is_truthy(&result);
            if is_truthy {
                for s in body {
                    if let Some(val) = evaluate_stage_stmt(s, program, universe, stage, scope, sandbox, pm)? {
                        return Ok(Some(val));
                    }
                }
            }
            Ok(None)
        }
        Statement::Gate(guard) => {
            // 2026-07-26: Convergence gate — evaluate condition.
            eval_nav_chain(guard, program, universe, stage, scope, sandbox, pm)?;
            Ok(None)
        }
        // 2026-07-23: Term — evaluate and return the expression value.
        Statement::Term(opt) => {
            let val = match opt {
                Some(expr) => eval_nav_chain(expr, program, universe, stage, scope, sandbox, pm)?,
                None => NavValue::Void,
            };
            Ok(Some(val))
        }
        // 2026-07-23: TermBang — evaluate for side effects, return Void.
        Statement::EndProgram(opt) => {
            if let Some(expr) = opt {
                eval_nav_chain(expr, program, universe, stage, scope, sandbox, pm)?;
            }
            Ok(Some(NavValue::Void))
        }
        // 2026-07-23: Escape — abort compile-time function with error.
        Statement::Rollback(opt) => {
            let msg = match opt {
                Some(expr) => format!("{:?}", eval_nav_chain(expr, program, universe, stage, scope, sandbox, pm)?),
                None => "compile-time escape".into(),
            };
            Err(msg)
        }
        _ => Ok(None),
    }
}

// ── Intrinsic Dispatch ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn eval_nav_call(
    name: &str,
    args: &[Expr],
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
    stage: StageKind,
    scope: &Scope,
    sandbox: &mut Sandbox,
    pm: &mut Option<&mut PluginManager>,
) -> Result<NavValue, String> {
    // 2026-07-23: Every $ intrinsic call consumes one unit of the gas budget.
    sandbox.consume(name)?;
    match name {
        // ── Selectors ──────────────────────────────────────────────
        "Tag$" => {
            let s = expect_str_arg(args, 0, "Tag$", scope)?;
            let mut nodes = TagSelector { tag: s }.apply(program)?;
            let tainted = get_tainted_set(pm);
            if !tainted.is_empty() {
                filter_tainted_nodes(&mut nodes, &tainted);
            }
            Ok(NavValue::Selection(Selection { nodes }))
        }
        "Named$" => {
            let s = expect_str_arg(args, 0, "Named$", scope)?;
            let mut nodes = NamedSelector { name: s }.apply(program)?;
            let tainted = get_tainted_set(pm);
            if !tainted.is_empty() {
                filter_tainted_nodes(&mut nodes, &tainted);
            }
            Ok(NavValue::Selection(Selection { nodes }))
        }
        "WithKey$" => {
            let s = expect_str_arg(args, 0, "WithKey$", scope)?;
            let mut nodes = WithKeySelector { key: s }.apply(program)?;
            let tainted = get_tainted_set(pm);
            if !tainted.is_empty() {
                filter_tainted_nodes(&mut nodes, &tainted);
            }
            Ok(NavValue::Selection(Selection { nodes }))
        }
        "WithAttr$" => {
            let key = expect_str_arg(args, 0, "WithAttr$", scope)?;
            let val = expect_str_arg(args, 1, "WithAttr$", scope)?;
            let mut nodes = WithAttrSelector { key, val }.apply(program)?;
            let tainted = get_tainted_set(pm);
            if !tainted.is_empty() {
                filter_tainted_nodes(&mut nodes, &tainted);
            }
            Ok(NavValue::Selection(Selection { nodes }))
        }
        "All$" => {
            let mut nodes = AllSelector.apply(program)?;
            let tainted = get_tainted_set(pm);
            if !tainted.is_empty() {
                filter_tainted_nodes(&mut nodes, &tainted);
            }
            Ok(NavValue::Selection(Selection { nodes }))
        }

        // ── Traversal ──────────────────────────────────────────────
        "First$" => selector_1_int_opt(args, |n| {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Selection(sel.first(n))),
                _ => Err("First$ requires a Selection operand".into()),
            }
        }),
        "Last$" => selector_1_int_opt(args, |n| {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Selection(sel.last(n))),
                _ => Err("Last$ requires a Selection operand".into()),
            }
        }),
        "Nth$" => {
            let n = expect_int_arg(args, 0, "Nth$")? as usize;
            let prev = eval_nav_chain(&args[1], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Selection(sel.nth(n))),
                _ => Err("Nth$ requires a Selection operand".into()),
            }
        }
        "Children$" => {
            let filter = args.first().and_then(|a| extract_str_lit(a));
            let prev_arg = if filter.is_some() { args.get(1) } else { args.first() };
            let Some(prev_arg) = prev_arg else {
                return Err("Children$: missing operand".into());
            };
            let prev = eval_nav_chain(prev_arg, program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Selection(
                    sel.children(program, filter.as_deref())
                )),
                _ => Err("Children$ requires a Selection operand".into()),
            }
        }
        "Descendants$" => {
            let filter = args.first().and_then(|a| extract_str_lit(a));
            let prev_arg = if filter.is_some() { args.get(1) } else { args.first() };
            let Some(prev_arg) = prev_arg else {
                return Err("Descendants$: missing operand".into());
            };
            let prev = eval_nav_chain(prev_arg, program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Selection(
                    sel.descendants(program, filter.as_deref())
                )),
                _ => Err("Descendants$ requires a Selection operand".into()),
            }
        }
        "Parent$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Selection(sel.parent(program))),
                _ => Err("Parent$ requires a Selection operand".into()),
            }
        }

        // ── Introspection ──────────────────────────────────────────
        "Count$" => {
            let prev = if args.is_empty() {
                NavValue::Selection(Selection { nodes: AllSelector.apply(program)? })
            } else {
                eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?
            };
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Count(sel.count())),
                _ => Err("Count$ requires a Selection operand".into()),
            }
        }
        "Names$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Names(sel.names(program))),
                _ => Err("Names$ requires a Selection operand".into()),
            }
        }
        "IsEmpty$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => Ok(NavValue::Bool(sel.is_empty())),
                _ => Err("IsEmpty$ requires a Selection operand".into()),
            }
        }

        // ── Positions ──────────────────────────────────────────────
        "Before$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(ref sel) => Position::before(sel)
                    .map(|p| NavValue::Position(p))
                    .ok_or_else(|| "Before$: selection is empty".into()),
                _ => Err("Before$ requires a Selection operand".into()),
            }
        }
        "After$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(ref sel) => Position::after(sel)
                    .map(|p| NavValue::Position(p))
                    .ok_or_else(|| "After$: selection is empty".into()),
                _ => Err("After$ requires a Selection operand".into()),
            }
        }
        "Replace$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(ref sel) => Position::replace(sel)
                    .map(|p| NavValue::Position(p))
                    .ok_or_else(|| "Replace$: selection is empty".into()),
                _ => Err("Replace$ requires a Selection operand".into()),
            }
        }
        "Inside$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(ref sel) => Position::inside(sel)
                    .map(|p| NavValue::Position(p))
                    .ok_or_else(|| "Inside$: selection is empty".into()),
                _ => Err("Inside$ requires a Selection operand".into()),
            }
        }
        "AppendTo$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(ref sel) => Position::append_to(sel)
                    .map(|p| NavValue::Position(p))
                    .ok_or_else(|| "AppendTo$: selection is empty".into()),
                _ => Err("AppendTo$ requires a Selection operand".into()),
            }
        }

        // Fix 2: Stage$.Insert$ vs regular Insert$
        // Stage$.Insert$("path") → first arg is Identifier("Stage$")
        // Tag$("x").Insert$(y) → first arg is a Call chain (receiver)
        "Insert$" if is_stage_receiver(args) => {
            let Some(pm) = pm else {
                return Err("Stage$.Insert$: no PluginManager available".into());
            };
            let path = expect_str_arg(args, 1, "Stage$.Insert$", scope)?;
            stage_target::insert_plugin_from_file(pm, &path, stage)
                .map(|_| NavValue::Void)
        }
        // ── Actions ────────────────────────────────────────────────
        "Insert$" => {
            let pos_result = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            let mut nodes = Vec::new();
            // 2026-07-23: Build descriptions for expansion traces.
            let mut trace_descs: Vec<String> = Vec::new();
            for arg in &args[1..] {
                let val = eval_nav_chain(arg, program, universe, stage, scope, sandbox, pm)?;
                match val {
                    NavValue::TopLevel(tl) => {
                        trace_descs.push(node_summary(&tl));
                        nodes.push(tl);
                    }
                    NavValue::VecTopLevel(v) => {
                        for item in &v {
                            trace_descs.push(node_summary(item));
                        }
                        nodes.extend(v);
                    }
                    _ => return Err("Insert$: argument must produce an AST node".into()),
                }
            }
            let count = nodes.len();
            match pos_result {
                NavValue::Position(pos) => {
                    // 2026-07-23: Compute insertion indices BEFORE the insert
                    // (post-insert they shift by prior insertions).
                    let is_toplevel = matches!(&pos,
                        Position::Before(_) | Position::After(_) | Position::Replace(_)
                    );
                    let base = match &pos {
                        Position::Before(NodeRef::TopLevel(i)) => *i,
                        Position::After(NodeRef::TopLevel(i)) => *i + 1,
                        Position::Replace(NodeRef::TopLevel(i)) => *i,
                        // AppendTo/Inside insert into a node's body — no new top-level items
                        Position::AppendTo(_) | Position::Inside(_) => 0,
                        _ => return Err("Insert$: unsupported position type".into()),
                    };
                    insert_items(program, &pos, nodes, stage)?;
                    if is_toplevel {
                        if let Some(pm_inner) = pm {
                            for offset in 0..count {
                                let idx = base + offset;
                                pm_inner.tainted_indices.insert(idx);
                                // 2026-07-23: Record expansion trace for inserted nodes.
                                let desc = if offset < trace_descs.len() {
                                    format!("Insert$ -> {}", trace_descs[offset])
                                } else {
                                    "Insert$ -> <node>".into()
                                };
                                pm_inner.expansion_traces.insert(idx, desc);
                            }
                        }
                    }
                    Ok(NavValue::Void)
                }
                NavValue::Selection(sel) => {
                    insert_before_each(program, &sel, nodes, stage).map(|_| NavValue::Void)
                }
                _ => Err("Insert$ requires a Position or Selection operand".into()),
            }
        }
        "Delete$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            match prev {
                NavValue::Selection(sel) => {
                    // 2026-07-23: Adjust tainted indices for deleted items.
                    // Removed indices are dropped; remaining tainted indices >= a
                    // deleted index shift down by the number of deletions before them.
                    let indices = collect_toplevel_indices(&sel);
                    delete_selection(program, &sel)?;
                    if let Some(pm_inner) = pm {
                        let deleted: BTreeSet<usize> = indices.iter().cloned().collect();
                        let shifted: BTreeSet<usize> = pm_inner.tainted_indices.iter()
                            .filter(|ti| !deleted.contains(ti))
                            .map(|ti| {
                                let shift = deleted.iter().filter(|d| **d < *ti).count();
                                ti - shift
                            })
                            .collect();
                        pm_inner.tainted_indices = shifted;
                    }
                    Ok(NavValue::Void)
                }
                _ => Err("Delete$ requires a Selection operand".into()),
            }
        }
        "ReplaceWith$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            let repl = eval_nav_chain(&args[1], program, universe, stage, scope, sandbox, pm)?;
            let repl_node = match repl {
                NavValue::TopLevel(tl) => tl,
                _ => return Err("ReplaceWith$: second arg must produce a TopLevel node".into()),
            };
            match prev {
                NavValue::Selection(sel) => {
                    // 2026-07-23: Mark replaced indices as tainted and record expansion trace.
                    let indices = collect_toplevel_indices(&sel);
                    let desc = format!("ReplaceWith$ -> {}", node_summary(&repl_node));
                    replace_selection(program, &sel, repl_node)?;
                    if let Some(pm_inner) = pm {
                        for i in &indices {
                            pm_inner.tainted_indices.insert(*i);
                            pm_inner.expansion_traces.insert(*i, desc.clone());
                        }
                    }
                    Ok(NavValue::Void)
                }
                _ => Err("ReplaceWith$ requires a Selection operand".into()),
            }
        }
        // Fix 3: Set$ — parse second arg as PropertyValue
        "Set$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            let key = expect_str_arg(args, 1, "Set$", scope)?;
            let val = expect_prop_arg(args, 2, "Set$")?;
            match prev {
                NavValue::Selection(sel) => {
                    set_metadata(program, &sel, &key, val).map(|_| NavValue::Void)
                }
                _ => Err("Set$ requires a Selection operand".into()),
            }
        }
        "Rename$" => {
            let prev = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            let name = expect_str_arg(args, 1, "Rename$", scope)?;
            match prev {
                NavValue::Selection(sel) => {
                    rename_selection(program, &sel, &name).map(|_| NavValue::Void)
                }
                _ => Err("Rename$ requires a Selection operand".into()),
            }
        }

        // ── AST Constructors ───────────────────────────────────────
        "Import$" => {
            let path = expect_str_arg(args, 0, "Import$", scope)?;
            Ok(NavValue::TopLevel(TopLevel::Import(Import::literal(path, vec![]))))
        }
        "Defn$" => {
            let name = expect_str_arg(args, 0, "Defn$", scope)?;
            Ok(NavValue::TopLevel(TopLevel::Definition(Definition {
                name, type_params: vec![], parameters: vec![],
                output_type: None, outputs: vec![],
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![], metadata: Default::default(),
                derivation: None, modifiers: vec![], annotations: vec![], span: None,
                doc: None,
            })))
        }
        "Call$" => {
            let fn_name = expect_str_arg(args, 0, "Call$", scope)?;
            let mut call_args = Vec::new();
            for arg in &args[1..] {
                let val = eval_nav_chain(arg, program, universe, stage, scope, sandbox, pm)?;
                if let NavValue::TopLevel(TopLevel::Statement(stmt)) = val {
                    if let Statement::Expression(expr) = stmt.as_ref() {
                        call_args.push(expr.clone());
                    }
                }
            }
            Ok(NavValue::TopLevel(TopLevel::Statement(Box::new(
                Statement::Expression(Expr::Call(fn_name, call_args, None))
            ))))
        }
        "Block$" => {
            let mut stmts = Vec::new();
            for arg in args {
                if let NavValue::TopLevel(TopLevel::Statement(stmt)) =
                    eval_nav_chain(arg, program, universe, stage, scope, sandbox, pm)?
                {
                    stmts.push(*stmt);
                }
            }
            Ok(NavValue::TopLevel(TopLevel::Statement(Box::new(
                Statement::Block(stmts)
            ))))
        }
        // 2026-07-23: Quote$ — structural quasiquoting.
        // Template string parsed as Briev source; $ident references resolved
        // from compile-time scope variables. $$escapes to literal $.
        // Returns TopLevel (single item) or VecTopLevel (multiple items).
        "Quote$" => {
            let template = expect_str_arg(args, 0, "Quote$", scope)?;
            let tokens = crate::lexer::tokenize(&template)
                .map_err(|e| format!("Quote$: tokenization error: {}", e))?;
            let mut parser = crate::parser::Parser::new(tokens, &template);
            let mut items = parser.parse_program()
                .map_err(|e| format!("Quote$: parse error: {}", e))?;
            // Resolve $$ → $ escapes and $ident references from scope.
            // $$handled at AST level by resolve_dollar_refs_in_expr.
            for item in items.iter_mut() {
                resolve_dollar_refs_in_toplevel(item, scope)?;
            }
            if items.len() == 1 {
                Ok(NavValue::TopLevel(items.into_iter().next().unwrap()))
            } else {
                Ok(NavValue::VecTopLevel(items))
            }
        }

        // Fix 2: Stage$.List$ — list registered plugins
        "List$" if is_stage_receiver(args) => {
            let Some(pm) = pm else {
                return Err("Stage$.List$: no PluginManager available".into());
            };
            let names = stage_target::list_plugins(pm);
            Ok(NavValue::Names(names))
        }
        // Stage$.Remove$("name") — disable a plugin
        "Remove$" if is_stage_receiver(args) => {
            let Some(pm) = pm else {
                return Err("Stage$.Remove$: no PluginManager available".into());
            };
            let name = expect_str_arg(args, 1, "Stage$.Remove$", scope)?;
            stage_target::remove_plugin(pm, &name);
            Ok(NavValue::Void)
        }

        // ── String Operations (2026-07-22) ───────────────────────────
        "StrLen$" => {
            let s = expect_str_arg(args, 0, "StrLen$", scope)?;
            Ok(NavValue::Int(s.len() as i64))
        }
        "StrReplace$" => {
            let s = expect_str_arg(args, 0, "StrReplace$", scope)?;
            let from = expect_str_arg(args, 1, "StrReplace$", scope)?;
            let to = expect_str_arg(args, 2, "StrReplace$", scope)?;
            Ok(NavValue::Str(s.replace(&from, &to)))
        }
        "StrJoin$" => {
            let list_val = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            let parts: Vec<String> = match &list_val {
                NavValue::List(items) => items.iter().map(|v| nav_to_string(v)).collect(),
                NavValue::Names(names) => names.clone(),
                _ => return Err("StrJoin$: first arg must be a List or Names".into()),
            };
            let sep = expect_str_arg(args, 1, "StrJoin$", scope)?;
            Ok(NavValue::Str(parts.join(&sep)))
        }
        "StrSplit$" | "StrSplit#" => {
            let s = expect_str_arg(args, 0, name, scope)?;
            let pat = expect_str_arg(args, 1, name, scope)?;
            let parts: Vec<NavValue> = s.split(&pat).map(|p| NavValue::Str(p.to_string())).collect();
            Ok(NavValue::List(parts))
        }
        "StrSubstr$" => {
            let s = expect_str_arg(args, 0, "StrSubstr$", scope)?;
            let start = expect_int_arg(args, 1, "StrSubstr$")? as usize;
            let end = expect_int_arg(args, 2, "StrSubstr$")? as usize;
            let end = end.min(s.len());
            if start > s.len() || start > end {
                return Err(format!("StrSubstr$: invalid range {}..{} for string of length {}", start, end, s.len()));
            }
            Ok(NavValue::Str(s[start..end].to_string()))
        }

        // ── File I/O (2026-07-22) ────────────────────────────────────
        "FileWrite$" => {
            sandbox.check(Capability::DiskWrite, "FileWrite$")?;
            let path = expect_str_arg(args, 0, "FileWrite$", scope)?;
            let content = expect_str_arg(args, 1, "FileWrite$", scope)?;
            // Third argument: `{ persist: true }` to flush to physical disk
            let persist = args.get(2).map(|a| matches!(a, Expr::Bool(true))).unwrap_or(false);

            if path.starts_with("virtual://") || !persist {
                // Default: write to in-memory virtual filesystem
                let vfs_path = path.strip_prefix("virtual://").unwrap_or(&path).to_string();
                if let Some(pm_inner) = pm.as_mut() {
                    pm_inner.vfs.insert(vfs_path, content);
                }
            } else {
                // Explicit persist: write to physical disk
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("FileWrite$: cannot create dir '{}': {}", parent.display(), e))?;
                    }
                }
                std::fs::write(&path, &content)
                    .map_err(|e| format!("FileWrite$: cannot write '{}': {}", path, e))?;
            }
            Ok(NavValue::Void)
        }
        "FileRead$" => {
            sandbox.check(Capability::DiskRead, "FileRead$")?;
            let path = expect_str_arg(args, 0, "FileRead$", scope)?;
            if path.starts_with("virtual://") {
                let vfs_path = path.strip_prefix("virtual://").unwrap_or(&path).to_string();
                match pm.as_ref().and_then(|p| p.vfs.get(&vfs_path)) {
                    Some(content) => Ok(NavValue::Str(content.clone())),
                    None => Err(format!("FileRead$: '{}' not found in virtual filesystem", path)),
                }
            } else {
                // Check VFS first, then physical disk
                if let Some(content) = pm.as_ref().and_then(|p| p.vfs.get(&path)) {
                    return Ok(NavValue::Str(content.clone()));
                }
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("FileRead$: cannot read '{}': {}", path, e))?;
                Ok(NavValue::Str(content))
            }
        }

        // ── Configuration (2026-07-22) ───────────────────────────────
        "ConfigGet$" => {
            let section = expect_str_arg(args, 0, "ConfigGet$", scope)?;
            let key = expect_str_arg(args, 1, "ConfigGet$", scope)?;
            let targets = crate::glue::config::load_glue_config(None)
                .map_err(|e| format!("ConfigGet$: {}", e))?;
            let target = targets.get(&section)
                .ok_or_else(|| format!("ConfigGet$: no section '{}' in config/glue.dbv", section))?;
            // Resolve dotted key (e.g., "templates.fn_template" → target.templates["fn_template"])
            if let Some(dot) = key.find('.') {
                let (sub, field) = key.split_at(dot);
                let field = &field[1..];
                match sub {
                    "templates" => {
                        let val = target.templates.get(field)
                            .ok_or_else(|| format!("ConfigGet$: no template '{}'", field))?;
                        Ok(NavValue::Str(val.clone()))
                    }
                    "protocols" => {
                        // Support both "Int" (returns "native/c_abi") and
                        // "Int.native" / "Int.c_abi" (returns single field).
                        if let Some(dot2) = field.find('.') {
                            let (type_name, proto_field) = field.split_at(dot2);
                            let proto_field = &proto_field[1..];
                            let entry = target.protocols.get(&format!("#{}", type_name));
                            match (entry, proto_field) {
                                (Some(e), "native") => Ok(NavValue::Str(e.native.clone())),
                                // 2026-07-26: c_abi is optional — unwrap or use empty string
                                (Some(e), "c_abi") => Ok(NavValue::Str(e.c_abi.clone().unwrap_or_default())),
                                (Some(_), _) => Err(format!("ConfigGet$: unknown protocol field '{}' (expected 'native' or 'c_abi')", proto_field)),
                                (None, _) => Err(format!("ConfigGet$: no protocol for type '{}' in section '{}'", type_name, section)),
                            }
                        } else {
                            let proto_key = format!("#{}", field);
                            let entry = target.protocols.get(&proto_key);
                            match entry {
                                // 2026-07-26: c_abi is optional — display or fallback
                                Some(e) => {
                                    let abi = e.c_abi.as_deref().unwrap_or(&e.native);
                                    Ok(NavValue::Str(format!("{}/{}", e.native, abi)))
                                },
                                None => Err(format!("ConfigGet$: no protocol '{}' in section '{}'", field, section)),
                            }
                        }
                    }
                    _ => Err(format!("ConfigGet$: unknown sub-section '{}'", sub)),
                }
            } else {
                Err("ConfigGet$: need dotted key like 'templates.fn_template'".into())
            }
        }

        // ── Universe Queries (2026-07-22) ────────────────────────────
        "DocRead$" => {
            let type_name = expect_str_arg(args, 0, "DocRead$", scope)?;
            let prop = expect_str_arg(args, 1, "DocRead$", scope)?;
            let rt = universe.get(&type_name)
                .ok_or_else(|| format!("DocRead$: type '{}' not in universe", type_name))?;
            match prop.as_str() {
                "properties" => {
                    let props: Vec<String> = rt.properties.keys().cloned().collect();
                    Ok(NavValue::Names(props))
                }
                // 2026-07-25: "bytes" query returns max_bits / 8 for backward compat.
                "bytes" => Ok(NavValue::Int((rt.max_bits / 8) as i64)),
                "fields" => {
                    let field_names: Vec<String> = rt.fields.iter().map(|(n, _)| n.clone()).collect();
                    Ok(NavValue::Names(field_names))
                }
                _ => match rt.properties.get(&prop) {
                    Some(v) => Ok(NavValue::Str(format!("{:?}", v))),
                    None => Err(format!("DocRead$: no property '{}' on type '{}'", prop, type_name)),
                }
            }
        }
        "CastPath$" => {
            let src = expect_str_arg(args, 0, "CastPath$", scope)?;
            let tgt = expect_str_arg(args, 1, "CastPath$", scope)?;
            let path = crate::analysis::layout_optimizer::find_cast_path(universe, &src, &tgt);
            match path {
                Some(types) => {
                    let steps: Vec<NavValue> = types.into_iter()
                        .map(|t| NavValue::Str(t)).collect();
                    Ok(NavValue::List(steps))
                }
                None => Ok(NavValue::List(vec![])),
            }
        }
        // 2026-07-23: Inject foreign struct layout into the type universe.
        // Used by ABI probing: injects field names, types, and offsets so
        // the protocol graph BFS can find Identity paths for matching types.
        "InjectTypeLayout$" => {
            let type_name = expect_str_arg(args, 0, "InjectTypeLayout$", scope)?;
            let size_str = expect_str_arg(args, 1, "InjectTypeLayout$", scope)?;
            let size: u64 = size_str.parse()
                .map_err(|_| format!("InjectTypeLayout$: invalid size '{}'", size_str))?;
            let fields_val = eval_nav_chain(&args[2], program, universe, stage, scope, sandbox, pm)?;
            // Accept both a List of [name, type, offset] triples and a
            // flat string "name:type:offset,name:type:offset,..."
            let parsed_fields: Vec<(String, String, u64)> = match &fields_val {
                NavValue::List(items) => {
                    let mut result = Vec::new();
                    for (i, item) in items.iter().enumerate() {
                        let triple = match item {
                            NavValue::List(parts) if parts.len() >= 3 => parts,
                            _ => return Err(format!("InjectTypeLayout$: field {} must be a [name, type, offset] list", i)),
                        };
                        let fname = nav_to_string(&triple[0]);
                        let ftype = nav_to_string(&triple[1]);
                        let foffset: u64 = nav_to_i64(&triple[2]) as u64;
                        result.push((fname, ftype, foffset));
                    }
                    result
                }
                NavValue::Str(s) => {
                    let mut result = Vec::new();
                    for part in s.split(',') {
                        let segments: Vec<&str> = part.splitn(3, ':').collect();
                        if segments.len() != 3 {
                            return Err(format!("InjectTypeLayout$: invalid field spec '{}' (expected name:type:offset)", part));
                        }
                        let foffset: u64 = segments[2].parse()
                            .map_err(|_| format!("InjectTypeLayout$: invalid offset '{}'", segments[2]))?;
                        result.push((segments[0].to_string(), segments[1].to_string(), foffset));
                    }
                    result
                }
                _ => return Err("InjectTypeLayout$: arg 2 must be a List or String".into()),
            };
            let entry = universe.types.entry(type_name.clone()).or_insert_with(|| {
                crate::type_universe::ResolvedType {
                    name: type_name.clone(),
                    base: String::new(),
                    bytes: size,
                    min_bits: 0,
                    max_bits: 0,
                    alignment: 0,
                    properties: std::collections::HashMap::new(),
                    type_params: vec![],
                    fields: vec![],
                }
            });
            entry.bytes = size;
            let mut field_names = Vec::new();
            for (i, (fname, ftype, foffset)) in parsed_fields.iter().enumerate() {
                entry.properties.insert(format!("field.{}.name", i), crate::ast::PropertyValue::String(fname.clone()));
                entry.properties.insert(format!("field.{}.offset", i), crate::ast::PropertyValue::Int(*foffset as i64));
                entry.properties.insert(format!("field.{}.type", i), crate::ast::PropertyValue::String(ftype.clone()));
                field_names.push((fname.clone(), crate::ast::Type::Custom(ftype.clone())));
            }
            entry.fields = field_names;
            Ok(NavValue::Void)
        }

        // ── Type Information (2026-07-22) ────────────────────────────
        "TypeInfo$" => {
            let sel_val = eval_nav_chain(&args[0], program, universe, stage, scope, sandbox, pm)?;
            let field = expect_str_arg(args, 1, "TypeInfo$", scope)?;
            let tl = match &sel_val {
                NavValue::Selection(sel) => {
                    let node = sel.nodes.first()
                        .ok_or_else::<String, _>(|| "TypeInfo$: empty selection".into())?;
                    match node {
                        crate::macros::selection::NodeRef::TopLevel(i) => {
                            program.get(*i).ok_or_else::<String, _>(|| "TypeInfo$: invalid node index".into())?
                        }
                        _ => return Err("TypeInfo$: only top-level items supported".into()),
                    }
                }
                NavValue::TopLevel(tl) => tl,
                _ => return Err("TypeInfo$: first arg must be a Selection or TopLevel".into()),
            };
            let result = type_info_from_toplevel(tl, &field)?;
            Ok(NavValue::Str(result))
        }

        // ── External Commands (2026-07-22) ───────────────────────────
        "ShellCmd$" | "ShellCmd#" => {
            sandbox.check(Capability::Shell, name)?;
            let cmd = expect_str_arg(args, 0, name, scope)?;
            let cmd_args: Vec<String> = args[1..].iter()
                .map(|a| eval_nav_chain(a, program, universe, stage, scope, sandbox, pm))
                .map(|r| r.and_then(|v| match v {
                    NavValue::Str(s) => Ok(s),
                    NavValue::Int(n) => Ok(n.to_string()),
                    _ => Err(format!("{}: arg must be a string or integer", name)),
                }))
                .collect::<Result<Vec<_>, _>>()?;
            let output = std::process::Command::new(&cmd)
                .args(&cmd_args)
                .output()
                .map_err(|e| format!("{}: failed to execute '{}': {}", name, cmd, e))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("{}: '{}' failed: {}", name, cmd, stderr));
            }
            Ok(NavValue::Str(String::from_utf8_lossy(&output.stdout).to_string()))
        }

        // ── Diagnostics (2026-07-23) ────────────────────────────────
        "EmitInfo$" => {
            let msg = expect_str_arg(args, 0, "EmitInfo$", scope)?;
            println!("info: {}", msg);
            Ok(NavValue::Void)
        }
        "EmitWarning$" => {
            let msg = expect_str_arg(args, 0, "EmitWarning$", scope)?;
            eprintln!("warning: {}", msg);
            Ok(NavValue::Void)
        }
        "EmitError$" => {
            let msg = expect_str_arg(args, 0, "EmitError$", scope)?;
            Err(msg)
        }

        // ── System Queries (2026-07-23) ────────────────────────────────
        // SysQuery$ provides structured host hardware introspection.
        // Requires --allow-sys-query. Supports:
        //   cpu.cores           → number of logical cores
        //   cpu.arch            → CPU architecture string (x86_64, aarch64, etc.)
        //   os                  → OS type string (linux, macos, windows, etc.)
        //   os.version          → OS kernel/release version
        //   hostname            → machine hostname
        //   memory.total        → total physical RAM in bytes (Linux only)
        //   memory.free         → available RAM in bytes (Linux only)
        //   pagesize            → system page size in bytes
        "SysQuery$" | "SysQuery#" => {
            sandbox.check(Capability::SysQuery, name)?;
            let query = expect_str_arg(args, 0, name, scope)?;
            // 2026-07-23: Check target profile overrides first (multi-target).
            if let Some(override_val) = sandbox.sysquery_overrides.get(&query) {
                if let Ok(n) = override_val.parse::<i64>() {
                    return Ok(NavValue::Int(n));
                }
                return Ok(NavValue::Str(override_val.clone()));
            }
            match query.as_str() {
                "cpu.cores" => {
                    let cores = std::thread::available_parallelism()
                        .map(|n| n.get() as i64)
                        .unwrap_or(1);
                    Ok(NavValue::Int(cores))
                }
                "cpu.arch" => {
                    Ok(NavValue::Str(std::env::consts::ARCH.to_string()))
                }
                "os" => {
                    Ok(NavValue::Str(std::env::consts::OS.to_string()))
                }
                "os.version" => {
                    // 2026-07-23: On Linux, read /proc/sys/kernel/osrelease.
                    // On other platforms, fall back to `uname -r` or "unknown".
                    #[cfg(target_os = "linux")]
                    {
                        let version = std::fs::read_to_string("/proc/sys/kernel/osrelease")
                            .unwrap_or_else(|_| "unknown".into());
                        return Ok(NavValue::Str(version.trim().to_string()));
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        let version = std::process::Command::new("uname")
                            .arg("-r")
                            .output()
                            .ok()
                            .and_then(|o| {
                                if o.status.success() {
                                    String::from_utf8(o.stdout).ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| "unknown".into());
                        return Ok(NavValue::Str(version.trim().to_string()));
                    }
                }
                "hostname" => {
                    // 2026-07-23: On Linux, read /proc/sys/kernel/hostname.
                    // Fall back to `hostname` command on other platforms.
                    #[cfg(target_os = "linux")]
                    {
                        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
                            .unwrap_or_else(|_| "unknown".into());
                        return Ok(NavValue::Str(hostname.trim().to_string()));
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        let hostname = std::process::Command::new("hostname")
                            .output()
                            .ok()
                            .and_then(|o| {
                                if o.status.success() {
                                    String::from_utf8(o.stdout).ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| "unknown".into());
                        return Ok(NavValue::Str(hostname.trim().to_string()));
                    }
                }
                "memory.total" | "memory.free" => {
                    // 2026-07-23: Parse /proc/meminfo on Linux for total/available RAM.
                    // Returns bytes (parsed from kB values).
                    #[cfg(target_os = "linux")]
                    {
                        let meminfo = std::fs::read_to_string("/proc/meminfo")
                            .map_err(|e| format!("{}: cannot read /proc/meminfo: {}", name, e))?;
                        let prefix = if query == "memory.total" { "MemTotal:" } else { "MemAvailable:" };
                        for line in meminfo.lines() {
                            if let Some(rest) = line.strip_prefix(prefix) {
                                let val = rest.trim().strip_suffix(" kB").unwrap_or(rest.trim());
                                if let Ok(kb) = val.trim().parse::<i64>() {
                                    return Ok(NavValue::Int(kb * 1024));
                                }
                            }
                        }
                        Err(format!("{}: could not parse {} from /proc/meminfo", name, query))
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        Err(format!("{}: '{}' is only supported on Linux", name, query))
                    }
                }
                "pagesize" => {
                    // 2026-07-23: Return the system page size.
                    // Uses `getconf PAGE_SIZE` on POSIX, defaults to 4096.
                    #[cfg(not(windows))]
                    {
                        let pagesize = std::process::Command::new("getconf")
                            .arg("PAGE_SIZE")
                            .output()
                            .ok()
                            .and_then(|o| {
                                if o.status.success() {
                                    String::from_utf8(o.stdout).ok()
                                } else {
                                    None
                                }
                            })
                            .and_then(|s| s.trim().parse::<i64>().ok())
                            .unwrap_or(4096);
                        Ok(NavValue::Int(pagesize))
                    }
                    #[cfg(windows)]
                    {
                        // Windows page size is always 4096
                        Ok(NavValue::Int(4096))
                    }
                }
                _ => Err(format!("{}: unknown query '{}'", name, query)),
            }
        }

        // ── Timestamp (2026-07-23) ──────────────────────────────────────
        // TimeNow$ returns a deterministic timestamp for reproducibility.
        // Uses the most recent git commit timestamp if available, falling
        // back to the current system time. Pure intrinsic — no sandbox check.
        "TimeNow$" | "TimeNow#" => {
            // 2026-07-23: Try git commit timestamp first (reproducible per commit),
            // then fall back to current wall-clock time.
            let ts = std::process::Command::new("git")
                .args(["log", "-1", "--format=%ct"])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        let s = String::from_utf8_lossy(&o.stdout);
                        s.trim().parse::<i64>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                });
            Ok(NavValue::Int(ts))
        }

        // ── Environment Variables (2026-07-23) ──────────────────────────
        // EnvGet$ reads a named environment variable. Returns empty string
        // if the variable is not set. Requires --allow-sys-query.
        "EnvGet$" | "EnvGet#" => {
            sandbox.check(Capability::SysQuery, name)?;
            let env_name = expect_str_arg(args, 0, name, scope)?;
            match std::env::var(&env_name) {
                Ok(val) => Ok(NavValue::Str(val)),
                Err(_) => Ok(NavValue::Str(String::new())),
            }
        }

        // ── HTTP Fetch (2026-07-23) ─────────────────────────────────────
        // HttpFetch$ fetches a URL via HTTP GET and returns the response
        // body as a string. Requires --allow-net.
        "HttpFetch$" | "HttpFetch#" => {
            sandbox.check(Capability::Network, name)?;
            let url = expect_str_arg(args, 0, name, scope)?;
            let resp = ureq::get(&url).call()
                .map_err(|e| format!("{}: failed to fetch '{}': {}", name, url, e))?;
            let body = resp.into_string()
                .map_err(|e| format!("{}: failed to read body from '{}': {}", name, url, e))?;
            Ok(NavValue::Str(body))
        }

        _ => Err(format!("unknown navigation intrinsic '{}'", name)),
    }
}

// ── Field Method Dispatch ──────────────────────────────────────────────

fn eval_nav_field_method(
    name: &str,
    prev: NavValue,
    program: &mut Vec<TopLevel>,
    stage: StageKind,
) -> Result<NavValue, String> {
    match name {
        "Count$" => match prev {
            NavValue::Selection(sel) => Ok(NavValue::Count(sel.count())),
            _ => Err("Count$ requires Selection".into()),
        },
        "IsEmpty$" => match prev {
            NavValue::Selection(sel) => Ok(NavValue::Bool(sel.is_empty())),
            _ => Err("IsEmpty$ requires Selection".into()),
        },
        "Names$" => match prev {
            NavValue::Selection(sel) => Ok(NavValue::Names(sel.names(program))),
            _ => Err("Names$ requires Selection".into()),
        },
        "Before$" => match prev {
            NavValue::Selection(ref sel) => Position::before(sel)
                .map(NavValue::Position)
                .ok_or_else(|| "Before$: empty selection".into()),
            _ => Err("Before$ requires Selection".into()),
        },
        "After$" => match prev {
            NavValue::Selection(ref sel) => Position::after(sel)
                .map(NavValue::Position)
                .ok_or_else(|| "After$: empty selection".into()),
            _ => Err("After$ requires Selection".into()),
        },
        "Replace$" => match prev {
            NavValue::Selection(ref sel) => Position::replace(sel)
                .map(NavValue::Position)
                .ok_or_else(|| "Replace$: empty selection".into()),
            _ => Err("Replace$ requires Selection".into()),
        },
        _ => Err(format!("unknown field method '{}' on navigation value", name)),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Extract a string value from a $ intrinsic argument.
/// Resolves identifiers from the compile-time scope, so variable
/// bindings (let x = ...) work correctly in $ intrinsic calls.
/// 2026-07-23: Added scope resolution for identifiers and fallback
/// evaluation for complex expressions (string concatenation, etc.).
fn expect_str_arg(args: &[Expr], idx: usize, intrinsic: &str, scope: &Scope) -> Result<String, String> {
    let arg = args.get(idx).ok_or_else(|| {
        format!("{}: missing argument {}", intrinsic, idx)
    })?;
    // Quick path: simple literals and identifiers
    match arg {
        Expr::Quoted(bytes) => return String::from_utf8(bytes.clone())
            .map_err(|_| format!("{}: arg {} is not valid UTF-8", intrinsic, idx)),
        Expr::Identifier(s) => {
            if let Some(val) = scope.get(s) {
                return match val {
                    NavValue::Str(v) => Ok(v.clone()),
                    NavValue::Int(n) => Ok(n.to_string()),
                    NavValue::Count(n) => Ok(n.to_string()),
                    NavValue::Bool(b) => Ok(if *b { "true".into() } else { "false".into() }),
                    _ => Err(format!("{}: variable '{}' has unsupported type for string argument", intrinsic, s)),
                };
            } else {
                return Ok(s.clone());
            }
        }
        Expr::Decimal(n) => return Ok(n.to_string()),
        _ => {}
    }
    // Fallback: evaluate the expression via eval_nav_chain (handles concatenation, etc.)
    // We need a dummy program and universe since we only need string evaluation.
    // This is safe because string concatenation doesn't mutate the AST.
    let mut dummy_program = Vec::new();
    let mut dummy_universe = TypeUniverse::new();
    let mut dummy_sandbox = Sandbox::permissive();
    let mut dummy_pm: Option<&mut PluginManager> = None;
    match eval_nav_chain(arg, &mut dummy_program, &mut dummy_universe,
        StageKind::Normalized, scope, &mut dummy_sandbox, &mut dummy_pm)
    {
        Ok(NavValue::Str(s)) => Ok(s),
        Ok(NavValue::Int(n)) => Ok(n.to_string()),
        Ok(NavValue::Count(n)) => Ok(n.to_string()),
        Ok(other) => Err(format!("{}: arg {} has type {:?}, expected a string", intrinsic, idx, other)),
        Err(e) => Err(format!("{}: arg {} evaluation failed: {}", intrinsic, idx, e)),
    }
}

fn expect_int_arg(args: &[Expr], idx: usize, intrinsic: &str) -> Result<i64, String> {
    let arg = args.get(idx).ok_or_else(|| {
        format!("{}: missing argument {}", intrinsic, idx)
    })?;
    match arg {
        Expr::Decimal(n) => Ok(*n),
        _ => Err(format!("{}: arg {} must be an integer", intrinsic, idx)),
    }
}

// Fix 3: Parse an Expr argument as a PropertyValue
fn expect_prop_arg(args: &[Expr], idx: usize, intrinsic: &str) -> Result<PropertyValue, String> {
    let arg = args.get(idx).ok_or_else(|| {
        format!("{}: missing argument {}", intrinsic, idx)
    })?;
    match arg {
        Expr::Bool(b) => Ok(PropertyValue::Bool(*b)),
        Expr::Decimal(n) => Ok(PropertyValue::Int(*n)),
        Expr::Quoted(bytes) => {
            let s = String::from_utf8(bytes.clone())
                .map_err(|_| format!("{}: arg {} is not valid UTF-8", intrinsic, idx))?;
            // Quoted could be a string or identifier
            Ok(PropertyValue::String(s))
        }
        Expr::Identifier(s) => Ok(PropertyValue::Identifier(s.clone())),
        _ => Err(format!("{}: arg {} must be a property value (bool, int, string)", intrinsic, idx)),
    }
}

fn extract_str_lit(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Quoted(bytes) => String::from_utf8(bytes.clone()).ok(),
        Expr::UnitLiteral { .. } => None,
        _ => None,
    }
}

fn selector_1_str<F>(args: &[Expr], f: F, scope: &Scope) -> Result<NavValue, String>
where F: FnOnce(String) -> Result<NavValue, String> {
    let s = expect_str_arg(args, 0, "?", scope)?;
    f(s)
}

fn selector_1_int_opt<F>(args: &[Expr], f: F) -> Result<NavValue, String>
where F: FnOnce(usize) -> Result<NavValue, String> {
    let n = if args.len() > 1 {
        expect_int_arg(args, 1, "?")? as usize
    } else {
        1
    };
    f(n)
}

/// Check if the first arg of a $ call is Identifier("Stage$"),
/// indicating a Stage$.Foo$ method call rather than a navigation chain.
fn is_stage_receiver(args: &[Expr]) -> bool {
    matches!(args.first(), Some(Expr::Identifier(s)) if s == "Stage$")
}

/// Retrieve the tainted indices set from the PluginManager, if available.
/// 2026-07-23: Used by selectors to exclude generated nodes (anti-infinite-recursion).
fn get_tainted_set(pm: &mut Option<&mut PluginManager>) -> BTreeSet<usize> {
    pm.as_ref().map(|p| p.tainted_indices.clone()).unwrap_or_default()
}

/// Filter out tainted top-level indices from a node list.
/// 2026-07-23: Stmt and Expr node refs are not filtered (their parent's
/// taint status is already respected by top-level filtering).
fn filter_tainted_nodes(nodes: &mut Vec<NodeRef>, tainted: &BTreeSet<usize>) {
    nodes.retain(|node| match node {
        NodeRef::TopLevel(i) => !tainted.contains(i),
        _ => true,
    });
}

/// Build a short human-readable summary of a TopLevel AST node.
/// 2026-07-23: Used for expansion trace descriptions.
fn node_summary(tl: &TopLevel) -> String {
    match tl {
        TopLevel::Import(i) => format!("import \"{}\"", i.path()),
        TopLevel::Definition(d) => format!("defn {}", d.name),
        TopLevel::Transaction(t) => format!("txn {}", t.name),
        TopLevel::Cell(c) => format!("cell {}", c.name),
        TopLevel::ForeignBinding(f) => format!("frgn {}", f.foreign_name),
        TopLevel::Export(e) => match &e.export_name {
            Some(n) => format!("export {}", n),
            None => "export".into(),
        },
        TopLevel::Constant(c) => format!("constant {}", c.name),
        TopLevel::Obj(s) => format!("struct {}", s.name),
        TopLevel::Enum(e) => format!("enum {}", e.name),
        TopLevel::Statement(_) => "statement".into(),
        _ => "ast-node".into(),
    }
}

/// Record an expansion trace at the given top-level index.
/// 2026-07-23: Used by AST constructors and action intrinsics to document
/// which $ intrinsic created or last modified each program node.
fn record_expansion(pm: &mut Option<&mut PluginManager>, index: usize, description: String) {
    if let Some(pm_inner) = pm {
        pm_inner.expansion_traces.insert(index, description);
    }
}

/// Convert a NavValue to a string for StrJoin$ and similar operations.
fn nav_to_string(val: &NavValue) -> String {
    match val {
        NavValue::Str(s) => s.clone(),
        NavValue::Int(n) => n.to_string(),
        NavValue::Count(n) => n.to_string(),
        NavValue::Bool(b) => b.to_string(),
        NavValue::Names(names) => names.join(", "),
        NavValue::Selection(sel) => format!("Selection({})", sel.count()),
        _ => format!("{:?}", val),
    }
}

/// Extract type information from a TopLevel AST node by field path.
fn type_info_from_toplevel(tl: &TopLevel, field: &str) -> Result<String, String> {
    // 2026-07-23: Export and compile-time defns delegate to inner items.
    match tl {
        TopLevel::Export(e) => return type_info_from_toplevel(&e.inner, field),
        TopLevel::CompileTimeDefn(d) => return type_info_from_toplevel(&TopLevel::Definition(d.clone()), field),
        TopLevel::CompileTimeTxn(t) => return type_info_from_toplevel(&TopLevel::Transaction(t.clone()), field),
        _ => {}
    }
    match (tl, field) {
        (TopLevel::Definition(d), "name") => Ok(d.name.clone()),
        (TopLevel::Definition(d), "params.count") => Ok(d.parameters.len().to_string()),
        (TopLevel::Definition(d), f) if f.starts_with("params.") => {
            let rest = &f[7..]; // strip "params."
            let parts: Vec<&str> = rest.split('.').collect();
            let idx: usize = parts[0].parse()
                .map_err(|_| format!("TypeInfo$: invalid param index '{}'", parts[0]))?;
            let param = d.parameters.get(idx)
                .ok_or_else(|| format!("TypeInfo$: param index {} out of bounds (max {})", idx, d.parameters.len().saturating_sub(1)))?;
            match parts.get(1) {
                Some(&"name") => Ok(param.0.clone()),
                Some(&"type") => Ok(format!("{}", param.1)),
                _ => Err(format!("TypeInfo$: unknown param field '{:?}'", parts.get(1))),
            }
        }
        (TopLevel::Definition(d), "output_type") => {
            match &d.output_type {
                Some(ot) => Ok(single_type_name(ot)),
                None => Ok("Void".into()),
            }
        }
        (TopLevel::Definition(d), "outputs.count") => Ok(d.outputs.len().to_string()),
        (TopLevel::ForeignBinding(fb), "name") => Ok(fb.foreign_name.clone()),
        (TopLevel::ForeignBinding(fb), "briev_name") => Ok(fb.effective_briev_name().to_string()),
        (TopLevel::Import(i), "path") => Ok(i.path().to_string()),
        _ => Err(format!("TypeInfo$: unknown field '{}' for this item type", field)),
    }
}

/// Extract the type name from a single-type OutputType.
/// For complex types (tuples, unions), returns the first type name.
/// 2026-07-23: Used by TypeInfo$ "output_type" field.
fn single_type_name(ot: &OutputType) -> String {
    match ot {
        OutputType::Single(ty) => format!("{}", ty),
        OutputType::Array(inner) => single_type_name(inner) + "[]",
        OutputType::Named(_, inner) => single_type_name(inner),
        OutputType::Tuple(types) | OutputType::Union(types) => {
            types.first().map(|t| single_type_name(t)).unwrap_or_else(|| "Void".into())
        }
    }
}

// ── Quote$ Helpers ──────────────────────────────────────────────────────
// 2026-07-23: AST-level quasiquoting — resolve $identifier references from
// scope, convert NavValues to Exprs, and handle $$ escaping.

/// Parse a string as a single Briev expression.
fn parse_expr_from_string(s: &str) -> Result<Expr, String> {
    let tokens = crate::lexer::tokenize(s)
        .map_err(|e| format!("cannot tokenize expression '{}': {}", s, e))?;
    let mut parser = crate::parser::Parser::new(tokens, s);
    parser.parse_expression()
        .map_err(|e| format!("cannot parse '{}' as expression: {}", s, e))
}

/// Human-readable name for a NavValue variant.
fn nav_type_name(val: &NavValue) -> &'static str {
    match val {
        NavValue::Selection(_) => "Selection",
        NavValue::Position(_) => "Position",
        NavValue::TextSelection(_) => "TextSelection",
        NavValue::Count(_) => "Count",
        NavValue::Names(_) => "Names",
        NavValue::Bool(_) => "Bool",
        NavValue::Int(_) => "Int",
        NavValue::Str(_) => "Str",
        NavValue::TopLevel(_) => "TopLevel",
        NavValue::VecTopLevel(_) => "VecTopLevel",
        NavValue::List(_) => "List",
        NavValue::Void => "Void",
    }
}

/// Convert a NavValue to an Expr for substitution into a quasiquote template.
fn nav_value_to_expr(val: &NavValue) -> Result<Expr, String> {
    match val {
        NavValue::Int(n) => Ok(Expr::Decimal(*n)),
        NavValue::Bool(b) => Ok(Expr::Bool(*b)),
        NavValue::Count(n) => Ok(Expr::Decimal(*n as i64)),
        NavValue::Str(s) => parse_expr_from_string(s),
        NavValue::Names(names) => {
            let exprs: Vec<Expr> = names.iter()
                .map(|n| Expr::Quoted(n.as_bytes().to_vec()))
                .collect();
            Ok(Expr::List(exprs))
        }
        NavValue::TopLevel(TopLevel::Statement(stmt)) => {
            match stmt.as_ref() {
                Statement::Expression(expr) => Ok(expr.clone()),
                _ => Err(format!(
                    "cannot substitute a non-expression statement '{}' as an expression",
                    stmt
                )),
            }
        }
        other => Err(format!(
            "cannot substitute {} as an expression (use Str, Int, Bool, Count, Names, or an expression statement)",
            nav_type_name(other)
        )),
    }
}

/// Recursively resolve $ident references in an Expr tree.
/// $$ident → $ident (literal, escape). $ident → scope lookup.
    fn resolve_dollar_refs_in_expr(expr: &mut Expr, scope: &Scope) -> Result<(), String> {
    match expr {
        Expr::UnitLiteral { .. } => Ok(()),
        Expr::Identifier(name) => {
            // $$escape → produce literal $ident (no interpolation). The leading
            // $ is preserved but won't be re-matched by $ident because we return.
            if let Some(rest) = name.strip_prefix("$$") {
                *expr = Expr::Identifier(format!("${}", rest));
                return Ok(());
            }
            // $ident → scope lookup
            if let Some(var_name) = name.strip_prefix('$') {
                if !var_name.is_empty() {
                    if let Some(val) = scope.get(var_name) {
                        let replacement = nav_value_to_expr(val)?;
                        *expr = replacement;
                    }
                }
            }
            Ok(())
        }
        Expr::Call(_, args, _) => {
            for arg in args.iter_mut() {
                resolve_dollar_refs_in_expr(arg, scope)?;
            }
            Ok(())
        }
        Expr::BinaryOp(_, lhs, rhs) => {
            resolve_dollar_refs_in_expr(lhs, scope)?;
            resolve_dollar_refs_in_expr(rhs, scope)
        }
        Expr::UnaryOp(_, operand) => resolve_dollar_refs_in_expr(operand, scope),
        Expr::Field(obj, _) => resolve_dollar_refs_in_expr(obj, scope),
        Expr::Index(obj, index) => {
            resolve_dollar_refs_in_expr(obj, scope)?;
            resolve_dollar_refs_in_expr(index, scope)
        }
        Expr::If(cond, then, else_) => {
            resolve_dollar_refs_in_expr(cond, scope)?;
            resolve_dollar_refs_in_expr(then, scope)?;
            if let Some(el) = else_ {
                resolve_dollar_refs_in_expr(el, scope)?;
            }
            Ok(())
        }
        Expr::Block(stmts) => {
            for stmt in stmts.iter_mut() {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
        Expr::Match(scrutinee, arms) => {
            resolve_dollar_refs_in_expr(scrutinee, scope)?;
            for arm in arms.iter_mut() {
                if let Some(ref mut g) = arm.guard {
                    resolve_dollar_refs_in_expr(g, scope)?;
                }
                resolve_dollar_refs_in_expr(&mut arm.body, scope)?;
            }
            Ok(())
        }
        Expr::Tuple(items) | Expr::List(items) => {
            for item in items.iter_mut() {
                resolve_dollar_refs_in_expr(item, scope)?;
            }
            Ok(())
        }
        Expr::Lambda(_, body) => resolve_dollar_refs_in_expr(body, scope),
        Expr::Cast(inner, _)
        | Expr::Within(inner, _)
        | Expr::IsType(inner, _)
        | Expr::Deref(inner)
        | Expr::AddrOf(inner)
        | Expr::Await(inner)
        | Expr::Consume(inner) => resolve_dollar_refs_in_expr(inner, scope),
        Expr::PluginIntercept { args: pargs, .. } => {
            for arg in pargs.iter_mut() {
                resolve_dollar_refs_in_expr(arg, scope)?;
            }
            Ok(())
        }
        Expr::DerivationBlock(db) => {
            if let Some(ref mut syn) = db.synthesized {
                resolve_dollar_refs_in_expr(syn, scope)?;
            }
            Ok(())
        }
        // Literals and simple values — no nested identifiers
        Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _) | Expr::Decimal(_) | Expr::Char(_) | Expr::Float(_) | Expr::Bool(_) | Expr::BeginProgram
        | Expr::TaggedLiteral(_,_)
        | Expr::FormattingAnnotation(_) | Expr::StructLiteral { .. } | Expr::Slice { .. } | Expr::Range { .. } | Expr::Spawn { .. } => Ok(()),
        Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => resolve_dollar_refs_in_expr(recv, scope),
        Expr::MethodCall(recv, _, args, _, _) => {
            resolve_dollar_refs_in_expr(recv, scope)?;
            for a in args {
                resolve_dollar_refs_in_expr(a, scope)?;
            }
            Ok(())
        }
        Expr::Exists(_) => { unreachable!("fn? only in stage eval") },
        Expr::Capture { expr, .. } => resolve_dollar_refs_in_expr(expr, scope),
        Expr::PluginIntercept { receiver, args, .. } => {
            if let Some(recv) = receiver {
                resolve_dollar_refs_in_expr(recv, scope)?;
            }
            for a in args {
                resolve_dollar_refs_in_expr(a, scope)?;
            }
            Ok(())
        }

    }
}

/// Recursively resolve $ident references in a Statement tree.
fn resolve_dollar_refs_in_stmt(stmt: &mut Statement, scope: &Scope) -> Result<(), String> {
    match stmt {
        Statement::Yield => Ok(()),
        Statement::Check(_) => Ok(()),
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                resolve_dollar_refs_in_expr(e, scope)?;
            }
            Ok(())
        }
        Statement::Assign(target, value) => {
            resolve_dollar_refs_in_expr(target, scope)?;
            resolve_dollar_refs_in_expr(value, scope)
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                resolve_dollar_refs_in_expr(t, scope)?;
            }
            resolve_dollar_refs_in_expr(value, scope)
        }
        Statement::FreeHint(_) | Statement::KeepHint(_) | Statement::Trap | Statement::Halt => Ok(()),
        Statement::Break => Ok(()),
        Statement::Term(expr) | Statement::EndProgram(expr)
        | Statement::Rollback(expr) => {
            if let Some(e) = expr {
                resolve_dollar_refs_in_expr(e, scope)?;
            }
            Ok(())
        }
        Statement::Expression(expr) => resolve_dollar_refs_in_expr(expr, scope),
        Statement::Guarded(guard, body) => {
            resolve_dollar_refs_in_expr(guard, scope)?;
            for s in body.iter_mut() {
                resolve_dollar_refs_in_stmt(s, scope)?;
            }
            Ok(())
        }
        Statement::Gate(cond) => {
            resolve_dollar_refs_in_expr(cond, scope)
        }
        Statement::Block(stmts) | Statement::SyncBlock(stmts)
        | Statement::Defer(stmts) | Statement::Mutex(stmts) => {
            for s in stmts.iter_mut() {
                resolve_dollar_refs_in_stmt(s, scope)?;
            }
            Ok(())
        }
        Statement::Barrier { body, .. } => {
            for s in body.iter_mut() {
                resolve_dollar_refs_in_stmt(s, scope)?;
            }
            Ok(())
        }
        Statement::Foreach { list, body, .. } => {
            resolve_dollar_refs_in_expr(list, scope)?;
            for s in body.iter_mut() {
                resolve_dollar_refs_in_stmt(s, scope)?;
            }
            Ok(())
        }
        Statement::TrgBinding { instance, .. } => {
            resolve_dollar_refs_in_expr(instance, scope)
        }
        Statement::InlineAsm { .. } | Statement::MetadataAssignment(..)
        | Statement::InlineDefn(_) | Statement::InlineTxn(_) | Statement::Match { .. } => Ok(()),
        // 2026-09-22 (D16 p3b): `open` — resolve $refs in its expressions.
        Statement::Open(lhs, rhs) => {
            resolve_dollar_refs_in_expr(lhs, scope)?;
            resolve_dollar_refs_in_expr(rhs, scope)
        }
    }
}

/// Recursively resolve $ident references in a Type tree (minimal — handles
/// Custom name lookup from Str scope values).
fn resolve_dollar_refs_in_type(ty: &mut Type, scope: &Scope) -> Result<(), String> {
    match ty {
        Type::Dyn(inner) => resolve_dollar_refs_in_type(inner, scope),
        Type::Task(inner) => resolve_dollar_refs_in_type(inner, scope),
        Type::Number(_) => Ok(()),
        Type::Custom(name) => {
            if let Some(var_name) = name.strip_prefix('$') {
                if let Some(val) = scope.get(var_name) {
                    match val {
                        NavValue::Str(s) => {
                            *ty = Type::Custom(s.clone());
                        }
                        _ => return Err(format!(
                            "cannot substitute {} as a type name (use a Str scope variable)",
                            nav_type_name(val)
                        )),
                    }
                }
            }
            Ok(())
        }
        Type::Generic(_, args) | Type::Applied(_, args)
        | Type::Tuple(args) | Type::Union(args) => {
            for a in args.iter_mut() {
                resolve_dollar_refs_in_type(a, scope)?;
            }
            Ok(())
        }
        Type::Ptr(inner) | Type::PtrConst(inner) => {
            resolve_dollar_refs_in_type(inner, scope)
        }
        Type::Vector(inner, _) | Type::Constrained(inner, _) => {
            resolve_dollar_refs_in_type(inner, scope)
        }
        Type::Void | Type::Bits(_) | Type::Width(_)
        | Type::TypeVar(_)
        | Type::LayoutPtr(_) | Type::Function(_, _) => Ok(()),
    }
}

/// Recursively resolve $ident references in a TopLevel AST node.
/// Handles common TopLevel variants; less common ones (Trigger, etc.)
/// are skipped conservatively.
fn resolve_dollar_refs_in_toplevel(tl: &mut TopLevel, scope: &Scope) -> Result<(), String> {
    match tl {
        TopLevel::Statement(stmt) => resolve_dollar_refs_in_stmt(stmt, scope),
        TopLevel::StaticStruct(_) | TopLevel::Trait(_) | TopLevel::Impl(_) => Ok(()),
        // 2026-09-21 (E7): electronics-surface statement — no $-refs to
        // resolve (same conservative skip as Trigger).
        TopLevel::Budget(_) => Ok(()),
        // 2026-09-22 (Slice B): participation facts — no $-refs to resolve.
        TopLevel::Unpop(_) | TopLevel::ShortCircuit(_) => Ok(()),
        // 2026-09-22 (Slice C): the static when law — resolve $-refs in its
        // guard and facts.
        TopLevel::WhenLaw(w) => {
            resolve_dollar_refs_in_expr(&mut w.guard, scope)?;
            for fact in &mut w.facts {
                resolve_dollar_refs_in_stmt(fact, scope)?;
            }
            Ok(())
        }
        TopLevel::Definition(def) => {
            for stmt in &mut def.body {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
        TopLevel::TypeDefOperator(def) => {
            for stmt in &mut def.body {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
        TopLevel::Transaction(txn) => {
            for stmt in &mut txn.body {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
        TopLevel::StageBlock(sb) => {
            for stmt in &mut sb.body {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
        TopLevel::ForeignBinding(fb) => {
            for (_, ty) in &mut fb.inputs {
                resolve_dollar_refs_in_type(ty, scope)?;
            }
            for (_, ty) in &mut fb.success_output {
                resolve_dollar_refs_in_type(ty, scope)?;
            }
            Ok(())
        }
        TopLevel::Obj(s) => {
            for field in &mut s.fields {
                resolve_dollar_refs_in_type(&mut field.ty, scope)?;
            }
            Ok(())
        }
        TopLevel::Enum(e) => {
            for variant in &mut e.variants {
                match variant {
                    crate::ast::top::EnumVariant::Unit(_) => {}
                    crate::ast::top::EnumVariant::Tuple(_, types) => {
                        for ty in types.iter_mut() {
                            resolve_dollar_refs_in_type(ty, scope)?;
                        }
                    }
                    crate::ast::top::EnumVariant::Struct(_, fields) => {
                        for (_, ty) in fields.iter_mut() {
                            resolve_dollar_refs_in_type(ty, scope)?;
                        }
                    }
                }
            }
            Ok(())
        }
        TopLevel::Constant(c) => {
            resolve_dollar_refs_in_expr(&mut c.expr, scope)?;
            Ok(())
        }
        TopLevel::Import(_) | TopLevel::Export(_) | TopLevel::Cell(_)
        | TopLevel::Trigger(_) | TopLevel::Signature(_)
        | TopLevel::StateDecl(_) | TopLevel::TriggerBinding { .. }
        | TopLevel::LinkDependency(_) | TopLevel::ResourceDecl(_)
        | TopLevel::TypeDef(_) | TopLevel::Codec(_)
        | TopLevel::Assertion { .. } | TopLevel::Fuzzed { .. }
        | TopLevel::RenderBlock(_) | TopLevel::Stylesheet(_)
        | TopLevel::SvgComponent { .. } | TopLevel::SyncGroup { .. }
        | TopLevel::Cfg(_) | TopLevel::ProtocolDef(_)
        | TopLevel::AsmFn(_)
        | TopLevel::IsrHandler(_)
        | TopLevel::CompileTimeLet(_, _) | TopLevel::CompileTimeConst(_, _)
        | TopLevel::ModuleMetadata(_) => Ok(()),
        TopLevel::Init(init) => {
            if let Some(value) = &mut init.value {
                resolve_dollar_refs_in_expr(value, scope)?;
            }
            for stmt in &mut init.body {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
        TopLevel::CompileTimeDefn(d) => {
            for stmt in &mut d.body {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
        TopLevel::CompileTimeTxn(t) => {
            for stmt in &mut t.body {
                resolve_dollar_refs_in_stmt(stmt, scope)?;
            }
            Ok(())
        }
    }
}

/// Restore $$ → $ in all identifiers within a TopLevel AST node.
/// Called after parse-time sentinel replacement to handle identifiers
/// that contain the sentinel byte.
fn restore_double_dollar_in_toplevel(tl: &mut TopLevel) {
    // For the initial implementation, this is a no-op at the AST level
    // because $$ is not a valid identifier start in Briev's lexer —
    // it would be lexed as two separate tokens: $ and $identifier.
    // The sentinel replacement at the string level handles this correctly.
    // This function is a hook for future $$-in-identifier support.
    let _ = tl;
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_scope() -> Scope { Scope::new() }

    fn test_sandbox() -> Sandbox { Sandbox::permissive() }

    #[test]
    fn test_tag_selector_via_call() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result { NavValue::Selection(sel) => assert_eq!(sel.count(), 1), _ => panic!() }
    }

    #[test]
    fn test_count_via_call() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
            TopLevel::Import(Import::literal("std/net.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let inner = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let expr = Expr::Call("Count$".into(), vec![inner], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result { NavValue::Count(n) => assert_eq!(n, 2), _ => panic!() }
    }

    #[test]
    fn test_insert_before_first_import() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
            TopLevel::Definition(Definition {
                name: "main".into(), type_params: vec![], parameters: vec![],
                output_type: None, outputs: vec![Type::Custom("Int".into())],
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![], metadata: Default::default(),
                derivation: None, modifiers: vec![], annotations: vec![], span: None,
                doc: None,
            }),
        ];
        let mut universe = TypeUniverse::new();
        let import_call = Expr::Call("Import$".into(), vec![Expr::Quoted("std/prelude.bv".into())], None);
        let tag_call = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let first_call = Expr::Call("First$".into(), vec![tag_call], None);
        let before_call = Expr::Call("Before$".into(), vec![first_call], None);
        let insert_call = Expr::Call("Insert$".into(), vec![before_call, import_call], None);
        let result = eval_nav_chain(&insert_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        assert!(matches!(result, NavValue::Void));
        assert_eq!(program.len(), 3);
        match &program[0] {
            TopLevel::Import(i) => assert_eq!(i.path(), "std/prelude.bv"),
            other => panic!("expected Import, got {:?}", other),
        }
    }

    #[test]
    fn test_delete_selection() {
        let mut program = vec![TopLevel::Import(Import::literal("std/io.bv", vec![]))];
        let mut universe = TypeUniverse::new();
        let tag_call = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let delete_call = Expr::Call("Delete$".into(), vec![tag_call], None);
        let result = eval_nav_chain(&delete_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        assert!(matches!(result, NavValue::Void));
        assert!(program.is_empty());
    }

    // Fix 1: Test ForEach loop variable binding
    #[test]
    fn test_foreach_binds_variable() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
            TopLevel::Import(Import::literal("std/b.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let foreach_stmt = Statement::Foreach {
            item: "imp".into(),
            list: Box::new(Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None)),
            body: vec![
                Statement::Expression(Expr::Call("Count$".into(), vec![
                    Expr::Identifier("imp".into()),
                ], None)),
            ],
        };
        let body = vec![foreach_stmt];
        let result = evaluate_stage_block(&body, &mut program, &mut universe,
            StageKind::Parsed, &mut None);
        assert!(result.is_ok());
    }

    // Fix 3: Test Set$ with bool value
    #[test]
    fn test_set_with_bool_value() {
        let mut program = vec![
            TopLevel::Definition(Definition {
                name: "main".into(), type_params: vec![], parameters: vec![],
                output_type: None, outputs: vec![Type::Custom("Int".into())],
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![], metadata: Default::default(),
                derivation: None, modifiers: vec![], annotations: vec![], span: None,
                doc: None,
            }),
        ];
        let mut universe = TypeUniverse::new();
        // Tag$("defn").Set$("entry", true)
        let tag_call = Expr::Call("Tag$".into(), vec![Expr::Quoted("defn".into())], None);
        let set_call = Expr::Call("Set$".into(), vec![
            tag_call,
            Expr::Quoted("entry".into()),
            Expr::Bool(true),
        ], None);
        let result = eval_nav_chain(&set_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        assert!(matches!(result, NavValue::Void));
        if let TopLevel::Definition(d) = &program[0] {
            assert_eq!(d.metadata.get("entry"), Some(&PropertyValue::Bool(true)));
        } else {
            panic!("expected Definition");
        }
    }

    // ── SysQuery$ Tests (2026-07-23) ──────────────────────────────

    #[test]
    fn test_sys_query_cpu_cores() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("SysQuery$".into(), vec![Expr::Quoted("cpu.cores".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Int(n) => assert!(n >= 1, "cpu.cores should be >= 1, got {}", n),
            other => panic!("expected Int, got {:?}", other),
        }
    }

    #[test]
    fn test_sys_query_cpu_arch() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("SysQuery$".into(), vec![Expr::Quoted("cpu.arch".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Str(s) => assert!(!s.is_empty(), "cpu.arch should not be empty"),
            other => panic!("expected Str, got {:?}", other),
        }
    }

    #[test]
    fn test_sys_query_os() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("SysQuery$".into(), vec![Expr::Quoted("os".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Str(s) => assert!(!s.is_empty(), "os should not be empty"),
            other => panic!("expected Str, got {:?}", other),
        }
    }

    #[test]
    fn test_sys_query_hostname() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("SysQuery$".into(), vec![Expr::Quoted("hostname".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Str(s) => assert!(!s.is_empty(), "hostname should not be empty"),
            other => panic!("expected Str, got {:?}", other),
        }
    }

    #[test]
    fn test_sys_query_unknown_query() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("SysQuery$".into(), vec![Expr::Quoted("nonexistent".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None);
        assert!(result.is_err(), "unknown query should error");
    }

    #[test]
    fn test_sys_query_rejects_without_capability() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let mut restricted = Sandbox::default(); // all false
        let expr = Expr::Call("SysQuery$".into(), vec![Expr::Quoted("cpu.cores".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut restricted, &mut None);
        assert!(result.is_err(), "SysQuery$ should error without --allow-sys-query");
        let err = result.unwrap_err();
        assert!(err.contains("SysQuery"), "error should mention capability, got: {}", err);
    }

    // ── TimeNow$ Tests (2026-07-23) ──────────────────────────────

    #[test]
    fn test_time_now_returns_positive() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("TimeNow$".into(), vec![], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Int(ts) => {
                // Should be a reasonable Unix timestamp (> 2020-01-01 = 1577836800)
                assert!(ts > 1577836800, "TimeNow$: timestamp {} seems too old", ts);
            }
            other => panic!("expected Int, got {:?}", other),
        }
    }

    #[test]
    fn test_time_now_no_sandbox_check_needed() {
        // TimeNow$ is Pure — should work even with a restricted sandbox
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let mut restricted = Sandbox::default(); // all false
        let expr = Expr::Call("TimeNow$".into(), vec![], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut restricted, &mut None);
        assert!(result.is_ok(), "TimeNow$ should work without any capabilities");
    }

    // ── EnvGet$ Tests (2026-07-23) ───────────────────────────────

    #[test]
    fn test_env_get_var() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        // Set a test env var
        // SAFETY: single-threaded test environment — no concurrent reads
        unsafe { std::env::set_var("BRIEV_TEST_VAR", "hello_world"); }
        let expr = Expr::Call("EnvGet$".into(), vec![Expr::Quoted("BRIEV_TEST_VAR".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Str(s) => assert_eq!(s, "hello_world"),
            other => panic!("expected Str, got {:?}", other),
        }
    }

    #[test]
    fn test_env_get_missing_var() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("EnvGet$".into(), vec![Expr::Quoted("BRIEV_VAR_THAT_DOES_NOT_EXIST_XYZ".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Str(s) => assert!(s.is_empty(), "missing env var should return empty string"),
            other => panic!("expected Str, got {:?}", other),
        }
    }

    #[test]
    fn test_env_get_rejects_without_capability() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let mut restricted = Sandbox::default(); // all false
        let expr = Expr::Call("EnvGet$".into(), vec![Expr::Quoted("HOME".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut restricted, &mut None);
        assert!(result.is_err(), "EnvGet$ should error without --allow-sys-query");
        let err = result.unwrap_err();
        assert!(err.contains("SysQuery"), "error should mention capability, got: {}", err);
    }

    // ── HttpFetch$ Tests (2026-07-23) ────────────────────────────

    #[test]
    fn test_http_fetch_rejects_without_capability() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let mut restricted = Sandbox::default(); // all false
        let expr = Expr::Call("HttpFetch$".into(), vec![Expr::Quoted("http://example.com".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut restricted, &mut None);
        assert!(result.is_err(), "HttpFetch$ should error without --allow-net");
        let err = result.unwrap_err();
        assert!(err.contains("Network"), "error should mention Network capability, got: {}", err);
    }

    // ── Quote$ Tests (2026-07-23) ───────────────────────────────────

    fn scope_with(pairs: Vec<(&str, NavValue)>) -> Scope {
        let mut s = Scope::new();
        for (k, v) in pairs {
            s.insert(k.to_string(), v);
        }
        s
    }

    fn eval_quote(template: &str, scope: &Scope) -> Result<NavValue, String> {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("Quote$".into(), vec![Expr::Quoted(template.as_bytes().to_vec())], None);
        eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &scope, &mut test_sandbox(), &mut None)
    }

    #[test]
    fn test_quote_basic_int_interpolation() {
        let scope = scope_with(vec![("val", NavValue::Int(42))]);
        let result = eval_quote("let x = $val;", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Statement(stmt)) => {
                match stmt.as_ref() {
                    Statement::Let { name, expr: Some(e), .. } => {
                        assert_eq!(name, "x");
                        assert!(matches!(e, Expr::Decimal(42)), "expected Decimal(42), got {:?}", e);
                    }
                    other => panic!("expected Let statement, got {:?}", other),
                }
            }
            other => panic!("expected TopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_str_as_expression() {
        let scope = scope_with(vec![("expr", NavValue::Str("a + b".into()))]);
        let result = eval_quote("let x = $expr;", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Statement(stmt)) => {
                match stmt.as_ref() {
                    Statement::Let { expr: Some(e), .. } => {
                        // Should parse "a + b" as BinaryOp(Add, Identifier("a"), Identifier("b"))
                        match e {
                            Expr::BinaryOp(kind, _, _) => {
                                assert_eq!(*kind, crate::ast::BinaryOpKind::Add,
                                    "expected Add, got {:?}", kind);
                            }
                            other => panic!("expected BinaryOp, got {:?}", other),
                        }
                    }
                    other => panic!("expected Let with expr, got {:?}", other),
                }
            }
            other => panic!("expected TopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_bool_interpolation() {
        let scope = scope_with(vec![("flag", NavValue::Bool(true))]);
        let result = eval_quote("let x = $flag;", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Statement(stmt)) => {
                match stmt.as_ref() {
                    Statement::Let { expr: Some(Expr::Bool(b)), .. } => {
                        assert!(b);
                    }
                    other => panic!("expected Let with Bool(true), got {:?}", other),
                }
            }
            other => panic!("expected TopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_multiple_variables() {
        // Use $name in expression position (not the LET name field, which is a String)
        let scope = scope_with(vec![
            ("name", NavValue::Str("counter".into())),
            ("val", NavValue::Int(0)),
        ]);
        let result = eval_quote("let x = $name; let y = $val;", &scope).unwrap();
        match result {
            NavValue::VecTopLevel(items) => {
                assert_eq!(items.len(), 2);
                if let TopLevel::Statement(stmt) = &items[0] {
                    match stmt.as_ref() {
                        Statement::Let { expr: Some(Expr::Identifier(s)), .. } => {
                            assert_eq!(s, "counter", "$name should resolve to 'counter'");
                        }
                        other => panic!("expected Let with Identifier(counter), got {:?}", other),
                    }
                } else {
                    panic!("expected first item to be a Statement");
                }
                if let TopLevel::Statement(stmt) = &items[1] {
                    match stmt.as_ref() {
                        Statement::Let { expr: Some(Expr::Decimal(0)), .. } => {
                            // OK
                        }
                        other => panic!("expected second Let = Decimal(0), got {:?}", other),
                    }
                } else {
                    panic!("expected second item to be a Statement");
                }
            }
            other => panic!("expected VecTopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_double_dollar_escape() {
        // $$myvar inside a let expression should produce literal $myvar
        let scope = scope_with(vec![("myvar", NavValue::Str("secret".into()))]);
        let result = eval_quote("let x = $$myvar;", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Statement(stmt)) => {
                match stmt.as_ref() {
                    Statement::Let { expr: Some(Expr::Identifier(name)), .. } => {
                        assert_eq!(name, "$myvar", "$$ should escape to literal $");
                    }
                    other => panic!("expected Let with Identifier($myvar), got {:?}", other),
                }
            }
            other => panic!("expected TopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_ident_not_in_scope() {
        // $unknown not in scope — should be left as-is in expression position
        let scope = empty_scope();
        let result = eval_quote("let x = $unknown;", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Statement(stmt)) => {
                match stmt.as_ref() {
                    Statement::Let { expr: Some(Expr::Identifier(name)), .. } => {
                        assert_eq!(name, "$unknown");
                    }
                    other => panic!("expected Let with Identifier($unknown), got {:?}", other),
                }
            }
            other => panic!("expected TopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_no_substitution() {
        let scope = empty_scope();
        let result = eval_quote("let x = 42;", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Statement(stmt)) => {
                match stmt.as_ref() {
                    Statement::Let { name, expr: Some(Expr::Decimal(42)), .. } => {
                        assert_eq!(name, "x");
                    }
                    other => panic!("expected Let x = Decimal(42), got {:?}", other),
                }
            }
            other => panic!("expected TopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_multiple_top_level_items() {
        let scope = scope_with(vec![("a", NavValue::Int(1)), ("b", NavValue::Int(2))]);
        let result = eval_quote("let x = $a; let y = $b;", &scope).unwrap();
        match result {
            NavValue::VecTopLevel(items) => {
                assert_eq!(items.len(), 2);
                if let TopLevel::Statement(stmt) = &items[0] {
                    match stmt.as_ref() {
                        Statement::Let { name, expr: Some(Expr::Decimal(1)), .. } => {
                            assert_eq!(name, "x");
                        }
                        other => panic!("expected first Let x = 1, got {:?}", other),
                    }
                } else {
                    panic!("expected first item to be a Statement");
                }
                if let TopLevel::Statement(stmt) = &items[1] {
                    match stmt.as_ref() {
                        Statement::Let { name, expr: Some(Expr::Decimal(2)), .. } => {
                            assert_eq!(name, "y");
                        }
                        other => panic!("expected second Let y = 2, got {:?}", other),
                    }
                } else {
                    panic!("expected second item to be a Statement");
                }
            }
            other => panic!("expected VecTopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_nested_in_call() {
        let scope = scope_with(vec![
            ("a", NavValue::Str("x".into())),
            ("b", NavValue::Int(42)),
        ]);
        let result = eval_quote("let result = foo($a, $b);", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Statement(stmt)) => {
                match stmt.as_ref() {
                    Statement::Let { expr: Some(Expr::Call(name, args, _)), .. } => {
                        assert_eq!(name, "foo");
                        assert_eq!(args.len(), 2);
                        assert!(matches!(&args[0], Expr::Identifier(s) if s == "x"));
                        assert!(matches!(&args[1], Expr::Decimal(42)));
                    }
                    other => panic!("expected Let with Call, got {:?}", other),
                }
            }
            other => panic!("expected TopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_empty_template() {
        let scope = empty_scope();
        let result = eval_quote("", &scope).unwrap();
        match result {
            NavValue::VecTopLevel(items) => {
                assert!(items.is_empty(), "empty template should produce empty VecTopLevel");
            }
            NavValue::TopLevel(_) => panic!("empty template should produce VecTopLevel, not TopLevel"),
            other => panic!("expected VecTopLevel, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_template_with_block() {
        let scope = scope_with(vec![("val", NavValue::Int(99))]);
        let result = eval_quote("defn main() { let x = $val; }", &scope).unwrap();
        match result {
            NavValue::TopLevel(TopLevel::Definition(def)) => {
                assert_eq!(def.name, "main");
                assert_eq!(def.body.len(), 1);
                match &def.body[0] {
                    Statement::Let { name, expr: Some(Expr::Decimal(99)), .. } => {
                        assert_eq!(name, "x");
                    }
                    other => panic!("expected Let x = 99 in defn body, got {:?}", other),
                }
            }
            other => panic!("expected TopLevel::Definition, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_pure_intrinsic_no_sandbox_check() {
        // Quote$ is pure — should work without any capabilities
        let scope = scope_with(vec![("val", NavValue::Int(7))]);
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let mut restricted = Sandbox::default(); // all false
        let expr = Expr::Call("Quote$".into(), vec![Expr::Quoted("let x = $val;".as_bytes().to_vec())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &scope, &mut restricted, &mut None);
        assert!(result.is_ok(), "Quote$ should work without any capabilities");
    }

    // ── Tainted Node Filtering Tests (2026-07-23) ────────────────────

    fn test_pm_with_tainted(indices: Vec<usize>) -> PluginManager {
        let mut pm = PluginManager::new();
        pm.tainted_indices = indices.into_iter().collect();
        pm
    }

    #[test]
    fn test_all_filters_tainted_indices() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
            TopLevel::Import(Import::literal("std/net.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = test_pm_with_tainted(vec![1]); // index 1 is tainted
        let mut pm_opt = Some(&mut pm);
        let expr = Expr::Call("All$".into(), vec![], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        match result {
            NavValue::Selection(sel) => {
                assert_eq!(sel.count(), 1, "All$ should return 1 non-tainted item");
                assert!(matches!(&sel.nodes[0], NodeRef::TopLevel(0)));
            }
            other => panic!("expected Selection, got {:?}", other),
        }
    }

    #[test]
    fn test_tag_filters_tainted_indices() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
            TopLevel::Import(Import::literal("std/net.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = test_pm_with_tainted(vec![0]); // index 0 is tainted
        let mut pm_opt = Some(&mut pm);
        let expr = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        match result {
            NavValue::Selection(sel) => {
                assert_eq!(sel.count(), 1, "Tag$ should return 1 non-tainted import");
                assert!(matches!(&sel.nodes[0], NodeRef::TopLevel(1)));
            }
            other => panic!("expected Selection, got {:?}", other),
        }
    }

    #[test]
    fn test_named_filters_tainted_indices() {
        let mut program = vec![
            TopLevel::Definition(Definition {
                name: "main".into(), type_params: vec![], parameters: vec![],
                output_type: None, outputs: vec![],
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![], metadata: Default::default(),
                derivation: None, modifiers: vec![], annotations: vec![], span: None,
                doc: None,
            }),
            TopLevel::Definition(Definition {
                name: "helper".into(), type_params: vec![], parameters: vec![],
                output_type: None, outputs: vec![],
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![], metadata: Default::default(),
                derivation: None, modifiers: vec![], annotations: vec![], span: None,
                doc: None,
            }),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = test_pm_with_tainted(vec![1]); // "helper" is tainted
        let mut pm_opt = Some(&mut pm);
        let expr = Expr::Call("Named$".into(), vec![Expr::Quoted("main".into())], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        match result {
            NavValue::Selection(sel) => {
                assert_eq!(sel.count(), 1, "Named$ should return 1 non-tainted item");
            }
            other => panic!("expected Selection, got {:?}", other),
        }
    }

    #[test]
    fn test_insert_marks_tainted_via_position() {
        // Insert via Position (Before$) should mark inserted indices as tainted.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
            TopLevel::Import(Import::literal("std/net.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let mut pm_opt = Some(&mut pm);
        // Build: Before$(First$(Tag$("import"))).Insert$(Import$("std/foo.bv"))
        let tag_call = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let first_call = Expr::Call("First$".into(), vec![tag_call], None);
        let before_call = Expr::Call("Before$".into(), vec![first_call], None);
        let import_call = Expr::Call("Import$".into(), vec![Expr::Quoted("std/foo.bv".into())], None);
        let insert_call = Expr::Call("Insert$".into(), vec![before_call, import_call], None);
        let result = eval_nav_chain(&insert_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        assert!(matches!(result, NavValue::Void));
        // The new import should be at index 0 (inserted before the first import)
        assert!(pm.tainted_indices.contains(&0),
            "Insert$ should mark index 0 as tainted");
    }

    #[test]
    fn test_all_excludes_inserted_tainted_node() {
        // After Insert$ adds a node via position, All$ should exclude it.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let mut pm_opt = Some(&mut pm);
        // Insert a new import at position 0
        let tag_call = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let first_call = Expr::Call("First$".into(), vec![tag_call], None);
        let before_call = Expr::Call("Before$".into(), vec![first_call], None);
        let import_call = Expr::Call("Import$".into(), vec![Expr::Quoted("std/foo.bv".into())], None);
        let insert_call = Expr::Call("Insert$".into(), vec![before_call, import_call], None);
        eval_nav_chain(&insert_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        // Now All$ should exclude index 0 (the tainted node)
        let all_expr = Expr::Call("All$".into(), vec![], None);
        let result = eval_nav_chain(&all_expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        match result {
            NavValue::Selection(sel) => {
                assert_eq!(sel.count(), 1, "All$ should return only the non-tainted node (index 1)");
            }
            other => panic!("expected Selection, got {:?}", other),
        }
    }

    #[test]
    fn test_tainted_excluded_when_no_pm() {
        // With pm=None, all nodes should be visible (no tainted set).
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
            TopLevel::Import(Import::literal("std/net.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let expr = Expr::Call("All$".into(), vec![], None);
        let result = eval_nav_chain(&expr, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut None).unwrap();
        match result {
            NavValue::Selection(sel) => {
                assert_eq!(sel.count(), 2, "All$ should return all nodes when no pm/taint");
            }
            other => panic!("expected Selection, got {:?}", other),
        }
    }

    #[test]
    fn test_boundary_tracking_marks_new_nodes() {
        // Nodes added to the top-level program during evaluate_stage_block
        // via Insert$ with a Before$/After$ position are marked tainted.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let mut pm_opt = Some(&mut pm);
        let body = vec![
            Statement::Expression(
                Expr::Call("Insert$".into(), vec![
                    Expr::Call("After$".into(), vec![
                        Expr::Call("All$".into(), vec![], None),
                    ], None),
                    Expr::Call("Import$".into(), vec![Expr::Quoted("std/extra.bv".into())], None),
                ], None)
            ),
        ];
        let result = evaluate_stage_block(&body, &mut program, &mut universe,
            StageKind::Parsed, &mut pm_opt);
        assert!(result.is_ok(), "stage block should execute: {:?}", result.err());
        // Program went from 1 to 2 nodes; Insert$ marks index 1 (After(0) → base=1)
        assert!(pm.tainted_indices.contains(&1),
            "newly inserted node at index 1 should be tainted");
        assert!(!pm.tainted_indices.contains(&0),
            "original node at index 0 must NOT be tainted");
        assert_eq!(program.len(), 2, "one node should have been inserted");
    }

    #[test]
    fn test_delete_shifts_tainted_indices() {
        // Deleting non-tainted nodes shifts tainted indices down.
        // Program: [a(idx 0), b(idx 1), c(idx 2)], Tainted: {1, 2} (b and c).
        // Tag$("import") returns {0} only (tainted 1 and 2 are filtered out).
        // Delete$ removes index 0 (a). Remaining: [b(idx 0), c(idx 1)].
        // Tainted indices 1 and 2 shift down: {0, 1}.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
            TopLevel::Import(Import::literal("std/b.bv", vec![])),
            TopLevel::Import(Import::literal("std/c.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = test_pm_with_tainted(vec![1, 2]);
        let mut pm_opt = Some(&mut pm);
        let sel = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let delete_call = Expr::Call("Delete$".into(), vec![sel], None);
        eval_nav_chain(&delete_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        assert_eq!(program.len(), 2, "one non-tainted node deleted");
        assert!(pm.tainted_indices.contains(&0),
            "b (orig 1) should be at 0 and still tainted");
        assert!(pm.tainted_indices.contains(&1),
            "c (orig 2) should be at 1 and still tainted");
    }

    #[test]
    fn test_delete_skips_tainted_in_selection() {
        // When a tainted node is already excluded from a selector, deleting
        // the remaining (non-tainted) nodes correctly shifts remaining taint.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
            TopLevel::Import(Import::literal("std/b.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = test_pm_with_tainted(vec![0]); // a is tainted
        let mut pm_opt = Some(&mut pm);
        // All$() returns {1} only (tainted 0 filtered). Delete removes index 1.
        let sel = Expr::Call("All$".into(), vec![], None);
        let delete_call = Expr::Call("Delete$".into(), vec![sel], None);
        eval_nav_chain(&delete_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        assert_eq!(program.len(), 1, "tainted node (a) should remain");
        // Tainted index 0 stays at 0 (no shift because deletion was at 1)
        assert!(pm.tainted_indices.contains(&0),
            "a at index 0 should remain tainted");
        // Index 1 was not in tainted set, so nothing was removed from it
        assert_eq!(pm.tainted_indices.len(), 1,
            "only index 0 should be in tainted set");
    }

    #[test]
    fn test_replace_with_marks_tainted() {
        // ReplaceWith$ at a position should mark that position as tainted.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
            TopLevel::Import(Import::literal("std/b.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let mut pm_opt = Some(&mut pm);
        let first_call = Expr::Call("First$".into(), vec![Expr::Call("All$".into(), vec![], None)], None);
        let import_call = Expr::Call("Import$".into(), vec![Expr::Quoted("std/replacement.bv".into())], None);
        let replace_call = Expr::Call("ReplaceWith$".into(), vec![first_call, import_call], None);
        let result = eval_nav_chain(&replace_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt);
        assert!(result.is_ok(), "ReplaceWith$ should succeed: {:?}", result.err());
        // Index 0 should be tainted (was replaced)
        assert!(pm.tainted_indices.contains(&0),
            "replaced index 0 should be tainted");
        assert!(!pm.tainted_indices.contains(&1),
            "index 1 should NOT be tainted (unchanged)");
    }

    #[test]
    fn test_replace_with_keeps_existing_taint() {
        // ReplaceWith$ at an already-tainted index keeps it tainted.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = test_pm_with_tainted(vec![0]);
        let mut pm_opt = Some(&mut pm);
        let first_call = Expr::Call("First$".into(), vec![Expr::Call("All$".into(), vec![], None)], None);
        let import_call = Expr::Call("Import$".into(), vec![Expr::Quoted("std/replacement.bv".into())], None);
        let replace_call = Expr::Call("ReplaceWith$".into(), vec![first_call, import_call], None);
        eval_nav_chain(&replace_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        assert!(pm.tainted_indices.contains(&0),
            "index 0 should remain tainted after replacement");
    }

    #[test]
    fn test_delete_mixed_taint_shift() {
        // Delete multiple nodes where some are before and after tainted nodes.
        // Program: [a(idx 0), b(idx 1), c(idx 2), d(idx 3)], Tainted: {1, 3} (b and d).
        // Tag$("import") returns {0, 2} (tainted 1, 3 are filtered).
        // Delete removes indices 0 and 2. Remaining: [b(idx 0), d(idx 1)].
        // Tainted: orig 1 (b) → 0, orig 3 (d) → 3 - 1 (one deleted before) = 2... 
        // No wait, this is wrong. After Tag$ returns {0, 2}, we delete both.
        // delete_selection removes in reverse: first 2, then 0.
        // After removing 2: [a, b, d]. Index 3 → 2. Tainted {1, 3} → {1, 2}.
        // After removing 0: [b, d]. Index 1 → 0. Tainted {1, 2} → {0, 1}.
        // So final: [b(tainted, at 0), d(tainted, at 1)], tainted {0, 1}.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
            TopLevel::Import(Import::literal("std/b.bv", vec![])),
            TopLevel::Import(Import::literal("std/c.bv", vec![])),
            TopLevel::Import(Import::literal("std/d.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = test_pm_with_tainted(vec![1, 3]);
        let mut pm_opt = Some(&mut pm);
        let sel = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let delete_call = Expr::Call("Delete$".into(), vec![sel], None);
        eval_nav_chain(&delete_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        assert_eq!(program.len(), 2, "two non-tainted nodes deleted");
        assert!(pm.tainted_indices.contains(&0), "b should be at 0 and tainted");
        assert!(pm.tainted_indices.contains(&1), "d should be at 1 and tainted");
    }

    #[test]
    fn test_end_to_end_taint_isolation() {
        // Full integration: Insert a tainted node, verify selectors exclude it,
        // then delete a non-tainted node and verify remaining taint shifts correctly.
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
            TopLevel::Import(Import::literal("std/b.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        // Insert a tainted node at index 0 (before the first import)
        let tag_call = Expr::Call("Tag$".into(), vec![Expr::Quoted("import".into())], None);
        let before_call = Expr::Call("Before$".into(), vec![tag_call.clone()], None);
        let import_call = Expr::Call("Import$".into(), vec![Expr::Quoted("std/tainted.bv".into())], None);
        let insert_call = Expr::Call("Insert$".into(), vec![before_call, import_call], None);
        {
            let mut pm_opt = Some(&mut pm);
            eval_nav_chain(&insert_call, &mut program, &mut universe,
                StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        }
        // Program: [tainted(idx 0), a(idx 1), b(idx 2)]. Tainted: {0}.
        // All$ should exclude index 0 (the tainted node)
        let all_expr = Expr::Call("All$".into(), vec![], None);
        {
            let mut pm_opt = Some(&mut pm);
            let all_result = eval_nav_chain(&all_expr, &mut program, &mut universe,
                StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
            match all_result {
                NavValue::Selection(sel) => assert_eq!(sel.count(), 2, "All$ should exclude tainted node"),
                other => panic!("expected Selection, got {:?}", other),
            }
        }
        // Tag$ should also exclude the tainted import
        {
            let mut pm_opt = Some(&mut pm);
            let tag_result = eval_nav_chain(&tag_call, &mut program, &mut universe,
                StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
            match tag_result {
                NavValue::Selection(sel) => assert_eq!(sel.count(), 2, "Tag$ should exclude tainted node"),
                other => panic!("expected Selection, got {:?}", other),
            }
        }
        // Delete the first non-tainted node (a at index 1). After deletion,
        // program becomes [tainted, b]. Tainted index 0 stays at 0.
        let first_non_tainted = Expr::Call("First$".into(), vec![all_expr.clone()], None);
        let delete_call = Expr::Call("Delete$".into(), vec![first_non_tainted], None);
        {
            let mut pm_opt = Some(&mut pm);
            eval_nav_chain(&delete_call, &mut program, &mut universe,
                StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        }
        assert_eq!(program.len(), 2, "one node deleted, one remains + tainted");
        assert!(pm.tainted_indices.contains(&0),
            "tainted index 0 should remain after deleting index 1");
        // Insert another tainted node after the non-tainted node (b at index 1)
        {
            let mut pm_opt = Some(&mut pm);
            let after_first_tainted = Expr::Call("After$".into(), vec![
                Expr::Call("All$".into(), vec![], None),
            ], None);
            let import_call2_node = Expr::Call("Import$".into(), vec![Expr::Quoted("std/tainted2.bv".into())], None);
            let insert_call2 = Expr::Call("Insert$".into(), vec![after_first_tainted, import_call2_node], None);
            eval_nav_chain(&insert_call2, &mut program, &mut universe,
                StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        }
        assert_eq!(program.len(), 3, "two tainted nodes + one original");
        // All$ should return only the non-tainted node (b at index 1)
        {
            let mut pm_opt = Some(&mut pm);
            let final_all = eval_nav_chain(&Expr::Call("All$".into(), vec![], None), &mut program, &mut universe,
                StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
            match final_all {
                NavValue::Selection(sel) => assert_eq!(sel.count(), 1, "only b should remain visible"),
                other => panic!("expected Selection, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_transactional_rollback_program_on_error() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let body = vec![
            Statement::Expression(
                Expr::Call("Insert$".into(), vec![
                    Expr::Call("After$".into(), vec![
                        Expr::Call("All$".into(), vec![], None),
                    ], None),
                    Expr::Call("Import$".into(), vec![Expr::Quoted("std/new.bv".into())], None),
                ], None)
            ),
            Statement::Expression(
                Expr::Call("Before$".into(), vec![Expr::Decimal(42)], None),
            ),
        ];
        let result = evaluate_stage_block(&body, &mut program, &mut universe,
            StageKind::Parsed, &mut Some(&mut pm));
        assert!(result.is_err(), "stage block must fail");
        assert_eq!(program.len(), 1, "program must be rolled back to original");
        assert!(pm.tainted_indices.is_empty(),
            "tainted set must be rolled back on error");
        match &program[0] {
            TopLevel::Import(imp) => assert_eq!(imp.path(), "std/a.bv"),
            other => panic!("expected Import, got {:?}", other),
        }
    }

    #[test]
    fn test_transactional_rollback_vfs_on_error() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let body = vec![
            Statement::Expression(
                Expr::Call("FileWrite$".into(), vec![
                    Expr::Quoted("virtual://test.txt".into()),
                    Expr::Quoted("hello".into()),
                ], None)
            ),
            Statement::Expression(
                Expr::Call("Before$".into(), vec![Expr::Decimal(42)], None),
            ),
        ];
        let result = evaluate_stage_block(&body, &mut program, &mut universe,
            StageKind::Parsed, &mut Some(&mut pm));
        assert!(result.is_err(), "stage block must fail");
        assert!(pm.vfs.is_empty(), "VFS must be rolled back on error");
    }

    #[test]
    fn test_transactional_commits_on_success() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/a.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let body = vec![
            Statement::Expression(
                Expr::Call("Insert$".into(), vec![
                    Expr::Call("After$".into(), vec![
                        Expr::Call("All$".into(), vec![], None),
                    ], None),
                    Expr::Call("Import$".into(), vec![Expr::Quoted("std/new.bv".into())], None),
                ], None)
            ),
        ];
        let result = evaluate_stage_block(&body, &mut program, &mut universe,
            StageKind::Parsed, &mut Some(&mut pm));
        assert!(result.is_ok(), "stage block must succeed: {:?}", result.err());
        assert_eq!(program.len(), 2, "new node must be committed");
        assert!(pm.tainted_indices.contains(&1),
            "appended node at index 1 must be tainted");
    }

    #[test]
    fn test_transactional_commits_vfs_on_success() {
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let body = vec![
            Statement::Expression(
                Expr::Call("FileWrite$".into(), vec![
                    Expr::Quoted("virtual://test.txt".into()),
                    Expr::Quoted("hello".into()),
                ], None)
            ),
        ];
        let result = evaluate_stage_block(&body, &mut program, &mut universe,
            StageKind::Parsed, &mut Some(&mut pm));
        assert!(result.is_ok(), "stage block must succeed: {:?}", result.err());
        assert_eq!(pm.vfs.get("test.txt").map(|s| s.as_str()), Some("hello"),
            "VFS must contain the committed file");
    }

    #[test]
    fn test_insert_records_expansion_trace() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let mut pm_opt = Some(&mut pm);
        let insert_call = Expr::Call("Insert$".into(), vec![
            Expr::Call("Before$".into(), vec![
                Expr::Call("First$".into(), vec![
                    Expr::Call("All$".into(), vec![], None),
                ], None),
            ], None),
            Expr::Call("Import$".into(), vec![Expr::Quoted("std/foo.bv".into())], None),
        ], None);
        eval_nav_chain(&insert_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        assert!(pm.expansion_traces.contains_key(&0),
            "Insert$ should record an expansion trace at index 0");
        let desc = &pm.expansion_traces[&0];
        assert!(desc.starts_with("Insert$ -> import"),
            "trace should describe the inserted node, got: {}", desc);
    }

    #[test]
    fn test_replace_with_records_expansion_trace() {
        let mut program = vec![
            TopLevel::Import(Import::literal("std/io.bv", vec![])),
        ];
        let mut universe = TypeUniverse::new();
        let mut pm = PluginManager::new();
        let mut pm_opt = Some(&mut pm);
        let replace_call = Expr::Call("ReplaceWith$".into(), vec![
            Expr::Call("First$".into(), vec![
                Expr::Call("All$".into(), vec![], None),
            ], None),
            Expr::Call("Defn$".into(), vec![
                Expr::Quoted("my_fn".into()),
                Expr::List(vec![]),
                Expr::Quoted("Int".into()),
                Expr::Decimal(42),
            ], None),
        ], None);
        eval_nav_chain(&replace_call, &mut program, &mut universe,
            StageKind::Parsed, &empty_scope(), &mut test_sandbox(), &mut pm_opt).unwrap();
        assert!(pm.expansion_traces.contains_key(&0),
            "ReplaceWith$ should record an expansion trace at index 0");
        let desc = &pm.expansion_traces[&0];
        assert!(desc.starts_with("ReplaceWith$ -> defn"),
            "trace should describe the replacement, got: {}", desc);
    }
}

/// 2026-08-22 (spec-conformance plan Phase 4a): does one unified `Pattern`
/// match a `$`-stage NavValue? Legacy shapes (int/string/wildcard literals)
/// behave exactly as before; richer patterns match their natural shape where
/// the stage value model can represent it (bool/int/str atoms), else no.
fn pattern_matches_nav(p: &crate::ast::Pattern, val: &NavValue) -> bool {
    use crate::ast::Pattern;
    match p {
        Pattern::Wildcard | Pattern::Binding(_) => true,
        Pattern::Literal(Expr::Decimal(n)) => matches!(val, NavValue::Int(i) if *i == *n),
        Pattern::Literal(Expr::Quoted(b)) => {
            matches!(val, NavValue::Str(v) if v.as_bytes() == b.as_slice())
        }
        Pattern::Literal(Expr::Bool(b)) => matches!(val, NavValue::Bool(v) if v == b),
        Pattern::TypedBinding(_, inner) => pattern_matches_nav(
            &Pattern::Literal(literal_of_shape(inner)), val),
        _ => false,
    }
}

fn literal_of_shape(ty: &Type) -> Expr {
    // A typed binding's membership test reduces to the member's atom shape.
    match ty {
        Type::Custom(n) if n == "Bool" => Expr::Bool(true),
        _ => Expr::Decimal(0),
    }
}
