# Branch ledger — stale experiment lessons (2026-11-17)

**Purpose.** Before deleting stale branches, this ledger records what each one
tried and what it proved, so the knowledge survives the ref. The branches
themselves are deleted in the same commit; this file is the durable record.

## `feat/ptr-provenance-fence-retirement` (PR #33, refuted)

**What it tried.** Retire the narrow volatile fence for raw accesses inside
Ptr-parameter defns (`9d809af71`), on the hypothesis that canonical Ptr call
marshaling + entry preloading had made it redundant.

**What it proved.** Experiment 1A (commit `c626c2dc`, fence disabled):
local LLVM 23 and the full lib suite went green, **but the LLVM-18 CI job
failed `ptr_crossdefn_probe` — `0` instead of `7`**. The fence was restored
in `f2a580cc` and draft PR #33 closed without merge.

**Lesson (durable).** Canonical Ptr call marshaling + entry preloading are
**insufficient on LLVM 18**. The fence stays; interprocedural provenance
(caching a canonical pointer per provenance root only where the defining
SSA name dominates, one canonical pointer per root, preload at
function/txn/main entry) is the real retirement path and remains deferred
behind P2-1. Do not re-run the fence-removal probe until the 1B
interprocedural provenance design exists — the refutation is recorded in
`docs/plans/2026-10-21-post-effect-shape-next-work.md` §"Result —
Experiment 1A refuted".

**Full trail.** `docs/plans/2026-10-21-post-effect-shape-next-work.md`
(Experiment 1A/1B design + refutation). The two branch commits (`c626c2dc`
fence-off, `f2a580cc` fence-on) are the refutation's A/B.

## `integrate/pr31` — pointer-valued collection store (merged as #31)

**What it tried.** Fix `pg_decode_fields` emitting invalid LLVM: a
string-literal pointer consumed by `store volatile i64`. Root cause:
inferred collection literal types were lost at generic calls.

**Outcome.** Merged as #31 (`412d84c10`). The branch's real commit
`367470ac8` is subsumed.

**Lesson (durable).** The first hypothesis (singleton `List<String>`
construction alone fails) was **refuted** by controlled Int/String probes —
the discriminator was `fields + [literal]` list concatenation with an inline
String RHS, not literal construction. Fix on the inferred-type lane, not the
collection emitter. Full trail: `docs/plans/2026-10-21-pointer-valued-
collection-store.md`.

## `integrate/pr32` — match terminating-arm values (merged as #32)

**What it tried.** A match containing a `trap` arm erased a continuing arm's
result (undefined return register in clang). Root cause: match type probing
treated the terminating arm's `Void` as a continuing valueless arm,
suppressing the merge phi, and did not reset/restore the termination flag
between probe emissions.

**Outcome.** Merged as #32 (`e6c59d4c1`). The branch's real commit
`23cbbdace` is subsumed.

**Lesson (durable).** Probe emission must retain types only for arms whose
emission reaches the merge, and restore termination state. Preserve genuine
continuing-`Void` and the all-arms-terminate path. Full trail:
`docs/plans/2026-10-21-match-terminating-arm-values.md`.

## Fully-merged branches (no un-landed knowledge)

- `feat/proof-result-cache` — SMT proof-result caching, merged to main
  (commit `a5441d675` era). No un-landed content.
- `pr-49`, `pr-50`, `pr-51` — nested-enum / object-payload fixes, merged.
  No un-landed content.
- `local/pr48-validated-pending-browser` — PR #48 (pooled-object-assignment),
  merged. No un-landed content.

## Stale local refs to a separate remote (`pr/61`, `pr/62`, `pr/62b`)

- `refs/remotes/pr/61`, `refs/remotes/pr/62`, `refs/remotes/pr/62b` — local
  refs to a *different* remote (`pr/`, not `origin/`). All three are fully
  on `main` (0 ahead, ancestors of main). Stale refs, no un-landed knowledge.
  Deleted via `git update-ref -d refs/remotes/pr/<n>`.

## Not deleted (active worktrees)

- `feat/e14a-intent-synthesis` — live foreign lane (see
  `docs/plans/INDEX.md` "live foreign lanes"). Do not touch.
- `feat/bad-dialect`, `feat/counsel-lab`, `feat/compiler-assurance-lean`,
  `intel-xpu-harvest`, `h5-phase-c4` — active worktrees, not stale.
- `feat/counsel-expand*`, `feat/counsel-seed*`, `feat/counsel-merge-test` —
  counsel layer, merged content but kept for the counsel workstream; not
  stale experiments.

## Deletion safety

- A branch is deleted only when it is **not checked out in any worktree** and
  its content is **either on main or fully recorded here**.
- The two experiment branches' A/B commits (`c626c2dc`/`f2a580cc`) remain in
  the object database (reachable by SHA) even after the refs are deleted — the
  refutation is reproducible from the ledger.
