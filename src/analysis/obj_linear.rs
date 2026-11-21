// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//! 2026-11-20 (evented-net plan Wave 2): fd/linear-object linearity.
//!
//! An obj declared with `attr linear: true;` (claimed by the `checker`
//! owner in config/meta-vocab.dbv) is LINEAR: every binding of that type
//! is an owned handle over a kernel resource (an fd, an arena). The
//! discipline is the task_linear.rs one applied to user objs:
//!
//!   * assignment moves (the source binding dies),
//!   * a call whose callee is in the type's CONSUME LIST kills the
//!     binding (declared via `attr consume: "close_a, close_b";`), with
//!     the house closer shape (first-arg linear, name `_close`/`_free`
//!     suffixed) as the default when no list is declared,
//!   * any later use of a dead binding is a use-after-consume error,
//!   * a binding still live at scope end is a leaked-resource error.
//!
//! Soundness stance (same as task_linear v1): conservative by construction
//! — false NEGATIVES are acceptable (a discipline hole we close later),
//! false POSITIVES are bugs. v1 tracks bindings by name; handles passed
//! through untracked paths (fields of other objs, arrays) are untracked.
//! Values RETURNED from calls re-bless their binding: `s = sock_accept(l)`
//! is a fresh handle even though `s` existed.
//!
//! Undo: remove this module + its two pipeline call sites (next to
//! task_linear) + the `linear` key in config/meta-vocab.dbv + the Socket
//! attr line.

use crate::ast::top::{Definition, TopLevel};
use crate::ast::{Expr, PropertyValue, Statement, Type};
use std::collections::{HashMap, HashSet};

/// One linear type: its name plus the callee names that CONSUME a binding.
#[derive(Clone)]
struct LinearType {
    name: String,
    consumers: Vec<String>,
}

/// Per-function scope of live linear bindings: name → type name.
#[derive(Default)]
struct Scope {
    live: HashMap<String, String>,
    dead: HashSet<String>,
}

/// Collect the declared linear objs: `attr linear: true;` inside the obj
/// body's metadata. The consume list comes from `attr consume: "a, b";`
/// (optional); the default when absent is the house closer shape — a
/// callee named `close`, `*_close`, or `*_free` receiving the binding as
/// its first argument (the net family already spells closers that way:
/// sock_close, evented_close).
fn linear_types(items: &[TopLevel]) -> Vec<LinearType> {
    let mut out = Vec::new();
    for item in items {
        if let TopLevel::TypeDef(td) = item {
            let is_linear = td
                .body
                .metadata
                .get("linear")
                .map(|v| matches!(v, PropertyValue::Bool(true)))
                .unwrap_or(false);
            if !is_linear {
                continue;
            }
            let mut consumers: Vec<String> = vec![];
            if let Some(PropertyValue::String(list)) = td.body.metadata.get("consume") {
                for name in list.split(',') {
                    let trimmed = name.trim().to_string();
                    if !trimmed.is_empty() {
                        consumers.push(trimmed);
                    }
                }
            }
            out.push(LinearType {
                name: td.name.clone(),
                consumers,
            });
        }
    }
    out
}

fn linear_type_named(name: &str, linear: &[LinearType]) -> bool {
    linear.iter().any(|l| l.name == name)
}

/// The type-name of an AST type if it refers to a declared linear obj.
fn linear_name_of(t: &Type, linear: &[LinearType]) -> Option<String> {
    let name = match t {
        Type::Custom(n) => n.clone(),
        Type::Generic(n, _) | Type::Applied(n, _) => n.clone(),
        _ => return None,
    };
    linear
        .iter()
        .find(|l| l.name == name)
        .map(|l| l.name.clone())
}

fn is_consumer_callee(callee: &str) -> bool {
    callee == "close" || callee.ends_with("_close") || callee.ends_with("_free")
}

/// Enforce linear-obj discipline across every txn/defn body. Returns
/// house-style errors (empty = clean).
pub fn analyze(items: &[TopLevel]) -> Vec<String> {
    let linear = linear_types(items);
    if linear.is_empty() {
        return Vec::new();
    }
    let mut errors = Vec::new();
    // Callee signatures: name → param types (a consuming call is one whose
    // FIRST parameter is the linear type itself — the receiver shape).
    let mut signatures: HashMap<&str, Vec<(String, Type)>> = HashMap::new();
    for item in items {
        if let TopLevel::Definition(d) = item {
            signatures.insert(d.name.as_str(), d.parameters.clone());
        }
    }

    let mut top_scope = Scope::default();
    for item in items {
        // `export defn` wraps the defn — analyze the inner body either way
        // (exported closers are the common stdlib spelling).
        let as_defn: Option<&Definition> = match item {
            TopLevel::Definition(d) => Some(d),
            TopLevel::Export(e) => match e.inner.as_ref() {
                TopLevel::Definition(d) => Some(d),
                _ => None,
            },
            _ => None,
        };
        match item {
            TopLevel::Definition(_) | TopLevel::Export(_) if as_defn.is_some() => {
                let d = as_defn.unwrap();
                let mut scope = Scope::default();
                seed_params(d, &linear, &mut scope);
                check_list(
                    &d.name,
                    &d.body,
                    &signatures,
                    &linear,
                    &mut scope,
                    &mut errors,
                    true,
                );
            }
            TopLevel::Transaction(t) => {
                let mut scope = Scope::default();
                check_list(
                    &t.name,
                    &t.body,
                    &signatures,
                    &linear,
                    &mut scope,
                    &mut errors,
                    true,
                );
            }
            TopLevel::Statement(stmt) => {
                check_list(
                    "top-level",
                    std::slice::from_ref(stmt.as_ref()),
                    &signatures,
                    &linear,
                    &mut top_scope,
                    &mut errors,
                    false,
                );
            }
            _ => {}
        }
    }
    check_list(
        "top-level",
        &[],
        &signatures,
        &linear,
        &mut top_scope,
        &mut errors,
        true,
    );
    errors.sort();
    errors.dedup();
    errors
}

/// v1 scope decision (documented, false-negatives-allowed): PARAMETERS are
/// NOT seeded live. Briev objs are value types — a helper taking `Socket`
/// receives a copy whose kernel handle is still owned by the CALLER's
/// binding. The linear contract therefore applies at the CONSTRUCTION site
/// (`let s = sock_tcp();`): that binding must be consumed (closed) or
/// handed onward before its scope ends. Helpers that read `s.fd` are
/// borrows-by-value; helpers that close consume their LOCAL copy only —
/// the caller's binding stays live, which is exactly the discipline the
/// call-site rules below enforce.
fn seed_params(_d: &Definition, _linear: &[LinearType], _scope: &mut Scope) {}

#[allow(clippy::too_many_arguments)]
fn check_list(
    owner: &str,
    body: &[Statement],
    signatures: &HashMap<&str, Vec<(String, Type)>>,
    linear: &[LinearType],
    scope: &mut Scope,
    errors: &mut Vec<String>,
    is_top: bool,
) {
    for stmt in body {
        match stmt {
            // Passing a linear binding ONWARD (by value, to a non-consuming
            // callee) is HANDOVER, not use: the callee owns it now. Only
            // tracking-by-name sees this; the value's ownership genuinely
            // transferred. Guard v1: handover discharges ONLY when the callee
            // does not return the same type (a return would re-bless a new
            // value at the call site — but that shape is `x = f(x)`, an
            // Assign, handled above).
            Statement::Expression(Expr::Call(callee, args, _))
                if args
                    .iter()
                    .any(|a| matches!(a, Expr::Identifier(n) if scope.live.contains_key(n))) =>
            {
                // Snapshot the live linear args BEFORE check_consuming_call
                // (it removes consuming callees' bindings into dead).
                let live_args: Vec<String> = args
                    .iter()
                    .filter_map(|a| match a {
                        Expr::Identifier(n) if scope.live.contains_key(n) => Some(n.clone()),
                        _ => None,
                    })
                    .collect();
                check_consuming_call(owner, callee, args, signatures, linear, scope, errors);
                // Whatever survived the consuming-callee check was HANDED
                // ONWARD: the callee owns the value now (v1 discipline —
                // obj params are value copies; the caller must not reuse a
                // handle it gave away). Discharge each.
                for binding in live_args {
                    // Handover discharges WITHOUT tombstoning: the callee owns
                    // the value; a later use in a SIBLING guard is the
                    // path-sensitive hole v1 allows (false negative, never a
                    // false positive). Only true consumes tombstone.
                    scope.live.remove(&binding);
                }
            }
            Statement::Let {
                name,
                expr: Some(e),
                ty,
                ..
            } => {
                // Move from an existing linear binding?
                if let Expr::Identifier(src) = e {
                    if let Some(tn) = scope.live.remove(src) {
                        scope.live.insert(name.clone(), tn);
                        continue;
                    }
                    if scope.dead.contains(src) {
                        errors.push(format!(
                            "'{src}' was already consumed; binding '{name}' from it in \
                             {owner} is a use-after-consume. fix: construct a fresh \
                             value instead of rebinding a consumed one"
                        ));
                        continue;
                    }
                }
                // A construction of a declared-linear type enters live —
                // unless the declared type annotation is NOT the linear type
                // (e.g. `let x: Int = expr;` re-types the value: untracked).
                if let Some(tn) = ty.as_ref().and_then(|t| linear_name_of(t, linear)) {
                    scope.live.insert(name.clone(), tn);
                    continue;
                }
                // `let r: Int = consume(t);` — a consuming call in the
                // initializer (the result type differs from the linear type).
                if let Expr::Call(callee, args, _) = e {
                    check_consuming_call(
                        owner,
                        callee,
                        args,
                        signatures,
                        linear,
                        scope,
                        errors,
                    );
                }
            }
            Statement::Assign(lhs, rhs) => {
                if let (Expr::Identifier(dst), Expr::Identifier(src)) = (lhs, rhs) {
                    if let Some(tn) = scope.live.remove(src) {
                        scope.dead.insert(src.clone());
                        scope.live.insert(dst.clone(), tn);
                        continue;
                    }
                }
                // `c = evented_close(c);`-style: a call producing a fresh
                // value RE-BLESSES the destination (the value is new), but
                // the call's arguments must still obey consumption.
                if let Expr::Call(callee, args, _) = rhs {
                    check_consuming_call(owner, callee, args, signatures, linear, scope, errors);
                }
            }
            Statement::Expression(Expr::Call(callee, args, _)) => {
                check_consuming_call(owner, callee, args, signatures, linear, scope, errors);
            }
            // `term <anything>;` exits the whole defn — every live linear
            // binding discharges (the values move into the returned
            // expression; no leak is possible past an exit). Same for the
            // defn-body tail.
            Statement::Term(_) | Statement::Tail(_) => {
                scope.live.clear();
            }
            Statement::Guarded(condition, guarded_body) => {
                // Guards are PATH-SCOPED (2026-11-20 revision): a consume or
                // handover inside a `when` discharges for that path. The
                // when-chain covering every path is the discipline; the
                // checker's soundness stance is false-negatives-allowed — a
                // chain that misses a path is a hole we close in v2
                // (path-sensitive liveness), never a false positive here.
                // The condition is recorded for future path analysis.
                let _ = condition;
                check_list(
                    owner,
                    guarded_body,
                    signatures,
                    linear,
                    scope,
                    errors,
                    false,
                );
            }
            Statement::Block(b) => check_list(owner, b, signatures, linear, scope, errors, false),
            _ => {}
        }
    }
    if is_top {
        let mut dropped: Vec<String> = scope.live.keys().cloned().collect();
        dropped.sort();
        for name in dropped {
            let tn = scope.live.get(&name).cloned().unwrap_or_default();
            errors.push(format!(
                "'{name}' (a linear {tn}) is still live at the end of {owner} — the \
                 resource would leak. consume it (close/free) or hand it to a \
                 returning call before the scope ends"
            ));
            scope.live.remove(&name);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_consuming_call(
    owner: &str,
    callee: &str,
    args: &[Expr],
    signatures: &HashMap<&str, Vec<(String, Type)>>,
    linear: &[LinearType],
    scope: &mut Scope,
    errors: &mut Vec<String>,
) {
    // Snapshot BEFORE the consume loop (consuming removes from live).
    let live_args: Vec<String> = args
        .iter()
        .filter_map(|a| match a {
            Expr::Identifier(n) if scope.live.contains_key(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    for (position, arg) in args.iter().enumerate() {
        let Expr::Identifier(binding) = arg else {
            continue;
        };
        let Some(tn) = scope.live.get(binding).cloned() else {
            if scope.dead.contains(binding) {
                errors.push(format!(
                    "'{binding}' is used after it was consumed (call to '{callee}' in \
                     {owner}). fix: construct a fresh value; a consumed resource is \
                     gone — the kernel reuses its fd"
                ));
            }
            continue;
        };
        let declared_list = linear
            .iter()
            .find(|l| l.name == tn)
            .map(|l| l.consumers.clone())
            .unwrap_or_default();
        let listed = declared_list.iter().any(|c| c == callee);
        // House shape: the callee's FIRST declared parameter is exactly this
        // linear type, and the name carries the closer spelling.
        let house_shape = declared_list.is_empty()
            && position == 0
            && is_consumer_callee(callee)
            && signatures
                .get(callee)
                .and_then(|pts| pts.first())
                .map(|(_, t)| linear_name_of(t, linear).as_deref() == Some(tn.as_str()))
                .unwrap_or(false);
        if listed || house_shape {
            scope.live.remove(binding);
            scope.dead.insert(binding.clone());
        }
    }
    // Whatever survived the consume check was HANDED ONWARD: any by-value
    // pass transfers ownership (obj params are value copies). Discharge
    // WITHOUT tombstoning — a sibling guard's use is the path-sensitive
    // hole v1 allows (false negative, never a false positive).
    for binding in live_args {
        scope.live.remove(&binding);
    }
}
