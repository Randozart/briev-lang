// ── Destination ownership body walker ─────────────────────────────────
// 2026-10-29 (Phase R1): semantic expression/statement traversal for
// `BodyAnalyzer`. Split into category helpers so each destination rule has a
// single responsibility. The backend does not consume this analysis yet.

use crate::analysis::destination_ownership::*;
use crate::ast::{Expr, Statement};
use std::collections::{BTreeSet, HashMap};

/// Coarse expression category. This keeps the source-order traversal
/// deterministic while keeping each rule short enough to audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprCategory {
    Leaf,
    Reference,
    Operator,
    CallLike,
    Projection,
    Composite,
    Control,
    Unknown,
}

impl ExprCategory {
    fn classify(expr: &Expr) -> Self {
        match expr {
            Expr::Quoted(_)
            | Expr::Decimal(_)
            | Expr::Char(_)
            | Expr::TaggedLiteral(_, _)
            | Expr::TaggedQuotedLiteral(_, _)
            | Expr::Bool(_)
            | Expr::Float(_)
            | Expr::BeginProgram
            | Expr::Wildcard => Self::Leaf,
            Expr::Identifier(_) => Self::Reference,
            Expr::BinaryOp(..)
            | Expr::UnaryOp(..)
            | Expr::Field(..)
            | Expr::Reflect(..)
            | Expr::Index(..)
            | Expr::Slice { .. }
            | Expr::Range { .. }
            | Expr::Cast(..)
            | Expr::Within(..) => Self::Operator,
            Expr::Call(..)
            | Expr::MethodCall(..)
            | Expr::Spawn { .. }
            | Expr::Await(..)
            | Expr::PluginIntercept { .. } => Self::CallLike,
            Expr::Tuple(..)
            | Expr::List(..)
            | Expr::StructLiteral { .. }
            | Expr::Lambda(..)
            | Expr::Deref(..)
            | Expr::AddrOf(..)
            | Expr::Consume(..) => Self::Composite,
            Expr::Block(..) | Expr::If(..) | Expr::Match(..) => Self::Control,
            _ => Self::Unknown,
        }
    }
}

pub(super) struct ExprCursor<'a> {
    pub(super) body_path: &'a str,
    pub(super) path: &'a mut Vec<usize>,
}

pub(super) struct StmtCursor<'a> {
    pub(super) body_path: &'a str,
    pub(super) path: &'a mut Vec<usize>,
}

impl<'a> StmtCursor<'a> {
    pub(super) fn new(body_path: &'a str, path: &'a mut Vec<usize>) -> Self {
        StmtCursor { body_path, path }
    }

    pub(super) fn expr_cursor<'b>(&'b mut self, expr_path: &'b mut Vec<usize>) -> ExprCursor<'b> {
        ExprCursor::new(self.body_path, expr_path)
    }
}

impl<'a> ExprCursor<'a> {
    pub(super) fn new(body_path: &'a str, path: &'a mut Vec<usize>) -> Self {
        ExprCursor { body_path, path }
    }
}

impl<'a> BodyAnalyzer<'a> {
    pub(super) fn expr(
        &mut self,
        expr: &Expr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match ExprCategory::classify(expr) {
            ExprCategory::Leaf => self.expr_leaf(cursor),
            ExprCategory::Reference => self.expr_reference(expr, cursor, scope),
            ExprCategory::Operator => self.expr_operator(expr, cursor, scope),
            ExprCategory::CallLike => self.expr_call_like(expr, cursor, scope),
            ExprCategory::Projection => self.expr_projection(expr, cursor, scope),
            ExprCategory::Composite => self.expr_projection(expr, cursor, scope),
            ExprCategory::Control => self.expr_control(expr, cursor, scope),
            ExprCategory::Unknown => self.expr_unknown(cursor),
        }
    }

    fn stmt_expr(
        &mut self,
        expr: &Expr,
        body_path: &str,
        expr_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> ValueId {
        expr_path.clear();
        expr_path.push(self.expr_serial as usize);
        self.expr_serial += 1;
        let mut cursor = ExprCursor::new(body_path, expr_path);
        self.expr(expr, &mut cursor, scope)
    }

    pub(super) fn temp_value(
        &mut self,
        cursor: &mut ExprCursor,
        origin: ValueOrigin,
        class: DestinationClass,
    ) -> ValueId {
        let path = format!("{}/expr/{}", cursor.body_path, format_path(cursor.path));
        let id = ValueId::new(self.owner.clone(), path);
        self.fact(id.clone(), origin, class, None)
    }

    fn expr_leaf(&mut self, cursor: &mut ExprCursor) -> ValueId {
        self.temp_value(cursor, ValueOrigin::Literal, DestinationClass::ScalarValue)
    }

    fn expr_reference(
        &mut self,
        expr: &Expr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let Expr::Identifier(name) = expr else {
            return self.expr_unknown(cursor);
        };
        if let Some(id) = scope.lookup(name) {
            self.use_value(&id);
            return id;
        }
        let path = format!("{}/external/{}", cursor.body_path, sanitize_path(name));
        let id = ValueId::new(self.owner.clone(), path);
        self.fact(
            id.clone(),
            ValueOrigin::StateOrGlobalLoad,
            DestinationClass::Boxed,
            Some(name.clone()),
        )
    }

    fn expr_operator(
        &mut self,
        expr: &Expr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match expr {
            Expr::BinaryOp(_, lhs, rhs) => {
                let lhs_id = self.child_expr(lhs, 0, cursor, scope);
                let rhs_id = self.child_expr(rhs, 1, cursor, scope);
                self.use_value(&lhs_id);
                self.use_value(&rhs_id);
                self.temp_value(
                    cursor,
                    ValueOrigin::BinaryResult,
                    DestinationClass::ScalarValue,
                )
            }
            Expr::UnaryOp(_, inner) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(
                    cursor,
                    ValueOrigin::UnaryResult,
                    DestinationClass::ScalarValue,
                )
            }
            Expr::Cast(inner, _) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(
                    cursor,
                    ValueOrigin::CastResult,
                    DestinationClass::ScalarValue,
                )
            }
            Expr::Field(inner, _) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::FieldLoad, DestinationClass::Boxed)
            }
            Expr::Reflect(inner, _, _) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
            }
            Expr::Index(base, index) => {
                let base_id = self.child_expr(base, 0, cursor, scope);
                let index_id = self.child_expr(index, 1, cursor, scope);
                self.use_value(&base_id);
                self.use_value(&index_id);
                self.temp_value(cursor, ValueOrigin::CollectionLoad, DestinationClass::Boxed)
            }
            Expr::Slice {
                array,
                start,
                end,
                stride,
            } => {
                let array_id = self.child_expr(array, 0, cursor, scope);
                self.use_value(&array_id);
                for (index, part) in [start, end, stride].into_iter().enumerate() {
                    if let Some(part) = part {
                        let id = self.child_expr(part, index + 1, cursor, scope);
                        self.use_value(&id);
                    }
                }
                self.temp_value(cursor, ValueOrigin::Composite, DestinationClass::Boxed)
            }
            Expr::Range { start, end, .. } => {
                let start_id = self.child_expr(start, 0, cursor, scope);
                let end_id = self.child_expr(end, 1, cursor, scope);
                self.use_value(&start_id);
                self.use_value(&end_id);
                self.temp_value(
                    cursor,
                    ValueOrigin::Composite,
                    DestinationClass::ScalarValue,
                )
            }
            Expr::Within(lhs, rhs) => {
                let lhs_id = self.child_expr(lhs, 0, cursor, scope);
                let rhs_id = self.child_expr(rhs, 1, cursor, scope);
                self.use_value(&lhs_id);
                self.use_value(&rhs_id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
            }
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_call_like(
        &mut self,
        expr: &Expr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match expr {
            Expr::Call(name, args, _) => self.expr_call(name, args, cursor, scope),
            Expr::MethodCall(receiver, _, args, _, _) => {
                self.expr_method(receiver, args, cursor, scope)
            }
            Expr::Spawn { args, .. } => self.expr_spawn(args, cursor, scope),
            Expr::Await(inner) => {
                let task = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&task);
                self.temp_value(cursor, ValueOrigin::TaskResult, DestinationClass::Boxed)
            }
            Expr::PluginIntercept { .. } => self.expr_unknown(cursor),
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_call(
        &mut self,
        name: &str,
        args: &[Expr],
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let mut argument_ids = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let arg_id = self.child_expr(arg, index, cursor, scope);
            argument_ids.push(arg_id);
        }

        let site = format!(
            "{}/expr/{}/call",
            cursor.body_path,
            format_path(cursor.path)
        );
        let site_id = self.call_site_id(&site);
        let summary = self.summarize_site(&site, name, &argument_ids);

        for (argument_id, obligation) in argument_ids.iter().zip(&summary.obligations) {
            self.use_value(argument_id);
            // Primitive by-value parameters cannot retain a handle. Every
            // non-primitive obligation (including Borrow) may outlive the
            // caller's frame: until a callee escape summary proves otherwise,
            // treat the argument as escaping.
            if *obligation != ParamObligation::Value {
                self.add_sink(
                    argument_id,
                    SinkKind::UnknownCallArgument,
                    &site,
                    EscapeKind::UnknownCallLifetime,
                );
            }
        }

        if summary.result == ResultDestination::Unknown {
            let fallback = argument_ids.first().cloned().unwrap_or_else(|| {
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
            });
            self.diagnostic(
                &fallback,
                DestinationDiagnosticCode::UnknownCallResult,
                format!("call to '{name}' has no destination summary; result is boxed"),
            );
        }
        if summary.obligations.contains(&ParamObligation::Unknown) {
            let fallback = argument_ids.first().cloned().unwrap_or_else(|| {
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
            });
            self.diagnostic(
                &fallback,
                DestinationDiagnosticCode::UnknownCallArgument,
                format!("call to '{name}' has an unknown parameter obligation"),
            );
        }

        let result = self.temp_value(cursor, ValueOrigin::UnknownCall, DestinationClass::Boxed);
        self.call_sites.insert(site_id, summary);
        result
    }

    fn expr_method(
        &mut self,
        receiver: &Expr,
        args: &[Expr],
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let receiver_id = self.child_expr(receiver, 0, cursor, scope);
        self.use_value(&receiver_id);
        let site = format!(
            "{}/expr/{}/method",
            cursor.body_path,
            format_path(cursor.path)
        );
        self.add_sink(
            &receiver_id,
            SinkKind::UnknownCallArgument,
            &site,
            EscapeKind::UnknownCallLifetime,
        );
        for (index, arg) in args.iter().enumerate() {
            let arg_id = self.child_expr(arg, index + 1, cursor, scope);
            self.add_sink(
                &arg_id,
                SinkKind::UnknownCallArgument,
                &site,
                EscapeKind::UnknownCallLifetime,
            );
        }
        self.temp_value(cursor, ValueOrigin::UnknownCall, DestinationClass::Boxed)
    }

    fn expr_spawn(&mut self, args: &[Expr], cursor: &mut ExprCursor, scope: &mut Scope) -> ValueId {
        let mut argument_ids = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            let arg_id = self.child_expr(arg, index, cursor, scope);
            argument_ids.push(arg_id);
        }
        let site = format!(
            "{}/expr/{}/spawn",
            cursor.body_path,
            format_path(cursor.path)
        );
        for argument_id in &argument_ids {
            self.add_sink(
                argument_id,
                SinkKind::UnknownCallArgument,
                &site,
                EscapeKind::UnknownCallLifetime,
            );
        }
        self.temp_value(cursor, ValueOrigin::UnknownCall, DestinationClass::Boxed)
    }

    fn expr_projection(
        &mut self,
        expr: &Expr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match expr {
            Expr::Tuple(values) => {
                for (index, value) in values.iter().enumerate() {
                    let id = self.child_expr(value, index, cursor, scope);
                    self.use_value(&id);
                }
                self.temp_value(
                    cursor,
                    ValueOrigin::Composite,
                    DestinationClass::PackedMemory,
                )
            }
            Expr::List(values) => {
                for (index, value) in values.iter().enumerate() {
                    let _ = self.child_expr(value, index, cursor, scope);
                }
                self.temp_value(cursor, ValueOrigin::Composite, DestinationClass::Boxed)
            }
            Expr::StructLiteral {
                type_name,
                fields,
                specs,
            } => {
                for (index, (_, value)) in fields.iter().chain(specs.iter()).enumerate() {
                    let _ = self.child_expr(value, index, cursor, scope);
                }
                let id = self.temp_value(
                    cursor,
                    ValueOrigin::Constructor,
                    DestinationClass::ScalarReplacedObject,
                );
                self.set_fact_type(&id, Type::Custom(type_name.clone()));
                id
            }
            Expr::Lambda(_, body) => self.expr_lambda(body, cursor, scope),
            Expr::Deref(inner) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Boxed)
            }
            Expr::AddrOf(inner) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Boxed)
            }
            Expr::Consume(inner) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
            }
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_lambda(&mut self, body: &Expr, cursor: &mut ExprCursor, scope: &mut Scope) -> ValueId {
        let mut captures = Vec::new();
        walk_expression_identifiers(body, &mut |name| {
            if let Some(id) = scope.lookup(name) {
                captures.push(id.clone());
            }
        });
        let site = format!(
            "{}/expr/{}/lambda",
            cursor.body_path,
            format_path(cursor.path)
        );
        for capture in captures {
            self.add_sink(
                &capture,
                SinkKind::ClosureCapture,
                &site,
                EscapeKind::ClosureLifetime,
            );
        }
        self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Boxed)
    }

    fn expr_control(&mut self, expr: &Expr, cursor: &mut ExprCursor, scope: &mut Scope) -> ValueId {
        match expr {
            Expr::Block(body) => self.expr_block_value(body, cursor, scope),
            Expr::If(condition, then_body, else_body) => {
                self.expr_if((condition, then_body, else_body.as_deref()), cursor, scope)
            }
            Expr::Match(scrut, arms) => self.expr_match(scrut, arms, cursor, scope),
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_block_value(
        &mut self,
        body: &[Statement],
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let mut child_scope = scope.clone();
        let mut value = self.expr_unknown(cursor);
        for (index, statement) in body.iter().enumerate() {
            let mut path = vec![index];
            if let Some(last) = self.stmt(statement, cursor.body_path, &mut path, &mut child_scope)
            {
                value = last;
            }
        }
        value
    }

    fn expr_if(
        &mut self,
        parts: (&Expr, &Expr, Option<&Expr>),
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let (condition, then_expr, else_expr) = parts;
        let condition_id = self.child_expr(condition, 0, cursor, scope);
        self.use_value(&condition_id);
        let then_id = self.child_expr(then_expr, 1, cursor, scope);
        self.use_value(&then_id);
        let else_id = match else_expr {
            Some(else_expr) => {
                let id = self.child_expr(else_expr, 2, cursor, scope);
                self.use_value(&id);
                id
            }
            None => self.expr_unknown(cursor),
        };
        let then_class = self.facts.get(&then_id).map(|fact| fact.class);
        let else_class = self.facts.get(&else_id).map(|fact| fact.class);
        if then_class == Some(DestinationClass::ScalarValue)
            && else_class == Some(DestinationClass::ScalarValue)
        {
            self.temp_value(
                cursor,
                ValueOrigin::MatchResult,
                DestinationClass::ScalarValue,
            )
        } else {
            self.temp_value(cursor, ValueOrigin::MatchResult, DestinationClass::Boxed)
        }
    }

    fn expr_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[crate::ast::MatchArm],
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let scrutinee_id = self.child_expr(scrutinee, 0, cursor, scope);
        self.use_value(&scrutinee_id);
        let mut results = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            let mut child_scope = scope.clone();
            bind_pattern(&arm.pattern, &scrutinee_id, &mut child_scope);
            let result = self.child_expr(&arm.body, index + 1, cursor, scope);
            results.push(result);
        }
        let all_scalar = results.iter().all(|result| {
            self.facts.get(result).map(|fact| fact.class) == Some(DestinationClass::ScalarValue)
        });
        let class = if all_scalar {
            DestinationClass::ScalarValue
        } else {
            DestinationClass::Boxed
        };
        self.temp_value(cursor, ValueOrigin::MatchResult, class)
    }

    fn expr_unknown(&mut self, cursor: &mut ExprCursor) -> ValueId {
        self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
    }

    fn child_expr(
        &mut self,
        expr: &Expr,
        index: usize,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        cursor.path.push(index);
        let value = self.expr(expr, cursor, scope);
        cursor.path.pop();
        value
    }

    pub(crate) fn stmt(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let category = StatementCategory::classify(statement);
        match category {
            StatementCategory::Binding => self.stmt_binding(statement, body_path, stmt_path, scope),
            StatementCategory::Assignment => {
                self.stmt_assignment(statement, body_path, stmt_path, scope)
            }
            StatementCategory::Return => self.stmt_return(statement, body_path, stmt_path, scope),
            StatementCategory::Iteration => {
                self.stmt_iteration(statement, body_path, stmt_path, scope)
            }
            StatementCategory::Control => self.stmt_control(statement, body_path, stmt_path, scope),
            StatementCategory::Effect => self.stmt_effect(statement, body_path, stmt_path, scope),
            StatementCategory::Other => self.stmt_other(statement, body_path, stmt_path, scope),
        }
    }

    fn stmt_binding(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let Statement::Let {
            name, names, expr, ..
        } = statement
        else {
            return None;
        };
        let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
        let mut value_id = None;
        if let Some(initializer) = expr {
            let mut expr_path = Vec::new();
            let produced = self.stmt_expr(initializer, body_path, &mut expr_path, scope);
            self.use_value(&produced);
            let origin = self
                .facts
                .get(&produced)
                .and_then(|fact| fact.origins.iter().next_back().copied())
                .unwrap_or(ValueOrigin::Unknown);
            let class = self
                .facts
                .get(&produced)
                .map(|fact| fact.class)
                .unwrap_or(DestinationClass::Unknown);
            let ty = self.facts.get(&produced).and_then(|fact| fact.ty.clone());
            let binding = ValueId::new(
                self.owner.clone(),
                format!("{site}/let/{}", sanitize_path(name)),
            );
            self.fact(binding.clone(), origin, class, Some(name.clone()));
            if let Some(ty) = &ty {
                self.set_fact_type(&binding, ty.clone());
            }
            self.use_value(&produced);
            scope.define(name, binding.clone());
            value_id = Some(binding);
            for extra in names {
                let extra_id = ValueId::new(
                    self.owner.clone(),
                    format!("{site}/let/{}", sanitize_path(extra)),
                );
                self.fact(extra_id.clone(), origin, class, Some(extra.clone()));
                if let Some(ty) = &ty {
                    self.set_fact_type(&extra_id, ty.clone());
                }
                scope.define(extra, extra_id);
            }
        } else {
            let mut declared = vec![name.clone()];
            declared.extend(names.iter().cloned());
            for binding_name in declared {
                let binding = ValueId::new(
                    self.owner.clone(),
                    format!("{site}/let/{}", sanitize_path(&binding_name)),
                );
                self.fact(
                    binding.clone(),
                    ValueOrigin::Unknown,
                    DestinationClass::Unknown,
                    Some(binding_name.clone()),
                );
                scope.define(&binding_name, binding);
            }
            value_id = scope.lookup(name);
        }
        value_id
    }

    fn stmt_assignment(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let Statement::Assign(target, value) = statement else {
            return None;
        };
        let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
        let mut expr_path = Vec::new();
        let value_id = self.stmt_expr(value, body_path, &mut expr_path, scope);
        self.use_value(&value_id);

        if let Some(name) = target.as_var_name() {
            if let Some(destination) = scope.lookup(name) {
                self.use_value(&destination);
                if self.loop_depth > 0 {
                    self.add_sink(
                        &destination,
                        SinkKind::LoopCarrier,
                        &site,
                        EscapeKind::LoopLifetime,
                    );
                }
                return Some(destination);
            }
            let state_id = ValueId::new(
                self.owner.clone(),
                format!("{site}/state/{}", sanitize_path(name)),
            );
            self.fact(
                state_id.clone(),
                ValueOrigin::StateOrGlobalLoad,
                DestinationClass::Boxed,
                Some(name.to_string()),
            );
            self.add_sink(
                &value_id,
                SinkKind::StateStore,
                &site,
                EscapeKind::StateLifetime,
            );
            return Some(state_id);
        }

        if let Expr::Field(base, _) = target {
            if let Some(base_name) = base.as_var_name() {
                if let Some(base_id) = scope.lookup(base_name) {
                    self.add_sink(
                        &base_id,
                        SinkKind::ObjectFieldStore,
                        &site,
                        EscapeKind::ObjectLifetime,
                    );
                    self.add_sink(
                        &value_id,
                        SinkKind::ObjectFieldStore,
                        &site,
                        EscapeKind::ObjectLifetime,
                    );
                    return Some(base_id);
                }
            }
        }

        Some(value_id)
    }

    fn stmt_return(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let (expr, statement_sink, escape) = match statement {
            Statement::Term(expr) => (expr.as_ref(), SinkKind::Return, EscapeKind::Returned),
            Statement::Tail(expr) => (Some(expr), SinkKind::Return, EscapeKind::Returned),
            Statement::Expression(expr) | Statement::Check(expr) | Statement::Gate(expr) => {
                (Some(expr), SinkKind::Return, EscapeKind::Returned)
            }
            Statement::EndProgram(expr) => (expr.as_ref(), SinkKind::Return, EscapeKind::Returned),
            _ => return None,
        };
        let sink = if self.exported_owner {
            SinkKind::ExportReturn
        } else {
            statement_sink
        };
        let escape = if self.exported_owner {
            EscapeKind::ExportLifetime
        } else {
            escape
        };
        let mut value = None;
        if let Some(expr) = expr {
            let mut expr_path = Vec::new();
            let value_id = self.stmt_expr(expr, body_path, &mut expr_path, scope);
            self.use_value(&value_id);
            let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
            self.add_sink(&value_id, sink, &site, escape);
            value = Some(value_id);
        }
        value
    }

    fn stmt_iteration(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        match statement {
            Statement::Foreach { item, list, body } => {
                let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
                let mut expr_path = Vec::new();
                let list_id = self.stmt_expr(list, body_path, &mut expr_path, scope);
                self.use_value(&list_id);
                let item_id = ValueId::new(
                    self.owner.clone(),
                    format!("{site}/foreach/{}", sanitize_path(item)),
                );
                self.fact(
                    item_id.clone(),
                    ValueOrigin::CollectionLoad,
                    DestinationClass::Boxed,
                    Some(item.clone()),
                );
                let mut child_scope = scope.clone();
                child_scope.define(item, item_id.clone());
                self.loop_depth += 1;
                for (index, statement) in body.iter().enumerate() {
                    let mut child_path = stmt_path.clone();
                    child_path.push(index);
                    self.stmt(statement, body_path, &mut child_path, &mut child_scope);
                }
                self.loop_depth -= 1;
                Some(item_id)
            }
            Statement::Match { expr, arms } => {
                let mut expr_path = Vec::new();
                let scrutinee_id = self.stmt_expr(expr, body_path, &mut expr_path, scope);
                self.use_value(&scrutinee_id);
                let mut child_scope = scope.clone();
                for (arm_index, arm) in arms.iter().enumerate() {
                    let mut child_path = stmt_path.clone();
                    child_path.push(arm_index);
                    let mut cursor = StmtCursor::new(body_path, &mut child_path);
                    self.stmt_match_arm(arm, &scrutinee_id, &mut cursor, &mut child_scope);
                }
                None
            }
            _ => None,
        }
    }

    fn stmt_match_arm(
        &mut self,
        arm: &crate::ast::StmtMatchArm,
        scrutinee_id: &ValueId,
        cursor: &mut StmtCursor,
        child_scope: &mut Scope,
    ) {
        for pattern in &arm.patterns {
            bind_pattern(pattern, scrutinee_id, child_scope);
        }
        for (index, statement) in arm.body.iter().enumerate() {
            let mut arm_path = cursor.path.clone();
            arm_path.push(index);
            self.stmt(statement, cursor.body_path, &mut arm_path, child_scope);
        }
    }

    fn stmt_control(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let (condition, body) = match statement {
            Statement::Guarded(condition, body) => (Some(condition), body),
            Statement::Block(body) => (None, body),
            Statement::SyncBlock(body) | Statement::Mutex(body) | Statement::Defer(body) => {
                (None, body)
            }
            _ => return None,
        };
        let mut child_scope = scope.clone();
        if let Some(condition) = condition {
            let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
            let mut expr_path = Vec::new();
            let condition_id =
                self.stmt_expr(condition, body_path, &mut expr_path, &mut child_scope);
            self.use_value(&condition_id);
            let _ = site;
        }
        for (index, child) in body.iter().enumerate() {
            let mut child_path = stmt_path.clone();
            child_path.push(index);
            self.stmt(child, body_path, &mut child_path, &mut child_scope);
        }
        None
    }

    fn stmt_effect(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
        match statement {
            Statement::ArrowAssign { target, value, .. } => {
                let mut expr_path = Vec::new();
                let value_id = self.stmt_expr(value, body_path, &mut expr_path, scope);
                self.use_value(&value_id);
                let collection_like = target
                    .as_deref()
                    .is_some_and(|target| matches!(target, Expr::MethodCall(..) | Expr::Index(..)));
                if collection_like {
                    self.add_sink(
                        &value_id,
                        SinkKind::CollectionInsert,
                        &site,
                        EscapeKind::CollectionLifetime,
                    );
                }
                Some(value_id)
            }
            Statement::Rollback(expr) => {
                let mut expr_path = Vec::new();
                let value = expr
                    .as_ref()
                    .map(|expr| self.stmt_expr(expr, body_path, &mut expr_path, scope));
                match value {
                    Some(value) => {
                        self.use_value(&value);
                        Some(value)
                    }
                    None => None,
                }
            }
            Statement::FreeHint(name) => {
                if let Some(id) = scope.lookup(name) {
                    self.use_value(&id);
                }
                None
            }
            Statement::KeepHint(name) => {
                if let Some(id) = scope.lookup(name) {
                    self.use_value(&id);
                }
                None
            }
            _ => None,
        }
    }

    fn stmt_other(
        &mut self,
        statement: &Statement,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        match statement {
            Statement::Expression(expr) | Statement::Check(expr) | Statement::Gate(expr) => {
                let mut expr_path = Vec::new();
                let value = self.stmt_expr(expr, body_path, &mut expr_path, scope);
                self.use_value(&value);
                Some(value)
            }
            Statement::Yield | Statement::Break | Statement::Trap | Statement::Halt => None,
            _ => {
                let _ = (body_path, stmt_path, scope);
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatementCategory {
    Binding,
    Assignment,
    Return,
    Iteration,
    Control,
    Effect,
    Other,
}

impl StatementCategory {
    fn classify(statement: &Statement) -> Self {
        match statement {
            Statement::Let { .. } => Self::Binding,
            Statement::Assign(..) => Self::Assignment,
            Statement::Term(_) | Statement::Tail(_) => Self::Return,
            Statement::Foreach { .. } | Statement::Match { .. } => Self::Iteration,
            Statement::Guarded(..)
            | Statement::Block(..)
            | Statement::SyncBlock(..)
            | Statement::Mutex(..)
            | Statement::Defer(..)
            | Statement::EndProgram(_) => Self::Control,
            Statement::ArrowAssign { .. }
            | Statement::Rollback(..)
            | Statement::FreeHint(_)
            | Statement::KeepHint(_) => Self::Effect,
            Statement::Expression(..) | Statement::Check(_) | Statement::Gate(_) => Self::Other,
            _ => Self::Other,
        }
    }
}

fn bind_pattern(pattern: &crate::ast::Pattern, scrutinee: &ValueId, scope: &mut Scope) {
    match pattern {
        crate::ast::Pattern::Binding(name) => scope.define(name, scrutinee.clone()),
        crate::ast::Pattern::TypedBinding(name, _) => scope.define(name, scrutinee.clone()),
        crate::ast::Pattern::EnumVariant(_, subpatterns) => {
            for subpattern in subpatterns {
                bind_pattern(subpattern, scrutinee, scope);
            }
        }
        crate::ast::Pattern::Tuple(subpatterns) => {
            for subpattern in subpatterns {
                bind_pattern(subpattern, scrutinee, scope);
            }
        }
        crate::ast::Pattern::Multi(subpatterns) => {
            for subpattern in subpatterns {
                bind_pattern(subpattern, scrutinee, scope);
            }
        }
        _ => {}
    }
}

fn walk_expression_identifiers(expr: &Expr, out: &mut dyn FnMut(&str)) {
    match expr {
        Expr::Identifier(name) => out(name),
        Expr::BinaryOp(_, lhs, rhs) => {
            walk_expression_identifiers(lhs, out);
            walk_expression_identifiers(rhs, out);
        }
        Expr::UnaryOp(_, inner)
        | Expr::Field(inner, _)
        | Expr::Reflect(inner, _, _)
        | Expr::Index(inner, _)
        | Expr::Cast(inner, _)
        | Expr::IsType(inner, _)
        | Expr::Within(inner, _)
        | Expr::Deref(inner)
        | Expr::AddrOf(inner)
        | Expr::Consume(inner)
        | Expr::Await(inner) => walk_expression_identifiers(inner, out),
        Expr::Call(_, args, _) => {
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        Expr::MethodCall(receiver, _, args, _, _) => {
            walk_expression_identifiers(receiver, out);
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        Expr::Slice {
            array,
            start,
            end,
            stride,
        } => {
            walk_expression_identifiers(array, out);
            if let Some(start) = start {
                walk_expression_identifiers(start, out);
            }
            if let Some(end) = end {
                walk_expression_identifiers(end, out);
            }
            if let Some(stride) = stride {
                walk_expression_identifiers(stride, out);
            }
        }
        Expr::Range { start, end, .. } => {
            walk_expression_identifiers(start, out);
            walk_expression_identifiers(end, out);
        }
        Expr::Spawn { args, .. } => {
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        Expr::Tuple(values) | Expr::List(values) => {
            for value in values {
                walk_expression_identifiers(value, out);
            }
        }
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, value) in fields.iter().chain(specs.iter()) {
                walk_expression_identifiers(value, out);
            }
        }
        Expr::Block(body) => crate::ast::visit::walk_stmts(body, &mut |expression| {
            walk_expression_identifiers(expression, out);
        }),
        Expr::If(condition, then_expr, else_expr) => {
            walk_expression_identifiers(condition, out);
            walk_expression_identifiers(then_expr, out);
            if let Some(else_expr) = else_expr {
                walk_expression_identifiers(else_expr, out);
            }
        }
        Expr::Match(scrutinee, arms) => {
            walk_expression_identifiers(scrutinee, out);
            for arm in arms {
                walk_expression_identifiers(&arm.body, out);
            }
        }
        Expr::Lambda(_, body) => walk_expression_identifiers(body, out),
        Expr::PluginIntercept { args, receiver, .. } => {
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
            if let Some(receiver) = receiver {
                walk_expression_identifiers(receiver, out);
            }
        }
        _ => {}
    }
}
