# Concurrency and Modifiers

**2026-07-31.** The modifier family (`seq`, `vol`, `async`, `sync<group>`), the
concurrency gate, and the principle they serve. Companion to `AGENTS.md` rules
#2, #20, #21.

## The principle

The compiler has special treatment — intrinsics, hashwords, reflection,
directives — and it does not pretend otherwise. What it forbids is *hiding*
that treatment behind ordinary-looking syntax.

- **Compiler-knowns are disclosed** with markers: `#` (intrinsic `Sqrt#`,
  hashword `Int`), `!` (compile-time expansion `my_macro!`), `.^`/`.^^`
  (reflection).
- **User-facing directives are ordinary keywords** — `seq`, `vol`, `async`,
  `sync<group>` — requiring no special computer-science knowledge to read.
- **No instruction may ever make code faster.** A modifier exists only to
  restrict the optimizer or demand a specific behaviour. The default is always
  the efficient path. A `seq`/`vol` program beating the default is a compiler
  bug: fix the default, never let the modifier be the win. This rule is what
  keeps the language free of an opaque optimization layer only advanced
  compiler engineers understand.

## The delimiter semantic load

Every delimiter carries exactly one meaning:

| Delimiter | Load | Examples |
|-----------|------|----------|
| `<>` | **compile-time type-level specialization** | `Stack<T>`, `String<UTF8>`, `asm<x86_64>`, `sync<group>` |
| `()` | **application & binding** | `f(a)`, `defn f(x: Int)`, `Person(...)`, `op Add: func(#L,#R)`, `op Add(Float)` (declarations take params) |
| `[]` | **containment / bound** | `Int[8]`, `[pre]` guards |
| `{}` | **grouping / definition** | blocks, struct literals |

`sync` is a compile-time identity (which group) — the same shape as `asm<chip>`
(which target) and `String<UTF8>` (which variant) — so it is `sync<group>`,
not `sync(group)`. `op Add(Float)` stays parenthesized: `op` is a nested
declaration, declarations take params, and it avoids angle-bracket nesting.

## The modifier family

All modifiers are **prefix** (`seq foreach`, `vol let`, `async node`). They are
never postfix (`node async` is rejected).

### `seq` — ordering, layout, sequence

"Calm down — do this in order, predictably." `seq` never enables an
optimization; it only disables or pins.

| Form | Meaning |
|------|---------|
| `seq struct Name` | declared field layout is preserved — `apply_field_modes` does not reorder, compact, or dead-eliminate |
| `seq txn foo` / `seq node foo` | sequential dispatch — never the parallel reactor |
| `seq Int[N]` | the array is accessed sequentially — no vector loads/stores |
| `seq foreach` | scalar loop with `!llvm.loop.vectorize.enable = false` |

### `vol` — memory visibility

`vol let x` emits every read and write as `load volatile`/`store volatile` —
never folded, never promoted, externally observable. The existing MMIO
volatile machinery is reused.

`seq` and `vol` are orthogonal and combinable: `vol seq let Int[x]` is a
volatile *and* sequential array — each access sequential, each access volatile.

### `async` — explicit simultaneous firing

`async` is not a compiler hint. It is an explicit acknowledgement that two (or
more) nodes may fire simultaneously. It is a semantic declaration.

### `sync<group>` — the group barrier

`sync<group>` marks a node as a member of a firing group. When multiple members
of the same group fire in one step, every synced member **holds off finishing
until every member of the group that fired has finished** — a group commit /
join point. No member's writes are observable until the whole fired group has
written. This is distinct from `async` (independent completion) and from plain
sequential (immediate per-node visibility).

## The concurrency gate (NO IMPLICIT CONCURRENCY)

The reactor never silently decides whether two reactive nodes may fire
together. For any pair of reactive nodes A and B:

1. If the proof engine proves `pre_A ∧ pre_B` **satisfiable** (both can be true
   at the same time), AND
2. there is **no XOR read-write overlap** between A and B (no field one writes
   that the other reads),

then the pair is **eligible to fire together**, and the compiler DEMANDS a
classification:

- `async node A` and `async node B` — explicit acknowledgement of simultaneous
  firing, or
- `sync<group> node A` and `sync<group> node B` — the same group barrier.

An eligible pair that is neither is a **hard error**:
`nodes A and B can fire together; declare 'async' on both or 'sync<group>' on both.`

The two escapes from the gate are the two ways a pair is provably safe without
a classification:
- the proof engine proves the preconditions **mutually exclusive**
  (`pre_A ∧ pre_B` is UNSAT), or
- an **XOR read-write overlap** forces them sequential by data dependency.

## Implementation notes

- **2026-08-01 (Phase 3c): the gate is ENFORCED.** `run_concurrency_gate` in
  `src/analysis/concurrency_gate.rs` runs after typechecking and is a hard
  compile error for any unclassified eligible pair (rule #21). `async node`
  (prefix) now preserves the async flag; `sync<group> node` is parseable
  source. Benchmarks with multiple auto-firing nodes were audited and
  classified (`async node` for parallel, `sync<group>` for barrier).
- Modifier lexing/parsing is prefix; `node async` (postfix) is rejected.
- The gate analysis reuses `check_satisfiable` (proof engine) for the SAT test
  and the write/read conflict analysis (`collect_assigned_identifiers` /
  `collect_read_identifiers`) for the XOR overlap.
- `sync<group>` barrier codegen: the fired group members buffer their writes;
  the group commits at the join point (sequential reactor: a group commit;
  parallel reactor: a thread join).
- The never-faster contract is a regression test: the default output is never
  slower than the `seq`/`vol` output.
