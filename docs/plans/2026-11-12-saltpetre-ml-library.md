# Saltpetre — the Briev ML library (2026-11-12)

## Decision (discussion record)

Promote the kiosk's `briev/ml/*.bv` stack into a co-located Briev library,
`lib/saltpetre/`. Full v0: tensor/proto/onnx/exec/frame/yunet move verbatim;
a new `session.bv` owns the arena lifecycle (library owns arenas — the
daemon shrinks to RPC glue). Tiny conformance fixtures move into
`tests/fixtures/saltpetre/` and gate in the compiler repo; the kiosk's
differential-vs-ORT acceptance harness stays kiosk-side (two-tier gates:
library conformance fast/in-repo, kiosk acceptance real-models/PTL).

Doctrine fit: the modules are pure `.bv` (zero compiler knowledge), so this
is Rules 14/24 in action — temporal machinery (ONNX shapes, op shapes,
format readers) lives in library-land; the eternal machinery (ownership
proofs, arena, escape facts) stays in the compiler. Saltpetre is also the
concrete vehicle for the locked OpenVINO-retire decision (ONNX stays the
model format; Briev executes the graph; VITRIOL remains the only sanctioned
external runtime).

Rejected alternatives: separate repo + submodules from day one (friction
with the compiler-pin model; split when a second consumer appears);
core-only v0 (frontends are already differential-tested — moving them too
costs little and makes the boundary honest).

## v0 layout

```
lib/saltpetre/
  tensor.bv      Tensor, ml_arena_* internals (verbatim move)
  proto.bv       ONNX protobuf reader (verbatim)
  onnx.bv        graph model + external data (verbatim)
  exec.bv        op executor; the op ledger IS the kiosk parity ledger
  frame.bv       image decode → tensor frontend (verbatim)
  yunet.bv       YuNet detection decoder — temporal, Rule-23-safe: it lives
                 here, in library-land, not in the compiler
  session.bv     NEW: sp_begin/sp_request_reset/sp_run/sp_end — owns the
                 tensor-arena block + the request coll arena
tests/fixtures/saltpetre/   tiny ONNX corpus (13 files, 56 KB)
tests/saltpetre/conformance.bv   in-repo gate program
tests/saltpetre_conformance.rs   compile+run gate (mirrors kiosk ml_smoke)
```

## Session contract (library owns arenas)

- `sp_begin(tensor_cap, coll_cap)` — `Alloc#` the tensor block,
  `ml_arena_install`, `coll_arena_begin`. Once, at service/node start.
- `sp_request_reset()` — `ml_arena_reset` + `coll_arena_reset`. Per request.
- `sp_run(graph, inputs, output_count)` — `ml_arena_reset` (per-inference
  valve), `execute_env`, copy the requested outputs out of the request
  arena. Replaces the caller's per-detect resets.
- `sp_end()` — `coll_arena_end` + disarm. On shutdown.

Graph caching stays kiosk-side for v0 (serving state); a generalizing
Session arrives later.

## Steps

1. **Probe** import resolution for relative `./x.bv` inside `lib/` before
   moving anything (fallback: `"saltpetre/x.bv"`-style paths).
2. Move modules; add `session.bv`; copy fixtures; write the conformance
   program + Rust gate; docs (`docs/architecture/saltpetre.md`, INDEX).
3. briev-lang gates: `cargo test --lib` + the new gate; commit; reconcile
   with main; push.
4. Kiosk: imports → `"saltpetre/…"`; delete `briev/ml/`; daemon arena
   choreography (5 sites) → `sp_begin`/`sp_request_reset`; local gates
   (ml_smoke, native smoke, VAD parity, dashboard rbv check, 200-call
   soak ≈171 MB flat); pin bump; push; acceptance CI.
5. Record §18 in the memory plan + kiosk progress.

## Risks

- Relative-import resolution inside the stdlib dir (probe first).
- `Alloc#` storage strategy for the tensor block in `sp_begin` — must land
  malloc, not arena/stack aliasing its own backing; checked in the first IR.
- `tests/ml/*.bv` env paths (`models/onnx/...`) unchanged — imports only.
