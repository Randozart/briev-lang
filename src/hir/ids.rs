// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Stable HIR identity types. IDs are assigned once in source order.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub u32);

pub use crate::errors::Span;

/// Monotonic ID source for one HIR lowering. Never reuse an ID across items.
#[derive(Debug, Default)]
pub struct IdSupply {
    next_node: u32,
    next_item: u32,
    next_def: u32,
    next_value: u32,
}

impl IdSupply {
    pub fn node(&mut self) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node += 1;
        id
    }

    pub fn item(&mut self) -> ItemId {
        let id = ItemId(self.next_item);
        self.next_item += 1;
        id
    }

    pub fn def(&mut self) -> DefId {
        let id = DefId(self.next_def);
        self.next_def += 1;
        id
    }

    pub fn value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }
}
