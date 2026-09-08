# Master Language Surface Reference + completeness gate

**Date:** 2026-09-08
**Status:** COMPLETE

## Problem

No single markdown document listed the language's current surface. The three
reference docs (`BRIEV_LANGUAGE_REFERENCE.md`, `QUICK-REFERENCE.md`,
`docs/architecture/intrinsics.md`) were 2-3 months stale and taught removed
syntax. The only authoritative keyword/intrinsic lists lived in code
(`src/lexer.rs`, `src/vocab.rs`, `src/intrinsic_signatures.rs`).

## Deliverable

**`docs/reference/MASTER-SYNTAX-REFERENCE.md`** — a flat, generated-from-code
reference with 12 sections: reserved keywords, soft/contextual keywords,
intrinsics (registry + legacy tags), operators, delimiters, directives,
hashwords, macros, annotations, reflection, FFI, removed/reserved surface.

Sources: `src/lexer.rs` (Token enum), `src/vocab.rs`
(`LanguageVocab::canonical()`), `src/intrinsic_signatures.rs`
(`get_intrinsic_signature`), `docs/architecture/hash-words.md`.

## Completeness gate

`src/vocab.rs::tests::master_syntax_reference_covers_every_keyword_and_intrinsic`
— one-directional code→doc test: every canonical vocab keyword + every
registered intrinsic must appear in the doc. Backed by a new
`REGISTERED_INTRINSICS` constant in `intrinsic_signatures.rs`. New code breaks
the test until the doc catches up; doc edits never do.

## Stale-doc handling

`BRIEV_LANGUAGE_REFERENCE.md`, `QUICK-REFERENCE.md`, `INDEX.md`,
`docs/architecture/intrinsics.md` got "SUPERSEDED (2026-09-08)" header notes
pointing to the master doc. Kept for historical context.

## Verification

- `cargo test --lib` green (2076 + 1 new completeness test)
- Completeness test passes

## Follow-up (phase 2, not in this slice)

- Backend feature-support matrix (LLVM/VM/SPIR-V/CIRCT/Webstack, intentional
  vs missing)
- Full old-syntax-still-in-codebase index (removed surface + dead emitter arms)