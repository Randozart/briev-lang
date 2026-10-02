# Plan: daily-use correctness sweep + GPU session (2026-09-28)

**Date:** 2026-09-28
**Status:** ACTIVE — user-approved 2026-09-28. Umbrella:
`2026-09-28-native-daily-use-gpu-parity-umbrella.md`.
**Ordering (user):** plan file first, then **GPU work** (GPUs free now),
then Phase A daily-use sweep, then Phase C.

## User decisions (2026-09-28)

- **D1 — `SysConf#`: KEEP** (not retire). Zero `.bv` users, but the user
  chose to retain the surface. `SysCall#(nr, …)` stays regardless — it is
  the backbone of no-C-runtime I/O (`cast_lanes.bv` write/exit/read,
  posix wrappers). The "no-arg `SysCall#()`" surface does not exist
  (signature requires `num`).
- **D2 — Stack read semantics**: add `op CopyFrom: peek` to `obj Stack`
  so `<-` reads non-destructively (RingBuffer precedent: it declares both
  `pop` and `read`).
- **D3 — Next focus**: daily-use correctness sweep (the txn
  name-capture bug is silent wrong-code on a common name; must be fixed
  per contract). GPUs free ⇒ GPU session runs first, immediately
  available.
- **D4 — GPU timing**: GPUs are free NOW — run the full Phase 3
  baseline/re-rank session before the daily-use sweep.
- **D5 — `test_collections.bv`**: repair to working API (aspirational
  `new_stack`/`new_queue`/`new_map` → real `= 0`/`= []` + arrows).
- **D6 — dirty benchmark binaries**: leave dirty; commit with next
  benchmark-source change.

## Findings entering this plan

- **Name-capture bug (new, high severity).** A local named `len` in a
  node body hijacks the inlined txn's `len` field reference. Evidence:
  `daily_use_smoke.ll` push uses the string-len register `%t15` as the
  `data[]` index and stores the new len into a dead alloca `%t31`;
  `stack_push_pop.ll` (no local named `len`) has correct field-5 GEP
  load/stores. Silent wrong-code: Stack state corrupt, `v1 <- st` read
  0. Isolation repro is A1 step 1.
- **Sweep-roots correction.** Conformance sweep roots are
  `lib/std, lib/compiler, lib/glue, examples, benchmarks, .smoke`
  (`src/conformance.rs` `active_roots`). `tests/` is NOT swept — the
  umbrella doc's claim that `daily_use_smoke.bv` is "picked up by the
  conformance sweep" is wrong. The gate needs an integration test that
  builds + runs + asserts output (A3).
- **`test_collections.bv`** 6 parse errors are inert (not swept) but the
  file is aspirational (D5).
- **GPU baseline at current tip**: composite decode p50 430.7 µs @4096
  (matches the 2026-09-25 deferred-era numbers — no regression from the
  session's native-side changes). GEMM 4096³ dispatch failed on both
  lanes in the last attempt (device contention / stale binary) — needs
  a clean-GPU re-run (B2).

## Phase B — GPU session (first; GPUs free now)

Per umbrella Phase 3 (`3a`–`3e`).

### B1. 3a Re-baseline (Rule 12) — DONE 2026-09-30
Record: `benchmarks/results/2026-09-30-b1-rebaseline.md` (full runtime
+ optimizer tables; raw logs `/tmp/opencode/b1_runtime.log`,
`b1_optimizer.log`). Sweep's one FAIL (`nbody_newton_accel`) was a
harness telemetry false positive — fixed in `build_and_bench.sh`
(`^# ` stderr filter); post-fix MATCH 11.19x. No Briev-side runtime
regressions vs 2026-09-25; `async_counters_idio` MISMATCH +
`UTF8_ops` SKIP both pre-existing (details in the record). Step 5
landed: followup-stages Stage 1 verdict + Stage 5 corrections.
1. Clean `cargo build --release`.
2. `bash benchmarks/build_and_bench.sh --runtime` (throughput category).
3. `bash benchmarks/build_and_bench.sh --optimizer` (compile-time
   folding category) — run both; record the full table.
4. Record to `benchmarks/results/2026-09-28-<name>.md` with the full
   table (all benchmarks, not just the regressed one).
5. Update the stale `2026-09-24-followup-stages.md` stage 5 — softmax
   retirement (`b1730dd7`, `7ee9597a`) and attention composite path
   (`0051d920`, `dd70f144`) have landed since it was written.

### B2. GEMM 4096³ both lanes — correctness before timing
1. On-device correctness gate FIRST (kernel-index rule): gemm_h_bench at
   4096³, both lanes (SPIR-V + PTX), real shape. Verify the output
   matrix is correct, not just timing.
2. Then timing. Target: 42+ TF (23.4 TF at 4096³ currently; S3b cp.async
   is the biggest prize, 23.4→38+ TF).
3. The `dispatch failed (A)` gap from the last attempt must be cleared —
   if it persists on a clean GPU, diagnose before any timing claim.

### B3. compare_baseline — DONE 2026-09-30
Only `float_math` worsened while the machine was otherwise faster
(Briev 0.0424→0.0458 vs broad improvements): A/B
`compare_baseline.sh float_math` → baseline `5d1d7e45` 0.4150s vs
current `6dca75b0` 0.4116s = 0.9918, within tolerance. No other
Briev-side time regressed. (Protocol as written:)
`bash benchmarks/compare_baseline.sh <name>` for any benchmark that moved
vs the baseline worktree (`../briv-compiler-baseline`, main-only,
untouched). Never excuse a regression as "noise" without this A/B.

### B4. Re-rank 5a/5b/5c/5d + execute top lane
From fresh B1 numbers, re-rank the Stage-5 ladder:
- **5c (3b)** — warp-slice threshold retirement (mechanical):
  `has_warp_slice` span≥512 / span%4==0 heuristics → `config/targets.toml`
  or composite params. Hardware facts (warp=32, `shfl`) stay in backend.
- **5b (3c)** — S3b cp.async GEMM (biggest GPU prize): 4096³ 23.4→38+ TF.
  Gate: on-device correctness both lanes, real shape, before timing.
  4096³ target 42+ TF requires ≤56-reg kernel body (ceiling, not tuning).
- **5a (3d)** — attention 198→125 µs: deferred-2pass → composite parity
  gate; retire remaining fused-attention family matcher entries (~1400
  lines).
- **5d (3e)** — M4 ladder rest (`detect_reduction`→`GemmPlan`, each
  behind perf A/B); M3 producer-consumer chain fusion (3→1 kernel ≤120 µs
  f32).
Each pick behind a Rule 12b pre-B experiment on the ACTUAL generated IR.
Execute the top-ranked lane; A/B before/after; record.

## Phase A — Daily-use correctness sweep

### A1. Name-capture bug (silent wrong-code)
1. **Isolation repro.** Minimal `.bv`: a node with a local named `len`
   + `st <- v` (Stack push) → confirm broken IR (data index = local
   value, len-field store missing). Matrix: which local names capture
   (obj field names: `len`, `data`, `read`, `write`, `count`, `cap`…).
   Confirm the no-local case stays correct.
2. **Root cause.** Trace where txn/op member body identifiers resolve
   when lowered into a node — find where caller-local scope leaks into
   member-body resolution (typechecker binding vs state-machine lowering
   vs emit).
3. **Fix at the resolution layer.** Member bodies resolve in member
   scope — receiver fields — never caller locals. No codegen patch.
4. **Tests.** Behavioral: push/peek/pop read-back correct with colliding
   local names; regression per capture class. `cargo test --lib`.

### D5. `test_collections.bv` repair — **DONE 2026-10-01**

Rewritten to the real API (the old form was aspirational: untyped
generic decls, `new_stack`/`new_map`, `[guard] { }` blocks): typed
generic decls, op Init construction (`= 0`), arrows per the A2 rule
(`<-` push/peek, `~<-` extract). Verified via brievc run: stack-peek-ok,
stack-after-pop-ok, ringbuffer-init-slot-ok, ringbuffer-fifo-ok (the
RingBuffer init semantics — the init value sits at the read cursor —
now documented in the fixture).

### A2. Stack `op CopyFrom: peek` (D2) — **DONE 2026-10-01**

- `lib/std/collections.bv`: `obj Stack` gains `op CopyFrom: peek(#Rh)`
  + `txn peek() -> T [len > 0][len >= 0]` (mirrors pop non-destructive).
- `benchmarks/stack_push_pop.bv`: the discard read becomes `~<- st;`
  (the benchmark IS a push/pop cycle — `~<-` names it honestly).
- Behavioral proof: `tests/tier1/stack_peek_pop.bv` (brievc run):
  peek-ok + pop-then-peek-ok — peek reads 99 without consuming, the
  pop removes it, a second peek reads 42 (impossible if `<-` popped).
- A/B `compare_baseline.sh stack_push_pop`: ratio 1.0092 — neutral
  (the same ExtractFrom member-call shape).
- Sweep: the only Stack `<-` read in the tree was the benchmark;
  daily_use_smoke uses `<-` pushes only; test_collections.bv is the
  D5 repair (separate).

**Design rule (2026-10-01, user decision, documented in
`lib/std/collections.bv` header):** `CopyFrom`/`ExtractFrom` are
op-binding names, never author surface — `<-` IS copy-from (its
default behaviour, via the type's `op CopyFrom` binding), `~<-` IS
extract-from. Every `<-`-supporting collection MUST declare
`op CopyFrom`; declaring only ExtractFrom makes `<-` destructive,
which breaks the arrow's meaning. The fix is stdlib DATA (the
binding), never a new keyword; the internal `txn peek()` is the
binding's implementation target (stdlib-internal, same grammar as
RingBuffer's `read(#Rh)`), not author surface.
1. `lib/std/collections.bv`: add `op CopyFrom: peek(#Rh)` +
   `txn peek() -> T [len > 0][len > 0] { term data[len - 1]; }` to
   `obj Stack` (mirror RingBuffer `read` precedent; check its guards).
2. **Sweep fallback-reliant sites.** Grep `.bv` for discard-form `<-` on
   CopyFrom-less types that now flip to peek. `stack_push_pop.bv`
   (`st <- count; <- st;`) becomes peek-discard (len grows → push guard
   trips) — switch to a pop form. Verify statement `~<- st` / named
   `popped ~<- st` legality on a state-field obj (piggy `~<-` precedent
   exists, but piggy is smash-by-design; check the consume cost on an
   inline-array Stack).
3. **A/B `stack_push_pop` timing** after the semantic change
   (compare_baseline or interleaved reference/experiment). Benchmark
   contract: same output as C, no regression.
4. Verify unchanged: RingBuffer `<-` reads, List arrows, PiggyBank
   sealed `x <- piggy` still errors.

### A3. Executable daily-use gate
1. `tests/daily_use_smoke.rs` integration test — pattern from
   `tests/async_compiled_events_test.rs`: `CARGO_BIN_EXE_brievc` → build
   → harness-exact clang link → run → assert exact stdout.
2. Extend `tests/tier1/daily_use_smoke.bv`: Stack push/peek/pop
   read-back (99/42), env, arithmetic, `when`+`println!`, `sync<g>`,
   string len — assert exact stdout.
3. Fix the umbrella doc claim (sweep roots exclude `tests/`).

### A4. `test_collections.bv` repair (D5)
1. Inventory aspirational API → rewrite against real constructors
   (`= 0` / `= []` + arrows), assert behavior.
2. Probe: run frontend_check over all `tests/*.bv`; few failures → add
   `tests/` to sweep roots (`active_roots`), many → keep roots +
   integration gates. Report the choice.

### A5. Plugin SyncGroup audit (Rule 17)
Grep every plugin's `walk_item` for a SyncGroup arm. 3+ plugins sharing
the pattern → extract a shared `walk_toplevel` helper; migrate
print/env/inline_frgn/script to it.

### A6. Broader daily-use probe (bounded: blockers only)
One `.bv` exercising: defn+generics call, List ops + foreach, HashMap,
string concat/split/compare, file I/O (`std/posix` or ffi), ring/queue.
Compile + run; fix fallout found (blockers only, not feature work).

### A7. Docs
Umbrella corrections: SysConf decision = KEEP (remove "pending
decision" note), sweep-roots fix, A/B outcomes. INDEX if status moves.
Commit per step.

## Phase C — after B (and A)
Phase 2e logo swap → Phase 4 queue (Wave 2b first, per umbrella order).

## Standing gates (every commit)
`cargo test --lib` green · no new warnings · Praetor on changed dirs ·
Kani for safety-critical · GPU: on-device correctness both lanes before
any timing claim · docs in same commit · never `git checkout --` /
`git restore`.

## Commit sequence
B1 baseline → B2 GEMM correctness → B3 compare → B4 re-rank + lane →
A1 fix → A2 stdlib+bench → A3 gate → A4 test repair → A5 audit → A6
probe → A7 docs.
