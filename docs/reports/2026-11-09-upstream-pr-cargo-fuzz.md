# PR: add coverage-guided fuzzing (`cargo-fuzz`) as shared defect-discovery infrastructure

> Draft PR body for `Randozart/briev-lang`. Numbers are filled from
> `docs/reports/2026-11-09-cargo-fuzz-integration.md`; refresh them if the
> campaign is re-run before opening.

## Why

Bugs are currently found by hand — building apps and tools, then writing a test
per defect. That does not scale and does not explore code paths nobody thought
to exercise. This PR adds **coverage-guided fuzzing** so the compiler mutates
inputs and walks its own branches automatically, alongside the existing
deterministic native harness.

Two complementary harnesses, one philosophy — *generate, then check against an
independent oracle*:

- `cargo-fuzz` (`fuzz/`) — fast, in-process, branch-coverage-driven.
- `deterministic_transitions` (`tests/`) — slow, real LLVM build + native run.

## What's added

- A standalone `fuzz/` workspace (nightly + libFuzzer) with three targets:
  - `frontend` — lexer/parser robustness; any panic or hang is a failure.
  - `arithmetic` — generates bounded integer programs, runs the interpreter,
    compares against an independent model. Must stay green.
  - `interpreter` — as `arithmetic`, plus tuples. Currently reproduces the
    **known OPEN** tuple defects (see `BUGS.md`).
- `fuzz/src/semantic.rs` — the byte→program generator and the model, with unit
  tests (also run under plain `cargo test` via `tests/cargo_fuzz_support.rs`).
- Reviewable seeds in `fuzz/seeds/`, hand-made reproducers in `fuzz/cases/`.
- `.github/workflows/fuzz.yml` — a blocking, bounded PR smoke lane and a
  non-blocking nightly `-fork` discovery lane that uploads findings.
- `docs/guides/coverage-guided-fuzzing.md`.

No production compiler code is modified. `cargo build` / `cargo test --lib` are
unaffected (separate workspace; no libFuzzer in those paths).

## The interesting design decision: two kinds of "pure"

The fuzzer immediately reproduces the short-circuit division defect
(`BUGS.md 2026-11-09`): `e != 0 && s / e > 0` executes the division on the
excluded edge and crashes. The backend's `expr_is_side_effect_free`
(`src/backend/llvm/logical.rs:159`) treats recursive arithmetic as pure and
therefore eager-evaluatable. **Purity is not speculation-safety.** The fuzzer
finds this, the deterministic harness confirms it natively, and the model
distinguishes a wrong answer from a crash.

## Evidence

- Targets built: `cargo +nightly fuzz build` — **42.0 s**, three binaries.
- `frontend` smoke: **112,537 executions / 31 s**, 3,843 new units, 5,142 edges,
  zero findings.
- `arithmetic` smoke: **34,844 executions / 61 s**; extended **106,038 / 301 s**;
  zero findings — the oracle is validated and the supported subset is clean.
- `interpreter` from a neutral 4-byte seed → **rediscovered** the tuple
  projection defect (`.1` rejected) at **42 executions**, and the destructuring
  defect (`right` unbound); both minimized to **4 bytes**
  (`06 06 f9 f9`, `00 07 00 00`).
- New finding (if any): none in this run. Rediscoveries are labelled as such.
- `cargo test --lib`: **3553 passed, 0 failed** (288.4 s). `git diff --check`: clean.

## How a teammate uses it

```bash
cargo +nightly fuzz build
cargo +nightly fuzz run arithmetic fuzz/corpus/arithmetic -- -max_total_time=120 -print_final_stats=1
# a finding appeared:
cargo +nightly fuzz tmin arithmetic fuzz/artifacts/arithmetic/crash-<hash>
cp fuzz/artifacts/arithmetic/minimized-* fuzz/cases/<name>.input
```

## Scope and honesty

- The `interpreter` target is expected red until the tuple defects are fixed;
  CI keeps it `continue-on-error` and uploads artifacts so a green nightly
  means "no *new* finding."
- Bounds are explicit (state `% 97`, nonzero divisors) so disagreements are
  actionable rather than undefined-behavior noise.
- No performance claim: fuzzer throughput is *inputs/second*, not compiler speed.
