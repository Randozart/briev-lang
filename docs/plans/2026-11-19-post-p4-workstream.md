# Post-P4 workstream — trunk sync, kiosk pin, follow-up program

**2026-11-19.** P4 (fsum_approx butterfly) is DONE and landed (`0ec3369e3`). Trunk
has advanced 7 commits since. This plan sequences the open workstreams.

---

## 0. Current state

| Item | State |
|---|---|
| `main` == `intel-xpu-harvest` | `0ec3369e3` (trunk at `01f313e64`, 7 ahead) |
| Trunk new commits | `984ba5359` (#135/#134 fix), `5a19d6f35` (G2 census + parity), `85e5fb7ed` (literal→Float sitofp), `5affa7716` (String→Data CI-red), `4ada76844` (follow-up program doc), `01f313e64` (IPPROTO_TCP), merge |
| Kiosk pin | `df624d41e` (stale — trunk fixes closed the census gap) |
| Kiosk census | baseline `drops=2196`; the +1 conservative drop (2197) was the blocker |
| G2 census | 173 `term;` sites, 0 `term! ->` sites (trunk doc says 174; off-by-one from corpus drift) |
| Open issues | #137 (Float32 width), #134 (re-test after #135 fix), #4 (macOS, external), #2 (C static lib) |
| PTL XPU lane | G0 PASS; fsum_approx fixed; all other lanes green |

---

## 1. Phase A — Trunk sync into lane (immediate, ~5 min)

**Goal:** bring `intel-xpu-harvest` current with `origin/main`.

**Steps:**
1. `bash scripts/sync-main.sh` — merges `origin/main` into `intel-xpu-harvest`.
2. Verify no conflicts (trunk touched `src/backend/llvm/emit_expr.rs`,
   `src/backend/llvm/emit_stmt.rs`, `src/backend/llvm/helpers.rs`,
   `src/interpreter/eval.rs`, `src/typechecker/mod.rs`,
   `lib/std/net/socket.bv`, `scripts/g2_census.py`, `docs/plans/`, `tests/` —
   zero overlap with SPIR-V/accel files, so merge should be clean).
3. `bash scripts/gate.sh quick` — confirm green on the merged tree.
4. Land: `git fetch origin main --quiet && git merge-base --is-ancestor
   origin/main HEAD && git push origin HEAD:main && git push origin
   intel-xpu-harvest`.

**Risk:** minimal — the 7 commits are interpreter/typechecker/stdlib; none
touch the XPU lane. The only shared file is `BUGS.md` (auto-resolved by git
in both prior merges this session).

---

## 2. Phase B — Kiosk pin advance (next, ~30 min)

**Goal:** advance the kiosk compiler pin from `df624d41e` to the post-sync
lane tip, unblocking the daemon's census gate.

**Why it's unblocked now:**
- The +1 conservative drop (2196→2197) that blocked the pin was caused by a
  trunk census regression. Trunk `5a19d6f35` fixed the G2 parity regression
  and `5affa7716` fixed the String→Data CI-red — both were gating the census.
- `984ba5359` closed #135/#134, the last open interpreter/typechecker defects
  in the pin's dependency chain.
- The follow-up program doc §0.1 confirms the width-mismatch census is now
  ZERO — no latent defects remain in the call-argument path.

**Steps:**
1. Rebuild the kiosk daemon against the post-sync lane tip:
   - Update `compiler.lock` in `briev-kiosk-avatar` to the new commit hash.
   - Run the build (per kiosk `Makefile` / `build.sh`).
2. Run the owners-census ratchet:
   - `bash harness/memcheck_ratchet.sh` against the rebuilt daemon.
   - If `drops` ≤ 2196 and `undecided` = 0: pin advance is clean.
   - If `drops` = 2197: the +1 is still present — investigate which new
     compiler change introduced it (likely the G2 census reclassification or
     the Float32 literal path). This is the expected failure mode; document
     the specific drop site.
3. Run the soak: `bash harness/soak_rss_truth.sh` — verify the ~97 KB/call
   `detect_faces` leak is the ONLY remaining runtime leak (census zero ≠
   zero runtime leak; this is the P3 caveat).
4. Commit the pin advance in the kiosk repo.

**Gate for pin advance (both must pass):**
- Owners-census: `releases ≥ 73`, `undecided = 0`, `drops ≤ 2196`.
- Soak: RssAnon plateau (no linear climb) OR the leak is accounted for in
  the ratchet as a known, bounded per-call allocation.

**If the census still shows 2197:** do NOT force the pin. Document the exact
drop site, root-cause it, and advance only when the census is clean. The
pin's purpose is reproducible baselines — a regressed census defeats that.

---

## 3. Phase C — Follow-up program (W3-1 through W3-4, sequenced)

The trunk doc (`4ada76844`) scopes four work items. W3-1 (literal→Float) is
**already landed** in trunk (`85e5fb7ed`). W3-2 (#135/#134) is **already
landed** (`984ba5359`). So the remaining work is:

### C1. Verify W3-1 and W3-2 on the lane (immediate, ~10 min)

After Phase A sync, confirm the two landed fixes are present in the lane:
- `tests/fixtures/literal_float_coercion.bv` exists and passes.
- `tests/tier1/enum_payload_param_bind.bv` exists and passes.
- `scripts/g2_census.py` is present and runnable.
- `cargo test --lib` green (3943+ tests).

### C2. #134 re-test (small, ~15 min)

The follow-up doc §2 says #134's repro must be re-tested on the fixed build
before claiming the #135 connection. Steps:
1. Locate #134's repro (enum-held collection alias mutation, interpreter vs
   native).
2. Run it on the post-sync build.
3. If it passes: close #134 with a comment on the issue.
4. If it fails: document the divergence, determine if it's the same "payload
   types were free at construction" root or independent.

### C3. #137 — Float32 storage width (medium, ~2-4 h)

The doc §4 says: "after W3-1, re-check whether the Float32 storage-width gap
can ride the same declared-parameter machinery." Steps:
1. Read #137's repro and the #142 scoping note (Float32 needs storage-format
   metadata the Float atom lacks).
2. Check whether `85e5fb7ed`'s `adapt_arg_to_param` machinery (the
   `LlvmContext::state_prefix` pattern — one question, one owner) extends
   naturally to storage-width metadata, or whether a new metadata channel is
   needed.
3. If extendable: implement the Float32 path in the same helper.
4. If not: document the gap, leave #137 open with a concrete next step.

### C4. G2 census migration (large, mechanical, background)

The doc §3 scopes: (a) sweep 173 `term;` sites classifying exit vs
checkpoint, (b) convert exits to `term! ->`, (c) flip the diagnostic from
warning to error.

**Prerequisite:** `scripts/g2_census.py` (present after Phase A) classifies
each site as CHECKPOINT or TRAILING. Run it first to get the classification.

**Steps:**
1. `python3 scripts/g2_census.py -v` — get the full per-site list.
2. For each CHECKPOINT site: verify it's genuinely ambiguous (statements
   follow the guard). For each TRAILING site: the rewrite is a no-op.
3. Convert TRAILING sites: `term;` → `term! ->` (mechanical, no behavior
   change).
4. For CHECKPOINT sites: determine if the author meant an exit. If yes:
   rewrite to `term! ->`. If no (true checkpoint): leave as `term;`.
5. After the sweep reaches zero ambiguous CHECKPOINT sites: flip the
   diagnostic in the compiler from warning to error.
6. Run the corpus + gate to confirm no regressions.

**This is deliberately last** — it's wide, mechanical, and no user requests
depend on it. Do it in a separate session with its own plan doc.

### C5. Hygiene (trivial, any gap)

- Delete merged local branches: `pr136/138/139/140/142/143`,
  `fix/call-contract-fact-scope`, `pr141`.
- Keep `fuzz/Cargo.lock` uncommitted.
- Verify `docs/plans/INDEX.md` reflects the new state.

---

## 4. Phase D — PTL XPU lane (ongoing, no new work)

The P4 fix is verified and landed. No new XPU work is unblocked by the trunk
commits. The XPU lane continues its existing campaign (NEO/OpenCL/GLSL/XMX/
OpenVINO) per its own plans. The G0 gate remains the regression check for any
future codegen changes that touch the SPIR-V or accel paths.

**One note:** the trunk's `85e5fb7ed` (literal→Float sitofp) touches
`src/backend/llvm/emit_expr.rs` — the LLVM backend, not the SPIR-V backend.
No action needed for the XPU lane. If future XPU work touches the shared
emit path, re-run G0.

---

## 5. Order and effort

| Phase | Item | Effort | Risk |
|---|---|---|---|
| A | Trunk sync | 5 min | none |
| B | Kiosk pin advance | 30 min | low (census gate) |
| C1 | Verify W3-1/W3-2 landed | 10 min | none |
| C2 | #134 re-test | 15 min | low |
| C3 | #137 Float32 | 2-4 h | medium |
| C4 | G2 census migration | 4-8 h (separate session) | low per-step, wide |
| C5 | Hygiene | 10 min | none |
| D | PTL XPU | ongoing | none |

**Immediate:** A → B → C1 → C2 (all under 1 h total). C3 and C4 follow in
subsequent sessions.

---

## 6. What this plan does NOT do

- Does NOT re-tighten `ir_verify` to width-exact (doc §6: the corpus is
  clean, the literal-coercion design requires cross-width passage;
  re-tighten only if W3-1 ships with a full corpus pass + policy note).
- Does NOT assume #134 is the same root as #135 without running its repro
  (Rule: verify claims in source, not in similarity).
- Does NOT start the G2 sweep before the census script classifies all sites.
- Does NOT force the kiosk pin if the census regresses — the pin's purpose
  is reproducible baselines.
- Does NOT touch the macOS (#4) or C-static-lib (#2) work — external scope.
