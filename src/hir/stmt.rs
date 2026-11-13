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
        /// Statement-level modifier names (currently `seq`), carried through
        /// the HIR round-trip so their semantics are never lost.
        modifiers: Vec<String>,
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
    /// 2026-11-12 (owned-storage pass): compiler-inserted storage release.
    /// Not source syntax — the HIR/AST body seam consumes no AST statement
    /// for it and emission expands to `Free#(name)` (Str) or
    /// `coll_destroy(name)` (List).
    ReleaseOwned { storage: StorageKind, name: String },
    /// 2026-11-12 (owned-storage pass): temp-transfer for a fresh return
    /// expression that borrows owned locals. Expands to
    /// `let name = expr; <releases>; term name;` so the operands free after
    /// the allocation evaluates. Same seam contract as ReleaseOwned.
    TempTransfer {
        name: String,
        ty: Option<Type>,
        expr: Box<HirExpr>,
        releases: Vec<(StorageKind, String)>,
        /// Which return form the transfer replaced (Term/Tail/EndProgram) —
        /// emission must preserve the original return semantics.
        ret: RetKind,
    },
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

/// Which owned-storage destructor a ReleaseOwned statement expands to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    /// `Free#(name)` — a String handle.
    Str,
    /// `coll_destroy(name)` — a coll List handle (object + arena-aware data).
    List,
}

/// The return form a TempTransfer replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetKind {
    Term,
    Tail,
    EndProgram,
}
