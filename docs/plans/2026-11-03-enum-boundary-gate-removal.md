# Enum boundary-gate removal — Proof 3 completion (2026-11-03)

Status: ACTIVE. Parent: `2026-11-03-ws-frame-fast-path-execution.md` Step 3.
Root cause of the I11 corruption and the state-store fix:
`BUGS.md` ("Enum image escapes to state without owning"),
`docs/plans/2026-11-03-ws-frame-safe-fast-path.md` Proof 3.

Progress:
- Phase C (partial): state stores (`c4a5f4a1`), tuple elements + lambda
  captures (`f397dcae`) own enum images. Validated; ASan-no-inline regression
  harness proven to catch the bug.
- Remaining: Phase A lifetime classification (for reclamation) → Phase B
  reclamation (txn arena + persistent management) → Phase C residual
  (unknown/FFI aggregates) → Phase D gate replacement → Phase E full matrix.

Owner decisions (2026-11-03):
- Pursue full gate removal (option 2).
- Reclamation must be designed, not incidental (no copy-and-leak sprawl).
- Validate correctness at every step.

## Goal

Replace the blanket whole-enum boundary gate
(`enum_layout.rs:107`, `allow_object_inline = !boundary_enums.contains(name)`)
with a proof-derived decision:

```
allow_object_inline = condition1 ∧ condition2 ∧ condition3
                      (Unknown ⇒ keep boxed)
```

- **Condition 1 — layout agreement:** every image-materialization site reads
  one `EnumLoweringPlan` layout (validators exist:
  `enum_lowering.rs::validate_variants`/`validate_representation`).
- **Condition 2 — escape ownership:** no enum image (or handle to one) may
  outlive the storage that owns it; every sink that outlives the producing
  frame materializes/owns the image first, OR the producer frame is proven
  to outlive the sink.
- **Condition 3 — no external raw-byte consumer:** only export/FFI-visible
  enums keep the frozen boxed payload; internal signature enums become
  inline-eligible.

Success: gate removed for proven-internal enums, I11 and all gates green,
no leaks introduced, `ws_frame` at or below C parity (Proof 2 unblocked:
`ws_partial`'s `malloc(34)` gone).

## Current state

- State-store escape sink fixed (commit `c4a5f4a1`): the `Assign` state
  family calls `own_enum_value` before publishing (`own_enum_for_state`,
  `emit_stmt.rs`). Correctness validated under gate relaxation.
- Tuple-element and lambda-capture escape sinks fixed (2026-11-03): enum
  elements/captures are owned before storing into the heap tuple image / env
  block (`emit_expr.rs::emit_tuple_materialize`, `Expr::Lambda`). Validated:
  `enum_tuple_escape.bv` SEGVed at `-O0 -fno-inline` under ASan before the
  fix, passes after; new daily-smoke test `enum_tuple_escape_asan_no_inline`
  with `LinkProfile::AsanNoInline`.
- **Shallow-owning insight (simplifies Phase A):** container materializers
  recurse, so owning each *enum element/capture* at its store site already
  covers nested aggregates (a nested tuple/struct is materialized by its own
  materializer, which owns its enum elements; a struct/collection of an enum
  is owned at that store). A deep transitive predicate is only needed for
  aggregates arriving from unknown/FFI sources. Phase A therefore reduces to
  `type_is_enum` checks at container-store sites + lifetime classification
  for reclamation, not a full transitive walk.
- Remaining gaps (transitive containment):
  - Tuple elements store enum handles raw (`emit_expr.rs::emit_tuple_materialize`).
  - Lambda env captures store enum handles raw (`emit_expr.rs` `Expr::Lambda`).
  - Struct/object fields own direct enum fields (`emit_expr.rs:~2993`) but not
    aggregates containing enums.
  - Collection push owns direct enum elements (`emit_stmt.rs:~1886`) but not
    aggregates containing enums.
  - Enums nested in enums are inline → safe.
- Analysis already enumerates sinks (`SinkKind`/`EscapeKind` in
  `src/analysis/destination_ownership.rs`) and call facts
  (`CallPlan.arguments[].requires_copy`, `ParameterPlan.obligation`); the
  generalized plan is attached to codegen (`backend/llvm/mod.rs:2793`).
- Existing reclamation machinery:
  - per-txn bump arena (`emit_arena_init/alloc/fini`,
    `backend/llvm/mod.rs:2366/2222/2442`);
  - scheduled `Free#` for heap state fields (`analysis/global_lifetime.rs`,
    `emit_scheduled_frees`, `emit_toplevel.rs:4884`).
- `own_enum_value` (`helpers.rs:2710`) is `malloc`-only and never freed today.

## Reclamation architecture (decision: tiers)

| Lifetime | Sinks | Reclaimer |
|---|---|---|
| **Txn-local** — value cannot outlive the firing body | tuple/closure/collection consumed in-node; returns consumed in-node | **existing txn arena** (`emit_arena_alloc`) → reclaimed at `emit_arena_fini` |
| **Persistent** — outlives the firing body | state stores, cross-node, long-lived collections/closures | **destination threading (B1)**: one persistent image per slot, constructed into directly (no copy, no leak); fallback **managed slot (B2)**: copy + free-before-overwrite + scheduled final free |
| **External** — export/FFI | exported ABI | frozen boxed payload (condition 3); unchanged |

Rationale: copy-and-leak at every sink is rejected. Txn-local escapes are
the common case and are fully reclaimed by the arena. Persistent escapes use
destination threading (the destination-ownership end-state) so no copy is
needed; B2 is the conservative fallback where threading cannot be proven.

## Phases

### Phase A — Analysis foundation (frontend; no codegen analysis, Rule 10)

Deliver `src/analysis/enum_materialization.rs` (new) exposing facts consumed
by the backend:

1. `type_embeds_enum_handle(ty) -> bool`: transitively, does a value of `ty`
   embed a **handle** to a separately-stored enum image? Uses
   `EnumLoweringPlan` layouts: enum-in-enum payloads are inline (not a
   handle); tuple/closure/aggregate elements and non-inline payloads are
   handles. `Unknown` ⇒ `true` (conservative box/copy).
2. `sink_materialization(sink_kind, ty, lifetime) -> {none, own_arena, own_persistent}`.
3. Lifetime classification `TxnLocal | Persistent | External` per sink from
   existing `ValueFact.sinks`/`escapes` + reactor reachability.
4. Populate tuple/struct element types in `ValueFact` (today only
   `tuple_element_count`); needed for (1) at aggregate sinks.

Store in `AnalysisResults` (`destination_ownership` extension). Deterministic
(sorted) maps only.

Exit: unit tests for containment (inline-enum-in-enum false; tuple-of-enum
true; nested aggregate true; unknown true), lifetime classification, and a
conservative-`Unknown` test. No emission change yet.

### Phase B — Reclamation primitives

1. `own_enum_image(out, [arena|malloc], words)` in the backend: arena-backed
   when the fact says `own_arena`, heap-backed for `own_persistent`.
2. Persistent state slots (B1): a per-enum-state-field persistent image;
   constructors/stores write into it (thread the destination). Where
   threading is unproven, use B2: copy + `free` the previous owned image
   before overwrite (per-field ownership shadow) + final `Free#` via
   `global_lifetime`.
3. LSan leak gate in `daily_use_smoke` (`ASAN_OPTIONS=detect_leaks=1`) for a
   dedicated corpus; the existing leakage from pre-existing `own_enum_value`
   sites is tracked separately.

Exit: arena-backed images provably reclaimed (no LSan growth over repeated
firings); persistent slots stable under repeated overwrites.

### Phase C — Emission at sinks (additive, Rule 6)

Recursively materialize embedded enum handles, driven by Phase A facts:

- Tuple: `emit_expr.rs::emit_tuple_materialize`. **DONE** (enum elements owned).
- Lambda env: `emit_expr.rs` `Expr::Lambda`. **DONE** (enum captures owned).
- Struct/object fields: `emit_expr.rs::emit_struct_field_store` (already owns
  direct enum fields).
- Collection push: `emit_stmt.rs` push path (already owns direct enum elements).
- State stores: reuse `own_enum_for_state` (already owns).
- Skip materialization when the value is proven non-escaping (Proof 1) or
  already owned (no double-copy).

Condition 1 audit: every materialization site reads the single
`EnumLoweringPlan`; fix any divergence.

Exit: nested-escape repros correct under ASan/UAR; no leaks (LSan);
instruction counts sane; no regressions.

### Phase D — Gate replacement

- Condition 3: `collect_boundary_enums` (`enum_layout.rs:317`) keeps only the
  `Export` (and FFI) arms; drop plain `Definition`/`Transaction` arms.
- `allow_object_inline = conditions1∧2∧3` with `Unknown ⇒ boxed`.
- Keep the old blanket gate as the fallback whenever any fact is `Unknown`.

Exit: gate removed for proven-internal enums; Proof 2 unblocked
(`object_payload_is_inline("Ok","WsFrame")` true ⇒ `ws_partial` joins
`caller_destination_defns` ⇒ no `malloc(34)`).

### Phase E — Verification matrix

Correctness:
- Minimal repros (new `tests/tier1/` fixtures): tuple-of-enum escape,
  closure capture, aggregate field, collection-of-aggregate, cross-firing
  state; plus the existing `defn_store.bv` shape.
- ASan `detect_stack_use_after_return=1`; LSan; `net_stack_helpers` O0–O3;
  `cargo test --lib`; `daily_use_smoke` (tier1 corpus).
- I11: external recommender app + Playwright runner is **absent** from this
  tree; until fetched, gate on the minimal repros + net_stack + tier1. If the
  app is available, run `run_i11_browser.py`.

Structural:
- `benchmarks/ll_determinism_check.sh --quick`.
- Praetor on changed dirs (no new diagnostics).
- Kani harness for the containment/lifetime facts (harness under `kani/` or
  the module's `#[cfg(kani)]` tests, matching existing conventions).

Performance:
- `bash benchmarks/build_and_bench.sh --runtime`; interleaved A/B for
  `ws_frame` (target: `ws_partial` malloc gone, ≤ C parity); confirm no other
  benchmark regressed.

## Risks & mitigation

- **Silent corruption** (wrong fact) → `Unknown ⇒ box`; exhaustive repros;
  ASan/UAR; Kani; condition-1 validators.
- **Leaks** → tiered reclamation (arena for txn-local, threading for
  persistent); LSan gate.
- **Extra copies** → skip when proven owned/non-escaping; measure.
- **Analysis complexity** → facts live in `src/analysis`; Praetor ≤15/≤100.
- **Gate removal too early** → condition 3 lands last, behind all sink
  coverage; blanket gate stays as fallback.

## Undo

- Phase A/D: delete `enum_materialization.rs`, restore
  `allow_object_inline = !boundary_enums.contains(name)` and the full
  `collect_boundary_enums` arms.
- Phase B/C: revert the additive sink materialization arms; the state-store
  fix (`c4a5f4a1`) is independent and stays.
