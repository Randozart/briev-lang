# Intel XPU / OpenVINO: harvest, FFI module, Panther Lake benchmark lane (2026-11-05)

**Status: PLANNED, NOT STARTED.** This plan is written to be executed by an
agent in a SEPARATE worktree, without session context. Read this top to
bottom before touching anything. The `main` branch carries in-progress H4
work (typed-HIR backend) — do not rebase or amend it; branch from `main`
and coordinate via git only.

Suggested branch: `intel-xpu-harvest`, created off `origin/main`.

**MANDATORY first reads** (house doctrine — this plan assumes them):
1. `AGENTS.md` (operating contract, golden rules, performance-recovery
   protocol, working rules).
2. `docs/plans/INDEX.md` (current status map).
3. `docs/architecture/derivation-not-recognition.md` and
   `docs/architecture/proof-vs-shape.md` (why the guardrails below exist).
4. `docs/architecture/gpu-backend-strategy.md`,
   `docs/architecture/abv-gpu-doctrine.md`,
   `docs/architecture/briev-vs-cuda-thesis.md` (the GPU doctrine this
   workstream must respect).
5. `docs/architecture/glue-ffi.md` + `docs/guides/ffi-and-export.md` (the
   FFI mechanism the module uses).

## 0. Origin and motivation (context you cannot skip)

The repo owner (Randozart) recently authored
[openvinotoolkit/openvino#39432](https://github.com/openvinotoolkit/openvino/pull/39432)
— **"[GPU] Fix rank-1 GatherElements with a constant input"**, open in
review against OpenVINO's Intel GPU plugin
(`src/plugins/intel_gpu/src/plugin/ops/constant.cpp`, +8/-4, plus a new
rank-1 instance in
`src/plugins/intel_gpu/tests/functional/shared_tests_instances/single_layer_tests/gather_elements.cpp`).

The bug, in short: OpenVINO's GPU plugin stores a 1-D tensor in a 4-slot
(batch, feature, height, width) layout. A 1-D **constant** lands in the
FEATURE slot as `(1, N, 1, 1)`; a 1-D **parameter** lands in BATCH as
`(N, 1, 1, 1)`. Gather, GatherND and Split carry hand-written
instructions telling the plugin to read 1-D constants from the batch slot;
**GatherElements was missing from that hand-curated list**, so its axis 0
pointed at the feature slot whose batch stride is 0 — the index was
multiplied away and every rank-1 row returned element 0. Evidence: the
minimal repro on the stock 2026.4.1 wheel gave GPU `[7,0,0,0]` vs CPU
`[7,42,99,0]` pre-fix, matching post-fix; 12/12 new
`GatherElementsLayerTest` instances fail pre-fix and pass post-fix; 252
functional / 67 GPU-unit gather-elements tests pass.

**Why this matters to Briev**: the bug exists because a mature vendor
maintains a hand-curated per-op layout table — that is *recognition*, and
it bit them. Briev derives element access from declared layouts (its
uniform `[data, cap, len]` collection layout + op surfaces) precisely so a
missing table row cannot exist. Record this receipt in the harvest docs —
it is the strongest real-world argument for the derivation thesis, and it
came from the vendor's own code.

## 1. Doctrine guardrails (hard rules for this workstream)

1. **Reference-verified codegen, never delegation.** OpenVINO's
   implementations are references Briev *derives from* (the thesis
   explicitly allows hand-written references with retirement gates). The
   vendor runtime NEVER enters Briev's kernel path. No Briev-graph →
   `ov_model` lowering. OpenVINO is a benchmark competitor and an FFI
   capability escape hatch, nothing more.
2. **Recognition ban (Rules 15/19/23)**: harvest FACTS (layouts, dispatch
   patterns, extension support matrices, tuning knobs, driver quirks) —
   never op lists to copy blindly. The GatherElements receipt above is the
   documented proof of why.
3. **Stdlib is the extension mechanism (Rule 14)**: the FFI module is a
   `.bv` file + `#Link`, zero compiler changes.
4. **No type-name matching in the backend (Rule 19)** when feeding cost
   models: facts go through config (`config/targets.toml`) and the
   `TypeUniverse`, not Rust string matches.
5. **Derivation gate applied to the harvest itself**: every harvested fact
   must name its Briev consumer (cost-model knob, dispatch doc, stdlib
   module, kernel plan) before the harvest doc is considered done. A fact
   with no consumer is not harvested, it is archived.
6. **Correctness before timing (house benchmark rule)**: any benchmark
   number enters `benchmarks/results/` only with a same-commit,
   same-geometry correctness gate vs reference output. Provisional
   timings are marked provisional in the results file itself.

## 2. Hardware and environment

- Target node: **Panther Lake** laptop-class Intel silicon on the owner's
  Tailscale network, host name **`overlordx86-64`** (SSH alias — VERIFY
  with the owner; do not assume).
- iGPU: Xe3-class (PTL is new silicon — verify compute-runtime / Level
  Zero driver support and OpenVINO GPU-plugin enumeration as the FIRST
  execution step; the owner references the stock **OpenVINO 2026.4.1**
  wheel, already used for PR #39432's repro).
- If the GPU plugin does not enumerate on the node yet, the benchmark lane
  degrades to OpenVINO **CPU** plugin vs Briev (still valuable; record why)
  and the driver-setup steps become part of the environment doc.

## 3. Verified repo facts (checked 2026-11-05 via GitHub API — re-verify
against your clone; the repo moves)

`src/plugins/intel_gpu/` contains:
- `graph/` — per-op lowering: ~150 op files including `gemm.cpp`,
  `grouped_matmul.cpp`, `scaled_dot_product_attention.cpp`,
  `paged_attention.cpp`, `rope.cpp`, `rms.cpp`, `kv_cache.cpp`,
  `moe_gemm.cpp`, `moe_3gemm_fused.cpp`, `gated_delta_net.cpp`,
  `paged_gated_delta_net.cpp`, `vl_sdpa.cpp`, `msda.cpp`,
  `paged_causal_conv1d.cpp`, `paged_selective_ssm.cpp` (modern LLM/SSM
  kernels), plus `layout_optimizer.cpp`, `graph_optimizer/`,
  `impls/`, `registry/`, `network.cpp`, `program.cpp`,
  `nodes_ordering.cpp`, `compilation_context.cpp`.
- `kernel_selector/` — kernel selection/tuning machinery.
- `runtime/` — Level Zero + OpenCL dispatch (queues, events, device
  enumeration).
- `plugin/` — op plumbing incl. `ops/constant.cpp` (PR #39432's file).
- `tests/` — functional single-layer suites (the GatherElements suite PR
  #39432 extended lives here), GPU unit tests.

The main OpenVINO repo also has the C API
(`src/c_api/` → `libopenvino_c`) which the FFI module binds.

## 4. Workstream A — Harvest (research → docs; START HERE)

1. Shallow-clone `openvinotoolkit/openvino` into `~/Desktop/Projects`
   (`git clone --depth 1 https://github.com/openvinotoolkit/openvino` —
   full history is multi-GB and irrelevant; the owner's PR gives you a
   live reference for review activity).
2. Deliverable: `docs/architecture/intel-backend-harvest.md` with the
   sections below. **Exit criterion: every fact names its Briev consumer.**
   Sections without consumers get archived, not harvested.
   - **Layout system**: b/f/h/w blocked layouts, `layout_optimizer.cpp`,
     `reorder.cpp` — how Intel wants tensors shaped; compare against
     Briev's uniform `[data, cap, len]` collection layout: coalescing
     behavior, XMX (matrix-engine) pressure, when Briev must reorder for
     Xe3 kernels vs when the derivation handles it.
   - **Dispatch**: `runtime/` Level Zero + OpenCL paths — queue creation,
     event/fence sync, device enumeration, kernel-binary handling
     (SPIR-V vs native binary import). This is the shape Briev's own
     Intel dispatch needs (feeds Workstream D).
   - **Extension/IGC matrix**: which SPIR-V extensions work per Xe arch
     (subgroups, cooperative matrix, `intel_subgroup_*`), IGC behaviors —
     decides Briev's XMX strategy on Xe3 and bounds what the portable
     SPIR-V tier may rely on.
   - **Per-arch configs**: `graph/` per-device execution strategies +
     `kernel_selector/` tuning → concrete knobs for
     `src/analysis/gpu_strategy.rs` (the shape-driven cost model) and
     entries for `config/targets.toml`.
   - **Op coverage map**: native impl vs fallback per op → Briev
     stdlib/kernel coverage priorities on Intel.
   - **Modern LLM kernels**: `paged_attention`, `rope`, `rms`, `kv_cache`,
     `moe_gemm`, `gated_delta_net`, `vl_sdpa`, `msda` — tiling,
     subgroup patterns, caching strategies as derivation references for
     Briev's attention/LLM kernel plans.
   - **Pitfalls ledger**: recognition receipts (start with the
     GatherElements story, §0), driver quirks, env workarounds
     (`OV_GPU_ALLOW_NEW_SHAPE_INFER=1`), known layout traps.
3. Discipline: this is a READING workstream. Do not modify Briev compiler
   code from the harvest; facts flow into docs first, then their named
   consumers pick them up in later workstreams. Commit the doc
   incrementally (one section per commit is fine).

## 5. Workstream B — Benchmark lane on Panther Lake

1. Node access: confirm the SSH alias for `overlordx86-64` WITH THE OWNER
   (unverified in this plan). Verify OpenVINO runtime: `python3 -c
   "import openvino as ov; print(ov.__version__); print(ov.core.available_devices())"`
   or the C API equivalent — GPU plugin must enumerate the Xe3 iGPU.
2. Add a harness entry for the node (see `benchmarks/build_and_bench.sh`
   structure; the node runs the harness remotely or the harness SSHes in —
   pick the pattern that matches existing `benchmarks/` tooling and
   document it in the results file).
3. Benchmarks (each with the correctness gate FIRST):
   - elementwise/reduction sweeps (memory-bound ceiling truth);
   - GEMM (Briev SPIR-V kernels vs OpenVINO GPU plugin vs OpenVINO CPU
     plugin);
   - SDPA/attention lane (Briev kernels vs `scaled_dot_product_attention`
     GPU plugin path) — as kernel coverage allows.
4. Results: `benchmarks/results/2026-11-XX-intel-ptl-<name>.md` with
   geometry, commit, gate evidence, and the collector timing source. The
   collector measurement stack from merged PR #64 (`lib/std/os/time.bv`
   checked clocks, bounded histograms) is the intended timing source —
   using Briev's own checked instrumentation to time Briev vs OpenVINO is
   the point.
5. Record PTL/Xe3 facts discovered (driver version, extension reports)
   into the harvest doc's extension matrix.

## 6. Workstream C — FFI module

1. `lib/std/ai/openvino.bv` + `#Link openvino` (links `libopenvino_c`).
   Minimal surface: core create/release, read model from file/BLOB,
   compile for device ("GPU"/"CPU"), create infer request, set/get input
   and output tensors (shape + typed data), infer, and the version query.
2. Ownership: descriptor pattern per the proven socket/fd work (merged
   PRs #63/#66/#68 — read their BUGS.md entries and
   `src/analysis/reclamation/`): every open has a close on success AND
   failure paths; reclamation analysis must see the FFI calls (it treats
   foreign calls as live boundaries already).
3. Gate test: `tests/openvino_gate.rs` + fixture under
   `tests/fixtures/load/` — MUST skip gracefully when the runtime/library
   is absent (same pattern as other gate tests); on the PTL node it runs
   GPU compile + infer and asserts shapes/values; timing via the
   collector.
4. Zero compiler changes expected. If you find yourself wanting one, STOP
   and re-read guardrail §1.3 — the module is `.bv` + `#Link` only.

## 7. Workstream D — Compiler-side payoff (feeds the GPU roadmap; LAST)

1. `docs/architecture/intel-dispatch.md` first: the design for Briev's own
   SPIR-V dispatch on Intel (L0/OpenCL) derived from the harvest — before
   any implementation.
2. Implementation lands as part of the multi-vendor matrix waves (see the
   followup-stages plan), NOT in this workstream's first pass.
3. `gpu_strategy.rs` + `config/targets.toml` Intel entries from the
   harvest's per-arch tables.
4. Cooperative-matrix/subgroup findings decide the XMX strategy on Xe3 —
   the Intel counterpart of the beyond-coopmat campaign.

## 8. Sequencing and coordination

1. A (harvest) starts immediately — read-only research, zero risk.
2. B and C need runtime verification on the node — resolve §2/§5 open
   points WITH THE OWNER first.
3. D is design-doc-then-implementation and belongs to the GPU roadmap
   waves.
4. H4/H5 work on `main` proceeds in parallel (owned by the main session);
   this workstream MUST NOT touch `src/hir/`, `src/backend/llvm/`
   emission, or `emit_*` files except `gpu_strategy.rs`/`targets.toml`
   knobs in Workstream D. Keep the branch `intel-xpu-harvest`; rebase onto
   `origin/main` as it moves.
5. Commit discipline: continuous commits, `cargo test --lib` green per
   commit, Praetor on changed dirs, architecture docs updated in the same
   commit as structural changes, BUGS.md for discovered bugs (Briev-side
   bugs go in the main repo's BUGS.md; OpenVINO-side findings go to
   upstream issues, not Briev's BUGS.md).

## 9. Open questions (resolve with the owner before B/C/D)

1. SSH alias + credentials for `overlordx86-64` (Tailnet).
2. OpenVINO runtime version + GPU plugin enumeration on the node.
3. Level Zero / OpenCL userland present for native Briev SPIR-V dispatch?
4. Clone logistics approved (shallow clone location)?
5. Priority vs H5 (default: interleaved, harvest first).
