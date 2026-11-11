//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Shared memory facts, independent of type/casting registries and OS entropy.
//! 2026-11-11: extracted for compositional ownership proofs after whole-Graph
//! construction exhausted CBMC in unrelated HashMap seed initialization.
//! Keep production and proof consumers on these same operations.
use super::{Node, NodeSet};
use std::collections::BTreeMap;

pub(super) fn seed_external(memory: &mut [NodeSet], root: Node) {
    memory[root].insert(root);
}

pub(super) fn root_contents(memory: &[NodeSet], named: &BTreeMap<(Node, String), NodeSet>, root: Node) -> NodeSet {
    let mut refs = memory[root].clone();
    for ((owner, _), values) in named { if *owner == root { refs.union_with(values); } }
    refs
}

pub(super) fn field_contents(points: &NodeSet, memory: &[NodeSet], named: &BTreeMap<(Node, String), NodeSet>, field: &str) -> NodeSet {
    let mut refs = NodeSet::new();
    for root in points.iter() {
        refs.union_with(&memory[root]);
        if let Some(values) = named.get(&(root, field.into())) { refs.union_with(values); }
    }
    refs
}

pub(super) fn contents(points: &NodeSet, memory: &[NodeSet], named: &BTreeMap<(Node, String), NodeSet>) -> NodeSet {
    let mut refs = NodeSet::new();
    for root in points.iter() { refs.union_with(&root_contents(memory, named, root)); }
    refs
}

pub(super) fn store(points: &NodeSet, memory: &mut [NodeSet], refs: &NodeSet) -> bool {
    let mut changed = false;
    for root in points.iter() { changed |= memory[root].union_with(refs); }
    changed
}

#[cfg(all(feature = "kani", feature = "kani_full"))]
mod proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(16)]
    fn external_storage_publication_remains_boundary_reachable() {
        let mut memory = vec![NodeSet::new(); 2];
        let named = BTreeMap::new();
        let mut incoming = NodeSet::new();
        incoming.insert(0);
        seed_external(&mut memory, 0);
        let member = if kani::any::<bool>() {
            field_contents(&incoming, &memory, &named, "slot")
        } else {
            contents(&incoming, &memory, &named)
        };
        let address = contents(&member, &memory, &named);
        let mut payload = NodeSet::new();
        payload.insert(1);
        store(&address, &mut memory, &payload);
        assert!(root_contents(&memory, &named, 0).iter().any(|root| root == 1));
    }
}
