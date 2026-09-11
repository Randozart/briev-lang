# OS Capability Frontier — Briev vs C/C++ (STATUS: SKELETON, Phase 5 pending)

**Status**: This is the skeleton of the Phase 5 deliverable from
`docs/plans/2026-09-11-rv64-capability-kernel.md`. It records the
requirements and verified findings so the final doc cannot inherit a
known assessment error. It does NOT yet contain the demo evidence —
the rv64 capability-kernel plan has NOT been started.

## 1. The verdict, locked (plan Addendum A, §6.3)

> Briev's capability deficit vs C/C++ = **ecosystem maturity + the
> systems-plumbing last mile — not language mechanism.**

This framing is locked. Any future revision must preserve the split
below and re-derive it from evidence, not memory.

## 2. Mechanism vs accumulated practice (the row that must never be compressed)

| Axis | Mechanism winner | Evidence |
|---|---|---|
| Compile-time computation | Briev | `$const`/`$let`/`$defn` erased pre-runtime (SPEC §18.1) vs `constexpr` |
| Staged execution | Briev | 11 explicit stages `PreLex…Linked`, user `$(Stage)` blocks (`src/plugin/mod.rs`, `src/parser/definitions.rs:1344`) — template instantiation is implicit and unstagable |
| Program representation access | Briev | Live-AST DSL (`Tag$`/`Named$`/`ForEach$`/`Insert$`/`Delete$`/`Set$`) — C++ TMP computes types, never sees the AST without libclang |
| Hygiene | Briev | Quotation/interpolation on AST values, hygiene preserved unless explicitly waived (SPEC §18.4) — C/C++ macros are textual, unhygienic |
| Metaprogramming security | Briev | Privileged macros declare capabilities; `macro-lock.toml` grants, diffs shown on change (`src/macros/lockfile.rs`) — no C/C++ equivalent |
| Compile-time failure semantics | Briev | `Error#` with reachability analysis, usage-gated sealed members (SPEC §18.6, PiggyBank) vs `static_assert` |
| Derivation | Briev | `:=` reference-impl synthesis, contract-checked (SPEC §18.5) |
| Generic-library gravity | C++ | 40 years of Boost/Hana/ranges-class machinery, mature overload/SFINAE/concepts resolution, battle-tested diagnostics — accumulated practice, not expressiveness |

## 3. Capability inventory (verified 2026-09-11; re-verify before citing)

Where Briev contends with or exceeds C/C++:
raw throughput (LLVM `-O3 -flto` parity enforced per-benchmark),
allocation (brk arena 2.5× faster than C malloc, native-runtime branch),
memory safety (contracts, dangling-pointer hard error, compile-time
no-alloc proofs — beyond both, not opt-in), GPU (`.abv` → SPIR-V + PTX,
cross-vendor), hardware synthesis (CIRCT/MLIR), concurrency intent
(Rule 22 classification demanded, never guessed), bare metal (Cortex-M
path real; rv64 gap = plumbing, see plan), multi-backend (LLVM, WASM,
SPIR-V, MLIR, PTX).

Where it does not yet contend: ecosystem gravity (libraries, tooling,
debuggers, sanitizers), kernel/OS class (hosted POSIX userspace proven
by the libc-free native runtime; bootable image + kernel = the rv64
plan's territory), template-era generic libraries (practice, not
mechanism), maturity surface (single young compiler), debugger/sanitizer
story.

## 4. Requirements for the final doc (from plan Addendum A)

1. The §2 split (mechanism vs practice) stays explicit — never one row.
2. **Debugging/probes frontier row**: reflection-driven probe generation
   exists (`lib/std/dwarf.bv:17`); kernel gap = GDB stub / QEMU `-s -S`
   integration, likely closable via reflection + staged system rather
   than new backend code.
3. The verdict framing of §1 with the rv64 capability-kernel plan as the
   evidence vehicle for the last-mile half.
4. Tier-3 kernel gaps table (S-mode/MMU, boot protocols, hosted-arch
   kernel-side ISR, ISA barriers, SMP, virtio/PLIC drivers, process
   model) carried from plan §5, each with its closing mechanism.
5. All existence claims re-verified at authoring time with file:line
   evidence, per the plan's Phase 0 discipline.

## 5. Evidence vehicle

`docs/plans/2026-09-11-rv64-capability-kernel.md` — recorded, not
started. Its Phase 4 demo (M-mode kernel + U-mode tasks + ecall
boundary + preemptive timer scheduler, pure Briev, QEMU virt rv64)
supplies the "boots and runs" proof this doc's systems columns need.
