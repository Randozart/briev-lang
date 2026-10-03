# Plan: declared dot — the detect_reduction retirement rung (M4, 2026-10-03)

**Status:** ACTIVE — the M4 ladder's row 3
(`2026-09-20-gpu-dialect-beyond-cuda.md`: "detect_reduction (Dot) — retire
when M4 dot declaration exists"). Mirrors the declared-matmul rung
(`2026-10-03-declared-matmul-gemmplan-retirement.md`, LANDED) — read it
for the doctrine framing; this plan records only the deltas.

## The shape of the loan

`detect_reduction` (accel.rs) derives the reduction FACT — that stays
(associativity licenses the general machinery: warp slicing, split, the
KernelPlan's `associative_reduce` proof). The loan is the SPECIALIZATION:
`is_cooperative_shape` (accel.rs:2736 = the `spirv_row_cooperative` knob
[shipped ON] + the Dot fact + no counter decomposition) routes
reduction-shaped bodies to the hand-written COOPERATIVE row-kernel
emitters (PTX `emit_cooperative_reduction_ptx`, SPIR-V kernel.rs) BEFORE
the GEMM tier. Recognition of a dot/matmul shape decides the lowering.

## Who actually routes cooperative today

The attention-chain fixtures: attn_s1, attn_decode, attn_decode_h,
gc_h128, gc_nophase, gc_rev — their qk/pv/gemm nodes are MATMUL-canonical
bodies (the probe: "attn_/gc_ nodes match the GEMM facts"). NOT the kwab
pair (nested loops — detect_reduction scans top-level single-statement
foreach only) and not softmax_rows (the Softmax branch — separate rung:
the 3-pass rows form has no declared composite yet; its ladder row is
detect_row_softmax's own residual).

So the Dot rung's declaration coverage ALREADY EXISTS for every fixture
that routes cooperative: `matmul!`. The new declaration covers the
remaining canonical form the cooperative emitter serves: the N=1 row dot
(gemv — `out[i] = Σ_k x[i·K+k]·y[k]`, no counter decomposition).

## Changes

1. `lib/std/numeric.bv`: `dot!(xbuf, ybuf, obuf, klen, widx, acc_ty, kk,
   acc)` — the canonical row dot, one work item per output row.
2. `analysis::accel`: `declared_composite(shape, name)` — the generic
   marker check (the declared-matmul channel generalizes); a test.
3. `is_cooperative_shape`: `&& (declared_matmul(shape) ||
   declared_composite(shape, "dot"))` — the cooperative channel follows
   the declaration, like the tensor channel. `kernel_plan.rs`'s
   cooperative-record arm gates identically (the plan records what the
   compiler will do).
4. Fixture migration: the six chain fixtures' matmul nodes → `matmul!`
   (the identical canonical body).
5. Unit tests: the synthetic cooperative shapes in the kernel/gemm tests
   declare; new: undeclared bodies keep `shape.reduction` (the FACT) but
   route non-cooperative.

## Gates

- Suite; Praetor no-new-rows.
- Byte-identity: the migrated fixtures' kernel blobs vs HEAD
  (disassembly-identical modulo constant order — the same canonical body
  reaches the same emitter); the 65-fixture sweep for collateral.
- Device: the cooperative routing is unchanged for every DECLARED shape —
  the standing evidence carries; no new timing (no new codegen).
- gemm_chain stays with the chain instrument (unchanged scope).

## Non-goals

- The Softmax branch (detect_row_softmax + the cooperative softmax
  emitter + a `softmax_rows!` composite) — the ladder's row-2 residual,
  its own increment.
- The LLVM-side const folding (queued separately).
