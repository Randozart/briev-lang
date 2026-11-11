# Closed-call regions: own allocations across descendant barriers

Base: upstream `main` `109e8bb4` (includes PR #81 publish-safety). Separate from
any optimization PR.

## Reproduction (baseline at this commit)

The load harness's scalar catalogue validator loses ~32.3 MB per call. Captured
1,682-item response, runtime-file input, same linked output `catalogue_checked,N`:

| Repetitions | definitely lost | peak live heap |
|---:|---:|---:|
| 1 | 32,316,928 B | 37,008,220 B |
| 4 | 129,218,560 B | 133,959,004 B |
| 16 | 509,456,128 B | 521,762,140 B |

Artifacts: app `target/main-memory-2026-11-11/` (compiler/input hashes, generated
IR, diagnostic log, memcheck + Massif output).

## Root cause

`List<T> + List<T>` elaborates to `iter_chain` → `iter_chain_loop`, whose
`result <- a[i]` lowers (backend) to the collection growth helper `coll_resize`.
`json_array_entries`/`json_object_entries` (`lib/std/json.bv`) use `entries +
added` / `pairs + entry` in a recursive parser, so a 1,682-item parse allocates
O(n) growing buffers and retires each previous one.

Two analysis/codegen gaps:
1. The reclamation walk models `<-` as a plain store; it never adds the growth
   helper call, so `coll_resize` (and its allocation) is absent from
   `catalogue_valid`'s closure.
2. `coll_resize` is a barrier ("allocation may survive in incoming storage"), so
   at runtime it suspends the caller region and its `malloc` is untracked. The
   existing region proof excludes barrier-callee allocations, so it cannot own
   them. `catalogue_valid` is already a region, but the runtime never adopts the
   growth buffers.

Upstream PR #81's publish-safety does not apply: it recognizes field-projection
publishers, and it correctly refuses to drop the barrier here because the
receiver (`iter_chain_loop`'s `result`) is returned.

## Fix

Add a stronger, frontend-derived proof for an already-eligible nonrecursive
region: the complete call closure's allocations (including barrier callees)
cannot reach the region's return, incoming storage, globals, or external escape
roots. Such a region is **closed**.

Model the missing allocation sources so the proof is sound:
- `<-` (collection insertion) adds the growth-helper call edge (the shared
  `intrinsic_helpers` registry), so `coll_resize` and its buffer enter the
  closure.
- Intrinsic helper effects enter the graph through the same registry (edge +
  argument union → parameters, helper result → intrinsic result). Missing helper
  bodies reject closed ownership. This includes persistent output-buffer
  publication, so an owner that prints is not closed.

Runtime: a closed region pushes a private closed-owner record. Allocation
adoption selects the nearest closed owner even while ordinary inheritance is
suspended, so barrier-descendant allocations (and retired intermediates) are
owned and freed at the closed region's exit. Ordinary nested scopes also adopt
into the closed owner; an independently closed child becomes the nearest owner
until its exit. Explicit free/realloc still search the active frame chain. Both
normal exit and program-end unwind pop closed records before their storage
expires. Ordinary frame layout and suspend/restore behavior are unchanged.

## Gates

- Positive local-container fixture; negatives: returned/borrowed/global/
  unknown/recursive escape and an output-buffer (print) owner.
- Native O0/O3 + Valgrind runtime bookkeeping (nested scopes, barrier suspension,
  independently closed child, realloc/free across barriers, program-end unwind).
- Same captured catalogue repeated 1/4/16: output equality, no invalid access,
  exit-leak accounting, peak-live trend.
- Full library suite, clean build, Praetor, LLVM determinism.
- The complete authenticated journey bounded-memory gate remains the acceptance
  target; this probe fix alone is not that gate.

Update `docs/architecture/llvm-memory-management.md`, backend rationale comments
and `BUGS.md` with the proof boundary and rollback path.
