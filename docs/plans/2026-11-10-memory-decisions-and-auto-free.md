# Memory decisions, provenance, and auto-free (2026-11-10)

Status: ACTIVE — M1 (compiler) first, then M2 tensor ownership, then M3 service.
Companions: `2026-11-03-lifetime-and-reclamation.md`,
`2026-11-03-arena-enablement.md`, `2026-08-01-free-check.md`,
`2026-11-10-unified-program-capability-and-ceilings.md`,
`2026-11-10-kiosk-native-acceptance-program.md`.

## 0. Decisions (locked)

1. **No `store` keyword.** Storage shape is determinate from lifetime,
   escape, size, target facts, and FFI visibility. A keyword that picks a
   shape is redundant or a performance hint; the observability razor rejects
   both. Layout/placement that is observable already has `seq`, `Ptr`,
   `@addr`, `vol`, and FFI boundaries.
2. **Plain `Alloc#(size)` is the author surface.** The compiler picks
   `Arena` when feasible, `Alloca` for a bounded frame, `Malloc` when the
   value escapes or the decision is unknown. An explicit strategy argument is
   an ambiguity/override, never a speed lever.
3. **Auto-free proved dead owners.** When the compiler proves storage stale
   (last owner gone), solely owned, and unobservable, it frees it at the
   owner-loss point — silently at runtime, but **always listed in
   `memcheck`**. A `keep` hint overrides.
4. **Remind on undecided ownership.** Unknown/escaping heap owners produce a
   warning by default; kiosk CI uses `--deny-memory-ambiguity`. Unsound free
   and stale use are errors.
5. **`Malloc#` remains the raw manual intrinsic.** It is the explicit
   `@malloc`/`@free` pair for ABI/embedded/manual allocations. It does not
   bypass diagnostics: provenance-tracked `Malloc#` allocations participate
   in the same decision obligation.
6. **No runtime GC/refcount.** Unknown lifetime still leaks (reported), per
   the lifetime north star. `keep until <event>` as a runtime lifetime is
   rejected; lexical cleanup uses `defer`/scopes/state/arena.
7. **No new syntax until a concrete ambiguity needs it.** If a case remains
   after provenance/decisions, add one site-local ambiguity keyword with the
   warn-default/error-when-unsound arm — named for ownership, never storage.

## 1. The gap that motivated this

The compiler already derives and reclaims:

- `Alloc#` with a proved `Frame`/`TxnBody` lifetime → stack or per-tick
  arena, reclaimed at tick end (`emit_arena_fini`).
- Reactor state fields initialized with `Malloc#`/`Alloc#` → last-touch
  scheduled free.
- Owned strings/aggregates/collections; consume/destroy; verified
  `free x;`/`keep x;` hints.
- `Unknown`/`Program` lifetime → boxed heap, **leaked by design**.

The tensor runtime opted out: `Tensor.ptr: Int`, `Malloc#` buffers, escaped
env lists, aliasing (`tensor_rename`). The allocation DAG cannot associate the
`Int` with an allocation, so no free/keep obligation fires. One-shot CLI runs
hide the leak; a long-running VAD/detect daemon exposes it.

## 2. Memory-decision model

### 2.1 Staleness

An allocation is **stale** when:

- its owning handle has no future use,
- no live alias/borrow/ordered reader can observe the storage,
- it is not externally retained (FFI/export/state field with unknown reader).

If stale and solely owned → `auto-free` at the last owner loss. Otherwise the
compiler classifies and demands a decision.

### 2.2 Decision states

| State | Meaning |
|---|---|
| `auto-free@site` | proved stale/sole/unobservable; compiler frees |
| `free@site` | author-pinned `free x;` |
| `keep` | author says it lives longer (`keep x;`) |
| `transfer` | returned/stored into an owned destination |
| `state-owned` | reactor state field; existing scheduler |
| `arena` | arena/stack lifetime (no explicit free) |
| `undecided` | warning; `--deny-memory-ambiguity` errors |
| `unsafe` | free/use would be unsound → error |

`memcheck` prints every allocation, including auto-frees, with the site,
owner-loss site, alias reason, and suggested decisions.

### 2.3 Diagnostics

- **Lost owner** (overwrite/scope exit/escape): "allocation becomes
  unreachable; add `free`, `keep`, return/transfer, or an arena/owned type."
- **Unknown** (alias/FFI/ordered reader): "lifetime unknown; explicit
  decision required."
- **Unsound free** (live alias/reader): error.
- **Stale use**: error (existing `free`/consume contract).
- **Redundant keep/free**: warning.

## 3. Provenance and interprocedural escape (M1 core)

### 3.1 Provenance gate (spike first)

Test whether allocation provenance survives:

`Malloc#`/`Alloc#` → local `Int` → object field → helper call → return.

If it does not, auto-free is limited to provenance-carrying handles. The
tensor ABI must then carry an owner identity (typed owner handle or explicit
owned type), not an opaque `Int`. **No provenance, no auto-free.**

### 3.2 Interprocedural escape summaries

The current allocation DAG treats every call argument as escaping. That forces
`Malloc` even when a helper only reads/writes. Add body-derived summaries:

- per callable/parameter: `escapes` (returned, state/field/collection/object,
  FFI/unknown call, escaped aggregate) or `non-escaping`;
- fixed point across the call graph; unknown/foreign calls escape;
- allocation DAG consumes the summary: only escaping argument positions mark
  an allocation escaped.

### 3.3 Arena context for plain `Alloc#`

Non-escaping `Alloc#` in a callable reachable from a transaction (or a
stateful runtime method) defaults to `Arena`; the callable must receive
`%State` so `emit_arena_alloc` is valid. Functions not reachable from a
reactor tick default to the current frame/stack/malloc rules. Explicit
strategy arguments still win.

### 3.4 Auto-free emission

Consume the dead-owner proof; emit strategy-aware free at the owner-loss
point. `keep x;`/`free x;` suppress/replace. No free for aliases/FFI/unknown.
Machine-readable `memcheck` output for CI.

## 4. M1 work breakdown

**M1.0 — Plan, baseline, provenance spike.**
- Write this plan; update INDEX/unified/kiosk pointers.
- Baseline: `memcheck` on current fixtures; IR for plain `Alloc#` in a
  stateless helper vs transaction; prove/disprove provenance through
  `Int`/object/cast/call.
- Add fixture tests for each decision state.

**M1.1 — Interprocedural escape summaries.**
- New/extended analysis with unit tests; allocation DAG consumes summaries.
- Fixtures: non-escaping read helper stays Arena; returned/stored/FFI escape
  → Malloc/decision.

**M1.2 — Arena context + `%State` propagation.**
- Mark callables needing arena as stateful; plain `Alloc#` selects Arena when
  feasible. IR gate: no `@malloc` for non-escaping calls from a tick; arena
  init/fini.

**M1.3 — Auto-free + keep override + memcheck.**
- Dead-owner free; `keep` suppress; report all decisions.
- `--deny-memory-ambiguity`; warning/error tests.

**M1.4 — Exit gates.**
- `cargo test --lib` green; no double-free; `__briev_free_count` fixtures.
- No false positives on stdlib/FFI.
- Kiosk path: zero undecided heap allocations after migration.
- Docs updated: `llvm-memory-management.md`, lifetime plan addendum,
  `memcheck` docs.

## 5. M2 — tensor ownership + per-tick arena

- `Tensor` carries an ownership class (`owned`/`arena`/`borrowed`/
  `persistent`) and a provenance-carrying handle.
- Activations: plain `Alloc#()` in a stateful inference txn → arena.
- Weights/model graph: persistent (`keep`/Malloc), cached.
- Session state: persistent tensor copied out before tick end.
- Daemon handles one request per tick so the per-tick arena resets.
- Gates: 10k-tick RSS plateau, no per-request malloc, YuNet/VAD/ResNet parity.

## 6. M3 — VAD RPC + op ledger + S2 exit

- VAD session/state RPC, edge hysteresis, barge-in hook.
- Op coverage ledger + fixtures; unsupported-op diagnostics.
- S2 exit report; device perf gate routed to S4.

## 7. Risks

- Provenance may not survive `Int`/casts/object fields; the tensor ABI may
  need an owned handle. Decide from the spike before building auto-free.
- Interprocedural summaries must stay conservative on unknown/foreign calls;
  false "non-escaping" is a use-after-free.
- Auto-free changes observability/liveness; IR/golden and benchmark tests.
- `deny` on legacy kiosk code will surface many leaks; phase the migration.
- Arena selection requires `%State`; stateless helpers must not silently fall
  back to malloc while claiming Arena.

## 8. Undo / retirement

- If the spike shows provenance cannot be retained without a tensor ABI
  redesign, record that decision and keep the explicit region allocator as a
  **TEMPORARY** fallback with a retirement gate: "compiler-owned when tensor
  handles carry allocation provenance."
- Do not add `store`/`keep until` syntax as a fallback.

## 9. M1.0 spike results (2026-11-10)

### 9.1 What the compiler did before this change

| Shape | Result |
|---|---|
| `Alloc#` in a stateless `defn` | `@malloc` |
| `Alloc#` in a txn, value dead inside the txn | arena (`%State` bump) |
| `Alloc#` in a txn passed to a helper that only `Load#`s it | arena — but only because calls were not modeled at all |
| `Alloc#` in a helper called from a txn | `@malloc` (defn defaulted stateless/Malloc) |
| `Alloc#` returned from a txn / stored into state | arena in some shapes — an escape hole |

Root causes found:

1. `let p: Int = Alloc#(n) as Int;` parses as `Cast(Call(Alloc#))`; the
   producer-link check matched only a top-level `Call`, so the allocation was
   never linked to `p`.
2. `add_producer` recorded `source_vars` but never added the consumer edge;
   the forward walk could not reach `Return`/`Call` sinks.
3. `DagNode::Call` was never constructed; a helper call could not mark an
   argument as escaping.
4. `extract_called_functions` ignored `Term`/`Tail`/block statements, so the
   call graph never saw `txn { term helper(n); }`.

### 9.2 Implemented in this slice

- `src/analysis/allocation.rs`:
  - body-derived parameter-escape summaries (least fixed point), alias
    tracking through lets/casts/fields/aggregates, state/return/unknown-call
    sinks; borrowing intrinsics (`Load#`/`Store#`/`Copy#`/...) do not escape;
  - call nodes in the allocation DAG for escaping argument positions only;
  - `add_producer` now links source variables;
  - `expr_contains_alloc` handles casts/wrappers;
  - `return_alias_vars` separates “returned value aliases this” from
    “identifier appeared in the result expression”;
  - `arena_reachable_defns`/`arena_leaf_safe` classify helper context; the
    context gate is **DISABLED** in this slice (see 9.6 — stateful object
    return ABI bug), but the analysis is in place for the next slice.
- `src/analysis/call_graph.rs`: `Term`/`Tail`/block/defer/sync/mutex bodies
  contribute calls; cell transactions/definitions are indexed.
- Unit tests: non-escaping helper keeps a txn allocation Arena; returning/
  storing/alias helpers force Malloc. Helper-context tests are removed until
  the stateful ABI is fixed.

### 9.3 Verified IR

Transaction-local non-escaping `Alloc#` remains arena. A helper that itself
allocates still emits `@malloc` in this slice because helper arena context is
disabled; returning/state-storing/alias helpers correctly emit `@malloc`.

### 9.4 Still open (next slices)

- Auto-free + `keep` override + memory-decision reporting in `memcheck`.
- Provenance tracking for `Malloc#`/`Int`-held raw handles (the tensor ABI).
- Per-tick arena sizing/growth and daemon one-request-per-tick.
- Formal decision fixtures for lost owner/unknown/unsafe; `--deny-memory-ambiguity`.

### 9.5 Node/cell roots (follow-up in the same slice)

The native daemon is a `node` (a cell with transactions), not a top-level
`Transaction`. `CallGraph::build_from_program` and `arena_reachable_defns`
only indexed top-level transactions/defns, so the daemon's helper calls were
still outside arena context. Cell transactions and cell definitions are now
indexed as call-graph roots, and `extract_called_functions` sees
`Term`/`Tail`/block bodies. The node-root infrastructure is in place but the
helper arena context stays disabled pending 9.6.

Still open for tensors: helpers return a `Tensor { ptr }`; the returned
pointer is an escape under the current summary (correct until destination
threading/ownership classes land), so tensor buffers need M2's owned-handle
or caller-provided-buffer design before auto-free can apply.

### 9.6 Blocker found — helper arena context changes the stateful ABI

Enabling helper arena context (even restricted to scalar/void leaf helpers)
makes parser/executor helpers stateful. That exposed a miscompile in the
**stateful object-return ABI**: `parse_value_info` returned an `OnnxValue`
whose `name` was read as a metadata key (`pkg.torch.onnx.original_node_name`)
instead of the input name (`x`). `ml_smoke` then failed with “node input not
found” for every model with initializers.

Evidence: with context disabled, the same compiler passes all tiny models;
with context enabled, `onnx_inspect` and the executor misread the graph.
`needs_arena` propagation and the call/escape fixes themselves are sound;
the corruption appears when a helper becomes stateful and returns an object.

**Decision:** disable helper arena context in this slice. Next M1.1 is to fix
the stateful object-return ABI (or prove the correct sret/state ordering) with
a focused regression, then re-enable the gate and keep the helper Arena tests.
No kiosk pin bump until this is resolved.

### 9.7 HIR emission does not consume the AST arena decision

Further spike found the actual missing link: `needs_arena` correctly listed
`helper`/`make`/`entry`, and HIR emission already consumes
`ctx.defn_needs_state` — but `compute_defn_needs_state_with_arena` added
arena names to the *leaf-consultation* set without seeding those names
themselves. The helper therefore stayed stateless and `Alloc#` fell back to
`@malloc`.

Fixed by seeding `always_stateful` names directly into the fixpoint. Leaf
scalar/void helpers now emit stateful with arena init/bump; callers receive
`%state` through the existing fixpoint. Returning aggregate/Tensor helpers
remain Malloc by the scalar-output gate, so the stateful object-return ABI is
not exercised; it remains a prerequisite before widening the gate to
aggregate-returning helpers.

## 10. M2 progress — tensor arena foundation (2026-11-11)

- Cell/node transactions now participate in allocation-strategy and
  arena-need analysis (`analyze_alloc_strategies` / `analyze_arena_need`),
  so a node-body `Alloc#` can select the tick arena. Unit tests cover the
  node transaction path.
- Kiosk `briev/ml/tensor.bv` gained a per-request bump arena (`install`,
  `reset`, `tensor_alloc`); `tensor_free` no-ops for arena tensors.
  `detect_faces` resets after each response; VAD copies state to an owned
  tensor before reset. Activations no longer Malloc-per-op.
- The arena block is still an owned `Malloc#` installed by the node entry.
  Switching that block to plain `Alloc#` reproduces the stateful
  object-return ABI miscompile (OnnxValue/message shapes), so the compiler
  arena path stays gated until that ABI regression is fixed.
- Face graph caching is not solved yet: top-level object/list globals are not
  emitted (undefined `@face_cached`/`@empty_xs`), so the daemon reparses per
  request and RSS still grows. Next: cache the parsed graph without relying
  on unsupported globals, or make parser graph/list allocations arena-backed.

## 11. Stateful object-return ABI — the active fix (2026-11-11)

Evidence: adding a node-body `Alloc#` (arena block) to the ONNX test/daemon
makes stateful parser helpers return a corrupted `OnnxValue`: the input name
reads as `pkg.torch.onnx.original_node_name` (a metadata key) instead of `x`,
and models with initializers fail with “node input not found”. Removing the
node `Alloc#` restores `x`. The corruption happens before execution, at
`parse_model_at` output, so it is the stateful object-return path, not tensor
math.

Constraints:
- Plain stateful functions returning scalar-field objects work.
- Stateful functions returning objects with String/List fields work at small
  `%State` sizes.
- The failure appears when the program also allocates an arena block in the
  node body (larger `%State`, extra stateful helpers).

Fix protocol:
1. Reduce to a minimal stateful function returning a String/List object plus a
   node-body `Alloc#`; confirm corruption.
2. Inspect emitted signatures and call sites for `%state`/sret ordering,
   especially stateful defns with object returns and callers that are
   themselves stateful/stateless.
3. Fix the ABI (state pointer must be the stable first argument; object
   returns use the same sret/handle convention regardless of state), add a
   regression, then re-enable plain `Alloc#` for the tensor arena block.

### 11.1 Resolution — parser metadata descent, not object-return ABI

The reduction changed the diagnosis. A stateful function returning an object is
fine in isolation; the corruption came from `parse_value_name_loop` continuing
past the ValueInfo type and walking `metadata_props` (`field 4`). Under the
arena/stateful layout that skip path descended into nested entries, so a
metadata key became the input name. `parse_value_name_loop` now terminates
after the type and graph strings are owned copies (`owned_string`), so graph
names no longer alias the model buffer.

Regression: `tests/ml/parser_arena.bv` allocates a node-body `Alloc#` and
asserts the graph input name is `x`; `harness/ml_smoke.sh` runs it. The tensor
arena block now uses plain `Alloc#()` and the compiler selects the tick arena.
Daemon RSS still grows because parser List/graph allocations are not all
arena-backed and the graph is reparsed per request; that is the next runtime
work, not an ABI blocker.

### 10.1 Face graph cache + residual RSS

The daemon caches list handles in `Int` globals and reconstructs `OnnxGraph`
per request; weights are owned tensors, activations use the tick arena.
60-call JPEG soak: ~3.2 MB/call → ~0.3 MB/call (167 → ~217 MB), first calls
dominate. Residual is per-request object/string heap, not model parsing;
full plateau remains open (compiler auto-free/decision pass is the next lever).

### 10.2 Residual RSS attribution (2026-11-11)

With the graph cache and tick arena in place, a 200-call JPEG detect soak still
grows ~0.2–0.3 MB/call (VmRSS 144→189 MB). Stage isolation:

- frame-only (image decode + input tensor + reset): ~0.15 MB/call;
- execute-only (graph run + reset, no detection decode): ~0.2 MB/call.

So the residual is small per-request heap objects (Tensor/Result/env-list/
string), not model parsing or tensor data. Moving conv patch buffers into the
arena was tried and **rejected**: baseline RSS rose to ~399 MB and growth did
not stop (the patches are large and short-lived; Malloc/free reuses them).
Patch buffers therefore stay owned.

This is the first concrete target for the auto-free/decision pass: free
non-escaping object/Result/List allocations at the request/txn boundary (or
region-allocate the request). Manual frees in the executor are not the fix —
the allocator sites are compiler-generated and interprocedural.

### 10.3 Request-allocator trial — rejected (2026-11-11)

Tried a linker-level request region: IR rewrite of `@malloc`/`@free`/`@realloc`
to `briev_req_*` wrappers with begin/end around each request.

- `onnx_smoke`/tiny and YuNet, VAD under the wrappers: tiny models passed.
- Daemon: crashed/hung once compression/response paths entered the request.
  Wrappers see pointers from the compiler arena/region and from libc in the
  same free/realloc stream; without provenance, a request bump allocator
  cannot distinguish “bump-owned” from “compiler-owned”, and deferring all
  frees during a request corrupted later paths.
- VAD similarly faulted with the wrappers, despite passing without them.

Reverted completely. Conclusion: allocation interception is not a substitute
for provenance. The auto-free/decision pass must operate on allocation sites
with ownership classes (Malloc vs compiler arena/region vs borrowed), not on
the `@malloc` symbol. This negative result is the gate for the next attempt.

### 10.4 Allocation-site hook trial — rejected on string realloc/overlap

Second attempt: mark node-reachable defns in IR (`; BRIEV_REQ_ALLOC`) and
rewrite only malloc/free/realloc inside those blocks to request hooks; graph
parsed per request; no graph cache.

- Tiny models passed with the hooks.
- Daemon aborted in `concat` with ASan `memcpy-param-overlap`; both ranges
  were inside the request block, with a corrupted/oversized copy length. The
  hook’s bump header does not match every pointer Briev passes to
  realloc/free (String data vs allocation base / interior pointers), so the
  request arena cannot safely service realloc without matching the runtime’s
  actual allocation layout/provenance.
- Reverted completely; no pin change.

Conclusion unchanged and sharper: allocation interception needs the compiler
to carry allocation **base + size + owner class** per pointer through String/
List/object representations. The next attempt starts with that metadata (or a
typed owned-handle), not with hooks keyed on address ranges.
