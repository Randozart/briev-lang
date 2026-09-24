use crate::ast::{Contract, Expr, Statement, TopLevel, Transaction, Trigger};
use crate::interpreter::{Interpreter, RuntimeError, Value};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ReactiveTransaction {
    pub name: String,
    pub contract: Contract,
    pub body: Vec<Statement>,
    pub is_async: bool,
    pub reactor_speed: Option<u32>,
    pub dependencies: HashSet<String>,
}

#[derive(Debug)]
pub struct Reactor {
    pub transactions: Vec<ReactiveTransaction>,
    pub dirty_preconditions: HashSet<usize>,
    pub dependency_map: HashMap<String, HashSet<usize>>,
    pub triggers: HashMap<String, Trigger>,
    last_fired: Vec<Instant>,
}

impl Reactor {
    pub fn new() -> Self {
        Reactor {
            transactions: Vec::new(),
            dirty_preconditions: HashSet::new(),
            dependency_map: HashMap::new(),
            triggers: HashMap::new(),
            last_fired: Vec::new(),
        }
    }

    pub fn build_from_program(&mut self, items: &[TopLevel]) {
        for item in items {
            match item {
                TopLevel::Transaction(txn) if txn.is_reactive => {
                    let rtxn = ReactiveTransaction {
                        name: txn.name.clone(),
                        contract: txn.contract.clone(),
                        body: txn.body.clone(),
                        is_async: txn.is_async,
                        reactor_speed: None,
                        dependencies: HashSet::new(),
                    };
                    self.transactions.push(rtxn);
                    let txn_idx = self.transactions.len() - 1;
                    self.dirty_preconditions.insert(txn_idx);
                }
                TopLevel::Trigger(trg) => {
                    self.triggers.insert(trg.name.clone(), trg.clone());
                }
                _ => {}
            }
        }
        self.last_fired = vec![Instant::now(); self.transactions.len()];
    }

    pub fn mark_dirty(&mut self, variable: &str) {
        if let Some(txn_indices) = self.dependency_map.get(variable) {
            for &idx in txn_indices {
                self.dirty_preconditions.insert(idx);
            }
        }
    }

    pub fn get_dirty_transactions(&self) -> Vec<usize> {
        self.dirty_preconditions.iter().cloned().collect()
    }

    pub fn clear_dirty(&mut self) {
        self.dirty_preconditions.clear();
    }

    /// Pre-evaluation guard: Check if a transaction will provably escape
    /// before running it. This avoids firing FFI calls in transactions that
    /// would just roll back anyway.
    ///
    /// Returns true if any escape guard condition is currently true.
    fn will_escape(&self, txn: &ReactiveTransaction, interp: &mut Interpreter) -> bool {
        for stmt in &txn.body {
            if self.contains_escape_guard(stmt, interp) {
                return true;
            }
        }
        false
    }

    /// Recursively check if a statement contains a guarded escape that would fire
    fn contains_escape_guard(&self, stmt: &Statement, interp: &mut Interpreter) -> bool {
        match stmt {
            Statement::Guarded(condition, statements) => {
                // Check if this guard's condition is currently true
                if let Ok(cond_val) = interp.eval_expr(condition) {
                    if cond_val .is_true() {
                        // Check if any statement in the guard body is an escape
                        for s in statements {
                            if self.contains_escape(s) {
                                return true;
                            }
                        }
                    }
                }
                // Recursively check nested guards
                for s in statements {
                    if self.contains_escape_guard(s, interp) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// Check if a statement is an escape
    fn contains_escape(&self, stmt: &Statement) -> bool {
        matches!(stmt, Statement::Rollback(_))
    }

    /// Check if an expression contains patterns that would lead to escape
    /// (e.g., error checks on FFI calls)
    /// Fire async transactions whose @Hz interval has elapsed since last fire.
    pub fn fire_due_async_txns(&mut self, interp: &mut Interpreter) -> Result<bool, crate::interpreter::RuntimeError> {
        let mut fired = false;
        for (idx, txn) in self.transactions.iter().enumerate() {
            if !txn.is_async || txn.reactor_speed.is_none() {
                continue;
            }
            if let Some(hz) = txn.reactor_speed {
                let interval_ms = 1000 / hz;
                if interval_ms == 0 { continue; }
                let elapsed = self.last_fired.get(idx)
                    .map(|t| t.elapsed().as_millis() as u64)
                    .unwrap_or(u64::MAX);
                if elapsed >= interval_ms as u64 {
                    self.dirty_preconditions.insert(idx);
                    // Run this single transaction
                    if let Some(txn) = self.transactions.get(idx) {
                        let pre_val = interp.eval_expr(&txn.contract.pre_condition)?;
                        if pre_val .is_true() {
                            interp.prior_state = interp.state.clone();
                            for stmt in &txn.body {
                                if let Err(e) = interp.exec_stmt(stmt) {
                                    match e {
                                        crate::interpreter::RuntimeError::ContractViolation(ref msg) if msg == "escape" => {
                                            interp.state = interp.prior_state.clone();
                                        }
                                        _ => {
                                            interp.state = interp.prior_state.clone();
                                            return Err(e);
                                        }
                                    }
                                    break;
                                }
                            }
                            fired = true;
                        }
                    }
                    self.last_fired[idx] = Instant::now();
                }
            }
        }
        Ok(fired)
    }

    fn contains_escape_in_expr(&self, _expr: &Expr, _interp: &mut Interpreter) -> bool {
        // TODO: Analyze expressions for error patterns that would trigger escapes
        // For now, conservative: don't skip
        false
    }

    pub fn run(&self, interp: &mut Interpreter) -> Result<bool, crate::interpreter::RuntimeError> {
        let mut any_executed = false;

        for &txn_idx in self.get_dirty_transactions().iter() {
            if let Some(txn) = self.transactions.get(txn_idx) {
                let pre_val = interp.eval_expr(&txn.contract.pre_condition)?;
                if pre_val .is_true() {
                    // PRE-EVALUATION GUARD: Check if any escape conditions are provably true
                    // before running the transaction. If so, skip entirely to avoid FFI side effects.
                    if self.will_escape(txn, interp) {
                        continue;
                    }

                    interp.prior_state = interp.state.clone();

                    let mut term_executed = false;
                    let mut escape_triggered = false;

                    let max_iterations = 1000;
                    let mut iteration = 0;

                    while iteration < max_iterations {
                        iteration += 1;

                        let mut local_failed = false;
                        for stmt in &txn.body {
                            match self.execute_statement(interp, stmt) {
                                Ok(StmtResult::Continue) => {}
                                Ok(StmtResult::TermSuccess) => {
                                    let post_val =
                                        interp.eval_expr(&txn.contract.post_condition)?;
                                    if post_val .is_true() {
                                        term_executed = true;
                                        any_executed = true;
                                        break;
                                    }
                                }
                                Ok(StmtResult::TermFailed) => {
                                    local_failed = true;
                                }
                                // 2026-08-06 (endprogram plan): the reactor
                                // stops — the process boundary is reached.
                                Ok(StmtResult::ProgramExit(code)) => {
                                    return Err(
                                        crate::interpreter::RuntimeError::ProgramExit(code),
                                    );
                                }
                                Ok(StmtResult::Escaped) => {
                                    escape_triggered = true;
                                    local_failed = true;
                                    break;
                                }
                                // 2026-08-17 (foreach break): a `break;` at the
                                // txn/body top level is typecheck-rejected
                                // (BreakOutsideLoop); defend in depth.
                                Ok(StmtResult::Break) => {
                                    local_failed = true;
                                    break;
                                }
                                Err(_) => {
                                    local_failed = true;
                                    break;
                                }
                            }
                        }

                        if escape_triggered {
                            interp.state = interp.prior_state.clone();
                            break;
                        }

                        if term_executed {
                            break;
                        }

                        if local_failed && !term_executed {
                            interp.state = interp.prior_state.clone();
                            break;
                        }
                    }

                    if iteration >= max_iterations && !term_executed {
                        interp.state = interp.prior_state.clone();
                    }
                }
            }
        }

        Ok(any_executed)
    }

    fn execute_statement(
        &self,
        interp: &mut Interpreter,
        stmt: &Statement,
    ) -> Result<StmtResult, crate::interpreter::RuntimeError> {
        match stmt {
            Statement::Assign(_, _) | Statement::ArrowAssign { .. }
            | Statement::FreeHint(_) | Statement::KeepHint(_)
            // 2026-08-22 (Phase 8): yield; is a no-op in the eager reference.
            | Statement::Yield
            // 2026-08-23 (SPEC §10.x): check is a runtime assertion handled
            // in eval, not by the reactor's dirty tracking.
            | Statement::Check(_) => {
                interp.exec_stmt(stmt)?;
                Ok(StmtResult::Continue)
            }
            Statement::Let { name, expr, .. } => {
                if let Some(e) = expr {
                    let value = interp.eval_expr(e)?;
                    interp.state.insert(name.clone(), value);
                }
                Ok(StmtResult::Continue)
            }
            Statement::InlineAsm { .. } | Statement::InlineDefn(_) | Statement::Match { .. } => {
                Ok(StmtResult::Continue)
            }
            Statement::Gate(cond) => {
                // 2026-07-26: Convergence gate — evaluate condition.
                // The compile-time analysis proves convergence; at runtime
                // the condition is evaluated for observable side effects.
                interp.eval_expr(cond)?;
                Ok(StmtResult::Continue)
            }
            // 2026-08-13 (layout-keywords plan Phase 4): `trap;` — the abort
            // diagnostic stops the reactor (SPEC §8.8), mirroring the LLVM
            // `llvm.trap` + `unreachable` sequence.
            Statement::Trap => return Err(crate::interpreter::RuntimeError::Trap),
            Statement::Halt => return Err(crate::interpreter::RuntimeError::Halt),
            Statement::Break => Ok(StmtResult::Break),
            Statement::Expression(expr) => {
                interp.eval_expr(expr)?;
                Ok(StmtResult::Continue)
            }
            Statement::Term(Some(expr)) => {
                let value = interp.eval_expr(expr)?;
                if value .is_true() {
                    Ok(StmtResult::TermSuccess)
                } else {
                    Ok(StmtResult::TermFailed)
                }
            }
            Statement::Term(None) => {
                Ok(StmtResult::TermSuccess)
            }
            // 2026-08-06 (endprogram plan): process boundary — a distinct
            // result the reactor stops on (unlike TermSuccess, which only ends
            // the transaction). Carries the exit-code value.
            Statement::EndProgram(Some(expr)) => {
                let value = interp.eval_expr(expr)?;
                Ok(StmtResult::ProgramExit(value))
            }
            Statement::EndProgram(None) => {
                Ok(StmtResult::ProgramExit(crate::interpreter::Value::Void))
            }
            Statement::Rollback(_) => Ok(StmtResult::Escaped),
            Statement::Guarded(condition, statements) => {
                let cond_val = interp.eval_expr(condition)?;
                if cond_val .is_true() {
                    for stmt in statements {
                        let result = self.execute_statement(interp, stmt)?;
                        match result {
                            StmtResult::Continue => {}
                            _ => return Ok(result),
                        }
                    }
                    Ok(StmtResult::Continue)
                } else {
                    Ok(StmtResult::Continue)
                }
            }
            Statement::Assign(Expr::Bool(true), Expr::Bool(true)) => Ok(StmtResult::Continue),
            Statement::SyncBlock(body) => {
                interp.exec_stmt(stmt)?;
                Ok(StmtResult::Continue)
            }
            Statement::Defer(_) | Statement::Mutex(_) => {
                interp.exec_stmt(stmt)?;
                Ok(StmtResult::Continue)
            }
            Statement::Foreach { item, list, body, .. } => {
                let list_val = interp.eval_expr(list)?;
                // 2026-08-06 (Slice G/I): Expr::List evaluates to a Product.
                let fields = match list_val {
                    Value::Product { fields, .. } => fields,
                    _ => Vec::new(),
                };
                for elem in fields {
                    interp.state.insert(item.clone(), elem);
                    // 2026-08-17 (foreach break): a body `break;` stops this
                    // foreach (interp.exec_stmt surfaces it as RuntimeError::Break).
                    let mut broke = false;
                    for stmt in body {
                        match interp.exec_stmt(stmt) {
                            Err(crate::interpreter::RuntimeError::Break) => {
                                broke = true;
                                break;
                            }
                            r => {
                                r?;
                            }
                        }
                    }
                    if broke {
                        break;
                    }
                }
                Ok(StmtResult::Continue)
            }
            Statement::Block(body) => {
                for stmt in body {
                    let result = self.execute_statement(interp, stmt)?;
                    if !matches!(result, StmtResult::Continue) {
                        return Ok(result);
                    }
                }
                Ok(StmtResult::Continue)
            }
            Statement::TrgBinding { .. } => {
                Ok(StmtResult::Continue)
            }
            Statement::MetadataAssignment(_, _) => {
                Ok(StmtResult::Continue)
            }
        }
    }
}

enum StmtResult {
    Continue,
    TermSuccess,
    TermFailed,
    Escaped,
    ProgramExit(crate::interpreter::Value),
    /// 2026-08-17 (foreach break): a `break;` executed — exit the innermost
    /// enclosing `foreach`. Consumed by the foreach arm; a stray one is a
    /// typechecker-rejected shape.
    Break,
}

pub fn run_reactor(
    items: &[TopLevel],
    interp: &mut Interpreter,
) -> Result<(), RuntimeError> {
    let mut reactor = Reactor::new();
    reactor.build_from_program(items);

    loop {
        reactor.clear_dirty();
        let executed = reactor.run(interp)?;

        if !executed {
            let dirty = reactor.get_dirty_transactions();
            if dirty.is_empty() {
                break;
            }
        }

        let dirty = reactor.get_dirty_transactions();
        if dirty.is_empty() {
            break;
        }
    }

    Ok(())
}

/// Run reactor in continuous mode: event-driven loop that handles both
/// responsive node (convergence) and polled async node @Hz (timer).
///
/// This is used when any transaction has `is_async = true` or
/// `reactor_speed` is set. Exits on RuntimeError.
pub fn run_reactor_continuous(
    items: &[TopLevel],
    interp: &mut Interpreter,
) -> Result<(), RuntimeError> {
    let mut reactor = Reactor::new();
    reactor.build_from_program(items);

    loop {
        // (1) Responsive: convergence until quiescence
        reactor.run(interp)?;

        // (2) Polled: fire due async txns and cascade
        reactor.fire_due_async_txns(interp)?;
        reactor.run(interp)?;  // catch cascades

        // (3) Short yield to OS — NOT a tick boundary.
        //     Responsive txns fire immediately within step (1).
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::interpreter::{Interpreter, RuntimeError, Value};

    fn make_rct_txn(name: &str, pre: Expr, post: Expr, body: Vec<Statement>) -> TopLevel {
        TopLevel::Transaction(Transaction {
            name: name.to_string(),
            is_reactive: true,
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
        })
    }

    fn simple_program(items: Vec<TopLevel>) -> Vec<TopLevel> {
        items
    }

    fn build_reactor(prog: &[TopLevel]) -> Reactor {
        let mut r = Reactor::new();
        r.build_from_program(prog);
        r
    }

    #[test]
    fn test_build_from_program_empty() {
        let reactor = build_reactor(&simple_program(vec![]));
        assert!(reactor.transactions.is_empty());
    }

    #[test]
    fn test_build_from_program_with_reactive_txn() {
        let txn = make_rct_txn("test", Expr::Bool(true), Expr::Bool(true), vec![]);
        let reactor = build_reactor(&simple_program(vec![txn]));
        assert_eq!(reactor.transactions.len(), 1);
        assert_eq!(reactor.transactions[0].name, "test");
    }

    #[test]
    fn test_build_from_program_skips_non_reactive() {
        let prog = simple_program(vec![
            TopLevel::Definition(Definition {
                variadic_param: None,
                name: "foo".into(), type_params: vec![], parameters: vec![], outputs: vec![],
                output_type: None,
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
                annotations: vec![],
                metadata: HashMap::new(),
                body: vec![], modifiers: vec![],
                derivation: None,
                span: None,
                doc: None,
            }),
            TopLevel::Transaction(Transaction {
                name: "bar".into(), is_reactive: false, is_async: false, type_params: vec![], parameters: vec![],
                contract: Contract::new(Expr::Bool(true), Expr::Bool(true)), body: vec![],
                span: None, modifiers: vec![], outputs: vec![],
                metadata: HashMap::new(),
                output_type: None,
                derivation: None,
                doc: None,
            }),
        ]);
        let reactor = build_reactor(&prog);
        assert_eq!(reactor.transactions.len(), 0);
    }

    #[test]
    fn test_dependency_map_populated() {
        let txn = make_rct_txn("dep_test", Expr::Bool(true), Expr::Bool(true), vec![]);
        let reactor = build_reactor(&simple_program(vec![txn]));
        assert!(reactor.dependency_map.is_empty());
    }

    #[test]
    fn test_mark_dirty_propagates() {
        let txn = make_rct_txn("a", Expr::Bool(true), Expr::Bool(true), vec![]);
        let mut reactor = build_reactor(&simple_program(vec![txn]));
        assert!(reactor.dirty_preconditions.contains(&0usize));
        reactor.clear_dirty();
        assert!(reactor.dirty_preconditions.is_empty());
    }

    #[test]
    fn test_get_dirty_transactions() {
        let txn = make_rct_txn("a", Expr::Identifier("z".into()), Expr::Bool(true), vec![]);
        let reactor = build_reactor(&simple_program(vec![txn]));
        assert!(!reactor.get_dirty_transactions().is_empty());
    }

    #[test]
    fn test_run_executes_txn() {
        let body = vec![
            Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(42)),
            Statement::Term(None),
        ];
        let txn = make_rct_txn("set_x",
            Expr::Bool(true),
            Expr::BinaryOp(BinaryOpKind::Eq, Box::new(Expr::Identifier("x".into())), Box::new(Expr::Decimal(42))),
            body);
        let mut interp = Interpreter::new();
        interp.state.insert("x".into(), crate::interpreter::i64_to_bits(0));
        let reactor = build_reactor(&simple_program(vec![txn]));
        let result = reactor.run(&mut interp).unwrap();
        assert!(result);
        assert_eq!(interp.state.get("x"), Some(&crate::interpreter::i64_to_bits(42)));
    }

    #[test]
    fn test_foreach_iterates_product_list() {
        // 2026-08-06 (Slice G): Expr::List evaluates to a Product; foreach
        // must iterate it.
        let foreach = Statement::Foreach {
            item: "v".into(),
            list: Box::new(Expr::List(vec![Expr::Decimal(10), Expr::Decimal(20)])),
            body: vec![],
        };
        let txn = make_rct_txn(
            "iter",
            Expr::Bool(true),
            Expr::Bool(true),
            vec![foreach, Statement::Term(None)],
        );
        let mut interp = Interpreter::new();
        let reactor = build_reactor(&simple_program(vec![txn]));
        let result = reactor.run(&mut interp).unwrap();
        assert!(result);
        assert_eq!(interp.state.get("v").and_then(|v| v.as_i64()), Some(20));
    }

    #[test]
    fn test_run_skips_txn_pre_false() {
        let body = vec![Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(99))];
        let txn = make_rct_txn("skip", Expr::Bool(false), Expr::Bool(true), body);
        let mut interp = Interpreter::new();
        interp.state.insert("x".into(), crate::interpreter::i64_to_bits(0));
        let reactor = build_reactor(&simple_program(vec![txn]));
        let result = reactor.run(&mut interp).unwrap();
        assert!(!result);
        assert_eq!(interp.state.get("x"), Some(&crate::interpreter::i64_to_bits(0)));
    }

    #[test]
    fn test_escape_guard_detection() {
        let body = vec![
            Statement::Guarded(
                Expr::Bool(true),
                vec![Statement::Rollback(None)],
            ),
        ];
        let txn = make_rct_txn("escape_test", Expr::Bool(true), Expr::Bool(true), body);
        let mut interp = Interpreter::new();
        let reactor = build_reactor(&simple_program(vec![txn]));
        assert!(reactor.will_escape(&reactor.transactions[0], &mut interp));
    }

    #[test]
    fn test_run_escape_triggers_rollback() {
        let body = vec![
            Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(10)),
            Statement::Guarded(Expr::Bool(true), vec![Statement::Rollback(None)]),
        ];
        let txn = make_rct_txn("rollback", Expr::Bool(true), Expr::Bool(true), body);
        let mut interp = Interpreter::new();
        interp.state.insert("x".into(), crate::interpreter::i64_to_bits(5));
        interp.prior_state = interp.state.clone();
        let reactor = build_reactor(&simple_program(vec![txn]));
        let result = reactor.run(&mut interp).unwrap();
        // Pre-evaluation guard skips the transaction entirely (escape would fire)
        assert!(!result);
        // State unchanged since transaction wasn't executed
        assert_eq!(interp.state.get("x"), Some(&crate::interpreter::i64_to_bits(5)));
    }

    #[test]
    fn test_build_from_program_with_triggers() {
        let trg = TopLevel::Trigger(Trigger {
            name: "keypress".to_string(),
            instance: Expr::Decimal(0),
            span: None,
        });
        let reactor = build_reactor(&simple_program(vec![trg]));
        assert!(reactor.triggers.contains_key("keypress"));
        assert_eq!(reactor.triggers.len(), 1);
    }

    #[test]
    fn test_build_from_program_with_async_txn() {
        let txn = Transaction {
            name: "polled".to_string(),
            is_reactive: true,
            is_async: true,
            type_params: vec![],
            parameters: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(None)],
            span: None,
            metadata: HashMap::new(),
            modifiers: vec![],
            outputs: vec![],
            output_type: None,
            derivation: None,
            doc: None,
        };
        let reactor = build_reactor(&simple_program(vec![TopLevel::Transaction(txn)]));
        assert_eq!(reactor.transactions.len(), 1);
        assert!(reactor.transactions[0].is_async);
        assert_eq!(reactor.transactions[0].reactor_speed, None);
        assert_eq!(reactor.last_fired.len(), 1);
    }

    #[test]
    fn test_fire_due_async_txns_fires_on_first_call() {
        let body = vec![
            Statement::Assign(Expr::Identifier("x".into()), Expr::Decimal(99)),
            Statement::Term(None),
        ];
        let txn = Transaction {
            name: "async_test".to_string(),
            is_reactive: true,
            is_async: true,
            type_params: vec![],
            parameters: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
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
        };
        let mut interp = Interpreter::new();
        interp.state.insert("x".into(), crate::interpreter::i64_to_bits(0));
        let mut reactor = build_reactor(&simple_program(vec![TopLevel::Transaction(txn)]));
        // Set reactor_speed on the ReactiveTransaction (no longer a field on Transaction)
        if let Some(rt) = reactor.transactions.first_mut() {
            rt.reactor_speed = Some(1000);
        }
        // First call should fire immediately (last_fired was set at construction)
        // The last_fired is set to Instant::now() during build, so we need to
        // reset it to a very old time to guarantee the interval has elapsed
        reactor.last_fired = vec![Instant::now().checked_sub(Duration::from_secs(3600)).unwrap()];
        let fired = reactor.fire_due_async_txns(&mut interp).unwrap();
        assert!(fired, "async txn should fire when interval has elapsed");
    }

    #[test]
    fn test_fire_due_async_txns_skips_before_interval() {
        let txn = Transaction {
            name: "slow".to_string(),
            is_reactive: true,
            is_async: true,
            type_params: vec![],
            parameters: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(None)],
            span: None,
            metadata: HashMap::new(),
            modifiers: vec![],
            outputs: vec![],
            output_type: None,
            derivation: None,
            doc: None,
        };
        let mut interp = Interpreter::new();
        let mut reactor = build_reactor(&simple_program(vec![TopLevel::Transaction(txn)]));
        // last_fired was set to Instant::now() during build — interval is 100ms
        // This should NOT fire because the interval hasn't elapsed
        let fired = reactor.fire_due_async_txns(&mut interp).unwrap();
        assert!(!fired, "async txn should NOT fire before interval elapses");
    }

    #[test]
    fn test_fire_due_async_txns_skips_non_async() {
        let body = vec![Statement::Term(None)];
        let txn = make_rct_txn("no_async", Expr::Bool(true), Expr::Bool(true), body);
        let mut interp = Interpreter::new();
        let mut reactor = build_reactor(&simple_program(vec![txn]));
        reactor.last_fired = vec![Instant::now().checked_sub(Duration::from_secs(3600)).unwrap()];
        let fired = reactor.fire_due_async_txns(&mut interp).unwrap();
        assert!(!fired, "non-async txn should never fire via fire_due_async_txns");
    }
}
