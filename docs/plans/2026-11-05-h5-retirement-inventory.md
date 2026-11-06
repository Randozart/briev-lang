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
