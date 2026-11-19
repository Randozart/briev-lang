# Preserve Data values through reference evaluation

## Root cause

On `efd99d7b`, the interpreter aborts while stringifying an integer in JSON.
`int_to_str` constructs a buffer, binds its address as `d: Data`, then casts
`d as String`. Data was represented as an Int atom; only syntactically nested
casts selected the Data encoding door. A named Data value therefore selected
the numeric formatting lane recursively, without progress. GDB showed repeated
`call_user_fn("int_to_str")`; a 64 MiB stack also overflowed.

## Correction and checks

Carry a distinct raw Data word in interpreter values, preserve it through
bindings/calls/aggregate projection, and dispatch representation casts from
that category. Numeric conversion still dispatches the declared stdlib lane.
Check named, returned and aggregate-carried Data, integer formatting, raw-bit
round trips, and the original JSON round trip against native AST/HIR output.
Run focused tests, library/build/quick/full gates and Praetor.

Update interpreter rationale comments, `BUGS.md`, and the casting architecture
guide in the same commit. Preserve the rejected stack-growth experiment's
record. No stack dependency or code-generation optimization is required.

## Native parity finding

The interpreter regression first passed, then its native AST oracle exposed
another missing representation cast: `Float64 → Data` panicked with
`Bitcast lane must be same-width: double → i64`. These types have equal width;
the final guard compared their spellings. Add the equal-width double/i64
lowering alongside the existing inverse route, with the same executable
raw-bit round-trip regression. This is lowering correctness, not optimization.
Run the backend determinism/toolchain checks for this additional code path.

The next native run exposed a second invalid-IR boundary: comparing the
Float64 result against a Float literal emitted `fcmp ... double, float`.
The shared operand normalizer previously adapted only Float/integer pairs.
Add same-protocol width promotion there, covering both operand orders and
arithmetic as well as comparison; both the template and fallback callers
already consume that normalizer. Preserve the existing integer/Float route.

## Separate format-metadata finding

A Float32 raw-word probe, `((1.5 as Float) as Data) as Int`, disagrees between
the reference and native engines both before and after this repair. Native
prints the correct binary32 word `1069547520`; the interpreter stores every
numeric Float in an untyped `Atom::Float(f64)`. Correct narrow-format raw casts
need format metadata through all value boundaries, not syntax recognition.
This repair's tested raw floating-word guarantee is Float64; the broader
format-metadata discrepancy is recorded separately upstream.
Tracking: https://github.com/Randozart/briev-lang/issues/137.

## Static review

Praetor reports the same 216 diagnostic sites as the baseline across all
changed Rust sources and the integration test. The new helpers/test have no
diagnostics; existing `emit_binary_op` and `eval_expr` complexity decreases.
The Float64 bitcast is an additive match arm, preserving all existing routes.

## Execution record

- Final focused regression: 2/2 passed, covering Data category/FFI words,
  interpreter JSON round trip and native AST/HIR value checks.
- `cargo test --lib`: 3,935 passed, 2 ignored, zero failed.
- `cargo build`: passed with the same 12 existing verifier warnings.
- `bash scripts/gate.sh quick`: 4,056 selected/passed; zero failed,
  missing or unreported cases. Manifest:
  `target/gate-results/20261119T080711Z-6077a7c22648/manifest.json`.
- Final release build passed; full IR-determinism corpus: 13/13 byte-identical.
- The final toolchain matrix executes all three programs under Clang 18/21,
  then fails byte parity on labelled elapsed-time output, as on baseline.
  A retained baseline/final comparison excluding only those timing observations
  confirms identical semantic output for both larger programs on both Clang
  versions; the matrix also reports identical recommender output.
- Full workspace compilation requires the separate missing-option fixture
  repair in PR #141; its upstream failure is recorded in that PR's plan.
