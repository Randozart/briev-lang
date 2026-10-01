# L1 primitive-coverage ledger

**2026-10-01.** The D26/D27 primitive-completeness audit (workstream 1 of
`docs/plans/2026-09-30-hardware-manipulation-expressiveness.md`, decision
`gpu-syntax-decision-record.md` §10.3): enumerate each first-target ISA
surface (NVIDIA PTX + SPIR-V, D27), diff it against the intrinsic registry
and `config/asm-lowering.dbvl`, and classify every gap by its fill path
(D26: *adding a hardware primitive is usually a data row, not Rust*).

Closure is the bar (`briev-capability-frontier.md`): **a gap here is a
bug, not a limit.** This ledger is the living record — every row carries
*what / evidence / fill path*; rows move to `DONE` with a commit id when
filled.

## 0. Method and reachability classes

Evidence is `file:line` in this tree (claims verified in source, not
memory). A capability is **reachable** for a lane only if one of these
holds:

| Class | Meaning |
|---|---|
| **I** | named intrinsic (registry) *and* an emission arm in that lane's dispatcher (`ptx::general::emit_intrinsic_call`, `spirv::lower::emit_intrinsic_call`) |
| **S** | structural — the emitter emits the instruction for some family (not source-callable) |
| **A** | abstract `Asm#("Op", ops…)` row in `asm-lowering.dbvl` + a lane `Asm#` dispatcher |
| **R** | raw `Asm#("raw", template, ops…)` — the disclosed escape |
| **X** | **missing** — gap, with fill path |

Host-only intrinsics (spawn, dl*, sysctl, http, shell, string/collection
ops) are out of scope: they are CPU stdlib surface, not hardware
manipulation.

## 1. Registry state (diff result)

- `get_intrinsic_signature`: **127 arms**; `REGISTERED_INTRINSICS`: **125
  entries**. The diff found two arms missing from the list — `Environ#`,
  `Fma#` — fixed in the audit commit (data hygiene: the list feeds vocab
  completion; the const→signature direction is the tested one).
- Group counts (registered): arithmetic/bitwise/comparison, math
  (`Sqrt# Sin# Cos# Fabs# Ceil# Floor# Exp# Pow# Max# Min# Fma#`), pointer
  + memory (`Deref# … Load# Store# VolatileLoad# VolatileStore# Copy# Fill#`),
  work ids (`GetGlobalId# GetGlobalSize# GetLocalId# WorkgroupSize#
  GetGroupId# GetNumGroups# Dims#`), subgroup (the reduction trio,
  `ShuffleDown# ShuffleXor# SubgroupBallot# SubgroupBroadcast#`),
  `Barrier# Fence#`, atomics (`AtomicLoad/Store/Cas/Xchg/Add/Sub/Or/And/Xor`
  + `AtomicLoadN/StoreN`), `Asm#`, CPU systems ops.
- The plan's headline "173 intrinsics" counts protocol/hashword surfaces
  too; the signature registry proper is 127 (this ledger's basis).

## 2. `config/asm-lowering.dbvl` state

- **2 rows**, both CPU: `Prefetch` (x86_64/aarch64/riscv64), `Rdtsc`
  (x86_64/riscv64). Row shape `Op: arity; "family:template"; …`, key =
  target family prefix (`config.rs:236`).
- **No `ptx:`/`spirv:` rows exist, and no GPU lane dispatches `Asm#` at
  all** — `Asm#` reaches only the LLVM lane
  (`src/backend/llvm/intrinsics.rs:171`). The PTX/SPIR-V dispatchers fall
  through to their unsupported-intrinsic errors
  (`ptx/general.rs:2215`, `spirv/lower.rs:1244`). The disclosed escape
  hatch of D26 therefore does not exist on the GPU lanes — the single
  most doctrine-critical gap in this audit.
  - **LANDED 2026-10-01** (plans `2026-10-01-bad-ptx-family.md`,
    `2026-10-01-bad-site-blocks.md`; dialect GPU section in
    `bad-dialect.md`): the kernel-level GPU escape is the `.bad`
    dialect's `ptx` family — whole authored units assemble via
    `ptxas` into real cubins and override the node CUDA-lane image by
    name (the dual-image merge contract); the derived SPIR-V emission
    keeps the Vulkan lane. Device gate: `benchmarks/bad_ptx_gate.sh`
    — both lanes, exact equality. Target rows inside units are
    `site ... end site` blocks (the anonymous dispatch form; the
    attached-exception syntax was retired with it). A `spirv` `.bad`
    family remains future (SSA-id text model vs line-oriented asm).
  - **`Asm#` on GPU lanes: stays OPEN** ("both eventually"): a PTX-only
    dispatcher arm alone is unreachable (`.abv` is pure dual-lane,
    `spirv/runner.rs:1038` hard-errors per node; the accel purity gate
    `accel.rs:452-469` does not admit `Asm#`) — rule 7 forbids the dead
    arm. It can only land together with an honest SPIR-V fragment
    story, which remains unresolved (binary emission, no assembler).
    Revisit after the `.bad ptx` family proves out.

## 3. NVIDIA PTX surface (sm_80/86/90/100)

| Capability | Class | Evidence / gap |
|---|---|---|
| work ids (lane/cta ids) | S | structural: `mov.u32 %r1, %ctaid.y` etc. (`ptx/general.rs:113-116`); named `GetGlobalId#`/`GetLocalId#`/`WorkgroupSize#` **not** in the lane dispatcher (`general.rs:2183-2218`) → **I-fill**: small arm mapping to `%ctaid.x*BLOCK+%tid.x` and friends |
| subgroup shuffles/broadcast/ballot | I | `ShuffleDown# ShuffleXor# SubgroupBallot# SubgroupBroadcast#` → `shfl.sync.*`/`vote.sync.ballot` (`general.rs:2206`, `emit_lane_intrinsic` `general.rs:2226`, ptxas-verified) |
| warp float reductions | I | `SubgroupFAdd/FMax/FMin#` → butterfly tree (`emit_warp_reduce`, `general.rs:2291`; `redux.sync` is integer-only until sm_100 — comment `general.rs:2286`) |
| integer redux (`redux.sync`) | X | comment-only; sm_100 arm for the reduction trio is an **I-fill** (profile-gated) |
| `elect.sync` (sm_90+) | X | no emission; vote-based derivable → **I-fill** (profile-gated) |
| math Exp/Max/Min/Fma | I | `general.rs:2158-2205` (`ex2.approx` composite for `Exp#`) |
| math Sqrt/Fabs (and friends) | I | **filled 2026-10-01**: `sqrt.rn.f32` / `abs.f32` arms in the lane dispatcher (parity with the SPIR-V lane's GLSL.std.450 lowering); locked by instruction-text tests + the well-formedness guard |
| barriers (named) | S+X | `bar.sync 0` structural in the combine (`general.rs:1694`); named `Barrier#` unsupported → **I-fill** |
| fences (named) | X | `Fence#` no arm → **I-fill** (`fence.acq_rel.gpu`/`barrier` scope decision: L2 scopes refine later) |
| atomics | X | **no `atom.`/`red.` emission anywhere in `src/backend/ptx/`** while `lib/std/atomic.bv` advertises "supported by all backends" → **I-fill, HIGH**: `AtomicAdd/Sub/Or/And/Xor/Cas/Xchg#` → `atom.global`/`red.global` (+ `.acq_rel/.release` per scope) |
| volatile | X | `VolatileLoad#/VolatileStore#` no lane arm → **I-fill** (`ld.volatile`/`st.volatile`) |
| Load#/Store# (named) | X | elementwise stores are structural; an explicit `Store#` call errors → **I-fill** |
| `mma.sync` | S | tensor family (`ptx/tensor.rs`, warp-tile emitters); no named intrinsic — correct: structure, not escape |
| `ldmatrix`/`stmatrix` | S/X | `ldmatrix` ✓ (`tensor.rs:375`); `stmatrix` ✗ → **I-fill** when the store-side tile path needs it |
| `cp.async` / `.bulk` | X | comment/aspiration only (3 files); the async-pipeline lever of `gpu-backend-strategy.md` §3 → **structural L5** (tensor pipeline), not an intrinsic |
| `mbarrier` | X | no emission; instruction (L1) + ordering semantics (L2) → **split fill**: instruction arm first, semantics via `scope<…>` |
| `wgmma` (sm_90) / `tcgen05` (sm_100) | X | no emission; next-generation tensor tiers → **structural L5** (profile-gated tensor family) |
| cluster / DSMEM (`mapa`, cluster launch) | X | **L2** per plan §3 (execution model), not a primitive row |
| TMA (tensormap, `cp.async.bulk.tensor`) | X | `gpu-backend-strategy.md` async-pipeline lever → structural + L2 hybrid; fill when the S4-class pipeline lands |
| L2 prefetch (`prefetch.global.L2`) | X | tensor.rs "prefetch" is the E5a register lookahead, not L2 → **A-fill**: first `ptx:` dbvl row candidate (`Prefetch` extension or a new op) |
| `Asm#` raw / abstract | X | LLVM-only (`llvm/intrinsics.rs:171`); GPU route = `.bad ptx` family (§2, in flight); the `Asm#` arm itself stays OPEN pending an honest SPIR-V fragment story |

## 4. SPIR-V surface (Vulkan 1.1–1.3 baseline + extensions)

| Capability | Class | Evidence / gap |
|---|---|---|
| work ids | I | `GetGlobalId# GetLocalId# WorkgroupSize#` (`spirv/lower.rs:1076` arm list) |
| subgroup shuffles/ballot/broadcast | I | `ShuffleDown# ShuffleXor# SubgroupBallot# SubgroupBroadcast#` (`lower.rs:935-1075`) |
| subgroup float reductions | I | `SubgroupFAdd/FMax/FMin#` → `OpGroupNonUniformFAdd/Max/Min` (bit-exact fixed tree, `lower.rs` + comment `src/intrinsic_signatures.rs:53`) |
| math Exp/Sqrt/Fabs/Fma/Max/Min | I | `OpExtInst` GLSL.std.450 (`lower.rs:1082-1141`) |
| Load#/Store# | I | in the arm list (`lower.rs:1076`) |
| cooperative matrix (coopmat) | S | `spirv/gemm.rs` (CooperativeMatrix path, config-gated) |
| control barriers | S+X | `OpControlBarrier` only inside GEMM (`gemm.rs:501,573,2650`); no general/barrier-named path in deferred/elementwise kernels → **I-fill** for `Barrier#` |
| images | S | `OpImage*` in 2 files (image-node path) |
| atomics | X | **no `OpAtomic*` emission anywhere** → **I-fill, HIGH** (parity with the PTX atomics row and `atomic.bv`) |
| volatile | X | no `OpMemorySemantic` volatile loads/stores → **I-fill** |
| fences / memory ordering | X | no `OpMemoryBarrier`/`OpFence` → **I-fill** (scopes are L2 declarations; the instruction row is L1) |
| spec constants | X | none emitted; belongs to **L4 `TargetProfile`/fundamentals** (device caps are derived facts, not authored constants) — classify there, not as an escape |
| `elect` / integer redux | X | `OpSubgroupNonUniformBallot`-derived elect is an **I-fill**; redux has no portable op (subgroup reduce covers the case) |
| portable `cp.async`/mbarrier/TMA | n/a | not in portable SPIR-V; the vendor-projection path (`abv-gpu-doctrine.md` per-tier) carries them — PTX tier first |
| `Asm#` raw / abstract | X | **OPEN** (§2): no assembler in the emission path; design before filling |

## 5. Gap ledger (ordered by leverage)

| # | Gap | Fill | Class |
|---|---|---|---|
| 1 | `Asm#` unreachable on GPU lanes (escape hatch missing) | route LANDED: `.bad ptx` family + bridge (device-gated); the `Asm#` arm itself stays OPEN pending an honest SPIR-V fragment story | closed-by-route 2026-10-01 |
| 2 | atomics on both lanes (`atomic.bv` overclaims) | I-arms: `atom.*`/`red.*`, `OpAtomic*` | I, HIGH |
| 3 | `Sqrt#`/`Fabs#` PTX parity | I-arm (**filled 2026-10-01**, `4ce9b6d7`) | I |
| 4 | work-id names on PTX | I-arm over structural ids | I |
| 5 | `Barrier#`/`Fence#` named on both lanes | I-arms (bar.sync / OpControlBarrier + scopes) | I (+L2 scopes) |
| 6 | `VolatileLoad#/Store#`, `Load#/Store#` on PTX | I-arms | I |
| 7 | L2 prefetch, elect, int redux (sm_100) | dbvl `ptx:` row / I-arms, profile-gated | A/I |
| 8 | `stmatrix` | I-arm when the store-tile path needs it | I |
| 9 | `mbarrier` | instruction arm (L1) + `scope` semantics (L2) | split |
| 10 | `cp.async`, `wgmma`, `tcgen05`, TMA | structural tensor-pipeline work (L5, profile tiers) | L5 |
| 11 | cluster/DSMEM, grid sync, dynamic parallelism, multi-GPU, ordering scopes | L2 plan (`scope<…>`, `sync<…>`, persistent) | L2 |
| 12 | spec constants | L4 `TargetProfile` fundamentals | L4 |
| 13 | SPIR-V `Asm#` form | design OPEN (assembler vs word list) | OPEN |

## 6. How to add a primitive (D26 recap)

1. **Ergonomics merit a name?** → signature arm in
   `intrinsic_signatures.rs` + entry in `REGISTERED_INTRINSICS` + an arm
   in **each** lane dispatcher that should support it + interpreter arm
   (reference: rule 5) + tests.
2. **No name?** → a whole-kernel unit: `bad<ptx>` (author owns
   fallout) — `site <target> =>` rows for target-only text; `Asm#`
   stays CPU/LLVM (its GPU arm is OPEN, §2).
3. **Same op, several targets?** → one `asm-lowering.dbvl` data row per
   family (`"ptx:…"`), no Rust.
4. **Execution semantics (sync/ordering/lifetime)?** → L2 declaration
   (`scope<…>`, `sync<…>`, `persistent`), never an intrinsic.
5. **Structure the compiler should derive?** → kernel-plan op / lowering
   arm (L5), never a keyword.

Every addition lands with: lane dispatcher arm, unit test (text/word
assert), suite green, this ledger row updated.
