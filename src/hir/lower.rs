// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! AST -> typed HIR skeleton lowering. H0a covers top-level callables and
//! export normalization only; bodies are counted, not semantically lowered.

use super::ids::{IdSupply, Span};
use super::program::{
    HirBodySkeleton, HirCallable, HirCallableKind, HirParam, HirProgram,
};
use crate::ast::{Definition, OutputType, TopLevel, Transaction, Type};

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
            body_len: d.body.len(),
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
            body_len: t.body.len(),
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
    body_len: usize,
    span: Option<Span>,
    exported: bool,
    export_symbol: Option<String>,
}

fn lower_callable(ids: &mut IdSupply, spec: CallableLowerSpec<'_>) -> HirCallable {
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
        body: HirBodySkeleton {
            node: ids.node(),
            span: spec.span,
            statement_count: spec.body_len,
        },
        span: spec.span,
    }
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
