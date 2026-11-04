# Typed HIR — H3/H4/H5 execution plan (2026-11-04)

Status: ACTIVE. Parent: `2026-11-02-typed-hir.md` (H0–H2, H3a landed).
Baseline: `main` at the PR-integration tip (`870115ab`; six collaborator PRs
merged, all gates green). Companion: `docs/architecture/agent-reference.md`.

## Goal

Finish the migration from AST-driven semantics to typed HIR:
- **H3** — every semantic analysis consumes HIR (AST walkers deleted after
  parity).
- **H4** — every backend consumes HIR facts; AST is lowering input only.
- **H5** — legacy AST paths, wrappers, path-derived IDs, and duplicate
  walkers are retired with before/after evidence.

Hard constraint: **no behavior change per slice**. AST remains authoritative
until its HIR replacement is proven equal; every slice preserves emitted IR.

## Progress (2026-11-04)

- **Collaborator PRs merged:** #61 (nested environment macros — exhaustive
  postorder expansion) and #62 (native allocation reclamation at proved call
  boundaries; shared `analysis::generic_specialization`, runtime-reflection
  liveness fix, memory-effects config). Both merged with `--no-ff`; full
  `cargo test --all`, HIR suites, and the AST/HIR `.ll` A/B are green.
- **HIR follow-ups for #62:** `hir::defn_liveness` mirrors the runtime
  `.^Length` receiver walk (new curated parity case); `hir::specialize` now
  consumes the shared `unify_generic_binding` / `mangle_specialization`
  (single source of truth for concrete callable identity).

- **H3.0 ✅** HIR coverage: impl/trait-default members with owner/trait_default,
  top-level globals (+declared storage, initializer), always_stateful
  (asm/bad), contracts (pre/post/watchdog + on-fire), transaction
  reactive/async flags, full output types, `frgn` bindings, declared
  protocols (body-less `type CStr: String<C_String>`), constants.
- **H3.1 ✅** `needs_state`/`needs_plain` over HIR — corpus parity: 103
  compared, 29 skipped (unsupported HIR), **0 mismatches**.
- **H3.2 ✅** `defn_liveness` over HIR — curated parity (dead/export/reactive/
  closure/endprogram/print-helper/construction/eq-helper).
- **H3.3 ✅** `classify_plain_lets` (corpus parity, 0 mismatches) +
  `compute_boundary_ownership` (curated parity: frgn seeding, params,
  lto, state reads, export chain).
- Each slice is behind parity tests; AST remains authoritative and no
  consumer is switched yet.
- **H3.4a ✅** `tail_recursion` / `tail-loop` plans over HIR — corpus parity:
  103 compared, 29 skipped, **0 mismatches**. HIR gained `type_params` /
  `variadic` fidelity.
- **H3.4b-1 ✅** `transition_graph` over HIR — corpus parity (113 compared,
  104 skipped unsupported, **0 mismatches**), including node facts
  (bounded_pre/increments/purity/write_set/lexicographic/effectively-pure),
  live fields, triggers, unguarded FFI. HIR gained `sync_domains` on
  callables, `has_triggers` + global `modifiers` (`out`/`vol` liveness pins),
  and a bug-for-bug mirror of the mutually exclusive purity arms.
- **H3.4b-2 (partial) ✅** `loop_carried`, `swan_song`, and
  `slp_isomorphism` over HIR — corpus parity (113 compared, 104 skipped,
  **0 mismatches** each).
- **H3.4b-2 (complete) ✅** `loop_shape` over HIR (consumes HIR graph +
  carried + swan + SLP facts) — corpus parity **113/0**, including
  field classes, local/carried sets, vector groups, pressure counts,
  convergence and program-level natural exit.
- **H3.4b-3 ✅** `enum_repr` + `tail_carrier` over HIR — corpus parity
  **113/0** (defns/variants/escaping lets; carrier identity, arg classes,
  sret locals, proofs). `sret_safe_defns` stays AST-only until its backend
  consumer moves (H4).
- **H3.4 COMPLETE.** Every H3.4 analysis (transition graph, loop carried,
  swan song, SLP, loop shape, tail recursion, enum repr, tail carrier) has
  an HIR mirror at corpus parity. All 14 HIR parity suites green.
- **H3.5 ✅** Task/collection analyses over HIR: `task_segments`
  (segmentation, spawn targets, task ABI), `task_linear` (handle
  linearity), `spawn_pool` (static/dependent capacities, storage classes,
  bounded-init sizing), `coll_length` (grow-guard elimination + pre-grow
  proofs) — corpus parity **113/0** each.
- **H3.6 (primitive) ✅** Generic callable-txn specialization by HIR
  substitution: `hir::specialize` maps type params → concrete args, re-ids
  every node, substitutes param/result/output/contract/body type slots, and
  retargets call sites at peer instantiations. Gate: the two-struct
  `iter_chain<T>`/`iter_chain_loop<T>` fixture (A and B) plus a new
  body-local generic txn; both instantiations verify. NOTE: wiring this in
  place of the backend's ad-hoc monomorphization is H4 work (its golden
  output/runtime gate), not this slice.
- **H4.1 (consumer flips) ✅** Under `BRIEV_HIR_EMIT=1`: `coll_length`
  grow-guard / pre-grow proofs, `defn_liveness` (same consumer type), and
  `boundary_ownership` (same consumer type) come from typed HIR. Debug
  builds still compute both and assert exact equality; the reclamation
  plan consumes the single chosen liveness set. Emission unchanged; A/B
  byte-identical (release quick corpus; debug-mode flag run also identical,
  exercising all debug cross-checks). Next type-agnostic candidates need a
  type shim: spawn pools (AST-typed dependent terms) and task segments
  (AST statements).
- **H4.1 (plumbing) ✅** `AnalysisResults` now carries the typed `HirProgram`
  for the post-desugar AST, built exactly once per analysis run.
  `BRIEV_HIR_EMIT=1` verifies HIR id/export invariants but does not change
  emission yet. Gate: `benchmarks/hir_emit_ab.sh` (AST vs HIR `.ll`
  byte-compare) — quick corpus green; `tests/hir_emit_plumbing.rs` pins
  one-lowering equality. This is the always-on tripwire for each family flip.
- **Next:** H4.1 family flip — feed inferred HIR value types/cast lanes from
  the frontend typer (today `HirExpr.ty`/`Cast.via` are syntactic/None), then
  emit expressions/calls/casts from HIR under the flag. H4.2–H4.4 follow.
  H5 retires AST paths. `soa_projection`/`soa_reorder` stay with H4 layout.

## Current state

- `lower_program` / `lower_program_checked` (H0–H2) build `HirProgram` with
  stable IDs, typed callables/statements/expressions/patterns, constructors,
  sinks, values, types, tail-call facts. `build_facts` bundles them (H3a).
- HIR is built only under `BRIEV_HIR_AUDIT` (`compile.rs`), for an audit log.
  No analysis or backend consumes it.
- Analyses are AST-based and consumed via the backend `AnalysisResults`:
  `export_abi::compute_defn_needs_state` / `needs_plain`,
  `DefnLiveness`, `storage_class::classify_plain_lets`,
  `boundary_ownership::compute_boundary_ownership`,
  `loop_shape::build_loop_shapes`, `tail_recursion`, `tail_carrier`,
  `task_segments`/`task_linear`/`spawn_pool`.
- HIR audit baseline: 60 unsupported expressions (plugin-intercept tags) over
  tier1+stdlib raw source; H3.0 closes these where analyses need them.

## H3 — Unify semantic analyses on HIR

### H3.0 — HIR coverage for analysis inputs

Deliver the nodes the analyses need (extend `lower` only where missing):
identifiers/bindings resolved to stable IDs, call targets, state-field vs
local distinction, guards/collections/constructors, pattern bindings, and
member/impl indexing (for `type_stateful_members`).
- Gate: `unsupported_counts` does not grow; verifier passes; corpus audit.
- Undo: additive lowering; revert the module.

### H3.1 — `needs_state` / `needs_plain` on HIR

Port `compute_defn_needs_state`/`compute_defn_needs_plain` to
`src/hir/needs_state.rs` (same least-fixpoint semantics: state fields minus
plain, txn calls, always-stateful intrinsics/defns, locals excluded).
- Parity: for every callable in the corpus, HIR result == AST result.
  Run via a new `tests/hir_needs_state_parity.rs` with `BRIEV_HIR_PARITY`
  diagnostics on mismatch (callable, expected, got).
- Gate: parity green over tier1 + stdlib + benchmarks; IR unchanged.
- Then switch `AnalysisResults.needs_state`/`needs_plain` to the HIR result
  behind an equality assert; AST computation deleted only in H5.

### H3.2 — `defn_liveness` on HIR

Port roots/imports and the intrinsic→helper table to HIR callable IDs.
- Parity: live set equality and helper attribution over the corpus.
- Gate: same emitted function set (defn liveness gates emission).

### H3.3 — `storage_class` + `boundary_ownership` on HIR

Port `classify_plain_lets` (plain vs machine state) and
`compute_boundary_ownership` (escape/ownership classes) to HIR using
`HirValueFacts`/`HirSinkFacts`.
- Parity: per-field/defn maps equal; IR unchanged.
- Note: boundary ownership feeds the enum whole-enum gate; parity protects it.

### H3.4 — `loop_shape` / `tail_recursion` / `tail_carrier` on HIR

Port the shape detectors and carrier plans using HIR statements/expressions
and `HirTailCallFacts`.
- Parity: identical `LoopShape`/`TailLoopPlan` per node; IR unchanged
  (loop shapes drive unrolling/vectorization).

### H3.5 — Task/collection analyses on HIR

Port `task_segments`, `task_linear`, `spawn_pool`, and collection/iterable
analyses to HIR.
- Parity: identical segments/linearity/spawn targets; runtime suite.

### H3.6 — Generic callable-txn specialization on HIR

Implement specialization by HIR substitution; verify with the existing
two-struct `iter_chain` golden and a new fixture.
- Gate: golden output, full corpus, runtime.

### H3 exit

All analyses read HIR; AST walkers unused by analyses (removed in H5);
corpus parity is a CI gate (`cargo test --all`).

## H4 — Backend consumes HIR (family-by-family)

Introduce a dual-mode emission path: AST-driven (current) vs HIR-driven,
selected by `BRIEV_HIR_EMIT=1` for testing. Each family must produce
**byte-identical `.ll`** before switching the default.

- **H4.1** expressions/calls/casts (HIR types, cast `via`, constructor refs).
- **H4.2** statements/loops/patterns (HIR stmt kinds, loop shapes, patterns).
- **H4.3** enums/objects/collections (HIR type defs, sinks, ownership facts).
- **H4.4** exports/FFI/special backends (export metadata; GLUE/wasm/JS).

Per family:
- Gate: IR A/B byte-equal over tier1 + benchmarks (`ll_determinism` style),
  runtime correctness, I11/netstack, wasm/JS.
- Only after equality: flip the default; keep the AST path behind a temporary
  env flag for one release, then delete in H5.

## H5 — Retire legacy

- Delete AST analyses/consumers replaced in H3/H4 (no dead walkers).
- Remove export/plugin wrappers from backend paths; HIR export metadata is the
  source.
- Delete path-derived IDs and compatibility adapters; parser AST is
  frontend-only.
- Publish before/after: emitted IR hashes, runtime table, compile time.
- Gate: `cargo test --all`, full runtime, determinism, Kani, Praetor.

## Verification matrix (every slice)

- `cargo test --all` (hard gate) and `cargo test --lib`.
- HIR parity tests (`tests/hir_*_parity.rs`) over tier1 + stdlib + benchmarks.
- IR identity: compile corpus twice (AST vs HIR path) and byte-compare `.ll`.
- `ll_determinism_check --quick` (full on backend commits).
- `build_and_bench.sh --runtime` for H3.4/H3.6/H4 (loop/codegen-sensitive).
- Praetor on changed dirs; Kani for analysis facts; I11 when enum/ownership
  changes.

## Risks and mitigations

- **HIR gaps** (unsupported plugin-intercept) — H3.0 closes only what analyses
  need; parity fails loudly with the offending callable.
- **Semantic drift** — least-fixpoint/ownership ports are compared to AST on
  the whole corpus, not a hand-picked fixture.
- **IR churn** — no slice changes IR; H4 gated by byte-identical `.ll`.
- **Two sources of truth** — temporary; the AST result is asserted equal, then
  deleted in H5 (never silently divergent).

## Documentation / undo

- Update `docs/architecture/agent-reference.md` and the typed-HIR plan status
  per slice; add architecture notes for the HIR analysis contract.
- Undo: slices are additive and flag-gated; revert the module/slice and keep
  AST authoritative. No data migrations; no public ABI or syntax changes.
