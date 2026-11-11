# Closed-call ownership verification (2026-11-11)

Base: upstream `main` `109e8bb4`. Candidate adds `closed_regions` and its runtime
consumer. No optimization PR is mixed in.

## Reproduction and fix

External scalar catalogue validator (`catalogue_valid`), captured 1,682-item
response, runtime-file input, same linked output `catalogue_checked,N`. Valgrind
3.26, `clang -O3 -flto -march=native -ffast-math` link.

| Repetitions | Before: definitely lost | Before: peak live | After: definitely lost | After: peak live |
|---:|---:|---:|---:|---:|
| 1 | 32,316,928 B | 37,008,220 B | 0 B | 37,164,092 B |
| 4 | 129,218,560 B | 133,959,004 B | 0 B | 37,164,092 B |
| 16 | 509,456,128 B | 521,762,140 B | 0 B | 37,164,092 B |

After the fix, 1/4/16 repetitions report **0 definitely, 0 indirectly and 0
possibly lost bytes**; the only remaining block is the 65,536-byte stdout buffer
(`stdout_byte`), which is *still reachable*, not leaked. Peak live heap is flat
at 37,164,092 bytes across all repetition counts.

Artifacts: app `target/main-memory-2026-11-11/` (compiler/input hashes, generated
IR, diagnostic log, memcheck + Massif output for before/after).

## Frontend proof

`catalogue_valid` is marked `closed_regions` (diagnostic). The proof follows the
complete call closure including barriers, and rejects any allocation reachable
from the region's return, incoming storage, globals or external escape roots.
`<-` growth is modelled as a fresh backing store; intrinsic-helper effects enter
through the shared `intrinsic_helpers` registry, so a region that prints (whose
`stdout_byte` buffer reaches a global) is not closed.

## Regression gates

- Native `reclamation_aliases` Valgrind-clean at O0 and O3/LTO (`reclamation_ok`).
- `reclamation_bounded` bounded-tail fixture passes (5/5 repeat runs).
- `tests/reclamation_runtime.rs`: 3 passed, including new closed-scope coverage
  (nested scopes adopting into a closed owner, an independently closed child,
  realloc/free across barriers, program-end unwind, no stale capture) at O0/O3.
- Focused reclamation unit tests: 46 passed.
- Full `cargo test --lib`: 3605 passed, 0 failed.
- `ll_determinism_check.sh --quick`: all deterministic.
- Praetor: no new diagnostics on changed files.

These characterize the probe fix. The complete authenticated journey
bounded-memory gate remains the acceptance target and is not claimed here.
