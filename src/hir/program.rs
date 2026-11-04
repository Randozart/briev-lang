// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0a typed-HIR skeleton: callable/export metadata with stable IDs.
//!
//! Body expressions/statements are represented by a counted placeholder until
//! H0b lowers them. No analysis or backend consumes this program yet.

use super::ids::{DefId, ItemId, NodeId, Span};
use super::stmt::HirStmt;
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
pub struct HirBody {
    pub node: NodeId,
    pub span: Option<Span>,
    pub statements: Vec<HirStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirCallable {
    pub id: DefId,
    pub item: ItemId,
    pub node: NodeId,
    pub name: String,
    pub kind: HirCallableKind,
    /// 2026-11-04 (H3.0): owning type for member/impl functions
    /// (`Type::member` keys, matching the AST `type_member_key` format).
    pub owner: Option<String>,
    /// Trait DEFAULT function (indexed by bare name, not type-qualified).
    pub trait_default: bool,
    pub params: Vec<HirParam>,
    pub result: Option<Type>,
    pub exported: bool,
    pub export_symbol: Option<String>,
    pub body: HirBody,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirField {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirEnumVariant {
    Unit(String),
    Tuple(String, Vec<Type>),
    Struct(String, Vec<HirField>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirTypeDefKind {
    Enum { variants: Vec<HirEnumVariant> },
    Object { fields: Vec<HirField> },
    Struct { fields: Vec<HirField> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirTypeDef {
    pub node: NodeId,
    pub item: ItemId,
    pub name: String,
    pub type_params: Vec<String>,
    pub kind: HirTypeDefKind,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirProgram {
    pub callables: Vec<HirCallable>,
    pub type_defs: Vec<HirTypeDef>,
}
