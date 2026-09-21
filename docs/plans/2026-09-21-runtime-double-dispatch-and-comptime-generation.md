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
stays a runtime `foreach` (fail-open, same rule as match). Loop-count
cap: 4096 iterations at expansion, then error (runaway generation is an
authoring bug, not silently accepted).

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
