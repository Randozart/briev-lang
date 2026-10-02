# 5a — decode attention measurement (2026-09-30)

5a's plan (followup-stages) assumed the composite attention path landed
and the only work was to retire the dead `fused_attention_*` family. This
measurement **refutes that premise**: at the target geometry the decode
attention is ~20× off the ≤125 µs goal.

Geometry: bitnet-2b decode, **D=128, H=20, HKV=5, G=4, NKV=4096**, CUDA
lane, GPU 1. Rigs: `benchmarks/m4_decode_microbench.sh` (3-kernel
composition) and `benchmarks/composite_decode_microbench.sh` (one-launch
composite), p50 of 200 reps.

| path | p50 | vs target ≤125 µs | vs ggml |
|---|---|---|---|
| 3-kernel composition (qk→softmax→pv) | **3443 µs** chain | 27× | **~59× slower** |
| — qk | 222 µs | | |
| — softmax | 41 µs | | |
| — **pv** | **3179 µs** | | |
| one-launch composite (`softmax_fused!`) | **2478 µs** launch | 20× | **~43× slower** |
| ggml stock fattn (test-backend-ops) | ~58 µs | — | — |

**The pv step dominates** (M=1 single-query, N=D=128, K=NKV=4096): with
M=1 the tensor mw kernel cannot tile, so the reduction over NKV is
serialised — the classic decode under-parallelisation. qk (222 µs) and
softmax (41 µs) are secondary.

## Consequence for 5a

- The `fused_attention_*` family is **not** dead weight at this geometry:
  the plan's own target table lists "Flash decode (hand-PTX)" at
  **125 µs f32 / 91 µs f16** — ~20× faster than the composite here. The
  composite wins only for *prefill* (512²), where the 1-kernel was 10×
  slower than the 2-pass composition. So the choice is **shape-dependent**.
- **Retiring the family unconditionally would regress decode** — the
  opposite of the planned intent. Do NOT delete until the composite reaches
  decode parity (or the cost model gates decode to the fused path).
- The real 5a work is a **decode-parallel attention kernel** (split-K /
  parallel NKV reduction / lane-mapped reduction per `0051d920`/`05c`), to
  bring the composite from ~2478 µs toward 125 µs.

## MEASUREMENT CAVEAT → RESOLVED (high-REPS sustained protocol)

The composite launch latency is **bimodal at low rep counts**: over 200
reps, p10 = 200.8 µs but p50 = 2470.5 µs. That was the **clock-ramp
transient** — the first few hundred launches run at a parked clock while
the driver ramps. Re-run with **REPS = 2000** (sustained) the distribution
tightens to **p10 194.9 / p50 200.6 / p90 207.1 µs**.

**Reliable protocol: use ≥1000 reps.** True baselines:

| path | ramped p50 |
|---|---|
| composite (one launch) | **200.6 µs** |
| 3-kernel (earlier, low-rep) | confounded — re-measure with high reps |
| ggml fattn | ~58 µs |
| target | ≤125 µs |

So the decode attention gap is **~1.6× to the target** (3.4× to ggml) —
NOT 20×. The earlier "20× off" and "2478 µs" figures are retracted as
clock-ramp artifacts. The structural finding stands: the kernel is
grid-underfilled (`grid = H`, no split-K) with heavy per-`j` overhead.


## Recommendation

Re-scope 5a: (1) gate the decode path by shape (prefill → composite,
decode → the fast path); (2) implement the parallel decode reduction; (3)
delete `fused_attention_*` only once the composite meets decode parity,
per Rule 24's retirement gate. The measurement rig is the m4 +
composite_decode microbenches above.

## Split-K sweep (2026-10-01) — design option 1 measured

`benchmarks/composite_decode_split_sweep.sh` (1200 reps, ramped, the
two-launch CUDA contract: partial n*S + combine n). Same session, same
rig — compare rows to each other:

| variant | p10 | p50 | p90 |
|---|---|---|---|
| no-split | 218.9 | 220.8 | 339.1 |
| split=2 | 222.3 | 223.5 | 232.0 |
| **split=4** | **200.4** | **202.5** | **210.2** |
| split=8 | 250.6 | 252.0 | 260.9 |
| split=16 | 318.5 | 324.1 | 336.6 |

Findings:
- **split=4 wins**: −8% p50 (220.8 → 202.5) and the tail TIGHTENS
  1.6× (p90 339 → 210 — the combine stabilizes what the no-split
  single-launch leaves to clock-cap variance).
- **S > 4 regresses**: per-CTA work shrinks below the two-pass
  overhead and the combine's merge cost grows with S. 4 is the model
  factor (`reduction_split_factor(20, 4096)` = 4) — the cost model
  and the measurement agree.
- The grid-underfill lever is worth ~8%, NOT the 1.6×: with S=4 the
  grid is 80 CTAs (fills 28 SMs) yet latency barely moves — the
  residual cost is the PER-J WORK (2 passes × 128 j-iterations/warp ×
  strips + 11 butterflies + Exp# + div slowpath). Lever 2 (fuse the
  two passes, inline the butterfly, hoist q) is the remaining path to
  125 µs.
- Harness fixes landed: `composite_decode_microbench.sh` (the split
  contract drive + BRIEVC_FLAGS default) and the sweep script (the
  two-launch drive parsed from the runner's own sequence; the first
  plain launch after the partial is the combine — later plain launches
  are other lanes' full images).
- Correctness: the split machinery's numerics are device-proven at the
  softmax_gate fixtures (5.91e-06 both lanes); the decode-geometry
  split correctness gate rides the next increment (lever 2 lands with
  its own m3-harness gate).

## E2/E3 — the j-iteration body composition (2026-10-01, spy-ptxas dump)

The split=2 partial's PTX (pre-ptxas, 832 lines / 21178 bytes, dumped
via a saving-ptxas wrapper) shows the per-j body is NOT load- or
butterfly-bound — it is **address-arithmetic bound**:

- 4 strips × (q, k, v) = 12 `ld.global` per j-iteration.
- EACH load's address is recomputed from scratch per iteration: ~10
  integer ops (`mul r,h,128` + `mul r30,128,4096` + `mul.wide` + two
  `add.u64` + the const offset) — including CONSTANT-FOLDABLE products
  (`128*4096 = 524288` emitted as two muls!) and the loop-INVARIANT q
  base (`h*128` + the const buffer offset).
- The butterflies are already inline (0 `call` in the pre-ptxas text —
  the out-of-line `CALL.REL.NOINC` figures were post-ptxas SASS
  artifacts; 10 `shfl` total per kernel = one butterfly set per pass,
  NOT per strip).
- 3 `bar.sync`, 1 `ex2` — secondary.

**Lever 2 re-sized:** strength-reduce the deferred-region j loop —
hoist the q base, carry k/v addresses by stride increment (`+= 4096*4`
per j), and let the emitter's index emission skip recomputing
loop-invariant sub-expressions. This is an index-emission change in
`emit_deferred_region`'s j loop → the kernel-rule on-device gate
applies (m3 harness, both lanes) before any timing claim.
