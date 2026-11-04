# Landing gates — 2026-11-03 (feat/typed-hir increment)

Commit under test: tip of `feat/typed-hir` after the arena/enum session.

## Gate results

| Gate | Result |
|---|---|
| `cargo test --lib` | 3460 pass |
| `cargo test --all` | **pass** (exit 0) |
| `daily_use_smoke` | 37 pass |
| `ll_determinism_check --quick` | all deterministic |
| `build_and_bench.sh --runtime` | every row **MATCH**; no unexplained regression |
| Praetor (changed dirs) | no new diagnostics (pre-existing complexity warnings only) |
| Kani | 13 `#[kani::proof]` harnesses present; not run in this pass (modules unchanged) |

## Regressions found by `cargo test --all` and fixed

1. **Invalid IR — duplicate PHI predecessor** (`async_compiled_events_test::
   compiled_ready_gate_observes_false_to_true`). The guard-body merge phi used
   the body's captured `cur_block`, but the body label was never assigned to
   `cur_block`; a body with no inline control flow resolved to the pre-guard
   block (both phi entries named it). Fix: seed `cur_block` with `.cmgbN`
   before emitting the guard body; inline CF advances it to the real final
   block (`a9d51580`).
2. **H0d audit baseline** grew 56 → 60 from the new arena/intrinsic fixtures
   (`VolatileStore#`/`VolatileLoad#`/`Free#`/`Ptr` casts surface as
   `plugin-intercept` when lowering raw source). Baseline bumped and documented;
   H3/H4 lower these.

## Runtime noise checked

`nbody_sqrt_idio` read 3.09 s once; two re-runs gave 2.51–2.52 s (0.86–0.97x C,
wins) — the outlier was noise, not a regression.

## Known pre-existing C-losses (unchanged, not introduced here)

`nbody_newton_accel`, `telemetry_stream`, `accumulator_flush`, `arena_bump`
(6.2x), `spsc_ring` (2.1x), `ws_frame` (~1.04x). `ws_frame`'s pinned baseline
remains UB-wrong; current is at C parity.
