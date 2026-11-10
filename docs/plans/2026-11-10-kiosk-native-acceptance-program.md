# The Kiosk Native Acceptance Program (2026-11-10)

**Status:** ACTIVE — top-level program.

**Unified program:** `docs/plans/2026-11-10-unified-program-capability-and-ceilings.md` — this plan is a detail of the one program (capability and ceiling meters + trust substrate).
**Mission:** make Briev capable of expressing and compiling the entire
`2d-Kiosk-Avatar` stack natively — service runtime, media, vision, ML graph
and kernels, transport, client — with **VITRIOL as the only sanctioned
external ML runtime (LLM inference)**, **ONNX as the model interchange
format**, and **OpenVINO as a temporary gateway that is retired over time**.
**Instrument:** a fork (`briev-kiosk`) that runs the original stack as a
differential oracle forever; every regression is a Briev defect, not an
exception.

> This plan is the external definition of done for the language. Compiler
> lanes (HIR, verification, lifetime, GPU, Intel, format facts, idioms) feed
> it; the kiosk gates them.

---

## 1. The proof statement

The program is complete when **all** of the following hold on the target
hardware:

1. The kiosk runs end-to-end (mic → VAD → STT → LLM → TTS → lip-sync →
   frames → WebRTC/client) from a single Briev-built deployment.
2. Every application/runtime/media/ML component is Briev source compiled by
   Briev. The only remaining external code is:
   - VITRIOL (LLM inference),
   - the declared OS/driver boundary (§2),
   - optionally an NPU vendor gateway if §14.2 decides it permanent.
3. The reference Python/PyTorch/OpenVINO/Rust implementations are **deleted
   from the runtime path**, not merely bypassed; the differential suite still
   passes.
4. Quality and latency are at parity or better on the locked SLO table
   (§10), including 25 fps frame pacing, stage budgets, barge-in, and the
   repo's own quality metrics (e.g. teacher parity 45.20 dB, sat/Lap).
5. The capability and retirement ledgers (§7, §11) are closed: every stack
   component has a native path and a gate; every FFI symbol has provenance
   and either a retirement record or a signed permanent-boundary decision.
6. `briev-kiosk` is the **official acceptance suite**: Briev PRs that break
   a scenario do not land.

A demo is not the proof; **removal** is.

---

## 2. Nativeness boundary (to be locked in ADR-000)

### 2.1 Native (must be Briev source, compiled by Briev)

Application logic, service runtime, concurrency/cancellation, config,
HTTP/WebSocket/MCP servers, media pipeline, DSP, image/vision operators,
codecs (unless §14.1 declares otherwise), WebRTC/transport logic, tensor
graph compiler and runtime, kernels, quantization, dynamic shapes, memory
management, client/renderer, tooling that is part of the runtime.

### 2.2 Sanctioned external boundary

| Boundary | Why | Status |
|---|---|---|
| **VITRIOL** | LLM inference is the one deliberately external runtime (llama.cpp class engine) | permanent, contract in §6/W10 |
| **OS/driver syscalls** | ALSA/PipeWire, V4L2/DRM, sockets, threads, signals, mmap, clocks, Vulkan/Level Zero loaders | permanent; each symbol listed with provenance |
| **ONNX** | model interchange format only; parsed and executed by Briev's tensor runtime | permanent format, no runtime |
| **OpenVINO** | interop gateway for models/stages not yet native (especially NPU), retired per op/model | temporary, ledger §11 |
| **LiveKit SFU** | separate server, not part of the kiosk binary; client protocol is native | boundary decision §14.3 |

### 2.3 Forbidden in the runtime path

Python, PyTorch, ONNX Runtime, OpenVINO graph execution once the stage is
native, Rust service/worker code once replaced, JS/WASM model runtimes.
Offline model conversion tooling may use external tools; the **runtime**
pipeline may not.

### 2.4 FFI provenance rule

Every FFI symbol carries: source, reason, consumer, whether it is platform or
third-party, retirement gate (or permanent decision), and the test that
covers it. No anonymous FFI. This is the proof-consumption discipline applied
to dependencies.

---

## 3. Why this stack (the reference oracle)

`Randozart/2d-Kiosk-Avatar` (private) — real-time conversational kiosk:

- **Pipeline:** audio capture → noise gate/resampler/VAD → STT (Whisper /
  faster-whisper) → LLM (TransformersLLM SmolLM2-360M or remote
  OpenAI-compatible) → TTS (Piper / StyleTTS2 / OpenVoice cloning /
  edge-tts) → visemes (Rhubarb) + MuseTalk/Ditto lip-sync inpainting →
  2D frames or 3D VRM → WebRTC (LiveKit) → React/Three.js or Bevy/wgpu.
- **Services:** Python ML daemon on a Unix socket (length-prefixed JSON-RPC;
  `configure`, `stt`, `llm`, `llm_stream`, `tts`, `tts_style`,
  `extract_visemes`, `muse_talk`, `sentiment`, `visual_*`, `stream_*`,
  `flashhead*`), Rust worker/HTTP API, config profiles
  (`ptl-kiosk.toml`, `entrance-host`, `expert-table`, `shop-assistant`),
  MCP/tools, hardware plugins.
- **Models/artifacts:** ElseUTalk/OpenVINO IR (`unet-vae-b4.xml`), Ditto
  student ONNX, Whisper, Piper/StyleTTS2/OpenVoice, SmolLM2/transformers,
  face detection, Silero VAD, RNNoise.
- **Hardware:** Panther Lake Core Ultra X7 358H + Arc B390 + AI Boost NPU
  (`/dev/accel/accel0`), 62 GB, CachyOS; CUDA path exists for other hosts.
- **Known baselines from the repo:** 25 fps live; 0.012 stage budget;
  15 LMDM steps ≈ 207 ms chunks; teacher parity 45.20 dB; OpenVINO
  ResNet-18 1188 FPS (Arc GPU) / 952 FPS (NPU); documented OpenVINO GPU
  plugin instability on B390 (2026-09); `DITTO_*` feature/plate-domain
  sensitivity is a quality gate, not a footnote.

---

## 4. Doctrine constraints (non-negotiable)

1. **Derivation, not recognition** (Rules 23/24): the compiler never
   recognizes models or algorithms. ONNX ops are general tensor ops;
   composites are declared in stdlib; structure-level idioms are gated
   separately (`derivation-not-recognition.md` §12).
2. **Facts and products**: hardware/format/capability facts live in config
   (`targets`, `ir-lowering`, format registry); lowerings are derived. No
   per-model knobs in the compiler.
3. **Proofs are consumed** (Rule 25): bounds, aliasing, ownership, lifetimes,
   single-writer, disjointness, quantization ranges — each proof names its
   runtime consumer (check removal, fast path) or records "no runtime
   consumer".
4. **Stdlib is the extension mechanism** (Rule 14): models, operators,
   pipelines, codecs live in `.bv`/stdlib/config, not compiler match arms.
5. **Expressiveness closure**: every optimum the compiler can reach must be
   expressible in the language; hand-written references are legitimate only
   with a retirement gate.
6. **Frontend-driven dispatch**: plans (tile/stage/quant/layout/device) are
   computed in the frontend and consumed by backends; tunables in config.
7. **Real-time**: latency/quality budgets are contracts with measurements,
   not aspirations; scheduling proofs where they pay.
8. **No silent fallback**: unsupported ONNX op, dtype, layout, device, or
   codec produces a precise diagnostic naming the missing capability.

---

## 5. Target architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│ Kiosk application plane (Briev)                                      │
│  turn state machine · barge-in · sessions/rooms · tools/MCP ·         │
│  profile/config hot-swap · avatar CRUD · HTTP/WS API · supervision   │
├──────────────────────────────────────────────────────────────────────┤
│ Service/runtime plane (Briev)                                        │
│  actors/channels · cancellation/priorities · timers · backpressure · │
│  TOML/env config · logging/tracing/metrics · packaging/OTA          │
├──────────────────────────────────────────────────────────────────────┤
│ Media/DSP/vision plane (Briev)                                       │
│  audio IO/resample/FFT/mel/VAD/Opus · camera/YUV/codecs ·            │
│  masks/matte/warp/LUT/NCC/putback · visemes · frame pacing          │
├──────────────────────────────────────────────────────────────────────┤
│ ML plane (Briev compiler + runtime)                                  │
│  ONNX frontend → tensor IR → optimizer → quantizer → memory planner  │
│  → device scheduler → kernel library → reference interpreter         │
├──────────────────────────────────────────────────────────────────────┤
│ Device plane                                                         │
│  CPU (LLVM/SIMD) · GPU (SPIR-V/PTX, coopmat/XMX) · NPU gateway       │
│  (OpenVINO → native if feasible) · VITRIOL (LLM only)               │
└──────────────────────────────────────────────────────────────────────┘
   Boundary: OS/driver syscalls + VITRIOL + ONNX (format) + temporary OV
```

Every plane is independently testable; the kiosk only composes them.

---

## 6. Workstreams

Workstreams are permanent ownership areas. Each has a design doc, a
capability ledger slice, and acceptance gates. Dependencies are explicit.

### W0 — Acceptance program, fork, and ledgers

**Mission:** the instrument that makes "native" measurable.

- Fork `briev-kiosk` (new private repo): `reference/` (pinned submodule of
  2d-Kiosk-Avatar), `briev/`, `harness/`, `models/`, `ci/`, `docs/`.
- Differential runner: reference vs candidate, fixed seeds/clocks, captured
  RPC/WS traffic, golden artifacts, tolerance classes.
- Scenario corpus (§11), SLO table (§10), fault injection, soak.
- Capability ledger (§7) and retirement ledger (§11).
- CI: always-on PTL runner + CPU-only runner; nightly full; trend dashboards.
- Upstream sync of Briev; reference never edited.

**Gate:** one-command differential run (reference vs itself) stable; baseline
SLOs locked; ledgers published.

### W1 — Service/runtime plane

**Mission:** Briev becomes a first-class service language.

Requirements:
- Actors/tasks with structured concurrency, channels, cancellation tokens,
  priorities, deadlines/timers, bounded queues/backpressure, supervision,
  graceful shutdown.
- Servers: Unix sockets (length-prefixed framing), TCP/UDP, HTTP/1.1+2,
  WebSocket (client+server), MCP (stdio/HTTP), TLS.
- Config: TOML profiles, env precedence, hot reload; JSON schemas/typed
  messages; logging/tracing/metrics; health/readiness.
- Process management (spawn/supervise Piper/edge-tts/VITRIOL), signals,
  file/mmap IO, clocks.
- Deployment: single binary + declared external libs, service units, OTA.

**Language features forced:** actor/channel/cancellation primitives,
priority/deadline-aware scheduler, server framework in stdlib, TOML,
schema-typed JSON, tracing.

**Gate:** S1 daemon implements the full RPC surface with delegated ML;
protocol conformance; ≤5% overhead vs reference on the scenario suite;
barge-in/cancellation semantics exact.

### W2 — Data, config, tooling

**Mission:** the means of production are Briev-owned.

- TOML, schema-typed JSON, dynamic dispatch/registry (engine selection),
  collections/strings ergonomics, error/optional ergonomics.
- Package/model manager (ONNX/GGUF manifests, versions, checksums), model
  cache and offline conversion pipelines (external tools allowed offline).
- Profiler/tracer, debugger, sanitizers, fuzzing harness, reproducible builds,
  cross-compilation, hot reload.
- ADRs and diagnostics for every unsupported surface.

**Gate:** the kiosk builds, packages, profiles, and debugs without external
build tooling beyond the compiler/toolchain; model manifest validated at
startup with actionable errors.

### W3 — Tensor/ML compiler and runtime (the capability frontier)

**Mission:** run ONNX models on Briev's own graph compiler/runtime.

Components:
1. **Tensor type system**: shape, dtype (f32/f16/bf16/int8/int4/int32),
   layout/stride, memory space, symbolic dims, quantization metadata
   (scale/zero-point, per-tensor/channel/group), bounds/aliasing facts.
2. **Graph IR**: ops, attributes, constants, dynamic dims, QDQ, control-flow
   (loop/if/scan), shape inference; canonicalization.
3. **ONNX frontend**: opset subset with a coverage ledger; initializers as
   weight artifacts; dtype/layout mapping; explicit unsupported diagnostics.
4. **Optimizer**: fusion (conv+act, norm+act, attention, dequant+matmul,
   requantize), layout propagation (NCHW/NHWC), constant folding, dead-code
   elimination, memory-aware scheduling.
5. **Quantizer**: int8/int4 recipes, calibration format, per-channel/group,
   fallback rules, accuracy gates.
6. **Memory planner**: arenas, weight mmap/streaming, activation reuse, KV
   cache, zero-copy host/device, ownership/lifetime proofs.
7. **Device scheduler**: shape/quant/layout → device/plan selection from
   target facts; batching, async pipelines, residency, multi-device.
8. **Reference interpreter**: exact CPU semantics (f32/f64 reference) for
   parity and fuzzing.
9. **Kernel library**: declared composites/general ops (see W4).
10. **Numerics**: tolerance classes per op/dtype, determinism, cross-device
    parity, quality metrics.

**Model ladder (first native proof):** Silero VAD or face detector →
ResNet-18 (perf gate vs 1188/952 FPS) → Ditto student → MuseTalk UNet+VAE →
Whisper small → Piper → StyleTTS2/OpenVoice → sentiment/vision extras.
LLM via VITRIOL (W10).

**Gate:** each model: import → interpreter parity → op coverage → device
parity → quality metrics → latency budget; unsupported op = diagnostic, no
silent fallback. No ONNX Runtime/PyTorch in the runtime path.

### W4 — Kernels and device plane

**Mission:** native kernels that reach device ceilings.

- CPU: SIMD kernels (f32/f16/bf16/int8/int4), threading, NUMA, cache blocking.
- GPU: SPIR-V (Vulkan/OpenCL flavors), PTX, cooperative matrices/XMX,
  async pipelines, local memory/periodic fills, vec4/v4half, occupancy.
- NPU: gateway contract; first via OpenVINO, native only if §14.2 says so.
- Quantized kernels: int8 dot/dp4a, int4 unpack/dot, dequant/requant fusion.
- Primitives: GEMM/GEMV, attention/SDPA, conv/dw/conv-transpose, pooling,
  norms, activations, softmax, RoPE, embedding, sampling, upsample, resize,
  FFT/mel, NMS, image filters.
- Strategy facts: measured tile/stage/load/coopmat choices in config; no
  per-model branches; frontend plans, backend consumes.

**Gate:** per-op/device performance vs the lane ceilings (recorded harness),
quality parity, spirv-val/PTX validation, determinism; f16/int8 rows at or
above the declared reference policies.

### W5 — Audio, DSP, and media

**Mission:** own the audio/video path.

- Audio IO (ALSA/PipeWire boundary), PCM conversions, resampler, noise gate,
  AGC, ring buffers, WAV, Opus (decode/encode) if native.
- FFT/mel/STFT, feature extraction for Whisper/Piper/StyleTTS2, VAD (Silero
  native model via W3, or native DSP VAD), barge-in pre-roll.
- Camera capture (V4L2), YUV/colour conversions, JPEG/PNG/WebP, video codecs
  (H.264/VP8/AV1) if native per §14.1, decoding/encoding, frame pacing,
  A/V sync.
- Codec conformance and robustness (corrupt streams, timestamps, reordering).

**Gate:** per-stage latency/quality parity; frame pacing; sync metrics;
memory budgets; corruption/fuzz robustness.

### W6 — Vision and avatar pipeline

**Mission:** native MuseTalk/Ditto-grade rendering and analysis.

- Face detection/bboxes, tracking, crop/align, masks/matte, subject matte,
  warp, LUT/colour transfer, NCC sharpener, putback, compositing.
- MuseTalk UNet+VAE / Ditto student inference through W3 (no per-model
  special cases; declared composites where applicable).
- Viseme extraction (Rhubarb-equivalent), viseme/emotional state machine,
  expression blending, idle variants, 2D frames and 3D VRM.
- Quality metrics: teacher parity dB, sat/Lap, PSNR/SSIM, temporal
  stability, mouth-motion; plate/background matrix (the reference's known
  domain sensitivity).

**Gate:** visual and metric parity per plate/avatar; 25 fps; GPU/NPU budgets;
retire Python pre/post and model stages as covered.

### W7 — Transport and WebRTC

**Mission:** native real-time transport.

- TCP/UDP/Unix, TLS, HTTP/1.1+2, WebSocket; RTP/RTCP, SRTP/DTLS, ICE,
  jitter buffer, congestion control (GCC/REMB), data channels.
- LiveKit protocol compatibility (client side); SFU remains external per
  §14.3.
- A/V sync, packet loss/jitter resilience, reconnection, barge-in data path.

**Gate:** native client joins the same rooms; packet-loss/jitter scenarios
within SLO; no Rust transport code in the path (or a signed boundary
decision).

### W8 — Kiosk application plane

**Mission:** all kiosk logic in Briev.

- Turn state machine: VAD/STT/LLM/TTS/lipsync sequencing, barge-in,
  interruption, timeouts, retries, tool calls, media commands.
- Sessions/rooms, avatar CRUD, profiles/hot swap, emotion/viseme/voice
  styles, sentiment, hardware tools, MCP federation.
- HTTP API, static hosting, supervision, health, metrics, logs.
- Config compatibility with the reference profiles and env vars.

**Gate:** protocol conformance; scenario parity; session/reconnect/soak;
zero Rust/Python service code in the path.

### W9 — Native client and renderer

**Mission:** the kiosk client is Briev too (scope decision §14.4).

- Native wgpu/Bevy-equivalent client: windowing, GPU renderer, VRM/2D
  compositing, audio playback, WebRTC client, input.
- Browser React UI retained unless §14.4 says rewrite; the browser is not a
  runtime requirement for the kiosk.

**Gate:** visual/audio parity, frame budget, input latency, crash recovery.

### W10 — VITRIOL boundary (the one external runtime)

**Mission:** a clean, tested LLM boundary that never leaks into the language.

- Contract: model format (GGUF), tokenizer/sampling ownership (§14.5),
  KV/session, streaming tokens, cancellation/interruption, batching, device
  selection (CUDA/Vulkan), process or library ABI, error/timeout semantics.
- Performance: prefill/decode latency at kiosk shapes; barge-in cancel;
  concurrent sessions; memory residency.
- Isolation: crash/restart without taking down the kiosk; version pinning.

**Gate:** turn-latency and barge-in SLOs; fault injection; no VITRIOL types in
Briev's public tensor/service API beyond the declared boundary.

### W11 — Real-time, performance, power

**Mission:** low latency is a first-class contract.

- Budgets per stage and E2E; frame pacing; scheduling priorities/deadlines;
  WCET analysis where feasible; deterministic numerics; clock/jitter.
- Zero-copy, residency, async pipelines, multi-device scheduling, NUMA,
  model load/startup, memory ceilings.
- Perf/W facts in target config; thermal guard; power states.
- Measurement discipline: same-commit correctness; validated timestamps;
  no folded/degenerate runs; P50/P95/P99 + spread.

**Gate:** SLO table (§10) green on the hardware matrix; soak stable; power
within budget.

### W12 — Verification, release, operations

**Mission:** trust at the scale of a product.

- Differential scenarios, golden artifacts, fuzzing, sanitizers, ASan/LSan,
  Kani for safety-critical code, determinism gates.
- Hardware matrix: PTL CPU/iGPU/NPU, CPU-only; later other hosts if in scope.
- Release: single binary/packaging, versioning, OTA, rollback, telemetry opt-in.
- Incident playbooks for model/driver failures.

**Gate:** acceptance suite is release-blocking; every capability has a
regression; no quarantined test without an owner+expiry.

---

## 7. Capability ledger

Each row gets a slice doc and a gate. Status legend: **native**, **partial**,
**absent**, **boundary**.

| # | Component | Reference | Native target | Status | Workstream |
|---|---|---|---|---|---|
| 1 | Unix RPC/JSON/streaming | Python daemon | Briev service | absent (pieces exist) | W1 |
| 2 | HTTP/WS/MCP servers | axum/Rust | Briev stdlib | absent | W1/W2 |
| 3 | Config/profiles/env | TOML/Python | Briev TOML/config | absent | W2 |
| 4 | Supervision/hot reload | systemd/Make | Briev runtime | absent | W1 |
| 5 | Audio IO/resample/DSP | Python/NumPy | Briev kernels | absent | W5 |
| 6 | VAD | WebRTC/Silero | Briev model/DSP | absent | W3/W5 |
| 7 | STT | Whisper/OV | Briev tensor runtime | absent | W3 |
| 8 | LLM | Transformers/remote | **VITRIOL** | boundary | W10 |
| 9 | TTS | Piper/StyleTTS2/OpenVoice | Briev tensor runtime | absent | W3 |
| 10 | Visemes | Rhubarb | Briev DSP/model | absent | W5/W6 |
| 11 | Lip-sync | MuseTalk/Ditto/OV | Briev tensor runtime + kernels | absent | W3/W6 |
| 12 | Face detection | Python/OV | Briev tensor runtime | absent | W3/W6 |
| 13 | Image pre/post | Python/OpenCV | Briev kernels | absent | W5/W6 |
| 14 | JPEG/PNG | OpenCV/stb | Briev/stb boundary | partial (decode) | W5 |
| 15 | Video codecs | H.264/VP8 | Briev (or boundary §14.1) | boundary/absent | W5/W7 |
| 16 | WebRTC | LiveKit client | Briev transport | absent | W7 |
| 17 | Sessions/rooms/tools | Rust/Python | Briev app | absent | W8 |
| 18 | Client/renderer | React/Bevy/wgpu | Briev client | absent (scope §14.4) | W9 |
| 19 | Tensor graph/ONNX | PyTorch/OV | Briev compiler+runtime | absent | W3 |
| 20 | Quantization | PyTorch/OV | Briev quantizer+kernels | absent | W3/W4 |
| 21 | GPU kernels | OV/PTX | Briev SPIR-V/PTX | partial | W4 |
| 22 | NPU | OpenVINO | gateway → native? | boundary | W4 |
| 23 | Real-time scheduling | ad-hoc | Briev contracts/scheduler | absent | W11 |
| 24 | Tooling/profiling | Python/Rust | Briev toolchain | partial | W2 |
| 25 | Acceptance suite | pytest/Rust tests | `briev-kiosk` harness | new | W0/W12 |

The ledger is updated in the fork; this table is the map.

---

## 8. Stage roadmap

No dates. Each stage is a gate; stages may overlap where dependencies allow.

### S0 — Acceptance program (W0)
Fork, reference pinned, harness, baselines, SLO table, ledgers, CI.
**Exit:** self-differential stable; reference runs on PTL; scenario corpus and
artifact tolerances locked.

### S1 — Service spine (W1, W2)
Briev daemon: RPC, config, lifecycle, streaming, cancellation, priorities,
supervision; ML delegated at the boundary.
**Exit:** full protocol conformance; E2E through Briev; ≤5% overhead;
barge-in/timeout parity.

### S2 — Tensor core v0 (W3, W4, W12)
Tensor types/IR, ONNX import, reference interpreter, CPU kernels for the
first op set, memory planner; tiny model → ResNet-18.
**Exit:** interpreter/device parity; op coverage ledger; no ONNX Runtime.

### S3 — Native media/pre-post (W5, W6)
Audio DSP, visemes, masks/matte/warp/LUT/NCC/putback, JPEG paths, frame
pacing.
**Exit:** stage parity/budgets; Python pre/post retired.

### S4 — Device + quantization (W4, W3)
GPU kernels, coopmat/XMX, int8/int4, dynamic shapes/KV, weight streaming,
NPU gateway; Ditto/MuseTalk native.
**Exit:** model parity; OV retired for covered stages; device performance
gates.

### S5 — Native transport (W7, W5)
TLS, HTTP/WS servers, WebRTC/RTP/SRTP/DTLS/ICE/jitter/CC, Opus/video (or
signed boundary), capture, A/V sync.
**Exit:** native client joins rooms; loss/jitter scenarios green.

### S6 — Native services/application (W8)
HTTP API, MCP, sessions/rooms, tools, avatar CRUD, profiles, supervision.
**Exit:** full backend Briev except VITRIOL/SFU boundary; scenario parity.

### S7 — Native ML breadth (W3/W4/W6)
Whisper, Piper, StyleTTS2/OpenVoice, sentiment, face detection, full
MuseTalk UNet+VAE; tokenizers/mel; OpenVINO retired except the declared NPU
gateway (or fully, §14.2).
**Exit:** full `ptl-kiosk` profile native; SLOs green.

### S8 — Native client + hardening (W9, W11, W12)
Native kiosk client/renderer (per §14.4), real-time proofs, zero-copy,
perf/W, soak, OTA; suite becomes release-blocking.
**Exit:** proof statement §1 satisfied; program moves to maintenance.

---

## 9. Model acceptance ladder

For every model (order in W3):

1. **Import**: ONNX parse, shape inference, dtype/layout mapping, QDQ.
2. **Reference**: Briev interpreter vs external reference (ONNX Runtime/OV/
   PyTorch is allowed **as an oracle only**) on fixed inputs; cosine/abs/rel
   tolerances.
3. **Op coverage**: every op in the graph supported and tested; unsupported =
   diagnostic, never a silent fallback.
4. **Device**: CPU, GPU, NPU-gateway parity; determinism; quantized variants.
5. **Quality**: model-specific metrics (WER for STT; MOS/UTMOS for TTS;
   teacher-parity dB, sat/Lap, PSNR/SSIM for lip-sync; mAP for detection).
6. **Latency**: per-stage budget under the kiosk pipeline; frame pacing.
7. **Soak/faults**: hours-long run, model reload, device reset, OOM.

The coverage ledger is per opset/op/dtype/device and lives in the fork.

---

## 10. SLO framework

Locked in S0 from measured reference data; no invented numbers.

| Metric | Where | Target rule |
|---|---|---|
| Stage latency P50/P95/P99 | every stage | ≤ reference (or better) after parity |
| E2E turn latency | utterance end → first frame+audio | ≤ reference; barge-in deadline |
| Frame pacing | lip-sync | 25 fps continuous; no drops |
| Audio/AV sync | output | within reference tolerance |
| Quality | per model | parity per §9 metrics and plate/background matrix |
| Startup/model load | kiosk boot | ≤ reference; warm path faster |
| Memory | RSS/VRAM/model size | within budgets; no leaks |
| Power | PTL package | within declared budget (perf/W facts) |
| Reliability | soak/faults | no crash/leak; recovery within budget |

Known reference anchors to preserve/beat: 25 fps, 0.012 stage budget,
15 LMDM steps ≈ 207 ms chunks, 45.20 dB teacher parity, OpenVINO ResNet-18
1188/952 FPS as the first perf gate.

Measurement discipline: same commit, same geometry, reference-verified;
validated device timestamps; no folded/degenerate workloads; P50/P95/P99 and
spread recorded; every timing enters a results file only with correctness.

---

## 11. Differential acceptance harness

- **Traffic replay**: captured Unix RPC and browser WS sessions, byte-level
  framing, streaming frames, errors, disconnects.
- **Scenario corpus**: greeting; multi-turn; barge-in/interrupt; language
  switch; tool calls/media; emotion/viseme/voice-style changes; long session;
  reconnect; model reload; profile hot-swap; degraded network; GPU reset;
  OOM; missing/corrupt model; clock drift; 8-hour soak.
- **Golden artifacts**: PCM/WAV, transcripts, tokens, JPEG/frames, visemes,
  bboxes, embeddings; tolerance classes per artifact.
- **Fault injection** and **resource limits** (memory, CPU, GPU/NPU, disk).
- **Reference/candidate runner**: fixed seeds/clocks, isolated processes,
  interleaved timing, both stacks on the same box.
- **Ledgers**: capability (§7) + retirement (per FFI and per external runtime:
  OpenVINO stages, Python stages, Rust services, codecs if boundary).
- **CI**: PTL hardware runner always-on; CPU-only runner for portability;
  nightly full suite; PR gate runs the fast scenario subset + ledgers check.

---

## 12. Integration with existing Briev plans

| Existing plan | Role in the program | Gate |
|---|---|---|
| H5 completion next leg / H5 retirement | compiler trust, typed HIR for analyses/lowering | S1/S2 prerequisites |
| Compiler verification program P1–P7 | differential/fuzz/snapshot trust | S0/S2 gates |
| Stability v0.1 | release discipline, interpreter parity | S1 base |
| Lifetime & reclamation, arenas, destination ownership | tensor/media memory, real-time | S2–S5 |
| Enum boundary-gate removal / ws_frame proofs | ownership correctness in services/media | S1/S5 |
| Proof completeness + proof-consumption ledger | guard removal in hot paths | S2–S5 |
| Eternal ABI | FFI/VITRIOL/OV boundary correctness | S1/S4 |
| JSON/wasm runtime closure | service/data plane | S1 |
| Float formats (facts/products) | f16/bf16 basis for ML | S2/S4 |
| Intel lane ceilings / G1.5 Stage F | GPU/NPU performance lane | S2/S4 |
| GPU fill-pipeline / attention targets | kernel ceilings | S4 |
| Idiom doctrine W5 / general workstream | structure-level gating, folds | S2–S4 |
| Cross-dialect interop 2b/3 | if hardware/embedded variants join | later |
| Independent app lanes (recommender, Otto, release) | orthogonal; parallel | — |

Foreign lanes (`feat/e14a-intent-synthesis`, `feat/bad-dialect`) remain
untouched.

---

## 13. Risks and mitigations

1. **WebRTC/media codec explosion** — decide native vs platform boundary
   early (§14.1); gate with conformance corpora; keep FFI only with signed
   decisions.
2. **Model quality parity** (plate/background sensitivity, TTS cloning) —
   parity matrix per model/domain; oracle-vs-native artifacts; no "close
   enough" without metrics.
3. **NPU without OpenVINO** — vendor compiler reality; declare the gateway
   or plan a native NPU backend; do not hide it.
4. **Tensor runtime scope** — op coverage grows; keep the reference
   interpreter and diagnostics; gate per model, not per ambition.
5. **Real-time proofs** — measure first; formalize WCET/preemption only
   where budgets demand; never trade correctness for proof theater.
6. **VITRIOL coupling** — ABI pinning, crash isolation, barge-in tests.
7. **Two-stack maintenance** — reference must stay runnable; retirement
   ledger keeps pressure on.
8. **Hardware/driver instability** — B390 OV GPU note; L0/Vulkan lanes are
   assets; fault injection in CI.
9. **Opportunity cost** — the program is the top-level priority; independent
   app lanes continue in parallel but do not redefine scope.

---

## 14. Decisions to lock

1. **Platform boundary**: native Opus/H.264/VP8/AV1 + full WebRTC, or
   permanent FFI with native logic above? (Recommendation: attempt native,
   allow a signed FFI decision for codecs/transport if it becomes the long
   pole; document either way.)
2. **NPU**: OpenVINO as permanent NPU gateway, or native NPU lowering
   required? (Recommendation: gateway first, native only with a vendor
   compiler plan.)
3. **LiveKit**: SFU stays external; client protocol native. Confirm.
4. **Client scope**: native kiosk client in scope (S8); browser UI retained
   as-is. Confirm.
5. **VITRIOL contract**: tokenizer/sampling ownership, KV/session, streaming,
   process vs library ABI.
6. **ONNX tooling**: offline conversion scripts may be external; runtime must
   be Briev. Confirm.
7. **Acceptance ownership**: `briev-kiosk` gates Briev PRs. Confirm.
8. **SLO table**: lock values in S0; define abort/fallback policy if a stage
   cannot meet budget.

---

## 15. First ten steps (S0 execution)

1. Create the private `briev-kiosk` repo; pin the reference as a submodule.
2. Add `harness/` skeleton: scenario manifest, RPC capture/replay, artifact
   diff, tolerance classes.
3. Run the reference on PTL; record stage/E2E latency, fps, quality, memory,
   power; lock the SLO table.
4. Publish the capability and retirement ledgers (§7/§11) in the fork.
5. Stand up the PTL CI runner and the differential job.
6. Write ADR-000 (nativeness boundary + FFI provenance) and ADR-001
   (VITRIOL contract).
7. Write the tensor IR/ONNX frontend design doc (W3) and the service runtime
   design doc (W1).
8. Start S1: minimal daemon (configure/stream_init/get_frame) with delegated
   ML, E2E through Briev.
9. Start S2: ONNX import + reference interpreter + CPU kernels for the first
   tiny model (Silero VAD or face detector).
10. Set the first perf gate: ResNet-18 parity vs the reference's
    1188/952 FPS, then the Ditto/MuseTalk ladder.

---

## Appendix A — Reference RPC surface (from the private repo)

`configure`; `stt`; `llm`; `llm_stream` (raw streamed frames); `tts`;
`tts_style`; `extract_visemes`; `muse_talk`; `sentiment`;
`visual_set_emotion`; `visual_set_viseme`; `visual_get_frame`;
`visual_reset`; `stream_init`; `stream_prepare_audio`; `stream_get_frame`;
`flashhead_init`; `flashhead_prepare_audio`; `flashhead_get_frame`;
`flashhead_composite_init`; `flashhead_composite_prepare_audio`;
`flashhead_composite_get_frame`.

Framing: 4-byte big-endian length + JSON `{method, params}`; streaming
methods write length-prefixed frames on the same connection; binary frames
for JPEG.

## Appendix B — Reference config/models (summary)

Env: `KIOSK_ML_BACKEND` (cuda/openvino/said/none), `KIOSK_STT_ENGINE`,
`KIOSK_TTS_ENGINE`, `KIOSK_LLM_ENGINE`, `KIOSK_MODELS_DIR`,
`KIOSK_LLM_MODEL`, `KIOSK_LLM_BASE_URL`, `KIOSK_LLM_API_KEY`,
`ML_SOCKET_PATH`, `OV_DEVICE`, `DITTO_*`.

Profiles: `ptl-kiosk` (PTL: whisper cpu, musetalk, piper, transformers,
OpenVINO), `entrance-host`, `expert-table`, `shop-assistant`.

Models: faster-whisper; TransformersLLM (SmolLM2-360M default) or
OpenAI-compatible; Piper/StyleTTS2/OpenVoice/edge-tts; MuseTalk OpenVINO IR
(`unet-vae-b4.xml`), Ditto student ONNX; face detection; Silero VAD; RNNoise;
Rhubarb visemes.

## Appendix C — Briev assets to build on

Contracts/proofs and proof consumption; reactor/execution model;
`async`/`sync<g>`; typed HIR; stdlib sockets/poll/threads/processes/signals,
JSON/HTTP/WebSocket, arena, dynlib, stb_image; SPIR-V/PTX/LLVM backends;
Vulkan/L0/NEO lanes with residency, native f16, coopmat; format facts; declared
composites (`matmul!`, `dot!`, `softmax_rows!`); OpenVINO FFI lane; freestanding
bare-metal path; determinism/verification gates.

## Appendix D — Glossary

- **Oracle**: the pinned reference stack; never edited.
- **Boundary**: sanctioned external code (VITRIOL, OS/driver, ONNX format,
  temporary OV).
- **Native**: Briev source compiled by Briev; external implementation deleted.
- **Retirement gate**: parity+quality+latency proof that allows deleting a
  dependency.
- **Capability ledger**: stack component → native status → gate → tests.
- **Model ladder**: ordered model ports used as escalating capability tests.
