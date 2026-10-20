# Stdlib fixture campaign, fixture 3 — Arena: first results

**Date:** 2026-10-20
**Status:** Fixture 3 first pass COMPLETE (obj semantics, aligned alloc, runtime
pin, benchmark, C reference). Performance gap recorded.

## What was built

- `lib/std/memory/arena.bv` — `Arena` is an `obj` now, so `arena_alloc`'s
  offset advance survives the call. Contracts: non-negative size, capacity
  bound, power-of-two alignment in `arena_alloc_aligned` (1..16). The backing
  address is an i64 handle (`base: Int`), matching ByteBuf and the current ABI.
- `tests/tier1/arena_smoke.bv` + `arena_smoke_golden_output` — bump deltas,
  16-byte alignment padding, reset. Golden `16|8|24|16|0`.
- `benchmarks/arena_bump.bv` + `arena_bump_c.c` — 1 MiB arena, 50M × 64-byte
  allocations with reset-on-full, store/read checksum; harness TAG `runtime`.

## Compiler fixes surfaced

1. **Ptr-returning defn call ABI**: `arena_alloc` returns `Ptr<Bit<8>>`; the
   ABI returns `ptr`, while every Briev consumer treats Ptr as an i64 handle.
   `arena_alloc(...)` in a Ptr-returning helper emitted
   `inttoptr i64 <ptr>` (invalid IR). `finish_user_call` now ptrtoints a
   ptr-ABI Ptr result once at the call boundary — same convention as Ptr
   params (ptr → i64 at entry, i64 → ptr at calls).
2. **`obj` Ptr-field literal store**: a `base: Ptr<...>` literal field was
   initialized by storing the i64 handle into a `ptr` field. Arena now uses
   `base: Int` (the honest handle form for this ABI); the general obj Ptr-field
   representation mismatch remains logged as a separate open item.

## Performance

| Metric | Value |
|---|---|
| Briev | 0.2563s |
| C | 0.0552s |
| Ratio | **4.64x (C wins)** |
| Correctness | MATCH (`0\n10` at BOUND=5; same periodic+final stream) |

The gap is call/object overhead in the hot path: each step calls
`arena_alloc` (not inlined), reads/writes the obj fields, then calls
`Store#`/`Load#`; the C reference is a direct pointer bump. The allocator's
contracts are proven and the aligned path is branch-free — the remaining cost
is the missing small-function inlining/field-scalar-replacement capability,
not allocator logic. Recorded as a performance discovery; no strategy keyword
or C-style hand-inline was used to hide it.

## Gates

- `arena_smoke_golden_output` PASS.
- `arena_bump` harness row MATCH.
- `cargo test --lib` / determinism / parity rerun at the fixture commit.
