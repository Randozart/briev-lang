# cargo-fuzz integration — measured results

## Environment

- Revision: `56afe57e` + this change set (`integration/journey-main`).
- Toolchain: rustc 1.100.0-nightly (2026-08-20), `cargo-fuzz 0.13.2`,
  `libfuzzer-sys = 0.4.13` (pinned), Ubuntu clang 21.1.8, x86_64 Linux.
- Build: `cargo +nightly-2026-08-21 fuzz build` — **Finished in 42.04s**,
  all three targets produced.

## Instrumentation size

The linked fuzz binaries instrument the compiler crate and its dependencies.
`interpreter` reports **159,016 inline 8-bit counters / PCs**; `frontend` and
`arithmetic` instrument a comparable set. Coverage numbers below are edges hit
within that instrumentation — an internal measure, not a statement about
compiler completeness.

## Smoke runs (the CI PR lane)

| Target | Budget | Executions | Exec/s | New corpus units | Edges | Findings |
|---|---|---|---|---|---|---|
| `frontend` | 31 s | 112,537 | 3,630 | 3,843 | 5,142 | none |
| `arithmetic` | 61 s | 34,844 | 571 | 285 | 1,518 | none |

`frontend` explores lexer/parser branches only; it cannot see codegen. It found
no panic or hang in >100k mutations, consistent with the existing proptest
`fuzz_frontend` coverage.

`arithmetic` exercises signed `+ - * / %`, guarded returns, and `% 97` bounds
across the interpreter and the independent model. **Zero disagreements in
34,844 executions** (61 s) and again in **106,038 executions** (301 s) — the
oracle is validated for the supported subset, and the subset itself is clean at
this budget.

## Discovery run: independent rediscovery of two open defects

The `interpreter` target adds tuple projection/destructuring. From a **neutral
four-byte seed `00 00 00 00`** (no hint of the defect) libFuzzer found a
disagreement within **42 executions**:

```
thread '<unnamed>' panicked at src/semantic.rs:88:5:
assertion `left == right` failed: semantic disagreement; input=[6, 6, 249, 249]
```

`cargo +nightly fuzz tmin interpreter <crash>` could not shrink below the same
4 bytes — already minimal (one initial byte + one operation triple).

Two minimized reproducers, each **4 bytes**:

| Input (hex) | Decoded | Failure | Matches |
|---|---|---|---|
| `06 06 f9 f9` | `pair.1` | interpreter rejects positional `.1` | BUGS.md tuple projection (open) |
| `00 07 00 00` | `let (l,r) = pair` | `right` unbound | BUGS.md tuple destructuring (open) |

Both are the **same root causes already recorded** — this is rediscovery, not a
new defect. That is still the point: the fuzzer reached them **from raw bytes,
with no per-bug test written**, and minimized them automatically.

## What this does and does not establish

- Establishes: three target harnesses build and run; the oracle is unit-tested
  and agrees with the interpreter on the non-tuple subset; libFuzzer rediscovered
  two known defects and minimized them to 4 bytes; the frontend survives 112k
  mutations; the non-tuple arithmetic subset stays clean across ~141k executions.
- Does **not** establish: exhaustive parser coverage, correctness of the
  non-tuple subset beyond the stated bounds, native-code correctness (that is
  `deterministic_transitions`), or any performance claim.

## Cost

One-time nightly + `cargo-fuzz` install; ~42 s to build; the blocking PR lane
is bounded to 30 s + 60 s. The nightly lane uses `-fork=$(nproc)` over a
matrix of targets and uploads findings. Corpus growth is cached between runs.

## Follow-ups (not in this PR)

1. A valid, contract-bearing program generator to fuzz the typechecker/proof
   engine — the layer most likely to hide new defects.
2. Wire minimized `fuzz/cases/` inputs into the deterministic harness so a
   rediscovered defect also gets a native regression.
3. Keep the `interpreter` target red until its defects are fixed.
