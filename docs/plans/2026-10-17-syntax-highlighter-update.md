# Syntax Highlighter — Real Update Pass

**Date:** 2026-10-17
**Status:** Active
**Scope:** The VS Code extension in `syntax-highlighter/` — five grammars
rewritten or patched, three new grammars, two languages removed, one
language added, vocabulary source-of-truth fix, icon reassignment, and a
committed-vsix rebuild.

## Motivation

Rule: *outdated docs are bugs* — and the syntax highlighter is the most
user-visible doc of all. A readiness audit (the web end-to-end audit of
the same date, `docs/plans/2026-10-17-readiness-gates.md`) surfaced that
the extension had drifted from the language on nearly every surface:

1. **The core `briev` grammar** missed seven canonical keywords the
   lexer has shipped for months, still listed a keyword (`barrier`) that
   has never been a lexer token, knew nothing about the `state let` /
   `plain let` storage assertions landed earlier the same day, colored
   hash-words with a four-family whitelist (`#L #R #T #Self #Link
   #System`) while the compiler emits the whole `#Int`/`#Float`/
   `#String`/`#Web` category family, and had no scope for the
   compound-assign tokens (`+=`, `|=` …) the shared lexer grew with the
   BILLD plan (2026-10-08) nor for electronics quantity literals
   (`330Ohm`, `3.3Volt`, `4k7`) that the quantity parser
   (`src/parser/quantity.rs`, 2026-09-24) accepts in `.ebv`/`.sbv`
   spec values and expressions.
2. **`language-configuration.json` declared `#` as the line-comment
   character.** Briev comments are `//`; `#` is the hash-word prefix.
   Ctrl+/ inserted a hash-word, not a comment.
3. **The `rbv` grammar** still special-cased `<script>` as the Briev
   wrapper tag. SPEC 21.1 now REJECTS script wrappers (the readiness
   audit's probe produced the diagnostic); the Briev-as-default format
   is the only format.
4. **Three languages were declared with no grammar at all** — `ebv`
   (electronics), `sbv` (silicon), `abv` (GPU). Two more were stale:
   `srbv` (a removed variant; `src/conformance.rs::classify` returns
   `None` for `.srbv`) and `sbriev` (a duplicate claim on the `.sbv`
   extension with no files anywhere in the tree).
5. **`.bld` — BILLD, the intermediate low-level dialect landed
   2026-10-08** — had no language entry, no grammar, and no
   language-configuration. Its parser (`src/parser/bld.rs`) documents a
   complete dialect: `bootstrap`/`defn`/`const`/`import` items,
   statements `let`/assign/compound-assign/call/`when`/`loop`/`while`/
   `break`/`continue`/`return`/block/`bad { }` blocks, with the
   statement-head keywords `else loop while continue return bad` being
   contextual (ordinary identifiers in the shared lexer).
6. **The `bad` grammar** predates two dialect extensions: `site` …
   `end site` anonymous target dispatch (2026-10-01,
   `docs/architecture/bad-dialect.md` §Sites) and the acknowledge
   prefixes `^`/`^^`/`^^^` (the acknowledge tier). It also lacked the
   `raw <target> [name]` … `end raw` verbatim-block keywords and the
   `ptx` target family that `config/bad-isa.dbvl` rows carry.
7. **The `beast` grammar targeted a line-oriented tag format that has
   not existed since 2026-07-14.** `src/beast/serialize.rs::to_beast`
   emits S-expressions; every real `.beast` file in the tree
   (`benchmarks/*.beast`, the GPU tmp snapshots) is a parenthesized
   list, not the `assign|term|guarded` line grammar the extension
   described.
8. **The `dbriev` grammar targeted a debug-IR** (`sig`/`TXN`/
   `REGISTER`/`ALIAS`/`RULE`/`CHECK` + uppercase Briev keywords) that
   the current parser (`src/dbriev/v2.rs`) does not recognize. The real
   `.dbv`/`.dbvl` format is a schema-and-table surface: `schema Name {
   field: Type; };` declarations, `>schema Device from "map.dbv"`
   import lines, `encoding`/`version`/`as` directives, and flat
   `KEY: v; v;` data rows (see `config/targets.dbvl`,
   `lib/boards/stm32f407/map.dbv`, `lib/boards/stm32f407/addresses.dbvl`).
9. **The vocabulary record lied about live surface.** `src/vocab.rs`
   recorded `state` as a Removed keyword (the old rstruct-era meaning)
   even though `state let x` / `plain let x` are the checked
   storage-class assertions shipped 2026-10-17
   (`docs/plans/2026-10-17-state-plain-storage-class.md`), and had no
   `plain` entry at all. The vocab is the canonical keyword record the
   lexer's accounting test and the LSP keyword completions read
   (`src/lsp.rs`), so the lie was load-bearing.
10. **The `bad` language had no icon**, and `bld` (new) needs one.

## Non-negotiables

- **The grammar never advertises syntax the parser rejects.** GPU
  strategy keywords (`scope<>`, `persistent`, `tile`, `fragment`, …)
  from `docs/architecture/gpu-syntax-decision-record.md` are NOT in the
  lexer; they stay out of the grammar until they are. An abv file
  colors as plain Briev — which is exactly what the `.abv` parser
  accepts today (GPU-by-default is the extension's routing semantics,
  not new syntax).
- **Scopes reuse names the shipped themes already map**
  (`keyword.control.briev` etc.) — zero theme edits.
- **Every construct claim is sourced** — the plan tables below name the
  parser file, architecture doc section, or config file each token set
  was extracted from. Regexes are validated mechanically (§Validation).
- **`state`/`plain` highlight via lookahead**, never in the bare
  keyword alternation: `\b(state|plain)(?=\s+let\b)` — a `.bv`
  identifier named `state` keeps identifier coloring; only the real
  assertion form lights up.

## Commit 1 — vocabulary source-of-truth fix

`src/vocab.rs` is the canonical keyword record: the lexer accounting
test (`lexer_keywords_are_accounted_in_vocab`) and the LSP keyword
completions (`canonical_keywords()`) both read it. Two edits:

| Entry | Was | Becomes |
|---|---|---|
| `state` | `VocabStatus::Removed` (rstruct era) | `VocabStatus::Canonical`, `KeywordContext::Declaration` — re-activated by the storage-assertion surface |
| `plain` | absent | `VocabStatus::Canonical`, `KeywordContext::Declaration` |

Test update: `removed_surface_is_recorded` drops `"state"` from its
removed-surface list (the other removed names — `sig`, `rstruct`,
`uni`, `meld`, `escape`, `term!`, `frgn!`, `Ptr!` — are untouched).

Gates: `cargo test --lib` green; Praetor on `src/vocab.rs` (directory
target `src/` — the file lives at the `src/` root).

Note: `state`/`plain` remain CONTEXTUAL in the parser (they lex as
identifiers; the statement parser recognizes the `state let`/`plain let`
shapes). The vocab entry records the surface's canonical status for
accounting and completions — it does not claim a dedicated lexer token.
This matches how the vocab has always recorded contextual surface
(`program`, `init`, `borrow` … are canonical without dedicated tokens).

## Commit 2 — highlighter sweep

### 1. `syntaxes/briev.tmLanguage.json`

**Keyword alternation** (`repository/keywords`):

| Change | Tokens | Source |
|---|---|---|
| remove | `barrier` | never a `Token` enum arm; not in vocab; not in `src/lexer.rs` — stale grammar-only word |
| add | `acquire bartered break halt pin program relaxed release` | vocab-canonical, lexer tokens or contextual surface; the pre-existing alternation had the other ~60 |

`true`/`false` are already scoped `constant.language.*` in the
booleans repository — not added to the keyword alternation.

**New pattern — storage-class assertions** (in the keywords
repository, before the bare-let pattern):

```
\b(state|plain)(?=\s+let\b)  → keyword.control.briev
```

**Hash-words** (`repository/hash-words`):

| Was | Becomes | Rationale |
|---|---|---|
| `#(L\|R\|T\|Self\|Link\|System)(<[a-zA-Z0-9_]+>)?` | `#[A-Z][A-Za-z0-9_]*(<[a-zA-Z0-9_]+>)?` | The category family (`#Int`, `#Float`, `#String`, `#Web`, `#System`…) is open-ended by design (`docs/architecture/hash-words.md`); a whitelist can only ever be stale. PascalCase-first keeps `#` alone or `#lowercase` uncolored. |

**Operators** — compound-assign tokens (the shared lexer grew these
with BILLD; the `.bld` statement parser consumes them and `.bv` gets a
parse error, but the token stream is shared so the highlighter must
scope them):

```
\+=|-=|\*=|/=|\|=|&=|\^=|<<=|>>=  → keyword.operator.compound-assign.briev
```

**New scope — physical-quantity literals**
(`repository/quantity-literals`), source of truth
`src/parser/quantity.rs` (the compact base dims `V A R F H W K s`,
the full-word aliases `Volt Amp Ohm Farad Henry Watt Kelvin`, SI
prefixes `p n u m k M`, the E-series fraction form `4k7`, and the
bare-prefix form `100n`):

```
\b[0-9]+(\.[0-9]+)?(p|n|u|m|k|M)?(Ohm|Volt|Amp|Farad|Henry|Watt|Kelvin|V|A|R|F|H|W|K|s)\b
\b[0-9]+(\.[0-9]+)?(p|n|u|m|k|M)?[kKrR][0-9]\b     → constant.numeric.quantity.briev
```

(The second line is the E-series fraction `4k7`/`2R2`-style form the
parser's `UnitSuffix::Fraction` accepts.) Time-unit scope (`cyc ns ms
s min`) stays as-is; the quantity rule overlaps it harmlessly (`10ms`
matches both, same visual family).

### 2. `language-configuration.json`

- `comments.lineComment`: `"#"` → `"//"` (bug fix — `#` is the
  hash-word prefix, never a comment).
- Delete the dead `folding.markers` block (the "PATH ANALYSIS"
  annotation markers; nothing in the tree emits that banner).

### 3. `syntaxes/rbv.tmLanguage.json`

- Delete the `script-tag` rule (`<script>|<script type="briev">`).
  SPEC 21.1 rejects script wrappers (the readiness audit's probe
  produced `rbv parse error: <script> wrappers are invalid (SPEC 21.1)`).
  `<script>` now colors as an ordinary HTML tag via the generic
  html-components rule. The `<view>` and `<style>` rules stay.
- The generic `b-[a-z0-9-]+(:…)?="…"` binding rule already covers all
  twelve live directives (`b-attr b-bind b-class b-each b-key b-on
  b-show b-style b-text b-trigger b-when b-window` — extracted from
  `src/glue/web_generator.rs`) — no directive whitelist to maintain.

### 4. `syntaxes/bad.tmLanguage.json`

Construct table: `docs/architecture/bad-dialect.md` §Grammar (the
authoritative line-shape table), §Sites, §Raw blocks, §The
acknowledge tier.

| Addition | Pattern (line-head anchored, matching the dialect's line-oriented shape) | Source |
|---|---|---|
| site dispatch | `^\s*site\b` and `^\s*end\s+site\b` → `keyword.control.block.bad`; `default`/target-arrow already partly covered — `default` joins the target-tag list | §Grammar, §Sites |
| raw blocks | `^\s*raw\s+(x86_64\|aarch64\|riscv64\|arm\|thumb\|ptx)\b` head, `^\s*end\s+raw\b` terminator, both `keyword.control.block.bad`; target on the head line reuses `entity.name.tag.target.bad` | §Raw blocks; terminator from `src/parser/bad.rs::try_raw_block` error text (`write 'raw x86_64' … 'end raw'`) and `examples/bad/boot_mbr.bad` |
| ack prefixes | `\^+\s*` before a mnemonic → `keyword.other.acknowledge.bad` | §The acknowledge tier |
| ptx target | joins the existing target-tag alternation (`default\|x86_64\|aarch64\|riscv64\|arm\|thumb`) | `config/bad-isa.dbvl` rows carry `ptx:` families |

The generic mnemonic rule (`^\s*[a-zA-Z_][a-zA-Z0-9_.]*`) already
colors friendly aliases (`Move`, `Add`, … — the `std/bad/friendly.bad`
prelude) and core ops alike; `site`/`end site`/`raw`/`end raw` rules
are ordered BEFORE it so the block keywords win line-head position.

### 5. NEW `syntaxes/bld.tmLanguage.json` + `language-configuration.json`

Dialect source: `src/parser/bld.rs` (the grammar-sketch comment and the
item/stmt dispatch), `docs/plans/2026-10-08-billd-intermediate-dialect.md`,
`examples/bld/*.bld`.

| Class | Tokens | Scope |
|---|---|---|
| shared keyword tokens | `import from as const defn bootstrap let when break` | `keyword.control.bld` |
| contextual statement-head keywords | `else loop while continue return` | `keyword.control.bld` (same visual family; the parser only recognizes them at statement heads, but coloring them everywhere is the honest TextMate-level approximation — same choice `.briev` makes for `state`) |
| `bad` block | `bad` before `{`, with a begin/end pattern that includes `source.bad` between the braces | nested-dialect coloring, not degradation |
| engine verbs | PascalCase identifier applied to `(` — `Halt(); Store(c, vga); DisableInterrupts();` | `support.function.engine.bld` (the registry `config/bld-intrinsics.dbvl` is data, not a hardcoded list — the grammar matches the SHAPE `PascalCase(`, never the vocabulary: Rule 3) |
| types | the shared primitive set (`Int Float Bool String Char Void Ptr Bits`) | `support.type.primitive.bld` |
| compound assigns | `+= -= *= /= \|= &= ^= <<= >>=` | `keyword.operator.compound-assign.bld` |
| everything else | strings, numbers, `//` comments, operators, brackets | standard scopes |

`bld/language-configuration.json` mirrors the briev one: `//`
line comments, the standard bracket pairs, the same indent patterns.

### 6. `syntaxes/beast.tmLanguage.json` — retarget to S-expressions

Emitter: `src/beast/serialize.rs` (header comment dates the format
2026-07-14; `to_beast` → `SExpr` → `to_string`). Grammar rewrite:

| Class | Pattern | Scope |
|---|---|---|
| parens | `(` `)` | `punctuation.section.parens.beast` |
| strings | `"…"` with backslash escapes | `string.quoted.double.beast` |
| numbers | integers/floats/hex | `constant.numeric.beast` |
| colon-keywords | `:bound :lit :range :async :reactive :ref :single :choice :ast …` (atoms WITH the leading colon, as emitted) | `keyword.other.attribute.beast` |
| tag atoms | the exhaustive atom set extracted from `serialize.rs`: `addrof alignment assign binop block body call capture cast choice constant contract defn deref expr field gate guarded ident if index init let list maxbits member members metadata method named none outputs param params parent pin pins plugin pos post post_authority pre properties protocol quantity range reactive ref refs single slot slots state statement string term term! toplevel trigger tuple txn typedef unop yield` | `support.function.tag.beast` |
| everything else | bare atoms (type names, identifiers) | `variable.other.atom.beast` |
| comments | `;`-line comments are NOT emitted by `to_string` — no comment rule (honest: the format has none) | — |

`fileTypes: ["beast"]` and `firstLineMatch: ^\(` stay.

### 7. `syntaxes/dbriev.tmLanguage.json` — retarget to schema/table

Parser: `src/dbriev/v2.rs::parse` (the top-level dispatch:
`schema`/`import`/`encoding`/`version`/`as`, case-insensitive via
`starts_with_ignore_case`; the `>schema` import-line form from
`lib/boards/stm32f407/addresses.dbvl` and the v2 document shape).
Corpus: `config/*.dbv`, `config/*.dbvl`, `lib/boards/*/*.{dbv,dbvl}`.

| Class | Pattern | Scope |
|---|---|---|
| comments | `//…$` | `comment.line.double-slash.dbriev` |
| import lines | `^\s*>\s*schema\b` and `\bfrom\b` | `keyword.control.import.dbriev` |
| top-level keywords | `schema import encoding version as` (case-insensitive) | `keyword.control.dbriev` |
| schema bodies | `{`/`}`/`;`/`:` punctuation; field types (`String Int Float Bool` + bare capitalized) | `punctuation` + `support.type.dbriev` |
| data rows | line-head `CAPS_OR_dotted.ident` before `:` | `entity.name.constant.dbriev` |
| values | hex `0x…`, decimals, quoted strings | `constant.numeric` / `string.quoted.double` |
| dotted module paths | `config/targets.dbvl`-style `a.b/c.d` in config rows | `entity.name.import.dbriev` |

The old debug-IR vocabulary (`sig`/`SIG`/`TXN`/`RCT`/`REGISTER`/
`ALIAS`/`RULE`/`CHECK`/`syscall`/`uni`/`escape`/`retry` and the
uppercase keyword families) is deleted wholesale — none of it exists in
`v2.rs`.

### 8. NEW wrapper grammars `ebv`/`sbv`/`abv`

Three ten-line files:

```json
{ "scopeName": "source.ebv", "patterns": [{ "include": "source.briev" }] }
```

(and the same for `source.sbv`, `source.abv`). Each dialect's parser
today accepts the plain Briev surface — `.ebv` adds `spec` quantity
values (already colored via the briev quantity rule), `.sbv` is bare
file-scope `let name: Type;` (already briev-shaped), `.abv` is Briev
with GPU-default routing. When GPU strategy keywords reach the lexer,
they land in the `abv` wrapper's own patterns — the wrapper is the
per-dialect extension point. `sbv` keeps `s-briev-logo.svg` (silicon).

### 9. `package.json`

| Change | Detail |
|---|---|
| remove language `srbv` | `.srbv` is a REMOVED variant (`src/conformance.rs::classify` returns `None`); no files, no grammar |
| remove language `sbriev` | duplicate `.sbv`-extension claim; no files; the live silicon id is `sbv` (user decision 2026-10-17: "keep language id sbv — this is silicon Briev now") |
| add language `bld` | extensions `[".bld"]`, icon beast-icon, configuration `./language-configuration-bld.json` (separate file — VS Code does not support per-language configuration sharing from one path) |
| icon: `bad` | NEW `bad-icon.svg` (user-provided 2026-10-17: the Briev logo in red with a yellow `AD` label — supersedes the earlier ride-the-beast-icon call) |
| icon: `bld` | NEW `billd-icon.svg` (user-provided 2026-10-17, from `assets/billd-icon.svg` — supersedes the earlier ride-the-beast-icon call) |
| grammars | add contributions for `bld`, `ebv`, `sbv`, `abv` (wrappers) — each with its own scopeName and language id |
| version | 0.1.0 → 0.2.0 |

### 10. `README.md`

Language list becomes: `.bv` (Briev), `.rbv` (Rendered Briev — web),
`.ebv` (Electronics Briev), `.sbv` (Silicon Briev), `.abv` (Accelerator
Briev — GPU), `.bld` (BILLD — intermediate low-level), `.bad` (Briev
Assembly Dialect), `.dbv`/`.dbvl` (Data Briev), `.beast` (BEAST
snapshots). The stale PATH-ANALYSIS folding mention goes with the
language-configuration change.

### 11. Validation

A single node script (`syntax-highlighter/validate_grammars.mjs`,
committed — it is the extension's regression gate) then `vsce package`:

1. JSON-parse all eight grammar files + both language configurations +
   package.json.
2. Compile every `match`/`begin`/`end` regex with JS RegExp (Oniguruma
   and JS agree on the constructs used — char alternations, lookaheads,
   anchors).
3. Assert every `package.json` grammar path exists on disk; assert no
   duplicate `scopeName`s; assert every language id referenced by a
   grammar contribution exists in the languages list.
4. Smoke-match against the real corpus, in-process:
   - `boot_mbr.bad` lines match `raw`/`end raw`; a synthetic
     `site`/`end site`/`^^` snippet matches the bad grammar.
   - `boot_protected_x86.bld` lines match `bootstrap`, `loop`, `when`,
     `Store(`, `bad {`.
   - `led_blinker.ebv` matches `330Ohm`/`1.8Volt`/`20mAmp`; a
     `state let` snippet matches the state pattern; `#Int` matches the
     hash-word rule; `x += 1` matches compound-assign.
   - A beast sexpr line (`( "Int8" ("maxbits" "64") )`) matches
     parens/string/atom rules; a dbriev line (`schema Device {`)
     matches the schema rule.
5. `node_modules/.bin/vsce package` rebuilds
   `briev-language-0.2.0.vsix`; the stale 0.1.0 vsix is deleted.

Gates: `cargo test --lib` green (vocab/lexer tests from Commit 1);
`node syntax-highlighter/validate_grammars.mjs` exit 0; `vsce package`
succeeds; Praetor unchanged-tree check is vacuous for this commit
(no Rust files) — Commit 1 carries the Praetor run.

### Execution order

1. Commit 1 — vocab fix (its own commit; the vocab lie is a compiler-
   side bug independent of the extension).
2. Commit 2 — the sweep, in dependency order: core briev grammar →
   language-configuration → rbv → bad → bld (new) → beast → dbriev →
   wrappers → package.json → README → validation script → vsix
   rebuild → single commit.

## Explicitly out of scope (recorded, not deferred silently)

- **GPU strategy keywords** (`scope<subgroup|…>`, `persistent`,
  `shared`, `tile`, `stage`, `unroll`, `split`, `vector`, `swizzle`,
  `fragment`, `lemma_properties`, `injective`) — the 2026-09-30 GPU
  syntax decision record is DESIGN; the lexer has none of these tokens.
  They land in the abv wrapper grammar (and the briev alternation,
  where they are shared) the day the parser accepts them.
- **`.bv`-side bad bodies are NESTED (user decision 2026-10-17)**:
  both real entry shapes — `bad [target] name(params) -> Ret [contract]
  { … }` (definitions.rs:326) and `bootstrap bad name(params)
  [contract] { … }` (definitions.rs:136) — get a begin/end pattern in
  the briev grammar that includes `source.bad` between the braces, so
  the body colors with the real .bad rules. The `bld`-in-`.bv`
  NESTING is NOT added: no parser construct exists (`bld fn` is the
  BILLD plan's explicit v1 non-goal), and the dialect itself is under
  active discussion with a contributor (shape vs. drop). When/if `bld
  fn` lands, its nesting pattern lands with it.
- **Universal-chaining operators** (`.N>>`, `.name>>`,
  `.(Type)>>`, `>>` captures) — `>>` is also the shared shift token;
  scoping it as chaining would mis-scope shifts. Default operator
  coloring is the honest degradation.
- **The `bad` grammar's generic mnemonic rule stays generic** — the
  18-op core ISA and the friendly-alias prelude are DATA
  (`config/bad-isa.dbvl`, `std/bad/friendly.bad`); a mnemonic
  whitelist in the grammar would violate Rule 3's spirit for zero
  benefit (the generic rule already colors every mnemonic).

## Undo

- Commit 1: revert the two vocab entries and the test list line.
- Commit 2: `git revert` the sweep commit; the vsix rebuild is
  reproducible from the reverted tree.
- The validation script is additive; it fails loudly if a grammar is
  reverted without its package.json contribution.

## Results (2026-10-17, complete)

- **Commit 1** `f60f7724` — vocab truth (`state`/`plain`) + plan + icons.
- **Commit 2** `c421afc0` — the full sweep: 6 grammars rewritten/patched,
  4 new (`bld`, `ebv`, `sbv`, `abv` wrappers), 2 language configurations,
  package.json 0.2.0 hygiene, README, `validate_grammars.mjs`, vsix 0.2.0.
- **Gates, both commits:** `cargo test --lib` 3162 green; Praetor
  baseline-identical on `src/vocab.rs`; `node syntax-highlighter/validate_grammars.mjs`
  → 155 regexes compile, 9 grammars consistent, 49 positive + 4 negative
  corpus smoke-matches, beast's 67 atoms locked to `serialize.rs` in both
  directions; `vsce package` → `briev-language-0.2.0.vsix` (0.1.0 removed).
- **Found by the gate while writing it:** dbriev config rows (`.bv: llvm;`
  in `config/targets.dbvl:16`) were not colorable — entry-keys now allow
  the leading dot.
- **Also fixed:** AGENTS.md's hash-word pillar claimed `#Category`
  hashwords were live backend directives; `src/parser/definitions.rs:6409`
  (Phase A4) retires them. Pillar rewritten, `hash-words.md` already correct.
