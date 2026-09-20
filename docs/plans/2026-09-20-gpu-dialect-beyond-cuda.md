# GPU Dialect Beyond CUDA/ggml

**Date:** 2026-09-20
**Status:** active
**Supersedes:** (none — new plan)
**Referenced by:** `gpu-backend-strategy.md`, `abv-gpu-doctrine.md`, `briev-vs-cuda-thesis.md`, `general-machinery.md`

---

## 1. The decision

We build a GPU dialect where **every optimization is derived from proof and
graph structure**, not from vocabulary recognition or hand-tuned heuristic
trees. The baseline is CUDA/ggml — we measure against cuBLAS 42 TFLOP/s
(F16, 4096³, RTX 3060) and ggml's best hand-tuned kernels.

**What we reject:**
- Vocabulary matching (softmax/attention/GEMM as named patterns — Rule 23)
- Calling cuBLAS or running nvcc in the codegen path
- Heuristic trees that carry invisible knowledge
- Undefined-behavior trust where contracts can prove

**What we build instead:**
- General-purpose passes (chain fusion, warp slicing, deferred normalizer,
  serial unroll) that handle every shape
- Proof-carrying optimization where every tiling, register cap, and fusion
  decision carries a checkable obligation
- A program DAG that lets the compiler discover independence, eliminate syncs,
  reuse buffers, and fuse chains — all from topology, never from names
- Hand-written reference kernels that prove the compiler's output is
  competitive

**The thesis:** CUDA's unit is the kernel a human wrote. Briev's unit is the
computation a compiler proves and schedules. This is structural, not
incremental.

---

## 2. The structural advantage — what Briev has that CUDA cannot

### 2.1 Program DAG (CUDA: no program DAG)

CUDA kernels are opaque blobs. Host code IS the schedule. CUDA cannot discover
that two kernels are independent — a human wrote the sequence.

Briev's `gpu_schedule` sees the whole computation:
- **Phase 2:** Independent nodes batch back-to-back (sync elimination via
  `xor_overlap` proof)
- **Phase 3:** Y-lifetime buffers reuse across nodes
- **Phase 4b:** Chain fusion from single-reader dead intermediates — no names,
  no pattern matching, just topology

**Concrete example:** The three-node attention chain (qk→softmax→pv) has a
single-reader intermediate (softmax reads qk output, pv reads softmax output).
The compiler proves non-aliasing from this, eliminates the two intermediate
DRAM roundtrips, and emits ONE kernel. CUDA's model has no mechanism to
discover this — a human must write the fused kernel or call a library.

### 2.2 Contracts as correctness (CUDA: undefined behavior)

CUDA relies on UB assumptions (poison, dereferenceable, argmem-only). When
wrong, it's a miscompile. The `#13` full-memory semantics fix was exactly
this: an LLVM assumption that, when wrong, produced wrong code.

Briev proves discipline:
- `[pre][post]` on every buffer — memory discipline is proven, not assumed
- Touched-field tables prove which fields a kernel reads/writes
- Last-use analysis proves when buffers can be reused
- Buffer-reuse opportunities are proofs, not heuristics

**Concrete advantage:** When the compiler proves buffer A and buffer B don't
alias, it can reorder loads and stores freely. CUDA's `__restrict__` is a
hint; Briev's contract is a proof.

### 2.3 Shape-driven synthesis (CUDA: fixed kernels)

A CUDA kernel is written for ONE geometry. Shape-adaptive strategy is a human
activity — rewrite the kernel, or call cuBLAS and trust its heuristic.

Briev's cost model (`src/analysis/gpu_strategy.rs`) picks tile/stages/load-
path from shape evidence, calibrated against a decompiled cuBLAS kernel map:

| Shape | Briev | cuBLAS | Verdict |
|-------|-------|--------|---------|
| 64³ | 0.16 TF | 0.05 TF | **3.2× win** |
| 128³ | 0.88 TF | 0.36 TF | **2.4× win** |
| 256³ | 4.34 TF | 3.34 TF | **1.3× win** |
| 1024³ | 20.0 TF | 19.6 TF | parity |
| 2048³ | 25.4 TF | 24.5 TF | parity |
| 4096³ | 23.4 TF | 26.2 TF | 0.89× (occupancy wall) |

The compiler doesn't "recognize GEMM" — it proves the tiling is valid for the
shape and emits code. Each shape gets optimal treatment automatically.

### 2.4 Reference-verified codegen (CUDA: no reference)

CUDA kernels are checked against whatever the programmer hand-wrote in host
code — another unverified artifact.

Briev has defined language semantics. The GPU kernel is one lowering of it,
verifiable against the CPU interpreter. `accel_probe` checks output equality
before committing dispatch. The flash-decode gate proved this: the hand-PTX
kernel (91 µs) was verified bit-accurate against the CPU reference before
timing.

### 2.5 One reactive model (CUDA: two ecosystems)

CUDA has no notion of a program that is both a GPU compute kernel and a bare-
metal interrupt service. Briev's reactor schedules GPU nodes and services
machine interrupts. The rv64 arc proved this: preemptive two-task micro-
kernel, machine-timer interrupts, bootstrap node — pure Briev, no C.

---

## 3. Benchmark targets — what we beat

### 3.1 GEMM (RTX 3060, sm_86)

| Shape | cuBLAS | Briev current | Briev target | Gap closer |
|-------|--------|---------------|--------------|------------|
| 4096³ | 42.0 TF | 23.4 TF | **42+ TF** | `cp.async` pipeline, occupancy tuning |
| 8192³ | ~38 TF | 30.2 TF | **40+ TF** | `cp.async`, `ldmatrix` |
| 2048³ | 24.5 TF | 25.4 TF | **27+ TF** | bank-conflict-free smem |

### 3.2 Attention decode (bitnet geometry, H=20, D=128, NKV=4096)

| Path | f32 | f16 | Target |
|------|-----|-----|--------|
| 3-kernel chain | 202 µs | — | retire (M3 fuses to 1) |
| Flash decode (hand-PTX) | 125 µs | 91 µs | **≤58 µs** (occupancy) |
| Deferred 2-pass | 198 µs | — | **≤125 µs** (M2+M3) |
| DRAM floor | 58 µs | 29 µs | — |

### 3.3 CPU benchmarks

Unblock the LLVM defn_liveness panic (`str_to_int` missing from
`intrinsic_helpers`). All `.bv` CPU benchmarks blocked by this pre-existing
issue.

---

## 4. Track 1 — General Machinery (M2→M3→M4)

*The generality play. Shapes emerge from proofs, not recognition.*

### M2 — Deferred Normalizer

**What:** Move division by a loop-completed scalar past the consumer's
accumulation. The consumer accumulates unnormalized terms; one division after.

**Proof obligation:** Linearity of consumer in the divided term + single-
writer `den`.

**Example:**
```
// Before: each p_j normalized individually
foreach j {
    p_j = exp(score_j - M) / L;   // division inside loop
    acc += p_j * v_j;
}

// After: accumulation deferred, one division after
foreach j {
    p_j_unnorm = exp(score_j - M); // no division
    acc += p_j_unnorm * v_j;
}
acc = acc / L;  // single division after loop
```

**Gate:** Correctness via M3; standalone test on mean-then-weighted-sum
fixture. a_err < 1e-3.

**Status:** DETECTION DONE (2026-09-20, `56bfbaa8`): `detect_deferred_normalizer`
in `src/analysis/accel.rs` proves the deferrable-normalize structure
(single-writer self-add denominator + trailing pure-division foreach);
`KernelShape.deferred_normalize` carries the proof; the PTX deferred-region
dispatch consumes it (frontend-driven). 8 tests; flash2p fixture
byte-identical when the knob is on (max_rel 1.95e-06, 200 µs); m3 PASS
both lanes. REMAINING: mean-then-weighted-sum runtime fixture; the
normalize-as-barrier removal for M3 fusion (the consumer-side absorption
of the raw numerator).

### M3 — Producer-Consumer Chain Fusion

**What:** Generalize `detect_chain_fusion` (Phase 4b) beyond
GEMM→elementwise→GEMM. The softmax node fuses with its dot producer (qk) and
linear consumer (pv) into ONE dispatch:
- sc_j computed in-loop (dot product)
- p_j weighted into consumer accumulators (p * v)
- normalization deferred (M2)
- j-sliced by warps (M1)
- LSE-merged across slices

**Proof obligations:**
- Single-reader dead intermediates prove non-aliasing
- `xor_overlap` proves independence of slice states
- LSE merge correctness (associativity + commutativity of weighted sum)

**Gate:** Three-node attention chain emits ONE kernel ≤ ~120 µs f32; launches
3 → 1; a_err < 1e-3 both lanes; m3 harness PASS.

**Status:** Not started. Depends on M2.

### M4 — numeric.bv Declarations + Vocabulary Retirement

**What:** `lib/std/numeric.bv`: softmax, dot, matmul declared as composites —
canonical bodies + registered lowering channel. The compiler lowers DECLARED
CALLS, not hand-expansions.

**Retirement order (each behind perf A/B gate):**

| # | Matcher | Lines | Retire when | Why this order |
|---|---------|-------|-------------|----------------|
| 1 | Fused-attention family | ~1400 | M3 general path ≤ best config | Largest dormancy; M3 directly replaces |
| 2 | `detect_row_softmax` | ~200 | M4 declared softmax handles it | M4 makes this a registered lowering |
| 3 | `detect_reduction` (Dot) | ~150 | M4 dot declaration exists | Simpler than GEMM; prerequisite |
| 4 | `GemmPlan` | ~500 | Full declaration framework from M4 | Most complex; affine-index migrates |

**Gate:** m3 templates via stdlib softmax; fused path ≤ M3's time; hand-PTX
gate kernel (125/91 µs) within noise of emerged path.

**Status:** Not started. Depends on M3.

---

## 5. Track 2 — GEMM Pipeline (S3b→S5)

*The TF ceiling play. Close 23→42 TF at 4096³.*

### S3b — Tensor GEMM with `cp.async` Pipeline

**What:** Multi-stage pipelined GEMM kernel using:
- `cp.async` for global→shared memory (async copies, decouple compute from
  memory)
- `ldmatrix` for shared→register fragment loads (4–8× fewer instructions)
- `mma.sync.aligned.m16n8k16` for compute (already proven exact in S3a)
- Bank-conflict-free smem layout (affine analysis → synthesized swizzle)
- `.maxnreg` from config knob (occupancy control)

**Pipeline structure:**
```
Stage 0: cp.async fill smem[0]
bar.sync
Stage 1: cp.async fill smem[1]; mma smem[0]
bar.sync
Stage 2: cp.async fill smem[2]; mma smem[1]
...
Epilogue: merge partial results, store
```

**Gate:** 4096³ ≥ 38 TFLOP/s (F16-acc); correctness across full shape
portfolio (512³, 1024³, 2048³, 4096³, 8192³).

**Status:** Not started. Builds on S3a (mma.sync proven).

### S4 — Correctness Gate

**What:** Whole-shape portfolio test. Every shape from 64³ to 8192³ must pass
correctness at 5e-3 (f32-acc) / 1e-2 (f16-acc).

**Gate:** All shapes PASS. Any failure blocks S5.

**Status:** Not started.

### S5 — Performance Gate

**What:** Close 29.3→42.0 TFLOP/s at 4096³. Specific levers:
- Occupancy tuning (3+ CTAs/SM vs current 2)
- `.v4.f32` vectorized 128-bit loads (from alignment contracts)
- Bank-conflict-free smem (affine access → swizzle synthesis)
- Register pressure analysis (proof-guided allocation)

**Gate:** 4096³ ≥ 42 TFLOP/s (matches cuBLAS anchor).

**Status:** Not started.

### S6 — Auto-tune Loop

**What:** `derive --stochastic` sweeps (tile × stages × warps) per device
profile. Winners cached in `config/targets.*`.

**Gate:** Per-device optimal found within 100 iterations.

**Status:** Not started.

---

## 6. Track 3 — Proof Infrastructure

*The long-term moat. Every optimization carries a proof certificate.*

### 6.1 Proof obligations for GPU codegen

| Optimization | Proof obligation | Status |
|-------------|-----------------|--------|
| Tiling validity | Tile boundaries fit in smem; no out-of-bounds | GemmPlan verifies |
| Register cap | Live values ≤ cap at every program point | `.maxnreg` from config |
| Coalescing | Affine coefficient of work-item counter is stride-1 | `coalescing.rs` G001 |
| Sync elimination | `xor_overlap` proves no read-write overlap | `concurrency_gate` |
| Chain fusion | Single-reader dead intermediate proves non-aliasing | Phase 4b |
| Deferred normalizer | Linearity + single-writer `den` | M2 (new) |
| Buffer reuse | Y-lifetime fits within node's execution window | gpu_schedule Phase 3 |
| Bank-conflict-free | Affine access map has no collisions | Synthesized swizzle |

### 6.2 Proof certificate format (future)

Every optimization pass emits a proof certificate:
```
optimization: cp.async_pipeline(stages=3, tile=64x64)
proof: smem_usage = 3 * 64 * 64 * 4 = 48 KB ≤ 48 KB (sm_86 limit)
        register_usage = 64 * 32 = 2048 ≤ 65536 (sm_86 total)
        occupancy = 2 CTAs/SM (valid: 48 KB smem / 48 KB = 1)
```

The certificate is checkable by a proof checker pass. If the proof fails,
the optimization is rejected and the fallback path is used.

### 6.3 Contract-guided vectorization

Contracts erase bounds checks. When the compiler proves `[pre] i < N` for a
loop body, the bounds check is unnecessary and the compiler can emit 128-bit
vector loads (`.v4.f32`). CUDA's `__restrict__` is a hint; Briev's contract
is a proof.

**Concrete path:**
1. Prove loop bounds from contracts
2. Prove alignment from layout contracts
3. Emit `.v4.f32` loads (4× bandwidth per instruction)

### 6.4 Bank-conflict-free smem synthesis

The compiler analyzes the affine access pattern of each shared memory
operation. If the pattern has bank conflicts, the compiler synthesizes a
swizzle (padding or XOR-based) that eliminates them.

**Concrete path:**
1. Extract affine access map from loop structure
2. Detect bank conflicts (stride % 32 == 0 or stride % 32 == 16)
3. Synthesize swizzle: `addr_swizzled = addr ^ (lane_id % 16)`
4. Apply swizzle to all shared memory accesses in the kernel

---

## 7. Track 4 — Hand-Written Reference Kernels

*Prove the compiler's output is competitive by writing kernels that beat ggml.*

### 7.1 Why hand-written kernels

The compiler's general machinery must produce output competitive with hand-
written code. But we need reference kernels to:
1. **Prove the target is achievable** — if we can't write it by hand, the
   compiler can't derive it
2. **Validate the compiler's output** — compare generated code against hand-
   written reference
3. **Identify the gap** — if the compiler is slower, understand why (register
   pressure, occupancy, pipeline efficiency)

### 7.2 Hand-written kernel targets

| Kernel | Geometry | ggml time | Target | Why this shape |
|--------|----------|-----------|--------|----------------|
| GEMM f16 | 4096³ | 3.27 ms (42 TF) | ≤3.2 ms | Prove we can match cuBLAS |
| Flash decode f16 | H=20 D=128 NKV=4096 | ~80 µs | ≤60 µs | Prove attention can beat ggml |
| Fused attention | H=20 D=128 NKV=4096 | ~73 µs (compose) | ≤60 µs | Prove fusion helps |
| GEMM f16 | 8192³ | ~10 ms | ≤9 ms | Prove scaling |

### 7.3 Hand-written kernel methodology

1. **Write the kernel** in hand-PTX (Briev-owned codegen path)
2. **Verify correctness** against the CPU reference (double-precision)
3. **Measure** against cuBLAS/ggml baseline (same hardware, same DVFS window)
4. **Document** the optimization decisions (why this tile, why this pipeline
   depth, why this register blocking)
5. **Feed insights** back to the compiler (the general machinery must learn to
   derive these decisions)

The hand-written kernels are NOT permanent — they are existence proofs. When
the general machinery produces output competitive with them, they retire.

### 7.4 The ggml beat condition

We beat ggml when:
1. **Same or better TFLOP/s** at the same shape
2. **Automatic** — the compiler derives the kernel from the `.abv` source,
   no hand-tuning
3. **General** — the same compiler pass handles GEMM, attention, and any
   future shape without vocabulary matching

The hand-written kernels prove condition 1 is achievable. The general
machinery (M2→M3→M4) proves conditions 2 and 3.

---

## 8. Milestones — ordered, gated

### M2 — Deferred Normalizer
*Prerequisite for M3. Enables softmax-pv fusion.*

| Gate | Metric |
|------|--------|
| Correctness | mean-then-weighted-sum fixture a_err < 1e-3 |
| M3 integration | Three-node chain emits ONE kernel, launches 3→1 |

**Status:** Not started.

### M3 — Chain Fusion
*The proof-of-concept for general machinery. Three-node attention → 1 kernel.*

| Gate | Metric |
|------|--------|
| Launches | 3→1 |
| Time | ≤120 µs f32 (flash gate: 125 µs) |
| Correctness | a_err < 1e-3 both lanes, m3 harness PASS |

**Status:** Not started. Depends on M2.

### S3b — Tensor GEMM Pipeline
*Close the GEMM TF gap. The biggest absolute perf gain.*

| Gate | Metric |
|------|--------|
| 4096³ TF | ≥38 TFLOP/s (F16-acc) |
| Correctness | Full shape portfolio PASS (512³–8192³) |

**Status:** Not started. Builds on S3a (mma.sync proven).

### S5 — Performance Gate
*Close to cuBLAS anchor.*

| Gate | Metric |
|------|--------|
| 4096³ TF | ≥42 TFLOP/s (matches cuBLAS) |

**Status:** Not started. Depends on S3b.

### M4 — Vocabulary Retirement
*Prove general machinery handles what matchers special-cased.*

| Gate | Metric |
|------|--------|
| Fused-attention retired | General path ≤ best config |
| `detect_row_softmax` retired | M4 declared softmax handles it |
| `detect_reduction` retired | M4 dot declaration exists |
| `GemmPlan` retired | Full declaration framework |

**Status:** Not started. Depends on M3.

---

## 9. Verification discipline

### Per-milestone checklist

1. **Baseline table** — ALL benchmark results at current commit BEFORE changes
2. **A/B experiment** — old vs new compiler, full suite, same machine
3. **`cargo test --lib`** — green before commit
4. **Praetor** — on changed files (complexity ≤ 15, lines ≤ 100, params ≤ 6)
5. **Device correctness** — run the shape portfolio on RTX 3060
6. **Commit** — targeted `git add`, descriptive message, benchmark results in
   commit message

### Regression guard

- Inspect every match arm (silent regressions come from removed arms)
- Verify optimized IR, not just tests
- Update architecture comments
- Never delete rationale comments — rewrite them

### The LTO lesson

`llc -O2` / raw `.ll` inspection does NOT reflect the `-O3 -flto` pipeline
used by the benchmark harness. Verify every codegen claim against the actual
linked binary before acting on it.

---

## 10. Risks

| Risk | Mitigation |
|------|------------|
| M2 linearity proof fails for non-linear consumers | Restrict to linear consumers; non-linear paths keep normalized form |
| `cp.async` pipeline depth exceeds smem budget | Dynamic stage count from cost model; fallback to bar.sync |
| Occupancy wall persists after tuning | Accept 2 CTAs/SM for large shapes; small shapes already win |
| Hand-written kernels can't beat ggml | Iterate on pipeline depth and register blocking; measure against cuBLAS, not just ggml |
| General machinery produces slower code than vocabulary matchers | Keep matchers until general path proves competitive; never retire early |
| LLVM defn_liveness panic blocks CPU benchmarks | Fix `str_to_int` row in `intrinsic_helpers`; separate track |

---

## 11. Supersession

This plan supersedes the ad-hoc GPU optimization scattered across:
- `gpu-backend-strategy.md` §5 (S3b-S6 milestones — folded into Track 2)
- `general-machinery.md` M1-M4 (folded into Track 1)
- `briev-vs-cuda-thesis.md` (concreteized into Track 3 + Track 4)

The existing docs remain as reference. This plan is the single source of
truth for GPU performance work.
