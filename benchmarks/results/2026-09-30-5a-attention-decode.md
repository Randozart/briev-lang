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

## MEASUREMENT CAVEAT (added after clock probing)

The composite launch latency is **bimodal**: over 200 reps, **p10 =
200.8 µs, p50 = 2470.5 µs, p90 = 2527.8 µs**. The p10 is consistent with
a ramped clock; the p50 with a capped one — the same small-grid clock
cap found in the GEMM campaign (a 20-CTA kernel sitting at ~210 MHz).
So the "20× off target" figure is **partly a rig artifact**: the ramped
latency is ~200 µs (≈1.6× off the 125 µs target), not 2478 µs.

This makes a **reliable sustained/clock-controlled decode measurement the
prerequisite** before any kernel optimization — the current per-step
microbench (host push between launches) lets clocks fall between
samples. Also note the 3-kernel pv p50 = 3179 µs is confounded the same
way.

Revised reading: decode attention is ~200 µs ramped vs ≤125 µs target
and ~58 µs ggml — a ~1.6× gap to the target (3.4× to ggml), driven by
the `grid = H` underfill (no split-K) plus per-`j` pass overhead — NOT a
20× defect.

## Recommendation

Re-scope 5a: (1) gate the decode path by shape (prefill → composite,
decode → the fast path); (2) implement the parallel decode reduction; (3)
delete `fused_attention_*` only once the composite meets decode parity,
per Rule 24's retirement gate. The measurement rig is the m4 +
composite_decode microbenches above.
