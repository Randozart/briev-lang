// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0a typed-HIR skeleton: callable/export metadata with stable IDs.
//!
//! Body expressions/statements are represented by a counted placeholder until
//! H0b lowers them. No analysis or backend consumes this program yet.

use super::expr::HirExpr;
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

/// Declared output type (mirrors AST `OutputType`; unions/tuples matter for
/// boundary-return ownership).
#[derive(Debug, Clone, PartialEq)]
pub enum HirOutput {
    Single(Type),
    Union(Vec<HirOutput>),
    Tuple(Vec<HirOutput>),
    Array(Box<HirOutput>),
    Named(String, Box<HirOutput>),
}

/// A watchdog `-> handler(val)` callback (H3.2 liveness: the handler NAME is
/// emitted code a body walk can never see).
#[derive(Debug, Clone, PartialEq)]
pub struct HirWatchdogOnFire {
    pub handler: String,
    pub arg: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirWatchdog {
    pub condition: HirExpr,
    pub fallback: Option<HirExpr>,
    pub on_fire: Option<HirWatchdogOnFire>,
}

/// Contract expressions are emitted code — liveness walks them.
#[derive(Debug, Clone, PartialEq)]
pub struct HirContract {
    pub pre: HirExpr,
    pub post: HirExpr,
    pub watchdog: Option<HirWatchdog>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirCallable {
    pub id: DefId,
    pub item: ItemId,
    pub node: NodeId,
    pub name: String,
    pub kind: HirCallableKind,
    /// 2026-11-04 (H3.2): contract expressions (pre/post/watchdog) — emitted
    /// code whose call edges liveness must follow.
    pub contract: Option<HirContract>,
    /// Reactive txns are reactor-dispatched by name (liveness roots).
    pub is_reactive: bool,
    /// Async txns pull in `wait_for_trigger` at the main tail.
    pub is_async: bool,
    /// 2026-11-04 (H3.0): owning type for member/impl functions
    /// (`Type::member` keys, matching the AST `type_member_key` format).
    pub owner: Option<String>,
    /// Trait DEFAULT function (indexed by bare name, not type-qualified).
    pub trait_default: bool,
    pub params: Vec<HirParam>,
    /// Generic type-parameter names.
    pub type_params: Vec<String>,
    /// Has a trailing variadic rest parameter.
    pub variadic: bool,
    pub result: Option<Type>,
    /// Full declared output shape (union/tuple/array fidelity).
    pub outputs: Option<HirOutput>,
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

/// 2026-11-04 (H3.3b): a `frgn` foreign binding — the FFI boundary surface
/// boundary-ownership inference classifies.
#[derive(Debug, Clone, PartialEq)]
pub struct HirForeign {
    /// Effective Briev name (`as <name>` or the foreign symbol).
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub ret: Option<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirTypeDef {
    pub node: NodeId,
    pub item: ItemId,
    pub name: String,
    pub type_params: Vec<String>,
    pub kind: HirTypeDefKind,
    /// Declared boundary protocol (`type CStr: String<C_String>`), if any.
    pub protocol: Option<String>,
    pub span: Option<Span>,
}

/// 2026-11-04 (H3.0): a top-level `let` / state declaration — the globals
/// the needs_state/storage-class analyses partition into machine state,
/// plain state, and constants.
#[derive(Debug, Clone, PartialEq)]
pub struct HirGlobal {
    pub node: NodeId,
    pub item: ItemId,
    pub name: String,
    pub ty: Option<Type>,
    /// Initializer (top-level statements are emitted into the init path; their
    /// callees are liveness roots).
    pub expr: Option<HirExpr>,
    /// Declared storage assertion (`state let` / `plain let`), if any.
    pub declared_storage: Option<String>,
    /// `state name: T;` (vs a top-level `let`).
    pub is_state_decl: bool,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirProgram {
    pub callables: Vec<HirCallable>,
    pub type_defs: Vec<HirTypeDef>,
    pub globals: Vec<HirGlobal>,
    /// `frgn` foreign bindings (effective Briev name → params/result).
    pub foreign: Vec<HirForeign>,
    /// Top-level `const` names (boundary state-field set includes them).
    pub constants: Vec<String>,
    /// 2026-11-04 (H3.0): `asm fn` / `bad fn` names — compiled out of band
    /// with the implicit `%state` first ABI arg, so a call always carries
    /// state (the AST always_stateful leaf set).
    pub always_stateful: Vec<String>,
}
