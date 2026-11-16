# Kiosk soak truth — raw-frame ownership leak (2026-11-16)

Plan: `docs/plans/2026-11-16-memory-and-xpu-finish-grind.md` §M1.
Supersedes the open question in
`2026-11-14-post-reland-hygiene-soak-tailloop.md` §B1 ("explain 159→188 MB
@300"). Pre-fix numbers are those recorded by the census campaign; post-fix
numbers are measured here at the pinned commit.

## 1. Geometry and command

| Item | Value |
|---|---|
| Frame | `/tmp/soak-frame.bin`, **4 915 200 bytes** = 640×640×3 **f32** (1 228 800 floats) |
| RPC | `detect_faces`, param `frame_path` (raw binary frame, not JPEG) |
| Socket | `/tmp/briev-kiosk-native.sock` |
| Model | `models/onnx/yunet/yunet.onnx` |
| Harness | `briev-kiosk/harness/soak.py` (persistent client, one connection) |
| Compiler | the kiosk pin `4aed72809` (`compiler.lock`), built locally |
| Daemon | `brievc build briev/service/daemon.bv` → `/tmp/pin-build/daemon` |
| Calls | 500 |
| Correctness gate | every `detect_faces` returned `error: None`; `native_smoke.sh` on the same tree gave `face rpc: raw=0, jpeg=2 ok` |

```
ML_SOCKET_PATH=/tmp/briev-kiosk-native.sock \
FACE_MODEL=<kiosk>/models/onnx/yunet/yunet.onnx \
  <build>/daemon &
python3 harness/soak.py --socket /tmp/briev-kiosk-native.sock \
  --frame /tmp/soak-frame.bin --param frame_path \
  --pid "$(pgrep -x daemon)" --n 500 --sample-every 50
```

## 2. Pre-fix (the leak) — as recorded by the census campaign

The frame `String` returned by `read_file` inside `load_frame_bin` was never
released, so the leak scaled with input size:

| Input | Growth |
|---|---|
| raw 640×640 f32 (4.9 MB) | **+4.07 MB/call** |
| raw 320×320 f32 (1.2 MB) | **+1.29 MB/call** |
| JPEG frame | +0.13 MB/call |

A 500-call run at the 640×640 geometry would add ~2 GB, which is the
"159.5 MB → 2191 MB" figure in the campaign record.

## 3. Post-fix — measured at the pinned commit (authoritative)

```
soak: target pid=3572837 comm=daemon rss=5.1MB      # hardened harness header
calls	vmrss_mb	delta_from_first
1	150.1	+0.0
50	183.0	+33.0
100	183.0	+33.0
150	183.0	+33.0
200	183.0	+33.0
250	183.0	+33.0
300	187.7	+37.7
350	185.9	+35.9
400	185.9	+35.9
450	183.2	+33.2
500	185.7	+35.6
```

Reading:

- **+33 MB is one-time staging**, complete by call 50: model load, parsed
  face-graph cache, request arena open. This is the shape the campaign
  described as "steps then flattens".
- **Calls 50 → 500 are flat within ±2.5 MB of allocator noise.** Net drift
  over 450 calls is **+2.7 MB ≈ +0.006 MB/call (6 KB/call)**, and it is not
  monotone (183.0 → 187.7 → 183.2 → 185.7 — it goes down as well as up).
  Against the pre-fix **+4.07 MB/call** that is a ~700× reduction; more
  usefully, it is indistinguishable from zero at this sample size.
- Verdict: **no per-call leak.** The residual movement is glibc arena
  behaviour, not ownership.

### 3.1 The earlier "flat 1.9 MB" curves were invalid — and why

Two earlier runs in this session recorded `1.9 MB → 1.6 MB` and
`2.1 MB → 1.5 MB`. Those numbers are impossible for a process that has run
`detect_faces` on a 4.9 MB frame with an ONNX model: the daemon is ~5 MB
before the first call and ~145 MB after it. They were sampling **the wrong
process** (a dead pid whose number had been reused, or the launching shell).

Root cause in the harness: `sample_vmrss_mb` returned **0.0** for a dead pid,
and the loop accepted it. A wrong pid therefore rendered as a perfectly
flat, tiny, leak-free curve — a false negative indistinguishable from a pass.

Fixed in `harness/soak.py` (2026-11-16): `require_live_daemon` now asserts
the pid exists, that `/proc/<pid>/comm` equals `--expect-comm` (default
`daemon`), and that VmRSS is readable — and the sample loop aborts if the pid
disappears mid-run instead of printing zeros. Verified with two negative
tests (dead pid → "is not running — refusing to report a curve"; live bash
pid → "is 'bash', expected 'daemon'").

**Consequence for the record:** the raw-frame fix was, until this run,
supported only by the IR evidence in §4 — the soak had never actually
measured the daemon. It is now measured, and the leak is gone.

## 4. IR evidence (the mechanism, not just the outcome)

`load_frame_bin` frees the frame String inside its own body, in the pinned
build:

```llvm
foreach.end40:
  %t56 = ptrtoint ptr %t29 to i64
  %t57 = inttoptr i64 %t56 to ptr
  call void @free(ptr %t57)
```

`%t29` is exactly the buffer that used to leak:

```llvm
%t0  = call i64 @read_file(ptr %state, ptr %t2)
%t29 = call ptr @unwrap$String$Int(i64 %t0)      ; <- freed at foreach.end40
%t47 = call float @f32_from_bytes(ptr %state, ptr %t29, i64 %t49)
```

Verified with `awk`/`grep` over `/tmp/pin-build/daemon.ll`.

## 5. Which change actually fixes it

Not the HIR escape walker (retired as unsound — plan §0.1). The fix is:

1. `fresh_changed` in `fresh_callables` — `ok0` was a stale snapshot taken
   when `fresh` was empty, so a newly-fresh allocator (`tensor_new`) never
   unlocked its ok-fresh deduction in later passes.
2. Nested-call attribution in `use_arg` — `tensor_set(t, i, f32_from_bytes(text, 4*i))`
   attributed `text` to `tensor_set`; a nested expression must be analyzed by
   its own callee.

## 6. Other honest caveats

1. **VmRSS, not RSS.** A page freed by the process but retained by the
   allocator still counts. A flat VmRSS proves "no unbounded growth", not
   "every allocation returns to the OS".
2. **The double-free was observed once and not reproduced.** A 500-call run
   on the walker build completed cleanly; a later rebuild of the same source
   aborted with `free(): invalid pointer` early. Both are consistent with a
   double free that only aborts when the allocator detects it. The walker's
   removal rests on the soundness argument, not on the crash.
3. **Geometry.** 6 KB/call at 4.9 MB input is ~0.12% of the input per call;
   a leak of that shape would need >8000 calls to show a trend above the
   noise. If a later campaign needs a tighter bound, sample RSS after
   `malloc_trim` or count malloc/free in `coll_arena.c`.
