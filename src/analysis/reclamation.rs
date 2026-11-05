//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! 2026-11-04: allocation regions licensed by whole-call escape proofs.
//! Allocation identity is independent of representation: caller images and
//! borrowed/static storage never become owned merely because they hold handles.
//! A region ends only when none of its allocations is reachable from a return,
//! incoming storage, global storage or an unclassified external consumer.
//! Undo: detach this plan and its LLVM consumer; ordinary allocation remains.

use crate::ast::{Expr, Statement, TopLevel};
use std::collections::{BTreeMap, BTreeSet};

mod walk;
mod socket_effects;

type Node = usize;

#[derive(Clone, Debug, Default)]
pub struct ReclamationPlan {
    pub entry_region: bool,
    pub default_encoding_targets: BTreeSet<String>,
    pub known: BTreeSet<String>,
    pub mutating: BTreeSet<String>,
    pub regions: BTreeSet<String>,
    pub barriers: BTreeSet<String>,
    pub reasons: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct Callable {
    params: Vec<Node>,
    result: Node,
    calls: BTreeSet<String>,
}

#[derive(Clone)]
enum Constraint {
    Bitwise(crate::ast::BinaryOpKind, Node, Node, Node),
    Copy(Node, Node),
    Load(Node, Node),
    Store(Node, Node),
    CopyMemory(Node, Node),
    Syscall(Node, Vec<Node>),
    EscapeContents(Node),
    FieldLoad(Node, String, Node),
    FieldStore(Node, String, Node),
}

#[derive(Clone)]
struct Graph {
    universe: crate::type_universe::TypeUniverse,
    casts: crate::casting::graph::CastingGraph,
    return_types: BTreeMap<String, crate::ast::Type>,
    generics: BTreeMap<String, crate::ast::Definition>,
    bodies: BTreeMap<String, (Vec<(String, crate::ast::Type)>, Vec<Statement>)>,
    built: BTreeSet<String>,
    handle_returns: BTreeSet<String>,
    default_encoding_targets: BTreeSet<String>,
    entry_nodes: BTreeSet<String>,
    user_operators: BTreeSet<String>,
    methods: BTreeMap<(String, String), Option<String>>,
    points: Vec<BTreeSet<Node>>,
    constants: Vec<BTreeSet<i64>>,
    inexact_constants: BTreeSet<Node>,
    memory: Vec<BTreeSet<Node>>,
    named_memory: BTreeMap<(Node, String), BTreeSet<Node>>,
    constraints: Vec<Constraint>,
    allocation_owner: BTreeMap<Node, String>,
    callables: BTreeMap<String, Callable>,
    globals: BTreeMap<String, Node>,
    global_types: BTreeMap<String, crate::ast::Type>,
    constructors: BTreeSet<String>,
    escaped: BTreeSet<Node>,
    unsupported: BTreeSet<String>,
}

impl Default for Graph {
    fn default() -> Self {
        Self { universe: crate::type_universe::TypeUniverse::new(), casts: crate::casting::graph::CastingGraph::new(),
            return_types: BTreeMap::new(), generics: BTreeMap::new(), bodies: BTreeMap::new(), built: BTreeSet::new(), handle_returns: BTreeSet::new(), default_encoding_targets: BTreeSet::new(), entry_nodes: BTreeSet::new(), user_operators: BTreeSet::new(), points: Vec::new(), constants: Vec::new(), memory: Vec::new(),
            inexact_constants: BTreeSet::new(), named_memory: BTreeMap::new(), constraints: Vec::new(), allocation_owner: BTreeMap::new(), callables: BTreeMap::new(),
            globals: BTreeMap::new(), global_types: BTreeMap::new(), constructors: BTreeSet::new(), escaped: BTreeSet::new(), unsupported: BTreeSet::new(), methods: BTreeMap::new() }
    }
}

impl Graph {
    fn constructor_key(&self, name: &str) -> Option<String> {
        let leaf = name.rsplit('.').next().unwrap_or(name);
        self.constructors.contains(leaf).then(|| leaf.into())
    }

    fn node(&mut self) -> Node {
        let node = self.points.len();
        self.points.push(BTreeSet::new());
        self.constants.push(BTreeSet::new());
        self.memory.push(BTreeSet::new());
        node
    }

    fn root(&mut self) -> Node {
        let node = self.node();
        self.points[node].insert(node);
        node
    }

    fn unknown_value(&mut self) -> Node {
        let node = self.node();
        self.inexact_constants.insert(node);
        node
    }

    fn allocation(&mut self, owner: &str) -> Node {
        let root = self.root();
        self.allocation_owner.insert(root, owner.into());
        root
    }

    fn copy(&mut self, source: Node, target: Node) {
        self.constraints.push(Constraint::Copy(source, target));
    }

    fn global(&mut self, name: &str) -> Node {
        if let Some(node) = self.globals.get(name) { return *node; }
        let node = self.root();
        self.inexact_constants.insert(node);
        self.globals.insert(name.into(), node);
        node
    }

    fn solve(&mut self) {
        while self.propagate_once() {}
        for constraint in self.constraints.clone() {
            match constraint {
                Constraint::Syscall(number, args) => { self.syscall(number, &args); }
                Constraint::EscapeContents(value) => { let refs = self.contents(value); self.escaped.extend(refs); }
                _ => {}
            }
        }
    }

    fn propagate_once(&mut self) -> bool {
        let mut changed = false;
        for constraint in self.constraints.clone() { changed |= self.apply(constraint); }
        changed
    }

    fn apply(&mut self, constraint: Constraint) -> bool {
        match constraint {
            Constraint::Bitwise(operator, left, right, result) => self.bitwise_facts(operator, left, right, result),
            Constraint::Copy(source, target) => {
                let refs = self.points[source].clone();
                let constants = self.constants[source].clone();
                let inexact = self.inexact_constants.contains(&source) && self.inexact_constants.insert(target);
                extend(&mut self.points[target], refs) | extend(&mut self.constants[target], constants) | inexact
            }
            Constraint::Load(base, result) => {
                let refs = self.contents(base);
                extend(&mut self.points[result], refs) | self.inexact_constants.insert(result)
            }
            Constraint::Store(base, value) => self.store(base, self.points[value].clone()),
            Constraint::CopyMemory(source, target) => self.store(target, self.contents(source)),
            Constraint::Syscall(..) => false,
            Constraint::EscapeContents(_) => false,
            Constraint::FieldLoad(base, field, target) => {
                let refs = self.field_contents(base, &field); extend(&mut self.points[target], refs) | self.inexact_constants.insert(target)
            }
            Constraint::FieldStore(base, field, value) => {
                let mut changed = false;
                for root in self.points[base].clone() {
                    changed |= extend(self.named_memory.entry((root, field.clone())).or_default(), self.points[value].iter().copied());
                }
                changed
            }
        }
    }

    fn contents(&self, base: Node) -> BTreeSet<Node> {
        self.points[base].iter().flat_map(|root| self.root_contents(*root)).collect()
    }

    fn root_contents(&self, root: Node) -> BTreeSet<Node> {
        let mut refs = self.memory[root].clone();
        for ((owner, _), values) in &self.named_memory { if *owner == root { refs.extend(values); } }
        refs
    }

    fn field_contents(&self, base: Node, field: &str) -> BTreeSet<Node> {
        let mut refs = BTreeSet::new();
        for root in &self.points[base] {
            refs.extend(&self.memory[*root]);
            if let Some(values) = self.named_memory.get(&(*root, field.into())) { refs.extend(values); }
        }
        refs
    }

    fn store(&mut self, base: Node, refs: BTreeSet<Node>) -> bool {
        let mut changed = false;
        for root in self.points[base].clone() {
            changed |= extend(&mut self.memory[root], refs.iter().copied());
        }
        changed
    }

    fn syscall(&mut self, number: Node, args: &[Node]) -> bool {
        // The platform table describes synchronous borrowing interfaces. An
        // unknown number remains capturing; this is never a helper-name rule.
        let numbers = &self.constants[number];
        let borrowed = !self.inexact_constants.contains(&number) && !numbers.is_empty() && numbers.iter().all(|number| {
            self.syscall_borrows(*number, args)
        });
        let refs = args.iter().flat_map(|arg| {
            if borrowed { self.contents(*arg) } else { self.points[*arg].clone() }
        }).collect::<BTreeSet<_>>();
        extend(&mut self.escaped, refs)
    }

    fn reachable(&self, roots: impl IntoIterator<Item = Node>) -> BTreeSet<Node> {
        let mut found = BTreeSet::new();
        let mut pending: Vec<_> = roots.into_iter().collect();
        while let Some(root) = pending.pop() {
            if found.insert(root) { pending.extend(self.root_contents(root)); }
        }
        found
    }

    fn plan(&self) -> ReclamationPlan {
        let global_roots = self.globals.values().flat_map(|node| self.points[*node].iter().copied());
        let escaped = self.reachable(global_roots.chain(self.escaped.iter().copied()));
        let mut plan = ReclamationPlan { known: self.callables.keys().cloned().collect(), ..Default::default() };
        for root in &escaped {
            if let Some(owner) = self.allocation_owner.get(root) { plan.barriers.insert(owner.clone()); }
        }
        plan.barriers.extend(self.unsupported.iter().cloned());
        for (name, callable) in &self.callables {
            self.classify(name, callable, &mut plan);
        }
        plan
    }

    fn classify(&self, name: &str, callable: &Callable, plan: &mut ReclamationPlan) {
        if plan.barriers.contains(name) { return; }
        if callable.calls.iter().any(|callee| self.call_closure(callee, &BTreeSet::new()).contains(name)) {
            // 2026-11-04: an added return epilogue would block LLVM tail-call
            // elimination and turn a bounded sampling loop into stack growth.
            // Nested proved scopes still reclaim each iteration's values.
            // Retire this fallback with a proved recursive-iteration consumer.
            plan.reasons.insert(name.into(), "recursive region needs an iteration-boundary consumer".into()); return;
        }
        if self.handle_returns.contains(name) {
            plan.reasons.insert(name.into(), "result image or payload can be caller-owned".into()); return;
        }
        if !self.call_closure(name, &BTreeSet::new()).is_disjoint(&self.unsupported) {
            plan.reasons.insert(name.into(), "callee effects are not classified".into()); return;
        }
        let owners = self.call_closure(name, &plan.barriers);
        let allocations: BTreeSet<_> = self.allocation_owner.iter()
            .filter(|(_, owner)| owners.contains(*owner)).map(|(root, _)| *root).collect();
        if allocations.is_empty() { return; }
        let returned = self.reachable(self.points[callable.result].iter().copied());
        if !returned.is_disjoint(&allocations) {
            plan.reasons.insert(name.into(), "allocation reaches the returned value".into());
            return;
        }
        let incoming = callable.params.iter().flat_map(|param| self.points[*param].iter().copied());
        let retained = self.reachable(incoming);
        if !retained.is_disjoint(&allocations) {
            let owners: BTreeSet<_> = retained.intersection(&allocations).filter_map(|root| self.allocation_owner.get(root)).collect();
            plan.reasons.insert(name.into(), format!("allocation may survive in incoming storage: {owners:?}"));
            return;
        }
        plan.regions.insert(name.into());
    }

    fn call_closure(&self, name: &str, barriers: &BTreeSet<String>) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut pending = vec![name.to_string()];
        while let Some(name) = pending.pop() {
            if barriers.contains(&name) || !found.insert(name.clone()) { continue; }
            if let Some(callable) = self.callables.get(&name) { pending.extend(callable.calls.iter().cloned()); }
        }
        found
    }
}

fn extend<T: Ord>(target: &mut BTreeSet<T>, values: impl IntoIterator<Item = T>) -> bool {
    let before = target.len();
    target.extend(values);
    target.len() != before
}

pub fn analyze(items: &[TopLevel], live: &std::collections::HashSet<String>, universe: Option<&crate::type_universe::TypeUniverse>) -> ReclamationPlan {
    let mut index = Graph::default();
    if let Some(universe) = universe { index.universe = universe.clone(); }
    for item in items { if let TopLevel::ProtocolDef(protocol) = item { index.casts.register_protocol_def(protocol); } }
    walk::register(&mut index, items, live);
    if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() { eprintln!("reclamation declared user operators: {:?}", index.user_operators); }
    let owners: Vec<_> = index.bodies.keys().cloned().collect();
    let mut result = ReclamationPlan::default();
    let mut dependencies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut allocating = BTreeSet::new();
    for owner in owners {
        let mut graph = index.clone();
        graph.inexact_constants.extend(graph.callables[&owner].params.iter().copied());
        walk::build_owner(&mut graph, &owner);
        graph.solve();
        let plan = graph.plan();
        allocating.extend(graph.allocation_owner.values().cloned());
        record_dependencies(&graph, &mut dependencies);
        if std::env::var_os("BRIEV_DIAG_RECLAMATION").is_some() { eprintln!("reclamation root {owner}: barriers={:?}, unsupported={:?}", plan.barriers, graph.unsupported); }
        if plan.regions.contains(&owner) { result.regions.insert(owner.clone()); }
        result.barriers.extend(plan.barriers);
        result.known.extend(plan.known);
        result.default_encoding_targets.extend(graph.default_encoding_targets);
        if let Some(reason) = plan.reasons.get(&owner) { result.reasons.insert(owner, reason.clone()); }
    }
    result.regions.retain(|owner| !result.barriers.contains(owner));
    // Main also executes global initialization. Until those allocations have
    // explicit global lifetime facts, only closed scalar initializers license
    // an entry scope. Helper proofs remain independent of this fallback.
    if index.entry_nodes.len() == 1 && walk::closed_initializers(items) {
        let owner = index.entry_nodes.iter().next().unwrap();
        result.entry_region = result.regions.contains(owner);
    }
    result.regions.retain(|owner| !index.entry_nodes.contains(owner));
    allocating.extend(result.regions.iter().cloned());
    allocating.extend(result.barriers.iter().cloned());
    result.mutating = dependencies.into_iter().filter(|(_, calls)| !calls.is_disjoint(&allocating)).map(|(owner, _)| owner).collect();
    result
}

fn record_dependencies(graph: &Graph, dependencies: &mut BTreeMap<String, BTreeSet<String>>) {
    for built in &graph.built {
        dependencies.entry(built.clone()).or_default().extend(graph.call_closure(built, &BTreeSet::new()));
    }
}

#[cfg(all(feature = "kani", feature = "kani_full"))]
mod kani_full_tests {
    use super::*;

    #[kani::proof]
    #[kani::unwind(256)]
    fn escaped_aliases_never_license_reclamation() {
        let mut graph = Graph::default();
        let incoming = graph.root();
        let result = graph.node();
        graph.callables.insert("proof".into(), Callable { params: vec![incoming], result, calls: BTreeSet::new() });
        let owned = graph.allocation("proof");
        let alias = graph.node();
        graph.copy(owned, alias);
        if kani::any::<bool>() { graph.constraints.push(Constraint::Store(owned, alias)); }
        if kani::any::<bool>() { graph.copy(alias, result); }
        else { graph.constraints.push(Constraint::FieldStore(incoming, "saved".into(), alias)); }
        graph.solve();
        assert!(!graph.plan().regions.contains("proof"));
    }

    #[kani::proof]
    #[kani::unwind(256)]
    fn closed_local_aliases_license_reclamation() {
        let mut graph = Graph::default();
        let incoming = graph.root();
        let result = graph.node();
        graph.callables.insert("proof".into(), Callable { params: vec![incoming], result, calls: BTreeSet::new() });
        let owned = graph.allocation("proof");
        let alias = graph.node(); graph.copy(owned, alias);
        if kani::any::<bool>() { graph.constraints.push(Constraint::Store(owned, alias)); }
        graph.solve();
        assert!(graph.plan().regions.contains("proof"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclassified_global_initialization_prevents_an_entry_region() {
        let plan = derive(r#"
            let retained: Ptr = Malloc#(32);
            node entry [true][true] {
                let local: Ptr = Malloc#(32);
                Store#(local,42,8);
                endprogram;
            };
        "#);
        assert!(!plan.entry_region, "{plan:?}");
    }

    #[test]
    fn an_unknown_reactor_condition_prevents_an_entry_region() {
        let plan = derive(r#"
            node entry [external_condition()][true] {
                let local: Ptr = Malloc#(32);
                Store#(local,42,8);
                endprogram;
            };
        "#);
        assert!(!plan.entry_region, "{plan:?}");
    }

    #[test]
    fn computed_or_partly_unknown_syscall_numbers_never_prove_borrowing() {
        let plan = derive(r#"
            defn inspect(number: Int) -> Bool {
                let pointer: Ptr = Malloc#(32);
                SysCall#(number+1,pointer);
                term true;
            };
        "#);
        assert!(!plan.regions.contains("inspect"), "{plan:?}");
        let mut graph = Graph::default();
        let known = graph.node(); graph.constants[known].insert(1);
        let unknown = graph.unknown_value();
        let merged = graph.node(); graph.copy(known, merged); graph.copy(unknown, merged);
        let pointer = graph.allocation("proof");
        graph.constraints.push(Constraint::Syscall(merged, vec![pointer]));
        graph.solve();
        assert!(graph.escaped.contains(&pointer));
    }

    #[test]
    fn process_entry_region_does_not_depend_on_a_source_node_name() {
        let plan = derive(r#"
            let done: Bool = false;
            node arbitrary_entry [!done][done] {
                let pointer: Ptr = Malloc#(32);
                Store#(pointer,42,8);
                endprogram;
            };
        "#);
        assert!(plan.entry_region, "{plan:?}");
        assert!(!plan.regions.contains("arbitrary_entry"));
    }

    fn derive(source: &str) -> ReclamationPlan {
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source).parse_program().unwrap();
        let live = items.iter().filter_map(|item| match item {
            TopLevel::Definition(definition) => Some(definition.name.clone()), _ => None,
        }).collect();
        analyze(&items, &live, None)
    }

    #[test]
    fn all_local_aliases_fit_one_region() {
        let plan = derive(r#"
            defn inspect() -> Bool {
                let first: Ptr = Malloc#(32);
                let second: Ptr = first;
                Store#(second,42,8);
                term Load#(first,8) == 42;
            };
        "#);
        assert!(plan.regions.contains("inspect"), "{plan:?}");
    }

    #[test]
    fn returned_storage_survives_until_its_consuming_caller_exits() {
        let plan = derive(r#"
            defn create() -> Ptr { term Malloc#(32); };
            defn inspect() -> Bool {
                let value: Ptr = create();
                let alias: Ptr = value;
                Store#(value,42,8);
                term Load#(alias,8) == 42;
            };
        "#);
        assert!(!plan.regions.contains("create"));
        assert!(plan.regions.contains("inspect"), "{plan:?}");
    }

    #[test]
    fn writing_fresh_storage_into_a_borrowed_destination_is_an_escape() {
        let plan = derive(r#"
            defn retain(destination: Ptr) -> Bool {
                let value: Ptr = Malloc#(32);
                Store#(destination,value,8);
                term true;
            };
        "#);
        assert!(!plan.regions.contains("retain"));
        assert!(plan.reasons["retain"].contains("incoming storage"));
    }

    #[test]
    fn a_pointer_returned_through_an_integer_is_not_a_scalar_lifetime() {
        let plan = derive(r#"
            defn create() -> Int {
                let value: Ptr = Malloc#(32);
                term value as Int;
            };
        "#);
        assert!(!plan.regions.contains("create"), "{plan:?}");
    }

    #[test]
    fn recursive_return_edges_reach_the_fixed_point() {
        let plan = derive(r#"
            defn create(depth: Int) -> Ptr {
                when depth == 0 { term Malloc#(32); };
                term create(depth-1);
            };
            defn inspect() -> Bool {
                let value: Ptr = create(3);
                Store#(value,42,8);
                term Load#(value,8) == 42;
            };
        "#);
        assert!(!plan.regions.contains("create"));
        assert!(plan.regions.contains("inspect"), "{plan:?}");
    }

    #[test]
    fn incoming_and_new_instances_of_one_allocation_site_are_distinct() {
        let plan = derive(r#"
            defn create() -> Ptr { term Malloc#(32); };
            defn inspect(prior: Ptr) -> Bool {
                let current: Ptr = create();
                Store#(current,42,8);
                term Load#(prior,8) == Load#(current,8);
            };
            node main [true][true] {
                let prior: Ptr = create();
                Store#(prior,42,8);
                inspect(prior);
                endprogram;
            };
        "#);
        assert!(plan.regions.contains("inspect"), "{plan:?}");
    }

    #[test]
    fn scalar_field_projection_does_not_retain_an_unrelated_owned_field() {
        let plan = derive(r#"
            let observed: Int = 0;
            obj Record { buffer: Ptr; count: Int; };
            defn inspect() -> Bool {
                let pointer: Ptr = Malloc#(32);
                let record: Record = Record { buffer: pointer, count: 7 };
                observed = record.count;
                Store#(record.buffer,42,8);
                term Load#(pointer,8) == 42;
            };
        "#);
        assert!(plan.regions.contains("inspect"), "{plan:?}");
        assert!(!plan.barriers.contains("inspect"));
    }

    #[test]
    fn inactive_enum_payloads_do_not_escape_through_a_scalar_arm() {
        let plan = derive(r#"
            let observed: Int = 0;
            enum Tagged { Reference(Ptr), Count(Int) };
            defn scalar(value: Tagged) -> Int {
                term match value { Reference(_) => 0, Count(number) => number, };
            };
            defn inspect() -> Bool {
                let pointer: Ptr = Malloc#(32);
                let value: Tagged = Reference(pointer);
                observed = scalar(value);
                Store#(pointer,42,8);
                term Load#(pointer,8) == 42;
            };
        "#);
        assert!(plan.regions.contains("inspect"), "{plan:?}");
        assert!(!plan.barriers.contains("inspect"));
    }
}
