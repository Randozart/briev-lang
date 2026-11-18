# R2: the GEMM canary is implemented, and it is NOT yet proven to bite

**2026-11-18.** `benchmarks/intel_ptl/g0_gemm.bv` gained a full-coverage input canary
(recomputes the `a`/`b` closed form for **every** element, batched 4096/firing, and prints
`CANARY_BAD`). On the Vulkan lane:

```
[briev_accel] kernel 'gemm' compiled on 'vulkan'
G0_GEMM_BAD=0 GPU_SUM=131033.188 CPU_SUM=131033.188 Y0=28.46875 YN=35.0625 CANARY_BAD=0
```

## The proof failed, and the reason is a fact about the fixture

`docs/plans/2026-11-18-residency-branch-and-canary.md` required the canary to be **seen
red** under a vendored pre-fix `LocalSize` parse. It was not:

| build | G0_GEMM_BAD | CANARY_BAD |
|---|---|---|
| shipped parse (`X*Y*Z`) | 0 | 0 |
| vendored pre-fix parse (`X` only) | **0** | **0** |

The vendored driver was genuinely linked (the archive substitution ran and the sums are
identical), and that is the point: **this fixture cannot express the bug.** Disassembly
says why —

* `ceil_gemm_512_f32.abv` → `OpExecutionMode %27 **LocalSize 16 16 1**` — the 2-D local
  size the 16× over-dispatch lived in;
* `g0_gemm.bv` → under `--llvm` its module carries **no embedded kernel blob at all**, and
  the shape it exercises does not select that tiled kernel.

So reverting the parse to `X`-only is a no-op here, `BAD` stays 0, and the canary has
nothing to see. Sampling vs full coverage was never the limiting factor — the earlier
4096-sample version and this full-sweep version agree exactly.

## What is true, and what is not

* **True:** the canary is a full-coverage input-integrity check and costs one pass over two
  1 MiB buffers; it reports clean on both the shipped driver and the vendored one.
* **Not true:** that it is a regression gate for the 16× class. It has never been red.
  A green check with no demonstrated red is exactly the pattern that let the 16× bug live
  for weeks, and this session already discarded one such test (a bespoke D1 fixture that
  passed with and without its fix).

## Next step, precisely

Move the canary to where the 2-D local-size kernel actually runs:

1. add the same canary to a tiled ceiling fixture (or a new `g2_gemm_tiled_check.abv` that
   declares a 2-D `LocalSize` and a CPU reference);
2. vendor the pre-fix parse again and require `CANARY_BAD > 0` there;
3. only then wire it into `g0.sh`.

Until then, `g0_gemm`'s canary carries an explicit STATUS paragraph in its header saying it
is unproven, so nobody cites it as evidence.
