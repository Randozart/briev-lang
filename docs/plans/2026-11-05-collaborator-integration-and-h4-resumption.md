# Collaborator integration + H4 resumption (2026-11-05)

Integrates Otto's nine open PRs into `main`, then resumes the H4 slice
work on `main`. This plan records both the operational sequence and the
HIR rationale that shapes the merge obligations.

## Why the HIR matters (rationale, for the record)

A typed, normalized tree lowered once from the post-desugar AST. Payoffs,
each with session evidence:

1. **One index, proven consumers.** 15 analyses have HIR implementations,
   each paired with a parity suite (HIR == AST over the full corpus,
   mechanically checked — 132–135 files, 0 mismatches, 0 skipped).
2. **The cross-check catches real bugs.** Eight defects found by building
   HIR and asserting equality: trait-default over-rooting, dropped
   protocol binding fns, lost destructure names, incomplete
   `Statement::PartialEq`, sync-wrapped txn classification, random
   vector-group ordering, two nondeterministic emit-site HashMap
   iterations, the field-receiver double-eval register burn.
3. **Frontend-driven dispatch, enforced.** 7 backend consumers read HIR
   under `BRIEV_HIR_EMIT=1` with debug equality asserts.
4. **Dual-mode emission with a standing tripwire.** 20/22 statement kinds
   + 14 expression families emit from HIR; `hir_emit_ab.sh` byte-compiles
   the corpus both ways; the debug sweep re-runs cross-checks per compile.
5. **H5 substrate.** One semantic tree for analyses, proofs, emission;
   audit ratchet at zero (PluginIntercept represented).

Costs: one lowering per compile; **mirror maintenance until H5** (every
AST behavior fix needs an HIR mirror — #69 is the live example); the
`to_ast` bridge until structural expression families land.

## Outcome (2026-11-05 — executed)

- Phase 0: `feat/typed-hir` verified fully contained in `main` (nothing to
  merge); HEAD compiles from a clean checkout (fixed a missing
  `pub(crate) emit_hir_operand` that had been left uncommitted).
- Phase 1: #63, #64, #66, #68, #70, #67, #71 merged `--no-ff`. #64/#66/#68
  BUGS.md keep-both; #66+#68 reclamation.rs/walk.rs conflicts resolved as
  unions (both modules; struct fields `variant_types` + `methods`; method
  arms = intrinsic-check-first then call-graph edges). #70 auto-adapted our
  HIR Field arm; sweep re-verified.
- Phase 2: #69 merged — AST Match merge fixed + **native HIR Match arm
  mirrored** (merge stays open); his runtime fixture green; full battery
  green. #65 merged — `emit_let_statement` kept; his destructure port
  applied to BOTH tuple paths in the helper (elem_tys from the
  initializer's semantic tuple type; element extraction through
  `tuple_values::emit_tuple_element`; `unbox_tuple_float_element`
  retired); reclamation walk union (#68 lifetime facts + #65 tuple
  shapes); `tests/tuple_values.rs` green.
- #65 flag: GitHub API refuses reopen with "These commits are already
  merged" (422) — the content is in base and GitHub's validation
  acknowledges it; the boolean `merged` flag cannot flip programmatically
  (needs base-push detection while open). Closed state, content verified.
- Final battery at `364f42eb`: lib 3512, all HIR suites, sweep 131/0,
  release A/B byte-identical, determinism, daily_use_smoke 37/37.

## Branch state (Phase 0)

All H4 work is already on `origin/main` (HEAD `be420d69`); there is no
separate HIR branch. Verify HEAD == origin/main, confirm any stale
`feat/typed-hir` ref is contained in main.

## Phase 1 — trivially mergeable PRs (numeric order, `--no-ff`, push each)

| PR | Content | Gate |
|----|---------|------|
| #63 | `read_file` descriptor lifetime fix (fs.bv + tests) | `cargo test --all` |
| #64 | Native timing, /proc sampling, bounded histograms (stdlib) | `cargo test --all` |
| #66 | Socket borrowing effects + concrete-method call-graph edges (reclamation + memory-effects.toml) | `cargo test --all`; resolve vs #68 if they touch `reclamation.rs` jointly |
| #68 | Structured-value lifetime facts (reclamation walk) | `cargo test --all` |
| #70 | Field receiver evaluated exactly once — **already adapts our HIR Field arm** (HIR route passes `None`; shared loader evaluates once) | `cargo test --all` + **debug sweep** (Field route through the seam must stay byte-identical) |
| #67 | Nonblocking socket steps + finite poll waits (stdlib) | `cargo test --all` |
| #71 | Bounded incremental HTTP response framing (stdlib) | `cargo test --all` |

All nine are green (`Default gates=success`). None of the clean seven
conflict against `main` (merge-tree verified).

## Phase 2 — conflicting merges, thoroughly tested

### #69 — statement-match fallthrough (merge first of the two)

- His fix: the Match merge consulted only the LAST arm's `terminated`
  flag, dropping code after a match whose earlier arms continued. New
  semantics: the merge stays OPEN (`terminated = false;
  cur_block = Some(merge)`); the no-match edge and continuing arms always
  reach it.
- Conflict resolution: keep our extractions (Term/Foreach/Assign/Let
  helpers); take his Match-merge fix in the AST arm.
- **HIR mirror (same-merge follow-up commit):** our native Match arm in
  `emit_hir_stmt.rs` still carries the old `if terminated { unreachable }`
  — replace with the fixed open-merge semantics. The seam's byte-identity
  harness would catch the divergence, but we mirror proactively.
- Verify: his fixture + `tests/statement_match_fallthrough.rs`, full
  sweep, A/B, determinism, `daily_use_smoke`.

### #65 — tuple value types (merge last)

- New `tuple_values.rs`; removes `unbox_tuple_float_element` from
  `emit_stmt.rs` (logic moves); edits the Let arm inline — but our main
  already extracted that arm into `emit_let_statement` (his base predates
  the extraction).
- Conflict resolution: keep our extraction; port his tuple-transport
  Let-arm changes INTO `emit_let_statement`; accept the unbox removal;
  keep our `emit_let_value` dual-fuel wired to the new `tuple_values.rs`
  helpers.
- Verify: full battery — lib, all HIR suites (full corpus), sweep 131/0,
  release A/B byte-identical, determinism, `daily_use_smoke` 37/37
  (runtime goldens prove the destructure/transport fixes), plus his added
  tests.

## Phase 3 — post-merge state

- INDEX.md + plan docs updated in the same commit as the final merges.
- End state: all 9 PRs closed-as-merged; `origin/main` green on lib,
  HIR suites, sweep, A/B, determinism, runtime goldens.

## Phase 4 — resume H4 on `main`

Per `2026-11-05-h4-completion-loop-convergence-swan-song-emitter.md`:

1. Structural expression families, thin arms first: List/Tuple, then
   Index, Range, Slice, Lambda, Spawn, StructLiteral (each: verbatim
   extraction + dual-fuel + sweep-asserted identity).
2. ArrowAssign register-taking refactor (its dispatch reads value AST
   shapes; needs operand-register split before nativizing).
3. `Cast.via` frontend-typer arc (HIR `MethodCall`/`Reflect` receiver
   typing).
4. H5 retirement once the above shrink the bridge to zero.
