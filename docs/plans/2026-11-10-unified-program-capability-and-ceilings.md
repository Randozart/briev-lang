# Unified program: capability and ceilings (2026-11-10)

**Status:** ACTIVE — supersedes the *scheduling* role of the individual active
plans below while keeping each as the detailed record.
**Thesis:** there is one program, not two. The kiosk acceptance suite is the
**external capability meter**; the Intel/GPU lane ceilings and the float-format
seam are the **internal performance meter**; H5/verification/stability,
lifetime/arenas/proofs are the **trust substrate** both depend on. A capability
without a measured ceiling is unproven; a ceiling without a capability consumer
is unverified.

## Plan family (all referenced, none superseded technically)

| Role | Plan |
|---|---|
| Capability / acceptance (outer) | `docs/plans/2026-11-10-kiosk-native-acceptance-program.md` |
| Performance / device ceilings | `docs/plans/2026-11-09-intel-lane-ceilings-and-probe-cost.md` |
| Numeric substrate (types/dtypes) | `docs/plans/2026-11-09-float-formats-facts-and-products.md` |
| Trust: HIR completion | `docs/plans/2026-11-09-h5-completion-next-leg.md` |
| Trust: verification | `docs/plans/2026-11-09-compiler-verification-program.md` |
| Trust: release discipline | `docs/plans/2026-10-25-stability-v0.1.md` |
| Trust: ownership/memory | `docs/plans/2026-11-03-lifetime-and-reclamation.md`, `2026-11-03-arena-enablement.md`, `2026-11-03-arena-runtime-parity-and-folding.md`, `2026-10-29-destination-ownership-reconciliation.md` |
| Proof consumption | `docs/plans/2026-10-26-proof-consumption-ledger.md`, `2026-10-26-proof-completeness-and-guard-elimination.md` |
| ABI / interop | `docs/plans/2026-10-27-eternal-abi-and-temporal-transport.md` |
| Intel lane | `docs/plans/2026-11-08-intel-g1.5-opencl-flavored-spirv.md` |
| Doctrine | `docs/architecture/derivation-not-recognition.md`, `docs/architecture/proof-vs-shape.md`, `docs/plans/2026-11-06-idiom-recognition-doctrine.md`, `docs/plans/2026-11-08-general-workstream-countable-body-and-folds.md` |
| Independent | `docs/plans/2026-10-26-recommender-app-acceptance-and-upstream.md`, `docs/plans/2026-10-25-otto-contribution-adoption.md` |

## 1. One program, two meters

```
          ┌─────────────────────────────────────────────────────┐
 external │  Kiosk acceptance suite (S0–S8)                     │
  meter   │  protocol · models · media · latency · quality      │
          └───────────────▲─────────────────────────┬───────────┘
                          │ consumes                │ exposes gaps
          ┌───────────────┴─────────────────────────▼───────────┐
 internal │  Ceilings & lanes: probe cost, f16 microkernel,     │
  meter   │  coopmat facts, NEO residency, GLSL/XMX ceilings,   │
          │  Stage F/G promotions                              │
          └───────────────▲─────────────────────────┬───────────┘
                          │ derives                  │ requires dtypes
          ┌───────────────┴─────────────────────────▼───────────┐
 numeric  │  Float formats: facts (bits/exp/mantissa), products │
 substrate│  (LLVM/SPIR-V/PTX), f16/bf16/int8, quant metadata    │
          └───────────────▲─────────────────────────┬───────────┘
                          │ registers types          │ consumes
          ┌───────────────┴─────────────────────────▼───────────┐
 trust    │  H5 · verification P1–P7 · stability v0.1 ·        │
          │  lifetime/arenas/destination · proofs · eternal ABI │
          └─────────────────────────────────────────────────────┘
```

- The kiosk is the **only** external proof of capability. It gates Briev PRs.
- The ceiling harness (`benchmarks/intel_ptl/ceiling/`, probe-gate IR,
  per-device matrix) is the **only** accepted evidence of performance.
- Float formats are the shared type/ABI substrate: kiosk ONNX dtypes and GPU
  kernel widths are the same facts.
- The trust substrate gates every commit; it is not a parallel ambition.

## 2. The extension rule (both directions)

1. **Ceiling → capability.** Every lane/format change must name the kiosk
   stage that consumes it, or record an explicit `no kiosk consumer` entry
   (mirror of the proof-consumption ledger). A faster kernel nobody uses is
   unverified, not a win.
2. **Capability → compiler.** Every kiosk gap that is not application logic
   becomes a compiler/stdlib/lane work item with a reproducible gate in the
   kiosk harness. The kiosk is the bug-finding instrument; the owning lane
   carries the fix and the regression.
3. **No third meter.** We do not invent ad-hoc benchmarks. The kiosk SLO table
   and the lane ceiling matrix are two views of the same result schema:
   `{shape, device, dtype, quality, device_time, wall, reference, gate}`.
4. **No comfort proxy.** The byte-faithful upstream proxy is scaffolding; each
   stage it fronts is retired per the kiosk retirement ledger, model by model,
   stage by stage. A proxy stage cannot be marked "native".

## 3. Work classes and the single critical path

There is one critical path at a time. Everything else is one of three classes.

**A. Critical path — kiosk S2 tensor core (until the first native model).**
ONNX import → tensor IR → reference interpreter → CPU kernels → first tiny
model (Silero VAD or face detector) → ResNet-18 perf gate. **S1 breadth is
frozen** after `configure`/`sentiment`: no more service methods for their own
sake.

**B. In-band — compiler/stdlib bugs and verification.** Any bug the kiosk
exposes (String ownership, UTF-8, search contracts, framing, timers, service
runtime) is fixed immediately, with a regression in the owning lane. H5 and
verification stay green per commit; they are not paused, only not *expanded*
beyond what the critical path needs.

**C. Dependency-gated — lanes and numeric work.** Resume only when a kiosk
stage/SLO names the dependency:

| Work | Resume when |
|---|---|
| Float P1 fact seam (`config/float-formats.dbvl`) | S2 needs a dtype/layout the hardcoded widths do not cover, or quantization starts |
| Float P2 self-describing formats | a model declares a non-standard format |
| f16 tiled microkernel / coopmat strategy facts | a kiosk model misses its device SLO in f16 |
| NEO/OpenCL residency | a kiosk stage's wall time is the copy, not the kernel |
| GLSL/XMX ceiling harness | the kiosk needs a true Vulkan/NPU ceiling, not the CL-C reference |
| Intel G1.5 Stage F promotion / G2 / G3 | a kiosk stage runs on L0/NEO or needs GEMM-on-IL |
| H5 completion / stability release | after the first native model, as the scheduled pause point (the kiosk will then find bugs faster than ad-hoc fixes can absorb) |

**D. Independent lanes** (recommender/Otto) run only when A is blocked on
hardware/memory, never as a third equal priority.

**Stop/go gate:** do not start C work while A has an unblocked next step.
Do not pause A for a C item unless the item is on A's measured path.

## 4. Unified ledger

Extend the kiosk capability ledger instead of duplicating it. Each row gains:

| Column | Meaning |
|---|---|
| capability | kiosk component (existing) |
| internal dependency | lane/format/compiler work it needs (plan + item) |
| external gate | parity/quality/latency scenario |
| device gate | lane ceiling reference / device budget |
| consumer check | which kiosk stage consumes the internal work (`none` allowed with reason) |
| retirement | FFI/Python/OV deletion state |
| status | absent/partial/native/boundary |

The lane results files answer the `device gate` and `consumer check` columns.
The float-format plan answers `internal dependency` for dtypes. The trust plans
are prerequisites, not rows.

## 5. Unified roadmap

**U0 — landed (record).**
Kiosk S0; S1 service spine + sentiment + configure; lane W1 probe gate, W2a
ceilings, W2b OpenCL C references, W2d matrix; float P0 (Half canonical,
base-chain widths, native f16 on NEO); L0 residency; Vulkan timestamp fix;
the `.rbv` progress page; CI harness + PTL runner (environment-limited).

**U1 — S2 entry + substrate (next).**
1. Lock edge SLOs: peak RSS, cold start, model load, binary size, offline,
   int8 accuracy (targets measured on a constrained cgroup, not the 62 GB box).
2. Choose and capture the first model (Silero VAD or face detector) as ONNX;
   write the tensor-IR + ONNX-subset design; reference interpreter on CPU.
3. Float P1 seam only to the extent the first model's dtypes/layouts require.
4. Keep verification/stability gates green; no H5 expansion yet.

**U2 — first native model + its ceiling.**
Python/OpenVINO deleted for that model; parity + edge budget + ResNet-18
perf gate; f16 microkernel/coopmat/residency only if the device SLO demands
it; lane result names this consumer.

**U3 — quantization + breadth.**
int8/int4 QDQ, per-channel/group, fused requant; conv/dw/norm/attention;
S2/S3 kiosk stages; GLSL/XMX ceilings where the model needs a true ceiling.

**U4 — model ladder + promotions.**
Ditto/MuseTalk student, Whisper, Piper/TTS clones; Stage F/G promotions;
edge budgets on device; OV retired per covered stage (NPU gateway decision
recorded).

**U5 — transport/app/client + release.**
S5–S7; H5 completion/stability as the scheduled pause point; the full kiosk
suite becomes release-blocking.

## 6. Unified measurement

One methodology, two views:

- Same commit, same geometry, reference-verified; validated device timestamps;
  no folded/degenerate workloads.
- Every result row: shape, device, dtype, quality, device time, wall, ceiling
  reference, gate, kiosk consumer.
- Kiosk SLO table = external view; lane matrix = internal view. A stage is not
  accepted if either view is missing.
- Edge budgets are first-class: RSS, startup, size, offline, quant accuracy —
  measured under a cgroup memory limit (the PTL OOM at ~10 GB is the warning
  that a shared 62 GB box is not an edge proxy).

## 7. Edge-readiness gates (explicit)

We may say "edge-ready" for a stage only when:

1. it runs native (Python/OV deleted for that stage) on the target device;
2. peak RSS, cold start, model load, and binary size are inside the locked
   edge budgets, measured under the constrained runner;
3. int8 (later int4) accuracy is within the declared tolerance;
4. it works offline and survives model/driver fault injection;
5. the measurement row names its lane ceiling and consumer check.

Until then, the honest statement is "acceptance infrastructure + service
spine", not "edge ML".

## 8. Doctrine and retirement (unchanged, binding)

Derivation not recognition; facts and products; proof consumption; no
vocabulary matching; no per-model match arms; stdlib extension; one sanctioned
external LLM runtime (VITRIOL); ONNX as format; OpenVINO interop→retire; every
FFI symbol has provenance and a retirement gate; every hand-written reference
has a retirement gate.

## 9. Current state and blockers

- Done: U0 list above; `cargo test --lib` 3571 green; the kiosk page live.
- Open: S2 tensor core (nothing native ML yet), float P1/P2, lane C items,
  H5 completion, stability release.
- Environmental: the PTL box is at 60+ GB used by other workloads; the CI
  runner was OOM-killed repeatedly. The cached Briev build and the
  `MIN_AVAIL_GB` skip are mitigations, not the fix; edge budgets must be
  measured on a constrained, dedicated runner.

## 10. Immediate actions

1. Register this plan at the top of `docs/plans/INDEX.md` as the umbrella; add
   a one-line pointer at the top of the kiosk, lane-ceiling, and float-format
   plans.
2. Add the unified columns in §4 to the kiosk capability ledger.
3. Freeze S1 breadth (no new service methods beyond `configure`/`sentiment`).
4. Lock edge SLOs and start U1: first model ONNX + tensor-IR design.
5. Only then resume C items that the first model actually names.

## 11. Decisions to confirm

1. First native model: **Silero VAD** (tiny, audio, used for barge-in) or
   **face detector** (vision, feeds MuseTalk). Recommendation: VAD first.
2. Edge budgets: RSS/startup/size targets and the constrained-runner size
   (e.g. 4 GB cgroup). Needed before U1 acceptance.
3. Float P1 seam before or after the first model? Recommendation: after — use
   the model's actual dtypes as the first fact row.
4. H5/stability pause point: after the first native model (recommended), not
   now.
