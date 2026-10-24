# wasm32 obj-init width fix + fullstack campaign remaining work

**Date:** 2026-10-23
**Status:** bug slice COMPLETE; campaign roadmap ACTIVE
**Base:** `feat/fullstack-task-board` at `138a57c4`.
**Trigger:** `benchmarks/rbv_gate.sh` fails in wasm32 on
`tests/fixtures/obj_init.rbv`; the failure is pre-existing but blocks the
normal web regression gate used by Task Board.

## Immediate bug

`obj_init.rbv` fails `llc --filetype=obj` for wasm32 with:

```llvm
%t9  = ptrtoint ptr %t5 to i32
%t10 = inttoptr i64 %t9 to ptr
```

LLVM rejects the second instruction: `%t9` is `i32`, but the conversion asks
for `i64`.

### ABI rule

wasm32 pointer-sized values are `i32`. The backend already derives this as
`ctx.int_bits = 32`, and most paths honor it. Not every `i64` is wrong:

- **`i32` required:** obj/struct/enum heap handles, pointer-to-pointer
  conversions, pointer-sized state slots on wasm32.
- **`i64` may be intentional:** LLVM GEP indices, boxed collection/tuple
  payload words, explicit 64-bit scalar storage, documented runtime ABI words.

Therefore this is a targeted width-conversion fix, not a wholesale ABI change.

### Plan

1. **Pin the repro**
   - Build `tests/fixtures/obj_init.rbv` with `--split`.
   - Capture the failing `.ll`.
   - Add a backend IR regression keyed to the wasm32 data layout.

2. **Locate the producer**
   - Trace the registers from struct/object materialization through field
     access/state seeding.
   - Prefer the existing `emit_inttoptr` helper over another format-string
     `i64`.
   - Audit adjacent loads/stores in the same path: a pointer-sized field must
     load/store at the target pointer width, not a hardcoded `i64`.

3. **Fix narrowly**
   - Replace the offending pointer round-trip with `i{ctx.int_bits}`.
   - Preserve intentional `i64` payload words.
   - Do not change all boxed values blindly.

4. **Regression tests**
   - wasm32 IR test for `obj R { path: String; ... }`:
     - object handle conversion is `inttoptr i32`;
     - no `inttoptr i64 <i32-defined register>`;
     - `llc` object emission succeeds.
   - Existing x86_64 IR tests must remain unchanged semantically.

5. **Gates**
   - `cargo test --lib`
   - `BRIEVC=./target/release/brievc bash benchmarks/rbv_gate.sh`
   - Task Board API gate
   - Task Board frontend browser gate
   - LLVM determinism quick gate if backend IR changes

6. **Record**
   - Move the BUGS.md entry from OPEN to FIXED.
   - Include the old/new IR and the ABI rationale.

## Result (2026-10-23)

Fixed in two focused paths:

1. `emit_instance_seed_from_value`
   - obj value handles now convert at `ctx.int_bits`;
   - struct-literal members load at their declared LLVM types;
   - the instance-column store adapts explicitly.

2. boxed-instance member reads
   - handle conversion uses `ctx.int_bits`;
   - pointer-sized members load their native `ptr`, not a hardcoded `i64`.

Old IR:

```llvm
%t9  = ptrtoint ptr %t5 to i32
%t10 = inttoptr i64 %t9 to ptr
%t12 = load i64, ptr %t11
```

New IR:

```llvm
%t9  = ptrtoint ptr %t5 to i32
%t10 = inttoptr i32 %t9 to ptr
%t12 = load ptr, ptr %t11
```

Validation:

- `tests/rbv_obj_init_wasm32.rs`: PASS;
- full `benchmarks/rbv_gate.sh`: PASS, including wasm IR checks and the
  real-browser counter smoke;
- `cargo test --lib`: **3,303 passed**;
- LLVM determinism quick gate: PASS;
- Task Board API gate: PASS;
- Task Board frontend browser gate: PASS;
- full runtime suite: **all MATCH**.

BUGS.md records the defect as FIXED. The broader campaign roadmap below
remains active.

## Remaining fullstack campaign work

### A5 — frontend mutation fetch

- Extend `lib/glue/web/web.js` beyond synchronous GET.
- Add typed wrappers in `lib/std/web/fetch.bv`:
  - `http_get`
  - `http_post`
  - `http_put`
  - `http_delete`
- Return structured success/error at the host boundary.
- Wire Task Board UI to:
  - create task;
  - toggle task;
  - delete task;
  - refresh after mutation.
- Add a real Chromium CRUD gate.
- Preserve CORS behavior and assert no page/console errors.

### A6 — application hardening

- Structured JSON errors for malformed bodies and invalid routes.
- Request-size and timeout policy in the backend loop.
- Clean shutdown policy.
- WebSocket live updates after CRUD is stable.
- Frontend loading/error/empty states.
- Escape/validate titles at the app boundary.
- Keep contracts; no workarounds for compiler/stdlib defects.

### A7 — PostgreSQL wire client

Implement `lib/std/db/pg.bv` incrementally:

1. pure protocol frame encode/decode;
2. offline golden corpus;
3. live `connect + SELECT 1`;
4. simple query;
5. parameterized query;
6. Task Board persistence.

V1 authentication: trust or MD5 only. SCRAM deferred.

### Application coverage after the first app

Add at least one non-web application so validation is not browser/IO biased:

- deterministic NDJSON/JSON log analyzer;
- stdin → aggregate counts/statistics → JSON summary;
- malformed-input exit semantics;
- no network/database variance.

### Broader compiler/backlog context (not blocking fullstack)

- Fix the pre-existing wasm32 obj-init width bug (this document).
- Re-enable/repair the full `rbv_gate.sh`.
- P1 pointer provenance work remains before removing the volatile fence.
- P2 benchmark gaps remain: vectorization, arena materialization, SPSC,
  `nbody_newton_accel`, boxing/handle tax.
- P0/P1 integration-test failures remain from the open-work backlog.
- Keep the pinned carrier baseline; do not optimize without controlled A/B.

## Acceptance for this slice

- wasm32 `obj_init` builds and the regression pin lands.
- `rbv_gate.sh` passes or the only remaining failures are newly documented,
  independent bugs with repros.
- Fullstack campaign plan accurately reflects A5–A7 and later coverage.
- No app-level workaround for the compiler defect.
