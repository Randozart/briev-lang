# Two-word enum by-value return ABI

**Date:** 2026-10-29
**Status:** BLOCKED — boundary ABI experiments refuted; full value representation or rebaseline required
**Scope:** LLVM enum-return lifetime and the `ws_frame` baseline regression.
**Parent:** `docs/plans/2026-10-28-enum-return-escape-heap.md`

## Problem

`retain_enum_return` must copy every returned enum image into the
caller-owned `%enum_out` slot. Removing the copy restores `ws_frame` to
baseline but crashes `server_protocol_gate`/`server_loop_gate`; narrowing the
copy, heap-promoting escape locals, and forwarding `%enum_out` all failed
performance or correctness. The current safe copy leaves `ws_frame` at a
locked-baseline regression and blocks `v0.1.0-preview`.

## Decision

Return **two-word enums by value** as `{ i64, i64 }`.

Most `Result<T, E>` values, including the WebSocket decode chain, are exactly
tag + payload handle. A by-value aggregate removes the returned-image lifetime
problem without changing large enums.

Large/multi-word enums keep the existing sret + `retain_enum_return` ABI.

## Why not the rejected alternatives

- **No copy:** fast but returns frame-owned images (`server_protocol_gate`
  crash).
- **Heap-promote escaping locals:** correct shape but slower than copying.
- **Full `%enum_out` forwarding:** distinct live images alias one destination
  (server gates failed).
- **Call-only forwarding:** improved absolute time but missed acceptance.

## Phase 0 result — REFUTED

The hot family (`ws_decode`, `ws_decode_mask`, `ws_min_len16/64`,
`ws_ext_len16/64`, `ws_mask_key_at`) was hand-transformed on the real
`ws_frame.ll` to return `{ i64, i64 }`, with the hidden `%enum_out` removed,
all callers storing the aggregate into local slots, and every retain/copy
region deleted.

Results at the same workload/output (`0016384`, MATCH):

- current reference: ~0.044–0.051 s;
- **no-copy control** (compiler experiment): ~0.012–0.013 s;
- by-value experiment: ~0.040–0.048 s.

The ABI change removed the explicit copies but only recovered ~10–15%, not
the 3–4x. Aggregate return/storage does not let LLVM eliminate the cost the
way the (unsafe) no-copy control does. Per the Phase 0 gate, the by-value ABI
is **not** implemented.

Conclusion: the copy is dominant, but moving it into aggregate materialization
does not solve it. The next candidate is return provenance/liveness analysis
that proves a returned image already lives in the caller-owned `%enum_out`
(or heap), so the existing sret ABI can skip the copy safely.

## Follow-up provenance result — no performance win

Returned-local destination forwarding was re-tested after the separate enum
retention bug was fixed:

- `server_loop_gate` passes (`upgrade/fragmented/owner/ping-pong/...` all true);
- `ws_frame` remains ~0.047s, unchanged from the copying path.

Forcing the existing heap-boxed enum ABI also measured ~0.045s.

The safe no-copy control (0.012s) returns a pointer to an `alloca` in the
callee's match-arm block; the caller reads it after return. That is a dangling
stack pointer even though ASan's use-after-return instrumentation did not flag
it in this build. It is not a valid performance target.

**Conclusion:** boundary-only by-value, forwarding, and boxed ABI all fail.
Matching the old baseline requires representing small enum values as SSA
aggregates end-to-end, or accepting a corrected-ABI rebaseline. No compiler
code was changed by this experiment.

## Phase 0 — Actual-IR proof

Before compiler edits:

1. Generate `ws_frame.ll` with the current release compiler.
2. Identify the hot two-word enum-returning family.
3. Hand-transform only that family:
   - return `{ i64, i64 }`;
   - remove the hidden `%enum_out`;
   - callers store the aggregate in a local slot and use its address as the
     enum handle.
4. Link with the exact benchmark command.
5. Verify output equality at a BOUND crossing a print boundary.
6. Interleave reference/experiment runs.

Proceed only if:

- output is correct;
- verifier/link succeed;
- `ws_frame` is ≤1.10x locked baseline.

Otherwise stop and revise.

## Phase 1 — ABI classification

In `src/analysis/enum_repr.rs` classify eligible defns:

- single enum output;
- image width == 2 i64 words;
- no FFI/C ABI exposure;
- compatible with existing sret/boxed classification.

Expose on `EnumRepresentationPlan`; backend consumes it.

## Phase 2 — Backend emission

### Value2 definition

- `define { i64, i64 } @<name>$value(ptr %state, params...)`
- `term <handle>` loads the two image words and returns the aggregate.
- No `%enum_out`; no `retain_enum_return`.

### Export wrapper

Preserve the existing public symbol and hidden-out ABI:

- wrapper calls `@<name>$value`;
- stores the aggregate into `%enum_out`;
- returns the old handle ABI.

JS/C consumers see no change.

### Call sites

- call `{ i64, i64 }`;
- store the aggregate into one per-call-site entry slot;
- use the slot address as the enum handle.

The slot is per call site, not per loop iteration.

### Ownership boundaries

Audit every escaping boundary and copy with `own_enum_value` where the result
can outlive the call-site slot:

- struct fields;
- list insert/append;
- state stores;
- match bindings that escape;
- results passed into another aggregate.

Add tests for `List<Result<...>>` built in a loop and two live results.

## Phase 3 — Export/FFI

- exported enum defns keep the old ABI through wrappers;
- FFI/cross-language enums stay on the existing ABI;
- add an IR test for wrapper shape.

## Phase 4 — Gates

New:

- Value2 signature/return;
- call-site aggregate store;
- export wrapper;
- large enum stays sret;
- ownership boundaries;
- simultaneous live results.

Existing:

- `enum_local_return`, nested enum lifetime;
- `server_protocol_gate`, full `server_loop_gate`;
- WebSocket codec, JSON parse;
- `cargo test --all`;
- full determinism, effect-shape;
- all recommender PostgreSQL gates;
- fake-peer browser twice;
- I11.

## Phase 5 — Performance

- `compare_baseline.sh ws_frame` ≤1.10x.
- Full runtime suite MATCH.
- Inspect IR: hot family returns `{ i64, i64 }`, no `ret_copy` in the hot
  chain, no new decode-path malloc.
- Run `ws_frame_codec_parity` and a real decode probe.

## Phase 6 — Docs/release

Update:

- `docs/architecture/backend-architecture.md` enum ABI split;
- `BUGS.md` rejected approaches + resolution;
- baseline results with IR/A-B evidence;
- readiness ledger.

Do not tag `v0.1.0-preview` until `ws_frame` passes.

## Fallback

- Extend by-value to all small enum widths first; or
- implement destination forwarding with a real liveness proof.

Never reintroduce unconditional copy removal or unsafe `%enum_out`
forwarding.
