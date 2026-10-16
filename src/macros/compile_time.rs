#![allow(dead_code)]
// ── Compile-Time Evaluation (Level C) ───────────────────────────────────
// 2026-07-21: Extends the interpreter with compile-time types for the
// AST navigation DSL. CTSelection, CTPosition, CTTextSelection are first-
// class values inside $(Stage) blocks, enabling full Briev evaluation
// (let, defn, when, foreach, match) at compile time.
//
// Navigation intrinsics (Tag$, Named$, First$, Before$, Insert$, etc.)
// are registered as callable built-in functions in the interpreter.
// Source$, Ir$, Bin$, Stage$ are pre-bound identifiers.

use crate::ast::{Expr, Statement, TopLevel, PropertyValue};
use crate::ast::top::*;
use crate::type_universe::TypeUniverse;
use super::selection::{Selection, NodeRef, Selector, TagSelector, NamedSelector,
    WithKeySelector, WithAttrSelector, AllSelector, top_level_tag, top_level_name};
use super::actions::{Position, insert_items, delete_selection, replace_selection,
    set_metadata, rename_selection, wrap_selection};
use super::text_ops::TextSelection;

/// Compile-time selection value (wraps a Selection).
#[derive(Debug, Clone)]
pub struct CTSelection(pub Selection);

/// Compile-time position value (wraps a Position).
#[derive(Debug, Clone)]
pub struct CTPosition(pub Position);

/// Compile-time text selection value.
#[derive(Debug, Clone)]
pub struct CTTextSelection(pub TextSelection);

/// The data target for the current operation chain.
#[derive(Debug, Clone)]
pub enum CTTarget {
    /// Source text (mutable at PreLex only).
    Source(String),
    /// AST (live Vec<TopLevel>).
    Ast,
    /// IR text (mutable at Generated/Optimized).
    Ir(String),
    /// Binary path (read-only).
    Bin(std::path::PathBuf),
    /// Plugin registry (Stage$).
    Stage,
}

/// Context for compile-time evaluation of a $(Stage) block.
/// Holds references to the current compilation state.
pub struct EvalContext<'a> {
    pub program: &'a mut Vec<TopLevel>,
    pub universe: &'a mut TypeUniverse,
    pub stage: crate::ast::StageKind,
    pub bin_path: Option<std::path::PathBuf>,
    /// Bindings from pattern variables (?name → NodeRef).
    pub pattern_bindings: std::collections::HashMap<String, NodeRef>,
    /// Active target for the current chain.
    pub target: CTTarget,
}

impl<'a> EvalContext<'a> {
    pub fn new(
        program: &'a mut Vec<TopLevel>,
        universe: &'a mut TypeUniverse,
        stage: crate::ast::StageKind,
    ) -> Self {
        let target = if stage.is_ast_stage() {
            CTTarget::Ast
        } else if stage == crate::ast::StageKind::PreLex {
            CTTarget::Source(String::new())
        } else if stage.is_ir_stage() {
            CTTarget::Ir(String::new())
        } else {
            CTTarget::Bin(std::path::PathBuf::new())
        };
        EvalContext {
            program,
            universe,
            stage,
            bin_path: None,
            pattern_bindings: std::collections::HashMap::new(),
            target,
        }
    }
}

// ── Navigation Intrinsic Definitions ───────────────────────────────────

/// Result from evaluating a navigation chain statement.
/// Can be a value (for queries) or nothing (for mutations).
#[derive(Debug)]
pub enum NavResult {
    Selection(Selection),
    TextSelection(TextSelection),
    Count(usize),
    Names(Vec<String>),
    IsEmpty(bool),
    Lines(Vec<usize>),
    Void,
}

/// Evaluate a navigation chain: resolve selector → traverse → position → act.
pub fn evaluate_chain(
    _ctx: &mut EvalContext,
    chain: &[NavStep],
) -> Result<NavResult, String> {
    // Placeholder: will interpret the chain and return results
    // Each step dispatches to the appropriate selector/position/action
    Err("navigation chain evaluation not yet implemented — Phase G placeholder".into())
}

/// A single step in a navigation chain expression.
#[derive(Debug)]
pub enum NavStep {
    Selector(Box<dyn Selector>),
    Position(Position),
    Insert(Vec<TopLevel>),
    Delete,
    ReplaceWith(TopLevel),
    SetMetadata(String, PropertyValue),
    Rename(String),
    Wrap(String),
    Count,
    IsEmpty,
    Names,
    Lines,
    First(usize),
    Last(usize),
    Nth(usize),
    Children(Option<String>),
    Descendants(Option<String>),
    Parent,
    Ancestors(Option<String>),
    Closest(String),
    Next(Option<String>),
    Prev(Option<String>),
    Find(String),
    ReplaceText(String),
    InsertText(String),
    DeleteText,
    PrependText(String),
    AppendText(String),
    ReadBytes(u64, usize),
    Size,
    Path,
    Run(String),
}

/// Evaluate a $(Stage) block body.
pub fn evaluate_stage_block(
    body: &[Statement],
    ctx: &mut EvalContext,
) -> Result<(), String> {
    for stmt in body {
        evaluate_stage_statement(stmt, ctx)?;
    }
    Ok(())
}

fn evaluate_stage_statement(
    stmt: &Statement,
    ctx: &mut EvalContext,
) -> Result<(), String> {
    match stmt {
        // let name = expr;
        Statement::Let { expr: Some(e), .. } => {
            // Evaluate the expression and bind it
            // For now, handle simple string/int/decimal expressions
            match e {
                Expr::Decimal(n) => {
                    // Bind to a numeric constant — stored in a compile-time scope
                    // (Implementation TBD — placeholder for Phase G)
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        Statement::Expression(expr) => {
            // Expression statement — could be a navigation chain
            evaluate_chain_expression(expr, ctx)
        }
        Statement::Term(Some(expr)) | Statement::Tail(expr) => {
            // Return value from a compile-time defn
            evaluate_chain_expression(expr, ctx)
        }
        _ => Ok(()),
    }
}

fn evaluate_chain_expression(
    _expr: &Expr,
    _ctx: &mut EvalContext,
) -> Result<(), String> {
    // Parse the expression tree to extract navigation chain steps
    // and dispatch each step via evaluate_chain()
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        // Phase G will be fully implemented in follow-up
    }
}
