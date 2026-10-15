# Lifecycle laws + termination-reachability warning

**2026-10-15.** Companion to `2026-10-15-bool-width-canonicalization.md`
(which unblocked `dir.bv` and surfaced the motivating case: a settled
reactor with no terminator idles forever — legal but silent).

## Design (locked with the maintainer)

### Surface — top-level lifecycle laws, when-forms only

```
when bound_reached { endprogram; };   // G ⟹ ended: law + reactor monitor
when serving { reside; };             // G ⟹ resident: compile-time intent
when true { reside; };                // the open-ended server/servo spelling
```

- **Unified with the existing `WhenLaw`** (top.rs:52, 2026-09-22
  when-law slice): position already decides meaning (statement-`when` =
  guarded, top-level = law); lifecycle arms are COMMAND forms inside an
  otherwise-fact law. A lifecycle law carries lifecycle arms only —
  facts stay in their own laws (propagate/verify separation unchanged).
- **No bare keyword**: `resident;` was cut (would duplicate
  `when true { reside; };`); `declare` was evaluated and REJECTED (see
  below).
- `reside;` is a new statement form legal ONLY inside top-level
  lifecycle-law arms — elsewhere a parse error stating the fix.
- `endprogram;` bare (EndProgram(None)) must parse.

### Rejected alternatives (recorded 2026-10-15, do not re-litigate without new facts)

| Alternative | Why rejected |
|---|---|
| `resident;` bare sugar | Duplicates `when true { reside; };`; cuts the condition, which is the feature |
| `declare reside, …;` top-level unifier | `declare reside;` loses to the when-form (no condition); `declare a: Int;` renames `let`; `declare keep x;` duplicates `keep`; `declare seq;` degrades strategy-keyword site precision. The half-baked feeling is the AST, not the surface: `.bv` top-level `let`/`keep`/`free` parse into `TopLevel::Statement(Box<Statement>)` while `TopLevel::StateDecl` already exists and the pipeline consumes it — that is a parser-hygiene fix, queued separately, no new keyword. |
| Runtime settle-exit (exit at empty frontier) | Changes semantics; stalling must stay legal. The warning carries the information instead. |

### Semantics

- **Semantics preserved**: a program with no terminator still runs until
  killed — warned, never rejected. Stalling is legal; unacknowledged
  stalling is what the warning is for.
- `when G { endprogram; }` lowers to a mangled monitor txn
  (precondition `[true]`, body `when G { endprogram; };`) rooted like
  the beginprogram machinery so defn-liveness keeps it. Monitor cost =
  the same class as any reactive txn watching G (documented, not
  hidden). The folder kills the monitor when G is provable.
- `when G { reside; }` emits nothing at runtime (residency IS the
  equilibrium behavior) — compile-time intent only.
- Uniform rule everywhere: embedded authors write
  `when true { reside; };` once. No capability special-casing.

### The analysis pass (`src/analysis/termination.rs`, first-class)

- Roots: beginprogram nodes, bootstrap node, machine-vectored ISRs,
  lifecycle monitor txns.
- MAY-reachability (sound in the silent direction) over the existing
  callee graph: from roots through node/txn/defn bodies, calls, spawns.
- Terminators: `endprogram`, `halt`, `trap` (all three stop the
  process; trap stops it loudly). Silent iff any terminator is
  reachable, or any lifecycle law has a reside arm (unsatisfiable
  guards already error in the law pass — suppression inherits that
  proof, no syntax-only suppression).
- Otherwise: warning in house style — what (runs until the process is
  killed), proof (the entry→live walk found no terminator), fix
  (`when <done> { endprogram; };` or `when true { reside; };`).

### Wiring

- Parser: lifecycle arms in `WhenLawDecl`; `reside` statement; bare
  `endprogram;`.
- AST: WhenLawDecl carries lifecycle arms; `Statement::Reside`.
- Vocab master (`vocab.rs:536` test), TextMate grammar regen
  (`brievc grammar`), SPEC.md, tutorial, execution-model doc
  (equilibrium section gains the lifecycle-law refinement).
- BUGS.md: the dir_probe3 hang (settled reactor, no terminator, libc-free
  `_start` has no exit) cross-referenced here.

### Tests

- Parser: lifecycle laws parse; bare `reside;` outside a top-level law
  errors with the fix; fact-laws unchanged.
- Analysis: zombie warns; endprogram/halt/trap each silence;
  `when G { endprogram; };` silences; `when G { reside; };` suppresses;
  a terminator inside a dead (unrooted) node does not silence.
- E2E: the dir.bv driver shape warns at build; the
  `when true { reside; };` variant builds silent; a
  `when n == 1 { endprogram; };` variant links, runs, exits.
- Interpreter first (reference): reside + lifecycle laws in eval before
  codegen (Rule 5).

### Sequencing

1. Bool-width + dir.bv land FIRST (separate commits; the dir smoke
   driver uses a plain `endprogram` statement — exists today).
2. This feature: parser/AST → lowering → analysis → diagnostic →
   tests → docs, one commit per coherent unit.

### Status

- [x] Plan written (this file)
- [x] Bool-width + dir.bv landed (007290bf..c3cba4a6)
- [x] Parser/AST: lifecycle arms + reside (WhenLawDecl.lifecycle,
      LifeCycleArm; reside parses as a bare identifier — no new token)
- [x] Lowering: monitor txn — injected AFTER typecheck in all three
      pipeline surfaces (compile.rs compile_source, pipeline.rs
      compile_to_typed + check_source_for). The typechecker never sees the
      monitor: a synthesized txn would trip the authored-txn contract
      requirement (explicit, non-tautological — the [true][true] catch-22);
      the monitor reuses the law's proof-checked guard verbatim.
- [x] termination.rs analysis + backend warning (consumes the ONE
      defn-liveness live set; ISR handlers scanned unconditionally)
- [x] defn-liveness row: Statement::EndProgram(None) roots exit_prog (the
      monitor's bare form had no row — IR-scan net rejected the program)
- [x] Interpreter reference: deferred — the bare-term divergence is parked
      with docs/plans/2026-10-15-term-tails-proposal.md (Proposal 1); the
      lifecycle work does not touch term semantics
- [x] Tests green (3098/0; lifecycle::tests ×3; dir/fs/daily smokes green)
      + docs wired (SPEC §9.4.3, execution-model doc)

Verified end to end (target/probe/lc1–lc4): zombie warns; reside
suppresses; endprogram-law silences + emits `__briev_lifecycle_0`; a
never-settling node ([true] precondition) exits 0 through the law's
endprogram — the monitor runtime-fires.


## Phase 4 close-out (2026-10-15, same session)

- `lib/std/os/temp.bv` — `mktemp_file` (O_CREAT|O_EXCL + rand + retry,
  probe-verified then committed `ddc515c4`).
- 16 os stubs re-headed honestly (junk const spam removed; each states
  what filling it needs + the undo path) — `4f73f70e`.
- `tests/mini_cli.rs` — the daily-driver golden: args + fs round trip +
  mktemp/remove + dir count + wall clock, build/link/run at
  -O3 -flto through the harness flags.

Remaining (parked, needs the maintainer rested): the term/tails language
proposals (docs/plans/2026-10-15-term-tails-proposal.md) and the
interpreter-side bare-term divergence. Everything decided is landed and
pushed.
