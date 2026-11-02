# Canonical enum payload and selective inline — 2026-11-01

## Decision

Canonical enum representation is **tag + owned payload handle**. Inline object
bytes are not a second default layout. Selective inline remains possible only
under a complete non-escape proof.

Why: layout is semantic; escape is proof. D4b made object bytes part of enum
layout and then had to track borrowed views through matches, calls, state, and
tail calls. That produced an unbounded correctness surface. Briev keeps the
eternal and moves temporal optimization into the destination plan.

## Safety rule

No proof ⇒ box. A scalar/inline representation is legal only if all of:

1. The enum is internal (not exported/foreign-visible).
2. The concrete object size is exact and fits the payload cap.
3. Every constructor/match binding of that object payload is proven
   non-escaping.
4. Every use of the binding is one of:
   - field projection/read;
   - arithmetic/scalar conversion of a field;
   - a direct argument to a known callable parameter whose parameter-retention
     summary is `NonEscaping`.
5. No return, state store, collection/tuple/object-field store, captured
   closure, address escape, unknown call, FFI call, or pointer round-trip into
   a longer-lived structure.

If any condition is unknown, the enum payload stays boxed. Missing proof can
cost performance, never correctness.

## Phases

### Phase A — safe baseline (current)

- D4b disabled; object payloads boxed.
- Keep the stale `expected_enum_type` constructor-owner fix.
- Delete or quarantine D4b-only code paths; `notail` is not a substitute for an
  escape proof.

### Phase B — parameter retention summaries — COMPLETE (2026-11-02)

Landed: `GeneralizedDestinationOwnershipPlan.non_escaping_params` and
`parameter_is_non_escaping()`, derived conservatively from R1 parameter
escapes/sinks. Field-read-only parameters are non-escaping; return/store/unknown
call cases are escaping. Regression: `parameter_retention_summary_is_conservative`.

- R1 already records parameter escapes. Expose a conservative summary keyed by
  callable name + parameter index:
  - `NonEscaping` iff the parameter's R1 value has zero escapes/sinks.
  - `Escaping` otherwise (unknown, return, store, unknown call, etc.).
- This is transitive-safe: calls to known params remain escaping until their
  callee summaries are consumed by a later fixpoint.
- Regression: field-reading consumer = non-escaping; consumer returning or
  storing the object = escaping; unknown callee = escaping.

### Phase C/D — selective inline and match materialization — CORRECTNESS FLOOR LANDED (2026-11-02)

Contributor PR49/50/51 are integrated. PR51's unconditional owned extraction is
the active correctness floor; our selective view path is retained only as
future copy-elision work. D4b is enabled and passes O0–O3, ASan, I11, full lib,
daily, determinism, and full runtime (`ws_frame` 1.33x C).

Match-bound inline objects now resolve through
`inline_object_binding_view_ok`: a view is retained only for a direct call
argument whose callee parameter is `NonEscaping`; every other use is copied to a
heap object before binding. Regression:
`inline_object_view_requires_non_escaping_callee_param`.

The path is inert while D4b is disabled. Activation probe (2026-11-02) with
`notail` + exact sizes + parameter-summary materialization: net_stack O0–O3
pass; I11 still aborts with `malloc(): invalid next size`. Reason: the enum
value itself can escape without a match binding, so the inline object pointer
leaks through the enum handle. Match-binding materialization alone is therefore
insufficient; D4b needs the whole-enum non-escape condition in Phase E before it
can be enabled.

- `EnumLoweringPlan` records per-enum/per-variant `object_payload_inline`.
- Enable only when every match binding and constructor use satisfies the safety
  rule above.
- The policy is a proof result, not a keyword or heuristic.
- If the policy is false, no inline flags exist on the runtime path.

### Phase D — match-binding materialization

- A match-bound inline object is a borrowed projection.
- If any use is not proven non-escaping, emit a heap copy before the first
  escaping use and bind that stable handle.
- Direct-call use is allowed only with a `NonEscaping` parameter summary.
- No `notail` reliance: a safe binding never points into a frame across a
  boundary that can outlive it.

### Phase E — whole-region scalar replacement — GATE LANDED (2026-11-02)

Whole-enum boundary gate: any enum appearing in a callable parameter/result
signature keeps the boxed object payload. Only enums that never cross a callable
signature remain eligible for inline object bytes. Regression:
`callable_signature_enum_keeps_boxed_object_payload`. D4b is still disabled;
activation must pass O0–O3/ASan/I11.

- For a local enum + object region with no escape, scalar-replace the enum image
  and object fields together.
- This is the general replacement for D4b’s type-wide inline layout.
- Cross-call speed requires internal aggregate returns or inlining; measure
  before adding ABI surface.

## Gates

- `cargo test --lib`
- `cargo test --test daily_use_smoke`
- `bash benchmarks/ll_determinism_check.sh --quick`
- `bash benchmarks/build_and_bench.sh --runtime`
- `net_stack_helpers` at O0/O1/O2/O3
- ASan with stack-use-after-return
- real recommender I11
- public export/FFI ABI unchanged

## Acceptance

Safe baseline may ship alone. Selective inline ships only when:

- O0–O3 + ASan + I11 pass;
- `net_stack_helpers`, JSON encode/decode, websocket codec, and worker startup
  pass;
- no benchmark regresses against the boxed baseline;
- the proof remains complete when new escape sinks are added.

## Rollback

The policy is a plan flag; flipping it false restores canonical boxed layout.
No source syntax or public ABI depends on it.

## Contributor PR integration (2026-11-02)

Contributor PRs are based on main `b85f953d` and target the same enum/object
failure area.

| PR | Role in this plan |
|---|---|
| 49 enum-object-layout | Exact resolved enum object capacity via `TypeUniverse`; supersedes ad-hoc object-size plumbing. |
| 50 enum-field-constructor-context | Scopes enum constructor context to a declared struct field type; fixes the field subset of stale `expected_enum_type`. |
| 51 enum-object-projection-lifetime | Owned extraction of inline object payloads. This is the safe baseline: unconditional byte-image copy. |
| 52 callable-transaction-emission | Entry-local transaction allocas + unique SSA names. |
| 53 browser-transaction-publication | Publication at the enclosing successful boundary only. |

### Integration order

1. PR 49 — resolved object capacity.
2. PR 50 — field constructor context.
3. PR 51 — owned extraction.
4. PR 52 — callable transaction emission.
5. PR 53 — browser publication.

PR 53 depends functionally on PR 52. PRs 49–51 are independent, but the combined
recommender and I11 must be validated after all five.

### Conflict policy

- Keep PR 49's `TypeUniverse` capacity source over duplicated local size maps.
- Keep PR 50's declared-field scoping; retain the broader constructor-owner
  validation only if it still catches non-field stale contexts.
- Use PR 51's unconditional owned extraction as the safety baseline. Our
  parameter-retention/view materialization becomes an optional copy-elision
  optimization layered on top: skip the copy only when the extraction binding
  is proven non-escaping.
- Keep P2 (`HashMap` member owner analysis) and D8 metadata cleanups; they are
  independent.
- Keep one normalized-program test helper; retain every distinct regression
  function and bug entry from all branches.
- No app files in compiler PRs.

### Decision gates after integration

1. Rebuild and run `net_stack_helpers` at O0/O1/O2/O3.
2. Run ASan and real recommender I11.
3. Run full runtime.
4. Measure D4b with PR 51:
   - safe + faster → keep inline object payloads with owned extraction;
   - safe but not faster → disable D4b and keep canonical boxed payloads;
   - unsafe → PR 51 is incomplete; inline object payloads stay disabled.

The long-term optimization is region-local enum/object scalar replacement, not
type-wide inline layout. Owned extraction is the correctness floor; copy elision
is a proof result.
