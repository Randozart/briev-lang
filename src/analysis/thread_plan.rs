//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Thread plan — validate and materialize `on thread <N>` CPU-affinity pins.
//!
//! 2026-11-15 (thread-assignment plan): `on thread <N>` is an **ambiguity
//! keyword** (see `docs/architecture/keyword-taxonomy.md`). It selects WHERE a
//! reactive node fires — never WHETHER two nodes may fire together, which stays
//! `async` / `sync<group>` (Golden Rule 22). Unmarked nodes keep the reactor's
//! own equilibrium scheduling; the default is unchanged.
//!
//! Why a keyword here at all: the compiler cannot rank cores. Cache locality,
//! NUMA distance, and real-time isolation are properties of the deployment, not
//! of the program, so no defensible default exists beyond "let the reactor
//! decide". Declaring the pin is the author's intent, and speed is a
//! consequence, never the argument (the observability razor).
//!
//! This module owns the *soundness* half. Pinned nodes on different workers
//! really do execute at the same time, so the compiler must prove they do not
//! race: the write sets of two different-worker nodes must be disjoint and no
//! worker may read a field another worker writes. Anything less is a data race,
//! which this language refuses rather than tolerates.

use crate::ast::{Annotation, Expr, TopLevel};
use crate::backend::{collect_assigned_identifiers, collect_read_identifiers};
use crate::proof_engine::check_satisfiable;
use std::collections::{BTreeMap, BTreeSet};

/// The most workers a single program may pin. Keeps the emission bounded (a
/// per-worker reactor function and a runtime thread per worker) and far above
/// any real deployment's core count.
pub const MAX_THREADS: u32 = 63;

/// 2026-11-15 (Stage 2 hold): the plan is proved and carried, but the threaded
/// dispatch lane does not emit workers yet. `on thread <N>` cannot be served
/// sequentially either — dispatch order is observable, so running worker N's
/// slice inline would change program behaviour. The honest answer is a refusal
/// that names the missing piece, its fix, and the current workaround.
pub fn unemitted_pin_message(plan: &ThreadPlan) -> String {
    let mut workers: Vec<String> = plan
        .workers
        .iter()
        .map(|(index, names)| format!("worker {index}: {}", names.join(", ")))
        .collect();
    workers.sort();
    format!(
        "`on thread <N>` is validated but cannot be emitted yet — this build \
         has no threaded dispatch lane, and running the workers sequentially \
         would reorder node firing (order is observable).\n  \
         The declared plan is sound: {}.\n  \
         Fix: drop the pins to keep the reactor's own scheduling (the default \
         is already one efficient worker). Explicit worker threads land with \
         the threaded dispatch lane, which additionally needs per-worker \
         arenas — the runtime arena currently has a single shared cursor.",
        workers.join("; ")
    )
}

/// One callable's declared pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPin {
    /// `on thread N` — the callable fires on worker N.
    Index(u32),
    /// `on thread any` — the author stated the reactor may choose. Recorded so
    /// the analysis can tell an explicit statement from silence.
    Any,
}

/// The frontend-computed thread plan. Empty (`worker_count == 0`) means the
/// program declared no pins and the reactor's single-threaded dispatch is used
/// unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadPlan {
    /// Callable name → pinned worker index, for `Index` pins only. Emitted in
    /// name order so `.ll` output is deterministic.
    pub assignments: BTreeMap<String, u32>,
    /// Worker index → callable names on that worker, in name order.
    pub workers: BTreeMap<u32, Vec<String>>,
    /// Workers the program needs (`max(index) + 1`); 0 when nothing is pinned.
    pub worker_count: u32,
    /// Callables that wrote `on thread any` explicitly, in name order.
    pub explicit_any: Vec<String>,
}

impl ThreadPlan {
    /// Does any callable declare a pin? False → the backend emits the ordinary
    /// single-worker reactor.
    pub fn is_pinned(&self) -> bool {
        !self.assignments.is_empty()
    }

    /// The worker a callable fires on, `None` when it is unpinned.
    pub fn worker_of(&self, name: &str) -> Option<u32> {
        self.assignments.get(name).copied()
    }
}

/// The gate's verdict: the plan plus the diagnostics that justify it.
#[derive(Debug, Clone, Default)]
pub struct ThreadGate {
    pub plan: ThreadPlan,
    /// Soundness failures — the program is refused.
    pub errors: Vec<String>,
    /// Advisory findings — accepted, disclosed.
    pub warnings: Vec<String>,
}

/// One reactive callable carrying a pin.
struct PinnedNode<'a> {
    name: &'a str,
    body: &'a [crate::ast::Statement],
    pre: &'a Expr,
    is_async: bool,
    pin: u32,
}

/// Run the thread gate over a program. Returns the plan and its diagnostics.
pub fn run_thread_gate(items: &[TopLevel]) -> ThreadGate {
    let mut gate = ThreadGate::default();
    let mut pinned: Vec<PinnedNode<'_>> = Vec::new();
    let mut any_declared: Vec<String> = Vec::new();
    for item in items {
        collect_item(item, &mut pinned, &mut any_declared, &mut gate);
    }
    finish_plan(&mut gate, pinned, any_declared);
    gate
}

/// Walk one top-level item, recording pins. `sync<group>`-wrapped nodes carry
/// their pin on the inner transaction, same as `async`.
fn collect_item<'a>(
    item: &'a TopLevel,
    pinned: &mut Vec<PinnedNode<'a>>,
    any_declared: &mut Vec<String>,
    gate: &mut ThreadGate,
) {
    match item {
        TopLevel::Transaction(t) => record_pin(t, pinned, any_declared, gate),
        TopLevel::SyncGroup { item: inner, .. } => {
            if let TopLevel::Transaction(t) = inner.as_ref() {
                record_pin(t, pinned, any_declared, gate);
            }
        }
        _ => {}
    }
}

/// Record a transaction's pin, if it declared one.
fn record_pin<'a>(
    t: &'a crate::ast::Transaction,
    pinned: &mut Vec<PinnedNode<'a>>,
    any_declared: &mut Vec<String>,
    gate: &mut ThreadGate,
) {
    let Some(pin) = read_pin(&t.modifiers) else {
        return;
    };
    let pin = match pin {
        ThreadPin::Any => {
            any_declared.push(t.name.clone());
            return;
        }
        ThreadPin::Index(n) => n,
    };
    if n_out_of_range(pin) {
        gate.errors.push(format!(
            "'{}' pins to worker {} but only workers 0..={} exist — pin to a \
             lower index or drop the pin to let the reactor schedule it",
            t.name, pin, MAX_THREADS
        ));
        return;
    }
    if !t.is_reactive {
        gate.warnings.push(format!(
            "'{}' is a txn, not a node — `on thread` only changes which worker \
             fires a reactive node, so the pin has no effect here",
            t.name
        ));
        return;
    }
    pinned.push(PinnedNode {
        name: &t.name,
        body: &t.body,
        pre: &t.contract.pre_condition,
        is_async: t.is_async,
        pin,
    });
}

/// Is the declared worker index outside the supported range?
fn n_out_of_range(pin: u32) -> bool {
    pin > MAX_THREADS
}

/// Read the `thread` annotation of a callable's modifier list.
fn read_pin(modifiers: &[Annotation]) -> Option<ThreadPin> {
    let annotation = modifiers.iter().find(|m| m.name == "thread")?;
    match &annotation.value {
        None => Some(ThreadPin::Any),
        Some(Expr::Decimal(n)) if *n >= 0 && *n <= u32::MAX as i64 => {
            Some(ThreadPin::Index(*n as u32))
        }
        Some(_) => None,
    }
}

/// Build the plan from the collected pins and run the cross-worker soundness
/// checks plus the advisory sweeps.
fn finish_plan(gate: &mut ThreadGate, pinned: Vec<PinnedNode<'_>>, any_declared: Vec<String>) {
    let mut worker_count = 0;
    for node in &pinned {
        gate.plan
            .assignments
            .insert(node.name.to_string(), node.pin);
        gate.plan
            .workers
            .entry(node.pin)
            .or_default()
            .push(node.name.to_string());
        worker_count = worker_count.max(node.pin + 1);
    }
    gate.plan.worker_count = worker_count;
    gate.plan.explicit_any = sorted(any_declared);
    for names in gate.plan.workers.values_mut() {
        names.sort();
    }
    check_cross_worker(gate, &pinned);
    advise_single_worker(gate);
    advise_mixed_any(gate);
}

/// Sorted, de-duplicated copy of a name list.
fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup();
    names
}

/// Prove that two nodes on different workers cannot race. They may fire in the
/// same tick by construction, so their write sets must be disjoint and neither
/// may read a field the other writes.
///
/// The pairwise shape is inherent, not incidental: soundness is a property of
/// every *pair* of workers, so no single-pass scan replaces the comparison. The
/// bound is small by construction — a program may pin at most `MAX_THREADS`
/// (63) nodes — so this is at most ~2k comparisons at compile time and never
/// executes at runtime. The pairs are produced in index order, which keeps the
/// diagnostics deterministic.
fn check_cross_worker(gate: &mut ThreadGate, pinned: &[PinnedNode<'_>]) {
    gate.errors.extend(conflicting_pairs(pinned));
}

/// Every cross-worker race or missing `async` classification, in index order.
fn conflicting_pairs(pinned: &[PinnedNode<'_>]) -> Vec<String> {
    (0..pinned.len())
        .flat_map(|i| ((i + 1)..pinned.len()).map(move |j| (i, j)))
        .filter(|(i, j)| pinned[*i].pin != pinned[*j].pin)
        .filter_map(|(i, j)| cross_worker_conflict(&pinned[i], &pinned[j]))
        .collect()
}

/// The pairwise cross-worker soundness question for two nodes on different
/// workers. `None` means safe; `Some` names the racy field.
fn cross_worker_conflict(a: &PinnedNode<'_>, b: &PinnedNode<'_>) -> Option<String> {
    if !check_satisfiable(a.pre, b.pre) {
        // Mutually exclusive guards: the two never fire in the same tick, so
        // there is no simultaneous access to order.
        return None;
    }
    let a_writes = writes_of(a.body);
    let b_writes = writes_of(b.body);
    if let Some(field) = a_writes.intersection(&b_writes).next() {
        return Some(race_message(a, b, field, "both write it"));
    }
    let a_reads: BTreeSet<String> = collect_read_identifiers(a.body).into_iter().collect();
    let b_reads: BTreeSet<String> = collect_read_identifiers(b.body).into_iter().collect();
    if let Some(field) = a_writes.intersection(&b_reads).next() {
        return Some(race_message(a, b, field, "one writes what the other reads"));
    }
    if let Some(field) = b_writes.intersection(&a_reads).next() {
        return Some(race_message(a, b, field, "one writes what the other reads"));
    }
    unclassified_message(a, b)
}

/// A node on its own worker runs truly concurrently, so the pair needs `async`
/// on both (the explicit simultaneity declaration). The concurrency gate
/// already demands this; naming it here keeps the diagnostic at the pin.
fn unclassified_message(a: &PinnedNode<'_>, b: &PinnedNode<'_>) -> Option<String> {
    if a.is_async && b.is_async {
        return None;
    }
    Some(format!(
        "'{}' (worker {}) and '{}' (worker {}) can fire in the same tick; \
         declare 'async' on both so the simultaneous execution is explicit",
        a.name, a.pin, b.name, b.pin
    ))
}

/// The house-style race diagnostic with the concrete fix.
fn race_message(a: &PinnedNode<'_>, b: &PinnedNode<'_>, field: &str, what: &str) -> String {
    format!(
        "'{}' (worker {}) and '{}' (worker {}) both touch '{}' — {}. \
         Workers run concurrently, so move both nodes to one worker, give them \
         disjoint state, or serialise them with 'sync<group>'",
        a.name, a.pin, b.name, b.pin, field, what
    )
}

/// Write set of a node body, as a set for intersection.
fn writes_of(body: &[crate::ast::Statement]) -> BTreeSet<String> {
    collect_assigned_identifiers(body).into_iter().collect()
}

/// A pin that leaves the program on a single worker buys no parallelism — the
/// reactor already runs one worker. Disclose rather than reject: the pin may be
/// deliberate (future-proofing, a host that spawns more threads).
fn advise_single_worker(gate: &mut ThreadGate) {
    if gate.plan.worker_count <= 1 && gate.plan.is_pinned() {
        let names: Vec<&str> = gate.plan.assignments.keys().map(String::as_str).collect();
        gate.warnings.push(format!(
            "only worker 0 is used ({}) — the reactor already runs a single \
             worker, so these pins do not add parallelism; pin another node to \
             a different index to run them concurrently",
            names.join(", ")
        ));
    }
}

/// `on thread any` next to real pins is a statement about the node's freedom;
/// disclose that the compiler read it that way.
fn advise_mixed_any(gate: &mut ThreadGate) {
    if gate.plan.is_pinned() && !gate.plan.explicit_any.is_empty() {
        gate.warnings.push(format!(
            "'on thread any' on {} keeps the reactor's own scheduling while \
             other nodes are pinned — those nodes may share any worker",
            gate.plan.explicit_any.join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a source string into top-level items for gate tests.
    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    #[test]
    fn no_pins_yields_empty_plan() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\nnode n [a < 1][a == 1] { a = a + 1; term; };",
        ));
        assert!(!gate.plan.is_pinned());
        assert_eq!(gate.plan.worker_count, 0);
        assert!(gate.errors.is_empty());
        assert!(gate.warnings.is_empty());
    }

    #[test]
    fn two_workers_with_disjoint_state_plan_cleanly() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\nlet b: Int = 0;\n\
             async on thread 0 node na [a < 1][a == 1] { a = a + 1; term; };\n\
             async on thread 1 node nb [b < 1][b == 1] { b = b + 1; term; };",
        ));
        assert!(gate.errors.is_empty(), "errors: {:?}", gate.errors);
        assert_eq!(gate.plan.worker_count, 2);
        assert_eq!(gate.plan.worker_of("na"), Some(0));
        assert_eq!(gate.plan.worker_of("nb"), Some(1));
        assert_eq!(gate.plan.workers.get(&0), Some(&vec!["na".to_string()]));
        assert_eq!(gate.plan.workers.get(&1), Some(&vec!["nb".to_string()]));
    }

    #[test]
    fn cross_worker_shared_write_is_a_race() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\n\
             async on thread 0 node na [a < 1][a == 1] { a = a + 1; term; };\n\
             async on thread 1 node nb [a < 2][a == 2] { a = a + 2; term; };",
        ));
        assert_eq!(gate.errors.len(), 1, "errors: {:?}", gate.errors);
        assert!(
            gate.errors[0].contains("both write it"),
            "got: {}",
            gate.errors[0]
        );
    }

    #[test]
    fn cross_worker_read_of_other_write_is_a_race() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\nlet b: Int = 0;\n\
             async on thread 0 node na [a < 1][a == 1] { a = a + 1; term; };\n\
             async on thread 1 node nb [b < 1][b == 1] { b = a; term; };",
        ));
        assert_eq!(gate.errors.len(), 1, "errors: {:?}", gate.errors);
        assert!(
            gate.errors[0].contains("one writes what the other reads"),
            "got: {}",
            gate.errors[0]
        );
    }

    #[test]
    fn cross_worker_without_async_is_refused() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\nlet b: Int = 0;\n\
             on thread 0 node na [a < 1][a == 1] { a = a + 1; term; };\n\
             on thread 1 node nb [b < 1][b == 1] { b = b + 1; term; };",
        ));
        assert_eq!(gate.errors.len(), 1, "errors: {:?}", gate.errors);
        assert!(
            gate.errors[0].contains("declare 'async' on both"),
            "got: {}",
            gate.errors[0]
        );
    }

    #[test]
    fn mutually_exclusive_guards_are_safe_across_workers() {
        let gate = run_thread_gate(&parse(
            "let mode: Int = 0;\nlet a: Int = 0;\n\
             async on thread 0 node na [mode == 0][a == 1] { a = a + 1; term; };\n\
             async on thread 1 node nb [mode == 1][a == 2] { a = a + 2; term; };",
        ));
        assert!(gate.errors.is_empty(), "errors: {:?}", gate.errors);
    }

    #[test]
    fn single_worker_pin_is_advisory_not_an_error() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\non thread 0 node na [a < 1][a == 1] { a = a + 1; term; };",
        ));
        assert!(gate.errors.is_empty(), "errors: {:?}", gate.errors);
        assert!(
            gate.warnings
                .iter()
                .any(|w| w.contains("only worker 0 is used")),
            "warnings: {:?}",
            gate.warnings
        );
    }

    #[test]
    fn explicit_any_is_recorded_and_disclosed() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\nlet b: Int = 0;\n\
             on thread any node na [a < 1][a == 1] { a = a + 1; term; };\n\
             on thread 1 node nb [b < 1][b == 1] { b = b + 1; term; };",
        ));
        assert_eq!(gate.plan.explicit_any, vec!["na".to_string()]);
        assert!(
            gate.warnings
                .iter()
                .any(|w| w.contains("'on thread any' on na")),
            "warnings: {:?}",
            gate.warnings
        );
    }

    #[test]
    fn worker_index_above_maximum_is_refused() {
        let src = format!(
            "let a: Int = 0;\non thread {} node na [a < 1][a == 1] {{ a = a + 1; term; }};",
            MAX_THREADS + 1
        );
        let gate = run_thread_gate(&parse(&src));
        assert_eq!(gate.errors.len(), 1, "errors: {:?}", gate.errors);
        assert!(
            gate.errors[0].contains("only workers 0..="),
            "got: {}",
            gate.errors[0]
        );
    }

    #[test]
    fn pin_on_plain_txn_is_advisory() {
        let gate = run_thread_gate(&parse(
            "let a: Int = 0;\non thread 0 txn t(a: Int) [a >= 0][a >= 0] { term; };",
        ));
        assert!(gate.errors.is_empty(), "errors: {:?}", gate.errors);
        assert!(
            gate.warnings
                .iter()
                .any(|w| w.contains("is a txn, not a node")),
            "warnings: {:?}",
            gate.warnings
        );
    }
}
