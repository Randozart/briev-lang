# Parity harness (plan §6)

Guards the briev_rt.c elimination: after each family migration, the
Briev-native runtime must reproduce byte-identical output to the pre-migration
C-backed compiler.

## Layout

- `corpus/` — dedicated formatting corpora (`.bv`), the precision gates:
  - `float_cases.bv` — pins `float_to_str` to C `printf("%.9g")`
    (NaN/inf/-0.0/denormals/rounding/exponent switches).
  - `int_cases.bv` — pins `int_to_str`/`uint_to_str` (`%ld`, INT64 extremes).
  - `parse_cases.bv` — pins `str_to_int` parse semantics (whitespace, sign,
    overflow, garbage) — return-value parity, not just output.
- `bench.list` — real runtime benchmarks at a BOUND that produces 2-3 print
  lines (`name BOUND`).
- `goldens/` — captured stdout from the CURRENT (C-backed) compiler. **Never
  edit by hand** — regenerate with the capture scripts before a migration.
- `capture.sh <name>` — (re)builds a corpus `.bv` and captures its golden.
- `capture_bench.sh <name> [bound]` — (re)captures a benchmark golden.
- `run.sh [names...]` — rebuilds every corpus+benchmark with the current
  compiler and diffs stdout against the goldens. A family is "done" only when
  every corpus it touches reports PASS.

## Usage

```bash
# full check
bash benchmarks/parity/run.sh
# one corpus / benchmark
bash benchmarks/parity/run.sh float_cases
```

## Rules

1. Capture goldens BEFORE migrating a family (the current compiler is the
   reference).
2. A golden that fails after a migration means the migration broke parity —
   fix the Briev-native implementation, never edit the golden.
3. `briev_rt.c` survives as `lib/runtime/briev_rt.legacy.c` only until the
   last family closes; it exists solely to re-derive an ambiguous golden.
