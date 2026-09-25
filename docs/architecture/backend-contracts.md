# Backend Contracts & Decision Record

**2026-08-23** · Distills every architectural decision from the backend
scaffolding effort (plans `2026-08-23-backend-scaffolding-foundation.md`
through `-webstack-v2-completion`, all CLOSED). This is the normative
reference for what each backend IS, how it integrates, and which emission
invariants are load-bearing. **Read this before touching any
`src/backend/<name>/` code.**

Regression prevention rule: if you change something documented here,
update this document in the same commit and say why.

---

## 1. Per-backend charters

| Target | Extension | Charter |
|--------|-----------|---------|
| **LLVM** | `.bv` `.ebv` | Reference implementation. Native binaries, embedded mode (`halt#`→`wfi`), wasm32 for webstack. Full language surface. |
| **VM** *(emit mode)* | none — `--backend vm` | Finish compilation on ANY machine with a tamer: one `.bounty` archive ships everywhere; macros adapt at install time. Output is `.lair` bytecode consumed by the self-hosted tamer (`lib/tamer/`) driven by `tamer/install_sim.c`. |
| **SPIR-V** | `.abv` | Standalone GPU kernels: valid Vulkan/OpenCL compute binaries validated by spirv-val. Kernel selection/bodies come from the frontend accel analysis. NOT related to `!> accel` offload (that is `BackendKind::Gpu` through LLVM). |
| **CIRCT** | `.sbv` | Synthesizable register-level hardware: MLIR (HW+Comb+Seq) that real CIRCT accepts, Verilog-exportable, simulable, synthesizable. |
| **Webstack** | `.rbv` | Rendered Briev → wasm32 via `LlvmBackend` + `GlueWebGenerator` JS shim (`src/glue/web_generator.rs`). v2 only — the TS emitter is deleted. |

Routing truth lives in `config/targets.dbvl`; dispatch in `src/compile.rs`
`codegen()`. The VM has no file extension BY DESIGN — it is an emit mode
of the same language, reachable via `--backend vm` or `brievc bounty`.

---

## 2. Uniform integration contract

1. **Analysis once.** `analyze_program` runs in the pipeline
   (`compile.rs codegen()`), never inside a backend. Every backend
   consumes the same `AnalysisResults` (dependency graph, accel entries,
   …). Backends CONSUME frontend decisions; they do not re-derive them.
2. **Capability matrix required.** Any backend without the full surface
   declares `CAPABILITIES` (see `src/backend/capabilities.rs`) and the
   pipeline runs `validate_program` BEFORE codegen. Out-of-surface
   constructs are compile errors with what/why/fix — never runtime traps,
   never silent drops.
   - ⚠️ LLVM claims full surface and skips the gate. That claim must stay
     true: the Statement::Match regression proved an unguarded "full"
     claim hides holes. If LLVM gains a gap, either fix it or shrink the
     declaration honestly.
3. **Errors channel pattern.** Constructs discovered unsupported DURING
   emission go into a backend error accumulator
   (`VmBackend.errors`, `CirctBackend.errors` RefCell); the pipeline
   hard-errors non-empty after generate. Pattern: record_unsupported()
   with dedup.
4. **Determinism law.** ANY HashMap iteration whose order feeds emitted
   output OR program rewriting must be sorted by key (or use BTreeMap).
   Violations produced different binaries per process (BUGS.md
   "rbv nondeterminism", CIRCT cell modules).

---

## 3. Cross-cutting emission laws

Hard-won in this effort; each one corresponds to a real defect class.

| Law | Failure it prevents |
|-----|---------------------|
| Result ids exist only when a result type exists | Phantom operands on result-less ops (OpUnreachable wc=2) |
| Terminators via TYPED builder methods only | Raw inserts leave selected_block open → NestedBlock panic |
| Decorations carry EXPLICIT operand lists | dr::Builder::decorate silently dropped params on the wire |
| SPIR-V globals section ordered: annotations → types/constants → variables | §2.4 layout violations ("invalid layout section") |
| Function-scope OpVariables are the FIRST instructions of entry block | "All OpVariable instructions…first block" |
| LoopMerge targets: BOTH merge and continue blocks always exist; continue carries the unconditional back-edge | Undefined forward refs / zero-back-edge loops |
| Guards: PRESET the slot, then ONE conditional set. Never two sequential whens where the second reads the mutated slot | EQ returned 0 for equal values (tamer), Abs-style logic corruption |
| Struct-of-array element reads LOAD; address-handles only for genuine structs | Pointers pushed onto value stacks |
| Contract placeholder wires forbidden — the condition op IS the definition | Duplicate result definitions |
| LLVM IR float literals always carry a decimal point (`{:?}`, never `{:e}`) | `1e-4` parses as an integer token — clang/opt reject the module |
| `%briev.field` constant order = C `BrievField` order (name, kind, host, elem, count, is_write, proj) | proj_offset at the wrong index misaligns runtime reads; opt type error |

> **2026-09-24 (stateless-defn × arena).** Stateless-body emission laws
> (stateless-defn ABI, plan 2026-09-23-frgn-elimination-round-2.md):
>
> | Law | Failure it prevents |
> |-----|---------------------|
> | A defn emitted WITHOUT `%state` (needs_state fixpoint = false) never references `%State` — `emit_arena_alloc` falls back to `@malloc` while `fun.stateless_body` is set (set/cleared by `emit_definition`) | clang "use of undefined value '%state'" — arena lowering (string concat, Alloc#) inside `define @f(i64 …)` (arena_churn/digits_of_int) |
> | Alloc# strategy gates that choose Arena (`arena_ptr_idx`, analysis strategy, explicit `Arena`) require `!stateless_body`; stateless falls back to malloc/alloca and bookkeeps **Malloc** | Arena bookkeeping on a malloc'd pointer → Free# skips `@free` → leak |
> | The arena-fields-in-%State decision (`needs_arena`/`arena_ptr_idx`) is program-wide; the stateless-vs-stateful decision is per-body — emission must honor BOTH, never infer one from the other | The AST fixpoint runs before arena lowering exists; inferring "arena fields ⇒ has %state" reintroduces %state into stateless signatures |
>
> **2026-09-24 (name shadowing).** Name-resolution law: a LOCAL binding
> (defn/txn param or `let` register — `fun.let_bindings`) SHADOWS a
> top-level pooled instance or field of the same name at EVERY resolution
> site. `instance_prefix_for` resolves local-first; call-argument
> `box_pooled` and the foreach-iterable guard skip the global path when
> `get_local(name)` is present. Only the generic identifier arm was
> local-first before; the three global-first sites boxed the pooled
> columns into a stateless body (clang `%state` error) and passed the
> global instance where the source passed a local param
> (hash_ops_idio/float_fmt — BUGS.md 2026-09-24).
>
> **2026-09-24 (block-tracking truth).** `FunctionContext.cur_block` MUST
> always name the block the append-only output buffer is actually open on.
> Inlined member bodies leave their final `match_end`/`foreach.end` region
> open (unterminated, for the caller's continuation) — restoring the
> pre-call block after `emit_member_body` makes every loop-boundary phi cite
> a block that never branched: countdown `init_pred` (`%entry` vs
> `.match_end_N`) and latch `body_final` (`.cdb_` vs `foreach.endN`) → clang
> "PHI node entries do not match predecessors". Rule: NO cur_block
> save/restore across inlined bodies; the 2026-09-08 cursor-foreach leak
> class is covered by the `let_binding_allocas` + `foreach_break_labels`
> restores (keep those). Cross-function: `cur_block = None` at every
> function start. Undo path: any reintroduced restore needs a failing IR
> test naming the bad predecessor (BUGS.md 2026-09-24).
>
> **2026-09-24 (statement-match routing).** Every hand-rolled statement
> walk (loop engine `emit_countable_body`, `emit_guard_body_stmt`,
> `emit_guard_block`, and any new walker) MUST route statement-position
> match — both `Expr::Match` inside `Statement::Expression` and direct
> `Statement::Match` (composite-produced, `plugin/composite.rs`) — to
> `emit_statement` (the `.smt_*` statement path; `emit_stmt.rs` 2026-09-14
> conversion). A generic `emit_expr` fallback probes valueless arms as
> Void and emits invalid `phi void`; a `_ => {}` catch-all silently drops
> the match (the 2026-08-23 callable-txn class). `term`/`endprogram`/
> let-value positions stay on `emit_expr` — they are VALUE positions.
> `emit_match` additionally never emits `phi void` (Void merge = label
> only; the typechecker rejects reading the Void result). Failure:
> clang "void type only allowed for function results" (enemy_swarm,
> BUGS.md 2026-09-24). Undo path: failing tests
> `test_statement_match_valueless_arms_emits_no_void_phi` +
> `test_void_match_value_emits_no_phi`.
>
> **2026-09-06 (plan 2026-09-06-cpp-expressiveness.md).** New emission laws
> for the C++-expressiveness surface:
>
> | Law | Failure it prevents |
> |-----|---------------------|
> | Pointer intrinsics (`PtrAdd#`/`PtrSub#`/`PtrDiff#`/`PtrEq#`/`PtrLt#`) treat bases as BOXED i64 handles — unconditionally `inttoptr` in, `ptrtoint` out; `&` boxes and `as Ptr<T>` retypes without changing the SSA value | `getelementptr` on an i64 SSA value — clang rejects "defined with type 'i64' but expected 'ptr'" |
> | `PtrAdd#`/`PtrSub#` always emit `getelementptr inbounds` | Silent OOB arithmetic escaping the inbounds UB contract |
> | Atomic orderings flow from the field carrier (`field:ordering`) and intrinsic ordering args (`ordering_arg()`); default `seq_cst` | Silent ordering downgrade or invalid IR (`fadd fast` on i64) |
> | SIMD intrinsics gate pointees at the TYPECHECKER (`Ptr<scalar>` = universe entry with no fields); element shape derives from storage size + float bits at the emitter | Element-wise arithmetic on struct pointees; unwarpable LLVM types |
> | SIMD chunk emission: loads → compute → store within one chunk (overlap-safe) | Aliased dst corrupting reads within a chunk |
> | Runtime-count SIMD loops use alloca induction variables, never phis from an unlabeled current block | Phi predecessors referencing the caller's unnamed block — invalid IR |
>
> **2026-09-06 (ISR plan, docs/plans/2026-09-06-isr-handlers-and-sections.md).** ISR emission laws:
>
> | Law | Failure it prevents |
> |-----|---------------------|
> | The mechanism resolves explicit-`<>` → profile (`ctx.isr_mechanism`) → error naming both fixes; validated against config/isr-targets.dbvl at the TYPECHECK | The asm<target> dead-data gap — unvalidated target strings reaching codegen |
> | The calling convention comes from the mechanism row (`IsrConv`), never name-matched in the backend | Convention drift between config and emitter |
> | Vector table entries are plain `ptrtoint (ptr @h to iN)` constexprs; the linker applies the Thumb bit via symbol relocation | LLVM 18 rejects `or` constexprs in globals; faking the bit in IR fights the linker |
> | The default handler's spin loop branches INTO the spin label from entry | Entry block with a self-predecessor — "Entry block to function must not have predecessors" |
> | When a program declares ISRs, state is the GLOBAL `@__briev_state`; every `%state = alloca` site aliases it via emit_state_base (zero-GEP) | ISR bodies (hardware stack) reading main's frame alloca — undefined `@field` loads |
> | ISR bodies + contracts are liveness roots in compute_referenced_fields | ISR-only counters eliminated as dead → undefined `@field` references |
> | `-> Void` definitions emit `ret void` | `ret void 0` — clang rejects |
>
> **2026-09-06 (Phase 8, section placement).** `section(".name")` on defn/const
> emits the LLVM section attribute (`section "..."` on the define / on the
> `@constant` global); the transitive no-alloc proof (check_section_proofs)
> runs at the TYPECHECK — the backend consumes the placement without re-proving
> it (frontend-driven dispatch).
>
> **2026-09-25 (accel descriptor + probe sandbox, BUGS.md 2026-09-25).**
> Three laws from the baseline-sweep accel bugs:
>
> | Law | Failure it prevents |
> |-----|---------------------|
> | The emitted `%briev.kernel` descriptor table (kernel.rs) is the LLVM mirror of the shared `BrievKernelDesc` ABI (`lib/runtime/briev_accel_rt.h` + `src/accel_rt.rs`) — any member added to the ABI is added to the table in the SAME commit, with the entry tail explicitly zeroed per the documented zero contract (`n_images 0`, `ptx null/0`, `program_bytes 0`, `seed_fields null/0`, `block_per_workitem 0`). Pinned from both sides: `test_kernel_desc_abi_layout` (member list + `size_of`/`offset_of`) and `accel_rt::desc_abi::test_briev_kernel_desc_abi_pinned` | Reader reads `ptx`/`ptx_size` past a 5-member entry → garbage memcpy size → SIGSEGV (nbody_newton_accel, 2026-09-25) |
> | Anything that runs txn bodies OUTSIDE the reactor (probe lanes, self-test, future replay tools) must leave NO global trace: `briev_accel_run_probe_<name>` snapshots `@briev_begin_<name>` before the lanes run and restores it after the verdict commit (bodies with a beginprogram precondition carry the goal check, which clears the REAL flag when the goal is met on the probe's state COPY) | Probe clears the entry flag → the real reactor's node never fires again → busy hang (nbody_newton_accel, 2026-09-25) |
> | Module-level derived constants (`state_size_bytes`, `state_ptr_param`) are computed AFTER every pass that grows their inputs (`build_field_index`, synthetic field registration, `apply_field_modes`) — never at the top of `generate()` | Frozen empty-world answer: probe received `state_size 0` for every program → 1-byte lane buffers → OOB gate → coin-flip verdicts, nondeterministic `0` output |


---

## 4. VM / tamer specification

### 4.1 `.lair` format (writer: `vm/assembler.rs`; reader: `tamer/install_sim.c`, `lib/tamer/loader.bv`)

```
header: "LAIR"(4) ver(u32)@4 endian(u8)+rsvd@8 flags(u32)@12
        then u64 section offsets/sizes @16..80:
        str_off str_size fn_off fn_size bc_off bc_len host_off host_size
strings: NUL-separated name table
fn entry: 20 B = name_idx(u32) bc_offset(u64, SECTION-relative)
          + bc_len(u32) local_count(u16) arg_count(u16)
host entry: 12 B = name_idx(u32) id(u32) arity(u32)
bytecode: opcode stream (opcodes in src/backend/vm/assembler.rs)
header total: 96 bytes
```

- **Addressing is BASE-ABSOLUTE**: readers index from the lair pointer;
  fn-table `bc_offset`s are section-relative and converted by the
  producer (manifest `entry_bc` ships ABSOLUTE).
- Multi-byte readers stitch BYTES via read_u8 (unaligned-safe).
- Host-table lookup lives in C (`briev_rt.c briev_host_arity_of`) — the
  Briev-side struct walk across the defn boundary was unreliable.

### 4.2 Canonical host-service ids

`Print#=0, Log#=1` (src/backend/vm/mod.rs). Unknown services ≥1000,
first-use order among themselves, rejected loudly at run time
(`briev_host_fail`). Names ride in the string table for diagnostics only.
Arity is recorded per call site into the 12-byte host entry — the
interpreter pops exactly that many slots.

### 4.3 Export ABI (Briev → native)

EVERY exported defn takes a LEADING `%state` pointer. C drivers calling
exports MUST declare it; omitting shifts every argument by one register
and produces phantom behavior. State writes from defn bodies do NOT
round-trip through the %state view — C owns memory/tables; Briev exports
pure functions (the `step()` pattern).

### 4.4 Bounty archive

Magic `BOUNDATA\0`(9) + ver/flags/count (u32 ×3 = 21 B header);
per-section `type(1)+offset(u64)+size(u64)` = 17 B entries; data at
absolute file offsets. Manifest JSON includes `entry_bc` (absolute byte
offset of the user entry function — user fns emit AFTER prelude helpers).

### 4.5 Conformance metric

`tools/parity_harness.sh` + `parity_expected_values_match_independent_evaluation`.
Corpus: `tmp_fixtures/parity/*.bv` (OWNED by this plan — sweeps must not
delete). One fixture per opcode family as opcodes grow. EXPECT lines are
locked against an independent evaluator, so host semantics == baked
contract == tamer behavior.

---

## 5. SPIR-V specification

- **Typed dr::Builder emission ONLY.** Raw insert_types_global_values of
  types/decorations/constants caused a seven-defect stack (duplicate ids,
  dropped decorate params, misordered layout). TypeCache replaced by a
  Briev-type dedup map over typed helpers; single id space.
- **build() buckets the globals section**: decorations → types/constants →
  variables (SPIR-V §2.4 puts annotations in their own section).
- `instr()` allocates a result id ONLY when a result type exists.
- LoopMerge names merge AND continue; both blocks always emitted; the
  continue block carries the unconditional back-edge (exactly one).
- Function-scope OpVariables pre-declared in ENTRY (locals pre-scanned via
  `collect_locals`); values stored later.
- Kernel selection/bodies from `AnalysisResults.accel`: eligible
  AccelEntries only; body = `shape.kernel_stmts` (PROVEN statements);
  `shape.index_var` binds to GetGlobalId(0). A GLCompute invocation IS one
  work item — no induction loop.
- Entry-point interface lists every Input variable + the SSBO variable.
- State = ONE Block-decorated StorageBuffer: fields sorted by name,
  explicit MemberOffset layout, ArrayStride on arrays, descriptor set 0
  binding 0. Reads/writes via AccessChain (+Load/Store).
- **`.bv` dispatch wrapper gate** (2026-09-02,
  `--accel-cpu-fallback N`): the `@txn_<name>` wrapper's GPU lane checks
  the WORK-ITEM count (not workgroup count — the wrapper runs before the
  blob's LocalSize is known; work items are the runtime's dispatch
  argument) against the threshold. Const-bound shapes fold at compile
  time (`br` straight to the CPU lane); runtime-bound shapes emit
  `icmp uge` + branch. `.abv` is GPU-only by charter — the flag never
  touches it. Probe functions are not threshold-gated in v1 (the probe
  measures both lanes and commits a verdict regardless).
- **Load#/Store# take ADDRESS EXPRESSIONS** (2026-08-26, plan §2.3):
  `Load#(field)`, `Load#(field[i])`, `Store#(field[i], v)` lower to
  SSBO AccessChain + OpLoad/OpStore; scalar state fields join the
  collected buffer surface. Numeric addresses do not exist in Vulkan
  address space — a non-address argument is a capability error naming
  the valid forms, and a byte-width argument must match the declared
  element size.
- **Scalar types are universe-driven** (2026-08-26, plan §2.4): the
  casting graph's SPIR-V table resolves (protocol, metadata) → shape.
  Briev `Int` emits `OpTypeInt(width, signedness=1)`, `UInt` signedness=0;
  Bool is OpTypeBool (not an i8). Int widths 8–64, Float widths 32/64;
  heap categories (String/Blob/Char/Data) and out-of-range widths are
  capability errors naming the category and fix. No type-name matches in
  the emitter.
- **Typed volatile access**: `VolatileLoad#(Ptr<T>) -> T`,
  `VolatileStore#(Ptr<T>, T) -> Bool` (2026-08-27) lower through the
  boxed-pointer ABI; width/alignment from the declared pointee via the
  casting graph. Raw i64 addresses belong to `Load#`/`Store#`.
- **Foreign imports** (2026-08-27, plan 2026-08-27-cbv-foreign-hardware-
  and-mmio.md Slice A): `extern Name(ports) -> outs from "path";` emits an
  `hw.module.extern` blackbox whose port list uses the ONE-paren-list
  grammar (inputs `%`-named, outputs BARE-named — no `-> (outs)` wrapper;
  that form belongs to `hw.instance`). Implicit `clock`/`reset` injection
  matches defined cells so instance sites are shape-identical. Referenced
  HDL copies beside the output (missing file = hard error). Extern cells
  contribute NO program-visible variables.
- **MMIO pins** (Slice B): numeric `@`-addressed triggers are top-module
  INPUT ports emitted ADDRESS-SORTED from the live `mmio_vars` table
  (deterministic layout rule). Dynamic/symbolic trigger addresses are
  capability errors (hardware pins are static). Pin assignment is a
  typechecker-level input-pin error.
- **Device-local residency ABI** (2026-08-31, plan 2026-08-31-gpu-next):
  the runtime driver keeps TWO buffers per kernel — a DEVICE_LOCAL working
  set the shader actually reads/writes (bound in the descriptor set), and a
  HOST_VISIBLE|HOST_COHERENT staging window the runtime writes. Seed and
  scalar counters cross inside the dispatch submission (vkCmdCopyBuffer with
  transfer→compute barriers); results stay in VRAM until
  `briev_accel_download` pulls them (`download_dev` op). An all-host
  fallback (no VRAM type / no 2D op) preserves the pre-residency behavior.
  Rationale, measured: a mapped-host SSBO makes every array read cross PCIe
  (~4GB/s effective — GEMV 15.5ms); VRAM residency cut it to 0.9ms
  (~38 GFLOP/s). "Device residency" means VRAM, not mapped host memory.
- **Driver traps (recorded, do not regress)**: a VkBufferMemoryBarrier with
  buffer = NULL is INVALID — the driver silently drops the entire command
  buffer (symptom: outputs stay zero, submit "succeeds"). The runner's
  runtime files are COPIED next to the generated runner: recompiling a
  runner after editing lib/runtime without recopying runs stale code.
- **Dispatch geometry** (§2b of gpu-next): the kernel may reconstruct its
  linear index as `i = gid.y * cols + gid.x` when the analysis derives
  `work_cols` from the body's own shift/mask decomposition. Correct under
  ANY launcher covering the total count (flat 1D gives gid.y == 0). The
  bounds guard is REQUIRED for 2D shapes (tail reaches cols-1 items) and
  for literal counts not divisible by the workgroup size.
- Canonical host ids mirror the VM table (`GetGlobalId#` etc. lower to
  BuiltIn inputs here).
- Validation: spirv-val on every emitted binary + spirv-dis structural
  sweep (GLCompute entry point, LocalSize mode, Block decoration,
  StorageBuffer class, DescriptorSet 0 / Binding 0) via the shared
  `validate_and_disassemble` helper (`test_harness_structural_sweep_on_scale_kernel`).
  Vulkan-runner smoke test is probe-gated (skips loudly without a runner).

---

## 6. CIRCT specification

- **Ports:** clock is `!seq.clock` (never i1); triggers/state render via
  mlir_type.
- **Sequential semantics (wire-map v2, 2026-08-25 — registers FIRST):**
  hw.module bodies are MLIR graph regions; circt-opt accepts
  use-before-def (probed). Phase A — init constant + `seq.firreg` per
  output var, each register consuming a forward-named `<var>_next` wire;
  the wire-map points at REGISTER OUTPUTS from the start. Phase B — txn
  bodies emit against live register outputs; assignments compute new
  wires and repoint pending (reads see cycle-start state = NBA
  semantics); guarded bodies mux on condition. Phase C — defines the
  forward-referenced `<var>_next = mux(%reset, init, pending-or-reg)`
  wires. Reset forces init; unwritten vars hold.
  The previous scheme (bodies read init constants) folded guards to
  compile-time constants and fired transitions unconditionally — a
  guarded counter ran past its bound, diverging from the interpreter at
  the bound cycle. See BUGS.md (2026-08-25).
- **Contract obligations as ports (2026-08-25):** pre-guard evaluated on
  live state GATES the commit: `commit = mux(pre_ok, computed_next,
  current_reg)` — refusal holds state and raises `halt` (registered OR
  of refusals). Post-guard is evaluated on COMMITTED next values via a
  shadow wire-map, as an implication `¬pre ∨ post` (a refused txn
  carries no post obligation), ANDed into the `check` output port.
  Toolchain findings driving this form: pinned build has NO `seq.assert`;
  module-level `sv.assert` is rejected ("non-procedural region");
  procedural `sv.assert` inside `sv.alwaysff (posedge %i1)` works but
  would need an extra i1 event input — ports need none and are
  simulatable everywhere. An inconsistent contract pair (post falsified
  by an accepted commit) shows up as a check drop in simulation — that
  is the violation signal, and sim parity caught exactly this on the
  counter fixture's original `[c<255][c<255]` pair.
- **seq.firmem memory macros + loud-default policy (2026-08-25,
  seq-firmem plan):** state arrays past `circt.firmem_min_depth`
  (config, 64) with zero init, single writer, and no postcondition
  element references default to the macro; everything else register
  file. `mem let` / `reg let` pin per-variable (pins are validated —
  impossible pins are capability errors naming the fix). Default-policy
  decisions surface in ONE aggregated note naming each array + reason;
  explicit pins silence it. Latency-0 reads keep cycle-exact parity
  with the interpreter; the §3.4 commit gate rides the write enable.
  Export: SeqToSV lowers firmem to an externally-generated module —
  brievc emits reference-implementation companions (`<var>_<D>x<W>.sv`)
  linked by the harness (BUGS.md: circt-opt cannot emit these bodies).
  Vivado A/B on xck26: 17 FF + 10 LUTRAM (macro) vs 522 FF + 242 LUT
  (registers).
- **Multi-txn arbitration:** several txns writing one var resolve by
  program order (last commit gate wins). Single-txn-per-var is the
  tested surface; beyond it stays honest via this documented rule.
- **Type lowering:** universe-driven (rule 19) — width from rt.bytes,
  signedness from protocol category (`Cast.Int`→siN, `Cast.UInt`→uN,
  `Cast.Float`→fN); BitRange widths honored. REQUIRES the normalizer
  keep-set to preserve `Cast.*` properties (they were being stripped —
  that was the "everything renders i64" bug). Top-level `let` IS state
  and must reach var_types.
- **Honest comb subset ONLY:** add/sub/mul/divu/divs/modu/mods/shl/shru/
  and/or/xor/parity/mux/icmp/neg/hw.constant. comb has NO ctpop/ctlz/
  cttz/rev/sqrt/sin/cos/pow/floor/ceil — those arms emitted unparsable
  MLIR and are deleted; unknown intrinsics record capability errors.
  Abs# lowers honestly (neg+icmp+mux).
- **No silent drops:** every unsupported construct records into
  `errors` (RefCell) → pipeline hard-error. cell_defs iterated SORTED.
- **Contracts:** see "Contract obligations as ports" above — pre gates
  the commit (refusal ⇒ hold + halt), post is an implication on
  committed values into `check`.
- `UInt[N]` source syntax is an ELEMENT ARRAY (Vector), not a sized
  scalar; Constrained/BitRange types are currently programmatic-only
  (parser gap, logged).
- Validation (2026-08-25, all tiers LIVE): probe-gated `circt-opt`
  round-trip (`test_emitted_module_parses_under_circt_opt`,
  `circt_tools_available()`); full chain in `tools/hw_harness.sh`
  (parse → lower-seq-to-sv + export-verilog → verilator lint →
  SIMULATION PARITY — 270-cycle verilator --binary run diffed against
  locked `.expect` sequences derived from interpreter semantics,
  generators in `tmp_fixtures/hw/*.expect.gen.py` → optional Vivado);
  synthesis via `tools/vivado_check.sh` on KV260 xck26-sfvc784-2LV-c —
  generated counter FSM synthesizes to 21 LUTs
  (`VIVADO_SYNTH=1 VIVADO_REPORT_DIR=... tools/hw_harness.sh`). HW corpus:
  counter, adder, array (register file), watchdog (countdown monitor).

---

## 7. Webstack specification

v2 only: `LlvmBackend` (wasm32-wasi target) emits the module;
`GlueWebGenerator` emits the JS shim; flush batching implemented in
`emit_stmt.rs` (`__web_flush_buf` / count-parameterized
`__web_flush_state` at term boundaries). AddressOf# is implemented in
LLVM intrinsics (eprintln diagnostics remain an opportunistic sweep).
There is no TS emitter.

---

## 8. Known limitations (open, deliberate)

1. **Convergent-txn loop engine** — a convergent txn whose guard never
   changes across iterations may spin (param-slot stores not persisting
   was observed pre-refactor; superseded architecture means it is
   UNVERIFIED today). Re-run the lib/tamer/vm.bv vm_loop repro before
   trusting either verdict. Related: Version-DAG phi fix (CLOSED).
2. **Sized-scalar source syntax** — BitRange types unreachable from the
   parser (programmatic-only). Spec decision needed.
3. **Vulkan smoke test** — needs an installed runner.
4. **LLVM diagnostics sweep** — eprintln warnings (AddressOf#, main.rs
   unused) remain an opportunistic house-style pass.
