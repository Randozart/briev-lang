# README Philosophy Refresh — plan

**Date:** 2026-10-18
**Status:** COMPLETE 2026-10-18
**Scope:** `README.md` only (targeted edits, not a rewrite) + two record docs
(this plan and `2026-10-18-philosophy-interview.md`).
**Explicitly out of scope:** `docs/architecture/philosophy.md` (dated
2026-06-17, predates the September doctrine — refresh deferred),
`spec/SPEC.md` §2.4, `keyword-taxonomy.md`.

## Motivation

The README was last updated 2026-08-23. It predates the September doctrine
(derivation-not-recognition, proof-vs-shape, expressiveness closure, the
keyword taxonomy) and carries stale facts (dialect targets, test counts,
CLI). The language owner asked for (a) an interrogation of his own
philosophy so the pillars could be made honest, (b) a stability report for
the six main dialects, (c) `.bad`/`.bld`/`.beast` listed as auxiliary
dialects, and (d) Otto Westerlund (@odecode) listed as an official
contributor.

The interrogation is recorded verbatim in
`2026-10-18-philosophy-interview.md` (13 questions, answers, the
consistency verdict, seven seams, two rulings). This plan locks the
decisions that came out of it.

## Locked decisions

1. **Stability labels** (evidence-based):
   - **Stable** — `.bv` (conformance sweep green, 2900+ tests, syntax
     freeze set landed, folio packages shipped), `.dbv`/`.dbvl` (stable as
     data format; the compiler's own config language).
   - **Functional** — `.abv` (correctness gates EXACT on CUDA + Vulkan
     lanes at real shapes; syntax frozen D31; performance campaigns
     active), `.rbv` (real-browser Playwright smoke, popstate routing,
     `brievc watch`; marked "functional for a stranger" by the
     three-surfaces umbrella).
   - **In development** — `.ebv` (bare metal proven via `.bv` × target
     triples; the electronics lane lives on the parked foreign branch
     `feat/e14a-intent-synthesis`).
   - **Experimental** — `.sbv` (graft C0/C1 only; synthesizability gate
     now wired but surface young; CIRCT toolchain issue open).
2. **Doctrine folded into the pillars** (no separate "doctrine" section);
   each pillar gains its honest basis from the interview answers.
3. **Seam rulings** (see interview doc §Seams):
   - `async`/`sync<g>` classification debate (strategy vs ambiguity):
     **deferred** — README phrases functionally ("the compiler refuses to
     guess"); SPEC/taxonomy untouched.
   - "Proves reachability and termination" overclaim: replaced with the
     **prove-and-report** framing.
4. **Contributor listing** for Otto Westerlund under Contributing,
   commit-backed (20 commits: static trait-constrained generics, `.sbv`
   gate wiring, LLVM memcpy/memset fixes, stdlib net stack, `.rbv` parse
   fix, cpp-vs-briev experiment).
5. **Auxiliary dialects section**: `.bad`, `.bld` (tier ladder
   `.bv` → `.bld` → `.bad`), `.beast`/`.beastpack`/`.bounty`.

## Work items

| # | Item | Where |
|---|------|-------|
| 1 | Fix `.rbv` target ("TypeScript..." → "WASM + JS shim") — both tables | lines ~43, ~155 |
| 2 | Fix `.ebv` row (name "Embedded" → "Electronics"; target → electronics backend) | line ~44 |
| 3 | Thesis section: honest-boundary paragraph | line ~71 |
| 4 | Pillars: extend 5 existing with honest basis; add 4 new (derivation, closure, keywords, concurrency) | lines ~73–103 |
| 5 | Language Variants table: add Status column + legend + bare-metal footnote | line ~151 |
| 6 | New Auxiliary Dialects section | after Language Variants |
| 7 | Quick Start: test count 1934 → 2900+, add `brievc init`/`run` | line ~110 |
| 8 | Contributors subsection | Contributing, line ~353 |
| 9 | License → "Apache-2.0 WITH LLVM-exception" | line ~361 |
| 10 | Footer date → 2026-10-18 | line ~365 |

## Verification

- Markdown renders: table pipe counts, heading levels, link targets.
- No code change: `cargo test --lib` not required, conformance sweep
  unaffected (docs-only).
- Factual claims cross-checked against: `docs/plans/INDEX.md` (test count,
  workstream states), `config/targets.dbvl` (backend routing),
  `benchmarks/recommender_corpus/README.md` (Otto credit),
  `LICENSE`/`NOTICE` (license wording), `docs/architecture/bad-dialect.md`
  + `bld-dialect.md` (auxiliary dialects).

## Follow-up candidates (not this commit)

- `docs/architecture/philosophy.md` refresh (fold September doctrine into
  the architecture doc; it still says "LLVM is the right backend" without
  the self-hosting caveat and documents the C-shim runtime as current).
- `keyword-taxonomy.md` Seam-1 reconciliation (async/sync class + Rule 22
  error-vs-warn justification) — needs a design ruling first.
- `spec/SPEC.md` wording check for the same prove-vs-check overclaim.
