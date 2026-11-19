# Trunk sync (F1–F4 + slicing fix) + 2D Kiosk Avatar assessment

**2026-11-19.** Follows `2026-11-19-post-p4-workstream.md`. The pin at
`44c67df98` is already at the kiosk's `origin/main`; the question is whether the
compiler lane should pull in 5 further trunk commits, and whether the 2D Kiosk
Avatar repo needs any compiler pin work.

---

## 0. Findings (evidence, not recall)

### 0.1 Trunk is 5 commits ahead of the lane

`origin/main` is at `c76f13c34`, 5 commits ahead of the lane tip `44c67df98`:

| commit | subject | source files touched (non-docs/tests) |
|---|---|---|
| `79aaa1a1b` | feat(census): feature census maps SPEC sections to fixture tags (F1) | `scripts/feature_census.py`, tier1 `.bv` tag additions |
| `ae69c2b4f` | feat(stdlib): header-conformance gate + SYS_SETRLIMIT fix (F4) | `lib/std/posix/syscall.bv`, `tests/stdlib_header_conformance.rs` |
| `dbdbedaa6` | feat(smoke): tier1 golden-output convention (F3) | `tests/tier1/*.golden` (new files) |
| `69cd99c18` | fix(slicing): heap-List slice lowering + exclusive-stop strided gather | `src/backend/llvm/emit_expr.rs`, `lib/std/cast_lanes.bv`, `lib/std/cast_portable.bv`, `BUGS.md` |
| `c76f13c34` | Merge (fix/divergence-willreturn into main) | — |

### 0.2 Overlap with the XPU lane: minimal

`git diff --name-only HEAD...origin/main` intersects the lane's XPU surface
(`src/backend/spirv/`, `src/accel_rt.rs`, `lib/runtime/`) in **zero files**.
The only shared files are:
- `BUGS.md` — additive entry at the top (the slicing fix record); git
  auto-resolves (same pattern as both prior syncs this session).
- `tests/tier1/accel_lane_report.bv` — a one-line `spec: 9.7` tag addition
  (F1 census tagging); purely additive.

The slicing fix (`69cd99c18`) touches `emit_expr.rs` — the LLVM backend, NOT
the SPIR-V backend. The G0/PTL gates remain the regression check for any future
change that reaches the shared emit path; the slicing change is host-side only
(heap-List slice lowering), so no PTL re-run is strictly required for it.

### 0.3 The 2D Kiosk Avatar is a separate repo, not a pin consumer

`/home/randozart/Desktop/Projects/2d-Kiosk-Avatar` (origin
`Randozart/2d-Kiosk-Avatar`) is a frontend/server/voice/ML repo
(`frontend/`, `server/`, `avatar-manager/`, `muse-stream/`, …). It has:
- **no `compiler.lock`** — it does not pin the Briev compiler;
- **zero `.bv` files** — no Briev source to compile;
- its local `main` == `origin/main` (0 ahead / 0 behind), clean tree.

So the "2D Kiosk Avatar upstream" work the user did is already pushed and is
orthogonal to the compiler pin. The **only** pin consumer is
`briev-kiosk-avatar` (`briev/service/daemon.bv`), which is already at the
post-sync pin `44c67df98` (kiosk commit `5564f1f`).

**Conclusion:** "pulling in the latest" = sync the 5 trunk commits into
`intel-xpu-harvest`, re-verify, land. No 2D-avatar repo action is needed.
The kiosk pin question (advance to the new tip?) is a follow-up: the new
commits are census/golden/slicing/stdlib work that does not change the daemon's
census shape (the W4 `pf_exp_digits` drop is already baselined at 2197), so the
pin can advance *after* the sync with no ratchet change expected.

---

## 1. Plan

### Step A — Sync trunk into the lane

1. `bash scripts/sync-main.sh` (commits any pending plan doc first if the
   script refuses — as it did this session).
2. Verify the merge: `BUGS.md` and `accel_lane_report.bv` auto-resolve;
   confirm no manual conflict.
3. `bash scripts/gate.sh quick` — must be PASS on the merged tree.
4. Land: `git fetch origin main --quiet && git merge-base --is-ancestor
   origin/main HEAD && git push origin HEAD:main && git push origin
   intel-xpu-harvest`.

### Step B — Kiosk pin advance to the new tip (optional follow-up)

1. Re-run the census ratchet against the new lane tip:
   `brievc memcheck briev/service/daemon.bv` — expect **2197** (unchanged; the
   F1–F4 + slicing commits don't touch the daemon's ownership shape).
2. If 2197: update `compiler.lock` to the new lane tip, commit + push the kiosk
   pin. If the census moves (e.g. the slicing fix or the syscall change alters
   a stdlib path the daemon imports): bisect as before and document before
   advancing.
3. `harness/native_smoke.sh` green → pin lands.

### Step C — 2D Kiosk Avatar: nothing to do

- No `compiler.lock`, no `.bv` files → no pin to advance.
- Confirm `origin/main` is pushed (it is: local == origin, clean tree).
- No compiler work needed for the 2D avatar unless it later ships `.bv`
  sources (which would then need a pin of its own).

---

## 2. Gates / acceptance

- `scripts/gate.sh quick` PASS on the merged tree.
- Kiosk ratchet: `releases >= 73`, `undecided = 0`, `drops <= 2197`.
- Kiosk `native_smoke.sh` all scenarios green (health, proxy, sentiment, faces).
- No new diagnostics in the XPU-lane files (the sync touches none).
- The 5 trunk commits' own tests (`feature_census.py`, `stdlib_header_conformance.rs`,
  tier1 `.golden` sweep, slicing `slice_basics.bv`) pass under the quick gate.

## 3. What this does NOT do

- Does NOT touch the 2D Kiosk Avatar repo (separate, no pin, no `.bv`).
- Does NOT re-run the PTL G0 gate for the slicing fix (host-side LLVM; no SPIR-V
  surface touched) — G0 remains the regression net for future SPIR-V changes.
- Does NOT force the kiosk pin if the census regresses — bisect and document
  first, as with the 2196→2197 W4 drop.
