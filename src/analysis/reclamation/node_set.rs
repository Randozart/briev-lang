//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! 2026-11-09: exact, sparse word sets for dense analysis node identifiers.
//! Repeated points-to unions must not insert each individual fact into a tree.
//! Ordered word keys retain deterministic traversal without allocating space for
//! absent high identifiers. Replace only with an equally exact set abstraction.

use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NodeSet {
    words: BTreeMap<usize, u64>,
    len: usize,
}

impl NodeSet {
    pub(super) fn new() -> Self { Self::default() }

    pub(super) fn len(&self) -> usize { self.len }

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
        self.words.iter().flat_map(|(&index, &bits)| WordNodes { base: index * 64, bits })
    }

    #[cfg(test)]
    pub(super) fn contains(&self, node: &usize) -> bool {
        self.words.get(&(node / 64)).is_some_and(|bits| bits & (1u64 << (node % 64)) != 0)
    }
}

fn merge_word(target: &mut u64, source: u64) -> usize {
    let added = (source & !*target).count_ones() as usize;
    *target |= source;
    added
}

struct WordNodes { base: usize, bits: u64 }

impl Iterator for WordNodes {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bits == 0 { return None; }
        let node = self.base + self.bits.trailing_zeros() as usize;
        self.bits &= self.bits - 1;
        Some(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn sparse_unions_match_ordered_sets_including_word_boundaries() {
        let universe = [0, 1, 63, 64, 65, 127, 128, usize::MAX];
        for mask in 0u16..256 {
            check_union(&universe, mask);
        }
    }

    fn check_union(universe: &[usize], mask: u16) {
        let mut first = NodeSet::new();
        let mut second = NodeSet::new();
        let mut expected = BTreeSet::new();
        for (index, &node) in universe.iter().enumerate() {
            let target = if mask & (1 << index) != 0 { &mut first } else { &mut second };
            assert!(target.insert(node));
            assert!(!target.insert(node));
            expected.insert(node);
        }
        assert_eq!(first.union_with(&second), second.len() != 0);
        assert!(!first.union_with(&second));
        assert_eq!(first.len(), expected.len());
        assert_eq!(first.iter().collect::<Vec<_>>(), expected.into_iter().collect::<Vec<_>>());
        first.clear();
        assert_eq!(first.len(), 0);
        assert_eq!(first.iter().next(), None);
    }

    #[test]
    fn overlapping_words_preserve_membership_and_exact_change_flag() {
        let mut first = NodeSet::new();
        let mut second = NodeSet::new();
        for node in 0..130 { first.insert(node); }
        for node in 60..190 { second.insert(node); }
        assert!(first.union_with(&second));
        assert_eq!(first.len(), 190);
        assert!(first.iter().eq(0..190));
        assert!(!first.contains(&190));
        let snapshot = first.clone();
        assert!(!first.union_with(&snapshot));
    }
}

#[cfg(all(feature = "kani", feature = "kani_full"))]
mod proofs {
    use super::*;

    #[kani::proof]
    fn word_union_preserves_all_facts_and_reports_exact_growth() {
        let before: u64 = kani::any();
        let source: u64 = kani::any();
        let mut after = before;
        let added = merge_word(&mut after, source);
        assert_eq!(after & before, before);
        assert_eq!(after & source, source);
        assert_eq!(after, before | source);
        assert_eq!(added, (after.count_ones() - before.count_ones()) as usize);
        assert_eq!(added == 0, after == before);
    }

    #[kani::proof]
    #[kani::unwind(3)]
    fn sparse_iteration_preserves_node_ids_and_order() {
        let node: usize = kani::any();
        let offset: u8 = kani::any();
        kani::assume(offset < 64);
        let base = (node / 64) * 64;
        let source = (1u64 << (node % 64)) | (1u64 << offset);
        let mut actual = 0u64;
        let mut previous = None;
        for found in (WordNodes { base, bits: source }) {
            assert_eq!(found / 64, node / 64);
            if let Some(last) = previous { assert!(found > last); }
            previous = Some(found);
            actual |= 1u64 << (found % 64);
        }
        assert_eq!(source, actual);
    }
}
