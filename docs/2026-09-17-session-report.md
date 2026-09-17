# Session Report — 2026-09-17

**Focus:** GPU warp-primitive intrinsics (flash-attention groundwork) + the
VITRIOL → CyberLlama pivot.

**Commits:** `9e934117` (mid-band cost model), `d9501df3` (warp intrinsics
phase 1), `ad6fbd56` (phase 2 + PTX reductions).

---

## 1. Context and the pivot

The session opened on GPU benchmark parity work: the mid-band cost-model fix
(`9e934117`, 512³ +3.5%, cuBLAS ratio 1.30× → 1.26×), the 1024² attention
false-alarm resolution (`76f36889`), and the L3 split-K refutation
(`0aac40eb`). With the cost model honest, the question became *what is the
GEMM advantage for* — and the answer is VITRIOL, the user's llama.cpp fork
(`/home/randozart/Desktop/Projects/VITRIOL`) that runs modern LLMs on
VRAM-starved hardware (PCIe expert streaming, TurboQuant KV, MTP decoding).

Plan: extract the VITRIOL llama.cpp fork into a standalone project —
**CyberLlama** (`~/Desktop/Projects/cyberllama`) — and replace its flash
attention decode (VEC) and prefill (MMA) paths with Briev kernels on sm_86.

## 2. Capability audit — what flash attention needs vs what Briev had

Research across VITRIOL's `ggml/src/ggml-cuda/fattn*.c*` and the Briev
compiler found the GEMM tier solid (`mma.sync m16n8k16`, `ldmatrix`,
`cp.async` multi-stage) but the softmax side empty:

| Needed | Status before session |
|---|---|
| Warp shuffle | No PTX emission anywhere |
| Row-max / row-sum warp reduction | Only `SubgroupFAdd#` (SPIR-V only) |
| `Exp#` | **Broken**: missing from interpreter; LLVM fell through to a nonexistent C symbol `Exp` |
| Fused multiply-add | Absent |
| Ballot / broadcast | Absent |
| Softmax in any form | Absent from compiler AND stdlib |

The existing fused-attention PTX kernels compute `Q·Kᵀ·V` with **no softmax**
— the root cause of the 10× fused-path regression vs the two-GEMM composition.

## 3. The design discussion — what deserves to be an intrinsic

The user's constraint: *only add intrinsics that are GPU-unique hardware
primitives inexpressible in pure `.bv` code.* The resulting layering:

- **Intrinsics** (single hardware instructions, no `.bv` expression):
  `ShuffleDown#`, `ShuffleXor#`, `SubgroupFAdd#/FMax#/FMin#`,
  `SubgroupBallot#`, `SubgroupBroadcast#`, `Fma#`.
- **Language fix, not intrinsic**: `Exp#` made to work on every backend —
  the type-generic lowering was already right on SPIR-V; LLVM/interpreter
  just needed their (missing) arms.
- **Stdlib, not intrinsic** (future work): `RowMax#`/`RowSum#` (shuffle +
  smem trees), online softmax. Algorithms built ON the primitives.

CPU fallbacks are honest: shuffles/ballot/broadcast are identity
(single-lane), reductions return their argument, `Fma#` uses `f64::mul_add`.

## 4. Phase 1 — `d9501df3`

`ShuffleDown#`, `ShuffleXor#` (PTX `shfl.*.sync.b32`, SPIR-V
`OpGroupNonUniformShuffle*`), `SubgroupFMax#`/`SubgroupFMin#` (SPIR-V
`OpGroupNonUniformFMax/Min`), `Exp#` fixes (interpreter arm, LLVM
`is_float_unary` + `STANDARD_OPS`, webstack supported set), and — prerequisite
for all PTX intrinsics — `Expr::Call` dispatch in the general PTX emitter
(previously it rejected any call expression).

## 5. Phase 2 — `ad6fbd56`

`Fma#` (PTX `fma.rn.f32`, SPIR-V `GLSL.std.450 Fma`, LLVM `@llvm.fma.f32/.f64`,
interpreter `mul_add`), `SubgroupBallot#` (PTX `vote.sync.ballot.b32`; SPIR-V
`OpGroupNonUniformBallot` → `CompositeExtract` lane word 0 → `UConvert` to
Int), `SubgroupBroadcast#` (PTX `shfl.sync.idx.b32`; SPIR-V
`OpGroupNonUniformBroadcast`), and PTX warp reductions via **`redux.sync`**.

### Key findings

1. **`redux.sync` killed the shared-memory plan.** The phase-1 plan doc
   specified a smem + `bar.sync` tree reduction for PTX max/min. Research
   found `redux.sync.max.f32` (PTX ISA 7.0, sm_80): register-only, one
   instruction, all lanes receive the result. Every PTX emission in the
   backend already targets `.target sm_86`, so the sm_80 floor is safe
   platform-wide. Verified against the HEAD baseline with Praetor before
   acting (Rule 20 discipline applied to an emission claim).
2. **SPIR-V ballot returns `uvec4`** regardless of subgroup size — the kernel
   extracts component 0 (lanes 0–31) and widens. Subgroups wider than 32
   would lose bits 32+; documented at the emission site.
3. **PTX shuffle syntax**: the `shfl.*.sync.b32 d, s, sel, membermask` form
   (mask last) — the pre-6.0 legacy form differs; `idx` variant carries an
   explicit `0x1F` clamp width for absolute-lane broadcast.
4. **Praetor gate held by extraction, not suppression**: the new arms pushed
   both `emit_intrinsic_call`s over the complexity limit. Fixed by splitting
   the subgroup family into `emit_subgroup_intrinsic` (+ unified
   `emit_subgroup_reduce` — the FAdd/FMax/FMin bodies were textually
   identical modulo opcode, a DRY win) on SPIR-V, and
   `emit_lane_intrinsic`/`emit_warp_reduce` on PTX. A HEAD-baseline Praetor
   run on extracted copies confirmed which diagnostics were new vs pre-existing
   (interpreter `execute_intrinsic` 37/56 and PTX `emit_expr` cognitive 16
   are baseline).
5. **`Exp#` LLVM bug class**: an intrinsic absent from `is_float_unary`
   falls through to `emit_external_call`, emitting a call to a C symbol that
   does not exist — link failure only visible at binary link time. Any future
   float intrinsic must be added to the match on the day it is registered.

## 6. State

- Tests 2269/2269 green at both commits. Docs updated in-commit: plan doc,
  MASTER-SYNTAX-REFERENCE, intrinsics-vs-stdlib.
- Known follow-ups: stdlib `RowMax#`/`RowSum#` + online softmax to prove the
  primitives compose; PTX `SubgroupFAdd#` now routes through `redux.sync`
  (bit-exactness vs the SPIR-V fixed-tree ordering is unverified — only
  matters if a kernel mixes backends mid-reduction, which nothing does).

## 7. Next — CyberLlama

Extract `/home/randozart/Desktop/Projects/VITRIOL/llama.cpp` to
`~/Desktop/Projects/cyberllama` (directory lowercase, project CyberLlama),
then implement flash attention decode (VEC) + prefill (MMA) replacements as
`.abv` kernels on sm_86, linked through `brievc build --library` GLUE FFI.
Integration surface researched this session: `ggml_flash_attn_ext` ABI
(Q F32 `[D,N,H,B]`, K/V F16 `[D,Nkv,Hkv,B]`, F16 causal mask, F32 out
`[Dv,H,N,B]`, scale/ALiBi/softcap op-params, GQA via H/Hkv).
