# Wave-9 plan — one caret: unify reflection to `^^` (2026-11-21)

**Trigger.** Owner: reflection was originally spelled `.^` (runtime) / `.^^`
(compile-time) to look like a dotted accessor, but the dot is not pulling its
weight — the corpus writes `value .^Length` with a space *before* the dot in
230 of 237 lib sites, so the dot is decoration floating between operands, not a
functioning accessor. Owner-confirmed design: **one spelling, `^^`, no runtime
`^` form, dotted forms retired with hard errors and no alias** (the `#Float` →
`Float` precedent, 2026-09-11 fundamentals doctrine).

Sync point: work starts from `origin/main == 3bf2fc375` (K3 Phase C merged;
local branch `wave6` fast-forwarded to it; conformance sweep green on this tip).

## The semantics being formalised (owner's dividing line, sharpened)

- `Name#(...)` — an **operation**: owner-written code, takes parameters, its
  value is whatever the implementation computes.
- `x^^Prop` — a **representation property**: a stored header slot or a
  type-derivable fact the compiler already knows. Never a computation.

Evidence the unification loses nothing:

- Runtime `.^Size` was already DELETED (2026-08-14 §6a) with a `panic!` —
  "the element count of a collection is the `Count#` intrinsic, not a
  reflection target". The runtime side of the split was already gone.
- `.^Length` on a Vector emits the *same* `vector_element_count` fold that
  compile-time `.^^Size` emits. The tag has not carried real weight for a year.
- The typechecker polices the line today with errors: a hand-written collection
  has no intrinsic length (use `op Count`); a computed count is an intrinsic
  (`CharCount#`). Those errors survive this change unchanged.
- Corpus: all 237 lib property sites read compiler-owned storage or a
  type-derived fact (`Length` 234, `Ready` 1, `Size` 1, `Bytes` 2; tests add
  `Element` 3, `Size` 4). None computes.

## Stages

**S1 · Lexer.** Add `#[token("^^")] CaretCaret` (lexes greedily before `^`,
same longest-match trick `DotCaretCaret` uses). Delete `DotCaret`,
`DotCaretCaret`. `^` stays `Token::BitXor` — XOR `a ^ b` untouched.

**S2 · Parser.** The postfix loop gains one arm: `CaretCaret` + identifier →
`Expr::Reflect(base, name, ReflectKind::CompileTime)`. The dotted forms become
parse errors pointing at the new spelling. `ReflectKind` and the AST variant
survive unchanged — the emitter's per-property table keeps its behavior.

**S3 · Corpus rewrite.** Mechanical: `.^` → `^^`, `.^^` → `^^` across
lib/tests/examples/benchmarks. Goldens re-pinned. ~317 sites (lib 236, tests
~62, examples ~19).

**S4 · IR equivalence.** `ll_determinism_check.sh --quick` byte-identical as
always, plus: for every fixture whose *spelling* changed but whose *meaning*
did not, the emitted `.ll` must be byte-identical to the old compiler's output.
The old-vs-new comparison uses the previously-built release binary.

**S5 · Docs.** SPEC hashword table: one row `value^^Field` — representation
property (stored header or type-derived fact); the runtime/compile-time rows
collapse. 191 live-doc sites across SPEC / architecture / guides / reference /
learn-briev. MASTER-SYNTAX-REFERENCE rows 493-497. Historical plans and reports
are not retroactively edited.

**S6 · Vocab + tests.** `src/vocab.rs:349-350` rows updated; new tests: dotted
forms are parse errors naming `^^`; `^^` parses on every property the table
knows; XOR regression test (`a ^ b` unaffected); goldens.

## Invariants

| # | Property | Pinned by |
|---|---|---|
| 1 | `x.^Y` and `x.^^Y` are parse errors whose message names `^^Y` as the fix | `dotted_reflection_is_a_parse_error` |
| 2 | `x^^Y` parses for every property the emitter table knows (`Length`, `Ptr`, `Size`, `Bytes`, `Alignment`, `Type`, `Element`, `Available`, `Ready`) | `double_caret_parses_every_property` |
| 3 | `^^` reflection produces byte-identical IR to the dotted form it replaced | `ll_determinism_check.sh` + per-fixture goldens |
| 4 | XOR is unaffected: `a ^ b`, `a ^= b` still parse as before | `xor_unaffected_by_caret_token` |
| 5 | `^^Length` on a hand-written collection is still "use `op Count`" | existing iterable-protocol errors unchanged (run the suite) |
| 6 | `^^Ready` still gates event ports (async Phase D behavior kept) | `tests/tier1` async fixtures + `examples/async-ready-gate.bv` |
| 7 | `^^Bytes`/`^^Size`/`^^Element` still fold at compile time where they did | `tests/fixtures/data_cast_interpreter.bv`, reflect-fold example |
| 8 | The vocab gate passes with the new rows | `src/vocab.rs` tests |
| 9 | No `.^^`/`.^` remains in lib/tests/examples/benchmarks/docs(live) | grep gate in the final stage |

## Tests to write

- `dotted_reflection_is_a_parse_error` (parser tests, one per old spelling)
- `double_caret_parses_every_property` (parser tests)
- `xor_unaffected_by_caret_token` (lexer + parser round-trip)
- Re-run: iterable-protocol errors, async-ready fixtures, data-cast fixture,
  reflect-fold example, vocab gate, goldens, conformance sweep.

## Non-goals

- No deprecation window or alias — owner-confirmed hard retirement.
- No new properties; the emitter table is unchanged.
- The `;`-after-tail question (wave-8 note) stays out.
