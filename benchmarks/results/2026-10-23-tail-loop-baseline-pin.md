# Baseline pin — carrier-gated tail-loop

**Date:** 2026-10-23
**New baseline tag:** `compiler-baseline-2026-10-23-tail-loop`
**Baseline commit:** `42279aafef711339dcba4f614588b9cf9a00a3f0`
**Lock commit:** see `benchmarks/BASELINE.lock` after the pin commit.
**Previous baseline:** `compiler-baseline-2026-10-21-carrier` (`5a1a0532`),
retained for rollback and historical comparison.

## Why this is the reference

The carrier-gated tail-loop slice merged as PR #34 after CI passed on both the
PR head and the merged `main` tip.

The merge is performance-safe by construction:

- the exhaustive same-environment corpus check recompiled all 46 standard
  `.bv` benchmarks against the previous baseline;
- **45 emitted `.ll` files were byte-identical**;
- the only difference was the intended `fsm_dispatch.ll` loop lowering;
- therefore non-FSM benchmarks cannot regress from this compiler change;
- `fsm_dispatch` improved from the prior carrier baseline to **0.3969x** in
  controlled A/B, with zero hot-path enum mallocs.

## Pin validation

After repointing `../briev-compiler-baseline` to the new tag and rebuilding:

- baseline and current both report commit `42279aaf`;
- the post-pin `fsm_dispatch` comparison passed lock enforcement;
- the same-commit FSM `.ll` files are byte-identical;
- the observed `0.8116x` same-commit timing ratio is within machine noise and
  is not evidence of a code change.

## Content at the pin

- direct self-tail recursion with loop-carried nested enum state lowers to an
  explicit loop;
- carrier ownership is seeded at entry and copied at the recursive boundary;
- plain tail recursion remains on the existing recursive path and LLVM TCO;
- full runtime suite: all MATCH;
- library tests: 3,301 green;
- effect-shape quick guard and LLVM determinism quick gate: PASS.

The lock is intentionally explicit. Future performance claims compare against
this commit, not against a moving main.
