# Compiler completion + verification next leg (2026-11-11)

Status: ACTIVE. Consolidates the remaining H5/HIR cleanup, the verification
program's open phases, and the new test-block follow-ups into one ordered
ledger. Rides on:
`2026-11-09-h5-completion-next-leg.md` (H5 flip + ports),
`2026-11-09-compiler-verification-program.md` (P1–P7),
`2026-11-10-ergonomic-unit-tests.md` (test blocks).

## Decisions recorded

- **Order**: test-block polish → `reclamation` HIR parity → P3 interpreter +
  wasm corpus gates → `batch_shape` → the large ports.
- **Consumerless analyses**: `range` and `dataflow` keep their analysis code
  but are marked **no-runtime-consumer** (their `AnalysisResults` fields stay
  empty); do not spend H5 effort on them. Revisit only when a consumer is
  designed. `destination_ownership` is analysis-only by its own header
  comment — same treatment: explicit defer, not a port target.
- **wasm32 gate stays** (Tier 1 per the stability plan; `tests/test_wasm.rs`
  and the rbv wasm tests already exist).
- **AST arm deletion waits** until the opt-out release window closes; track
  `to_ast::` (41 now) and `hir_emit_enabled()` (28 now) as the debt meters.

## Workstream T — test-block polish (first)

### T1. Domain filtering semantics
- `test_engine::run_tests` distinguishes **no test blocks** (`Ok(None)`) from
  **blocks exist but none match `--domain`** (`Ok(Some(empty))`).
- A no-match domain is a failure with the fix: list available domains. A file
  with no blocks keeps the current “no test blocks” PASS under
  `--tests-only`, and falls through to the program check otherwise.
- `--list-tests` prints available labels per file.

### T2. Test-count reporting
- `TestOutcome` carries `tests_run`/`tests_failed`; the runner summary reports
  tests (not just files): `3/4 tests passed; 1/2 files passed`. Existing
  file-level PASS/FAIL lines stay.

### T3. Docs
- `docs/guides/unit-test-blocks.md`: syntax, domains, isolation, runner flags,
  what is not allowed, IR/no-emission guarantee, future expectations.
- Link from INDEX and `learn-briev` where appropriate.

### T4. Known gap record
- BUGS.md: `:=` reference-implementation bodies typecheck but the interpreter
  executes the (empty) authored body — a pre-existing Rule-5 divergence found
  while testing. Not fixed here; named so it is not forgotten.

### T5. Gates/tests
- `tests/unit_test_blocks.rs`: no-match domain fails with available domains;
  test-count summary; list-tests; existing pass/fail/isolation/IR-equality.
- `scripts/gate.sh quick` already includes the suite; keep it there.

## Workstream R — `reclamation` HIR parity (second, multi-session)

- Read the AST analyzer (`src/analysis/reclamation.rs` + `walk.rs`,
  `socket_effects.rs`, `region_verify.rs`) and the `#79`/`#81` changes; define
  the HIR fact set (points, escapes, barriers, reachability, publish safety,
  redundant keeps, lifetime fallbacks).
- Build `src/hir/reclamation.rs` incrementally: facts first (access paths,
  writes/reads, escapes, sites), then the solver, then the plan result.
- **Parity suite first**: fixture pairs comparing AST vs HIR:
  - txn-local scalar/aggregate allocations; publishes into parameters
    (`publish_into_parameter_leak` fixture); escaped enums/collections; socket
    effects; barrier dedup; keep hints; cycle/reject paths.
- Flip the backend consumer (`analyze_program`: `reclamation`) under
  `BRIEV_HIR_EMIT` with `debug_assert_eq!` over the plan (needs `PartialEq` on
  the plan or a deterministic rendered comparison).
- Gates: lib, corpus sweep, A/B, `publish_reclamation`, memory campaign, Kani
  if the port touches invariants.

## Workstream V — verification remaining

### V1. Interpreter-over-corpus (P3)
- Reuse the `deterministic_transitions` harness style: compile each eligible
  program once, run it natively at `-O0`/`-O3` and through the reference
  interpreter (reactor + callable txns), compare observable results.
- Prefer **value/state comparison** over stdout capture (the interpreter
  prints through `print!`); for node programs compare final state fields and
  exit behaviour.
- Skip list with reasons + coverage ratchet; quick subset per commit.

### V2. wasm32 corpus gate (P3) — BLOCKED 2026-11-11
- Eligible tier1 subset compiled to wasm32, executed with node (or the
  existing wasm test harness), outputs compared with native.
- Explicit skip list for host-only fixtures (sockets, MMIO, process spawn).
  *2026-11-11: blocked.* `.bv` wasm32 emission rejects the stdlib prelude
  (`i64` inttoptr where wasm32 wants `i32`), identical with the AST opt-out —
  a pre-existing target-width bug recorded in BUGS.md. `.rbv` wasm tests pass.
  Gate resumes after the `.bv` prelude width audit.

### V3. P4 seeds/growth
- Feed tier1 sources into the structured fuzzer as seeds; grow generator
  slices (collections → enums/tuples → strings) only when a slice is clean.

### V4. P6 reduction
- `scripts/reduce.sh` + `scripts/interesting.sh` around the existing
  shrink/replay artifacts (cvise preferred, creduce fallback).

### V5. P7 corpus
- Add 4–9 deterministic algorithms (hash map, tree/heap traversal, string
  pipeline) with inline `EXPECT-STDOUT` pins in `tests/directives/`.

### V6. P2 corpus
- Extend `tests/compile_fail/` with ownership/dyn/derivation/diagnostic
  families (required substrings only).

## Workstream H — H5 ports after reclamation

1. `batch_shape` — decide the swan-song consumer type: move
   `AnalysisResults.swan_songs` to HIR (preferred) or keep the `to_ast`
   boundary and port detection over converted AST (document the exception).
2. `accel` → `coalescing` + `image_storage`; `gpu_schedule`; `region`;
   `electronics` (largest; follow the same parity-then-flip recipe).
3. Debt meters: `to_ast::` and `hir_emit_enabled()` may only shrink after the
   opt-out release; AST arms deleted family-by-family.

## Workstream D — ledger reconciliation (same commit as T)

- Verification plan: P5 is DONE (not TODO); P7 count is 6 (sieve added).
- INDEX suite state: lib 3601, daily 41, quick gate 36.
- AGENTS per-commit checklist: add `scripts/gate.sh quick|full`.
- H5 inventory: note no-runtime-consumer decisions for `range`/`dataflow`/
  `destination_ownership`.

## Gates for every commit

`bash scripts/gate.sh quick`; `cargo test --all` before pushes touching
shared analysis; corpus sweep (AST/HIR `.ll`) for emission changes; A/B for
backend-visible changes; determinism check for IR-touching commits; Praetor on
changed dirs; docs in the same commit.

## Execution record

**2026-11-11 — T (test-block polish) DONE.** `test_engine::run_tests` returns
`Ok(None)` for no test blocks vs `Ok(Some(TestRun))` (possibly empty reports)
when a `--domain` filter matches nothing; an unmatched domain fails with the
available domains. `TestOutcome` carries `tests_run`/`tests_failed`, and the
runner summary reports tests (`N/M tests passed; F/G files passed`) instead of
files only. Guide: `docs/guides/unit-test-blocks.md`; INDEX/AGENTS/ledger
updated. BUGS.md records the pre-existing interpreter `:=` derivation gap.
Tests extended: unmatched-domain failure, available-domain listing, and
test-count summary. Gates: unit tests 6; quick gate (pending full run);
diagnostics.

**2026-11-11 — D (ledger reconciliation) DONE.** Verification P5 marked done,
P7 heading says six; INDEX suite counts refreshed; AGENTS per-commit
checklist now names `scripts/gate.sh`; H5 inventory records the
no-runtime-consumer decisions.

**2026-11-11 — V1 (interpreter-over-corpus) DONE ahead of R.** Six curated
programs (`tests/interpreter_corpus_parity.rs`) end with `endprogram <value>`;
the native exit code and the reference interpreter's `ProgramExit` value must
agree. Covers recursion, nested foreach, list read/write, Bool flags, and
matrix ops. Found a real bug: unused Bool plain globals emitted
`global i8 false` (i1 constant) — fixed to `1`/`0` and recorded in BUGS.md.
Added to the quick gate (~16 s).

**2026-11-11 — R started (parity foundation + first fact).**

- `tests/hir_reclamation_parity.rs`: pins that HIR lowering preserves the
  callable/body/global surfaces the reclamation walker reads and that every
  HIR callable body round-trips through `to_ast` structurally unchanged
  (alloc/escape, publish-into-parameter, guards/keep fixtures; generic body
  preserved). This is the gate the port grows against.
- `src/hir/reclamation.rs` (first port slice):
  `closed_initializers` over `HirProgram` with AST-parity unit tests (literal
  globals closed; state and non-literal globals open; no globals closed).
  The test-only AST wrapper lives in `analysis::reclamation`.

**2026-11-11 — R2 (registration facts) DONE for the structural subset.**
`hir::reclamation::registration_facts` mirrors `walk::register` for bodies
(names, params, txn +2 for pre/post), generics (generic defns stay out of
bodies), return types, entry nodes (`is_reactive`), async-unsupported, and
globals/types (lets, `state`, constants). A test-only AST snapshot in
`analysis::reclamation` proves equality on callable and global fixtures
(plain/exported/generic defns, reactive node, async txn; let/state/const;
enum constructors; user-operator types via the new
`HirTypeDef.has_user_operators`; the method alias map, including synthetic
`Owner::member` bodies/return types for explicit receivers). Remaining before
constraints: default-encoding targets (populated during the AST cast walk, so
they move with the constraint port).

**Next R slices (constraints port checklist).** The registration facts are
done; this is the ordered map for `walk::Builder` → HIR:

1. **Graph API to mirror**: `node/root/unknown_value/global/allocation/copy/
   load/constructor_key`, `constraints`, `points/constants/inexact_constants`,
   `memory`, `callables/bodies`, `generics` + `specialize/mangle`, `methods` +
   `method_target`, user-operator/encoding sets, publish fields
   (`incoming_store_owners`, `publish_receiver_param`, `publish_call_args`),
   `unsupported`/`capture`, and the casting lanes
   (`casts.type_to_protocol/find_path/LaneKind`).
2. **Builder methods in order**: `statement` (Let/Assign/Term/Tail/Expression/
   Check/Gate/Guarded/Block/SyncBlock/Mutex/Defer/Foreach/Match/ArrowAssign/
   EndProgram/Rollback/KeepHint), `statement_match/pattern*`, `assign*` +
   `record_incoming_store`, `expression` (all AST variants incl.
   StructLiteral/List/Tuple/Slice/Index/Field/Deref/Cast/MethodCall/Consume/
   AddrOf/IsType/Unary/Binary/Reflect/If/Match/Block/Range), `call/
   method_call/call_nodes/intrinsic_or_unknown/cast/cast_lane/binary/unary/
   generic_result/specialize/free_member_target/method_target`, and
   `value_allocates/alloc_bindings`, `propagate_incoming_store`,
   `record_publish_call`, `projection_root_and_path`, `expression_allocates`.
3. **Parity harness first**: a test-only AST dump of per-owner constraints
   (ordered `Constraint` debug strings) + callable-call sets, compared against
   the HIR walk on fixtures that grow from scalars → allocations → publishes →
   casts → generics → methods. Only then port `solve()`/`plan()` absorption.
4. **Consumer flip** (`analyze_program` reclamation) only when the full plan
   compares equal under `BRIEV_HIR_EMIT`, with the AST plan as the oracle.

**2026-11-11 — R3 (constraints + Graph/solver) LANDED.**
`src/hir/reclamation/graph.rs` mirrors the AST Graph (nodes/points/constants/
memory, callables, constraints, publish fields, casting lanes, specialisation,
solver, plan). `src/hir/reclamation/walk.rs` ports `Builder` (statements,
patterns, assign/projection stores, publish recording/propagation,
alloc_bindings, calls/intrinsics, casts, generics, methods). A test-only AST
constraint dump (`analysis::reclamation::constraints_for_test`) proves ordered
constraint + call-edge equality on 13 fixtures: scalars, allocations,
struct/list/tuple/slice/index/field, Print# capture, branches/matches,
user calls + recursion, generics, explicit/free-receiver methods, blocks,
user operators, nested field stores, publish propagation, casts. Praetor
clean after splitting registration/expression/expression_type helpers.

**2026-11-11 — R4 (plan parity + consumer flip) DONE.**
`hir::reclamation::analyze` mirrors the AST analyze loop (register, per-owner
clone/build/solve/plan, accumulator absorb, safe-publisher reclassification,
entry-region rule, mutating set). Plan parity tests compare full
`ReclamationPlan` equality on scalars/allocations/returns and a combined
publish/method/generic fixture. The backend consumer is flipped under
`BRIEV_HIR_EMIT` with `debug_assert_eq!`; the AST plan remains the opt-out path
and oracle. Gates: reclamation 65; quick 39/39; corpus sweep 143/0 with the
oracle armed; release A/B byte-identical; determinism clean.

**R is complete.** Next: V2 wasm corpus gate, P4 seeds, P6 cvise wrapper, P7
additions; then the large H5 ports.
