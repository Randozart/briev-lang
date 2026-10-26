# Briev stability and v0.1 release

**Date:** 2026-10-25
**Status:** ACTIVE — Phase 1 complete; Phase 2 (reference parity) next
**Branch:** `stability/v0.1`

## Decision set

| Decision | Choice |
|---|---|
| Tier 1 supported | x86_64 Linux native + wasm32 browser |
| Tier 2 compile-only | aarch64 Linux, rv64 |
| Experimental | GPU, embedded, silicon, other backends |
| C parity | not a v0.1 blocker; required for the competitive release |
| Performance rule | no unexplained regression against the pinned baseline |
| Preview release | `v0.1.0-preview` after Phases 1–2 |
| Stable release | `v0.1.0` after Phases 3–4 |
| `cargo test --all` | hard CI gate immediately, with a tracked quarantine |
| Recommender server | external; not a stability dependency |

## Stability definition

A release is stable when:

- Tier 1 targets build and run their acceptance corpus in CI;
- the interpreter and compiled backend agree on the shared corpus;
- malformed input cannot panic the compiler;
- `cargo test --all`, daily corpus, conformance, browser gates and determinism
  are green;
- every quarantined test has a tracked removal condition;
- no runtime regression is accepted without a recorded cause;
- install, versioning and target support are documented and tested.

## Phase 1 — Gate integrity

1. Add `cargo test --all` as a blocking CI job.
2. Audit all-target failures/hangs.
3. Fix known broken drivers/tests:
   - C-driven Node bridge;
   - C-driven Python bridge;
   - CLI entry-command hangs.
4. Where immediate repair is not safe, quarantine with:
   - BUGS.md entry;
   - owner;
   - exact removal condition;
   - a separate non-blocking “known failures” job that still executes them.
5. Add parser/typechecker fuzz smoke to CI.
6. Audit reachable compiler panics on malformed input.
7. Keep docs/status truthful.

Acceptance: `cargo test --all` green; quarantined tests are explicit and still
run non-blocking; malformed-input probes return diagnostics.

## Phase 2 — Reference/backend parity

1. Interpreter executes callable Briev txns.
2. Interpreter supports the raw-address → String door.
3. Fix large `ws_encode` in the reactor interpreter path.
4. Close compound bitwise-or residual and enum-payload match hangs.
5. Add a differential gate: same fixtures through interpreter and compiled
   backend, byte/exit/output equality.
6. Re-land generic callable transaction specialization with a concrete
   multi-struct `iter_chain` gate.
7. Fix `dyn Trait` / PHI-mismatch defects.

Acceptance: shared corpus matches in both execution paths; no Rule-5
divergence in the gate.

## Phase 3 — Platform contract

1. Publish `brievc --targets` and a support table.
2. CI: Tier 1 build+run; Tier 2 compile-only; experimental excluded.
3. Run `benchmarks/toolchain_matrix.sh` before releases.
4. Smoke-test clean install/package flows.
5. Ensure unsupported targets fail with diagnostics, not crashes.

Acceptance: declared targets are continuously checked; support is explicit.

## Phase 4 — Performance non-regression

1. Nightly/release-gated full runtime suite.
2. Compare against the pinned baseline.
3. Publish every row and flag regressions.
4. Record cause/owner for every slower row.
5. Keep correctness and performance gates separate.

Acceptance: no unexplained regression; performance status is visible.

## Phase 5 — Release engineering

1. Add `CHANGELOG.md`, release notes, artifact checksums.
2. Tag `v0.1.0-preview` after Phases 1–2.
3. Tag `v0.1.0` after Phases 3–4.
4. Publish support/breaking-change policy and supported LLVM/Clang versions.
5. Add issue templates for miscompiles, interpreter divergence, panics and
   performance regressions.

## Explicit non-goals

- Otto’s server port; `server.bv` stays external.
- App-specific code in stdlib.
- New language features before Phase 1–2.
- Contract weakening to make fixtures pass.
- GPU/embedded correctness before Tier 1 stability.

## Validation

- `cargo test --all`
- `cargo test --lib`
- `cargo test --test daily_use_smoke`
- `cargo test --bin brievc`
- conformance sweep
- Task Board Chrome/API
- browser host/heap/snapshot gates
- determinism quick/full
- full runtime suite before `v0.1.0`
- Praetor on changed directories

## Phase 1 execution log

- `cargo test --all` is now a blocking CI gate; the first run was audited.
- Fixed two real compiler regressions exposed by it:
  - mutable Event-wire parameters now load the wire id from their `%ps` slot
    instead of passing the alloca address;
  - strict event traps inside stateless defns use `llvm.trap` rather than
    referencing an absent `%state`.
- Quarantined, not deleted, the non-Tier1 C bridge/CLI failures listed in
  BUGS.md. A non-blocking CI lane still runs ignored tests.
- `cargo test --all`: PASS (ignored set skipped); conformance and tier-1
  corpus pass.
- Task Board Chrome/API, rbv gate, determinism, accel non-hang and host
  transport all pass.

Remaining Phase 1: add fuzz smoke and the malformed-input panic audit.
