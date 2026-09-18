# Coalesced-KV Memory Path — Plan & Audit

**Date:** 2026-09-18
**Status:** Active — M1 starting
**Parent:** `docs/plans/2026-09-18-fused-f16-decode-node.md` (P1 landed
`66367d63`; findings `benchmarks/results/2026-09-18-p1-lane-mapped-reductions.md`)

## The audit: language vs compiler vs layout

The P1 finding was that the fused node's 58 µs gate needs "ggml-grade
memory-path engineering." Per the capability-frontier principle, that
must decompose into language expressibility (a language gap to fix) or
compiler scheduling (a compiler gap to build). Each requirement, audited
against the tree:

| What a competitive fused kernel needs | Language | Compiler |
|---|---|---|
| Fused algorithm (online softmax, rescale, one pass) | ✅ P1 lane-mapped reductions, sequential semantics proven on device | ✅ PTX lane-mapped emitter |
| f16 KV storage | ✅ `Float16` + widening (P0, 36/36) | ✅ both lanes |
| K-cache layout choice (d-major = coalesced) | ✅ state layout is author-chosen; `k[kh*D*NKV + d*NKV + j]` is plain indexing | n/a |
| Loop-carried per-lane state (m/sum/acc) | ✅ registers persist across loop iterations (M2a precedent) | ✅ |
| Load pipelining / ILP | ✅ expressible in principle (named temporaries, prologue loads) — but hand-writing it is a speed keyword in disguise, forbidden by Rule 2's spirit | ❌ the one real gap: `emit_lane_reduction` consumes loads immediately |

**No new language feature is needed.** A lane-dimension escape hatch
would be a Rule-2 violation (a keyword for speed) and is rejected. The
missing piece is exactly one compiler capability: automatic software
pipelining in the lane-mapped loop. The GEMM tier has the cp.async
precedent.

Two decisive facts:

1. **ggml hits 58 µs with the same 20 warps** (one warp per head, VEC
   decode) — occupancy is NOT their advantage. Their advantage: (a) K
   cache column-major per head (`[d][j]`) so lane-over-j reads coalesce,
   (b) deep load pipelining. Both are compiler-schedulable.
2. **The layout fix is testable on the EXISTING 3-kernel composition for
   free.** qk today: work item t=(h,j), loads `k[kh*D*NKV + j*D + d]` —
   consecutive threads 512 B apart (scattered). pv today: work item
   (h,d), loads `v[.. + j*D + d]` — consecutive threads (d+1) adjacent
   (already coalesced). The qk/pv cost asymmetry (227 vs 146 µs for the
   same bytes) IS the coalescing signature. Transposing K alone (V stays
   j-major — transposing V would break pv) should collapse qk toward the
   bandwidth bound.

## Steps

| Step | Work | Gate |
|---|---|---|
| **M1 — layout experiment** (validator, no compiler work) | `examples/gpu/attention_decode_kdmaj.abv` (K-only d-major), `KLAYOUT=dmaj` env in the m3 harness (seed scatters into d-major; CPU reference indexes d-major) and the m4 microbench. Run correctness (both lanes) + timing at bitnet p4096. | qk 227 µs → ≤ ~100 µs proves coalescing dominance; chain 413 → ~230–260 µs predicted. Correctness stays exact (same values, different storage order). **Kill line:** qk does not move ⇒ coalescing model wrong ⇒ re-derive, no M3. |
| **M2 — append-path batching** (runtime, small) | d-major K means a per-token append = D scattered 4 B elements vs 1 row; `briev_accel_push_ranges` caps at 16 ranges — needs a bulk mode (cap raise or batched copy). | only gates M4 integration; not on M1's critical path |
| **M3 — compiler software pipelining** | `emit_lane_reduction`: double-buffer the loaded operands (prologue load → consume-current/issue-next loop). Additive in the lane path only; serial fallback untouched. Rule 2 discipline: the DEFAULT must beat any hand-pipelined source. | probe dot drops toward the load-amortized bound; 2275 tests + M3 matrix + baseline A/B |
| **M4 — fused node v2** | Real fused node (online softmax + acc + f16) on the pipelined lane form with d-major K | the P2 gate as written: beat 3-kernel, competitive with 58 µs |
| **M5/M6** | fattn.cu arm + llama-bench matrix | only on M4 pass; verdict rules as before |

## Honesty constraints

- The layout experiment must keep VALUES identical (same RNG stream,
  scattered into the target layout) so correctness measures composition
  error, not a data bug.
- Append cost is part of any integration number (M2 exists so it is
  measured, not hidden).
- If M3's pipelined default cannot reach the hand-tuned bound, that is
  the finding: recorded as the measured gap between the compiler's
  default and ggml's hand scheduling, never papered over with an
  escape hatch.

## Doctrine (user directive, 2026-09-18)

The compiler is smart and must be smart: under all circumstances it
must SHOW THROUGH ANALYSIS what the fastest code would be. If the
program is poorly written, the compiler can only emit as fast as it
can prove. The programmer's role is real but is exercised one way —
by writing better algorithms — never through strategy keywords or
pragma trickery.

Consequences landed with M1/M3.5:

- Layout is a CONDITIONAL best, not a default: coalescing is a property
  of (layout × work-item mapping), and the preference flips with the
  mapping (qk wants d-major K; pv wants j-major V; the fused node's
  lanes-over-d wants j-major K — the OPPOSITE of qk). A blanket
  compiler default cannot exist, and auto-transposing state would
  cross the host ABI (field tables, seed, append ranges). The knowledge
  lives in the composition/stdlib layer, which owns the kernel and its
  buffer layout together — the BLAS model.
- What the compiler owes by default is the ANALYSIS: the G001 pass
  (`src/analysis/coalescing.rs`, `113f3b94`) reports proven cross-thread
  strides with the mechanical fix, silently for broadcasts and
  cooperative-row mappings, never rewriting. "As fast as the compiler
  can prove" now has a memory-coalescing voice.
- M4 note: the fused node uses j-major K (opposite of qk's d-major).
  If both forms ever share a state buffer, the composition owns an
  explicit transpose node (expressible today, amortized per context).
