# Briev rv64 Capability Kernel — Plan (2026-09-11)

**Status**: RECORDED, NOT STARTED. This plan exists as a true test of native
Briev capability. It was scoped in a single session against a verified
audit of the tree at `214337d2` (main) + `feat/briev-native-runtime`
(`729bfc8d`). No implementation work has been done under this plan.

## 0. Goal

Prove the **technical capability frontier**: what stands between
Briev-today and "an OS written in Briev". Vehicle: a functional demo
micro-kernel in pure Briev, RISC-V rv64, QEMU `virt` board — M-mode
kernel + U-mode tasks + ecall boundary + preemptive timer scheduler.
Every phase closes a *capability* (general mechanism), never a
demo-shaped special case.

**Decisions recorded at scoping**:
- Motivation is *technical capability assessment*, not building a
  production OS — but the demo kernel must actually boot and run.
- Architecture: RISC-V rv64, QEMU `virt` board.
- Privilege scope: M-mode kernel + U-mode tasks. S-mode/MMU/virtio/SMP
  explicitly out of scope — that is tier 3 (see §5), and this plan's job
  is frontier evidence, not the full OS.
- The plan was authored by the agent at the user's request; the agent is
  NOT expected to execute it. Any future session picking this up should
  re-verify §1 claims before trusting them.

## 1. What exists (verified 2026-09-11)

| Piece | State | Reusable for rv64? |
|---|---|---|
| `SysCall#` inline asm | x86_64 (`syscall`) + aarch64 (`svc #0`) emitted | pattern only |
| `isr<riscv_machine>` mechanism row | `config/isr-targets.dbvl`: `4; none; 0; mret; none; 512; default_isr_handler; ; riscv_machine` — LLVM `"interrupt"="machine"` attr does trap save/restore, runtime-built table (no link-time section) | **directly** |
| `VolatileLoad#`/`VolatileStore#`, `trg @addr`, `vol let` | landed (cbv Slice C, d59d1ecf) | **directly** — NS16550A UART + CLINT are plain MMIO |
| `section(".name")` + transitive no-alloc proof | landed 2026-09-06 (isr-handlers-and-sections plan) | **directly** — boot code runs before heap exists |
| Family F `_start` | hosted Linux process entry (captured environ, getenv via Ptr) | **shape only** — bare/freestanding variant needed |
| ELF object path | `llc -O2 -filetype=obj` proven in glue `.a` path (`src/compile.rs:1852`) | **directly** — needs riscv64 triple + `-relocation-model=static` |
| Linker script | one handwritten: `lib/targets/stm32f407.ld` | pattern only |
| `Asm#` two-mode asm fundamental | landed on `feat/briev-native-runtime` (d770cec4) + prelude `asm.bv` (e72055c7) | CSR/fence/mret vehicle — operand support UNVERIFIED, Phase 0 item |
| `linux_kernel.toml` | `lib/targets/linux_kernel.toml`: `backend = "c"`, `module_init` — dead C-backend artifact, misleading name | delete (Phase 1 cleanup) |

Branch context: `feat/briev-native-runtime` (33 commits) deleted
`briev_rt.c` entirely — all runtime families pure-Briev over `SysCall#`
inline asm, brk arena 2.5× faster than C malloc, cooperative async
(Family H, pthread pool deleted), owned hosted `_start` (Family F).
That branch makes the *hosted* freestanding story real; this plan
extends the same discipline to *bare metal*.

## 2. Phases

### Phase 0 — Ground truth + baseline (½ day)

1. `cargo test --lib`, `cargo build` — green baseline.
2. Trace the current `.b.bv` freestanding path end-to-end for
   `thumbv7em`: what `triple_is_freestanding()` gates
   (`src/backend/llvm/mod.rs:31`), what skips `briev_rt.c`, where entry
   is emitted. Name every reusable piece in the plan doc.
3. Toolchain audit: `llc --version` must list riscv backend; `ld.lld`
   or `riscv64-linux-gnu-ld`; `qemu-system-riscv64`. Install what is
   missing.
4. `Asm#` capability check: operand support or string-only? This
   decides the Phase 3 CSR strategy (asm vs intrinsics).

**Gate**: audit table committed to this plan doc as an addendum.

### Phase 1 — rv64 target surface (config + data files, ~zero compiler changes)

1. `lib/targets/qemu-virt-rv64.ld` — handwritten, pattern of
   `stm32f407.ld`: entry `0x80000000`, single RAM region (QEMU
   `-kernel` loads the image directly — no FLASH-to-RAM copy), `.bss`,
   stack symbol exported for the entry emission.
2. `lib/boards/qemu-virt-rv64/`:
   - `addresses.dbvl` — UART0 `0x10000000`, CLINT `0x02000000`,
     PLIC `0x0c000000`, RTC `0x00101000`, virtio `0x10001000+`.
   - `registers.dbvl` — NS16550A THR/LSR offsets.
   - `map.dbv` — schema, following the stm32f407 shape exactly.
3. Target profile: `target_triple = "riscv64-unknown-none"`,
   `isr_mechanism = "riscv_machine"`, bare entry style, linker
   invocation (`ld.lld -T qemu-virt-rv64.ld -nostdlib --gc-sections`).
4. Delete `lib/targets/linux_kernel.toml` (dead artifact; its existence
   misleads audits — as this plan's own audit showed).

**Gate**: `brievc build --target qemu-virt-rv64 hello.bv` produces a
valid riscv64 ELF (`readelf -h` sanity), correctly linked, not yet
bootable. **Test**: compile+link integration test.

**This phase is the generality proof**: a new board/target must be pure
data files + config — zero Rust match arms. If it cannot be, that is a
finding, not something to route around.

### Phase 2 — Boot capability

1. Freestanding entry emission, config-driven
   (`entry_point.style = "bare"` in the target profile): entry symbol
   at load address, `sp` initialized from the linker-provided stack
   symbol, `.bss` zeroed. Single-RAM-region case needs no `.data` copy;
   the emission must either handle the copied case generally or the
   profile declares load==link explicitly — decide from the Phase 0
   trace, never silently.
2. `section(".init")` boot function does the remaining bring-up in
   Briev (no-alloc proof already covers it).
3. UART driver: `lib/boards/qemu-virt-rv64/uart.bv` —
   `VolatileLoad#`/`VolatileStore#` on THR/LSR, polled ready bit,
   `Print` protocol impl. Stdlib/board territory, zero new intrinsics.

**Gate**: `qemu-system-riscv64 -M virt -bios none -kernel
briev-kernel.elf -nographic` prints `briev`. **Test**:
`tests/bare/qemu-rv64.sh` — timeout-guarded, golden-output diff;
integration-only (not in `cargo test`, external dep).

### Phase 3 — Trap capability

1. CSR access: via `Asm#` (if Phase 0 confirms operands) or minimal
   `Csrr#`/`Csrw#` intrinsics. CSR access is ISA-level mechanism, same
   honesty class as `SysCall#` — legitimate intrinsic territory.
2. `mtvec` programmed to the runtime-built riscv_machine trap table.
3. CLINT timer: `mtimecmp` MMIO write;
   `isr<riscv_machine> handler` timer tick increments a counter and
   returns via `mret` (emitted by the mechanism already).

**Gate**: kernel prints an incrementing tick count driven purely by
timer traps. **Test**: extend the QEMU script assertions. The 512-byte
ISR frame-bound contract must hold (it is a compile-time check — a too
fat handler is a compile error, exercise that error path in a test).

### Phase 4 — Micro-kernel capability demo

1. U-mode entry: `mret` with `mstatus.MPP=U` (asm). Two tasks.
2. ecall boundary: kernel-side trap dispatch on `mcause` =
   ecall-from-U → kernel syscall table (pure Briev match on register
   args; write syscall prints via the Phase 2 UART driver).
3. Preemptive scheduler: timer trap → context save / switch / restore.
   Context switch is kernel-side `.bv` + asm — **stdlib/kernel code,
   not compiler knowledge** (Rule 14: stdlib is the extension
   mechanism).
4. Demo: two tasks printing via the ecall-write syscall, timer
   preempted, interleaved output.

**Gate**: QEMU shows interleaved task output through the syscall
boundary — the full userspace↔kernel loop, all in Briev. **Test**:
golden output file diff.

### Phase 5 — Frontier document (the actual deliverable)

`docs/architecture/os-capability-frontier.md`: capability-by-
capability table (proved-by-demo vs missing), each tier-3 gap named
with the mechanism that would close it. This becomes the grounded,
evidence-backed answer to "how close is Briev to a Linux-class
kernel" — replacing the session-level assessment that motivated this
plan.

## 3. Rules compliance

- **No new Rust knowledge of boards/arch**: Phase 1 is data files;
  Phases 2–4 compiler emission is config-driven mechanism (existing
  pattern: `isr_mechanism`, `entry_point.style`), board knowledge in
  `.dbvl`/`.bv`.
- **Additive only** (Rule 6): entry emission = new config-gated arm;
  no existing path touched; `_ => return None;` fallthroughs unchanged.
- **Always finish** (Rule 7): no stubs — each phase ends at its gate.
- **Tests or it doesn't exist** (Rule 9): each emission change gets a
  unit test + the QEMU integration gate; `cargo test --lib` green per
  commit.
- **Contract-first** (Rule 1): ISR frame bounds and no-alloc section
  proofs stay at full strength; the demo must satisfy them, not dodge
  them.
- **Docs** (Rule 13): this plan at start; frontier doc +
  `docs/architecture/agent-reference.md` target-profile section +
  board-file README updated in the same commits as structural changes.
- **Not a performance plan** → no benchmark baseline table required
  (Rule 12 scopes that to performance work); per-commit test gates
  stand in as regression discipline.

## 4. Risks

| Risk | Mitigation |
|---|---|
| `Asm#` lacks operands → CSR access clunky | Phase 0 audit decides; `Csrr#`/`Csrw#` intrinsics are the honest fallback (ISA-level, like `SysCall#`) |
| LLVM riscv `interrupt` attr vs manual save | attr is already the registered convention; verify the emitted prologue/epilogue in Phase 3 IR before building the scheduler on it |
| `llc -filetype=obj` + ld.lld diverges from the hosted `-O3 -flto` clang pipeline | boot path is static-reloc freestanding — LTO irrelevant at M-mode; document the divergence in backend-contracts.md |
| QEMU timing flakes in the integration script | timeout guards, golden-output diff, marked integration-only |
| `.b.bv` path assumptions don't transfer to rv64 | Phase 0 trace names every gated piece before any code is written; surprises go into the audit addendum, not around them |

## 5. Tier-3 frontier (explicitly out of scope; enumerated for Phase 5)

Gaps between the Phase-4 demo and a Linux-class kernel, each with the
mechanism that would close it:

| Gap | Closing mechanism |
|---|---|
| S-mode + two-stage privilege | OpenSBI handoff or M-mode firmware; `mret`-delegation config in target profile |
| MMU / page tables / TLB | `spec AddressSpace` (spec'd, unimplemented §17.2), `sfence.vma` via asm, `PhysAddr` type + casting-graph integration |
| Boot protocols (multiboot/UEFI/DTB) | boot-header emission in the profile; DTB parser as stdlib `.bv` |
| Kernel-side ISR on hosted archs (x86 IDT is runtime-built) | IDT/GDT construction intrinsics or stdlib over `Ptr` + section placement |
| ISA barriers beyond C11 `Fence#` (DSB/DMB/ISB class) | asm or `Fence#` mechanism-kind parameter |
| SMP: per-CPU data, IPI, spinlocks | per-CPU section + PLIC/IMSIC drivers — stdlib + data files first, intrinsics only if proven insufficient |
| Drivers: virtio, PLIC-level interrupt routing | board `.bv` drivers; PLIC MMIO is ordinary volatile access |
| Process model (fork/exec-class semantics) | kernel-stdlib design work; the reactor/contract model vs Unix process semantics needs its own plan |

## 6. Addendum A — 2026-09-11: metaprogramming audit finding (same session, post-commit)

Dated addendum per plan discipline — §1–§5 above are unchanged; gates and
scope are unaffected. Recorded because the audit surfaced capabilities
that alter *how* phases get built, not *what* they prove.

### 6.1 Additional §1 existence-table rows (verified, implemented)

| Piece | State | Reusable for rv64? |
|---|---|---|
| Staged metaprogramming | 11-stage plugin pipeline (`PreLex…Linked`), user `$(Stage)` inline blocks, live-AST navigation DSL (`Tag$`/`Named$`/`ForEach$`/`Insert$`/`Delete$`/`Set$`), hygienic quotation (`spec/SPEC.md` §18.4), capability lockfile (`src/macros/lockfile.rs`), sandboxed macro VFS | **directly** — program-level kernel code can be generated/checked at `$(Parsed)`/`$(Generated)` instead of new emission arms |
| DWARF probe generation from reflection | `$defn gen_probe_fields` / `probe_struct_layout` (`lib/std/dwarf.bv:17`, `:58`) | **directly** — seeds the kernel debugging story (trap/scheduler debugging in QEMU, Phases 3–4) |

Related correction to session-level assessment (chat, not this plan):
the initial C/C++ capability verdict claimed Briev "loses on template
metaprogramming depth". Verified evidence contradicts the mechanism
half: staging, AST access, hygiene, and capability security are
structurally impossible for C++ templates. The surviving C++ advantage
is accumulated practice (generic-library gravity, overload/concepts
maturity) — ecosystem, not mechanism. `docs/architecture/os-capability-frontier.md`
(the Phase 5 deliverable) must carry this split explicitly; see its
skeleton, committed alongside this addendum.

### 6.2 Doctrine note for Phases 2–4

Where kernel-side work is program-level (trap tables, syscall dispatch,
task structs), staged metaprogramming (`$` declarations, `$(Stage)`
blocks, AST DSL) is a third extension route alongside "config + stdlib
`.bv`" — and the established doctrine applies unchanged: prefer it over
new emission arms when the knowledge is program-level, with special
treatment disclosed via the `$`/`!` markers (Golden Rule 3). New Rust
match arms remain reserved for genuine compiler mechanism (entry
emission, linker invocation), never board or kernel knowledge.

### 6.3 Phase 5 scope additions

The frontier doc must additionally record:

1. **Debugging/probes frontier row** — reflection-driven probe
   generation exists (`dwarf.bv`); kernel gap = GDB stub / QEMU
   `-s -S` integration, likely closable via the same reflection + staged
   system rather than new backend code.
2. **Verdict framing, locked**: "capability deficit vs C/C++ =
   ecosystem maturity + systems-plumbing last mile, not language
   mechanism" — with this plan as the evidence vehicle for the
   last-mile half.

## 7. Provenance

- Authored: 2026-09-11, agent session (opencode), at user request.
- Addendum A: 2026-09-11, same session, post-commit (§6).
- Evidence basis: live audit of `main` @ `214337d2` and
  `feat/briev-native-runtime` @ `729bfc8d` — `git log`, ISR registry,
  `linux_kernel.toml`, `src/compile.rs` link paths, stm32 board files,
  `src/plugin/mod.rs`, `src/parser/definitions.rs`, `src/macros/lockfile.rs`,
  `lib/std/dwarf.bv`, `spec/SPEC.md` §18.
- Companion context: `docs/plans/2026-09-09-briev-native-runtime-and-family-realignment.md`
  (the hosted native-runtime work this plan builds on),
  `docs/plans/2026-09-06-isr-handlers-and-sections.md` (ISR + section
  mechanics reused here).
- Execution status: not started. Any session resuming this plan must
  begin at Phase 0 and re-verify §1 before trusting it.
