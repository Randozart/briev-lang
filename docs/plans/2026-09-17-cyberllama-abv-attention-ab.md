# CyberLlama .abv Attention — A/B Test Plan

**Date:** 2026-09-17
**Status:** Active
**Parent:** `docs/plans/2026-09-17-cyberllama-setup.md` (M0/M1 done)
**Baseline:** `benchmarks/results/2026-09-17-cyberllama-before-ledger.md`

---

## Execution order (amended 2026-09-17, post device-gate study)

The study of `src/backend/spirv/kernel.rs` + the M0 runner artifacts exposed a
hard architectural constraint that reorders M2a/M2b:

- **GGML integration must run on the CUDA path.** ggml's device allocations
  are CUDA buffers; a Vulkan-launched kernel cannot consume them. The
  exported `briev_flash_attn_f16` must therefore launch through
  `briev_dev_cuda` (`cuModuleLoadData` on PTX text), not the SPIR-V runner.
- The standalone runner embeds **SPIR-V only** (`k0` blob = SPIR-V magic) —
  the CUDA device driver ("S2: compiler emits PTX text") never receives a
  blob. `brievc build --backend gpu` — the PTX-tier build — currently
  **panics** on a defn-liveness error (`__stdout_flush` unreached).

Revised order (riskiest unknown first):
1. **M2.0 — CUDA path end-to-end**: fix the `--backend gpu` defn-liveness
   panic; make the standalone runner embed PTX for the CUDA driver (SPIR-V
   blob stays for Vulkan); `pairs.abv` runs and verifies on device through
   `cuModuleLoadData`. *This is the M4 foundation — without it there is no
   integration path at all.*
2. **M2a — softmax row kernel on the PTX tier**: cooperative row shape
   (work-item = row, lane = strided position — the dot-product precedent
   from plan 2026-09-01), three phases (max → exp-sum → normalize) with
   subgroup reductions between: PTX `redux.sync.max/add.f32` +
   `shfl`-style phase sync; SPIR-V twin via `emit_coop_reduce_store`
   machinery. Detected from explicit three-pass source (honest shape, no
   hidden treatment).
3. M2b / M3 / M4 / M5 as written above.

## Goal

Answer with numbers: **does replacing CyberLlama's flash-attention kernels
with Briev (.abv) kernels benefit token throughput?** Stock fork vs .abv
integration, same binary pair, same GPU state, driver 615.71.09 pinned.

## Test matrix — three models, three roles

| Model | Size | GPUs | Role | Rows |
|---|---|---|---|---|
| **bitnet-2b-tq1_0** | 1.1 GB | GPU 0 only | **attention-visibility primary** — dense 2B, GQA-4, head_dim 128, KV ≈ 50–60% of decode bandwidth at p4096 | tg32 @ p0/p1024/p4096, pp512/pp4096 |
| **mellum2-claude-Q2_K** | 4.7 GB | GPU 0 only | real-MoE sanity — measured context-flat (attention <3% of decode); must NOT move | tg32 @ p0, pp512 |
| **Qwen3.8-27B UD-IQ3_S** | 11.2 GB | dual-slot (layer split) | real-world showcase — the actual inference workload | tg32 @ p0/p4096, pp512 |

Qwen3.8 Q3_K_M (12.9 GB) as fallback if IQ3_S misbehaves. Single-GPU IQ3_S
probe (11.9 GB card) optional curiosity row — marginal fit.

Method: llama-bench, interleaved rounds (r≥3), `< /dev/null`, results
appended to the before-ledger file as the "after" section. p0 rows are the
sanity gate: **unchanged within noise or the integration is wrong.**

## Milestones

### M2a — softmax row-op kernel (the novel compiler piece)

Row-wise online softmax over F32 logits + F16 causal mask (−inf), expressed
in the general accel tier. Cross-lane row reductions via the warp intrinsics
(`ShuffleDown#`/`SubgroupFMax#` → PTX `shfl.*.sync`/`redux.sync`; SPIR-V
`OpGroupNonUniform*`). Precedent: the cooperative-row-kernel path
(`emit_coop_reduce_store`, plan 2026-09-01) already injects `SubgroupFAdd#`
into detected row-reduction loops — row-max/row-sum are the sibling shapes.

Deliverables:
- `attn/softmax_rows.abv` compiling through the accel partition
- Emitted PTX passes `ptxas -arch=sm_86`; SPIR-V passes `spirv-val`
- Interpreter/LLVM fallback correct (CPU reference for M3)

### M2b — attention composition + C export

`briev_flash_attn_f16` export (GLUE FFI, `brievc build --library`):
Q F32 `[D,N,H,B]` · K/V F16 `[D,Nkv,Hkv,B]` · F16 mask · out F32
`[Dv,H,N,B]` · scale/ALiBi/softcap · GQA = H/Hkv. Composition:
QKᵀ GEMM (tensor tier) → softmax_rows → PV GEMM. D=64/128, decode (N=1)
first. Host-stub mechanism (exported symbol → accel launch) researched in
`src/glue/` + `src/library.rs` during this milestone.

### M3 — correctness harness (also closes the coopmat device gate)

Standalone C driver: scalar CPU reference, random tensors, D∈{64,128} ×
GQA∈{1,4,8} × Nkv∈{256,1024,4096}, causal + padded masks. Gate: max-rel
< 1e-2 (f16 KV, f32 accum), zero NaN, PTX/SPIR-V cross-check. **First
coopmat-exercising on-device run since the driver swap** — doubles as the
615 device gate for the tensor tier.

### M4 — ggml-cuda dispatch integration

`briev-attn` branch in cyberllama: `BEST_FATTN_KERNEL_BRIEV` arm in
`ggml_cuda_get_best_fattn_kernel` (fattn.cu) → launch wrapper → exported
Briev function. Gate: sm_86 + F16 KV + D∈{64,128} only; everything else
(sm_61, quantized KV, MLA dims, sparse n_kv_max) falls through to stock.
Additive only. nvcc-compiled CPU-side.

### M5 — the A/B

Run the matrix against the before-ledger. Verdict rules:
- bitnet tg @ p4096 is the headline: any win must show here
- mellum p0 + bitnet p0 unchanged = integration sane
- No visible win in any attention row = the composition loses to GGML's
  fused MMA kernel; record and dissect (dispatch overhead? tile shape?
  softmax kernel latency?) — honest negative counts as a result

## Non-goals

- Quantized KV caches (TQ-KV, Q4_0/Q8_0 KV) — F16 KV path only
- MLA dims (192/320/576), sparse n_kv_max paths — stock kernels keep them
- Dual-GPU attention sharding — Qwen3.8 rows run the stock split; .abv
  kernels run per-device inside it
- VITRIOL MoE/TQ/MTP machinery — untouched

## Risks

| Risk | Mitigation |
|---|---|
| Accel partition can't express row-softmax | Cooperative-row precedent; worst case extend `accel.rs` shape detection (additive) |
| 3-launch composition overhead eats the win | Decode N=1: GGML VEC is also multi-kernel; measure per-launch cost in M3 |
| Coopmat regressions on 615 | M3 is the gate before any system A/B |
| BitNet TQ weights interaction | Weights stay on stock kernels; only src[3] attention node swaps |
