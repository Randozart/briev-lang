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

## The dot emitter (cooperative vs flat general) — VERDICT: KEEP-AS-LOAN

**Unblocked the same session**: the "typecheck defect" was NOT a
compiler defect — the campaign's reference file was CORRUPTED (a stale
git-show path during the fixture's remove/restore cycle dropped its two
import lines; the unexpanded `dot!` call's raw `Float` argument then
hit the checker). BUGS.md's entry is rewritten to the closed form; the
conformance sweep caught what the manual ref-file flow could not.

`dot_row.abv` RESTORED (declared `dot!`, D=128, 20 rows, 20 outputs).
The gate gained the `dot` kind: the all-ones seed (x = rows·K, y = K),
every output = K exactly (20 outputs).

Interleaved µs/launch (500 launches/cell, both lanes):

| round | knob | CUDA | Vulkan |
|---|---|---|---|
| 1 | on | 27.8 | 23.0 |
| 1 | off | 14.7 | 24.6 |
| 2 | on | 12.2 | 23.1 |
| 2 | off | 19.9 | 25.2 |
| 3 | on | 15.0 | 23.2 |
| 3 | off | 19.7 | 32.3 |

Warm bands: **CUDA emitter 12-15 vs general 15-20 (≈1.3×); Vulkan
emitter ~23 vs general ~25-32 (≈1.05-1.4×).**

**Verdict: KEEP-AS-LOAN** — the emitter leads on both lanes; the gap is
REAL but far smaller than the softmax's. The M3 scope covers BOTH row
forms; the dot form's closure is the cheaper first target.

## Sanity cells

gc_h128 blobs: knob ON = knob OFF (md5-identical) ✓ — the chain
fixtures decompose their counters and never route cooperative; the knob
is structurally incapable of affecting them.

## The standing state — SUPERSEDED by M3 (2026-10-03, late session)

**M3 LANED: the emitters RETIRED.** The general PTX lowering learned the
row form (plan `2026-10-03-m3-row-form-lowering.md`): the declared row
shapes synthesize into lane-chunked rounds (`GetLocalId#(0)` lanes, the
row = ctaid.y, block 32) + `SubgroupFMax#`/`SubgroupFAdd#` butterfly
lowering, through the GENERAL emitter — the hand-written
`emit_cooperative_softmax_ptx`/`emit_cooperative_dot_ptx` and the
routing claim DELETED (~200 lines). The knob `spirv_row_cooperative`
retired from the gate (the row form = the general lowering's own
decision; the SPIR-V lane's cooperative synthesis = unconditional like
the PTX's).

**The M3 row-form verdict vs the emitters' recorded warm numbers:**

| shape | emitter (retired) | M3 row form (warm) |
|---|---|---|
| softmax 4×256 CUDA | 15-17 | 13.6-17.9 |
| softmax 4×256 Vulkan | ~27 | 23.0-31.1 |
| dot 20×128 CUDA | 12-15 | 12.4-15.5 |
| dot 20×128 Vulkan | ~23 | 23.1-24.0 |

**The retirement gate: MET** — the general-derived row kernels reach
the emitters' numbers (within band, both lanes, both shapes).
Correctness: EXACT every cell (the gate), the conformance sweep 7/7,
suite 2855.

The M3-era mid-campaign lesson: the first M3 dot kernel failed CUDA
with o = 96 (= 128 − 32, one missing tile round) — the desc
block_threads = 64 (the general geometry) while the synthesized body is
written for 32 lanes: the double-covered indices + the OOB y reads. The
row-form geometry = 32 lanes, enforced at the desc.
