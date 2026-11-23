# Wave-10 plan — hygiene sweep, the fact-join fix, and the correctness families (2026-11-22)

**Trigger.** Owner: "You don't need my call on the scope of work, I will
always approve completeness. Syntax and semantics are what I care about."
Plus: clean up the local worktrees, make sure the main folder is on `main`,
and check whether orphaned documents in worktrees ought to be committed —
"other agents may have forgotten."

**Owner decision locked in this conversation:** `endprogram` is
**unconditional** — when it is encountered, the program stops. Therefore a
guard `when cond { endprogram … };` licenses `!cond` for the fall-through
path in ANY body kind, node included. This is the #151 refinement that was
held for an owner call; it is now given.

**Sync:** work starts from `origin/main == 0c1c9a88c` (the net/G22 lane's
tip; `main` folder already sits on it).

## Stage A — hygiene: orphaned docs, branches, worktrees

### A1 · Orphaned documents (owner: "see if they ought to be committed")

| doc | where | verdict | action |
|---|---|---|---|
| `.opencode/plans/2026-11-20-goose-research.md` | main folder | research notes with fetched citations, NO tracked equivalent | commit to `docs/plans/` |
| `.opencode/plans/2026-11-20-stdlib-critique-research.md` | main folder | external stdlib critique that drove wave-5; NO tracked equivalent | commit to `docs/plans/` |
| `.opencode/plans/2026-11-20-checkfree.md` | main folder | superseded by tracked `docs/plans/2026-11-20-checkfree-obligation.md`, but contains the original design + the soundness STOP narrative | commit under `.opencode/plans/` is wrong home — move the *unique* narrative into the tracked doc as a history section; keep the file out |
| `.opencode/plans/2026-11-20-evented-net.md` | main folder | covered by tracked wave-7 plan (the net campaign's own docs live on the evented-net branch) | leave untracked; the PR's branch carries its own docs |
| `docs/plans/2026-11-16-verification-next-steps.md` (untracked copy in counsel-seed) | counsel-seed worktree | a NEWER measured section ("corpus stands, measured, current main", 49-file CI-bounded corpus after PR #105) than the tracked version (dated `424c69ed`) | preserve the newer measurements: merge the differing section into the tracked doc, then the worktree copy can go |
| `2026-11-20-wave6-defer-index-types.md`, `2026-11-20-wave7-…` (main `.opencode`) | main folder | superseded by tracked wave records | leave |

### A2 · Branches and worktrees

Measured: 96 local branches, 16 worktrees, 8 with modifications.

- **Superseded, delete after verifying zero unique content:**
  `briev-dogfood`/`feat/stdlib-path-check` (G0 landed on main separately —
  `src/pipeline.rs:986` carries the comment; commit `6e2f80150`'s content is
  duplicated), `counsel-seed`/`feat/verify-summaries` (Load# width-bounded
  modelling is on main at `briev-verify/src/symbolic.rs:344`).
- **Keep, do not delete (unique content, candidates for separate decisions):**
  - `briev-call-fact-soundness` (`fix/call-contract-fact-scope`): 8 unmerged
    commits — Kani repro work for statement-entry proof timeouts and 6 plan
    docs. Research-relevant to the fact-engine campaign (Stage C).
  - `briev-counsel-lab` (`feat/counsel-lab`): 5 unique plan docs
    (counsel publish/bug-hunt roadmap) + code stale-baseline; keep branch,
    harvest only the docs into `docs/plans/` if owner wants the counsel
    roadmap on trunk.
  - `briev-intel-xpu` (lane session's tree, clean): leave to that session.
  - `briev-compiler-baseline` (locked A/B baseline per AGENTS.md): keep.
- **Delete freely (merged into main):** ~40 branches `git branch --merged`
  reports.
- **Stash, never `checkout --`:** every dirty worktree not listed above,
  each stash labelled with the worktree + reason.

### A3 · Main folder

Already on `main` at origin tip; only untracked notes + `fuzz/Cargo.lock`
(excluded by policy). After A1 there is nothing left to do except confirm.

## Stage B — the fact-join regression (Tier 1, unblocks the kiosk)

The reduced shape (BUGS.md 2026-11-21):

```briev
let c: Int = 0;
when n > 1 { c = chan(i); };   // chan's post: 0 <= c < 8
acc = acc + get(8, c);          // c >= 0 && c < 8 NOT provable post-wave-8
```

Root: the wave-8 checker changes dropped the fact join for a local assigned
a provable value on one path and a call result (with post facts) on the
other. Pre-wave-8 `4caace291` accepted it. Fix at the assignment/guard
boundary: when a guard body reassigns a local, the join must merge (a) the
pre-guard binding's facts and (b) the branch's post-facts about the new
value — instead of killing the local's facts and deriving nothing. Test: the
BUGS-reduced program proves; the kiosk daemon (`briev-kiosk-avatar/service/
daemon.bv`) builds at trunk (41 → 0 refutations).

## Stage C — the fact-engine refinements (#151, with the owner's decision)

1. **`endprogram` refinement (owner-decided: unconditional).** A guard body
   ending in `endprogram` licenses `!cond` for the successor in any body
   kind — same mechanism as wave-8's `stmts_end_defn`, extended to node/txn
   walkers. The wave-8 predicate already treats `EndProgram(_)` as an exit;
   what is missing is the *node-path* walker using it.
2. **`trap;` refinement.** Same: `trap` cannot fall through, so `!cond`
   holds on the successor.
3. **Monotonicity across a diverging-branch call.** A call inside a guard
   branch that always exits cannot be observed on the fall-through path;
   facts established by earlier guards must survive it.

All three land as one change to the guard-exit predicate used by the fact
walkers, with per-case tests. These un-disentangle #151's three repros and
are the same machinery Stage B needs.

## Stage D — the correctness families

- **Float seam** (#153 literal f32-rounding in a Float64 slot; #154
  `Float64 as String` uses the 32-bit formatter; #155 `json.bv` binary32
  loss). One campaign: literal → storage → formatter honour the declared
  width. Binary64 end-to-end.
- **Clone family** (#158 struct-local reassignment corrupts/SIGSEGV; #159
  List-of-struct append corrupts fields). Ownership-clone defect; the K3 R2
  ty-stamp lesson applies.
- **#157** aliased `Result<String,String>` locals in a recursive function.
- **#152** HashMap query lowers to a non-dominating alloca.
- **#156** empty list literal in an obj field: front-door check.
- **#148 closure**: close with the wave-7 regression reference.

## Stage E — gates

Per-stage: build, focused tests. End: `cargo test --lib`,
`scripts/gate.sh full`, determinism quick, Praetor on changed dirs, push,
CI green.

## Invariants

| # | Property | Pinned by |
|---|---|---|
| 1 | The BUGS-reduced join program proves at trunk | `fact_join_conditional_reassignment` (typechecker test) |
| 2 | The kiosk daemon builds at trunk (41 → 0) | kiosk repo build check (documented; the daemon repo is separate) |
| 3 | `when cond { endprogram … };` licenses `!cond` in a node body | `endprogram_guard_refines_node_fallthrough` |
| 4 | `when cond { trap; };` licenses `!cond` | `trap_guard_refines_fallthrough` |
| 5 | A call in an always-exiting guard branch preserves earlier obj-field facts | `diverging_branch_call_preserves_facts` |
| 6 | A node guard that merely continues (bare `term;`) still licenses nothing | `node_bare_term_still_licenses_nothing` (guards the boundary) |
| 7 | `Float64` literal → slot → `as String` is binary64 round-trip | `float64_round_trip` (per #153/#154) |
| 8 | JSON fractional parse preserves binary64 | #155's repro as a test |
| 9 | Struct-local reassignment/appending does not corrupt fields | #158/#159 repros as tests |
| 10 | No dotted reflection returns; no `.bv` regression in corpus | grep gate + conformance sweep |
| 11 | Orphaned docs are committed or explicitly left, with the decision recorded | Stage A table + commit history |
| 12 | Deleted branches have zero unique content, verified before deletion | `git branch --merged` + per-branch `log origin/main..HEAD` check |

## Tests to write

- Stage B/C: one typechecker test per invariant 1, 3–6 (the BUGS repro
  verbatim as invariant 1's input).
- Stage D: each issue's minimal repro as a regression test (compile+run for
  the codegen ones, check-level for the float seam).
- Stage A: no new tests; the deliverable is the commit record + this table.
