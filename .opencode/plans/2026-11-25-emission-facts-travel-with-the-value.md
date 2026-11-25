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

## 7. Outcome (measured 2026-11-25)

| point | wasm32 invalid | native IR | note |
|---|---|---|---|
| baseline `451e0f7a3` | 26 | — | before this plan |
| **W** `c7495fd52` | **23** | identical | Environ#/Argc#/Args# emit Int at Int's width |
| **A** `e089c3d87` | 23 | identical | `WidthChannel` + epoch — inert, as required |
| **B** (this commit) | **22** | identical | `inttoptr_word` ptr-identity + `ClockMonotonicNs#` narrowing |

So 26 → 22, with the epoch infrastructure in place and no regression at any step.

### What stage B learned, and why C is NOT done

Stage B's premise was "make producers record, then switch readers". The first pass
found the class is bigger than the plan assumed, in a specific way worth writing
down: **the backend emits `inttoptr … to ptr` at 71 sites**, and only the two helpers
(`inttoptr_word`, `pointer_from_handle`) record the result. Any other site produces a
pointer the channel does not know about, so the next `inttoptr_word` over it falls
back to `i64` and emits `inttoptr i64 %t0` where `%t0` is a ptr — the
arena_smoke / plain_ptr_smoke / spsc_smoke class.

Two ways to close it, in order of preference:

1. **Harvest the facts from the emission** (recommended). `src/ir_verify.rs` already
   answers "what type is `%x` defined as" (`defined_register`) — the same parser that
   verifies the module can populate the channel. The IR is the single source of truth
   about what was emitted, so the recorder should not be 71 hand-written calls at all;
   it should be one pass over the body that cannot forget. Needs a body-boundary hook
   that has the emitted text (the current `begin_function` does not receive `out`).
2. Route all 71 sites through one `emit_inttoptr(...)` helper. Mechanical, but it is
   the same discipline this plan exists to replace.

**C stays unlanded on purpose.** Switching readers to `value_width_in_current_epoch`
before every producer records turns today's wrong-but-plausible answers into missing
answers, and the earlier measurement (`unicode_search_smoke`) shows that is worse.
The invariant is already pinned by
`a_previous_epoch_record_is_invisible_to_a_current_epoch_read`, which asserts the
migration reader still sees the stale fact — that test is the switch's precondition.

### Files that still need the treatment

`arena_smoke`, `plain_ptr_smoke`, `spsc_smoke`, `arena_scope_reset`,
`arena_scope_reset_rejected` (all one class: unrecorded `inttoptr` results), plus the
`load`/`cast` width programs (`hashmap_*`, `literal_float_coercion`, `float64_precision`,
`test_piggybank`, `arena_folded_loop`, `arena_free_cast`, `state_column_slice`,
`str_open_end_slice`, `slice_basics`, `float16_roundtrip`, `nested_local_dominance`,
`test_pooled_member_foreach`, `ptr_crossdefn_probe`, `spawn_await_segments*`).
