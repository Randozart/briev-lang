# PTL night queue (2026-11-17) — access, preflight, and the experiment that matters

## Status at the end of the night

| step | result |
|---|---|
| 0 access | **done** — `prestopoverlord@100.92.76.67` (the earlier probe misspelled the account); key auth, unattended |
| 1 preflight | **done** — Arc B390, Vulkan 1.4.357, `cooperativeMatrix = true`, subgroup 32, OpenCL + glslang + spirv-dis present, 20 GiB free |
| 2 P1 four-arm A/B | **done, and it overturned X1** — see below |
| 3 P2 (f16/f32-acc kernel) | **retired**: f32 accumulation is 1.46x *slower*; there was never a kernel to fix |
| 4 P3 NEO residency | not started (the evening went into the harness defect, which was worth more) |
| 5 P5 K3 check | **done, negative** — `MemoryMax` is unenforced on this node too |

### What P1 actually found

`brievc run` was the measurement instrument, and it was wrong: it links a generic
driver that dispatches its own work-item count (16x the tile grid for the coopmat
tier). Measured the way a program ships, 4096³ f16 coopmat is **26.23 ms
(5.24 TFLOP/s)**, not 367.8 — **3.2x faster than the hand-written CL-C reference**
and ~30x faster than the f32 fixture. X1's premise ("coopmat shipped as a 9x
regression") is withdrawn; the shipped default is correct.

Full evidence and the re-taken ceiling table (both arms, same night):
`benchmarks/results/2026-11-17-ptl-f16-tier.md`.

### The new top target

The re-taken table leaves one real gap: **f32-storage GEMM is 4.7x (512³) and 8.5x
(4096³) behind the CL-C reference**, because that path never enters the
cooperative-matrix tier (which needs f16 storage) and there is no tiled f32 kernel
at all — only coopmat or naive. Per doctrine that is a general tiling derivation,
not a vendor special case, and it is a much better-scoped problem than X1 was:
there is a same-night baseline to beat.


The node is free and reachable on the tailnet; other work there may be killed.
What follows is the runnable queue plus the one thing blocking it.

## 0. Access — the blocker is authentication, not reachability

Read-only probes from this box:

```
$ ssh -o BatchMode=yes randozart@100.92.76.67        → Permission denied (publickey,password)
$ ssh -o BatchMode=yes prestooverlord@100.92.76.67   → Permission denied (publickey,password)
```

`prestooverlord@100.92.76.67` is the identity recorded in
`benchmarks/results/2026-11-06-intel-ptl-first-pass.md` and
`docs/architecture/intel-backend-harvest.md:653`.

**Probe matrix (2026-11-17, after the owner's "done")** — every local key against
both candidate accounts, `IdentitiesOnly=yes`, `BatchMode=yes`:

| key | `prestooverlord` | `randozart` |
|---|---|---|
| `id_ed25519` | Permission denied (publickey,password) | Permission denied |
| `id_ed25519_pf4rqd81` | Permission denied | Permission denied |
| `id_github` | Connection reset by peer | Connection reset |
| `id_rsa` | Connection reset by peer | Connection reset |

No ssh-agent is running, no `ptl`-named env var or credential file exists, and
`~/.ssh/config` has no alias for the host. The port answers (the resets come from
sshd itself, and the denial lists `publickey` as an offered method), so the node is
up and **a key is expected** — it is simply not one of the keys on this box.

Two readings, and the field that distinguishes them: either the key was installed
for a different account, or it was the public key of a *different* machine. Both are
one command to settle:

```bash
# from the box that holds the key the node already trusts:
ssh -o BatchMode=yes prestooverlord@100.92.76.67 'echo OK'

# or install THIS box's key, once:
ssh-copy-id -i ~/.ssh/id_ed25519.pub prestooverlord@100.92.76.67

# or hand me the prefix in a file I can source (keeps it out of the repo):
echo 'BRIEV_PTL_SSH="sshpass -p '''<pw>''' ssh -o StrictHostKeyChecking=accept-new prestooverlord@100.92.76.67"' \
    > ~/.config/briev-ptl-ssh && chmod 600 ~/.config/briev-ptl-ssh
```

The original note follows.

```bash
# (a) preferred: install the key once, then every run is unattended
ssh-copy-id -i ~/.ssh/id_ed25519.pub prestooverlord@100.92.76.67

# (b) ephemeral, per the harness's own design: supply the prefix at run time
export BRIEV_PTL_SSH="sshpass -p '<pw>' ssh -o StrictHostKeyChecking=accept-new prestooverlord@100.92.76.67"
```

`benchmarks/intel_ptl/run.sh` already takes `BRIEV_PTL_SSH` as a command prefix and
stores nothing in the repo, so (b) needs no code change and (a) needs nothing at
all. **Until one of them exists, the queue below cannot start** — and I will not
script around a password prompt.

## 1. A correction that changes the experiment

Before the queue, the record needs fixing, because it decides what is worth
measuring tonight. `benchmarks/results/2026-11-09-intel-lane-ceilings.md:249-256`
is the primary source:

| GEMM f16 | coopmat | tiled (`spirv_coopmat: 0`) | **f32 tiled** | CL-C ceiling |
|---|---|---|---|---|
| 512³ | 1.05 ms | **1.70 ms** | 0.123 ms | 0.233 ms |
| 4096³ | 371 ms | **459 ms** | 39.7 ms | 84.4 ms |

* **Like-for-like, coopmat WINS the f16 A/B** — 1.05 vs 1.70 ms and 371 vs
  459 ms. The record says so explicitly: "the coopmat tier is the better f16 path
  here".
* The `0.123 / 39.7 ms` figures are the **f32** tiled path — a different dtype.
  `docs/plans/2026-11-16-memory-and-xpu-finish-grind.md` §X1 (and my
  `2026-11-17-x1-denominator-audit.md`, which inherited its framing) compared
  f16-coopmat against f32-tiled and called the result a coopmat regression. It is
  not: it is the **f16 tier being 9× slower than the f32 tier**.
* The record's own conclusion is therefore the right one, and it is already
  itemised as W3: *"the emitter needs an f16-storage/f32-accumulate tiled kernel
  (like the C reference, 84 ms)"*, plus *"coopmat selection must follow measured
  device facts"*.

So tonight's headline is **not** "coopmat on/off" (already measured, verdict:
keep coopmat for f16). It is: **why is the f16 path 9× behind the f32 path on the
same device, when the CL-C reference reaches 1628 GFLOP/s in f16?**

Candidate causes, in the order I would test them (all cheap):

1. **Accumulator precision.** `config/ir-lowering.dbvl` has
   `spirv_coopmat_f16acc: 1` — f16 accumulation. The CL-C reference accumulates in
   f32. An f16 accumulator both loses precision and, on Xe3, halves the mma
   throughput headroom; the existing knob makes this a pure A/B.
2. **Load width.** The CL-C reference stages `v4half` (64-bit loads); if Briev's
   fill loads halves one at a time, the fill dominates the panel (the emitter's own
   comments already flag "the fill is ~8× the mma instruction volume per panel",
   and `spirv_coopmat_fill_pairs`/`fill_quads` exist for exactly this).
3. **Subgroup size.** Mesa/ANV on Xe3 vs the emitted assumption
   (`spirv_coopmat_subgroups: 2`).

## 2. Preflight — RUN 2026-11-17, results

Access works with **`prestopoverlord@100.92.76.67`** (key, unattended; the earlier
probe used a misspelling). Node and device, verbatim:

```
hostname        Overlord-x8664
kernel          7.2.2-1-cachyos
cpu             Intel(R) Core(TM) Ultra X7 358H        (Panther Lake)
memory          62 GiB total, 20 GiB available          (the 2026-11-06 record had 3)
gpu             /dev/dri/card0, /dev/dri/renderD128
tooling         vulkaninfo clinfo glslangValidator spirv-dis clang   (all present)
vulkan instance 1.4.357 (Mesa)
cooperativeMatrix                   = true
cooperativeMatrixSupportedStages    count = 1
subgroupSize                        = 32   (min 16, max 32)
shaderFloat16                       = true
shaderBFloat16CooperativeMatrix     = true
```

```
opencl          Platform Intel(R) OpenCL Graphics → Device Arc(TM) B390 GPU
                Platform Intel(R) OpenCL          → Device Ultra X7 358H (CPU)
cgroup v2       cpuset cpu io memory hugetlb pids rdma misc dmem  (systemd-run present)
load            ~1.0; others running: llama-server, data-crawler (not mine — leaving them)
```

So every arm of P1 is measurable: coopmat exists, subgroups are 32-wide
(`spirv_coopmat_subgroups: 2` ⇒ 64 threads per workgroup, legal), and f16 storage
is exposed. **X2 is also available** (NEO OpenCL enumerates the same B390), and
**P5's cgroup check is meaningful** (the memory controller is mounted). The 3 GB
memory caveat that degraded the 2026-11-06 numbers does not apply tonight — 20 GiB
available.

`llama-server` and `data-crawler` are running. They are not mine and 20 GiB is
enough for 4096³ f16 (137 GFLOP, buffers ~100 MB), so the plan leaves them alone;
if a run needs the GPU to itself, ask before killing.

### A defect this preflight found in my own audit

`benchmarks/results/2026-11-17-x1-denominator-audit.md` reports "4096 workgroups at
4096³" from `GemmPlan::dispatch_geometry`, which computes
`tiles_n = n / TILE`. The emitter does something different
(`src/backend/spirv/gemm.rs:3172`):

```rust
let subgroups = GemmPlan::coopmat_subgroups();
let tiles_x = (plan.n / (64 * subgroups as i64)) as u32;   // one WG spans 64·subgroups columns
```

With the shipped `spirv_coopmat_subgroups: 2`, a workgroup covers **128** columns,
so the real dispatch at 4096³ is **2048** workgroups, not 4096 — my audit
over-counts by the subgroup factor and its `covered == output` assertion passes only
because `workgroups · tile_m · tile_n` happens to multiply back to `m·n` when
`tile_n` is under-stated by exactly the same factor… which it is not: it is
under-stated but `workgroups` is over-stated by the same 2×, so the product is still
`m·n`. The *conclusion* (grid is full, not folded) survives; the **number** in the
results file is wrong.

Fix in BUILD, not here: `dispatch_geometry` must derive `tile_n = TILE * subgroups`,
the audit assertion must be re-run, and the results file gets a correction line
(it is a timestamped record, so the correction is appended, not rewritten).

## 3. The queue, in order

```bash
ssh $PTL 'vulkaninfo --summary 2>/dev/null | head -30'         # device, driver, apiVersion
ssh $PTL 'vulkaninfo 2>/dev/null | grep -iE "cooperative_matrix|subgroupSize"'  # is coopmat exposed, and at what subgroup
ssh $PTL 'clinfo -l'                                            # X2: NEO/OpenCL present?
ssh $PTL 'command -v glslangValidator glslc spirv-dis'          # X3 tooling for inspecting SPIR-V
ssh $PTL 'systemctl show -p MemoryMax /user.slice 2>/dev/null; cat /sys/fs/cgroup/cgroup.controllers'  # K3: can limits bite here?
```

Record the answers verbatim in the results file. If `cooperative_matrix` is
absent, X1's coopmat arm cannot be re-measured and the queue drops to the f16-tiled
work (which is the prize anyway).

### P1 — reproduce the recorded A/B (10 min, no speculation)

Four builds of `benchmarks/intel_ptl/ceiling/ceil_gemm_4096_f16.abv`, same binary,
differing only by `--config-dir` overrides:

| arm | overrides |
|---|---|
| a | `spirv_coopmat: 1; spirv_coopmat_f16acc: 1;` (today's default) |
| b | `spirv_coopmat: 1; spirv_coopmat_f16acc: 0;` |
| c | `spirv_coopmat: 0; spirv_coopmat_f16acc: 1;` |
| d | `spirv_coopmat: 0; spirv_coopmat_f16acc: 0;` |

Verdict from `benchmarks/deferred_ab_gate.sh` (variant-vs-variant element-wise),
not single-arm checks; the work count of each arm comes from
`bash benchmarks/gpu_workcount_audit.sh` so no timing is trusted against an
unknown denominator. **Expected outcome if the record holds:** a < c, and b < a —
i.e. f32 accumulation is the lever. If b ≈ a, the accumulate hypothesis is dead
and the fill-width hypothesis (2) becomes primary.

Also run each arm **twice**, once with an explicit device finish: if the wall time
drops when finishing is explicit, X4's `clFinish` gap is confirmed and every
earlier timing on this lane gets a caveat.

### P2 — the f16 tiled microkernel (the actual prize)

If P1 shows accumulation is the lever, the deliverable is an **f16-storage /
f32-accumulate tiled kernel**, which the CL-C reference proves is achievable on
this device at 84.4 ms (1628 GFLOP/s). That is W3(1) and it is a general lowering
change (an f16 multiply feeding an f32 accumulator), not a cooperative-matrix
feature — so it also serves every non-coopmat target.

Target: beat 371 ms (today's coopmat) decisively; the honest ceiling to aim at is
84.4 ms, and parity with the f32 tiled path (39.7 ms) is the stretch. Record
against all three.

### P3 — X2 NEO residency (needs the preflight's `clinfo`)

Mirror the L0 shape already landed (`mapped`, `launch_dev`, `launch_dev2d`,
reused timestamp event) for the NEO driver. Evidence: wall vs device time per
dispatch; L0 landed at wall ≈ device, which is the target shape.

### P4 — X3 GLSL/XMX ceiling (only if P2 needs a reference)

Same-device CL-C numbers at the shapes in question, so the comparison is
like-for-like. Do not quote the 84.4 ms figure without re-measuring it tonight —
it came from a run with a 3 GB-available / 55 GB-swap caveat.

### P5 — K3 enforceability check (free, 2 minutes)

`harness/edge_budget.sh` refuses to score when `MemoryMax` does not bite. Run it
on the node: if the node's user slice can enforce memory limits, K3 finally has a
runner and the only missing piece is the owner's four numbers.

## 4. Rules for the night

* Everything in a scratch dir (`/tmp/briev-ptl`, the harness default); touch
  nothing else on the node, and kill only my own processes.
* Kernel-selecting knobs are A/B'd by variant diff; never a single-arm timing
  (Rule: a keyword- or knob-beaten default is a compiler bug).
* Every number into `benchmarks/results/2026-11-17-ptl-f16-tier.md` with: device,
  driver, API version, the exact `--config-dir` overrides, the work-count table,
  and the correctness gate result. Provisional if the memory caveat recurs.
* If the preflight fails or auth stalls, that is the recorded outcome — the
  evening must not end with an unrecorded attempt.

## 5. Order and cost

| # | item | time | gate |
|---|---|---|---|
| 0 | access (owner: one command) | 1 min | `ssh -o BatchMode=yes ... true` |
| 1 | preflight | 5 min | answers recorded |
| 2 | P1 four-arm A/B + finish test | 15 min | variant gate + work counts |
| 3 | P2 f16/f32-acc tiled kernel | hours | 84.4 ms reference, correctness first |
| 4 | P3 NEO residency | evening | wall ≈ device |
| 5 | P5 K3 check | 2 min | harness refuses or measures |

Start with P1: it costs 15 minutes and it either confirms the lever or kills the
hypothesis before any kernel work.
