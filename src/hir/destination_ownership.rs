// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-12 (typed-HIR port): destination ownership analysis over `HirProgram`.
//!
//! Faithful port of `analysis::destination_ownership` (Phase R1,
//! docs/plans/2026-10-29-destination-ownership-reconciliation.md) onto typed
//! HIR. Ownership proofs, destination tracking, and every walk/decision read
//! HIR callables/statements/expressions only — no `hir::to_ast` crossing.
//!
//! The R1 fact types (`ValueId`, `ValueFact`, `CallableSummary`,
//! `CallSiteFact`, diagnostics and their enums) are AST-free and are REUSED
//! from `analysis::destination_ownership` so a later consumer flip is a
//! type-identical swap. The wrapping plan differs in one field:
//! `DestinationOwnershipPlan.generalized` is built by
//! `analysis::destination_model::build(&[TopLevel], …)`, which walks AST
//! items. 2026-11-12: `hir::destination_model` is the HIR-native port and
//! produces the same schema from this plan; this module's
//! `HirDestinationOwnershipPlan` is its legacy-facts input. The parity suite
//! (`tests`) asserts exact canonical equality of the R1 facts between both
//! analyzers.
//!
//! Coverage notes (mirrors the AST analyzer exactly, quirks included):
//! - Analyzed: top-level `defn`/`txn`/`$defn`/`$txn`, `export`-wrapped or
//!   `export`-modified callables, and TypeDef member defns/txns
//!   (owner-qualified `Type::member` names). `isr` handlers, `impl` fns,
//!   trait defaults, `sync<>`-wrapped callables, and standalone op
//!   operators are not (the AST walker never sees them either; op fns have
//!   no HIR callable at all).
//! - `endprogram expr` classifies as Control and its expression is NOT
//!   walked — the AST analyzer has the same dead arm. Ported as-is so the
//!   parity suite can pin the behavior.
//! - Match-arm guards are bound but their expressions are not walked, as in
//!   the AST walker.
//! - `is_primitive` is shared with the AST analyzer (one predicate, two
//!   callers).
//!
//! Undo: delete this module, its `tests` submodule, the `pub mod` line in
//! `src/hir/mod.rs`, the AST-side test-only `canonical_dump`, and restore
//! `is_primitive` visibility; no other code depends on it.

pub(crate) use crate::analysis::destination_ownership::{
    CallSiteFact, CallSiteId, CallableDestinationFacts, CallableKind, CallableSummary,
    DestinationClass, DestinationDiagnostic, DestinationDiagnosticCode, DestinationSeverity,
    EscapeKind, ParamObligation, ResultDestination, SinkKind, ValueFact, ValueId, ValueOrigin,
    is_primitive,
};

use super::program::{HirCallable, HirCallableKind, HirProgram};
use crate::ast::Type;
use std::collections::{BTreeMap, BTreeSet};

mod destination_walk;

#[cfg(test)]
mod tests;

/// Phase R1 facts over typed HIR — the fields of
/// `DestinationOwnershipPlan` that are derivable without the AST.
/// `generalized` (Phase D1) is absent: `destination_model::build` consumes
/// `&[TopLevel]` and needs its own port.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirDestinationOwnershipPlan {
    /// Callable summaries keyed by Briev call name.
    pub callables: BTreeMap<String, CallableSummary>,
    /// Body facts keyed by canonical owner name.
    pub callables_by_owner: BTreeMap<String, CallableDestinationFacts>,
    /// Call-site facts keyed by owner/site.
    pub call_sites: BTreeMap<CallSiteId, CallSiteFact>,
    /// Deterministic diagnostics.
    pub diagnostics: BTreeSet<DestinationDiagnostic>,
}

/// Internal callable signature/body. Bodies are borrowed, not cloned: the
/// HIR program outlives the analysis.
pub(super) struct CallableSpec<'a> {
    pub(super) owner: String,
    pub(super) name: String,
    pub(super) kind: CallableKind,
    pub(super) parameters: Vec<(String, Type)>,
    pub(super) result: Option<Type>,
    pub(super) body: &'a [super::stmt::HirStmt],
    pub(super) exported: bool,
}

/// Compute the Phase R1 destination ownership facts from a HIR program.
pub fn analyze(program: &HirProgram) -> HirDestinationOwnershipPlan {
    let (specs, export_names) = index_callables(program);
    let recursive = recursive_callables(&specs);
    let summaries = summarize_callables(&specs, &recursive);
    let facts = analyze_callables(&specs, &summaries, &recursive);

    let mut plan = HirDestinationOwnershipPlan {
        callables: summaries,
        ..Default::default()
    };
    for fact_set in facts {
        plan.call_sites.extend(fact_set.call_sites.clone());
        plan.diagnostics.extend(fact_set.diagnostics.clone());
        let name = specs_by_owner(&specs, &fact_set.owner);
        plan.callables_by_owner.insert(name, fact_set);
    }
    // AST parity: a name exported anywhere stays exported even when a later
    // same-name definition overwrote its spec (index_callables marks after
    // indexing).
    for name in &export_names {
        if let Some(summary) = plan.callables.get_mut(name) {
            summary.exported = true;
        }
    }
    plan
}

/// Index every callable the AST analyzer would see. Insertion order is
/// program order; same-name later definitions win (BTreeMap insert).
fn index_callables(
    program: &HirProgram,
) -> (BTreeMap<String, CallableSpec<'_>>, BTreeSet<String>) {
    let mut specs = BTreeMap::new();
    let mut export_names = BTreeSet::new();
    for callable in &program.callables {
        // The AST walker never sees these: `isr` handlers, `impl` members,
        // trait defaults, and `sync<>`-wrapped items are separate AST arms
        // index_callables does not match.
        if callable.is_isr || callable.is_impl_fn || callable.trait_default {
            continue;
        }
        if !callable.sync_domains.is_empty() {
            continue;
        }
        let export_marked = callable.exported;
        if export_marked {
            export_names.insert(callable.name.clone());
        }
        let Some(spec) = callable_spec(callable) else {
            continue;
        };
        specs.insert(spec.name.clone(), spec);
    }
    (specs, export_names)
}

/// Build the spec for one lowered callable, or `None` when the HIR callable
/// has no AST-analyzer counterpart (standalone op operators are not lowered
/// as callables at all, so they never reach here).
fn callable_spec(callable: &HirCallable) -> Option<CallableSpec<'_>> {
    let exported_modifier = callable.modifiers.iter().any(|m| m == "export");
    match callable.kind {
        HirCallableKind::Defn | HirCallableKind::CompileTimeDefn => Some(CallableSpec {
            owner: format!("defn:{}", callable.name),
            name: callable.name.clone(),
            kind: CallableKind::InternalDefn,
            parameters: params_of(callable),
            result: callable.result.clone(),
            body: &callable.body.statements,
            exported: exported_modifier || callable.exported,
        }),
        HirCallableKind::Transaction => Some(CallableSpec {
            owner: format!("txn:{}", callable.name),
            name: callable.name.clone(),
            kind: CallableKind::InternalTransaction,
            parameters: params_of(callable),
            result: callable.result.clone(),
            body: &callable.body.statements,
            exported: exported_modifier || callable.exported,
        }),
        HirCallableKind::CompileTimeTxn => Some(CallableSpec {
            owner: format!("ctxn:{}", callable.name),
            name: callable.name.clone(),
            kind: CallableKind::InternalTransaction,
            parameters: params_of(callable),
            result: callable.result.clone(),
            body: &callable.body.statements,
            exported: exported_modifier || callable.exported,
        }),
        HirCallableKind::MemberDefn | HirCallableKind::MemberTransaction => {
            let owner_type = callable.owner.as_ref()?;
            let (kind, owner_prefix) = match callable.kind {
                HirCallableKind::MemberTransaction => (CallableKind::Member, "txn"),
                _ => (CallableKind::Member, "defn"),
            };
            Some(CallableSpec {
                owner: format!("{owner_prefix}:{owner_type}::{}", callable.name),
                name: format!("{owner_type}::{}", callable.name),
                kind,
                parameters: params_of(callable),
                result: callable.result.clone(),
                body: &callable.body.statements,
                // AST member_spec hardcodes exported=false for members.
                exported: false,
            })
        }
        HirCallableKind::IsrHandler => None,
    }
}

fn params_of(callable: &HirCallable) -> Vec<(String, Type)> {
    callable
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty.clone()))
        .collect()
}

fn recursive_callables(specs: &BTreeMap<String, CallableSpec<'_>>) -> BTreeSet<String> {
    specs
        .values()
        .filter(|spec| {
            let mut calls = BTreeSet::new();
            destination_walk::collect_calls(spec.body, &mut calls);
            calls.contains(&spec.name)
        })
        .map(|spec| spec.name.clone())
        .collect()
}

fn summarize_callables(
    specs: &BTreeMap<String, CallableSpec<'_>>,
    recursive: &BTreeSet<String>,
) -> BTreeMap<String, CallableSummary> {
    specs
        .values()
        .map(|spec| {
            let direct_recursive = recursive.contains(&spec.name);
            (spec.name.clone(), summarize_callable(spec, direct_recursive))
        })
        .collect()
}

fn summarize_callable(spec: &CallableSpec<'_>, direct_recursive: bool) -> CallableSummary {
    let parameters = spec
        .parameters
        .iter()
        .map(|(_, ty)| {
            if is_primitive(ty) {
                ParamObligation::Value
            } else {
                ParamObligation::Borrow
            }
        })
        .collect();

    let result = match spec.result.as_ref() {
        None => ResultDestination::Void,
        Some(ty) if is_primitive(ty) => ResultDestination::SsaValue,
        Some(_) => ResultDestination::Unknown,
    };

    CallableSummary {
        name: spec.name.clone(),
        kind: spec.kind,
        parameters,
        result,
        exported: spec.exported,
        direct_recursive,
        may_alias_args: spec.parameters.iter().any(|(_, ty)| !is_primitive(ty)),
    }
}

fn analyze_callables(
    specs: &BTreeMap<String, CallableSpec<'_>>,
    summaries: &BTreeMap<String, CallableSummary>,
    recursive: &BTreeSet<String>,
) -> Vec<CallableDestinationFacts> {
    let exported_owners: BTreeSet<String> = specs
        .values()
        .filter(|spec| spec.exported)
        .map(|spec| spec.owner.clone())
        .collect();
    specs
        .values()
        .map(|spec| {
            destination_walk::analyze_callable(
                spec,
                specs,
                summaries,
                exported_owners.contains(&spec.owner),
                recursive.contains(&spec.name),
            )
        })
        .collect()
}

/// Owner key → call name (the AST analyzer's `specs_by_owner`).
fn specs_by_owner(specs: &BTreeMap<String, CallableSpec<'_>>, owner: &str) -> String {
    specs
        .values()
        .find(|spec| spec.owner == owner)
        .map(|spec| spec.name.clone())
        .unwrap_or_else(|| owner.to_string())
}

/// Test-only canonical dump of the R1 facts, shared with the AST analyzer's
/// dump so the parity suite compares one formatting.
#[cfg(test)]
pub(crate) mod parity_dump {
    use super::*;

    fn value_key(id: &ValueId) -> String {
        format!("{}/{}", id.owner, id.path)
    }

    fn origins_str(fact: &ValueFact) -> String {
        fact.origins
            .iter()
            .map(|origin| format!("{origin:?}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn sinks_str(fact: &ValueFact) -> String {
        fact.sinks
            .iter()
            .map(|(kind, sites)| {
                format!("{}=[{}]", kind.as_str(), sites.clone().into_iter().collect::<Vec<_>>().join(","))
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    fn copies_str(fact: &ValueFact) -> String {
        fact.copy_points
            .iter()
            .map(|(site, escape)| format!("{site}:{escape}"))
            .collect::<Vec<_>>()
            .join(";")
    }

    fn value_line(fact: &ValueFact, id: &ValueId) -> (String, String) {
        let ty = fact
            .ty
            .as_ref()
            .map(|ty| format!("{ty:?}"))
            .unwrap_or_else(|| "-".to_string());
        let tuple = fact
            .tuple_element_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        (
            format!("value/{}", value_key(id)),
            format!(
                "name={};ty={};tuple={};origins=[{}];class={};uses={};overlap={};sinks={{{}}};escapes=[{}];copies={{{}}};reasons=[{}]",
                fact.source_name.as_deref().unwrap_or("-"),
                ty,
                tuple,
                origins_str(fact),
                fact.class.as_str(),
                fact.uses,
                fact.may_overlap,
                sinks_str(fact),
                fact.escapes
                    .iter()
                    .map(|e| e.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                copies_str(fact),
                fact.reasons.clone().into_iter().collect::<Vec<_>>().join(","),
            ),
        )
    }

    fn callsite_line(site_id: &CallSiteId, call: &CallSiteFact) -> (String, String) {
        (
            format!("callsite/{}/{}/{}", site_id.owner, site_id.site, call.callee),
            format!(
                "kind={:?} result={} alias={} obligations=[{}] args=[{}]",
                call.kind,
                call.result.as_str(),
                call.may_alias_args,
                call.obligations
                    .iter()
                    .map(|o| o.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                call.argument_values
                    .iter()
                    .map(value_key)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        )
    }

    fn dump_owner(
        out: &mut BTreeMap<String, String>,
        name: &str,
        facts: &CallableDestinationFacts,
    ) {
        out.insert(
            format!("owner/{name}"),
            format!(
                "exported={} recursive={} params=[{}]",
                facts.exported,
                facts.direct_recursive,
                facts
                    .param_values
                    .iter()
                    .map(value_key)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
        for (id, fact) in &facts.values {
            let (key, line) = value_line(fact, id);
            out.insert(key, line);
        }
        for (site_id, call) in &facts.call_sites {
            let (key, line) = callsite_line(site_id, call);
            out.insert(key, line);
        }
    }

    pub(crate) fn dump(
        callables: &BTreeMap<String, CallableSummary>,
        callables_by_owner: &BTreeMap<String, CallableDestinationFacts>,
        call_sites: &BTreeMap<CallSiteId, CallSiteFact>,
        diagnostics: &BTreeSet<DestinationDiagnostic>,
    ) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (name, summary) in callables {
            out.insert(
                format!("callable/{name}"),
                format!(
                    "kind={:?} exported={} recursive={} result={} alias={} params=[{}]",
                    summary.kind,
                    summary.exported,
                    summary.direct_recursive,
                    summary.result.as_str(),
                    summary.may_alias_args,
                    summary
                        .parameters
                        .iter()
                        .map(|o| o.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            );
        }
        for (name, facts) in callables_by_owner {
            dump_owner(&mut out, name, facts);
        }
        for (site_id, call) in call_sites {
            let (key, line) = callsite_line(site_id, call);
            out.insert(format!("plan-{key}"), line);
        }
        // (owner, path, code, message) is unique inside the BTreeSet — equal
        // diagnostics dedup, so the tuple itself is a collision-free key.
        for diag in diagnostics {
            out.insert(
                format!(
                    "diag/{}/{}/{}/{}",
                    diag.owner,
                    diag.value_path,
                    diag.code.as_str(),
                    diag.message
                ),
                format!("sev={:?}", diag.severity),
            );
        }
        out
    }

}
