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

### Phase C/D — selective inline and match materialization — CODE LANDED (2026-11-02)

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
