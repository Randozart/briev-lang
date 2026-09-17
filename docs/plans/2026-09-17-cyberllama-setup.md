# CyberLlama — Setup & Briev Flash-Attention Integration

**Date:** 2026-09-17
**Status:** Active
**Source:** `/home/randozart/Desktop/Projects/VITRIOL/llama.cpp` (clean, `main`,
`ab61c2a01`; origin = `Randozart/llama.cpp`, upstream = `ggml-org/llama.cpp`,
turbo-tan = TQ3 quant source)
**Target:** `/home/randozart/Desktop/Projects/cyberllama`
**Depends on:** warp-primitive intrinsics (`d9501df3`, `ad6fbd56`) — see
`docs/2026-09-17-session-report.md`

---

## What CyberLlama is

CyberLlama = the VITRIOL llama.cpp fork as a standalone project, plus Briev
flash-attention kernels on sm_86. VITRIOL's MoE streaming stack (expert LRU
cache, predictive prefetch, copy-engine DMA, TurboQuant) is kept untouched —
the Briev integration touches ONLY the attention dispatch.

## Milestones

### M0 — Extraction (this session)

1. `git clone` the local repo (NOT `cp -r` — 16 GB is build artifacts +
   models; clone carries source only, `build/` stays ignored).
2. Remotes: `origin` → `https://github.com/Randozart/llama.cpp.git`
   (pushes go to the user's fork), `upstream` → ggml-org, `turbo-tan` kept.
   Clone sets origin to the local path — fix before first push.
3. Record base commit `ab61c2a01` in the repo (branch `briev-attn` off `main`
   so `main` stays a clean mirror of the fork).
4. Verify: `git log -1` matches, CMakeLists present, source count sane.

### M1 — Baseline build — DONE (2026-09-17)

**Hardware amendment (2026-09-17):** the machine now carries **2× RTX 3060
12GB** (the GTX 1070 Ti is gone — CachyOS reinstall). Consequences, applied:

- Build config: `-DCMAKE_CUDA_ARCHITECTURES=86` only (was `61;86`).
- CUDA toolkit: 13.4 at `/opt/cuda` (12.9 was kept only for Pascal).
- Driver: **580.178.04 pinned — no upgrade mid-milestone.** The Briev GPU
  stack carries driver-pinned workarounds (Vulkan image barriers,
  `cp.async` visibility) validated on exactly this version. Upgrade later,
  as its own controlled step.
- sm_61 removed from the test matrix; stock CUDA fallback arms stay in
  source for other hardware.
- Two symmetric cards → `--split-mode row` (tensor parallel) viable for M5+;
  24 GB aggregate VRAM. VITRIOL's LULL/MoE streaming handles both devices.

Configure (working incantation):
```bash
cmake -B build -DGGML_CUDA=ON -DGGML_CUDA_FA=ON \
    -DCMAKE_CUDA_ARCHITECTURES=86 -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_CUDA_COMPILER=/opt/cuda/bin/nvcc \
    -DCMAKE_CUDA_HOST_COMPILER=/usr/bin/g++-14
cmake --build build --config Release -j8
```

Smoke result (`llama-bench`, mellum 12B.A2.5B Q2_K, ngl 99, single GPU):
`pp128 = 1193 ± 87 t/s`, `tg32 = 155.8 ± 2.1 t/s`; both CUDA devices
enumerated (23.8 GB total); VITRIOL stats block intact. Baseline binary at
`build/bin/llama-bench`, build `ab61c2a01 (1737)`.

### M2 — Briev attention library skeleton

- `attn/flash_attn.abv`: export `briev_flash_attn_f16` — VEC (decode,
  N=1) and MMA (prefill) variants, head dims {64, 128} first.
- `brievc build --library` → `libbriev_attn.a` + C header.
- ABI (from the researched `ggml_flash_attn_ext` contract):
  Q F32 `[D,N,H,B]` · K/V F16 `[D,Nkv,Hkv,B]` · F16 mask (−inf = masked,
  `FATTN_KQ_STRIDE=256` padding) · out F32 `[Dv,H,N,B]` · scale,
  ALiBi (m0/m1/n_head_log2), logit_softcap params · GQA = H/Hkv.
- Softmax = online (running max/sum) using the new warp intrinsics;
  FMA accumulate via `Fma#`.

### M3 — Correctness harness (before ANY ggml integration)

Standalone C driver: random tensors → Briev kernel vs a scalar CPU reference
→ max-rel error bound. Shapes: D∈{64,128}, GQA∈{1,4,8}, Nkv∈{256,1024},
causal + padded-mask cases. Gate: error < 1e-2 (f16 KV, f32 accum), zero NaN.
Also the PTX/SPIR-V cross-check (same kernel, both backends, same result).

### M4 — ggml-cuda dispatch integration

New arm in `ggml_cuda_get_best_fattn_kernel` (fattn.cu): sm_86 + F16 K/V +
supported head dim → `BEST_FATTN_KERNEL_BRIEV` → launch the Briev kernel via
the linked library. Everything else (sm_61 Pascal, quantized KV, MLA dims
192/320/576, sparse n_kv_max) falls through to the stock CUDA kernels —
additive only, no existing path modified.

### M5 — Honest A/B

Interleaved best-of-N, single process, vs the M1 binary on identical prompts:
tokenlatency decode (N=1) and prefill (N=512/1024) at D=64/128, GQA realistic
(6-8). Record in benchmarks ledger. The exit criterion is the same as every
GPU experiment: no win = no integration kept.

## Non-goals / constraints

- ~~sm_61 (GTX 1070 Ti) keeps stock CUDA kernels~~ **moot (2026-09-17):**
  hardware is now 2× 3060; stock kernels remain in source for portability.
- Quantized KV (Q4_0/Q8_0/TQ3) NOT in scope for M2-M5 — F16 KV cache path
  only. Quantized dequant-in-kernel is a later milestone if F16 wins hold.
- VITRIOL's MoE/TQ3/MTP machinery: untouched.
- ggml op-graph ABI is frozen by ggml — the Briev side conforms to IT,
  never the reverse.

## Risk register

| Risk | Mitigation |
|---|---|
| ABI drift between ggml versions | Pin to base commit; rebase discipline via `main` mirror |
| redux.sync bit-exactness vs SPIR-V tree | Irrelevant across kernels; cross-checked in M3 anyway |
| cp.async/mma sync bugs corrupt silently | M3 harness with padded/misaligned masks before perf work |
| Library state (`__briev_init_state`) vs CUDA context | Kernel entry needs no Briev state — pure pointer ABI; verify in M2 |
