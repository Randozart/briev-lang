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

### DONE (2026-09-26)

Shipped as committed: field + `pipeline::module_plugin_factory` +
`ImportResolver::run_module_prelude` (extracted for the Praetor gate),
installed at `compile_to_typed`, `pipeline::parse_and_check`, and
`compile::compile_source`; `library::parse_and_check` keeps `None` (that
path runs no plugin stages even at the root). Suite **2688/0** (2684 + the
4 tests above); conformance sweep green; `brievc check` green on both
`.ebv` fixtures. Praetor gate: 53→52 diagnostics — no new entries (one
removed: `check_source_for`, migrated onto the new `Default`); four
pre-existing ">50 lines" values grew by exactly +1 (the one-line factory
assignment at each call site).

Three deviations from the text above, each recorded at its site:

1. **Universe threading dropped.** All three root call sites run Parsed
   with a BLOCK-LOCAL `parsed_universe` that is discarded before resolution
   — threading `&mut TypeUniverse` through `resolve_imports` would give
   modules MORE state than a root file gets (root parity broken) plus ~25
   test-call-site churn. Each module's prelude gets a fresh
   `TypeUniverse::new()`, mirroring the root exactly.
2. **`std/…` specifiers skip the prelude.** Stdlib files are the prelude's
   CONTENT, not its consumer (root prelude + flat inlining already put
   their names in scope), and running it would self-cycle: verified std
   files carry prelude anchors (`lib/std/io.bv` has `import`), so an std
   file's own prelude would re-insert the std bundle while its `std→std`
   imports are still in `in_progress` → the 2026-07-01 cycle guard errors
   on every build. `std/electronics.bv` remains reachable from any `.ebv`
   module; it just parses plain.
3. **`BuildOptions: Default` added** (Rule 17: the tree hand-rolled this
   field set in 6+ literals; `check_source_for` migrated, CLI literals stay
   explicit overrides). Provenance comments live on the field, the factory,
   and the helper — call sites are bare assignments.

Profile flags already read the MODULE's path: the factory forwards
`resolved_path` to the same `build_plugin_manager` → `get_extension` a root
compile uses — no extra gate.

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

### DONE (2026-09-26)

**Gate implemented** (`import_resolver.rs`): `check_cross_module_collisions`
runs in `resolve_imports_inner` before `dedup_items_with_origins`, at every
nesting level. `record_imported_names` retired (the gate sees every pair the
record saw, keyed like the dedup it guards, plus the root pairs the record
could not see). Module alias is no longer a collision escape — the alias tag
never renamed inlined items, so differing aliases only silenced the retired
record while dedup silently dropped one name; `test_module_alias_is_no_longer_a_collision_escape` pins that. SPEC §7.2 edited: the escape list is "a selective rename or an identical definition" (alias withdrawn), plus a new line stating the no-overload invariant (one unqualified name per scope, across modules).

**No-overload ground truth** (verified in source before fixing): the
typechecker holds ONE signature per callable name — `fn_param_types` /
`fn_return_types` are `HashMap<String, …>` keyed by name only
(`src/typechecker/mod.rs:1961`, `:2002`), registration is plain `.insert`
(`:4706`, `:5439`) so a second same-name defn silently overwrites the first.
There is no `(name, param_types)` key anywhere in call resolution. C4's
pre-dedup gate is the only thing that catches the pair.

**Sweep fixes (this commit)** — the gate surfaced 3 active sources; fixing
them exposed the stdlib's same-name-different-signature pattern, fixed
proactively:

- `examples/electronics/led_blinker.ebv` — deleted the local `type Connector`
  (identical to `std/electronics.bv`'s except two unused `spec` declarations;
  the instance sets neither, so the netlist is unchanged).
- `examples/electronics/usb_sensor.ebv` — renamed the local types that
  shadow std's: `Resistor` → `RatedResistor` (keeps the concrete
  `spec Rating: 0.25W` — switching to std's `Rating: any` would turn the
  dissipation proof into INFINITY, a contract weakening, Golden Rule 1),
  `Led` → `BomLed` (the no-physics variant; std's `Led` requires
  `ForwardVoltage`/`DynamicResistance` the instances don't supply),
  `Capacitor` → `BoardCapacitor` (keeps `spec Tolerance: any` — std's
  `Capacitor` has no tolerance clause, and the decoupler pins sit on driven
  nets, so dropping the declaration makes the pin-tolerance contract fire).
  The `usb_sensor_gate_fixture` mutation anchor in
  `src/backend/electronics/mod.rs` updated to the new name.
- `lib/std/string.bv` — deleted its `char_at` defn (`-> String`, 1-char
  substring; zero callers repo-wide — `char.bv`'s `char_at -> Char` is the
  canonical one, called by lexer/reader/soa/needs_state), and narrowed the
  `std/char.bv` import to `{ int_to_char }` (its only char-borne use, ×9).
  The builder import is already selective (`{ StringBuilder, new_builder,
  append_char, append_str }`) — the full import would re-leak `len` /
  `to_string` against string.bv's own.
- `lib/std/char.bv` — builder import narrowed to
  `{ StringBuilder, new_builder, append_char }`; the full import used to
  leak `len`/`to_string` into every importer's scope (e.g. string.bv's via
  its `std/char` import).
- `lib/compiler/token.bv` — builder import narrowed to
  `{ StringBuilder, new_builder, append_str, append_int, append_char }`.
- `lib/compiler/lexer.bv` — gained selective `std/string`
  (`{ char_at, len, to_int, to_float }`) and `std/string_builder`
  (`{ StringBuilder, new_builder, append_char, to_string }`) imports. This
  also fixes lexer's two pre-existing `len(String)` type errors (its only
  import was `std/char`, whose builder-nested `len` took a StringBuilder).
  Its remaining type errors (range `+`, `Char == Int`, `TokenEof`) are
  pre-existing tamer-WIP, baseline-verified unchanged in kind.
- `tests/_iso.bv` — deleted its local `char_at` (duplicated `char.bv`'s
  with a different dump; `char.bv`'s is in scope via its `std/char` import).

**Latent pairs left in place (verified out of every real scope, no code
change)**: `hashmap.bv` `len<K,V>` + `is_empty` vs `string.bv`/`string_builder.bv`
's (no swept scope co-imports both — `hashmap.bv` imports nothing, and the
only co-importer is `main.bv`, which is parse-broken WIP that never reaches
resolution); `option.bv`/`result.bv` `unwrap`/`is_some`/etc. (importer sets
disjoint except the same parse-broken `main.bv`). `lib/std/encoding.bv`
carries INTRA-file duplicate defns (`is_hex_string`, `reverse_string`,
`count_char`, `url_decode_simple` each ×2) — same-origin, so C4-correct
(benign) and invisible to the gate; it is tamer-track WIP with zero callers
repo-wide and is deliberately left untouched per the tamer ownership note.

**Tamer-track note**: `lib/compiler/*.bv` are excluded from the conformance
sweep ("tamer WIP, coordinate before migrating" — `conformance.rs:227`).
C4 is invisible to the parse-broken ones (`ast`, `main`, `parser`,
`proof_engine`, `range`, `typechecker`, `call_graph` — parse fails before
resolution). The checkable ones (`token`, `lexer`, `needs_state`, `reader`,
`soa_reorder`) were baseline-checked before and after: token/needs_state/
reader/soa_reorder unchanged-OK, lexer improved from 2 type errors to the
pre-existing tamer-WIP set (see above).

**Verification**: `cargo test --lib` = 2694/0 (2688 baseline + 6 new C4 tests
+ sweep now passing). Conformance sweep green. `brievc check` on all touched
files: string/char/encoding/token/needs_state/reader/soa_reorder/_iso OK;
lexer improved (above); test_char parse-broken pre-existing (WIP test, not in
active roots). Praetor: the Rust-side changes were already gated at the C4
implementation (import_resolver/pipeline/compile passed at 39/39); this
commit's additional changes are `.bv`/`.ebv`/SPEC/plan (Praetor does not
analyze them) plus a 3-line test-anchor edit in `backend/electronics/mod.rs`
(no new diagnostics — anchor string only, no complexity/line/param change).

## C5 — `.rbv`→`.bv` declared edge (item 5)

**DONE 2026-09-26.** The first declared edge, committed.

- Interop plan edge-registry table; row 1: `.rbv`→`.bv` — same-file binding
  surface `b-bind`, write-contract routing = single-writer proof
  (`pipeline.rs:586-657`), view keeps targets live (`:574-579`).
- Corpus: `examples/view-bind-edge.rbv` — one READ binding (`b-text` observes
  the root signal) + one WRITE-routed binding (`b-bind` routes to the unique
  user-writer txn `set_greeting`, single-writer proof); the conformance sweep
  picks it up (active source, `.rbv` → `Rendered`, `conformance.rs:178`).
- Test: `test_rbv_edge_declared_surface` (`import_resolver.rs`) — the example
  passes the same frontend check the conformance sweep runs: the `.rbv`'s
  Briev remainder parses, the write binding's target stays live for the txn's
  write + flush, and the single-writer route resolves.

## C6 — Docs (same arc)

**DONE 2026-09-26.** The documentation arc that closes Wave 1.

- **SPEC §7** — §7.1 gains the extension-search rules (extension-less
  searches `.bv`→`.ebv`; explicit known code extensions search only that
  extension; the not-found diagnostic names exactly what it searched; data
  and asset files are not code imports). §7.2 carries the collision rule +
  no-overload invariant (landed in C4). New §7.5 (per-module dialect
  semantics): each imported module runs its own dialect's Parsed-stage
  prelude (`.ebv` → electronics, `.bv` → native), a `.rbv` module contributes
  only its Briev remainder, profile flags are read from the module's own
  path, and `std/…` specifiers skip the prelude (they are the prelude's
  content). Provenance (every item carries its module of origin; the root's
  don't) is stated here and referenced by the §7.2 gate.
- **`glue-ffi.md` §4.3** — cross-module provenance + the pre-dedup collision
  gate, with the bridge-modules-are-not-exempt note and a worked `CStr`
  example (name-keyed, no overloading).
- **Interop plan** — the per-pair table row 1 is now DECLARED (C5) and the
  Waves section marks Wave 1 DONE (C0–C5) with a pointer to this plan.
- **AGENTS.md e14a exclusion line** — left alone (branch still live; manual
  edit deferred by owner).

**Verification**: docs-only commit (SPEC, `glue-ffi.md`, both plans) — no
Rust changed, so no Praetor/cargo gate; conformance sweep and `cargo test
--lib` remain green (2695/0 from C5).

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
