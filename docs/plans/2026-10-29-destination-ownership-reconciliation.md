# Destination ownership reconciliation

**Date:** 2026-10-29
**Status:** ACTIVE — planning/reconciliation gate
**Scope:** enum, object, composite, loop-carried, and ABI value destinations in
the LLVM backend.
**Implementation target:** LLVM native + wasm32 Tier 1; other backends remain
conservative.
**Supersedes:** the ad-hoc `BRIEV_ENUM_VALUE2` ObjectDestination implementation
strategy.
**Reconciles:** small-sum unboxing, nested enum layout, carrier slots,
tail-loop lowering, ownership algebra, boundary ownership, P2-1/P2-2, and the
2026-10-29 enum value model experiments.

This plan does not add source syntax. It replaces scattered backend metadata
with one frontend-derived destination/ownership plan and one lowering contract.

## 1. Why this plan exists

Recent work proved two facts:

1. The dominant `ws_frame` cost is not the two-word enum image itself. It is
   construction and lifetime handling of the boxed `WsFrame` object payload.
2. Hand-threaded destination metadata is unsound. The gated
   ObjectDestination slice repeatedly produced aliasing/lifetime crashes after
   small control-flow changes.

The repository already contains the correct conceptual lineage:

- `2026-10-21-small-sum-unboxing.md` defined
  `InlineLocal` / `ReturnedSmall` / `Boxed`.
- `2026-10-21-nested-enum-layout.md` derived concrete layouts and B3 safety
  rules.
- `2026-10-21-nested-carrier-slots.md` solved loop/recursive ownership with a
  stable carrier and explicit copy.
- `2026-10-23-tail-recursion-loop-lowering.md` made that carrier-gated and
  loop-based where possible.
- `2026-08-20-ownership-algebra-phase9.md` designed frontend-derived ownership
  constraints consumed by the backend.
- `2026-08-31-boundary-ownership-inference.md` built an ownership lattice and
  transitive propagation for ABI boundaries.
- `2026-10-20-open-work-backlog.md` ranked P2-1 small-sum unboxing and P2-2
  inlining/scalar replacement as the performance sequence.

This plan reconciles those threads. It is not a new heuristic.

## 2. Definitions

- **Destination**: storage that receives a constructed or copied value.
  Examples: SSA value, caller return slot, local alloca, stable carrier,
  state cell, collection element, heap box.
- **Owner**: the program extent entitled to read/overwrite a destination.
- **Escape**: a value live beyond its current destination's owner.
- **Overlap**: two live values may reference the same bytes.
- **Carrier**: destination whose lifetime spans loop iterations or recursive
  calls.
- **Materialization**: conversion from SSA/destination form to memory.
- **Boxed**: heap-owned canonical fallback with a stable handle.

Safety invariant: no destination may be read after its owner ends. If a value
escapes, the compiler must copy it into a destination owned by the consumer or
use the canonical boxed form.

## 3. Decision summary

1. **One frontend analysis owns destination decisions.**
   The backend consumes `ValueDestinationPlan`; it does not infer ownership from
   register names or local sets.

2. **Representations are proof-selected.**
   Scalar SSA, caller destination, stable carrier, scalar-replaced object, and
   boxed fallback are all legal. Unknown means boxed/copy.

3. **Every representation crossing is explicit.**
   SSA → destination, destination → boxed, boxed → SSA, carrier → aggregate,
   aggregate → carrier, and ABI conversions use named lowering helpers.

4. **Copies are proof-selected, not forbidden.**
   If source/destination overlap, live ranges overlap, or ownership is
   ambiguous, emit an explicit copy.

5. **No internal stack pointer escapes.**
   Returned/stored values use the consumer's destination, a stable carrier, or
   a boxed canonical value.

6. **Exports/FFI remain stable.**
   Public ABI conversions happen only in wrapper/lowering edges.

## 4. Representation matrix

| Class | Description | Ownership/lifetime proof required | Lowering |
|---|---|---|---|
| `ScalarValue` | two-word enum/composite with no object payload | consumed in current value graph; no escape | LLVM SSA aggregate or hidden `sret`; no heap image |
| `CallerDestination` | returned composite whose caller owns the slot | callee writes exactly this destination; no overlap with inputs | hidden destination ABI |
| `ScalarReplacedObject` | small non-escaping object | all field uses visible; no state/list/closure/FFI escape | SSA/alloca fields; no malloc |
| `StableCarrier` | loop-carried or recursive state | carrier outlives iterations; temporary/carrier non-overlap | hidden carrier + explicit backedge copy |
| `PackedMemory` | state/list/struct/tuple element | container owns complete value | concrete layout store/load |
| `Boxed` | unknown, external, recursive, oversized | stable canonical ownership | malloc image + handle |

First implementation may use `Boxed` aggressively. Correctness precedes
coverage.

## 5. Source facts already available

The analysis must reuse, not recreate:

- `EnumLayoutPlanner`: concrete image words, payload words, depth cap.
- `expected_enum_type` / `item_return_type`: concrete construction context.
- `TailLoopPlan`: direct self-tail recursion eligibility.
- `TailCarrierPlan`: loop-carried nested state.
- `TransitionGraph` / reactor order: tick and loop lifetime boundaries.
- `global_lifetime`: proven last consumer/free scheduling.
- `defn_liveness`: emitted ABI surface.
- `boundary_ownership`: export/FFI ownership lattice.
- Future Phase 9 constraints: `borrow`, `consume`, `owned`, `shared`,
  `borrowed<source>`.

## 6. Frontend analysis

### 6.1 New module

Add `src/analysis/destination_ownership.rs`.

It produces:

```rust
pub enum DestinationClass {
    ScalarValue,
    CallerDestination,
    ScalarReplacedObject,
    StableCarrier,
    PackedMemory,
    Boxed,
}

pub struct ValueDestinationFact {
    pub value: ValueId,
    pub ty: Type,
    pub class: DestinationClass,
    pub origin: ValueOrigin,
    pub escapes: EscapeSet,
    pub may_overlap: bool,
    pub requires_copy_at: BTreeSet<ProgramPoint>,
    pub owner: OwnerScope,
}

pub struct DestinationOwnershipPlan {
    pub values: BTreeMap<ValueId, ValueDestinationFact>,
    pub calls: BTreeMap<CallSiteId, CallDestinationFact>,
    pub loops: BTreeMap<LoopId, LoopDestinationFact>,
    pub boundaries: BTreeMap<BoundaryId, BoundaryDestinationFact>,
}
```

Exact IDs may differ during implementation, but the plan must be deterministic,
serializable for diagnostics, and stored in `AnalysisResults`.

### 6.2 Value graph

Build a graph over semantic values, not LLVM registers:

- origins: literal, parameter, call result, state load, collection element,
  field load, constructor, match arm, phi/join;
- operations: move, borrow, copy, construct, project, store;
- sinks: local use, return, state store, collection insert, field store, call
  argument, closure capture, loop backedge, export/FFI.

Each `let`, parameter, temporary, match arm, and loop variable gets a stable
semantic value ID. Environments map source names to IDs.

### 6.3 Abstract interpretation

Run statement-level abstract interpretation:

1. Seed parameters and state loads with conservative facts.
2. Walk each defn/txn body in program order.
3. Join facts at CFG merges.
4. Iterate loops to fixed point.
5. Summarize each call.

The lattice meet is conservative:

- same class + non-overlap → keep;
- different classes → boxed/copy;
- unknown escape → boxed/copy;
- recursive/external boundary → canonical ownership.

### 6.4 Call summaries

For each internal callable:

- parameter obligations: `borrow`, `consume`, `owned`, `shared`, unknown;
- result destination behavior: writes caller destination, returns SSA, boxes;
- escape summary: which arguments/results survive the call;
- overlap summary: result may/may not alias arguments;
- recursion summary: direct/mutual/none.

Conservative default:

- unknown callee consumes a copy;
- unknown result boxes;
- unknown alias requires copy.

### 6.5 Destination rules

- Constructor into destination `D`: write tag/payload/fields into `D`.
- Call result into destination `D`: pass `D` to a callee that is proven to
  write it; otherwise call then copy into `D`.
- Match arms:
  - all arms write same `D` → propagate `D`;
  - distinct destinations → allocate join destination and copy arm results;
  - unknown arm → boxed/join destination.
- Return local `L` through caller destination `C`:
  - `L == C` → no copy;
  - `L != C`, non-overlap → copy `L → C`;
  - overlap/unknown → materialize canonical boxed value.
- State/list/object store:
  - proven complete value → concrete store/copy;
  - unknown/local temporary → copy into container-owned storage.
- Loop backedge:
  - live values copy into stable carriers;
  - temporary destination must not survive the iteration.
- Export/FFI:
  - canonical ABI conversion at boundary;
  - ownership from boundary analysis or explicit Phase 9 declaration.

### 6.6 Alias/overlap

Destination forwarding is allowed only when:

- the destination does not alias any input live across the call;
- the callee summary proves it writes the complete destination;
- no other live value reads the old destination contents during construction;
- the destination owner covers all uses.

If any check fails, allocate a fresh destination and insert an explicit copy at
the proven transfer point.

## 7. LLVM lowering

### 7.1 Shared lowering helpers

Add one module, likely `src/backend/llvm/value_lowering.rs`, exposing:

- `store_value_to_destination`;
- `copy_value`;
- `materialize_box`;
- `unbox_value`;
- `create_caller_destination`;
- `create_stable_carrier`;
- `project_enum_tag_payload`;
- `project_object_field`.

All representation conversions go through these helpers. No local ad-hoc
`ptrtoint`/`inttoptr`/memcpy sequences for destination decisions.

### 7.2 ScalarValue

For two-word enums/composites with no object payload:

- local flow: SSA `{ tag, payload }`;
- internal return: existing hidden `sret` or aggregate ABI selected by one
  deterministic rule;
- constructors write the destination;
- match uses `extractvalue`/slot loads.

This slice must preserve `server_protocol_gate`, `server_loop_gate`,
`ws_frame_codec_parity`, and all current tests.

### 7.3 Small object scalar replacement

For small non-escaping objects:

- replace malloc image with local alloca/SSA fields;
- field accesses become direct SSA/GEP operations;
- if the object later escapes, materialize into the consumer destination;
- if proven last-use and scheduler-owned, no leak path is invented;
- if unknown, retain boxed.

This consumes P2-2 and is the intended answer to the `WsFrame` allocation cost.

### 7.4 Returned composite destinations

For proven internal call/return chains:

- caller allocates or forwards the destination;
- callee writes it on every returned path;
- caller consumes it before the owner ends;
- if the result escapes, copy into a longer-lived destination.

Destination forwarding may cross multiple helpers only when call summaries
prove the chain. The old conservative filter remains valid: unknown chains box.

### 7.5 Loops and recursion

Loop-carried values use `StableCarrier`:

- carrier allocated by loop/initial caller;
- temporary result and carrier do not alias;
- backedge copies temporary → carrier;
- Err/unchanged values may pass carrier directly;
- recursive self-call passes the same carrier.

This preserves the nested-carrier and tail-loop rules.

### 7.6 State/list/object containers

Container stores use `PackedMemory` or `Boxed`:

- concrete layout store when the value is complete and container-owned;
- boxed canonical fallback when layout/ownership is unknown;
- no store of a local destination pointer.

State layout changes are a separate slice after value correctness.

### 7.7 ABI edges

Exported and FFI functions keep the current public ABI:

- internal aggregate implementation;
- wrapper boxes/unboxes and converts to the public form;
- boundary ownership comes from `boundary_ownership` plus Phase 9 overrides.

## 8. Implementation phases

### Phase R0 — reconciliation and freeze

- [ ] Link this plan from `docs/plans/INDEX.md`.
- [ ] Mark the enum value model plan as reconciled by this plan.
- [ ] Preserve all failed experiment patches/records.
- [ ] Add a regression inventory:
  - B3 passthrough failure;
  - carrier alias failure;
  - ObjectDestination alias failure;
  - ws_frame no-copy UB control;
  - fake-peer snapshot OOB;
  - callable-txn tail-state leak.
- [ ] No compiler behavior change.

**Gate:** docs review confirms every prior invariant is represented here.

### Phase R1 — semantic value IDs and facts

- [ ] Add stable semantic value IDs for params, lets, temps, match results,
  loop variables, and call results.
- [ ] Build origin/use/sink facts.
- [ ] Add call summaries for internal defns.
- [ ] Add conservative escape/overlap facts.
- [ ] Add diagnostics (`BRIEV_DIAG_DEST=1`) showing class, owner, escapes,
  copies, and reason.
- [ ] No codegen change.

Tests:

- direct constructor return;
- match-of-calls return;
- two live results;
- state/list/field store;
- loop-carried value;
- recursive carrier;
- pass-through call chain;
- unknown call;
- export/FFI.

**Gate:** analysis tests prove known-safe shapes and conservatively reject
known-unsafe shapes.

### Phase R2 — destination lowering skeleton

- [ ] Add shared lowering helpers.
- [ ] Route existing enum sret construction/match through them.
- [ ] Delete reliance on scattered destination metadata from the failed patch.
- [ ] Preserve default IR except planned mechanical changes.
- [ ] Run full lib/tier-1/determinism/effect gates.

**Gate:** all current suites green; ws/FSM/SPSC output parity.

### Phase R3 — scalar destination correctness

- [ ] Represent scalar enum results through the destination plan.
- [ ] Prove/require destination forwarding only on non-overlap chains.
- [ ] Copy on overlap/escape.
- [ ] Remove `retain_enum_return` only for plan-proven paths.
- [ ] Keep boxed fallback for unknown paths.

Tests:

- `ws_decode → ws_decode_mask`;
- local `Ok` pass-through;
- two live `Result` values;
- match result returned;
- match result stored;
- direct constructor return.

**Gate:** server protocol/loop gates, ws codec parity, full lib, no UB under
ASan stack-use-after-return where available.

### Phase R4 — object scalar replacement

- [ ] Add small non-escaping object classification.
- [ ] Lower malloc images to SSA/alloca fields for plan-proven objects.
- [ ] Materialize at escape.
- [ ] Keep boxed path for unknown escape.
- [ ] Benchmark ws_frame.

**Gate:**

- `ws_frame` output exact;
- no eligible hot-path `malloc(34)`;
- `ws_frame` ≤1.10x corrected baseline or explicitly approved;
- all runtime outputs MATCH.

### Phase R5 — carriers and loops

- [ ] Replace carrier heuristics with destination-plan carriers.
- [ ] Preserve FSM loop correctness at BOUND `0..8`, `1M`, `50M`.
- [ ] Verify temporary/carrier non-overlap.
- [ ] Ensure tail-loop lowering remains carrier-safe.

**Gate:** FSM 0 enum mallocs, no stack growth, SPSC/ws unchanged, all MATCH.

### Phase R6 — ABI/container integration

- [ ] Route export/FFI through boundary ownership wrappers.
- [ ] Route state/list/object stores through the plan.
- [ ] Add wasm32 width/layout tests.
- [ ] Add interpreter/backend parity cases for composite values.

**Gate:** all app/server/browser gates plus parity corpus.

### Phase R7 — cleanup and measurement

- [ ] Remove dead `InlineLocal`/sret special cases replaced by the plan.
- [ ] Remove rejected experiment scaffolding.
- [ ] Run full benchmark suite.
- [ ] Publish before/after table.
- [ ] Update backend architecture and AGENTS reference.

**Gate:** release readiness only after every phase gate is green.

## 9. Tests and gates

### Correctness fixtures

- direct enum constructor return;
- local constructor consumed by match;
- nested `Result<Result<T,E>,E>`;
- `Result<WsFrame,String>` chain;
- two live results across another call;
- result stored in state;
- result stored in list;
- result stored in object field;
- result captured by closure (boxed fallback unless proven);
- loop-carried enum state;
- direct recursive carrier;
- recursive/depth-capped enum;
- tuple/multi-payload variant;
- exported enum/composite function;
- wasm32 state/list/object fixture.

### Execution gates

- `cargo test --lib`;
- `cargo test --all`;
- tier-1 fixtures;
- `browser_pending_peer.mjs`;
- recommender PostgreSQL/server suites;
- I11 where available;
- FSM BOUND `0..8`, `1M`, `50M`;
- ws_frame output and codec parity;
- SPSC parity;
- full runtime suite MATCH.

### Analysis gates

- deterministic `ValueDestinationPlan`;
- diagnostics expose class/reason;
- every unknown/boxed fallback has a reason;
- no destination forwarding without non-overlap proof;
- no internal destination pointer escapes;
- no new `ptrtoint`/`inttoptr` outside shared helpers for this lane.

### Performance gates

- actual `-O3 -flto` linked binaries;
- interleaved reference/experiment/C timing;
- ws_frame, FSM, SPSC, arena_bump, recommender;
- no benchmark regression accepted as noise without A/B;
- target C parity or an explicit recorded gap.

## 10. Risks and controls

| Risk | Control |
|---|---|
| AST value IDs are unstable | Define stable semantic IDs tied to statement/expr paths + defn identity; test determinism |
| Call summaries too conservative | Start boxed; improve only with measured benchmark evidence |
| Destination forwarding aliases inputs | Require non-overlap proof; otherwise fresh destination + copy |
| Object scalar replacement changes observable address semantics | Only apply to objects whose address is not observable/exported |
| Loop carriers recurse unexpectedly | Reuse `TailCarrierPlan`/`TailLoopPlan` gates; unknown remains boxed |
| Payload ownership ambiguous | Phase 9 conservative defaults; unknown boxes/copies |
| wasm layout mismatch | Test state/list/object ABI on wasm32 before enabling |
| Interpreter divergence | Add parity cases before changing backend lowering |
| Regression hidden by env gate | Default gates must run both fallback and enabled paths where safe |
| Scope creep into GC | Explicit non-goal; ownership is compiler-proven or boxed |

## 11. Non-goals

- No garbage collector.
- No source-level borrow annotations in this slice.
- No new strategy keyword.
- No app-specific lowering.
- No broad tail-loop enablement without carrier proof.
- No public ABI break.
- No GPU/embedded-specific representation in the first implementation.

## 12. Documentation updates

Same-commit updates required for implementation phases:

- `docs/architecture/backend-architecture.md`: destination/value lowering.
- `docs/architecture/llvm-memory-management.md`: owner/lifetime rules.
- `docs/architecture/layout-contracts.md`: concrete layout consumption.
- `docs/architecture/derivation-not-recognition.md`: proofs feeding slot reuse.
- `docs/architecture/agent-reference.md`: backend context contract.
- `docs/plans/INDEX.md`: active lane link.
- `docs/plans/2026-10-29-enum-value-model.md`: mark reconciled, preserve evidence.
- `BUGS.md`: failed experiment references and fixed regressions.

## 13. Rollback

Each phase is independently revertible.

- R1 is analysis-only.
- R2 helper routing must remain output-equivalent or be reverted.
- R3/R4 are gated behind explicit plan facts; unknown falls back to current ABI.
- Failed object/carrier slices remain recorded and unmerged.

Never revert by discarding uncommitted work; save and reverse-apply patches.

## 14. Decision requested

Approve this reconciliation as the replacement for ad-hoc ObjectDestination
metadata. The next implementation step is Phase R1: semantic value IDs,
destination/ownership facts, call summaries, and diagnostics with no codegen
change.
