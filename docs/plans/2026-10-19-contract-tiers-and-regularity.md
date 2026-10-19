# Contract tiers (Option 1) + language regularity — contracts as pure static proofs

**Date:** 2026-10-19
**Status:** ACTIVE — owner ruling this session: Option 1.
**Charter:** the owner's design statement — "the compiler should be able to
predict contract violations at compile time; that's the whole point of
Briev's design." This plan makes that literal. Core-language change per the
campaign charter — discussed and ruled before implementation (2026-10-19).

## The ruling (owner, 2026-10-19)

**Option 1 — contracts are pure static proofs.**

| Tier | Meaning | New behavior |
|---|---|---|
| A — PROVEN | precondition discharged at the call site | ok; frontend SKIPS emitting the precondition branch (negative-cost made literal) |
| B — REFUTED | precondition provably false at the call site | **compile error** naming the failing conjunct + the trivial proof |
| C — UNPROVEN | neither provable nor refutable | **compile error**: what couldn't be proven, why, and the fix (supply the guard/fact, or restructure the boundary with `Result`/`Option` — the ws_decode idiom is canonical) |

Tier C is ELIMINATED for contracts: no runtime contract checks, ever. The
`check` STATEMENT remains the one deliberate runtime assertion form, with
defined failure: trap, stderr message naming the contract expression,
**exit code 101** (proposal unobjected). The interpreter already enforces
`check`; the backend's no-op status (BUGS.md finding #1) ends.

No escape flag. Warn-the-default is the anti-pattern; the migration census
is small enough to fix in-campaign.

## Baseline (Rule 12 equivalent — semantics work)

- 2026-10-19 tree at `fad30da8`: 3168 lib tests green; harness 79/79 MATCH
  (results file `benchmarks/results/2026-10-18-fixture1-ws-frame.md`);
  determinism ALL DETERMINISTIC.
- Current contract behavior (verified by probe): `get_u8(b, -5)` and
  `pos(0-3)` with `[x > 0]` compile CLEAN (tiers B and C are unimplemented);
  `emit_precondition_check` branches to `unreachable` (assume/UB) for every
  call site; `check` is discarded by the backend.

## Workstreams

### A — Contract tiers (core change, first)

1. **Typechecker gate**: at every call to a contracted defn/txn, evaluate the
   callee's precondition conjunction against the caller's known facts
   (proof engine). Three outcomes per the ruling. Node preconditions
   (reactive wake conditions) are NOT call-site contracts — the reactor
   machinery is untouched; this gates `defn`/`txn` call sites only. Bodies
   that call their own contracted siblings discharge via the same facts
   guards establish (`when off + 4 <= buf.len { get_u16_be(buf, off + 2) }`
   — the ws_decode pattern, already proven to discharge).
2. **Diagnostics** (house style): what failed, which conjunct, the proof
   where one exists (refuted → "x = 0 - 3 refutes x > 0"), the fix.
3. **Emission**: frontend stops emitting precondition branches for PROVEN
   sites; UNPROVEN/REFUTED no longer compile, so the assume/`unreachable`
   path disappears naturally. `check` statements: backend emits
   `br false → trap block` (fprintf stderr + exit 101); interpreter
   unchanged (already ContractViolation).
4. **Migration census**: the full 30-module sweep + suite + benchmarks are
   the census. Every erroring site: add the establishing guard/contract
   fact, or convert the boundary to `Result`. Fix all in-campaign.
5. **Pins**: tier tests (proven skips emission — IR check; refuted errors;
   unproven errors; `check` traps with 101 in the compiled binary);
   interpreter/backend `check` parity.

### B — `.len` regularity (same resolution paths, ride-along)

1. Gate the compiler-owned hidden `cap`/`len` slots on `coll obj` types from
   user-facing field resolution (`mod.rs` registers them into
   `struct_types`; access goes through `op Count()`/the scaffold only).
2. Retire `len_cap_compare` (emit_expr.rs:68) name-matching of the
   scaffold's own generated guard — explicit marker instead (Rule 23).
3. SPEC: document the length taxonomy (obj field / `op Count` + `Count#` /
   `.^Length` + `CharCount#`) — `.len` is obj-field-only.

### C — obj-globals (finding #3, refined 2026-10-19)

Post-99da6def state: `.len` on a global obj returns **0** (wrong value);
passing a global obj to a defn **segfaults**. Root-cause the plain-let obj
global path (the @global holds a handle; field reads and arg passing
mis-resolve it). Fix + pins (probe3b/probe3c shapes as tests).

### D — interpreter compound-expression defect (BUGS.md 2026-10-18, OPEN)

Compound bitwise-or over non-literal operands yields stale values in
spliced programs (also GrowBuf growth). Root-cause the interpreter's
binary-op operand evaluation; restore the deferred len64 + encode
round-trip parity scenarios as the fix's pins.

### E — Fixture 2: atomic SPSC ring (`std/atomic.bv`)

Per the campaign plan (`2026-10-18-stdlib-fixture-campaign.md`), now with
Option 1 teeth: the ring's power-of-2 capacity contracts are
compile-error-enforced at call sites — the negative-cost axiom tested for
real. First step unchanged: verify async pair scheduling against
`async_counters.bv`. C reference: fair C11-atomics SPSC.

## Sequencing

A → B → C → D → E. A reshapes how fixture code is written (every remaining
fixture's contracts become proofs-or-errors), so it goes first. B shares
A's resolution paths. C and D are independent fixes; E is the payoff.

Per-workstream gates (house standard): `cargo test --lib`, sweep, harness
re-run when codegen is touched, determinism quick gate, Praetor on changed
dirs, BUGS.md entries, discovery-log updates, commits per logical step.
