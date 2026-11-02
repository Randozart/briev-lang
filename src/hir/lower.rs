// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! AST -> typed HIR skeleton lowering. H0b lowers callable bodies
//! structurally; type resolution and normalization are H1.

use super::expr::{HirExpr, HirExprKind, HirLiteral, HirMatchArm};
use super::ids::{IdSupply, Span};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirBody, HirCallable, HirCallableKind, HirParam, HirProgram};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use crate::ast::{
    Definition, Expr, MatchArm, OutputType, Pattern, Statement, StmtMatchArm,
    TopLevel, Transaction, Type,
};

pub fn lower_program(items: &[TopLevel]) -> HirProgram {
    let mut ids = IdSupply::default();
    let mut callables = Vec::new();
    for item in items {
        lower_item(item, false, None, &mut ids, &mut callables);
    }
    HirProgram { callables }
}

fn lower_item(
    item: &TopLevel,
    exported: bool,
    export_symbol: Option<String>,
    ids: &mut IdSupply,
    out: &mut Vec<HirCallable>,
) {
    match item {
        TopLevel::Definition(d) => out.push(lower_definition(
            d,
            HirCallableKind::Defn,
            exported,
            export_symbol,
            ids,
        )),
        TopLevel::Transaction(t) => out.push(lower_transaction(
            t,
            HirCallableKind::Transaction,
            exported,
            export_symbol,
            ids,
        )),
        TopLevel::CompileTimeDefn(d) => out.push(lower_definition(
            d,
            HirCallableKind::CompileTimeDefn,
            exported,
            export_symbol,
            ids,
        )),
        TopLevel::CompileTimeTxn(t) => out.push(lower_transaction(
            t,
            HirCallableKind::CompileTimeTxn,
            exported,
            export_symbol,
            ids,
        )),
        TopLevel::Export(export) => {
            let symbol = export.export_name.clone().or_else(|| item_name(&export.inner));
            lower_item(&export.inner, true, symbol, ids, out);
        }
        TopLevel::TypeDef(td) => {
            for member in &td.body.members {
                match member {
                    TopLevel::Definition(d) => out.push(lower_definition(
                        d,
                        HirCallableKind::MemberDefn,
                        false,
                        None,
                        ids,
                    )),
                    TopLevel::Transaction(t) => out.push(lower_transaction(
                        t,
                        HirCallableKind::MemberTransaction,
                        false,
                        None,
                        ids,
                    )),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn lower_definition(
    d: &Definition,
    kind: HirCallableKind,
    exported: bool,
    export_symbol: Option<String>,
    ids: &mut IdSupply,
) -> HirCallable {
    lower_callable(
        ids,
        CallableLowerSpec {
            name: &d.name,
            kind,
            parameters: &d.parameters,
            result: result_type(d.output_type.as_ref(), &d.outputs),
            body: &d.body,
            span: d.span,
            exported,
            export_symbol,
        },
    )
}

fn lower_transaction(
    t: &Transaction,
    kind: HirCallableKind,
    exported: bool,
    export_symbol: Option<String>,
    ids: &mut IdSupply,
) -> HirCallable {
    lower_callable(
        ids,
        CallableLowerSpec {
            name: &t.name,
            kind,
            parameters: &t.parameters,
            result: result_type(t.output_type.as_ref(), &t.outputs),
            body: &t.body,
            span: t.span,
            exported,
            export_symbol,
        },
    )
}

struct CallableLowerSpec<'a> {
    name: &'a str,
    kind: HirCallableKind,
    parameters: &'a [(String, Type)],
    result: Option<Type>,
    body: &'a [Statement],
    span: Option<Span>,
    exported: bool,
    export_symbol: Option<String>,
}

fn lower_callable(ids: &mut IdSupply, spec: CallableLowerSpec<'_>) -> HirCallable {
    let body_node = ids.node();
    let statements = lower_body(spec.body, ids);
    HirCallable {
        id: ids.def(),
        item: ids.item(),
        node: ids.node(),
        name: spec.name.to_string(),
        kind: spec.kind,
        params: spec
            .parameters
            .iter()
            .map(|(name, ty)| HirParam {
                name: name.clone(),
                ty: ty.clone(),
            })
            .collect(),
        result: spec.result,
        exported: spec.exported,
        export_symbol: spec.export_symbol,
        body: HirBody {
            node: body_node,
            span: spec.span,
            statements,
        },
        span: spec.span,
    }
}

fn lower_body(body: &[Statement], ids: &mut IdSupply) -> Vec<HirStmt> {
    body.iter().map(|statement| lower_stmt(statement, ids)).collect()
}

fn lower_stmt(statement: &Statement, ids: &mut IdSupply) -> HirStmt {
    let kind = match statement {
        Statement::Let { name, ty, expr, .. } => HirStmtKind::Let {
            name: name.clone(),
            ty: ty.clone(),
            expr: expr.as_ref().map(|expr| lower_expr(expr, ids)),
        },
        Statement::Assign(target, value) => {
            HirStmtKind::Assign(lower_expr(target, ids), lower_expr(value, ids))
        }
        Statement::ArrowAssign {
            target,
            value,
            consume,
        } => HirStmtKind::ArrowAssign {
            target: target.as_ref().map(|target| lower_expr(target, ids)),
            value: lower_expr(value, ids),
            consume: *consume,
        },
        Statement::Term(expr) => HirStmtKind::Term(expr.as_ref().map(|expr| lower_expr(expr, ids))),
        Statement::Tail(expr) => HirStmtKind::Tail(lower_expr(expr, ids)),
        Statement::Check(expr) => HirStmtKind::Check(lower_expr(expr, ids)),
        Statement::Expression(expr) => HirStmtKind::Expression(lower_expr(expr, ids)),
        Statement::Guarded(cond, body) => {
            HirStmtKind::Guarded(lower_expr(cond, ids), lower_body(body, ids))
        }
        Statement::Block(body) => HirStmtKind::Block(lower_body(body, ids)),
        Statement::Foreach { item, list, body } => HirStmtKind::Foreach {
            item: item.clone(),
            list: lower_expr(list, ids),
            body: lower_body(body, ids),
        },
        Statement::Match { expr, arms } => HirStmtKind::Match {
            expr: lower_expr(expr, ids),
            arms: arms.iter().map(|arm| lower_stmt_match_arm(arm, ids)).collect(),
        },
        other => HirStmtKind::Other(stmt_tag(other)),
    };
    HirStmt {
        id: ids.node(),
        span: None,
        kind,
    }
}

fn lower_stmt_match_arm(arm: &StmtMatchArm, ids: &mut IdSupply) -> HirStmtMatchArm {
    HirStmtMatchArm {
        patterns: arm.patterns.iter().map(|pat| lower_pattern(pat, ids)).collect(),
        body: lower_body(&arm.body, ids),
    }
}

fn lower_expr(expr: &Expr, ids: &mut IdSupply) -> HirExpr {
    let (ty, kind) = match expr {
        Expr::Quoted(bytes) => (Some(Type::Custom("String".into())), HirExprKind::Literal(HirLiteral::Bytes(bytes.clone()))),
        Expr::Decimal(value) => (Some(Type::int()), HirExprKind::Literal(HirLiteral::Int(*value))),
        Expr::Char(value) => (Some(Type::Custom("Char".into())), HirExprKind::Literal(HirLiteral::Char(*value))),
        Expr::TaggedLiteral(value, tag) => (Some(Type::int()), HirExprKind::Literal(HirLiteral::TaggedInt(*value, tag.clone()))),
        Expr::TaggedQuotedLiteral(bytes, tag) => (Some(Type::Custom("String".into())), HirExprKind::Literal(HirLiteral::TaggedBytes(bytes.clone(), tag.clone()))),
        Expr::Bool(value) => (Some(Type::Custom("Bool".into())), HirExprKind::Literal(HirLiteral::Bool(*value))),
        Expr::Float(value) => (Some(Type::float()), HirExprKind::Literal(HirLiteral::Float(*value))),
        Expr::BeginProgram => (Some(Type::Custom("Bool".into())), HirExprKind::Literal(HirLiteral::BeginProgram)),
        Expr::Identifier(name) => (None, HirExprKind::Identifier(name.clone())),
        Expr::Call(callee, args, analysis_id) => (None, HirExprKind::Call {
            callee: callee.clone(),
            args: args.iter().map(|arg| lower_expr(arg, ids)).collect(),
            analysis_id: *analysis_id,
        }),
        Expr::BinaryOp(op, left, right) => (None, HirExprKind::Binary {
            op: *op,
            left: Box::new(lower_expr(left, ids)),
            right: Box::new(lower_expr(right, ids)),
        }),
        Expr::UnaryOp(op, inner) => (None, HirExprKind::Unary {
            op: *op,
            inner: Box::new(lower_expr(inner, ids)),
        }),
        Expr::Field(receiver, name) => (None, HirExprKind::Field {
            receiver: Box::new(lower_expr(receiver, ids)),
            name: name.clone(),
        }),
        Expr::Cast(inner, to) => (Some(to.clone()), HirExprKind::Cast {
            inner: Box::new(lower_expr(inner, ids)),
            to: to.clone(),
        }),
        Expr::Tuple(items) => (None, HirExprKind::Tuple(items.iter().map(|item| lower_expr(item, ids)).collect())),
        Expr::List(items) => (None, HirExprKind::List(items.iter().map(|item| lower_expr(item, ids)).collect())),
        Expr::StructLiteral { type_name, fields, .. } => (None, HirExprKind::StructLiteral {
            type_name: type_name.clone(),
            fields: fields.iter().map(|(name, value)| (name.clone(), lower_expr(value, ids))).collect(),
        }),
        Expr::Match(scrutinee, arms) => (None, HirExprKind::Match {
            scrutinee: Box::new(lower_expr(scrutinee, ids)),
            arms: arms.iter().map(|arm| lower_expr_match_arm(arm, ids)).collect(),
        }),
        Expr::Block(body) => (None, HirExprKind::Block(lower_body(body, ids))),
        other => (None, HirExprKind::Other(expr_tag(other))),
    };
    HirExpr {
        id: ids.node(),
        span: None,
        ty,
        kind,
    }
}

fn lower_expr_match_arm(arm: &MatchArm, ids: &mut IdSupply) -> HirMatchArm {
    HirMatchArm {
        pattern: lower_pattern(&arm.pattern, ids),
        body: lower_expr(&arm.body, ids),
    }
}

fn lower_pattern(pattern: &Pattern, ids: &mut IdSupply) -> HirPattern {
    let kind = match pattern {
        Pattern::Wildcard => HirPatternKind::Wildcard,
        Pattern::Literal(expr) => HirPatternKind::Literal(lower_expr(expr, ids)),
        Pattern::Binding(name) => HirPatternKind::Binding(name.clone()),
        Pattern::TypedBinding(name, ty) => HirPatternKind::TypedBinding(name.clone(), ty.as_ref().clone()),
        Pattern::EnumVariant(name, pats) => HirPatternKind::EnumVariant(
            name.clone(),
            pats.iter().map(|pat| lower_pattern(pat, ids)).collect(),
        ),
        Pattern::Tuple(pats) => HirPatternKind::Tuple(
            pats.iter().map(|pat| lower_pattern(pat, ids)).collect(),
        ),
        Pattern::Range(start, end) => HirPatternKind::Range(lower_expr(start, ids), lower_expr(end, ids)),
        Pattern::RangeInclusive(start, end) => HirPatternKind::RangeInclusive(lower_expr(start, ids), lower_expr(end, ids)),
        Pattern::Multi(pats) => HirPatternKind::Multi(
            pats.iter().map(|pat| lower_pattern(pat, ids)).collect(),
        ),
    };
    HirPattern { id: ids.node(), kind }
}

fn result_type(output: Option<&OutputType>, outputs: &[Type]) -> Option<Type> {
    output
        .and_then(|output| output.all_types().into_iter().next())
        .or_else(|| outputs.first().cloned())
}

fn item_name(item: &TopLevel) -> Option<String> {
    match item {
        TopLevel::Definition(d) => Some(d.name.clone()),
        TopLevel::Transaction(t) => Some(t.name.clone()),
        TopLevel::CompileTimeDefn(d) => Some(d.name.clone()),
        TopLevel::CompileTimeTxn(t) => Some(t.name.clone()),
        _ => None,
    }
}

fn stmt_tag(statement: &Statement) -> &'static str {
    match statement {
        Statement::Let { .. } => "let",
        Statement::Assign(..) => "assign",
        Statement::ArrowAssign { .. } => "arrow-assign",
        Statement::Term(_) => "term",
        Statement::Tail(_) => "tail",
        Statement::Check(_) => "check",
        Statement::Expression(_) => "expression",
        Statement::Guarded(..) => "guarded",
        Statement::Block(_) => "block",
        Statement::Foreach { .. } => "foreach",
        Statement::Match { .. } => "match",
        _ => "other-statement",
    }
}

fn expr_tag(expr: &Expr) -> &'static str {
    match expr {
        Expr::MethodCall(..) => "method-call",
        Expr::Reflect(..) => "reflect",
        Expr::Index(..) => "index",
        Expr::Slice { .. } => "slice",
        Expr::If(..) => "if",
        Expr::Spawn { .. } => "spawn",
        Expr::Lambda(..) => "lambda",
        Expr::PluginIntercept { .. } => "plugin-intercept",
        _ => "other-expression",
    }
}
