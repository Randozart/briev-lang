# PTX tier: mma-issue ceiling investigation (research round 2026-09-11 evening)

## Why

The double-pump rejection (previous section) refuted the shared-BW model.
The shipped kernel (21.0 TF @4096³) sits at 86% of our own compute-only
ceiling (22.7 TF) but at 41% of the dense f16-acc silicon peak (~51.6 TF)
and 50% of the cuBLAS anchor (42 TF). A research round identified what the
anchor actually is and what it does differently.

## Research findings

**The anchor is cuBLAS.** The vendored llama.cpp routes F16×F16 mul_mat to
`use_batched_cublas_f16` → `cublasGemmEx` (ggml-cuda.cu:2450, compute type
f16 — only the f16-acc rate explains 42 of a 51.6 dense peak). "Beating
the anchor" = matching the CUTLASS-style sm80 mainloop, which is public:

CUTLASS `efficient_gemm.md` + Bruce-Lee-LY/cuda_hgemm (≥95% of cuBLAS on
sm_86, MIT) recipe:

1. Multi-stage cp.async pipeline: 3–4 stages, k-depth 32 (ours: 2 × 16)
2. Ps2r register double-buffer: next kstep fragments load under this
   kstep's mma phase (ours: 2-group ld-ahead)
3. ONE epilogue: full-K register accumulation (ours: y RMV every 512-k
   = 8 drains per K-loop)
4. Warp tile 64×64 = 32 independent mma chains (ours: 32×64 = 16)
5. cp.async 16B transfers, swizzled smem, L2-aware CTA rasterization

**The decisive local fact**: fills are already subordinated (shipped 21.0
vs compute-only ceiling 22.7; consistent with the S4/chunk VERDICTs). The
wall is inside the mma phase. Either the ISA needs ~32 independent chains
(our 16-chain schedule issue-stalls), or the 22.7 ceiling was our
schedule's ceiling, not the hardware's. A pure-issue microbenchmark on
this box answers that — stronger than paper numbers for an 110W-capped
3060.

## Baseline table (commit 54329eaf, sustained, batched)

| measurement | value |
|---|---|
| PTX f16acc (4,4)@512T 4096³ | **21.0 TF** (1.22e-3) |
| PTX f16acc 2048³ / 8192³ | 17.6 / 20.1 TF |
| PTX f32 (4,2)@256T 4096³ | 19.4 TF (2.44e-4) |
| coopmat f16acc 2048³ / 4096³ / 8192³ | 27.7 / 7.7–9.5 / 21.2 TF |
| compute-only ceiling (mma_ceiling_bench, ledger era) | 22.7 TF |
| **anchor: cuBLAS f16 f16acc 4096³** | **42.0 TF** |
| dense f16-acc silicon peak (boost clocks) | ~51.6 TF |

## Experiment ladder

| # | experiment | expected | decision point |
|---|-----------|----------|----------------|
| E1 | pure-mma issue microbench: chains ∈ {4,8,16,32,64}, f16acc + f32acc, no smem | pins the ISA ceiling | if 16 chains cap ≈23 TF → E3/E4 must deliver 32 chains; if ≈45 TF → the schedule overhead is the wall |
| E2 | full-K accumulation (drop the 8 y-RMV drains; contract: cuBLAS/coopmat measure 4–8e-3 at K≤8192 < 1e-2 gate) | +2–5% | cheap keeper |
| E3 | k32 × 3-stage pipeline (CUTLASS sm86 config, 72KB, 2 CTAs) | +5–15% | main structural rung |
| E4 | Ps2r phase-level fragment double-buffer + hoisted addressing (the 2026-09-10 hoist lost 11% — revisit only at k32) | +5–10% | after E3 |
| E5 | cp.async 16B fills + L2 rasterization swizzle | +2–8% | polish |

Protocol per rung: dump → ptxas reg check → on-device correctness ≤1e-2 →
sustained interleaved A/B (ptx_gemm_bench, 3 reps) → 2048/4096/8192 sweep →
VERDICT in the ledger (docs/plans/2026-09-08-ptx-tier-execution.md).

## E1 design

PTX kernel, one .b64 param (state pointer, DCE-guard store target), no
smem: N independent mma chains per warp (N acc register pairs), operands
in registers from lane/tid-derived constants, R iterations of an N-chain
mma block. Launch: 512-thread CTAs × 28 (1 CTA/SM, matching the GEMM's
occupancy). Total FLOP = R·N·4096·warps; ptx_gemm_bench reports TF from
M,N,K = the cube root — keep totals ≈ 100–150 GFLOP so runs are ~5–8 ms.

Sweeps: chains {4, 8, 16, 32, 64} × {f16acc, f32acc}. The f16acc/f32acc
ratio cross-checks the GA10x 2× dense rate claim on this driver.

## Documentation plan

- This plan: hypotheses, baselines, ladder.
- Ledger (2026-09-08 execution doc): E1 VERDICT + rung results as they land.
- No architecture doc changes until a rung lands (E3 would touch
  abv-gpu-doctrine's tier notes only if it changes the default config).

## E1 VERDICT (2026-09-11 night): schedule overhead is the wall, not chains

Pure-issue microbench (dump_mma_microbench, 512T × 28 CTAs, sustained):

| chains | regs | TFLOP/s |
|--------|------|---------|
| 4 | 20 | 52.4 |
| 8 | 28 | 52.6 |
| 16 | 42 | 52.8 |
| 32 | 74 | 53.4 |
| 64 | 138 | launch-impossible at 512T (138×512 > 64K regfile) |

**16 chains saturate the tensor cores at ~53 TF — the full dense peak.**
The plan's decision point resolves to the second branch: the 22.7 TF
"compute-only ceiling" was our KLOOP's own ceiling (ldmatrix + per-ld
address math + branch competing for issue), not the hardware's. The
52.8 → 22.7 → 21.0 decomposition puts the entire 2.3× inside the mma
phase's support stream.

Consequence for E3/E4: do NOT design for 32 chains. Design for a
skinny KLOOP — loop-carried pointer arithmetic (kstep stride adds
instead of per-fragment address recomputation), k32 stages, phase-level
Ps2r — so the issue budget goes to mma, not math. E1b/E1c (ldmatrix-mix
and ALU-load sensitivity microbenches) quantify the decomposition next.

## E1b/d/f ladder + rasterization VERDICT (2026-09-11 late night)

The mix microbenches (dump_mma_mix_microbench, variants b/c/d/f) decompose
the 52.8 → 21.0 gap:

| rung | adds | TFLOP/s |
|------|------|---------|
| E1 | pure mma issue | 52.8 |
| E1b | + ldmatrix(x4+8×x2) + 150 ALU ops, fixed addrs | **53.5 (free)** |
| E1d | + cp.async fills + wait_group 1 + bar.sync, L2-resident data | 42.5 |
| E1f | E1d with STREAMING (DRAM-latency) fill reads | 32.9 |
| real kernel | | 21.0 |
| cuBLAS anchor | | 42.0 |

Readings: support ALU is FREE (schedulers absorb it; the 2026-09-10
"hoisting dropped 11%" note stays unexplained but the issue-budget model
is dead). The fill rhythm (wait+barrier) costs ~10 TF; DRAM-latency
streaming costs ~10 more. E1d — fills+barrier with L2-resident data — is
exactly cuBLAS.

**E5-rasterization VERDICT: REJECTED (−7%).** Group-swizzled the CTA
decode (8m×4n, divisor-safe): 19.6 vs 21.0 across three tight reps. The
L2-reuse model does not bind for this kernel at this shape — DRAM
bandwidth is far from saturated (~56GB/s of ~360), so packing panel
sharers closer in time buys nothing and the theory is refuted. Reverted;
the negative stands in the ledger per rule 20.

Open for next session (the 32.9 → 21.0 delta, three suspects, all cheap
probes): wait_group 0 in the real kernel vs 1 in E1f; 2-CTA/SM
occupancy contention (E1f ran 1/SM); the every-32-iteration y-RMV
promotion. The s4 paradox (deeper prefetch slower) remains open and now
bears on the wait_group question directly.

## Probe round 2 (2026-09-11 late night): occupancy heals fills; ldmatrix lookahead is the wall

Corrected-accounting occupancy sweep (E1f streaming fills):

| occupancy | E1b no-ALU | E1c +150 ALU | E1f +streaming fills |
|-----------|-----------|--------------|----------------------|
| 1 CTA/SM | 52.6 | 53.8 | 32.7 |
| 2 CTA/SM | 54.7 | 54.0 | 41.0 |
| 4 CTA/SM | 55.2 | 54.9 | **42.5** |

- **wait_group 0 vs 1: no difference** (E1g = E1f within noise). The
  full-drain theory is dead; the s4 paradox is NOT wait_group slack.
- **Support ALU is free at every occupancy** — the issue-slot model is
  dead for good.
- **Occupancy heals the fill-latency stall**: 4 independent CTA fill
  streams reach cuBLAS level even with DRAM-latency fills. The real
  kernel runs 2/SM (64 regs × 512T).

**The no-fill KLOOP re-measured TODAY: 29.9 TF** (ledger's 22.7 was
pre-pipeline-fix). vs E1b's 53.5 with identical ldmatrix+mma counts and
fixed addresses: the delta is the **ldmatrix lookahead depth**. The B
schedule keeps 2 groups live with a ld-ahead of g+2 — one or two mma
(~8–16 clk) hide a ~30 clk ldmatrix → each of the 10 lds per kstep
stalls ~15–20 clk ⇒ ~89–130 clk/kstep measured. The cure (all 8 B + 2 A
fragments live across the phase boundary, or k32 with cross-kstep
prefetch) costs +18 registers — over the 2-CTA/SM budget at 512T.

Next session, in order:
1. Re-emit (2,4)@256T stages-2 f16acc and re-sweep the config — the
   12.2/13.6 sweep numbers predate the pipeline-pair fix; at 64 regs
   × 256T the (2,4) geometry runs **4 CTAs/SM** (the E1f@4/SM effect)
   AND frees the registers for deeper fragment lookahead.
2. If occupancy wins, the new geometry becomes the select default.
3. Otherwise: ld-ahead restructuring against the register wall
   (triple-lookahead at 512T, or 256T with full-phase preload).
4. The y-RMV promo (~7% of memory instructions) is a cheap final rung.

## Probe round 3 addendum (same night): occupancy loses on the real kernel

(2,4)@256T stages-2 f16acc (the 4-CTA/SM candidate), three tight reps:
**15.58 TF vs 21.0 at (4,4)@512T.** The E1f occupancy effect does not
transfer to the real kernel — the config sweep's (4,4) verdict stands
post-fix. The occupancy lever is dead.

The register-wall gamble is nonetheless rational: E1b measured 52.6 at
1 CTA/SM — occupancy only pays when there are stalls to hide, and
full-phase fragment preload removes the stalls themselves. E4's shape:
1 CTA/SM × 512T × ~90 regs, all 8 B + 2 A fragments live across the
phase boundary, addresses computed as loop-carried increments. Target:
the no-fill KLOOP ceiling 29.9 → 45+, shipped 21.0 → 30+. E4 is next
session's opening rung.

## E4a VERDICT (2026-09-11 night): cluster preload +1% — kept; the wall moves again

E4a shipped: all 8 B + both A fragments load back-to-back before the mma
phase (the g+2 alternate-pair pipeline and its historical bug site are
gone — simpler schedule, same 64 regs after ptxas, correctness identical
at 1.789e-03). Result: 21.43 vs 21.23 TF — a keeper, not a rung.

The ld-stall model over-promised: ptxas was evidently hiding more of the
g+2 lookahead than the model credited. The decisive unexplained number
is now **E1f vs the real kernel at identical 2-CTA/SM occupancy: 41.0 vs
21.0**. Everything the ladder tested (issue, chains, ALU, ld order,
wait slack, occupancy) is excluded; what E1f lacks vs the real kernel:
the every-512-k y-RMV promotion, the swizzled-smem write/read bank
interleaving with cycling kstep addresses, and the 512×2-thread barrier.
Next session: strip the promo from a dump variant (the only single-
feature probe left that can carry ~20 TF), then the smem-port question.

## THE WALL BROKEN (2026-09-11, night): full-K + store-only epilogue — 29.3 TF (4096³)

The promo-strip probe (sed the predicated branch unconditional) measured
45.4 TF — the every-512-k y-RMV promotion was costing ~half the kernel.
Root causes, in order of discovery:

1. **Full-K accumulation**: the 138 pipeline-draining RMV rounds are gone;
   the f16x2 chains accumulate the whole K loop. Contract verified on
   device: 5.2e-3 @K=4096, 8.2e-3 @K=8192 — the same K-budget curve the
   coopmat tier documents (boundary ≈ K=12288, beyond that the f32-acc
   tier serves).
2. **Store-only final epilogue**: with full-K, the final pass owns every
   y element — the RMV's cold-miss global READS (17 TF of end-of-kernel
   DRAM interference: 45.4 stripped vs 28.0 with one RMV round) and the
   y-zeroing pass are both gone. The final store is a straight-line tail
   AFTER KEND — the in-loop predicated promo structure itself measured
   28 vs 45 with an identical body made unconditional, so the loop tail
   stays clean.

Production blob path (stale-binary artifact chased down: ALWAYS rebuild
the release binary before trusting a blob bench):

| shape | before (session start) | now | rel |
|-------|------------------------|-----|-----|
| 2048³ | 17.6 | **25.5** | 1.30e-3 |
| 4096³ | 21.0 | **29.3** | 4.44e-3 |
| 8192³ | 20.1 | **30.2** | 8.22e-3 |

Tier picture flipped: the PTX f16acc tier now beats coopmat at 4096³
(29.3 vs ~9.5) AND 8192³ (30.2 vs 21.2), nearly ties at 2048³ (25.5 vs
27.7). Anchor: 29.3/42 = 70%.

Open: the stripped-probe 45.4 ran at 40 ptxas regs (3 CTAs/SM — the
dead promo body shrank the allocation) vs our 64 (2 CTAs/SM). Whether
a ≤42-reg schedule of the LIVE kernel exists (3 CTAs/SM) is the next
rung; the Coopmat 2048³ lead (27.7 vs 25.5) may also fall to it.

## Fill-gap diagnosis (2026-09-12): the 29.3 → ~41 TF mystery

### Background

E1f (streaming fills + wait_group + bar.sync, 512T × 28 CTAs) measured
41.0 TF at 2 CTA/SM — the same occupancy as the shipped kernel (29.3 TF).
The ~12 TF gap (29%) must come from a difference between E1f and the real
kernel.

**Critical discovery**: E1f's fills do NOT write to shared memory. The
cp.async targets are register operands (`%rdB0`–`%rdB5` = .b32 registers),
not smem addresses. E1f streams DRAM → registers → mma (the mma reads
from the same registers). The real kernel streams DRAM → smem → ldmatrix
→ registers → mma. E1f's41 TF therefore measures the mma + DRAM-fill
ceiling WITHOUT the smem write/read path.

### Hypotheses (ordered by suspicion)

**H1: Smem write contention (fill vs mma overlap).** The real kernel's
cp.async writes to smem during the fill phase; the ldmatrix reads from
the same smem during the mma phase. When both happen on the same pipeline
stage (the intended overlap), they contend for smem ports. The fill writes
use a swizzled address (`((n>>3)^(k&(gr-1)))*16`); the mma reads use a
different pattern. If the contention stalls one side, the pipeline stalls.

**H2: Smem bank conflicts during mma reads.** The ldmatrix reads from
smem using a pattern determined by the lane's position. If the swizzled
fill writes leave the smem banks in a state that causes bank conflicts
during the ldmatrix reads, each ldmatrix stalls for the conflict
resolution cycles.

**H3: Fill address computation overhead.** The real kernel's fill has
~200 ALU instructions per kstep for address computation (the swizzle,
the cooperative copy logic, the stage cycling). E1f's fill has ~20 ALU
 instructions (the streaming offset). If the ALU overhead competes with
cp.async issue bandwidth, the fill rate drops.

**H4: Barrier overhead.** The real kernel's bar.sync 0 at 512 threads
may cost more than E1f's barrier (same thread count, so this is unlikely
— but worth measuring).

### Experiment design

New microbenchmark: `dump_mma_fill_microbench` (follows the pattern of
dump_mma_microbench / dump_mma_mix_microbench). Four variants:

**Variant a (baseline)**: E1f-style fill — cp.async to registers (no smem),
wait_group 1, membar.cta, bar.sync, then 16 mma from the register-held
operands. This should reproduce the41 TF baseline and confirm the setup.

**Variant b (smem-fill, no swizzle)**: cp.async to smem (the real kernel's
fill targets), sequential address computation (no XOR swizzle), then
ldmatrix from the same smem, then 16 mma. Isolates the smem write/read
path without swizzle overhead.

**Variant c (smem-fill, swizzled)**: Same as (b) but with the real kernel's
XOR swizzle on the fill addresses. Isolates the swizzle's contribution.

**Variant d (fill+mma overlap)**: The fill for stage N+1 happens while
mma for stage N runs (the pipeline's intended behavior). Same smem,
same swizzle, but the overlap is explicit. If (d) < (c), the overlap
contention is measurable.

All variants: 512T × 28 CTAs (matching E1's protocol), k-loop of 256
iterations, streaming DRAM source (like E1f), correct smem layout
matching the real kernel's geometry.

### Measurement

Each variant dumped to /tmp/opencode/mb_fill_{a,b,c,d}.ptx, assembled
to cubin via ptxas, timed via ptx_gemm_bench (3 reps, interleaved).

Expected results (predictions):

| variant | predicted TF | rationale |
|---------|-------------|-----------|
| a (no smem) | ~41 | reproduces E1f |
| b (smem, no swizzle) | ~33-37 | smem write/read adds ~4-8 TF overhead |
| c (smem, swizzled) | ~29-33 | swizzle adds ~2-4 TF overhead |
| d (overlap) | ~29-31 | overlap contention adds ~0-2 TF |

If H1 is correct: d < c (overlap costs more than serialized).
If H2 is correct: c < b (swizzle causes bank conflicts on reads).
If H3 is correct: c ≈ b (swizzle is free; the gap is elsewhere).
If H4 is correct: all variants with bar.sync ≈ all without.

### Documentation protocol

Results recorded in this file as they land. Each variant's VERDICT:
REJECTED (hypothesis disproved) or CONFIRMED (hypothesis supported)
with the measured delta and confidence level. The final VERDICT
identifies the primary contributor and proposes a fix.

## Fill-gap results (2026-09-12): A fill is the wall, pipeline overlap is free

### Full variant table (512T × 28 CTAs)

| variant | TF | regs | smem | what |
|---------|-----|------|------|------|
| e (baseline) | 40.5 | 48 | 2048 | clean fill + ldmatrix + mma (serialized) |
| s (swizzled) | 42.6 | 48 | 2048 | swizzled B fill + ldmatrix + mma |
| d (double fill) | 34.2 | 48 | 2048 | 2× B fill + ldmatrix + mma |
| m (mma only) | 49.6 | 57 | 2048 | mma from pre-filled smem |
| f (fill only) | 177.6 | 12 | 2048 | fill throughput (no mma) |
| p (2-stage) | 39.1 | 53 | 4096 | double-buffered: fill buf(i+1) while mma buf(i) |
| **a (A+B fill)** | **30.9** | **48** | **2048** | A fill + B fill + ldmatrix + mma (full pattern) |
| **w (A fill only)** | **33.8** | **48** | **2048** | A fill overhead = 6.7 TF |
| **i (interleaved)** | **30.4** | **48** | **2048** | interleaved A+B fills (no improvement) |
| **t (3-stage)** | **38.0** | **55** | **6144** | 3-stage pipeline (SLOWER than 2-stage) |
| **real kernel** | **29.3** | **64** | **3072** | full pipeline (2-stage overlap) |

### VERDICT: H1 REJECTED — pipeline overlap is not the wall

Variant p (39.1 TF) is only 1.4 TF below the serialized baseline e
(40.5). The double-buffered pipeline adds negligible overhead — the
smem port contention between fill writes and ldmatrix reads is absorbed
by the hardware scheduler. The pipeline overlap was a red herring.

### VERDICT: the A fill IS the wall (~90% of the gap)

Variant a (30.9 TF) accounts for nearly the entire gap from e (40.5)
to the real kernel (29.3). The A fill adds 8 stores per kstep to the
A smem region, costing ~10 TF. This matches the real kernel's
structure: the A fill (8 cp.async per kstep) is the dominant overhead.

Gap decomposition:
- B fill + swizzle: free (e → s: +2.1 TF, swizzle helps)
- Double B fill: −6.3 TF (e → d)
- A fill: −9.6 TF (e → a: 40.5 → 30.9)
- Pipeline overlap: −1.4 TF (e → p: 40.5 → 39.1)
- Store tail + misc: −1.6 TF (a → real: 30.9 → 29.3)

### Hypothesis status

| hypothesis | status | evidence |
|-----------|--------|----------|
| H1: smem write contention (fill vs mma overlap) | **REJECTED** | p (39.1) ≈ e (40.5) — pipeline overlap costs only 1.4 TF |
| H2: smem bank conflicts during mma reads | **REJECTED** | s (42.6) ≥ e (40.5) — swizzle is free |
| H3: fill address computation overhead | **REJECTED** | ALU absorbed per E1b (53.5 TF with 150 ALU ops) |
| H4: barrier overhead | **REJECTED** | all variants use bar.sync |
| H5: A fill overhead | **CONFIRMED** | a (30.9) ≈ real (29.3) — A fill is ~90% of the gap |
| H6: interleaving A+B helps | **REJECTED** | i (30.4) ≈ a (30.9) — no improvement |
| H7: 3-stage pipeline helps | **REJECTED** | t (38.0) < p (39.1) — more smem hurts occupancy |

### Why the A fill costs so much

The A fill writes 8 × 4B = 32B per thread per kstep to smem. Each
store goes through the smem port pipeline, which has limited throughput.
The B fill also uses the same smem ports. Combined, the fill work
(A + B) saturates the smem write bandwidth, leaving fewer cycles for
ldmatrix reads. The mma instruction itself is fast (49.6 TF from
pre-filled smem), but the fill-to-read ratio is the bottleneck.

The real kernel's A fill uses cp.async (asynchronous), while the
microbenchmark uses synchronous ld+st. The cp.async should be faster,
but the smem port saturation is the same — the fill work itself is the
dominant factor, not the copy mechanism.

### Fill overhead decomposition

- A fill alone: 6.7 TF (e → w: 40.5 → 33.8)
- B fill alone: ~3 TF (a minus w: 30.9 vs 33.8, but this is approximate)
- Total fill: ~10 TF (e → a: 40.5 → 30.9)
- Pipeline overlap: 1.4 TF (e → p: 40.5 → 39.1)
- Store tail + misc: 1.6 TF (a → real: 30.9 → 29.3)

### Actionable fix: widen A fill to 8 or 16-byte cp.async

The A fill uses 4-byte synchronous stores. Widening to 8 or 16-byte
cp.async would:
1. Halve or quarter the number of store instructions
2. Bypass the register file (cp.async writes directly to smem)
3. For 16-byte: bypass L1 cache (L1 BYPASS mode)

The constraint: smem destination must be 8 or 16-byte aligned. The
current A fill writes to `rdA + g*64` which is 64-byte aligned — more
than sufficient.

Implementation: replace `ld.global.b32 + st.shared.b32` with
`cp.async.ca.shared.global [dst], [src], 8` or `16`. This requires
computing the shared memory address via `cvta.to.shared` and adjusting
the thread-to-data mapping so each thread copies 8 or 16 bytes.

The 3-stage pipeline (t: 38.0 TF) is slower than 2-stage (p: 39.1)
because the extra smem (6144 vs 4096) reduces occupancy. The 2-stage
pipeline is already optimal for this tile size.

### Widened A fill: measured 2026-09-12

cp.async research: 4/8/16B widths; `.cg` (L1 bypass) is 16B-only; 4/8B
use `.ca` (L1 ACCESS). CUTLASS fills A with 16B cp.async, 4 threads per
row (tid>>2 = row, tid&3 = 8-f16 chunk), 4 passes per stage.

Microbench (sync ld+st fills, interleaved ×3): a (8×4B) 31.6 TF,
8 (4×8B) 37.0 TF, c (2×16B) 35.4 TF — 8B beats 16B (each 16B lane
transaction splits across 16 A rows; 8B across 8).

On-kernel A/B (4096³ f16acc, per_a=2 → 1×8B vs 2×4B cp.async.ca,
interleaved ×4, same window): old 28.97 TF, new 29.41 TF — **+0.44 TF,
new wins 4/4 rounds**, correctness byte-identical (5.208e-3, same worst
element). All shapes pass: 2048³ 1.5e-3 / 25.4 TF, 4096×4096×16 exact /
2.1 TF, 8192³ 9.1e-3 / 30.6 TF. 64 regs, no spills — 2 CTAs/SM kept.

**Lesson (generalizes the LTO lesson): a synchronous-fill microbench
overstates widening wins — the real fill is already async cp.async, so
the microbench's gain comes mostly from unblocking the register
roundtrip, which cp.async never paid. On-device A/B is the only verdict.**

Shipped as a rung ladder in `tensor_gemm_ptx_smem_mw` (a_fill_rung):
per_a%4==0 → 16B `.cg` (L1 bypass), else per_a%2==0 → 8B `.ca`, else
4B. Prologue + K-loop fills merged into one `emit_a_fill` emitter.

### Next levers (unmeasured)

1. **B fill widening**: per_b=4 → 16B×1 fires natively; the XOR swizzle
   is exactly 16B-granular (swizzle unit == copy unit), so 16B `.cg`
   copies map cleanly. B cost ~4.8 TF.
2. **warp_mh=4 config**: per_a=4 → the 16B `.cg` rung fires for A too;
   also quarters B loads (4×/kstep vs 16×). Unknown why production
   selects warp_mh=2 — worth an A/B.

### B fill widened: measured 2026-09-12 (later)

The insight that made this safe: the B smem swizzle permutes exactly
16B chunks — a 16B global chunk (8 consecutive n of one k-row) IS one
swizzle unit and lands whole at `((n>>3)^(k&(gr-1)))*16`, so the
within-chunk offset add simply drops at the 16B rung. Emitted per
thread: 1×16B `.cg` (was 4×4B `.ca`); prologue + K-loop B fills merged
into one `emit_b_fill` emitter (rung ladder mirrors A's).

On-kernel A/B (4096³ f16acc, B-16B vs A-rung-only, interleaved ×4,
same window): 29.61 vs 26.34 TF — **+3.3 TF, B-16B wins 4/4 rounds**,
correctness byte-identical (5.208e-3, same worst element). All shapes
pass and improve: 2048³ 25.4→26.7 TF, 8192³ 30.6→30.8 TF, K=16 exact.
(Absolute numbers drift ~3 TF between DVFS windows — only the
interleaved same-window A/B is comparable.) 64 regs, no spills kept.

Session net at 4096³ f16acc: 28.97 → 29.61 TF in-window, ~70% of the
cuBLAS anchor.

### warp_mh=4: SHIPPED 2026-09-12 (the earlier rejection was invalid)

**Correction of record**: the "warp_mh=4 REJECTED" entry below (and
commit f1ea6e8b) measured mh2-vs-mh2 — the A/B patched the dispatch
constant in mod.rs, but the bench used dump-test cubins whose
warp_mh=2 literal never changed. The true mh4 dumps could not even
assemble: the f16acc register declaration hardcoded %a<8>/ %b<16>
instead of scaling with the warp shape (4*mhr / 2*gr), so every
warp_mh=4 dump failed ptxas on unknown %a8-a15. Both bugs fixed; a
selector test now pins the generalized aspect guards.

True A/B (dp44 = (4,4)@512T warp_mh=4, interleaved ×4 same window):
**32.80 vs 29.87 TF — mh4 wins 4/4 (+2.9 TF)**, byte-identical
correctness. per_a=4 fires the 16B `.cg` A rung and B loads drop 16×→
4× per kstep (per_b=2 → 8B B rung — the rung trade is net-positive,
the opposite of the invalid measurement's claim).

Dispatch flip (f16acc → warp_mh=4, f32 keeps mh2): select_mw_nw and
mw_ok generalized to the warp aspect (M%(16·mhr·mw), N%(8·gr·nw)),
shared_bytes formula now (mw·mhr·512 + nw·gr·256)·stages. All shapes
verified on-device at mh4: 2048³ 27.9 TF, 4096³ 32.3 TF, 8192³
**34.1 TF (81% of the 42 TF anchor)**, K=16 exact. 63 regs, 2 CTAs/SM.

**Process lesson (BUGS.md)**: an A/B that edits dispatch constants
while benching dump-test cubins measures nothing — dump literals and
dispatch constants diverge silently. Bench the artifact the dispatch
actually emits, or make the dump read the same constant.

### E1/E2/E3 session record (2026-09-12)

- **E1 re-decomposition** (one window): microbench e 41.8, a 30.3, 8
  35.7, c 34.1; production kernel 28.5. The sync-fill microbenches
  read one broadcast address (zero DRAM) — they model smem/instruction
  cost only; production sits ~7 TF below its sync analog, which is the
  unmodeled DRAM-side cost. Sync-model residual e−8 = 6.1 TF.
- **E2 stage sweep** (zero code): production (4,4)@512T s2 28.10 avg
  vs s4 28.28 (noise, 2/3), 82 26.9, 28 22.9, 24s2 24.1, c24 18.3.
  2-stage (4,4) confirmed optimal even post-widening.
- **E3 warp-split** (microbench variant sp): split A-warps/B-warps at
  16B each measures 32.2 vs 8's 35.2 TF — REJECTED. Halving the active
  warps per fill stream costs more memory-level parallelism than the
  width gain returns; the mixed-stream widening (variant 8, every
  thread touching both fills) is the right shape. Never reached the
  kernel.

### Window-1 session record (2026-09-12, post-mh4)

- **L3 re-anchor** (one window): microbench e 41.0 (r1/r2; r3 thermal
  dip), a 29.8, 8 35.0; f16 mh4 kernel 29.72 avg. Kernel/e ratio 72%
  (mh2 era: 68%). Kernel-vs-sync-analog gap ~5.3 TF → L2 gate passed
  on measurement.
- **L1 f32 warp_mh A/B: mh4 REJECTED** — f32 mh2 20.29 avg vs mh4
  17.54 (mh2 wins 3/3, +2.75 TF) despite mh4's occupancy doubling
  (96 regs → 2 CTAs/SM vs 128 regs → 1). The f32 serial schedule keeps
  its tuned 32x64 warp. `ptx_warp_mh(f16_acc)` helper extracted —
  dispatch and dump artifacts now read ONE constant (BUGS.md rule);
  f32 correctness exact at both shapes (0.0 rel err: seed values are
  all multiples of 0.125, so every f32/f64 partial sum is exact).
- **Driver fix**: `ptx_gemm_bench.c` hardcoded an f16 y tile
  (state_bytes = y_off + M·N·2) and read y as f16 — the f32 kernels
  faulted at check time with ILM. `BRIEV_Y_ELEM` (2|4) now sizes the
  state buffer and the sampled reference.
- **L2 sector-pairing hypothesis: REFUTED by analysis (pre-build)**.
  At D = tid·16, lane pairs (2i, 2i+1) already cover cols 0-15/16-31
  of the SAME 32B sector — every A-fill warp transaction fully uses
  all 16 sectors it touches (512B contiguous). The 8B B rung is 256B
  contiguous — also sector-perfect. The ~5 TF residual is NOT mapping
  waste; candidates are cross-stripe L2-line utilization (32B used of
  each 128B L2 line per stripe), A/B stream interference, or
  fill/ldmatrix bank contention. Discriminating them needs a DRAM-real
  microbench (computed per-thread global addresses — the sync
  broadcast-address harness structurally cannot see this layer).

### Next levers (revised)

1. **DRAM-real microbench** (`dr` variant family): fills read computed
   `a[m][k]`/`b[k][n]` addresses at per-CTA tile bases; variants sweep
   A/B interleaving, stripe rasterization order, and L2-friendly CTA
   schedules. This is the instrument the residual analysis lacks.
2. **f32 tier**:mh4 rejected, but the f32 kernel at 20.3 TF vs the
   f16acc tier's 29.7 is a 1.45× gap — the f32 accumulator register
   budget (64 f32) caps the tile; a 2-stage f32 variant (smem 16384 →
   2 CTAs at 128 regs) is unexplored.
3. **2048³ boundary**: PTX 27.9 vs coopmat 27.7 — confirm which tier
   the dispatcher picks and document the crossover.

### E4c: (2,4)@256T f16acc pairing — SHIPPED 2026-09-13

**Hypothesis.** The DRAM-real microbench (2026-09-13) proved the kernel
mma-schedule-bound, not fill-bound. The (4,4)@512T CTA runs 16 warps
against the 2 mma pipes with a 24KB stage pair; an 8-warp CTA with 4
co-resident CTAs/SM (16KB smem, 64 regs × 256T = 16384 regs — exactly
4 CTAs by the register file) interleaves 4 independent fill/ldmatrix/mma
pipelines per SM and halves the bar.sync domain.

**Experiment (interleaved A/B, same window, MW_SMEM set per variant).**

| shape  | (4,4)@512T baseline | (2,4)@256T E4c | (4,2)@256T walker-default |
|--------|--------------------:|---------------:|--------------------------:|
| 2048³  | 27.91–28.10         | **31.39–31.55** | 27.75–29.01              |
| 4096³  | 34.38–34.59         | **35.08–35.59** | 30.74–31.18              |
| 8192³  | 34.20–34.22         | **36.27–36.39** | —                        |

(4,2) refutes the naive "drop the cap to 256" fix — the nw-heavy aspect
is the win, not the thread count: warps stacked along N replicate
A-fragment reads across the warp row.

**Correctness** (BRIEV_GEMM_F16ACC=1 gate): 2048³ 1.546e-3, 4096³
5.208e-3, 8192³ 9.115e-3 (identical to the (4,4) full-K signatures),
k16 0.0 exact. E2E: `brievc build examples/gpu/gemm_2048x2048x2048.abv
--backend ptx --config-dir <f16>` emits threads=256 smem=16384; the
default (f32) path is byte-identical before/after ((4,2)@256T/32768).

**Dispatch change.** `thread_cap` 512→256 for all tiers (f32 was already
256) + `walk_order(mhr)`: mhr≥4 grows nw-first ([(2,2),(1,2),(2,1)]),
mhr=2 keeps mw-first so the f32 (4,2) landing is preserved. Odd/skinny
shapes stop on divisibility guards before the order matters — (96,4096)
still falls back to (1,1).

**Process traps hit this window (BUGS.md):** MW_SMEM unset → smem=0
launch → IMA storm that looked like a kernel bug; ptxas
`-maxrregcount=128` on the natural-64 kernel PESSIMIZES to 80 regs
(occupancy 2→1 CTA/SM, −28% TF) — assemble f16acc with the production
cap 64 only.

### E5a: cross-kstep B lookahead — REJECTED 2026-09-13 (post-E4c)

**Hypothesis.** With E4c's 256T geometry the ledger's stated cure ("all 8
B + 2 A fragments live across the phase boundary, +18 regs") fits the
register file: a second B set (79 regs natural) keeps 3 CTAs/SM. The mma
would consume register-resident B, hiding all ldmatrix latency behind the
previous kstep's tensor stream.

**Implementation.** `ptx_tensor_b_lookahead` knob (default off, byte-
identical off-path, E2E-diffed); parity-branched mma blocks, prologue
preload, guarded tail prefetch — emitters in `emit_e5a_*`. Emission
verified stable across the helper extraction (79 regs, 34.46 TF pre/post).

**Result (interleaved A/B ×3-4, MW_SMEM=16384):**

| shape  | E4c cluster | E5a lookahead |
|--------|------------:|--------------:|
| 2048³  | 31.1–31.4   | 30.2–30.3     |
| 4096³  | 35.3–35.6   | 34.4–34.7     |
| 8192³  | 36.3        | 34.5          |

Correctness identical signatures (1.5e-3 / 5.2e-3; k16 exact).

**Why it loses.** Three compounding costs, one doubtful gain:
1. The tail prefetch is NOT earlier in the dependency chain — the mma of
   kstep s+1 still waits ~fill-issue + A-lds past the B lds, the same
   distance the E4a cluster's first mma waits past its own lds. ptxas
   already interleaves the cluster's lds with the fill issue.
2. 79 regs → 3 CTAs/SM: the occupancy-heals-fills effect (probe round 2)
   is worth ~1.5 TF per CTA slot — gone, worst where fills dominate
   (8192³, −5%).
3. Post-barrier smem burst: the tail B lds from all 8 warps issue
   simultaneously after the bar, instead of being staggered by mma
   completion times inside the compute cluster.

The lookahead may return for stages≥3 / k32-deep pipelines where the
dependency distance argument genuinely changes; the knob stays as the
instrument. Default off; ship path byte-identical.

### E5b: B-region bank de-phase — REJECTED 2026-09-13 (post-E4c)

**Hypothesis.** A at +0 and B at +8192 are both ≡0 mod 128 (same bank
phase); a 32B pad (+8 banks, 16B ldmatrix alignment kept) lets A and B
ldmatrix from co-resident warps co-issue instead of serializing on shared
bank groups.

**Result (interleaved A/B ×4, pad=32, smem 16448, 64 regs / 4 CTAs/SM):**
E4c 35.43/35.85/35.82/35.49 vs E5b 33.62/34.79/34.94/35.12 — loses 4/4
by 1-2 TF at 4096³. Correctness identical (5.208e-3).

**Verdict.** The same-bank-serialization model is dead: same-warp
ldmatrix issue serializes on the LSU regardless of banks, and cross-warp
phase collision is already staggered by warp scheduling. The aligned
layout is neutral-or-better. `ptx_tensor_bsmem_pad` knob kept default-0
(one-line additive in the generator; instrument for future smem-layout
work). No further pad sweep — the mechanism is refuted, not under-tuned.
