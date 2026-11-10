# Composed semantic discovery

Extend the upstream deterministic transition harness rather than duplicate its
runtime-input execution, deadlines, shrinking, replay, and artifact capture.

The first increment generates sequences of bounded integer operations, including
state-dependent guards, and checks interpreter and native traces against an
independent Rust evaluator. Sequence shrinking reduces interactions as well as
inputs. Existing replay cases remain compatible through a defaulted field.

Validation: known-answer oracle tests, a seeded family-13 native campaign,
`cargo test --lib`, `cargo build`, and changed-directory Praetor checks.
This is correctness tooling, not a compiler performance change.

Documentation: update the deterministic-testing guide with the new family,
bounds, replay schema, and oracle limitations. Preserve existing rationale
comments and historical plans. No compiler architecture API changes.

## First increment results

- Base: `0f66fe48` (`origin/main`); branch `test/semantic-differential-discovery`.
- Family 13, seed 1, 16 generated cases: interpreter/model/native O0/O3 agree.
- Known-answer and native composition smoke tests pass.
- Library suite: 3573 passed (265.87 seconds); the first invocation hit its
  200-second tool deadline, so the complete run used a longer deadline.
- `cargo build`, `git diff --check`, and `praetor validate --warn --target tests`
  pass.
- Remaining expansion: typed recursive expressions, multi-node reactor
  scheduling, and compiler-pass configuration variation. This increment varies
  LLVM optimization levels on one emitted module, not Briev's own passes.
- Remote fork synchronization is blocked by invalid GitHub authentication;
  local main was fast-forwarded successfully in its existing worktree.
