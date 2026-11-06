# Intel PTL / OpenVINO benchmark lane — first pass (2026-11-06)

**Plan:** `docs/plans/2026-11-05-intel-xpu-openvino-harvest.md` §5 (Workstream B).
**Node:** `Overlord-x8664` (`prestopoverlord@100.92.76.67`, Tailnet).
**Compiler commit:** `b8b7784c` (harvest + FFI lane); this record adds only
`benchmarks/intel_ptl/` and this file.
**Environment:** see harvest §10 — Panther Lake **Arc B390** (Xe3,
`arch=v30.0.4`, 96 EU), OpenVINO **2026.3.1**, NEO OpenCL 26.31.39395,
Mesa Vulkan 26.2.1, Level Zero 1.32, kernel 7.2.2-1-cachyos.
**Run-time memory caveat:** the node reported **~3 GB available / 55 GB swap
in use** during these runs (62 GB total). This forced the SDPA geometry
reduction and is visible as p99 outliers (see Notes). Numbers are a first-pass
floor, not a tuned record.

## Harness pattern (documented per plan §5.2)

The harness **SSHes into the node** (`benchmarks/intel_ptl/run.sh`): it syncs
`lib/`, `config/`, `plugins/`, the lane, and the host-built `brievc`; runs the
gate/timing scripts remotely; streams `results.json` back. This matches the
existing benchmark tooling (shell-driven, no daemon) and keeps the compiler
binary identical between host and node. Credentials are supplied at runtime
through `BRIEV_PTL_SSH` — never stored in the repo.

## Timing source

- **Primary (Briev collector):** `benchmarks/intel_ptl/collector.bv` uses
  `lib/std/os/time.bv` (`now_ns`, checked clock) and
  `lib/std/stats/histogram.bv` (bounded, inclusive buckets) to time the
  OpenVINO infer call through `lib/std/ai/openvino.bv`. Input blobs are bound
  into the request tensors **once**, outside the timed region, so the
  measurement is the plugin's infer work (the fair analogue of a Briev kernel
  reading resident buffers).
- **Secondary (Python):** `harness.py` records `perf_counter_ns`; the Python
  API additionally binds/copies inputs and outputs **per call**, so those
  numbers are end-to-end-with-binding and are not the comparison point.
- Correctness gates run **before** any timing; a failing gate suppresses the
  timing samples (the runner exits non-zero).

## Geometry and correctness gates

| bench | geometry | dtype | GPU gate | CPU gate |
|---|---|---|---|---|
| elem | self-add, N = 64 Mi | f32 (f32 precision hint) | **PASS** (max rel 0) | **PASS** (max rel 0) |
| gemm | square MatMul 4096³ | f16 | **PASS** (max rel 3.68e-4) | **PASS** (3.68e-4) |
| sdpa | `scaled_dot_product_attention`, B=8 H=32 S=1024 D=128 | f16 | **PASS** (max rel 2.19e-4) | **PASS** (4.98e-4) |

Reference = NumPy f32 from the f16 inputs (GEMM matmul; attention
softmax(QKᵀ/√D)V, chunked over batch to bound memory). Gates are deliberately
tighter than the plan's 2e-2 (measured errors are ~2–4 orders lower).

**SDPA geometry reduced:** the plan's 32/32/4096/128 needs >3.2 GB for Q/K/V
alone; with ~3 GB available the lane uses 8/32/1024/128 (137.4 GFLOP per
infer, identical total FLOPs to the 4096³ GEMM by construction). Re-run at
full geometry when the node has memory headroom.

## Collector timings (ns, Briev checked clock + histogram)

| bench | device | iters | p50 | p90 | p99 | max |
|---|---|---|---|---|---|---|
| elem | GPU | 60 | **10,518,999** | 10,575,999 | 234,432,600 | 234,432,600 |
| elem | CPU | 8 | **18,053,999** | 88,892,999 | 88,892,999 | 88,892,859 |
| gemm | GPU | 20 | **9,329,999** | 13,279,999 | 18,319,999 | 18,311,466 |
| gemm | CPU | 2 | **288,599,999** | 312,099,999 | 312,099,999 | 312,054,076 |
| sdpa | GPU | 12 | **8,539,999** | 17,799,999 | 187,029,999 | 187,024,923 |
| sdpa | CPU | 1 | **418,199,999** | 418,199,999 | 418,199,999 | 418,184,839 |

Derived (p50):

| bench | GPU | CPU | GPU vs CPU |
|---|---|---|---|
| elem (512–768 MB traffic) | ~49–73 GB/s | ~28–43 GB/s | ~1.7× |
| gemm (137.4 GFLOP) | **14.7 TFLOP/s** | 0.48 TFLOP/s | ~31× |
| sdpa (137.4 GFLOP) | **16.1 TFLOP/s** | 0.33 TFLOP/s | ~49× |

Python secondary p50 (per-call binding included): elem 273 ms / 332 ms,
gemm 56.1 ms / 310.9 ms, sdpa 78.2 ms / 426.3 ms (GPU / CPU). Use only to show
the binding overhead; the collector is authoritative.

## Notes and open cells

- **Briev kernel cells are intentionally absent.** The plan allows Briev/C
  only when a runner exists; Briev has no Intel SPIR-V execution lane yet
  (Workstream D design). The collector is nevertheless Briev's own
  instrumentation timing the competitor, as specified.
- **Outliers:** elem GPU p99/max ≈ 234 ms, sdpa GPU p99 ≈ 187 ms — single
  frequency/thermal or memory-pressure events (55 GB swap in use). p50 is
  stable across repeats (±2%).
- **OpenVINO is a competitor/reference here**, never a Briev codegen path
  (plan §1.1).
- **Gate first:** every timing above is post-gate; gates and raw samples are
  in `benchmarks/results/2026-11-06-intel-ptl-first-pass.json`.

## Reproduce

```bash
cargo build --release
BRIEV_PTL_SSH="ssh <node>" bash benchmarks/intel_ptl/run.sh
# single bench:  BRIEV_PTL_BENCHES=elem ...
```

`BRIEV_PTL_SSH` is a command prefix (`ssh user@host`, or an ephemeral
`sshpass … ssh …`), never a committed credential.
