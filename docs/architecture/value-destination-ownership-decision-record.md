# Value-Destination Ownership — Architecture & Decision Record

**2026-10-30.** Status: **accepted implementation design; implementation in
progress.** This record fixes the generalized destination/ownership model for
compiler-generated value lowering. It is not a language-surface change: Briev
source syntax, contracts, visibility, and public/runtime ABIs remain unchanged.

**Execution plan:**
`docs/plans/2026-10-30-generalized-value-destination-ownership.md`

**Reconciliation and evidence:**
`docs/plans/2026-10-29-destination-ownership-reconciliation.md`

**Companions:** `derivation-not-recognition.md`,
`proof-vs-shape.md`, `layout-contracts.md`,
`llvm-memory-management.md`, `boundary-ownership` analysis,
`backend-architecture.md`, and the Phase 9 ownership-algebra lineage.

---

## 0. Decision in one sentence

**The compiler derives a destination/ownership plan from semantic values and
proofs; backend lowering consumes that plan through shared helpers; unsafe
destination reuse, hidden register-name heuristics, and unsupported LLVM
assumptions are forbidden.**

No Briev syntax is added. Unknown ownership boxes/copies. Proven ownership may
select faster internal representations.

---

## Part I — Context

### 1. Problem

The LLVM backend contains several partial lifetime systems:

- enum `%enum_out`, `%esrt`, `%einl`, and `sret` paths;
- `retain_enum_return`;
- `own_enum_value`;
- tail-loop carriers;
- object malloc and field-store specialization;
- state/list/object destination handling;
- loop-engine liveness and foreach alloca demotion;
- boundary ownership computed but not consumed by LLVM;
- allocation analysis not uniformly consumed by composite lowering.

These encode pieces of the same question:

> Where does a semantic value live, who owns it, when may it alias, and when
> must it be copied?

Because the answer was previously derived in scattered backend paths, fixes
became local and fragile. Failed experiments showed the cost:

- removing enum copies produced dangling callee-frame pointers;
- narrowing copies to identifiers restored `server_loop_gate` failures;
- ad-hoc destination metadata repeatedly produced aliasing/lifetime crashes;
- ScalarValue aggregates alone did not remove the dominant `WsFrame` cost;
- a hand destination-reuse prototype reached old-baseline performance but was
  incorrect without complete ownership semantics.

The problem is therefore not enum-specific. It is a **generalized value
destination and ownership problem**.

### 2. Existing foundation

Phase R1 landed `AnalysisResults.destination_ownership`:

- stable semantic value IDs;
- origins, uses, sinks, escapes, and copy points;
- callable summaries;
- call-site obligations;
- conservative destination classes;
- deterministic diagnostics.

Phase D1 completed the generalized schema on 2026-10-30: value shapes,
callable/parameter/result/hidden-ABI plans, call argument/result plans, loop
plans with `LoopCarrier` facts, boundary plans, proof/fallback reasons, and
`export` wrapper indexing. The production plan is constructed after all R1
facts are aggregated. It is still analysis-only; the backend does not consume
it yet.

Other analyses already contain necessary inputs:

- `EnumLayoutPlanner`;
- `EnumRepresentationPlan`;
- `TailLoopPlan`;
- `TailCarrierPlan`;
- `global_lifetime`;
- `boundary_ownership`;
- `defn_liveness`;
- transition/reactor graphs;
- allocation analysis;
- spawn/SoA plans;
- loop-engine liveness.

This record defines the contract that reconciles them.

---

## Part II — Model

### 3. Semantic values and physical destinations

A Briev semantic value is independent of its physical destination. A
destination is the compiler-selected storage that receives a materialized
value.

Destinations include:

- SSA scalar/aggregate;
- caller-owned result slot;
- local alloca/scalar-replaced object;
- stable loop/recursive carrier;
- state cell;
- collection element;
- object field;
- heap box;
- ABI boundary buffer.

A destination may be read or overwritten only inside its owner lifetime. No
destination pointer may escape to a consumer that outlives that owner unless
the value is copied into a destination owned by the consumer.

### 4. Value shape

`ValueShape` is derived from concrete type/layout facts:

- scalar;
- enum;
- object;
- tuple;
- collection;
- slice/view;
- closure;
- task;
- reference;
- opaque/external.

Shape records payload words, inline words, boxed fallback, recursion, and
external ABI status. It is target-independent; target width and ABI conversion
are applied by lowering.

Shape is derived from concrete layout facts, never from Briev type names in
backend match arms.

### 5. Ownership classes

| Class | Meaning |
|---|---|
| `Owned` | this value/destination owns its storage |
| `Borrowed` | source lifetime dominates all uses |
| `Consumed` | last use consumes the value |
| `Shared` | proof permits shared observation without mutation |
| `Unknown` | safe fallback; box/copy |

Missing proof is `Unknown`. `Unknown` never selects destination forwarding,
zero-copy, or scalar replacement.

### 6. Destination classes

| Destination class | Meaning | Requires proof |
|---|---|---|
| `ScalarValue` | scalar/small aggregate in SSA | no escape; no object-payload escape |
| `CallerDestination` | helper writes caller-owned result slot | complete write, lifetime, non-overlap |
| `ScalarReplacedObject` | object becomes local fields | no field address escape |
| `StableCarrier` | loop/recursive lifetime destination | carrier outlives live range; non-overlap |
| `PackedMemory` | concrete inline layout | container owns complete value |
| `ZeroCopyView` | borrow/view | lifetime dominates use; no mutation overlap |
| `Boxed` | canonical heap-owned fallback | always legal |
| `Unknown` | proof unavailable | always box/copy |

### 7. Obligations

Parameter obligations:

- `Value`;
- `Borrow`;
- `Consume`;
- `Owned`;
- `Shared`;
- `CopyFromBorrow`;
- `Unknown`.

Result destinations:

- `Void`;
- `SsaValue`;
- `CallerDestination`;
- `Boxed`;
- `Unknown`.

Unknown parameter/result obligations are conservative. They may be refined by
callable summaries, but never by emit-time guesses.

---

## Part III — Decisions

### D1. One frontend plan owns destination decisions — DECIDED

All destination, ownership, alias, escape, copy, carrier, and ABI-boundary
decisions come from `AnalysisResults.destination_ownership`.

Backend lowering consumes the plan. It must not infer ownership from:

- LLVM register names;
- local destination maps;
- instruction context;
- function name patterns;
- Briev type-name match arms;
- algorithm structure.

### D2. Stable semantic value IDs — DECIDED

A semantic value ID is independent of LLVM register numbering.

It must include:

- callable identity;
- value kind;
- canonical source path;
- lexical/iteration scope identity;
- incarnation where a binding is reassigned.

Value IDs must be deterministic and stable across LLVM register renaming. They
are not required to survive arbitrary AST rewrites, but each rewrite must map
old facts to new IDs or invalidate the affected facts conservatively.

### D3. Proof-selected representation — DECIDED

Faster representations are selected only with proof.

| Representation | Required proof |
|---|---|
| `ScalarValue` | no escape; no object-payload escape |
| `CallerDestination` | complete write; lifetime covers all uses; no input overlap |
| `ScalarReplacedObject` | no field-address escape; all uses visible |
| `StableCarrier` | carrier outlives live range; non-overlap with temporary |
| `PackedMemory` | container owns complete value |
| `ZeroCopyView` | lifetime dominates use; no mutation overlap |
| `Boxed` | always legal |
| `Unknown` | always box/copy |

No source syntax is required or permitted to request these.

### D4. Unknown proof boxes/copies — DECIDED

Unknown or partial ownership is represented as `Boxed` or explicit copy.

Missing proof costs performance only. It cannot affect correctness. The
fallback reason must be retained by diagnostics/reports.

### D5. No internal stack pointer escape — DECIDED

A destination pointer may not be consumed after its owner lifetime ends.

If a value escapes, lowering must either:

1. write it into a destination owned by the consumer;
2. copy it into a stable carrier;
3. materialize a boxed value; or
4. use a proven borrowed/zero-copy form whose lifetime dominates every use.

### D6. Every representation crossing is explicit — DECIDED

Conversions must go through shared lowering helpers:

- construct into destination;
- copy;
- materialize box;
- unbox;
- load/store field;
- load/store collection element;
- project enum tag/payload;
- create caller destination;
- create stable carrier;
- convert internal ↔ ABI.

No emitter may scatter ownership-specific `ptrtoint`, `inttoptr`, `memcpy`, or
destination selection decisions.

### D7. Atomic callable ABI plans — DECIDED

A callable's result/parameter/hidden-ABI plan is selected once and consumed by:

1. the function signature;
2. parameter binding;
3. body return paths;
4. every call site.

No callable may mix legacy and destination-plan prototypes. No call site may
assume a prototype absent from the callee plan.

### D8. Public and runtime ABI remain frozen — DECIDED

The migration does not change:

- exported function ABI;
- C/JS runtime helper ABI;
- collection runtime block layout;
- String/Blob layout;
- closure environment ABI;
- task scheduler ABI;
- wasm JS shim protocol;
- executable observable behavior.

If a future lane changes a frozen layout, it is a separate versioned ABI lane,
not part of destination-ownership migration.

### D9. Internal helper ABI may change — DECIDED

Hidden internal parameters and result destinations may change if the callable
plan proves them and every call site is emitted atomically.

Examples:

- object-result destination;
- enum-result destination;
- loop carrier;
- scalar-replaced object return form.

These are compiler-internal, not author-visible.

### D10. Explicit copies are allowed — DECIDED

A copy is not a failure. It is the correct lowering when:

- source and destination overlap;
- live ranges overlap;
- ownership is unknown;
- value escapes its current owner;
- ABI requires a materialized form.

Only proof-backed copies may be removed.

### D11. LLVM is not replaced or fought — DECIDED

The compiler emits explicit semantics and truthful attributes. LLVM remains
responsible for:

- SSA construction;
- SROA;
- inlining;
- alias analysis;
- instruction scheduling;
- register allocation;
- target lowering.

The destination model avoids:

- phantom handles;
- hidden pointers into dead frames;
- unsupported `noalias` / `nocapture`;
- destination reuse with unknown overlap;
- metadata that blocks SROA/inlining without proof.

### D12. No new Briev syntax — DECIDED

This migration adds no source syntax.

No ownership keyword, strategy keyword, move annotation, destination
annotation, or author-facing proof marker is introduced.

If existing Phase 9 ownership declarations already carry a proof, lowering may
consume them. This design does not require new syntax.

### D13. Conservative diagnostics are mandatory — DECIDED

Every non-boxed destination records its proof. Every boxed/copy fallback
records its reason.

Diagnostics must expose:

- representation class;
- destination class;
- owner;
- escape set;
- copy obligation;
- proof kind;
- fallback reason;
- call-site destination;
- ABI conversion.

Diagnostics are compiler data, not user-facing warnings.

### D14. Backend type-name matching remains forbidden — DECIDED

Destination facts are keyed by concrete shape, ownership, effect, and proof.
Backend lowering must not redispatch on `Result`, `WsFrame`, `List`,
closure, or any source type name.

Generic shape classes may drive lowering; specific type names may not.

### D15. Boundary ownership must be consumed — DECIDED

`boundary_ownership` currently exists but LLVM lowering does not consume it.
Destination lowering must consume the boundary lattice at:

- exports;
- foreign calls;
- runtime helper boundaries;
- wasm/web view boundaries;
- task scheduler edges;
- persisted-state edges.

Public/runtime external forms remain frozen. Internal-to-boundary conversion
is explicit.

---

## Part IV — Lowering architecture

### 16. Shared lowering module — DECIDED

Add:

```text
src/backend/llvm/value_lowering.rs
```

It exposes one contract for all representation crossings.

Required operations include:

- `store_value_to_destination`;
- `copy_value`;
- `move_value`;
- `materialize_box`;
- `unbox_value`;
- `construct_enum_in_destination`;
- `construct_object_in_destination`;
- `construct_tuple_in_destination`;
- `project_enum_tag_payload`;
- `project_object_field`;
- `project_tuple_element`;
- `load_collection_element`;
- `store_collection_element`;
- `create_caller_destination`;
- `create_stable_carrier`;
- `copy_to_stable_carrier`;
- `convert_internal_to_abi`;
- `convert_abi_to_internal`.

No destination/ownership conversion may be emitted outside this module.

### 17. Atomic callable emission — REQUIRED

For each callable:

1. read its destination plan;
2. select hidden ABI parameters;
3. emit signature;
4. bind parameters from the same plan;
5. emit body using value/call plans;
6. emit return paths using the same result plan;
7. emit all call sites using the same plan.

A callable must not mix plan-derived and legacy ABI decisions. A call site must
not mix prototypes with its callee.

### 18. Compatibility mode is migration scaffolding — DECIDED

During migration, per-callable lowering modes may exist only if:

1. the callable is non-exported;
2. the full direct/call graph is same-mode;
3. no public/runtime boundary directly observes mixed forms;
4. the mode is recorded and removable.

Compatibility mode is not the endpoint. Lane F removes it.

---

## Part V — Proof rules

### 19. Destination forwarding proof — REQUIRED

Destination forwarding is legal only when:

1. the callee writes the complete destination on every returned path;
2. destination lifetime covers every use;
3. destination does not overlap any input live across the call;
4. no other live value reads old destination contents during construction;
5. no hidden escape occurs;
6. recursion/loop interaction uses a proven carrier;
7. target widths agree;
8. ABI is internal or covered by a wrapper.

If any condition fails, allocate a fresh destination and emit an explicit copy.

### 20. Scalar replacement proof — REQUIRED

Object scalar replacement is legal only when:

1. the object is small by concrete layout;
2. no field address escapes;
3. no state/list/closure/task/FFI use escapes;
4. every field use is visible in the value graph;
5. materialization at escape is expressible.

Otherwise the object remains boxed.

### 21. Carrier proof — REQUIRED

A stable carrier is legal only when:

1. carrier lifetime spans every live iteration;
2. temporary and carrier do not overlap;
3. backedge copies preserve all live values;
4. recursive reentry does not overwrite a live outer carrier;
5. loop exit consumes the final carrier or copies it.

Unknown loop lifetime uses boxed/copy or existing conservative carriers.

### 22. Zero-copy proof — REQUIRED

A zero-copy/borrowed form is legal only when:

1. source lifetime dominates every use;
2. no mutation overlaps a read;
3. ABI accepts borrowed/zero-copy form;
4. owner is recorded.

Otherwise materialize/copy.

### 23. Proof meet is conservative — REQUIRED

| Fact A | Fact B | Meet |
|---|---|---|
| same proven class, non-overlap | same proven class, non-overlap | keep class |
| different proven classes | any | boxed/copy |
| proven | unknown | boxed/copy |
| non-overlap | maybe-overlap | maybe-overlap |
| function-local | escapes | escaped |
| internal | external | boxed/boundary |
| loop-local | loop-carried | loop-carried |

No proof may be invented. Missing proof is safe fallback.

---

## Part VI — Migration

### 24. Generalized interface first, lane migration second — DECIDED

The fact schema, lowering helpers, ABI contract, and gates cover all value
classes before backend migration.

Migration proceeds in independently gated lanes:

| Lane | Domain |
|---|---|
| A | scalar enum/`Result` |
| B | small non-escaping objects |
| C | state/collections/tuples/slices |
| D | loops/recursion/carriers |
| E | closures/tasks/ABI boundaries |
| F | cleanup/retirement |

Each lane must preserve observable behavior and external ABI.

### 25. Lane A — scalar enum/`Result` — REQUIRED

Scope:

- two-word scalar enums;
- enums boxed by concrete layout;
- constructor/match agreement;
- scalar SSA versus caller destination;
- internal sret replacement.

Non-scope:

- object payload scalar replacement;
- collection/state layout changes;
- public ABI changes.

Gate:

- `server_protocol_gate`;
- `server_loop_gate`;
- JSON round-trip;
- ws codec parity;
- all recommender server gates;
- `cargo test --lib`;
- full IR determinism.

### 26. Lane B — small object scalar replacement — REQUIRED

Scope:

- small object construction;
- field-level local representation;
- field escape/materialization;
- object argument/return destinations;
- `WsFrame`-shaped values as the first performance case.

Gate:

- exact `ws_frame` output;
- no malloc for proven eligible objects;
- corrected performance target;
- ASan/stack-use-after-return checks where available;
- full recommender/server/browser gates.

### 27. Lane C — containers/state — REQUIRED

Scope:

- state load/store;
- collection construction/insert/index;
- tuple construction/access;
- object field load/store;
- slices/views;
- boxed fallback.

Frozen runtime layouts do not change. Destination facts govern internal
representation and conversion at container edges.

Gate:

- recommender PostgreSQL/server suites;
- wasm32 state/list/object fixtures;
- collection parity;
- JSON/list/object round trips.

### 28. Lane D — loops/recursion/carriers — REQUIRED

Scope:

- foreach;
- countable loops;
- version-DAG loop fields;
- direct tail recursion;
- recursive carrier reuse;
- reactor tick lifetime.

Gate:

- FSM `BOUND 0..8`, `1M`, `50M`;
- recursive carrier fixtures;
- no stack growth;
- no per-iteration malloc for proven carriers;
- loop-engine tests.

### 29. Lane E — boundaries — REQUIRED

Scope:

- exports;
- foreign calls;
- runtime helpers;
- wasm/web views;
- task scheduler edges.

Public/runtime forms remain frozen. Internal-to-boundary conversion is explicit
and consumes boundary ownership.

Gate:

- C/JS export round trips;
- task/async suites;
- browser fake-peer and I11;
- wasm32 ABI tests;
- `--no-stdlib` parity corpora.

### 30. Lane F — retirement — REQUIRED

After all value classes are proven:

1. remove `%enum_out`, `%esrt`, `%einl`;
2. remove `retain_enum_return`;
3. remove `own_enum_value`;
4. remove old enum/object destination metadata;
5. remove backend lifetime heuristics;
6. remove compatibility mode;
7. update architecture docs and AGENTS reference;
8. run final release evidence.

---

## Part VII — Testing and evidence

### 31. Differential correctness — REQUIRED

Every migrated callable/value class must pass:

- native output comparison;
- wasm output comparison where applicable;
- state-visible result comparison;
- exit-code comparison;
- browser DOM/text comparison where relevant;
- small-bound and large-bound runs;
- JSON/list/object round trips.

### 32. IR evidence — REQUIRED

For behavior-preserving refactors:

- capture function-level IR before/after;
- expected differences are limited to destination form/copies;
- unrelated register/code-shape changes must be explained;
- byte-identical output remains required for non-lane refactors.

### 33. Sanitizer evidence — REQUIRED

Native lifetime-sensitive lanes run sanitizer checks where practical:

- ASan;
- stack-use-after-return;
- bounded and large workloads.

Every report is investigated. Reports are not ignored.

### 34. Proof audits — REQUIRED

For every non-boxed plan fact:

1. verify proof status;
2. verify destination lifetime;
3. verify overlap proof;
4. verify hidden ABI consistency;
5. verify boundary conversion;
6. verify no fallback contradiction.

### 35. Performance evidence — REQUIRED

Benchmarks use actual release link commands and interleaved A/B timing.

Minimum lanes:

- `ws_frame`;
- FSM;
- SPSC;
- arena_bump;
- recommender;
- full runtime suite.

Targets:

- exact output;
- no hot-path malloc for proven non-escaping values;
- `ws_frame` ≤ corrected safe target;
- no unexplained regression;
- every improvement linked to proof class.

---

## Part VIII — Consequences

### 36. Positive consequences

- One ownership model replaces scattered lifetime systems.
- Backend lowering becomes auditable.
- Destination forwarding becomes proof-driven.
- Object scalar replacement has a general mechanism.
- Loop carriers, enum returns, and object destinations share one contract.
- LLVM receives honest, optimizer-friendly IR.
- Unknown ownership is safely conservative.
- Future backends can consume the same target-independent facts.

### 37. Negative consequences and accepted costs

- More upfront analysis infrastructure.
- Temporary dual lowering paths during migration.
- More conservative fallbacks initially.
- Additional diagnostics and evidence obligations.
- Object scalar replacement requires a dedicated lane.
- ABI switches require atomic callable updates.
- Boundary conversion requires wrappers.

These costs are accepted because they replace repeated miscompile-prone ad-hoc
lifetime patches.

### 38. Non-goals

- No garbage collector.
- No source-level ownership syntax.
- No user strategy keywords.
- No app-specific lowering.
- No algorithm recognition.
- No public ABI break.
- No runtime C/JS layout break.
- No broad tail-loop enablement without carrier proof.
- No contract weakening.
- No GPU-specific destination lowering in this lane.

---

## Part IX — Invariants

### I1 — Lifetime invariant

No destination is read after its owner lifetime ends.

### I2 — Ownership invariant

Every value use has an owner-compatible ownership class.

### I3 — Overlap invariant

Destination forwarding requires proven non-overlap. Otherwise explicit copy.

### I4 — Representation invariant

Every representation crossing uses shared lowering helpers.

### I5 — ABI invariant

Public/runtime ABI remains frozen. Internal ABI plans are atomic per callable.

### I6 — Fallback invariant

Unknown or ambiguous ownership uses boxed/copy.

### I7 — Proof invariant

Faster internal representation requires proof. Proof absence is safe.

### I8 — Backend invariant

Backend lifetime decisions come from `AnalysisResults.destination_ownership`,
never from register names, type names, algorithm shape, or local emit context.

### I9 — Diagnostic invariant

Every destination choice and fallback has a machine-readable reason.

### I10 — Migration invariant

During migration, no callable emits mixed plan/legacy prototypes.

---

## Part X — Alternatives considered

### A. Keep enum-only destination work

Rejected. `ws_frame` proved the dominant cost is object payload lifetime, not
the two-word enum alone. Enum-only machinery would require later generalization
and another rewrite.

### B. Continue ad-hoc backend metadata

Rejected. Repeated failures showed backend-local metadata cannot safely
represent ownership, aliasing, escape, and destination lifetime.

### C. Emit two-word aggregates for all enums

Rejected as the complete solution. It did not remove object allocation and did
not recover target performance. It may remain a representation choice for
scalar values under proof.

### D. Unsafe no-copy optimization

Rejected. It returned pointers into dead frames and matched output only by
coincidence/undefined lifetime.

### E. Heap-box everything

Retained as fallback, not default. It is safe but loses proven destination and
scalar-replacement performance.

### F. Introduce user ownership syntax

Rejected for this lane. Existing facts/proofs are sufficient for internal
lowering. Any author-facing syntax would require a separate derivation-gap
proposal under the keyword admission process.

### G. Let LLVM solve everything without destination facts

Rejected. LLVM cannot recover missing ownership/lifetime semantics from IR
that hides those facts behind integer handles.

---

## Part XI — Open items

1. Choose durable `SemanticValueId` serialization form.
2. Define exact call-graph handling for same-name definitions.
3. Define object scalar-replacement size threshold from layout/benchmark data.
4. Decide when loop-carried facts consume reactor transition graphs directly.
5. Integrate Phase 9 ownership declarations with destination proof meet.
6. Define boundary ownership meet for runtime helpers.
7. Add wasm/native destination lowering conformance corpus.
8. Add destination-plan snapshots to determinism tooling.
9. Add fallback-reason reports to benchmark evidence.
10. Define migration schedule for non-LLVM backends.

Open items do not block Lane A. Items affecting correctness must be resolved
before their lane starts.

---

## Part XII — Status

**Decision:** accepted.

**Implementation:** in progress on
`feat/generalized-value-destination-ownership`.

**Current phase:** D4a facts and internal enum object-payload inlining are
complete. The remaining D4 work is caller-destination/materialization for
escaping object returns (starting with `ws_partial`).

**Rollback:** delete `enum_lowering.rs` and its context field, restore emitter
reads of `enum_layout_planner`, and restore `defn_sret` from `sret_safe_defns`.
Delete `value_lowering.rs` and the enum fields from the generalized plan to
return to D1/D2 analysis-only state.

This record is authoritative for destination/ownership lowering. Implementation
status lives in the execution plan; do not edit historical evidence in place.
