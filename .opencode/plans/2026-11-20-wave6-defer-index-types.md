# Wave-6 plan — Goose borrowables: defer, index types + checkfree, clock intrinsic, verify-on-load (2026-11-20)

**Owner-approved scope** (confirm gate passed): the four near-term items
from the Goose research notes (.opencode/plans/2026-11-20-goose-research.md
§9, items 1–4). Parked items (varint, relative refs, per-variant enums,
vDSO, guarded allocs) stay parked.

## Work items

### W6-1 `defer` statement (new keyword: `defer`)
Syntax:
```briev
defer buf_free(sa);
defer { cleanup(a); b = 0; }
```
Semantics (block-exit scope, this wave):
- A `defer` statement registers its body for the enclosing statement block.
- Bodies run LIFO when the block exits — via fallthrough OR via a guard
  `term;`/`term <v>` exit that leaves the block.
- Bodies written before an `endprogram` statement run before it.
- Per-iteration loop semantics: follow-up, documented as deferred.
- The deferred body must be a call statement or a block of call/assign
  statements (no `term`, no guards inside defers this wave).

Implementation surface:
- AST: `Statement::Defer(Vec<Statement>)`.
- Parser: `defer` keyword in statement position.
- Interpreter: block executor collects defers, runs LIFO at list end and
  on TermReturn propagation through the block.
- Native (emit_stmt): at block end (fallthrough) emit deferred bodies;
  before guard-term abort branches; before `endprogram` calls in the same
  block.

### W6-2 Index types + `checkfree`
Syntax:
```briev
let i: u32 in xs = xs.Count#();   // construct from the collection's Count#
let v: Int = xs[i];               // in-bounds by the type; no runtime check
defn total(xs: List<Int>) checkfree -> Int { ... }   // proof obligation
```
Semantics:
- `u32 in <coll>` constructs only from `<coll>.Count#()`, from another
  value already typed `u32 in <coll>`, or (inside a `foreach i in
  0..<coll>.Count#()`-style context) never implicitly.
- Subscript `coll[i]` with `i: u32 in coll` emits the inbounds access
  WITHOUT a runtime bounds check (the type is the proof) and the
  interpreter suppresses its OOB error for that subscript.
- Subscript with a plain Int keeps today's semantics (interpreter OOB
  error; native inbounds UB, per the §11.4 note).
- Constructing `u32 in a` from `b.Count#()` (wrong collection) is a type
  error.
- `checkfree` on a defn: every subscript on a parameter/local collection
  in the body must use an index-typed (or statically in-range literal)
  index; violations are compile errors naming the line.
- SPEC §15-analog section documenting index types + checkfree.

### W6-3 `ClockMonotonicNs#` intrinsic
- Registry + signature `(-> Int, post >= 0)`.
- Per-triple number via `resolve_syscall_number`: add `ClockMonotonic` to
  the tables (x86_64=228, asm-generic/aarch64=113).
- Stdlib: `os/time.bv` switches `rt_now_ns` to it (the aarch64=113 TODO
  at cast_lanes.bv:1221 retires).

### W6-4 FromBytes#/ToBytes# stdlib family (over SPEC §16.2 ABI)
- `lib/std/memory/verify.bv` (new): `data_check(word: Int, max_len: Int)
  -> Bool` — over the frozen ABI ([i64 len][bytes][NUL], payload @8):
  length word >= 0, length + 9 <= a caller-supplied region bound, NUL
  byte present at word + 8 + len. Plus `bytes_view(word)` helpers.
- Fixture: round-trip a String through Data; tampered length and NUL
  rejected.

## Invariants (each pinned by a named test)

1. **INV-DEFER-LIFO** — two defers in one block run latest-first at block
   exit. Test: `tests/tier1/defer_lifo.bv` golden `b|a`.
2. **INV-DEFER-GUARD** — a guard `term;` exit still runs the defers
   written before the guard in the same block. Test:
   `tests/tier1/defer_guard_exit.bv` (golden includes the deferred mark).
3. **INV-DEFER-ENDPROGRAM** — defers written before an `endprogram` in the
   same block run before it. Test: `defer_guard_exit` variant.
4. **INV-IDX-CONSTRUCT** — `let i: u32 in xs = xs.Count#();` typechecks;
   constructing from another collection's Count# is a type error. Tests:
   `tests/tier1/index_types.bv` (positive) + typechecker test
   `index_type_wrong_collection_rejected` (mod typechecker tests).
5. **INV-IDX-NOCHECK** — a subscript through an index-typed value reads
   in-bounds on both engines with no OOB error (interpreter suppresses;
   native plain inbounds). Test: `tests/tier1/index_types.bv` golden.
6. **INV-IDX-PLAIN-KEEPS-SEMANTICS** — plain Int subscripts keep today's
   semantics (interpreter loud error). Existing OOB tests + new negative
   test in the typechecker/interpreter suite.
7. **INV-CHECKFREE** — a `checkfree` defn with an unguarded plain-Int
   subscript on a parameter collection fails to compile with the site
   named; an index-typed version compiles. Tests:
   `index_types_checkfree_rejects_plain`, `..._accepts_index_typed`.
8. **INV-CLOCK** — `ClockMonotonicNs#` returns >= 0 and is monotonic
   across two reads (t2 >= t1). Tests: unit test on the per-triple number
   (`clock_monotonic_numbers_per_triple`); smoke fixture printing two
   reads with t2 >= t1.
9. **INV-VERIFY** — `data_check` accepts a well-formed String→Data word
   and rejects a tampered length word / missing NUL. Tests:
   `tests/tier1/data_verify.bv` (golden `ok|bad|bad`).
10. **INV-NO-REGRESSION** — full lib suite + quick gate + CI green on
    every landed commit (existing suite).

## Named tests

- `tests/tier1/defer_lifo.bv` + `.golden` (`b|a`)
- `tests/tier1/defer_guard_exit.bv` + `.golden` (deferred mark on guard exit)
- `tests/tier1/index_types.bv` + `.golden` (construct, read, write, iterate)
- `tests/tier1/float32_storage_width_tests.bv`-style test blocks:
  `index_types_tests.bv` (interpreter parity for index reads)
- `tests/tier1/data_verify.bv` + `.golden` (`ok|bad|bad`)
- typechecker unit tests: `index_type_wrong_collection_rejected`,
  `index_types_checkfree_rejects_plain`, `index_types_checkfree_accepts_index_typed`
- intrinsics unit test: `clock_monotonic_numbers_per_triple`
- smoke: `ClockMonotonicNs#` monotonic pair fixture
  `tests/tier1/clock_monotonic_smoke.bv`

## In-scope syntax (confirmed with the owner)

One keyword (`defer`), one type suffix (`u32 in <coll>`), one modifier
(`checkfree`). Items W6-3/W6-4 add zero syntax. No operator/delimiter
changes; SPEC sections updated for each (new §Index types + checkfree
under §10/§15-analog, §11.4 cross-reference, §17 intrinsic table row).

---

## Execution record (2026-11-20, landed)

| Item | Commit | Notes |
|---|---|---|
| W6-1 defer@endprogram | `e470dba59` | defer already existed (2026-08-09, transaction-scoped LIFO); `endprogram` was the ONE exit that never flushed it. Added the `flush_defer_cleanup` call before the value's side effects. SPEC §11.5 note + `tests/tier1/defer_lifo.bv` (golden `body|a|b`). |
| W6-2 index types | `4d0a8d7f5` | `Type::IndexIn(coll, elem)`; parse (`u32 in xs`, UInt/u32/U32); Display; 8 downstream arms; construction rule (only `xs.Count#()` or same-typed value); arithmetic routes to the carrier and degrades; **protocol transparency in `casting_graph::type_to_protocol`** — without it an index resolved to the Data category and `Print#(i)` emitted `bits_to_str` of an integer (segfault; caught by the fixture). |
| W6-3 ClockMonotonicNs# | `0d412e07a` | rt helper `briev_clock_monotonic_ns`; per-triple numbers (x86_64 228, asm-generic 113) in `resolve_syscall_number`; declare-guarded in `emit_toplevel`; registry + signature; supported-ops set (normalizer); monotonicity smoke. |
| W6-4 data_verify | `f9068a797` | `data_check(word, max_bytes)` / `data_len`; native `1|0|0`; interpreter parity via test blocks. |
| SPEC | `7ff6fec2b` | §15.4a index types; §17.3 intrinsic + data_verify notes; §11.4 cross-reference. |
| Reference doc | `8dfa58c67` | The vocab gate (registry→doc one-directional check) caught the missing `ClockMonotonicNs#` row. |

### Traps found (each cost a debug cycle, all recorded in comments)

1. **`peek_next()` vs `peek()`** — after the type-name match consumed the
   name, the cursor sits ON the next token; `peek_next()` looks one PAST it.
   The `in` guard and the collection-name read both needed `peek()`.
2. **The `u32` spelling was not in parse_type's name table** — it fell to
   `parse_named_type_body` (Custom), so `prim_name` never bound. Added
   `"UInt" | "u32" | "U32"` to the table.
3. **`IndexIn` resolved to the Data protocol category** (no universe key) →
   `Print#` treated the integer as a bits pointer → SIGSEGV. Fixed by
   routing `type_to_protocol` through the element carrier.
4. **`Load#`'s second argument is BYTES, not bits** (`emit_load` emits
   `load i{bytes*8}`) — `Load#(word, 64)` is an i512 load.
5. **`max_bytes` must bound the LENGTH, not the address** — the first draft
   checked `word + 9 > max_bytes`, and a heap address always exceeds a
   small bound, rejecting well-formed blocks.
6. **A String literal's block is a constant global** — tampering it in a
   fixture is optimised away (`before=5 after=5`). The fixture builds its
   own writable block with `Alloc#`/`Store#`.
7. **The intrinsic match arm must `return`** — the match expects `()`; arms
   that produce a register write it via `return BTypedRegister { … }`.

### Gates

lib **3953/3953**, daily smoke **53/53**, determinism **ALL
DETERMINISTIC**, quick gate **pass**, CI **15/15 success on `5a99992a6`**.

### Remaining follow-up (named, not done)

- **`checkfree` modifier** — the proof obligation that rejects a function
  whose subscripts are neither index-typed nor statically in-range. Grammar
  decision (modifier position vs attribute) + analysis pass; the index
  type it ranges over now exists.
- Per-variant enum sizing, varint storage, relative refs, guarded
  allocations — the parked wave-6 items from the research notes §9.
