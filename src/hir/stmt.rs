// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0b typed statement nodes. Structural lowering only; no consumer.

use super::expr::HirExpr;
use super::ids::{NodeId, Span};
use super::pattern::HirPattern;
use crate::ast::{PropertyValue, Type};

#[derive(Debug, Clone, PartialEq)]
pub struct HirStmtMatchArm {
    pub patterns: Vec<HirPattern>,
    pub body: Vec<HirStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirStmt {
    pub id: NodeId,
    pub span: Option<Span>,
    pub kind: HirStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirStmtKind {
    Let {
        name: String,
        /// All bound names for destructuring lets (`let (a, b) = ...`);
        /// single-name lets carry `vec![name]`.
        names: Vec<String>,
        ty: Option<Type>,
        expr: Option<HirExpr>,
        /// Declared modifiers (`state`, `plain`, `out`, `vol`, …).
        modifiers: Vec<crate::ast::Annotation>,
    },
    Assign(HirExpr, HirExpr),
    ArrowAssign {
        target: Option<HirExpr>,
        value: HirExpr,
        consume: bool,
    },
    Term(Option<HirExpr>),
    Tail(HirExpr),
    Check(HirExpr),
    Expression(HirExpr),
    Guarded(HirExpr, Vec<HirStmt>),
    Block(Vec<HirStmt>),
    Foreach {
        item: String,
        list: HirExpr,
        body: Vec<HirStmt>,
    },
    Match {
        expr: HirExpr,
        arms: Vec<HirStmtMatchArm>,
    },
    Break,
    Trap,
    Halt,
    Yield,
    EndProgram(Option<HirExpr>),
    Gate(HirExpr),
    Rollback(Option<HirExpr>),
    FreeHint(String),
    KeepHint(String),
    SyncBlock(Vec<HirStmt>),
    Defer(Vec<HirStmt>),
    Mutex(Vec<HirStmt>),
    Open(HirExpr, HirExpr),
    TrgBinding {
        name: String,
        instance: HirExpr,
    },
    MetadataAssignment(String, PropertyValue),
    InlineAsm {
        asm_string: String,
        clobbers: Vec<String>,
        span: Option<crate::errors::Span>,
    },
    Other(&'static str),
}
