# Defn Liveness Emission

**2026-09-13.** The compiler emits only definitions transitively reachable
from live code. Dead-code elimination is a compiler decision, never
delegated to LTO.

## The contract

**Imports grant capability; liveness gates emission cost.** Importing a
stdlib module makes its types and functions *available* — it does not put
them in the binary. A definition is emitted iff it is reachable from the
roots below. There is no flag to win and no flag needed: this is the
default on every target, hosted included. A function nobody can reach is
dead code — the compiler is right to eliminate it (observability-as-liveness
pillar, applied to functions instead of values).

## Roots

| Root | Why it is always emitted |
|---|---|
| Reactive `node`/`txn` (incl. obj/cell members) | the reactor fires these by name |
| Synthesized `__init` | wraps top-level statements |
| `#export` items | ABI surface |
| ISR handlers | vector tables reference the symbols |
| `asm<…>` functions | top-level observable asm |
| op members / impl members | dispatch names are mangled at emission — conservative keep |
| spawn targets (`spawn Enemy(...)` etc.) | fn-pointer tables |
| any defn, when live code uses `.^^` reflection | reflection reaches members by name |

Everything else is live iff reachable through the call graph (explicit
`Expr::Call` edges) plus the intrinsic→helper table.

## The intrinsic→helper table

Intrinsic lowerings may emit calls to pure-Briev helper defns
(`Print#` → `__print_int` → `write_all` → `int_to_str`; foreach over
String → `briev_str_next_char`; `==` on strings → `briev_str_eq`; …).
That mapping lives in exactly one place:
`src/analysis/defn_liveness.rs :: intrinsic_helpers()`. Adding a row is a
data change, never a pass change.

**Safety net:** after emission the backend scans the IR for calls to
program defns that were not marked live and fails the compile with the
name to add. An incomplete table is an immediate compiler error — never a
silent linker failure.

## What this fixed

The rv64 bare-metal gap (plan `2026-09-13-defn-liveness-emission.md` §1):
the prelude imports 14 stdlib modules; the backend used to emit all ~250
defines and rely on LTO for DCE. `-nostdlib` links failed without LTO
(undefined `malloc`/`memcpy` from *unused* stdlib code) and LTO broke
riscv64 relocations instead. With liveness emission a 3-function program
emits 3 functions, freestanding, no LTO required.

## Interplay with `--no-std`

Different layers. `--no-std` = "do not even import the prelude" (no
capabilities). Liveness = "emit only what imported code is used"
(no emission cost). Both default-honest: `--no-std` disables the whole
prelude family (`prelude`, `prelude-native`, `prelude-hw`,
`prelude-electronics`), not just the one plugin name.

## Consumed by

- LLVM backend Definitions/callable-txn emission gate
  (`src/backend/llvm/mod.rs`).
- `AnalysisResults.defn_liveness` — computed in `analyze_program`,
  backend consumes, never re-derives (frontend-driven dispatch).
