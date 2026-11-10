# IR layers, serialized artifacts, and the BIRD naming decision (2026-11-10)

Status: decision record. Answers three questions raised after the H5 default
flip: is HIR making Briev cleaner, was Beast reconsolidated with HIR, and
should Beast be renamed BIRD.

## 1. The layers (one in-memory IR, several serialized artifacts)

| Layer | Form | Role |
|---|---|---|
| Source `.bv` | text | Author surface |
| AST | `Vec<TopLevel>` (`src/ast/`) | Parser output; plugin live-AST surface; distribution payload |
| **HIR** | `HirProgram` (`src/hir/`) | Typed semantic model; **default codegen input** since `e7ded9eb` |
| LLVM IR | `.ll` text | Backend output; `opt -passes=verify` validated |

Serialized artifacts are separate from those layers:

| Artifact | Carries | Consumers |
|---|---|---|
| `.beast` | AST, S-expression text, per pipeline stage | `--emit-beast` visualization; authoring `Tag$`/`Pattern$`/`Named$` |
| `.beastpack` | typed AST, gzip + blake3, stripped/obfuscated | `.bounty` distribution; install-time tamer compilation |
| `hir::render` | HIR debug text (deterministic, not parsed) | pass-level snapshots (`tests/snapshots/hir/`) |
| `.hir` | HIR canonical text (`--emit-hir`, this change) | tooling/diffing the typed shape |
| `.ir` | real backend IR text (`--emit-beast codegen/optimize`) | IR review/debug |

## 2. Is HIR making Briev cleaner?

Architecturally yes; the codebase is mid-migration, so not yet.

**Wins**
- One typed program model for the flipped analyses; no per-pass AST re-walks.
- Default emission is HIR with byte-identical `.ll`; typed facts (receiver
  types, cast lanes, law markers) are available to passes and verification.
- HIR verifier (structure + typed invariants) and pass-level snapshots are new
  correctness surfaces the AST never had.
- Flip-readiness found real structural bugs (impl-member liveness over-rooting,
  dropped ISR handlers).

**Transitional cost (measured 2026-11-10)**

| Metric | Value |
|---|---|
| `src/hir/` | ~14.3k lines |
| `src/analysis/` (AST) | ~61.7k lines |
| `src/backend/llvm/` | ~59.2k lines, 934 `crate::ast::` refs |
| HIR→AST bridge (`to_ast::`) | 41 sites |
| `hir_emit_enabled()` branches | 28 |

**Cleanliness exit criteria**
- `to_ast::` in the backend → 0; `hir_emit_enabled()` → 0 (flag removed after
  the opt-out release).
- AST-only analyses → 0 outside the parser, import resolver, and Beast
  serialization/pattern surfaces (ports: `accel`, `gpu_schedule`,
  `electronics`, `reclamation`, `region`, `dependency_graph`, `casing`,
  `dataflow`, `range`, `purity`, `allocation`, `batch_shape`).
- AST emission arms deleted family-by-family; `to_ast` shrinks with them.

## 3. Was Beast reconsolidated with HIR?

**No.** Beast was never an HIR format and the flip did not change it.

- The plugin data path has been live-AST since 2026-07-21 (`on_ast`); the
  historical `to_beast → pipe → from_beast` mid-end was superseded. Plugins
  never read `.beast` (`docs/architecture/features/plugins.md`).
- Beast today is: (a) read-only AST snapshots for plugin/selector authoring,
  (b) the `beast::pattern` query DSL, (c) the text payload inside
  `beastpack`/`.bounty` distribution.
- HIR gained its own text view (`hir::render`) for pass snapshots, but there is
  **no HIR serializer or parser**, so it is a dump, not a dialect.
- Bug found while auditing this: `--emit-beast codegen|optimize` claimed `.ir`
  but still serialized the AST. Fixed in this change: those stages now write
  the real backend IR text.

## 4. BIRD naming decision

**BIRD = Briev Intermediate Representation Dialect is reserved for a
round-trippable serialized HIR.** It is not adopted now.

Why not rename Beast:
- BEAST is AST, not IR. Renaming it BIRD would label the visualization
  format as an intermediate representation while the actual IR (HIR) has no
  serialized form — the confusion increases.
- `.beast`, `.beastpack`, `--emit-beast`, `BeastStage`, and the pattern DSL
  are user-facing; a pure rebrand touches formats/CLI/docs for zero semantic
  gain and needs compatibility aliases.
- Historical plans are timestamped records and stay unchanged.

Adoption gates for BIRD (all required):
1. `hir` S-expression (or documented format) with **serialize and parse**,
   version tag, deterministic ordering, round-trip tests.
2. A consumer beyond debug dumps: tooling or a plugin/IR pipeline that reads
   BIRD, with an explicit stage contract.
3. A distribution decision: `.bounty`/beastpack may keep AST (it preserves
   source-level structure tamer/install-time compilation needs) and BIRD may
   coexist; if distribution moves to HIR, a pack version and migration exist.
4. A forward decision record deprecating/redefining “BEAST”; old `.beast`
   files remain readable through the compatibility window.

Until then: **BEAST stays the AST snapshot/pattern/distribution format; HIR is
the IR; `hir::render` is its debug text; BIRD is reserved.**

## 5. Plugin system intended shape (verified)

- 11 stages: `PreLex, Parsed, Resolved, Typed, Normalized, Verified,
  Allocated, Provenanced, Generated, Optimized, Linked`.
- AST stages call `on_ast(&mut Vec<TopLevel>, &mut TypeUniverse)` on the live
  program; IR stages call `on_ir(&mut String)` with backend IR text; Linked
  calls `on_bin(path)`. No serialization in the data path.
- Inline `$(Stage)` blocks are extracted before their stage and registered;
  system plugins live in `plugins/<stage>/`; selection is per-extension via
  `config/targets.dbvl`.
- `.beast` is authoring/debug output only; plugins operate on the live AST.
- Verification: an end-to-end test compiles a program with an inline
  `$(Generated)` `Ir$` rewrite and asserts the emitted `.ll` changed; a
  `$(Typed)` `EmitInfo$` proves live-AST stage execution. See
  `tests/plugin_pipeline.rs`.

## 6. Emission shape (post-flip)

- Default: HIR-driven emission; `BRIEV_HIR_EMIT=0|false|off|no` opts out for
  one release (AST arms remain until deleted).
- Artifacts: `--emit-beast <stage>` writes `.beast` for AST stages and now
  **real `.ir`** for codegen/optimize; `--emit-hir` writes the typed HIR text
  at the Typed and Provenanced stages.
- Gates: `scripts/gate.sh quick|full`, corpus sweep, HIR/AST `.ll` A/B,
  determinism check, diff tests.

## References

- `docs/plans/2026-11-09-h5-completion-next-leg.md` (flip, seam refactors)
- `docs/plans/2026-11-09-compiler-verification-program.md` (gates, snapshots)
- `docs/architecture/features/plugins.md` (live-AST plugin surface)
- `docs/architecture/overview.md` (pipeline stages)
