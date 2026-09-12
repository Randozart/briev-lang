# The Causal DAG — Compile-Time Wiring for Reactive Programs

**2026-09-12** (plan: `docs/plans/2026-09-12-dynamics-causal-dag.md`;
findings: BUGS.md 2026-09-12). This is the pass the June 2026 dirty-flag
design specified: the compiler derives the wiring from the contracts.
The program signals intent; the compiler owns the machinery.

## Semantics encoded

- **`[pre]` = eligibility. `[post]` = the declaration of completion.**
- **Edge `A → B`** iff A writes a field B's pre/body reads. Every edge
  carries a soundness level:
  - **PROVEN** — every `pre(B)` conjunct is entailed by `post(A)` (fields
    A rewrote) or `pre(A)` (fields A preserves). Dominance rules per
    field: `f == c` entails any comparison on `c` it implies; `f >= c`
    entails `>= c'`/`> c'`/`!= c'` for `c > c'`; mirrored for `<=`/`<`;
    `!=` entails only `!=` with an equal bound. Enum-literal, call, and
    trigger conjuncts are not provable v1 — they make the edge WEAK.
  - **WEAK** — real dependency, enabling unproven. Reported, never fused.
- **Components** (Tarjan over the edge set, self-loops included):
  - **chain** — acyclic; with all-PROVEN edges it is the fusible straight
    -line shape ("A fires into the outcome of X") for a future codegen slice.
  - **cycle with declared completions** — at least one member's post
    declares completion: v1 trusts the contract.
  - **cycle with no declared completion** — refused at compile time:

```
reactive liveness:
  the reactive cycle (flip) has no provable convergence and no liveness
  obligation — none of its nodes declares a completion in a postcondition,
  so it may never quiesce between events. fix: state the completion in the
  nodes' postconditions (e.g. [mode == Mode::Alarm], [done == true]), or
  make the exit condition explicit in a pre.
```

## Surfaces

- `src/analysis/causality.rs` — `run(&[TopLevel]) -> CausalGraph`
  (nodes, edges + proof levels, refusals), `explain(&CausalGraph) -> Vec<String>`
  (the "what fires into what" report).
- Wired in `compile.rs` after the concurrency gate; refusals are fatal
  (`reactive liveness:` error block). `--explain-causality` prints the
  report to stderr; silent by default.

## v1 limits (documented, honest)

- **Contract-trust rule**: a cyclic component passes if ANY member
  declares a substantive post. A junk substantive post on one member of a
  multi-node cycle masks an oscillation. The deep verifier is a Z3
  fixpoint over the SCC firing game — future work.
- Field facts are `Identifier op literal` only; enum-literal comparisons
  (`mode == Mode::Idle`), boolean-of-field conjuncts beyond the Bool
  literal encoding, and call-shaped conjuncts stay WEAK.
- Edges over instance fields of objs/cells are out of scope v1
  (top-level `let` state only).

## Backend trust (the LTO lesson)

No fusion codegen was built on faith. Whether LLVM's `-O3 -flto` pipeline
already collapses chain round-trips in the emitted tick loop is a MEASURED
question (see BUGS.md 2026-09-12 follow-up). A Briev-owned fusion pass is
built only on a proven backend inability.
