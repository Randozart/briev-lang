# Thread lane + repo state — open items for decision

**2026-11-16.** Status: **decision list — nothing here is started unless marked
DONE.** Captured at the end of the session that landed the threaded dispatch
lane, so the open questions survive the session.

**Landed (for context):** `1b13bc45b` (interprocedural resource effects + thread
gate rows), `5d5703cad` (threaded dispatch: runtime worker pool, per-worker
ticks, threaded main, emission hold retired), `3ea341d66` (observable =
leaves-the-process; unknown-callee/`Cast.*` rules; arena retirement gate).

Companions: `2026-11-15-thread-assignment.md`,
`2026-11-16-per-worker-arena-and-resource-effects.md`.

---

## A. Correctness gaps in the landed threaded lane

### A1. `emit_main` still prefers the async-phase arm — real bug

```rust
if self.has_async_txns && !self.is_lightweight_async {
    self.emit_async_phase(out, "%state");        // never touches the barriers
} else {
    if threaded { release }
    call @reactor_tick
    if threaded { wait }
}
```

`emit_async_phase` calls `@async_body_<name>` on the main thread and never
releases the workers. So for a **pinned program with non-lightweight async
nodes** (`strategy.rs` marks lightweight only for pure bodies with a
non-constant `bounded_pre`):

- every worker blocks at the release barrier,
- worker N's tick never runs, so its nodes never fire,
- the program cannot converge.

The landed fixture is lightweight-async, which is why `tests/thread_pin.rs`
passes.

**Fix:** make the threaded arm authoritative — one `if threaded { release; tick;
wait } else if has_async_txns && !lightweight { async_phase } else { tick }`.
**Test:** a fixture with a non-lightweight async pinned node (e.g. guarded by a
boolean state flag rather than a counter bound) plus a run assertion.

**Decision needed:** none — this is a straight bug. Worth doing first.

### A2. Per-worker arenas (Part A) — blocked on one pass

Attempted and reverted. Injecting `__arena_ptr_w<N>`/`__arena_end_w<N>`/
`__arena_base_w<N>` plus one reservation sliced by main produced an invalid
module: the emitted `%State` type had only the user fields while the emitters
referenced the injected indices.

**Root cause:** `ctx.field_types` is rebuilt by the FieldMode promotion pass
(`src/backend/llvm/mod.rs`, the `FieldMode::Always | LazyCached` arm that
re-pushes `old_types`), which drops slots the arena injection appended while
`field_index_map` keeps them.

**Options:**
1. Inject arena slots *before* the promotion pass and make the pass carry them.
2. Teach the promotion pass about the arena slots explicitly.
3. Keep a single arena but serialise allocating workers (`sync<group>`-like) —
   rejected: serialising defeats the purpose and adds a runtime lock.

**Current state:** the both-allocate refusal stands, pinned by
`tests/compile_fail/thread_pin_alloc_shared_arena.bv` as a retirement gate.
**Decision needed:** option 1 vs 2 (affects how other injected fields are
handled later).

### A3. Part B / D.2 — same prerequisite as A2

- `current_worker` plumbing was part of the reverted Part A attempt.
- The cross-worker write-mask debug assertion needs injected slots to be visible
  in the write masks, i.e. A2 first.

**Decision needed:** whether the debug assertion is worth having at all once
A2 makes the arena partition structural.

### A4. Unowned coverage gaps in the threaded lane

Not yet tested at all:

- `has_wake_triggers` + pinned (the `@llvm.wake.any()` convergence path with a
  worker pool running).
- A pinned program that hits `.end` from the entry exit-check before any tick
  (pool started, zero ticks, then `briev_workers_stop`).
- `endprogram` fired from a non-zero worker (process exit from a pool thread).
- `MAX_THREADS` (63) at scale: 63 barrier participants and a 63-entry table.

**Decision needed:** which of these deserve fixtures now vs when A1/A2 land.

---

## B. Repository state that needs a human decision

### B1. Open PR #101 — `fix/compiler-correctness`

"Make compiler correctness gates complete and independently observable", open
since 2026-11-15. **Decision needed:** merge, re-scope, or close.

### B1-result. PR #101 landed

`fix/compiler-correctness` was green on its own head but CONFLICTING with main
in two files. Resolved and landed as `a981ae79a`:
`.github/workflows/ci.yml` (kept the PR's independent `counsel` job) and
`BUGS.md` (union of both sides' 2026-11-15 entries). Post-merge `cargo build
--lib` clean and `bash scripts/gate.sh quick` → `gate: pass`.

### B2-triage. Branch inventory (2026-11-16)

Measured with `git cherry main <branch>` (commits whose patch is NOT in main).
Buckets, so the decision below is per bucket rather than per branch:

**Already in main (content-wise merged) — nothing to do:**
`val92`, `val93`, `main-prep`, `fix/wasm-tuple-abi`, `feat/tail-loop-lowering`,
`feat/counsel-contracts`, `experiment/task-board-structured`,
`experiment/struct-collection-each`, `electronics-hir-port2`,
`integrate/pr31`, `integrate/pr32`.

The last two were verified by hand, not just by `git cherry`: main already has
`src/typechecker/call_literals.rs` (200 lines, superset of the branch's 152 —
merging would regress) and `src/backend/llvm/match_termination_tests.rs`.
Their features arrived through different commits, which is why the patch-ids
differ. `fix/aggregate-arg-handle-abi` is 551 files *behind* main; merging the
branch would revert main, so its single fix must be cherry-picked if wanted.

**Intentionally abandoned / experimental — do not merge:**
`feat/canonical-effect-abi` (head commit: "abandon canonical EffectCtx branch
with findings"), `experiment/effect-state-carry`, `experiment/nested-sret`,
`experiment/nested-enum-b3`, `experiment/nested-b3-hoist`,
`feat/ptr-provenance-fence-retirement`, `recovery-branch` (July, superseded).

**Live lanes referenced by INDEX.md — do not touch from main-side:**
`feat/e14a-intent-synthesis`, `feat/bad-dialect`.

**Needs an owner decision (real, recent, unmerged):**
| Branch | Unmerged / total | Last commit |
|---|---|---|
| `feat/counsel-lab` | 51 / 51 | 2026-11-14 counsel campaign |
| `intel-xpu-harvest` | 10 / 13 | 2026-11-16 GPU grind waves 0–2 |
| `feat/verify-mask-offset-kernel` | 8 / 9 | 2026-11-16 CI position for PR #104 |
| `feat/counsel-expand` | 4 / 5 | 2026-11-16 counsel Phase 2–3 checklist |
| `h5-phase-c4` | 6 / 6 | 2026-11-13 HIR phase C4 final audit |
| `feat/counsel-merge-test` | 6 / 7 | PR #104's own commit (already on main as `e215d341a`) |

`feat/counsel-merge-test` looks superseded by #104 and can likely be deleted;
`feat/verify-mask-offset-kernel` may be the same story. `h5-phase-c4` is the
most likely candidate for a genuine "finished but unmerged" branch — it needs a
build+gate run before anyone merges it.

### B2. 29 local branches are ahead of `main`

Names include `feat/counsel-lab` (51), `recovery-branch` (35),
`intel-xpu-harvest` (11), `feat/verify-mask-offset-kernel` (9),
`feat/canonical-effect-abi` (6), `h5-phase-c4` (6), several `experiment/*`,
`backup/pre-a2-cleanup`, `val92`/`val93`, `integrate/pr31`/`pr32`,
`main-prep`, `electronics-hir-port2`, `feat/counsel-*`, `fix/*`.

Not all are finished — many are experiments or backups. `docs/plans/INDEX.md`
declares two **live foreign lanes that must never be merged from main-side
work**: `feat/e14a-intent-synthesis` (`../briev-e14a`) and `feat/bad-dialect`
(`../briv-compiler-bad-dialect`), plus the baseline worktree.

**Decision needed:** an explicit keep/merge/delete list. Without it, "merge
everything ahead of main" would land half-finished experiments.

### B3 (RESOLVED 2026-11-16). CI red on `main` cleared

The `exponent_val` / `ascii_lower` failures were fixed by the mask-interval
modelling in #104 (`e215d341a`), now merged. Verified after the merge:
`import_resolver::test_rbv_edge_declared_surface`,
`packages::dependency_module_is_importable_by_name` and the full
`conformance_sweep` all pass.

Historical note (keep for the pattern, not for action):

`gh run view`: `import_resolver::test_rbv_edge_declared_surface` and
`packages::dependency_module_is_importable_by_name` fail on
`exponent_val`'s precondition `((exp8 >= 0) && (exp8 <= 254))`, and
`conformance_sweep` fails on `ascii_lower`'s
`(byte_at(left, k) >= 0)`. Verified identical on pristine `c9d403f70`, so
nothing from the thread work is red.

**Owner work:** establish the guard at those call sites (`when` guard or a
`Result` boundary in `lib/std/float_fmt.bv` / `lib/std/net/http.bv`).

### B4. `Verify Bodies Census` workflow fails on infrastructure

Last run: *"the hosted runner lost communication with the server"* after 55
minutes. Looks like a resource-exhaustion timeout, not a code failure.

**Decision needed:** split the census across shards, raise the runner class, or
accept it as advisory.

---

## C. Older backlog (unchanged by this session)

| Item | Note |
|------|------|
| Toolchain matrix on CI | needs ≥2 LLVM/Clang versions on the host |
| Issue #2 v2 ABI | Bool/String/struct marshalling + generated headers |
| HIR port queue | `reclamation`, `electronics`, `destination_ownership`, `accel`, `gpu_schedule` |
| Casting-graph Kani proofs | `find_path` / `resolve_llvm_type` |
| Issues #5/#7/#8 | need external reproducers (`test-project-otto`, `transition_validate`) |

---

## E. Execution plan (2026-11-16, approved)

Four steps, each its own commit, each verified before the next. The first is a
correctness fix in work already on `main`; the second closes the one open
branch decision; the last two remove the arena restriction.

### E1 — `emit_main`: the threaded arm wins (A1)

**Change.** `src/backend/llvm/loop_engine/mod.rs`, `emit_main`:

```rust
let threaded = self.ctx.thread_plan.is_pinned();
if threaded {
    call void @briev_workers_release()
    call void @reactor_tick(ptr %state)
    call void @briev_workers_wait()
} else if self.has_async_txns && !self.is_lightweight_async {
    self.emit_async_phase(out, "%state");
} else {
    call void @reactor_tick(ptr %state)
}
```

Why the threaded arm must come first: `emit_async_phase` runs the async bodies
on the main thread and never touches the barrier pair, so a pinned program that
takes that arm leaves every worker parked while main executes other bodies —
worker N's tick never runs and its nodes never fire.

**Fixture.** `tests/fixtures/thread_pin_async_guard.bv`: two pinned nodes whose
preconditions are boolean state flags with no `bounded_pre` increment, which is
exactly the shape `strategy.rs` refuses to call lightweight. Both `async`,
disjoint state, one printing reporter (single observable sink). Assert the run
prints the expected value — a deadlock or a missing tick fails the test by
timeout/number.

**Verify.** `cargo test --test thread_pin`; `scripts/gate.sh quick`.

### E2 — `h5-phase-c4` decision (6 commits, 2026-11-13)

**Method.** Temp branch from `main`, merge, `cargo build --lib`, then
`scripts/gate.sh quick`. If green → land it with the usual commit message. If it
conflicts substantively in HIR lowering (not just docs/INDEX) → close the branch
with the conflict list recorded in this document, because a stale HIR rewrite is
not something to reconcile blindly.

### E3 — Part A: per-worker arenas, injected through the FieldMode pass (A2)

The reverted attempt injected `__arena_ptr_w<N>` **after** `build_field_index`,
and `ctx.field_types` is rebuilt by the FieldMode promotion arm
(`FieldMode::Always | LazyCached` re-pushes `old_types`), which drops the new
slots while `field_index_map` keeps them → emitted `%State` shorter than the
indices referenced.

**Change.** Emit the arena slot triple per worker in the same place the existing
single-worker triple is emitted (inside `build_field_index`, before the
promotion pass), tagged so the promotion arm carries them as `FieldMode::Always`
rather than relying on the generic path. Concretely:

1. `arena_workers: Vec<ArenaSlots>` on `LlvmBackend` + `current_worker: u32`,
   `fn arena_slots() -> Option<ArenaSlots>` (falls back to the legacy triple).
2. Injection: when `thread_plan.worker_count > 1`, push one triple per worker
   with `field_types`/`field_briev_types` entries at injection time (the
   existing single-worker code path stays byte-identical).
3. `emit_arena_init` → rewind (`ptr = base`) when partitioned; `emit_arena_alloc`
   → bump within the slice, `malloc` on overflow, never `brk`/`realloc`;
   `emit_arena_fini` → no-op when partitioned.
4. `emit_arena_reserve_partitioned` in `main` before the pool starts: one
   `brk`/`malloc`/static region of `workers × arena_initial_size`, then one
   triple store per worker.
5. `emit_reactor_named(.., worker)` sets `current_worker` for the body and
   restores it after.

**Then retire the refusal:** delete the both-allocate row from
`thread_plan.rs`, and turn
`tests/compile_fail/thread_pin_alloc_shared_arena.bv` into
`tests/fixtures/thread_pin_alloc.bv` (a positive fixture) asserting each
worker's tick touches only its own slots and that neither emits `brk`.

**Verify.** `%State` for a pinned allocating program lists `__arena_ptr_w0` and
`__arena_ptr_w1`; both ticks write only their own; the linked program runs;
**unpinned** corpus `.ll` stays byte-identical (`ll_determinism_check`).

### E4 — cross-worker write-mask assertion (A3/D.2)

After `build_write_masks`, in debug builds only: for every `%State` slot, the
set of workers whose tick functions can write it must have size ≤ 1. Exemption
list starts empty; a slot that needs one is itself a finding. This is the
mechanical guard that makes "inject a slot and forget the partition" impossible
to miss — the failure that produced A2.

**Verify.** An intentional injected shared slot in a scratch build trips the
assertion; the shipped tree passes the full gate.

### Order and gates

| Step | Gate before commit |
|---|---|
| E1 | `tests/thread_pin` + `scripts/gate.sh quick` |
| E2 | build + `scripts/gate.sh quick` (on the temp branch) |
| E3 | `tests/thread_pin`, `tests/thread_pin_alloc`, determinism corpus, quick gate |
| E4 | quick gate; scratch-build assertion check |

## D. Suggested order once decisions are made

1. Finish/abort the in-progress merge on `main` (see B1-era work — the merge of
   `676d5996b` landed the tautology-discharge + `mantissa_val` fixes, which is
   the same family as B3).
2. A1 (the `emit_main` guard) — smallest change, closes a live correctness hole.
3. A2 option chosen in the plan, then A3, then A4's fixtures.
4. B3 guards to get CI green again.
