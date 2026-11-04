//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Constraint construction for region escape proofs. Unsupported forms capture
//! their inputs instead of being interpreted as evidence for reclamation.
use super::*;
use crate::ast::{BinaryOpKind, Pattern, UnaryOpKind};

pub(super) fn register(graph: &mut Graph, items: &[TopLevel], live: &std::collections::HashSet<String>) {
    for item in items { register_item(graph, item, live); }
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
        TopLevel::Definition(definition) if live.contains(&definition.name) => register_definition(graph, definition),
        TopLevel::Transaction(transaction) => register_transaction(graph, transaction),
        TopLevel::Export(export) => register_item(graph, &export.inner, live),
        TopLevel::SyncGroup { item, .. } | TopLevel::Fuzzed { item, .. } => register_item(graph, item, live),
        TopLevel::Enum(definition) => register_enum(graph, definition),
        TopLevel::TypeDef(definition) => register_typedef(graph, definition),
        TopLevel::Constant(constant) => {
            let node = graph.global(&constant.name);
            graph.global_types.insert(constant.name.clone(), constant.ty.clone());
            if let Expr::Decimal(value) = &constant.expr { graph.constants[node].insert(*value); graph.inexact_constants.remove(&node); }
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
    let ty = output.and_then(|output| output.all_types().into_iter().next());
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
    }
}

fn register_typedef(graph: &mut Graph, definition: &crate::ast::TypeDef) {
    if definition.body.op_bindings.iter().any(|binding| !matches!(&binding.expr,
        Expr::Call(name, _, _) if crate::intrinsic_signatures::get_intrinsic_signature(name).is_some())) {
        graph.user_operators.insert(definition.name.clone());
    }
    for slot in &definition.body.slots {
        if let Some(name) = slot.name.strip_prefix("__variant_") { graph.constructors.insert(name.into()); }
    }
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
    let mut builder = Builder { graph, owner, bindings, types, result: callable.result };
    builder.statements(statements);
}

struct Builder<'a> {
    graph: &'a mut Graph,
    owner: &'a str,
    bindings: BTreeMap<String, Node>,
    types: BTreeMap<String, crate::ast::Type>,
    result: Node,
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
                if let Some(ty) = ty.clone().or_else(|| expr.as_ref().and_then(|expr| self.expression_type(expr))) { self.types.insert(name.clone(), ty); }
                let target = self.local(name);
                if let Some(expr) = expr {
                    let value = self.expression(expr);
                    self.graph.copy(value, target);
                    for name in names { let target = self.local(name); self.load(value, target); }
                }
            }
            Statement::Assign(target, value) => {
                let value = self.expression(value);
                self.assign(target, value);
            }
            Statement::Term(Some(value)) | Statement::Tail(value) => {
                let value = self.expression(value); self.graph.copy(value, self.result);
            }
            Statement::Term(None) => {}
            Statement::Expression(value) | Statement::Check(value) | Statement::Gate(value) => { self.expression(value); }
            Statement::Guarded(condition, body) => { self.expression(condition); self.statements(body); }
            Statement::Block(body) | Statement::SyncBlock(body) | Statement::Mutex(body) | Statement::Defer(body) => self.statements(body),
            Statement::Foreach { item, list, body } => {
                if matches!(list.as_ref(), Expr::Range { .. }) { self.types.insert(item.clone(), crate::ast::Type::int()); }
                let source = self.expression(list);
                let target = self.local(item);
                self.load(source, target);
                self.statements(body);
            }
            Statement::Match { expr, arms } => {
                let value = self.expression(expr);
                for arm in arms { self.statement_arm(value, arm); }
            }
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

    fn statement_arm(&mut self, value: Node, arm: &crate::ast::StmtMatchArm) {
        for pattern in &arm.patterns { self.pattern(pattern, value); }
        self.statements(&arm.body);
    }

    fn assign(&mut self, target: &Expr, value: Node) {
        if let Expr::Identifier(name) = target {
            let target = self.binding(name);
            self.graph.copy(value, target);
            return;
        }
        if let Expr::Field(base, field) = target {
            let base = self.expression(base);
            self.graph.constraints.push(Constraint::FieldStore(base, field.clone(), value)); return;
        }
        let base = match target {
            Expr::Field(base, _) | Expr::Index(base, _) | Expr::Deref(base) => self.expression(base),
            _ => { self.unsupported(); return; }
        };
        self.graph.constraints.push(Constraint::Store(base, value));
    }

    fn load(&mut self, base: Node, value: Node) { self.graph.constraints.push(Constraint::Load(base, value)); }

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
                let payload = self.graph.node();
                self.load(value, payload);
                for field in fields { self.pattern(field, payload); }
            }
            Pattern::Multi(patterns) => { for pattern in patterns { self.pattern(pattern, value); } }
            Pattern::Literal(expression) => { self.expression(expression); }
            Pattern::Range(start, end) | Pattern::RangeInclusive(start, end) => { self.expression(start); self.expression(end); }
            _ => {}
        }
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
            Expr::List(values) | Expr::Tuple(values) => {
                let root = self.graph.allocation(self.owner);
                for value in values { self.store_expression(root, value); }
                root
            }
            Expr::Index(base, index) => {
                self.expression(index);
                let base = self.expression(base); let result = self.graph.node(); self.load(base, result); result
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
            Expr::Consume(value) | Expr::Capture { expr: value, .. } => self.expression(value),
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
                let value = self.expression(value); let result = self.graph.node();
                for arm in arms { self.expression_arm(value, result, arm); }
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
        let left = self.expression(lhs); let right = self.expression(rhs); let result = self.graph.unknown_value();
        if self.expression_type(lhs).is_none() || self.expression_type(rhs).is_none() {
            self.unsupported(); self.capture(&[left, right]);
        }
        if self.expression_type(lhs).is_some_and(|ty| self.has_user_operators(&ty)) {
            self.unsupported(); self.capture(&[left, right]);
        }
        if matches!(operator, BinaryOpKind::Eq | BinaryOpKind::Neq | BinaryOpKind::Lt | BinaryOpKind::Gt
            | BinaryOpKind::Le | BinaryOpKind::Ge | BinaryOpKind::And | BinaryOpKind::Or) { return result; }
        self.graph.copy(left, result); self.graph.copy(right, result);
        if operator == BinaryOpKind::Concat || self.allocating_operator(lhs, operator) || self.allocating_operator(rhs, operator) {
            let allocation = self.graph.allocation(self.owner); self.graph.copy(allocation, result);
        }
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
            Expr::Cast(_, target) => Some(target.clone()),
            Expr::Call(name, arguments, _) => self.graph.return_types.get(name).cloned().or_else(|| self.generic_result(name, arguments)).or_else(|| {
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
                match self.expression_type(base)? { crate::ast::Type::Vector(inner, _) => Some(*inner), _ => None }
            }
            Expr::BinaryOp(_, lhs, _) => self.expression_type(lhs),
            Expr::Field(base, field) => {
                let ty = self.expression_type(base)?;
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
        let output = generic.output_type.as_ref()?.all_types().into_iter().next()?;
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
        let (source, source_variant) = self.graph.casts.type_to_protocol(&self.graph.universe, &source);
        let (target, target_variant) = self.graph.casts.type_to_protocol(&self.graph.universe, target);
        let Some(steps) = self.graph.casts.find_path(&source, &source_variant, &target, &target_variant) else { self.unsupported(); return value };
        for step in steps { value = self.cast_lane(&step.lane, value); }
        value
    }

    fn cast_lane(&mut self, lane: &crate::casting::graph::LaneKind, value: Node) -> Node {
        use crate::casting::graph::LaneKind;
        match lane {
            LaneKind::ExtCall(name) => self.call_nodes(name, &[value]),
            LaneKind::ExtCallDyn(name) => self.call_nodes(name, &[value]),
            LaneKind::Chain(first, second) => { let value = self.cast_lane(first, value); self.cast_lane(second, value) }
            LaneKind::CastFromBitCallback => { self.unsupported(); value }
            _ => value,
        }
    }

    fn call(&mut self, name: &str, arguments: &[Expr]) -> Node {
        let args: Vec<_> = arguments.iter().map(|arg| self.expression(arg)).collect();
        if let Some(generic) = self.graph.generics.get(name).cloned() {
            return self.specialize(&generic, arguments, &args);
        }
        self.call_nodes(name, &args)
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
            if let Some(crate::ast::OutputType::Single(ty)) = &mut definition.output_type { *ty = crate::typechecker::substitute_type(ty, &substitution); }
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
