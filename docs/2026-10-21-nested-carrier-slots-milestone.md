# Milestone — nested enum carrier slots

**Date:** 2026-10-21
**Branch:** `feat/nested-carrier-slots`
**Implementation:** `04dd1681`
**Docs pin:** `b5d73f30`
**Plan:** `docs/plans/2026-10-21-nested-carrier-slots.md`
**Tag:** `milestone/nested-carrier-slots`

## What landed

Briev can now keep inline nested enum state alive across recursive iterations
without heap-boxing it.

The FSM hot loop previously paid for:

1. the outer `Result` image;
2. the nested `TcpState` image;
3. allocator pressure and indirect tag/payload loads on every transition.

It now uses:

- a concrete enum layout;
- a hoisted sret temporary for the current transition;
- a stable carrier slot for loop-carried state;
- an explicit ownership copy at the recursive boundary.

## Result

| Metric | Before | After |
|---|---:|---:|
| `fsm_dispatch` ratio | 3.08x | **1.48–1.49x** |
| FSM enum `malloc(16)` in hot path | many | **0** |
| Correctness at BOUND `0..8`, 1M, 50M | yes | **yes** |
| Stack growth at 50M | failed in earlier attempt | **none** |
| Full runtime suite | MATCH | **MATCH** |
| Library tests | green | **3,296 green** |

`ws_frame` also remains a Briev win at **~0.27x**, and `spsc_ring` remains at
**~1.89x** with zero enum boxes.

## Why it matters

This was not a benchmark trick. The compiler now derives:

- the concrete nested enum layout;
- the temporary sret lifetime;
- the loop-carried ownership boundary;
- a stable carrier slot;
- an ownership copy at the recursive edge.

No benchmark names, no field prefixes, no strategy keyword, no source change.

## The road here

1. The effect-shape work restored the stateless and escaped wins.
2. The canonical `%EffectCtx` ABI was correct but lost the carry-winning shape.
3. Fixed-size nested sret slots removed boxes but caused stack overflow by
   blocking tail-call optimization.
4. Hoisted sret slots fixed TCO.
5. The first nested retry exposed the real lifetime rule: the temporary result
   cannot own loop-carried state.
6. Carrier slots added the missing ownership boundary.

Failed attempts remain preserved:

- `experiment/nested-enum-b3` (`9a922d04`)
- `experiment/nested-b3-hoist` (`2805db65`)

They were not merged.

## Verification

- FSM outputs match C at BOUND `0..8`, `1_000_000`, and `50_000_000`.
- FSM enum `malloc(16)` count: **0**.
- Full runtime suite: **all MATCH**.
- `cargo test --lib`: **3,296 passed**.
- Effect-shape quick guard: **PASS**.

## Celebration

This one was earned the hard way: three rejected lifetimes, one TCO trap, and a
lot of assembly reading. The final design is small, structural, and safe.

The FSM is no longer paying allocator tax for existing.
