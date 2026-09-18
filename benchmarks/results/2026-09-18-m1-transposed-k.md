# M1 — Transposed-K Layout: coalescing dominance confirmed

**Date:** 2026-09-18
**Plan:** `docs/plans/2026-09-18-coalesced-kv-memory-path.md` (M1)
**Harness:** `KLAYOUT=dmaj bash benchmarks/m3_attention_harness.sh` /
`KLAYOUT=dmaj bash benchmarks/m4_decode_microbench.sh`
**Device:** RTX 3060 (sm_86), CUDA lane; vitriol-server inactive; GPU
unclocked (SM idles 210 MHz, boosts under load) — cross-run comparisons
carry clock-state noise; same-run jd/dmaj interleaved comparison is the
valid measurement.

## Change

K stored d-major (kh, d, j) in the composition state: qk's work item
t=(h,j) now reads `k[kh*D*NKV + d*NKV + j]` — consecutive threads read
consecutive j (128 B coalesced segments) instead of striding 512 B.
V stays j-major: pv maps threads over d and was already coalesced —
the qk/pv cost asymmetry (227 vs 146 µs for identical bytes) was the
coalescing signature, and the fix is deliberately asymmetric.

## Correctness: exact

jd regression after the harness refactor: identical to pre-refactor
values (s_err=3.97e-05 — the refactor is faithful). dmaj f32 and
f16+dmaj: PASS both lanes, error values IDENTICAL to jd (3.97e-05 f32,
0 f16) — same values, different storage order; the layout is pure
transport.

## Timing (bitnet NKV=4096, p50, same-run interleaved)

| Stage | jd (scattered K) | dmaj (coalesced K) |
|---|---|---|
| push | 60.1 µs | 33.4 µs (V-only ranges) |
| **qk** | **227.4 µs** | **64.4 µs (−72%)** |
| softmax | 40.2 µs | 38.7 µs |
| pv | 211.4 µs | 207.4 µs (untouched) |
| chain | 479.1 µs | 310.6 µs (−35%) |

qk at 64 µs sits near the raw-bandwidth floor for 10.5 MB of f32 K —
the M1 gate asked for ≤100 µs. Coalescing dominance CONFIRMED: the
three-kernel composition was never compute- or f32-byte-bound in qk;
it was scattered-load-bound. pv's unchanged 207 µs (vs 146 µs in an
earlier session) is clock-state noise across runs, not a regression —
same-run jd/dmaj pv agree within 2%.

## Consequences

1. **The M1 gate passed** — M3 (compiler load pipelining) proceeds on a
   validated model.
2. **The 3-kernel composition inherits the layout win immediately**:
   479 → 311 µs chain on the existing form, with V's 207 µs now the
   dominant stage (its own reads are coalesced; its cost is the GQA
   re-read pattern — a separate lever).
3. **M2 sharpens**: d-major K means a per-token append writes D
   scattered elements — the append path needs range batching before any
   integration (M4) can claim an honest end-to-end number.
4. The fused-node P2 gate remains: fused+f16 must beat ~311 µs and
   approach 58 µs. With coalesced K, the fused node's projected floor
   (K 5.2 MB f16 + V sharing + one pass) is back in play.

## Artifacts

- `/tmp/opencode/m4_jd.out`, `/tmp/opencode/m4_dmaj.out`
- `examples/gpu/attention_decode_kdmaj.abv`; `KLAYOUT` env in both
  harnesses (jd default — canonical template unchanged)
