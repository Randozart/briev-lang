# HANDOFF — GPU backend (.abv / SPIR-V): state, doctrine, next steps

> **2026-09-04 STATUS UPDATE** (read first; everything below is history):
>
> - **STAGE 0 CLOSED — PORTABLE PATH WINS:** the fold-proof mma ceiling
>   microkernel measured the KHR coopmat lowering at ≥107 TFLOP/s ≈
>   100% of the F16-acc hardware peak (102 TF) — the production GEMM's
>   gap is pipeline overhead, NOT vendor capping. Stage 2 (PTX tier)
>   demoted to an optional escape hatch, re-arms only on a driver-era
>   ceiling regression (doctrine §6). Ledger row 2026-09-04c/d.
> - **STAGE 1 RUNGS (all behind in-process A/B):** **D3 paired smem
>   fill (v2f16 member view) LANDED — 16.7→20.7 TFLOP/s (+24%), default
>   on** (2026-09-04d). **D1 2-panels/stage REJECTED** — 2× slower,
>   occupancy-bound (16 live fragments + 16 accs); knob 1 (2026-09-04d).
>   **D3b f16×4 fill REJECTED** — both forms (2×-unroll 1.183×, true
>   v4f16 view 1.133×); after D3 the fill is transaction-bound, not
>   instruction-bound (2026-09-04f). **D2 register-prefetch refill
>   REJECTED** — the fused post-barrier refill wins by 2–6% on clean
>   rounds; the 32-reg carry across the mma + issue-slot contention
>   beat the latency hiding (2026-09-04g). Machinery kept behind knobs
>   (all default off/1); the pair view is width-parameterized.
> - **REMAINING STAGE 1:** D4 occupancy shaping (LocalSize × subgroups
>   — note: subgroups>1 with the smem path needs a per-subgroup smem
>   design check before any sweep). Race target unchanged: ggml-cuda
>   42.0 TFLOP/s; ours 20.7 best window at the anchor shape.
> - **DEBUG TRAPS (this era):** `field_int(key, N)`'s N = the FIELD
>   INDEX, not a default (index 1 → None → the config default —
>   'reverted' binaries kept emitting pps=2). The D1 refill's panel
>   index must be RUNTIME ((kt_pair+2)·pps+pi — stage s re-reads at
>   pair-iteration kt+2). NaN rel-err compares false → a NaN-output
>   kernel prints '0.000e+00 OK'; gemm_h_bench now dumps y[0] vs ref vs
>   the f16 grid at the correctness gate.
> - **DOCTRINE UPGRADE (user-locked):** `.abv` = one program, peak on
>   every probed device, Briev-owned codegen only — no nvcc, no cuBLAS
>   in the compiler path. Backend tier architecture: portable SPIR-V
>   tier + per-vendor projections of ONE frontend plan (PTX tier
>   planned). Doctrine: `docs/architecture/abv-gpu-doctrine.md`.
> - **TENSOR TIER CORRECT + VALIDATED:** smem double-buffer staging is
>   the default coopmat form (root cause of the era's all-zeros: the
>   fill's flat index double-strided, commit 51b791cd). R sweep: R=4
>   optimal. Shape sweep: 2048³ 19.0 TFLOP/s, 8192³ smem +23% vs
>   direct. Ledger rows 2026-09-04 / 2026-09-04b.
> - **MEASUREMENT DISCIPLINE:** in-process A/B harness (two SPVs,
>   alternating-order batched submissions, per-kernel dispatch counts)
>   — self-A/B ratio 0.985-1.001. Pre-harness cross-run comparisons
>   were DVFS-dominated; within-run ratios are the signal.
> - **CAMPAIGN:** `docs/plans/2026-09-04-beyond-coopmat.md` — Stage 0
>   ceiling microkernel (decisive: is the KHR lowering double-pumping?)
>   → Stage 1 portable extraction (deeper pipeline, register-prefetch,
>   f16x2 fills, occupancy) → Stage 2 Briev PTX tier (gated on Stage 0).
>   Race target unchanged: ggml-cuda 42.0 TFLOP/s; ours 16.5 best
>   window at the anchor shape.

> **2026-09-02 STATUS UPDATE** (read this first; sections below are the
> 2026-08-31 snapshot, kept for history):
>
> - M1 GEMV + O1/O2/O3 + `brievc run x.abv` + the resident gate: all
>   LANDED (ledger rows in `2026-08-31-vitriol-gemm-comparison.md`).
> - **Float16 pipeline correct end-to-end on device** (RTX 3060): naive
>   tier 4096³ rel 2.4e-4; the y-fill fault is dead. Root causes fixed:
>   f16-as-storage-format lowering (f32 Function storage + OpFConvert
>   boundaries) and the runtime's unconditional feature requests (now
>   features2-probed, apiVersion-1.2 instance, tensor-device preference).
> - **DOCTRINE**: `docs/architecture/benchmark-strategy.md` §
>   Anti-Overfit Doctrine — shape tiers only, general path as the
>   correctness reference, general-syntax performance parity, VERDICT
>   discipline, watch the small/irregular shapes.
> - **NEXT (start here)**: `docs/plans/2026-09-02-tensor-tier-run.md` —
>   the coopmat tensor tier run (spirv_coopmat=1, validate against the
>   naive reference, race the 12.6 TFLOP/s ggml anchor), then the
>   256³ small-shape generality quirk.
>
> **2026-09-02 PORTFOLIO + FALLBACK SESSION** (plan:
> `docs/plans/2026-09-02-gpu-portfolio-and-fallback.md`):
> - **Portfolio broadened** (statuses in `benchmark-strategy.md` § GPU
>   Benchmark Portfolio): `reduce.abv` (0.771ms / 69 GB/s — latency-bound
>   serial FADD chains; subgroup-coop accumulation is the fix, NOT a
>   shared-memory tree — see O6 note in the plan), `gather_8.abv`
>   (69.5 GB/s random-read; stencil deferred — SPIR-V rejects guarded
>   bodies), `softmax.abv` + the `Exp#` intrinsic (278.9 GB/s).
> - **saxpy store mystery RESOLVED**: the bench passed `n_fields=3` for
>   4 fields — z was invisible to the runtime (BUGS.md). Harnesses MUST
>   mirror the generated runner's field count AND offsets.
> - **`--accel-cpu-fallback N` shipped** (.bv lane only; .abv stays
>   GPU-only by doctrine): shapes below the work-item threshold fall to
>   the CPU loop — const shapes fold at compile time, runtime shapes
>   icmp-gate. Two latent `.bv`-binary emission bugs fixed en route
>   (traps 11-12 below); the descriptor/probe path is now opt-verified.


**Date:** 2026-08-31 (end of session)
**Branch:** main. Session commits: `5a52f961`, `961ca293`, `e8bfc789`,
`85531646`, `94f9e3dd`, `863dcbc5`, `0dcdeaa1`.
**Read together with:** `docs/plans/2026-08-31-abv-gpu-by-default.md` (route
+ fixes log) and `docs/plans/2026-08-31-vitriol-gemm-comparison.md` (the
benchmark target + optimization ladder + ledger).

---

## 1. The doctrine (locked by the user — do not revisit)

- **`.abv` IS the GPU language.** Pure GPU code: every eligible counted loop
  is a kernel, the extension carries the accel intent (no `!> accel:`
  metadata, no per-node keywords), compile emits kernels + a runnable
  program. No CPU fallback lane in the artifact.
- **`.bv` IS the CPU language** with *annotated, probe-verified* GPU
  offload (`!> accel:` + optional per-node `accel`). The GPU lane runs only
  when the auto-tuning probe proves it beats the vectorized CPU fold.
- **The SPIR-V backend gets the same optimization commitment as LLVM.**
  Frontend-driven (analysis computes, backend consumes), config-tuned,
  measured before built. A losing optimization is a VERDICT entry, not a
  silent revert (VITRIOL evidence rules).

## 2. Architecture map (what exists, where)

Two GPU paths share the runtime; do not conflate them:

| path | frontend | kernels | execution |
|------|----------|---------|-----------|
| **`.abv` standalone** (pure GPU) | accel policy auto-injected (`apply_abv_accel_default`, compile.rs) | `src/backend/spirv/` — one .spv PER KERNEL, entry "main" (`runner::build_kernels`) | generated `x_runner.c` — reactive scheduler, resident launches (`runner::emit_runner`) |
| **`.bv` offload** (accel-annotated) | `AnalysisResults.accel` (src/analysis/accel.rs) | `collect_accel_kernels` → same spirv backend (`llvm/kernel.rs`) | dispatch wrapper `@txn_<name>` → `briev_accel_rt` → driver (Vulkan/OpenCL/CPU) |

Key files:
- `src/backend/spirv/{mod,kernel,lower,builder,runner}.rs` — the kernel
  backend (Vulkan-native SPIR-V via rspirv; spirv-val vulkan1.3 clean)
- `src/backend/llvm/{kernel,dispatch}.rs` + `emit_toplevel.rs` — offload
  wrappers, descriptor tables, blob embedding
- `src/analysis/accel.rs` — the eligibility proof (foreach loops now
  accepted; see below)
- `lib/runtime/briev_accel_rt.c` (+ `briev_dev_vulkan.c`, `briev_dev_opencl.c`)
  — device-agnostic dispatch, persistent buffers, device residency,
  probe. `BRIEV_ACCEL_VERBOSE=1` prints init/launch diagnostics.
- `src/compile.rs` — `.abv` policy injection, kernel-scoped capability gate,
  runner emission, runtime copying
- `src/backend/llvm/loop_engine/ssa.rs` + `dispatch.rs` — accel nodes route
  through `@txn_<name>` wrappers (do not inline them)

## 3. What works (verified on the RTX 3060, NVIDIA 580.178.04)

- Annotation-free `.abv` → spirv-val(vulkan1.3)-passing kernels: int,
  float, casts (S/UConvert, FConvert, CToF/FToS), unary ops, const
  materialization, structured `foreach` loops, bounds guard
  (`gid < N` from the `[i < N]` bound), phase-gated kernels
  (`[phase == 1 && i < nb]` conjunctions)
- `examples/gpu/{mini,scale,fmadd,gemv,gemv_small}.abv` — runnable via the
  generated runners
- `.bv` offload: full dispatch chain closed; nbody_newton_accel bit-exact
  vs the C reference through real Vulkan launches
- Device residency: arrays stay on GPU across launches, scalars sync
  host→device per step (the host phase machine owns them);
  `briev_accel_download` pulls the projection at the end
- Persistent staging buffers per kernel: ~5× launch cost cut

## 4. Measured reality (do not re-litigate without new evidence)

- O(N)-per-step workloads (nbody accel shape): **launch/PCIe-bound**. C's
  in-cache AVX-512 loop wins even against resident launches (C 14ms vs
  resident GPU 64ms at nb=4096/bound=1000). The probe correctly commits
  CPU. This is the design working.
- The GPU-favorable territory is **compute-dense launches with device
  residency**: the O(N²) flattened all-pairs shape is PROVEN expressible
  and correct (`benchmarks/gpu/pairs.bv`, `pairs.abv`); 16M interactions
  compute on device.
- Constraint discovered: 64-bit int div/rem is NOT a shader op — row/col
  splits use power-of-2 shift/mask until magic-number division lowering
  exists (tracked).
- First attempts at every stage failed for boring ABI/layout reasons, all
  fixed and documented: llc filetype, blob embedding, SSBO member widths,
  driver Vulkan struct ABIs (sType, 6-arg vkMapMemory, compute stage enum
  6 not 0x20, bind point 1 not 0x4000, command-buffer reset, memory-type
  search, features enabled).

## 5. NEXT (priority order — start here in a new session)

> **2026-09-08 update:** items 1–6 below are ALL DONE as of 2026-09-04.
> M1 ledger + harness (gemv_bench.c), O2 FMA (KEEP, ledger row), O3 float4
> (VERDICT: below 20% threshold — KEPT as infra, GEMV is DRAM-bound, split-K
> is the lever), resident policy (`analyze_resident_safety` at
> `src/analysis/accel.rs:1482`, gated in LLVM backend), multi-const runner
> fix (test `multi_const_bounds_resolve_each_const_distinctly` pins it),
> `brievc run` (native in-process runner at `compile.rs:1510`). The GEMM
> campaign closed at 0.708ms = 24.3 TFLOP/s (95% of RTX 3060 FP16-acc
> tensor peak) — see `docs/plans/2026-09-08-gemm-occupancy-campaign.md`.
> Remaining: the beyond-coopmat Stage 1 pipeline work (D1–D4) is the live
> lever; Stage 2 PTX tier is DEMOTED to optional (portable path reaches HW
> tensor peak — ledger 2026-09-04c).

1. **M1 benchmark harness + first ledger number.** GEMV M=K=4096: warm-up
   separated from steady-state, GPU vs single-thread CPU vs llama.cpp
   GEMV on the same box. Write into the ledger in
   `docs/plans/2026-08-31-vitriol-gemm-comparison.md`. A reusable bench
   harness (C, reuses briev_accel_rt) beats ad-hoc runners.
2. **O3: float4 vectorized loads** (`OpTypeVector float×4`): the big GEMV
   lever (memory-bound kernel). Needs an alignment proof — the SSBO
   layout offsets are known; require `offset % 16 == 0` and trip counts
   divisible by 4, else stay scalar. Measure against M1's number.
3. **O2: verify FMA fusion** (cheap): check the NVIDIA driver fuses
   FMul+FAdd in the loop; if not, emit GLSL.std.450 Fma via an
   ExtInstImport (the import + instruction plumbing is the work).
4. **Resident-launch policy in wrapper emission** (`.bv` offload): the
   runtime ABI (`briev_accel_launch_resident`/`download`) exists; the
   emitted wrapper must choose it for step-looped nodes. Needs the
   analysis gate: all readers of a resident array are kernels.
   UNSOUND otherwise (host state goes stale) — the gate is the work.
5. **Runner multi-const count fix** (small, known): the fast-forward
   `S_i = N` resolves a const-in-expression bound incorrectly when two
   consts are in play (N2 resolved as NB). Repro: `benchmarks/gpu/pairs.bv`
   runner prints `i = 4096` instead of 16777216.
6. **`brievc run x.abv`** as a native subcommand (currently a 6-line shell
   wrapper: build → cc runner → exec; see the pattern in the session log).

## 6. O2–O6 ladder (after 1–3)

| rung | what | GPU effect |
|------|------|-----------|
| O2 ✓next | FMA fusion check | math throughput |
| O3 ✓next | float4 loads | memory throughput |
| O4 | shared-memory tiling (GEMM) | DRAM traffic — the big GEMM lever |
| O5 | occupancy shaping (LocalSize search) | latency hiding |
| O6 | tree reductions in shared memory | reduction epilogue |

LocalSize is currently hardcoded 64×1×1 (`VK_LOCAL_SIZE_X` in the driver,
`LOCAL_SIZE_X` in spirv/kernel.rs — keep them equal).

## 7. Traps that cost this session time (all fixed — do not regress)

1. llc spirv64 needs `-filetype=obj` (LLVM 22 emits assembly text by
   default) and hex double-bit float literals (decimal floats rejected).
2. Embedded blob constants: single `c"…"` token, no string juxtaposition.
3. The SPIR-V normalizer must NOT strip universe properties (bits/
   protocol-category keys feed the eligibility proof and resolve_spirv_shape).
4. Top-level `let` parses as `Statement(Let)`, not `StateDecl` — both forms
   must be handled (collect_state_fields does; keep it that way).
5. Dispatch wrappers require `accel_kernel_idx` registered BEFORE host
   emission; collect blobs AFTER, with stable indices (empty blob = CPU
   fallback slot).
6. Descriptor field order = kernel SSBO member order = name-sorted
   collect_state_fields (field_index_map has extra internal fields — never
   use it for the projection).
7. Blob pointer in the descriptor is a full `ptr` (the old `i32 ptrtoint`
   broke PIE and misaligned the C struct).
8. Driver: every VkStruct field must match the real ABI (verify against
   /usr/include/vulkan/vulkan_core.h); enable device features (NULL =
   all-off → Int64/Float64 pipelines fail silently); reset command buffers;
   prefer HOST_VISIBLE|HOST_COHERENT memory; scalar sync is
   host→device-only in resident mode (device→host would clobber the
   fast-forwarded counter).
9. The FFI cache (`~/.cache/briev-compiler/ffi/*.o`) is content-hashed but
   STALE BINARIES linger: when runtime C changes, clear the cache AND
   rebuild, or you debug ghost behavior.
10. rg `-rn` is "replace with n" — do not use it for searching (cost real
    time twice).
11. **LLVM IR float literals must carry a decimal point.** Rust's `{:e}`
    prints `1e-4` — LLVM parses that as an integer token and clang/opt
    reject the module. Always `{:?}` (round-trip shortest, always `.`).
    Found when `--accel-cpu-fallback` first forced the probe path through
    opt-verification.
12. **`%briev.field` constants must match the C struct order exactly**
    (name, kind, host_offset, elem_bytes, count, is_write, proj_offset).
    The old emitter slotted proj_offset at index 3 — misaligned runtime
    reads AND an opt type error. The generated runner's field tables
    (.abv path) were always correct; only the LLVM-side constant emitter
    was wrong. The `.bv` binary descriptor path was never clang-verified
    before — consider an opt-verify smoke test for .bv accel changes.
13. **C bench harnesses must mirror the GENERATED RUNNER's field table —
    count AND offsets AND n_fields.** The saxpy bench carried 4 BrievField
    entries with n_fields=3: the runtime silently ignored z, which looked
    exactly like a store-visibility driver bug. The runner is the single
    layout authority.

## 8. Session-start checklist

```bash
cargo test --lib && cargo test --bin brievc   # both green at handoff
spirv-val --target-env vulkan1.3 <k.spv>       # every kernel path
# end-to-end .abv:
env BOUND=1 ./target/release/brievc build examples/gpu/gemv_small.abv \
    --out examples/gpu/run_abv
cc -O2 examples/gpu/run_abv/gemv_small_runner.c \
    -o examples/gpu/run_abv/gemv_small -lm -ldl
timeout 30 examples/gpu/run_abv/gemv_small     # prints i = 64
# end-to-end .bv offload:
env BOUND=20 BODYCOUNT=1024 ./target/release/brievc build \
    benchmarks/gpu/nbody_force.bv --out benchmarks/gpu --optimize-budget 2048
env BOUND=20 BODYCOUNT=1024 BRIEV_ACCEL_DEVICE=vulkan \
    BRIEV_ACCEL_VERBOSE=1 ./benchmarks/gpu/nbody_force   # 0.499899864
```

Pre-existing diagnostics baseline (not yours to fix opportunistically
unless touched): `briev_dev_vulkan.c` clangd noise (single-TU include
pattern); 2 `brievc` build warnings (unused variable in main.rs, etc.).
