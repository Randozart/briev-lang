# Contextual collections in enum payloads

## Baseline and goal

On `efd99d7b`, `Array([IntVal(3)])` type-checks but panics with
`a bare list literal reached codegen without a collection type`. An ordinary
`List<JsonValue>` call argument works: the enum payload emitter drops its
already-resolved payload type before emitting that same literal.

The harness's returned enum, tuple payload, appended enum and nested JSON
update reproducers now produce the expected native output on this baseline.
Those already-repaired paths must stay intact.

## Mechanism and verification

Use the resolved enum payload type to construct collection literals through
their declared collection operators. Cover empty/nonempty, generic and
multi-field enum payloads, including nested collections; no type-name matching
or alternative collection layout. Reuse typed literal construction across the
enum and existing typed argument paths where applicable.

Run the fixture before/after, interpreter assertions and native AST/HIR at O0
and O3 with Valgrind; run library, quick and backend determinism/toolchain
gates. This is a correctness change, with no performance claim.

Document the missing context and regression in `BUGS.md`; update
`docs/architecture/backend-architecture.md` and preserve existing enum payload
rationale comments when factoring code. Record actual checks and limitations.

## Execution record

- Baseline fixture reaches the bare-list panic. The fixed regression passes
  interpreter assertions and AST/HIR native execution at O0/O3, including
  Valgrind access checks.
- `cargo test --lib`: 3,935 passed, 2 ignored, zero failed.
- `cargo build`: passed with the same 12 existing verifier warnings.
- `bash scripts/gate.sh quick`: 4,056 selected/passed; zero failed,
  missing or unreported cases. Manifest:
  `target/gate-results/20261119T070651Z-edb4ca224870/manifest.json`.
- Release build passed; full IR-determinism corpus: 13/13 byte-identical.
- Praetor reports the same 211 baseline diagnostic sites; the added helpers
  and integration regression introduce none.
- Toolchain matrix initially lacked `u.data`. With the public MovieLens 100k
  input and the harness-generated similarity file, all three corpus programs
  run successfully under Clang 18 and 21. The script fails byte parity because
  `multinode`/`giantnode` print `prediction_time_ns`/`prediction_time_ms`.
  A controlled baseline/fixed A/B retained every output: these labelled timing
  values are the only differences. Excluding those observations gives identical
  semantic output across revisions and toolchains. The unmodified gate remains
  failing; no performance conclusion is drawn from these correctness runs.

An exploratory `Basket<T> { Packed(List<T>) }` also exposed a separate
typechecker inference defect: it checks `List<T>` against `List<Int>` before
inferring the nested parameter. The emission regression covers concrete
payloads and `Wrapper<List<Int>>`; it does not claim to repair nested inference.
Tracking: https://github.com/Randozart/briev-lang/issues/135.
