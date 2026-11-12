# Saltpetre — the Briev ML library

`lib/saltpetre/` is Briev's machine-learning library: an ONNX model reader,
a tensor runtime, a graph executor, and frontends. It is pure `.bv` — the
compiler carries no knowledge of it (Rules 14/24: library-land holds the
temporal machinery; the compiler holds proofs, arenas, and lowering).

## Modules

| Module | Contents |
|---|---|
| `tensor.bv` | `Tensor`, allocation/drop, the `ml_arena_*` request-arena internals |
| `proto.bv` | ONNX protobuf reader (incl. external-data files) |
| `onnx.bv` | graph model, value info, initializers |
| `exec.bv` | op executor — the op ledger is the kiosk parity ledger |
| `frame.bv` | image decode → tensor frontend |
| `yunet.bv` | YuNet detection decoder — **temporal**, Rule-23-safe: it lives here, not in the compiler |
| `session.bv` | arena lifecycle owner: `sp_begin` / `sp_request_reset` / `sp_run` / `sp_run_env` / `sp_end` |

## Arena ownership (session contract)

The library owns both request-scoped arenas. Services call `sp_begin` once
at node entry (the activation block is allocated from the entry region, so
it must be node-entry context — see the `sp_begin` comment), reset per
request via `sp_request_reset`, and run inferences through `sp_run` (which
resets the inference arena per run and returns an owned copy of the
output). `sp_end` closes the request coll arena on shutdown.

## Gates — two tiers

1. **Library conformance** (in-repo, fast): `tests/saltpetre_conformance.rs`
   compiles `tests/saltpetre/conformance.bv` and runs the tiny ONNX corpus
   (`tests/fixtures/saltpetre/`, ORT-generated goldens) at atol/rtol 1e-3 —
   conv accumulation order differs from ORT by a few ulps, so this tier is
   tolerant parity by policy, matching the kiosk differential harness.
2. **Kiosk acceptance** (external capability meter, PTL): real models
   (YuNet f32/int8, ResNet, Silero VAD) against onnxruntime via
   `briev-kiosk-avatar/harness/ml_parity.py` — unchanged; it gates kiosk
   PRs against the library.

## Consumers

`briev-kiosk-avatar` imports `saltpetre/…` (daemon + its test programs).
New consumers should treat `session.bv` as the entry surface; module
functions are the escape hatch for library authors.

## History

Extracted verbatim from `briev-kiosk-avatar/briev/ml/` on 2026-11-12
(plan: `docs/plans/2026-11-12-saltpetre-ml-library.md`). Op additions
continue through the kiosk acceptance ladder first, then graduate here with
a conformance fixture.
