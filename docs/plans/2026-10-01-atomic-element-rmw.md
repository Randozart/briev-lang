# Atomic element RMW on all backends — `AtomicAddAt#` first (ledger gap #2)

**2026-10-01.** Status: **active**. `lib/std/atomic.bv` claims "supported
by all backends"; the primitive-coverage ledger (gap #2, HIGH) records
the truth: zero atomics on either GPU lane. This plan closes the gap,
starting with the kernel-critical op.

Companions: `primitive-coverage.md` (gap #2), `briev-capability-frontier.md`
(contracts-vs-UB), Rule 22 (no implicit concurrency — the atomic call is
the explicit classification), `gpu-syntax-decision-record.md` (D18
diagnostics).

## 1. Verified facts (file:line)

- **The existing contract is pointer-based and i64-only**:
  `AtomicAdd#(ptr-as-Int, val[, ordering]) -> old` — interpreter
  `interpreter/intrinsics.rs:490` (heap RMW at address), LLVM
  `llvm/intrinsics.rs` `emit_atomic_rmw` (`inttoptr` + `atomicrmw`),
  ordering args `relaxed/acquire/release/bartered/seq`, default seq_cst
  (2026-09-06 cpp-expressiveness plan). These stay UNTOUCHED.
- **`AddressOf#` is NOT an address-of-expression**: it resolves a STRING
  literal through the embedded MMIO table
  (`interpreter/intrinsics.rs:382` → `resolve_address_for_interp`,
  `address_resolver.rs`) — "uart" → a device address. Reusing it for
  element addresses would corrupt the embedded contract. Route closed.
- **`Ptr#` is an identity cast** (`interpreter/intrinsics.rs:224`) — no
  element-address intrinsic exists.
- **GPU Float arrays are f32 storage; CPU Float is f64** — a float
  atomic would differ across backends by storage width. Int is i64 in
  both worlds (`elem_bytes: 8`).
- **SPIR-V readiness**: `Int64` capability already declared
  (`spirv/builder.rs:49`); `OpAtomicIAdd` on a StorageBuffer element
  needs the `Int64Atomics` capability (rspirv has it) + the
  `shaderBufferInt64Atomics` device feature at pipeline creation
  (`gpu_rt.rs` — must verify the feature chain; RTX 3060 supports
  VK_EXT_shader_atomic_int64). `spirv-val` gates the binary.
- **PTX readiness**: `atom.add.u64`/`atom.exch`/`atom.cas.b64` with
  `.acq_rel.gpu` scope (sm_60+); the elementwise emitter already
  computes element addresses (`mul.wide.u32` + `add.u64`, seen in the
  work-id fixture dump).
- **The analysis collision (the real design work)**:
  `collect_buffers`/`collect_stmt_buffers` (`accel.rs:1383-1421`)
  classify writes via `Assign(Index(arr,…))` — an atomic's target array
  is a whole-array CALL ARGUMENT and would be classified as NOTHING
  (omitted from layout/desc = broken). And the **disjoint-write
  eligibility proof** rejects shared writes by construction — while
  shared writes are the entire point of an atomic.

## 2. Design

1. **New element-addressed family**: `AtomicAddAt#(buf, i, v) -> old`
   (buf: Int array, i/v: Int). The kernel idiom — C++ `atomic_ref`
   vs raw-pointer `fetch_add`. The existing pointer family stays for
   systems code; the At family is the kernel surface. **Int-only first
   slice** (dodges the f32/f64 storage split; counters/flags/queues are
   the real kernel use). Family expansion (`SubAt#/CasAt#/XchgAt#…`,
   Float-at on f32 with the SPIR-V atomic-float extension) = follow-up
   I-arms, each ledger-tracked.
2. **Every backend implements the same reference semantics**
   (rule 5): interpreter = heap RMW at the array's element address;
   LLVM = GEP + `atomicrmw add i64` (default seq_cst); PTX = address
   math + `atom.acq_rel.gpu.add.u64`; SPIR-V = AccessChain +
   `OpAtomicIAdd` (device scope, SequentiallyConsistent memory
   semantics) + `Int64Atomics` capability + runtime feature.
3. **The atomic call IS the Rule-22 classification**: the eligibility
   checker exempts `Atomic*At#` target arrays from the disjoint-write
   proof — naming an atomic IS the disclosed, race-free-at-the-
   instruction-level concurrent write. Everything else keeps the
   disjoint requirement. Buffer collection adds the target array to
   read_buffers AND write_buffers (it is both).
4. **Purity**: `AtomicAddAt#` joins the accel purity allowlist
   (order-nondeterministic by contract — the author asked for it).
5. **No strategy keywords, no new config**: the op is the disclosure
   (D18: capability errors are diagnostics; the runtime feature
   mismatch — a device without 64-bit buffer atomics — is a loud
   pipeline-creation error naming the fix).

## 3. Slices (commit each; gates per AGENTS)

- **A1** — registry signature + interpreter arm (heap RMW) + LLVM arm
  (GEP + atomicrmw) + unit tests. CPU truth first (rule 5 direction).
- **A2** — analysis: purity admission + buffer collection (target array
  = read+write) + disjoint-write exemption (proof note recorded).
- **A3** — PTX arm (address math + `atom.acq_rel.gpu.add.u64`) + unit
  text tests.
- **A4** — SPIR-V arm (capability + AccessChain + OpAtomicIAdd) +
  `shaderBufferInt64Atomics` feature in the runtime pipeline + spirv-val
  + unit tests.
- **A5** — device gate: `benchmarks/atomic_gate.sh` — N workitems each
  `AtomicAddAt#(total, 0, 1)` → `total[0] == N` on BOTH lanes (a real
  concurrency check: f32-fixtured lanes would race; the Int lane must
  count exactly). Fixture `examples/gpu/atomic_inc.abv`.
- **A6** — ledger + docs close-out (gap #2 partial: Add filled, family
  remainder tracked; atomic.bv gains the At wrapper).

## 4. Risks

- `shaderBufferInt64Atomics` feature not plumbed in `gpu_rt.rs` → A4
  extends the features chain (verify against the existing feature
  creation code; RTX 3060 OK).
- rspirv `Capability::Int64Atomics` / `Op::AtomicIAdd` availability —
  core op, expected present; spirv-val is the gate.
- Disjoint-proof exemption scope: ONLY the exact `Atomic*At#` call
  shape; any other shared write stays a hard eligibility rejection
  (Rule 22 intact).
- **Interpreter array storage model (OPEN, first A1 investigation)**:
  program values are `Value::Bits(Vec<u8>)` ONLY (`interpreter/mod.rs:3`)
  and arrays appear to be INLINE Bits (`zero_bits`, mod.rs:933) — i.e.
  plausibly cloned on binding, which would break reference semantics for
  a by-array atomic (the arm would mutate a clone; the binding keeps the
  old bytes). The existing atomic arms dodge this by taking explicit
  Int addresses into the global `heap`. A1 must resolve: do top-level
  arrays live in the heap with bindings holding addresses, or inline?
  If inline: options are (a) the At arm resolves the array through the
  statement-level binding path (name-keyed mutation), or (b) kernel
  check-mode executes At ops through a small dedicated pass that owns
  the binding (never through the by-value intrinsic call path). Decide
  on evidence, not convenience — reference semantics are the contract
  (the device gate would catch a clone bug, but the interpreter must be
  right FIRST: rule 5).

## 5. Gates

Suite green; warnings 19; gemm_h byte-identity; Praetor no new rows;
device gate both lanes at A5; docs same commit.

## 6. Status

**A1–A5 DONE 2026-10-01** (commits 29fc25ad, ce05a0a1, e19ea501 + this):
- A1 CPU truth: registry + interpreter (RMW in the binding — the value-
  model question resolved: arrays are `Value::Product`, the arm mutates
  the same store the element-assign path mutates) + LLVM (state-GEP →
  element GEP → atomicrmw).
- A2 analysis: purity admission; the At-target classified read+write
  (before: invisible to collect_expr_buffers entirely); the disjoint-
  write proof bypassed BY SHAPE (the call inspects no Assign — the call
  IS the Rule-22 classification).
- A3 PTX: element address via the buf[i] math, ONE true
  `atom.acq_rel.gpu.global.add.u64`; Int-Let routing fixed (calls yield
  f32-flattened Ints, cvt into the u32 local); ptxas smoke in the test.
- A4 SPIR-V: (buf,i) synthesizes the buf[i] address expression, reuses
  emit_addr, ONE OpAtomicIAdd; Int64Atomics declared ATOMIC-SITE-ONLY
  (unconditional form broke gemm_h byte-identity — the gate caught it);
  scope/semantics as OpConstant OBJECT operands (spirv-val caught the
  literal form); shaderBufferInt64Atomics in the C probe chain.
- A5 device gate: `benchmarks/atomic_gate.sh` + `atomic_inc.abv` —
  1024 work items × 1 atomic add, `total[0] == 1024` on BOTH lanes
  (zero lost updates). PTX and SPIR-V agree bit-for-bit with the CPU
  contract.

**A6 remainder (follow-ups, each an I-arm per the ledger):** the At-
family expansion (Sub/Cas/Xchg/And/Or/Xor), Float-at on f32 (SPIR-V
needs the atomic-float extension), the pointer-based Atomic* family on
GPU lanes, non-default orderings on SPIR-V (PTX carries them via the
scope qualifier), and a stdlib wrapper (blocked: no size-generic array
params in .bv defn signatures).
