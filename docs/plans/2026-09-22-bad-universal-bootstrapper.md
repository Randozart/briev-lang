# The universal bootstrapper in .bv (portable core + per-target raw)

**2026-09-22**

One `.bv` file whose `bootstrap bad` body is ~90% portable core ops —
banner, MMIO, copy, handoff — with only the entry prologues as
`raw <target>` blocks. Boots x86_64 (MBR + multiboot), riscv64, and
thumbv7m from the same source. The point: prove that `.bad`'s universal
core ISA is genuinely universal — the target-specific parts are raw
blocks, everything else is written once.

## Motivation

The three previous phases proved each boot path independently (MBR,
multiboot, riscv64 bootstrap-bad, thumb bootstrap-bad). This phase proves
they can share ONE source. It also closes two real gaps that block the
shared handoff:

1. **A bootstrap body cannot call a `.bv` defn** — `defn_liveness` roots
   the BadFn *name* but never scans its body, so a `.bv` defn referenced
   only from the loader is judged dead → dropped → unresolved symbol.
2. **`brievc build` has no `--no-link`** — an MBR `.code16`/`.org 510`
   body inside a `.bv` universal source cannot skip the 64-bit link.

## Work order

### 1. Gap 2 — bootstrap bodies can call `.bv` defns

`src/analysis/defn_liveness.rs:191-194` roots only the BadFn name. Fix:
when indexing a `BadFn(bf)` with `bf.bootstrap`, `parse_bad(&bf.body)` and
walk the instructions — root every symbol referenced by `call`/`addr`/
`jmp`. The LLVM side already emits `define @name(...)` for defns
(`emit_toplevel.rs:2790`); `.bad` calls reach them at link time with
manual C-ABI register setup. Test: a bootstrap body `call kernel_bv`
where `kernel_bv` is a real `.bv` defn with a contract — compiles and
links.

### 2. Gap 1 — `--no-link` on `brievc build`

`parse_build_args` (main.rs:257) lacks the `--no-link` flag that `run_bad`
has (main.rs:535). Add it so a `.bv`-side MBR (`.code16` + `.org 510` in
a raw block) can objcopy the flat sector without the 64-bit link. Test:
`bootloader.bv` built `--target x86_64 --raw-bin --no-link` → 512-byte
MBR, SeaBIOS boots it.

### 3. The universal bootloader — `examples/bad/bootloader.bv`

```
bootstrap bad Reset_Handler() [true] {
    raw x86_64          // MBR real-mode: .code16, INT 10h, .org 510, 0x55AA
        ...
    end
    raw riscv64         // PMP grant (csrw pmpaddr0/pmpcfg0) then fall to core
        ...
    end
    raw thumbv7m        // vector table + Reset_Handler entry
        ...
    end
    // ── portable core (universal ops only, no csrw/int/syscall) ──
    addr r0, msg
    mov r2, UART_BASE   // .const per target
    .loop: ldub ... store ... jz ... jmp
    call kernel_bv      // handoff to a real .bv defn (gap 2)
    halt
    msg: .asciz "briev\n"
}
```

- One source, three targets — 3 `brievc build` invocations (`--triple` +
  `--linker-script` per target).
- Portable core avoids family-gapped ops (`csrw`/`int`/`syscall` live
  only in raw blocks) — uses only universal rows.
- `.const UART_BASE` per target — MMIO base as a const, target-agnostic core.
- `kernel_bv` is a real `.bv` defn (contracts, comptime) — proves the
  `.bad`↔`.bv` handoff, prints a second banner line via the same UART.

### 4. Gate scripts

`tests/bare/qemu-universal-{mbr,rv64,arm}.sh` — each builds bootloader.bv
for its target and asserts the banner (and the kernel defn's line) prints.

### 5. Docs

bad-dialect.md: "The universal bootstrapper" pattern section (portable
core + per-target raw + `.bv` handoff); defn-liveness doc update; this
plan.

## Not in scope

- Real disk drivers (AHCI/NVMe) — the bootstrapper loads a second stage
  already in the image/MMIO, not a real OS from disk.
- `--all-targets` single-invocation multi-build (documented as follow-up;
  profiles exist but don't carry triple/entry).

## Undo

Gap 2: revert the BadFn liveness arm. Gap 1: revert the `--no-link` flag.
Example/gates: delete the files.

## Doc updates

- `docs/architecture/bad-dialect.md`: universal-bootstrapper section.
- `docs/architecture/defn-liveness.md`: bootstrap bodies root callees.
- `docs/plans/2026-09-22-bad-ack-tier-and-bootstrap-bad.md`: mark the
  universal bootstrapper DONE.