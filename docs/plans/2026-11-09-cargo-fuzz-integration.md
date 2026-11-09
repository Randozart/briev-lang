# cargo-fuzz integration and upstream demonstration

## Goal
Add coverage-guided fuzzing that is independently portable to upstream: raw
frontend robustness plus generated, bounded integer programs checked against an
independent model. Reuse the compiler's public APIs without production changes.
Demonstrate actual libFuzzer execution, corpus evolution, failure replay and
minimization. Distinguish rediscovery of known tuple defects from new findings.

## Implementation
An isolated `fuzz/` Cargo workspace keeps libFuzzer out of production builds.
Three targets: frontend, arithmetic (supported subset), interpreter (includes
tuple projection/destructuring and therefore known open defects). Check in
reviewable seed inputs separately from generated corpora/artifacts, pin the
fuzz tool/dependencies, add a bounded PR smoke plus nightly/manual sharded
discovery workflow with artifacts, and supply local commands and a PR body.

## Verification and evidence
Build with installed nightly-2026-08-21 and cargo-fuzz 0.13.2; measure executed
inputs/coverage/runtime from libFuzzer logs after build, never conflate this
throughput with native compilation or correctness proof. Run a passing smoke,
actual discovery from a nonfailing seed, replay, and minimization. Verify helper
properties, library suite, build, Praetor on changed code directories, and quick
IR determinism. No optimization or compiler performance claim is proposed.

## Documentation
Add `docs/guides/coverage-guided-fuzzing.md`, a measured report, and an upstream
PR draft. Link the existing deterministic-testing guide and explain which
execution surfaces each tool covers. Preserve existing rationale comments and
keep open-defect exploration separate from required passing PR checks.

## Results
- `cargo +nightly fuzz build`: 42.0 s, all three targets.
- `frontend`: 112,537 execs / 31 s, 5,142 edges, 0 findings.
- `arithmetic`: 34,844 / 61 s and 106,038 / 301 s, 0 findings (oracle clean).
- `interpreter`: disagreement at 42 execs from a neutral 4-byte seed —
  rediscovered the open tuple projection (`.1`) and destructuring (`right`)
  defects; `fuzz tmin` reduced both to 4 bytes.
- No new defect found; rediscovery labelled as such.
- `tests/cargo_fuzz_support.rs` runs the oracle's 4 unit tests under plain
  `cargo test` so CI on stable validates the generator/model without libFuzzer.
- Praetor (`fuzz`, `tests`) passes; stable `cargo build` passes; the follow-on
  full `cargo test --lib` is recorded in the report when complete.
- CI: `.github/workflows/fuzz.yml` adds a blocking bounded PR smoke lane and a
  non-blocking nightly `-fork` matrix that uploads findings.

Full evidence: `docs/reports/2026-11-09-cargo-fuzz-integration.md`.
Upstream PR body: `docs/reports/2026-11-09-upstream-pr-cargo-fuzz.md`.
No production compiler file changed.
