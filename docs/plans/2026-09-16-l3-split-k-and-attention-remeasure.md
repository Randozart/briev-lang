# L3 — Split-K for 2048³ + attention-composition re-measurement

**2026-09-16.** Two experiments: (1) the wave-quantization tail at 2048³,
attacked with split-K; (2) the attention composition re-measured on the f16
tensor path. Both follow AGENTS.md Rule 20 (measure before you build) — the
plan is the experiment design, and the build only happens if the numbers
net positive.

## 1. L3 Split-K for 2048³

### The problem

2048³ runs 256 CTAs (128×128 tile) over 112 slots (4 CTAs/SM × 28 SMs) =
**2.29 waves**. The 0.29 partial wave runs 32 CTAs on 112 slots = 71% idle
for one wave's duration. Measured tail waste ≈ 24% (E7 analysis,
2026-09-13). E7 (persistent tiles) tried to reclaim it with a grid-stride
loop but lost to the loop's register cost. Split-K is different: it raises
the CTA count (more parallelism) instead of looping in place.

### The mechanism

Partition K into S chunks; each CTA computes its 128×128 tile for ONE
chunk (S× more CTAs, S× less K per CTA); partial sums land in an f32
workspace; one combine kernel reduces the S partials to the f16 output.

- 2048³, S=2: 512 CTAs → 4.57 waves → tail waste drops from 24% to ~9%.
- Contracts prove associativity safety (the split path is MORE accurate —
  f32 partials vs the f16-pair chain) and workspace liveness.
- Eligibility: shapes with < 3 waves AND K ≥ 512 (chunks stay ≥ 256 deep);
  S = ceil(112·2/ctas) bounded to divide K/16.

### The risk (why Rule 20 comes first)

The combine kernel reads/writes the f32 workspace: 2048×2048×4×2 bytes =
32 MB. At 360 GB/s that's ~0.09 ms of pure workspace traffic. The main
GEMM at 26.9 TF is 0.64 ms. If the tail recovery (~15% of 0.64 = 0.096 ms)
is eaten by the combine cost, split-K nets ZERO. The experiment must
measure BOTH:
1. The actual tail waste at 2048³ (is it really 24%?).
2. The actual combine-kernel cost (a trivial f32 workspace reduce).

### Experiment A — the tail waste

Run the existing 2048³ kernel and compare against a "full-wave" baseline.
Method: time 2048³ (256 CTAs), then time the SAME kernel launched with a
CTA count that fills 112 slots cleanly (e.g. 448 CTAs of a smaller tile, or
compare per-CTA throughput at 2048³ vs a shape with the same tile but
multiple-of-112 grid). The ratio isolates the tail.

Simpler: measure the kernel at 2048³ and at 4096³ with the SAME 128×128
tile and compute per-CTA efficiency. If 2048³'s per-CTA TF is materially
below 4096³'s, the tail is real; if they match, the tail is NOT the
bottleneck and split-K cannot help.

### Experiment B — the combine cost

Write the combine as a trivial PTX/LLVM kernel (read S f32 partials, add,
write f16) and time it at 2048². If combine_cost ≥ tail_recovery, split-K
loses; document the negative per Rule 20.

### Gate

- Correctness: 2048³ split-K output max_rel ≤ 1e-2 vs the non-split kernel
  (the f16 rounding bound; the split path is MORE accurate).
- Performance: split-K total (main + combine) > non-split by ≥ 3%
  (outside noise).
- If the experiment shows net-negative, RECORD the negative and do not
  build — the ledger is the deliverable.

## Experiment A RESULT (2026-09-16) — the premise is REFUTED

Measured 2048³ (128×128 tile, 256 CTAs) with a batched variable-grid sweep
(driver API, batch timing, 50 iters):

| ctas | waves | ms | TF | TF/CTA |
|------|-------|-----|-----|--------|
| 256 | 2.29 | 0.661 | 25.99 | 0.102 |
| 224 | 2.00 | 0.665 | 22.59 | 0.101 |
| 168 | 1.50 | 0.497 | 22.70 | 0.135 |
| 112 | 1.00 | 0.334 | 22.51 | 0.201 |
| 56 | 0.50 | 0.172 | 21.81 | 0.390 |
| 28 | 0.25 | 0.106 | 17.67 | 0.631 |

**Per-CTA TF halves when CTAs double** (0.63 → 0.39 → 0.20 → 0.10, ~0.5×
per doubling). A compute-bound kernel would show CONSTANT per-CTA TF. The
~0.5× per doubling is the signature of a **memory-bandwidth-bound** kernel:
the memory system saturates regardless of SM occupancy.

**Conclusion:** the 2.29-wave tail is NOT the bottleneck at 2048³ — the
kernel is bandwidth-bound, so the tail's idle SMs cost nothing (they'd be
waiting on memory anyway). Split-K would add CTA contention AND a combine
pass's memory traffic — strictly worse. **L3 split-K is NOT built.** This
is the Rule 20 case: the pre-build measurement refuted the hypothesis.

The flat per-CTA TF at 224 vs 256 CTAs (0.101 vs 0.102, 2.0 vs 2.29 waves)
confirms: the 0.29 partial wave adds no measurable cost.

## 2. Attention composition on the f16 tensor path

### The problem

The attention-decode benchmark (`attn_decode.abv`) uses f32 naive GEMMs.
The 0.073 ms @512² composition number (86% of cuBLAS) was measured with the
f16 tensor path. The end-to-end benchmark has never confirmed the f16
composition reaches it.

### Experiment C

Wire the attention-decode chain (qk → scale → pv) to the f16 tensor path
(already gated by the selector + `fusion_applies`), run on-device, and
compare vs cuBLAS composition at 512²/1024².

### Gate

Correctness (max_rel ≤ 1e-2) + the 0.073 ms claim reproduced. If the
composition does NOT reach it, the gap is the next target.

## Experiment C RESULT (2026-09-16) — 512² confirmed, 1024² open

The full f16 attention chain (qk → fused-scale → pv) at **512² is CORRECT
end-to-end (max_rel 1.6e-3, 0 bad) at 0.0705 ms** (per-launch-sync; the
true batch is lower). cuBLAS composition at 512² (two 512³ GEMMs, best
algo 14.7 TF) ≈ 0.075 ms. **Briev is ~10% faster than cuBLAS on the
attention composition at 512² — the 0.073 ms claim is confirmed and
strengthened** (the earlier 86%-of-cuBLAS used a default-algo cuBLAS).

At **1024² the chain FAILS** (max_rel 5.4e24): both qk and pv kernels are
CORRECT standalone (qk 1.3e-3, pv 3.9e-3), and qk's fused-scale output IS
in the aliased s2 slot (host shows 720), but pv reads garbage (+inf).
This is a **data-flow issue in the 2-kernel composition at 1024²** — the
aliased s2/s proj (6291504) flows qk→pv for 512² (1572912) but not 1024².
Recorded as a BUGS.md open item. The 512² result stands; 1024² needs the
runner's field-table/alias handling investigated.

## Deliverables

- A/B tables for both experiments (interleaved, batch timing).
- Split-K kernel + dispatch policy ONLY if net-positive.
- Findings recorded in `docs/plans/2026-09-16-gpu-strategy-findings-and-levers.md`.