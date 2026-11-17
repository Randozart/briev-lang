# PTL serious-work plan (2026-11-17, night 2) — the tiled GEMM was launched 16x too large

## Status at the end of night 2 — FIXED

| item | result |
|---|---|
| P0.0 identify the mechanism | **done, and the first reading was wrong twice before it was right.** Not `emit_tiled`'s arithmetic (correct) and not `briev_accel_launch_resident` (forwards verbatim): `briev_dev_vulkan.c` parsed `OpExecutionMode ... LocalSize W H D` and kept only `W` as the workgroup's invocation count. Right for 1-D local sizes, wrong by `Y*Z` for 2-D — and the tiled GEMM's local size is `16 16 1` |
| P0.3 kernel tile guard | **done** — `emit_tiled` returns early when `tile_m >= m/TILE`; `tile_n` needs none (`wgid % tiles_x`) |
| P0.1 runtime fix | **done** — `local_total = X*Y*Z`, both dispatch sites divide by it |
| P0.2 one geometry | **done for the run path** — `prepare_run`'s tensor/tiled formulas now mirror `spirv_dispatch_geometry_stmt`; `run` and the shipped artifact agree to 0.1 % |
| P0.4 re-measure | **done for 4096³** — tiled 799.6 → **39.3 ms** (gx 65536 → 4096, the exact tile grid); coopmat unchanged at 26.1 ms. **Both tiers now beat the hand-written CL-C reference (94.3 ms)** |
| correctness fixture (P0.1 in the plan, renumbered) | **not built** — still worth having: every ceiling fixture is zero-filled, which is why a 16× over-dispatch went unseen for weeks |
| P1 NEO residency | not started |

The claim that "the tiled tier has no bounds check and writes out of range" was
**right**, but the mechanism was the launcher, not the kernel's own arithmetic — and
the guard now makes the kernel safe against the next launcher bug either way.


Follows `docs/plans/2026-11-17-ptl-night-queue.md` and
`benchmarks/results/2026-11-17-ptl-f16-tier.md`. Night 1 established that
`brievc run` was the wrong instrument and re-took the ceiling table. Reading the
*emitted* runner for the tiled tier tonight turned up something worse than a
performance gap.

## 0. The finding, in one paragraph

`emit_tiled` (`src/backend/spirv/gemm.rs:523`) decodes its output tile from
`wgid_x`: `tiles_x = n / TILE`, `tile_n = wgid_x % tiles_x`,
`tile_m = wgid_x / tiles_x`, and **never bounds-checks either**. The dispatch the
compiler emits for that tier is `n_gemm = M·N` work items
(`/tmp/ceil-c/..._runner.c:855`: `briev_accel_launch_resident(0, state, n_gemm)` with
`local = 256`), i.e. **65536 workgroups where the tile grid has 4096**. So
`tile_m` runs to 1023 against 64 real tile rows and the kernel stores into
`y[m][n]` for rows up to 65535 in a 4096-row array — a 16× out-of-bounds write.
`cooperativeMatrixRobustBufferAccess = false` on this device, so those stores are
undefined behaviour, not clamped discards. It has not been noticed because every
ceiling fixture is zero-initialised: the kernel writes zeros over zeros and the
"correctness" line prints `0x0000`.

The same 16× shows up in three places with three different wrong values, which is
why it went unseen:

| site | launches | should be |
|---|---|---|
| emitted runner, tiled lane | `n_gemm` = M·N items (65536 WGs) | `tiles_m·tiles_n·THREADS·THREADS` = 4096 WGs |
| emitted runner, coopmat lane | `(n_gemm/(16·4·64))·32` = 131072 items (2048 WGs) | 2048 WGs — **correct** |
| `brievc run`, `prepare_run` | tensor `count/256·32`, tiled `count/4096·16` | derived from the emitted geometry |

And it explains night 1's remaining puzzle: the tiled tier measured 459 ms (f16)
and 800 ms (f32) against the CL-C reference's 84.5 / 94.3 ms. Divided by the 16×
redundancy those become **~29 ms and ~50 ms — faster than the hand-written
reference**, which is the opposite of the "5–8× slow" reading taken at face value.

## 1. Why this is the right thing to do with the hours

It is a **correctness** bug in shipped codegen, not a benchmark curiosity: any
Briev program whose GEMM takes the tiled tier writes outside its output buffer
today. It also blocks every honest measurement of that tier, which is the only
non-coopmat path for f32 storage.

## 2. Plan

### P0.1 — a correctness fixture that can actually fail (do this first)

The ceiling fixtures cannot detect this: zeros in, zeros out, and no expected
result. Needed before any fix can be judged:

* a small `.abv` GEMM with **known non-zero inputs and a checked expected output**
  (e.g. `A[i][j] = i + j`, `B[j][k] = j - k` or an identity-times-row) whose
  reference is computed by the CPU path in the same file, so the GPU result is
  compared rather than printed;
* run it on both tiers (`spirv_coopmat: 1` and `0`). Today the tiled tier should
  either mismatch or fault; after the fix it must pass on both;
* put it under `tests/` (it is a codegen contract, not a benchmark) and have the
  corpus/CI run it when a GPU is present, `SKIP` otherwise.

This is the gate for everything below. A fix without it is unverifiable.

### P0.2 — one geometry, three consumers

The launch count is currently computed three times, in three places, with three
constants (`n_gemm`, `(n_gemm/(16·4·64))·32`, `count/4096·16`). Per the
frontend-driven-dispatch pillar there must be one decision:

* expose the per-kernel launch geometry from the GEMM plan itself —
  `GemmPlan::dispatch_geometry()` already exists for the coopmat tier and now
  includes the subgroup factor; extend it to the tiled tier
  (`tiles_m·tiles_n·THREADS·THREADS`) and have **both** the emitted runner
  (`runner.rs::dispatch_geometry_stmt`) and `prepare_run` read that one value;
* delete the ad-hoc `/256·32`, `/4096·16`, and the bare `n_gemm` for tiled
  lanes. A grep for numeric geometry constants in `runner.rs` should come back
  empty when this is done.

### P0.3 — a kernel must not trust its launch

`emit_tiled` (and the coopmat kernel) should guard `tile_m < tiles_m &&
tile_n < tiles_n` and return early. Defence in depth: the launcher is one bug away
from corrupting memory, and `cooperativeMatrixRobustBufferAccess = false` means the
driver will not save us. Cost: one compare + branch per workgroup, invisible at
4096³ and correct at any shape.

Also worth a look while in there: `tiles_x = n / TILE` with a non-divisible `n`
truncates silently — the plan gate is supposed to reject non-divisible shapes;
confirm it does, or make the guard cover it.

### P0.4 — re-measure, both tiers, with real data

With P0.1 as the gate and P0.2/P0.3 landed:

| tier | before (night 1) | expected after |
|---|---|---|
| tiled f16 4096³ | 459 ms | ~29 ms |
| tiled f32 4096³ | 800 ms | ~50 ms |
| coopmat f16 4096³ | 26.2 ms | unchanged (already correct) |

If the tiled numbers land there, the "f32 GEMM is 8.5× behind the reference" target
from night 1 dissolves — the gap was the dispatch — and the honest comparison
becomes coopmat 26 ms vs tiled ~29 ms (f16) and tiled ~50 ms vs reference 94 ms
(f32). Record both, then decide whether the remaining work is the tiled kernel's
panel-fill quality (CL-C loads 4 elements per thread per panel; check what
`emit_tiled`'s fill does) or nothing at all.

### P1 — NEO residency (the other real lever, if time remains)

OpenCL enumerates the same B390. NEO pays a full projection copy per dispatch;
L0 was fixed by exposing `mapped`, `launch_dev`, `launch_dev2d` and reusing one
timestamp event. Target: wall ≈ device time per dispatch. The accel runtime's
`briev_accel_launch_resident` path already exists, so this is a driver-side
(`briev_dev_opencl.c`) mirror of the Vulkan shape rather than new compiler work.

### P2 — the harness defect, one more time

`briev_lane.sh` now builds+links (night 1) so it measures the shipped artifact.
Two follow-ups: (a) `brievc run`'s dispatch must be fixed by P0.2 — it is the same
bug in a second consumer, and `run` is the only way to execute `.abv` without a C
toolchain; (b) the ceiling fixtures should gain a **correctness line** (compare
against a CPU reference) so a zero-vs-zero pass can never again look like a pass.

## 3. Order and gates

| # | item | time | gate |
|---|---|---|---|
| 1 | P0.1 correctness fixture (non-zero, checked) | 1 h | tiled tier FAILS on today's compiler; coopmat passes |
| 2 | P0.3 kernel guard | 30 m | fixture still fails (dispatch is still wrong) but no OOB writes |
| 3 | P0.2 one geometry, three consumers | 2 h | fixture passes on both tiers; `cargo nextest --lib`; grep shows no numeric geometry left |
| 4 | P0.4 re-measure + record | 1 h | table above; results file updated |
| 5 | P1 NEO residency | 2 h | wall ≈ device per dispatch |
| 6 | P2 harness correctness line | 30 m | a deliberately-wrong kernel fails it |

Start at 1. If the fixture does not fail today, my reading of `emit_tiled` is wrong
and that is worth knowing before any of the rest — the fastest possible refutation,
which is the point of doing it first.
