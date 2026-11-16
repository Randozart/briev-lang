# Kiosk soak truth — raw-frame ownership leak (2026-11-16)

Plan: `docs/plans/2026-11-16-memory-and-xpu-finish-grind.md` §M1.
Supersedes the open question in
`2026-11-14-post-reland-hygiene-soak-tailloop.md` §B1 ("explain 159→188 MB
@300"). Pre-fix numbers are those recorded by the census campaign; post-fix
numbers are measured here.

## 1. Geometry and command

| Item | Value |
|---|---|
| Frame | `/tmp/soak-frame.bin`, **4 915 200 bytes** = 640×640×3 **f32** (1 228 800 floats) |
| RPC | `detect_faces`, param `frame_path` (raw binary frame, not JPEG) |
| Socket | `/tmp/briev-kiosk-native.sock` |
| Model | `models/onnx/yunet/yunet.onnx` |
| Harness | `briev-kiosk/harness/soak.py` (persistent client, one connection) |
| Calls | 500 |
| Correctness gate | `detect_faces` returned `count: 0`, `error: None` (no crash, no frame parse failure) |

Command (as run):

```
ML_SOCKET_PATH=/tmp/briev-kiosk-native.sock \
FACE_MODEL=<kiosk>/models/onnx/yunet/yunet.onnx \
  <build>/daemon &
python3 harness/soak.py --socket /tmp/briev-kiosk-native.sock \
  --frame /tmp/soak-frame.bin --param frame_path --pid <pid> \
  --n 500 --sample-every 50
```

## 2. Pre-fix (the leak) — as recorded by the census campaign

The frame `String` returned by `read_file` inside `load_frame_bin` was never
released, so the leak scaled with input size:

| Input | Growth |
|---|---|
| raw 640×640 f32 (4.9 MB) | **+4.07 MB/call** |
| raw 320×320 f32 (1.2 MB) | **+1.29 MB/call** |
| JPEG frame | +0.13 MB/call |

A 500-call run at the 640×640 geometry would therefore add ~2 GB. That is the
basis for the "159.5 MB → 2191 MB" figure in the campaign record.

## 3. Post-fix — measured, flat

Compiler state: walker-free owned-storage pass (the form committed as
`00fcd5e90`; the soak binary was built one step earlier with the dead
helpers still present — behaviourally identical, and **re-verified at the
pinned commit in §5**).

### 3.1 500 calls, sample every 50 (walker removed)

```
calls	vmrss_mb	delta_from_first
1	1.9	+0.0
50	1.9	+0.0
100	1.7	-0.1
150	1.7	-0.1
200	1.7	-0.1
250	1.6	-0.3
300	1.6	-0.3
350	1.6	-0.3
400	1.6	-0.3
450	1.6	-0.3
500	1.6	-0.3
```

### 3.2 500 calls, sample every 10 (earlier run, HIR-walker build)

```
1 → 2.1 MB, 10–20 → 2.1, 30–230 → 1.7, 240–500 → 1.5
```

Both runs are **flat from the first sample**; the only variation is the
allocator releasing pages as the process settles (−0.3 to −0.6 MB). There is
no per-call slope. The stair-step that the 2026-11-14 plan could not explain
(159→188 MB @300) does not reproduce at this geometry and commit.

## 4. IR evidence (the mechanism, not just the outcome)

`load_frame_bin` frees the `read_file` String inside its own body:

```llvm
foreach.end40:
  %t56 = ptrtoint ptr %t29 to i64
  %t57 = inttoptr i64 %t56 to ptr
  call void @free(ptr %t57)
```

`%t29` is the frame String: `%t0 = call i64 @read_file(...)` then
`%t29 = call ptr @unwrap$String$Int(i64 %t0)`. The free targets exactly the
`unwrap` of the `read_file` Ok payload — the buffer that used to leak.
Verified with `sed`/`grep` on the emitted `.ll` for the same build that was
soaked.

## 5. Honest caveats

1. **The HIR escape walker is not the fix, and removing it is not what made
   the leak disappear.** The fix is (a) `fresh_changed` recomputing `ok0`
   when `fresh` grows, and (b) nested-call attribution in `use_arg`. The
   walker was removed because it is unsound (non-monotone cross-pass
   summaries — see plan §0.1), not because it did not work.
2. **The double-free was noticed once and then not reproduced.** The 500-call
   run on the walker build (§3.2) completed cleanly; a later rebuild of the
   same source aborted with `free(): invalid pointer` within the first calls.
   Both observations are consistent with a double-free that only aborts when
   the allocator detects it (heap layout dependent). The walker's removal
   stands on the soundness argument, not on the crash.
3. **VRSS, not RSS.** `soak.py` samples `VmRSS`; a page freed by the process
   but retained by the allocator still counts. A flat VmRSS therefore proves
   "no unbounded growth", not "every allocation returns to the OS".
4. The 1.5–1.9 MB figures are the daemon's own footprint at this build; they
   are not comparable to the 144–190 MB figures in earlier campaign entries,
   which were measured on a different daemon build (model cache primed,
   request arena open, larger parent RSS).

## 6. Follow-up

The authoritative re-measurement at the pinned commit (with all kiosk gates
green on the same tree) is recorded in the kiosk repo
(`docs/progress.md`, entry for the pin bump). This file fixes the *mechanism*
and the *shape* of the curve; the pinned-commit entry fixes the *current*
number.
