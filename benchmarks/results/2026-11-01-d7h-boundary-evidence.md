# D7h boundary evidence — 2026-11-01

Branch: `feat/destination-ownership-d7-d8`

## Green

| Gate | Evidence |
|---|---|
| wasm32 collections/views | `cargo test --test rbv_list_each_wasm` — PASS |
| wasm32 object handles | `cargo test --test rbv_obj_init_wasm32` — PASS |
| wasm32 router | `cargo test --test rbv_router` — PASS |
| no-prelude boundary export | `cargo test --test no_prelude_boundary` — PASS |
| C bridge | `bash benchmarks/build_and_bench.sh bridge_glue --correctness` — MATCH |
| multi bridge | `bash benchmarks/build_and_bench.sh bridge_multi --correctness` — PASS |
| GLUE config suite | `cargo test --test glue_bridge_tests` — 13/13 PASS |
| full runtime | `bash benchmarks/build_and_bench.sh --runtime` — all outputs MATCH |
| public export ABI | wrapper tests: boxed object/enum `i64` public surface |

`no_prelude_boundary` compiles:
`export defn add(a: Int, b: Int) -> Int` with `--disable-plugin prelude`,
and asserts both `@add` and `@add__briev_internal`.

## Open

- Real-browser fake-peer/I11 gate: recommender app worktree is absent in this
  environment; do not claim browser evidence from compiler-only tests.
- Explicit JS export consumer: the full `run_gate.sh` aborts on unavailable
  host toolchains; CI must run the JS-only lane.
