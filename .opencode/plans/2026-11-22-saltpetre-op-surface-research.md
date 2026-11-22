# Saltpetre op surface + GPU retirement ledger — RESEARCH (2026-11-22)

Owner direction: **saltpetre is the compat lib now; `.abv` is the destination it
retires toward.** Do not pick a lane; adopt a per-op retirement ledger as the
standing mechanism. This note is the evidence for the plan that follows.

## 1. What saltpetre actually is (measured, not assumed)

`lib/saltpetre/` = 2,563 lines, 8 modules:

| file | lines | role |
|---|---|---|
| `exec.bv` | 1,268 | **the op surface** — conv1d/conv2d/gemm/maxpool/batchnorm/relu/sigmoid/tanh/sqrt/reshape/flatten/slice/gather/concat/pad/split/quantize/dequantize/binary |
| `onnx.bv` | 521 | ONNX **protobuf parser** (no compute) |
| `tensor.bv` | 205 | tensor alloc/get/set/copy + arena, `Tensor` type |
| `session.bv` | 165 | arena lifecycle (`sp_begin`/`sp_open_coll`/`sp_request_reset`) |
| `yunet.bv` | 130 | model-specific (YUNet post-processing) |
| `audio.bv` | 129 | mel/fft path |
| `frame.bv` | 83 | frame decode |
| `proto.bv` | 62 | protobuf reader |

So saltpetre is **already a tensor + ONNX-inference library** — 80 % of the
"our PyTorch" description. What it is *not*: autograd, dynamic shapes, or any
GPU/vendor path.

**The dispatch point**: `run_node` (`exec.bv` ~1010–1229) is a
`when node.op_type == "..."` chain, ~33 arms. That chain is legitimate here —
a library parsing ONNX must dispatch on op names (the *compiler* matching op
names is what Rule 23 forbids). It is exactly where per-op backend routing
belongs, and today every arm routes to the scalar CPU implementations above.

## 2. What exists on the vendor/GPU side

| surface | state | granularity |
|---|---|---|
| `lib/std/ai/openvino.bv` | **exists**, 372 lines, 24 `frgn … from #Link<openvino_c>` | **model-level** (ov_model → compiled → infer request) |
| `lib/std/gpu.bv` | exists, 46 lines, SPIR-V builtins (`get_global_id`, `barrier`) via `#`-intrinsics | kernel-level |
| `lib/std/numeric.bv` | `matmul!`, `matmul_f16!`, `softmax_rows!` `$defn` macros with `attr declared_kind:` | op-level, **Briev-emitted** |
| `.abv` tier | 10 files, `benchmarks/intel_ptl/ceiling/` (GEMM/element/reduce f16+f32 ceilings) | whole program |
| `async accel node` **in `.bv`** | proven by `tests/tier1/accel_lane_report.bv` | node-level, usable *from a stdlib module* |
| vendor device drivers | `lib/runtime/briev_dev_{cuda,vulkan,opencl,levelzero}.c` + `src/accel_rt.rs` | below the language |
| **op-level vendor bindings** | **NONE** — no cuBLAS / cuDNN / oneDNN `.bv` anywhere | — |

The last row is the load-bearing gap: **there is no op-level vendor compat layer
to be "the compat lib" today.** OpenVINO is model-level. cuBLAS/cuDNN/oneDNN
would have to be written.

Consequence — delegation granularity decides which vendor is even usable:

- **op-level** delegation needs an op-level API: cuBLAS (Gemm), cuDNN (Conv),
  oneDNN/MKL-DNN (CPU Gemm+Conv). None bound yet.
- **model-level** delegation needs a model-level API: OpenVINO (bound already),
  ONNX Runtime (not bound).

These are two different ledgers keyed on different things (op vs model). The
plan must name which one it starts with.

## 3. Doctrine: does this violate anything?

No — *provided the boundary is stated*. The two relevant guardrails are both
about the **compiler**, not libraries:

- `abv-gpu-doctrine.md` §1.3: "The compiler's answer is never 'call cuBLAS' and
  never 'run nvcc'. **FFI lanes (`frgn`, `#Link<>`) exist for users; they are not
  the codegen path.**"
- `2026-11-05-intel-xpu-openvino-harvest.md` guardrail 1: "The vendor runtime
  NEVER enters Briev's kernel path… OpenVINO is a benchmark competitor and **an
  FFI capability escape hatch, nothing more.**"

Both forbid the *compiler* delegating. Saltpetre is a library. `openvino.bv`
already exists under exactly that reading. So saltpetre routing an op to a
vendor lib is legal **library-level** delegation — the escape hatch — and it
stays legal only if each such arm carries a **retirement condition**. An FFI arm
with no retirement condition is the thing the doctrine actually forbids.

That is the whole design: the escape hatch is legitimate *because it is
temporary and tracked*, not because it is fast.

## 4. Existing gates this work must not weaken

| gate | what it pins |
|---|---|
| `tests/saltpetre_conformance.rs` | 6-fixture corpus (relu, sign, mlp, conv, conv_ext, tail) vs **ORT-generated goldens, bit-for-bit** |
| `tests/openvino_gate.rs` | OpenVINO binding content + handle release ordering |
| `daily_use_smoke::accel_declaration_is_never_dropped_silently` | an `accel` body is never silently CPU-fallen-back; `--deny-accel-cpu-fallback` must refuse |
| `tests/idiom_ledger_gate.rs` | **the mechanism to imitate**: textual scan of matcher names; a matcher without a ledger row fails the build |
| `benchmarks/deferred_ab_gate.sh` | variant-diff A/B (two arms, element-wise compare) |
| `benchmarks/compare_baseline.sh` | locked-baseline A/B |

House rule that governs every number: timing enters results only with a
same-commit, same-geometry correctness gate; kernel addressing changes need the
on-device check at a real shape.

## 5. What is genuinely missing (the work)

1. **No provenance on the op surface.** `run_node`'s arms carry no statement of
   which backend serves the op, so "how much of saltpetre is Briev-native vs
   delegated" is unanswerable today.
2. **No ledger.** Nothing tracks per-op retirement; nothing fails when an op is
   delegated forever.
3. **No op-level vendor bindings** (if the owner wants op-level delegation).
4. **No model-level benchmark.** No yunet/onnx entry in `benchmarks/`; the
   conformance corpus is correctness-only, tiny, and has no timing arm.
5. **Generic tensor/float helpers still live in saltpetre** rather than
   `lib/std`, so an op cannot be re-pointed at `.abv` by a one-line dispatch.

## 6. Proposed shape (for the confirm gate)

Three phases, each independently shippable:

- **P1 — op-surface consolidation.** Extract generic tensor/float/cast helpers
  from `lib/saltpetre/{tensor,exec}.bv` into `lib/std`; make `run_node` a
  single per-op routing table (`op → backend`), not 33 inline arms; add a
  machine-readable **provenance table** (`lib/saltpetre/op-backends.dbvl`) with
  one row per op: `{op, backend: briev|ffi(<lib>)|model(<lib>), graduation}`.
  Keep `saltpetre_conformance` green bit-for-bit. **No behavior change.**
- **P2 — the ledger gate.** A Rust test (imitating `idiom_ledger_gate.rs`) that
  fails when: an op is dispatched but has no provenance row; a row declares
  `ffi`/`model` with an empty `graduation`; or a row declares `briev` while the
  arm still calls an FFI symbol. Plus record the census count ("N of M ops
  Briev-native") in the test output so the number is always visible.
- **P3 — the first graduation (device work).** A/B one op that already has a
  Briev-emitted channel — `Gemm` (`matmul!`/`matmul_f16!` exist) — on the
  measured geometry, with the same-geometry correctness gate, on PTL. Flip the
  row, delete the delegated arm, add the bench.

P1+P2 are repo-local and can land in one session. P3 needs the PTL node and is
its own session.

**Open decision for the owner**: the ledger's key. Op-level (cuBLAS/cuDNN/
oneDNN — needs new bindings) or model-level (OpenVINO — already bound)? The
plan starts model-level if the answer is "ship the compat lib now", op-level if
the answer is "graduate ops one at a time". The P1/P2 mechanism is identical
either way, which is why it is safe to build first.

## Sources

Read in this session: `lib/saltpetre/{exec,tensor,onnx,session}.bv`,
`lib/std/ai/openvino.bv`, `lib/std/gpu.bv`, `lib/std/numeric.bv` (matmul
macros), `tests/tier1/accel_lane_report.bv`, `tests/saltpetre_conformance.rs`,
`tests/idiom_ledger_gate.rs`, `benchmarks/deferred_ab_gate.sh`,
`docs/architecture/abv-gpu-doctrine.md` §1,
`docs/architecture/briev-capability-frontier.md`,
`docs/plans/2026-11-05-intel-xpu-openvino-harvest.md` §1 guardrails.
