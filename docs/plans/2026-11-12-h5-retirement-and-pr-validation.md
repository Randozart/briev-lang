# H5 retirement + PR validation (2026-11-12)

Status: ACTIVE. Executes the chosen lane: validate the two open collaborator
PRs, then convert the completed H5 port into actual deletion.

Predecessors: `2026-11-11-compiler-completion-and-verification.md`,
`2026-11-05-h5-retirement-inventory.md`,
`2026-11-04-typed-hir-h3-h5-execution.md`.

## Part 1 — PR validation (first)

### #92 `fix(llvm): nested enum allocation and payload layouts` (+271/−10)
- Touches `src/analysis/enum_layout.rs`, `src/backend/llvm/emit_expr.rs`,
  docs, a fixture and `tests/nested_enum_allocation.rs`.
- Claim: resolve the concrete constructor before sizing/choosing the image;
  inline a nested enum only when the complete payload tree fits the depth
  budget, else retain a handle.
- CI: Default gates FAILURE (branch-side). Validation: merge into a clean
  worktree from current main; run `cargo test --lib enum`, the new
  `nested_enum_allocation` suite, `daily_use_smoke`, quick gate, and the
  corpus sweep; inspect the emitted `.ll`/Valgrind evidence in the PR body.

### #93 `fix(reclamation): stop element reads inheriting container storage` (+241/−3)
- Touches `src/analysis/reclamation.rs` + `walk.rs` **and the new HIR
  reclamation graph/walk**, docs, fixture, `tests/reclamation_runtime.rs`.
- Claim: element reads should not inherit container storage class through
  provenance (a load/collection-element correctness fix).
- Risk: it modifies the freshly flipped HIR reclamation analysis. Validate
  against the plan-parity oracle: run `analysis::reclamation`, `hir::reclamation`,
  `publish_reclamation`, `reclamation_runtime`, memory campaign, quick gate,
  sweep. Any HIR/AST drift is a blocker.
- The PR body reports 10 full-gate failures on an older HEAD, nine attributed
  to pre-existing branches (C boundary/C++ roundtrip, ffi_proof_engine_tests,
  integration_features) and one environmental. Re-run the full gate on current
  main and compare the failing set.

### Merge protocol
1. `git fetch` both PR heads; `git merge-tree` against current `main` and each
   other (they touch disjoint files today).
2. One temp worktree per PR; merge; run the targeted suites above.
3. Merge #93 first if HIR parity is clean (it touches live analysis), then #92
   (backend layout), or as conflicts dictate; re-run quick gate after each.
4. Never weaken a failing test to land a PR; record any pre-existing failure in
   the plan/BUGS with the exact test names.

## Part 2 — H5 retirement

Current debt meters: **`to_ast::` 53 refs**; **`hir_emit_enabled()` 39
branches**; emitter entries still take AST items; `hir_stmt_bodies` /
`hir_sourced_body` still bridge.

### Phase A — Emitter entries take HIR
- `emit_definition`/`emit_transaction` (`emit_toplevel.rs:4923`, `:5954`) take
  `&HirCallable`; delete the body hook: `hir_stmt_bodies` (llvm/mod.rs:1451,
  build at 8701, refill at 2850), `hir_sourced_body` (emit_hir_stmt.rs:30),
  and the `hir_sourced_body` calls in `emit_toplevel`.
- Declaration surfaces on HIR: params/outputs/contracts/exports, global
  initializers, `emit_ssa_main` txn walk, `reordered_items`, export ABI,
  module metadata. Gate: A/B opt-out-vs-default byte-identical, sweep, daily_use.
- Acceptance: no `hir_stmt_bodies`; entries take HIR; `to_ast` count drops.

### Phase B — Remaining consumer analyses
Port with the established parity-then-flip recipe:
1. `destination_ownership` (1.2k; plan consumer in `AnalysisResults`).
2. `destination_model` (2.9k; loop-engine plans).
3. `inline_cost` (293; inline decisions).
Then re-sweep `crate::analysis::` uses in `backend/mod.rs`; `range`/`dataflow`
remain consumerless by decision (no port), `call_graph` is an empty field.
Each: AST oracle, debug-assert flip, named consumer, module tests.

### Phase C — Native-ize remaining reconstruction (drive `to_ast` down)
Locations today: `emit_expr.rs` 22, `emit_hir_stmt.rs` 12, `emit_stmt.rs` 3,
`backend/mod.rs` 3 (+ hir-side boundaries). Convert the dispatch-only families
(Match scrutinee/arms, Slice/Index, StructLiteral/Tuple, MethodCall/Reflect/
Field, short-circuit, plugin intercept) to HIR-taking helpers. Documented
constructor exceptions (collection-witness List arm, `#StdOut` `Print#`) are
kept only if the owner explicitly accepts them; the exit criterion is
`to_ast` = 0 outside those exceptions, or the exception list is ratified.

### Phase D — AST arm deletion / flag retirement
After the opt-out release window:
- Delete AST arms family-by-family (Rule 6 inversion), starting with arms whose
  seam paths are already HIR-native; each deletion runs the full battery.
- Remove `hir_emit_enabled()` branches (39) as their AST alternatives vanish.
- Finally delete `hir::to_ast` and the AST-only declaration shims.
- Keep the sweep/A-B/determinism nets; interpreter stays AST (Rule 5).

### Gates per step
`bash scripts/gate.sh quick`; `cargo test --all` before pushes touching shared
analysis; corpus sweep; A/B opt-out-vs-default; determinism quick; daily_use;
Praetor on changed dirs; docs in the same commit.

## Part 3 — Alternatives (documented, not taken now)

- **Wasm32 `.bv` Ptr physical-width ABI** — unblocks V2. Deferred: architectural
  (TypedRegister carries no LLVM type; Ptr handles are i64 by convention), needs
  a convention decision + inttoptr/ptrtoint audit. Trigger: before the next
  Tier-1 release gate or when a wasm consumer appears.
- **Verification growth** — P7 to 15 algorithms, P4 generator slides, P2
  compile-fail expansion. Deferred as incremental background work; no
  architectural blocker.
- **Stability v0.1** — freeze, nightly perf baseline, preview release. Deferred
  until H5 deletion lands so the freeze happens on the final architecture.
- **Doctrine lanes** — enum-boundary Proof 3 gate removal, memory auto-free M1,
  lifetime substrate phases. Deferred; they are larger independent programs and
  touch runtime semantics, not the HIR retirement path.
- **Intel/GPU ceilings** — separate active lane; not blocked by this plan.

## Estimates
PR validation: 0.5 day (plus any PR-specific fixes). Phase A: 1–2 days.
Phase B: 3–5 days. Phase C: 2–4 days (family by family). Phase D: 1–2 days
after the opt-out release. Docs/ledger reconciliation included in each commit.

## Execution record (2026-11-12)

- Reviewed and merged **#93** (collection element provenance): targeted HIR/AST
  reclamation suites, publish/reclamation runtime, quick gate green; full suite
  matched the pre-existing `c_driver_boundary` failure.
- Reviewed and merged **#92** (nested enum allocation/payload layouts):
  `nested_enum_allocation` 1, enum 80, quick 42/42; same single pre-existing
  full-suite failure. BUGS.md append conflicts resolved during both merges.
- Runtime integration fix included en route: `briev_rt.c` now includes the coll
  request arena, arena entry points are weak (FFI object + runtime include must
  coexist), and owned-`_start` harness links pass `-nostartfiles`
  (`c03c12e6`, `e28919b3`).
- **Known blockers before Phase D:** the HIR/AST reclamation-plan drift on
  `c_driver_boundary` (BUGS.md 2026-11-12) must be fixed while the AST oracle
  still exists. Phase A (entry switch) can proceed; do not delete the oracle
  until this drift is resolved.
