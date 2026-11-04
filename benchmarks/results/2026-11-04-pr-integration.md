# PR integration — 2026-11-04

Base: `main` at `c2c143bb` (post typed-HIR/arena landing). All six open PRs
(odecode) were merged with `--no-ff` after local verification.

## Merged PRs

| PR | Merge commit | Verified by |
|---|---|---|
| #55 concrete bit casts | `624020e8` | `bit_encoding_override_receives_canonical_storage_word`, `rbv_list_each_wasm`, `arena_free_through_cast_is_noop` |
| #56 guarded tail returns | `1c31417a` | `guarded_tail_backedge_does_not_swallow_later_return` |
| #57 discarded enum payloads | `dca741d0` | `enum_wildcard_allocations`, 75 enum unit tests |
| #58 enum result capture | `2b0d4ab4` | `process_units`, `test_enum_handle_abi_no_struct_decl`, `enum_list_lifetime_regression` |
| #60 stdout integer lifetime | `cb875805` + `57ccfbe9` | `print_integer_lifetime` |
| #59 indexed collection writes | `b4a0eb7a` | `load_prerequisites` (2), 15 collection / 57 index / 75 enum unit tests |

All six now report `state=closed, merged=true` on GitHub (the `--no-ff`
merges preserve the PR head SHAs).

## #60 performance finding and resolution

The PR's recursive `stdout_decimal_digits` formatter removed the retained
String but regressed print-heavy benchmarks 40–60%:

| Benchmark | pre-#60 | with #60 | after fix |
|---|---:|---:|---:|
| print_loop | 0.0144 | 0.0244 | **0.0155** |
| queue_drain | 0.0150 | 0.0241 | **0.0150** |
| sparse_dispatch | 0.0310 | 0.0472 | **0.0324** |
| stack_push_pop | 0.0215 | 0.0298 | **0.0220** |

Controlled A/B isolated the cause to the formatter (restoring `int_to_str`
recovered the times; `%enum_out` nocapture was unrelated — reverting #58's
`nocapture` changed nothing).

Resolution (`57ccfbe9`): keep the fast `int_to_str` path and free both temps —
the intermediate `Data` buffer inside `int_to_str` (`briev_bits_to_str` copies
from it) and the returned String in `print_int`. `print_integer_lifetime`
passes with **zero live allocations** across 2,048 printed pairs, and the
benchmarks return to their prior times. `stdout_decimal_digits` retired; the
PR's test/fixture/docs are kept, and the plan doc records the follow-up.

## Final gates on merged `main` (`1e08db65`)

- `cargo test --all` — **pass**
- `cargo test --lib` 3461 · `daily_use_smoke` 37 green
- `ll_determinism_check --quick` — all deterministic
- `build_and_bench.sh --runtime` — **every row MATCH**; no unexplained
  regression (`ws_frame` 1.01–1.12x C across re-runs; a 1.39x outlier was
  noise). Pre-existing C-losses unchanged (`arena_bump`, `spsc_ring`,
  `nbody_newton_accel`, `telemetry_stream`, `accumulator_flush`).

## Branch audit (Phase 1)

All intended work was already on `main`: `feat/tail-loop-lowering`,
`fix/wasm-tuple-abi` doc, and PR #48 were present; the remaining local/remote
branches are experiments (`experiment/*`, `feat/canonical-effect-abi`,
`feat/ptr-provenance-fence-retirement`) or integration remnants
(`integrate/pr31`, `integrate/pr32`, `main-prep`) — left in place for owner
review (no deletion).
