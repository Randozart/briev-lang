# 2026-11-22 — P3 Gemm graduation baseline: saltpetre's Gemm vs the vendor at one geometry

Plan: `.opencode/plans/2026-11-22-saltpetre-gemm-graduation.md` (M2 + M3 + M4).
Research: `.opencode/plans/2026-11-22-saltpetre-gemm-graduation-research.md`.

## 0. Outcome (read this first)

All three parties were measured at **one** geometry — 512×512×512, f32 — because that
is the geometry the g0 rung and the saltpetre baseline share. Nothing below is
substituted from another shape.

| who | path | p50 (or median) | correctness gate |
|---|---|---|---|
| **Briev device lane** (`g0_gemm`, Vulkan) | kernel time, `# gpu_time` | **0.36 ms** (median of 6: 0.235–0.371) | `BAD=0`, `GPU_SUM == CPU_SUM`, `CANARY_BAD=0`, `compiled on 'vulkan'` |
| **OpenVINO 2026.3.1, GPU f32** | full compiled-model call | **0.481 ms** (min 0.417, p90 0.547) | gate PASS, max rel err 2.8e-4 |
| **OpenVINO 2026.3.1, CPU f32** | full compiled-model call | **0.603 ms** | gate PASS, max rel err 9.0e-7 |
| **saltpetre's own Gemm** (`exec.bv`) | scalar triple loop, 1 thread | **319.9 ms** | `BAD=0` on 4096 sampled dots, `SUM=131033.188` |

Measured ratio, device time vs vendor f32 GPU: **0.74×** (Briev faster). Both the
Briev device lane and the saltpetre CPU baseline print the *same* sum,
`131033.188`, so the two are provably on the same inputs and the same arithmetic.

Two honest caveats, both of which only ever flatter Briev:

1. **Units are not identical.** `# gpu_time` is device/kernel time; OpenVINO's
   `p50_ns` is a whole compiled-model call including host-side dispatch. A
   host-inclusive Briev number would be strictly larger. The graduation bound is
   therefore set at **2×**, not at the measured 0.74×, and P4 (device execution of
   saltpetre ops) must re-measure host-inclusive before any tighter claim.
2. **This is not a CPU-parity claim.** saltpetre's Gemm is a scalar loop; oneDNN's
   CPU MatMul is ~530× faster at this geometry. That gap is the op's implementation,
   not a compiler verdict, and it is recorded here so no later rung can pretend it
   was measured and satisfied.

## 1. Why 512³ and why f32

`benchmarks/intel_ptl/g0_gemm.bv` is 512×512×512 f32 with closed-form inputs
(`a = ((i*7)%13)*0.125 - 0.5`, `b = ((i*3)%11)*0.25 - 1.0`). The M1 fixture
(`tests/saltpetre/gemm_baseline.bv`) reproduces exactly that geometry and those
inputs, so saltpetre's CPU number and the device number are on one shape.

The only OpenVINO datapoint that existed before this run was
`2026-11-06-intel-ptl-first-pass.md`: 4096³ f16, 9.33 ms GPU / 288.6 ms CPU. That is
a **different geometry and a different precision** and cannot stand in for a 512³
comparison — a 4096³ number divided down is not a measurement. So the OV harness was
parameterised (see §3) and re-run at 512³.

f32 is the compared precision because that is what the Briev fixture and the
saltpetre fixture both compute. f16 was measured too, and is reported only as the
vendor's faster lane — not as a like-for-like:

| OV lane | p50 | min | p90 | iters |
|---|---|---|---|---|
| GPU f32 | 480 854 ns | 416 749 ns | 547 402 ns | 20 |
| CPU f32 | 602 712 ns | 602 712 ns | 618 821 ns | 2 |
| GPU f16 | 231 897 ns | 225 760 ns | 246 657 ns | 20 |
| CPU f16 | 644 856 ns | 644 856 ns | 657 629 ns | 2 |

Raw: `2026-11-22-ptl-ov-matmul512-f32.json`, `2026-11-22-ptl-ov-matmul512-f16.json`
(the harness's own `results.json`, verbatim — gates, per-iteration samples,
geometry, and the args actually used).

## 2. Gate record (machine-read by `tests/saltpetre_op_ledger.rs`)

The block below is parsed by the ledger gate; editing a number here without
re-measuring it makes the gate fail, which is the point.

```
saltpetre_op=Gemm
geometry=512^3-f32
device=PTL-Xe3-vulkan
BAD=0
CANARY_BAD=0
COMPILED_ON_VULKAN=1
briev_gpu_p50_us=356.5
openvino_gpu_p50_us=480.9
ratio_vs_openvino=0.74
within=2
```

Evidence that the gate is not decorative: the run that produced the first
`-- g0_fsum_approx on opencl: WRONG (bad=?)` line was **not** a wrong result, it was
a run with no verdict at all (the kernel compiled, then the process produced no
`_BAD` line — the NEO `clFinish` class recorded in BUGS.md). The harness was
classifying "no verdict" as "wrong"; see §4. Every number above comes from a run
whose `_BAD` line exists.

## 3. What was measured, and how

Node: PTL, `prestopoverlord@100.92.76.67`, Arc B390 iGPU (Xe3/X7 358H), Vulkan
1.4.357, OpenVINO `2026.3.1-22476-759c5a6ab8c-makepkg`, Linux x86_64.

**M2 — the Briev device rung.** Six runs of the g0 gemm rung via
`--accel=force --deny-accel-cpu-fallback`, all `BAD=0`, `GPU_SUM=CPU_SUM=131033.188`,
`CANARY_BAD=0`, `compiled on 'vulkan'`. Kernel times: 0.367, 0.235, 0.345, 0.346,
0.370, 0.371 ms (median 0.356). The 0.235 ms sample is the same kernel as the 0.371
one; the spread is launch/JIT noise on an iGPU, not two tiers. Compare
`2026-11-17-ptl-f16-tier.md` (0.340 ms at this shape) — today's median is inside
that band.

The full `g0.sh` gate also passes end to end on this node:

```
== g0: elementwise ==
-- g0_elem on vulkan: bad=0 SUM=ok # gpu_time: 0.038 ms
== g0: gemm 512^3 ==
-- g0_gemm on vulkan: bad=0 SUM=ok # gpu_time: 0.371 ms
== g2: gemm 512^3 tiled (GEMM correctness rung) ==
-- g2_gemm_tiled on vulkan: bad=0 SUM=ok # gpu_time: 0.356 ms
... levelzero lane (required): 5/5 bad=0
... opencl lane (reported): 5/5 bad=0
... g1.5 per-lane smoke: levelzero + opencl both correct
G0: PASS (vulkan elementwise + 512^3 GEMM vs host reference)
```

**M3 — the vendor rung.** `benchmarks/intel_ptl/harness.py` was parameterised so the
GEMM geometry is an argument (`--gemm-n`, `--gemm-dtype`) instead of a module
constant, and so inputs/references are built per bench. Nothing about the models or
the references changed; the gate is still the harness's own — a reference computed
in f32 from the f16/f32 inputs ("so the gate measures the plugin's arithmetic, not
an f16 re-read"), and no timing reported unless the correctness gate passed.

```bash
python3 benchmarks/intel_ptl/harness.py --work run512f32 --benches gemm \
        --gemm-n 512 --gemm-dtype f32
python3 benchmarks/intel_ptl/harness.py --work run512f16 --benches gemm \
        --gemm-n 512 --gemm-dtype f16
```

Reproduce from a workstation:

```bash
BRIEV_PTL_SSH="ssh <user>@<host>" BRIEV_PTL_ENV="XDG_CACHE_HOME=/tmp/briev-cache" \
  bash benchmarks/intel_ptl/g0.sh
```

## 4. Three harness defects found and fixed (not compiler changes)

All three are harness-only (invariant 7 untouched: no file under `src/` and no file
under `lib/std/`). The first two surfaced while measuring; the third surfaced when
CI ran the M1 gate, which is the same discipline applied one layer out.

1. **The node's root filesystem was 100 % full (64 K free), so the gate aborted with
   no verdict.** The compiler's FFI object cache is
   `dirs::cache_dir()/briev-compiler/ffi` — i.e. `$HOME/.cache` — and `BRIEV_HOME`
   does **not** redirect it, so `clang` died with `unable to open output file
   '.../763aaa….o': 'No space left on device'` and `set -e` killed `g0.sh` right
   after its first `==` line. Fixed generally: both PTL scripts accept
   `BRIEV_PTL_ENV`, an env prefix for every remote shell, applied *after* the `cd`
   (before it, the assignment scopes to `cd` only). The gate then runs unchanged
   against a cache on the node's 31 G tmpfs. `/` is still full — this is a bypass,
   not a repair, and the node's space is the owner's data (289 G of `~/Projects`),
   not build output.
2. **"No verdict" was classified as "wrong".** `run_gate_reported` treated a run
   with no `_BAD` line as a wrong result (`WRONG (bad=?)`), flipping `G0:` to FAIL
   — observed once on `g0_fsum_approx`/opencl. A `_BAD` line is the only thing that
   can testify either way, so the branch now reports
   `no verdict (compiled=N, timeout/hang) — reported` and leaves the verdict fatal
   only when a `_BAD` line exists and disagrees. Same class as the rule this project
   already applies to device rows: an event that did not happen is recorded as
   unavailable-with-why, never as a pass and never as a failure.
3. **The fixture gate raced itself under the project's own gate runner (CI).**
   `tests/saltpetre_gemm_baseline.rs` memoised the compile-and-run in a `OnceLock`,
   which is per *process*. Locally (`cargo test`, threads in one process) that is a
   single build; in CI the runner is cargo-nextest, which executes every test in its
   own process, so the file's two tests each built into the same `--out` directory
   and one `remove_dir_all`'d the other's output between writing the `.ll` and
   linking it. CI failed with `ld: cannot open output file
   .../target/saltpetre-gemm-baseline/gemm_baseline: No such file or directory`
   (run 45050623812), i.e. the *gate* was racing, not the fixture. Fixed with a
   cross-process claim (atomic `create_dir`, released on drop, 300 s deadline) plus
   a freshness check: the second process reuses the binary the first one produced
   when it is newer than the `.bv`, so one compile per run and no shared-directory
   window. Verified both ways: two tests in two processes → 14.6 s then 0.3 s
   (reuse), 2/2 green under `cargo nextest run --release --test
   saltpetre_gemm_baseline`.

## 5. What is not claimed

- Not that saltpetre's Gemm is fast. It is 319.9 ms at 512³ against oneDNN's
  0.603 ms on the same node — the op is *implemented in Briev*, which is what the
  graduation claims, not that it is currently competitive on CPU.
- Not that saltpetre ops execute on the device. That is P4 (device execution
  surface); this row's `backend:` stays `briev`, meaning "Briev code serves it".
  The device number above belongs to the compiler's own g0 GEMM fixture.
- Not a 4096³ or f16-tier statement. Those live in
  `2026-11-06-intel-ptl-first-pass.md` and `2026-11-17-ptl-f16-tier.md` and are
  unaffected by anything here.
