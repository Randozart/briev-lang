# Reactive Recipes — shapes that compile

Every minimal reactive program in this document compiled and ran against the
current compiler; every failure mode listed here was reproduced this session
(probes: `benchmarks/recommender_corpus/handoff_repro3.bv`, the plan
`docs/plans/2026-10-16-stability-remediation.md` S2/S3, and the results file
`benchmarks/results/2026-10-16-recommender-cpp-vs-briev.md`). When the
compiler rejects a shape, the diagnostic names the problem and the fix —
this page maps the shapes to those messages so the rejection is never a
surprise.

Read with: `docs/architecture/briev-execution-model.md` (the reactor IS the
scheduler), SPEC §11 (reactive semantics), Rule 22 (no implicit
concurrency).

## The shapes that compile

### 1. Multi-node phase chain (the workhorse)

Each node does a bounded chunk, then hands off by bumping a shared phase
field. `beginprogram` nodes are SINGLE-PASS: their postcondition is
`[true]`. Middle nodes declare real goal postconditions the cursor provably
advances toward. The last node exits via `endprogram`.

```briev
let phase: Int = 0;

async node build_data [beginprogram && phase == 0][true] {
    // ... one bounded chunk ...
    phase = 1;
    term;
};

async node build_sparse [phase == 1][phase == 2] {
    // ...
    phase = 2;
    term;
};

async node report [phase == 2][true] {
    // ... final output ...
    endprogram;
};
```

Live examples: `benchmarks/recommender.bv` (4 nodes),
`benchmarks/recommender_corpus/recommender_multinode.bv` (7 nodes).

### 2. Counter-goal convergence (loops with a proven bound)

A node that fires repeatedly toward a counter goal:

```briev
let x: Int = 0;

node first [beginprogram && x < 3][x == 3] {
    x = x + 1;
    term;
};
```

The postcondition must be provable from the body: the body must move the
counter toward the goal. Verified end-to-end by `handoff_repro3.bv`
(prints `3`, exits).

### 3. `async` + distinct phase gates (Rule 22 classification)

Per-tick parallel-looking nodes are classified explicitly: `async` on both
members of a pair (acknowledging simultaneous firing), each gated on a
DISTINCT phase value — the `benchmarks/nbody_newton_accel.bv` pattern, and
the shape of `recommender.bv`. Pairs that may fire together and are NOT
classified are a compile error (Rule 22 — the compiler demands the
classification, it never guesses). Use `sync<g>` instead when members must
barrier.

## The failure modes and their diagnostics

Each of these was hit and reproduced on 2026-10-16; the messages below are
the compiler's actual front door.

| You wrote | The compiler says (abridged) | Why |
|---|---|---|
| `beginprogram` node with a counter goal `[done == 1]` | "goal is not provably reachable — the body must advance a counter toward it (or use '[true]' for a single pass)" | beginprogram fires exactly once; single-pass semantics |
| Single `[true]` node whose body increments a counter | "the reactive cycle (…) has no provable convergence and no liveness obligation — none of its nodes declares a completion … fix: state the completion in the nodes' postconditions, or make the exit condition explicit in a pre" | an incrementing counter is an unbounded self-loop: it never quiesces between events |
| Plain (non-`async`) nodes with goals but no completion in any postcondition | same convergence rejection, naming the cycle | the reactor must prove equilibrium is reachable |
| A `let` counter incremented unboundedly inside a `[true]` node | same as above — the counter IS the non-convergence | counters need goals (`[count == N]`) or a completion elsewhere in the cycle |
| Reserved word as a name (`halt`, `trap`, `out`, …) | "'halt' is a reserved keyword — it is the bare-metal stop statement — and cannot be used as an identifier. fix: rename the identifier" | the word is a dedicated token; see the meanings table in the message |
| `node name @ vector` with no target ISR mechanism | the target-profile error naming `config/isr-targets.dbvl` | machine-vectored nodes resolve the mechanism from the active target |

## Reserved words (rename on collision)

`halt` (bare-metal stop) · `trap` (hardware abort) · `out` (observability) ·
`vol` (volatile) · `seq` (declaration order) · `pack` (layout) · `coll`
(collection layout) · `accel` (GPU offload) · `atomic`, `relaxed`,
`acquire`, `release`, `bartered` (memory ordering) · `union` (union type) ·
`spawn` (task spawn) · `beginprogram` (authored reset). The diagnostic
names the meaning; the canonical ledger lives in the lexer
(`src/lexer.rs`, canonical-vocab test).

## Sorted-iteration note for kernel authors

If you extend the compiler: any `HashMap` iteration that produces LLVM IR
must be sorted by key (house rule §4) — `benchmarks/ll_determinism_check.sh`
byte-compiles a corpus across two processes and will name your emitter if
you forget. Perf note: unsorted iteration costs up to ~9% run-to-run timing
variance, so the gate also protects benchmark sanity.
