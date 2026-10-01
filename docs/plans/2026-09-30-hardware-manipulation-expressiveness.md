# Hardware-manipulation expressiveness — plan

**2026-09-30.** Status: **active plan — syntax decisions resolved** in
`docs/architecture/gpu-syntax-decision-record.md` (D1–D27, A1–A10). To be
able to beat hand-written CUDA, Briev must be able to **manipulate the
hardware** as directly as CUDA/CUTLASS and beyond — while preserving the
derivation doctrine.

**Companions:** `derivation-not-recognition.md` (derive; recognition is
the sin), `keyword-taxonomy.md` (strategy vs ambiguity keywords vs
intrinsics), `briev-capability-frontier.md` (expressiveness closure),
`abv-gpu-doctrine.md` (one plan, per-vendor projections),
`gpu-backend-strategy.md` (the CUDA levers: cp.async, ldmatrix, mbarrier,
wgmma, TMA, fragment control), `docs/plans/2026-09-30-kernel-plan-and-per-target-lowering.md`.

---

## 1. The principle

> **Every hardware capability must be reachable** — by *derivation* where
> the compiler can determine it, by a **disclosed escape** where it
> cannot. Manipulation happens through *primitives*, *execution-model
> features*, and *declared shapes/schedules* — **never** through strategy
> keywords (behaviour-only) and **never** through recognition.

Closure is the acceptance bar: **if CUDA/CUTLASS can do X, Briev reaches
X** — derived or declared.

## 2. Why this is required

- The portable SPIR-V path cannot express `cp.async`-class pipelines, TMA,
  `wgmma`, exact fragment layouts (`gpu-backend-strategy.md` §3). Hand-PTX
  wins precisely on **fragment control** ("direct — mma fragments already
  exact" vs "LLVM NVVM owns packing, can fight exact layout"). To beat
  CUDA, Briev must have that control.
- CUDA's structural gap is the converse: it has no whole-program DAG or
  contracts, so it cannot *discover* fusion/scheduling. Briev's edge is
  that the *default* derives those; the escape covers the machine control
  the default cannot decide.

## 3. The five layers

### L1 — Primitive layer (every instruction/feature reachable)
- Existing: 173 intrinsics (work ids, barriers/fences, subgroups/shuffles,
  atomics, volatile, memory ops) + `Asm#` **two-mode** (abstract lowering
  table `config/asm-lowering.dbvl` — itself data-change-extension — plus raw).
- Requirement: **audit against each target ISA** (PTX/CUDA `mma.sync`,
  `ldmatrix`/`stmatrix`, `cp.async`/`.bulk`, `mbarrier`, `wgmma`, `tcgen05`,
  `cluster`/DSMEM, TMA, `redux`/`elect`, prefetch/L2; SPIR-V coopmat,
  subgroups, control barriers, images, spec constants; AMD/Intel analogues).
  Missing rows are **data additions**, not Rust changes.
- Doctrine: hardware primitives, disclosed (`#`), never algorithms.

### L2 — Execution-model layer (the realities)
Genuine execution contracts, not derivable from the DAG. **All in scope.**
- Grid-wide sync / cooperative launch (grid barrier).
- Persistent / grid-stride kernels.
- Warp specialization (producer/consumer).
- Clusters / DSMEM (sm_90+ distributed shared memory).
- Dynamic parallelism (device-side launch).
- Multi-GPU / peer / streams.
- Memory-ordering scopes (acquire/release/relaxed, fences).
Each is **derived** where the compiler can decide, **expressed** where it
must be declared — never silently guessed.

### L3 — Shape layer (explicit control where the model is uncertain)
CUTLASS exists because the tensor-core path must be hand-shaped. Briev
needs the same: **tile, stage depth, pipeline, fragment layout, swizzle,
loop→fundamental binding, unroll, split**.
- This is **shape**, not scheduling (preconditions are the scheduler —
  `gpu-syntax-decision-record.md` §1.5). Declared as **source node-level
  parametric modifiers** (D1): `tile<M,N>`, `stage<N>`, `unroll<N>`,
  `split<N>`, `vector<N>`, `swizzle<…>`, `fragment<…>` (D14).
- Per `keyword-taxonomy.md`, these are **ambiguity** declarations (legal
  only where the compiler's effectiveness model is uncertain); where it
  can derive the shape, the declaration is redundant and the default must
  win (a redundancy **warning**, D2).

### L4 — Fundamental-binding layer
The target profile carries **fundamentals** (instruction + structural
requirement); the compiler **binds** structures to them by requirement.
Explicit binding is the escape for *required hardware semantics* or a
derivation gap — retirement-gated.

### L5 — Derivation layer (the default win)
Whole-program DAG + proofs give Briev fusion, proof-licensed transforms,
scheduling, and per-shape/per-device selection that CUDA cannot discover.
Migrate the SPEC §9.8 tier recognizers to general derivation + the
`KernelPlan` per-target lowerings — this is where "beat CUDA by default"
is won.

## 4. Workstreams

1. **L1 primitive-completeness audit** — enumerate each target ISA surface;
   diff vs the intrinsic registry + `asm-lowering.dbvl`; fill via data rows
   + emitters. **Audit DONE 2026-10-01**: the ledger is
   `docs/architecture/primitive-coverage.md` (PTX + SPIR-V surfaces,
   registry diff — 127 signature arms vs 125 listed, `Fma#`/`Environ#`
   hygiene fixed — the dbvl state, and a 13-row gap ledger with fill
   classes: I-arms, A data rows, L2/L4/L5, and the one OPEN design — the
   SPIR-V `Asm#` form that blocks the escape hatch on GPU lanes). First
   fills: PTX `Sqrt#`/`Fabs#` parity arms; remaining fills land per the
   ledger's leverage order. **Escape route LANDED same day**: the `.bad`
   dialect's `ptx` family + the accel image-override bridge (whole-unit
   GPU escape, `benchmarks/bad_ptx_gate.sh` both-lanes PASS) — ledger
   gap #1 closed by route; the `Asm#`-on-GPU arm itself stays OPEN
   pending an honest SPIR-V fragment story.
2. **L2 execution-model features** — per feature: spec the semantics,
   decide *derive vs express*, add the primitive/declaration, gate it.
3. **L3 schedule/shape surface** — design the site-local control form
   (ambiguity keyword or metaprogrammed schedule), disclosed,
   retirement-gated toward derivation.
4. **L4 fundamentals catalog** — formalize `TargetProfile` fundamentals +
   requirements; binding in the lowerings.
5. **L5 derivation completeness** — retire tier recognizers into general
   passes; coverage ledger + recognition gate.
6. **Governance** — closure ledger, recognition gate, ambiguity
   warn/error gate, primitive-coverage ledger.

## 5. Doctrine compliance (non-negotiables)

- **Derive the default** — the fastest legal code for the declared intent
  is the compiler's, not a keyword's.
- **Disclosure** — every primitive/schedule/fundamental is marked
  (`#`/site-local), never hidden.
- **No recognition** — no algorithm-name matching; general passes only.
- **No strategy keyword for speed** — behaviour constraints only; a
  keyword-beaten default is a bug.
- **Closure** — every capability reachable; a gap is a bug, not a limit.

## 6. The central fork — RESOLVED

**Derivation-first** (doctrine): the default derives; the escape (L1/L3)
reaches anything the default can't; explicit control is the exception
(and, for shape, an *ambiguity* declared at the site). Briev's "beat CUDA"
lives in L5, with the disclosed escapes of §6 syntax.

## 7. Open questions — RESOLVED by the decision record

All six were decided in `docs/architecture/gpu-syntax-decision-record.md`:

1. **Derivation-first** (A4, §6).
2. **L3 form** → renamed **shape**; declared as **source node-level
   parametric modifiers** (D1), broad vocabulary (D14).
3. **L2 form** → **scope blocks** `scope<kind[, size]>` (D3/D5), with
   `persistent` a separate derived-by-default lifetime (D6/D9); sync via
   extended `sync<…>` (D4/D7).
4. **L1 target scope** → NVIDIA PTX + SPIR-V first (D27); primitive path
   layered (D26).
5. **Priority** → L1/L5 first (implementation plan, §4).
6. **Syntax** → the full D1–D27 record; open details listed there §10.
