// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! AST -> typed HIR skeleton lowering. H0b lowers callable bodies
//! structurally; type resolution and normalization are H1.

use super::expr::{HirExpr, HirExprKind, HirLiteral, HirMatchArm};
use super::ids::{IdSupply, Span};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{
    HirBody, HirCallable, HirCallableKind, HirContract, HirEnumVariant, HirField,
    HirGlobal, HirParam, HirProgram, HirTypeDef, HirTypeDefKind, HirWatchdog,
    HirWatchdogOnFire,
};
use super::stmt::{HirStmt, HirStmtKind, HirStmtMatchArm};
use crate::ast::{
    Contract, Definition, Expr, MatchArm, OutputType, Pattern, Statement, StmtMatchArm,
    TopLevel, Transaction, Type,
};

pub fn lower_program(items: &[TopLevel]) -> HirProgram {
    let mut ids = IdSupply::default();
    let mut callables = Vec::new();
    let mut type_defs = Vec::new();
    let mut globals = Vec::new();
    let mut always_stateful = Vec::new();
    {
        let mut out = LowerOutputs {
            callables: &mut callables,
            types: &mut type_defs,
            globals: &mut globals,
            always_stateful: &mut always_stateful,
        };
        for item in items {
            lower_item(item, false, None, &mut ids, &mut out);
        }
    }
    HirProgram { callables, type_defs, globals, always_stateful }
}

struct LowerOutputs<'a> {
    callables: &'a mut Vec<HirCallable>,
    types: &'a mut Vec<HirTypeDef>,
    globals: &'a mut Vec<HirGlobal>,
    always_stateful: &'a mut Vec<String>,
}

fn lower_item(
    item: &TopLevel,
    exported: bool,
    export_symbol: Option<String>,
    ids: &mut IdSupply,
    out: &mut LowerOutputs<'_>,
) {
    match item {
        TopLevel::Definition(d) => out.callables.push(lower_definition(
            d,
            HirCallableKind::Defn,
            exported,
            export_symbol,
            ids,
        )),
        TopLevel::Transaction(t) => out.callables.push(lower_transaction(
            t,
            HirCallableKind::Transaction,
            exported,
            export_symbol,
            ids,
        )),
        TopLevel::CompileTimeDefn(d) => out.callables.push(lower_definition(
            d,
            HirCallableKind::CompileTimeDefn,
            exported,
            export_symbol,
            ids,
        )),
        TopLevel::CompileTimeTxn(t) => out.callables.push(lower_transaction(
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
                    TopLevel::Definition(d) => {
                        let mut c = lower_definition(
                            d,
                            HirCallableKind::MemberDefn,
                            false,
                            None,
                            ids,
                        );
                        c.owner = Some(td.name.clone());
                        out.callables.push(c);
                    }
                    TopLevel::Transaction(t) => {
                        let mut c = lower_transaction(
                            t,
                            HirCallableKind::MemberTransaction,
                            false,
                            None,
                            ids,
                        );
                        c.owner = Some(td.name.clone());
                        out.callables.push(c);
                    }
                    _ => {}
                }
            }
            if let Some(decl) = lower_type_def(td, ids) {
                out.types.push(decl);
            }
        }
        // 2026-11-04 (H3.0): impl members are type-qualified (`Type::member`),
        // the keys the needs_state fixpoint and type_stateful_members use.
        TopLevel::Impl(imp) => {
            for f in &imp.functions {
                if f.body.is_empty() {
                    continue;
                }
                let mut c = lower_definition(
                    f,
                    HirCallableKind::MemberDefn,
                    false,
                    None,
                    ids,
                );
                c.owner = Some(imp.target.clone());
                out.callables.push(c);
            }
        }
        // Trait defaults are indexed by BARE name (no type qualification), so a
        // default shared by stateful and stateless conformers stays one node.
        TopLevel::Trait(tr) => {
            for f in &tr.functions {
                if f.body.is_empty() {
                    continue;
                }
                let mut c = lower_definition(
                    f,
                    HirCallableKind::MemberDefn,
                    false,
                    None,
                    ids,
                );
                c.owner = Some(tr.name.clone());
                c.trait_default = true;
                out.callables.push(c);
            }
        }
        // 2026-11-04 (H3.0): top-level globals are the state-field leaf set
        // for needs_state/storage_class. `constant` items are deliberately
        // excluded (they lower to immutable loads, not %State).
        TopLevel::Statement(stmt) if !exported => {
            if let crate::ast::Statement::Let { name, ty, .. } = stmt.as_ref() {
                out.globals.push(HirGlobal {
                    node: ids.node(),
                    item: ids.item(),
                    name: name.clone(),
                    ty: ty.clone(),
                    expr: None,
                    is_state_decl: false,
                    span: None,
                });
            }
        }
        TopLevel::StateDecl(s) if !exported => out.globals.push(HirGlobal {
            node: ids.node(),
            item: ids.item(),
            name: s.name.clone(),
            ty: Some(s.ty.clone()),
            expr: None,
            is_state_decl: true,
            span: s.span,
        }),
        TopLevel::AsmFn(a) => out.always_stateful.push(a.name.clone()),
        TopLevel::BadFn(b) => out.always_stateful.push(b.name.clone()),
        TopLevel::Enum(def) => out.types.push(lower_enum(def, ids)),
        TopLevel::Obj(def) => out.types.push(lower_object(def, ids)),
        TopLevel::StaticStruct(def) => out.types.push(lower_struct(def, ids)),
        _ => {}
    }
}

fn lower_type_def(def: &crate::ast::TypeDef, ids: &mut IdSupply) -> Option<HirTypeDef> {
    if def.body.slots.iter().all(|slot| slot.name.starts_with("__variant_")) {
        return Some(lower_enum_type_def(def, ids));
    }
    if def.body.slots.is_empty() {
        return None;
    }
    Some(HirTypeDef {
        node: ids.node(),
        item: ids.item(),
        name: def.name.clone(),
        type_params: def.type_params.iter().map(|p| p.name.clone()).collect(),
        kind: HirTypeDefKind::Struct {
            fields: def
                .body
                .slots
                .iter()
                .map(|slot| HirField {
                    name: slot.name.clone(),
                    ty: slot.ty.clone(),
                })
                .collect(),
        },
        span: def.span,
    })
}

fn lower_enum_type_def(def: &crate::ast::TypeDef, ids: &mut IdSupply) -> HirTypeDef {
    let variants = def
        .body
        .slots
        .iter()
        .map(|slot| {
            let name = slot.name.trim_start_matches("__variant_").to_string();
            match &slot.ty {
                Type::Void => HirEnumVariant::Unit(name),
                Type::Tuple(types) => HirEnumVariant::Tuple(name, types.clone()),
                ty => HirEnumVariant::Tuple(name, vec![ty.clone()]),
            }
        })
        .collect();
    HirTypeDef {
        node: ids.node(),
        item: ids.item(),
        name: def.name.clone(),
        type_params: def.type_params.iter().map(|p| p.name.clone()).collect(),
        kind: HirTypeDefKind::Enum { variants },
        span: def.span,
    }
}

fn lower_enum(def: &crate::ast::EnumDefinition, ids: &mut IdSupply) -> HirTypeDef {
    let variants = def
        .variants
        .iter()
        .map(|variant| match variant {
            crate::ast::EnumVariant::Unit(name) => HirEnumVariant::Unit(name.clone()),
            crate::ast::EnumVariant::Tuple(name, types) => {
                HirEnumVariant::Tuple(name.clone(), types.clone())
            }
            crate::ast::EnumVariant::Struct(name, fields) => HirEnumVariant::Struct(
                name.clone(),
                fields
                    .iter()
                    .map(|(name, ty)| HirField {
                        name: name.clone(),
                        ty: ty.clone(),
                    })
                    .collect(),
            ),
        })
        .collect();
    HirTypeDef {
        node: ids.node(),
        item: ids.item(),
        name: def.name.clone(),
        type_params: def.type_params.iter().map(|p| p.name.clone()).collect(),
        kind: HirTypeDefKind::Enum { variants },
        span: def.span,
    }
}

fn lower_object(def: &crate::ast::StructDefinition, ids: &mut IdSupply) -> HirTypeDef {
    HirTypeDef {
        node: ids.node(),
        item: ids.item(),
        name: def.name.clone(),
        type_params: def.type_params.clone(),
        kind: HirTypeDefKind::Object {
            fields: def
                .fields
                .iter()
                .map(|field| HirField {
                    name: field.name.clone(),
                    ty: field.ty.clone(),
                })
                .collect(),
        },
        span: def.span,
    }
}

fn lower_struct(def: &crate::ast::StructDef, ids: &mut IdSupply) -> HirTypeDef {
    HirTypeDef {
        node: ids.node(),
        item: ids.item(),
        name: def.name.clone(),
        type_params: def.type_params.iter().map(|p| p.name.clone()).collect(),
        kind: HirTypeDefKind::Struct {
            fields: def
                .fields
                .iter()
                .map(|(name, ty)| HirField {
                    name: name.clone(),
                    ty: ty.clone(),
                })
                .collect(),
        },
        span: def.span,
    }
}

fn lower_definition(
    d: &Definition,
    kind: HirCallableKind,
    exported: bool,
    export_symbol: Option<String>,
    ids: &mut IdSupply,
) -> HirCallable {
    let contract = lower_contract(&d.contract, ids);
    lower_callable(
        ids,
        CallableLowerSpec {
            name: &d.name,
            kind,
            contract: Some(contract),
            is_reactive: false,
            is_async: false,
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
    let contract = lower_contract(&t.contract, ids);
    lower_callable(
        ids,
        CallableLowerSpec {
            name: &t.name,
            kind,
            contract: Some(contract),
            is_reactive: t.is_reactive,
            is_async: t.is_async,
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
    contract: Option<HirContract>,
    is_reactive: bool,
    is_async: bool,
    parameters: &'a [(String, Type)],
    result: Option<Type>,
    body: &'a [Statement],
    span: Option<Span>,
    exported: bool,
    export_symbol: Option<String>,
}

fn lower_contract(c: &Contract, ids: &mut IdSupply) -> HirContract {
    let pre = lower_expr(&c.pre_condition, ids);
    let post = lower_expr(&c.post_condition, ids);
    let watchdog = c.watchdog.as_ref().map(|w| {
        let condition = lower_expr(&w.condition, ids);
        let fallback = w.fallback.as_deref().map(|e| lower_expr(e, ids));
        HirWatchdog {
            condition,
            fallback,
            on_fire: w.on_fire.as_ref().map(|f| HirWatchdogOnFire {
                handler: f.handler.clone(),
                arg: f.arg.clone(),
            }),
        }
    });
    HirContract { pre, post, watchdog }
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
        contract: spec.contract,
        is_reactive: spec.is_reactive,
        is_async: spec.is_async,
        owner: None,
        trait_default: false,
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
        Statement::Break => HirStmtKind::Break,
        Statement::Trap => HirStmtKind::Trap,
        Statement::Halt => HirStmtKind::Halt,
        Statement::Yield => HirStmtKind::Yield,
        Statement::EndProgram(expr) => HirStmtKind::EndProgram(expr.as_ref().map(|expr| lower_expr(expr, ids))),
        Statement::Gate(expr) => HirStmtKind::Gate(lower_expr(expr, ids)),
        Statement::Rollback(expr) => HirStmtKind::Rollback(expr.as_ref().map(|expr| lower_expr(expr, ids))),
        Statement::FreeHint(name) => HirStmtKind::FreeHint(name.clone()),
        Statement::KeepHint(name) => HirStmtKind::KeepHint(name.clone()),
        Statement::SyncBlock(body) => HirStmtKind::SyncBlock(lower_body(body, ids)),
        Statement::Defer(body) => HirStmtKind::Defer(lower_body(body, ids)),
        Statement::Mutex(body) => HirStmtKind::Mutex(lower_body(body, ids)),
        Statement::Open(left, right) => HirStmtKind::Open(lower_expr(left, ids), lower_expr(right, ids)),
        Statement::TrgBinding { name, instance } => HirStmtKind::TrgBinding {
            name: name.clone(),
            instance: lower_expr(instance, ids),
        },
        Statement::MetadataAssignment(name, value) => HirStmtKind::MetadataAssignment(name.clone(), value.clone()),
        Statement::InlineAsm { asm_string, clobbers, span } => HirStmtKind::InlineAsm {
            asm_string: asm_string.clone(),
            clobbers: clobbers.clone(),
            span: *span,
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
            via: None,
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
        other => return lower_special_expr(other, ids),
    };
    HirExpr {
        id: ids.node(),
        span: None,
        ty,
        kind,
    }
}

fn lower_special_expr(expr: &Expr, ids: &mut IdSupply) -> HirExpr {
    let (ty, kind) = match expr {
        Expr::MethodCall(receiver, name, args, analysis_id, chain_refs) => (None, HirExprKind::MethodCall {
            receiver: Box::new(lower_expr(receiver, ids)),
            name: name.clone(),
            args: args.iter().map(|arg| lower_expr(arg, ids)).collect(),
            analysis_id: *analysis_id,
            chain_refs: chain_refs.clone(),
        }),
        Expr::Reflect(receiver, name, kind) => (None, HirExprKind::Reflect {
            receiver: Box::new(lower_expr(receiver, ids)),
            name: name.clone(),
            kind: *kind,
        }),
        Expr::Index(array, index) => (None, HirExprKind::Index(
            Box::new(lower_expr(array, ids)),
            Box::new(lower_expr(index, ids)),
        )),
        Expr::Slice { array, start, end, stride } => (None, HirExprKind::Slice {
            array: Box::new(lower_expr(array, ids)),
            start: start.as_ref().map(|expr| Box::new(lower_expr(expr, ids))),
            end: end.as_ref().map(|expr| Box::new(lower_expr(expr, ids))),
            stride: stride.as_ref().map(|expr| Box::new(lower_expr(expr, ids))),
        }),
        Expr::Range { start, end, inclusive } => (None, HirExprKind::Range {
            start: Box::new(lower_expr(start, ids)),
            end: Box::new(lower_expr(end, ids)),
            inclusive: *inclusive,
        }),
        Expr::Spawn { type_name, args, storage } => (None, HirExprKind::Spawn {
            type_name: type_name.clone(),
            args: args.iter().map(|arg| lower_expr(arg, ids)).collect(),
            storage: *storage,
        }),
        Expr::If(condition, then_branch, else_branch) => (None, HirExprKind::If {
            condition: Box::new(lower_expr(condition, ids)),
            then_branch: Box::new(lower_expr(then_branch, ids)),
            else_branch: else_branch.as_ref().map(|expr| Box::new(lower_expr(expr, ids))),
        }),
        Expr::Lambda(params, body) => (None, HirExprKind::Lambda {
            params: params.clone(),
            body: Box::new(lower_expr(body, ids)),
        }),
        Expr::IsType(inner, ty) => (None, HirExprKind::IsType {
            inner: Box::new(lower_expr(inner, ids)),
            ty: ty.clone(),
        }),
        Expr::Deref(inner) => (None, HirExprKind::Deref(Box::new(lower_expr(inner, ids)))),
        Expr::AddrOf(inner) => (None, HirExprKind::AddrOf(Box::new(lower_expr(inner, ids)))),
        Expr::Consume(inner) => (None, HirExprKind::Consume(Box::new(lower_expr(inner, ids)))),
        Expr::Await(inner) => (None, HirExprKind::Await(Box::new(lower_expr(inner, ids)))),
        Expr::UnitLiteral { value, unit } => (Some(Type::float()), HirExprKind::UnitLiteral {
            value: *value,
            unit: unit.clone(),
        }),
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
        Statement::InlineDefn(_) => "inline-defn",
        _ => "other-statement",
    }
}

fn expr_tag(expr: &Expr) -> &'static str {
    match expr {
        Expr::PluginIntercept { .. } => "plugin-intercept",
        Expr::Within(..) => "within",
        Expr::DerivationBlock(..) => "derivation",
        Expr::Capture { .. } => "capture",
        Expr::FormattingAnnotation(..) => "formatting-annotation",
        Expr::Exists(..) => "exists",
        _ => "other-expression",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirLowerIssue {
    pub tag: &'static str,
    pub count: usize,
}

/// Checked lowering for consumers: every `Other(tag)` is a frontend
/// normalization gap. Corpus audits may use unchecked lowering to measure it.
pub fn lower_program_checked(items: &[TopLevel]) -> Result<HirProgram, Vec<HirLowerIssue>> {
    let program = lower_program(items);
    let counts = super::stats::unsupported_counts(&program);
    let mut issues: Vec<HirLowerIssue> = counts
        .expression_tags
        .into_iter()
        .map(|(tag, count)| HirLowerIssue { tag, count })
        .chain(
            counts
                .statement_tags
                .into_iter()
                .map(|(tag, count)| HirLowerIssue { tag, count }),
        )
        .collect();
    issues.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(b.tag)));
    if issues.is_empty() {
        Ok(program)
    } else {
        Err(issues)
    }
}
