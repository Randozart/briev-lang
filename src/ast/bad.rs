// ── .bad (Briev Assembly Dialect) AST ─────────────────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-09-21 (bad-dialect plan): AST for .bad programs. Fully standalone
// from the Briev Expr/Type world — .bad is pure assembly + contracts, so
// this module defines its own minimal node set. The line-oriented parser
// (src/parser/bad.rs) produces these; the backend (src/backend/bad/)
// consumes them.
//
// Structure mirrors the source's sequence semantics: a label or defn owns
// every instruction line that follows it until the next top-level item
// (zero braces, no indentation sensitivity — see plan 2026-09-21).

use crate::errors::Span;

/// A whole .bad file. `items` preserves source order — label membership is
/// positional (instructions belong to the nearest preceding owner).
#[derive(Debug, Clone)]
pub struct BadProgram {
    pub items: Vec<BadTopLevel>,
    pub span: Span,
}

/// One top-level item. Order is load-bearing: labels/defns own the
/// instruction lines that follow them.
#[derive(Debug, Clone)]
pub enum BadTopLevel {
    /// `section .text` / `global _start` — assembly directives.
    Directive(BadDirective),
    /// `msg: .asciz "hello\n"` — data label + directive (colon-suffix form).
    Data(BadDataLabel),
    /// `_start [post: r0 == 0]` — a code label; owns following instructions.
    Label(BadLabel),
    /// `defn store_pair a, addr` — inlined-as-is reusable sequence.
    Defn(BadDefn),
    /// `alias result = r0` — source-level register sugar.
    Alias(BadAlias),
}

/// `section .text` / `global _start`.
#[derive(Debug, Clone)]
pub struct BadDirective {
    pub name: String,
    /// Everything after the name, verbatim (`.text`, `_start`).
    pub args: String,
    pub span: Span,
}

/// `msg: .asciz "hello\n"`.
#[derive(Debug, Clone)]
pub struct BadDataLabel {
    pub name: String,
    /// The directive after the colon (`.asciz`, `.word`, ...).
    pub directive: BadDirective,
    pub span: Span,
}

/// A code label and the items it owns (until the next top-level item).
#[derive(Debug, Clone)]
pub struct BadLabel {
    pub name: String,
    /// True for local labels (`.name:`) — scoped to the enclosing global
    /// label and uniquified at emission.
    pub local: bool,
    /// Label-level contracts: `[pre: ...] [post: ...]` on the label line.
    pub contracts: Vec<BadContract>,
    pub body: Vec<BadBodyItem>,
    pub span: Span,
}

/// One item inside a label body: an instruction or an intermediate local
/// label (`.loop:` between instructions).
#[derive(Debug, Clone)]
pub enum BadBodyItem {
    Instr(BadInstr),
    Local(BadLocal),
}

/// A local label definition inside a label body: `.loop:`.
#[derive(Debug, Clone)]
pub struct BadLocal {
    /// Name WITHOUT the leading dot.
    pub name: String,
    pub span: Span,
}

/// `alias result = r0` — pure source sugar, resolved before register
/// mapping; aliases never reach the backend.
#[derive(Debug, Clone)]
pub struct BadAlias {
    pub name: String,
    pub register: String,
    pub span: Span,
}

/// The shape of a defn (plan §Grammar: two shapes, both line-oriented).
#[derive(Debug, Clone)]
pub enum BadDefnShape {
    /// Universal body lines (+ optional per-instruction exceptions). The
    /// body IS the default; no `default =>` row present. Local labels
    /// (`.name:`) are legal — every expansion hygienically renames them.
    Sequence(Vec<BadBodyItem>),
    /// Pure target table: `default? => instr; instr` rows. A `default`
    /// row must use universal core syntax; a target row replaces the
    /// whole defn on match.
    Branch(Vec<BadBranch>),
}

/// `defn store_pair a, addr` — inlined as-is at every call site.
#[derive(Debug, Clone)]
pub struct BadDefn {
    pub name: String,
    pub params: Vec<String>,
    pub shape: BadDefnShape,
    pub span: Span,
}

/// One instruction line plus any `target => ...` exception lines that
/// follow it.
#[derive(Debug, Clone)]
pub struct BadInstr {
    pub mnemonic: String,
    pub operands: Vec<BadOperand>,
    /// Inline contract (`[sp % 16 == 0]`) written BEFORE this instruction.
    pub contract: Option<BadContract>,
    /// `x86_64 => lea ...` lines attached to this instruction. Empty =
    /// universal only.
    pub exceptions: Vec<BadBranch>,
    pub span: Span,
}

/// A `target => body` row. In an instruction's `exceptions`, the body is
/// target-owned assembly (the mnemonic need not be core ISA). In a branch
/// defn, the body is core-syntax instructions; `target == "default"` with
/// `is_default` marks the universal row.
#[derive(Debug, Clone)]
pub struct BadBranch {
    pub target: String,
    pub is_default: bool,
    pub body: Vec<BadInstr>,
    pub span: Span,
}

/// An operand token. `42` / `-42` are immediates; identifier-shaped
/// tokens (r0, sp, msg, _start, defn params) are Names resolved at
/// lowering: params first (defn scope), then the register table, else
/// label/symbol. Anything with arithmetic shape (`addr + 8`, `MAX * 4`)
/// is an Expr — evaluated at lowering through the comptime pass
/// (`.const` table + defn-param immediates).
#[derive(Debug, Clone, PartialEq)]
pub enum BadOperand {
    Int(i64),
    /// Float literal (`1.5`, `3.14e-2`) — original text kept for
    /// literal-form targets (aarch64 `ldr =1.5`); pool targets key on
    /// the parsed value's bits.
    Float(String),
    Name(String),
    Expr(String),
}

/// Contract predicates (MVP grammar — chained compare on a register term,
/// plus the two special predicates). `[sp % 16 == 0]` and `[r0 == 0]` are
/// `Compare` chains evaluated left to right; `[r0 preserved]` /
/// `[r0 valid]` are the special forms.
#[derive(Debug, Clone)]
pub enum BadContractPred {
    /// `[r0 preserved]` — proven via callee-saved status or verified
    /// push/pop pairing.
    Preserved(String),
    /// `[r0 valid]` — register is mapped on the target (full pointer
    /// proofs deferred; documented in bad-dialect.md).
    Valid(String),
    /// `[sp % 16 == 0]`, `[r0 == 0]` — left-to-right op chain.
    Compare {
        lhs: String,
        ops: Vec<(BadCmpOp, i64)>,
    },
    /// `[frame: 32]` — the body keeps sp within 32 bytes, restores it
    /// exactly, and holds 16-alignment at every call. Label-level only.
    Frame(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadCmpOp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Mod,
}

/// One `[...]` contract (label-level or inline): a conjunction of preds.
#[derive(Debug, Clone)]
pub struct BadContract {
    pub preds: Vec<BadContractPred>,
    pub span: Span,
}
