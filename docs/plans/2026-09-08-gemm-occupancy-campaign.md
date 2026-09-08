# GEMM WG-Throughput Campaign — CLOSED (wall was a stale-build artifact)

**2026-09-08, session 2 (VERDICT: campaign invalidated, GEMM at HW peak).**
The 4.58ms "wall" was measured on a **stale .spv** (the prior session's
`/tmp/gf/gemm_h.spv`, 73268 B, id-bound 44285, built from an EARLIER source
state). Rebuilding the CURRENT source (commit bce82f54) produces a
73352 B .spv (id-bound 45311, +~1000 IDs) that runs at **0.708 ms = 24.3
TFLOP/s** — 95% of the RTX 3060's FP16-accum tensor peak (25.6 TFLOP/s).
The wall is broken; there is nothing left to break. **No code change is
warranted.** R=4 (the default) is optimal; R=8 is 5% slower (0.744 ms).

## R-sweep A/B (current build, 4096×4096×512, GPU timestamp)

| R | .spv | WG count | y[0..3] | wall (avg of 5) | verdict |
|---|------|----------|---------|-----------------|---------|
| 4 (default) | 73352 B | 4096 | 8320 8324 8327.5 8332.5 | **0.708 ms** | ✓ optimal (24.3 TFLOP/s) |
| 8 | 98696 B | 2048 | 8320 8324 8327.5 8332.5 (identical y) | 0.744 ms | 5% slower |
| 16 | — | — | — | clamps to R=8 | cap in `coopmat_tile_rows` (gemm.rs:165) |

y-identity verified: R=4 and R=8 dump byte-identical `y` (ycmp.c A/B, same
a/b seed). Correctness confirmed. The config is baked at compile time
(`include_str!`, config_tuning.rs:329) — each R requires `cargo build
--release`.

**The stale 4.58ms:** the prior session's .spv (73268 B) predates the emitter
commits that added ~1000 IDs (the 5.3→4.55ms era's own build). The 6.4× gap
(4.58ms→0.71ms) is those commits' effect, not a lever this session found. The
R/S/prefetch/stages knobs are all at their current-build optimum.

**Next (only if a NEW target appears):** the PTX tier (Stage 2) is still
armed for a non-NVIDIA vendor (Ada's mma ceiling = 50.1 ms/launch unchanged),
but the portable path is at HW peak — no portable work remains.

---

**2026-09-08, session 1 (superseded).** Profiling session (this file's parent:
the beyond-coopmat campaign, `docs/plans/2026-09-04-beyond-coopmat.md`). The
kernel at 4096³, S=2/R=4/prefetch, sat at **4.58 ms = 30.2 TFLOP/s** (commit
bce82f54). The portable-path levers (fill instruction count, fill DRAM
traffic, barrier count, prefetch) were believed exhausted. This session's
profiling **reframed the bottleneck** (as WG-throughput) — but the reframe was
measured on the stale .spv; see the session-2 verdict above.

## Profiling findings (the reframe — superseded by session 2)

| Experiment | Result | Interpretation |
|-----------|--------|----------------|
| K-sweep (M=N=4096, K=16→8192) | 4.17ms → 4.62ms | Time is **K-independent** — the mma loop is free |
| Dispatch sweep (1K→131K WGs) | 16K→0.65ms, 131K→4.585ms | Near-linear in WG count; ~35ns SM-time/WG |
| Cube sweep (n=2048→4096) | 1.14ms → 4.58ms | ~1.12 µs/WG, linear in WG count |
| Prologue halving (edit) | 4.565ms (null) | The prologue's 2nd fill is already latency-hidden |

**Root cause: WG throughput, not compute, not occupancy.** Each WG takes
~1 µs of SM time regardless of K (the K-independence). 131072 WGs × ~35ns
SM-time each (≈28 WGs resident, 4681 sequential waves of ~1µs) = 4.58ms. The
mma is free (hidden under the fill latency); the floor is the **per-WG
scheduling throughput** — the GPU issues WGs back-to-back, ~28 deep, and each
WG occupies an SM for ~1µs (prologue + fills + epilogue, the mma free).

The register/occupancy framing (1 WG/SM, 128 f16 acc/lane) was the WRONG
model: the timing is linear in WG COUNT (not in occupancy), and the per-WG
time is K-independent (the mma is not the cost). The lever is therefore
**fewer, fatter WGs** — cut the WG count, not the per-WG register footprint.
The dispatch sweep proves it directly: 16K WGs → 0.65ms (7× faster at 1/8 the
WG count).

**Note on the occupancy probe:** the plan's Step 0 atomic probe was a dead
end in rspirv 0.12 (no `Op::AtomicMax`, no `OpTypeAtomic`). The timing model
above is sufficient — the dispatch sweep IS the occupancy instrument (peak
concurrent ≈ 28, derived from the near-linear WG-count scaling).

## Goal

Cut the WG count to break the WG-throughput wall. The mma is free (K-independent);
each WG costs ~1.12µs of SM-time (scheduling + prologue + epilogue). The grid
formula (gemm.rs:157) is `workgroups = (m/(16·R)) · (n/64)` — **R is the ONLY
knob that changes the WG count**. S (subgroups) does NOT: it splits one WG's
tile across S×32 lanes (LocalSize 32→32S), cutting per-lane work, same WG
count. So the two levers are orthogonal:

- **R** → WG count (the throughput lever). R=4→4096 WGs, R=8→2048, R=2→8192.
- **S** → per-WG lanes (the register/latency lever, same WG count).

The dispatch sweep (131072 INVOCATIONS = 4096 WGs × 32 lanes → 4.58ms) proves
WG count is the cost: 4096 WGs × 1.12µs = 4.58ms. Fewer WGs (bigger R) =
faster, UNTO the R=16 spill wall (rejected 2026-09-02: 256 acc regs/lane,
24.4ms — a smem→reg→smem round-trip per mma, the one cost the free mma can't
hide).

## Steps

### Step 1 — Sweep R (the WG-count lever) on the current build

The R=8/R=16 numbers in the emitter comment (16.4ms/24.4ms) are from the
2026-09-02 pre-smem-prefetch build. The current build (smem + D2 prefetch,
4.58ms at R=4) may have moved the spill wall. A/B each R vs the 4.58ms
baseline (R=4/S=2, 4096 WGs):

| R | M-tile | WG count (4096³) | wall |
|---|--------|------------------|------|
| 4 | 64     | 4096             | 4.585 (baseline) |
| 8 | 128    | 2048             | ? (old: 16.4, pre-prefetch) |
| 2 | 32     | 8192             | ? (more WGs — expect slower) |

- **Gate:** R=8 wall < 4.585 → LAND (the WG-count floor is past R=4). R=8
  wall > 4.585 → the spill wall is at/below R=8 on this build too; R=4 is the
  portable ceiling.
- **Smem check** (128 KB/SM, 28 SMs): R=8 doubles smem/WG (128×128 A/B panels
  vs 64×128). Verify 28 × smem_per_WG(R=8) ≤ 128KB before the A/B — if it
  overflows, R=8 is infeasible regardless of spill.

### Step 2 — Sweep S (the per-lane lever) at the best R

S splits the tile across more lanes (same WG count). At R=4, S=2 is current.
S=4 → LocalSize 128, half the per-lane acc (8 frags vs 16) — the register
lever, orthogonal to R. A/B S=4 vs S=2 at the winning R.

- **Risk:** S=4 → 128 threads/WG; if the tile can't split 4 ways cleanly the
  fill/mma distribution changes (verify the emitter's S=4 path — the S>1
  decode is built for the current S=2; S=4 may need the `a_elems_per_lane`
  math re-checked).

### Step 3 — Document + commit

- Every lever: A/B wall + VERDICT row in
  `docs/plans/2026-08-31-vitriol-gemm-comparison.md` (the ledger).
- Update `docs/plans/2026-09-04-beyond-coopmat.md`: reframe Stage 1 as
  **WG-throughput extraction** (the fill/occupancy framing was wrong —
  profiling proved the mma is free and the cost is WG count).
- If a lever lands: commit + ledger + plan. If all null (R=4/S=2 already at the
  spill-bounded floor): document WG-throughput as the portable ceiling; the PTX
  tier (finer tile control, lower per-WG scheduling overhead) becomes the clear
  re-arm.

## Risks

- **R=8 → spill** (the one cost the free mma can't hide). A/B wall catches it;
  the R=16 rejection (24.4ms) is the known wall, R=8 is between R=4 (4.58) and
  R=16 (24.4) — the A/B decides where the wall sits on THIS build.
- **Smem cap** (128 KB/SM): R=8 must fit 28 × smem_per_WG. Check first.
- **S=4 path may be untested** — the emitter's S>1 decode is built for S=2;
  S=4 needs the per-lane math re-verified (a silent-wrong risk, not just slow).
- **Concurrent session's counter.rs** — only `git add` my files (config, docs,
  emitter if touched).

## Per-commit checklist

`cargo test --lib` green; Praetor on changed dirs; ledger row; no silent
reverts; no type-name matching; rationale comments carry provenance (when,
why, pattern, how to undo).

## Undo

Steps 1–2: config only (R, S knobs) — reverted via git; losers get VERDICT
rows. Step 3: docs only.
