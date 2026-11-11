// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! R (2026-11-11): reclamation analysis over typed HIR — first slice.
//!
//! The full port translates the AST `Graph`/solver in
//! `analysis::reclamation` behind a parity gate. This module starts with the
//! closed-initializer predicate the entry-region decision reads; AST remains
//! authoritative until the port is complete.

use super::program::HirProgram;

mod graph;
mod walk;

/// True when every top-level initializer is a closed (literal/absent) value:
/// no state declaration and no non-literal top-level `let` initializer.
///
/// AST parity (`analysis::reclamation::walk::closed_initializers`): StateDecl
/// is closed=false; a top-level `let` is closed only for Int/Float/Bool/Char
/// literal or absent initializers; all other item kinds are closed=true.
pub fn closed_initializers(program: &HirProgram) -> bool {
    program.globals.iter().all(|global| {
        if global.is_state_decl {
            return false;
        }
        match &global.expr {
            None => true,
            Some(expr) => matches!(
                expr.kind,
                super::expr::HirExprKind::Literal(
                    super::expr::HirLiteral::Int(_)
                        | super::expr::HirLiteral::Float(_)
                        | super::expr::HirLiteral::Bool(_)
                        | super::expr::HirLiteral::Char(_)
                )
            ),
        }
    })
}

/// AST parity: a member uses an implicit receiver when its body mentions
/// `self` or any declaring-type slot name.
fn uses_implicit_receiver(callable: &super::program::HirCallable, field_names: &[String]) -> bool {
    let mut implicit = false;
    crate::hir::expr_walk::walk_stmts(&callable.body.statements, &mut |e| {
        if let super::expr::HirExprKind::Identifier(name) = &e.kind {
            implicit |= name == "self" || field_names.iter().any(|f| f == name);
        }
    });
    implicit
}

/// 2026-11-12: HIR parity dump. Mirrors the AST helper: register, then build
/// each body owner in a fresh clone, rendering ordered constraints with node
/// identities canonicalised by first appearance, plus sorted call edges.
#[cfg(test)]
pub(crate) fn constraints_for_test(
    program: &HirProgram,
    universe: Option<&crate::type_universe::TypeUniverse>,
    casts: Option<&crate::casting::graph::CastingGraph>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut index = graph::Graph::default();
    if let Some(universe) = universe {
        index.universe = universe.clone();
    }
    if let Some(casts) = casts {
        index.casts = casts.clone();
    }
    walk::register(&mut index, program);
    dump_constraints(&index)
}

#[cfg(test)]
fn dump_constraints(
    index: &graph::Graph,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let owners: Vec<_> = index.bodies.keys().cloned().collect();
    let mut dumped = std::collections::BTreeMap::new();
    for owner in &owners {
        if !index.callables.contains_key(owner) {
            continue;
        }
        let mut graph = index.clone();
        graph
            .inexact_constants
            .extend(graph.callables[owner].params.iter().copied());
        walk::build_owner(&mut graph, owner);
        dumped.insert(
            owner.clone(),
            crate::analysis::reclamation::render_constraints_for_test(&graph.constraints),
        );
    }
    let mut calls_graph = index.clone();
    for owner in &owners {
        if !index.callables.contains_key(owner) {
            continue;
        }
        calls_graph
            .inexact_constants
            .extend(calls_graph.callables[owner].params.iter().copied());
        walk::build_owner(&mut calls_graph, owner);
    }
    for (name, callable) in &calls_graph.callables {
        let calls = callable.calls.iter().cloned().collect();
        dumped.insert(format!("calls:{name}"), calls);
    }
    dumped
}

/// HIR mirror of `analysis::reclamation::analyze`: register, build each body
/// owner on a fresh graph clone, solve, absorb into one plan.
pub fn analyze(
    program: &HirProgram,
    universe: Option<&crate::type_universe::TypeUniverse>,
) -> crate::analysis::reclamation::ReclamationPlan {
    let mut index = graph::Graph::default();
    if let Some(universe) = universe {
        index.universe = universe.clone();
    }
    walk::register(&mut index, program);
    let owners: Vec<_> = index.bodies.keys().cloned().collect();
    let mut result = crate::analysis::reclamation::ReclamationPlan::default();
    let mut accumulator = Accumulator::default();
    for owner in owners {
        let mut graph = index.clone();
        graph
            .inexact_constants
            .extend(graph.callables[&owner].params.iter().copied());
        graph.seed_incoming_storage(&owner);
        walk::build_owner(&mut graph, &owner);
        graph.solve();
        let plan = graph.plan();
        accumulator.absorb(&owner, &graph, &plan, &mut result);
    }
    accumulator.reclassify_safe_publishers(&mut result);
    result.regions.retain(|owner| !result.barriers.contains(owner));
    result.closed_regions = std::mem::take(&mut accumulator.closed_regions);
    result.closed_regions.retain(|owner| result.regions.contains(owner));
    if index.entry_nodes.len() == 1 && closed_initializers(program) {
        let owner = index.entry_nodes.iter().next().unwrap();
        result.entry_region = result.regions.contains(owner);
    }
    result.regions.retain(|owner| !index.entry_nodes.contains(owner));
    result.closed_regions.retain(|owner| result.regions.contains(owner));
    accumulator.finish_mutating(&mut result);
    result
}

#[derive(Default)]
struct Accumulator {
    dependencies: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    allocating: std::collections::BTreeSet<String>,
    publish_only: std::collections::BTreeSet<String>,
    escape_barriers: std::collections::BTreeSet<String>,
    publish_sites: std::collections::BTreeMap<String, Vec<bool>>,
    closed_regions: std::collections::BTreeSet<String>,
}

impl Accumulator {
    fn absorb(
        &mut self,
        owner: &str,
        graph: &graph::Graph,
        plan: &crate::analysis::reclamation::ReclamationPlan,
        result: &mut crate::analysis::reclamation::ReclamationPlan,
    ) {
        self.allocating
            .extend(graph.allocation_owner.values().cloned());
        for built in &graph.built {
            self.dependencies
                .entry(built.clone())
                .or_default()
                .extend(graph.call_closure(built, &std::collections::BTreeSet::new()));
        }
        if plan.regions.contains(owner) {
            result.regions.insert(owner.to_string());
        }
        if plan.closed_regions.contains(owner) {
            self.closed_regions.insert(owner.to_string());
        }
        result.barriers.extend(plan.barriers.iter().cloned());
        self.publish_only.extend(plan.publish_only.iter().cloned());
        self.escape_barriers
            .extend(plan.escape_barriers.iter().cloned());
        for (publisher, sites) in &plan.publish_sites {
            self.publish_sites
                .entry(publisher.clone())
                .or_default()
                .extend(sites.iter().copied());
        }
        result.known.extend(plan.known.iter().cloned());
        result
            .default_encoding_targets
            .extend(graph.default_encoding_targets.iter().cloned());
        if let Some(reason) = plan.reasons.get(owner) {
            result.reasons.insert(owner.to_string(), reason.clone());
        }
    }

    fn reclassify_safe_publishers(
        &self,
        result: &mut crate::analysis::reclamation::ReclamationPlan,
    ) {
        let safe: std::collections::BTreeSet<String> = self
            .publish_sites
            .iter()
            .filter(|(_, sites)| !sites.is_empty() && sites.iter().all(|safe| *safe))
            .map(|(publisher, _)| publisher.clone())
            .collect();
        result.barriers.retain(|name| {
            !(safe.contains(name)
                && self.publish_only.contains(name)
                && !self.escape_barriers.contains(name))
        });
    }

    fn finish_mutating(self, result: &mut crate::analysis::reclamation::ReclamationPlan) {
        let mut allocating = self.allocating;
        allocating.extend(result.regions.iter().cloned());
        allocating.extend(result.barriers.iter().cloned());
        result.mutating = self
            .dependencies
            .into_iter()
            .filter(|(_, calls)| !calls.is_disjoint(&allocating))
            .map(|(owner, _)| owner)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hir(src: &str) -> HirProgram {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        crate::hir::lower_program(&parser.parse_program().expect("parse"))
    }

    fn ast(src: &str) -> bool {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        crate::analysis::reclamation::closed_initializers_for_test(&items)
    }

    fn parity(src: &str) {
        assert_eq!(closed_initializers(&hir(src)), ast(src), "drift for:\n{src}");
    }

    #[test]
    fn literal_globals_are_closed() {
        parity("let a: Int = 1;\nlet b: Bool = false;\nlet c: Float = 1.5;\nlet d: Char = 'x';\n");
    }

    #[test]
    fn state_and_non_literal_globals_are_open() {
        parity("state let n: Int = 0;\n");
        parity("let xs: List<Int> = [1, 2];\n");
        parity("let n: Int = compute();\ndefn compute() -> Int { term 1; };\n");
    }

    fn facts_hir(src: &str) -> RegistrationFacts {
        registration_facts(&hir(src))
    }
    fn facts_ast(src: &str) -> RegistrationFacts {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        crate::analysis::reclamation::registration_facts_for_test(&items)
    }
    fn parity_facts(src: &str) {
        assert_eq!(facts_hir(src), facts_ast(src), "registration drift:\n{src}");
    }

    #[test]
    fn registration_matches_ast_for_callables() {
        parity_facts(
            "defn plain(a: Int) -> Int { term a; };\n\
             export defn exported(a: Int) -> Int { term a; };\n\
             defn generic<T>(x: T) -> T { term x; };\n\
             node reactive [true][true] { term; };\n\
             async txn waiting [true][true] { term; };",
        );
    }

    #[test]
    fn registration_matches_ast_for_enum_constructors() {
        parity_facts("enum Mode { Active(Int), Idle };");
    }

    #[test]
    fn registration_matches_ast_for_methods_and_user_operators() {
        parity_facts(
            "defn add_impl(a: Int, b: Int) -> Int { term a + b; };\n\
             obj Box { value: Int; defn get(b: Box) -> Int { term b.value; };\n\
                        op Add: add_impl(#Lh, #Rh); };",
        );
    }

    #[test]
    fn registration_matches_ast_for_globals() {
        parity_facts(
            "let a: Int = 1;\nstate let n: Int = 0;\nconst C: Int = 3;\n\
             defn f(a: Int) -> Int { term a; };",
        );
    }

    #[test]
    fn no_globals_is_closed() {
        parity("defn f() -> Int { term 1; };\n");
    }

    fn constraint_parity(src: &str) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let mut casts = crate::casting::graph::CastingGraph::new();
        for item in &items {
            if let crate::ast::TopLevel::ProtocolDef(protocol) = item {
                casts.register_protocol_def(protocol);
            }
        }
        let expected = crate::analysis::reclamation::constraints_for_test(&items, None);
        let actual = constraints_for_test(&crate::hir::lower_program(&items), None, Some(&casts));
        assert_eq!(
            actual, expected,
            "constraint drift:\n{src}\n--- AST ---\n{expected:#?}\n--- HIR ---\n{actual:#?}"
        );
    }

    fn plan_parity(src: &str) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let live: std::collections::HashSet<String> = items
            .iter()
            .filter_map(|item| match item {
                crate::ast::TopLevel::Definition(d) => Some(d.name.clone()),
                crate::ast::TopLevel::Transaction(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect();
        let expected =
            crate::analysis::reclamation::analyze(&items, &live, None);
        let actual = analyze(&crate::hir::lower_program(&items), None);
        assert_eq!(actual, expected, "plan drift:\n{src}");
    }

    #[test]
    fn plan_matches_ast_for_scalars_and_allocations() {
        plan_parity(
            "defn scalar() -> Int { let x: Int = 1; term x; };\n\
             defn allocate() -> Bool {\n\
                 let p: Ptr = Malloc#(32);\n\
                 Store#(p,42,8);\n\
                 term Load#(p,8) == 42;\n\
             };",
        );
    }

    #[test]
    fn plan_matches_ast_for_returns_and_publish() {
        plan_parity(
            "defn escape() -> Ptr { term Malloc#(8); };\n\
             defn local() -> Bool {\n\
                 let p: Ptr = Malloc#(8);\n\
                 Store#(p,1,8);\n\
                 term Load#(p,8) == 1;\n\
             };",
        );
    }

    #[test]
    fn plan_matches_ast_for_publish_methods_and_generics() {
        plan_parity(
            "obj T { key: String; flag: Int; };\n\
             defn enc(n: Int) -> String { term \"a\" + n as String; };\n\
             defn begin(t: T) -> Int { t.key = \"\"; t.flag = 1; term 0; };\n\
             defn upgrade(t: T, port: Int) -> Int {\n\
                 let key: String = enc(port);\n\
                 let started: Int = begin(t);\n\
                 t.key = key;\n\
                 term started;\n\
             };\n\
             obj Packet { storage: Ptr; defn read(packet: Packet) -> Int { term Load#(packet.storage,8); }; };\n\
             defn inspect() -> Int { let p: Packet = Packet { storage: Malloc#(8) }; term p.read(); };\n\
             defn identity<T>(value: T) -> T { term value; };\n\
             defn use_generic() -> Int { let q: Ptr = Malloc#(8); let alias: Ptr = identity(q); term Load#(alias,8); };",
        );
    }

    #[test]
    fn constraints_match_ast_for_scalars() {
        constraint_parity(
            "defn scalars(a: Int) -> Int {\n\
                 let x: Int = a + 1;\n\
                 x = x * 2;\n\
                 term x;\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_allocations() {
        constraint_parity(
            "defn allocate() -> Bool {\n\
                 let pointer: Ptr = Malloc#(32);\n\
                 Store#(pointer,42,8);\n\
                 let other: Ptr = Alloc#(16);\n\
                 Free#(other);\n\
                 term Load#(pointer,8) == 42;\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_structures_and_slices() {
        constraint_parity(
            "obj Record { buffer: Ptr; count: Int; };\n\
             defn structured() -> Bool {\n\
                 let pointer: Ptr = Malloc#(32);\n\
                 let record: Record = Record { buffer: pointer, count: 7 };\n\
                 let list: List<Int> = [1,2,3];\n\
                 let pair = (pointer,7);\n\
                 let first = pair.0;\n\
                 let sliced = list[0:2];\n\
                 let indexed = list[0];\n\
                 Store#(record.buffer,42,8);\n\
                 term Load#(pointer,8) == 42;\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_print_capture() {
        constraint_parity(
            "defn show() -> Void {\n\
                 let pointer: Ptr = Malloc#(32);\n\
                 Print#(pointer);\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_branches_and_matches() {
        constraint_parity(
            "enum Tagged { Reference(Ptr), Count(Int) };\n\
             defn branch(value: Tagged) -> Int {\n\
                 let pointer: Ptr = Malloc#(8);\n\
                 let loaded: Int = match true { true => Load#(pointer,8), false => 0, };\n\
                 when loaded == 0 { Store#(pointer,1,8); };\n\
                 term match value { Reference(held) => held as Int, Count(number) => number, };\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_user_calls_and_recursion() {
        constraint_parity(
            "defn create(depth: Int) -> Ptr {\n\
                 when depth == 0 { term Malloc#(32); };\n\
                 term create(depth-1);\n\
             };\n\
             defn inspect() -> Bool {\n\
                 let value: Ptr = create(3);\n\
                 Store#(value,42,8);\n\
                 term Load#(value,8) == 42;\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_generics() {
        constraint_parity(
            "defn identity<T>(value: T) -> T { term value; };\n\
             defn use_generic() -> Int {\n\
                 let pointer: Ptr = Malloc#(32);\n\
                 let alias: Ptr = identity(pointer);\n\
                 Store#(alias,1,8);\n\
                 term Load#(pointer,8);\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_explicit_receiver_methods() {
        constraint_parity(
            "obj Packet {\n\
                 storage: Ptr;\n\
                 defn read(packet: Packet) -> Int { term Load#(packet.storage,8); };\n\
             };\n\
             defn inspect() -> Int {\n\
                 let packet: Packet = Packet { storage: Malloc#(8) };\n\
                 term packet.read();\n\
             };\n\
             defn free_call() -> Int {\n\
                 let packet: Packet = Packet { storage: Malloc#(8) };\n\
                 term read(packet);\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_blocks() {
        constraint_parity(
            "defn blocked() -> Int {\n\
                 let pointer: Ptr = Malloc#(8);\n\
                 { let alias: Ptr = pointer; Store#(alias,1,8); };\n\
                 term Load#(pointer,8);\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_user_operators() {
        constraint_parity(
            "defn add_impl(a: Int, b: Int) -> Int { term a + b; };\n\
             obj Boxed { value: Int; op Add: add_impl(#Lh, #Rh); };\n\
             defn combine(a: Boxed, b: Boxed) -> Int { term a + b; };",
        );
    }

    #[test]
    fn constraints_match_ast_for_nested_field_stores() {
        constraint_parity(
            "obj Inner { fd: Int; };\n\
             obj Mid { a: Int; inner: Inner; b: Int; };\n\
             obj Wrap { marker: Int; mid: Mid; };\n\
             defn assign(w: Wrap) -> Void {\n\
                 w.mid.inner = Inner { fd: 3 };\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_publish_propagation() {
        constraint_parity(
            "obj T { key: String; flag: Int; };\n\
             obj W { t: T; n: Int; };\n\
             defn enc(n: Int) -> String { term \"a\" + n as String; };\n\
             defn begin(t: T, req: String) -> Int {\n\
                 t.key = \"\";\n\
                 t.flag = 1;\n\
                 term req .^Length;\n\
             };\n\
             defn upgrade(t: T, port: Int) -> Int {\n\
                 let key: String = enc(port);\n\
                 let request: String = \"GET \" + key + \" HTTP/1.1\";\n\
                 let started: Int = begin(t, request);\n\
                 t.key = key;\n\
                 term started;\n\
             };\n\
             defn start_upgrade(w: W, port: Int) -> Void {\n\
                 let ignored: Int = upgrade(w.t, port);\n\
             };\n\
             defn poll(w: W, port: Int) -> Int {\n\
                 start_upgrade(w, port);\n\
                 term w.t.key .^Length;\n\
             };",
        );
    }

    #[test]
    fn constraints_match_ast_for_casts() {
        constraint_parity(
            "defn casting() -> Int {\n\
                 let pointer: Ptr = Malloc#(32);\n\
                 let address: Int = pointer as Int;\n\
                 term address;\n\
             };",
        );
    }
}

/// R2 (2026-11-11): structural registration facts the reclamation walker needs
/// before constraints can be built. Fields not yet ported (constructors,
/// default-encoding targets, user operators, method alias map) are left out
/// until their AST sources have HIR equivalents.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistrationFacts {
    pub bodies: std::collections::BTreeMap<String, (Vec<(String, crate::ast::Type)>, usize)>,
    pub generics: std::collections::BTreeSet<String>,
    pub return_types: std::collections::BTreeMap<String, crate::ast::Type>,
    pub entry_nodes: std::collections::BTreeSet<String>,
    pub unsupported: std::collections::BTreeSet<String>,
    /// Enum variant names (constructor keys).
    pub constructors: std::collections::BTreeSet<String>,
    /// Types with a non-intrinsic op binding (user operator barrier).
    pub user_operators: std::collections::BTreeSet<String>,
    /// (owner, member) → synthetic `Owner::member` callable when the receiver
    /// is explicit; `None` shadows transactions and implicit-receiver methods.
    pub methods: std::collections::BTreeMap<(String, String), Option<String>>,
    pub globals: std::collections::BTreeSet<String>,
    pub global_types: std::collections::BTreeMap<String, crate::ast::Type>,
}

/// Mirror of `analysis::reclamation::walk::register` for the structural fields
/// above (AST parity is asserted in tests).
pub fn registration_facts(program: &HirProgram) -> RegistrationFacts {
    let mut facts = RegistrationFacts {
        bodies: Default::default(),
        generics: Default::default(),
        return_types: Default::default(),
        entry_nodes: Default::default(),
        unsupported: Default::default(),
        constructors: Default::default(),
        user_operators: Default::default(),
        methods: Default::default(),
        globals: Default::default(),
        global_types: Default::default(),
    };
    register_global_facts(program, &mut facts);
    register_enum_facts(program, &mut facts);
    for def in &program.type_defs {
        if def.has_user_operators {
            facts.user_operators.insert(def.name.clone());
        }
    }
    for def in &program.type_defs {
        register_type_methods(def, program, &mut facts);
    }
    for callable in &program.callables {
        register_callable_fact(callable, &mut facts);
    }
    facts
}

fn register_global_facts(program: &HirProgram, facts: &mut RegistrationFacts) {
    for global in &program.globals {
        facts.globals.insert(global.name.clone());
        if let Some(ty) = &global.ty {
            facts.global_types.insert(global.name.clone(), ty.clone());
        }
    }
    for (name, ty) in &program.const_types {
        facts.globals.insert(name.clone());
        facts.global_types.insert(name.clone(), ty.clone());
    }
}

fn register_enum_facts(program: &HirProgram, facts: &mut RegistrationFacts) {
    for def in &program.type_defs {
        register_enum(def, facts);
    }
}

fn register_enum(def: &super::program::HirTypeDef, facts: &mut RegistrationFacts) {
    let super::program::HirTypeDefKind::Enum { variants } = &def.kind else {
        return;
    };
    for variant in variants {
        let name = match variant {
            super::program::HirEnumVariant::Unit(n)
            | super::program::HirEnumVariant::Tuple(n, _)
            | super::program::HirEnumVariant::Struct(n, _) => n,
        };
        facts.constructors.insert(name.clone());
    }
}

fn member_callable(c: &super::program::HirCallable) -> bool {
    !c.is_impl_fn
        && !c.trait_default
        && matches!(
            c.kind,
            super::program::HirCallableKind::MemberDefn
                | super::program::HirCallableKind::MemberTransaction
        )
}

/// Explicit-receiver methods register as `Owner::method` synthetic callables;
/// implicit-receiver members only shadow the key (AST parity).
fn register_type_methods(
    def: &super::program::HirTypeDef,
    program: &HirProgram,
    facts: &mut RegistrationFacts,
) {
    let fields = match &def.kind {
        super::program::HirTypeDefKind::Object { fields }
        | super::program::HirTypeDefKind::Struct { fields } => fields,
        super::program::HirTypeDefKind::Enum { .. } => return,
    };
    let field_names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
    let members = program
        .callables
        .iter()
        .filter(|c| c.owner.as_deref() == Some(def.name.as_str()) && member_callable(c));
    for callable in members {
        let key = (def.name.clone(), callable.name.clone());
        facts.methods.insert(key.clone(), None);
        if !synthetic_method(def, callable, &field_names) {
            continue;
        }
        let synthetic = format!("{}::{}", def.name, callable.name);
        facts.methods.insert(key, Some(synthetic.clone()));
        let params = callable
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty.clone()))
            .collect();
        facts
            .bodies
            .insert(synthetic.clone(), (params, callable.body.statements.len()));
        if let Some(ty) = &callable.result {
            facts.return_types.insert(synthetic, ty.clone());
        }
    }
}

fn synthetic_method(
    def: &super::program::HirTypeDef,
    callable: &super::program::HirCallable,
    field_names: &[String],
) -> bool {
    if callable.kind != super::program::HirCallableKind::MemberDefn
        || !def.type_params.is_empty()
        || !callable.type_params.is_empty()
        || uses_implicit_receiver(callable, field_names)
    {
        return false;
    }
    callable
        .params
        .first()
        .is_some_and(|first| first.ty.universe_key() == Some(def.name.as_str()))
}

fn register_callable_fact(
    callable: &super::program::HirCallable,
    facts: &mut RegistrationFacts,
) {
    use super::program::HirCallableKind;
    if callable.owner.is_some() || callable.is_isr || callable.trait_default || callable.is_impl_fn {
        return;
    }
    if callable.kind == HirCallableKind::Defn && !callable.type_params.is_empty() {
        facts.generics.insert(callable.name.clone());
        return;
    }
    match callable.kind {
        HirCallableKind::Defn => register_plain_callable(callable, facts, 0),
        HirCallableKind::Transaction => {
            let contract = usize::from(callable.contract.is_some()) * 2;
            register_plain_callable(callable, facts, contract);
            if callable.is_reactive {
                facts.entry_nodes.insert(callable.name.clone());
            }
            if callable.is_async {
                facts.unsupported.insert(callable.name.clone());
            }
        }
        _ => {}
    }
}

fn register_plain_callable(
    callable: &super::program::HirCallable,
    facts: &mut RegistrationFacts,
    extra_statements: usize,
) {
    let params = callable
        .params
        .iter()
        .map(|p| (p.name.clone(), p.ty.clone()))
        .collect();
    facts.bodies.insert(
        callable.name.clone(),
        (params, callable.body.statements.len() + extra_statements),
    );
    if let Some(ty) = &callable.result {
        facts.return_types.insert(callable.name.clone(), ty.clone());
    }
}
