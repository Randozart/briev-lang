# H5 retirement inventory (2026-11-05) — what retires, when, and how

Planning artifact for the H5 phase of the typed-HIR migration
(`docs/plans/2026-11-04-typed-hir.md`,
`docs/plans/2026-11-04-typed-hir-h3-h5-execution.md`,
`docs/plans/2026-11-05-h4-completion-loop-convergence-swan-song-emitter.md`).
Pure documentation of the current state — nothing here changes code.
Update it as families land; the inventory IS the H5 work list.

## Current coverage state (at `e32d9b25`)

### Statements — 20/22 kinds native through the seam
Native (HIR-driven, `emit_hir_statement`): Expression, Term, Tail,
Assign, Let, Guarded, Block, SyncBlock, Mutex, Foreach, Match, Break,
Trap, Halt, Yield, Check, Gate, Rollback, FreeHint, KeepHint, EndProgram.
Conversion-based by design:
- **Defer** — queues AST bodies on the defer stack for later flush;
  conversion is semantically identical (flush runs `emit_statement`).
  Retires when the defer stack carries HIR bodies (small refactor:
  `defer_bodies: Vec<Vec<HirStmt>>` + `flush_defer_cleanup` walking the
  seam).
- **ArrowAssign** — every dispatch path inspects value/target AST shapes
  (wire fire, streams, insert/extract strategies, provenance). Retires
  with the register-taking refactor of the strategy helpers
  (`find_insert_strategy`/`find_extract_strategy` consumers), a focused
  session.

### Expressions — 21 families native, all reconstructible kinds seam-covered
Native (operand-split or leaf): literals, identifier (605-line shared
loader), unary, binary (And/Or short-circuit falls back — intentional),
consume, await, deref, cast (operand split at `emit_cast_operand`),
addr-of, call (dual-fueled user-call path), field, reflect, method-call,
is-type, tuple (dual-fueled materialization), struct-literal, block
(seam-recursed children), index (receiver-register split),
slice (array-register split), match (dispatch; bodies-via-seam queued).
Conversion/dispatch-based by design:
- **Lambda / Spawn** — semantic dispatch consumes AST shapes
  (`PendingClosure` construction; task/pool/storage helpers). Both are
  thin dispatch over AST-taking helpers; conversion is lossless and
  gated. Retire behind helper refactors if ever worth it — low priority,
  they are rare and safe.
- **If** — synthesized-only (derive engine); no arm planned (parser note:
  Briev has no `if`). The HIR carries the node; conversion covers it.
- **Match expr** — full reconstruct dispatch; bodies-via-seam dual-fuel
  queued (see the emit_match scoping notes in the H4 plan — five
  consumption points, probe discipline, no guards in the expression form).

## What still reads the AST (the H5 inventory)

Even with every statement and expression kind seam-covered, these
surfaces still walk the AST:
1. **Emitter entries** — `emit_definition`/`emit_transaction` take AST
   items; the body hook substitutes converted HIR bodies (the choke
   point). H5: entries take `&HirCallable` directly and the converted-AST
   shim dies.
2. **Declaration/ABI surfaces** — params, outputs, contracts, exports,
   global initializers, `emit_ssa_main`'s txn walk, `reordered_items`,
   export ABI, module metadata: AST-item walkers. H5: these consume
   `HirProgram` (the analyses already do).
3. **The `to_ast` bridge** — retires piece by piece as the last
   dispatch-only families go native and the entries above switch.
4. **AST-only analyses not yet ported** — first H5 task executed
   (2026-11-05): `analyze_program` still derives these from `items`
   WITHOUT an HIR mirror (H3 covered the semantic set; these are the
   remainder, ordered by likely value):
   `dependency_graph::build`, `density::compute`,
   `modulo_partition::detect`, `destination_ownership::analyze` (HIR
   port EXISTS but the flipped consumer is boundary-only — verify),
   `dyn_concrete_sets::analyze`, `web_commits::analyze`,
   `electronics::derive_netlist`, `global_lifetime::analyze`,
   `accel::analyze`, `batch_shape::detect`, `coalescing::analyze`,
   `image_storage::detect`, `gpu_schedule::build`,
   `reclamation::analyze` (+ its walk — reference-verified analysis per
   the #62/#65/#68 lineage; HIR port optional, parity suite first),
   `termination`, `watchdog`, `casing`, `dataflow`, `range`, `region`,
   `purity`, `allocation` (verify consumer sites; some may consume only
   AnalysisResults fields computed above). Each joins the inventory with
   its consumer named before its HIR port is written — same
   named-consumer rule as the Intel harvest.
   **Ported + consumer flipped (2026-11-05):** `web_commits` —
   `src/hir/web_commits.rs`, consumer flipped under
   `BRIEV_HIR_EMIT` with a debug equality assert; module unit test pins
   AST/HIR parity on the original discriminator source. (Sweep note: one
   run showed 130/5 — a transient AST compile flake; the rerun and a
   second consecutive run are 131/4, 0 mismatches. Watch for recurrence:
   a genuinely failing AST fixture would be a real bug.)
   **Ported + consumer flipped (2026-11-05):** `modulo_partition` —
   `src/hir/modulo_partition.rs`, consumer flipped under the flag with a
   debug equality assert; 2 parity tests (eight-way + negatives) compare
   HIR == AST including the negative cases. Coverage mirrors the AST's
   BARE-reactive-txn filter (`node` is the reactive keyword; `txn` is not
   reactive — the test sources use `node`).
   **Ported + consumer flipped (2026-11-05):** `density` —
   `src/hir/density.rs`, consumer flipped under the flag with a debug
   equality assert; 2 parity tests (float-vs-int txns, const-typed
   propagation). New HIR fidelity field required by the port:
   `HirProgram::const_types` (the AST keeps `TopLevel::Constant.ty`, which
   seeds the float fixpoint).
   **Ported + consumer flipped (2026-11-05):** `dyn_concrete_sets` —
   `src/hir/dyn_concrete_sets.rs`, consumer flipped with a debug equality
   assert; 3 parity tests mirror the AST fixtures (Switch/Inline/Vtable/
   ignored). New HIR fidelity fields required: `HirTypeDef.traits`
   (declared parent links) and `HirProgram.trait_functions` (ALL trait
   function names — callable lowering skips body-less requirement slots,
   so conformance reads this table).
   **Ported + consumer flipped (2026-11-09, H5 Leg B):** `termination` —
   `src/hir/termination.rs`, 4 curated parity tests. `analyze_dual` lowers a
   fresh HIR at the analysis point (before the plugin stages, by design) and
   uses it under `BRIEV_HIR_EMIT`, with the AST result as fallback and debug
   oracle. Coverage note: top-level non-let statements cannot be mirrored
   (HIR carries globals only) — inert for this rule.
   **Already HIR-sourced transitively:** `batch_shape` — its input map
   comes from `build_swan_songs`, which is HIR-derived under the flag
   (H4 slice B). No port needed; add it to the flip-batch verification
   list only.
   **Ported, PORT-ONLY (2026-11-05):** `watchdog` —
   `src/hir/watchdog.rs`, 7 parity tests mirroring the AST fixtures
   (unknown-trigger, no-handler, no-conflict, does-not-falsify,
   falsifies-ok, on-fire validation, empty). Fidelity fields required:
   `HirWatchdog.is_required` (the `![..]` vs `?[..]` flag — the AST reads
   it for the required-handler-conflict path), `HirProgram.trigger_names`
   (HIR had only a `has_triggers` bool), and `HirCallable.is_impl_fn`
   (the on-fire callable set = bare txns/defns + type members; impl
   functions excluded — the AST walker never unwraps impl blocks).
   **Ported + consumer flipped (2026-11-09, H5 Leg B):** `analyze_dual` and
   `check_on_fire_handlers_dual` build a fresh HIR at the watchdog point
   (after the plugin stages, before the backend tree) and use it under
   `BRIEV_HIR_EMIT`, with the AST result as fallback/debug oracle. Parser
   note for fixtures: watchdog syntax is `?[cond]` / `![cond] ->
   handler(val)` after the postcondition brackets — no `@`; trigger
   references in guards are bare identifiers.
   **Ported + consumer flipped (2026-11-05):** `global_lifetime` —
   `src/hir/global_lifetime.rs`, consumer flipped with a debug equality
   assert. The field-initializer map is dual-built (HIR: state decls get
   an Int-0 image mirroring the AST builder's Decimal(0); top-level lets
   keep their declared initializer); node_order/foldable stay
   transition-graph-derived. Touch sets via the ported
   `collect_statement_identifiers`/`collect_state_identifiers` mirrors
   (incl. the 2026-07-21 addr/deref/consume fix and the 2026-08-01
   arrow fix); manual-free walk mirrors Free#-through-cast and
   free/keep hints. `GlobalLifetime` gains PartialEq for the assert.
5. **Interpreter** — deliberately OUT of H5 scope (Rule 5: the
   interpreter is the reference; it stays AST-backed).

## Retirement mechanics (the H5 sequence)

1. **Flip the default**: `hir_emit_enabled()` returns true; `BRIEV_HIR_EMIT`
   becomes an opt-OUT escape hatch for one release. Gate: full battery
   (lib, 21+ HIR suites, sweep, A/B — now comparing opt-out vs default,
   still byte-identical), determinism, daily_use_smoke, benchmarks.
2. **Family-by-family AST-arm deletion**: for each native family, delete
   the AST arm body in `emit_statement`/`emit_expr_inner` down to
   `unreachable!`/panic, then delete unreachable arms entirely when a
   whole statement/expr kind has no remaining AST caller. Every deletion
   is gated by the same battery. Rule 6 (additive) inverts here: H5 is
   the sanctioned subtraction phase.
3. **Emitter entries switch**: `emit_definition`/`emit_transaction` take
   `&HirCallable`; the substitution hook, `hir_stmt_bodies`, and
   `hir_sourced_body` die. The `to_ast` bridge shrinks to zero and the
   module deletes.
4. **Analysis consumers flip from `items` to `analysis.hir`** — most
   already did in H4; sweep the remainder.
5. **Final state**: the backend consumes `HirProgram` + declarations;
   the AST exists only up to lowering (parser/desugar/typecheck/plugins)
   and in the interpreter.

## Preconditions before step 1 (the flip)

- [ ] `emit_match` bodies-via-seam dual-fuel landed (queued).
- [ ] `ArrowAssign` register-taking refactor landed OR its conversion
      path accepted as permanent (owner decision — it is safe either
      way; the refactor is about nativeness, not correctness).
- [ ] `Defer` HIR-stack refactor landed or accepted as permanent.
- [ ] Structural families Lambda/Spawn accepted as permanent dispatch
      (owner decision; low priority).
- [ ] Full-corpus A/B green for N consecutive days of landings (the
      tripwire never fired).
- [ ] Runtime goldens green (`daily_use_smoke` 37/37) and the benchmark
      suite MATCH across the board.
- Owner sign-off on the flip itself (it makes HIR the default, not just
  an experiment).

## Post-retirement invariants to preserve forever

- `hir::verify` id/export invariants gate every consumer flip.
- The debug sweep + `hir_emit_ab.sh` (inverted to opt-out) stay forever:
  they are the regression net that replaced trust.
- The interpreter stays the AST reference (Rule 5) — its divergence from
  the compiler is a bug signal, per the existing BUGS.md discipline.
- Recognition ban, derivation gate, and the named-consumer rule apply to
  any future emission feature: new emission lands on HIR first, always.

## H5 default flip (2026-11-09)

Typed-HIR emission is the default (`hir_emit_enabled()` true unless
`BRIEV_HIR_EMIT=0|false|off|no`). Owner sign-off given after: HIR/opt-out
suites both 3624/0, sweep 139/0, A/B byte-identical, runtime benchmarks MATCH.
AST arms remain for one release as the opt-out path; ports and family-by-family
deletion continue.

**Ported + consumer flipped (2026-11-10):** `dependency_graph` —
`src/hir/dependency_graph.rs` mirrors the AST variable graph (state decls,
trigger names, `TriggerBinding` names via the new
`HirProgram.trigger_bindings` fidelity field, bare-txn read edges, Kahn topo
order + bit indices) and returns the same result struct. The backend consumer
flips under `BRIEV_HIR_EMIT` with a `debug_assert_eq!` oracle. Parity tests
cover empty/trg-only/chains/cycles. Prerequisite fixed first: the AST graph's
Kahn queue was seeded from a `HashSet` and iterated `HashMap` dependents, so
trg bit assignments varied per process — both are sorted now
(`tests/dependency_graph_determinism.rs`, 4 separate compiles).

**Ported + consumer flipped (2026-11-10):** `casing` — `src/hir/casing.rs`
mirrors the identifier-casing advisories (functions snake_case; types,
structs, traits, cells PascalCase) over `HirProgram` declared-name surfaces.
New fidelity fields preserve the AST's origin distinction:
`type_names`/`static_struct_names`/`trait_names`/`cell_names`. The shared
`check_name` helper backs both implementations; `analyze_dual` flips the two
pipeline call sites with a debug equality oracle. Tests: conforming-silent and
each-category parity.

**Ported + consumer flipped (2026-11-10):** `purity` — `src/hir/purity.rs`
mirrors the state-purity advisory (effective-Void, called, no observable
effect; conservative call-graph fixpoint over raw-effect statements and
Call/Spawn edges). `analyze_dual` flips the compile.rs call site with a
rendered-warning debug oracle. Parity tests cover effectless/printing/
transitive/uncalled/value-output/spawn shapes.

**No-runtime-consumer decisions (2026-11-11).** `range`
(`AnalysisResults.param_ranges` is always empty) and `dataflow`
(`dataflow_errors` is always empty) keep their analysis code but are NOT H5
port targets until a consumer is designed. `destination_ownership` is
analysis-only by its own header comment (no backend lowering consumes it) and
is deferred the same way. H5 effort goes to analyses with live consumers.
