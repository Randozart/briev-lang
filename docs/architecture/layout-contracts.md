# Layout Contracts — the ABI proof chain

**Date:** 2026-09-18
**Status:** Active doctrine
**Origin:** the memory-path campaign
(`docs/plans/2026-09-18-coalesced-kv-memory-path.md`; the M1 experiment
proved coalescing = layout × mapping with qk 227→64 µs). See also the
analysis-transparency doctrine in `briev-capability-frontier.md`.

## What this document fixes

Where layout knowledge lives, who owns it, and what the compiler may do
with it. The rule in one line: **layout is algorithm design with an ABI
tail — the author expresses it, the compiler proves facts about it and
reports them, and neither silently rewrites it across the host boundary.**

## The proof chain (what exists today)

A state field's device layout is *derived, not declared*. Each link is
machine-checked:

1. **Element facts** — `spec` physical keys (`Bits/MaxBits/Bytes/
   Alignment/Endian`, agent-reference §8.2) fix the element's storage
   width and alignment through the casting graph. `Float16` fields, for
   example, lower to half storage with f32-widened compute.
2. **Projection layout** — declaration order + element facts produce the
   projection: host offsets and device offsets per field
   (`projection_offsets`), the SSBO member structure, and the generated
   field tables (`{ name, kind, host_offset, elem_bytes, count,
   is_write, proj_offset }`). The two offset spaces are the ABI seam:
   alignment padding makes them differ, so every cross-boundary move
   carries both.
3. **The append contract** — `briev_accel_push_ranges(state, triples, n)`
   takes (projection offset, host offset, bytes) triples: the host
   staging buffer and the device working set agree through the tables,
   never through hardcoded arithmetic. Any layout change that keeps the
   tables honest keeps the append path honest.
4. **Array stride** — array members decorate `ArrayStride` from the
   element facts; multi-dimensional order (row-major vs column-major
   storage of a logically 2D buffer) is *index arithmetic in the .abv*,
   which is the author's algorithmic choice, not metadata.

## The conditional-best rule

Coalescing is a property of **(layout × work-item mapping)**, not of
layout alone. The preference flips with the mapping:

| Kernel | Mapping | Coalesced order |
|---|---|---|
| qk (score) | work item = (h, j), serial d | d-major K |
| pv (combine) | work item = (h, d), serial j | j-major V |
| fused node | lanes = d, serial j | j-major K |

Therefore no blanket layout default can exist, and the compiler must
never auto-transform storage: the layout is part of the program's host
contract (field tables, seed, append ranges), and a transform that is
right for one kernel is wrong for another sharing the buffer. Ownership
follows the BLAS model: **the composition (or stdlib attention library)
owns the kernel and its buffer order together.** When two mappings must
share one buffer, the composition owns an explicit transpose node —
expressible today, amortized per context.

## The compiler's role: analysis, never rewriting

The G001 pass (`src/analysis/coalescing.rs`) is the analysis voice of
this document: for flat 1D kernels it proves the cross-thread stride of
every state-array load (affine coefficient of the work-item counter;
div-protected terms are run-constant — the local-affine view that makes
`q[t/NKV * D + d]` a broadcast and `k[..., j*D + d]` a stride-D read),
reports the proven stride with the mechanical fix, stays silent where it
cannot prove, and names the defeating term (G002) where it cannot.

Cooperative row kernels are exempt by proof, not by whitelist: their
lanes come from `tid.x`, not the work-item counter, so the work-item
stride is a row stride — not a defect (`is_cooperative_shape`, one
source of truth in the analysis layer).

What the compiler may NOT do: silently transpose, relayout, or reorder
state storage. What it may do (future, when the append contract can
express it): insert *provable* layout-adaptation transforms between
kernels — that is scheduling, not algorithm-stealing — with the same
prove-and-report obligations.

## Pressure-valve candidates (evidence-gated, not speculative)

- **Machine-checked layout agreement for appends** — if M4 integration
  shows append paths drifting from the .abv's index order, the valve
  opens for a `spec`-level storage-order declaration that the field
  tables verify. Not built: the index-math form is proven, and a spec
  key before the evidence is a directive in disguise.
- **Aliasing facts** — aggressive kernels will eventually need "these
  buffers do not overlap" as contracts. Enters as contract language
  (general), never as `restrict`-style per-pointer pragmas.
