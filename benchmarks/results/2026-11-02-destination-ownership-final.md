# Destination ownership / enum object payload — final 2026-11-02

Merged main: `4dfa4e85` (plus closeout commit)
Compiler: D4b enabled, PR49 resolved sizing + PR50 field context + PR51 owned
extraction; callable-signature enum-object inline gate active.

## Correctness

- `cargo test --lib`: 3441 passed.
- `cargo test --test daily_use_smoke`: 32/32 passed.
- `ll_determinism_check.sh --quick`: PASS.
- net_stack O0/O1/O2/O3: correct.
- ASan stack-use-after-return: correct.
- real recommender I11: PASS, `maxLongTaskMs=0`.
- full runtime: all outputs MATCH.
- GLUE, bridge_glue, bridge_multi: PASS.
- PR49–53 closed as included.

## Runtime snapshot (same-main full suite)

| Benchmark | Briev | C | Ratio |
|---|---:|---:|---:|
| ws_frame | 0.0152s | 0.0114s | 1.33x |
| recommender | 0.0125s | 0.0120s | 1.04x |
| hash_ops | 0.9690s | 1.1358s | 0.85x |
| fsm_dispatch | 0.1402s | 0.1616s | 0.87x |
| arena_bump | 0.3248s | 0.0499s | 6.51x |
| spsc_ring | 0.1489s | 0.0732s | 2.03x |

## Rejected optimizations

- Match-binding copy elision: passed correctness but controlled `ws_frame`
  regressed from 1.02x C to 1.13x C. Removed; PR51 owned extraction is active.
- Relaxing the callable-boundary inline gate: `malloc(): invalid next size` in
  I11. Gate is mandatory.

## Remaining performance lane

Region-local enum/object scalar replacement: replace a fully non-escaping
enum image and object fields with SSA/stack storage; materialize once at an
escape. Separate plan required.
