# Dogfooding gaps — ledger

**Why this file exists.** Writing real Briev programs surfaced four API/semantic
gaps in three days. Each one lived only in a pull-request description, so the
fifth would have been lost the moment its branch was deleted. This is the
durable home: append, do not re-discover.

Format: gap · what it cost the author · status · where the fix lives.

| # | Gap | Symptom the author hit | Status |
|---|---|---|---|
| G0 | `check` ignored `--stdlib-path` | a file outside the tree could never be typechecked (`std/types/bootstrap.bv` unresolvable) — and `check --stdlib-path X f.bv` used the flag itself as the FILE name | **fixed** `6e2f80150` + `87c2d1d5c` (missing value now errors; the duplicate `BRIEV_STDLIB_PATH` fallback removed) |
| G1 | no help after a subcommand | `brievc check --help` → `cannot read '--help'`; every flag list was undiscoverable without reading `src/main.rs` | **fixed** `e1f6f1971` (usage on stdout, exit 0, scan stops at `--`) |
| G2 | `term;` vs `term! ->` inside a `when` guard | a guard ending in bare `term;` is a convergence checkpoint, not an exit — the program kept running past it | **advisory today** (warning names both spellings); a hard gate needs the corpus migrated first |
| G3 | a reactor program could not return a JSON `String` | `serve_respond_json(conn, code, body)` could only answer numeric bodies | **fixed** `603360af7` — the SIGSEGV was a nested call inside `Print#(...)` never being typechecked, not the string machinery |
| G4 | false "will run forever" on every CLI program | `node main [beginprogram][true]` warned `reactive txn 'main' … will run forever` | **fixed** `2abb4b076` (any `beginprogram` conjunct; was only the bare marker) |

## What the gaps had in common

G3 is the load-bearing one: three of the four were **silent** — the compiler
printed nothing and the author got a crash, a wrong exit, or an unusable flag.
The pattern is the same in each case: a code path that decided not to look, and
no gate that could notice. `scripts/gate.sh quick` now builds the binary target
(`cargo check --all-targets`), and `ir_verify` proves call/definition agreement
without consulting the toolchain — both exist because a dogfooding run found what
the suites did not.

## How to use it

When a real program hits something the language cannot express or the compiler
handles wrongly:

1. Append a row here **first** (gap, symptom, status).
2. Repro in the file that names it (`tests/tier1/…` or `tests/fixtures/…`).
3. Only then a fix — with the gap id in the commit message.

Known open work this ledger points at: G2 (promote to a hard gate after the
corpus migration), and the two call-site ABI retirements recorded in
`docs/plans/2026-11-18-compiler-health-program.md` §5.5 (`char_to_str`'s
truncated `i32` argument in 7 programs; five `ptr`/`i64` handle positions).
