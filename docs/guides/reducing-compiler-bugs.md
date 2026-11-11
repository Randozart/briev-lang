# Reducing compiler-bug reproductions (P6)

When the compiler or a backend misbehaves, shrink the input before fixing it,
then pin the minimal program as a regression.

```bash
# 1. Pick the predicate that matches the bug
scripts/interesting.sh ice        bug.bv   # compiler crash/panic
scripts/interesting.sh diagnostic bug.bv   # compile error that is not a crash
scripts/interesting.sh opt        bug.bv   # -O0 and -O3 disagree at runtime

# 2. Reduce
scripts/reduce.sh ice bug.bv
scripts/reduce.sh opt bug.bv
```

`reduce.sh` prefers `cvise`, falls back to `creduce`, and writes
`bug.min.bv`. Install one (`apt install cvise`) — the script exits 3 if neither
is present.

## Promotion

- Compile/diagnostic bugs → `tests/compile_fail/<name>.bv` with
  `EXPECT-ERROR: <substring>` (required substrings, not exact text).
- Runtime/optimizer bugs → `tests/directives/<name>.bv` with
  `EXPECT-STDOUT:` pins, or a focused `tests/*.rs` differential.
- Record the root cause in `BUGS.md`; the reduced fixture is the proof.

The predicates use `$BRIEVC` when set (CI: the built binary), else
`target/debug/brievc`.
