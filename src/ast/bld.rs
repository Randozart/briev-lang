// ── .bld (BILLD — Briev Intermediate Low-Level Dialect) AST ────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-10-08 (BILLD plan, docs/plans/2026-10-08-billd-intermediate-dialect.md):
// AST for .bld programs — the execution-recipe tier between .bv (proven,
// bounded, reactor) and .bad (physical registers, line-oriented).
//
// Division of labor mirrors the .bad AST's standalone stance:
// - EXPRESSIONS are the shared Briev `Expr` — the shared expression parser
//   gives precedence, calls, and arithmetic for free (DRY).
// - STATEMENTS/DECLARATIONS are BILLD's own small set: unbounded
//   `loop`/`while` (the physical world spins — polling, spinlocks, idle),
//   `if`/`else`, PascalCase engine-intrinsic calls, naked `return`, and
//   verbatim `bad { … }` passthrough blocks into .bad grammar.
//
// The parser (src/parser/bld.rs) produces these; the backend
// (src/backend/bld/) lowers them to `BadProgram` for the .bad backend —
// BILLD never enters the .bv pipeline (no reactor, no termination gates).
//
// To undo: delete this file + src/parser/bld.rs, revert `pub mod bld;` in
// src/ast/mod.rs and src/parser/mod.rs, and drop the five compound-assign
// tokens from src/lexer.rs.

use crate::ast::{Expr, Type};
use crate::errors::Span;

/// A whole .bld file. `items` preserves source order.
#[derive(Debug, Clone)]
pub struct BldProgram {
    pub items: Vec<BldTopLevel>,
    pub span: Span,
}

/// One top-level item. Order is load-bearing only for readability —
/// resolution (defn bodies, bootstrap entry) is by name.
#[derive(Debug, Clone)]
pub enum BldTopLevel {
    /// `import "std/bad/arch.bad";` — pulls in a .bad or .bld module.
    Import(BldImport),
    /// `const STACK_TOP = 0x90000;` — a named compile-time constant.
    Const(BldConst),
    /// `defn uart_putc(c: Int) { … }` — a reusable recipe, lowered like
    /// a .bad sequence defn (called, never inlined by default).
    Defn(BldDefn),
    /// `bootstrap Reset_Handler() { … }` — the authored machine entry.
    /// Exactly one per file; the body IS the reset-vector recipe.
    Bootstrap(BldBootstrap),
}

/// `import "path";`
#[derive(Debug, Clone)]
pub struct BldImport {
    pub path: String,
    pub span: Span,
}

/// `const NAME = expr;` — compile-time constant, folded at lowering.
#[derive(Debug, Clone)]
pub struct BldConst {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

/// `defn name(params) -> Type { body }` — return type optional.
#[derive(Debug, Clone)]
pub struct BldDefn {
    pub name: String,
    pub params: Vec<BldParam>,
    pub ret: Option<Type>,
    pub body: Vec<BldStmt>,
    pub span: Span,
}

/// `bootstrap Name() { body }` — the machine entry (established
/// terminology: `bootstrap` = authored reset, machine-entry.md). Takes
/// no parameters; the author owns sp setup and the handoff.
#[derive(Debug, Clone)]
pub struct BldBootstrap {
    pub name: String,
    pub body: Vec<BldStmt>,
    pub span: Span,
}

/// One parameter: `c` or `c: Int`. Untyped params are inferred at
/// lowering from call sites (v1: a loud error if unresolvable).
#[derive(Debug, Clone)]
pub struct BldParam {
    pub name: String,
    pub ty: Option<Type>,
    pub span: Span,
}

/// One statement. Every variant carries the span of its head token so
/// lowering can point diagnostics at the recipe line that failed.
#[derive(Debug, Clone)]
pub enum BldStmt {
    /// `let name: T? = expr;` — declaration; the initializer is REQUIRED
    /// (a recipe binding always has a value).
    Let {
        name: String,
        ty: Option<Type>,
        value: Expr,
        span: Span,
    },
    /// `target = value;` — plain and compound assignment (`x |= 1` is
    /// desugared here to `x = x | 1`; see src/parser/bld.rs).
    Assign {
        target: Expr,
        value: Expr,
        span: Span,
    },
    /// A call statement — an engine intrinsic (`DisableInterrupts();`)
    /// or a defn call (`uart_putc(65);`). The ONLY expression form
    /// legal as a statement (everything else must be assigned).
    Call {
        callee: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// `{ … }` — a bare block (scope grouping).
    Block {
        body: Vec<BldStmt>,
        span: Span,
    },
    /// `if cond { … } else if cond { … } else { … }` — `otherwise` holds
    /// the else branch (an else-if is a nested `If` inside it).
    If {
        cond: Expr,
        then: Vec<BldStmt>,
        otherwise: Option<Vec<BldStmt>>,
        span: Span,
    },
    /// `loop { … }` — unbounded spin; exits via `break`. Never enters
    /// the .bv termination gates (this lane bypasses them by design).
    Loop {
        body: Vec<BldStmt>,
        span: Span,
    },
    /// `while cond { … }` — unbounded conditional spin.
    While {
        cond: Expr,
        body: Vec<BldStmt>,
        span: Span,
    },
    /// `break;`
    Break {
        span: Span,
    },
    /// `continue;`
    Continue {
        span: Span,
    },
    /// `return;` / `return expr;` — the naked exit (no hidden frame).
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// `bad { … }` — verbatim .bad passthrough: sections, data labels,
    /// contracts, raw blocks, physical registers. The escape hatch, in
    /// the dialect itself (raw-block doctrine).
    Bad {
        text: String,
        span: Span,
    },
}
