// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! AST -> typed HIR skeleton lowering. H0b lowers callable bodies
//! structurally; type resolution and normalization are H1.

use super::expr::{HirExpr, HirExprKind, HirLiteral, HirMatchArm};
use super::ids::{IdSupply, Span};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{
    HirBody, HirCallable, HirCallableKind, HirContract, HirEnumVariant, HirField, HirInit,
    HirForeign, HirGlobal, HirOutput, HirParam, HirProgram, HirTypeDef, HirTypeDefKind,
    HirWatchdog,
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
    let mut foreign = Vec::new();
    let mut constants = Vec::new();
    let mut const_exprs = Vec::new();
    let mut const_types = Vec::new();
    let mut inits = Vec::new();
    let mut protocol_binding_fns = Vec::new();
    let mut trigger_names = Vec::new();
    let mut trigger_bindings = Vec::new();
    let mut trait_functions: Vec<(String, Vec<String>)> = Vec::new();
    let has_triggers;
    {
        let mut out = LowerOutputs {
            callables: &mut callables,
            types: &mut type_defs,
            globals: &mut globals,
            foreign: &mut foreign,
            constants: &mut constants,
            const_exprs: &mut const_exprs,
            const_types: &mut const_types,
            inits: &mut inits,
            protocol_binding_fns: &mut protocol_binding_fns,
            trigger_names: &mut trigger_names,
            trigger_bindings: &mut trigger_bindings,
            trait_functions: &mut trait_functions,
            always_stateful: &mut always_stateful,
            has_triggers: false,
        };
        for item in items {
            lower_item(item, false, None, &mut ids, &mut out);
        }
        has_triggers = out.has_triggers;
    }
    HirProgram {
        callables,
        type_defs,
        globals,
        foreign,
        constants,
        const_exprs,
        const_types,
        inits,
        protocol_binding_fns,
        trigger_names,
        trigger_bindings,
        trait_functions,
        always_stateful,
        has_triggers,
    }
}

struct LowerOutputs<'a> {
    callables: &'a mut Vec<HirCallable>,
    types: &'a mut Vec<HirTypeDef>,
    globals: &'a mut Vec<HirGlobal>,
    foreign: &'a mut Vec<HirForeign>,
    constants: &'a mut Vec<String>,
    const_exprs: &'a mut Vec<(String, HirExpr)>,
    const_types: &'a mut Vec<(String, Type)>,
    inits: &'a mut Vec<HirInit>,
    protocol_binding_fns: &'a mut Vec<String>,
    trigger_names: &'a mut Vec<String>,
    trigger_bindings: &'a mut Vec<String>,
    trait_functions: &'a mut Vec<(String, Vec<String>)>,
    always_stateful: &'a mut Vec<String>,
    has_triggers: bool,
}


/// Cast-edge / cross-op bindings named by a protocol definition (emission
/// roots; mirrors the AST index).
fn lower_protocol_bindings(pd: &crate::ast::top::ProtocolDef, out: &mut LowerOutputs<'_>) {
    for edge in &pd.cast_edges {
        if let Some(b) = &edge.binding {
            out.protocol_binding_fns.push(b.fn_name.clone());
        }
    }
    for op in &pd.cross_ops {
        if let Some(pv) = &op.impl_args {
            crate::analysis::defn_liveness::collect_binding_fns(pv, out.protocol_binding_fns);
        }
    }
}

/// TypeDef members (owner-qualified) plus the type declaration itself.
fn lower_typedef_item(td: &crate::ast::TypeDef, ids: &mut IdSupply, out: &mut LowerOutputs<'_>) {
    for member in &td.body.members {
        match member {
            TopLevel::Definition(d) => {
                let mut c = lower_definition(d, HirCallableKind::MemberDefn, false, None, ids);
                c.owner = Some(td.name.clone());
                out.callables.push(c);
            }
            TopLevel::Transaction(t) => {
                let mut c =
                    lower_transaction(t, HirCallableKind::MemberTransaction, false, None, ids);
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

/// Impl members: type-qualified (`Type::member`) keys used by needs_state.
/// `isr<mech> handler @ vector: name(params) [pre][post] { body }` — lowered
/// as a transaction-shaped callable so liveness walks its body edges; the
/// `is_isr` flag roots it (the AST roots the handler name directly).
fn lower_isr_handler(h: &crate::ast::top::IsrHandler, ids: &mut IdSupply) -> HirCallable {
    let contract = lower_contract(&h.contract, ids);
    let mut c = lower_callable(
        ids,
        CallableLowerSpec {
            name: &h.name,
            kind: HirCallableKind::IsrHandler,
            contract: Some(contract),
            is_reactive: false,
            is_async: false,
            parameters: &h.params,
            type_params: Vec::new(),
            variadic: false,
            result: None,
            output_list: Vec::new(),
            outputs: None,
            body: &h.body,
            span: Some(h.span),
            exported: false,
            export_symbol: None,
        },
    );
    c.is_isr = true;
    c
}

fn lower_impl_item(imp: &crate::ast::top::ImplDef, ids: &mut IdSupply, out: &mut LowerOutputs<'_>) {
    for f in &imp.functions {
        if f.body.is_empty() {
            continue;
        }
        let mut c = lower_definition(f, HirCallableKind::MemberDefn, false, None, ids);
        c.owner = Some(imp.target.clone());
        c.is_impl_fn = true;
        out.callables.push(c);
    }
}

/// Trait defaults are indexed by BARE name plus the requirement-slot table.
fn lower_trait_item(tr: &crate::ast::top::TraitDef, ids: &mut IdSupply, out: &mut LowerOutputs<'_>) {
    if !out.trait_functions.iter().any(|(n, _)| n == &tr.name) {
        out.trait_functions.push((
            tr.name.clone(),
            tr.functions.iter().map(|d| d.name.clone()).collect(),
        ));
    }
    for f in &tr.functions {
        if f.body.is_empty() {
            continue;
        }
        let mut c = lower_definition(f, HirCallableKind::MemberDefn, false, None, ids);
        c.owner = Some(tr.name.clone());
        c.trait_default = true;
        out.callables.push(c);
    }
}

/// `sync<domains>`: lower the inner item and tag its callables with the
/// domains (effective_txns parity).
struct SyncGroupCtx<'a> {
    item: &'a TopLevel,
    domains: &'a [String],
    exported: bool,
    export_symbol: Option<String>,
}

fn lower_sync_group(ctx: &SyncGroupCtx<'_>, ids: &mut IdSupply, out: &mut LowerOutputs<'_>) {
    let before = out.callables.len();
    lower_item(ctx.item, ctx.exported, ctx.export_symbol.clone(), ids, out);
    for c in &mut out.callables[before..] {
        c.sync_domains = ctx.domains.to_vec();
    }
}


/// Top-level `let` (non-exported) → state-field leaf global.
fn lower_global_statement(
    stmt: &crate::ast::Statement,
    ids: &mut IdSupply,
    out: &mut LowerOutputs<'_>,
) {
    let crate::ast::Statement::Let { name, ty, expr, modifiers, .. } = stmt else {
        return;
    };
    let declared_storage = modifiers.iter().find_map(|m| match m.name.as_str() {
        "state" => Some("state".to_string()),
        "plain" => Some("plain".to_string()),
        _ => None,
    });
    out.globals.push(HirGlobal {
        node: ids.node(),
        item: ids.item(),
        name: name.clone(),
        ty: ty.clone(),
        expr: expr.as_ref().map(|e| lower_expr(e, ids)),
        declared_storage,
        is_state_decl: false,
        modifiers: modifiers.iter().map(|m| m.name.clone()).collect(),
        span: None,
    });
}


/// Top-level `const`: the name set, initializer, and declared type.
fn lower_constant_item(
    c: &crate::ast::top::Constant,
    ids: &mut IdSupply,
    out: &mut LowerOutputs<'_>,
) {
    out.constants.push(c.name.clone());
    out.const_exprs.push((c.name.clone(), lower_expr(&c.expr, ids)));
    out.const_types.push((c.name.clone(), c.ty.clone()));
}

/// Top-level `init` declaration.
fn lower_init_item(i: &crate::ast::top::InitDecl, ids: &mut IdSupply, out: &mut LowerOutputs<'_>) {
    out.inits.push(HirInit {
        name: i.name.clone(),
        bound: i.bound.clone(),
        value: i.value.as_ref().map(|e| lower_expr(e, ids)),
        body: lower_body(&i.body, ids),
    });
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
        TopLevel::Trigger(trg) => {
            out.has_triggers = true;
            if !out.trigger_names.contains(&trg.name) {
                out.trigger_names.push(trg.name.clone());
            }
        }
        TopLevel::TriggerBinding { name, .. } => {
            if !out.trigger_bindings.contains(name) {
                out.trigger_bindings.push(name.clone());
            }
        }
        TopLevel::ProtocolDef(pd) => lower_protocol_bindings(pd, out),
        TopLevel::Export(export) => {
            let symbol = export.export_name.clone().or_else(|| item_name(&export.inner));
            lower_item(&export.inner, true, symbol, ids, out);
        }
        // 2026-11-05 (H3.4b): `sync<group>` wraps a transaction that the
        // reactor still dispatches. `analysis::effective_txns` walks these;
        // HIR must contain them or live-field/dispatch analyses lose the node.
        TopLevel::SyncGroup { item, domains } => {
            let ctx = SyncGroupCtx { item, domains, exported, export_symbol };
            lower_sync_group(&ctx, ids, out);
        }
        TopLevel::TypeDef(td) => lower_typedef_item(td, ids, out),
        // 2026-11-04 (H3.0): impl members are type-qualified (`Type::member`),
        // the keys the needs_state fixpoint and type_stateful_members use.
        TopLevel::Impl(imp) => lower_impl_item(imp, ids, out),
        // Trait defaults are indexed by BARE name (no type qualification), so a
        // default shared by stateful and stateless conformers stays one node.
        TopLevel::Trait(tr) => lower_trait_item(tr, ids, out),
        TopLevel::IsrHandler(h) => out.callables.push(lower_isr_handler(h, ids)),
        // 2026-11-04 (H3.0): top-level globals are the state-field leaf set
        // for needs_state/storage_class. `constant` items are deliberately
        // excluded (they lower to immutable loads, not %State).
        TopLevel::Statement(stmt) if !exported => lower_global_statement(stmt, ids, out),
        TopLevel::StateDecl(s) if !exported => out.globals.push(HirGlobal {
            node: ids.node(),
            item: ids.item(),
            name: s.name.clone(),
            ty: Some(s.ty.clone()),
            expr: None,
            declared_storage: None,
            is_state_decl: true,
            modifiers: Vec::new(),
            span: s.span,
        }),
        TopLevel::ForeignBinding(fb) => out.foreign.push(HirForeign {
            name: fb
                .briev_name
                .clone()
                .unwrap_or_else(|| fb.foreign_name.clone()),
            params: fb.inputs.clone(),
            ret: fb.success_output.first().map(|(_, t)| t.clone()),
        }),
        TopLevel::Constant(c) => lower_constant_item(c, ids, out),
        TopLevel::Init(i) => lower_init_item(i, ids, out),
        TopLevel::AsmFn(a) => out.always_stateful.push(a.name.clone()),
        TopLevel::BadFn(b) => out.always_stateful.push(b.name.clone()),
        TopLevel::Enum(def) => out.types.push(lower_enum(def, ids)),
        TopLevel::Obj(def) => out.types.push(lower_object(def, ids)),
        TopLevel::StaticStruct(def) => out.types.push(lower_struct(def, ids)),
        _ => {}
    }
}

fn binding_fns(def: &crate::ast::TypeDef) -> Vec<String> {
    let mut out = Vec::new();
    for op in &def.body.operators {
        if let Some(pv) = &op.impl_args {
            crate::analysis::defn_liveness::collect_binding_fns(pv, &mut out);
        }
    }
    out
}

fn lower_type_def(def: &crate::ast::TypeDef, ids: &mut IdSupply) -> Option<HirTypeDef> {
    // A body-less declaration is a boundary alias (`type CStr: String<C_String>`)
    // when it carries a protocol; otherwise it has no HIR shape.
    if def.body.slots.is_empty() {
        let protocol = def.protocol.clone()?;
        return Some(HirTypeDef {
            node: ids.node(),
            item: ids.item(),
            name: def.name.clone(),
            type_params: def.type_params.iter().map(|p| p.name.clone()).collect(),
            kind: HirTypeDefKind::Struct { fields: Vec::new() },
            protocol: Some(protocol),
            is_coll: def.coll,
            traits: def.traits.clone(),
            op_bindings: def.body.op_bindings.iter().map(|b| b.name.clone()).collect(),
            binding_fns: binding_fns(def),
            span: def.span,
        });
    }
    if def.body.slots.iter().all(|slot| slot.name.starts_with("__variant_")) {
        return Some(lower_enum_type_def(def, ids));
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
        protocol: def.protocol.clone(),
        is_coll: def.coll,
        traits: def.traits.clone(),
        op_bindings: def.body.op_bindings.iter().map(|b| b.name.clone()).collect(),
        binding_fns: binding_fns(def),
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
        protocol: def.protocol.clone(),
        is_coll: def.coll,
        traits: def.traits.clone(),
        op_bindings: def.body.op_bindings.iter().map(|b| b.name.clone()).collect(),
        binding_fns: binding_fns(def),
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
        protocol: None,
        is_coll: false,
        traits: Vec::new(),
        op_bindings: Vec::new(),
        binding_fns: Vec::new(),
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
        protocol: None,
        is_coll: false,
        traits: Vec::new(),
        op_bindings: Vec::new(),
        binding_fns: Vec::new(),
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
        protocol: None,
        is_coll: false,
        traits: Vec::new(),
        op_bindings: Vec::new(),
        binding_fns: Vec::new(),
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
            type_params: d.type_params.iter().map(|p| p.name.clone()).collect(),
            variadic: d.variadic_param.is_some(),
            result: result_type(d.output_type.as_ref(), &d.outputs),
            output_list: d.outputs.clone(),
            outputs: d.output_type.as_ref().map(lower_output),
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
            type_params: t.type_params.iter().map(|p| p.name.clone()).collect(),
            variadic: t.variadic_param.is_some(),
            result: result_type(t.output_type.as_ref(), &t.outputs),
            output_list: t.outputs.clone(),
            outputs: t.output_type.as_ref().map(lower_output),
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
    type_params: Vec<String>,
    variadic: bool,
    result: Option<Type>,
    output_list: Vec<Type>,
    outputs: Option<HirOutput>,
    body: &'a [Statement],
    span: Option<Span>,
    exported: bool,
    export_symbol: Option<String>,
}

fn lower_output(o: &OutputType) -> HirOutput {
    match o {
        OutputType::Single(t) => HirOutput::Single(t.clone()),
        OutputType::Union(v) => HirOutput::Union(v.iter().map(lower_output).collect()),
        OutputType::Tuple(v) => HirOutput::Tuple(v.iter().map(lower_output).collect()),
        OutputType::Array(inner) => HirOutput::Array(Box::new(lower_output(inner))),
        OutputType::Named(n, inner) => HirOutput::Named(n.clone(), Box::new(lower_output(inner))),
    }
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
            is_required: w.is_required,
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
        type_params: spec.type_params,
        variadic: spec.variadic,
        is_impl_fn: false,
        is_isr: false,
        sync_domains: Vec::new(),
        result: spec.result,
        output_list: spec.output_list,
        outputs: spec.outputs,
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

/// Lower a standalone statement list (defer bodies registered from the AST
/// path). Fresh ids; deterministic order.
pub fn lower_body_statements(body: &[Statement]) -> Vec<HirStmt> {
    let mut ids = IdSupply::default();
    lower_body(body, &mut ids)
}

fn lower_body(body: &[Statement], ids: &mut IdSupply) -> Vec<HirStmt> {
    body.iter().map(|statement| lower_stmt(statement, ids)).collect()
}

fn lower_stmt(statement: &Statement, ids: &mut IdSupply) -> HirStmt {
    let kind = match statement {
        Statement::Let { name, names, ty, expr, modifiers, .. } => HirStmtKind::Let {
            name: name.clone(),
            names: names.clone(),
            ty: ty.clone(),
            expr: expr.as_ref().map(|expr| lower_expr(expr, ids)),
            modifiers: modifiers.clone(),
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
        Statement::Foreach { item, list, body, modifiers, .. } => HirStmtKind::Foreach {
            item: item.clone(),
            list: lower_expr(list, ids),
            body: lower_body(body, ids),
            modifiers: modifiers.iter().map(|m| m.name.clone()).collect(),
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

/// Lower one standalone expression (closure bodies registered from the AST
/// path). Fresh ids; deterministic order.
pub fn lower_standalone_expr(expr: &Expr) -> HirExpr {
    let mut ids = IdSupply::default();
    lower_expr(expr, &mut ids)
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
        Expr::StructLiteral { type_name, fields, specs, .. } => (None, HirExprKind::StructLiteral {
            type_name: type_name.clone(),
            fields: fields.iter().map(|(name, value)| (name.clone(), lower_expr(value, ids))).collect(),
            specs: specs.iter().map(|(name, value)| (name.clone(), lower_expr(value, ids))).collect(),
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
        Expr::MethodCall(receiver, name, args, analysis_id) => (None, HirExprKind::MethodCall {
            receiver: Box::new(lower_expr(receiver, ids)),
            name: name.clone(),
            args: args.iter().map(|arg| lower_expr(arg, ids)).collect(),
            analysis_id: *analysis_id,
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
        Expr::PluginIntercept { name, args, type_args, receiver } => {
            (
                None,
                HirExprKind::PluginIntercept {
                    name: name.clone(),
                    args: args.iter().map(|a| lower_expr(a, ids)).collect(),
                    type_args: type_args.clone(),
                    receiver: receiver.as_ref().map(|r| Box::new(lower_expr(r, ids))),
                },
            )
        }
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
        guard: arm.guard.as_ref().map(|g| lower_expr(g, ids)),
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
        Expr::Within(..) => "within",
        Expr::DerivationBlock(..) => "derivation",
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
    // 2026-11-05 (H4): PluginIntercept is a fully-represented HIR node now
    // (no longer an `Other` gap), but it is still FRONTEND-ONLY syntax —
    // plugin stages must expand it before any checked consumer (the backend
    // panics on an intercept reaching codegen). Checked lowering therefore
    // rejects it explicitly; unchecked lowering carries the full surface.
    let intercepts = count_plugin_intercepts(&program);
    if intercepts > 0 {
        issues.push(HirLowerIssue { tag: "plugin-intercept", count: intercepts });
    }
    issues.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(b.tag)));
    if issues.is_empty() {
        Ok(program)
    } else {
        Err(issues)
    }
}

fn count_plugin_intercepts(program: &HirProgram) -> usize {
    let mut count = 0usize;
    for c in &program.callables {
        count += callable_intercepts(c);
    }
    for g in &program.globals {
        if g.expr.as_ref().is_some_and(is_intercept) {
            count += 1;
        }
    }
    count
}

/// Body + contract intercept count for one callable.
fn callable_intercepts(c: &HirCallable) -> usize {
    use crate::hir::expr_walk::walk_stmts;
    let mut count = 0usize;
    walk_stmts(&c.body.statements, &mut |e| {
        if is_intercept(e) {
            count += 1;
        }
    });
    if let Some(ct) = &c.contract {
        if is_intercept(&ct.pre) {
            count += 1;
        }
        if is_intercept(&ct.post) {
            count += 1;
        }
    }
    count
}

fn is_intercept(e: &crate::hir::expr::HirExpr) -> bool {
    matches!(e.kind, crate::hir::expr::HirExprKind::PluginIntercept { .. })
}
