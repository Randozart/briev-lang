# Arena runtime measurement — 2026-11-03

Companion: `docs/plans/2026-11-03-arena-runtime-parity-and-folding.md`.

## Observable benchmark (arena vs arena)

`benchmarks/arena_churn_sym.bv` — bump arena, **volatile** store/load so the
memory work is observable (no store→load forwarding / dead-allocation fold).
Mirrors `benchmarks/arena_churn_sym_c.c`.

`BOUND=50_000_000`, interleaved ×5, same host, same output
(`12499997500000` checked at `BOUND=5_000_001`).

| run | Briev arena | C arena |
|---|---:|---:|
| 1 | 339 ms | 240 ms |
| 2 | 310 ms | 224 ms |
| 3 | 319 ms | 244 ms |
| 4 | 332 ms | 230 ms |
| 5 | 321 ms | 230 ms |
| **mean** | **~324 ms** | **~234 ms** |

**Briev is ~1.38x C.** Root cause: per allocation, the arena round-trips
`%state` (load ptr/end, store ptr) plus the lazy-init base check. A stored
value through the allocated pointer may alias `%state`, so LLVM cannot keep
the bump in a register. C keeps `used`/`buf`/`cap` in registers.

## Foldable variant (the "prove it away" path)

`benchmarks/arena_churn.bv` (non-volatile) is foldable: with the baseline
compiler it scaled sublinearly (18 ms at 50M) because the allocation was
proven dead. With the current arena codegen the bump's `%state` stores
prevent the fold (linear, ~324 ms). Preserving the fold is workstream O
(dead-allocation elimination).

## Next (R)

Register-resident bump/end (local cursor promoted by SROA, cold out-of-line
grow, scope init/reset) — target ≤ C (~234 ms) and ideally below.
