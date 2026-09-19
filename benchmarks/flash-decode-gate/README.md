# Flash-Decode Gate (2026-09-19)

Hand-written PTX gate experiment for the fused attention decode shape
(plan: `docs/plans/2026-09-19-flash-decode-gate.md`). The kernel replaces
the 3-kernel qk→softmax→pv composition with ONE launch.

## Kernel design (flash_v4)

- Grid = H(20) blocks × 1024 threads (32 warps). Block h = query head.
- Warp w owns the j slice [w·128, (w+1)·128) — 4096 KV positions covered.
- Per warp: online-softmax state (m, l) + per-lane acc[4] over
  di = lane + 32i (FULL di range — the di/j coupling mistake in v2 is the
  cautionary tale: each di must see ALL its j's, so warps split ONLY j).
- Dot: 4-element d strip per lane + 5-round shfl butterfly. K/V j-major:
  every global load coalesced; no smem staging needed for loads.
- Slice states merge once via smem (m, l, acc[128] per warp = 2 KB+256 B;
  1 bar.sync per kernel). LSE merge: out = Σ_w acc_w·e^{m_w−M} / Σ_w l_w·e^{m_w−M}.

## Layout contract (learned the hard way — see plan §"forensics")

- a_out MUST live at a proj offset disjoint from every input. The runtime's
  first launch primes via a RAW host→device copy + an extra dispatch;
  an output slot aliasing an input makes dispatch #1's output poison the
  seed for dispatch #2 (and the seeder skips inputs whose slot a write-first
  field claims).
- v proj = host offset (11161640) keeps the raw prime consistent.

## Results (RTX 3060, bitnet geometry H=20 HKV=5 NKV=4096 D=128)

| variant                    | correctness        | time     |
|----------------------------|--------------------|----------|
| 3-kernel chain (unrolled)  | a_err 9.65e-06     | 202 µs   |
| flash v4 (f32, this gate)  | max_rel 2.34e-06   | **123 µs** |
| flash v5 (f16 KV, serial)  | max_rel 2.94e-05   | **94 µs**  |
| flash v5 (f32 + pipelined j)| max_rel 1.09     | 129 µs (no gain — not merged) |
| f32 DRAM floor             | —                  | 58 µs    |
| f16 DRAM floor             | —                  | 29 µs    |

GATE VERDICT: PASS (2.06× f32, 2.7× f16 over the 253 µs pre-unroll chain)
→ build the frontend `FlashDecode` LoopShape + emitter.

f16 KV: PASS, −24% vs f32. The f16 reference decodes the stored bits
exactly, so the residual error is the fp32 dot over f16-rounded inputs.

Pipelining the j loop (prefetch j+1): measured NO gain at 32 warps
(129 vs 123 µs) — warp parallelism already hides the latency; the
491 µs @ 4-warp → 123 µs @ 32-warp jump was the latency lever and it is
spent. Remaining gap to 58 µs (f16: 94 µs) is occupancy/tuning territory
(packed f16x2 loads, slice interleave for ILP, L2 policy), not structure.

## Build

```
cc -O2 -I. -o flash_v4_main flash_v4_main.c -lvulkan -lOpenCL -lcuda -lpthread -lm
./flash_v4_main
```

`flash_v5_f16*` is the f16-KV variant (same structure; `ld.global.u16` +
`cvt.f32.f16` strip loads, f16 seeded storage, f16-decoding reference).

`flash_v4_main.c` embeds the PTX text (driver JIT) and carries its own
double-precision CPU reference; `flash_v4.ptx` is the readable kernel.
The kernel needs the runtime's block_threads=1024 desc field and a dummy
scalar field (the `u` counter slot) so the resident dispatch takes the
dirty-scalar path instead of re-uploading the full projection per launch.

## Pipeline postscript (measured, 2026-09-19)

The "prefetch j+1" pipeline (f16p/p32) was built and measured after a
cheerleader review claimed 30-45%: **0-1% measured, twice** (f16 95 vs
94 µs; f32 129 vs 123 µs). Warp parallelism (32/block) already hides the
load latency the pipeline targets — the 491 → 123 µs jump from 4 → 32
warps was that fix, spent. The remaining 94 → 58 µs gap is occupancy
(20 blocks on 28 SMs = 8 SMs idle — bitnet-specific; H=32 models fill
the machine) plus tuning (packed f16x2 loads), not structure.
