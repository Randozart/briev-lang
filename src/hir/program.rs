// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0a typed-HIR skeleton: callable/export metadata with stable IDs.
//!
//! Body expressions/statements are represented by a counted placeholder until
//! H0b lowers them. No analysis or backend consumes this program yet.

use super::ids::{DefId, ItemId, NodeId, Span};
use crate::ast::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirCallableKind {
    Defn,
    Transaction,
    MemberDefn,
    MemberTransaction,
    CompileTimeDefn,
    CompileTimeTxn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirParam {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirBodySkeleton {
    pub node: NodeId,
    pub span: Option<Span>,
    pub statement_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirCallable {
    pub id: DefId,
    pub item: ItemId,
    pub node: NodeId,
    pub name: String,
    pub kind: HirCallableKind,
    pub params: Vec<HirParam>,
    pub result: Option<Type>,
    pub exported: bool,
    pub export_symbol: Option<String>,
    pub body: HirBodySkeleton,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirProgram {
    pub callables: Vec<HirCallable>,
}
