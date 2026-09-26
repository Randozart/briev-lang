# Interop Wave 1 — execution plan (2026-09-25)

**Refines:** `docs/plans/2026-09-24-followup-stages.md` Stage 2 (Wave 1),
plan of record `docs/plans/2026-09-21-cross-dialect-interop.md:104-111`.
**Prerequisite for:** `2026-09-25-sbv-ebv-bridge.md` (Wave 2 design record —
its typed-import substrate is this wave).
**Scope decision (2026-09-25):** all five items, one arc; item 3 resolved by
IMPLEMENTING the `.ebv` import candidate (not just fixing the diagnostic
text). Wave 1 is computational-domain machinery — protocol-agnostic by
design; nothing here pre-judges the physical bridge.

## Ground truth (verified in source, 2026-09-25)

- `resolve_imports` (`src/import_resolver.rs:404-461`): index walk; resolved
  items spliced at the import site and revisited — nested imports resolve
  recursively. Cycle guard: `in_progress` (`:938-944`).
- Default arm parses EVERYTHING as bare `.bv` (`lex_source` + `Parser::new`,
  `:954-976`) — no `lex_for_path` (`.f` layouts), no
  `preprocess_source_for_path` (`.rbv` markup), no per-dialect prelude.
- Candidate search tries `{mp}.bv` in 3 rounds (`:886-922`); diagnostic at
  `:924-934` promises `{bv,ebv}` — the stale-text bug this wave makes true.
- Prelude = Parsed-stage AST plugins chosen by ROOT extension only
  (`pipeline.rs:810-812` → `filter_for_extension`, `plugin/mod.rs:264-278`);
  `pm.run_ast(StageKind::Parsed, &mut items, &mut universe)`
  (`plugin/mod.rs:299`) is reusable per-module. Imported modules never pass
  the Parsed stage → never get their dialect's prelude.
- Collisions: import-vs-import guarded (`record_imported_names`, `:469-516`,
  impls exempt, identical-dump escape, diamond benign). ROOT-vs-import
  unguarded — `dedup_items` (`:1227-1244`) keeps the last occurrence;
  the winner depends on statement order. That order-dependent shadowing is
  the trap item 4 removes.
- `resolve_imports` call sites: `pipeline.rs:888`, `:984`, `compile.rs:254`.
- Conformance: `classify()`/`SourceKind` (`conformance.rs:155-183`,
  `:35-50`); sweep auto-discovers `examples/**` per dialect
  (`:212-349`).
- e14a merged to main (`d0341631`): `.ebv` electronics (parser, backend,
  `prelude-electronics`, `examples/electronics/*.ebv`) is on main; `.dbv`
  schema+record grammar exists (`examples/data-briev/registry.dbv`,
  `check_data_source` at `pipeline.rs:756`).

## C0 — Post-merge verification — DONE 2026-09-25, GREEN

`cargo test --lib` on merged main: **2677/0**. Both `.ebv` fixtures
(`usb_sensor`, `led_blinker`) check and build clean post-merge; KiCad
emission works. No repo defects.

Operational lesson (same class as the stale-archive bug, 2026-09-25
BUGS.md build.rs entry): judging a merged tree with a pre-merge binary
produces confident, wrong diagnoses — three phantom "divergent check
paths" chased for an hour before the stale `target/release/brievc`
(pre-`d0341631`) was suspected. Rule: after ANY merge, rebuild before
the first CLI judgment. The conformance sweep (fresh lib) was right
throughout; the stale CLI was wrong.

## C1 — Provenance plumbing (machinery only, zero behavior change)

**DONE 2026-09-26**: 3 tests green (`test_provenance_survives_two_level_chain`,
`test_provenance_root_items_are_none`, `test_provenance_cache_shared_origins`);
suite 2680/0 (2677 + 3); Praetor parity (filter metrics identical to baseline,
`resolve_import` improved 58→51 cognitive / 375→337 lines — CSS/SVG loaders
extracted to stay under the line budget). CSS/SVG/DBriev loads register
records; `import "target"` (generated board items) and root items stay `None`.

New on `ImportResolver`:

```rust
pub modules: Vec<ModuleRecord>,      // append-only; index = module id
pub item_origins: Vec<Option<u32>>,  // parallel to FINAL items; None = root
// ModuleRecord { specifier: String, source_path: PathBuf, kind: SourceKind }
```

Mechanics: every `items` mutation in `resolve_imports` mirrors on a shadow
`origins` vec (`remove`⇔`remove`, `splice`⇔`splice`). `resolve_import`
returns `(Vec<TopLevel>, Vec<Option<u32>>)` (private); the
`loaded_modules` cache stores the pair; `dedup_items` becomes
`dedup_items_with_origins`, preserving survivors' origins. Public
`resolve_imports` signature unchanged — callers read the new fields.

Tests: `test_provenance_survives_two_level_chain` (A→B→C: C's items carry
C's id, not B's); `test_provenance_root_items_are_none`;
`test_provenance_cache_shared_origins` (diamond → same id both sites).

## C2 — Truthful extension dispatch (items 1+3)

**DONE 2026-09-26**: 4 tests green (`test_ebv_module_imports`,
`test_diagnostic_names_only_searched_exts`,
`test_rbv_module_imports_briev_remainder`, `test_dbv_import_rejected`);
suite 2684/0; Praetor no-new-diagnostics (`resolve_import` improved
58→32 cognitive / 375→325 lines via `search_module_file` +
`not_found_diagnostic` extraction). Implemented per plan with one
documented refinement: explicit known code extensions (`.bv`, `.ebv`,
`.rbv`, `.abv`, `.sbv`) resolve as single-extension searches (this is how
`.rbv`/`.abv`/`.sbv` become reachable — implicit search stays
`.bv`→`.ebv` only); imports now lex via `lex_for_path` (shared with the
root path). **Data refusal deviation**: the explicit `x.dbv` specifier
still loads typed constants (learn-briev/11-triggers.md:237 documents
`import bindings from "std/bindings/system_triggers.dbv"` — feature kept),
so `DataStructured | DataLine` rejection is implemented as diagnostic
enrichment instead of a dead classify arm: the not-found error probes for
`{mp}.dbv`/`{mp}.dbvl`, explains "data dialects are not code imports",
and gives the explicit-specifier fix (what/why/fix, house style).

1. Each search round tries `{mp}.bv` then `{mp}.ebv` — the existing
   diagnostic's promise becomes true. Wave 1 searches exactly these two;
   `.abv`/`.sbv`/`.rbv` remain unsearchable until their waves, and the
   diagnostic names only what is searched.
2. Diagnostic (house style, `src/errors.rs`): what was searched, why, fix.
3. Per-kind parse after `classify(&resolved_path)`:
   - `Briev | Electronics`: `lex_for_path` + shared parser (electronics
     syntax is lexer/AST-level on main).
   - `Rendered`: `preprocess_source_for_path` first, parse the remainder.
   - `DataStructured | DataLine`: error — data dialects are not code
     imports (what/why/fix).
   - `Accelerator | Silicon`: parse as Briev with a dated comment — per-kind
     semantics land in their waves; shared parser accepts them today.

Tests: `test_ebv_module_imports`; `test_diagnostic_names_only_searched_exts`;
`test_rbv_module_imports_briev_remainder`; `test_dbv_import_rejected`.

## C3 — Per-module dialect semantics (item 2)

- `ImportResolver` gains
  `plugin_factory: Option<Box<dyn Fn(&str) -> Result<PluginManager, String>>>`
  (ext → scoped PM, built via the existing `build_plugin_manager` +
  `filter_for_extension(ext)`, honoring `--no-std`/`--disable-plugin`),
  installed by the three call sites; `None` (tests) = today's behavior.
- Module of kind K: run its ext's Parsed-stage prelude on the module items
  (`run_ast(StageKind::Parsed, …)` — the plugin inserts the `Import$` stdlib
  anchor; the recursive walk resolves it). Stdlib double-splice impossible:
  same-path cache + identical-dump dedup.
- Universe threading: `&mut TypeUniverse` passed through from the call sites
  (they own `parsed_universe`).
- Profile flags (strict/bare/formatted) read from the MODULE's path for the
  provenance record; per-module profile ENFORCEMENT stays per-feature (no
  new gates invented here).

Tests: `test_ebv_import_gets_electronics_prelude`;
`test_bv_import_gets_native_prelude`; `test_prelude_not_double_spliced`;
`test_no_plugin_factory_is_todays_behavior`.

## C4 — Cross-dialect collision rule (item 4)

Before dedup: any pair sharing an `item_key` (`(kind, name)`) whose origins
differ → hard error unless identical `{:?}` dump (existing benign-dup
escape). Same-module diamond stays benign; impls stay exempt; positional
root-shadows-import is REMOVED (order-dependent shadowing across module
boundaries was the trap). Aliasing = the existing selective rename
(`import "m" { kernel as gpu_kernel }`) — documented as THE escape.

Diagnostic (house style): names both sides, why (shared root namespace,
order-dependent winner), fix (rename or `as`).

Conformance sweep flags any example relying on positional shadowing; fix
those examples in this commit.

Tests: `test_root_import_collision_is_error`;
`test_collision_across_dialects_is_error`; `test_identical_defs_benign`;
`test_diamond_import_benign`; `test_alias_resolves_collision`;
`test_impls_stay_exempt`.

## C5 — `.rbv`→`.bv` declared edge (item 5)

- Interop plan edge-registry table; row 1: `.rbv`→`.bv` — same-file binding
  surface `b-bind`, write-contract routing = single-writer proof
  (`pipeline.rs:586-657`), view keeps targets live (`:574-579`).
- Corpus: `examples/` example exercising one read binding + one
  write-routed binding; sweep picks it up.
- Test: `test_rbv_edge_declared_surface` (frontend_check on the example).

## C6 — Docs (same arc)

SPEC §7 (resolution order, `.ebv` candidates, collision rule, `as`),
`glue-ffi.md` (provenance + collision behavior), interop plan wave-1
status rows. AGENTS.md e14a exclusion line: left alone for now (branch
still live; manual edit deferred by owner).

## Verification per commit

`cargo test --lib` green · the commit's new tests · Praetor on changed
files (`--target` = DIRECTORY) · conformance sweep green · no new warnings.

## Risks

- C3 is the intricate commit (scoped PM construction + universe plumbing).
- Recursive resolution depth is guarded (`in_progress`); cache keys
  unchanged.
- The e14a branch is still live — future merges may touch `pipeline.rs`;
  this wave's files (`import_resolver.rs`, `conformance.rs`, small call-site
  plumbing) are low-overlap but merges need review discipline.
