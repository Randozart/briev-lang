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

## Outcome (2026-11-21)

**Landed.** `^^` is the only reflection spelling. Census: 328 `.bv` sites +
257 embedded-in-Rust sites rewritten; 191 live-doc sites; historical
plans/reports/archive untouched (frozen history policy); `learn-briev/examples`
rewritten (live tutorial); `.smoke` fixtures rewritten (tracked, sweep-covered).

**Equivalence evidence (S4).** The probe (byte_len via `^^Length` + inline
`^^Length` + `^^Bytes`) compiled by the new binary produces function bodies for
`byte_len` and `txn_entry` that are **instruction-identical** to the same
program in the old spelling compiled by the pre-wave-9 binary. Three module-level
differences were individually attributed to trunk drift between the two
binaries (K3-era stdlib), not reflection: an unconditional Family-I clock
declare, the stdlib's own `.^Ready` error-message string (now `^^Ready`), and
attribute-id renumbering from the first. `ll_determinism_check.sh --quick`:
ALL DETERMINISTIC.

**Bugs found by the migration itself:**

- My first corpus rewrite used a broken capture group (`r'\1'` against
  `\.\^(\^?)`) and deleted the caret entirely in ~19 embedded-Rust sites
  (`s.^Length` → `s Length`). Caught by the analysis/embedded-source tests;
  repaired by diffing every touched file against HEAD. The correct pattern
  (`r'\.\^\^?'` → `"^^"`) is the one that shipped.
- `defn_liveness`'s open/closed reflection split keyed on the AST tag. Under
  the unified surface every site carries `CompileTime`, so the keep-all arm
  became dead and the closed-read arm swallowed everything — pinned by
  `unclassified_reflection_pins_everything`. Fixed by dispatching on property
  NAME: `Length`/`Ptr`/`Ready` are closed reads; anything else keep-all.

**Tests:** `dotted_reflection_is_a_parse_error` (invariant 1),
`double_caret_parses_every_property` (2), `xor_unaffected_by_caret_token` (4),
plus the existing iterable-protocol/async/data-cast suites pinning 5–7. Vocab
gate green with the deduplicated row.

**Spec row:** `value^^Field` — reflection, a representation property: a stored
header slot or a type-derivable fact, never a computation (that is the `#`
family's job).

## Addendum — post-merge holdouts (2026-11-21, second gate run)

The K3-R2 merge (remote `0698324e7`…`1f9f3c823`) re-touched `.bv` fixtures on
the remote side, re-introducing 5 dotted sites (`server_loop_smoke` 2,
`evented_smoke` 3). A further sweep found the same class in: top-level test
crates embedding `.bv` sources (`interpreter_callable_txn`, `wasm32_parity`),
`.rbv` view fixtures (`browser_json_values`, `generic_wasm_scan` — reflection
inside txn contracts), tracked `.smoke` copies, and the HIR twin of
`defn_liveness` dispatching by tag while the AST twin dispatches by name (the
backend/mod.rs:647 parity assert fired, as designed).

Two gate-caught test defects of my own:
* the reject-path parser test used the old spellings as its input — the sweep
  rewrote them into accept-path inputs. Inputs are now built from fragments
  the sweep cannot touch.
* the HIR closed set was narrower than the AST's (missed
  Size/Bytes/Alignment/Type) — widened to the identical list.

Final: full gate 4587/4587-class green (tier1 corpus + sweep + smoke
included), same-compiler IR determinism ALL DETERMINISTIC, cross-compiler
function-body equivalence PASS for the reflection-using bodies.
