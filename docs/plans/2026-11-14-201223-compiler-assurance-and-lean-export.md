---
schema: "codex-work-plan/v1"
created_at: "2026-11-14T20:12:23+01:00"
timezone: "Europe/Amsterdam"
status: "planned"
baseline_commit: "8da7e61d2ecda2efa371a7ac8a7652c699cfc17e"
supersedes: []
superseded_by: []
related: ["docs/plans/2026-11-09-compiler-verification-program.md"]
---

# compiler-assurance-and-lean-export

## Purpose

Establish implementation-linked compiler proofs and discover semantic bugs, then explore Briev-to-Lean exports with independently checked guarantees.

## Approach and alternatives

Create an isolated worktree; audit a small production proof component against explicit machine semantics; reproduce and fix discovered bugs; build checked Lean foundations with negative controls before extending to typed-HIR proof export.

- Aeneas extraction connects Rust more directly but requires a compatibility experiment; handwritten Lean models require explicit refinement evidence.

## Scope and documentation

Compiler assurance first; staged Lean export second. No language syntax changes or whole-compiler correctness claim.

## Verification

- Production regressions, independent semantic boundary checks, kernel-checked Lean proofs when tooling is available, negative controls, required Rust gates, Praetor and source-bound evidence.

## Results and evidence

Record actual outcomes, affected contracts/docs, commits and evidence links as work completes.

## Change history

- 2026-11-14T20:12:23+01:00 — Plan created before implementation.
