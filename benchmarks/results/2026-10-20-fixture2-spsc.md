# Stdlib fixture campaign, fixture 2 — SPSC ring: first results

**Date:** 2026-10-20
**Status:** Fixture 2 first pass COMPLETE (ring module, runtime pin, benchmark,
C11 reference). Honest single-threaded scope recorded.

## What was built

- `lib/std/atomic.bv` — `SpscRing` obj: monotonic head/tail indices, power-of-
  two capacity masking, acquire/release atomic loads/stores, `Err(0)` for an
  empty pop. `sring_new` allocates the slot array and the two atomic cells;
  `sring_free` releases all three.
- `tests/tier1/spsc_smoke.bv` + `spsc_smoke_golden_output` — fill to capacity,
  overflow rejection, drain order, empty result. Golden `7|21|7|0|1`.
- `benchmarks/spsc_ring.bv` + `benchmarks/spsc_ring_c.c` — one push + one pop
  per step on a 1024-slot ring; C11 atomics with matching acquire/release
  discipline. Harness TAG `runtime`.

## Scope limit (explicit)

The reactor is single-threaded today; `os/thread.bv` is a stub. This fixture
validates atomic lowering, ordering markers, masked index math, and the ring
protocol. It does **not** validate cross-thread races. The C reference uses
C11 atomics, but the single-threaded harness cannot exercise a true race.

## Performance

| Metric | Value |
|---|---|
| Briev | 0.1764s |
| C | 0.0930s |
| Ratio | **1.90x (C wins)** |
| Correctness | MATCH (`0\n10` at BOUND=5; identical periodic+final stream) |

The gap is the per-pop `Result<Int,Int>` heap box plus the atomic/object
field path; C returns a scalar and touches two inline atomic cells. This is
the same small-sum-type representation gap Fixture 4 exposed (2.82x FSM), now
visible on a different shape. Recorded as a performance discovery; no
strategy keyword or source hand-inline was used to hide it.

## Gates

- `spsc_smoke_golden_output` PASS.
- `spsc_ring` harness row MATCH.
- `cargo test --lib` / determinism / parity rerun at the fixture commit.

## Follow-ups

1. Unboxed small sum types / Result ABI (also Fixture 4).
2. True threading lane (pthreads) so the SPSC protocol can be exercised under
   a race detector; today this is a lowering/ordering test only.
