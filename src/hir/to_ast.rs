// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! HIR → AST reconstruction for boundary consumers that still emit AST
//! (H4.2 transition). Lossless for supported HIR nodes; `None` for
//! `Other` (unsupported plugin/intercept shapes), so callers fall back to the
//! AST path instead of emitting a guessed node.
//!
//! HIR keeps every field that matters for emission: declared types, let
//! modifiers, struct-literal `spec` fields, arrow `consume`, chain refs, and
//! analysis ids. `Cast.via` is analysis metadata (not syntax) and is dropped.

use super::expr::{HirExpr, HirExprKind, HirLiteral, HirMatchArm};
use super::pattern::{HirPattern, HirPatternKind};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use crate::ast::top::{Annotation, Statement, StmtMatchArm};
use crate::ast::{MatchArm, Pattern};

fn opt_expr(e: &Option<HirExpr>) -> Option<Option<crate::ast::Expr>> {
    match e {
        None => Some(None),
        Some(x) => expr(x).map(Some),
    }
}

fn opt_box(e: &Option<Box<HirExpr>>) -> Option<Option<Box<crate::ast::Expr>>> {
    match e {
        None => Some(None),
        Some(x) => expr(x).map(|v| Some(Box::new(v))),
    }
}

fn opt_ref(e: &Option<&HirExpr>) -> Option<Option<crate::ast::Expr>> {
    match e {
        None => Some(None),
        Some(x) => expr(x).map(Some),
    }
}

pub fn expr(e: &HirExpr) -> Option<crate::ast::Expr> {
    use crate::ast::Expr;
    Some(match &e.kind {
        HirExprKind::Literal(lit) => match lit {
            HirLiteral::Bytes(v) => Expr::Quoted(v.clone()),
            HirLiteral::Int(n) => Expr::Decimal(*n),
            HirLiteral::Char(c) => Expr::Char(*c),
            HirLiteral::TaggedInt(n, t) => Expr::TaggedLiteral(*n, t.clone()),
            HirLiteral::TaggedBytes(v, t) => Expr::TaggedQuotedLiteral(v.clone(), t.clone()),
            HirLiteral::Bool(b) => Expr::Bool(*b),
            HirLiteral::Float(f) => Expr::Float(*f),
            HirLiteral::BeginProgram => Expr::BeginProgram,
        },
        HirExprKind::Identifier(n) => Expr::Identifier(n.clone()),
        HirExprKind::Call { callee, args, analysis_id } => Expr::Call(
            callee.clone(),
            args.iter().map(expr).collect::<Option<Vec<_>>>()?,
            *analysis_id,
        ),
        HirExprKind::Binary { op, left, right } => {
            Expr::BinaryOp(*op, Box::new(expr(left)?), Box::new(expr(right)?))
        }
        HirExprKind::Unary { op, inner } => Expr::UnaryOp(*op, Box::new(expr(inner)?)),
        HirExprKind::Field { receiver, name } => {
            Expr::Field(Box::new(expr(receiver)?), name.clone())
        }
        HirExprKind::Cast { inner, to, .. } => Expr::Cast(Box::new(expr(inner)?), to.clone()),
        HirExprKind::MethodCall { receiver, name, args, analysis_id, chain_refs } => {
            Expr::MethodCall(
                Box::new(expr(receiver)?),
                name.clone(),
                args.iter().map(expr).collect::<Option<Vec<_>>>()?,
                *analysis_id,
                chain_refs.clone(),
            )
        }
        HirExprKind::Reflect { receiver, name, kind } => {
            Expr::Reflect(Box::new(expr(receiver)?), name.clone(), *kind)
        }
        HirExprKind::Index(a, b) => Expr::Index(Box::new(expr(a)?), Box::new(expr(b)?)),
        HirExprKind::Slice { array, start, end, stride } => Expr::Slice {
            array: Box::new(expr(array)?),
            start: opt_box(start)?,
            end: opt_box(end)?,
            stride: opt_box(stride)?,
        },
        HirExprKind::Range { start, end, inclusive } => Expr::Range {
            start: Box::new(expr(start)?),
            end: Box::new(expr(end)?),
            inclusive: *inclusive,
        },
        HirExprKind::Spawn { type_name, args, storage } => Expr::Spawn {
            type_name: type_name.clone(),
            args: args.iter().map(expr).collect::<Option<Vec<_>>>()?,
            storage: *storage,
        },
        HirExprKind::If { condition, then_branch, else_branch } => Expr::If(
            Box::new(expr(condition)?),
            Box::new(expr(then_branch)?),
            opt_box(else_branch)?,
        ),
        HirExprKind::Match { scrutinee, arms } => Expr::Match(
            Box::new(expr(scrutinee)?),
            arms.iter().map(match_arm).collect::<Option<Vec<_>>>()?,
        ),
        HirExprKind::Lambda { params, body } => {
            Expr::Lambda(params.clone(), Box::new(expr(body)?))
        }
        HirExprKind::IsType { inner, ty } => Expr::IsType(Box::new(expr(inner)?), ty.clone()),
        HirExprKind::Deref(inner) => Expr::Deref(Box::new(expr(inner)?)),
        HirExprKind::AddrOf(inner) => Expr::AddrOf(Box::new(expr(inner)?)),
        HirExprKind::Consume(inner) => Expr::Consume(Box::new(expr(inner)?)),
        HirExprKind::Await(inner) => Expr::Await(Box::new(expr(inner)?)),
        HirExprKind::UnitLiteral { value, unit } => Expr::UnitLiteral {
            value: *value,
            unit: unit.clone(),
        },
        HirExprKind::Tuple(items) => {
            Expr::Tuple(items.iter().map(expr).collect::<Option<Vec<_>>>()?)
        }
        HirExprKind::List(items) => {
            Expr::List(items.iter().map(expr).collect::<Option<Vec<_>>>()?)
        }
        HirExprKind::StructLiteral { type_name, fields, specs } => Expr::StructLiteral {
            type_name: type_name.clone(),
            fields: fields
                .iter()
                .map(|(n, e)| Some((n.clone(), expr(e)?)))
                .collect::<Option<Vec<_>>>()?,
            specs: specs
                .iter()
                .map(|(n, e)| Some((n.clone(), expr(e)?)))
                .collect::<Option<Vec<_>>>()?,
        },
        HirExprKind::PluginIntercept { name, args, type_args, receiver, chain_refs } => {
            Expr::PluginIntercept {
                name: name.clone(),
                args: args.iter().map(expr).collect::<Option<Vec<_>>>()?,
                type_args: type_args.clone(),
                receiver: match receiver {
                    None => None,
                    Some(r) => Some(Box::new(expr(r)?)),
                },
                chain_refs: chain_refs.clone(),
            }
        }
        HirExprKind::Block(stmts) => Expr::Block(stmt_body(stmts)?),
        HirExprKind::Other(_) => return None,
    })
}

fn match_arm(arm: &HirMatchArm) -> Option<MatchArm> {
    Some(MatchArm {
        pattern: pattern(&arm.pattern)?,
        guard: opt_expr(&arm.guard)?,
        body: Box::new(expr(&arm.body)?),
    })
}

fn stmt_match_arm(arm: &HirStmtMatchArm) -> Option<StmtMatchArm> {
    Some(StmtMatchArm {
        patterns: arm.patterns.iter().map(pattern).collect::<Option<Vec<_>>>()?,
        body: stmt_body(&arm.body)?,
    })
}

pub fn pattern(p: &HirPattern) -> Option<Pattern> {
    Some(match &p.kind {
        HirPatternKind::Wildcard => Pattern::Wildcard,
        HirPatternKind::Literal(e) => Pattern::Literal(expr(e)?),
        HirPatternKind::Binding(n) => Pattern::Binding(n.clone()),
        HirPatternKind::TypedBinding(n, t) => Pattern::TypedBinding(n.clone(), Box::new(t.clone())),
        HirPatternKind::EnumVariant(n, ps) => Pattern::EnumVariant(
            n.clone(),
            ps.iter().map(pattern).collect::<Option<Vec<_>>>()?,
        ),
        HirPatternKind::Tuple(ps) => {
            Pattern::Tuple(ps.iter().map(pattern).collect::<Option<Vec<_>>>()?)
        }
        HirPatternKind::Range(a, b) => Pattern::Range(expr(a)?, expr(b)?),
        HirPatternKind::RangeInclusive(a, b) => Pattern::RangeInclusive(expr(a)?, expr(b)?),
        HirPatternKind::Multi(ps) => {
            Pattern::Multi(ps.iter().map(pattern).collect::<Option<Vec<_>>>()?)
        }
    })
}

pub fn stmt(s: &HirStmt) -> Option<Statement> {
    Some(match &s.kind {
        HirStmtKind::Let { name, names, ty, expr: e, modifiers } => Statement::Let {
            name: name.clone(),
            names: names.clone(),
            ty: ty.clone(),
            expr: opt_expr(e)?,
            modifiers: modifiers
                .iter()
                .map(|m| Annotation { name: m.clone(), value: None })
                .collect(),
        },
        HirStmtKind::Assign(l, r) => Statement::Assign(expr(l)?, expr(r)?),
        HirStmtKind::ArrowAssign { target, value, consume } => Statement::ArrowAssign {
            target: opt_expr(target)?.map(Box::new),
            value: Box::new(expr(value)?),
            consume: *consume,
        },
        HirStmtKind::Term(e) => Statement::Term(opt_expr(e)?),
        HirStmtKind::Tail(e) => Statement::Tail(expr(e)?),
        HirStmtKind::Check(e) => Statement::Check(expr(e)?),
        HirStmtKind::Expression(e) => Statement::Expression(expr(e)?),
        HirStmtKind::Guarded(c, b) => Statement::Guarded(expr(c)?, stmt_body(b)?),
        HirStmtKind::Block(b) => Statement::Block(stmt_body(b)?),
        HirStmtKind::Foreach { item, list, body } => Statement::Foreach {
            item: item.clone(),
            list: Box::new(expr(list)?),
            body: stmt_body(body)?,
        },
        HirStmtKind::Match { expr: e, arms } => Statement::Match {
            expr: Box::new(expr(e)?),
            arms: arms.iter().map(stmt_match_arm).collect::<Option<Vec<_>>>()?,
        },
        HirStmtKind::Break => Statement::Break,
        HirStmtKind::Trap => Statement::Trap,
        HirStmtKind::Halt => Statement::Halt,
        HirStmtKind::Yield => Statement::Yield,
        HirStmtKind::EndProgram(e) => Statement::EndProgram(opt_expr(e)?),
        HirStmtKind::Gate(e) => Statement::Gate(expr(e)?),
        HirStmtKind::Rollback(e) => Statement::Rollback(opt_expr(e)?),
        HirStmtKind::FreeHint(n) => Statement::FreeHint(n.clone()),
        HirStmtKind::KeepHint(n) => Statement::KeepHint(n.clone()),
        HirStmtKind::SyncBlock(b) => Statement::SyncBlock(stmt_body(b)?),
        HirStmtKind::Defer(b) => Statement::Defer(stmt_body(b)?),
        HirStmtKind::Mutex(b) => Statement::Mutex(stmt_body(b)?),
        HirStmtKind::Open(a, b) => Statement::Open(Box::new(expr(a)?), Box::new(expr(b)?)),
        HirStmtKind::TrgBinding { name, instance } => Statement::TrgBinding {
            name: name.clone(),
            instance: expr(instance)?,
        },
        HirStmtKind::MetadataAssignment(n, v) => {
            Statement::MetadataAssignment(n.clone(), v.clone())
        }
        HirStmtKind::InlineAsm { asm_string, clobbers, span } => Statement::InlineAsm {
            asm_string: asm_string.clone(),
            clobbers: clobbers.clone(),
            span: *span,
        },
        HirStmtKind::Other(_) => return None,
    })
}

pub fn stmt_body(body: &[HirStmt]) -> Option<Vec<Statement>> {
    body.iter().map(stmt).collect()
}
