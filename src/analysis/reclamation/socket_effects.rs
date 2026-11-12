//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Platform-configured synchronous borrowing, including flag-dependent calls.
use super::{Graph, Node};
use crate::ast::BinaryOpKind;
use std::collections::BTreeSet;

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

fn effects() -> &'static Effects {
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

impl Graph {
    pub(super) fn syscall_borrows(&self, number: i64, args: &[Node]) -> bool {
        let table = effects();
        if table.synchronous_borrowing.contains(&number) {
            return true;
        }
        table.conditional_borrowing.iter().any(|rule| {
            let Some(argument) = args.get(rule.argument) else {
                return false;
            };
            rule.number == number
                && !self.inexact_constants.contains(argument)
                && !self.constants[*argument].is_empty()
                && self.constants[*argument]
                    .iter()
                    .all(|value| value & rule.absent_mask == 0)
        })
    }

    /// 2026-11-05: prove composed flags without dropping pointer provenance.
    /// Bound the lattice so loops cannot grow it indefinitely; saturation is
    /// unknown, never a proof. Retire with the general scalar fact engine.
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

fn bitwise(operator: BinaryOpKind, lhs: i64, rhs: i64) -> i64 {
    match operator {
        BinaryOpKind::BitAnd => lhs & rhs,
        BinaryOpKind::BitOr => lhs | rhs,
        BinaryOpKind::BitXor => lhs ^ rhs,
        _ => panic!("bitwise constraint requires a bitwise operator"),
    }
}

#[cfg(all(feature = "kani", feature = "kani_full"))]
mod proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(128)]
    fn retained_or_unknown_send_flags_never_license_reclamation() {
        let mut graph = Graph::default();
        let flags: i64 = kani::any();
        let unknown: bool = kani::any();
        let node = graph.node();
        graph.constants[node].insert(flags);
        if unknown {
            graph.inexact_constants.insert(node);
        }
        if graph.syscall_borrows(44, &[node; 4]) {
            assert!(!unknown && flags & 67108864 == 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_output_is_borrowed_even_with_dynamic_flags() {
        let mut graph = Graph::default();
        let unknown = graph.unknown_value();
        assert!(graph.syscall_borrows(318, &[unknown; 3]));
        assert!(!graph.syscall_borrows(999999, &[unknown; 3]));
        let source = r#"
            defn entropy(flags: Int) -> Int {
                let buffer: Ptr = Malloc#(16);
                term SysCall#(318, buffer, 16, flags);
            };
            defn unclassified(flags: Int) -> Int {
                let buffer: Ptr = Malloc#(16);
                term SysCall#(999999, buffer, 16, flags);
            };
        "#;
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source)
            .parse_program()
            .unwrap();
        let live = ["entropy", "unclassified"]
            .map(String::from)
            .into_iter()
            .collect();
        let plan = super::super::analyze(&items, &live, None);
        assert!(plan.regions.contains("entropy"), "{plan:?}");
        assert!(plan.barriers.contains("unclassified"), "{plan:?}");
    }

    fn constant(graph: &mut Graph, value: i64) -> Node {
        let node = graph.node();
        graph.constants[node].insert(value);
        node
    }

    #[test]
    fn borrowing_requires_complete_copying_flags() {
        let mut graph = Graph::default();
        let ordinary = constant(&mut graph, 64 | 16384);
        let zerocopy = constant(&mut graph, 67108864);
        let unknown = graph.unknown_value();
        assert!(graph.syscall_borrows(44, &[ordinary; 4]));
        assert!(!graph.syscall_borrows(44, &[zerocopy; 4]));
        assert!(!graph.syscall_borrows(44, &[unknown; 4]));
        assert!(!graph.syscall_borrows(44, &[]));
        graph.constants[ordinary].insert(67108864);
        assert!(!graph.syscall_borrows(44, &[ordinary; 4]));
        assert!(graph.syscall_borrows(53, &[]));
        assert!(!graph.syscall_borrows(46, &[]));
    }

    #[test]
    fn call_graph_proves_copying_sends_but_not_retained_or_dynamic_sends() {
        let source = r#"
            const NONBLOCK: Int = 64;
            const NOSIGNAL: Int = 16384;
            defn send_copy() -> Int {
                let bytes: Ptr = Malloc#(32);
                term SysCall#(44,-1,bytes,32,NONBLOCK | NOSIGNAL);
            };
            defn send_retained() -> Int {
                let bytes: Ptr = Malloc#(32);
                term SysCall#(44,-1,bytes,32,67108864 | NOSIGNAL);
            };
            defn send_dynamic(flags: Int) -> Int {
                let bytes: Ptr = Malloc#(32);
                term SysCall#(44,-1,bytes,32,flags | NOSIGNAL);
            };
        "#;
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source)
            .parse_program()
            .unwrap();
        let live = ["send_copy", "send_retained", "send_dynamic"]
            .map(String::from)
            .into_iter()
            .collect();
        let plan = super::super::analyze(&items, &live, None);
        assert!(plan.regions.contains("send_copy"), "{plan:?}");
        assert!(!plan.regions.contains("send_retained"), "{plan:?}");
        assert!(!plan.regions.contains("send_dynamic"), "{plan:?}");
    }

    #[test]
    fn composed_flags_retain_pointer_provenance_and_saturate_safely() {
        let mut graph = Graph::default();
        let left = constant(&mut graph, 64);
        let right = constant(&mut graph, 16384);
        let result = graph.node();
        graph.points[left].insert(left);
        assert!(graph.bitwise_facts(BinaryOpKind::BitOr, left, right, result));
        assert_eq!(graph.constants[result], [16448].into_iter().collect());
        assert!(graph.points[result].contains(&left));
        assert!(graph.syscall_borrows(44, &[result; 4]));
        graph.inexact_constants.insert(right);
        graph.bitwise_facts(BinaryOpKind::BitOr, left, right, result);
        assert!(!graph.syscall_borrows(44, &[result; 4]));
        let broad = graph.node();
        graph.constants[broad].extend(0..65);
        let capped = graph.node();
        graph.bitwise_facts(BinaryOpKind::BitXor, broad, left, capped);
        assert!(graph.inexact_constants.contains(&capped));
        assert!(!graph.syscall_borrows(44, &[capped; 4]));
        assert_eq!(bitwise(BinaryOpKind::BitAnd, 7, 3), 3);
    }

    #[test]
    fn explicit_receiver_methods_preserve_escape_and_shadowing() {
        let source = r#"
            obj Packet {
                storage: Ptr;
                defn read(packet: Packet) -> Int { term Load#(packet.storage,8); };
                defn keep(packet: Packet) -> Ptr { term packet.storage; };
                defn hidden() -> Ptr { term storage; };
            };
            defn hidden(packet: Packet) -> Int { term 0; };
            defn inspect() -> Int {
                let packet: Packet = Packet { storage: Malloc#(8) };
                term packet.read();
            };
            defn escaping() -> Ptr {
                let packet: Packet = Packet { storage: Malloc#(8) };
                term packet.keep();
            };
            defn shadowed() -> Ptr {
                let packet: Packet = Packet { storage: Malloc#(8) };
                term packet.hidden();
            };
        "#;
        let tokens = crate::lexer::tokenize(source).unwrap();
        let items = crate::parser::Parser::new(tokens, source)
            .parse_program()
            .unwrap();
        let live = ["hidden", "inspect", "escaping", "shadowed"]
            .map(String::from)
            .into_iter()
            .collect();
        let plan = super::super::analyze(&items, &live, None);
        assert!(plan.regions.contains("inspect"), "{plan:?}");
        assert!(!plan.regions.contains("escaping"), "{plan:?}");
        assert!(plan.barriers.contains("shadowed"), "{plan:?}");
    }
}
