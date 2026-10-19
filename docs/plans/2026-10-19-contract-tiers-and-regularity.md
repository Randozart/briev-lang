# Contract tiers (Option 1) + language regularity — contracts as pure static proofs

**Date:** 2026-10-19 (comprehensively updated 2026-10-20 after the design
review — see "Rulings, 2026-10-20").
**Status:** ACTIVE.
**Charter:** the owner's design statement — "the compiler should be able to
predict contract violations at compile time; that's the whole point of
Briev's design." This plan makes that literal. Core-language changes are
discussed and ruled before implementation.

## Rulings (owner)

**2026-10-19 — Option 1: contracts are pure static proofs.**

| Tier | Meaning | Behavior |
|---|---|---|
| A — PROVEN | precondition discharged at the call site | ok; no contract code exists to emit (see Emission) |
| B — REFUTED | provably false at the call site | **compile error** naming the failing conjunct + the trivial proof |
| C — UNPROVEN | neither provable nor refutable | **compile error**: what couldn't be proven, why, the fix (establish the guard, or restructure the dynamic boundary with `Result`/`when` — the ws_decode idiom) |

Tier C is eliminated: no runtime contract checks, ever. No escape flag at
the call site; the compiler's own rollout stage (`config/contracts.dbvl`,
IrrPolicy precedent) stages the migration with recorded flip criteria.

**2026-10-20 — `check` is a proof dialogue, static-only (ruling ii).**
`check X` is the programmer asking the compiler: "given everything
established at this point, X holds — correct?" Three answers:

- **Yes, proven** — the fact is recorded (strengthens downstream proofs);
  nothing is emitted; the statement is fully consumed at compile time. Its
  source residue documents intent for human readers.
- **No, refuted** — compile error with the trivial proof.
- **I cannot tell** — compile error: establish X with a guard, or the
  condition is genuinely dynamic → `Result`/`when` is the idiom, not an
  assertion.

Consequences: the backend `check` no-op is **correct by ruling** (BUGS.md
finding #1 closes as by-design); the interpreter's runtime `check`
enforcement RETIRES (both engines agree — a static-only check has no
runtime meaning); the language has **zero runtime assertion forms**.

**2026-10-20 — obj-globals resolve through plain/state, inferred by
default.** Not rejected, not implicitly half-supported:

- The existing `plain let` / `state let` syntax (SPEC §899-924) is the
  designed vehicle. **Inference is the default**: unmarked top-level lets
  are classified by usage (SPEC §920 stands — no new requirement).
- Explicit `state let` / `plain let` assertions stay optional — the
  programmer's clarity and the compiler's assistance. The assistance is
  real: **assertion-vs-inference mismatch is a compile error** (verify the
  check exists; add if missing).
- Boxed (obj-typed) top-level bindings need their category's path completed:
  `plain` obj = the @global holds the i64 HANDLE (field reads = load →
  inttoptr → GEP box; arg passing = handle passing — completing the
  half-built path behind `.len`-returns-0 and the arg-passing segfault);
  `state` obj = a %State handle column (mirroring the working Ptr state
  columns).

**2026-10-20 — assume-lowering reclassified.** Where the frontend has
PROVEN a fact, lowering to `assume` is a feature (an optimization hint the
backend consumes), not a bug. The flip sweep's job is to CLASSIFY:
proven-facts → keep as assume; unchecked-dynamic → eliminate.

**2026-10-20 — GrowBuf interpreter misfire:** a true bug (wrong observable
`len`), with fix-path representational freedom: debug the executed Briev
scaffold in the VirtualHeap, or implement the interpreter's GrowBuf
natively (Vec-backed; `cap` is compiler-owned, and after workstream B's
hidden-slot gating `len` is the only observable). Decide at implementation
with the minimal repro in hand.

## Session log — PR interlude (hiatus), resumed 2026-10-20

Work on this plan paused for a PR interlude: reviewing and merging Otto's
open PRs (#25 lexer `\r`, #23 const String global, #24 dot-call `%state`,
#27 `Malloc#` raw-memory attribute, #28 Float tuple unboxing) and
reconciling #26 into the unified `lib/std/net/websocket.bv` (closed as
superseded; full record in `docs/plans/2026-10-20-pr-review-net-stack.md`).
The interlude fixed several bugs that intersect this plan's paths —
notably the TRANSITIVE raw-memory classification (a caller's memory
attribute must be at least as permissive as its callees' effects) and the
trait-declaration splice retention — and landed four net modules that grew
the contract census (see below).

**Resumed order (owner-approved):**

1. **Refresh the census** under `policy: warn` and update the ledger
   (the 90 fns / 23,117 sites figure predates the interlude).
2. **Workstream C — obj-globals** via plain/state (inference default,
   assertions optional, mismatch = compile error; complete the plain-obj
   handle path and the state-obj column). Live defects: a global obj's
   `.len` reads 0, and passing a global obj to a defn segfaults.
   **DONE 2026-10-20** (symptoms): the existing OBJECT-INSTANCE-POOL
   representation was fixed rather than replaced — (a) non-literal
   initializers are now SEEDED into the binding's row, (b) each static
   instance owns a distinct row (was shared row "0"), (c) explicit-receiver
   member calls on pooled receivers box from the row instead of panicking.
   Pins: `obj_globals` golden. Placement assistance verified (plain on a
   compound initializer, and state the machine never references, both
   error with actionable text). DEVIATION FROM THE RULING'S SKETCH: the
   implementation keeps pool columns rather than switching obj bindings to
   a single @global/%State HANDLE slot; plain-obj handles remain disallowed
   by the placement policy. Recorded for the owner — the ruly intent
   (correct global obj bindings) is met; the representation differs.
3. **Interpreter parity trio** — DONE 2026-10-20 (with residuals):
   callable txns already had a convergence loop; the real blockers were
   (a) the casting graph's String-target lanes were unhandled (`65 as Char
   as String` yielded the Char atom, CharCount# 0), (b) the Blob→String door
   was unhandled, (c) obj literals produced anonymous Products (type name
   discarded) so member dot-calls fell through to Void, (d) trait DEFAULTS
   were never registered per conforming type (`put_u16_be` undefined), and
   (e) module CONSTANTS never resolved. All fixed; plus the encoder's
   per-byte recursion became an iterative foreach (stack depth no longer
   proportional to payload). The encode round-trip scenario is RESTORED in
   the codec parity test (7/16-bit forms). Residuals OPEN (BUGS.md):
   ws_encode's 64-bit payload in the reactor path, and `String as Data as
   Int` remains unrepresentable (Strings are Value::Bits with no address).
   Also fixed while here: ws_encode never wrote the MASKING KEY on masked
   frames (a real stdlib wire-format bug found by the restored round-trip).
4. **The migration arc** (buckets 1→3→2, assume sweep, flip to `deny`),
   then B (`.len` regularity), then fixtures 2–4.

## Baseline (Rule 12 equivalent — semantics work)

- 2026-10-20 (post-interlude) tree: 3188 lib tests green; harness 79/79
  MATCH
  (`benchmarks/results/2026-10-18-fixture1-ws-frame.md`); determinism
  ALL DETERMINISTIC.
- The call-site contract gate is LANDED, STAGED (`4ffa5b8c`): machinery
  complete (callgate prover + registry + facts), policy
  `config/contracts.dbvl` default `allow`. Census at tip: **90 contracted
  fns / 23,117 unproven sites** across the sweep, in three migration
  buckets (below).
  **Census increment (2026-10-20, PR #26 landing).** `lib/std/net/sha1.bv`
  (9 contract-bearing defns), `http.bv` (8) and `base64.bv` (4) joined the
  sweep — ~21 new sites, overwhelmingly the bucket-1 noise class
  (`[x >= 0]`, `[term >= 0]`, `[term >= 0 && term <= MASK32]`). The landed
  `websocket.bv` carries only the proven core's preconditions (2), because
  its original helper contracts were replaced by the bounds-proven core.
  The 90/23,117 figure was STALE; refreshed 2026-10-20 under
  `policy: warn` (see "Census refresh" below).
- The interpreter compound-expression defect is ROOT-CAUSED AND FIXED
  (`59b2b1e5` — B1 string-deref misfire; bitwise ops disambiguate by
  representation); len64 parity restored. Residual: the encode path's
  GrowBuf growth misfire (OPEN, own repro needed).

## Migration census — three buckets

1. **Noise conjuncts** (mechanical): `[s.^Length >= 0]`, cast-chain
   (`as Data as Int >= 0`), intrinsic-sign restatements (`Args#() >= 0`) —
   restatements of non-negativity by construction; remove, the callee gains
   nothing.
2. **Dynamic boundaries → Result** (per-site judgment): genuinely
   input-derived conditions behind a contract — convert to the ws_decode
   idiom.
3. **Domain/termination contracts** (the delicate core): `[n >= 0 && n <= 20]`
   IS zeros' recursion proof; callers must carry the proof-flow — the
   float/string formatting internals (float_fmt.bv, cast_lanes.bv).

## Census refresh (2026-10-20, after the PR interlude)

Measured with `policy: warn` over the conformance sweep:

- **79,415 unproven announcements, 0 refuted, 144 unique contracted fns.**
  Announcements duplicate per consumer (the sweep re-checks spliced imports),
  so the unique-fn count is the migration surface; the announcement count is
  a churn metric.
- Bucket split over the 144: **65 noise-only** preconditions (every conjunct
  a non-negativity/`[true]` restatement), **63 bounds/domain** (real
  `<`/`<=`/masks/`!=`, incl. the float/string formatting core), **16
  unresolved** call names (lambdas/parameters/foreign wrappers —
  classification noise, to triage).
- **Finding fixed during the census:** the prover's refutation was unsound
  (`fact_refutes` ignored atom identity; `fact_entails` had false arms). It
  had reported 5,122 REFUTED — trivially-true conjuncts like `2 >= 2` and
  `(b0 & 31) >= 0` — which would have rejected valid programs at the `deny`
  flip. Fixed with an exact entailment table + atom checks + equality
  twin matching; BUGS.md 2026-10-20. The census above is therefore the first
  trustworthy one.
- Interpreter-side check statements: none in the corpus (the `check` gate's
  deny flip is about user programs, not stdlib).

**Bucket 1 migrated (2026-10-20).** All 65 noise-only preconditions (pure
non-negativity restatements) plus the chained callees they fed were
neutralized: a `[noise]` pre became `[true]` where a meaningful post exists,
or the whole contract was omitted when the post was also `true` (the
tautology rule rejects `[true][true]`). Verified by iterating the warn
census three times: each round showed a handful of callees whose body used
the removed assumption (`sha1_build_padded`, `when_hi`, `task_done`,
`task_result`, `read64le`, `pf_exp_digits`, `sha1_padded_len`); migrating
those too left ZERO new warnings.

Result: **79,415 → 39,865 announcements (−50%)**, unique contracted fns
**144 → 85**. Remaining work is the two substantive buckets (3: dynamic
boundaries → Result; 2: float/string domain/termination proof-flow).

## Workstreams

### A — Contract gate: LANDED, STAGED; migration next

Infrastructure complete (`4ffa5b8c`: callgate prover + fn_contracts
registry + known_facts + the infer_call gate). Remaining: the bucket
migration (1 → 3 → 2), the `check` gate (below) as a flip prerequisite,
the assume-classification sweep, then `policy: deny` flips when sweep +
suite + harness are green under `warn` with zero announcements.

### A' — `check` static-only gate (ruling ii) — FIRST (self-contained)

1. Typechecker `Check` arm: prove-or-refute the condition against
   `known_facts` (reuses the callgate prover — no new machinery). PROVEN →
   facts recorded (already the behavior); REFUTED / UNPROVEN → compile
   errors with the house diagnostics. Staged under the same
   `contracts.dbvl` policy (allow/warn/deny).
2. Backend `check` no-op: annotate as by-design (ruling ii); BUGS.md
   finding #1 closes.
3. Interpreter: retire runtime `check` enforcement — the typechecker has
   fully consumed the statement; both engines agree it has no runtime
   meaning.
4. Pins: tier tests (proven records + discharges downstream; refuted
   errors; unproven errors); policy-staged behavior.

### B — `.len` regularity

1. Gate the compiler-owned hidden `cap`/`len` slots on `coll obj` types
   from user-facing field resolution (mod.rs registers them into
   struct_types; access goes through `op Count()`/the scaffold only).
2. Retire `len_cap_compare` (emit_expr.rs:68) name-matching of the
   scaffold's own generated guard — explicit marker instead (Rule 23).
3. SPEC: document the length taxonomy (obj field / `op Count` + `Count#` /
   `.^Length` + `CharCount#`) — `.len` is obj-field-only.

### C — obj-globals via plain/state (ruling: inferred, assertions optional)

1. Verify/add the assertion-vs-inference mismatch check (the assistance).
2. Complete the plain-obj handle path (@global i64 handle → inttoptr → GEP
   box for field reads; handle passing for args).
3. State-obj = %State handle column (mirror Ptr state columns).
4. Pins: probe3b (`.len` on a classified plain obj global) and probe3c
   (global obj as call arg) shapes as regression tests.

### D — GrowBuf interpreter misfire

Minimal repro (grow_new(8) + N puts, inspect len/cap per put) → decide
native-interpreter vs scaffold-execution fix → restore the round-trip
parity scenario (bit 5 in `tests/ws_frame_codec_parity.rs`).

### E — Fixture 2: atomic SPSC ring (`std/atomic.bv`)

Per `2026-10-18-stdlib-fixture-campaign.md`, into the fully-flipped
language: power-of-2 capacity contracts compile-error-enforced at call
sites; async pair scheduling verified against `async_counters.bv`; C
reference = fair C11-atomics SPSC.

## Sequencing

A' (`check` gate) → C (obj-globals) → A-migration (buckets 1 → 3 → 2 +
assume sweep + flip) → D → B → E. Rationale: A' is small and self-
contained and is a flip prerequisite; C closes real wrong-value/segfault
bugs; the migration is the long arc; D and B are independent; E lands last
into the contract-true language.

Per-workstream gates (house standard): `cargo test --lib`, sweep, harness
re-run when codegen is touched, determinism quick gate, Praetor on changed
dirs, BUGS.md entries, discovery-log updates, commits per logical step.
