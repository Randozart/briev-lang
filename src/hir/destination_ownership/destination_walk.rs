// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (typed-HIR port): destination ownership body walker.
//!
//! Port of `analysis::destination_ownership::destination_walk` onto HIR
//! statements/expressions. Category helpers keep the source-order traversal
//! deterministic and each destination rule single-responsibility. The backend
//! does not consume this analysis yet. Statement/expression coverage mirrors
//! the AST walker arm-for-arm (see the module header in
//! `hir::destination_ownership` for the quirk list).

use super::CallableSpec;
use crate::analysis::destination_ownership::{
    CallSiteFact, CallSiteId, CallableDestinationFacts, CallableKind, CallableSummary,
    DestinationClass, DestinationDiagnostic, DestinationDiagnosticCode, DestinationSeverity,
    EscapeKind, ParamObligation, ResultDestination, SinkKind, ValueFact, ValueId, ValueOrigin,
    is_primitive,
};
use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral, HirMatchArm};
use crate::hir::expr_walk as hir_visit;
use crate::hir::pattern::{HirPattern, HirPatternKind};
use crate::hir::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use crate::ast::Type;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Coarse expression category. This keeps the source-order traversal
/// deterministic while keeping each rule short enough to audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExprCategory {
    Leaf,
    Reference,
    Operator,
    CallLike,
    Composite,
    Control,
    Unknown,
}

impl ExprCategory {
    fn classify(expr: &HirExpr) -> Self {
        match &expr.kind {
            HirExprKind::Literal(
                HirLiteral::Bytes(_)
                | HirLiteral::Int(_)
                | HirLiteral::Char(_)
                | HirLiteral::TaggedInt(_, _)
                | HirLiteral::TaggedBytes(_, _)
                | HirLiteral::Bool(_)
                | HirLiteral::Float(_)
                | HirLiteral::BeginProgram,
            )
            | HirExprKind::Wildcard => Self::Leaf,
            HirExprKind::Identifier(_) => Self::Reference,
            HirExprKind::Binary { .. }
            | HirExprKind::Unary { .. }
            | HirExprKind::Field { .. }
            | HirExprKind::Reflect { .. }
            | HirExprKind::Index(..)
            | HirExprKind::Slice { .. }
            | HirExprKind::Range { .. }
            | HirExprKind::Cast { .. }
            | HirExprKind::IsType { .. } => Self::Operator,
            HirExprKind::Call { .. }
            | HirExprKind::MethodCall { .. }
            | HirExprKind::Spawn { .. }
            | HirExprKind::Await(_)
            | HirExprKind::PluginIntercept { .. } => Self::CallLike,
            HirExprKind::Tuple(_)
            | HirExprKind::List(_)
            | HirExprKind::StructLiteral { .. }
            | HirExprKind::Lambda { .. }
            | HirExprKind::Deref(_)
            | HirExprKind::AddrOf(_)
            | HirExprKind::Consume(_) => Self::Composite,
            HirExprKind::Block(_) | HirExprKind::If { .. } | HirExprKind::Match { .. } => {
                Self::Control
            }
            // AST parity: unit literals fall through to Unknown on the AST
            // side; HIR `Other` has no AST counterpart and stays Unknown.
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StmtCategory {
    Binding,
    Assignment,
    Return,
    Iteration,
    Control,
    Effect,
    Other,
}

impl StmtCategory {
    /// AST `StatementCategory::classify` mirrored arm-for-arm. `EndProgram`
    /// deliberately classifies as Control (whose handler does not walk it),
    /// matching the AST analyzer.
    fn classify(stmt: &HirStmt) -> Self {
        match &stmt.kind {
            HirStmtKind::Let { .. } => Self::Binding,
            HirStmtKind::Assign(..) => Self::Assignment,
            HirStmtKind::Term(_) | HirStmtKind::Tail(_) => Self::Return,
            HirStmtKind::Foreach { .. } | HirStmtKind::Match { .. } => Self::Iteration,
            HirStmtKind::Guarded(..)
            | HirStmtKind::Block(..)
            | HirStmtKind::SyncBlock(..)
            | HirStmtKind::Mutex(..)
            | HirStmtKind::Defer(..)
            | HirStmtKind::EndProgram(_) => Self::Control,
            HirStmtKind::ArrowAssign { .. }
            | HirStmtKind::Rollback(..)
            | HirStmtKind::FreeHint(_)
            | HirStmtKind::KeepHint(_) => Self::Effect,
            _ => Self::Other,
        }
    }
}

pub(super) struct ExprCursor<'a> {
    pub(super) body_path: &'a str,
    pub(super) path: &'a mut Vec<usize>,
}

impl<'a> ExprCursor<'a> {
    pub(super) fn new(body_path: &'a str, path: &'a mut Vec<usize>) -> Self {
        ExprCursor { body_path, path }
    }
}

#[derive(Clone)]
pub(super) struct Scope {
    pub(super) values: HashMap<String, ValueId>,
}

impl Scope {
    pub(super) fn lookup(&self, name: &str) -> Option<ValueId> {
        self.values.get(name).cloned()
    }

    pub(super) fn define(&mut self, name: &str, id: ValueId) {
        self.values.insert(name.to_string(), id);
    }
}

pub(super) struct BodyAnalyzer<'a> {
    pub(super) owner: String,
    pub(super) specs: &'a BTreeMap<String, CallableSpec<'a>>,
    pub(super) summaries: &'a BTreeMap<String, CallableSummary>,
    pub(super) exported_owner: bool,
    pub(super) facts: BTreeMap<ValueId, ValueFact>,
    pub(super) call_sites: BTreeMap<CallSiteId, CallSiteFact>,
    pub(super) diagnostics: BTreeSet<DestinationDiagnostic>,
    /// Active foreach nesting depth. Loop-carried assignments are marked with
    /// a LoopCarrier sink so the generalized plan can build loop facts.
    pub(super) loop_depth: usize,
    /// Monotonic ordinal for root expressions. Without it, two root
    /// expressions in one statement share `expr/` and collapse into one value.
    pub(super) expr_serial: u64,
}

/// Callee names reachable from a body — the recursive-callable seed. Mirrors
/// the AST `collect_calls_expr` arm-for-arm (match-arm guards are not
/// visited there either).
pub(super) fn collect_calls(body: &[HirStmt], out: &mut BTreeSet<String>) {
    for stmt in body {
        collect_calls_stmt(stmt, out);
    }
}

fn collect_calls_stmt(stmt: &HirStmt, out: &mut BTreeSet<String>) {
    match &stmt.kind {
        HirStmtKind::Let { expr: Some(expr), .. }
        | HirStmtKind::Assign(_, expr)
        | HirStmtKind::Term(Some(expr))
        | HirStmtKind::Tail(expr)
        | HirStmtKind::Check(expr)
        | HirStmtKind::Gate(expr)
        | HirStmtKind::Expression(expr)
        | HirStmtKind::EndProgram(Some(expr))
        | HirStmtKind::Rollback(Some(expr)) => collect_calls_expr(expr, out),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(target) = target {
                collect_calls_expr(target, out);
            }
            collect_calls_expr(value, out);
        }
        HirStmtKind::Foreach { list, body, .. } => {
            collect_calls_expr(list, out);
            collect_calls(body, out);
        }
        HirStmtKind::Guarded(condition, body) => {
            collect_calls_expr(condition, out);
            collect_calls(body, out);
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body) => collect_calls(body, out),
        HirStmtKind::Match { expr, arms } => {
            collect_calls_expr(expr, out);
            for arm in arms {
                collect_calls(&arm.body, out);
            }
        }
        HirStmtKind::TrgBinding { instance, .. } => collect_calls_expr(instance, out),
        HirStmtKind::Open(a, b) => {
            collect_calls_expr(a, out);
            collect_calls_expr(b, out);
        }
        _ => {}
    }
}

fn collect_calls_expr(expr: &HirExpr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        HirExprKind::Call { callee, args, .. } => {
            out.insert(callee.clone());
            for arg in args {
                collect_calls_expr(arg, out);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            collect_calls_expr(receiver, out);
            for arg in args {
                collect_calls_expr(arg, out);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_calls_expr(left, out);
            collect_calls_expr(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Lambda { body: inner, .. } => collect_calls_expr(inner, out),
        HirExprKind::Index(base, index) => {
            collect_calls_expr(base, out);
            collect_calls_expr(index, out);
        }
        HirExprKind::Slice {
            array,
            start,
            end,
            stride,
        } => {
            collect_calls_expr(array, out);
            for part in [start, end, stride].into_iter().flatten() {
                collect_calls_expr(part, out);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            collect_calls_expr(start, out);
            collect_calls_expr(end, out);
        }
        HirExprKind::Spawn { args, .. } => {
            for arg in args {
                collect_calls_expr(arg, out);
            }
        }
        HirExprKind::Block(body) => collect_calls(body, out),
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_calls_expr(condition, out);
            collect_calls_expr(then_branch, out);
            if let Some(else_branch) = else_branch {
                collect_calls_expr(else_branch, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_calls_expr(scrutinee, out);
            for arm in arms {
                collect_calls_expr(&arm.body, out);
            }
        }
        HirExprKind::Tuple(values) | HirExprKind::List(values) => {
            for value in values {
                collect_calls_expr(value, out);
            }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, expr) in fields.iter().chain(specs.iter()) {
                collect_calls_expr(expr, out);
            }
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            for arg in args {
                collect_calls_expr(arg, out);
            }
            if let Some(receiver) = receiver {
                collect_calls_expr(receiver, out);
            }
        }
        _ => {}
    }
}

pub(super) fn format_path(path: &[usize]) -> String {
    path.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("_")
}

pub(super) fn sanitize_path(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Bind a match pattern's names to the scrutinee value (AST `bind_pattern`).
fn bind_pattern(pattern: &HirPattern, scrut: &ValueId, scope: &mut Scope) {
    match &pattern.kind {
        HirPatternKind::Binding(name) | HirPatternKind::TypedBinding(name, _) => {
            scope.define(name, scrut.clone());
        }
        HirPatternKind::EnumVariant(_, subpatterns)
        | HirPatternKind::Tuple(subpatterns)
        | HirPatternKind::Multi(subpatterns) => {
            for subpattern in subpatterns {
                bind_pattern(subpattern, scrut, scope);
            }
        }
        _ => {}
    }
}

/// Identifier reads inside one expression tree (the comprehensive AST
/// `walk_expression_identifiers` used for lambda captures).
fn walk_expression_identifiers(expr: &HirExpr, out: &mut dyn FnMut(&str)) {
    match &expr.kind {
        HirExprKind::Identifier(name) => out(name),
        HirExprKind::Binary { left, right, .. } => {
            walk_expression_identifiers(left, out);
            walk_expression_identifiers(right, out);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Field { receiver: inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::IsType { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Reflect { receiver: inner, .. }
        | HirExprKind::Lambda { body: inner, .. } => walk_expression_identifiers(inner, out),
        HirExprKind::Call { args, .. } => {
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            walk_expression_identifiers(receiver, out);
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        HirExprKind::Index(base, index) => {
            walk_expression_identifiers(base, out);
            walk_expression_identifiers(index, out);
        }
        HirExprKind::Slice {
            array,
            start,
            end,
            stride,
        } => {
            walk_expression_identifiers(array, out);
            for part in [start, end, stride].into_iter().flatten() {
                walk_expression_identifiers(part, out);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            walk_expression_identifiers(start, out);
            walk_expression_identifiers(end, out);
        }
        HirExprKind::Spawn { args, .. } => {
            for arg in args {
                walk_expression_identifiers(arg, out);
            }
        }
        HirExprKind::Tuple(values) | HirExprKind::List(values) => {
            for value in values {
                walk_expression_identifiers(value, out);
            }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, value) in fields.iter().chain(specs.iter()) {
                walk_expression_identifiers(value, out);
            }
        }
        HirExprKind::Block(body) => hir_visit::walk_stmts(body, &mut |expression| {
            walk_expression_identifiers(expression, out);
        }),
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            walk_expression_identifiers(condition, out);
            walk_expression_identifiers(then_branch, out);
            if let Some(else_branch) = else_branch {
                walk_expression_identifiers(else_branch, out);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_expression_identifiers(scrutinee, out);
            for arm in arms {
                walk_expression_identifiers(&arm.body, out);
            }
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
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

/// Facts for one callable body (the AST `analyze_callable`).
pub(super) fn analyze_callable<'a>(
    spec: &CallableSpec<'a>,
    specs: &'a BTreeMap<String, CallableSpec<'a>>,
    summaries: &'a BTreeMap<String, CallableSummary>,
    exported_owner: bool,
    direct_recursive: bool,
) -> CallableDestinationFacts {
    let mut analyzer = BodyAnalyzer {
        owner: spec.owner.clone(),
        specs,
        summaries,
        exported_owner,
        facts: BTreeMap::new(),
        call_sites: BTreeMap::new(),
        diagnostics: BTreeSet::new(),
        loop_depth: 0,
        expr_serial: 0,
    };
    let mut scope = Scope {
        values: HashMap::new(),
    };
    let mut param_values = Vec::new();
    for (index, (name, ty)) in spec.parameters.iter().enumerate() {
        let class = if is_primitive(ty) {
            DestinationClass::ScalarValue
        } else {
            DestinationClass::Borrowed
        };
        let id = analyzer.fact(
            ValueId {
                owner: spec.owner.clone(),
                path: format!("param/{}/{}", index, sanitize_path(name)),
            },
            ValueOrigin::Parameter,
            class,
            Some(name.clone()),
        );
        param_values.push(id.clone());
        scope.define(name, id);
    }
    let body_path = format!("body/{}", sanitize_path(&spec.name));
    for (index, statement) in spec.body.iter().enumerate() {
        let mut path = vec![index];
        analyzer.stmt(statement, &body_path, &mut path, &mut scope);
    }
    CallableDestinationFacts {
        owner: spec.owner.clone(),
        exported: spec.exported,
        param_values,
        values: analyzer.facts,
        call_sites: analyzer.call_sites,
        diagnostics: analyzer.diagnostics,
        direct_recursive,
    }
}

impl<'a> BodyAnalyzer<'a> {
    pub(super) fn fact(
        &mut self,
        id: ValueId,
        origin: ValueOrigin,
        class: DestinationClass,
        name: Option<String>,
    ) -> ValueId {
        let entry = self.facts.entry(id.clone()).or_insert_with(|| ValueFact {
            id: id.clone(),
            source_name: name,
            ty: None,
            tuple_element_count: None,
            origins: BTreeSet::from([origin]),
            class,
            uses: 0,
            sinks: BTreeMap::new(),
            escapes: BTreeSet::new(),
            copy_points: BTreeMap::new(),
            may_overlap: !matches!(origin, ValueOrigin::Literal),
            reasons: BTreeSet::new(),
        });
        entry.origins.insert(origin);
        id
    }

    pub(super) fn set_tuple_element_count(&mut self, id: &ValueId, count: usize) {
        if let Some(fact) = self.facts.get_mut(id) {
            fact.tuple_element_count = Some(count);
        }
    }

    pub(super) fn set_fact_type(&mut self, id: &ValueId, ty: Type) {
        if let Some(fact) = self.facts.get_mut(id) {
            fact.ty = Some(ty);
        }
    }

    pub(super) fn use_value(&mut self, id: &ValueId) {
        if let Some(fact) = self.facts.get_mut(id) {
            fact.uses += 1;
        }
    }

    pub(super) fn add_sink(
        &mut self,
        id: &ValueId,
        sink: SinkKind,
        site: &str,
        escape: EscapeKind,
    ) {
        let class = self
            .facts
            .get(id)
            .map(|fact| fact.class)
            .unwrap_or(DestinationClass::Unknown);
        if let Some(fact) = self.facts.get_mut(id) {
            fact.sinks.entry(sink).or_default().insert(site.to_string());
            fact.escapes.insert(escape);
            fact.copy_points
                .insert(site.to_string(), escape.as_str().to_string());
        }

        let fallback = matches!(class, DestinationClass::Boxed | DestinationClass::Unknown);
        if fallback
            && matches!(
                sink,
                SinkKind::Return | SinkKind::StateStore | SinkKind::CollectionInsert
            )
        {
            self.diagnostics.insert(DestinationDiagnostic {
                owner: self.owner.clone(),
                value_path: id.path.clone(),
                severity: DestinationSeverity::ConservativeFallback,
                code: DestinationDiagnosticCode::EscapeRequiresCopy,
                message: format!(
                    "{} at '{}' requires a copy because the value is {}",
                    sink.as_str(),
                    site,
                    class.as_str()
                ),
            });
        }
    }

    pub(super) fn diagnostic(
        &mut self,
        id: &ValueId,
        code: DestinationDiagnosticCode,
        message: impl Into<String>,
    ) {
        self.diagnostics.insert(DestinationDiagnostic {
            owner: self.owner.clone(),
            value_path: id.path.clone(),
            severity: DestinationSeverity::ConservativeFallback,
            code,
            message: message.into(),
        });
    }

    pub(super) fn call_site_id(&self, site: &str) -> CallSiteId {
        CallSiteId {
            owner: self.owner.clone(),
            site: site.to_string(),
        }
    }

    pub(super) fn summarize_site(
        &mut self,
        site: &str,
        callee: &str,
        args: &[ValueId],
    ) -> CallSiteFact {
        let (kind, obligations, result, may_alias_args) = match self.summaries.get(callee) {
            Some(summary) => (
                summary.kind,
                summary.parameters.clone(),
                summary.result,
                summary.may_alias_args,
            ),
            None => (
                CallableKind::Unknown,
                vec![ParamObligation::Unknown; args.len()],
                ResultDestination::Unknown,
                true,
            ),
        };

        CallSiteFact {
            id: self.call_site_id(site),
            callee: callee.to_string(),
            kind,
            obligations,
            result,
            may_alias_args,
            argument_values: args.to_vec(),
        }
    }

    pub(super) fn expr(
        &mut self,
        expr: &HirExpr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match ExprCategory::classify(expr) {
            ExprCategory::Leaf => self.expr_leaf(cursor),
            ExprCategory::Reference => self.expr_reference(expr, cursor, scope),
            ExprCategory::Operator => self.expr_operator(expr, cursor, scope),
            ExprCategory::CallLike => self.expr_call_like(expr, cursor, scope),
            ExprCategory::Composite => self.expr_projection(expr, cursor, scope),
            ExprCategory::Control => self.expr_control(expr, cursor, scope),
            ExprCategory::Unknown => self.expr_unknown(cursor),
        }
    }

    fn stmt_expr(
        &mut self,
        expr: &HirExpr,
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
        let id = ValueId {
            owner: self.owner.clone(),
            path,
        };
        self.fact(id.clone(), origin, class, None)
    }

    fn expr_leaf(&mut self, cursor: &mut ExprCursor) -> ValueId {
        self.temp_value(cursor, ValueOrigin::Literal, DestinationClass::ScalarValue)
    }

    fn expr_reference(
        &mut self,
        expr: &HirExpr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let HirExprKind::Identifier(name) = &expr.kind else {
            return self.expr_unknown(cursor);
        };
        if let Some(id) = scope.lookup(name) {
            self.use_value(&id);
            return id;
        }
        let path = format!("{}/external/{}", cursor.body_path, sanitize_path(name));
        let id = ValueId {
            owner: self.owner.clone(),
            path,
        };
        self.fact(
            id.clone(),
            ValueOrigin::StateOrGlobalLoad,
            DestinationClass::Boxed,
            Some(name.clone()),
        )
    }

    fn expr_operator(
        &mut self,
        expr: &HirExpr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match &expr.kind {
            HirExprKind::Binary { left, right, .. } => {
                let lhs_id = self.child_expr(left, 0, cursor, scope);
                let rhs_id = self.child_expr(right, 1, cursor, scope);
                self.use_value(&lhs_id);
                self.use_value(&rhs_id);
                self.temp_value(cursor, ValueOrigin::BinaryResult, DestinationClass::ScalarValue)
            }
            HirExprKind::Unary { inner, .. } => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::UnaryResult, DestinationClass::ScalarValue)
            }
            HirExprKind::Cast { inner, .. } => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::CastResult, DestinationClass::ScalarValue)
            }
            HirExprKind::Field { receiver, .. } => {
                let id = self.child_expr(receiver, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::FieldLoad, DestinationClass::Boxed)
            }
            HirExprKind::Reflect { receiver, .. } => {
                let id = self.child_expr(receiver, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
            }
            HirExprKind::Index(base, index) => {
                let base_id = self.child_expr(base, 0, cursor, scope);
                let index_id = self.child_expr(index, 1, cursor, scope);
                self.use_value(&base_id);
                self.use_value(&index_id);
                self.temp_value(cursor, ValueOrigin::CollectionLoad, DestinationClass::Boxed)
            }
            HirExprKind::Slice {
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
            HirExprKind::Range { start, end, .. } => {
                let start_id = self.child_expr(start, 0, cursor, scope);
                let end_id = self.child_expr(end, 1, cursor, scope);
                self.use_value(&start_id);
                self.use_value(&end_id);
                self.temp_value(cursor, ValueOrigin::Composite, DestinationClass::ScalarValue)
            }
            // AST parity: IsType classifies as Operator but the AST handler
            // has no arm for it — the inner expression is not walked and the
            // result is an Unknown temp.
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_call_like(
        &mut self,
        expr: &HirExpr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match &expr.kind {
            HirExprKind::Call { callee, args, .. } => self.expr_call(callee, args, cursor, scope),
            HirExprKind::MethodCall { receiver, args, .. } => {
                self.expr_method(receiver, args, cursor, scope)
            }
            HirExprKind::Spawn { args, .. } => self.expr_spawn(args, cursor, scope),
            HirExprKind::Await(inner) => {
                let task = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&task);
                self.temp_value(cursor, ValueOrigin::TaskResult, DestinationClass::Boxed)
            }
            HirExprKind::PluginIntercept { .. } => self.expr_unknown(cursor),
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_call(
        &mut self,
        name: &str,
        args: &[HirExpr],
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
        receiver: &HirExpr,
        args: &[HirExpr],
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

    fn expr_spawn(&mut self, args: &[HirExpr], cursor: &mut ExprCursor, scope: &mut Scope) -> ValueId {
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

    fn list_value(
        &mut self,
        values: &[HirExpr],
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let site = format!(
            "{}/expr/{}/collection",
            cursor.body_path,
            format_path(cursor.path)
        );
        for (index, value) in values.iter().enumerate() {
            let id = self.child_expr(value, index, cursor, scope);
            self.use_value(&id);
            // A list stores handles, so every element outlives this
            // expression. Recording the sink keeps frame images from leaking
            // through heap containers.
            self.add_sink(
                &id,
                SinkKind::CollectionInsert,
                &site,
                EscapeKind::CollectionLifetime,
            );
        }
        self.temp_value(cursor, ValueOrigin::Composite, DestinationClass::Boxed)
    }

    fn struct_literal_value(
        &mut self,
        expr: &HirExpr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let HirExprKind::StructLiteral {
            type_name,
            fields,
            specs,
        } = &expr.kind
        else {
            unreachable!("struct_literal_value called on a non-literal expression")
        };
        let site = format!(
            "{}/expr/{}/object-fields",
            cursor.body_path,
            format_path(cursor.path)
        );
        for (index, (_, value)) in fields.iter().chain(specs.iter()).enumerate() {
            let child = self.child_expr(value, index, cursor, scope);
            self.use_value(&child);
            // A field stores the child handle in the parent's image. The child
            // aliases the parent's lifetime; without a parent-lifetime proof it
            // must never be a frame image.
            self.add_sink(
                &child,
                SinkKind::ObjectFieldStore,
                &site,
                EscapeKind::ObjectLifetime,
            );
        }
        let id = self.temp_value(
            cursor,
            ValueOrigin::Constructor,
            DestinationClass::ScalarReplacedObject,
        );
        self.set_fact_type(&id, Type::Custom(type_name.to_string()));
        if self.loop_depth > 0 {
            let site = format!("{}/expr/{}", cursor.body_path, format_path(cursor.path));
            self.add_sink(&id, SinkKind::LoopCarrier, &site, EscapeKind::LoopLifetime);
        }
        id
    }

    fn expr_projection(
        &mut self,
        expr: &HirExpr,
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        match &expr.kind {
            HirExprKind::Tuple(values) => {
                let site = format!(
                    "{}/expr/{}/tuple",
                    cursor.body_path,
                    format_path(cursor.path)
                );
                for (index, value) in values.iter().enumerate() {
                    let id = self.child_expr(value, index, cursor, scope);
                    self.use_value(&id);
                    self.add_sink(&id, SinkKind::TupleStore, &site, EscapeKind::TupleLifetime);
                }
                let id = self.temp_value(
                    cursor,
                    ValueOrigin::Composite,
                    DestinationClass::PackedMemory,
                );
                self.set_tuple_element_count(&id, values.len());
                id
            }
            HirExprKind::List(values) => self.list_value(values, cursor, scope),
            HirExprKind::StructLiteral { .. } => self.struct_literal_value(expr, cursor, scope),
            HirExprKind::Lambda { body, .. } => self.expr_lambda(body, cursor, scope),
            HirExprKind::Deref(inner) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Boxed)
            }
            HirExprKind::AddrOf(inner) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                let site = format!(
                    "{}/expr/{}/address",
                    cursor.body_path,
                    format_path(cursor.path)
                );
                self.add_sink(
                    &id,
                    SinkKind::AddressEscape,
                    &site,
                    EscapeKind::AddressLifetime,
                );
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Boxed)
            }
            HirExprKind::Consume(inner) => {
                let id = self.child_expr(inner, 0, cursor, scope);
                self.use_value(&id);
                self.temp_value(cursor, ValueOrigin::Unknown, DestinationClass::Unknown)
            }
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_lambda(&mut self, body: &HirExpr, cursor: &mut ExprCursor, scope: &mut Scope) -> ValueId {
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

    fn expr_control(&mut self, expr: &HirExpr, cursor: &mut ExprCursor, scope: &mut Scope) -> ValueId {
        match &expr.kind {
            HirExprKind::Block(body) => self.expr_block_value(body, cursor, scope),
            HirExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.expr_if(
                (condition, then_branch, else_branch.as_deref()),
                cursor,
                scope,
            ),
            HirExprKind::Match { scrutinee, arms } => self.expr_match(scrutinee, arms, cursor, scope),
            _ => self.expr_unknown(cursor),
        }
    }

    fn expr_block_value(
        &mut self,
        body: &[HirStmt],
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
        parts: (&HirExpr, &HirExpr, Option<&HirExpr>),
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
            self.temp_value(cursor, ValueOrigin::MatchResult, DestinationClass::ScalarValue)
        } else {
            self.temp_value(cursor, ValueOrigin::MatchResult, DestinationClass::Boxed)
        }
    }

    fn expr_match(
        &mut self,
        scrutinee: &HirExpr,
        arms: &[HirMatchArm],
        cursor: &mut ExprCursor,
        scope: &mut Scope,
    ) -> ValueId {
        let scrutinee_id = self.child_expr(scrutinee, 0, cursor, scope);
        self.use_value(&scrutinee_id);
        let mut results = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            let mut child_scope = scope.clone();
            bind_pattern(&arm.pattern, &scrutinee_id, &mut child_scope);
            // AST parity: the arm body is walked in the OUTER scope (the
            // pattern bindings live only in child_scope).
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
        expr: &HirExpr,
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
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let category = StmtCategory::classify(statement);
        match category {
            StmtCategory::Binding => self.stmt_binding(statement, body_path, stmt_path, scope),
            StmtCategory::Assignment => {
                self.stmt_assignment(statement, body_path, stmt_path, scope)
            }
            StmtCategory::Return => self.stmt_return(statement, body_path, stmt_path, scope),
            StmtCategory::Iteration => {
                self.stmt_iteration(statement, body_path, stmt_path, scope)
            }
            StmtCategory::Control => self.stmt_control(statement, body_path, stmt_path, scope),
            StmtCategory::Effect => self.stmt_effect(statement, body_path, stmt_path, scope),
            StmtCategory::Other => self.stmt_other(statement, body_path, stmt_path, scope),
        }
    }

    fn stmt_binding(
        &mut self,
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let HirStmtKind::Let {
            name, names, expr, ..
        } = &statement.kind
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
            let binding = ValueId {
                owner: self.owner.clone(),
                path: format!("{site}/let/{}", sanitize_path(name)),
            };
            self.fact(binding.clone(), origin, class, Some(name.clone()));
            if let Some(ty) = &ty {
                self.set_fact_type(&binding, ty.clone());
            }
            self.use_value(&produced);
            scope.define(name, binding.clone());
            value_id = Some(binding);
            for extra in names {
                let extra_id = ValueId {
                    owner: self.owner.clone(),
                    path: format!("{site}/let/{}", sanitize_path(extra)),
                };
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
                let binding = ValueId {
                    owner: self.owner.clone(),
                    path: format!("{site}/let/{}", sanitize_path(&binding_name)),
                };
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
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let HirStmtKind::Assign(target, value) = &statement.kind else {
            return None;
        };
        let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
        let mut expr_path = Vec::new();
        let value_id = self.stmt_expr(value, body_path, &mut expr_path, scope);
        self.use_value(&value_id);

        if let HirExprKind::Identifier(name) = &target.kind {
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
            let state_id = ValueId {
                owner: self.owner.clone(),
                path: format!("{site}/state/{}", sanitize_path(name)),
            };
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

        // 2026-11-07 (nested-field object escape): a field store publishes the
        // assigned handle into an object that can outlive this frame. The
        // recorded sink must be found for the ROOT receiver at ANY projection
        // depth, not only `x.field = …`. `p.mid.inner = Inner { … }` has a
        // base of `Field(Field(p, mid), …)`; a var-name probe sees no
        // identifier there, so no escape would be recorded and the literal
        // stayed a frame alloca whose handle dangled once the defn returned.
        // Peel the projection chain to its root variable so depth-1 and
        // depth-N stores are treated identically.
        if matches!(target.kind, HirExprKind::Field { .. }) {
            if let Some(base_name) = Self::projection_root_var(target) {
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

    /// Root variable of a projection chain (`p.mid.inner` → `p`,
    /// `a[i].f` → `a`). Descends through field and index projections so an
    /// assignment target at any depth resolves to the value that actually
    /// outlives the producing frame.
    fn projection_root_var(expr: &HirExpr) -> Option<&str> {
        match &expr.kind {
            HirExprKind::Identifier(name) => Some(name.as_str()),
            HirExprKind::Field { receiver: base, .. } | HirExprKind::Index(base, _) => {
                Self::projection_root_var(base)
            }
            _ => None,
        }
    }

    fn stmt_return(
        &mut self,
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        // AST parity: classify routes only Term/Tail here; the Expression/
        // Check/Gate arms on the AST side are unreachable and not ported.
        // EndProgram classifies as Control, so its expression is never sunk.
        let expr = match &statement.kind {
            HirStmtKind::Term(expr) => expr.as_ref(),
            HirStmtKind::Tail(expr) => Some(expr),
            _ => return None,
        };
        let (sink, escape) = if self.exported_owner {
            (SinkKind::ExportReturn, EscapeKind::ExportLifetime)
        } else {
            (SinkKind::Return, EscapeKind::Returned)
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
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        match &statement.kind {
            HirStmtKind::Foreach {
                item, list, body, ..
            } => {
                let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
                let mut expr_path = Vec::new();
                let list_id = self.stmt_expr(list, body_path, &mut expr_path, scope);
                self.use_value(&list_id);
                let item_id = ValueId {
                    owner: self.owner.clone(),
                    path: format!("{site}/foreach/{}", sanitize_path(item)),
                };
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
            HirStmtKind::Match { expr, arms } => {
                let mut expr_path = Vec::new();
                let scrutinee_id = self.stmt_expr(expr, body_path, &mut expr_path, scope);
                self.use_value(&scrutinee_id);
                let mut child_scope = scope.clone();
                for (arm_index, arm) in arms.iter().enumerate() {
                    let mut child_path = stmt_path.clone();
                    child_path.push(arm_index);
                    self.stmt_match_arm(arm, &scrutinee_id, (body_path, &child_path), &mut child_scope);
                }
                None
            }
            _ => None,
        }
    }

    fn stmt_match_arm(
        &mut self,
        arm: &HirStmtMatchArm,
        scrutinee_id: &ValueId,
        into: (&str, &[usize]),
        child_scope: &mut Scope,
    ) {
        let (body_path, arm_path) = into;
        for pattern in &arm.patterns {
            bind_pattern(pattern, scrutinee_id, child_scope);
        }
        for (index, statement) in arm.body.iter().enumerate() {
            let mut stmt_in_arm = arm_path.to_vec();
            stmt_in_arm.push(index);
            self.stmt(statement, body_path, &mut stmt_in_arm, child_scope);
        }
    }

    fn stmt_control(
        &mut self,
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let (condition, body) = match &statement.kind {
            HirStmtKind::Guarded(condition, body) => (Some(condition), body),
            HirStmtKind::Block(body)
            | HirStmtKind::SyncBlock(body)
            | HirStmtKind::Mutex(body)
            | HirStmtKind::Defer(body) => (None, body),
            // EndProgram reaches this category by classification but is not
            // handled here, exactly as on the AST side: its expression is
            // never walked.
            _ => return None,
        };
        let mut child_scope = scope.clone();
        if let Some(condition) = condition {
            let mut expr_path = Vec::new();
            let condition_id =
                self.stmt_expr(condition, body_path, &mut expr_path, &mut child_scope);
            self.use_value(&condition_id);
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
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        let site = format!("{}/stmt/{}", body_path, format_path(stmt_path));
        match &statement.kind {
            HirStmtKind::ArrowAssign { target, value, .. } => {
                let mut expr_path = Vec::new();
                let value_id = self.stmt_expr(value, body_path, &mut expr_path, scope);
                self.use_value(&value_id);
                if let Some(HirExprKind::Identifier(name)) =
                    target.as_ref().map(|target| &target.kind)
                {
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
                    }
                }
                let collection_like = target
                    .as_ref()
                    .is_some_and(|target| {
                        matches!(
                            target.kind,
                            HirExprKind::MethodCall { .. } | HirExprKind::Index(..)
                        )
                    });
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
            HirStmtKind::Rollback(expr) => {
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
            HirStmtKind::FreeHint(name) | HirStmtKind::KeepHint(name) => {
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
        statement: &HirStmt,
        body_path: &str,
        stmt_path: &mut Vec<usize>,
        scope: &mut Scope,
    ) -> Option<ValueId> {
        match &statement.kind {
            HirStmtKind::Expression(expr)
            | HirStmtKind::Check(expr)
            | HirStmtKind::Gate(expr) => {
                let mut expr_path = Vec::new();
                let value = self.stmt_expr(expr, body_path, &mut expr_path, scope);
                self.use_value(&value);
                Some(value)
            }
            HirStmtKind::Yield
            | HirStmtKind::Break
            | HirStmtKind::Trap
            | HirStmtKind::Halt => None,
            _ => None,
        }
    }
}
