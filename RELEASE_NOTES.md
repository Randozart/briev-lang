# Briev v0.1.0-preview

**Date:** 2026-10-28
**Support contract:** Tier 1 = native Linux `.bv` and browser wasm32 `.rbv`;
all GPU, electronics, silicon, SPIR-V, and unlisted backends are experimental.

## What works

- Briev language end to end on native Linux and the wasm32 browser runtime:
  parsing/typechecking, contracts, reactive reactor, callable transactions,
  collections, strings, JSON, networking transports, PostgreSQL client, and
  the recommender application.
- Reference interpreter and compiled backend agree on the shared parity corpus,
  including callable transactions and 7000-byte WebSocket frames.
- Browser/server integration passes the deterministic pending-peer gate and the
  real I11 browser/server gate (`maxLongTaskMs=0`).
- Deterministic IR output passes the quick and full corpus gates.
- `brievc targets` prints the baked target support table.

## Release evidence

- `cargo test --all`: green.
- Conformance sweep, contract census, effect-shape guard: green.
- IR determinism: full corpus byte-identical.
- Toolchain matrix: clang/LLVM 23.1.3 native/multinode/giantnode green with
  MovieLens input present.
- Runtime benchmark suite: output parity on every row; no locked-baseline
  regressions in the sampled A/Bs. `ws_frame` is 0.30x C after fixing a
  redundant enum-return image copy.
- Recommender app: PostgreSQL/MovieLens server gates and browser gates pass;
  PR pins the compiler revision.

## Known limitations

- C/C++/Python/Node bridge round-trips, C cancellation, the needs-state
  reference case, and some CLI entry tests remain quarantined.
- GPU, PTX, SPIR-V, electronics, silicon, and freestanding targets are not
  release-blocking and may change.
- The external recommender checkout lacks `recommender_server_properties.rs`;
  its server-property gate is an explicit skip.
- Toolchain-matrix multinode/giantnode rows need MovieLens `u.data`; without it
  those rows are skipped rather than silently passing.

## Build and smoke test

```bash
cargo build --release --bin brievc
./target/release/brievc targets

cat > hello.bv <<'BV'
node main [beginprogram][true] {
    println!("hello from briev");
    endprogram;
};
BV
./target/release/brievc build hello.bv --out ./hello
BOUND=1000 ./hello/hello
```

The native artifact links with the system clang/LLVM toolchain. Browser builds
additionally require LLVM's wasm backend:

```bash
./target/release/brievc build page.rbv --split --out ./page
```
