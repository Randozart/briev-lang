# Slicing — zero-copy views and the layout that makes them free

**Recorded 2026-10-09.** Commits: `e5f523e0` (strided slice unblock), `6b995f1b`
(contiguous slice → zero-copy view).

## The core fact: `List` and a column share a layout

A 1-D vector state column is `[N x elem]`. A `List<elem>` handle (the boxed
Tier-List ABI) is a 24-byte header:

```
{ base_ptr : ptr, cap : i64, len : i64 }
```

and `sub[i]` reads **slot `i+1`** off `base_ptr` (slot 0 is the length header;
see `emit_expr.rs` Ptr-Index, the `is_list_type` → `+1` offset). Because a
`Vector<N>` column is *exactly* the `[N x elem]` array the header would point
at, a slice of the column is a **pointer rebase + length** — no data movement.

That shared layout is what makes the view free. Without it (e.g. a strided or
2-D slice) the compiler must gather.

## Contiguous slice `data[lo:hi]` → zero-copy VIEW

`emit_contiguous_slice_view` (`src/backend/llvm/emit_expr.rs:2043`) emits, per
slice:

1. **Clamp** `lo`, `hi` into `[0, N]` with `icmp` + `select` (no runtime call):
   `len = hi − lo`, forced to `0` when `hi < lo` (empty range). Mirrors the
   old runtime gather's `coll_min`/`coll_hi_fix` contract.
2. **Rebase**: `base = data + lo` — one GEP into the column at element `lo`.
3. **Header**: `malloc(i64 24)`, store `{ base, len, len }`. That is the slice.

The `base` is a **live GEP register**, `ptrtoint`-ed into the header's
`base_ptr` slot — no `inttoptr` round-trip needed at emission. Downstream reads
(`sub[i]`) load the header's `base_ptr` back to a pointer, so the whole chain is
plain `getelementptr`/`load`/`store`/`ptrtoint`/`inttoptr` — exactly the IR LTO's
InstCombiner folds. Verified against the generated `.ll` (`benchmarks/
slice_reduce.bv`): the header `base` is stored from the GEP reg, and the
foreach reads the source column through it. This is the LTO lesson applied — the
claim was checked in the generated IR, not assumed from `llc`.

Reads then flow through the existing Ptr-Index path (`sub[i]` → `base_ptr +
(i+1)`), so `data[1:5]` read at `sub[2]` lands on `data[1+2] = data[3]`. Zero
copy.

**What it replaces.** The old path (`briev_slice_range64` in
`lib/std/cast_lanes.bv`) did `malloc([len+1])` + a `Copy#` loop over the
elements. On a 150-element slice taken every iteration of a 5M-iteration
hot loop, that was the entire cost.

**A/B (the proof).** `benchmarks/slice_reduce.bv` — each iteration takes
`data[OFF:OFF+LEN]` and reduces it; `data` is **mutated** each iteration so the
slice is a runtime value (the non-mutating variant folds the whole reduction to
O(1) — the observability trap, a correct fold, not a throughput number).

| path | time @ BOUND=5M | `.text` |
|---|---|---|
| copy (`Alloc#+Copy#`/iter) | 0.342–0.375 s | 3522 B |
| view (zero-copy) | 0.00097–0.00150 s | 3290 B |

~250–380× on this workload. `.text` near-identical (not a binary-size fold).
Both print the same output (correct).

**Why LLVM won't do this natively.** The compiler lowers `data[1:5]` to a call
to a gather function. LLVM sees a buffer + a call — it has no way to know the
call *can be replaced* by a pointer rebase + length. That substitution is
**Briev's type knowledge** (a `List` is a pointer+length header; a `Vector<N>`
is the same array). It is the "derive, don't recognize" win: one general
lowering, no per-shape special case in the backend.

## Strided slice `data[lo:hi:step]` → gather (cannot be a view)

A strided slice is *not* a contiguous rebase — it is a gather. It is lowered
to the stdlib runtime gathers `briev_slice_strided64` / `briev_slice_strided_f32`
(`lib/std/cast_lanes.bv`), which build a fresh contiguous `[len][elems]` block
with the stride baked in. Before `e5f523e0`, any strided slice was a
`panic!` (`emit_expr.rs`, Slice arm). The f32 variant widens 4-byte elements
into 8-byte slots (the pre-existing f32-list convention).

**Decision rule** (derived, not matched): if the slice is a 1-D contiguous
sub-range of a `[N x elem]` column → **view** (free). Otherwise (strided, or a
column that is not the same element type) → **gather**.

## The alias caveat (read this before relying on the view)

The view is an **alias** of the source: `data[1:5]` and `data` share memory.
Mutating `data[3]` changes `sub[2]`. The old copy was *independent*.

- For **read-only** slice usage (reductions, passing to a consumer, the common
  case) the alias is invisible and the view is strictly faster.
- A program that **mutates the source** and *expects the slice to be a frozen
  copy* would observe new semantics. No such case exists in the suite (recorded
  here; flag it the first time it bites).

If that case ever appears, the fix is a **general** one (a "copy on write" or an
explicit `copy` modifier on the slice), not a per-shape backend arm — consistent
with the no-recognition doctrine.

## Where the code lives

- `src/backend/llvm/emit_expr.rs` — `emit_contiguous_slice_view` (:2043); the
  Slice arm routes contiguous → view, strided → gather (:~1863-1933);
  `box_gather_as_tier_list` (:2114) for the gather result.
- `lib/std/cast_lanes.bv` — `briev_slice_range64`/`_f32` (contiguous gather,
  retained for non-column uses), `briev_slice_strided64`/`_f32` (strided).
- `src/backend/llvm/mod.rs` — the gather declarations.
- `benchmarks/slice_reduce.bv` — the A/B workload.
- `src/backend/llvm/tests.rs` — `test_strided_slice_emits_strided_gather`,
  `test_contiguous_slice_emits_zero_copy_view`.
