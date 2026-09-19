> **SUPERSEDED 2026-09-19** by `2026-09-19-general-machinery.md`
> (review decision: no application-level shape recognizer; general
> machinery instead). The runtime-layout findings (§5) remain valid and
> are absorbed there. The FlashDecodeInfo struct edit this plan spawned
> was deliberately reverted before commit.

# FlashDecode LoopShape + PTX Emitter

**Date:** 2026-09-19
**Status:** plan; gate evidence in `benchmarks/flash-decode-gate/` (PASS: 123 µs f32 / 94 µs f16 vs 202 µs best chain)
**Depends on:** gate verdict (met, 2.06-2.7×); frontend-driven dispatch doctrine

## 1. Goal

When a Briev program expresses decode attention the natural way, the
compiler must recognize the shape and emit the gate-proven fused kernel —
same code the hand-PTX ships — parameterized by (H, HKV, G, NKV, D, E).
No strategy keywords; MAXIMUM EFFICIENT DEFAULT.

## 2. The .abv surface (what the author writes)

```
async node fattn [h < H][h == H] {
    let kh: Int = h / G;
    let m: Float = -1e30;
    let l: Float = 0;
    let acc: Float[D];                       // persistent per-head vector
    foreach j < NKV {
        let sc: Float = 0;
        foreach d < D {
            sc = sc + q[h*D+d] * K[kh*NKV*D + j*D + d];
        }
        sc = sc * scale;
        let mn: Float = max(m, sc);
        let p:  Float = exp(sc - mn);
        let cf: Float = exp(m - mn);
        l = l * cf + p;
        foreach d < D {
            acc[d] = acc[d] * cf + p * V[kh*NKV*D + j*D + d];
        }
        m = mn;
    }
    foreach d < D {
        a_out[h*D+d] = acc[d] / l;
    }
}
```

Naive lowering of this is a per-head serial kernel (~ms). The FlashDecode
shape lowering is the gate kernel (~94-123 µs). Same source, same results.

## 3. Frontend detection (AnalysisResults)

New `FlashDecodeInfo` beside `ReductionInfo` in the kernel-shape analysis,
matched when a node's kernel_stmts contain, in order:

1. Work item `h` with count H; a `kh = h / G` decomposition (GQA group).
2. A scalar triple (m, l) initialized (−inf/very-negative, 0) and a
   persistent local array acc[D] (or acc-as-vector local).
3. An outer foreach j over a runtime-invariant count NKV whose body:
   a. an inner foreach d reduce: `sc = sc + q[h*D+d] * K[kh..j..d]`
      (the existing dot-reduce pattern, sc scaled by a const after);
   b. the online-softmax update: `mn = max(m, sc)`,
      `p = exp(sc − mn)`, `cf = exp(m − mn)`, `l = l·cf + p`;
   c. a second inner foreach d: `acc[d] = acc[d]·cf + p·V[..j..d]`;
   d. `m = mn`.
4. After the loop: `out[h*D+d] = acc[d] / l` for all d.

Every deviation (extra statements in the j body, non-linear indices,
stores into inputs) → fall through to the existing general path with a
remark. The `_ => return None` discipline holds: this is a new arm, not a
rewrite.

Fields: `{ h_var, kh_expr, H, G, NKV, D, q, k, v, out, acc_local, m, l,
scale_expr, j_var, d_var }` — resolved against the program's const table.

## 4. Backend emitter (PTX, CUDA lane; SPIR-V later)

Consumes FlashDecodeInfo; emits the v4/v5 structure
(`benchmarks/flash-decode-gate/flash_v5_f16.ptx` is the reference artifact):

- grid = H blocks × 1024 threads (32 warps); warp w owns
  j ∈ [w·(NKV/32), (w+1)·(NKV/32)); requires NKV % 32 == 0 (remark
  otherwise → general path).
- Per-warp online softmax + 4-wide strip loads (K and V j-major required —
  else remark → general path; the layout contract lives with the
  composition/stdlib, the compiler only verifies coalescing class).
- 5-round butterfly reduce; smem LSE merge (red 256 B + alphas 128 B +
  smacc 32·512 B); one bar.sync.
- f16 lanes: `ld.global.u16` + `cvt.f32.f16` (elem from the casting graph,
  not name matching — Rule 19).

Register/identity map taken verbatim from the gate kernel; the emitter is
a template filler over the info fields, not a re-derivation.

## 5. Runtime contract

- a_out proj slot must be disjoint from q/k/v (gate forensics #1) — the
  field packing for this kernel allocates the output slot after all inputs.
- v proj = host offset (forensics #2) — the layout builder already
  satisfies this for freshly packed layouts; assert at emission.
- desc carries a touched scalar (counter) so dispatch takes the dirty
  path (forensics #4) — the runtime fix belongs in
  `briev_dev_cuda_launch_dev2d`: `n_dirty == 0` should mean "no copies",
  not "full copy". One-line C change + note in BUGS.md.

## 6. Tests

1. Frontend: detection unit tests — matching node PASS, each deviation
   (extra stmt, missing cf, non-linear acc index) → None.
2. Emitter: emitted PTX matches the gate kernel modulo offsets/consts
   (golden-file with normalized immediates).
3. Behavioral: the .abv above through the full pipeline, CUDA lane,
   bitnet geometry — parity vs the CPU reference (a_err < 1e-3) and vs
   the gate kernel's output (≤ fp noise).
4. Geometry sweep: (H, HKV, G, NKV, D) ∈ {(20,5,4,4096,128),
   (32,4,8,4096,128), (24,4,6,4096,128)} — the three CyberLlama shapes.
5. Interpreter stays reference: the node's semantics are plain Briev;
   interpreter already runs it (no new interpreter code).

## 7. Non-goals

- Prefill/training shapes (j = query positions): separate shape, later.
- Tensor-core (mma) flash: the decode kernel is FMA/shfl; mma decode is a
  different plan.
- Vulkan/SPIR-V lane: follows after CUDA parity (subgroup ops needed).

## 8. Order of work

1. Runtime: n_dirty==0 fix (one line) + BUGS.md entry.
2. Frontend FlashDecodeInfo + detection tests.
3. PTX emitter + golden test.
4. Behavioral test through the full pipeline (bitnet geometry).
5. Geometry sweep + benchmark row in `benchmarks/results/`.
