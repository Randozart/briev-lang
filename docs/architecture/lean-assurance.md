# Compiler assurance and Lean export

Owner: the compiler-assurance workstream in
[the implementation plan](../plans/2026-11-14-201223-compiler-assurance-and-lean-export.md).

The first objective is assurance about the existing Rust compiler and discovery
of missed bugs. A later optional Briev-to-Lean backend would export program
semantics and proof obligations. No language syntax change is part of this work.

## First component: solver response classification

The [solver response contract](solver-response.md) documents the production fix,
its trust boundary and real-Z3 regressions. This is the first implementation-linked
assurance component; it is not a proof of query generation or compiler soundness.

## Bounded implementation proof

The Kani harness `solver_response::proofs::sat_model_cannot_authorize_unsat`
executes the production parser. Its domain is a leading `sat\n` followed by
any eight ASCII bytes (including control characters). Its assertion requires
SAT, not merely absence of UNSAT. The bound and ASCII restriction are explicit;
this does not verify arbitrary-length or arbitrary-Unicode solver output.

```bash
cargo kani --manifest-path briev-verify/Cargo.toml --harness sat_model_cannot_authorize_unsat
```

A private copy restoring substring classification supplies the negative control;
the harness must reject it on the SAT-preservation assertion. Check completed
verification and unwinding assertions, not just Kani compilation. The existing
local Kani is 0.67.0; no tool download is required for this first harness.

## Lean model and current evidence limits

`verification/lean/SolverResponse.lean` is an initial model, with Lean pinned by
the adjacent `lean-toolchain`. It states exact-status recognition theorems and
includes SAT-model/error/unknown controls. Run from that directory:

```bash
lean --version
lean SolverResponse.lean
```

**Status at introduction: not kernel-checked in this environment.** Lean is not
installed and direct GitHub DNS resolution failed. No Lean correctness result is
claimed. The model begins after whitespace normalization; it does not prove Rust
string preprocessing or implementation refinement. Its axiom reports must be
reviewed when checked. Kernel-checked statements, once available, establish only
the stated model properties. An intentional false-theorem rejection and a
production extraction/refinement experiment remain required before accepting
the component as a verified Rust implementation.

## Export acceptance boundary

A future export must name the supported typed-HIR subset, machine arithmetic,
errors, effects and termination assumptions. Unsupported constructs must produce
coverage diagnostics. Contracts must become obligations, not assumed facts.
Emitting Lean that typechecks is not equivalent to proving a source contract.

Guarantees about native output additionally need a connection to lowering and
optimization, through preservation proofs or checked translation evidence.
Exporting after optimization alone can reproduce an optimizer bug in both the
program and its purported specification. Source revision and artifact hashes,
theorems, assumptions, trusted components, skipped obligations and coverage must
accompany any eventual guarantee manifest. The ordered milestones in the plan
separate these obligations rather than promising arbitrary-program proofs.
