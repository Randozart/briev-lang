# Emission facts travel with the value — the epoch plan

**Status:** in progress (owner-confirmed 2026-11-25)
**Rule this implements:** AGENTS.md Golden Rule 26
**Predecessors:** `docs/plans/2026-11-25-wasm32-debt-27-to-zero.md` (27 → 26),
`docs/plans/2026-11-25-ptr-emission-debt-proposal.md`

## 1. The finding that shapes the plan

`FunctionContext` is **one long-lived struct**, built once and reused for every
function; `begin_function()` (`context.rs:1646`) resets three of its ~14
emission-time maps *in place*. Meanwhile `begin_function` sets
`txn_counter = 0`, so **register names repeat identically in every body** — while
the field's own comment claims the counters are "NEVER rewound". The comment is
stale, and the reset is the root cause:

> with names repeating and the map outliving the body, any producer that forgets
> to record leaves the reader looking at a **foreign function's** fact — silently,
> and with a plausible-looking width.

That is what wave-11 spent a day chasing: `%t115` recorded `i32` and emitted
`i64`; `%ac3` recorded `i32` and emitted `i64`. The channel has 11 writers
against 12 readers.

It is therefore **not** a discipline problem ("someone forgot to reset"), and the
fix is not a better reset. It is that a name is not a value: two different values
in two bodies share the key. `counter_scoped_regs` (added in S2) was provenance
invented to give a map a lifetime — the tell named in Rule 26.

## 2. Why not a nested `BodyState` struct

Moving ~14 fields into `BodyState` rewrites every `self.fun.<field>` access in the
backend — thousands of lines — and the only property it buys over the mechanism
below is name-level tidiness. The property that matters ("a fact does not outlive
the body that produced it") is achievable with **one** struct field and **no** use-site
churn, and it is enforceable at the reader instead of by convention at the writer.

So: **an epoch stamp**. Every body gets a monotonically increasing epoch;
every record carries the epoch that produced it; a reader can ask for
current-epoch-only. `begin_function` already is the one place every body passes
through (5 call sites, all body starts), so the epoch cannot be forgotten.

The migration is ordered by measurement, not by taste:

* **A** makes the lifetime *representable* — records carry an epoch, nobody filters yet. Inert.
* **B** makes producers record what they emitted, so current-epoch lookups succeed.
* **C** flips readers to current-epoch-only and deletes the provenance set. Safe only after B.

Each step is separately verifiable, and C is a one-line switch that can be flipped
only when B is measurably complete.

## 3. Stages

| # | stage | change | gate |
|---|---|---|---|
| W | warm-up | `briev_getenv_int_impl` emitter truncates arg 1 and not arg 2 (5-6 programs) | wasm32 count drops, 0 regressions |
| A | epoch | `body_epoch` on `FunctionContext`, bumped only by `begin_function`; records carry it; readers may filter. Fix the stale "NEVER rewound" comment | native IR byte-identical; full gate |
| B | producers | every *defining* instruction records its emitted width; detector = `ir_verify` + the cross-target sweep | wasm32 count drops; 0 regressions |
| C | flip | readers ask current-epoch-only; delete `counter_scoped_regs` + draining + `note_counter_scoped` | wasm32 ≤ previous; the three provenance tests replaced |
| D | siblings | apply the same epoch to `reg_type_cache`, `reg_float_cache`, `ssa_old_*`, `phi_*` — one class per commit | byte-identical + full gate per commit |
| G | gate floor | sweeps assert a positive floor; fail at zero | a deliberately empty artifact dir must fail |

Order: **W → A → B → C → (D, G)**.

## 4. Invariants

Each invariant names the test that pins it. Tests live in
`src/backend/llvm/tests.rs` (unit tests have access to `FunctionContext`
internals; integration tests do not).

| # | invariant | test |
|---|---|---|
| I-1 | A recorded width carries the epoch of the body that recorded it | `a_record_carries_the_epoch_of_its_body` |
| I-2 | Only `begin_function` starts an epoch; no other path bumps it | `only_begin_function_starts_an_epoch` |
| I-3 | The epoch never decreases across bodies | `the_epoch_never_regresses_across_bodies` |
| I-4 | A previous-epoch record is invisible to a current-epoch read | `a_previous_epoch_record_is_invisible_to_a_current_epoch_read` |
| I-5 | A missing record falls back deterministically and does not invent a width | `a_fallback_is_used_when_the_producer_never_recorded` |
| I-6 | Any stage that does not intend to change emission leaves native IR **byte-identical** | `/tmp/ir_*` corpus diff (12 fixtures), run per stage |
| I-7 | The wasm32 invalid-IR count is monotone non-increasing | `scripts/cross_target_ir.sh` vs `config/cross-target-baseline.toml` |
| I-8 | After C, no per-name provenance set exists, and lifetime is enforced at read | `no_provenance_set_is_needed_for_lifetime` |
| I-9 | A gate that inspects nothing fails | `the_sweep_fails_when_it_inspects_nothing` |
| I-10 | `txn_counter`'s documentation matches its behaviour | `the_counter_documentation_matches_its_behaviour` |

## 5. Non-goals

* Not a site-by-site wasm32 grind (measured at +1 program per day).
* Not blanket integer argument adaptation (measured worse twice: +4 and +1).
* Not a rewrite of the ~60 program-lifetime declaration maps — they are populated
  from the AST, have program lifetime, and are not name-keyed emission facts.

## 6. Measurement ledger

| point | wasm32 invalid | native IR | note |
|---|---|---|---|
| after wave-2 `451e0f7a3` | 26 | identical | baseline |
| after W | | | |
| after A | | | inert, must be identical |
| after B | | | |
| after C | | | |
