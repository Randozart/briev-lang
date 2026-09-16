# Briev vs CUDA — the structural-capability thesis

**Author:** Randy Smits-Schreuder Goedheijt <randozart@gmail.com>
**2026-09-16.** Why Briev's architecture enables automatic behaviors that
CUDA's model has no mechanism to produce. This is a thesis document for
future reference, not a finished implementation. Companion to:
`docs/architecture/briev-capability-frontier.md` (expressiveness closure),
`docs/architecture/briev-execution-model.md` (the reactor),
`docs/plans/2026-09-14-gpu-schedule-pass.md` (the node DAG).

## The honest framing

CUDA is Turing-complete — *anything* is expressible. "Incapable" therefore
means **structurally incapable as an architecture**: classes of *automatic*
behavior the CUDA model has no place for, because CUDA's unit is the
hand-written kernel and its host orchestration is hand-written
`cudaMemcpy`/`cudaLaunch` sequences. Briev's unit is the **reactive node
DAG with contracts** — the compiler reads the *whole program* and derives
execution. That difference is where the real capabilities live.

The claim is not "CUDA cannot do X" absolutely — it is "CUDA cannot do X
**automatically, provably, or derivably**; a human must scaffold it by
hand, forever, and it is verified by testing, never by proof." Briev's
compiler does them from program semantics. Every capability below is
measured-proven on at least one path in this repository.

## Capability 1 — the compiler IS the scheduler (program-derived orchestration)

In Briev, the reactor and the `gpu_schedule` DAG analysis derive *which
kernels launch when*, *in what order*, and *whether syncs are even
needed*. Proven in this session (2026-09-16): independent nodes batch
back-to-back (sync elimination — `gpu_schedule` Phase 2), y-lifetime
buffers reuse (`gpu_schedule` Phase 3), chain fusion is *detected from
topology, never named* (no `FusedAttention`, no operand names — the
`GEMM → elementwise → GEMM` shape falls out of single-reader dead
intermediates).

**CUDA's structural gap:** there is no program DAG to analyze. Kernels
are opaque blobs; the host code IS the schedule, written by hand, and no
compiler pass sees the whole computation. CUDA cannot *discover* that two
kernels are independent — a human wrote the sequence. Briev's compiler
*proves* independence (`concurrency_gate::xor_overlap`, `region`
independence) and eliminates syncs it can prove unnecessary.

## Capability 2 — contracts are the correctness lever, not undefined-behavior trust

Briev's optimization lever is **proof**, not UB. The capability-frontier
doc makes this precise: LLVM-class systems live on poison/argmem
assumptions the compiler *claims*; Briev's `[pre][post]` *prove* them. The
`#13` full-memory semantics fix is the worked example — an LLVM assumption
("this is argmem-only") that, when wrong, was a miscompile; Briev's model
proves the memory discipline instead.

**CUDA's structural gap:** a CUDA kernel has no contract surface — nothing
for the compiler to verify. Buffer lifetimes, aliasing, race freedom are
*tested*, never *proven*. The touched-field tables, last-use analysis, and
buffer-reuse opportunities in this session's chain fusion are all *proofs*;
CUDA's model has no place for a compiler proving a kernel only touches
declared fields.

## Capability 3 — strategy selection as an analysis pass (shape-driven synthesis)

This session's core result: the cost model (`src/analysis/gpu_strategy.rs`)
picks tile/stages/load-path *from shape evidence*, calibrated against the
decompiled cuBLAS kernel map, producing parity-or-better with **zero
hand-tuned kernels**. The fused-attention 10× regression was found by
measurement, not missed by assumption.

**CUDA's structural gap:** a CUDA kernel is written for ONE geometry.
Shape-adaptive strategy is a *human* activity — rewrite the kernel per
shape, or call cuBLAS and trust its heuristic. CUDA the *language* cannot
generate a 64³ kernel vs a 4096³ kernel differently; the "strategy" lives
in the binary you wrote, not in any analysis. Briev's compiler *synthesizes
the kernel from the shape*; not expressible in CUDA because CUDA kernels
are the fixed artifact, not the output of a compiler analysis.

## Capability 4 — the interpreter-as-reference (reference-verified codegen)

Briev runs the same program on CPU and GPU; the interpreter is the
reference. The `accel_probe` output-equality gate checks GPU output against
CPU semantics at runtime before committing a dispatch decision. The
interpreter-is-reference rule is the correctness backbone: if the
interpreter runs it, the backend must compile it — fix codegen, never the
interpreter.

**CUDA's structural gap:** there is no reference. A CUDA kernel is checked
against *whatever the programmer also hand-wrote in host code* — the
"reference" is another unverified artifact. Briev has a *defined language
semantics*; the GPU kernel is one *lowering* of it, verifiable against the
other. CUDA's kernels are the only definition.

## Capability 5 — one reactive model spanning GPU compute and bare metal

The rv64 arc proved `bootstrap node`, `node @ vector`, full-context traps,
preemptive scheduling — all in *language-level syntax*. The reactor that
schedules GPU nodes is the same reactor that services machine interrupts.
A Briev program is one program; its scheduling is one mechanism across
compute and embedded.

**CUDA's structural gap:** CUDA has no notion of a program that is both a
GPU compute kernel *and* a bare-metal interrupt service — it is two
ecosystems (device kernels + host C) with no shared semantic model. Briev's
single reactive model spans both.

## The recursive claim (the strongest one)

Briev can express the contract-proved compiler *writing itself*. The
capability-frontier doc (line 66-77) argues: SSA dominance, use-def
consistency, and pass-pipeline invariants expressed as *contracts on the
compiler itself*, checked at the compiler's own compile time. "Contracts as
fuel" applies recursively to the compiler writing itself. C++/LLVM-class
systems cannot claim this rigor class — C++ has no contract surface on the
compiler's own passes. This is the one place Briev would be strictly more
rigorous than its C++/LLVM ancestry, not merely comparable.

## The honest counterweights

- **"Incapable" is architectural, not absolute.** CUDA can *do* all of
  these with enough hand-written scaffolding. The claim is they are
  **automatic, provable, and derived** in Briev's model and **manual,
  fragile, and forever-re-written** in CUDA's.
- **What CUDA has that Briev doesn't yet**: maturity, Nsight profiling,
  15 years of tuned libraries, and the 4096³ register-blocking edge
  (measured: cuBLAS 26.2 vs Briev 23.4 TF @4096³ — a kernel-efficiency
  gap, at the Briev kernel's architecture ceiling of 84.5% of the cuBLAS
  anchor). The residual gap to CUDA is *ecosystem*, not mechanism
  (capability-frontier line 161).
- **Every capability listed is proven on at least one path** — but the
  *general* claim (all of it on arbitrary programs) is the future, not the
  present. The honest bar is set by the 4096³ gap: closing it needs a new
  kernel architecture, not tuning.

## One-sentence synthesis

> CUDA's unit is the kernel a human wrote; Briev's unit is the computation
> a compiler proves and schedules. Everything Briev's architecture enables —
> proof-based correctness, program-derived scheduling, shape-driven strategy
> synthesis, reference-verified codegen — is exactly what CUDA's architecture
> has no mechanism to do automatically.