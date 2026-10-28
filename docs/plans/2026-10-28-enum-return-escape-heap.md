# Enum-return escape-heap ownership

**Date:** 2026-10-28
**Status:** ACTIVE
**Scope:** LLVM enum/sret return lifetime and performance.

## Problem

The first recovery removed the unconditional enum-return image copy. That
restored `ws_frame` performance but crashed `server_protocol_gate`: a returned
local image pointed into the returning function's stack frame.

The current compromise retains every `term <identifier>` image. It is correct,
but the hot WebSocket decoder returns a local `Result` produced by an
sret/match chain. The extra full-image copy dominates the benchmark
(`compare_baseline.sh ws_frame` reported 2.48x against the locked baseline).

## Goal

Eliminate the copy for exactly the bindings whose image can be made to outlive
the frame. Keep the copy for every binding whose ownership cannot be proved.

Non-goals for this slice:

- changing the public enum ABI;
- changing payload ownership;
- optimizing non-enum returns;
- accepting a benchmark regression to land correctness.

## Proof

A local binding is escape-heap-eligible when all hold:

1. Its defining function returns an enum.
2. The binding is immutable after initialization.
3. A `term`/`tail` in the function returns that exact identifier.
4. Its initializer is constructor-shaped, an sret call, or a match of those —
   not a plain alias of another local.

For such a binding, codegen allocates the constructed/called image on the heap
while evaluating the initializer. The returned handle owns heap storage, so the
return needs no image copy.

Anything else keeps the existing safe copy. A conservative miss costs
performance; it cannot affect correctness.

## Implementation

### Analysis

Extend `src/analysis/enum_repr.rs` with a deterministic per-defn map:

```text
defn -> set(local names)
```

The pass reuses the enum/sret classification. It rejects:

- reassigned targets (`x = ...`, arrow targets, field/index writes);
- alias initializers (`let out = inner_local;`);
- bindings not directly returned;
- names shadowed in nested scopes if scope tracking is uncertain.

Expose the set on `EnumRepresentationPlan`; the LLVM driver already builds
that plan before enum lowering.

### Backend consumption

`FunctionContext` consumes the per-function escape set and a scoped flag:

- `enum_result_heap_required`.

In `emit_definition`:

- load the escape set for the current defn;
- do **not** install stack `enum_inline_slots` or `sret_call_slots` for an
  escaping binding.

In `Statement::Let`:

- if the binding is escape-eligible, set `enum_result_heap_required` around
  initializer evaluation;
- restore the prior flag afterward, including on guarded/match evaluation.

In constructor emission:

- when the flag is active, do not use an inline or `%enum_out` slot;
- allocate the image with `malloc`.

In sret-call destination selection:

- when the flag is active, choose a fresh `malloc` destination even if a stack
  `sret_call_slot` is active.

In `term`/`tail`:

- if the returned identifier is in the proven escape set, skip
  `retain_enum_return`;
- otherwise retain the existing safe copy.

## Gates

### Correctness

- New analysis unit tests:
  - direct constructor/match/sret-call return is eligible;
  - alias return is not;
  - reassigned return is not;
  - non-returned local is not.
- New backend IR tests:
  - eligible sret call gets a malloc destination;
  - eligible constructor gets a malloc image;
  - eligible return emits no `ret_copy`;
  - ineligible local return still retains/copies.
- Existing lifetime gates:
  - `enum_local_return`;
  - nested enum lifetime;
  - `server_protocol_gate`;
  - full recommender server/account/cache/worker/browser gates;
  - `cargo test --all`.

### Performance

- Inspect the `ws_frame` IR for the hot returned binding.
- Run `benchmarks/compare_baseline.sh ws_frame`.
- Acceptance: current/baseline ratio no worse than **1.10x**.
- Rerun the full runtime benchmark suite; every output must MATCH.

## Experiment log — 2026-10-28

- Analysis and tests landed. The codegen experiment was reverted before commit.
- Full escape-initializer heap promotion generated one malloc per eligible
  match arm; `ws_frame` remained ~4.1x baseline.
- Full `%enum_out` forwarding eliminated copies and mallocs but made distinct
  live images alias one destination; recommender server gates failed. Reverted.
- Sret-call-only forwarding improved absolute time (~0.061s → ~0.048s) but did
  not reach acceptance; constructor paths still forced heap allocation.
- A pure-RHS eager logical path was retained: it is semantically safe, improves
  generated CFG quality, and is covered by existing logical tests. It does not
  by itself close `ws_frame`.

Conclusion: copy elision requires a representation/ABI change or full
lifetime-aware slot allocation. The current safe identifier copy remains.

## Evidence and rollback

Record the before/after IR, A/B averages, and gate results in
`benchmarks/results/2026-10-28-v0.1-preview-baseline.md`.

If acceptance misses, keep this work only if all correctness gates pass and
record the measured ratio. The next escalation is a small by-value enum ABI
(`[N x i64]`) or explicit return-slot forwarding. Do not tag
`v0.1.0-preview` while `ws_frame` is a known regression.
