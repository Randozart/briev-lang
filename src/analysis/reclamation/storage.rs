//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Shared memory facts, independent of type/casting registries and OS entropy.
//! 2026-11-11: extracted for compositional ownership proofs after whole-Graph
//! construction exhausted CBMC in unrelated HashMap seed initialization.
//! Keep production and proof consumers on these same operations.
use super::{Node, NodeSet};
use std::collections::BTreeMap;

pub(super) trait Facts: Clone {
    fn empty() -> Self;
    fn insert(&mut self, root: Node);
    fn union_with(&mut self, other: &Self) -> bool;
    fn iter(&self) -> impl Iterator<Item = Node>;
}

impl Facts for NodeSet {
    fn empty() -> Self { NodeSet::new() }
    fn insert(&mut self, root: Node) { NodeSet::insert(self, root); }
    fn union_with(&mut self, other: &Self) -> bool { NodeSet::union_with(self, other) }
    fn iter(&self) -> impl Iterator<Item = Node> { NodeSet::iter(self) }
}

pub(super) fn seed_external<S: Facts>(memory: &mut [S], root: Node) {
    memory[root].insert(root);
}

pub(super) fn root_contents<S: Facts>(memory: &[S], named: &BTreeMap<(Node, String), S>, root: Node) -> S {
    let mut refs = memory[root].clone();
    for ((owner, _), values) in named { if *owner == root { refs.union_with(values); } }
    refs
}

pub(super) fn field_contents<S: Facts>(points: &S, memory: &[S], named: &BTreeMap<(Node, String), S>, field: &str) -> S {
    let mut refs = S::empty();
    for root in points.iter() {
        refs.union_with(&memory[root]);
        if let Some(values) = named.get(&(root, field.into())) { refs.union_with(values); }
    }
    refs
}

pub(super) fn contents<S: Facts>(points: &S, memory: &[S], named: &BTreeMap<(Node, String), S>) -> S {
    let mut refs = S::empty();
    for root in points.iter() { refs.union_with(&root_contents(memory, named, root)); }
    refs
}

pub(super) fn store<S: Facts>(points: &S, memory: &mut [S], refs: &S) -> bool {
    let mut changed = false;
    for root in points.iter() { changed |= memory[root].union_with(refs); }
    changed
}

#[cfg(all(feature = "kani", feature = "kani_full"))]
mod proofs {
    use super::*;
    use super::super::node_set::{merge_word, WordNodes};

    // Explicit bounded set model, not a substitute implementation of the
    // storage rules. Union and iteration use the production word primitives;
    // NodeSet's sparse-map integration is tested against ordered sets.
    #[derive(Clone)]
    struct WordFacts(u64);

    impl Facts for WordFacts {
        fn empty() -> Self { Self(0) }
        fn insert(&mut self, root: Node) {
            assert!(root < 64);
            merge_word(&mut self.0, 1u64 << root);
        }
        fn union_with(&mut self, other: &Self) -> bool { merge_word(&mut self.0, other.0) != 0 }
        fn iter(&self) -> impl Iterator<Item = Node> { WordNodes { base: 0, bits: self.0 } }
    }

    #[kani::proof]
    // Two roots, one bitset word, and no named fields need only short loops.
    // Unwinding assertions remain enabled: an insufficient bound is a failure.
    #[kani::unwind(4)]
    fn external_storage_publication_remains_boundary_reachable() {
        let mut memory = [WordFacts::empty(), WordFacts::empty()];
        let named = BTreeMap::new();
        let mut incoming = WordFacts::empty();
        incoming.insert(0);
        seed_external(&mut memory, 0);
        let member = if kani::any::<bool>() {
            field_contents(&incoming, &memory, &named, "x")
        } else {
            contents(&incoming, &memory, &named)
        };
        let address = contents(&member, &memory, &named);
        let mut payload = WordFacts::empty();
        payload.insert(1);
        store(&address, &mut memory, &payload);
        assert!(root_contents(&memory, &named, 0).iter().any(|root| root == 1));
    }
}
