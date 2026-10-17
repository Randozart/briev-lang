# The dead-callable dispatch regression — full report (2026-10-17)

**Commits:** fix `821d8155`, harness `54232054`, sweep ledger `3c15268a`.
**Plan:** `docs/plans/2026-10-17-series-converge-watchdog-dispatch.md`.
**Results files:** `benchmarks/results/2026-10-17-post-stability-runtime.md`
(morning, pre-fix compiler), `…runtime-fixed.md` (post-fix),
`…post-stability-optimizer.md`.

## 1. Symptom

`benchmarks/series_converge` printed `""` (C reference `0.500050008`) —
MISMATCH in every results file since 2026-10-13; last MATCH
`2026-09-30-b1-rebaseline.md`. The morning post-stability runtime suite
(2026-10-17, first complete run since 10-13) showed 14 C-wins and the
MISMATCH.

## 2. Bisect — method and the flaky-gate lesson

Range: `13ee67fa` (2026-09-30 23:37, last MATCH-era commit) → `7731f545`
(HEAD at session start), 286 commits. Method: temp worktree
(`/tmp/opencode/sc-b`, removed after) + `git bisect run` with a gate
script (build debug brievc → compile `series_converge.bv` with
`BOUND=50000000` → run → grep output for `0.500050008`).

**First bisect attempt produced contradictory verdicts.** The runtime
gate (compile + run + compare output) flaked under bisect load: the same
commit classified good during the run and bad on manual re-test (×2). A
second bisect on a narrowed range flaked the same way. Root cause of the
flakiness was never fully pinned (suspect: load-sensitive `timeout 15`
on the run step); the lesson is the important part:

> **A bisect gate must be a deterministic compile-time property, not a
> program run.** The stable gate was: build brievc, emit `--llvm`, grep
> the diagnostics for `txn 'converge' dispatched via per-field phi`
> (present = good era, absent = broken era). Zero runtime, zero flakes.

Verified boundary (deterministic gate, ×2 each): parent `d6f89727` good,
`eae6ab66` bad. Intermediate points: `eb1f1614` good, `009bfa68` (=
`eae6ab66`'s descendant, not parent) bad, `a4bb0393`/`aac37915` bad.

**First bad commit: `eae6ab66`** (2026-10-06 12:44, "fix(typechecker,
interpreter): List + List elaborates to iter_chain; arrow push").

## 3. Root cause (proven end to end)

`eae6ab66` added `Import$("std/iterator.bv")` to the native prelude so
`List + List` elaborates to the stdlib `iter_chain` without a user
import. That module declares `txn iter_take_loop<T>(...)` — and per the
author, **`txn` is always non-reactive** (the remnant keyword from when
reactive nodes were called "rct txn"; `node` → `is_reactive: true`,
`txn` → `is_reactive: false`).

The txn is callable and dead (never referenced, never emitted —
defn-liveness correctly skips it), but:

1. `ReactorTransitionGraph::build` includes every effective txn
   (`is_reactive` flag carried on the node) — by design; frontend
   consumers filter it (`build_loop_shapes` skips `!is_reactive`,
   `program_convergence` filters reactive txns).
2. The backend dispatch gates re-derived counts **without the reactive
   filter**: `foldable` (`graph.nodes.len() == 1 && txns.len() == 1`) saw
   2 → false.
3. Dispatch fell to `emit_ssa_main` (direct SSA loop). The contract
   watchdog `?[(x - last)² > eps] -> print_best(x)` is emitted **only**
   by the counter engine (`loop_engine/counter.rs`, `.cmwd_` labels).
   The SSA path never emits it → the print went silent → `""`.

IR diff (good vs bad era, `--llvm`): good `main` contains
`.cm_header/.cmwd_36/.cmwdf_36` with
`call float @print_best(ptr %state, float %ppf35)` inside the folded
loop; bad `main` is `.ss_main_loop` with `any_active` and **no call to
print_best anywhere** (the function is defined but never called).

Blast radius: every program imports the prelude, so **every** program
with a single foldable reactive node rode the slow SSA path since
2026-10-06. This was not a series_converge-specific bug.

## 4. Fix (`821d8155`) — reactive-only dispatch

One bug class, five sites in `src/backend/llvm/mod.rs` (all raw counts
that included callable txns):

1. `foldable` gate — counts reactive nodes/txns via filtered vecs
   (`reactive_nodes`/`reactive_txns`); the swan-song body and watchdog
   now come from the reactive txn (was `txns[0]`, which under pollution
   could have been a callable).
2. Enum legacy single-txn params (`enum_ci/enum_ti/enum_tcn`) — same
   reactive counts.
3. A003 + dead-fields diagnostics loops — skip `!is_reactive`. Also
   kills the false `info: txn 'iter_take_loop' folded to O(1)` line that
   every build printed since 10-06.
4. `multi_foldable` async set — callables neither join nor veto the
   multi-txn fold.
5. `internal_fold_info` — reactive-count early-out (single reactive node
   = main-fold territory) and the eligibility proof filters
   non-reactive others (a callable's precondition speaks of its
   parameters at call time; folding doesn't change the runtime check).

Plus: the B4 never-dispatched warning moved ahead of dispatch-mode
selection. Its pinning test had passed for the wrong reason — the dead
callable broke the raw-count gate, shunting the program into `!folded`
where the warning lived; with reactive counting the fold happens and the
warning must fire mode-independently (folded and precomputed modes now
covered).

**New regression test:**
`dead_callable_txn_does_not_break_single_node_fold` — node + dead
generic callable txn (iter_take_loop shape) must keep the per-field-phi
dispatch, emit the `.cmwd_` check and the on-fire call. Note: it took
three program iterations to get the test right — const bound +
counter-only body legitimately precomputes / takes InlineSsa; the
series-faithful shape is runtime bound (`get_env_int!`) + non-counter
write.

## 5. Harness fixes (same commits)

- **`build_c` cross-reference false failure:** a benchmark with no C
  source but a `BRIEV_CROSS_REF` mapping (queue_drain_idio →
  queue_drain_sym) returned 1 from `build_c`, so the new build-failure
  tolerance recorded a false `queue_drain_idio:C BUILD-FAIL` and skipped
  its row in the morning suite. A cross-reference is not a failure — the
  timing phase reads the reference's binary, built by the reference's own
  `build_c` call. Only a missing cross-ref target or a failed clang is
  real (clang now guarded).
- **Dead `UTF8_ops` entry retired** (`54232054`): source moved to
  `benchmarks/archive/legacy-stdlib/` in `80c89d4c` when its stdlib deps
  were deleted; adjudicated in the 09-30 rebaseline §5. Every suite run
  since recorded a false BUILD-FAIL/SKIP row. Entry + TAG removed with a
  retirement note; C reference kept for the archive.

Operational trap found on the way: the first post-fix QUICK run still
said MISMATCH because **`target/release/brievc` was stale** (built
pre-fix). The harness uses the release binary; rebuild release after any
compiler fix before harness runs.

## 6. Verification

- `cargo test --lib` 3154/0.
- Full `ll_determinism_check.sh`: ALL DETERMINISTIC.
- Praetor `src/backend/llvm`: no new diagnostics (an interim +1 on
  `internal_fold_info` was removed by filtering reactive nodes in the
  iterator instead of adding a `||` branch; `generate_with_analysis`
  metric drift 1590→1597/424→426 accepted, same as the fixpoint commit).
- series_converge: `0.500050008` MATCH; QUICK row 0.60x Briev.
- queue_drain_idio: row restored, 0.44x Briev MATCH.
- async_counters_idio: MISMATCH (07-19 note, doubled-pair output) →
  MATCH. Baseline compiler (pre-fix, `502ca764`) still prints the
  doubled-pair order; the fixed compiler prints C's order — causality
  A/B confirmed.

## 7. Blast radius — precise numbers

Runtime suite, same machine, same day: morning = pre-fix compiler
(commit `7731f545`-era release), fixed = `821d8155`. Ratio = Briev/C;
>1 = C wins.

| bench | morning (pre-fix) | fixed | Δ |
|---|---|---|---|
| ring_buffer | 1.20x C | 0.96x Briev | recovered |
| float_math | 2.74x C | 0.90x Briev | recovered |
| float_math_nonzero | 0.97x Briev | 1.24x C | **worsened** |
| sparse_dispatch | 0.74x Briev | 0.76x Briev | ~ |
| print_loop | 0.52x Briev | 0.48x Briev | ~ |
| nbody_newton | 1.09x C | 0.97x Briev | recovered |
| nbody_newton_accel | 12.89x C | 11.97x C | ~ |
| nbody_sqrt | 0.64x Briev | 0.72x Briev | **worsened** |
| nbody_sqrt_idio | 1.06x C | 0.86x Briev | recovered |
| fasta | 0.86x Briev | 0.58x Briev | improved |
| fannkuch_redux | 4.47x C | 1.11x C | recovered |
| mandelbrot | 1.25x C | 1.20x C | improved |
| kalman_filter_runtime | 1.85x C | 1.08x C | recovered |
| knucleotide | 1.19x C | 1.22x C | **worsened** |
| cancel_math | 1.39x C | 0.88x Briev | recovered |
| bit_clear | 0.75x Briev | 0.60x Briev | improved |
| queue_drain | 1.37x C | 1.35x C | ~ |
| queue_drain_sym | 0.51x Briev | 0.47x Briev | improved |
| queue_drain_idio | BUILD-FAIL (false) | 0.45x Briev | restored |
| stack_push_pop | 0.72x Briev | 0.58x Briev | improved |
| interval_step | 1.33x C | 1.23x C | improved |
| telemetry_stream | 0.93x Briev | 0.86x Briev | improved |
| pid_control | 1.00x tie | 0.99x Briev | ~ |
| matrix_pipeline | 1.00x tie | 0.99x Briev | ~ |
| accumulator_flush | 0.93x Briev | 0.89x Briev | improved |
| sweep_sparse | 0.96x Briev | 1.30x C | **worsened** |
| sweep_mid | 1.34x C | 1.30x C | improved |
| sweep_dense | 1.03x C | 1.14x C | **worsened** |
| sweep_arr | 1.02x C | 1.13x C | **worsened** |
| series_converge | 2.00x C MISMATCH | 1.00x tie MATCH | fixed |
| global_lifetime | 0.67x Briev | 0.40x Briev | improved |
| deep_recursion | 2.00x C | 0.75x Briev | recovered |
| arena_churn | 0.86x Briev | 0.29x Briev | improved |
| linked_list | 0.55x Briev | 0.51x Briev | improved |
| hash_ops | 0.91x Briev | 0.74x Briev | improved |
| hash_ops_idio | 1.14x C | 1.01x tie | improved |
| enemy_swarm | 0.90x Briev | 0.75x Briev | improved |
| recommender | 1.04x C | 1.02x C | ~ |
| bridge_glue / bridge_multi | MATCH / PASS | MATCH / PASS | — |

(Plain `~` = within a few percent, likely noise; the suite is single-run.)

**Reading:** 10 benches recovered or flipped to Briev wins — they were
silently riding the direct-SSA path. Six rows appeared to worsen:
float_math_nonzero +0.27, sweep_sparse +0.34, sweep_dense +0.11,
sweep_arr +0.11, nbody_sqrt +0.08, knucleotide +0.03.

**The worsened set did not survive triage.** Controlled A/B against a
baseline repointed to `13ee67fa` (last pre-regression commit, fresh
release build) refuted every one, and exposed a second harness bug:
`compare_baseline.sh` only rebuilt the baseline binary **if it was
missing**, so the baseline worktree's binaries were whatever compiler had
last built them — queue_drain's was from **August 16**,
float_math_nonzero/interval_step's from **September 7**. Three
"confirmed regressions" (queue_drain 2.51x, float_math_nonzero 1.30x,
interval_step 1.11x) were comparisons against compilers *older than the
baseline commit itself*; with fresh baseline builds all three read
0.98–1.00. The script now always rebuilds both binaries.

**Final triage verdicts (5×5 interleaved, BOUND=500M, baseline =
`13ee67fa` fresh):**

| bench | vs pre-regression | verdict |
|---|---|---|
| float_math_nonzero | 0.99x | OK |
| sweep_sparse | 1.01x | OK |
| sweep_dense | 1.04x | OK |
| sweep_arr | 0.72x | faster |
| nbody_sqrt | 0.96x | OK |
| knucleotide | 1.00x | OK |
| mandelbrot | 1.03x | OK (1.20x vs C = historical gap, not drift) |
| fannkuch_redux | 1.02x | OK (1.11x vs C = historical gap) |
| kalman_filter_runtime | 1.04x | OK (1.08x vs C = historical gap) |
| queue_drain | 0.98x | OK (2.51x was a stale-baseline artifact) |
| interval_step | 1.00x | OK (1.11x was a stale-baseline artifact) |
| sweep_mid | 0.98x | OK |
| nbody_newton_accel | **1.10x** | real >10% drift vs pre-regression |

The single live signal is **nbody_newton_accel**: 1.10x over the
pre-regression compiler, on top of the long-standing ~12x gap to the
vectorized C reference (11.97x post-fix). That bench is the CPU
accel-offload subject with its own plan; recorded as the triage handoff,
not fixed here. The remaining C-wins are historical gaps of the same
shape they had before the regression window — the sweep's "beats C
everywhere" goal continues from a clean, honestly-measured baseline.

## 8. Optimizer suite (same day, post-fix)

`benchmarks/results/2026-10-17-post-stability-optimizer.md`: iir_filter,
precompute_sum, const_heavy, async_counters_idio — all precomputed,
MATCH. UTF8_ops row retired (§5). 4/4 clean.

## 9. Open items

1. **Watchdog emission gap (BUGS.md, OPEN):** watchdog exists only on
   counter-engine paths (per-field-phi, countdown). InlineSsa
   (`emit_folded_main`), version-DAG (`emit_version_dag_main`),
   direct-SSA (`emit_ssa_main`), and the reactor loops silently drop the
   `?[...] -> action` contract for non-foldable dispatches. Fix shape:
   every folded-body engine honors the watchdog (header check + on-fire
   + early exit, mirroring `.cmwd_`) or refuses the dispatch with a
   diagnostic naming the dropped contract.
2. **nbody_newton_accel**: the one live triage signal (1.10x over
   pre-regression, ~12x vs C) — handoff to the accel-offload plan.
3. Full `cargo test` still blocked by the concurrent session's untracked
   dead-API `tests/ffi_*_tests.rs` (do not touch).

## 10. Lessons (methodology)

1. **Bisect gates must be compile-time deterministic properties** — the
   runtime output gate flaked under load and produced two contradictory
   bisects before the IR-string gate settled it in one clean run.
2. **A/B provenance is part of the measurement** — a baseline binary
   whose mtime predates the baseline commit invalidates the A/B.
   `compare_baseline.sh` now rebuilds both sides every run; any future
   harness must do the same.
3. **Stale release binaries mislead** — the harness uses
   `target/release/brievc`; rebuild release after any compiler fix
   before drawing conclusions from harness output (the first post-fix
   QUICK run still said MISMATCH for exactly this reason).
4. **The regression hid in plain sight for 10 days** because the
   affected benches still MATCHed (correct, just slow) — correctness
   suites alone would not have caught it; the benchmark sweep's
   "beats C" ratio gate did.
