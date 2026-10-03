# Emitter retirement A/B — the M4 emitters' half (2026-10-03)

Plan: `docs/plans/2026-10-03-emitter-retirement-ab.md`. Machine: 2× RTX
3060, llama-server KILLED pre-campaign (it held ~10GB per GPU), GPU 1
pristine, driver 615.71.09. Instrument:
`benchmarks/emitter_ab_gate.sh` (the generated-runner splice: timed
repeat + the exact verifier; knob via INLINE `--config-dir`).

## Protocol note — the gate's own first bug

The first A/B used the ENV-PREFIX form (`BRIEFC_FLAGS=... brievc build`)
— **`BRIEFC_FLAGS` is read by NOTHING in the tree** (the softmax_gate's
`${BRIEFC_FLAGS:-}` expands the CALLER's env into argv — the script
form works; the env-prefix form silently runs the shipped config). Both
"arms" ran the cooperative kernel (identical timings 36.9/37.2). The
blob-size check (both arms 13480B) caught it. Fixed: `--config-dir`
inline, and the knob verified at the reader (`[COOP-TRACE]`, removed
post-campaign).

## The softmax emitter (cooperative vs flat general) — VERDICT: KEEP-AS-LOAN

`softmax_rows` 4×256 f32, declared `softmax_rows!`, zero-seed →
every output must be exactly 1/256 (bitwise). Correctness: **EXACT PASS
every cell, both lanes, both arms** (the emitter AND the general path
are correct — the choice is pure performance).

Interleaved µs/launch (500 launches/cell, both lanes):

| round | knob | CUDA | Vulkan |
|---|---|---|---|
| 1 | on | 39.6 | 27.1 |
| 1 | off | 41.1 | 73.2 |
| 2 | on | 15.1 | 39.4 |
| 2 | off | 40.8 | 46.2 |
| 3 | on | 17.1 | 26.8 |
| 3 | off | 43.5 | 50.6 |

(One warm-up outlier per lane on the first cell; the 39.4 Vulkan ON
spike in round 2.) Warm bands: **CUDA emitter 15-17 vs general 41-43
(≈2.5×); Vulkan emitter ~27 vs general ~46-50 (≈1.8×).**

**Verdict: KEEP.** The cooperative softmax emitter wins 1.8-2.5× on
both lanes. The general path is CORRECT (exact) but half the speed —
the retirement gate ("general machinery reaches their numbers") is NOT
met. The measured gap = the M3 scope: general machinery must learn the
lane-mapped row form (32 lanes × rows, the work-item id = the row, the
subgroup reductions) to close 2×.

## The dot emitter — BLOCKED on the typecheck defect

`dot_row.abv` (declared `dot!`, D=128, 20 rows) trips the typechecker's
`undefined variable 'Float'` — a shape/name/param-order-sensitive
checker defect, filed in BUGS.md with the instrumentation shipped
(the four `[UNDEF]` raise-site probes + the `BRIEV_UNDEF_BT`
display-time backtrace). The fixture is REMOVED from the tree — the
conformance sweep caught it (the sweep works; the blob-sweeps didn't).
The dot emitter's A/B waits for the defect's fix; the fixture content
lives in the BUGS.md repro.

## Sanity cells

gc_h128 blobs: knob ON = knob OFF (md5-identical) ✓ — the chain
fixtures decompose their counters and never route cooperative; the knob
is structurally incapable of affecting them.

## The standing state

- The cooperative emitters stay (declared loans with MEASURED gaps:
  softmax 1.8-2.5×).
- The M3 scope: the row-form general lowering (the lane-mapped
  cooperative pattern) — the campaign that would retire these emitters.
- The knob `spirv_row_cooperative` stays (the loan's opt-in + the A/B
  instrument).
- The dot half: blocked on BUGS.md's `dot!` typecheck defect.
