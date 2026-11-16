//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Resource effects — what each callable does to state that outlives its call,
//! **transitively** (callees included).
//!
//! 2026-11-16 (per-worker arena plan, Part C): the concurrency and thread gates
//! decide "may these two nodes run at the same time?" from the read/write sets
//! of each node body. Those sets come from `collect_assigned_identifiers` /
//! `collect_read_identifiers`, which are **intraprocedural** — a call is not
//! expanded into its callee's effects. So two nodes that both print (both
//! mutating `stdout_fill` inside the stdlib print lane) look independent, and
//! any real concurrency would interleave their output.
//!
//! This pass closes that gap with a may-write / may-observe over-approximation
//! computed once in the frontend (the frontend-driven-dispatch pillar), in the
//! same shape as the existing `analysis::allocation::analyze_arena_need` +
//! `call_graph::propagate_arena_need` pair.
//!
//! Deliberate properties:
//!
//! - **Over-approximate, never under.** A refusal is the conservative
//!   direction: a false positive refuses a program the author can fix by
//!   serialising the pair; a false negative would ship a race.
//! - **Globals only.** `writes`/`reads` list top-level `let` names. Locals are
//!   per-call and cannot be shared between workers, so they are not effects. A
//!   local that shadows a global name is (conservatively) still counted.
//! - **One definition of "observable".** The purity probe already owns that
//!   notion (`analysis::purity::body_has_observable_effect`); this pass consumes
//!   it rather than growing a second, drift-prone definition.
//! - **`allocates` is taken, not recomputed.** It comes from
//!   `analyze_arena_need`, which already propagates through the call graph, so
//!   the two summaries cannot disagree.
//! - **Deterministic.** Every collection is ordered; no iteration feeds LLVM IR
//!   from a `HashMap`.

use crate::ast::TopLevel;
use crate::backend::{collect_assigned_identifiers, collect_read_identifiers};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// What one callable may do to shared state, transitively.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceEffects {
    /// Top-level `let`s this callable (or a callee) may write.
    pub writes: BTreeSet<String>,
    /// Top-level `let`s this callable (or a callee) may read.
    pub reads: BTreeSet<String>,
    /// Performs an **externally** observable effect: an intrinsic or foreign
    /// call (printing, a syscall, MMIO via `vol`/`Store#`), asm, or program
    /// termination — directly or through a callee.
    ///
    /// This is deliberately narrower than `analysis::purity`'s "has any
    /// effect", which also counts plain local assigns (it answers "is this
    /// callable doing anything at all?"). A write to a *disjoint* state field
    /// is not an ordering hazard between workers; reaching the outside world
    /// is. Two different questions, so two predicates.
    pub observable: bool,
    /// Needs the bump arena (transitive; mirrors `analyze_arena_need`).
    pub allocates: bool,
}

impl ResourceEffects {
    /// Do these two summaries touch the same global in a way that makes
    /// concurrent execution racy? Write/write and write/read both qualify;
    /// read/read does not.
    pub fn races_with(&self, other: &ResourceEffects) -> Option<SharedAccess> {
        if let Some(name) = self.writes.intersection(&other.writes).next() {
            return Some(SharedAccess::BothWrite(name.clone()));
        }
        if let Some(name) = self.writes.intersection(&other.reads).next() {
            return Some(SharedAccess::WriteReads(name.clone()));
        }
        if let Some(name) = other.writes.intersection(&self.reads).next() {
            return Some(SharedAccess::WriteReads(name.clone()));
        }
        None
    }

    /// Do **both** sides perform an observable sink? One side observing is
    /// fine — only one worker then produces output, so the order is still
    /// deterministic. Two workers observing concurrently cannot be ordered (a
    /// `sync<group>` barrier joins, it does not sequence), so the interleaving
    /// would be observable nondeterminism.
    pub fn both_observe(&self, other: &ResourceEffects) -> bool {
        self.observable && other.observable
    }

}

/// How two summaries collide on one global.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedAccess {
    /// Both callables write the same global.
    BothWrite(String),
    /// One writes what the other reads.
    WriteReads(String),
}

impl SharedAccess {
    /// The global involved.
    pub fn name(&self) -> &str {
        match self {
            SharedAccess::BothWrite(name) | SharedAccess::WriteReads(name) => name,
        }
    }

    /// The collision in words, for a diagnostic.
    pub fn describe(&self) -> &'static str {
        match self {
            SharedAccess::BothWrite(_) => "both write it",
            SharedAccess::WriteReads(_) => "one writes what the other reads",
        }
    }
}

/// Every callable's summary, keyed by name, in name order.
#[derive(Debug, Clone, Default)]
pub struct ResourceMap {
    effects: BTreeMap<String, ResourceEffects>,
}

impl ResourceMap {
    /// The summary for a callable; empty when the callable is unknown (an
    /// intrinsic, an import, or a name outside this compilation unit).
    pub fn of(&self, name: &str) -> ResourceEffects {
        self.effects.get(name).cloned().unwrap_or_default()
    }

    /// Number of summarised callables (diagnostics and tests).
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Whether the map summarises nothing.
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

/// Compute the resource-effect summary for every callable.
///
/// `needs_arena` is `analyze_arena_need`'s result — already propagated through
/// the call graph — so `allocates` is taken rather than recomputed.
pub fn analyze_resource_effects(items: &[TopLevel], needs_arena: &HashSet<String>) -> ResourceMap {
    let globals = global_names(items);
    let user_names = user_callable_names(items);
    let mut direct: BTreeMap<String, ResourceEffects> = BTreeMap::new();
    for (name, body) in callable_bodies(items) {
        let mut effects = direct_effects(body, &globals, &user_names);
        effects.allocates = needs_arena.contains(&name);
        direct.insert(name, effects);
    }
    let graph = build_call_graph(items);
    ResourceMap {
        effects: propagate(direct, &graph),
    }
}

/// Every user-defined callable in the program. A call to a name outside this
/// set is an intrinsic or an import, which reaches the outside world.
fn user_callable_names(items: &[TopLevel]) -> HashSet<String> {
    let mut names = HashSet::new();
    for (name, _) in callable_bodies(items) {
        names.insert(name);
    }
    names
}

/// Top-level `let` names — the only names that can be shared across workers.
fn global_names(items: &[TopLevel]) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in items {
        if let TopLevel::Statement(stmt) = item {
            if let crate::ast::Statement::Let { name, .. } = stmt.as_ref() {
                names.insert(name.clone());
            }
        }
    }
    names
}

/// Every callable name paired with its body, in program order.
fn callable_bodies(items: &[TopLevel]) -> Vec<(String, &[crate::ast::Statement])> {
    let mut out = Vec::new();
    for item in items {
        match item {
            TopLevel::Transaction(t) => out.push((t.name.clone(), t.body.as_slice())),
            TopLevel::Definition(d) => out.push((d.name.clone(), d.body.as_slice())),
            TopLevel::Cell(cell) => out.extend(cell_bodies(cell)),
            _ => {}
        }
    }
    out
}

/// A cell owns its own transactions and definitions; index both so a call into
/// a cell member is summarised like any other callable.
fn cell_bodies(cell: &crate::ast::CellDef) -> Vec<(String, &[crate::ast::Statement])> {
    cell.transactions
        .iter()
        .map(|t| (t.name.clone(), t.body.as_slice()))
        .chain(
            cell.definitions
                .iter()
                .map(|d| (d.name.clone(), d.body.as_slice())),
        )
        .collect()
}

/// The direct (non-transitive) part of one body's summary.
fn direct_effects(
    body: &[crate::ast::Statement],
    globals: &HashSet<String>,
    user_names: &HashSet<String>,
) -> ResourceEffects {
    let writes: BTreeSet<String> = collect_assigned_identifiers(body)
        .into_iter()
        .filter(|name| globals.contains(name))
        .collect();
    let reads: BTreeSet<String> = collect_read_identifiers(body)
        .into_iter()
        .filter(|name| globals.contains(name))
        .collect();
    ResourceEffects {
        writes,
        reads,
        observable: externally_observable(body, user_names),
        allocates: false,
    }
}

/// Does this body reach the outside world directly?
///
/// Ordering-relevant effects only. A plain assignment to a global is *not* one:
/// two workers each bumping their own counter are deterministic. Nor is an
/// intrinsic that only touches program memory (`Alloc#`, `Sqrt#`, `Copy#`,
/// `AtomicAdd#`): those either have their own row (the arena) or are exactly
/// what makes sharing safe. What cannot be ordered is writing to something
/// outside the process — output, a syscall, MMIO, arbitrary asm, termination —
/// or handing control to host code through an import.
fn externally_observable(body: &[crate::ast::Statement], user_names: &HashSet<String>) -> bool {
    let mut observable = body.iter().any(terminating_statement);
    crate::ast::visit::walk_stmts(body, &mut |expr| {
        if let crate::ast::Expr::Call(name, _, _) = expr {
            if leaves_the_process(name, user_names) {
                observable = true;
            }
        }
    });
    observable
}

/// Does calling `name` write to something the process does not own?
///
/// A call to a name outside `user_names` is an import (`frgn`/stdlib) — host or
/// library code, so treat it as reaching outside. For the compiler-known
/// intrinsics only the ones that cross the process boundary qualify; the rest
/// (`Alloc#`, arithmetic, casts, atomics, `Load#`/`Store#` on program memory)
/// stay inside the program's own state and are not ordering hazards.
fn leaves_the_process(name: &str, user_names: &HashSet<String>) -> bool {
    // Compiler-generated casting lanes (`Cast.Int`, `Cast.Ptr`, the protocol
    // `CastFrom`/`CastTo` helpers) are pure representation changes — they
    // allocate nothing and touch nothing outside the value being converted.
    // They are not user callables, so without this they would read as imports.
    if name.starts_with("Cast.") || name.starts_with("CastFrom.") || name.starts_with("CastTo.") {
        return false;
    }
    if !name.ends_with('#') {
        return !user_names.contains(name);
    }
    matches!(
        name,
        "Print#"
            | "Asm#"
            | "VolatileLoad#"
            | "VolatileStore#"
            | "SysCall#"
            | "SysConf#"
            | "SysQuery#"
            | "SetEnv#"
            | "EnvGet#"
            | "Environ#"
            | "GetCwd#"
            | "ChDir#"
            | "Argc#"
            | "Args#"
            | "DlOpen#"
            | "DlSym#"
            | "DlClose#"
            | "HttpFetch#"
            | "ShellCmd#"
            | "Backtrace#"
    )
}

/// Statements that end or escape the program's normal data flow.
fn terminating_statement(stmt: &crate::ast::Statement) -> bool {
    matches!(
        stmt,
        crate::ast::Statement::EndProgram(_)
            | crate::ast::Statement::Trap
            | crate::ast::Statement::Halt
            | crate::ast::Statement::InlineAsm { .. }
            | crate::ast::Statement::Rollback(_)
    )
}

/// Callee edges per callable (transactions and definitions alike).
fn build_call_graph(items: &[TopLevel]) -> BTreeMap<String, Vec<String>> {
    let mut graph = crate::analysis::call_graph::CallGraph::new();
    graph.build_from_program(items);
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, _) in callable_bodies(items) {
        let mut callees: Vec<String> = Vec::new();
        if let Some(found) = graph.edges_from(&name) {
            callees.extend(found.iter().cloned());
        }
        if let Some(found) = graph.defn_edges_from(&name) {
            callees.extend(found.iter().cloned());
        }
        callees.sort();
        callees.dedup();
        edges.insert(name, callees);
    }
    edges
}

/// Push each callee's effects into its callers until nothing changes — the
/// same fixpoint shape as `call_graph::propagate_arena_need`, extended to the
/// full effect set.
fn propagate(
    mut effects: BTreeMap<String, ResourceEffects>,
    graph: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, ResourceEffects> {
    let names: Vec<String> = effects.keys().cloned().collect();
    while sweep_once(&names, graph, &mut effects) {}
    effects
}

/// One fixpoint sweep over every callable. Returns whether anything grew, which
/// drives the caller's loop — one iteration level per function so each stays
/// flat (the fixpoint itself is inherent: a chain of n callees needs n sweeps in
/// the worst case without a topological order, and the call graph may be
/// cyclic, so a sweep is the correct shape).
fn sweep_once(
    names: &[String],
    graph: &BTreeMap<String, Vec<String>>,
    effects: &mut BTreeMap<String, ResourceEffects>,
) -> bool {
    names.iter().fold(false, |changed, name| {
        merge_callees(name, graph, effects) || changed
    })
}

/// Fold one callable's callees into its own summary. Returns whether it grew.
fn merge_callees(
    name: &str,
    graph: &BTreeMap<String, Vec<String>>,
    effects: &mut BTreeMap<String, ResourceEffects>,
) -> bool {
    let Some(callees) = graph.get(name) else {
        return false;
    };
    let user_names: HashSet<String> = effects.keys().cloned().collect();
    let merged = callees
        .iter()
        .fold(ResourceEffects::default(), |mut acc, callee| {
            let Some(e) = effects.get(callee) else {
                // Unknown callee: an intrinsic or an import. Only the ones that
                // cross the process boundary count (the same rule the direct
                // scan uses), so `Alloc#`/`Store#` do not read as output.
                if leaves_the_process(callee, &user_names) {
                    acc.observable = true;
                }
                return acc;
            };
            acc.writes.extend(e.writes.iter().cloned());
            acc.reads.extend(e.reads.iter().cloned());
            acc.observable |= e.observable;
            acc.allocates |= e.allocates;
            acc
        });
    let Some(slot) = effects.get_mut(name) else {
        return false;
    };
    merge_into(slot, merged)
}

/// Fold a callee summary into a caller's slot. Returns whether anything grew.
fn merge_into(slot: &mut ResourceEffects, merged: ResourceEffects) -> bool {
    let before = slot.clone();
    slot.writes.extend(merged.writes);
    slot.reads.extend(merged.reads);
    slot.observable |= merged.observable;
    slot.allocates |= merged.allocates;
    *slot != before
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse source into top-level items.
    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    /// Summarise a program with no arena users.
    fn summarize(src: &str) -> ResourceMap {
        analyze_resource_effects(&parse(src), &HashSet::new())
    }

    #[test]
    fn direct_global_write_is_summarised_but_is_not_an_observable_sink() {
        let map = summarize(
            "let a: Int = 0;\n\
             node n [a < 1][a == 1] { a = a + 1; term; };",
        );
        let effects = map.of("n");
        assert!(effects.writes.contains("a"), "effects: {effects:?}");
        assert!(
            !effects.observable,
            "a write to disjoint state is not an ordering hazard"
        );
    }

    #[test]
    fn an_intrinsic_call_is_an_observable_sink() {
        let map = summarize("node n [true][true] { Print#(1); term; };");
        assert!(
            map.of("n").observable,
            "Print# reaches stdout, so it is an observable sink"
        );
    }

    #[test]
    fn transitive_write_through_one_defn_hop() {
        let map = summarize(
            "let a: Int = 0;\n\
             defn bump() -> Int { a = a + 1; term a; };\n\
             node n [a < 1][a == 1] { let v: Int = bump(); term; };",
        );
        let effects = map.of("n");
        assert!(
            effects.writes.contains("a"),
            "a write through a callee must be summarised, got: {effects:?}"
        );
    }

    #[test]
    fn transitive_write_through_two_hops() {
        let map = summarize(
            "let a: Int = 0;\n\
             defn inner() -> Int { a = a + 1; term a; };\n\
             defn outer() -> Int { term inner(); };\n\
             node n [a < 1][a == 1] { let v: Int = outer(); term; };",
        );
        assert!(
            map.of("n").writes.contains("a"),
            "two-hop write must propagate, got: {:?}",
            map.of("n")
        );
    }

    #[test]
    fn a_printing_callee_makes_the_caller_observable() {
        let map = summarize(
            "defn shout() -> Int { Print#(1); term 0; };\n\
             node quiet [true][true] { let v: Int = shout(); term; };",
        );
        assert!(
            map.of("quiet").observable,
            "a printing callee must make the caller observable"
        );
    }

    #[test]
    fn alloc_and_store_are_not_observable() {
        let map = summarize(
            "node n [true][true] { let p: Ptr = Alloc#(32) as Ptr; Store#(p as Int, 7, 8); term; };",
        );
        let e = map.of("n");
        assert!(!e.observable, "effects: {e:?}");
    }

    #[test]
    fn locals_are_not_effects() {
        let map =
            summarize("node n [true][true] { let local: Int = 1; local = local + 1; term; };");
        let effects = map.of("n");
        assert!(
            effects.writes.is_empty(),
            "a local write is not a resource effect, got: {effects:?}"
        );
    }

    #[test]
    fn reads_are_tracked_for_write_read_overlap() {
        let map = summarize(
            "let a: Int = 0;\n\
             node writer [true][true] { a = 1; term; };\n\
             node reader [true][true] { let v: Int = a; term; };",
        );
        assert!(map.of("writer").writes.contains("a"));
        assert!(
            map.of("reader").reads.contains("a"),
            "reader must record the read, got: {:?}",
            map.of("reader")
        );
        let race = map.of("writer").races_with(&map.of("reader"));
        assert_eq!(
            race,
            Some(SharedAccess::WriteReads("a".to_string())),
            "write/read overlap must be reported"
        );
    }

    #[test]
    fn read_read_is_not_a_race() {
        let map = summarize(
            "let a: Int = 0;\n\
             node r1 [true][true] { let v: Int = a; term; };\n\
             node r2 [true][true] { let w: Int = a; term; };",
        );
        assert_eq!(map.of("r1").races_with(&map.of("r2")), None);
    }

    #[test]
    fn allocates_is_taken_from_needs_arena() {
        let items = parse("node n [true][true] { let p: Ptr = Alloc#(16) as Ptr; term; };");
        let mut needed = HashSet::new();
        needed.insert("n".to_string());
        let map = analyze_resource_effects(&items, &needed);
        assert!(
            map.of("n").allocates,
            "allocates must mirror analyze_arena_need's set"
        );
    }

    #[test]
    fn allocates_matches_analyze_arena_need_exactly() {
        // The two summaries must not drift: `allocates` IS that set.
        let mut items = parse(
            "defn grow() -> Int { let p: Ptr = Alloc#(64) as Ptr; term 0; };\n\
             node n [true][true] { let v: Int = grow(); term; };",
        );
        let needs = crate::analysis::allocation::analyze_arena_need(&mut items);
        let map = analyze_resource_effects(&items, &needs);
        for name in &needs {
            assert!(
                map.of(name).allocates,
                "'{name}' needs the arena per analyze_arena_need but the summary disagrees"
            );
        }
        assert!(
            map.of("n").allocates,
            "arena need propagates through the call graph in both summaries"
        );
    }
}
