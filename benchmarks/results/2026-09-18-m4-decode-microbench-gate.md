# M4 Decode Microbench Gate — Briev composition vs stock ggml fattn

**Date:** 2026-09-18
**Plan:** `docs/plans/2026-09-18-m3-matrix-and-m4-fattn-shim.md` (step 2)
**Harness:** `bash benchmarks/m4_decode_microbench.sh` (decode shim:
rotate q, append K/V row per kv head via `briev_accel_push_ranges`,
dispatch qk → softmax → pv; a_out device-resident, no host readback)
**Device:** RTX 3060 12GB (sm_86), driver 615.71.09, CUDA lane
**Geometry:** bitnet-2b decode — D=128, H=20, HKV=5, G=4, NKV=4096, f32 KV

## Verdict: NO WIN — fattn.cu integration not wired

| Stage | p50 |
|---|---|
| push (q + 5 K rows + 5 V rows, 11 HtoD ranges) | 58.5 µs |
| qk | 226.9 µs |
| softmax | 40.0 µs |
| pv | 145.9 µs |
| **Briev chain (kernel parity)** | **412.8 µs** |
| **Briev total (incl. append)** | **471.3 µs** |
| **ggml stock fattn (hsk=128, nb=1, kv=4096, f16 KV)** | **58.3–59.6 µs** |

Stock comparator from `test-backend-ops perf -o FLASH_ATTN_EXT` (this
fork's maintained harness): nr23=[1,1] 59.61 µs, [4,1] 58.28 µs, [8,1]
58.44 µs — flat in head count at fixed kv (KV-read-bound), so bitnet's
20/5 sits on the same rows. GQA=4 (bitnet's exact G) is one of the rows.

**Briev is ~7× slower at kernel parity, ~8× with append included.**

## Dissection (why the composition loses)

1. **f32 KV doubles every KV byte.** ggml reads f16; every Briev kernel
   moves 2× the KV data against the same 360 GB/s. Pure handicap, worth
   ~2× on the memory-bound stages.
2. **qk at ~13% of BW floor** (226.9 µs vs ~30 µs for 10.5 MB): the
   vec4-FMA path underutilizes — one vec4 pair per lane-iteration at
   BLOCK=64 does not saturate, and S materialization adds a full
   H·NKV·4B write stream ggml never makes.
3. **pv re-reads V per query head** (GQA): H × NKV·D·4 = 42 MB of V
   traffic vs 5 distinct 2.1 MB slices; 145.9 µs ≈ 80% of that
   redundant floor (L2 catches some of the 4× group sharing).
4. **Three passes over S/O1** (materialize → normalize → reuse) vs
   ggml's single fused online-softmax pass that never leaves registers.
5. **No tensor cores**: ggml's MMA f16 path hits ~31 TFLOPS at nb=4096;
   even the decode rows benefit from its f16 dot shapes.
6. **Append cost 58.5 µs** for ~15 KB: 11 synchronous HtoD calls
   dominate; batchable to ~1 range set but not the story.

Even zeroing push and fully batching launches, the chain is ~7× stock.
The gap is structural: one fused f16 kernel that reads K/V once vs three
f32 kernels that materialize intermediates. Honest negative recorded per
the plan's verdict rule.

## Consequence + future path

`BEST_FATTN_KERNEL_BRIEV` wiring is **not** built (gate says stop). A
future attempt must change the composition's SHAPE, not tune it: a single
fused decode node with online softmax in registers, native f16 KV loads
(`Cast.#`-based or typed buffers), and GQA V sharing inside the node —
i.e. the accel partition must be able to EXPRESS what ggml's fattn
kernel is, per the capability-frontier principle (the compiler's optimum
must be expressible in the language). That is frontend expressiveness
work with its own plan; this gate is its justification.

## Artifacts

- `/tmp/opencode/m4.pm7q` (script run 2026-09-18)
- Runtime additions (kept, additive): `briev_accel_push_ranges` +
  driver `upload_ranges` hook — the decode-append path any future
  integration needs regardless of the fattn verdict.
