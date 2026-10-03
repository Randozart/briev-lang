# Plan: emitter retirement A/B — the M4 emitters' half (2026-10-03)

**Status:** ACTIVE. The declared-coverage half of the M4 ladder LANDED
(`2026-10-03-declared-dot-detect-reduction-rung.md` + the matmul plan):
every cooperative specialization now follows a declaration. This campaign
runs the OTHER half — the ladder's retirement gate: **the cooperative
row-kernel emitters retire when general machinery reaches their numbers.**

## The experiment

The A/B exists as a config flip — `spirv_row_cooperative: 1` (the
hand-written emitters) vs `0` (the general/flat path) — same compiler,
same binaries. No rebuild between arms; the cleanest Rule-12 interleave
available.

**The machine contract:** llama-server KILLED before any timing (it held
~10GB on BOTH 3060s — the VRAM-full regime that produced the 2026-10-02
2× anomaly). GPU 1 = the pristine timing GPU; GPU 0 carries the desktop.

## The matrix

| shape | emitter (knob ON) | general (knob OFF) | correctness |
|---|---|---|---|
| `softmax_rows` 4×256 f32 | cooperative softmax (PTX + SPIR-V synthesize) | flat per-thread 3-pass | zero-seed → every output = 1/256 EXACTLY (f32-exact: exp(0)=1, sum of 256 ones, ÷256) |
| `dot_row` (NEW, declared `dot!` 20×128 f32) | cooperative dot | flat per-thread dot | all-ones → o = 128 EXACTLY (20 outputs) |
| attn_decode / gc_* (sanity) | knob-INDEPENDENT (decompose → tiled tier) | same | blobs identical across the knob |

`dot_row.abv` is new: today NO fixture routes the cooperative DOT emitter
(the chain fixtures decompose → tiled tier; the kwab pair nests →
general). `dot!` was declared without a fixture — this supplies it.

## Protocol

1. Correctness BEFORE timing, every cell, both lanes (the
   declared-gate splice mechanics: seed + in-process verify).
2. Timing: interleaved ON/OFF rounds ×3, both lanes, p10/p50/p90.
3. Verdict per emitter:
   - general ≥ emitter → **RETIRE** (delete the path + the knob row;
     the knob's parse stays for one release with a deprecation note)
   - emitter wins → **KEEP-AS-LOAN**; the measured gap = the M3 scope
     ledger (general machinery must learn the row forms)
   - tie (within band) → **RETIRE** (less code, same numbers)

## Deliverables

`benchmarks/results/2026-10-03-emitter-retirement-ab.md` (the tables +
verdicts), deletions or the M3 scope doc, INDEX update.

## Non-goals

- M3 implementation (scoped FROM this campaign's data, not before).
- The tensor-tier emitters (production, not loans).
