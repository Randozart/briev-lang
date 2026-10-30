# Generalized value-destination ownership implementation

**Date:** 2026-10-30
**Status:** ACTIVE — approved implementation lane
**Branch:** `feat/generalized-value-destination-ownership`
**Design record:**
`docs/architecture/value-destination-ownership-decision-record.md`
**Parent/reconciliation plan:**
`docs/plans/2026-10-29-destination-ownership-reconciliation.md`
**Implementation scope:** LLVM native x86_64 + wasm32 Tier 1.
**Language surface:** unchanged. No new Briev syntax, keywords, annotations,
or strategy markers.
**Compatibility contract:** public export ABI and frozen C/JS/runtime layouts
do not change. Internal helper ABI may change atomically per callable.

This plan turns the destination-ownership design into the generalized
implementation. It is not an enum-only patch. The destination model, fact
schema, lowering helpers, call ABI contract, migration gates, and cleanup rules
are defined for every Briev value domain from the beginning. Implementation
migrates one value class at a time, but every switch is made from the same
generalized plan.

## 1. Executive summary

The compiler currently has several partial lifetime systems:

- enum `sret` / `%enum_out` / `%esrt` / `%einl`;
- `retain_enum_return` and `own_enum_value`;
- tail-loop carriers;
- object malloc paths;
- state/list/object store specialization;
- boundary ownership computed but not consumed by LLVM;
- loop-engine liveness and foreach alloca demotion;
- allocation strategy analysis not uniformly consumed by composite lowering.

These systems encode pieces of the same question: **where does a semantic
value live, who owns it, when may it alias, and when must it be copied?**

Phase R1 introduced `DestinationOwnershipPlan`: stable semantic value IDs,
origins, uses, sinks, escapes, call summaries, conservative destination
classes, and diagnostics. It is analysis-only.

This lane implements the generalized destination model in three concurrent
tracks:

1. **Proof/plan completion** — extend R1 into a complete frontend fact model.
2. **Generalized lowering** — add one shared value-lowering contract and
   consume the plan.
3. **Lane migration** — switch value domains from legacy paths to the plan,
   atomically per callable, then retire legacy machinery.

No source syntax is added. If a proof exists, the compiler may choose a faster
internal representation. If no proof exists, it boxes/copies. This is not a
user-facing exception; it is a compiler-internal lowering decision.

## 2. Why generalized first

The failed experiments established the mechanism and the failure mode:

- Scalar enum aggregates alone did not recover `ws_frame` performance.
- The dominant cost was the boxed `WsFrame` object payload.
- A hand destination-reuse prototype reached the old baseline range.
- Ad-hoc backend metadata repeatedly caused aliasing/lifetime crashes.
- Unsafe no-copy controls were fast only because they returned pointers into
  dead frames.

An enum-only implementation would have to be generalized later, likely
requiring another rewrite. Building the generalized interface now avoids
that.

This does **not** mean one giant rewrite commit. It means the model, interfaces,
and regression gates cover all value classes from the start. Migration proceeds
in independently gated lanes.

## 3. Source-language policy

No Briev syntax is added or changed.

The compiler may choose faster internal representations only when its existing
facts and proofs support them. Unknown ownership always maps to the safe boxed
or copy fallback.

If existing contract/ownership declarations already carry a proof, the backend
may consume them. This proposal does not introduce new author-facing ownership
annotations.

This follows:

- the observability razor;
- derivation-not-recognition;
- proof-vs-shape;
- Golden Rule 2: fastest proven default, never a speed keyword;
- Golden Rule 25: proof licenses optimization;
- derivation-not-recognition: no algorithm/value-name recognition.

## 4. Compatibility policy

### Frozen external surfaces

The following remain unchanged during the entire migration:

- public exported function ABIs;
- C/JS runtime helper signatures;
- collection runtime layout `[data, cap, len]`;
- String runtime layout;
- closure environment ABI;
- task scheduler ABI;
- wasm JS shim protocol;
- persisted state layout unless explicitly versioned in a later lane;
- executable observable behavior.

### Mutable internal surfaces

The following may change atomically per callable/function:

- hidden helper parameters;
- internal call-site argument forms;
- local SSA/alloca representation;
- enum/object/tuple destination selection;
- loop-carried temporary representation;
- copy elimination.

If a callable plan changes representation, the function signature and every
call site are emitted from the same plan in the same compile. No mixed
prototypes are legal.

### Backend scope

LLVM is the first implementation target. The fact model is target-independent.
Other backends may continue current conservative lowering until they choose to
consume the plan. No backend may redispatch by Briev type name; type shape and
proof facts are the only keys.

## 5. Current foundation

Phase R1 has landed:

- `src/analysis/destination_ownership.rs`;
- `src/analysis/destination_ownership/destination_walk.rs`;
- semantic `ValueId`s based on callable owner and source path;
- origins, uses, sinks, escapes, copy points;
- callable summaries and call-site obligations;
- conservative classes;
- deterministic diagnostics;
- `AnalysisResults.destination_ownership`;
- no backend consumption.

Existing supporting analyses to reconcile:

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

The generalized plan consumes and reconciles these; it does not duplicate their
layout or ownership logic in the backend.

## 6. Generalized fact model

### 6.1 Type shape registry

Add a target-independent `ValueShape` layer derived from concrete type/layout
facts.

```rust
pub enum ValueShapeKind {
    Scalar,
    Enum,
    Object,
    Tuple,
    Collection,
    Slice,
    Closure,
    Task,
    Reference,
    Opaque,
}

pub struct ValueShape {
    pub ty: Type,
    pub kind: ValueShapeKind,
    pub payload_words: Option<usize>,
    pub inline_words: Option<usize>,
    pub boxed_fallback: bool,
    pub recursive: bool,
    pub external_abi: bool,
}
```

Rules:

- derived from `EnumLayoutPlanner`, casting graph/layout contracts, and
  concrete generic instantiation;
- no Briev type-name dispatch in the backend;
- recursive/oversized/external shapes are `boxed_fallback`;
- collection runtime layouts remain frozen in this lane.

### 6.2 Value identity

Phase R1 uses callable owner plus source path. The implementation must preserve
this but add stable durability:

```rust
pub struct SemanticValueId {
    pub callable: CallableId,
    pub kind: ValueKind,
    pub source_path: SourcePath,
    pub incarnation: u32,
}
```

Requirements:

- stable across LLVM register renaming;
- deterministic across compilation processes;
- stable enough for diagnostics and golden tests;
- handles lexical shadowing through a scope path;
- handles loop iterations through iteration-scope identity, not runtime count.

A source-path representation may remain internal, but it must be canonical and
serializable.

### 6.3 Value facts

Each semantic value carries:

```rust
pub struct ValueFact {
    pub id: SemanticValueId,
    pub ty: Type,
    pub shape: ValueShape,
    pub origins: BTreeSet<ValueOrigin>,
    pub uses: BTreeSet<UseSite>,
    pub sinks: BTreeSet<Sink>,
    pub escapes: BTreeSet<Escape>,
    pub last_use: Option<ProgramPoint>,
    pub may_overlap: bool,
    pub ownership: OwnershipClass,
    pub destination: DestinationClass,
    pub obligations: BTreeSet<Obligation>,
    pub reasons: BTreeSet<ProofReason>,
    pub fallback_reason: Option<FallbackReason>,
}
```

`OwnershipClass` includes:

- `Owned`;
- `Borrowed`;
- `Consumed`;
- `Shared`;
- `Unknown`.

`DestinationClass` includes:

- `ScalarValue`;
- `CallerDestination`;
- `ScalarReplacedObject`;
- `StableCarrier`;
- `PackedMemory`;
- `ZeroCopyView`;
- `Boxed`;
- `Unknown`.

### 6.4 Callable plans

```rust
pub struct CallableDestinationPlan {
    pub callable: CallableId,
    pub exported: bool,
    pub parameter_plans: Vec<ParameterPlan>,
    pub result_plan: ResultPlan,
    pub hidden_abi: HiddenAbiPlan,
    pub direct_recursive: bool,
    pub tail_loop: Option<TailLoopDestinationPlan>,
    pub calls: BTreeMap<CallSiteId, CallDestinationPlan>,
    pub proof_status: ProofStatus,
}
```

Result plans include:

```rust
pub enum ResultDestination {
    Void,
    SsaValue,
    CallerDestination,
    Boxed,
    Unknown,
}
```

Parameter obligations include:

```rust
pub enum ParameterObligation {
    Value,
    Borrow,
    Consume,
    Owned,
    Shared,
    CopyFromBorrow,
    Unknown,
}
```

Unknown is conservative.

### 6.5 Call-site plans

```rust
pub struct CallDestinationPlan {
    pub site: CallSiteId,
    pub callee: CallableId,
    pub argument_plans: Vec<ArgumentPlan>,
    pub result_plan: Option<ResultUsePlan>,
    pub destination: Option<DestinationId>,
    pub may_overlap_inputs: bool,
    pub requires_copy: bool,
    pub proof_status: ProofStatus,
}
```

Rules:

- destination forwarding only with non-overlap proof;
- internal calls with unknown summaries box/copy;
- exported/FFI calls use boundary wrappers;
- recursive calls reuse proven carriers.

### 6.6 Loop and reactor plans

Loop plans include:

- loop-carried values;
- iteration-local values;
- carrier ID;
- carrier lifetime;
- backedge copy/move obligations;
- temporary/carrier overlap facts;
- phi versus alloca lowering choice.

Reactor plans include:

- state values live across ticks;
- temporary values confined to a tick;
- wake-set-read values;
- event-bound values;
- flush-bound web values.

Unknown lifetime maps to state/boxed, not a guessed temporary.

### 6.7 Boundary plans

Boundary facts classify every crossing:

```rust
pub enum BoundaryClass {
    ExportedValue,
    ForeignValue,
    RuntimeHelper,
    WebView,
    PersistedState,
    ForeignTask,
}

pub struct BoundaryDestinationPlan {
    pub boundary: BoundaryId,
    pub class: BoundaryClass,
    pub internal_form: DestinationClass,
    pub external_form: ExternalForm,
    pub conversion: BoundaryConversion,
    pub ownership: OwnershipClass,
}
```

Export/FFI/runtime external forms remain frozen. Internal forms may change.

## 7. Proof engine

### 7.1 Inputs

The proof engine combines:

- R1 semantic value facts;
- AST/CFG statement structure;
- callable call graph;
- concrete type shapes;
- `EnumLayoutPlanner`;
- tail-loop and tail-carrier plans;
- reactor transition graph;
- global lifetime/free scheduling;
- boundary ownership lattice;
- declared Phase 9 ownership constraints where present;
- contract-proven facts where available.

### 7.2 Proofs required

Destination forwarding is legal only when all hold:

1. the callee writes the complete destination on every returned path;
2. destination lifetime covers every use;
3. destination does not overlap any input live across the call;
4. no other live value reads the old destination contents during construction;
5. no hidden escape occurs;
6. recursion/loop interaction is represented by a proven carrier;
7. target widths agree;
8. ABI is internal or covered by a wrapper.

Scalar replacement is legal only when:

1. the object is small by concrete layout;
2. no field address escapes;
3. no state/list/closure/task/FFI use escapes;
4. every field use is visible in the value graph;
5. materialization at any escape is expressible.

Carrier use is legal only when:

1. carrier lifetime spans all live iterations;
2. temporary and carrier do not overlap;
3. backedge copies preserve all live values;
4. recursive reentry does not overwrite a live outer carrier;
5. loop exit consumes the final carrier or copies it.

Zero-copy/borrow is legal only when:

1. source lifetime dominates every use;
2. no mutation overlaps a read;
3. ABI accepts a borrowed/zero-copy form;
4. the plan records the owner.

### 7.3 Conservative lattice

Proof meet is intentionally lossy:

| Fact A | Fact B | Meet |
|---|---|---|
| same proven class, non-overlap | same proven class, non-overlap | keep class |
| different proven classes | any | boxed/copy |
| proven | unknown | boxed/copy |
| non-overlap | maybe-overlap | maybe-overlap |
| function-local | escapes | escaped |
| internal | external | boxed/boundary |
| loop-local | loop-carried | loop-carried |

No proof may be invented. If a proof is unavailable, fallback is legal.

### 7.4 Proof obligations and diagnostics

Every non-boxed destination must expose:

- proof kind;
- source program points;
- callable IDs;
- value IDs;
- lifetime owner;
- non-overlap proof;
- copy obligations;
- fallback reason if not selected.

Example:

```text
value: Result<WsFrame,String>
destination: CallerDestination
proof:
  callee writes complete result
  destination lifetime = caller frame
  no overlap with input ByteBuf
  all paths write result
copy: none
fallback: not required
```

Example:

```text
value: WsFrame
destination: ScalarReplacedObject
proof:
  no field address escape
  no state/list/closure/export use
  fields visible in function graph
materialization: return path
```

Example:

```text
value: closure
destination: Boxed
fallback: captured by task scheduler; ownership unknown
```

## 8. Generalized lowering contract

### 8.1 New lowering module

Add:

```text
src/backend/llvm/value_lowering.rs
```

Public contract:

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

All representation crossings use these helpers. Emitters no longer make
ad-hoc `ptrtoint` / `inttoptr` / memcpy ownership decisions.

### 8.2 Lowering invariants

1. Every destination write is complete or explicitly partial by layout.
2. No destination is read before initialization on any executable path.
3. Every escape writes a destination owned by the consumer.
4. Every overlapping source/destination pair is copied first.
5. Every representation conversion is explicit.
6. Every hidden parameter comes from the callable plan.
7. Every call-site destination comes from the same callable plan.
8. Every ABI boundary uses a wrapper/conversion plan.
9. Every fallback has a reason.
10. No backend lowering branches on Briev type names.

### 8.3 Atomic callable emission

For each callable:

1. read `CallableDestinationPlan`;
2. choose hidden ABI parameters;
3. emit signature;
4. bind parameters from the same plan;
5. emit body using value/call plans;
6. emit result/return using the same result plan;
7. emit call sites in other functions from the same plan.

No callable may mix plan-derived and legacy ABI decisions.

### 8.4 Lane compatibility mode

During migration, the backend may select:

```rust
pub enum LoweringMode {
    Legacy,
    Generalized,
}
```

per callable only if:

- the callable is non-exported;
- its entire call graph is same-mode;
- no public/runtime boundary directly observes mixed forms.

The long-term state is generalized-only. Compatibility mode is migration
scaffolding, not an architectural endpoint.

## 9. Migration lanes

### Lane A — generalized scalar enum/`Result`

**Goal:** remove enum-specific ad-hoc machinery while preserving correctness.

Scope:

- two-word scalar enums;
- enums boxed by concrete layout;
- constructor/match agreement;
- scalar SSA versus caller destination;
- internal `sret` replacement.

Non-scope:

- object payload scalar replacement;
- collection/state layout changes;
- public ABI changes.

Tasks:

1. Extend destination plan with enum shapes and constructor/match agreement.
2. Add plan-driven enum construction/match helpers.
3. Emit callable result plans atomically.
4. Consume plan at call sites.
5. Replace `retain_enum_return` on plan-proven paths.
6. Keep boxed fallback for unknown paths.
7. Compare IR/output against current compiler.

Gate:

- `server_protocol_gate`;
- `server_loop_gate`;
- JSON round-trip;
- ws codec parity;
- all recommender server gates;
- `cargo test --lib`;
- full IR determinism;
- no new unproved copies.

### Lane B — small object scalar replacement

**Goal:** eliminate proven non-escaping object allocations.

Scope:

- small object construction;
- field-level local representation;
- field escape/materialization;
- object argument/return destinations;
- `WsFrame`-shaped values as first performance case.

Tasks:

1. Add object shape and escape facts.
2. Add scalar-replacement proof.
3. Add alloca/SSA object lowering.
4. Add materialization on state/list/return/FFI escape.
5. Remove malloc only on proven objects.
6. Benchmark `ws_frame`.

Gate:

- exact `ws_frame` output;
- no malloc for proven eligible objects;
- `ws_frame` ≤ corrected performance target;
- ASan/stack-use-after-return checks where available;
- full recommender/server/browser gates.

### Lane C — containers, state, tuples, slices

**Goal:** use destination facts at all memory/container crossings without
changing frozen runtime layouts.

Scope:

- state load/store;
- collection construction/insert/index;
- tuple construction/access;
- object field load/store;
- slices/views;
- boxed fallback.

Tasks:

1. Add sink-specific destination plans.
2. Add container-owned destination model.
3. Add explicit conversion at container boundaries.
4. Preserve frozen runtime layouts.
5. Remove ad-hoc ownership copies where plan proves them unnecessary.

Gate:

- recommender PostgreSQL/server suites;
- wasm32 state/list/object fixtures;
- collection parity;
- JSON/list/object round trips;
- no destination pointer stored in state/collection.

### Lane D — loops, recursion, carriers

**Goal:** one carrier model for all loop-carried/recursive values.

Scope:

- foreach;
- countable loops;
- version-DAG loop fields;
- direct tail recursion;
- recursive carrier reuse;
- reactor tick lifetime.

Tasks:

1. Merge loop-carried analysis and tail-carrier plan into destination plan.
2. Replace hidden carrier ad-hoc decisions.
3. Emit carrier copies through shared helpers.
4. Preserve phi/alloca lowering choices from loop engine.
5. Prove temporary/carrier non-overlap.

Gate:

- FSM `BOUND 0..8`, `1M`, `50M`;
- recursive carrier fixtures;
- no stack growth;
- no per-iteration malloc for proven carriers;
- loop-engine tests.

### Lane E — closures, tasks, boundaries

**Goal:** consume boundary facts at every external edge.

Scope:

- closures;
- spawned tasks;
- foreign calls;
- exports;
- web views;
- runtime helpers.

Tasks:

1. Keep closures/tasks boxed initially.
2. Add boundary destination plans.
3. Consume `boundary_ownership` at LLVM ABI edges.
4. Add wrappers for internal destination forms.
5. Verify frozen public/runtime forms.

Gate:

- C/JS export round trips;
- task/async suites;
- browser fake-peer and I11;
- wasm32 ABI tests;
- `--no-stdlib` parity corpora.

### Lane F — retirement and cleanup

**Goal:** leave one destination system.

Tasks:

1. Remove `%enum_out`, `%esrt`, `%einl`.
2. Remove `retain_enum_return`.
3. Remove `own_enum_value`.
4. Remove old enum/object destination metadata.
5. Remove backend lifetime heuristics.
6. Remove migration compatibility mode.
7. Update architecture docs and AGENTS reference.
8. Run full release evidence.

Gate:

- no backend lifetime decisions from register names;
- no representation conversion outside `value_lowering.rs`;
- no mixed lowering modes;
- full tests/performance/benchmark evidence.

## 10. Exact implementation phases

### Phase D0 — branch baseline and contract tests

**Deliverables:**

- branch baseline recorded;
- golden fixtures selected;
- output/IR snapshots captured;
- compatibility surface documented;
- migration policy added to docs.

**Tasks:**

1. Record current compiler SHA and benchmark outputs.
2. Capture current IR for ws_frame, FSM, SPSC, JSON, list/object fixtures.
3. Add executable golden output tests where missing.
4. Add compatibility manifest listing frozen ABIs/layouts.
5. Add migration status doc/table.
6. Ensure no behavior change.

**Gate:** all current tests pass; snapshots reproducible.

### Phase D1 — complete semantic fact schema

**Deliverables:**

- generalized `ValueShape`;
- durable `SemanticValueId`;
- value/callable/loop/boundary plans;
- proof obligations and reasons;
- deterministic diagnostics.

**Tasks:**

1. Refactor R1 types into `destination_ownership` submodules if needed.
2. Add `ValueShape` from concrete layout facts.
3. Add `CallableId`, `DestinationId`, `ProgramPoint`, `UseSite`.
4. Add callable result/parameter/hidden ABI plans.
5. Add loop plans.
6. Add boundary plans.
7. Add `ProofStatus` and fallback reasons.
8. Add deterministic serialization/debug output.
9. Add unit tests for every fact class.
10. Ensure no backend consumption yet.

Gate:

- analyzer tests;
- no IR change;
- deterministic plan output;
- Praetor clean;
- full lib tests.

**Status 2026-10-30: COMPLETE.** `src/analysis/destination_model.rs` now
carries generalized value, callable, call, loop, and boundary plans. R1 now
indexes `export` wrappers, loop-carried assignments emit `LoopCarrier` sinks,
and call arguments/results are present in the generalized value table. No
backend pass consumes the plan yet. Export wrappers are now indexed, call
arguments/results and loop carriers enter the generalized value table, and the
production plan is built only after R1 facts are aggregated. Validation: 10
focused tests, 3384 lib tests, quick IR determinism, Praetor clean.

### Phase D2 — shared value lowering skeleton

**Deliverables:**

- `src/backend/llvm/value_lowering.rs`;
- representation-conversion helpers;
- destination handle abstraction;
- plan-facing lowering context;
- no call sites migrated yet.

Tasks:

1. Add lowering context carrying callable/loop/boundary plans.
2. Add pure conversion helpers first.
3. Add IR-emitting destination helpers.
4. Add explicit copy/materialization helpers.
5. Add unit/IR tests for helper output.
6. Keep all current emitters unchanged.

Gate:

- current tests/IR determinism;
- helper IR tests pass;
- no legacy emitter changed except optional imports.

**Status 2026-10-30: COMPLETE.** Added
`src/backend/llvm/value_lowering.rs`: plan-facing `ValueLoweringContext`,
pure `ConversionPlan` decisions, boundary-plan mapping, and exact
`llvm.memcpy`/`llvm.memset` emitters over resolved `MemoryRange`s. No emitter
call sites changed. Validation: 9 helper tests, 3393 lib tests, quick IR
determinism, Praetor clean.

### Phase D3 — generalized enum lane

**Status 2026-10-30: D3a COMPLETE (facts only).** The generalized plan now
records declared enum constructors, payload types, enum-shaped values, and
per-callable enum result plans. `CallerDestination` is claimed only when the
backend's existing `sret_safe_defns` gate also proves the callable safe;
exported enum results stay boxed to preserve the public ABI. No enum emitter
changed yet. Validation: 12 destination-model tests, 3396 lib tests, quick IR
determinism, Praetor clean.

**Deliverables:**

- enum shape facts;
- constructor/match agreement facts;
- atomic callable enum ABI plan;
- plan-driven enum construction/match;
- call-site enum destinations.

Tasks:

1. Add enum destination facts.
2. Select per-callable result destination.
3. Select hidden destination only with proof.
4. Emit enum construction through shared helpers.
5. Emit match through shared helpers.
6. Emit call sites through the same plan.
7. Replace legacy enum paths on migrated callables.
8. Keep boxed fallback for unknown paths.

Gate:

- server protocol/loop gates;
- JSON round-trip;
- ws codec parity;
- all recommender server gates;
- full lib/determinism;
- no incorrect retain/copy.

### Phase D4 — generalized object lane

**Deliverables:**

- object shape and escape facts;
- scalar replacement proof;
- plan-driven object destination;
- local SSA/alloca object lowering;
- materialization on escape.

Tasks:

1. Add object field use/escape graph.
2. Prove non-escaping small objects.
3. Lower proven objects to fields/allocas.
4. Materialize on state/list/return/FFI escape.
5. Keep boxed fallback.
6. Route `WsFrame` construction through the plan.
7. Benchmark A/B with exact output.

Gate:

- exact ws_frame output;
- no malloc for proven objects;
- corrected performance target;
- server/browser gates;
- ASan/stack-use-after-return where available.

### Phase D5 — container/state lane

**Deliverables:**

- sink-specific container plans;
- destination conversion at state/list/object edges;
- removal of ad-hoc ownership copies where proven.

Tasks:

1. Add state sink plans.
2. Add collection sink plans.
3. Add object field sink plans.
4. Add tuple sink plans.
5. Preserve frozen container runtime layout.
6. Add wasm/native width tests.
7. Compare IR/output.

Gate:

- recommender server suites;
- wasm32 fixtures;
- list/object round trips;
- full tests.

### Phase D6 — loop/carrier lane

**Deliverables:**

- unified loop destination facts;
- plan-driven stable carriers;
- plan-driven phi/alloca choices;
- recursive carrier plans.

Tasks:

1. Merge tail-carrier facts into destination plan.
2. Merge loop-carried/loop-engine liveness inputs.
3. Select carriers by proof.
4. Emit carrier copies via shared helpers.
5. Migrate FSM/recursive fixtures.
6. Compare outputs/performance.

Gate:

- FSM all BOUND gates;
- recursive carrier tests;
- no stack growth;
- no per-iteration allocation;
- SPSC/ws parity.

### Phase D7 — boundary lane

**Deliverables:**

- boundary destination plans;
- ABI wrappers;
- LLVM consumption of boundary ownership;
- frozen external ABI evidence.

Tasks:

1. Add boundary IDs and plans.
2. Route exports through wrappers.
3. Route FFI/runtime helper calls through conversion helpers.
4. Add wasm/web boundary tests.
5. Compare external ABI surfaces byte-for-byte where applicable.

Gate:

- C/JS export tests;
- browser fake-peer/I11;
- runtime helper tests;
- `--no-stdlib` parity;
- wasm app gates.

### Phase D8 — retirement

**Deliverables:**

- legacy destination machinery removed;
- one lowering mode;
- final architecture/benchmark/docs evidence.

Tasks:

1. Remove enum sret/inline special cases.
2. Remove retain/own enum helpers.
3. Remove ad-hoc metadata.
4. Remove compatibility mode.
5. Update architecture docs/agent reference.
6. Run all performance/correctness evidence.
7. Publish before/after table.

Gate:

- generalized lowering only;
- full tests;
- all runtime outputs MATCH;
- release readiness review.

## 11. Regression and differential testing

### 11.1 Output differential

For every migrated callable/value class:

- run native and wasm fixtures at small and large bounds;
- compare stdout/exit code;
- compare state-visible results;
- compare browser DOM/text state where relevant;
- fail on any mismatch.

### 11.2 IR differential

For behavior-preserving refactors:

- compare function IR after plan switch where practical;
- expected differences are limited to destination form/copies;
- any unrelated register/code-shape change must be explained.

### 11.3 Sanitizer differential

For native lifetime-sensitive lanes:

- compile with ASan when practical;
- enable stack-use-after-return where useful;
- run bounded and large workloads;
- investigate all reports; no ignore-by-default.

### 11.4 Proof audits

For every non-boxed plan fact:

- check proof status;
- check destination lifetime;
- check overlap proof;
- check hidden ABI consistency;
- check boundary conversion;
- check fallback reason absence.

### 11.5 Performance audits

For every lane:

- capture baseline and candidate timings;
- interleave runs;
- use actual release link commands;
- capture IR evidence;
- record output parity;
- explain every regression.

## 12. Compatibility manifests

Maintain a machine-readable compatibility manifest listing:

- exported symbol ABI;
- runtime helper ABI;
- collection block layout;
- String/Blob layout;
- closure env layout;
- task argv/scheduler layout;
- web flush/view protocol;
- wasm state pointer protocol;
- state persistence layout.

During migration:

- internal forms may change;
- manifest entries may not;
- every manifest-observing change is a separate versioned lane, not this one.

## 13. Performance targets

Targets are corrected safe targets, not the dangling-pointer baseline.

### ws_frame

- exact output;
- no hot-path malloc for proven `WsFrame`;
- target: ≤1.10x the best safe hand-validated destination result;
- aspirational: C parity if LLVM can scalarize the destination path;
- no unsafe stack-handle shortcut.

### FSM

- exact output;
- zero enum/object allocation for proven carriers;
- no native stack growth at large bounds;
- wasm runs without memory growth failure.

### SPSC

- exact output;
- no queue representation regression;
- no destination copies inside the hot loop unless required by ownership.

### General runtime

- all rows MATCH;
- no unexplained regression;
- improvements recorded per value class;
- regressions have proof-linked explanations.

## 14. Documentation updates

Same-commit updates are required as phases land.

### Decision record

New ADR:

```text
docs/architecture/value-destination-ownership-decision-record.md
```

Contains:

- decision;
- context;
- alternatives;
- consequences;
- invariants;
- migration policy;
- rollback policy.

### Architecture updates

- `backend-architecture.md`: destination lowering contract and module map;
- `llvm-memory-management.md`: destination ownership vs allocation/free;
- `layout-contracts.md`: layout facts feeding destination plans;
- `derivation-not-recognition.md`: proof-derived destination decisions;
- `agent-reference.md`: backend must consume `AnalysisResults.destination_ownership`;
- `proof-vs-shape.md`: representation selected by proof;
- `boundary ownership` docs: LLVM consumption policy.

### Spec updates

Update §2.2 and §2.3 only to codify existing semantic policy:

- semantic values are independent of destination form;
- materialization selects a destination representation by target facts/proofs;
- ownership/lifetime facts govern copies;
- unknown destinations use safe boxed fallback;
- no source syntax is added.

No grammar, keyword, operator, or author-surface change is introduced.

## 15. Risks and controls

| Risk | Control |
|---|---|
| AST source paths change across desugaring | canonical semantic IDs + stability tests |
| Destination forwarding aliases an input | require non-overlap proof; otherwise fresh destination + copy |
| Hidden ABI mismatch | atomic callable plan consumed by signature/calls |
| Exported ABI accidentally changes | frozen manifest + wrapper-only policy |
| Runtime layout break | frozen manifest; container lane internal-only |
| Recursive carrier overwrite | carrier proof + non-overlap test |
| Loop destination treated as temporary | loop plan class + fixed-point lifetime proof |
| wasm/native width mismatch | target width in shape and lowering tests |
| Closure/task escapes ignored | conservative boxed until dedicated facts |
| Boundary ownership ignored | boundary lane consumes lattice at ABI edges |
| Proof complexity explodes | conservative meet; incremental callable summaries |
| Miscompile from bad proof | differential/sanitizer/golden gates; proofs are auditable |
| Old and new lowering coexist forever | retirement gate and compatibility-mode removal |
| Benchmark regression blamed on noise | interleaved A/B and proof-linked report |

## 16. Rollback policy

Each phase is independently revertible.

- D1 is analysis-only.
- D2 adds helpers with no emitter change.
- D3–D7 migrate lanes behind plan facts and compatibility constraints.
- Any lane failure reverts to legacy lowering for that callable/value class.
- Failed experiments are saved as patches and recorded, never discarded.
- Public/runtime ABI rollback is not expected because external ABI remains frozen.

## 17. Definition of done

The generalized implementation is complete when:

1. Every Briev value domain has a destination plan.
2. Every callable has an ABI/result plan.
3. Every call site has argument/result plans.
4. Every loop has carrier plans.
5. Every boundary has conversion plans.
6. Backend lifetime decisions come only from `AnalysisResults.destination_ownership`.
7. All representation crossings use `value_lowering.rs`.
8. Legacy enum/object destination machinery is removed.
9. No destination pointer can escape its owner.
10. No unsupported LLVM assumption is emitted.
11. All current and new tests pass.
12. All runtime outputs MATCH.
13. Determinism, Praetor, conformance, ASan-sensitive lanes, and benchmark evidence pass.
14. Public/runtime ABIs are unchanged.
15. Docs and release evidence are current.

## 18. Initial work checklist

- [x] Reconciliation plan landed.
- [x] Phase R1 semantic facts landed.
- [x] Compatibility policy defined.
- [x] Create comprehensive design/ADR
      (`docs/architecture/value-destination-ownership-decision-record.md`).
- [x] Update spec semantic/materialization sections (§2.2/§2.3).
- [x] Update architecture docs (backend architecture, LLVM memory,
      layout contracts, derivation doctrine).
- [x] Capture branch baseline
      (`benchmarks/results/2026-10-30-value-destination-branch-baseline.md`).
- [x] Complete Phase D1 schema completion (analysis-only).
- [x] Complete Phase D2 shared value-lowering skeleton (no call sites).
- [x] Complete Phase D3a enum destination facts (no emitter change).
