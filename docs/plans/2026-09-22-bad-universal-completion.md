# .bad universal completion: aarch64 + Interpretation B + --all-targets + disk

**2026-09-22**

Four items that complete the universal-bootstrapper story:

1. **aarch64 universal target** — `arch.bad` gains aarch64 `uart_init` +
   `putc` named raw blocks; `bootloader.bv` boots 4/4 architectures.
2. **Interpretation B** — `.bv` typed calls to `.bad` primitives: a `.bad`
   import can surface `bad fn`-style typed signatures that `.bv` code
   calls with contracts.
3. **`--all-targets`** — one `brievc build` → all target binaries, with
   per-target triple/entry/linker config in profiles.
4. **`load_sectors` disk abstraction** — a portable load helper over
   per-target raw disk primitives.

## #1 aarch64 universal target

aarch64 (qemu virt, `-bios none`) boots at the ELF entry, needs no PMP
(riscv-only), no vector table (thumb-only), no mode switch (x86-only) —
just set the UART base and rejoin `core`. The PL011 UART is at
0x09000000 (32-bit DATA register, full-width store).

- `std/bad/arch.bad`: `raw aarch64 uart_init` (mov x2, 0x09000000; b core)
  + `raw aarch64 putc` (str w0, [x2]; ret).
- `examples/bad/bootloader.bv`: no source change — the same file boots
  aarch64 because arch.bad now carries the block.
- Gate `tests/bare/qemu-universal-aarch64.sh`: qemu-system-aarch64,
  machine virt, `-kernel`, assert "universal boot".

## #2 Interpretation B — .bv typed calls to .bad primitives

The `.bv` side wants to call a `.bad` primitive as a typed, contract-checked
function. Today `bad fn add(a: Int, b: Int) -> Int` exists as a `.bv`
top-level declaration that compiles through the bad backend and is
declared in LLVM. The gap: a `.bad` IMPORT cannot surface callable typed
signatures — the `.bv` frontend never sees them.

Design: a `.bad` file may declare `bad fn`-style typed signatures in a
comment-free, parser-readable form. Minimal viable version:
- The `.bad` import resolver records the `.bad` path (done).
- When a `.bv` file calls `uart_putc(x)` and a `.bad` import defines a
  named raw block `uart_putc` (per-arch), the call lowers to the
  per-arch symbol with C-ABI register passing (manual — caller sets
  abi_args regs, reads the return reg).
- Type signature comes from a `.bv`-side `bad fn uart_putc(c: Int) -> Int`
  declaration that names the `.bad` symbol (like a frgn but compiled via
  the bad backend). This is the honest, small version: the `.bv` author
  declares the typed surface; the `.bad` file provides the per-arch body.

Scope for this pass: verify the existing `bad fn` declaration path can
reference a `.bad`-imported named raw block symbol (cross-backend call
already works for bootstrap bodies; extend to a `.bv` `bad fn` whose body
is `import "arch.bad"` + `call uart_putc`). Test: `.bv` defn calls a
`bad fn` that calls an imported `.bad` named block; contract checked.

## #3 --all-targets

`manifest.rs:78` documents `--all-targets`, never implements it. The
`run_build` loop always yields exactly one target profile
(`main.rs:928`). Add:
- `--all-targets` flag → the loop iterates a configured target list
  (riscv64-unknown-none + thumbv7m-none-eabi + aarch64-unknown-none) each
  with triple, linker script, entry.
- briev.toml `[target.<name>]` profiles gain optional `triple`,
  `linker_script`, `entry` (currently CLI-only).
- Each target builds the SAME source; bootstrap bad + per-arch raw blocks
  pick their family. One command → all binaries.

Scope: flag + profile fields + loop. Build ergonomics only; no source
change.

## #4 load_sectors disk abstraction

`std/bad/` gains a `load_sectors`-style portable helper: a `.bad` defn
(portable copy loop over loadoff/storeoff) parameterized by source
address/sector count, with per-arch `raw` blocks providing the disk-read
primitive where one exists (x86 INT 13h). For targets with no disk
(riscv/arm use qemu -kernel / MMIO), the helper degrades to a
documented memory-copy path. Primary deliverable: the abstraction shape
+ docs; a real AHCI/NVMe driver is explicitly out of scope.

## Work order

#1 → #2 → #3 → #4. Each committed when green (cargo test --lib + gates).

## Doc updates

- bad-dialect.md: aarch64 in arch.bad; Interpretation B section; disk
  helper note.
- SPEC §20: --all-targets, bad-fn-imported-symbol call.
- Plan records.