# Counsel follow-ups plan (post-#102/#100)

Status: **DONE**, plus a follow-on prover fix (PR #104).

Outcome in one line: the `mantissa_val` unproven bucket is closed *at the
prover*, not by weakening the contract. `--verify-bodies` on `.smoke/` went
from 40 unproven to 0 with zero violations, and `proven` rose from 104 to 119.

### Resolution log

| Step | Result |
|---|---|
| Path A (strengthen pre alone) | Rejected on its own — the body still did not prove |
| Path B (document the gap) | Superseded by the kernel below |
| **Kernel (PR #104)** | Masked-offset interval + literal-shift folding; `mantissa_val` proves |

The kernel is the honest fix: it teaches the prover a true fact
(`(x & mask) + n ∈ [n, mask+n]`) instead of relaxing a postcondition. It is
guarded by tests for all three outcomes (proved / refuted / undecided), and a
60-file sweep reported zero violations.

---

## Historical record (as planned, 2026-11-15)

Three near-term follow-ups from the verify-bodies campaign:
1. Merge PR #102 (`feat/counsel-expand` — stdlib fixes + census script + CI).
2. Triage the `mantissa_val` unproven diagnostic (the dominant unproven bucket).
3. Run PR #100 (`feat/counsel-seed-v3` — 4 Rust advisory contracts) through review/merge.

## 0. Open PR state (verified in scratch worktree)

| PR | branch | head commit | contents | status |
|---|---|---|---|---|
| #100 | `feat/counsel-seed-v3` | `16f4ef692` | 4 Rust `#[post]` advisory contracts: `Interval::size`, `CallGraph::node_count`, `count_statements_recursive`, test-only `canonical_rid` | open, unreviewed |
| #102 | `feat/counsel-expand` | `6efc844a3` | `float_fmt.bv` (exponent_val pre tighten + mantissa_val guards), `iterator.bv` (iter_collect empty guard), `scripts/verify-bodies-census.sh`, `.github/workflows/verify-bodies-census.yml`, this campaign's plan doc | open, unreviewed |

`feat/counsel-expand` is built **on top of** `feat/counsel-seed-v3` (its base is
`16f4ef692`). So **#102 conceptually contains #100's diff.** They are not
independent trees:

```
main (8da7e61d2/5de99c4cd)
└── feat/counsel-seed-v3 (16f4ef692)   ← PR #100
    └── feat/counsel-expand (6efc844a3) ← PR #102
```

**Decision point 0 (sequencing).** Because #102 already carries #100's 4
advisory contracts plus the stdlib/census work, the cleanest landing order is:

- **Merge #100 first**, then #102's base rebases to main automatically and #102
  shows only the delta (stdlib + census + CI). No conflict, no duplicate
  contracts.
- Alternative: squash-merge #102 and let #100 close as "superseded." More
  noise, but one landing. **Recommendation: merge #100 → merge #102.**

---

## 1. Root cause — `mantissa_val` (the dominant unproven bucket)

### 1.0 Gate result (2026-11-15)

**Path A tried, rejected.** Added `&& mant < 0x800000` to the pre, rebuilt
brievc, ran `--verify-bodies` on `float_fmt.bv`: `mantissa_val` still unproven.

**Root cause:** The prover does not model bitwise `&` as a numeric bound.
Even though the caller passes `mant = bits & 0x7FFFFF` (guaranteeing
`mant < 0x800000`), the prover can't derive that constraint from the pre
or from the call site. Strengthening the pre didn't help because the body
still has paths where the prover can't prove `mant < 0x800000` →
`mant + 0x800000 < 1 << 24`.

**Decision: Path B.** Keep the pre as-is (`mant >= 0`), keep the subnormal
guards (`exp8==0 && mant==0 → term 1`), document as an accepted unproven
gap. The post is semantically correct for all callers; the prover just
can't see the 23-bit mask bound.

### 1.1 The contract as committed in #102

```briev
defn mantissa_val(exp8: Int, mant: Int) [exp8 >= 0 && exp8 <= 255 && mant >= 0]
    [term >= 1 && term < 1 << 24]
    -> Int {
    when exp8 == 0 && mant == 0 { term 1; };
    when exp8 == 0 && mant > 0 { term mant; };
    term mant + 0x800000;
};
```

Three terminating paths. The post is `term >= 1 && term < 1 << 24`
(`term < 16777216`).

| path | `term` | `term < 1<<24` needs | provable from pre `mant >= 0`? |
|---|---|---|---|
| `exp8==0 && mant==0` | `1` | trivial | ✅ |
| `exp8==0 && mant>0` | `mant` | `mant < 1<<24` | ❌ pre has no `mant` upper bound |
| default (exp8≠0) | `mant + 0x800000` | `mant < 0x800000` | ❌ pre has no `mant` upper bound |

The prover report `could not prove the postcondition on path 1 of 3` points at
the first non-trivial path; the two subnormal/default paths can't be discharged
because **the pre does not upper-bound `mant`.**

### 1.2 The caller already supplies that bound

`float_format` (float_fmt.bv:432) is the only *source* caller of
`mantissa_val` (every other hit is generated `.ll`):

```briev
let mant: Int = bits & 0x7FFFFF;          // 23-bit mask → mant ∈ [0, 0x7FFFFF]
let m: Int = mantissa_val(exp8, mant);    // float_fmt.bv:453
```

`bits & 0x7FFFFF` guarantees `mant < 0x800000` by construction. So the pre
`mant >= 0` is *weaker than what the caller actually guarantees* — this is a
missing fact, not a semantic defect. The earlier "prover limitation on `term`
in `when` guards" hypothesis (from the campaign summary) is a red herring: the
real blocker is the unbounded `mant` in the pre.

### 1.3 The honest fix (strengthen, never weaken)

Tighten the pre to capture the caller's 23-bit fact:

```briev
defn mantissa_val(exp8: Int, mant: Int)
    [exp8 >= 0 && exp8 <= 255 && mant >= 0 && mant < 0x800000]
    [term >= 1 && term < 1 << 24]
    -> Int { ... }   // body unchanged
```

- **Path A (recommended).** Add `mant < 0x800000` to the pre. This is a
  *strengthening* that reflects the actual caller, so it satisfies the
  "never weaken specs" rule and directly closes the dominant unproven bucket.
- **Risk (call-site propagation).** `float_format` must now establish the
  strengthened pre `mant < 0x800000` at the call. The prover bounds it only if
  it models `bits & 0x7FFFFF` as `< 0x800000`. If it can't, the diagnostic
  *moves* from `mantissa_val`'s body to `float_format`'s call site — a wash,
  not a regression, but it may not reduce the total unproven count.
- **Verification gate before committing:** re-run
  `brievc build lib/std/float_fmt.bv --verify-bodies` after the edit.
  - Expected: `mantissa_val` body diagnostic gone.
  - Check: did `float_format` gain a new unproven call-site diagnostic?
- **Path B (fallback).** If Path A only moves the diagnostic, or if the
  `&`-bound is undecidable even at the call site, accept `mantissa_val` as a
  *documented unproven gap* (record the reason in the census plan doc; keep the
  subnormal guards; leave pre as-is). This is honest — the post is semantically
  correct for the caller, the prover just can't see the 23-bit mask bound.

---

## 2. Pre-merge gates for PR #102

Before #102 is mergeable, the campaign infrastructure it ships must be sound:

1. **Census script** — `scripts/verify-bodies-census.sh` must pass its own
   sanity check on a small corpus (e.g. `.smoke/*.bv`):
   - exits 0 on a zero-violated run,
   - emits JSON that parses (`python3 -m json.tool`),
   - the summary counts match the per-file sums.
   - Known fragility (from campaign notes): the JSON summary block's
     `proven/unproven/skipped/violated` counters must be populated, not just
     `total_files`. Confirm before merge.
2. **CI workflow** — `.github/workflows/verify-bodies-census.yml`:
   - valid YAML (`python3 -c "import yaml,sys; yaml.safe_load(open(...))"`),
   - runs `scripts/verify-bodies-census.sh` with `BRIEVC=./target/release/brievc`,
   - archives the JSON,
   - **fails only when `violated > 0`** (non-blocking on unproven/skipped).
   - Confirm it is a *non-blocking* job (does not gate the PR) — the campaign
     decision was "report now, block later."
3. **No contract weakening in the diff** — the stdlib changes are
   pre-tightenings (`exponent_val` `exp8<=254`, `mantissa_val`/`iter_collect`
   guards + the new `mant < 0x800000` pre). Re-read each to confirm none relax
   a post or a caller-facing guarantee.

---

## 3. PR #100 review (Rust advisory contracts)

The 4 contracts are **advisory / unproven by design** (Counsel 0.5.0 subset):

- `Interval::size` — `UnsupportedCounts::total` proven? No: the body has an
  `as`-cast → outside the pure-integer subset → **unproven (advisory)**.
- `CallGraph::node_count` — collection length fact applies to *parameters*, not
  struct fields (`self.txn_names`) → **unproven (advisory)**.
- `count_statements_recursive` — `while let` / method-effect path not modelled
  → **unproven (advisory)**.
- `canonical_rid` (test-only) — opaque `HashMap::get` → **unproven (advisory)**.

**Review checklist:**
- [ ] Every contract has an intent comment (Praetor intent rule).
- [ ] No `#[pre]`/`#[post]` was weakened to make a check pass.
- [ ] `counsel-check` run is recorded as "no Violated" (advisory is fine).
- [ ] The 4 are honest: they document intent, labelled unproven, not proofs.
- [ ] No `git commit --no-verify` / no `// praetor:ignore` in the diff.

Because they are advisory, the bar is *honesty + no violation*, not *proven*.

---

## 4. Concrete step checklist

### Phase 1 — `mantissa_val` (DONE — Path B)
- [x] 1. Tried strengthening pre with `&& mant < 0x800000`.
- [x] 2. Built brievc with `--target-dir /scratch/briev-builds/bc-target`.
- [x] 3. `--verify-bodies` on `float_fmt.bv`: `mantissa_val` **still unproven**.
      Reason: briev-verify cannot model bitwise `&` as a numeric bound.
      Even though the caller passes `mant = bits & 0x7FFFFF`, the prover
      cannot derive `mant < 0x800000` from that.
- [x] 4b. Reverted pre change. Documented as accepted unproven gap (Path B).
      Commit: `c14976873` on `feat/counsel-expand` → PR #102.

### Phase 2 — merge order (BUILD)
- [ ] 1. Sanitize #102 pre-merge gates (§2): census script + CI sanity,
      confirm non-blocking.
- [ ] 2. Merge **PR #100** first (squash). Its base is now `main`.
- [ ] 3. Rebase/refresh **PR #102** onto updated `main`; confirm clean diff.
- [ ] 4. Merge **PR #102** (squash). Counsel layer + stdlib fixes + census/CI
      land on main.

### Phase 3 — record (BUILD, optional)
- [ ] 1. Update `docs/plans/2026-11-14-counsel-expansion-plan.md` with the
      `mantissa_val` outcome (Path A vs B) + final census counts + merged PR
      numbers.
- [ ] 2. `memory_write` the merge of #100/#102 and the `mantissa_val`
      resolution so it survives across sessions.

## 5. Risks / decision points (summary)

- **Sequencing (Decision 0):** merge #100 before #102 to keep the diff clean;
  #102 base is #100. If #102 has already been merged by the time #100 is ready,
  reverse the order.
- **`mantissa_val` (Path A vs B):** A is preferred (honest strengthening,
  closes the bucket). Guarded by the §1.3 verification gate; fall back to B
  (documented gap) if A only relocates the diagnostic.
- **Census fragility:** summary counters + JSON validity must be confirmed
  before #102 merges, or CI will archive a broken artifact.
- **CI non-blocking:** the new workflow must *not* gate PRs during this
  baseline phase (fail only on `violated`).

## 6. Not in scope

- Re-enabling/extending the dormant `briev_verify` symbolic prover for
  bitwise-AND bounds (`& 0x7FFFFF` → `< 0x800000`). That is the deeper fix for
  Path-B cases; track separately as a prover roadmap item.
- Growing the `counsel` crate beyond 0.5.0. Only if the above is blocked.
- Moving `--verify-bodies` from non-blocking to blocking. Blocked until the
  `mantissa_val`-class false-positive rate is understood and zero-violated.

## Phase 2 — merge order (BUILD)
- [x] 1. Census script validated on smoke corpus: JSON parses, summary counters
      match per-file sums, exits 0 on zero-violated.
- [x] 2. PR #100 review (below). Honest advisory contracts, no weakened specs.
- [ ] 3. Merge PR #100 first (squash). Its base is now `main`.
- [ ] 4. Rebase/refresh PR #102 onto updated `main`; confirm clean diff.
- [ ] 5. Merge PR #102 (squash). Counsel layer + stdlib fixes + census/CI land on main.

## Phase 3 — record (BUILD, optional)
- [x] This doc written with full findings, Path B resolution, and merge checklist.
- [ ] 1. Update docs/plans/2026-11-14-counsel-expansion-plan.md with merged PR numbers.
- [ ] 2. memory_write the merge of #100/#102 and the mantissa_val resolution.

## PR #100 review (done — ready to merge)
Files: src/analysis/call_graph.rs + src/analysis/region.rs

Contracts added:
1. CallGraph::node_count — Unproven (advisory): Vec::len not modelled
2. Interval::size — Unproven (advisory): as-u64 cast outside subset
3. count_statements_recursive — Unproven (advisory): while-let outside subset
4. canonical_rid (test-only) — Unproven (advisory): HashMap::get opaque

Checklist:
- [x] Every contract has intent comment preserved
- [x] No #[pre]/#[post] weakened to pass a check
- [x] counsel-check: 0 Violated (advisory is fine)
- [x] No --no-verify / no // praetor:ignore in diff
- [x] Honest: documents intent, labelled unproven, not claimed as proof
