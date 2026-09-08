# Deprecated syntax markers + sync{} silent-drop fix

**Date:** 2026-09-08
**Status:** COMPLETE
**Depends on:** 2026-09-08-master-syntax-reference.md, 2026-09-08-backend-support-matrix.md

## Problem

The language has syntax that is explicitly marked legacy in code/comments but
still parses and compiles. The master reference did not distinguish
"deprecated (still works)" from "removed (rejected)". Worse, one deprecated
form — `sync { }` — silently dropped its body on the LLVM backend.

## Findings (7 deprecated-but-working forms)

`foreach (item in list)`, `sync { }` (→ `mutex { }`), `!> key: value;`
(→ `spec`), single-value `print!`/`println!` (→ format-string form),
`Get#`/`Insert#` (→ `At#`/`InsertAt#`), `ToInt#`/`ToFloat#`/`ToString#`
(→ casts), `maxbits <~ N;` (→ `spec MaxBits: N;`).

## Changes

1. **emit_stmt.rs**: `Statement::SyncBlock` now emits like `Mutex` (serial
   section inline). Was swallowed by the `_ =>` catch-all — body vanished.
   Regression test `test_llvm_emits_sync_block_body` asserts the body's `add`
   survives codegen.
2. **MASTER-SYNTAX-REFERENCE.md**: new §13 "Deprecated (still works, do not
   use)" flat table (7 forms + modern replacements + deprecation markers);
   inline `[deprecated]` tags on `foreach`, `sync`, `!>`, `print!`/`println!`,
   `Get#`/`Insert#`. `node name()` left unmarked (tolerated, not deprecated).
3. **LEGACY-SURFACE-INDEX.md**: §9 split "(b) parsed legacy forms" into
   deprecated-but-works (7) vs tolerated (1); notes the sync{} LLVM fix.
4. **BACKEND-SUPPORT-MATRIX.md**: concurrency-sections row updated from
   SILENT DROP to ✅; `SyncBlock` removed from the LLVM hazards ledger.

## Verification

- `cargo test --lib` green (2087, incl. new regression test)
- Manual: `.bv` with `sync { x = x + 1; }` → `add` present in `.ll`, program
  prints `1`

## Follow-up

- Migrate call sites off the 7 deprecated forms, then reject them (promote to
  §12 removed surface).
- Same silent-drop class remains: `InlineAsm`, `MetadataAssignment`,
  `TrgBinding` (declared true, no emit arm).