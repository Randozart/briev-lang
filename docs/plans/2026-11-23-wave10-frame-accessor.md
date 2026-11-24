# Wave-10a — the frame accessor: `::` replaces `::` (2026-11-23)

**Status:** GRIND — implemented and verified (see "Outcome" at the end).
**Owner decision (2026-11-23):** name the concept **frame accessor**; spell it
`value::Prop`. Scope is deliberately minimal — this is *static introspection on
known compiler properties*, not an extensible declaration mechanism.
**Promote to** `docs/plans/2026-11-23-wave10-frame-accessor.md` on landing.

## Trigger

`::` is bad to type. On US it is Shift+6 twice; on German QWERTZ, Nordic and
French AZERTY, `^` is a **dead key** — `value::Length` costs four key presses
with dead-key gymnastics to produce two characters. 345 corpus occurrences pay
that cost.

Explored and rejected:

| glyph | why not |
|---|---|
| `#` infix | `#` is an identifier char, so `coll#Length` lexes as ONE token needing post-split; `(a+b)#Length` takes a different parse path. Non-uniform. |
| `.Length#` | visually near-identical to `Count#()`; owner: "no visual distinction". |
| `.Length##` | distinct and free, but the second degree has no honest load (Rule 21). |
| backtick | free, but thin glyph, "raw/quoted" connotation, dead key on German. |
| `&`, `!`, `%` | already tokens with incompatible meanings (bitwise-and/address-of; plugin suffix + `!>` metadata; modulo). Hard parse conflicts. |
| `@` | mechanically free, reads as "at", but owner preferred `::`. |
| `repr` declarations | over-built — see below. |

## What we deliberately do NOT build

Recorded so this discussion is not re-derived:

- No `frame`/`repr` declarations in type bodies.
- No redeclaration, inheritance or contradiction rules for properties.
- No contracts on property declarations.
- No foreign-type property declarations.
- No instantiation narrowing.

**Reason:** the set is nine names, and all nine bear on bootstrap primitives —
which Rule 15 explicitly permits the compiler to know by name (its stated sole
exception). A declaration surface would have no user.

**Trigger to revisit:** a user-defined type that owns an ABI with hidden header
slots. If that appears, the discussed shape applies: closed *kinds*
(slot / type-fact / live), declared *membership*, read-only, contracts on the
declaration.

## Invariants

Each is pinned by a named test; `I-*` numbering is referenced by the stage that
establishes it.

1. **I-1 (same AST).** `x::Prop` produces the identical `Expr::Reflect(base,
   Prop, ReflectKind::CompileTime)` node that `x::Prop` produced. Pinned by
   `frame_accessor_parses_to_reflect_ast` (asserts the AST, never a string).
2. **I-2 (same IR — the load-bearing one).** For every one of the nine
   properties, the emitted module is **byte-identical** to what the `::`
   spelling emitted for the same program. Pinned by
   `frame_accessor_emits_identical_ir` (compile each fixture twice from two
   source spellings and compare `.ll` bytes) plus
   `benchmarks/ll_determinism_check.sh --quick`.
3. **I-3 (hard retirement, no alias).** `x::Length` is a compile error naming
   `::`. Wave-9 precedent (`#Float`→`Float`): no compatibility alias.
   Pinned by `old_caret_caret_spelling_is_rejected_naming_the_new_form`.
4. **I-4 (XOR untouched).** `a ^ b` and `a ^= b` lex and parse exactly as
   before; the single caret is never consumed by the new path. Pinned by
   `single_caret_stays_bitwise_xor`.
5. **I-5 (`::` is not a path, anywhere).** `Enum::Variant` in pattern position
   is still rejected — now with a message about *patterns*, not about "`::` is
   not Briev syntax" (that sentence becomes false). Pinned by
   `double_colon_in_pattern_is_still_rejected`.
6. **I-6 (closed set unchanged).** The nine properties are the same nine; an
   unknown name is still an error, and the error now names the full set and
   uses the new vocabulary ("frame property", not "reflection target").
   Pinned by `unknown_frame_property_names_the_full_set`.
7. **I-7 (no stale spelling in any diagnostic).** Every user-facing message that
   mentions the accessor says `::` — parser (three sites), typechecker,
   annotator, `parser/definitions.rs`. Pinned by
   `no_diagnostic_names_the_retired_spelling` (grep-style assertion over the
   message constants exercised by a table of bad programs).
8. **I-8 (docs describe one form).** SPEC §17 and the grammar table, the
   agent-reference chaining table, and the tutorial describe `::` only — and
   the tutorial's `.^`/`.::` runtime/compile-time split, which wave-9 flattened
   into self-contradictory text, is corrected rather than renamed.
   Pinned by a **new** `docs_mention_only_the_current_spelling` test (no
   docs-spelling test exists today; this is added, not extended): it asserts
   that no live doc under `spec/`, `docs/architecture/`, `learn-briev/`
   mentions the retired spelling outside a historical-record path.
9. **I-9 (goldens unchanged).** Every tier1 golden is byte-identical before and
   after: this is a surface change, so stdout must not move. Pinned by the
   existing `daily_use_smoke` corpus sweep.
10. **I-10 (no determinism drift).** Emitted `.ll` remains byte-identical across
    processes. Pinned by `benchmarks/ll_determinism_check.sh --quick`.

## Current state (evidence)

**Lexer.** `src/lexer.rs:527` — `#[token("::")] CaretCaret`; `Display` at
`:781`. `:` is `Colon`, `:=` is `ColonEq`. **No `ColonColon` token exists** —
`::` lexes as two `Colon`s, documented at `src/parser/expressions.rs:1293`.
Logos longest-match puts a defined `::` before `:` (same trick as `::` before
`^`, `..=` before `..`).

**Parser.** Postfix reflect arm `src/parser/expressions.rs:595-610`. Two
retirement paths to repoint: dot-caret `:437-445`, single-caret `:611-621`.
`::` has a targeted rejection in **pattern** position only (`:1295-1300`, test
at `:1892-1897`); in **expression** position it currently fails with a generic
message, so S2 both adds the accessor arm and gives the pattern path the new
wording. Slice bounds use `Colon` **inside `[]`** (`:665-702`) — a `ColonColon`
token cannot interfere there, since `data[a::b]` was never valid.

**Typechecker.** `resolve_reflect` `src/typechecker/mod.rs:8977-9044`.
Unknown-target message `:9033-9043` currently names only six of the nine.

**Emitter.** `emit_reflection` dispatch `src/backend/llvm/emit_expr.rs` ~
`7070-7237`; unreachable fallthrough `panic!` at `:7233`.

**Annotator.** `src/annotator.rs:774` formats `{}::{}` for diagnostics.

**Other message sites.** `src/parser/definitions.rs:654` (`feature::Available`
inside an error string).

**Unrelated `::` that must survive:** the BAD assembly line-ack prefix (`^`,
`::`, `::^`) is parsed from **raw text**, not from tokens —
`src/parser/bad.rs:1823 carets_of`. It does not use `Token::CaretCaret`, so the
token is reflection-only and can be retained purely to emit I-3's diagnostic.
Corpus check: zero line-initial `::` in `.bv`/`.rbv`/`.abv`, so no BAD-ack
migration hazard.

**Known pre-existing gap (not in scope).** `lib/compiler/lexer.bv` (the
compiler-in-Briev) has no reflection token at all — it does not support `::`
today. Nothing regresses; whoever next grows that lexer adds `ColonColon`.

**Corpus scale.** `lib` 245 occ / 35 files · `tests` 58 / 22 · `examples` 29 / 8
· `benchmarks` 10 / 2 · `.smoke` 3 / 2 ⇒ **345 occurrences, 69 files**.
Single-caret uses that must be left alone: `lib/std/atomic.bv:60` (`old ^ val`),
`lib/compiler/lexer.bv:357` (`'^'` char literal), `lib/pp/exprs.bv:21,43`
(emitted `"^"` strings).

**Rust-side.** 329 occurrences in 45 files; the semantically meaningful sites
are the lexer/parser/typechecker/annotator/emitter above — the rest are Briev
source strings inside tests and must be rewritten with the corpus.

**Docs.** `spec/SPEC.md` 42 lines (including the falsehood at `:1270`,
"`::` is not Briev syntax", and the grammar block at `:3947`).
`docs/architecture/agent-reference.md` 2 lines (`:149`, "no `::` module
paths"). `learn-briev/` 114 lines, including `00a-base-design.md:18` and
`:183`, which still describe the retired `.^`/`.::` split with both spellings
rendered identically — stale from wave-9 and self-contradictory.

## Design

One spelling, one meaning, no new vocabulary in the language surface:

```briev
let n: Int = s::Length;      // stored byte header — O(1), never a computation
let b: Int = s::Bytes;
let e: Int = buf::Element;   // type-derived fold
when wire::Ready { ... }     // reactor state
```

- `::` = **frame accessor**: resolved against the value's frame (its
  representation), never against the instance's declared members. That is why it
  reads correctly on a *value* even though C++/Rust apply `::` only to types.
- The set stays closed and compiler-owned. `resolve_reflect` remains the single
  home for the knowledge.
- Read-only, as today: no `x::Length = n`; writes go through `Store#` or the
  owning type's ops.

## Stages

Each stage is its own commit; the gate runs per stage.

### S1 · Lexer (`src/lexer.rs`)

- Add `#[token("::")] ColonColon` **before** `#[token(":")] Colon`.
- Keep `CaretCaret` (used only to emit I-3's diagnostic; BAD does not need it).
- `Display`: `ColonColon => write!(f, "::")`.
- Unit test: `::` lexes as one `ColonColon`, `:` alone as `Colon`, `:=` as
  `ColonEq`, `a::b` still two `CaretCaret`.

### S2 · Parser (`src/parser/expressions.rs`)

- Postfix arm: `eat(&Token::ColonColon)` + `expect_identifier()` →
  `Expr::Reflect(Box::new(expr), name, ReflectKind::CompileTime)`. The AST is
  unchanged (I-1).
- Replace the reflect arm's `CaretCaret` consumption with a **hard error**
  naming `::` (I-3).
- Repoint the dot-caret (`:437-445`) and single-caret (`:611-621`) messages:
  the retired spelling is now `::`, and the fix is `x::Prop`.
- Pattern position (`:1295-1300`): treat `Colon`+`Colon` **and** `ColonColon` as
  the same rejection, with a message about patterns:
  *"`::` is not a pattern syntax — qualified variant patterns use the dot form:
  `Enum.Variant(args)`; `::` reads a frame property in expression position."*
- Update the pinned test at `:1892-1897` and the two-caret test at `:1652`.

### S3 · Typechecker + annotator + message sites

- `src/annotator.rs:774`: `format!("{}::{}", …)`.
- `src/typechecker/mod.rs:9033-9043`: unknown-target message becomes
  *"unknown frame property — expected Length, Ptr, Size, Bytes, Alignment, Type,
  Element, Available, or Ready"* and the `Absolute` hint becomes `::Absolute`
  (I-6).
- `src/parser/definitions.rs:654`: `feature::Available` (I-7).
- The `resolve_reflect` doc comment: keep the owner's dividing line verbatim,
  retargeted to `::` — do **not** delete the rationale (Rule 13: preserve
  commentary, rewrite it).

### S4 · Corpus rewrite (69 files, 345 occurrences)

Mechanical `\^\^` → `::` (also collapsing surrounding spaces: `value::Length`
→ `value::Length`), **excluding**:

- `docs/archive/**`, `.opencode/plans/2026-11-21-wave9-one-caret.md`, `archive/**`
  — timestamped/historical records, never retroactively edited.
- any single `^` (XOR, char literals, emitted strings listed above).

Then the same rewrite across the 45 Rust files' embedded Briev sources, and
comments (Rule 13: the rationale comments are rewritten, not dropped).

Goldens are **not** touched — I-9 requires byte-identical stdout.

### S5 · Docs

- `spec/SPEC.md`: 42 lines `::`→`::`; rewrite `:1270` to
  *"`::` is not a path — Briev's names are flat. `x::Prop` is a frame accessor,
  resolved against the value's frame."*; update the grammar block at `:3947`
  and the §17 title/vocabulary to "frame accessor".
- `docs/architecture/agent-reference.md:149`: same clarification; keep the
  flat-names rule intact (it is still true).
- `docs/architecture/universal-chaining.md`: the suffix table's reflection row.
- `learn-briev/`: 114 lines; **fix** `00a-base-design.md:18` and `:183`, which
  are stale from wave-9 and currently contradict themselves.
- Any other `docs/architecture/*` file that mentions the accessor.

### S6 · Verify

- `cargo test --lib` targeted, then `bash scripts/gate.sh quick`.
- `benchmarks/ll_determinism_check.sh --quick` (I-10).
- `bash scripts/gate.sh full` + tier1 corpus before landing.
- Praetor on changed directories (`--target <dir>`, never a file); diff against
  `HEAD` to prove no new diagnostics.

## Named tests

| test | invariant |
|---|---|
| `frame_accessor_parses_to_reflect_ast` | I-1 |
| `frame_accessor_emits_identical_ir` | I-2 |
| `old_caret_caret_spelling_is_rejected_naming_the_new_form` | I-3 |
| `single_caret_stays_bitwise_xor` | I-4 |
| `double_colon_in_pattern_is_still_rejected` | I-5 |
| `unknown_frame_property_names_the_full_set` | I-6 |
| `no_diagnostic_names_the_retired_spelling` | I-7 |
| `frame_accessor_parses_every_property` (renamed from `double_caret_parses_every_property`) | I-1, I-6 |
| `docs_mention_only_the_current_spelling` | I-8 |
| `daily_use_smoke::tier1_corpus_compiles_links_and_runs` | I-9 |
| `benchmarks/ll_determinism_check.sh --quick` | I-10 |

I-2 is the one that proves the whole change is surface: same bytes out.

## Undo path

Per stage, revert that stage's commit. Ordering makes this safe: S1 alone is
inert (a new token nobody consumes); S2 introduces the new spelling while `::`
still errors, so S2+S1 revert cleanly; S4 is mechanical and reversible by the
inverse rewrite. No data migration, no ABI change.

## Doc maintenance (Rule 13)

- **Rewrite, never delete** the rationales: the `resolve_reflect` dividing line,
  the wave-9 "one spelling" note in the lexer, the `CaretCaret` doc comment
  (it becomes "retired reflection spelling, kept for the migration diagnostic").
- The BAD dialect's `^`/`::`/`::^` ack documentation is **untouched** — it is a
  different feature that happens to share the glyph.
- Add a short note to `docs/architecture/universal-chaining.md` recording that
  `::` is a frame accessor, not a path, so the next reader does not re-open the
  question.

## Provenance (Rule 16)

- **When:** 2026-11-23.
- **Why:** `::` requires shift+dead-key on DE/Nordic/AZERTY layouts; 345 corpus
  sites pay it.
- **What pattern it targets:** every `Expr::Reflect` site (expression position
  only; patterns keep the dot form).
- **How to undo:** revert S1+S2 and re-run the inverse corpus rewrite.

## Risks

1. **A blind `::`→`::` rewrite could touch a single `^`.** Mitigation: match the
   double caret only, and assert the known single-caret sites
   (`atomic.bv:60`, `compiler/lexer.bv:357`, `pp/exprs.bv:21,43`) are unchanged
   in the diff.
2. **`::` frees a token that was previously a reliable error.** After S1, any
   stray `::` outside the postfix position produces a *different* (less helpful)
   message unless the pattern path and the postfix path both handle it — hence
   I-5 and the explicit pattern arm.
3. **Doc-surface drift.** `learn-briev` is already stale from wave-9; renaming
   without correcting `00a-base-design.md:18/:183` would launder a
   contradiction. S5 fixes it explicitly.
4. **Diagnostic regression on unknown names.** The current catch-all omits
   `Element`/`Available`/`Ready`; if S3 merely renames, I-6 fails.

## Out of scope (recorded, not deferred silently)

- Extensible frame declarations (`frame`/`repr`) — see "What we deliberately do
  NOT build", with its revisit trigger.
- Contracts/postconditions on frame properties so the prover can elide bounds
  checks (the `term >= 0` idea). A ten-line follow-up in `resolve_reflect`, no
  syntax; separate wave, separate A/B.
- `::` support in the compiler-in-Briev lexer (`lib/compiler/lexer.bv`), which
  has no reflection token today.


## Outcome (GRIND, 2026-11-23)

### What landed

| stage | result |
|---|---|
| S1 lexer | `ColonColon` token before `:`; `CaretCaret` kept **solely** for the migration diagnostic (BAD ack parses raw text, so the token is reflection-only). 3 new lexer tests. |
| S2 parser | `::` → `Expr::Reflect` (identical AST); `^^` now a hard error naming `x::Length`; dot-caret and single-caret messages retargeted; pattern-position `::` rejected with new wording. 4 parser tests added/renamed. |
| S3 messages | annotator renders `recv::Prop`; `resolve_reflect` unknown-set now names all nine; `Absolute` hint retargeted; `parser/definitions.rs` `feature::Available`. 2 new tests (I-6, I-7). |
| S4 corpus | 70 `.bv`/`.rbv`/`.abv` files (303 lines) + 39 Rust files. Goldens untouched. |
| S5 docs | SPEC (43 subs + §17 renamed to "Frame accessors" + the false "`::` is not Briev syntax" sentence replaced), agent-reference, universal-chaining, MASTER-SYNTAX-REFERENCE, 10 `learn-briev` files including the stale `.^`/`.^^` split. |
| S6 verify | quick gate PASS; tier1 corpus PASS with **unchanged goldens**; determinism ALL DETERMINISTIC; full gate PASS. |

### I-2 proven out of band (the load-bearing evidence)

Built a baseline worktree at the pre-change commit (`/scratch/briev-builds/w10-base`,
`git worktree add ... HEAD`) and compiled the *same program* twice — `^^` with the
baseline compiler, `::` with the new one — then byte-compared the `.ll`:

```
IR identical except one constant:
- "briev: read of an unready event port outside a task - gate the read with ^^Ready\n"
+ "briev: read of an unready event port outside a task - gate the read with ::Ready\n"
```

Same length, same code, and the divergence is an intentional user-guidance string
that must name the current spelling. Everything else — every load, fold, and
register — is byte-identical. In-repo guard:
`backend::llvm::tests::frame_accessor_reads_keep_the_stored_and_folded_shapes`.

### Two rewrite hazards found and fixed

1. **`^^` inside a string literal is DATA, not syntax.** The blanket sweep
   corrupted `lib/std/net/http_response.bv`'s HTTP token punctuation set
   (`"!#$%&'*+-^^_`|~"` → `...::...`). Restored. A detector over the corpus diff
   (quote-aware scan) found exactly three in-string occurrences; the other two
   were a guidance message (intentionally updated) and an `.rbv` `b-text`
   directive (correctly rewritten — the view compiler's `root_signal` splits `::`).
2. **The docs sweep initially overreached** — it rewrote `docs/plans/*` (timestamped
   historical records), `spec/archived/**`, `BUGS.md`, `AGENTS.md`, and ASCII-art
   caret underlines (`^^^^ explicit` → `::::^ explicit`). All restored and the
   sweep re-scoped to live documentation only; every remaining markdown change is
   a verified pure `^^`→`::` substitution.

### Correction to the plan's "out of scope"

The plan listed *"contracts/postconditions on frame properties"* as a separate
follow-up. It **already exists**: `src/typechecker/mod.rs` records
`term::Length >= 0` (and `::Ptr`/`::Ready`) as a known fact, so a bounds
discharge already has what it needs. Nothing to do; recorded so it is not
re-attempted.

### Vocabulary decision

SPEC §17 is now titled **"Frame accessors"** and the tables use that term. Prose
elsewhere may still say "reflection" descriptively (it is accurate English for
reading compiler-known metadata); renaming every occurrence was judged churn
without benefit. The authoritative naming is the section title and the `::` row.
