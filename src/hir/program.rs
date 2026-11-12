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
    /// 2026-11-09: `isr handler` declaration (vector-table rooted, not a
    /// reactor transaction — mirrors the AST's separate TopLevel arm).
    IsrHandler,
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

impl HirOutput {
    pub fn all_types(&self) -> Vec<Type> {
        match self {
            HirOutput::Single(ty) => vec![ty.clone()],
            HirOutput::Union(types) | HirOutput::Tuple(types) => {
                types.iter().flat_map(|t| t.all_types()).collect()
            }
            HirOutput::Array(inner) | HirOutput::Named(_, inner) => inner.all_types(),
        }
    }
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
    /// Whether a handler conflict is REQUIRED — the AST WatchdogSpec flag,
    /// read by the watchdog diagnostics pass.
    pub is_required: bool,
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
    /// True when this callable came from an `impl` block (distinguishes the
    /// on-fire callable set from type-member declarations).
    pub is_impl_fn: bool,
    /// 2026-11-09 (H5 flip gate): an `isr<mech> handler @ vec: name(...)`
    /// declaration. The vector table roots it like the AST IsrHandler arm;
    /// the body still walks for callee edges.
    pub is_isr: bool,
    pub params: Vec<HirParam>,
    /// Generic type-parameter names.
    pub type_params: Vec<String>,
    /// Has a trailing variadic rest parameter.
    pub variadic: bool,
    /// `sync<domains>` wrapping this txn; empty for unwrapped callables.
    pub sync_domains: Vec<String>,
    /// Declared callable modifiers (`accel`, `approx`, `exact`, …), by name.
    /// 2026-11-11 (H5 accel port): the offload analysis reads the same
    /// modifiers the AST `Transaction`/`Definition` carries; names only
    /// because every policy keyword here is value-less.
    pub modifiers: Vec<String>,
    pub result: Option<Type>,
    /// Explicit multi-output list (`defn f() -> A, B`), if any.
    pub output_list: Vec<Type>,
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

/// 2026-11-12 (H5 electronics port): one `pin` declaration in a component
/// type body. `number` is the KiCad pin number, `class_ref` the ascribed
/// electrical-class fundamental name, `unit` the symbol unit clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirPinDecl {
    pub name: String,
    pub number: u64,
    pub class_ref: Option<String>,
    pub unit: Option<String>,
}

/// A static forced-fact law (`hold G { F₁; … }`) carried on the type or at
/// top level. The electronics analysis reads the guard/facts to derive
/// conditional voltage drives. Runtime `when` statements stay in bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct HirHoldLaw {
    pub guard: HirExpr,
    pub facts: Vec<HirStmt>,
}

/// A top-level `unpop`/`shortcircuit` participation fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirParticipation {
    pub instance: String,
    pub shortcircuit: bool,
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
    /// `coll obj` type (collection proof + grow-guard elimination).
    pub is_coll: bool,
    /// Declared parent/trait links (`type X: Trait`).
    pub traits: Vec<String>,
    /// Declared op-binding names (`op Grow: ...`).
    pub op_bindings: Vec<String>,
    /// 2026-11-11 (reclamation port): at least one op binding is a
    /// user-supplied implementation rather than an intrinsic call. The AST
    /// reclamation graph treats such types as user operators (barrier).
    pub has_user_operators: bool,
    /// Op-binding implementation fn names (`op Add: cstring_concat`).
    pub binding_fns: Vec<String>,
    /// 2026-11-12 (electronics): first-class component pins, in declaration
    /// order. Empty for non-component types.
    pub pins: Vec<HirPinDecl>,
    /// Declared schematic reference-designator prefix (`reference "R";`).
    pub reference: Option<String>,
    /// Type-level `spec`/`attr` metadata bag.
    pub metadata: std::collections::BTreeMap<String, crate::ast::PropertyValue>,
    /// Type-inherited static laws (`hold G { … }`).
    pub hold_laws: Vec<HirHoldLaw>,
    /// Declared named operating modes, in declaration order.
    pub modes: Vec<String>,
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
    /// Declared modifier names (`out`, `vol`, `state`, `plain`, …).
    pub modifiers: Vec<String>,
    /// Declared modifier payloads (`net: "…"`, `stdnet: "…"`) in declaration
    /// order. The electronics analysis reads the author net attachments here.
    pub annotations: Vec<(String, Option<HirExpr>)>,
    pub span: Option<Span>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirInit {
    pub name: String,
    /// Declared value set (dependent pool sizing).
    pub bound: Option<crate::ast::top::BoundSpec>,
    pub value: Option<HirExpr>,
    pub body: Vec<HirStmt>,
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
    /// Top-level `const` initializers (name, value) for surface walks.
    pub const_exprs: Vec<(String, HirExpr)>,
    /// Top-level `const` declared types (name, type) — density's float
    /// seeding needs the annotation the AST keeps on `TopLevel::Constant`.
    pub const_types: Vec<(String, Type)>,
    /// Top-level `init` declarations (runtime-seeded invariants).
    pub inits: Vec<HirInit>,
    /// At least one top-level `trigger` declaration exists.
    pub has_triggers: bool,
    /// Top-level `trigger` names (watchdog validity + §3.2 surface).
    pub trigger_names: Vec<String>,
    /// 2026-11-10: top-level `trg name @ instance.port` binding names — the
    /// externally-driven input set the dependency graph seeds.
    pub trigger_bindings: Vec<String>,
    /// 2026-11-12 (electronics): top-level `unpop`/`shortcircuit` facts.
    pub participations: Vec<HirParticipation>,
    /// 2026-11-12 (electronics): top-level static `hold` laws.
    pub hold_laws: Vec<HirHoldLaw>,
    /// 2026-11-10 (casing port): declared-name surfaces by origin. The AST
    /// advisory distinguishes `type` from `struct`, `trait`, and `cell`
    /// declarations; `type_defs` alone conflates TypeDef and StaticStruct.
    pub type_names: Vec<String>,
    pub static_struct_names: Vec<String>,
    pub trait_names: Vec<String>,
    pub cell_names: Vec<String>,
    /// Top-level `trait` declarations: name + ALL declared function names
    /// (requirements included — callable lowering skips body-less slots, so
    /// the conformance analysis reads this table instead). First declaration
    /// of a name wins, matching the AST walker.
    pub trait_functions: Vec<(String, Vec<String>)>,
    /// Binding fns named by `proto` cast edges / cross-ops. The casting graph
    /// calls them at emission, so liveness must root them.
    pub protocol_binding_fns: Vec<String>,
    /// 2026-11-04 (H3.0): `asm fn` / `bad fn` names — compiled out of band
    /// with the implicit `%state` first ABI arg, so a call always carries
    /// state (the AST always_stateful leaf set).
    pub always_stateful: Vec<String>,
}
