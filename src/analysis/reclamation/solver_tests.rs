//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Differential oracle: the original element-wise ordered-set transfer rules.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct TreeFacts {
    points: Vec<BTreeSet<Node>>,
    memory: Vec<BTreeSet<Node>>,
    named: BTreeMap<(Node, String), BTreeSet<Node>>,
}

impl TreeFacts {
    fn capture(graph: &Graph) -> Self {
        Self {
            points: graph
                .points
                .iter()
                .map(|set| set.iter().collect())
                .collect(),
            memory: graph
                .memory
                .iter()
                .map(|set| set.iter().collect())
                .collect(),
            named: graph
                .named_memory
                .iter()
                .map(|(key, set)| (key.clone(), set.iter().collect()))
                .collect(),
        }
    }

    fn read(&self, base: Node, field: Option<&str>) -> BTreeSet<Node> {
        let mut result = BTreeSet::new();
        for root in &self.points[base] {
            result.extend(&self.memory[*root]);
            result.extend(self.named_values(*root, field));
        }
        result
    }

    fn named_values(&self, root: Node, field: Option<&str>) -> BTreeSet<Node> {
        self.named
            .iter()
            .filter(|((owner, name), _)| *owner == root && field.is_none_or(|field| field == name))
            .flat_map(|(_, values)| values.iter().copied())
            .collect()
    }

    fn store(&mut self, base: Node, field: Option<&str>, values: BTreeSet<Node>) {
        for root in &self.points[base] {
            let target = match field {
                Some(field) => self.named.entry((*root, field.into())).or_default(),
                None => &mut self.memory[*root],
            };
            target.extend(&values);
        }
    }

    fn apply(&mut self, constraint: &Constraint) {
        match constraint {
            Constraint::Copy(source, target) => {
                let values = self.points[*source].clone();
                self.points[*target].extend(values);
            }
            Constraint::Bitwise(_, left, right, target) => {
                let values = self.points[*left]
                    .union(&self.points[*right])
                    .copied()
                    .collect::<Vec<_>>();
                self.points[*target].extend(values);
            }
            Constraint::Load(base, target) => {
                let values = self.read(*base, None);
                self.points[*target].extend(values);
            }
            Constraint::FieldLoad(base, field, target) => {
                let values = self.read(*base, Some(field));
                self.points[*target].extend(values);
            }
            Constraint::Store(base, value) => self.store(*base, None, self.points[*value].clone()),
            Constraint::FieldStore(base, field, value) => {
                self.store(*base, Some(field), self.points[*value].clone())
            }
            Constraint::CopyMemory(source, target) => {
                self.store(*target, None, self.read(*source, None))
            }
            Constraint::EscapeContents(_) | Constraint::Syscall(..) => {}
        }
    }

    fn solve(&mut self, constraints: &[Constraint]) {
        loop {
            let before = self.clone();
            self.propagate(constraints);
            if *self == before {
                return;
            }
        }
    }

    fn propagate(&mut self, constraints: &[Constraint]) {
        for constraint in constraints {
            self.apply(constraint);
        }
    }
}

#[test]
fn cyclic_memory_constraints_match_elementwise_fixed_point() {
    for seed in 0..24 {
        let mut graph = cyclic_graph(seed);
        let mut reference = TreeFacts::capture(&graph);
        reference.solve(&graph.constraints);
        graph.solve();
        assert_eq!(TreeFacts::capture(&graph), reference, "seed {seed}");
        let fixed = TreeFacts::capture(&graph);
        assert!(!graph.propagate_once());
        assert_eq!(TreeFacts::capture(&graph), fixed);
    }
}

fn cyclic_graph(mut seed: usize) -> Graph {
    let mut graph = Graph::default();
    for _ in 0..130 {
        graph.root();
    }
    for index in 0..260 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let first = seed % 130;
        let second = (seed / 131) % 130;
        let third = (seed / 17033) % 130;
        let field = format!("field{}", index % 3);
        graph.constraints.push(match index % 8 {
            0 => Constraint::Copy(first, second),
            1 => Constraint::Store(first, second),
            2 => Constraint::Load(first, second),
            3 => Constraint::FieldStore(first, field, second),
            4 => Constraint::FieldLoad(first, field, second),
            5 => Constraint::CopyMemory(first, second),
            6 => Constraint::Bitwise(crate::ast::BinaryOpKind::BitOr, first, second, third),
            _ => Constraint::Copy(second, first),
        });
    }
    graph
}
