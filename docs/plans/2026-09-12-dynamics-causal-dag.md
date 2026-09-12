# Dynamics: the Causal DAG — Proof of the Model

**Date:** 2026-09-12
**Status:** ACTIVE
**Predecessor findings:** BUGS.md `2026-09-12: reactive realization gap investigated — deferred`

## Design of record (quoted, normative)

**2026-06-15-trg-reactive-dirty-flag.md:**
> "compile-time dependency graph + bitmask dirty flags enable a flat `step()`
> function that recomputes only what changed"
> "Event-Driven Wake (No Polling): The program never spin-waits."
> "`step()` iterates variables in topological order"

**2026-06-11-async-reactor-triggers.md:**
> "Responsive (`node`) | Dirty state | Immediately when a dependency changes.
> Runs convergence loop to quiescence."

**Session-locked intent (2026-09-12, with the author):** the program signals
intent; the compiler owns realization. `[pre]` = eligibility; `[post]` = the
declaration of completion. At compile time the compiler derives the wiring:
if A's firing provably enables B, that is an edge, and a static chain
`A → B → C → X` can fire "into the outcome of X" — intermediate side effects
preserved, round-trips elided. Provable termination folds; unprovable
termination requires a checkable liveness obligation in the post — no silent
spinning exists. Rule 22 governs simultaneous eligibility.

**Scope discipline (author, 2026-09-12):** this slice is *proof that the DAG
works*, not a fusion engine. LLVM and other backends can already do many of
the optimizations this would hand-roll. Backends are trusted until measured
otherwise (the LTO lesson); fusion codegen is built only if the experiment
proves the backend cannot.

## What already exists (verified 2026-09-12)

| Piece | Location | State |
|---|---|---|
| Per-node write sets, preconditions, bounded-pre, ranking | `analysis/transition_graph.rs:24` (`ReactorNode`) | built |
| Body read/assigned collectors | `backend::collect_read_identifiers` / `collect_assigned_identifiers` | built |
| Expr-level identifier walker (for pre-reads) | `analysis/dependency_graph.rs:218` | built |
| Implication proving (Z3) | `proof_engine::prove_contract` (`mod.rs:20`) | built |
| Terminability / convergence checks | `proof_engine::is_proven_terminable` (:229), `check_convergence` (:400) | built |
| Pairwise eligibility + XOR overlap | `analysis/concurrency_gate.rs` | built (Rule 22) |
| Analysis wiring point after typecheck | `compile.rs:366` (concurrency gate) | integration point |
| Backend graph consumption | `compile.rs:1371` (`b.ctx.transition_graph`) | pattern to mirror |
| The fossil of the original design | `interpreter/reactor.rs` `dependency_map`/`mark_dirty` | dead code, never populated |

## Build

### 1. `src/analysis/causality.rs` — the pass

- **Nodes**: reactive transactions (`is_reactive == true`), from the same
  walk the concurrency gate uses.
- **Reads/writes**: writes = `collect_assigned_identifiers(body)`; reads =
  `collect_read_identifiers(body)` ∪ expr-walker over `[pre]` ∪ `[post]`.
- **Edges**: `A → B` iff `writes(A) ∩ (reads_pre(B) ∪ reads_body(B)) ≠ ∅`.
  Every edge carries a soundness level:
  - **PROVEN** — `post(A) ⇒ pre(B)` discharged via the proof engine
    (conservative first version: structural implication + `check_satisfiable`
    negative screen; Z3 obligation where the structural check is
    inconclusive).
  - **WEAK** — write/read overlap only, implication unproven. Reported,
    never fused.
- **Components** (over the PROVEN subgraph; weak edges never form cycles):
  - **chain** — acyclic, all edges proven
  - **foldable cycle** — `is_proven_terminable`/`check_convergence` positive
    on the component
  - **bounded-only** — ranking (`lexicographic_vars`) exists but convergence
    not proven
  - **unproven** — neither
- **Output**: `CausalGraph { nodes, edges, components, refusal: Vec<String> }`
  stored alongside `transition_graph` on the backend ctx (mirror
  `compile.rs:1371`). Frontend decides; backend consumes later.

### 2. Liveness refusal (the DAG made load-bearing)

An **unproven** cycle component whose nodes' posts carry no checkable
liveness obligation (a bound, deadline, or monotone measure — the shapes the
watchdog grammar already parses: `within N <unit>`, `?[cond]`, bounded
counters) is a compile error:

```
the reactive cycle 'a' -> 'b' -> 'a' has no provable convergence and no
liveness obligation — it may never quiesce between events. fix: state the
completion bound in a postcondition (e.g. [x != x0] after at most N steps,
or `within N ms`), or make the exit condition explicit in a pre.
```

Provable/foldable/bounded components pass. Additive-only: new pass, new
error arm; no existing match arms touched (Rule 6).

### 3. Report surface — "what fires into what"

`--explain-causality` flag → per node: proven enablers, weak enablers,
triggers sampled, component classification. Inspectable, silent by default.
This is the user-facing answer to "what triggers what".

### 4. The LLVM experiment (Rule 20 — before any fusion talk)

Canonical chain program (`idle → alarm → shutdown` over a state variable,
plus a data-dependent 3-chain):

1. Compile with today's loop emission; collect the `.ll`
2. Link with the harness's exact command
   (`clang -O3 -flto -march=native -ffast-math -fdata-sections
   -ffunction-sections -Wl,--gc-sections`)
3. Inspect the optimized binary/IR: does the tick round-trip survive
   (per-tick pre re-evaluation in the hot loop), or does LLVM collapse the
   chain once contracts hold?
4. Record findings in BUGS.md (this entry's follow-up) — where the backend
   already delivers the designed realization, and where (if anywhere)
   Briev-owned emission is actually needed.

**A fusion pass is built only on a measured LLVM inability** — never on
faith, never as a heuristic.

## Tests (behavioral, not literal)

- chain: hysteresis FSM (`idle ⇄ alarm`) — two proven edges, chain component
- implication negative: B's pre reads a field A writes but `post(A) ⇒
  pre(B)` fails → WEAK edge, no chain claim
- foldable cycle: counted self-loop with bounded post → foldable, passes
- unproven cycle without liveness post → refusal text names the cycle
- unproven cycle WITH `within` post → passes
- weak-edge cycle (mutual writes, unproven implications) → refusal
- report snapshot: `--explain-causality` output deterministic (sorted)
- determinism: BTreeMap iteration throughout (HashMap rule)

## Documentation

- `docs/architecture/causality.md` — the pass, edge soundness levels,
  component meanings, refusal contract (this commit series)
- SPEC §11 note: nodes carry pre (eligibility) and post (completion); the
  compiler derives the wiring; unproven cycles need liveness obligations
- BUGS.md follow-up with the LLVM experiment result
- `docs/plans/INDEX.md` entry

## Out of scope (deferred, per BUGS.md 2026-09-12)

Fusion/fold codegen, sync\<g\> completion barrier, interpreter reactor
revival, plain-txn dead-code diagnostic, event-driven wake (epoll).

## Followups (documented 2026-09-12, post-slice)

Built on this slice, in rough priority order:

1. **Deep verifier** — Z3 fixpoint over the SCC firing game; replaces the
   v1 contract-trust rule (a junk substantive post on one member of a
   multi-node cycle currently masks an oscillation).
2. **FSM proofs** — reachability / deadlock / unreachable-modes over the
   causal graph; the liveness machinery reused. Zero new syntax.
3. **Fact enrichment** — enum-literal comparisons (`mode == Mode::Idle`)
   as field facts; today they force WEAK edges. The hysteresis chain
   becomes fully PROVEN once landed.
4. **Instance-field edges** — obj/cell state; v1 covers top-level `let` only.
5. **Dispatch consumption** — PROVEN chains feed backend shape selection
   (drop the empty confirm pass). Measured first: LLVM already folds the
   whole program for foldable shapes; only worth it on a demonstrated
   backend failure.
6. **plain-txn dead-code diagnostic** — body-carrying, never-called,
   non-reactive txn.

Deferred (BUGS.md 2026-09-12): event-driven wake (epoll), sync<g>
completion barrier, interpreter reactor revival, fusion codegen
(measured unnecessary for foldable shapes — the backend delivers).
