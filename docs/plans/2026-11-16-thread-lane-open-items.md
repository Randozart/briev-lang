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

### B3. CI has been red on `main` for unrelated reasons

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

## D. Suggested order once decisions are made

1. Finish/abort the in-progress merge on `main` (see B1-era work — the merge of
   `676d5996b` landed the tautology-discharge + `mantissa_val` fixes, which is
   the same family as B3).
2. A1 (the `emit_main` guard) — smallest change, closes a live correctness hole.
3. A2 option chosen in the plan, then A3, then A4's fixtures.
4. B3 guards to get CI green again.
