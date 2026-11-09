# Real-defect transition campaign

## Goal
Run the deterministic harness without synthetic corruption, broaden its bounded
transition vocabulary, and report independently replayed defects rather than
claiming correctness from a small passing sample. Preserve the existing staged
demonstration and unrelated untracked work.

## Method
Add replay-compatible transition families covering signed division/remainder,
nested guards, local mutation, short-circuiting, and collection indexing.
Expose case-count and family controls; hold family fixed while shrinking so a
counterexample remains attributable. Keep integer magnitudes and divisors safe.
Run independent seeds with separate artifacts, replay findings, inspect emitted
IR, and distinguish invalid generators, reference limitations, and miscompiles.
Record confirmed defects in BUGS.md and a campaign results document with small
replay inputs. Compiler fixes are not part of this requested investigation.

## Checks and documentation
Run focused harness checks, native O0/O3 executions, library tests, build,
Praetor, and the quick determinism gate for changed test infrastructure. Update
the usage guide with campaign controls, retain its scope limitations, and keep
existing rationale comments. This is correctness testing, not a performance
optimization; there are no benchmark speed claims.

## Results
Thirteen families exercised with seed 42; nine completed eight cases each.
Those nine completed another four cases each with seed 2026: 108 passing
generated cases across both native optimization levels. Four failing families
yielded minimal replay fixtures: short-circuit division (native SIGFPE), list
rebinding mutation (native/reference output disagreement), tuple projection
(reference error), and tuple destructuring (reference missing binding).
All four were confirmed with the typed frontend; native artifacts were rebuilt
independently at O0/O3. Full report and root-cause evidence:
`docs/reports/2026-11-09-deterministic-transition-campaign.md`.

Added explicit ignored reference reproducers for the two open tuple defects;
the default smoke and supported-family oracle tests remain normal tests. Saved
JSON cases for all four findings intentionally fail when replayed. No production
compiler files were modified during this investigation.

Infrastructure verification: focused suite 5 passed / 2 explicitly ignored open
defect reproducers; `cargo build` passed; Praetor (`--target tests`) passed;
quick LLVM determinism passed all three corpora; `git diff --check` passed.
The full library suite also passed: 3553 tests, zero failures (333.21 seconds).
