# M3 Matrix Completion + M4 fattn Shim (CyberLlama A/B track)

**Date:** 2026-09-18
**Status:** Step 1 DONE (M3 matrix 18/18 PASS). Step 2 DONE — gate verdict
**NO WIN** (Briev chain 412.8 µs vs stock fattn 58.3–59.6 µs at bitnet
NKV=4096; `benchmarks/results/2026-09-18-m4-decode-microbench-gate.md`).
Step 3 NOT built (gate says stop). Step 4 moot. Future path: fused f16
single-node decode composition — frontend expressiveness work, its own
plan.
**Parent:** CyberLlama plan `37ab73093` (cyberllama repo, M0-M5) + before-ledger
`benchmarks/results/2026-09-17-cyberllama-before-ledger.md`
**Preceded by:** SPIR-V vec4 member-index fix (`ac1f9d2a`) — both M3 lanes PASS
at the original geometry (H=32, HKV=8, G=4, NKV=256).

## Goal

Get from "composition correct at one geometry" to the honest M5 A/B:
bitnet-2b tg32 @ p0/p1024/p4096, Briev attention vs stock fattn, driver
615.71.09, single GPU. Verdict rules live in the parent plan; bitnet p4096
is the headline row, p0 rows are the sanity gate.

## Geometry truth (from GGUF metadata, read 2026-09-18)

| Model | H | HKV | G | D | ctx | Role |
|---|---|---|---|---|---|---|
| bitnet-2b-tq1_0 | 20 | 5 | 4 | 128 | 4096 | headline visibility |
| mellum2-claude-Q2_K | 32 | 4 | 8 | 128 | 131072 | must-not-move sanity |
| qwen3.8-27B UD-IQ3_S | 24 | 4 | 6 | 128 | 262144 | showcase (later) |

The current `attention_decode.abv` + `m3_attention_harness.sh` bake
H=32/HKV=8/G=4/NKV=256 into consts, buffer decls, AND the injected CPU
reference. Everything must instantiate from (D, H, HKV, G, NKV).

## Steps

1. **M3-matrix harness parameterization** — `m3_attention_harness.sh` takes
   D/H/HKV/G (env or args) alongside NKV; buffer sizes computed, CPU
   reference generated from the same geometry. Gate: max-rel < 1e-3 on all
   three lanes' metrics, both CUDA and Vulkan, for the matrix
   {H=20,HKV=5}, {H=32,HKV=4}, {H=24,HKV=4} × NKV ∈ {256, 1024, 4096}.
   Cooperative-shape gate + is_cooperative fallbacks must survive H=20/24.
2. **M4 shim** — build the accel runner + `briev_accel_rt.c` +
   `briev_dev_cuda.c` into `libbriev_attn.a` with a pointer-ABI export:
   `briev_attn_decode(q, k, v, out, nkv, scale)` doing D2D staging into the
   projection buffer, the 3-kernel launch chain, and a download of a_out
   (per the .abv header's M4 comment). Microbench gate BEFORE ggml wiring:
   Briev end-to-end (incl. staging copies) vs ggml's own fattn decode time
   at bitnet geometry p4096. No win at kernel parity => stop, dissect,
   record honest negative; do not wire fattn.cu.
3. **M4 ggml dispatch** (only on a microbench win) — `BEST_FATTN_KERNEL_BRIEV`
   arm in fattn.cu, sm_86 + F16 KV + D=128 decode only, stock fallback
   elsewhere. KV append optimization (copy only the n_kv delta) is a
   follow-up refinement, not the first cut.
4. **M5 matrix** — llama-bench interleaved r>=3 on the before-ledger rows:
   bitnet tg32 p0/p1024/p4096 + pp512/pp4096; mellum tg32 p0 + pp512
   (must-not-move); append to before-ledger as the "after" section.

## Honesty constraints

- Briev composition is F32 KV; ggml fattn is F16 KV (2x KV bandwidth
  against us). A win despite that is real; a loss must note the f32/f16
  gap before any conclusion. No f16 Briev variant until the f32 verdict.
- Staging copies are part of the Briev number. Never exclude them.
- p0 sanity rows: unchanged within noise, or the integration is wrong.

## Files

- `benchmarks/m3_attention_harness.sh` — parameterize (step 1)
- `examples/gpu/attention_decode.abv` — instantiation source (step 1;
  template stays canonical, sizes derive from D/H/HKV/G/NKV)
- cyberllama `ggml/src/ggml-cuda/` — fattn arm (step 3, gated)
- `benchmarks/results/2026-09-17-cyberllama-before-ledger.md` — after
  section appended in step 4 (never rewritten)
