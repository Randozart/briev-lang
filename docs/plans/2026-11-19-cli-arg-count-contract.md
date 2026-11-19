# Correct the arg_count contract position

`lib/std/cli.bv` spells `[term >= 0]` as its only contract bracket. A single
bracket is a precondition, so callers can be asked to prove a result value
before the call. The declared nonnegative result belongs in the second bracket:
`[true][term >= 0]`. Preserve the result guarantee and the `Argc#` body.

Reproduce via a caller with its own explicit contract, then add native
AST/HIR argument-count coverage and contract-call validation to the existing
CLI integration suite. Record the diagnosis in `BUGS.md`; update the function
comment. No architecture/API shape changes. Run library, quick, CLI and
Praetor checks and record the actual outcomes.

## Execution evidence

Ordinary callers, a contracted wrapper, and `--verify-bodies` all built on
`efd99d7b`; those experiments did not reproduce a current-main caller rejection.
The parsed-contract regression does fail before the correction:
`argc is callable before its result exists: UndefinedVariable { name: "term" }`.
It evaluates both contract halves rather than matching the source spelling.

- CLI integration: all 11 tests pass, including the parsed-contract regression
  and argument-count execution under both native emission routes.
- `cargo test --lib`: 3,935 passed, 2 ignored, zero failed.
- `cargo build`: passed with the same 12 existing verifier warnings.
- Praetor on `tests/` and `git diff --check`: passed.
- `bash scripts/gate.sh quick`: 4,057 selected/passed; zero failed,
  missing or unreported cases. Manifest:
  `target/gate-results/20261119T072737Z-263d597de496/manifest.json`.
