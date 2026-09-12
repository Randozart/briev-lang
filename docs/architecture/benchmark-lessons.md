# GPU benchmark optimization lessons

Extracted from the PTX tier campaign (2026-09-08 → 2026-09-11). These are
generalizable patterns — applicable to any GPU benchmark, not just GEMM.

## 1. The RMV/Promo wall

**Pattern**: periodic read-modify-write operations on global memory inside
a kernel loop are devastating. Our y-RMV promotion ran every 512 ksteps,
reading the accumulating y from DRAM, adding the local fragment, and writing
it back. It cost **53% of throughput** (21→45 TF when stripped).

**Why it's worse than it looks**: the RMV reads are cold-miss (4B scattered
reads across 64MB of y) at the *end* of the kernel — they poison DRAM
bandwidth for still-running CTAs in later waves. One execution of the RMV
body cost ~17 TF equivalent of interference.

**Fix pattern**: full-K register accumulation + store-only epilogue after
KEND. The loops stays clean (no in-loop branch, no global read, no global
write until the end). The final store is a straight-line tail that ptxas
can schedule freely.

**Applicability**: any kernel with periodic reduction writes, intermediate
checkpoint writes, or per-tile atomic accumulations. The fix is always the
same: accumulate in registers, store once at the end.

**Diagnostic**: if a kernel is much slower than a "stripped" variant that
skips a write pass, the write pass's DRAM interference (not the pass's own
execution time) is the cost. Measure the stripped variant to bound the
loss, then fix the accumulation structure.

## 2. The stale binary trap

**Pattern**: the test profile and release profile can diverge. Running
`cargo test --lib` rebuilds the test profile; `cargo build --release` may
be a no-op if the binary is cached. A stale release binary produces blobs
with the old emitter output — the bench measures old behavior while
believing it measures new.

**Symptom**: the dump-test kernel (built by the test profile) shows the
new behavior; the production blob (built by the release binary) shows the
old behavior. Identical source, different binaries.

**Fix**: always `cargo build --release` before benchmarking production
blobs. Verify the binary's freshness (mtime, strings check, or SASS
fingerprint). Then regenerate the blob and verify its SASS matches
(cuobjdump -sass LDG/STG count is a quick fingerprint — the old kernel
had hundreds of LDGs from the RMV; the new one has 2).

**Applicability**: any project with multiple build profiles (debug,
release, test). Always rebuild the profile you're benchmarking.

## 3. The K-budget curve for f16 accumulation

**Pattern**: f16 accumulation error grows with K (the reduction length).
The relationship is roughly linear: ~1e-3 per ~4096 K-elements. The 1e-2
gate is approached near K=12288.

**Measured curve** (RTX 3060, both coopmat and PTX tiers):

| K | f16acc rel_err | f32acc rel_err |
|---|---------------|----------------|
| 4096 | 5.2e-3 | ~2e-4 |
| 8192 | 8.2e-3 | ~4e-4 |
| 12288 | ~1.2e-2 | — (exceeds gate) |

**Why it's physics, not code**: the error comes from the accumulation
itself (f16 has 10-bit mantissa; each addition can lose precision). The
backends (coopmat, PTX) show the same curve because they accumulate the
same way. The store-only epilogue reduces per-K error slightly (no RMV
re-reads to compound rounding), but the accumulation error dominates.

**Applicability**: any f16 GEMM or reduction. The tier router must enforce
K≤12288 for f16acc; larger K belongs on the f32-acc tier. This is a
compile-time gate, not a runtime decision.

## 4. Register allocation as the hidden occupancy lever

**Pattern**: ptxas's register allocation determines occupancy, not the
programmer's intent. The natural register count (what the code "needs")
differs from the allocated count (what ptxas decides) — and the difference
can change the number of CTAs per SM.

**Example**: our dead promo body (316 instructions that never executed)
changed ptxas's allocation from 40 regs (3 CTAs/SM) to 64 regs (2
CTAs/SM). The dead code cost 16 TF — not by executing, but by changing
the register pressure model that ptxas uses for scheduling.

**Diagnostic**: run `ptxas -v` on the PTX. If the allocated count is
significantly different from what the live code needs, ptxas is allocating
registers for dead paths. Remove dead code from the loop body entirely —
don't just predicate it to never execute.

**Fix pattern**: keep loop bodies lean. Move work outside the loop (after
KEND). Don't leave dead code in the loop body "just in case" — ptxas
allocates for it anyway. The `-maxrregcount` flag caps the allocation but
doesn't force spilling below the natural count.

**Applicability**: any GPU kernel where occupancy matters. The register
budget is 64K regs/SM; at 512 threads/CTA, 64 regs/CTA = 2 CTAs/SM;
at 42 regs/CTA = 3 CTAs/SM. Every register saved in the loop body is a
potential CTA.

## 5. The probe-strip technique

**Pattern**: to measure the cost of a suspected wall, sed-strip it from a
dump cubin and time the result. This is faster than emitter surgery and
gives a clean A/B.

**Protocol**:
1. Dump a cubin with the suspected wall active
2. `sed` the relevant branch/predicates to make the wall unconditional (or
   remove it entirely)
3. Re-assemble with ptxas
4. Time the stripped version vs the original
5. Diff the actual PTX to understand what changed

**Caveat**: the stripped binary may differ in register allocation, occupancy,
or scheduling from the original — the "stripped" time is a lower bound, not
an achievable target. Always verify the register count (`ptxas -v`) and
check for dead code artifacts.

**Example**: our promo strip measured 45.4 TF, but the actual fix (full-K
store-only) achieved 29.3. The 16 TF gap was a register allocation artifact
(40 vs 64 regs). The strip gave the direction; the structural fix gave the
number.

**Applicability**: any benchmark where a single mechanism is suspected of
being the wall. The strip technique isolates its cost without rewriting
the emitter.

## 6. The partitioned tier routing pattern

**Pattern**: different problem sizes favor different backends. There is no
universal "best" backend — the optimal choice depends on the shape.

**Measured split** (RTX 3060, sustained, 110W):

| shape | winner | margin |
|-------|--------|--------|
| 2048³ | coopmat (27.7 vs 25.5) | 8% |
| 4096³ | PTX (29.3 vs ~9.5) | 3× |
| 8192³ | PTX (30.2 vs 21.2) | 43% |

**Why it happens**: coopmat benefits from the Vulkan driver's L2 residency
at small shapes (the data fits in L2, so the driver's fill path is fast).
PTX benefits from `cp.async`/`ldmatrix`-class scheduling at large shapes
(the data doesn't fit in L2, so the pipeline matters). The crossover is
shape-dependent and driver-era-dependent.

**Applicability**: any multi-backend GPU compiler. The tier router should
be keyed on measured shape buckets (recorded in the ledger), never on
source annotations. The router is a dispatch decision, not a semantics
decision.
