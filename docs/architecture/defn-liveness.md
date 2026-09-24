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
| Synthesized init / top-level statements | emitted directly into the init path |
| `#export` items | ABI surface |
| ISR handlers | vector tables reference the symbols |
| `asm<…>` functions | top-level observable asm |
| Type/obj behavioral members — **usage-triggered**: rooted when live code CONSTRUCTS the type (`HashMap { … }`, `spawn Enemy(…)`, ctor call) | prelude collection implementations must not leak into programs that construct nothing |
| spawn targets (`spawn Enemy(...)` etc.) | fn-pointer tables |
| cast/proto binding functions (named in `proto`/type metadata) | the casting graph calls them at emission |
| every callee of top-level statements | their bodies join the closure from birth |
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
riscv64 relocations instead. With liveness emission the idiomatic
foreach-on-String hello emits **13 defines** (from 252) — `entry` →
`uart_write` → `briev_str_next_char` → `byte_raw`/`decode_*` — and boots
freestanding in QEMU, no LTO. Plan §3a records the implementation
findings: usage-triggered member rooting, the net's first real catch
(getenv adapters), and the embedded-main argc-capture gate.
2026-09-23 (frgn-elimination round 2): the getenv C-ABI adapters were
DELETED with the ghost env frgns — get_env!/get_env_int! now call the
pure-Briev briev_getenv_{briev,int}_impl walkers (cast_lanes.bv) directly,
threading the compiler-owned `@__briev_environ` via the `Environ#()`
intrinsic. The liveness table's `Environ#` row roots the two impls (a
runtime get_env!() keeps them emitted).

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
