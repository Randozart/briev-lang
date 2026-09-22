# Syntax examples corpus — one example per SPEC form

**2026-09-21.** ACTIVE — 19 files landed (`956a4bf4`), each verified
`briev check` + enforced by the conformance sweep. Amendment 2026-09-22
(syntax-cleanup plan): the corpus covers ONLY implemented forms; the
aspirational forms below were audited and removed/never written.

## The enforcement is already built

SPEC §1.5: "Treat every normative example as a conformance fixture" —
the charter predates this plan. The conformance sweep
(`src/conformance.rs`, `active_roots()`) walks `examples/` recursively on
every `cargo test --lib`: every file is parsed + elaborated + typechecked
(`frontend_check`, honoring `// target:` headers). **New files under
`examples/syntax/` are enforced automatically — zero walker changes.**

## Layout + rules

- Home: `examples/syntax/<section>/<feature>.bv` (e.g.
  `examples/syntax/decl/enum-match.bv`). `examples` root is already an
  active sweep root; `learn-briev/` is NOT (would be invisible — dead
  fixtures).
- One FORM per file; header comment names the SPEC § and the form.
- Each file standalone: parses + typechecks with stdlib enabled; imports
  resolve relative to the file's dir.
- Naming traps (self-revealing, but avoid): never `briev_*_test_*`
  (sweep exclusion regex); never unknown profile-flag chars in names
  (silently skipped by `classify()`).
- Keep the tree free of build artifacts (existing examples/ dirs carry
  `.ll`/`.o`/`target/` cruft — do not inherit).
- Dialect forms welcome: `.abv`/`.ebv`/`.sbv`/`.rbv` files are checked
  with their own dialect semantics by the sweep.
- Highlighter: no registration needed (grammars match by extension).
  If a corpus example surfaces syntax the tmLanguage grammars miss,
  update the grammar in the same commit.

## Audit note (2026-09-22, syntax-cleanup plan)

Forms in SPEC that are NOT implemented and are therefore NOT in the
corpus (each was confirmed dead/aspirational by the cleanup audit):

| Form | Status |
|------|--------|
| `barrier<group>` | REMOVED — contract-only no-op, group name never read; `[expr];` gate is the real convergence point |
| `machine { }` block | never existed; prose-only (machine-entry = `bootstrap node`) |
| `guard` keyword | never existed; guards are shapes (`[cond] stmt`, `[cond];`, `when`) |
| `vol node` | never existed; volatility is a variable-level pin (`vol let`) |
| `atomic` on node/let | never existed; atomic is a FIELD modifier only |
| `use` keyword | `import` is the keyword |
| `op ... [commutative]` lemma | REMOVED — dead field, no consumer, redundant with `-ffast-math` |
| `Enum::Variant` | REMOVED — member access `Enum.Variant(...)` is canonical |

## Landed corpus (examples/syntax/, 19 files)

- `decl/`: trait, proto-axiom, impl-ops, coll, coll-obj, enum, equation,
  external (frgn), import, sync-group, bootstrap
- `stmt/`: mutex, seq-modifier, match-when, comptime-block ($(Stage)),
  trap
- `flow/`: ranges-slices

## Build order — the discovered gaps first

Original gap list (2026-09-21), kept for history. Zero examples existed
for: `trait` / `proto` / `impl`, `coll`, `$defn` composites + stages `$()`,
`.^^` reflection, `mutex`, `watchdog`, `axiom`, `atomic` / `vol` / `seq`
directives, quotation / derivation / `Error#`, critical sections, op
declarations. The syntax-cleanup audit (2026-09-22) retired the
aspirational subset (see table above); the rest remain open:

Remaining gaps: `$defn` composites + stages `$()`, `.^^` reflection,
`watchdog`, `axiom` on op bindings, `atomic`/`vol`/`seq` field+let
directives, quotation / derivation / `Error#`, critical sections, op
declarations, `import` selective/aliased forms, `render`.

THIN (1-3 trivial instances): contracts beyond `[true]`, enum + match
patterns, tuples, ranges `..`, slices, `beginprogram`/`endprogram`,
reflection `.^`, images (gpu-only today).

Then a per-section sweep of SPEC §3-§22 (~170-190 distinct forms):
§3 files/profiles (`.s` strict, `.f` formatted, `.b` bare, `// target:`),
§5 delimiters + transfer arrows (`-> <~ ~>`), §7 module imports
(aliases/selective/visibility/re-export), §8 declarations (`init`,
`struct`+`spec`, structural sums, `type`, metadata), §9 (`defn`/closures,
`txn`, `object`+ports, `cell`, `accel` GPU tiers), §10 contracts + inline
gates + liveness, §11 iteration forms + `beginprogram`/`endprogram` +
critical sections, §12 spawn/await + the no-implicit-concurrency rule
(`async`/`sync<g>`), §13 triggers + MMIO address triggers, §14 ownership
(`free`/`keep`), §15 ops + portable SIMD, §16 literals (bytes/list/map,
`Int[N]`), §17 reflection descriptors, §18 compile-time bindings +
stages + quotation, §19 `frgn` variants (optional symbols, variadics,
layouts, MMIO, `export`), §20 `asm`, §21 Rendered Briev (views,
components, directives), §22 Data Briev (`.dbv` schema + validation).

## Companion benefit

The corpus doubles as a regression net for every later compiler change
(the sweep runs in `cargo test --lib`), as learn-briev feedstock (the
tutorial is stale — last touched 2026-06-22 — and holds code inline with
zero standalone files), and as the grammar-coverage audit for
syntax-highlighter/.
