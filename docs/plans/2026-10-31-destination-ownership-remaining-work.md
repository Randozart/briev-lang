# Generalized destination ownership — remaining work

**Status (2026-10-31): active.** The generalized destination/ownership branch
was fast-forwarded into `main` at `3c2aeaea`. Phases D1–D6 are complete and
D7 is partial (D7a–D7e). This plan is the execution queue for finishing D7,
closing D8, and leaving one proven lowering mode.

Authoritative context:

- umbrella: `docs/plans/2026-10-30-generalized-value-destination-ownership.md`
- architecture: `docs/architecture/value-destination-ownership-decision-record.md`
- merged baseline: `3c2aeaea`
- work from a fresh branch, e.g. `feat/destination-ownership-d7-d8`

## Non-negotiables

- Public export ABI, runtime ABI, collection layout, String/Blob, closure,
  task, wasm shim, and persisted-state layouts stay frozen.
- No new Briev syntax or ownership keywords.
- Unknown ownership falls back to box/copy; a missing proof can cost
  performance, never correctness.
- No type-name or algorithm-vocabulary recognition in backend code.
- Internal ABI changes are atomic per callable: signature + every call site in
  one commit.
- No regression is excused as noise without a controlled same-machine A/B.

## Verified merged state

At `3c2aeaea`:

- 3420 lib tests green.
- daily-use suite 29/29 green, including `net_stack_helpers` at O3.
- quick `.ll` determinism green.
- full runtime suite outputs MATCH; `ws_frame` 1.08x C in the merged run.
- GLUE suite, `bridge_glue`, `bridge_multi` green.
- Public signatures for scalar/string and stateful exports unchanged versus
  parent.
- D5b object alias fix landed: object-literal fields record parent-lifetime
  escapes; nested `HttpRequest` inside returned `HttpParse` is heap-backed.
- D4c object caller destination is enabled; `ws_partial` writes into a
  caller-owned image.
- D6h retains one narrow fallback scan for generated foreach bindings that R1
  never records. This is correctness debt, not the intended end state.

## P0 — pin the bugs found during merge readiness — COMPLETE (2026-10-31)

Permanent regressions landed:

- `object_literal_field_aliases_parent_lifetime` in the destination model.
- `generated_foreach_binding_keeps_entry_slot` in the daily suite.
- `net_stack_helpers_asan_stack_lifetime` in the daily suite (ASan,
  `detect_use_after_return=1`).
- D4b inline enum-object payloads disabled; full O0/O1/O2/O3 and ASan reruns
  pass with the path off. Re-enable belongs in P3 after the alias proof.

These bugs were found late; each needs a permanent regression test before more
optimization.

1. **Nested object literal alias**
   - fixture: `tests/tier1/net_stack_helpers.bv`.
   - regression: compile/link/run at `-O3 -flto`; require
     `aGVsbG8=|s3pPLMBiTxaQ9kYGzzhZRbK+xOo=|OK`.
   - unit fact: I0 stores an object literal field, the parent escapes, the
     child fact must include `ObjectLifetime`, so
     `stack_object_types_by_owner` excludes it.

2. **Generated foreach binding dominance**
   - fixture: `tests/tier1/test_hashmap_rehash.bv`.
   - regression: generated IR must pass `clang` validation; no `alloca` inside
     a loop branch may feed a load after the loop.
   - goal: replace the legacy assignment scan with an R1 fact that covers
     macro/generated bindings.

3. **Object caller destination lifetime**
   - fixture: `ws_frame` plus `net_stack_helpers`.
   - regression: ASan with `detect_stack_use_after_return=1`.
   - keep `%object_out` conservative (`ptr align 8`, no false `nocapture`).

Acceptance: all three tests run in `cargo test --test daily_use_smoke` or a
focused backend test, not only ad-hoc shell checks.

## P1 — finish D7 boundary lane

### D7f — complex export wrappers — COMPLETE (2026-10-31)

Exported object and enum returns keep the frozen boxed public ABI and receive
public wrappers. `object_repr` never grants an exported callable a hidden
`%object_out`; D3 already keeps exported enum results boxed. Public signatures
remain `i64` handles; internal bodies are emitted under
`<name>__briev_internal`.

Wrappers now allocate internal hoisted-sret and recursive-carrier slots, pass
them in the internal signature order, and keep them off the public signature.
A focused backend test pins slot allocation and call ordering. Plain-context
exports (environment-gated diagnostic mode) and the `#export("name")` remap
path stay on their existing direct-symbol/wrapper paths.

Work:

1. Define wrapper eligibility as a reusable ABI description, not a boolean
   chain in `emit_toplevel`.
2. Emit wrappers for:
   - enum results with boxed public ABI;
   - object results that keep the boxed public ABI;
   - hoisted-sret callables;
   - recursive-carrier callables;
   - plain-context exports.
3. Keep authored symbol public; move body to a deterministic internal symbol.
4. Ensure internal Briev calls may use the internal symbol without changing
   output or introducing a wrapper recursion layer.
5. Byte-compare public prototypes against the pre-wrapper compiler. Bodies may
   differ; public signatures may not.
6. Add C and JS consumer tests for scalar, String, stateful, and boxed-enum
   exports.

Acceptance:

- every `export defn` has exactly one public symbol;
- public prototypes byte-identical to `3c2aeaea` for the export corpus;
- GLUE, bridge_glue, bridge_multi, C/JS export tests green;
- no `_start`/duplicate-symbol regressions.

### D7g — runtime-helper and bridge boundaries — COMPLETE (2026-11-01)

Inventory result: every user-visible runtime-helper crossing is a `frgn`
ForeignBinding and therefore already has a ForeignCall boundary plan.
Compiler-internal helpers (`int_to_str`, print lanes, collection growth) are
not public boundaries and stay internal. No separate `RuntimeHelper` plan is
invented.

Direct and bridge calls consume the same boundary ABI decision; the declared
FFI result type remains the ABI contract. The inventory regression
`every_foreign_binding_has_a_lookupable_boundary` pins plan lookup for every
`frgn`.

Acceptance:

- FFI parser/typechecker/proof suites green;
- bridge_glue and bridge_multi green;
- no undeclared or mismatched helper declarations.

### D7h — platform boundary evidence — PARTIAL (2026-11-01)

Green:

- wasm32: `rbv_list_each_wasm`, `rbv_obj_init_wasm32`, `rbv_router`.
- no-prelude/stdlib boundary export: `tests/no_prelude_boundary.rs`.
- C/multi bridge: `bridge_glue`, `bridge_multi`, GLUE suite.
- Node/JS export + extension: `tests/js_export_boundary.rs` calls the real
  `gate.js` consumer.
- Full public ABI regressions from P0/D7f.

Two JS blockers were fixed: `glue/c.bv` now explicitly imports its cast-lane
dependency so bridge IR defines `cstr_len`; bridge export uses library-mode
emission; and stateful export wrappers pass `%state` exactly once.

Open: real-browser fake-peer/I11 gate (app worktree absent in this
environment). See `benchmarks/results/2026-11-01-d7h-boundary-evidence.md`.

Run and record:

1. C export consumer at `-O3 -flto`.
2. JS/Node export consumer.
3. wasm32 compile + link + run for an export fixture.
4. browser fake-peer/I11 task gates on the recommender app pin.
5. `brievc --no-stdlib` parity for primitives and boundary-only fixtures.
6. External ABI surface comparison before/after for every public export.

Acceptance:

- all D7 gate rows green;
- evidence appended to this plan or a dated results file;
- no frozen ABI delta.

## P2 — close D6 foreach fallback — BLOCKED (2026-11-01)

Investigation finding: for `test_hashmap_rehash`, the emitted
`txn_go` contains HashMap probe loops whose assignments are **created during
callable expansion inside emission**, after frontend analysis. The generalized
plan for `txn:go` has no LoopPlans at all (`loops = []`), so the legacy scan is
the only source for names such as `r` and `done_slot`. This is not a missing
source-name filter in R1; the body shape does not exist when R1 runs.

The backend fallback therefore stays until inline/callable expansion is
destination-plan-aware. The required fix is to replay callee loop-carrier facts
(and any remapped locals) into the caller's emission context, or to run the
destination adapter over the expanded body before emission.

The backend currently scans assignment names inside a foreach body only when
R1 did not report them. This fallback exists because generated/macro-expanded
bindings can bypass R1 scoping.

Work:

1. Identify the generated binding shape from `test_hashmap_rehash`.
2. Make callable inline expansion carry callee `LoopCarrier` facts / remapped
   locals into the caller emission context. R1 alone cannot see this shape.
3. Remove `collect_foreach_assigned` from the backend only after the hashmap
   dominance regression passes without it.
4. Keep plan attachment a hard pipeline invariant.
5. Add a regression test that fails if any foreach assignment alloca is emitted
   inside the loop body.

Acceptance: no AST liveness rewalk remains in backend loop lowering.

## P3 — harden object frame images and caller destinations — IN PROGRESS (2026-11-01)

Landed: tuple elements record `TupleStore` / `TupleLifetime`; a child object
stored in a returned tuple stays heap-backed (`tuple_object_alias`, `41\n42\n`
at O3/LTO). Collection elements, object fields, loop-assigned parent fields,
and `&object` address escapes now have destination-model regressions proving
the child is materialized and excluded from `stack_object_types_by_owner`.
The collection-return runtime path is covered by `collection_rebind_gate`.

0. Re-enable D4b only after an O0/O1/O2/O3 + ASan proof of the i64 handle
   round-trip. Current status: disabled; object payloads stay boxed.
1. Add alias tests for:
   - nested object literal fields;
   - object in tuple — DONE (`TupleLifetime` + `tuple_object_alias`);
   - object in collection returned across a function — DONE
     (`CollectionLifetime` + `collection_rebind_gate`);
   - object field assigned in a loop — DONE (`ObjectLifetime` regression);
   - `&object` across FFI — DONE (`AddressLifetime` regression).
2. Verify `stack_object_types_by_owner` is alias-wide: one escaping alias
   excludes frame images for that owner/type.
3. Keep D4c eligibility proof-based; test both accepted and rejected call
   positions.
4. Run ASan/stack-use-after-return on `ws_frame`, `net_stack_helpers`, and
   collection rebind fixtures.
5. Revisit `%object_out` attributes only with an IR A/B and ASan evidence.

Acceptance: no frame image can escape its owner lifetime by any path.

## P4 — D8 retirement

Only after P0–P3 are green.

1. Remove legacy enum sret/inline special cases superseded by
   `EnumLoweringPlan`.
2. Remove `retain_enum_return` / `own_enum_value` call sites that no longer
   carry a unique obligation.
3. Remove ad-hoc destination metadata from `FunctionContext` that the
   generalized plan owns.
4. Remove the dual legacy/direct paths left for complex exports once D7f is
   complete.
5. Remove compatibility-only fallbacks and the generated-foreach scan.
6. Update:
   - `docs/architecture/value-destination-ownership-decision-record.md`;
   - `docs/architecture/backend-architecture.md`;
   - `docs/architecture/llvm-memory-management.md`;
   - `docs/architecture/layout-contracts.md`;
   - `AGENTS.md` reference index if the plan moves from active to historical.
7. Publish a before/after table for allocations, IR size, and runtime ratios.

Acceptance:

- one lowering mode;
- no legacy destination path reachable;
- full lib, daily, determinism, runtime, GLUE/bridge, wasm/web gates green;
- release-readiness review recorded.

## Gate matrix

| Gate | Command / evidence |
|---|---|
| Unit | `cargo test --lib` |
| Daily corpus | `cargo test --test daily_use_smoke` |
| Determinism | `bash benchmarks/ll_determinism_check.sh --quick` |
| Full runtime | `bash benchmarks/build_and_bench.sh --runtime` |
| WS codec | `cargo test --test ws_frame_codec_parity` |
| GLUE | `cargo test --test glue_bridge_tests` |
| C bridge | `bash benchmarks/build_and_bench.sh bridge_glue --correctness` |
| Multi bridge | `bash benchmarks/build_and_bench.sh bridge_multi --correctness` |
| Object lifetime | clang ASan, `detect_stack_use_after_return=1` |
| Export ABI | normalized `define` prototype diff vs `3c2aeaea` |
| No-stdlib | `brievc --no-stdlib` boundary fixture |
| Wasm | wasm32 compile/link/run fixture |
| Browser | recommender fake-peer + I11 gates |

## Known risks

- **Alias completeness:** D5b was broken once by an unrecorded parent-object
  alias. Every new aggregate edge needs an escape fact before frame images are
  allowed.
- **Handle round-trips:** object/enum pointers cross through i64 handles; any
  new hidden destination must survive `ptrtoint`/`inttoptr` under LTO.
- **Wrapper recursion:** exported bodies calling the authored name can route
  through the public wrapper; D7f must decide and test internal-call routing.
- **Generated AST:** macro/composite expansion can create bindings R1 does not
  see. Do not remove the fallback until a regression proves R1 covers it.
- **Performance drift:** `ws_frame` improved, but several benchmarks remain
  above 1.0x C. D8 is a cleanup milestone, not a performance guarantee.

## Exit criteria

Merge the remaining branch only when:

1. P0 regression tests are permanent.
2. D7 platform gates are recorded.
3. No backend AST liveness rewalk remains.
4. D8 retirement is complete.
5. The full gate matrix is green at the merge commit.
