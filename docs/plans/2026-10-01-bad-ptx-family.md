# `.bad` ptx family + accel image-override bridge — plan

**2026-10-01.** Status: **active**. The L1 primitive-coverage audit
(`docs/architecture/primitive-coverage.md`) found the D26 escape hatch
(`Asm#`) unreachable on GPU lanes and, structurally, unfillable there:
`.abv` is pure dual-lane (the SPIR-V build hard-errors per node) and an
expression-level text fragment has no honest SPIR-V form. The resolved
route (user decision 2026-10-01): **the `.bad` dialect gains a `ptx`
family** and becomes the primary kernel-level GPU escape; `Asm#` stays
CPU/LLVM (ledger gap #1 stays OPEN for a possible later PTX arm — "both
eventually"); single-instruction GPU needs stay on the named-intrinsic
I-arm track. A `spirv` `.bad` family is **recorded as future, not
implemented** (SPIR-V assembly text is SSA-id/type-explicit — a poor fit
for the line-oriented dialect; the derived SPIR-V emission owns the
Vulkan lane).

Companions: `docs/architecture/bad-dialect.md` (GPU addendum, this
commit), `primitive-coverage.md` (gap #1), `gpu-syntax-decision-record.md`
(D26/D27), `2026-08-27-cbv-foreign-hardware-and-mmio.md` (foreign-unit
precedent).

## 1. Verified facts the design stands on (file:line)

- **Kernel ABI = ONE `.param .b64` (the state pointer)**:
  `lib/runtime/briev_dev_cuda.c:414` ("One param: the projection pointer
  (kernel `.param .b64 param0`)"); the derived emitters emit exactly
  `.visible .entry main (.param .b64 proj_param)` and load it once
  (`ptx/general.rs:198/112`).
- **Entry name is hardcoded `main`** in the drivers (compile.rs comment:
  "Entries stay 'main': the device drivers hardcode pName 'main'").
- **All toolchains on PATH**: `ptxas` (also probed by
  `ptx::compile_cubin`, `ptx/mod.rs:1501` — candidate list incl.
  TRITON_PTXAS), `spirv-as`, `spirv-val`.
- **The `.bv`/`.bad` integration exists**: `bad fn name(params) -> Ret
  [post] [pre] { … }` (BadFn, `ast/top.rs:1475`) →
  `compile_bad_fn_objects` (`compile.rs:2210`) → `generate_bad_fn`
  (`backend/bad/mod.rs:89`) → family-routed `.s` + assembler. `.bad`
  directives parse generically (`BadDirective`, `parser/bad.rs:401-410`)
  — geometry directives need **no grammar change**.
- **The bridge precedent exists**: compile.rs merges PTX blobs into
  SPIR-V kernels by node name (dual-image, tensor special case absorbed
  — `RunnerKernel.ptx`, geometry fields travel with the blob).

## 2. Decisions

1. **Family/target `ptx`** (family key `ptx`; `spirv` future).
2. **Authoring form**: a new optional target marker on BadFn —
   `bad<ptx> fn <node_name>(state: Ptr) { … }`. Explicit, disclosed
   (D26); `BadFn` gains `target: Option<String>` (default `None` = host
   triple, everything existing unchanged). The fn NAME is the bridge key
   and must equal an accel node's name.
3. **Kernel signature**: first slice requires exactly one `Ptr` param —
   it lowers to `.param .b64 <name>`, prologue `ld.param.b64 %rd1,
   [<name>];`, and `param_env` binds the name to `%rd1`. Any other
   signature = loud error naming the ABI (multi-param kernels are future
   work and need runner changes first).
4. **Entry naming**: the family emits `.visible .entry main` ALWAYS (the
   drivers hardcode it); the fn name is metadata for the bridge, not the
   cubin entry.
5. **Header**: family auto-emits `.version 8.0 / .target sm_86 /
   .address_size 64` — the same constant the derived lane's templates
   use today; wiring an arch config knob is a follow-up for BOTH lanes.
6. **Registers**: PTX virtual registers — dialect registers lower to
   fresh vregs by width class (Int/Ptr → `.u64` `%rd<N>`, Float → `.f32`
   `%f<N>`, comparisons → `.pred` `%p<N>`). No physical mapping rows
   (ptxas allocates) — `bad-registers.dbvl` gets only an `imm`-prefix
   row for `ptx` (empty prefix) so immediate substitution works.
7. **Geometry**: `.blockthreads <N>` / `.sharedbytes <N>` directive
   lines in the unit; defaults 64 / 0 (the lane defaults). The bridge
   carries them into `RunnerKernel.block_threads`/`shared_bytes`.
8. **ISA**: core `ptx:` rows in `bad-isa.dbvl` for the portable core
   (`mov`, `ld.global`, `st.global`, `add/sub/mul/fma` f32, `setp`,
   special-register reads `tid.x`/`ctaid.x`, `bra`, `ret`, `bar.sync`,
   shift/and for decode math). Anything beyond = `ptx =>` exception rows
   — the dialect's own raw escape (author owns fallout, per the table
   header contract).
9. **Bridge**: in the accel chain, AFTER the `build_ptx_kernels` merge:
   every `bad<ptx>` fn → `generate_bad_fn` → `compile_cubin` (PTX-text
   blob if ptxas is absent — the lane's existing fallback) → replace the
   matched node's `k.ptx` + geometry. Missing node name = loud error.
   Host-side `compile_bad_fn_objects` **skips** ptx-target fns (they are
   not host objects).
10. **Doctrine**: the escape is maximally disclosed (a whole authored
    unit, named, contract-carrying), never a hidden inline fragment;
    contracts on the unit parse and check per the existing bad contract
    machinery (register-preservation proofs degenerate on vregs — the
    param/post contracts are the author's IO statement; node-IO contract
    checking against the derived node is FUTURE work, recorded).

## 3. Milestones (commit each; gates per AGENTS)

- **M1 (this commit)** — design record: this plan, the
  `bad-dialect.md` GPU addendum, ledger gap #1 re-route.
- **M2+M3** — data rows + family: `bad-isa.dbvl`/`bad-registers.dbvl`
  `ptx` rows; `BadFn.target` + parser `bad<ptx>`; `lower.rs` ptx arm
  (vreg naming, `.entry main` prologue, param binding, directive pass-
  through); `assemble()` ptx routing (reuse `compile_cubin`; PTX-text
  fallback). Tests: parse, lower text (entry/param/vregs/row ops),
  exception rows, missing-row capability error, assemble smoke (ptxas
  present → cubin bytes; absent → text).
- **M4** — bridge: override pass in the accel chain, host-object skip,
  loud missing-node error. Tests: unit + fixture `.abv` (runner.c
  embeds the authored blob with declared geometry; derived SPIR-V image
  intact). **Device**: trivial override node (scale) vs derived
  reference, BOTH lanes — the kernel-rule on-device gate (this touches
  dispatch/merge).
- **M5** — example (`examples/gpu/bad_override.abv` + unit), BACKEND-
  SUPPORT-MATRIX `.bad`/PTX note, ledger close-out, plan DONE entries.

**Gates every commit**: suite green, warnings 19, gemm_h byte-identity
(the bridge is default-inert with no `bad<ptx>` fns), Praetor no new
rows; device check at M4.

## 4. Risks

- Runner ABI drift — mitigated: M1 pins it file:line; M4 device check.
- ptxas arch — same constant as the derived lane; knob follow-up.
- ISA-table gaps — `ptx =>` exceptions cover anything unrolled.
- Bridge/merge interactions (split nodes, tensor lanes) — override runs
  after the existing merge and refuses (loud) to override a node whose
  PTX image is a split partial/combine set (companions are per-kernel
  blobs; overriding a partial would break the two-launch contract).

## Amendment 2026-10-01 (M4 gate findings)

The device gate surfaced two M3 defects, both fixed the same day:

1. **Geometry-directive emit leak** — `.blockthreads`/`.sharedbytes`
   were consumed in pass 1 but `emit_directive`'s default arm pushed
   them verbatim into the PTX; ptxas rejected the unknown directive and
   the bridge silently fell back to text. Fixed: emit-side skip arm +
   `ptx_unit_with_geometry_compiles_to_cubin` regression test.
2. **Undeclared virtual registers** — the §1 "ptxas allocates" note was
   wrong: ptxas allocates PHYSICAL registers only for DECLARED virtuals.
   Fixed data-driven: the `declare` row (bad-registers.dbvl) carries the
   per-family register-bank declarations (`.reg .b64 %rd<16>;` bank
   form; `%p1`/`%rt1`/`%fs1` reserved temps), emitted by the kernel
   wrapper. Also: the dialect `mul` row needed `mul.lo.u64` (PTX integer
   multiply requires the `.lo/.hi` qualifier; `mul.u64` is invalid).

Bridge device gate: `benchmarks/bad_ptx_gate.sh` — authored CUDA unit
vs derived SPIR-V image of the same copy node, BOTH lanes PASS with
exact equality (0.00e+00).
