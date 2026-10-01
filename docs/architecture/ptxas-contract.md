# The ptxas contract — what the assembler decides, and what it never will

**2026-10-01.** Every claim here is grounded in this tree: `file:line`,
measured results, or recorded incidents. Origin: an external claim audit
("targeting PTX to beat CUDA") showed the ptxas mental model is folklore
even among practitioners — this doc pins ours once, so no agent touching
the PTX lane rediscovers it the slow way. Companions:
`gpu-backend-strategy.md` (emitter-route evaluation — why the GPU lane
hand-emits PTX instead of going through LLVM NVPTX),
`abv-gpu-doctrine.md` (codegen ownership), `briev-vs-cuda-thesis.md`
(the structural argument).

## 1. What ptxas is

An **assembler + scheduler + register allocator**. It maps our virtual
registers to physical ones (the `declare` row's `%rd<16>` banks exist so
ptxas has something to allocate), schedules instructions within its
freedom, and emits SASS. It is **not** an optimizer of your algorithm:

- It **never** fixes tiling, coalescing, or fusion. Bad access patterns
  compile to fast-running bad access patterns.
- Loop unrolling decisions mostly happen at the emitter's level (or
  nvcc's, upstream) — the folklore "ptxas aggressively unrolls
  everything" is wrong, and matters: it tempts people to under-specify
  PTX and hope the black box saves them. It will not.

## 2. What survives PTX (your authority)

`mma.sync` shapes and `ldmatrix`/`stmatrix` operand layouts; `shfl.sync`
patterns; shared-memory addressing — **including bank-conflict padding
and swizzles** (the addressing is explicit in the PTX text); global
access patterns; predicates and guard structure; register-pressure
signals via `.maxnreg`. This is why the lane can reach 27.5 TF at
4096³: the data movement is decided in the text we emit.

## 3. What is ptxas's last word

Fine-grained scheduling and pipelining around your dependencies,
scoreboard choices, register-bank assignment, some instruction-pairing
details. Two consequences:

1. **Inspect SASS and timing, never PTX text, when judging emission
   quality** — this repo's analogue of the LTO lesson
   (AGENTS.md, Performance Recovery Protocol §6): the artifact that runs
   is the cubin, and the pipeline in between is closed.
2. **Identical PTX performs differently across ptxas versions.** First
   hand: `-maxrregcount=108` cubins passed ptxas 12.8 and 13.3 and
   **faulted IMA at runtime on both** (`ptx/mod.rs:1354` — "never ship
   capped-register cubins, verify with ptxas -v").

## 4. Control surface (what we actually hold)

- **`maxnreg`** — the one flag we pass (`compile_cubin`,
  `ptx/mod.rs:1501`). And it matters: occupancy shapes are measured
  facts, not preferences — (2,4)@256T beats (4,4)@512T at 4096³,
  18.4 vs 16.2 TF (`ptx/mod.rs:1350`).
- **Offline ptxas, cubin shipping, JIT bypassed.** The driver JIT
  ignores `CU_JIT_MAX_REGISTERS` (166 regs vs the requested 128 →
  1 CTA/SM, −27%), rejects `.maxnreg` directive text, and its internal
  state wedges after fault storms (`ptx/mod.rs:1480`). Offline ptxas is
  not a nicety — it is the difference between controlling occupancy and
  not. JIT-from-text remains only as the fallback path.
- **Version pinning** — `compile_cubin` probes `TRITON_PTXAS` before
  PATH and the installed triton backends, because Triton ships its own
  ptxas for exactly this reason: pin the black box you measured against.
- **Reproducible invocations** — unique workdirs per compile call
  (2026-10-01, BUGS.md): concurrent ptxas runs previously raced on
  shared input paths (spurious failures → silent text fallback).

## 5. Drift above ptxas: the driver

The closed stack shifts under you at the driver level too: the
615.71.09 driver regressed the Vulkan lane (shared-memory/barrier
codegen) by 2.5× — a recorded benchmark-visible regression from a
version bump, no code change. Conclusion for both layers: **gate
comparisons on recorded versions, and re-run the ledger after every
toolchain/driver bump.**

## 6. The honest ladder (recorded, 4096³ GEMM f32, this machine)

| Tier | TF | Note |
|---|---|---|
| Naive CUDA | ~1.9 | what "hand-written CUDA" usually means |
| **Briev PTX lane** | **27.5** | 14× over naive; small shapes (64³–256³) beat cuBLAS |
| cuBLAS | ~42 | the actual bar — 65% of it at large K is today's frontier |
| NVIDIA driver 615.71.09 Vulkan lane | 12.2 | driver-bound, not a lane property |

"Beat CUDA" honestly means: beat naive CUDA today, close the cuBLAS gap
at scale — the levers are the ones ptxas will never apply for you
(pipelines, TMA-class data movement, the S4-class work in
`gpu-backend-strategy.md`).

## 7. The NVPTX verdict (short form)

Full evaluation: `gpu-backend-strategy.md` (emitter routes). Short
form: LLVM NVPTX is a sensible default for scalar/control-flow kernels,
and backwards at the tensor frontier — it lags new ISA (`wgmma`,
`tcgen05`, TMA, clusters), and extracting `mma` + `ldmatrix` +
`cp.async` pipelines from it degrades to inline asm with LLVM's
scheduler still in charge. CUTLASS, FlashAttention, and Triton emit
PTX directly; this lane hand-emits and is a working counterexample to
"don't write raw PTX" — at 27.5 TF with device-validated parity
(`benchmarks/parity/run.sh`). The cost is real emitter work; the
payment is exactly the authority section 2 lists.
