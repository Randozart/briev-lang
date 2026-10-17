# Defn-liveness fixpoint — emission-derived roots (bridge_glue blocker)

**Date:** 2026-10-17
**Status:** DONE 2026-10-17 (fix + tests + docs landed; verification results
below). Suite enablers (harness tolerance, pp-types determinism corpus,
baseline repoint, sweep resume) remain — see last section.
**Blocks:** `benchmarks/build_and_bench.sh` full suite (`set -euo pipefail` →
bridge build abort = whole suite); tests `pp_roundtrip_tests.rs`,
`c_driver_library.rs`; `examples/glue-host/*`, possibly `lib/compiler/*.bv`.
**Parent:** `docs/plans/2026-10-16-post-stability-benchmark-sweep.md` (step 1
blocker discovered during preflight).

## Root cause (verified at source, 2026-10-17)

`brievc build pp-types.bv` panics: `defn liveness: emitted code calls
unreached defn(s) ["str_to_int"]` (mod.rs IR-scan net). Chain:

1. `glue/c.bv` `CastTo/CastFrom` bindings → `cstr_to_briev` etc. are
   liveness roots (ProtocolDef arm).
2. `cstr_to_briev` body `(buf as Data) as String` — String-TARGET cast →
   family row roots `float_to_str` (defn_liveness.rs:865, `734a8dac`
   2026-10-05, json.bv fix).
3. `float_to_str → float_format → exp_style{,_mant} → fixed_body →
   strip_frac_zeros` (cast_lanes.bv:110, float_fmt.bv — all edges read).
4. `strip_frac_zeros` has `frac[…] as Int` — String→Int ExtCall lane →
   emitted `call i64 @str_to_int(…)`.
5. `str_to_int` unrooted: consuming-lane root reverted `74ba6742`
   2026-10-13 (correct revert — over-approx rooted parse family in every
   unit + exposed latent WASM byte_at).
6. Net fires correctly: `briev_rt.c` has no `str_to_int` (C copy deleted
   by frgn-elim `b78a03e7` 2026-09-24); unit defines it (prelude) so no
   `declare` either → generated `.ll` would be invalid
   (`use of undefined value '@str_to_int'`, BUGS.md 2026-08-04 precedent).

Any one of the three commits absent → no bug. **Not our session's
regression** (session started 2026-10-16 17:25; break date 2026-10-13).
WHY-NEVER-FLAGGED: harness `set -e` aborts at bridge (near array end — no
full suite since 10-13); the fixtures that exercise it are INTEGRATION
tests, excluded from the per-commit `cargo test --lib` gate; no CI.
Class recurrence: 5 "unreached defn" BUGS.md incidents since the net
landed 2026-09-13 — each fixed by a hand-added row. This is the first
with **no row slot** (a plain `as` cast has no intrinsic name).

## Decision: Option A — emission-derived fixpoint (chosen over B, C)

- **A (chosen):** on net hit, root the missed names + their liveness
  closure (`DefnLiveness::build_with_roots`), union into `live_defns`
  via a persisted `liveness_fixpoint_extra` set, re-emit. Termination
  is mathematical: missed ⊆ defn_params, missed ∩ extra = ∅ (live names
  are filtered out), extra strictly grows → ≤ |defn_params| rounds,
  real chains take 1–2. Ground truth = what emission actually called
  (derivation-not-recognition); no over-approx/bloat (respects the
  10-13 revert); no type plumbing; kills the whole incident class —
  future misses self-heal.
- **B rejected** (typechecker-recorded cast pairs): exact but moderate
  cross-module plumbing (typechecker → liveness + casting-graph lane
  table exposure) and still leaves non-cast rows manual (whack-a-mole
  continues for intrinsics).
- **C rejected** (row completion): smallest diff but leaves the bare
  case `export defn f(s: String) -> Int { term s as Int; }` broken
  (probe 2), over-approximates again — incident #6.

## Implementation sketch (src/backend/llvm/mod.rs)

1. `#[derive(Clone)]` on `AnalysisResults` (fields are plain data);
   `generate` reads it via `.clone()` instead of `.take()` so round N+1
   consumes the SAME analysis (bit-identical except the live set).
2. Split public `generate` (fresh entry: clears `liveness_fixpoint_extra`,
   snapshots `entry_warnings`) → private `generate_rounded(…,
   entry_warnings)` (old body; recursion target).
3. Assignment site: `live_defns = analysis…live ∪ liveness_fixpoint_extra`.
4. Net: on missed — `closure = DefnLiveness::build_with_roots(items,
   &missed)`; `extra ∪= closure.live`; defensive bound
   `extra.len() ≤ defn_params.len()` (violated ⇒ internal error panic);
   `self.warnings = entry_warnings.clone()` (drop discarded round's
   warnings); recurse `generate_rounded(items, exit_condition,
   entry_warnings)` (param still owned — ctx holds a clone at :2701).
5. Final panic (should be unreachable) keeps detailed message: missed
   name + enclosing `define` line (the TEMP instrumentation promoted).
6. Re-entrancy audit (done pre-impl): only `precomputed_analysis` was
   `take()`n (now cloned); emission-phase self-fields
   (`arena_ptr_idx`:2864, `bootstrap_txn`:2917, `exit_condition`:2701,
   `emit_all_defns`, `dep_graph`…) are re-assigned from the same
   analysis each round ⇒ idempotent; warnings restored from snapshot;
   `trap_frame_emitted` setter located + reset check (grep in emit
   modules during impl).

## Verification (ordered)

- **Probe 1 — pin window:** build at `74ba6742^` (expect PASS) and
  `74ba6742` (expect FAIL) in a /tmp worktree. [hypothesis check before
  any code]
- **Probe 2 — bare case:** minimal `export … term s as Int` via HEAD
  brievc (expect net panic ⇒ class proven beyond float_fmt).
- **Probe 3 — blast radius:** `brievc build` over glue importers
  (`examples/glue-host/*.bv`, `lib/compiler/{reader,needs_state,soa_reorder}.bv`).
- **Regression test (lib):** fixture with in-unit `defn str_to_int`
  (unrooted) + rooted defn whose lowering calls it (`s as Int`) → after
  fix: no panic, `define … @str_to_int` present; ×2 process byte-stable.
- Gates: `cargo test --lib`; full `cargo test` (pp_roundtrip +
  c_driver_library must pass again); `ll_determinism_check.sh --quick`;
  Praetor changed dirs.
- Docs same commit: BUGS.md entry (dated, cause, fix, undo),
  `docs/architecture/defn-liveness.md` safety-net contract
  ("self-converges from emitted calls, then panics"), this plan.

## Verification results (2026-10-17, all green)

- Probe 1 (bisect pin): `1c8b0bc6` PASS / `734a8dac` FAIL / `7f8ac908` FAIL
  / HEAD FAIL → window = `734a8dac` exactly (as hypothesized).
- Probe 2 (bare case): FAIL pre-fix → exit 0 post-fix.
- Probe 3 (blast radius): boundary/rank/bench/reader now exit 0.
  `examples/glue-host/node_bridge.bv` progressed from the net panic to a
  DIFFERENT latent defect — reflection `.^^Len` on an Int local
  (`emit_expr.rs:4341`, Phase-1b boundary) inside the newly-rooted
  `cstring_concat` closure the net had been masking. Logged in BUGS.md;
  separate subsystem, not in this fix's scope (file untouched).
- `cargo test --lib`: 3152 green (3151 + fixpoint regression test).
- Targeted integration: `pp_roundtrip_tests` 1/1, `c_driver_library` 8/8
  (both were hard-broken pre-fix).
- Full determinism (`ll_determinism_check.sh`, backend-touching commit
  rule): ALL DETERMINISTIC.
- Praetor: `glue_bridge_tests.rs` PASS clean; `src/backend/llvm` at
  HEAD-archive baseline 79 = working-tree 79 diagnostics — only `generate`
  metric values drifted (423→424 cyclomatic, 1587→1590 cognitive) on the
  pre-existing flag; zero new diagnostics.
- Implementation deviations from the sketch (recorded, not deviations in
  guarantee): analysis is MOVED through `generate_with_analysis` instead
  of cloned (no `Clone` derive needed on `AnalysisResults`); the
  defensive bound is `liveness_fixpoint_rounds < 16` (with chain in the
  terminal panic) rather than an `extra.len()` bound.
- Full `cargo test` (suite-level gate): BLOCKED — and the investigation
  found why it was meaningless anyway: uncompilable since `aac37915`
  2026-10-06 (`GlueTarget.host_fns`), fixed here; two remaining dead-API
  test files are untracked WIP of a concurrent session (BUGS.md second
  entry). Targeted integration runs cover this fix.

## Suite enablers (same workstream, separate commits)

- Harness: build loop records BUILD-FAIL and continues; aggregate
  non-zero exit at end — one broken benchmark must never abort the
  suite (this abort cost the project full-suite visibility since 10-13).
- `pp-types` build added to `ll_determinism_check.sh --quick` corpus
  (cheap standing tripwire for this class).
- Baseline worktree `../briev-compiler-baseline` (sits at `5d1d7e45`
  2026-09-06, unbuildable — `briev_accel_init` cfg bug at that commit):
  repoint to pre-session `502ca764` (decided 2026-10-17) so the sweep's
  A/B covers this whole change window.
- Full `cargo test` (integration) → pre-release ritual (alongside
  toolchain matrix); per-commit gate stays `--lib`.
- Then: resume the benchmark sweep per the parent plan.
