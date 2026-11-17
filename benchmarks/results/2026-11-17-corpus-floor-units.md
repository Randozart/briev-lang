# 2026-11-17 — corpus floor, restated in defined units

Plan: `docs/plans/2026-11-17-three-open-threads.md` §3 (thread 3). Supersedes the
numbers in `2026-11-17-corpus-err-triage.md` §§2–3, which mixed units.

## 1. Why this file exists

The recorded floor — "242 files / 210 PROVEN / 18 UNPROVEN / 34 ERR" — could not
be reproduced, and the reason is not a lost regression: **no bucket in that
sentence shares a unit with any other.** The corpus script counted

| bucket | unit |
|---|---|
| `PROVEN` | file |
| `UNPROVEN` | goal |
| `SKIPPED` | goal |
| `VIOLATED` | goal occurrence |
| `ERR` | error line |

and the census script counts goals throughout. Comparing any two of those is
meaningless, which is why every "the corpus is greener" claim so far has been
untestable.

The runner now prints two labelled blocks — `files_*` and `goals_*` — and the goal
numbers come from `--verify-report` JSON (the same source the census uses), not
from grepping human-readable lines.

## 2. The floor (2026-11-17, after class D + C1/C2/C3 + the units fix)

```
=== VERIFY-BODIES CORPUS ===
Files:              243

files (one per corpus file):
  files_proven:      63      # no failure of any kind
  files_failing:     5       # the build emitted an error
  files_unproven:    176     # >= 1 goal the prover could not decide

goals (one per obligation, from --verify-report):
  goals_proven:      5348
  goals_unproven:    207
  goals_skipped:     34559
  goals_violated:    0       # a contract falsely proved - GATE

lines (diagnostic occurrences):
  no_postcondition:  0
  skipped_lines:     34559
  error_lines:       7       # > files_failing when a file emits several
```

**Gates:** `files_failing` and `goals_violated` only. The first is "this file does
not build"; the second is "a contract was falsely proved". `goals_proven` moves
when prover coverage changes, so it is a progress indicator, never a gate.

## 3. What changed in this pass

Class D fixed the two stdlib files that had never compiled:

| file | was | now |
|---|---|---|
| `lib/std/posix/io.bv` | `use of undefined value '@FD_STDOUT'`, then `redefinition of global '@SYS_CLOSE'` | builds clean |
| `lib/std/posix/process.bv` | `redefinition of global '@SYS_CLONE'` | builds clean |

Two distinct defects, both now fixed:

1. **`export let` was invisible to both storage walks.** `classify_plain_lets`
   (`src/analysis/storage_class.rs`) and the `let` arm of `build_field_index`
   iterated `TopLevel::Statement` only; `TopLevel::Export` is unwrapped in seven
   places in the backend and was in neither. So an exported let was classified
   nowhere, and every read fell to the identifier read's last resort, which
   emitted `load … ptr @X` for a symbol nothing declared. Both walks now share
   `storage_class::let_of_item`, and the last resort no longer emits a reference
   to an undeclared symbol: it warns with the cause and materialises a zero, so
   the module stays valid instead of failing at `opt`.
2. **The stdlib re-declared syscall numbers.** `io.bv` imports `syscall.bv` and
   re-declared six `SYS_*` names as `export let` while `syscall.bv` exports them
   as `export const`; `process.bv` did the same for `SYS_CLONE`/`SYS_EXECVE`. Both
   declarations reached the module, producing two definitions of one symbol. The
   duplicate declarations are removed — the import already made them visible, and
   `syscall.bv` stays the single home for syscall numbers.

`files_failing` went from ~18 to 8 with class D (the two stdlib files plus three
examples that advanced past their first error), then to 5 with class C.

## 4. The 8 files still failing, by class

| class | files | error |
|---|---|---|
C1, C2 and C3 are fixed (see the commit series); `ptr-arithmetic`, `ptr-swap` and
`sig_as_type` now build and link, and `wildcard_lift.bv` — which imports them —
went clean with them. What remains:

| class | files | error |
|---|---|---|
| B survivor | `examples/syntax/meta/defn-composite.bv` | `integer/byte constant must have integer/byte type` (a *second* site; the Decimal-into-float-slot init is fixed) |
| E link | `examples/ptr-runtime.bv` | `linker command failed` |
| F source | `examples/mmap-demo.bv`, `networking.bv`, `process-spawn.bv` | `type errors:` — the example sources themselves, not codegen |

`files_failing` went 18 → 8 (class D) → 5 (class C). The three `type errors:` files
are example-source staleness, a different lane from codegen.

## 5. What the historical numbers probably were

`goals_proven` for this corpus is 5348, and `goals_unproven` is 207 — neither is
210 or 18. The census-style per-goal tally cannot have produced "210 PROVEN"
for *this* file set. Combined with the script's own header note (an accident once
appended the report three times, tripling every per-goal count), the honest
conclusion is that the historical sentence is not attributable to any reproducible
invocation of either tool. It is superseded, not corrected.
