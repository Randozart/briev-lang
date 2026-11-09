//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Constraint construction for region escape proofs. Unsupported forms capture
//! their inputs instead of being interpreted as evidence for reclamation.
use super::*;
use crate::ast::{BinaryOpKind, Pattern, UnaryOpKind};
mod structured;
mod traits;
#[cfg(test)]
mod tuple_fields_tests;

pub(super) fn register(graph: &mut Graph, items: &[TopLevel], live: &std::collections::HashSet<String>) {
    for item in items { register_item(graph, item, live); }
    traits::register_defaults(graph, items);
}

pub(super) fn closed_initializers(items: &[TopLevel]) -> bool {
    items.iter().all(closed_initializer)
}

fn closed_initializer(item: &TopLevel) -> bool {
    match item {
        TopLevel::Statement(statement) => match statement.as_ref() {
            Statement::Let { expr, .. } => expr.as_ref().is_none_or(|expression|
                matches!(expression, Expr::Decimal(_) | Expr::Bool(_) | Expr::Float(_) | Expr::Char(_))),
            _ => false,
        },
        TopLevel::StateDecl(_) => false,
        TopLevel::Export(export) => closed_initializer(&export.inner),
        TopLevel::SyncGroup { item, .. } | TopLevel::Fuzzed { item, .. } => closed_initializer(item),
        _ => true,
    }
}

fn register_item(graph: &mut Graph, item: &TopLevel, live: &std::collections::HashSet<String>) {
    match item {
        // Implicit casts can call source definitions absent from direct-call liveness.
        // Register available bodies; build_owner still expands only reached callees.
        TopLevel::Definition(definition) => register_definition(graph, definition),
        TopLevel::Transaction(transaction) => register_transaction(graph, transaction),
        TopLevel::Export(export) => register_item(graph, &export.inner, live),
        TopLevel::SyncGroup { item, .. } | TopLevel::Fuzzed { item, .. } => register_item(graph, item, live),
        TopLevel::Enum(definition) => register_enum(graph, definition),
        TopLevel::TypeDef(definition) => register_typedef(graph, definition),
        TopLevel::Constant(constant) => {
            let node = graph.global(&constant.name);
            graph.global_types.insert(constant.name.clone(), constant.ty.clone());
            if let Expr::Decimal(value) = &constant.expr {
                // A literal constant is a value, not the address of its global storage.
                graph.points[node].clear();
                graph.constants[node].insert(*value); graph.inexact_constants.remove(&node);
            }
        }
        TopLevel::StateDecl(state) => { graph.global(&state.name); graph.global_types.insert(state.name.clone(), state.ty.clone()); }
        TopLevel::Statement(statement) => register_global(graph, statement),
        _ => {}
    }
}

fn register_definition(graph: &mut Graph, definition: &crate::ast::Definition) {
    if !definition.type_params.is_empty() {
        graph.generics.insert(definition.name.clone(), definition.clone()); return;
    }
    register_callable(graph, &definition.name, definition.parameters.len());
    graph.bodies.insert(definition.name.clone(), (definition.parameters.clone(), definition.body.clone()));
    register_result(graph, &definition.name, definition.output_type.as_ref(), false);
}

fn register_result(graph: &mut Graph, name: &str, output: Option<&crate::ast::OutputType>, void_default: bool) {
    let ty = output.and_then(output_value_type);
    let scalar = match output {
        Some(crate::ast::OutputType::Single(ty)) => scalar_result(graph, ty),
        None => void_default,
        _ => false,
    };
    if let Some(ty) = ty { graph.return_types.insert(name.into(), ty); }
    if scalar { return; }
    // Destination representation cannot prove allocation absence. Projection
    // and ABI materialization may create a new image even for borrowed input.
    // Model that possible allocation separately; only actual malloc results
    // enter runtime records. Retire with explicit materialization provenance.
    graph.handle_returns.insert(name.into());
    let image = graph.allocation(name);
    let result = graph.callables[name].result;
    graph.copy(image, result);
}

// 2026-11-05: product results retain their shape for inferred bindings and
// projections. Flattening to the first leaf blocks otherwise closed regions.
// Retire with a shared semantic output-type resolver.
fn output_value_type(output: &crate::ast::OutputType) -> Option<crate::ast::Type> {
    use crate::ast::{OutputType, Type};
    match output {
        OutputType::Tuple(items) => items.iter().map(output_value_type).collect::<Option<Vec<_>>>().map(Type::Tuple),
        OutputType::Named(_, inner) => output_value_type(inner),
        _ => output.all_types().into_iter().next(),
    }
}

fn scalar_result(graph: &Graph, ty: &crate::ast::Type) -> bool {
    let fields = ty.universe_key().and_then(|key| graph.universe.get(key)).is_some_and(|resolved| !resolved.fields.is_empty());
    if fields { return false; }
    crate::type_universe::protocol_category(&graph.universe, ty)
        .is_some_and(|category| matches!(category.as_str(), "Int" | "UInt" | "Bool" | "Float" | "Char" | "Bit"))
}

fn register_transaction(graph: &mut Graph, transaction: &crate::ast::Transaction) {
    register_callable(graph, &transaction.name, transaction.parameters.len());
    let mut body = transaction.body.clone();
    // Reactor conditions execute in the driver too; their effects must be in
    // the entry proof rather than hidden outside the analyzed source body.
    body.push(Statement::Expression(transaction.contract.pre_condition.clone()));
    body.push(Statement::Expression(transaction.contract.post_condition.clone()));
    graph.bodies.insert(transaction.name.clone(), (transaction.parameters.clone(), body));
    register_result(graph, &transaction.name, transaction.output_type.as_ref(), transaction.is_reactive);
    if transaction.is_reactive { graph.entry_nodes.insert(transaction.name.clone()); }
    if transaction.is_async { graph.unsupported.insert(transaction.name.clone()); }
}

fn register_enum(graph: &mut Graph, definition: &crate::ast::EnumDefinition) {
    for variant in &definition.variants {
        let name = match variant {
            crate::ast::EnumVariant::Unit(name) | crate::ast::EnumVariant::Tuple(name, _) | crate::ast::EnumVariant::Struct(name, _) => name,
        };
        graph.constructors.insert(name.clone());
        structured::register_variant(graph, definition, variant);
    }
}

fn register_typedef(graph: &mut Graph, definition: &crate::ast::TypeDef) {
    register_methods(graph, definition);
    if definition.body.op_bindings.iter().any(|binding| !matches!(&binding.expr,
        Expr::Call(name, _, _) if crate::intrinsic_signatures::get_intrinsic_signature(name).is_some())) {
        graph.user_operators.insert(definition.name.clone());
    }
    for slot in &definition.body.slots {
        if let Some(name) = slot.name.strip_prefix("__variant_") {
            graph.constructors.insert(name.into());
            structured::register_slot(graph, definition, name, &slot.ty);
        }
    }
}

// 2026-11-05: concrete explicit-receiver methods participate in the same call
// graph as free functions. Unsupported members retain a shadowing entry so
// they can never accidentally dispatch to a same-named free function.
fn register_methods(graph: &mut Graph, definition: &crate::ast::TypeDef) {
    for member in &definition.body.members {
        if let TopLevel::Transaction(method) = member {
            graph.methods.insert((definition.name.clone(), method.name.clone()), None);
        }
        let TopLevel::Definition(method) = member else { continue };
        let key = (definition.name.clone(), method.name.clone());
        graph.methods.insert(key.clone(), None);
        if !definition.type_params.is_empty() || !method.type_params.is_empty() { continue; }
        if uses_implicit_receiver(definition, member) { continue; }
        let Some((_, receiver)) = method.parameters.first() else { continue };
        if receiver.universe_key() != Some(definition.name.as_str()) { continue; }
        let mut callable = method.clone();
        callable.name = format!("{}::{}", definition.name, method.name);
        register_definition(graph, &callable);
        graph.methods.insert(key, Some(callable.name));
    }
}

fn uses_implicit_receiver(definition: &crate::ast::TypeDef, member: &TopLevel) -> bool {
    let mut implicit = false;
    crate::ast::visit::walk_toplevel(member, &mut |expression| {
        if let Expr::Identifier(name) = expression {
            implicit |= name == "self" || definition.body.slots.iter().any(|slot| slot.name == *name);
        }
    });
    implicit
}
fn register_callable(graph: &mut Graph, name: &str, count: usize) {
    let params = (0..count).map(|_| graph.root()).collect();
    let result = graph.node();
    graph.callables.insert(name.into(), Callable { params, result, calls: BTreeSet::new() });
}

fn register_global(graph: &mut Graph, statement: &Statement) {
    if let Statement::Let { name, ty, .. } = statement {
        graph.global(name);
        if let Some(ty) = ty { graph.global_types.insert(name.clone(), ty.clone()); }
    }
}

pub(super) fn build_owner(graph: &mut Graph, owner: &str) {
    if !graph.built.insert(owner.into()) { return; }
    if let Some((params, statements)) = graph.bodies.get(owner).cloned() { body(graph, owner, &params, &statements); }
}

fn body(graph: &mut Graph, owner: &str, params: &[(String, crate::ast::Type)], statements: &[Statement]) {
    let Some(callable) = graph.callables.get(owner).cloned() else { return };
    let bindings = params.iter().zip(callable.params).map(|((name, _), node)| (name.clone(), node)).collect();
    let types = params.iter().cloned().collect();
    let param_names = params.iter().map(|(name, _)| name.clone()).collect();
    let mut builder = Builder { graph, owner, bindings, types, result: callable.result, param_names, alloc_bindings: BTreeSet::new() };
    builder.statements(statements);
}

struct Builder<'a> {
    graph: &'a mut Graph,
    owner: &'a str,
    bindings: BTreeMap<String, Node>,
    types: BTreeMap<String, crate::ast::Type>,
    result: Node,
    param_names: BTreeSet<String>,
    /// 2026-11-08: locals bound to storage-producing values. The syntactic
    /// `expression_allocates` check missed `t.key = key` where `key` is a local
    /// holding a call result, so the store was not treated as publishing and
    /// the callable stayed a reclaimable region (dangling handle). Track
    /// allocation provenance across binds and copies to close that gap.
    alloc_bindings: BTreeSet<String>,
}

impl Builder<'_> {
    fn binding(&mut self, name: &str) -> Node {
        if let Some(node) = self.bindings.get(name) { return *node; }
        self.graph.global(name)
    }

    fn local(&mut self, name: &str) -> Node {
        if let Some(node) = self.bindings.get(name) { return *node; }
        let node = self.graph.node();
        self.bindings.insert(name.into(), node);
        node
    }

    fn statements(&mut self, statements: &[Statement]) {
        for statement in statements { self.statement(statement); }
    }

    fn statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Let { name, names, expr, ty, .. } => {
                self.statement_let(name, names, ty.as_ref(), expr.as_ref())
            }
            Statement::Assign(target, value) => self.statement_assign(target, value),
            Statement::Term(Some(value)) | Statement::Tail(value) => {
                let value = self.expression(value); self.graph.copy(value, self.result);
            }
            Statement::Term(None) => {}
            Statement::Expression(value) | Statement::Check(value) | Statement::Gate(value) => { self.expression(value); }
            Statement::Guarded(condition, body) => { self.expression(condition); self.statements(body); }
            Statement::Block(body) | Statement::SyncBlock(body) | Statement::Mutex(body) | Statement::Defer(body) => self.statements(body),
            Statement::Foreach { item, list, body , ..} => {
                if matches!(list.as_ref(), Expr::Range { .. }) { self.types.insert(item.clone(), crate::ast::Type::int()); }
                let source = self.expression(list);
                let target = self.local(item);
                self.load(source, target);
                self.statements(body);
            }
            Statement::Match { expr, arms } => self.statement_match(expr, arms),
            Statement::ArrowAssign { target, value, .. } => {
                let value = self.expression(value);
                if let Some(target) = target {
                    let target = self.expression(target);
                    self.graph.constraints.push(Constraint::Store(target, value));
                }
            }
            Statement::EndProgram(value) | Statement::Rollback(value) => { if let Some(value) = value { self.expression(value); } }
            Statement::KeepHint(name) => { let value = self.binding(name); self.capture(&[value]); }
            Statement::Break | Statement::Trap | Statement::Halt
            | Statement::MetadataAssignment(..) | Statement::FreeHint(_) => {}
            _ => self.unsupported(),
        }
    }

    fn statement_match(&mut self, expression: &Expr, arms: &[crate::ast::StmtMatchArm]) {
        let value = self.expression(expression);
        let ty = self.expression_type(expression);
        for arm in arms {
            self.statement_pattern_types(arm, ty.as_ref());
            self.statement_arm(value, arm);
        }
    }
    fn statement_pattern_types(&mut self, arm: &crate::ast::StmtMatchArm, ty: Option<&crate::ast::Type>) {
        for pattern in &arm.patterns { self.pattern_types(pattern, ty); }
    }
    fn bind_types(&mut self, name: &str, names: &[String], declared: Option<&crate::ast::Type>, expr: Option<&Expr>) {
        let Some(ty) = declared.cloned().or_else(|| expr.and_then(|expr| self.expression_type(expr))) else { return };
        self.types.insert(name.into(), ty.clone());
        if names.len() <= 1 { return; }
        if let crate::ast::Type::Tuple(elements) = ty {
            self.types.extend(names.iter().cloned().zip(elements));
        }
    }

    fn statement_let(&mut self, name: &str, names: &[String], ty: Option<&crate::ast::Type>, expr: Option<&Expr>) {
        self.bind_types(name, names, ty, expr);
        let target = self.local(name);
        let Some(expr) = expr else { return };
        if self.value_allocates(expr) { self.alloc_bindings.insert(name.into()); }
        let value = self.expression(expr);
        if names.len() <= 1 { self.graph.copy(value, target); return; }
        // 2026-11-06 tuple provenance (#75): index-labelled fields, not a raw
        // memory load, so unselected tuple contents do not publish.
        self.destructure_names(value, names);
    }

    fn statement_assign(&mut self, target: &Expr, value_expr: &Expr) {
        let allocating = self.value_allocates(value_expr);
        let value = self.expression(value_expr);
        if allocating {
            if let Expr::Identifier(name) = target { self.alloc_bindings.insert(name.clone()); }
        }
        self.assign(target, value, allocating);
    }

    fn statement_arm(&mut self, value: Node, arm: &crate::ast::StmtMatchArm) {
        for pattern in &arm.patterns { self.pattern(pattern, value); }
        self.statements(&arm.body);
    }

    fn assign(&mut self, target: &Expr, value: Node, allocating: bool) {
        if let Expr::Identifier(name) = target {
            let target = self.binding(name);
            self.graph.copy(value, target);
            return;
        }
        if self.assign_field(target, value, allocating) { return; }
        let base = match target {
            Expr::Field(base, _) | Expr::Index(base, _) | Expr::Deref(base) => self.expression(base),
            _ => { self.unsupported(); return; }
        };
        self.graph.constraints.push(Constraint::Store(base, value));
    }

    /// 2026-11-07/08: a field store is attributed to the ROOT receiver so a
    /// store through more than one projection (`p.mid.inner = …`) is seen
    /// exactly like `p.inner = …`. Resolving the intermediate `p.mid` through
    /// `expression` yields a content-less field-load node, so the store landed
    /// nowhere and the allocation was wrongly licensed as a reclaimable region
    /// whose handle dangled after teardown. Peeling to the root keeps the
    /// allocation reachable from the incoming parameter. Returns whether the
    /// target was a field store (handled here).
    fn assign_field(&mut self, target: &Expr, value: Node, allocating: bool) -> bool {
        let Expr::Field(_, _) = target else { return false; };
        if let Some((root, path)) = projection_root_and_path(target) {
            self.record_incoming_store(allocating, root);
            let base = self.expression(root);
            self.graph.constraints.push(Constraint::FieldStore(base, path, value));
            return true;
        }
        if let Expr::Field(base, field) = target {
            let base = self.expression(base);
            self.graph.constraints.push(Constraint::FieldStore(base, field.clone(), value));
        }
        true
    }

    /// 2026-11-07: a store into a projection of an INCOMING receiver publishes
    /// storage into caller-owned memory. Record the owner so the plan marks it
    /// a barrier: an INHERIT lowering would leave the caller's region active
    /// and reclaim the storage on caller exit, dangling a handle the caller
    /// keeps.
    fn record_incoming_store(&mut self, allocating: bool, root: &Expr) {
        if !allocating { return; }
        if let Expr::Identifier(root_name) = root {
            if self.param_names.contains(root_name.as_str()) {
                self.graph.incoming_store_owners.insert(self.owner.into());
            }
        }
    }

    fn load(&mut self, base: Node, value: Node) { self.graph.constraints.push(Constraint::Load(base, value)); }

    // 2026-11-06: tuple positions retain independent pointer provenance, as
    // object fields do. Keep raw-memory fallback in FieldLoad; retire only
    // when a shared aggregate-layout analysis supplies equivalent identities.
    fn destructure_names(&mut self, value: Node, names: &[String]) {
        for (index, name) in names.iter().enumerate() {
            let target = self.local(name);
            self.graph.constraints.push(Constraint::FieldLoad(value, index.to_string(), target));
        }
    }

    fn pattern(&mut self, pattern: &Pattern, value: Node) {
        match pattern {
            Pattern::Binding(name) | Pattern::TypedBinding(name, _) => {
                let target = self.local(name);
                self.graph.copy(value, target);
            }
            Pattern::EnumVariant(variant, fields) => {
                let payload = self.graph.node();
                let variant = self.graph.constructor_key(variant).unwrap_or_else(|| variant.clone());
                self.graph.constraints.push(Constraint::FieldLoad(value, format!("@variant:{variant}"), payload));
                for field in fields { self.pattern(field, payload); }
            }
            Pattern::Tuple(fields) => {
                for (index, field) in fields.iter().enumerate() {
                    let payload = self.graph.node();
                    self.graph.constraints.push(Constraint::FieldLoad(value, index.to_string(), payload));
                    self.pattern(field, payload);
                }
            }
            Pattern::Multi(patterns) => { for pattern in patterns { self.pattern(pattern, value); } }
            Pattern::Literal(expression) => { self.expression(expression); }
            Pattern::Range(start, end) | Pattern::RangeInclusive(start, end) => { self.expression(start); self.expression(end); }
            _ => {}
        }
    }

    fn tuple(&mut self, values: &[Expr]) -> Node {
        let root = self.graph.allocation(self.owner);
        for (index, expression) in values.iter().enumerate() {
            let value = self.expression(expression);
            self.graph.constraints.push(Constraint::FieldStore(root, index.to_string(), value));
        }
        root
    }

    fn expression(&mut self, expression: &Expr) -> Node {
        match expression {
            Expr::Identifier(name) => self.binding(name),
            Expr::Decimal(value) => {
                let node = self.graph.node(); self.graph.constants[node].insert(*value); node
            }
            Expr::Quoted(_) => self.graph.root(),
            Expr::Call(name, args, _) => self.call(name, args),
            Expr::StructLiteral { fields, specs, .. } => {
                let root = self.graph.allocation(self.owner);
                for (field, value) in fields.iter().chain(specs) {
                    let value = self.expression(value);
                    self.graph.constraints.push(Constraint::FieldStore(root, field.clone(), value));
                }
                root
            }
            Expr::List(values) => {
                let root = self.graph.allocation(self.owner);
                for value in values { self.store_expression(root, value); }
                root
            }
            Expr::Tuple(values) => self.tuple(values),
            // 2026-11-05: slices may allocate a copy or retain a view. Track
            // both identities; a scalar consumer can reclaim local copies,
            // while an escaping slice keeps its backing storage reachable.
            Expr::Slice { array, start, end, stride } => {
                let base = self.indexed_base(array);
                for bound in [start, end, stride].into_iter().flatten() { self.expression(bound); }
                let result = self.graph.allocation(self.owner);
                self.graph.copy(base, result);
                self.graph.constraints.push(Constraint::CopyMemory(base, result));
                result
            }
            Expr::Index(base, index) => {
                self.expression(index);
                let base = self.indexed_base(base); let result = self.graph.node(); self.load(base, result); result
            }
            Expr::Field(base, field) => {
                let base = self.expression(base);
                let result = if self.graph.constructor_key(field).is_some() { self.graph.allocation(self.owner) } else { self.graph.node() };
                self.graph.constraints.push(Constraint::FieldLoad(base, field.clone(), result)); result
            }
            Expr::Deref(base) => {
                let base = self.expression(base); let result = self.graph.node(); self.load(base, result); result
            }
            Expr::Cast(value, target) => self.cast(value, target),
            Expr::MethodCall(receiver, name, args, None) => {
                // 2026-11-05 union (#66 + #68): a NON-OBSERVABLE intrinsic
                // method carries intrinsic effects (#68); every other
                // concrete method gets call-graph edges (#66).
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
            Expr::Consume(value) => self.expression(value),
            Expr::AddrOf(value) => {
                let value = self.expression(value); let result = self.graph.allocation(self.owner);
                self.graph.constraints.push(Constraint::Store(result, value)); result
            }
            Expr::IsType(value, _) => { self.expression(value); self.graph.node() }
            Expr::UnaryOp(operator, value) => self.unary(*operator, value),
            Expr::BinaryOp(operator, lhs, rhs) => self.binary(*operator, lhs, rhs),
            Expr::Reflect(value, name, _) => {
                let value = self.expression(value);
                if matches!(name.as_str(), "Length" | "Size" | "Count" | "Bytes" | "Bits" | "Capacity" | "Alignment") { return self.graph.unknown_value(); }
                if !matches!(name.as_str(), "Ptr" | "Data" | "Address") { self.unsupported(); self.capture(&[value]); }
                value
            }
            Expr::If(condition, yes, no) => {
                self.expression(condition); let result = self.graph.unknown_value();
                let yes = self.expression(yes); self.graph.copy(yes, result);
                if let Some(no) = no { let value = self.expression(no); self.graph.copy(value, result); }
                result
            }
            Expr::Match(value, arms) => {
                let ty = self.expression_type(value);
                let value = self.expression(value); let result = self.graph.node();
                for arm in arms { self.pattern_types(&arm.pattern, ty.as_ref()); self.expression_arm(value, result, arm); }
                result
            }
            Expr::Block(statements) => self.block(statements),
            Expr::Range { start, end, .. } => { self.expression(start); self.expression(end); self.graph.node() }
            Expr::Bool(_) | Expr::Char(_) | Expr::Float(_) | Expr::BeginProgram
            | Expr::Wildcard | Expr::FormattingAnnotation(_) | Expr::UnitLiteral { .. } | Expr::Exists(_) => self.graph.node(),
            _ => {
                if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() { eprintln!("reclamation unsupported expression in {}: {expression:?}", self.owner); }
                self.unsupported(); self.graph.root()
            }
        }
    }

    // 2026-11-05: richer index types must not license unclassified user-op
    // effects. Keep the barrier until those operation bodies join the graph.
    fn indexed_base(&mut self, expression: &Expr) -> Node {
        let value = self.expression(expression);
        if self.expression_type(expression).is_none_or(|ty| self.has_user_operators(&ty)) {
            self.unsupported(); self.capture(&[value]);
        }
        value
    }
    fn store_expression(&mut self, target: Node, value: &Expr) {
        let value = self.expression(value);
        self.graph.constraints.push(Constraint::Store(target, value));
    }

    fn expression_arm(&mut self, value: Node, result: Node, arm: &crate::ast::MatchArm) {
        self.pattern(&arm.pattern, value);
        if let Some(guard) = &arm.guard { self.expression(guard); }
        let value = self.expression(&arm.body);
        self.graph.copy(value, result);
    }

    fn block(&mut self, statements: &[Statement]) -> Node {
        let result = self.graph.node();
        let Some((last, prefix)) = statements.split_last() else { return result };
        self.statements(prefix);
        match last {
            Statement::Expression(value) | Statement::Tail(value) => {
                let value = self.expression(value); self.graph.copy(value, result);
            }
            statement => self.statement(statement),
        }
        result
    }

    fn binary(&mut self, operator: BinaryOpKind, lhs: &Expr, rhs: &Expr) -> Node {
        let left = self.expression(lhs); let right = self.expression(rhs);
        let result = self.binary_value(operator, left, right);
        if self.expression_type(lhs).is_none() || self.expression_type(rhs).is_none() {
            self.unsupported(); self.capture(&[left, right]);
        }
        if self.expression_type(lhs).is_some_and(|ty| self.has_user_operators(&ty)) {
            self.unsupported(); self.capture(&[left, right]);
        }
        if matches!(operator, BinaryOpKind::Eq | BinaryOpKind::Neq | BinaryOpKind::Lt | BinaryOpKind::Gt
            | BinaryOpKind::Le | BinaryOpKind::Ge | BinaryOpKind::And | BinaryOpKind::Or) { return result; }
        if operator == BinaryOpKind::Concat || self.allocating_operator(lhs, operator) || self.allocating_operator(rhs, operator) {
            let allocation = self.graph.allocation(self.owner); self.graph.copy(allocation, result);
        }
        result
    }

    fn binary_value(&mut self, operator: BinaryOpKind, left: Node, right: Node) -> Node {
        if matches!(operator, BinaryOpKind::BitAnd | BinaryOpKind::BitOr | BinaryOpKind::BitXor) {
            let result = self.graph.node();
            self.graph.constraints.push(Constraint::Bitwise(operator, left, right, result));
            return result;
        }
        let result = self.graph.unknown_value();
        if matches!(operator, BinaryOpKind::Eq | BinaryOpKind::Neq | BinaryOpKind::Lt | BinaryOpKind::Gt
            | BinaryOpKind::Le | BinaryOpKind::Ge | BinaryOpKind::And | BinaryOpKind::Or) { return result; }
        self.graph.copy(left, result); self.graph.copy(right, result);
        result
    }

    fn unary(&mut self, operator: UnaryOpKind, expression: &Expr) -> Node {
        let value = self.expression(expression);
        if self.expression_type(expression).is_none_or(|ty| self.has_user_operators(&ty)) {
            self.unsupported(); self.capture(&[value]);
        }
        if operator == UnaryOpKind::Not { return self.graph.node(); }
        if operator == UnaryOpKind::BitNot && self.allocating_operator(expression, BinaryOpKind::BitXor) {
            let fresh = self.graph.allocation(self.owner);
            self.graph.copy(value, fresh); return fresh;
        }
        let result = self.graph.unknown_value(); self.graph.copy(value, result); result
    }

    fn has_user_operators(&self, ty: &crate::ast::Type) -> bool {
        let Some(mut key) = ty.universe_key() else { return false };
        let mut visited = BTreeSet::new();
        while visited.insert(key.to_string()) {
            if self.graph.user_operators.contains(key) { return true; }
            let Some(resolved) = self.graph.universe.get(key) else { break };
            key = &resolved.base;
        }
        false
    }

    fn expression_type(&self, expression: &Expr) -> Option<crate::ast::Type> {
        match expression {
            Expr::Identifier(name) => self.types.get(name).or_else(|| self.graph.global_types.get(name)).cloned(),
            Expr::Quoted(_) => Some(crate::ast::Type::string()),
            Expr::Decimal(_) => Some(crate::ast::Type::int()),
            Expr::TaggedLiteral(..) => Some(crate::ast::Type::int()),
            Expr::Char(_) => Some(crate::ast::Type::char_()),
            Expr::Float(_) => Some(crate::ast::Type::float()),
            Expr::Bool(_) => Some(crate::ast::Type::bool_()),
            Expr::Tuple(values) => values.iter().map(|value| self.expression_type(value))
                .collect::<Option<Vec<_>>>().map(crate::ast::Type::Tuple),
            Expr::Cast(_, target) => Some(target.clone()),
            Expr::MethodCall(receiver, name, args, None) => {
                // 2026-11-05 union (#66 + #68): intrinsic return kinds win;
                // concrete methods resolve through the call graph.
                let intrinsic = crate::intrinsic_signatures::get_intrinsic_signature(name)
                    .and_then(|signature| match signature.return_kind {
                        crate::intrinsic_signatures::ReturnKind::Native(category) => {
                            Some(Some(crate::ast::Type::Custom(category.into())))
                        }
                        crate::intrinsic_signatures::ReturnKind::Exact(ty) => Some(Some(ty)),
                        _ => None,
                    });
                match intrinsic {
                    Some(ty) => ty,
                    None => {
                        let target = self.method_target(receiver, name, args.len())?;
                        let mut arguments = vec![(**receiver).clone()]; arguments.extend_from_slice(args);
                        self.graph.return_types.get(&target).cloned().or_else(|| self.generic_result(&target, &arguments))
                    }
                }
            }
            Expr::Call(name, arguments, _) => self.graph.return_types.get(name).cloned().or_else(|| self.generic_result(name, arguments)).or_else(|| {
                let target = self.free_member_target(name, arguments)?;
                self.graph.return_types.get(&target).cloned()
            }).or_else(|| {
                use crate::intrinsic_signatures::ReturnKind;
                match crate::intrinsic_signatures::get_intrinsic_signature(name)?.return_kind {
                    ReturnKind::Exact(ty) => Some(ty), ReturnKind::Native(category) => Some(crate::ast::Type::Custom(category.into())), _ => None,
                }
            }),
            Expr::UnaryOp(_, inner) | Expr::Consume(inner) => self.expression_type(inner),
            Expr::Reflect(..) => Some(crate::ast::Type::int()),
            Expr::StructLiteral { type_name, .. } => Some(crate::ast::Type::Custom(type_name.clone())),
            Expr::Index(base, index) => {
                if let (Expr::Tuple(values), Expr::Decimal(index)) = (base.as_ref(), index.as_ref()) {
                    return usize::try_from(*index).ok().and_then(|index| values.get(index)).and_then(|value| self.expression_type(value));
                }
                let mut context = crate::typechecker::TypecheckContext::new(&self.graph.universe);
                context.bindings.insert("base".into(), self.expression_type(base)?);
                context.bindings.insert("index".into(), self.expression_type(index)?);
                let expression = Expr::Index(Box::new(Expr::Identifier("base".into())), Box::new(Expr::Identifier("index".into())));
                crate::typechecker::infer_type_only(&expression, &mut context).ok()
            }
            Expr::Slice { array, .. } => self.expression_type(array),
            Expr::BinaryOp(_, lhs, _) => self.expression_type(lhs),
            Expr::Field(base, field) => {
                let ty = self.expression_type(base)?;
                if let crate::ast::Type::Tuple(elements) = &ty {
                    return field.parse::<usize>().ok().and_then(|index| elements.get(index)).cloned();
                }
                let resolved = self.graph.universe.get(ty.universe_key()?)?;
                let field_type = &resolved.fields.iter().find(|(name, _)| name == field)?.1;
                let args = match &ty { crate::ast::Type::Applied(_, args) | crate::ast::Type::Generic(_, args) => args.as_slice(), _ => &[] };
                let substitution = resolved.type_params.iter().cloned().zip(args.iter().cloned()).collect();
                Some(crate::typechecker::substitute_type(field_type, &substitution))
            }
            _ => None,
        }
    }

    fn generic_result(&self, name: &str, arguments: &[Expr]) -> Option<crate::ast::Type> {
        let generic = self.graph.generics.get(name)?;
        let params: Vec<_> = generic.type_params.iter().map(|param| param.name.clone()).collect();
        let mut substitution = std::collections::HashMap::new();
        for ((_, declared), argument) in generic.parameters.iter().zip(arguments) {
            if let Some(concrete) = self.expression_type(argument) {
                crate::analysis::generic_specialization::unify_generic_binding(declared, &concrete, &params, &mut substitution);
            }
        }
        let output = output_value_type(generic.output_type.as_ref()?)?;
        Some(crate::typechecker::substitute_type(&output, &substitution))
    }

    fn allocating_operator(&self, value: &Expr, operator: BinaryOpKind) -> bool {
        let Some(ty) = self.expression_type(value) else { return false };
        let rune = match operator { BinaryOpKind::Add => "+", BinaryOpKind::BitAnd => "&", BinaryOpKind::BitOr => "|", BinaryOpKind::BitXor => "^", _ => return false };
        matches!(crate::type_universe::get_operator_intrinsic(&self.graph.universe, rune, &ty),
            Some(crate::ast::OpBinding::Intrinsic(name)) if matches!(name.as_str(), "StringConcat#" | "StringBitAnd#" | "StringBitOr#" | "StringBitXor#"))
    }

    fn cast(&mut self, expression: &Expr, target: &crate::ast::Type) -> Node {
        let original = self.expression(expression);
        let mut value = self.graph.unknown_value(); self.graph.copy(original, value);
        let Some(source) = self.expression_type(expression) else {
            if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() { eprintln!("reclamation unknown cast source in {}: {expression:?}", self.owner); }
            self.unsupported(); return value
        };
        if self.has_user_operators(&source) || self.has_user_operators(target) { self.unsupported(); return value; }
        let encoding_target = target.universe_key().map(str::to_owned);
        let address_cast = matches!(&source, crate::ast::Type::Ptr(_)) || matches!(target, crate::ast::Type::Ptr(_));
        let (source, source_variant) = self.graph.casts.type_to_protocol(&self.graph.universe, &source);
        let (target, target_variant) = self.graph.casts.type_to_protocol(&self.graph.universe, target);
        let Some(steps) = self.graph.casts.find_path(&source, &source_variant, &target, &target_variant) else {
            // LLVM lowers a raw address cast even without a registered lane.
            // Preserve its alias identity, and reject other unclassified casts.
            if !address_cast { self.unsupported(); }
            return value;
        };
        for step in steps { value = self.cast_lane(&step.lane, value, encoding_target.as_deref()); }
        value
    }

    fn cast_lane(&mut self, lane: &crate::casting::graph::LaneKind, value: Node, encoding_target: Option<&str>) -> Node {
        use crate::casting::graph::LaneKind;
        match lane {
            LaneKind::ExtCall(name) => self.call_nodes(name, &[value]),
            LaneKind::ExtCallDyn(name) => self.call_nodes(name, &[value]),
            LaneKind::Chain(first, second) => { let value = self.cast_lane(first, value, encoding_target); self.cast_lane(second, value, encoding_target) }
            LaneKind::CastFromBitCallback => {
                let Some(target) = encoding_target else { self.unsupported(); return value };
                // The native encoding provider copies the input bytes into fresh storage.
                // The consumer must check the resolved override registry before using this fact.
                self.graph.default_encoding_targets.insert(target.into());
                let fresh = self.graph.allocation(self.owner);
                self.graph.constraints.push(Constraint::CopyMemory(value, fresh));
                fresh
            }
            _ => value,
        }
    }

    fn method_target(&self, receiver: &Expr, name: &str, supplied: usize) -> Option<String> {
        let ty = self.expression_type(receiver)?;
        let key = (ty.universe_key()?.to_string(), name.to_string());
        if let Some(target) = self.graph.methods.get(&key) {
            let target = target.as_ref()?;
            return (self.graph.callables[target].params.len() == supplied + 1).then(|| target.clone());
        }
        (self.graph.callables.contains_key(name) || self.graph.generics.contains_key(name)).then(|| name.to_string())
    }

    fn method_call(&mut self, receiver: &Expr, name: &str, args: &[Expr]) -> Node {
        let target = self.method_target(receiver, name, args.len());
        let mut arguments = vec![receiver.clone()]; arguments.extend_from_slice(args);
        if let Some(target) = target { return self.call(&target, &arguments); }
        let values = arguments.iter().map(|arg| self.expression(arg)).collect::<Vec<_>>();
        self.capture(&values); self.unsupported(); self.graph.unknown_value()
    }

    fn call(&mut self, name: &str, arguments: &[Expr]) -> Node {
        let args: Vec<_> = arguments.iter().map(|arg| self.expression(arg)).collect();
        if let Some(generic) = self.graph.generics.get(name).cloned() {
            let result = self.specialize(&generic, arguments, &args);
            self.propagate_incoming_store(name, arguments);
            return result;
        }
        if let Some(target) = self.free_member_target(name, arguments) {
            let result = self.call_nodes(&target, &args);
            self.propagate_incoming_store(&target, arguments);
            return result;
        }
        let result = self.call_nodes(name, &args);
        self.propagate_incoming_store(name, arguments);
        result
    }

    /// 2026-11-08: does this value expression produce (or name) storage that can
    /// outlive the producing frame? Combines the syntactic allocation check with
    /// the tracked set of locals bound to allocation-producing values, so
    /// `t.key = key` (key from `let key = call(...)`) counts as publishing.
    fn value_allocates(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Identifier(name) => self.alloc_bindings.contains(name),
            _ => expression_allocates(expr),
        }
    }

    /// 2026-11-08: propagate a callee's publish-into-incoming-parameter effect
    /// to this callable at any argument projection depth. When the caller passes
    /// an argument that is (or is rooted at) one of its own parameters — a bare
    /// parameter, a field projection such as `w.t`, or a longer chain — and the
    /// callee stores allocating storage into an incoming receiver, then this
    /// callable likewise publishes into caller-owned memory. Without this the
    /// caller was misclassified as a reclaimable function-region and its
    /// teardown freed storage the caller still references (WebSocket key case).
    /// Over-approximating here is safe: a barrier only disables region reclamation.
    fn propagate_incoming_store(&mut self, callee: &str, arguments: &[Expr]) {
        if !self.graph.incoming_store_owners.contains(callee) { return; }
        let publishes = arguments.iter().any(|argument| {
            projection_root_and_path(argument)
                .map(|(root, _)| match root {
                    Expr::Identifier(root_name) => self.param_names.contains(root_name.as_str()),
                    _ => false,
                })
                .unwrap_or(false)
        });
        if publishes { self.graph.incoming_store_owners.insert(self.owner.into()); }
    }

    fn free_member_target(&self, name: &str, arguments: &[Expr]) -> Option<String> {
        if self.graph.callables.contains_key(name) { return None; }
        self.method_target(arguments.first()?, name, arguments.len()-1)
    }

    fn specialize(&mut self, generic: &crate::ast::Definition, arguments: &[Expr], args: &[Node]) -> Node {
        let params: Vec<_> = generic.type_params.iter().map(|param| param.name.clone()).collect();
        let mut substitution = std::collections::HashMap::new();
        for ((_, declared), argument) in generic.parameters.iter().zip(arguments) {
            if let Some(concrete) = self.expression_type(argument) {
                crate::analysis::generic_specialization::unify_generic_binding(declared, &concrete, &params, &mut substitution);
            }
        }
        let Some(concrete) = params.iter().map(|param| substitution.get(param).cloned()).collect::<Option<Vec<_>>>() else {
            self.unsupported(); return self.graph.node();
        };
        let name = crate::analysis::generic_specialization::mangle_specialization(&generic.name, &concrete);
        if !self.graph.callables.contains_key(&name) {
            let mut definition = generic.clone(); definition.name = name.clone(); definition.type_params.clear();
            for (_, ty) in &mut definition.parameters { *ty = crate::typechecker::substitute_type(ty, &substitution); }
            for statement in &mut definition.body { crate::typechecker::substitute_stmt_types(statement, &substitution); }
            if let Some(output) = &mut definition.output_type { traits::substitute_output(output, &substitution); }
            let item = TopLevel::Definition(definition.clone());
            let live = [name.clone()].into_iter().collect();
            register(self.graph, std::slice::from_ref(&item), &live);
            build_owner(self.graph, &name);
        }
        self.call_nodes(&name, args)
    }

    fn call_nodes(&mut self, name: &str, args: &[Node]) -> Node {
        if let Some(variant) = self.graph.constructor_key(name) {
            let root = self.graph.allocation(self.owner);
            for arg in args { self.graph.constraints.push(Constraint::FieldStore(root, format!("@variant:{variant}"), *arg)); }
            return root;
        }
        if let Some(callable) = self.graph.callables.get(name).cloned() {
            self.graph.callables.get_mut(self.owner).unwrap().calls.insert(name.into());
            for (arg, param) in args.iter().zip(&callable.params) { self.graph.copy(*arg, *param); }
            build_owner(self.graph, name);
            return callable.result;
        }
        self.intrinsic_or_unknown(name, &args)
    }

    fn intrinsic_or_unknown(&mut self, name: &str, args: &[Node]) -> Node {
        let result = self.graph.unknown_value();
        match name {
            "Alloc#" | "Malloc#" => {
                if args.len() != 1 { self.unsupported(); }
                return self.graph.allocation(self.owner);
            }
            "Free#" => {}
            "Load#" => { if let Some(base) = args.first() { self.load(*base, result); } }
            "Store#" | "Fill#" => { if args.len() >= 2 { self.graph.constraints.push(Constraint::Store(args[0], args[1])); } }
            "Copy#" => { if args.len() >= 2 { self.graph.constraints.push(Constraint::CopyMemory(args[1], args[0])); } }
            "SysCall#" => { if let Some(number) = args.first() { self.graph.constraints.push(Constraint::Syscall(*number, args[1..].to_vec())); } }
            "Print#" => { for arg in args { self.graph.constraints.push(Constraint::EscapeContents(*arg)); } }
            "Environ#" => { self.graph.points[result].insert(result); }
            "StringConcat#" | "StringBitAnd#" | "StringBitOr#" | "StringBitXor#" => return self.graph.allocation(self.owner),
            _ => {
                if crate::intrinsic_signatures::get_intrinsic_signature(name).is_some_and(|signature| !signature.observable) {
                    for arg in args { self.graph.copy(*arg, result); }
                    let signature = crate::intrinsic_signatures::get_intrinsic_signature(name).unwrap();
                    if !matches!(signature.return_kind, crate::intrinsic_signatures::ReturnKind::Native(_) | crate::intrinsic_signatures::ReturnKind::Never) {
                        let allocation = self.graph.allocation(self.owner); self.graph.copy(allocation, result);
                    }
                    return result;
                }
                if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() { eprintln!("reclamation unclassified call in {}: {name}", self.owner); }
                self.capture(args); self.unsupported();
            }
        }
        result
    }

    fn capture(&mut self, args: &[Node]) {
        let sink = self.graph.global("$external");
        for arg in args { self.graph.copy(*arg, sink); }
    }

    fn unsupported(&mut self) {
        self.graph.unsupported.insert(self.owner.into());
        let inputs: Vec<_> = self.bindings.values().copied().collect();
        self.capture(&inputs);
    }
}

/// 2026-11-07 (nested field store): root receiver and dotted field path of a
/// pure field-projection chain (`p.mid.inner` → (`p`, `mid.inner`)). Returns
/// `None` for any chain that is not a plain identifier followed only by field
/// segments, so index/deref targets keep their existing handling.
fn projection_root_and_path(expr: &Expr) -> Option<(&Expr, String)> {
    let mut names: Vec<&str> = Vec::new();
    let mut current = expr;
    loop {
        match current {
            Expr::Field(base, field) => {
                names.push(field.as_str());
                current = base.as_ref();
            }
            Expr::Identifier(_) => {
                names.reverse();
                return Some((current, names.join(".")));
            }
            _ => return None,
        }
    }
}

/// 2026-11-07 (nested field store): does evaluating this expression create
/// storage that can outlive the producing frame? Used to mark a callable that
/// publishes such storage into an incoming parameter's field as a barrier.
/// Scalar stores (`schedule.offered = total`) never allocate, so they must not
/// disable the caller's region (that turned a per-sample temporary into a
/// leaked heap block in the process collector).
fn expression_allocates(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Call(..)
            | Expr::StructLiteral { .. }
            | Expr::List(_)
            | Expr::Tuple(_)
            | Expr::Slice { .. }
            | Expr::AddrOf(_)
            | Expr::If(..)
            | Expr::Match(..)
            | Expr::Block(_)
    )
}
