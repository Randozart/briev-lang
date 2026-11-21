# Reconcile `feat/evented-net` with the latest remote (2026-11-21)

Context: owner asked to reconcile the evented work with the latest remote.
RESEARCH found that `origin/main` moved to `3db43e4c2` and my branch is ALREADY
based exactly on it (0 behind, 13 ahead). So the reconciliation is not a
pull/rebase — it is a *verification* that my work is still green on the new
gate logic, plus the follow-up that the two new gate commits make actionable.

## What landed on main since I last rebased (already in my base)

- `f819a43b5` **the CLI gate-parity fix = my issue #149, now resolved.**
  `brievc check`/an inspection run now *reports* the gate instead of refusing
  to install it. Previously the CLI was a liar: it never installed
  `config/contracts.dbvl`, so a local `brievc check` passed what the conformance
  sweep (the real deny gate) rejected. That asymmetry cost one 21-minute CI
  round-trip per discovery.
- `3db43e4c2` **two real prover gaps fixed + two loud exemptions.**
  Gaps: `derived_length_facts` (`x.^Length >= 0` now composes arithmetically)
  and tensor `term.n == n`. Exemptions: `exempt: "tests/saltpetre,
  tests/fixtures/load"` in `config/contracts.dbvl`, announced on stderr.
  My fixture is `tests/tier1/evented_smoke.bv` — NOT exempted — so my five
  fact-establishing wrappers still stand under `deny`.

## Invariants

1. **Evented branch stays 0-behind `origin/main` and green on it.**
   Pin: `git rev-list --count HEAD..origin/main` == 0; `scripts/gate.sh quick`
   passes; `tests/tier1/evented_smoke.golden` byte-identical.
2. **The CLI now agrees with the conformance sweep (the #149 payoff).**
   Pin: run `brievc check` (the inspection/CLI path that #149 fixed) on
   `lib/std/net/evented.bv` and the fixture; it must report the gate and
   reach the *same* verdict as the in-suite sweep (both clean for my files).
   Before #149 the CLI reported nothing (a lie); now it must report green.
3. **No behaviour change to the evented module or golden.**
   Pin: `evented_smoke.golden` unchanged. Any simplification of the five
   wrappers (allowed ONLY if main's `derived_length_facts` makes one
   provable) must keep the golden byte-identical.
4. **Scope discipline.** Only the evented branch + its docs are touched in
   this pipeline. The stale `feat/stdlib-checksum` checkout is NOT touched
   (separate branch, out of scope).

## Named tests / checks that prove it correct

- `T1 reconcile`: `git rev-list --count HEAD..origin/main` == 0 and
  `git merge-base --is-ancestor origin/main HEAD` == true.
- `T2 gate-quick`: `scripts/gate.sh quick` → pass.
- `T3 golden`: build `tests/tier1/evented_smoke.bv`, run it,
  `diff <(out tr -d '\n') tests/tier1/evented_smoke.golden` → empty.
- `T4 cli-parity (#149)`: `brievc check <file>` on `lib/std/net/evented.bv`
  and `tests/tier1/evented_smoke.bv` now *announces the gate* and is green —
  the exact asymmetry #149 closed. Record before/after behaviour.
- `T5 wrapper-simplification probe`: try removing `slice_len` (and any
  other wrapper) one at a time; if the file still type-checks under the new
  prover, keep the simplification, else restore. Golden must stay identical.

## Steps

1. `T1` + `T2` + `T3` (certify the branch is current and green).
2. `T4` — exercise the now-fixed CLI gate on my files; document the #149
   payoff (CLI now reports the gate).
3. `T5` — attempt to drop any wrapper main's `derived_length_facts` newly
   makes unnecessary; keep the ones that still are. Re-pin golden only if
   the golden actually changes (it should not).
4. Update the `docs/` gap note: mark #149 as RESOLVED on main (f819a43b5),
   and note whether #151's findings survive the new prover gaps.
5. Commit docs-only changes to the evented branch; push; confirm CI stays
   green.

## Non-goals

- Not touching `feat/stdlib-checksum` (foreign branch).
- Not merging PR #147 (owner's call).
- Not removing the `tests/fixtures/load` / `tests/saltpetre` exemptions
  (main's deliberate, loud, config-declared limits).
