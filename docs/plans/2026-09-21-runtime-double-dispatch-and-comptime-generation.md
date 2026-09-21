# Runtime double-dispatch fix + comptime generation (language tier-1)

**2026-09-21.** Two plans in one arc: restore honest two-lane gating (A),
then the generation tier of the metaprogramming layer (B). Roadmap
context: `2026-09-21-comptime-fold-expansion.md` (Phase 1 landed);
forward queue: reflection conditions → `$txn` topology templates.

## A — the 2× was never the SPIR-V backend

**Root cause (refutes BUGS.md 2026-09-20):**
`briev_accel_rt.c` (`briev_accel_launch_resident_2d`, ~line 571) "primes"
the lazy Vulkan buffer by calling `briev_accel_launch` — a FULL
seed+dispatch+download — as a buffer-allocation hook. The prime's
download then replaces host `acc` with run-1's accumulated output; the
resident path re-seeds "inputs only" from that already-clobbered host
state (violating its own no-live-value-clobber contract), and dispatch #2
accumulates onto run-1's output: `acc = Σ + Σ` → exactly 2×. Verified:
two dispatches in the verbose log, per-element ratio 2.000000±7e-6 across
all d, SPIR-V k0 disassembly correct instruction-by-instruction.

**Why D=16 passes by accident:** the ONLINE arm's first iteration
multiplies acc by `Exp#(m_ - mn_)` with `m_ = -1e30` → `Exp(-inf) = 0` —
run 2 self-zeros run 1's stale acc. The deferred arm's plain self-add has
no such rescale. Every earlier Vulkan-validated kernel used pure stores
(idempotent under double-fire); the composite's RMW-into-zero-scratch is
the first shape that can observe the bug.

**Fix:** side-effect-free `ensure_buffer` driver op where available;
otherwise snapshot pre-prime host bytes and re-seed from them (the
authored zero-on-entry contract wins). Stop the prime's download from
clobbering host state. In passing: `k0_fields` marks host-only scalars
(`h`) device-written, so end-of-run download overwrites the runner's
fast-forwarded counter — cosmetic, document or fix if metadata is
trivially available.

**Gates:** `softmax_gate.sh` BOTH fixtures BOTH lanes PASS (deferred arm
expected to drop from 0.99 → ~7e-06 on Vulkan); M3 harness both lanes;
`cargo test --lib`; BUGS.md re-narrowed with the mechanism + the
self-healing note. Timing record: composite 1-launch vs 202 µs 3-kernel
chain vs 91 µs flash at gate geometry → `benchmarks/results/`.

## B — comptime generation (the language tier-1 feature)

Phase 1 made composite bodies *select* between futures (match/when
pruning). B adds *generation*: expansion-time unrolling over comptime
ranges and comptime lists, plus expansion-time assertions. All
pre-analysis — expanded output is ordinary statements; every downstream
pass unchanged. Serves all dialects (embedded table synthesis, tile
sweeps, generated shuffle sequences), not only GPU.

### B1 — comptime `foreach` unrolling

Inside a composite body, `foreach k in 0..N` where the range evaluates
comptime (`eval_const` on both ends) → expansion splices the body once
per iteration, binding `k` to the iteration value in the fold env
(arithmetic on `k` folds; conditions on `k` prune). Runtime range →
stays a runtime `foreach` (fail-open, same rule as match). A comptime
range LARGER than the 4096-generation cap also degrades to a runtime
loop — a big range legitimately wants loop semantics, and degradation
gives correct + efficient without forcing the author to wrap anything
(revised from "error": an error would be hostile to the correct
pattern; runaway *statements-per-iteration* is capped by the same
number in practice via nesting depth).

Soundness: the body folds under a CLONE of the env (a runtime loop may
run zero times); the item name is bound per-iteration clone, never
escapes. Generated statements may themselves contain comptime
lets/matches — folded recursively (nesting works by construction).

### B2 — comptime lists

The stage evaluator already has `NavValue::List`. The fold env gains a
`List(Vec<ComptimeVal>)` variant; `foreach x in [a, b, c]` (list literal
or a `$const`/`const` list) splices per element. Element type: comptime
scalars only (fail-closed on structures — the fold never guesses).

### B3 — comptime `check`

`check <comptime-expr>;` inside a composite body: evaluates at
expansion; false → expansion error naming the composite and the
expression (sharp author-facing failure instead of a downstream runtime
gate mystery). Non-foldable → left for the ordinary runtime check path
(fail-open). Reuses `Statement::Check` — no new syntax.

### Tests

Unit (composite.rs): unroll exact count + `1 << k` folding inside;
conditions on the item prune per iteration; runtime range degrades;
list-driven generation (`$const` list); comptime check true passes /
false errors / non-foldable degrades; nested unroll-in-unroll; runaway
cap errors; env isolation (body mutations don't leak between
iterations). Fixture: a composite whose generated structure is provably
unrolled (statement count) and numerically identical to the rolled form
on the existing softmax gate (butterfly-style generation lands with
Front D, not here).

## Non-goals

- Recursive composites (keep the 8-round error; generation replaces the
  use case).
- Statement-typed params, out-bindings (postponed — documented earlier).
- Backend changes: none. Generation happens at expansion; both lanes'
  kernels come out of the same ordinary-body machinery.

## Status: LANDED (2026-09-21, same day)

**A** (`02418f49`): pre-prime snapshot/restore in
`briev_accel_rt.c`. All gates both lanes PASS — deferred arm on Vulkan
0.99 -> 2.06e-05; m3 composite template a_err 2.93e-06 / 8.30e-06.
BUGS.md re-narrowed (both compiler hypotheses withdrawn).

**B** (this commit): comptime generation landed with one design
revision — generation is a PRE-SUBSTITUTION static pass
(`unroll_static`): only the declaration's OWN text generates (literal
ranges, list literals, seed consts); caller spans never do. First
draft unrolled post-substitution ranges, which would have exploded the
softmax composite's caller-span loops (256x128) — the conformance
sweep plus a compile-time look caught it before any gate ran. An
`expr` parameter is a runtime quantity by contract, even when a
particular call passes a literal: that literal is POLICY, not
structure. The fold stays the selection layer (Phase 1); in-place
subexpression folding was added for assign sides and kept loop lists
(unrolled items leave fully-folded arithmetic behind).

Soundness fixes en route (both caught by the conformance sweep):
- stale-outer-env after kept bodies: a runtime loop's clone folds under
  a clone, so its runtime kills never reached the outer env — `s_`
  survived as Int(0) and the in-place fold rewrote the normalize tail
  into acc/0 (a NaN kernel). Rule: after ANY kept (may-run-zero-times)
  body, `env_kill_tree` kills its binders in the outer env (all four
  kept sites: foreach, both match forms, when).
- the unroll item is a VALUE: substituted literally per iteration
  (reuse of the parameter substitution), not merely env-bound.

Tests: 30 composite tests (unroll/prune per iteration, param-range
stays runtime, list literals + const lists, comptime check true/false/
degrade, arithmetic on unrolled items, shift folding). 2323 lib tests
green; gates re-run after the fold changes: all PASS both lanes.
