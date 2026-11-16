# Open-issues audit — 2026-11-15

Checked all 9 open issues in `Randozart/briev-lang` against the work that landed
on `main` over 2026-11-13 → 2026-11-15. No open PRs remain (PR #100 was closed
this session; #94/#96/#97 are merged).

## Issue-by-issue

| # | Title | Resolved by recent work? | Notes |
|---|-------|--------------------------|-------|
| **5** | check succeeds while build fails (check path stops before normalization/codegen validation) | **Unknown — likely NOT** | The check-path/normalize/codegen gap is a separate concern from the div/0 guard, aggregate-handle ABI, or float-relational fixes. Needs a targeted repro to confirm. Filed by `@odecode` from `test-project-otto` (2026-08-03/04). |
| **6** | LLVM backend emits mixed pointer/integer operand types in compare and print lowering | **Partially — the compare side is fixed** | PR #97 (merged) fixed Float relational comparison in the **interpreter** (f64 domain). The **LLVM backend** compare type-mismatch is a distinct emission bug. However, the float div/0 guard work (`3a2fea32f` + `74311bce0`) added the mixed-operand adapter that normalizes Int/Float operands before `fcmp`/`fdiv`, which may have fixed the compare side. The **print** side (pointer-typed print receiving non-pointer) is not addressed by any of the recent commits. **Needs a targeted repro to split compare vs print.** |
| **7** | LLVM emission references unresolved state/local symbols in outlined/nested control flow | **Unknown** | No recent commit specifically targets outlined/nested control-flow symbol resolution. The aggregate-handle ABI fix (PR #94) touched call-boundary marshalling but not state-load resolution in outlined paths. Needs a repro. |
| **8** | term in transaction guard flow emits contradictory outputs (reject + success) | **Unknown** | No recent commit addresses guard-flow convergence. This is a control-flow correctness issue, orthogonal to the float/div/ABI work. |
| **9** | Fixed aggressive clang flags increase fragility for large IR; no safe fallback | **Unknown — likely NOT** | The recent work used the benchmark harness's exact flags (`-O3 -flto -march=native -ffast-math …`), but did not add a low-optimization fallback path to the build system. `src/compile.rs:1455` (cited in the issue) is unchanged. |
| **10** | String concat ambiguous: `+` rejected, concat maps to `++` | **Unknown** | No recent commit touches string concat operator mapping (`src/type_universe/operators.rs:31`). |
| **14** | DRY: centralize three `llvm.memcpy` emission sites into one `emit_memcpy` helper | **Unknown — likely NOT** | No recent commit refactors the memcpy emission sites. This is a Rule 17 (DRY) follow-up from #13. |
| **4** | Issue compiling on Mac | **Unknown** | Platform-specific; no macOS CI or recent Mac-targeted fix. Filed by `@l3nz` (2026-08-09). |
| **2** | Feature: compile `#export` defns into a C-callable static library | **Unknown — open feature request** | No recent commit implements `#export` → static lib. Filed by `@revred` (2026-07-11). |

## Summary

- **Most likely resolved (with caveats):** Issue #6 (compare side only, via the mixed-operand adapter in the div/0 guard work). The print side and the interpreter-vs-backend distinction need a targeted repro to confirm.
- **All others: unknown or likely not addressed.** None of the 2026-11-13→15 commits (float div/0, float relationals, aggregate ABI, Z3 CI, integer wrapping, Float64 printing, counsel contracts, parity corpus fix) directly target issues #5, #7, #8, #9, #10, #14, #4, or #2.
- **No open PRs** — the repo is in a clean state for picking up issue work.

## Resolution (2026-11-15, post-action)

All 9 issues were audited, investigated, and dispositioned:

| # | Disposition | Commit / evidence |
|---|-------------|-------------------|
| **6** | **CLOSED (fixed)** — verified working: float compare, mixed Int/Float compare, print lowering all correct | `adapt_mixed_float_operands` (2026-09-02) + print path |
| **10** | **CLOSED (fixed)** — `a + b` on String works, verified end-to-end | `("String", "Add") => StringConcat#` in operators.rs (2026-08-03) |
| **14** | **CLOSED (fixed)** — `emit_memcpy` helper in helpers.rs; 5 sites migrated; IR byte-identical | `6c39cbe2c` |
| **9** | **CLOSED (fixed)** — `--safe` flag: `-O0 -march=x86-64`, no LTO, no fast-math | `7a27c91a9` |
| **4** | **CLOSED (fixed, needs Mac confirm)** — `SysCall#` c_long cast; `shm_open` error already archived | `ad5e03330` |
| **5** | **OPEN (cannot repro)** — needs `test-project-otto`; verifier exists; commented | issue comment 6893435083 |
| **7** | **OPEN (cannot repro)** — needs `test-project-otto` CF shapes; verifier catches; commented | issue comment 6893435086 |
| **8** | **OPEN (cannot repro)** — 3 guard+term patterns all correct; commented | issue comment 6893435144 |
| **2** | **OPEN (v2 scope)** — v1 (Int/Float defns → C-callable .a) works; v2 needs Bool/String marshalling + header + CI test | issue comment 6893651767 |
