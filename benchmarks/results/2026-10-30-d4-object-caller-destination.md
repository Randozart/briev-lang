# D4 object caller destination — 2026-10-30

Branch: `feat/generalized-value-destination-ownership`
Current: object caller destination + internal enum object-payload inline
Parent: `1353e6ad` (object payload inline, no caller destination)

## Correctness

- `ws_frame`: `0016384` MATCH (harness and linked A/B binaries).
- ASan: `clang -O1 -g -fsanitize=address`, `detect_stack_use_after_return=1`,
  exit 0, output `0016384`.
- JSON parse/stringify gates: PASS.
- ws codec parity: PASS.
- full runtime harness: all benchmark outputs MATCH.

## Controlled ws_frame A/B

Same source, same clang link command, interleaved runs, `BOUND=50000000`.

| Binary | Runs (s) | Mean (s) |
|---|---:|---:|
| parent | 0.0486, 0.0497, 0.0474, 0.0485, 0.0484, 0.0439 | 0.0477 |
| current | 0.0455, 0.0493, 0.0468, 0.0464, 0.0423, 0.0449 | 0.0459 |
| C reference | 0.0331, 0.0274, 0.0317, 0.0282, 0.0340, 0.0279 | 0.0304 |

Ratios:

- parent / current: **1.041x**
- current / C: **1.510x**
- parent / C: **1.571x**

## Allocation evidence

- `ws_frame.ll` malloc sites: parent 63 -> current 62.
- `ws_partial` has hidden `%object_out` and no `malloc`.
- The `Result<WsFrame,_>` object payload is written/copied inline; no per-frame
  `WsFrame` heap image remains.

## Full runtime harness (same commit)

Selected/runtime suite completed with every correctness column MATCH.
Key rows:

| Benchmark | Briev (s) | C (s) | Ratio | Correct |
|---|---:|---:|---:|---|
| recommender | 0.0137 | 0.0148 | 0.93x | MATCH |
| ws_frame | 0.0396 | 0.0268 | 1.48x | MATCH |
| fsm_dispatch | 0.1342 | 0.2398 | 0.56x | MATCH |
| arena_bump | 0.2446 | 0.0564 | 4.34x | MATCH |
| spsc_ring | 0.1391 | 0.0750 | 1.85x | MATCH |
| linked_list | 0.9621 | 1.8274 | 0.53x | MATCH |

Provisional: full-suite ratios vary with machine load; the controlled ws_frame
A/B above is the decision evidence for this slice.
