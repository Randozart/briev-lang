# Harvest: the emitted IR is the recorder — plus the nightly repair

**Status:** in progress (owner-confirmed 2026-11-25: "Let's do it", order 1→2→3)
**Builds on:** `docs/plans/2026-11-25-emission-facts-travel-with-the-value.md` (§7)
**Golden Rule:** 26 (emission facts travel with the value)

## 1. Findings this plan is built on

1. **Nightly root cause (established from CI logs, run 45633440352).** The
   `ast-optout-suite` job panics at `tests/saltpetre_gemm_baseline.rs:97`
   spawning `target/release/brievc` — a file the job never builds and never
   fetches. The PR gate runs the same suite green because it unpacks the shared
   compiler via `./.github/actions/use-ci-compiler`. The nightly has been
   structurally incapable of passing since the saltpetre tests landed
   (2026-11-22/23); locally it passes because the worktree convention
   hand-copies the binary. Fix: add the same action to the nightly job.
   `verification-nightly.yml` has `workflow_dispatch`, so the fix is verifiable
   the same day.

2. **Harvest is feasible because readers already hold both sides.** Every
   width-reading helper has signature `(&mut self, out: &mut String, …)` —
   `self.fun` (the channel) and `out` (the emitted text) are distinct borrows.
   `src/ir_verify.rs:1008` already answers "what type is `%x` defined as"
   (`defined_register` + `instruction_result_type`). The IR text is the ground
   truth about what was emitted; the recorder should be one incremental pass
   over that text, not 71 hand-written calls.

3. **Intra-function harvest is the only kind that can work.** Post-function
   harvest arrives too late: the wrong width was already chosen mid-body. But
   definitions precede uses in the text, so harvest-on-read with a persisted
   offset amortizes to O(body length) per function — and the `define`-line
   state machine (the same one `mistyped_operands` uses) clears per-function
   state for free.

## 2. Stages

| # | stage | change | gate |
|---|---|---|---|
| N | nightly | add `use-ci-compiler` to `ast-optout-suite` | dispatch the workflow; job green |
| B′ | harvest | `WidthChannel::harvest_new_definitions(out)`; `inttoptr_word` harvests before choosing the width | wasm32 count drops; zero regressions; goldens unchanged |
| C | flip | readers move to `value_width_in_current_epoch`; delete `counter_scoped_regs`, draining, `note_counter_scoped` | gated on a corpus-wide no-stale-read audit; sweep monotone |
| D | siblings | epoch for `reg_type_cache` / `ssa_old_*` / `phi_*` — only if time permits, one class per commit | byte-identity + full gate |

Order: **N → B′ → C** (D optional). G (gate floor) landed last wave.

## 3. Invariants

| # | invariant | test |
|---|---|---|
| I-1 | A harvested fact equals the type in the emitted text | `harvest_reads_the_emitted_text_not_a_claim` |
| I-2 | Harvest state resets at a `define` boundary — names from a previous function are not harvested into this one | `harvest_resets_at_a_new_function` |
| I-3 | Harvest is incremental: a second call sees only the new text | `harvest_is_incremental_and_does_not_rescan` |
| I-4 | `inttoptr_word` width comes from the harvested truth, not a hand record | `inttoptr_uses_the_harvested_width` |
| I-5 | A hand record cannot override the text (harvest wins) | `the_emitted_text_beats_a_stale_hand_record` |
| I-6 | Native semantics unchanged | full gate + tier1 goldens; any byte-diff investigated individually |
| I-7 | wasm32 invalid-IR count monotone non-increasing | sweep vs `cross-target-baseline.toml` |
| I-8 | After C, no name outlives its body without an epoch check | `no_provenance_set_is_needed_for_lifetime` |
| I-9 | The nightly job has the compiler artifact before running tests | `the_nightly_builds_the_compiler_before_testing` (workflow YAML test) |

## 4. Non-goals

* Not rewriting the 71 sites into helper calls — harvest replaces them.
* Not switching all channel readers in one commit — C moves `inttoptr_word`
  first (the measured consumer), then audits who else reads.
* Not touching call-argument adaptation (S3's measured rejection stands).

## 5. Measurement ledger

| point | wasm32 | nightly | note |
|---|---|---|---|
| `e2d3e3adc` | 22 | red (structurally) | baseline |
| after N | — | green | dispatch-verified |
| after B′ | | | |
| after C | | | |
