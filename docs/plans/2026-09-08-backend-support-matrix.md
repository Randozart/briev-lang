# Backend support matrix + legacy surface index

**Date:** 2026-09-08
**Status:** COMPLETE
**Depends on:** 2026-09-08-master-syntax-reference.md

## Deliverables

1. **`docs/reference/BACKEND-SUPPORT-MATRIX.md`** — per-feature support across
   the five backends (LLVM, VM, SPIR-V, CIRCT, Webstack): expression surface,
   statement surface, truly-emitted intrinsics, and a "known gaps/hazards"
   ledger of every silent drop / stub / rejected construct.

2. **`docs/reference/LEGACY-SURFACE-INDEX.md`** — every removed/old syntax
   still in the codebase, categorized: (a) lexed, (b) parsed, (c)
   rejected-with-error, (d) dead codegen/analysis, (e) documented-only.

3. Cross-links added to `MASTER-SYNTAX-REFERENCE.md`.

## Key findings

- `BackendCapabilities` (52 flags) is the matrix skeleton; per-backend
  CAPABILITIES + emission survey fill it.
- **Webstack has NO capability declaration** and the `validate_program` gate is
  SKIPPED for it — inherits LLVM full() surface, gated only by the intrinsic
  whitelist. Noted as a gap.
- **LLVM silent drops** (declared true, no emit arm): `SyncBlock`,
  `InlineAsm`, `MetadataAssignment`, `TrgBinding`. Plus semantic stubs:
  `IsType`→true, `DerivationBlock`→0, `Within` deadline dropped, `Check`
  unemitted.
- **CIRCT silent drops**: cell-body `_ => {}`, `format_init_expr _ => "0"`,
  contract-condition `_ => true`, `%0` operand fallbacks.
- **SPIR-V and CIRCT** have normalizer-vs-emitter intrinsic mismatches
  (names pass the normalizer, die in the emitter, or vice versa).
- **VM** has one silent runtime-trap gap (unknown assign target) and
  match-pattern fall-through.
- **Legacy surface**: 3 removed tokens still lexed (`meld`, `pvt`, `sed`);
  ~26 dead codegen/analysis sites (5 LLVM intrinsic arms, ~12 AST variants
  with no parser producer, 3 dead fields, 1 dead diagnostic); 12 stale
  `features/*.md` docs teaching removed syntax; 2 tolerated legacy parse
  forms (`foreach (x in list)`, `sync {}`); removed syntax in `lib/compiler/*.bv`.

## Verification

- `cargo test --lib` green (2086).
- No code changes — documentation-only.

## Follow-up candidates (not in this slice)

- Fix the LLVM/CIRCT/VM silent-drop gaps (declare false, or emit).
- Delete the dead AST variants + LLVM intrinsic arms.
- Rewrite/stub the 12 stale `features/*.md` docs.
- Remove the tolerated legacy parse forms.