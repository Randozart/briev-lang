// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12: HIR constraint construction, mirroring
//! `analysis::reclamation::walk`. Unsupported forms capture their inputs
//! instead of being interpreted as evidence for reclamation.

use super::graph::{Callable, Graph, Node};
use crate::analysis::reclamation::Constraint;
use crate::ast::{BinaryOpKind, Type, UnaryOpKind};
use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral};
use crate::hir::ids::NodeId;
use crate::hir::pattern::{HirPattern, HirPatternKind};
use crate::hir::program::{
    HirCallable, HirCallableKind, HirEnumVariant, HirOutput, HirProgram, HirTypeDef,
    HirTypeDefKind,
};
use crate::hir::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(super) fn register(graph: &mut Graph, program: &HirProgram) {
    for global in &program.globals {
        graph.global(&global.name);
        if let Some(ty) = &global.ty {
            graph.global_types.insert(global.name.clone(), ty.clone());
        }
    }
    for (name, expression) in &program.const_exprs {
        let node = graph.global(name);
        if let HirExprKind::Literal(HirLiteral::Int(value)) = &expression.kind {
            graph.points[node].clear();
            graph.constants[node].insert(*value);
            graph.inexact_constants.remove(&node);
        }
    }
    for (name, ty) in &program.const_types {
        graph.global(name);
        graph.global_types.insert(name.clone(), ty.clone());
    }
    for definition in &program.type_defs {
        register_typedef(graph, definition);
    }
    for definition in &program.type_defs {
        register_methods(graph, definition, program);
    }
    for callable in &program.callables {
        register_free(graph, callable);
    }
    register_defaults(graph, program);
}

fn register_typedef(graph: &mut Graph, definition: &HirTypeDef) {
    let HirTypeDefKind::Enum { variants } = &definition.kind else {
        if definition.has_user_operators {
            graph.user_operators.insert(definition.name.clone());
        }
        return;
    };
    for variant in variants {
        let (name, fields) = variant_fields(variant);
        graph.constructors.insert(name.clone());
        graph
            .variant_types
            .insert(name, (definition.type_params.clone(), fields));
    }
    if definition.has_user_operators {
        graph.user_operators.insert(definition.name.clone());
    }
}

fn variant_fields(variant: &HirEnumVariant) -> (String, Vec<Type>) {
    match variant {
        HirEnumVariant::Unit(name) => (name.clone(), Vec::new()),
        HirEnumVariant::Tuple(name, fields) => (name.clone(), fields.clone()),
        HirEnumVariant::Struct(name, fields) => {
            (name.clone(), fields.iter().map(|field| field.ty.clone()).collect())
        }
    }
}

fn register_methods(graph: &mut Graph, definition: &HirTypeDef, program: &HirProgram) {
    let field_names = member_field_names(definition);
    let members = program.callables.iter().filter(|callable| {
        callable.owner.as_deref() == Some(definition.name.as_str())
            && !callable.is_impl_fn
            && !callable.trait_default
            && matches!(
                callable.kind,
                HirCallableKind::MemberDefn | HirCallableKind::MemberTransaction
            )
    });
    for callable in members {
        register_method(graph, definition, callable, &field_names);
    }
}

fn member_field_names(definition: &HirTypeDef) -> Vec<String> {
    match &definition.kind {
        HirTypeDefKind::Object { fields } | HirTypeDefKind::Struct { fields } => {
            fields.iter().map(|field| field.name.clone()).collect()
        }
        HirTypeDefKind::Enum { .. } => Vec::new(),
    }
}

fn register_method(
    graph: &mut Graph,
    definition: &HirTypeDef,
    callable: &HirCallable,
    field_names: &[String],
) {
    let key = (definition.name.clone(), callable.name.clone());
    graph.methods.insert(key.clone(), None);
    if !explicit_receiver(definition, callable, field_names) {
        return;
    }
    let mut synthetic = callable.clone();
    synthetic.name = format!("{}::{}", definition.name, callable.name);
    register_definition(graph, &synthetic);
    graph.methods.insert(key, Some(synthetic.name));
}

fn explicit_receiver(
    definition: &HirTypeDef,
    callable: &HirCallable,
    field_names: &[String],
) -> bool {
    if callable.kind != HirCallableKind::MemberDefn
        || !definition.type_params.is_empty()
        || !callable.type_params.is_empty()
        || super::uses_implicit_receiver(callable, field_names)
    {
        return false;
    }
    callable
        .params
        .first()
        .is_some_and(|receiver| receiver.ty.universe_key() == Some(definition.name.as_str()))
}

fn register_free(graph: &mut Graph, callable: &HirCallable) {
    if callable.owner.is_some()
        || callable.is_isr
        || callable.trait_default
        || callable.is_impl_fn
    {
        return;
    }
    match callable.kind {
        HirCallableKind::Defn => {
            if !callable.type_params.is_empty() {
                graph.generics.insert(callable.name.clone(), callable.clone());
                return;
            }
            register_definition(graph, callable);
        }
        HirCallableKind::Transaction => register_transaction(graph, callable),
        _ => {}
    }
}

fn register_definition(graph: &mut Graph, callable: &HirCallable) {
    register_callable(graph, &callable.name, callable.params.len());
    let params: Vec<(String, Type)> = callable
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect();
    graph
        .bodies
        .insert(callable.name.clone(), (params, callable.body.statements.clone()));
    register_result(graph, &callable.name, callable.outputs.as_ref(), false);
}

fn register_transaction(graph: &mut Graph, callable: &HirCallable) {
    register_callable(graph, &callable.name, callable.params.len());
    let mut body = callable.body.statements.clone();
    if let Some(contract) = &callable.contract {
        body.push(contract_statement(contract.pre.clone()));
        body.push(contract_statement(contract.post.clone()));
    }
    let params: Vec<(String, Type)> = callable
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect();
    graph.bodies.insert(callable.name.clone(), (params, body));
    register_result(graph, &callable.name, callable.outputs.as_ref(), callable.is_reactive);
    if callable.is_reactive {
        graph.entry_nodes.insert(callable.name.clone());
    }
    if callable.is_async {
        graph.unsupported.insert(callable.name.clone());
    }
}

fn contract_statement(expression: HirExpr) -> HirStmt {
    HirStmt {
        id: NodeId(0),
        span: expression.span,
        kind: HirStmtKind::Expression(expression),
    }
}

fn register_result(
    graph: &mut Graph,
    name: &str,
    output: Option<&HirOutput>,
    void_default: bool,
) {
    let ty = output.and_then(output_value_type);
    let scalar = match output {
        Some(HirOutput::Single(ty)) => scalar_result(graph, ty),
        None => void_default,
        _ => false,
    };
    if let Some(ty) = ty {
        graph.return_types.insert(name.into(), ty);
    }
    if scalar {
        return;
    }
    graph.handle_returns.insert(name.into());
    let image = graph.allocation(name);
    let result = graph.callables[name].result;
    graph.copy(image, result);
}

fn output_value_type(output: &HirOutput) -> Option<Type> {
    match output {
        HirOutput::Tuple(items) => items
            .iter()
            .map(output_value_type)
            .collect::<Option<Vec<_>>>()
            .map(Type::Tuple),
        HirOutput::Named(_, inner) => output_value_type(inner),
        _ => output.all_types().into_iter().next(),
    }
}

fn scalar_result(graph: &Graph, ty: &Type) -> bool {
    let fields = ty
        .universe_key()
        .and_then(|key| graph.universe.get(key))
        .is_some_and(|resolved| !resolved.fields.is_empty());
    if fields {
        return false;
    }
    crate::type_universe::protocol_category(&graph.universe, ty)
        .is_some_and(|category| matches!(category.as_str(), "Int" | "UInt" | "Bool" | "Float" | "Char" | "Bit"))
}

fn register_defaults(graph: &mut Graph, program: &HirProgram) {
    for callable in &program.callables {
        if !callable.trait_default || callable.body.statements.is_empty() {
            continue;
        }
        if graph.callables.contains_key(&callable.name) || graph.generics.contains_key(&callable.name) {
            continue;
        }
        let mut generic = callable.clone();
        generic.type_params = vec!["Self".into()];
        graph.generics.insert(callable.name.clone(), generic);
    }
}

fn register_callable(graph: &mut Graph, name: &str, count: usize) {
    let params = (0..count).map(|_| graph.root()).collect();
    let result = graph.node();
    graph.callables.insert(
        name.into(),
        Callable {
            params,
            result,
            calls: BTreeSet::new(),
        },
    );
}

pub(super) fn build_owner(graph: &mut Graph, owner: &str) {
    if !graph.built.insert(owner.into()) {
        return;
    }
    if let Some((params, statements)) = graph.bodies.get(owner).cloned() {
        body(graph, owner, &params, &statements);
    }
}

fn body(graph: &mut Graph, owner: &str, params: &[(String, Type)], statements: &[HirStmt]) {
    let Some(callable) = graph.callables.get(owner).cloned() else { return };
    let bindings = params
        .iter()
        .zip(callable.params)
        .map(|((name, _), node)| (name.clone(), node))
        .collect();
    let types = params.iter().cloned().collect();
    let param_names = params.iter().map(|(name, _)| name.clone()).collect();
    let mut builder = Builder {
        graph,
        owner,
        bindings,
        types,
        result: callable.result,
        param_names,
        alloc_bindings: BTreeSet::new(),
    };
    builder.statements(statements);
}

struct Builder<'a> {
    graph: &'a mut Graph,
    owner: &'a str,
    bindings: BTreeMap<String, Node>,
    types: BTreeMap<String, Type>,
    result: Node,
    param_names: BTreeSet<String>,
    alloc_bindings: BTreeSet<String>,
}

impl Builder<'_> {
    fn binding(&mut self, name: &str) -> Node {
        if let Some(node) = self.bindings.get(name) {
            return *node;
        }
        self.graph.global(name)
    }

    fn local(&mut self, name: &str) -> Node {
        if let Some(node) = self.bindings.get(name) {
            return *node;
        }
        let node = self.graph.node();
        self.bindings.insert(name.into(), node);
        node
    }

    fn statements(&mut self, statements: &[HirStmt]) {
        for statement in statements {
            self.statement(statement);
        }
    }

    fn statement(&mut self, statement: &HirStmt) {
        match &statement.kind {
            HirStmtKind::Let { name, names, expr, ty, .. } => {
                self.statement_let(name, names, ty.as_ref(), expr.as_ref())
            }
            HirStmtKind::Assign(target, value) => self.statement_assign(target, value),
            HirStmtKind::Term(Some(value)) | HirStmtKind::Tail(value) => {
                let value = self.expression(value);
                self.graph.copy(value, self.result);
            }
            HirStmtKind::Term(None) => {}
            HirStmtKind::Expression(value) | HirStmtKind::Check(value) | HirStmtKind::Gate(value) => {
                self.expression(value);
            }
            HirStmtKind::Guarded(condition, body) => {
                self.expression(condition);
                self.statements(body);
            }
            HirStmtKind::Block(body)
            | HirStmtKind::SyncBlock(body)
            | HirStmtKind::Mutex(body)
            | HirStmtKind::Defer(body) => self.statements(body),
            HirStmtKind::Foreach { item, list, body, .. } => {
                if matches!(list.kind, HirExprKind::Range { .. }) {
                    self.types.insert(item.clone(), Type::int());
                }
                let source = self.expression(list);
                let target = self.local(item);
                self.load(source, target);
                self.statements(body);
            }
            HirStmtKind::Match { expr, arms } => self.statement_match(expr, arms),
            HirStmtKind::ArrowAssign { target, value, .. } => {
                let value = self.expression(value);
                if let Some(target) = target {
                    let target = self.expression(target);
                    let backing = self.graph.allocation(self.owner);
                    self.graph.growers.insert(self.owner.into());
                    // 2026-11-12: mark the proxy as container storage, not an
                    // element value. Element reads exclude it; container escape
                    // still reaches it through `root_contents`.
                    self.graph.container_storage.insert(backing);
                    self.graph.constraints.push(Constraint::Store(target, backing));
                    self.graph.constraints.push(Constraint::Store(target, value));
                }
            }
            HirStmtKind::EndProgram(value) | HirStmtKind::Rollback(value) => {
                if let Some(value) = value {
                    self.expression(value);
                }
            }
            HirStmtKind::KeepHint(name) => {
                let value = self.binding(name);
                self.capture(&[value]);
            }
            HirStmtKind::Break
            | HirStmtKind::Trap
            | HirStmtKind::Halt
            | HirStmtKind::MetadataAssignment(..)
            | HirStmtKind::FreeHint(_) => {}
            _ => self.unsupported(),
        }
    }

    fn statement_match(&mut self, expression: &HirExpr, arms: &[HirStmtMatchArm]) {
        let value = self.expression(expression);
        let ty = self.expression_type(expression);
        for arm in arms {
            self.statement_arm_typed(arm, value, ty.as_ref());
        }
    }

    fn statement_arm_typed(&mut self, arm: &HirStmtMatchArm, value: Node, ty: Option<&Type>) {
        for pattern in &arm.patterns {
            self.pattern_types(pattern, ty);
        }
        self.statement_arm(value, arm);
    }

    fn bind_types(
        &mut self,
        name: &str,
        names: &[String],
        declared: Option<&Type>,
        expr: Option<&HirExpr>,
    ) {
        let Some(ty) = declared
            .cloned()
            .or_else(|| expr.and_then(|expr| self.expression_type(expr)))
        else {
            return;
        };
        self.types.insert(name.into(), ty.clone());
        if names.len() <= 1 {
            return;
        }
        if let Type::Tuple(elements) = ty {
            self.types.extend(names.iter().cloned().zip(elements));
        }
    }

    fn statement_let(
        &mut self,
        name: &str,
        names: &[String],
        ty: Option<&Type>,
        expr: Option<&HirExpr>,
    ) {
        self.bind_types(name, names, ty, expr);
        let target = self.local(name);
        let Some(expr) = expr else { return };
        if self.value_allocates(expr) {
            self.alloc_bindings.insert(name.into());
        }
        let value = self.expression(expr);
        if names.len() <= 1 {
            self.graph.copy(value, target);
            return;
        }
        self.destructure_names(value, names);
    }

    fn statement_assign(&mut self, target: &HirExpr, value_expr: &HirExpr) {
        let allocating = self.value_allocates(value_expr);
        let value = self.expression(value_expr);
        if allocating {
            if let HirExprKind::Identifier(name) = &target.kind {
                self.alloc_bindings.insert(name.clone());
            }
        }
        self.assign(target, value, allocating);
    }

    fn statement_arm(&mut self, value: Node, arm: &HirStmtMatchArm) {
        for pattern in &arm.patterns {
            self.pattern(pattern, value);
        }
        self.statements(&arm.body);
    }

    fn assign(&mut self, target: &HirExpr, value: Node, allocating: bool) {
        if let HirExprKind::Identifier(name) = &target.kind {
            let target = self.binding(name);
            self.graph.copy(value, target);
            return;
        }
        if self.assign_field(target, value, allocating) {
            return;
        }
        let base = match &target.kind {
            HirExprKind::Field { receiver, .. }
            | HirExprKind::Index(receiver, _)
            | HirExprKind::Deref(receiver) => self.expression(receiver),
            _ => {
                self.unsupported();
                return;
            }
        };
        self.graph.constraints.push(Constraint::Store(base, value));
    }

    fn assign_field(&mut self, target: &HirExpr, value: Node, allocating: bool) -> bool {
        if !matches!(target.kind, HirExprKind::Field { .. }) {
            return false;
        }
        if let Some((root, path)) = projection_root_and_path(target) {
            self.record_incoming_store(allocating, root);
            let base = self.expression(root);
            self.graph.constraints.push(Constraint::FieldStore(base, path, value));
            return true;
        }
        if let HirExprKind::Field { receiver, name } = &target.kind {
            let base = self.expression(receiver);
            self.graph.constraints.push(Constraint::FieldStore(base, name.clone(), value));
        }
        true
    }

    fn record_incoming_store(&mut self, allocating: bool, root: &HirExpr) {
        if !allocating {
            return;
        }
        if let HirExprKind::Identifier(root_name) = &root.kind {
            if self.param_names.contains(root_name.as_str()) {
                self.graph.incoming_store_owners.insert(self.owner.into());
                self.graph
                    .publish_receiver_param
                    .insert(self.owner.into(), root_name.clone());
            }
        }
    }

    fn load(&mut self, base: Node, value: Node) {
        self.graph.constraints.push(Constraint::Load(base, value));
    }

    fn destructure_names(&mut self, value: Node, names: &[String]) {
        for (index, name) in names.iter().enumerate() {
            let target = self.local(name);
            self.graph
                .constraints
                .push(Constraint::FieldLoad(value, index.to_string(), target));
        }
    }

    fn pattern(&mut self, pattern: &HirPattern, value: Node) {
        match &pattern.kind {
            HirPatternKind::Binding(name) | HirPatternKind::TypedBinding(name, _) => {
                let target = self.local(name);
                self.graph.copy(value, target);
            }
            HirPatternKind::EnumVariant(variant, fields) => {
                let payload = self.graph.node();
                let variant = self.graph.constructor_key(variant).unwrap_or_else(|| variant.clone());
                self.graph.constraints.push(Constraint::FieldLoad(
                    value,
                    format!("@variant:{variant}"),
                    payload,
                ));
                for field in fields {
                    self.pattern(field, payload);
                }
            }
            HirPatternKind::Tuple(fields) => {
                for (index, field) in fields.iter().enumerate() {
                    let payload = self.graph.node();
                    self.graph.constraints.push(Constraint::FieldLoad(
                        value,
                        index.to_string(),
                        payload,
                    ));
                    self.pattern(field, payload);
                }
            }
            HirPatternKind::Multi(patterns) => {
                for pattern in patterns {
                    self.pattern(pattern, value);
                }
            }
            HirPatternKind::Literal(expression) => {
                self.expression(expression);
            }
            HirPatternKind::Range(start, end) | HirPatternKind::RangeInclusive(start, end) => {
                self.expression(start);
                self.expression(end);
            }
            HirPatternKind::Wildcard => {}
        }
    }

    fn pattern_types(&mut self, pattern: &HirPattern, ty: Option<&Type>) {
        match &pattern.kind {
            HirPatternKind::Binding(name) => {
                if let Some(ty) = ty {
                    self.types.insert(name.clone(), ty.clone());
                }
            }
            HirPatternKind::TypedBinding(name, ty) => {
                self.types.insert(name.clone(), ty.clone());
            }
            HirPatternKind::EnumVariant(name, patterns) => {
                self.variant_pattern_types(name, patterns, ty)
            }
            HirPatternKind::Tuple(patterns) => {
                if let Some(Type::Tuple(fields)) = ty {
                    for (pattern, field) in patterns.iter().zip(fields) {
                        self.pattern_types(pattern, Some(field));
                    }
                }
            }
            HirPatternKind::Multi(patterns) => {
                for pattern in patterns {
                    self.pattern_types(pattern, ty);
                }
            }
            _ => {}
        }
    }

    fn variant_pattern_types(&mut self, name: &str, patterns: &[HirPattern], ty: Option<&Type>) {
        let name = self.graph.constructor_key(name).unwrap_or_else(|| name.into());
        let Some((params, fields)) = self.graph.variant_types.get(&name).cloned() else { return };
        let args = match ty {
            Some(Type::Applied(_, args) | Type::Generic(_, args)) => args.as_slice(),
            _ => &[],
        };
        let substitution = params.into_iter().zip(args.iter().cloned()).collect();
        for (pattern, field) in patterns.iter().zip(fields) {
            self.pattern_types(
                pattern,
                Some(&crate::typechecker::substitute_type(&field, &substitution)),
            );
        }
    }

    fn tuple(&mut self, values: &[HirExpr]) -> Node {
        let root = self.graph.allocation(self.owner);
        for (index, expression) in values.iter().enumerate() {
            let value = self.expression(expression);
            self.graph
                .constraints
                .push(Constraint::FieldStore(root, index.to_string(), value));
        }
        root
    }

    fn expression(&mut self, expression: &HirExpr) -> Node {
        match &expression.kind {
            HirExprKind::Identifier(name) => self.binding(name),
            HirExprKind::Literal(HirLiteral::Int(value)) => {
                let node = self.graph.node();
                self.graph.constants[node].insert(*value);
                node
            }
            HirExprKind::Literal(HirLiteral::Bytes(_)) => self.graph.root(),
            HirExprKind::Call { callee, args, .. } => self.call(callee, args),
            other => self.expression_structural(expression, other),
        }
    }

    fn expression_structural(&mut self, expression: &HirExpr, kind: &HirExprKind) -> Node {
        match kind {
            HirExprKind::StructLiteral { .. }
            | HirExprKind::List(_)
            | HirExprKind::Tuple(_)
            | HirExprKind::Slice { .. }
            | HirExprKind::Index(..)
            | HirExprKind::Field { .. }
            | HirExprKind::Deref(_)
            | HirExprKind::Cast { .. }
            | HirExprKind::MethodCall { .. }
            | HirExprKind::Consume(_)
            | HirExprKind::AddrOf(_)
            | HirExprKind::IsType { .. } => self.expression_composite(expression, kind),
            HirExprKind::Unary { op, inner } => self.unary(*op, inner),
            HirExprKind::Binary { op, left, right } => self.binary(*op, left, right),
            HirExprKind::Reflect { receiver, name, .. } => {
                let value = self.expression(receiver);
                if matches!(
                    name.as_str(),
                    "Length" | "Size" | "Count" | "Bytes" | "Bits" | "Capacity" | "Alignment"
                ) {
                    return self.graph.unknown_value();
                }
                if !matches!(name.as_str(), "Ptr" | "Data" | "Address") {
                    self.unsupported();
                    self.capture(&[value]);
                }
                value
            }
            HirExprKind::If { condition, then_branch, else_branch } => {
                self.expression(condition);
                let result = self.graph.unknown_value();
                let yes = self.expression(then_branch);
                self.graph.copy(yes, result);
                if let Some(no) = else_branch {
                    let value = self.expression(no);
                    self.graph.copy(value, result);
                }
                result
            }
            HirExprKind::Match { scrutinee, arms } => {
                let ty = self.expression_type(scrutinee);
                let value = self.expression(scrutinee);
                let result = self.graph.node();
                for arm in arms {
                    self.pattern_types(&arm.pattern, ty.as_ref());
                    self.expression_arm(value, result, arm);
                }
                result
            }
            HirExprKind::Block(statements) => self.block(statements),
            HirExprKind::Range { start, end, .. } => {
                self.expression(start);
                self.expression(end);
                self.graph.node()
            }
            HirExprKind::Literal(
                HirLiteral::Bool(_)
                | HirLiteral::Char(_)
                | HirLiteral::Float(_)
                | HirLiteral::BeginProgram,
            )
            | HirExprKind::UnitLiteral { .. }
            | HirExprKind::Other("formatting-annotation")
            | HirExprKind::Other("exists") => self.graph.node(),
            _ => {
                if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() {
                    eprintln!("reclamation HIR unsupported expression in {}: {expression:?}", self.owner);
                }
                self.unsupported();
                self.graph.root()
            }
        }
    }

    fn expression_composite(&mut self, expression: &HirExpr, kind: &HirExprKind) -> Node {
        match kind {
            HirExprKind::StructLiteral { fields, specs, .. } => {
                let root = self.graph.allocation(self.owner);
                for (field, value) in fields.iter().chain(specs) {
                    let value = self.expression(value);
                    self.graph
                        .constraints
                        .push(Constraint::FieldStore(root, field.clone(), value));
                }
                root
            }
            HirExprKind::List(values) => {
                let root = self.graph.allocation(self.owner);
                for value in values {
                    self.store_expression(root, value);
                }
                root
            }
            HirExprKind::Tuple(values) => self.tuple(values),
            HirExprKind::Slice { array, start, end, stride } => {
                let base = self.indexed_base(array);
                for bound in [start, end, stride].into_iter().flatten() {
                    self.expression(bound);
                }
                let result = self.graph.allocation(self.owner);
                self.graph.copy(base, result);
                self.graph.constraints.push(Constraint::CopyMemory(base, result));
                result
            }
            HirExprKind::Index(base, index) => {
                self.expression(index);
                let base = self.indexed_base(base);
                let result = self.graph.node();
                self.load(base, result);
                result
            }
            HirExprKind::Field { receiver, name } => {
                let base = self.expression(receiver);
                let result = if self.graph.constructor_key(name).is_some() {
                    self.graph.allocation(self.owner)
                } else {
                    self.graph.node()
                };
                self.graph.constraints.push(Constraint::FieldLoad(base, name.clone(), result));
                result
            }
            HirExprKind::Deref(base) => {
                let base = self.expression(base);
                let result = self.graph.node();
                self.load(base, result);
                result
            }
            HirExprKind::Cast { inner, to, .. } => self.cast(inner, to),
            HirExprKind::MethodCall { receiver, name, args, analysis_id } => {
                if analysis_id.is_some() {
                    self.unsupported();
                    return self.graph.root();
                }
                if crate::intrinsic_signatures::get_intrinsic_signature(name)
                    .is_some_and(|signature| !signature.observable)
                {
                    let mut values = vec![self.expression(receiver)];
                    values.extend(args.iter().map(|argument| self.expression(argument)));
                    self.intrinsic_or_unknown(name, &values)
                } else {
                    self.method_call(receiver, name, args)
                }
            }
            HirExprKind::Consume(value) => self.expression(value),
            HirExprKind::AddrOf(value) => {
                let value = self.expression(value);
                let result = self.graph.allocation(self.owner);
                self.graph.constraints.push(Constraint::Store(result, value));
                result
            }
            HirExprKind::IsType { inner, .. } => {
                self.expression(inner);
                self.graph.node()
            }
            _ => {
                self.unsupported();
                self.graph.root()
            }
        }
    }

    fn indexed_base(&mut self, expression: &HirExpr) -> Node {
        let value = self.expression(expression);
        if self
            .expression_type(expression)
            .is_none_or(|ty| self.has_user_operators(&ty))
        {
            self.unsupported();
            self.capture(&[value]);
        }
        value
    }

    fn store_expression(&mut self, target: Node, value: &HirExpr) {
        let value = self.expression(value);
        self.graph.constraints.push(Constraint::Store(target, value));
    }

    fn expression_arm(
        &mut self,
        value: Node,
        result: Node,
        arm: &crate::hir::expr::HirMatchArm,
    ) {
        self.pattern(&arm.pattern, value);
        if let Some(guard) = &arm.guard {
            self.expression(guard);
        }
        let value = self.expression(&arm.body);
        self.graph.copy(value, result);
    }

    fn block(&mut self, statements: &[HirStmt]) -> Node {
        let result = self.graph.node();
        let Some((last, prefix)) = statements.split_last() else { return result };
        self.statements(prefix);
        match &last.kind {
            HirStmtKind::Expression(value) | HirStmtKind::Tail(value) => {
                let value = self.expression(value);
                self.graph.copy(value, result);
            }
            _ => self.statement(last),
        }
        result
    }

    fn binary(&mut self, operator: BinaryOpKind, lhs: &HirExpr, rhs: &HirExpr) -> Node {
        let left = self.expression(lhs);
        let right = self.expression(rhs);
        let result = self.binary_value(operator, left, right);
        if self.expression_type(lhs).is_none() || self.expression_type(rhs).is_none() {
            self.unsupported();
            self.capture(&[left, right]);
        }
        if self
            .expression_type(lhs)
            .is_some_and(|ty| self.has_user_operators(&ty))
        {
            self.unsupported();
            self.capture(&[left, right]);
        }
        if matches!(
            operator,
            BinaryOpKind::Eq
                | BinaryOpKind::Neq
                | BinaryOpKind::Lt
                | BinaryOpKind::Gt
                | BinaryOpKind::Le
                | BinaryOpKind::Ge
                | BinaryOpKind::And
                | BinaryOpKind::Or
        ) {
            return result;
        }
        if operator == BinaryOpKind::Concat
            || self.allocating_operator(lhs, operator)
            || self.allocating_operator(rhs, operator)
        {
            let allocation = self.graph.allocation(self.owner);
            self.graph.copy(allocation, result);
        }
        result
    }

    fn binary_value(&mut self, operator: BinaryOpKind, left: Node, right: Node) -> Node {
        if matches!(
            operator,
            BinaryOpKind::BitAnd | BinaryOpKind::BitOr | BinaryOpKind::BitXor
        ) {
            let result = self.graph.node();
            self.graph
                .constraints
                .push(Constraint::Bitwise(operator, left, right, result));
            return result;
        }
        let result = self.graph.unknown_value();
        if matches!(
            operator,
            BinaryOpKind::Eq
                | BinaryOpKind::Neq
                | BinaryOpKind::Lt
                | BinaryOpKind::Gt
                | BinaryOpKind::Le
                | BinaryOpKind::Ge
                | BinaryOpKind::And
                | BinaryOpKind::Or
        ) {
            return result;
        }
        self.graph.copy(left, result);
        self.graph.copy(right, result);
        result
    }

    fn unary(&mut self, operator: UnaryOpKind, expression: &HirExpr) -> Node {
        let value = self.expression(expression);
        if self
            .expression_type(expression)
            .is_none_or(|ty| self.has_user_operators(&ty))
        {
            self.unsupported();
            self.capture(&[value]);
        }
        if operator == UnaryOpKind::Not {
            return self.graph.node();
        }
        if operator == UnaryOpKind::BitNot
            && self.allocating_operator(expression, BinaryOpKind::BitXor)
        {
            let fresh = self.graph.allocation(self.owner);
            self.graph.copy(value, fresh);
            return fresh;
        }
        let result = self.graph.unknown_value();
        self.graph.copy(value, result);
        result
    }

    fn method_expression_type(
        &self,
        receiver: &HirExpr,
        name: &str,
        args: &[HirExpr],
        analysis_id: Option<usize>,
    ) -> Option<Type> {
        {
                if analysis_id.is_some() {
                    return None;
                }
                let intrinsic = crate::intrinsic_signatures::get_intrinsic_signature(name)
                    .and_then(|signature| match signature.return_kind {
                        crate::intrinsic_signatures::ReturnKind::Native(category) => {
                            Some(Some(Type::Custom(category.into())))
                        }
                        crate::intrinsic_signatures::ReturnKind::Exact(ty) => Some(Some(ty)),
                        _ => None,
                    });
                match intrinsic {
                    Some(ty) => ty,
                    None => {
                        let target = self.method_target(receiver, name, args.len())?;
                        let mut arguments = vec![receiver.clone()];
                        arguments.extend_from_slice(args);
                        self.graph
                            .return_types
                            .get(&target)
                            .cloned()
                            .or_else(|| self.generic_result(&target, &arguments))
                    }
                }
            }
    }

    fn call_expression_type(&self, callee: &str, args: &[HirExpr]) -> Option<Type> {
        self
                .graph
                .return_types
                .get(callee)
                .cloned()
                .or_else(|| self.generic_result(callee, args))
                .or_else(|| {
                    let target = self.free_member_target(callee, args)?;
                    self.graph.return_types.get(&target).cloned()
                })
                .or_else(|| {
                    use crate::intrinsic_signatures::ReturnKind;
                    match crate::intrinsic_signatures::get_intrinsic_signature(callee)?
                        .return_kind
                    {
                        ReturnKind::Exact(ty) => Some(ty),
                        ReturnKind::Native(category) => Some(Type::Custom(category.into())),
                        _ => None,
                    }
                })
    }

    fn has_user_operators(&self, ty: &Type) -> bool {
        let Some(mut key) = ty.universe_key() else { return false };
        let mut visited = BTreeSet::new();
        while visited.insert(key.to_string()) {
            if self.graph.user_operators.contains(key) {
                return true;
            }
            let Some(resolved) = self.graph.universe.get(key) else { break };
            key = &resolved.base;
        }
        false
    }

    fn expression_type(&self, expression: &HirExpr) -> Option<Type> {
        match &expression.kind {
            HirExprKind::Identifier(name) => self
                .types
                .get(name)
                .or_else(|| self.graph.global_types.get(name))
                .cloned(),
            HirExprKind::Literal(HirLiteral::Bytes(_)) => Some(Type::string()),
            HirExprKind::Literal(HirLiteral::Int(_) | HirLiteral::TaggedInt(..)) => {
                Some(Type::int())
            }
            HirExprKind::Literal(HirLiteral::Char(_)) => Some(Type::char_()),
            HirExprKind::Literal(HirLiteral::Float(_)) => Some(Type::float()),
            HirExprKind::Literal(HirLiteral::Bool(_)) => Some(Type::bool_()),
            HirExprKind::Tuple(values) => values
                .iter()
                .map(|value| self.expression_type(value))
                .collect::<Option<Vec<_>>>()
                .map(Type::Tuple),
            HirExprKind::Cast { to, .. } => Some(to.clone()),
            HirExprKind::MethodCall { receiver, name, args, analysis_id } => {
                self.method_expression_type(receiver, name, args, *analysis_id)
            }
            HirExprKind::Call { callee, args, .. } => self.call_expression_type(callee, args),
            HirExprKind::Unary { inner, .. } | HirExprKind::Consume(inner) => {
                self.expression_type(inner)
            }
            HirExprKind::Reflect { .. } => Some(Type::int()),
            HirExprKind::StructLiteral { type_name, .. } => {
                Some(Type::Custom(type_name.clone()))
            }
            HirExprKind::Index(base, index) => {
                if let (HirExprKind::Tuple(values), HirExprKind::Literal(HirLiteral::Int(index))) =
                    (&base.kind, &index.kind)
                {
                    return usize::try_from(*index)
                        .ok()
                        .and_then(|index| values.get(index))
                        .and_then(|value| self.expression_type(value));
                }
                let mut context = crate::typechecker::TypecheckContext::new(&self.graph.universe);
                context
                    .bindings
                    .insert("base".into(), self.expression_type(base)?);
                context
                    .bindings
                    .insert("index".into(), self.expression_type(index)?);
                let expression = crate::ast::Expr::Index(
                    Box::new(crate::ast::Expr::Identifier("base".into())),
                    Box::new(crate::ast::Expr::Identifier("index".into())),
                );
                crate::typechecker::infer_type_only(&expression, &mut context).ok()
            }
            HirExprKind::Slice { array, .. } => self.expression_type(array),
            HirExprKind::Binary { left, .. } => self.expression_type(left),
            HirExprKind::Field { receiver, name } => {
                let ty = self.expression_type(receiver)?;
                if let Type::Tuple(elements) = &ty {
                    return name
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| elements.get(index))
                        .cloned();
                }
                let resolved = self.graph.universe.get(ty.universe_key()?)?;
                let field_type = &resolved
                    .fields
                    .iter()
                    .find(|(field, _)| field == name)?
                    .1;
                let args = match &ty {
                    Type::Applied(_, args) | Type::Generic(_, args) => args.as_slice(),
                    _ => &[],
                };
                let substitution = resolved
                    .type_params
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect();
                Some(crate::typechecker::substitute_type(field_type, &substitution))
            }
            _ => None,
        }
    }

    fn generic_result(&self, name: &str, arguments: &[HirExpr]) -> Option<Type> {
        let generic = self.graph.generics.get(name)?;
        let params: Vec<_> = generic.type_params.clone();
        let mut substitution = HashMap::new();
        for (declared, argument) in generic.params.iter().zip(arguments) {
            if let Some(concrete) = self.expression_type(argument) {
                crate::analysis::generic_specialization::unify_generic_binding(
                    &declared.ty,
                    &concrete,
                    &params,
                    &mut substitution,
                );
            }
        }
        let output = output_value_type(generic.outputs.as_ref()?)?;
        Some(crate::typechecker::substitute_type(&output, &substitution))
    }

    fn allocating_operator(&self, value: &HirExpr, operator: BinaryOpKind) -> bool {
        let Some(ty) = self.expression_type(value) else { return false };
        let rune = match operator {
            BinaryOpKind::Add => "+",
            BinaryOpKind::BitAnd => "&",
            BinaryOpKind::BitOr => "|",
            BinaryOpKind::BitXor => "^",
            _ => return false,
        };
        matches!(
            crate::type_universe::get_operator_intrinsic(&self.graph.universe, rune, &ty),
            Some(crate::ast::OpBinding::Intrinsic(name))
                if matches!(
                    name.as_str(),
                    "StringConcat#" | "StringBitAnd#" | "StringBitOr#" | "StringBitXor#"
                )
        )
    }

    fn cast(&mut self, expression: &HirExpr, target: &Type) -> Node {
        let original = self.expression(expression);
        let mut value = self.graph.unknown_value();
        self.graph.copy(original, value);
        let Some(source) = self.expression_type(expression) else {
            if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() {
                eprintln!("reclamation HIR unknown cast source in {}: {expression:?}", self.owner);
            }
            self.unsupported();
            return value;
        };
        if self.has_user_operators(&source) || self.has_user_operators(target) {
            self.unsupported();
            return value;
        }
        let encoding_target = target.universe_key().map(str::to_owned);
        let address_cast = matches!(&source, Type::Ptr(_)) || matches!(target, Type::Ptr(_));
        let (source, source_variant) = self
            .graph
            .casts
            .type_to_protocol(&self.graph.universe, &source);
        let (target, target_variant) = self
            .graph
            .casts
            .type_to_protocol(&self.graph.universe, target);
        let Some(steps) = self
            .graph
            .casts
            .find_path(&source, &source_variant, &target, &target_variant)
        else {
            if !address_cast {
                self.unsupported();
            }
            return value;
        };
        for step in steps {
            value = self.cast_lane(&step.lane, value, encoding_target.as_deref());
        }
        value
    }

    fn cast_lane(
        &mut self,
        lane: &crate::casting::graph::LaneKind,
        value: Node,
        encoding_target: Option<&str>,
    ) -> Node {
        use crate::casting::graph::LaneKind;
        match lane {
            LaneKind::ExtCall(name) => self.call_nodes(name, &[value]),
            LaneKind::ExtCallDyn(name) => self.call_nodes(name, &[value]),
            LaneKind::Chain(first, second) => {
                let value = self.cast_lane(first, value, encoding_target);
                self.cast_lane(second, value, encoding_target)
            }
            LaneKind::CastFromBitCallback => {
                let Some(target) = encoding_target else {
                    self.unsupported();
                    return value;
                };
                self.graph.default_encoding_targets.insert(target.into());
                let fresh = self.graph.allocation(self.owner);
                self.graph.constraints.push(Constraint::CopyMemory(value, fresh));
                fresh
            }
            _ => value,
        }
    }

    fn method_target(&self, receiver: &HirExpr, name: &str, supplied: usize) -> Option<String> {
        let ty = self.expression_type(receiver)?;
        let key = (ty.universe_key()?.to_string(), name.to_string());
        if let Some(target) = self.graph.methods.get(&key) {
            let target = target.as_ref()?;
            return (self.graph.callables[target].params.len() == supplied + 1)
                .then(|| target.clone());
        }
        (self.graph.callables.contains_key(name) || self.graph.generics.contains_key(name))
            .then(|| name.to_string())
    }

    fn method_call(&mut self, receiver: &HirExpr, name: &str, args: &[HirExpr]) -> Node {
        let target = self.method_target(receiver, name, args.len());
        let mut arguments = vec![receiver.clone()];
        arguments.extend_from_slice(args);
        if let Some(target) = target {
            return self.call(&target, &arguments);
        }
        let values = arguments.iter().map(|arg| self.expression(arg)).collect::<Vec<_>>();
        self.capture(&values);
        self.unsupported();
        self.graph.unknown_value()
    }

    fn call(&mut self, name: &str, arguments: &[HirExpr]) -> Node {
        let args: Vec<_> = arguments.iter().map(|arg| self.expression(arg)).collect();
        if let Some(generic) = self.graph.generics.get(name).cloned() {
            let result = self.specialize(&generic, arguments, &args);
            self.propagate_incoming_store(name, arguments);
            return result;
        }
        if let Some(target) = self.free_member_target(name, arguments) {
            let result = self.call_nodes(&target, &args);
            self.record_publish_call(&target, arguments);
            self.propagate_incoming_store(&target, arguments);
            return result;
        }
        let result = self.call_nodes(name, &args);
        self.record_publish_call(name, arguments);
        self.propagate_incoming_store(name, arguments);
        result
    }

    fn intrinsic_helper_effects(&mut self, intrinsic: &str, args: &[Node], result: Node) {
        let helpers = crate::analysis::defn_liveness::intrinsic_helpers(intrinsic);
        if helpers.is_empty() { return; }
        let arguments = self.graph.unknown_value();
        for arg in args { self.graph.copy(*arg, arguments); }
        for helper in helpers { self.record_helper_effect(helper, arguments, result); }
    }

    fn record_helper_effect(&mut self, helper: &str, arguments: Node, result: Node) {
        let Some(callable) = self.graph.callables.get(helper).cloned() else {
            self.graph.unmodeled_helpers.insert(self.owner.into());
            return;
        };
        self.graph.callables.get_mut(self.owner).unwrap().calls.insert(helper.into());
        for param in callable.params { self.graph.copy(arguments, param); }
        self.graph.copy(callable.result, result);
        build_owner(self.graph, helper);
    }

    fn record_publish_call(&mut self, target: &str, arguments: &[HirExpr]) {
        let Some(receiver) = self.graph.publish_receiver_param.get(target).cloned() else { return };
        let Some(index) = self.graph.bodies.get(target).and_then(|(params, _)| {
            params.iter().position(|(name, _)| name == &receiver)
        }) else {
            return;
        };
        let local = match arguments.get(index) {
            Some(HirExpr { kind: HirExprKind::Identifier(name), .. })
                if self.bindings.contains_key(name) && !self.param_names.contains(name) =>
            {
                Some(self.binding(name))
            }
            _ => None,
        };
        self.graph
            .publish_call_args
            .entry(target.into())
            .or_default()
            .push((self.owner.into(), local));
    }

    fn value_allocates(&self, expr: &HirExpr) -> bool {
        match &expr.kind {
            HirExprKind::Identifier(name) => self.alloc_bindings.contains(name),
            _ => expression_allocates(expr),
        }
    }

    fn propagate_incoming_store(&mut self, callee: &str, arguments: &[HirExpr]) {
        if !self.graph.incoming_store_owners.contains(callee) {
            return;
        }
        let publishes = arguments.iter().any(|argument| {
            projection_root_and_path(argument)
                .map(|(root, _)| match &root.kind {
                    HirExprKind::Identifier(root_name) => {
                        self.param_names.contains(root_name.as_str())
                    }
                    _ => false,
                })
                .unwrap_or(false)
        });
        if publishes {
            self.graph.incoming_store_owners.insert(self.owner.into());
        }
    }

    fn free_member_target(&self, name: &str, arguments: &[HirExpr]) -> Option<String> {
        if self.graph.callables.contains_key(name) {
            return None;
        }
        self.method_target(arguments.first()?, name, arguments.len() - 1)
    }

    fn specialize(
        &mut self,
        generic: &HirCallable,
        arguments: &[HirExpr],
        args: &[Node],
    ) -> Node {
        let params: Vec<_> = generic.type_params.clone();
        let mut substitution = HashMap::new();
        for (declared, argument) in generic.params.iter().zip(arguments) {
            if let Some(concrete) = self.expression_type(argument) {
                crate::analysis::generic_specialization::unify_generic_binding(
                    &declared.ty,
                    &concrete,
                    &params,
                    &mut substitution,
                );
            }
        }
        let Some(concrete) = params
            .iter()
            .map(|param| substitution.get(param).cloned())
            .collect::<Option<Vec<_>>>()
        else {
            self.unsupported();
            return self.graph.node();
        };
        let name =
            crate::analysis::generic_specialization::mangle_specialization(&generic.name, &concrete);
        if !self.graph.callables.contains_key(&name) {
            let mut definition = generic.clone();
            definition.name = name.clone();
            definition.type_params.clear();
            for param in &mut definition.params {
                param.ty = crate::typechecker::substitute_type(&param.ty, &substitution);
            }
            substitute_body(&mut definition.body.statements, &substitution);
            if let Some(outputs) = &mut definition.outputs {
                substitute_output(outputs, &substitution);
            }
            if let Some(result) = &mut definition.result {
                *result = crate::typechecker::substitute_type(result, &substitution);
            }
            definition.output_list = definition
                .output_list
                .iter()
                .map(|ty| crate::typechecker::substitute_type(ty, &substitution))
                .collect();
            register_definition(self.graph, &definition);
            build_owner(self.graph, &name);
        }
        self.call_nodes(&name, args)
    }

    fn call_nodes(&mut self, name: &str, args: &[Node]) -> Node {
        if let Some(variant) = self.graph.constructor_key(name) {
            let root = self.graph.allocation(self.owner);
            for arg in args {
                self.graph.constraints.push(Constraint::FieldStore(
                    root,
                    format!("@variant:{variant}"),
                    *arg,
                ));
            }
            return root;
        }
        if let Some(callable) = self.graph.callables.get(name).cloned() {
            self.graph
                .callables
                .get_mut(self.owner)
                .unwrap()
                .calls
                .insert(name.into());
            for (arg, param) in args.iter().zip(&callable.params) {
                self.graph.copy(*arg, *param);
            }
            build_owner(self.graph, name);
            return callable.result;
        }
        self.intrinsic_or_unknown(name, args)
    }

    fn intrinsic_or_unknown(&mut self, name: &str, args: &[Node]) -> Node {
        let result = self.graph.unknown_value();
        self.intrinsic_helper_effects(name, args, result);
        match name {
            "Alloc#" | "Malloc#" => {
                if args.len() != 1 {
                    self.unsupported();
                }
                return self.graph.allocation(self.owner);
            }
            "Free#" => {}
            "Load#" => {
                if let Some(base) = args.first() {
                    self.load(*base, result);
                }
            }
            "Store#" | "Fill#" => {
                if args.len() >= 2 {
                    self.graph.constraints.push(Constraint::Store(args[0], args[1]));
                }
            }
            "Copy#" => {
                if args.len() >= 2 {
                    self.graph.constraints.push(Constraint::CopyMemory(args[1], args[0]));
                }
            }
            "SysCall#" => {
                if let Some(number) = args.first() {
                    self.graph
                        .constraints
                        .push(Constraint::Syscall(*number, args[1..].to_vec()));
                }
            }
            "Print#" => {
                for arg in args {
                    self.graph.constraints.push(Constraint::EscapeContents(*arg));
                }
            }
            "Environ#" => {
                self.graph.points[result].insert(result);
            }
            "StringConcat#" | "StringBitAnd#" | "StringBitOr#" | "StringBitXor#" => {
                return self.graph.allocation(self.owner);
            }
            _ => {
                if crate::intrinsic_signatures::get_intrinsic_signature(name)
                    .is_some_and(|signature| !signature.observable)
                {
                    for arg in args {
                        self.graph.copy(*arg, result);
                    }
                    let signature =
                        crate::intrinsic_signatures::get_intrinsic_signature(name).unwrap();
                    if !matches!(
                        signature.return_kind,
                        crate::intrinsic_signatures::ReturnKind::Native(_)
                            | crate::intrinsic_signatures::ReturnKind::Never
                    ) {
                        let allocation = self.graph.allocation(self.owner);
                        self.graph.copy(allocation, result);
                    }
                    return result;
                }
                if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() {
                    eprintln!("reclamation HIR unclassified call in {}: {name}", self.owner);
                }
                self.capture(args);
                self.unsupported();
            }
        }
        result
    }

    fn capture(&mut self, args: &[Node]) {
        let sink = self.graph.global("$external");
        for arg in args {
            self.graph.copy(*arg, sink);
        }
    }

    fn unsupported(&mut self) {
        self.graph.unsupported.insert(self.owner.into());
        let inputs: Vec<_> = self.bindings.values().copied().collect();
        self.capture(&inputs);
    }
}

fn projection_root_and_path(expr: &HirExpr) -> Option<(&HirExpr, String)> {
    let mut names: Vec<&str> = Vec::new();
    let mut current = expr;
    loop {
        match &current.kind {
            HirExprKind::Field { receiver, name } => {
                names.push(name.as_str());
                current = receiver.as_ref();
            }
            HirExprKind::Identifier(_) => {
                names.reverse();
                return Some((current, names.join(".")));
            }
            _ => return None,
        }
    }
}

fn expression_allocates(expr: &HirExpr) -> bool {
    matches!(
        expr.kind,
        HirExprKind::Call { .. }
            | HirExprKind::StructLiteral { .. }
            | HirExprKind::List(_)
            | HirExprKind::Tuple(_)
            | HirExprKind::Slice { .. }
            | HirExprKind::AddrOf(_)
            | HirExprKind::If { .. }
            | HirExprKind::Match { .. }
            | HirExprKind::Block(_)
    )
}

fn substitute_body(statements: &mut [HirStmt], substitution: &HashMap<String, Type>) {
    for statement in statements {
        substitute_statement(statement, substitution);
    }
}

fn substitute_statement(statement: &mut HirStmt, substitution: &HashMap<String, Type>) {
    match &mut statement.kind {
        HirStmtKind::Let { ty, expr, .. } => {
            if let Some(ty) = ty {
                *ty = crate::typechecker::substitute_type(ty, substitution);
            }
            if let Some(expr) = expr {
                substitute_expression(expr, substitution);
            }
        }
        HirStmtKind::Assign(left, right) => {
            substitute_expression(left, substitution);
            substitute_expression(right, substitution);
        }
        HirStmtKind::ArrowAssign { target, value, .. } => {
            if let Some(target) = target {
                substitute_expression(target, substitution);
            }
            substitute_expression(value, substitution);
        }
        HirStmtKind::Term(Some(expr))
        | HirStmtKind::EndProgram(Some(expr))
        | HirStmtKind::Rollback(Some(expr))
        | HirStmtKind::Tail(expr)
        | HirStmtKind::Check(expr)
        | HirStmtKind::Gate(expr)
        | HirStmtKind::Expression(expr) => substitute_expression(expr, substitution),
        HirStmtKind::Guarded(condition, body) => {
            substitute_expression(condition, substitution);
            substitute_body(body, substitution);
        }
        HirStmtKind::Block(body)
        | HirStmtKind::SyncBlock(body)
        | HirStmtKind::Defer(body)
        | HirStmtKind::Mutex(body)
        | HirStmtKind::Foreach { body, .. } => substitute_body(body, substitution),
        _ => {}
    }
}

fn substitute_expression(expression: &mut HirExpr, substitution: &HashMap<String, Type>) {
    match &mut expression.kind {
        HirExprKind::Cast { inner, to, .. } | HirExprKind::IsType { inner, ty: to } => {
            substitute_expression(inner, substitution);
            *to = crate::typechecker::substitute_type(to, substitution);
            return;
        }
        _ => {}
    }
    substitute_expression_children(expression, substitution);
}

fn substitute_expression_children(expression: &mut HirExpr, substitution: &HashMap<String, Type>) {
    match &mut expression.kind {
        HirExprKind::Call { args, .. } | HirExprKind::Tuple(args) | HirExprKind::List(args) => {
            for arg in args {
                substitute_expression(arg, substitution);
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            substitute_expression(left, substitution);
            substitute_expression(right, substitution);
        }
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => substitute_expression(inner, substitution),
        HirExprKind::Field { receiver, .. } | HirExprKind::Reflect { receiver, .. } => {
            substitute_expression(receiver, substitution);
        }
        HirExprKind::MethodCall { receiver, args, .. } => {
            substitute_expression(receiver, substitution);
            for arg in args {
                substitute_expression(arg, substitution);
            }
        }
        HirExprKind::Index(left, right) => {
            substitute_expression(left, substitution);
            substitute_expression(right, substitution);
        }
        HirExprKind::Slice { array, start, end, stride } => {
            substitute_expression(array, substitution);
            for part in [start, end, stride].into_iter().flatten() {
                substitute_expression(part, substitution);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            substitute_expression(start, substitution);
            substitute_expression(end, substitution);
        }
        HirExprKind::Block(body) => substitute_body(body, substitution),
        HirExprKind::If { condition, then_branch, else_branch } => {
            substitute_expression(condition, substitution);
            substitute_expression(then_branch, substitution);
            if let Some(else_branch) = else_branch {
                substitute_expression(else_branch, substitution);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            substitute_expression(scrutinee, substitution);
            for arm in arms {
                if let HirPatternKind::TypedBinding(_, ty) = &mut arm.pattern.kind {
                    *ty = crate::typechecker::substitute_type(ty, substitution);
                }
                if let Some(guard) = &mut arm.guard {
                    substitute_expression(guard, substitution);
                }
                substitute_expression(&mut arm.body, substitution);
            }
        }
        HirExprKind::StructLiteral { fields, specs, .. } => {
            for (_, value) in fields.iter_mut().chain(specs.iter_mut()) {
                substitute_expression(value, substitution);
            }
        }
        HirExprKind::Lambda { body, .. } => substitute_expression(body, substitution),
        HirExprKind::Spawn { args, .. } => {
            for arg in args {
                substitute_expression(arg, substitution);
            }
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            if let Some(receiver) = receiver {
                substitute_expression(receiver, substitution);
            }
            for arg in args {
                substitute_expression(arg, substitution);
            }
        }
        _ => {}
    }
}

fn substitute_output(output: &mut HirOutput, substitution: &HashMap<String, Type>) {
    match output {
        HirOutput::Single(ty) => *ty = crate::typechecker::substitute_type(ty, substitution),
        HirOutput::Union(items) | HirOutput::Tuple(items) => {
            for item in items {
                substitute_output(item, substitution);
            }
        }
        HirOutput::Array(inner) | HirOutput::Named(_, inner) => {
            substitute_output(inner, substitution)
        }
    }
}
