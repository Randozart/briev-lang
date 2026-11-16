# Next steps: tail-loop ownership, IR determinism, RSS truth, XPU lanes (2026-11-16)

**Status:** ACTIVE. Follows `2026-11-16-memory-and-xpu-finish-grind.md` (whose
waves 0–3 landed) and records the four open engineering items plus the
**worktree/branch/sync protocol** this lane now runs under.

Baseline at the time of writing: `main` and `intel-xpu-harvest` were both at
`2d1ac6191`; `cargo test --lib` 3857 passed; `scripts/gate.sh quick` pass;
kiosk daemon census **73 releases / 0 undecided / 2196 drops**;
`--deny-memory-ambiguity` exit 0; kiosk pin `641af77` → `2d1ac6191`.

---

## 1. Where we work (binding protocol)

`main` is a **shared trunk** with several concurrent agent sessions. This lane
therefore works in **its own worktree on its own branch**, and only lands on
`main` at milestones.

| Item | Value |
|---|---|
| Own worktree | `/home/randozart/Desktop/Projects/briev-intel-xpu` |
| Own branch | `intel-xpu-harvest` |
| Shared trunk | `main`, normally checked out at `../briev-lang` |
| Kiosk worktree | `/home/randozart/Desktop/Projects/briev-kiosk-avatar` (`main`) |
| Kiosk pin | `compiler.lock` → a commit that exists on `origin` |

### Rules

1. **Never work in `../briev-lang`.** It holds `main` for another session and
   may carry that session's uncommitted files. Reading `origin/main` is enough.
2. **Never `git switch main`** in this worktree — `main` is checked out
   elsewhere, so git refuses; and switching would put our work on the shared
   trunk by accident.
3. **Never use `git checkout --` / `git restore` / `git checkout .`** (AGENTS.md
   Rule 8 — destroys uncommitted work). `git reset HEAD` (unstage) is fine.
4. **Land on `main` with a fast-forward only:** `git push origin HEAD:main`,
   and only when `git merge-base --is-ancestor origin/main HEAD` is true. No
   force-push, ever.
5. **Always `git fetch` before pushing** — other sessions push to `main`
   concurrently. A rejected push means main moved; merge and retry (this
   happened twice on 2026-11-16).
6. **Never amend a pushed commit**; add a follow-up instead.
7. **Build trees live in `/scratch/briev-builds/`, never `/tmp`** (AGENTS.md:
   tmpfs mmap failures kill the linker).

### Sync with main (occasionally, not continuously)

Run `bash scripts/sync-main.sh` from this worktree. It fetches, reports whether
a merge is needed, merges `origin/main`, and on conflict prints the resolution
recipe instead of guessing. Then:

```
cargo build --release        # must be clean
cargo test --lib             # must be 0 failed
bash scripts/gate.sh quick   # ~100 s, per-commit gate
# plus, if the merge touched analysis/ownership/liveness:
bash harness/memcheck_ratchet.sh   (in the kiosk) and a daemon soak
```

**Expected conflicts.** Two files have conflicted on every merge so far, and
both are mechanical:

- `BUGS.md` — both sides prepend entries. Keep **both**, newest first.
- `src/hir/owned_storage.rs` — if `main` ran `cargo fmt --all`, the hunks are
  formatting-only (verify by diffing with whitespace stripped: the two differ
  only by trailing commas). Take this branch's side, then `rustfmt` the file.

**Milestone push.** When a wave completes and the gates are green:
`git push origin HEAD:main` (plus `git push origin intel-xpu-harvest`), then
bump the kiosk pin to the pushed commit.

---

## 2. Item M2 — tail-loop destruction (highest value)

**Why first.** It is the only *measured, unbounded* leak left, and it is fully
local (no hardware). A tail-recursive defn whose recursive call passes an owned
temporary never frees it: measured maxRSS 12.6 MB at N=1 000, 4 775.9 MB at
100 000, **23 675.5 MB at 400 000** (quadratic, because the carried string grows
each iteration).

**Already done.** The shape is *visible*: `memcheck` reports
`self-tail argN ... freed nowhere` as **undecided**, so it blocks
`--deny-memory-ambiguity`. Scalar arguments and parameter aliases are excluded
(both unit-tested) so the report does not lie.

**The fix, precisely.** In `try_emit_tail_loop_backedge`
(`src/backend/llvm/emit_expr.rs`), per owned backedge argument:

1. At loop entry, initialise a hidden `owned` slot to `false`; the parameter
   slot holds the **caller's** value.
2. Before each backedge store, branch on `owned`: if true, load the current
   slot value and `free` it; then store the new value and set `owned = true`.
3. On the base/exit path, release the final carried value where it is consumed.

**Why the obvious fix is wrong** (recorded so it is not retried): freeing the
slot value unconditionally frees the *caller's* argument on the first
iteration — the slot is initialised from the caller's handle. That produced
`free(): invalid pointer` on an earlier attempt.

**Fact source.** Which backedge args are owned belongs in the frontend
(`RecursiveCarrierValue`, `src/analysis/destination_model.rs`), per
frontend-driven dispatch — never a name/type heuristic in the backend.

**Gates.** `tests/tier1/owned_tail_cleanup.bv` becomes memory-checked (malloc
counter or freed-count golden); a new fixture for the `term f(concat(...))`
shape; `cargo test --lib`; kiosk gates + soak; the leak probe
(`get_env_int!` bound, maxRSS at two Ns) recorded in a results file.

## 3. Item R — RSS step-growth: arena retention or true leak?

**Why it matters.** Every soak conclusion depends on this. Current evidence:
three builds (oldest `4aed72809`, pre-merge `751c66932`, merged) all walk
150 → 190–204 MB over 400 calls in **steps with flat stretches**, so it is not
a merge regression and not a clean per-call leak. The campaign's own record
(2026-11-11) already called it "small per-request heap objects; a full plateau
is still open".

**Do this first:** read what main's `06b769598` ("Thread lane A4 … **retract the
arena report** and fix the gate it hid") retracted — do not re-derive a refuted
conclusion.

**Then, in order:**
1. Count `malloc`/`free` in `lib/std/c/coll_arena.c` and diff the per-call
   delta — this distinguishes "we allocate and free" from "we allocate and
   retain".
2. Sample RSS after `malloc_trim(0)` mid-soak. If the number drops to the
   plateau, it is arena retention, not a leak.
3. Only then consider a longer window; 400 calls cannot resolve a trend whose
   steps exceed the mean slope.

**Gates.** A results file carrying both counters and the trim experiment, with
the same-geometry correctness gate as every other soak.

## 4. Item D — IR determinism — **DONE 2026-11-16**

**Outcome.** `%ip_N` was never a second bug: it is the init-store register for
`%State` field `N`, so an index shift renames it. One root cause — hash-container
iteration in the state layout:

- `apply_field_modes` Phase 2 appended LazyCached cache-slot pairs in
  `field_modes` / `projection_usage` order (now sorted, via
  `register_cache_slots` + `push_cache_slot_pair`);
- `assign_field_modes` (`src/analysis/transition_graph.rs`) handed out
  `cache_index` in `all_state_fields` (HashSet) order (now sorted; the value has
  no consumer today, so this is trap-removal).

The kiosk daemon now compiles byte-identically in two separate processes
(1 298 331 bytes, empty `diff`). New corpus fixture
`tests/tier1/state_layout_determinism.bv` (state-level `obj` projected onto two
members, twelve scalar fields, six interned const strings) is byte-identical
across three processes and joined `benchmarks/ll_determinism_check.sh`.

**Scope note.** The LazyCached path only fires when a projection set has ≥2
targets AND the name is a `%State` key; an unpacked `obj` instance is laid out
as `base.member` columns, so the cache path stays dormant for the ordinary
instance shape. The daemon reproduction — not the fixture — is the load-bearing
evidence. If a future change makes that path live, the sort is already in place.

The original item follows.

`src/hir/owned_storage.rs` release order is fixed (sorted). Two remain, found by
building the kiosk daemon twice in separate processes:

- `%State` field index (`i32 8` vs `i32 9`);
- interned string assignment (`%ip_8` vs `%ip_9`).

Both are the HashMap-iteration hazard (`AGENTS.md`: sort every map iteration
that reaches IR; unsorted order costs up to ~9 % perf variation and defeats IR
A/Bs).

**The gate gap is half the bug:** `benchmarks/ll_determinism_check.sh` passes on
its corpus (recommender/multinode/pp-types) because none of those combines many
state fields with many interned strings.

**Work.** Find the unsorted iteration in the state-layout assignment and in
string interning, sort them, and add a state-heavy fixture with many interned
strings to the determinism corpus so the class cannot regress.

**Gates.** Two-process byte-identical `.ll` for the new fixture AND for the
kiosk daemon; `bash benchmarks/ll_determinism_check.sh --quick` and full.

## 5. Item X — XPU lanes (blocked on device access)

Six items, all recipe-ready, all needing the PTL node. The node answers on
`100.92.76.67` but only with interactive password auth (no key), so a session
cannot drive it unattended; the local box is NVIDIA-only.

| id | item | recipe |
|---|---|---|
| X1 | f16 coopmat tier (9× slower than the exact tiled kernel at 4096³) | A/B with `spirv_coopmat: 0` in `config/ir-lowering.dbvl`; verify the **work count** of both arms before comparing times |
| X2 | NEO/OpenCL residency | `launch_dev`/`launch_dev2d`, mirroring the L0 shape |
| X3 | GLSL/XMX ceiling harness (W2c) | hand-written GLSL reference; local toolchain verified (`glslangValidator`, `spirv-val`, Vulkan headers) |
| X4 | `clFinish` hang (`BUGS.md` 2026-11-09, ~1/3 under load) | reproduce in the ceiling harness (no compiler in the loop), then bisect |
| X5 | Stage F promotion gate | unblocked since the probe fix (641 MB → 204 KB); re-run and record |
| X6/X7 | dispatch waves 0–3; OpenVINO FFI gate on the node | `intel-dispatch.md` §9; `tests/openvino_gate.rs` |

**Blocked on:** key-based access to the PTL node.

## 6. Item K3 — edge budgets (blocked on owner numbers)

Peak RSS, cold start, model load and binary size must be locked and measured
under a constrained runner (cgroup), not on the 62 GB host. Until then no stage
may be called "edge-ready" (unified plan §7).

---

## 7. Ordering and rationale

1. **M2** — measured, unbounded, local, and it is the last known ownership
   *bug* rather than debt.
2. **D** — cheap, and it protects every future IR A/B (including M2's own
   evidence).
3. **R** — needs the A4 retraction read first; it decides how much of the
   remaining soak noise is real.
4. **X / K3** — as soon as access and numbers exist.

## 8. Gates (every commit)

`cargo test --lib` · `scripts/gate.sh quick` (`full` before pushes touching
shared analysis) · IR determinism (backend commits) · kiosk gates + census
ratchet when the pin moves · Praetor on changed directories · any perf/memory
number in a results file only with a same-geometry correctness gate.

## 9. Documentation to update in the same commit

| change | doc |
|---|---|
| M2 lands | `BUGS.md` entry closed; `2026-11-16-memory-and-xpu-finish-grind.md` §M2 status |
| sync protocol changes | this file §1 |
| determinism corpus grows | `benchmarks/ll_determinism_check.sh` comments + `BUGS.md` |
| RSS verdict | a new `benchmarks/results/` record |
| lane state | `docs/plans/INDEX.md`, `benchmarks/results/2026-11-XX-intel-*.md` |
