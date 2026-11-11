// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12: HIR mirror of `analysis::reclamation::Graph`. The AST graph is
//! authoritative until a consumer flips; this port keeps the solver and plan
//! shapes identical so the parity gate compares only Builder output.

use super::super::program::HirCallable;
use crate::analysis::reclamation::{Constraint, ReclamationPlan};
use crate::ast::{BinaryOpKind, Type};
use crate::casting::graph::CastingGraph;
use crate::type_universe::TypeUniverse;
use std::collections::{BTreeMap, BTreeSet};

pub(super) type Node = usize;

#[derive(Clone, Default)]
pub(super) struct Callable {
    pub(super) params: Vec<Node>,
    pub(super) result: Node,
    pub(super) calls: BTreeSet<String>,
}

#[derive(Clone)]
pub(super) struct Graph {
    pub(super) universe: TypeUniverse,
    pub(super) casts: CastingGraph,
    pub(super) return_types: BTreeMap<String, Type>,
    pub(super) generics: BTreeMap<String, HirCallable>,
    pub(super) bodies: BTreeMap<String, (Vec<(String, Type)>, Vec<super::super::stmt::HirStmt>)>,
    pub(super) built: BTreeSet<String>,
    pub(super) handle_returns: BTreeSet<String>,
    pub(super) default_encoding_targets: BTreeSet<String>,
    pub(super) entry_nodes: BTreeSet<String>,
    pub(super) user_operators: BTreeSet<String>,
    pub(super) methods: BTreeMap<(String, String), Option<String>>,
    pub(super) points: Vec<NodeSet>,
    pub(super) constants: Vec<BTreeSet<i64>>,
    pub(super) inexact_constants: BTreeSet<Node>,
    pub(super) memory: Vec<NodeSet>,
    pub(super) named_memory: BTreeMap<(Node, String), NodeSet>,
    pub(super) constraints: Vec<Constraint>,
    pub(super) allocation_owner: BTreeMap<Node, String>,
    pub(super) callables: BTreeMap<String, Callable>,
    pub(super) globals: BTreeMap<String, Node>,
    pub(super) global_types: BTreeMap<String, Type>,
    pub(super) constructors: BTreeSet<String>,
    pub(super) variant_types: BTreeMap<String, (Vec<String>, Vec<Type>)>,
    pub(super) escaped: BTreeSet<Node>,
    pub(super) unsupported: BTreeSet<String>,
    pub(super) incoming_store_owners: BTreeSet<String>,
    pub(super) unmodeled_helpers: BTreeSet<String>,
    pub(super) publish_receiver_param: BTreeMap<String, String>,
    pub(super) publish_call_args: BTreeMap<String, Vec<(String, Option<Node>)>>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            universe: TypeUniverse::new(),
            casts: CastingGraph::new(),
            return_types: BTreeMap::new(),
            generics: BTreeMap::new(),
            bodies: BTreeMap::new(),
            built: BTreeSet::new(),
            handle_returns: BTreeSet::new(),
            default_encoding_targets: BTreeSet::new(),
            entry_nodes: BTreeSet::new(),
            user_operators: BTreeSet::new(),
            methods: BTreeMap::new(),
            points: Vec::new(),
            constants: Vec::new(),
            inexact_constants: BTreeSet::new(),
            memory: Vec::new(),
            named_memory: BTreeMap::new(),
            constraints: Vec::new(),
            allocation_owner: BTreeMap::new(),
            callables: BTreeMap::new(),
            globals: BTreeMap::new(),
            global_types: BTreeMap::new(),
            constructors: BTreeSet::new(),
            variant_types: BTreeMap::new(),
            escaped: BTreeSet::new(),
            unsupported: BTreeSet::new(),
            incoming_store_owners: BTreeSet::new(),
            unmodeled_helpers: BTreeSet::new(),
            publish_receiver_param: BTreeMap::new(),
            publish_call_args: BTreeMap::new(),
        }
    }
}

impl Graph {
    pub(super) fn constructor_key(&self, name: &str) -> Option<String> {
        let leaf = name.rsplit('.').next().unwrap_or(name);
        self.constructors.contains(leaf).then(|| leaf.into())
    }

    pub(super) fn node(&mut self) -> Node {
        let node = self.points.len();
        self.points.push(NodeSet::new());
        self.constants.push(BTreeSet::new());
        self.memory.push(NodeSet::new());
        node
    }

    pub(super) fn root(&mut self) -> Node {
        let node = self.node();
        self.points[node].insert(node);
        node
    }

    pub(super) fn unknown_value(&mut self) -> Node {
        let node = self.node();
        self.inexact_constants.insert(node);
        node
    }

    pub(super) fn allocation(&mut self, owner: &str) -> Node {
        let root = self.root();
        self.allocation_owner.insert(root, owner.into());
        root
    }

    pub(super) fn copy(&mut self, source: Node, target: Node) {
        self.constraints.push(Constraint::Copy(source, target));
    }

    pub(super) fn global(&mut self, name: &str) -> Node {
        if let Some(node) = self.globals.get(name) {
            return *node;
        }
        let node = self.root();
        self.inexact_constants.insert(node);
        self.globals.insert(name.into(), node);
        node
    }

    pub(super) fn solve(&mut self) {
        while self.propagate_once() {}
        for constraint in self.constraints.clone() {
            match constraint {
                Constraint::Syscall(number, args) => {
                    self.syscall(number, &args);
                }
                Constraint::EscapeContents(value) => {
                    let refs = self.contents(value);
                    self.escaped.extend(refs.iter());
                }
                _ => {}
            }
        }
    }

    fn propagate_once(&mut self) -> bool {
        let mut changed = false;
        for constraint in self.constraints.clone() {
            changed |= self.apply(constraint);
        }
        changed
    }

    fn apply(&mut self, constraint: Constraint) -> bool {
        match constraint {
            Constraint::Bitwise(operator, left, right, result) => {
                self.bitwise_facts(operator, left, right, result)
            }
            Constraint::Copy(source, target) => {
                let refs = self.points[source].clone();
                let constants = self.constants[source].clone();
                let inexact =
                    self.inexact_constants.contains(&source) && self.inexact_constants.insert(target);
                self.points[target].union_with(&refs)
                    | extend(&mut self.constants[target], constants)
                    | inexact
            }
            Constraint::Load(base, result) => {
                let refs = self.contents(base);
                self.points[result].union_with(&refs) | self.inexact_constants.insert(result)
            }
            Constraint::Store(base, value) => self.store(base, self.points[value].clone()),
            Constraint::CopyMemory(source, target) => self.store(target, self.contents(source)),
            Constraint::Syscall(..) => false,
            Constraint::EscapeContents(_) => false,
            Constraint::FieldLoad(base, field, target) => {
                let refs = self.field_contents(base, &field);
                self.points[target].union_with(&refs) | self.inexact_constants.insert(target)
            }
            Constraint::FieldStore(base, field, value) => {
                let mut changed = false;
                for root in self.points[base].clone().iter() {
                    changed |= self
                        .named_memory
                        .entry((root, field.clone()))
                        .or_default()
                        .union_with(&self.points[value]);
                }
                changed
            }
        }
    }

    fn contents(&self, base: Node) -> NodeSet {
        let mut refs = NodeSet::new();
        for root in self.points[base].iter() {
            refs.union_with(&self.root_contents(root));
        }
        refs
    }

    fn root_contents(&self, root: Node) -> NodeSet {
        let mut refs = self.memory[root].clone();
        for ((owner, _), values) in &self.named_memory {
            if *owner == root {
                refs.union_with(values);
            }
        }
        refs
    }

    fn field_contents(&self, base: Node, field: &str) -> NodeSet {
        let mut refs = NodeSet::new();
        for root in self.points[base].iter() {
            refs.union_with(&self.memory[root]);
            if let Some(values) = self.named_memory.get(&(root, field.into())) {
                refs.union_with(values);
            }
        }
        refs
    }

    fn store(&mut self, base: Node, refs: NodeSet) -> bool {
        let mut changed = false;
        for root in self.points[base].clone().iter() {
            changed |= self.memory[root].union_with(&refs);
        }
        changed
    }

    fn syscall(&mut self, number: Node, args: &[Node]) -> bool {
        let numbers = &self.constants[number];
        let borrowed = !self.inexact_constants.contains(&number)
            && !numbers.is_empty()
            && numbers
                .iter()
                .all(|number| self.syscall_borrows(*number, args));
        let mut refs = NodeSet::new();
        for arg in args {
            let values = if borrowed {
                self.contents(*arg)
            } else {
                self.points[*arg].clone()
            };
            refs.union_with(&values);
        }
        extend(&mut self.escaped, refs.iter())
    }

    pub(super) fn reachable(&self, roots: impl IntoIterator<Item = Node>) -> BTreeSet<Node> {
        let mut found = BTreeSet::new();
        let mut pending: Vec<_> = roots.into_iter().collect();
        while let Some(root) = pending.pop() {
            if found.insert(root) {
                pending.extend(self.root_contents(root).iter());
            }
        }
        found
    }

    pub(super) fn plan(&self) -> ReclamationPlan {
        let global_roots = self.globals.values().flat_map(|node| self.points[*node].iter());
        let escaped = self.reachable(global_roots.chain(self.escaped.iter().copied()));
        let mut plan = ReclamationPlan {
            known: self.callables.keys().cloned().collect(),
            ..Default::default()
        };
        for root in &escaped {
            if let Some(owner) = self.allocation_owner.get(root) {
                plan.escape_barriers.insert(owner.clone());
            }
        }
        plan.escape_barriers.extend(self.unsupported.iter().cloned());
        let publishers: BTreeSet<String> = self.incoming_store_owners.iter().cloned().collect();
        plan.publish_only = publishers.difference(&plan.escape_barriers).cloned().collect();
        plan.barriers = plan.escape_barriers.clone();
        plan.barriers.extend(publishers.iter().cloned());
        for (name, callable) in &self.callables {
            self.classify(name, callable, &mut plan);
        }
        plan.publish_sites = self.publish_safety();
        plan
    }

    fn publish_safety(&self) -> BTreeMap<String, Vec<bool>> {
        self.publish_call_args
            .iter()
            .map(|(publisher, sites)| {
                let safety = sites
                    .iter()
                    .map(|(owner, local)| self.site_is_safe(owner, *local))
                    .collect();
                (publisher.clone(), safety)
            })
            .collect()
    }

    fn site_is_safe(&self, owner: &str, local: Option<Node>) -> bool {
        let Some(local) = local else { return false };
        let global_roots = self.globals.values().flat_map(|node| self.points[*node].iter());
        if self
            .reachable(global_roots.chain(self.escaped.iter().copied()))
            .contains(&local)
        {
            return false;
        }
        let Some(callable) = self.callables.get(owner) else { return false };
        !self
            .reachable(self.points[callable.result].iter())
            .contains(&local)
    }

    fn classify(&self, name: &str, callable: &Callable, plan: &mut ReclamationPlan) {
        if plan.barriers.contains(name) {
            return;
        }
        if callable
            .calls
            .iter()
            .any(|callee| self.call_closure(callee, &BTreeSet::new()).contains(name))
        {
            plan.reasons.insert(
                name.into(),
                "recursive region needs an iteration-boundary consumer".into(),
            );
            return;
        }
        if self.handle_returns.contains(name) {
            plan.reasons
                .insert(name.into(), "result image or payload can be caller-owned".into());
            return;
        }
        if !self
            .call_closure(name, &BTreeSet::new())
            .is_disjoint(&self.unsupported)
        {
            plan.reasons
                .insert(name.into(), "callee effects are not classified".into());
            return;
        }
        let owners = self.call_closure(name, &plan.barriers);
        let allocations: BTreeSet<_> = self
            .allocation_owner
            .iter()
            .filter(|(_, owner)| owners.contains(*owner))
            .map(|(root, _)| *root)
            .collect();
        if allocations.is_empty() {
            return;
        }
        let returned = self.reachable(self.points[callable.result].iter());
        if !returned.is_disjoint(&allocations) {
            plan.reasons
                .insert(name.into(), "allocation reaches the returned value".into());
            return;
        }
        let incoming = callable
            .params
            .iter()
            .flat_map(|param| self.points[*param].iter());
        let retained = self.reachable(incoming);
        if !retained.is_disjoint(&allocations) {
            let owners: BTreeSet<_> = retained
                .intersection(&allocations)
                .filter_map(|root| self.allocation_owner.get(root))
                .collect();
            if owners.iter().any(|owner| owner.as_str() == name) {
                plan.barriers.insert(name.into());
            }
            plan.reasons.insert(
                name.into(),
                format!("allocation may survive in incoming storage: {owners:?}"),
            );
            return;
        }
        plan.regions.insert(name.into());
    }

    pub(super) fn call_closure(&self, name: &str, barriers: &BTreeSet<String>) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut pending = vec![name.to_string()];
        while let Some(name) = pending.pop() {
            if barriers.contains(&name) || !found.insert(name.clone()) {
                continue;
            }
            if let Some(callable) = self.callables.get(&name) {
                pending.extend(callable.calls.iter().cloned());
            }
        }
        found
    }

    /// Platform-configured synchronous borrowing (mirror of the AST table).
    pub(super) fn syscall_borrows(&self, number: i64, args: &[Node]) -> bool {
        let table = syscall_effects();
        if table.synchronous_borrowing.contains(&number) {
            return true;
        }
        table.conditional_borrowing.iter().any(|rule| {
            let Some(argument) = args.get(rule.argument) else { return false };
            rule.number == number
                && !self.inexact_constants.contains(argument)
                && !self.constants[*argument].is_empty()
                && self.constants[*argument]
                    .iter()
                    .all(|value| value & rule.absent_mask == 0)
        })
    }

    /// Mirror of `analysis::reclamation::socket_effects::bitwise_facts`.
    pub(super) fn bitwise_facts(
        &mut self,
        operator: BinaryOpKind,
        left: Node,
        right: Node,
        result: Node,
    ) -> bool {
        let before = (
            self.points[result].len(),
            self.constants[result].len(),
            self.inexact_constants.contains(&result),
        );
        let mut references = self.points[left].clone();
        references.union_with(&self.points[right]);
        self.points[result].union_with(&references);
        if self.inexact_constants.contains(&left) || self.inexact_constants.contains(&right) {
            self.inexact_constants.insert(result);
        }
        self.extend_bitwise_constants(operator, left, right, result);
        before
            != (
                self.points[result].len(),
                self.constants[result].len(),
                self.inexact_constants.contains(&result),
            )
    }

    fn extend_bitwise_constants(
        &mut self,
        operator: BinaryOpKind,
        left: Node,
        right: Node,
        result: Node,
    ) {
        if self.inexact_constants.contains(&result) {
            return;
        }
        let values: BTreeSet<_> = self.constants[left]
            .iter()
            .flat_map(|lhs| {
                self.constants[right]
                    .iter()
                    .map(move |rhs| bitwise(operator, *lhs, *rhs))
            })
            .collect();
        if values.len() + self.constants[result].len() > 64 {
            self.inexact_constants.insert(result);
            return;
        }
        self.constants[result].extend(values);
    }
}

#[derive(serde::Deserialize)]
struct Effects {
    synchronous_borrowing: BTreeSet<i64>,
    #[serde(default)]
    conditional_borrowing: Vec<Conditional>,
}

#[derive(serde::Deserialize)]
struct Conditional {
    number: i64,
    argument: usize,
    absent_mask: i64,
}

fn syscall_effects() -> &'static Effects {
    static TABLE: std::sync::OnceLock<Effects> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let config: toml::Value =
            toml::from_str(include_str!("../../../config/memory-effects.toml"))
                .expect("bundled memory-effect table");
        config["linux_x86_64"]
            .clone()
            .try_into()
            .expect("platform memory effects")
    })
}

fn bitwise(operator: BinaryOpKind, lhs: i64, rhs: i64) -> i64 {
    match operator {
        BinaryOpKind::BitAnd => lhs & rhs,
        BinaryOpKind::BitOr => lhs | rhs,
        BinaryOpKind::BitXor => lhs ^ rhs,
        _ => panic!("bitwise constraint requires a bitwise operator"),
    }
}

pub(super) fn extend<T: Ord>(target: &mut BTreeSet<T>, values: impl IntoIterator<Item = T>) -> bool {
    let before = target.len();
    target.extend(values);
    target.len() != before
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NodeSet {
    words: BTreeMap<usize, u64>,
    len: usize,
}

impl NodeSet {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn clear(&mut self) {
        self.words.clear();
        self.len = 0;
    }

    pub(super) fn insert(&mut self, node: usize) -> bool {
        let word = self.words.entry(node / 64).or_default();
        let added = merge_word(word, 1u64 << (node % 64));
        self.len += added;
        added != 0
    }

    pub(super) fn union_with(&mut self, other: &Self) -> bool {
        let before = self.len;
        for (index, bits) in &other.words {
            self.len += merge_word(self.words.entry(*index).or_default(), *bits);
        }
        before != self.len
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words
            .iter()
            .flat_map(|(&index, &bits)| WordNodes { base: index * 64, bits })
    }
}

fn merge_word(target: &mut u64, source: u64) -> usize {
    let added = (source & !*target).count_ones() as usize;
    *target |= source;
    added
}

struct WordNodes {
    base: usize,
    bits: u64,
}

impl Iterator for WordNodes {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bits == 0 {
            return None;
        }
        let node = self.base + self.bits.trailing_zeros() as usize;
        self.bits &= self.bits - 1;
        Some(node)
    }
}
