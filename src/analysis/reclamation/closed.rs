//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! 2026-11-11: a proven region may also own the allocations its call closure
//! makes beneath descendant barriers when none of those allocations can escape
//! the region. This is the whole-call companion to the existing region proof:
//! the ordinary proof stops the closure at a barrier; this one follows every
//! call and checks the barrier-callee allocations too. Undo by dropping
//! closed-region selection; ordinary barriers and region proofs are unchanged.
use super::{Graph, Node, ReclamationPlan};
use std::collections::BTreeSet;

impl Graph {
    pub(super) fn closed_regions(&self, plan: &ReclamationPlan) -> BTreeSet<String> {
        plan.regions.iter().filter(|name| self.is_closed_region(name, plan)).cloned().collect()
    }

    /// A region is closed when every allocation owned by its complete call
    /// closure is unreachable from the region's return, incoming storage,
    /// global storage and external escape roots. The closure must include a
    /// barrier (otherwise the ordinary region proof already suffices) and must
    /// not depend on an intrinsic whose helper effects are unmodeled.
    fn is_closed_region(&self, name: &str, plan: &ReclamationPlan) -> bool {
        let closure = self.call_closure(name, &BTreeSet::new());
        if closure.is_disjoint(&plan.barriers) && closure.is_disjoint(&self.growers) { return false; }
        if !closure.is_disjoint(&self.unmodeled_helpers) { return false; }
        let allocations: BTreeSet<_> = self.allocation_owner.iter()
            .filter(|(_, owner)| closure.contains(*owner)).map(|(root, _)| *root).collect();
        if allocations.is_empty() { return false; }
        let global_roots = self.globals.values().flat_map(|node| self.points[*node].iter());
        let escaped = self.reachable(global_roots.chain(self.escaped.iter().copied()));
        if !allocations.is_disjoint(&escaped) { return false; }
        let callable = &self.callables[name];
        let incoming = callable.params.iter().flat_map(|param| self.points[*param].iter());
        let roots = incoming.chain(self.points[callable.result].iter());
        self.reachable(roots).is_disjoint(&allocations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::reclamation::{Callable, Constraint};

    /// A region whose helper barrier publishes into a local container: the
    /// helper allocation is local to the enclosing call, so it is closed.
    fn local_publication() -> (Graph, Node, Node, Node) {
        let mut graph = Graph::default();
        let incoming = graph.root();
        let result = graph.node();
        let container = graph.allocation("outer");
        let payload = graph.allocation("grow");
        graph.callables.insert("outer".into(), Callable {
            params: vec![incoming], result, calls: BTreeSet::from(["grow".into()]),
        });
        let helper_result = graph.node();
        graph.callables.insert("grow".into(), Callable {
            params: vec![container], result: helper_result, calls: BTreeSet::new(),
        });
        graph.incoming_store_owners.insert("grow".into());
        graph.constraints.push(Constraint::Store(container, payload));
        (graph, incoming, result, payload)
    }

    #[test]
    fn local_publication_keeps_the_helper_barrier_and_closes_the_enclosing_call() {
        let (mut graph, _, _, _) = local_publication();
        graph.solve();
        let plan = graph.plan();
        assert!(plan.regions.contains("outer"));
        assert!(plan.closed_regions.contains("outer"));
        assert!(plan.barriers.contains("grow"));
        assert!(!plan.closed_regions.contains("grow"));
    }

    #[test]
    fn descendant_allocations_cannot_escape_any_enclosing_boundary() {
        for escape in 0..4 {
            let (mut graph, incoming, result, payload) = local_publication();
            match escape {
                0 => graph.constraints.push(Constraint::FieldStore(incoming, "saved".into(), payload)),
                1 => graph.copy(payload, result),
                2 => { let global = graph.global("saved"); graph.constraints.push(Constraint::Store(global, payload)); }
                _ => { graph.escaped.insert(payload); }
            }
            graph.solve();
            assert!(!graph.plan().closed_regions.contains("outer"), "escape {escape}");
        }
    }

    #[test]
    fn unsupported_helper_bodies_and_recursive_closures_are_not_closed() {
        let (mut graph, _, _, _) = local_publication();
        graph.unmodeled_helpers.insert("grow".into());
        graph.solve();
        assert!(!graph.plan().closed_regions.contains("outer"));
        graph.unmodeled_helpers.clear();
        graph.callables.get_mut("grow").unwrap().calls.insert("outer".into());
        assert!(!graph.plan().closed_regions.contains("outer"));
    }

    #[test]
    fn publication_through_nested_incoming_storage_cannot_close_the_caller() {
        let (mut graph, incoming, _, payload) = local_publication();
        graph.external_storage(incoming);
        let member = graph.node();
        let address = graph.node();
        graph.constraints.push(Constraint::FieldLoad(incoming, "storage".into(), member));
        graph.constraints.push(Constraint::Load(member, address));
        graph.constraints.push(Constraint::Store(address, payload));
        graph.solve();
        assert!(!graph.plan().closed_regions.contains("outer"));
    }
}
