# The canary is the surface — pulling in the 2D Kiosk Avatar work

**2026-11-19.** Reframes `2026-11-19-trunk-sync-2d-avatar.md` (which covered the
compiler-side sync; that still stands and is step 1). The user's point:
`briev-kiosk-avatar` exists to prove the *2D Kiosk Avatar* stack builds in
Briev — the daemon is the canary, and the canary's validity is the RPC surface
it mirrors. Upstream (`2d-Kiosk-Avatar`) moved ~150 commits since the daemon
was forked; the surface the daemon claims conformance to has **grown under it**.

---

## 0. Evidence (fresh, not recall)

### 0.1 The upstream method table (current `main`, `da5b576`)

`server/server-ml/main.py::_HANDLERS` (29 entries) + `_video_methods` (14,
routed through `_BACKEND`) + `_STREAM_HANDLERS` (1):

```
configure, stt, tts, extract_visemes, tts_style, llm, llm_stream (streaming),
muse_talk, process_avatar, cache_vae, said_process, render_transition,
visual_set_emotion, visual_set_viseme, visual_get_frame, visual_reset,
get_capabilities, sentiment,
flashhead_init, flashhead_prepare_audio, flashhead_get_frame,
flashhead_composite_init, flashhead_composite_prepare_audio,
flashhead_composite_get_frame,
lipsync_offline, lipsync_status
```

### 0.2 The daemon's current surface (`briev/service/daemon.bv`, 742 lines)

Implements: `health`, `echo`, `sentiment` (native engine, W1/S2 edge),
`detect_faces` (native, via yunet), `configure` / `configure_native`,
`vad_open`/`vad_chunks`/`vad_feed`/`vad_close`. Unknown methods return an
explicit `{"error":"unknown method: …"}` — **that is correct design** (the
surface is added method-by-method; the comment at `daemon.bv:158` says so).

### 0.3 The gap is structural, not incidental

The daemon was forked at S1 (service spine; "ML delegated at the boundary").
The recent upstream wave — the 2026-11-12/13 mouth/voice/live commits —
*changed the shape of the avatar pipeline* (flashhead compositor, grouped
mouth pilot, stream_get_frame tuple returns `(dict, bytes|None)`, new
`lipsync_offline`/`lipsync_status`, `render_transition`). None of those
methods are in the daemon. The S-stage roadmap maps this exactly: S1 done,
S2 (tensor core v0) is next, and the surface additions are the *leading edge*
of S2–S6.

**What "pulling in the latest" therefore means:** not a compiler pin, but
**re-baselining the canary's conformance surface** against upstream `main`,
so the differential oracle (the program's instrument, `docs/program.md` §3)
measures against the *current* protocol — and so the next S-stage work is
scoped against real method shapes, not stale ones.

---

## 1. Plan

### Step 1 — Compiler lane sync (carried over, ~10 min)

The 5 trunk commits (F1–F4 + slicing fix) still need landing — they're cheap
and independent:
1. `bash scripts/sync-main.sh` (commit the plan docs first if it refuses).
2. `bash scripts/gate.sh quick` → PASS.
3. Land: `git fetch origin main --quiet && git merge-base --is-ancestor
   origin/main HEAD && git push origin HEAD:main && git push origin
   intel-xpu-harvest`.

### Step 2 — Re-baseline the canary surface (the actual ask, ~1 h)

Deliverable: a committed surface ledger in `briev-kiosk-avatar` that is the
*single source of truth* for "which upstream methods does the daemon claim,
and what does each require."

1. **Snapshot the upstream table.** Script
   (`harness/surface_census.py` or extend `run_scenario.py`) that parses
   `server/server-ml/main.py` (`_HANDLERS`, `_video_methods`,
   `_STREAM_HANDLERS`) + `get_capabilities` and writes
   `harness/surface-baseline.toml`: method name, params, return shape,
   streaming-or-not, engine key. Run against upstream `da5b576` and commit the
   baseline. This is the differential oracle's input — the same role
   `memcheck-baseline.txt` plays for the census.
2. **Classify the daemon's coverage.** Extend the census into a report:
   - IMPLEMENTED (daemon has a handler + scenario): health, echo, sentiment,
     detect_faces, configure, vad_*;
   - DELEGATED (by S1 design — forwarded/placeholder): none currently;
   - ABSENT: the remaining ~28 methods, tagged by workstream (S2 stt/tts/llm,
     S3 visemes, S6 muse_talk/flashhead/lipsync, …).
   - **PROTOCOL-DRIFT watchlist:** methods whose upstream shape changed since
     the fork (e.g. `stream_get_frame` now returns a `(dict, bytes)` tuple —
     if the daemon ever implements it, that shape is the one to mirror;
     `visual_get_frame` response keys).
3. **Ratchet.** The baseline file is versioned like the census baseline:
   upstream may add methods (the ABSENT list may grow), but a method marked
   IMPLEMENTED may not silently drop or drift — a scenario must fail if the
   daemon's response shape stops matching the baseline. This keeps the canary
   honest as upstream keeps moving.

### Step 3 — Kiosk pin advance to the synced lane tip (~15 min)

Same gates as before: census ratchet (expect 2197 — the F1–F4/slicing commits
don't touch daemon ownership shape), `native_smoke.sh` green, then advance
`compiler.lock` to the new lane tip + commit + push.

### Step 4 — Scope the next S-stage from the ledger (doc-only, in the kiosk repo)

Write `docs/plans/<date>-canary-s2-surface.md` in `briev-kiosk-avatar`:
the first 2–3 ABSENT methods to implement, chosen by dependency order
(`stt` first — it is the pipeline head and the smallest native engine after
sentiment), each with its upstream request/response shape quoted from the
baseline. The XPU lane's accel surface (W4) feeds `detect_faces`/
`muse_talk`-class tensor methods — that is where the XPU campaign and the
canary intersect, and the ledger makes that intersection explicit instead of
implicit.

---

## 2. Gates / acceptance

- Lane: `gate.sh quick` PASS; landed to `main` + `intel-xpu-harvest`.
- Kiosk: `surface-baseline.toml` committed, generated from upstream `da5b576`;
  coverage report (IMPLEMENTED/ABSENT/DRIFT) reviewed and committed;
  ratchet + `native_smoke.sh` green; pin advanced.
- The ledger must be *regenerable*: re-running the census against upstream
  `main` must reproduce the file (modulo the drift delta), so a future
  "pull in the latest" is one command + a review of the diff.

## 3. What this does NOT do

- Does NOT implement the absent methods now — that is S2–S6 work, scoped in
  Step 4, executed in later sessions with their own plans.
- Does NOT touch `2d-Kiosk-Avatar` (read-only reference; the oracle runs the
  reference stack, it does not modify it).
- Does NOT force the pin or the baseline on a regression — bisect and
  document first (the 2196→2197 precedent).
