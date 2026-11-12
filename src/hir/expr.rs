// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0b typed expression nodes. Types are best-effort in H0b; H1 resolves the
//! full typed surface. Unsupported syntax is explicit and counted by verify.

use super::ids::{NodeId, Span};
use super::pattern::HirPattern;
use super::stmt::HirStmt;
use crate::ast::{BinaryOpKind, ReflectKind, SpawnStorage, Type, UnaryOpKind};

#[derive(Debug, Clone, PartialEq)]
pub enum HirLiteral {
    Bytes(Vec<u8>),
    Int(i64),
    Char(char),
    TaggedInt(i64, String),
    TaggedBytes(Vec<u8>, String),
    Bool(bool),
    Float(f64),
    BeginProgram,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirMatchArm {
    pub pattern: HirPattern,
    pub guard: Option<HirExpr>,
    pub body: HirExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirExpr {
    pub id: NodeId,
    pub span: Option<Span>,
    pub ty: Option<Type>,
    pub kind: HirExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirExprKind {
    Literal(HirLiteral),
    Identifier(String),
    Call {
        callee: String,
        args: Vec<HirExpr>,
        analysis_id: Option<usize>,
    },
    Binary {
        op: BinaryOpKind,
        left: Box<HirExpr>,
        right: Box<HirExpr>,
    },
    Unary {
        op: UnaryOpKind,
        inner: Box<HirExpr>,
    },
    Field {
        receiver: Box<HirExpr>,
        name: String,
    },
    Cast {
        inner: Box<HirExpr>,
        to: Type,
        /// Resolved protocol lane sequence. `None` = endpoints not yet typed.
        via: Option<Vec<crate::casting::graph::CastStep>>,
    },
    MethodCall {
        receiver: Box<HirExpr>,
        name: String,
        args: Vec<HirExpr>,
        analysis_id: Option<usize>,
    },
    Reflect {
        receiver: Box<HirExpr>,
        name: String,
        kind: ReflectKind,
    },
    Index(Box<HirExpr>, Box<HirExpr>),
    /// 2026-11-12 (electronics full-surface port): the `[*]` wildcard
    /// selector — every element of the indexed array. Only valid as an
    /// index (`t[*]`, `u2.gpio[*]`); resolved by eager expansion in the
    /// electronics analysis, never emitted.
    Wildcard,
    Slice {
        array: Box<HirExpr>,
        start: Option<Box<HirExpr>>,
        end: Option<Box<HirExpr>>,
        stride: Option<Box<HirExpr>>,
    },
    Range {
        start: Box<HirExpr>,
        end: Box<HirExpr>,
        inclusive: bool,
    },
    Spawn {
        type_name: String,
        args: Vec<HirExpr>,
        storage: SpawnStorage,
    },
    If {
        condition: Box<HirExpr>,
        then_branch: Box<HirExpr>,
        else_branch: Option<Box<HirExpr>>,
    },
    Lambda {
        params: Vec<String>,
        body: Box<HirExpr>,
    },
    IsType {
        inner: Box<HirExpr>,
        ty: Type,
    },
    Deref(Box<HirExpr>),
    AddrOf(Box<HirExpr>),
    Consume(Box<HirExpr>),
    Await(Box<HirExpr>),
    UnitLiteral {
        value: f64,
        unit: String,
    },
    Tuple(Vec<HirExpr>),
    List(Vec<HirExpr>),
    /// Plugin-intercepted chained macro (`println!(...)`, `get_env_int!(...)`):
    /// the plugin system owns expansion; the compiler carries the surface.
    PluginIntercept {
        name: String,
        args: Vec<HirExpr>,
        type_args: Vec<crate::ast::Type>,
        receiver: Option<Box<HirExpr>>,
    },
    StructLiteral {
        type_name: String,
        fields: Vec<(String, HirExpr)>,
        /// `spec` fields (component physics parameters), distinct from
        /// ordinary literal fields; empty for ordinary struct literals.
        specs: Vec<(String, HirExpr)>,
    },
    Match {
        scrutinee: Box<HirExpr>,
        arms: Vec<HirMatchArm>,
    },
    Block(Vec<HirStmt>),
    Other(&'static str),
}
