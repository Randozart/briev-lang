# Deterministic transition testing

Run on x86_64 Linux with Rust, clang, and GNU `timeout` available:

```bash
cargo test --test deterministic_transitions -- --nocapture
```

Four deterministic generated programs each process 1–16 signed integer events.
The interpreter computes the reference trace. The actual compiler emits LLVM;
clang links it at both O0 and O3. Each native invocation receives the current
state and next event through environment variables, so its inputs are runtime
values. The harness feeds the returned state into the next invocation and
compares every transition, not merely the final value.

## Watch it find a defect

```bash
BRIEV_SIM_DEMO_FAULT=1 cargo test --test deterministic_transitions -- --nocapture
```

**This command intentionally fails.** It corrupts the first native observation
by +1, without modifying the compiler or reference. Proptest shrinks the failing
program parameters and event sequence (at most 32 shrink iterations). This can
take several minutes because each candidate is compiled and linked.

Inspect `target/simulation-failure/`: `case.json` is the replay input,
`transition.bv` and `.ll` are the native program artifacts, `failure.txt` shows
the disagreement, and `metadata.json` records tool versions, revision, dirty
tree status, and whether the synthetic fault was enabled. This directory is
overwritten by subsequent failures; copy it to retain multiple investigations.

Replay the saved counterexample, first with and then without the synthetic fault:

```bash
BRIEV_SIM_REPLAY=target/simulation-failure/case.json BRIEV_SIM_DEMO_FAULT=1 cargo test --test deterministic_transitions -- --nocapture
BRIEV_SIM_REPLAY=target/simulation-failure/case.json cargo test --test deterministic_transitions -- --nocapture
```

The first fails; the second passes. Replay is based on saved parameters/events,
not a promise that seeds survive generator changes. `BRIEV_SIM_SEED=42` explores
another deterministic campaign. `BRIEV_SIM_ARTIFACTS=/path` selects an artifact
directory; use a distinct directory for each concurrent campaign.

## Real-defect campaigns

`BRIEV_SIM_CASES` sets a positive generated-case count (default 4).
`BRIEV_SIM_FAMILY` selects a fixed transition family (default 0):

| ID | Transition |
|---|---|
| 0 | Guarded addition/subtraction |
| 1 | Signed division by a positive nonzero constant |
| 2 | Signed remainder by a positive nonzero constant |
| 3 | Nested guards with early returns |
| 4 | Guarded local mutation |
| 5 | Short-circuit guard protecting division by zero |
| 6 | Three-element list with runtime indexing |
| 7 | Tuple construction and guarded projection |
| 8 | List element mutation |
| 9 | Mutation through a second list binding |
| 10 | List returned from a helper function |
| 11 | Tuple destructuring |
| 12 | Match-arm guarded return and fallthrough |

```bash
BRIEV_SIM_FAMILY=2 BRIEV_SIM_CASES=16 BRIEV_SIM_SEED=42 cargo test --test deterministic_transitions -- --nocapture
```

The family is included in saved cases and held fixed during shrinking. Old demo
cases without this field replay as family 0. Set no synthetic-fault variable
when investigating real defects. A compile rejection must be investigated for
generator validity before classifying it as a compiler bug.

`BRIEV_SIM_TYPED_REFERENCE=1` runs the reference through the full typed frontend
before interpretation. Use this to exclude missing desugaring/type normalization
in the direct-parser adapter as the cause of a suspected interpreter defect.
Known failing replay cases from the real campaign live in
`tests/fixtures/simulation/`; families 5, 7, 9, and 11 currently reproduce
defects. The default smoke campaign remains family 0. Explicit ignored
interpreter reproducers are expected to fail until their underlying bugs are fixed.

## Scope and scaling boundary

This is an executable transition-model test, not yet a reactive-scheduler,
virtual-clock, arbitrary-FFI, or GPU simulation. The generated function uses a
guard, addition, subtraction, signed values, and state carried between events.
Bounds exclude overflow. The interpreter uses its direct parser/statement path;
native execution uses the real build pipeline. The small common language subset
does not need imported types or normalization on the reference side.

Process timeouts contain native/compiler hangs; they are infrastructure failures,
not proof of semantic nontermination. Shrinking is bounded and does not guarantee
a globally minimal case or preserve failure class for unrelated infrastructure
errors. A failing replay is never silently skipped when clang is missing.

The existing GitHub `cargo test --all` gate includes this test; CI uploads its
failure bundle even when the test fails. The synthetic-fault mode is opt-in and
must not be enabled for normal CI. Historical artifacts are not cleared by a
passing local run. Pin toolchains before relying on cross-machine replay.

Next extensions: persist curated real failures, add independent semantic models,
generate more transition structures, then introduce shared FFI/event adapters
and controlled reactor scheduling. Seed sharding belongs in the campaign runner;
test-function sharding alone will not split this property's generated cases.
