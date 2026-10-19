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

## Baseline (Rule 12 equivalent — semantics work)

- 2026-10-20 tree: 3176 lib tests green; harness 79/79 MATCH
  (`benchmarks/results/2026-10-18-fixture1-ws-frame.md`); determinism
  ALL DETERMINISTIC.
- The call-site contract gate is LANDED, STAGED (`4ffa5b8c`): machinery
  complete (callgate prover + registry + facts), policy
  `config/contracts.dbvl` default `allow`. Census at tip: **90 contracted
  fns / 23,117 unproven sites** across the sweep, in three migration
  buckets (below).
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
