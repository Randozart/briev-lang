# Broaden the native + memory campaigns (2026-11-10)

Status: ACTIVE. Branch `test/broaden-campaigns`. This is step #3 from the
defect-discovery plan (`docs/plans/2026-11-10-exhaustive-defect-campaign.md`).

## Goal

Widen the two expensive, high-signal harnesses beyond their current families:
the **memory-defect campaign** (`tests/memory_campaign.rs`) and the **native
differential** (`tests/deterministic_transitions.rs`).

## Memory campaign: families 5–7

Added three ownership-heavy shapes that target the historically buggy
aggregate-escape sinks (BUGS.md 2026-11-03):

- **5 — enum payload allocation.** An allocation carried as an enum payload and
  matched out (Sum image ownership).
- **6 — nested object field.** An allocation inside a nested object field.
- **7 — tuple of allocations.** Two allocations held in a tuple then projected.

Each is compiled natively and run under ASan (O0/O1) and Valgrind. All three
are clean across seeds.

## Native differential: float family — blocked, reported

A `Float` native family was prototyped (via `as Float`/`as Int` casts, to keep
the harness's Int trace channel). It exposed two things:

1. **Fixed:** the interpreter's `Int↔Float` casts were identity (`7 as Float`
   stayed `Int(7)`; `2.75 as Int` stayed `Float(2.75)`), while the backend
   converts. See `BUGS.md`.
2. **Open:** `Float` is declared 32-bit but its operators bind to `*F64#`
   intrinsics and the interpreter uses f64, so interpreter-vs-native float
   values drift (f32 rounding). Exact equality cannot gate a float family.

The float family was therefore **not landed**; it needs one width end-to-end or
a tolerance-aware trace channel. Recorded as an OPEN finding in `BUGS.md`.

## Gates

- `cargo test --lib` — see below
- `memory_campaign` families 5/6/7 — 6 cases each, clean under Valgrind + ASan
- cast regression tests — `interpreter::eval::tests::test_cast_*`
- **broad sweep** (families 0,2,3,4,5,6,7 × seeds 11,22 × 10 cases = 140
  programs) — all clean under Valgrind + ASan; zero findings

## Follow-ups

- A float native family once the width question is resolved (or with a
  tolerance channel).
- More memory families (closure capture, collection-of-allocations, reallocation
  loops) and more seeds for the existing ones.
