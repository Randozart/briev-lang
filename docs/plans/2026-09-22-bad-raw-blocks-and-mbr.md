# .bad raw target blocks + real-mode MBR

**2026-09-22**

Two connected pieces that complete the boot path:

1. **Part A — `raw <target>` ... `.end` blocks.** The ergonomic escape
   hatch for multi-line target-verbatim assembly. Today a 32-bit prologue
   or a 16-bit MBR body needs one `x86_64 => <line>` exception per line —
   and directives like `.code32` cannot even ride an exception row (they
   parse as instruction mnemonics → `unknown_mnemonic`). A `raw` block
   emits its lines verbatim for ONE target and skips them for the others.
   This fixes the multiboot 32-bit prologue AND makes the MBR body
   writable.
2. **Part B — `int $N` core op + a real MBR.** The toolchain already
   produces a bootable 512-byte sector (`as --64` + `.code16` + `.org
   510` + `.byte 0x55,0xAA` → `--raw-bin` → SeaBIOS boots it — verified).
   What is missing is grammar: an `int` op for BIOS software interrupts
   and the raw-block body. `examples/bad/boot_mbr.bad` proves the full
   chain.

## Motivation

The boot path was left with two gaps in the previous phase:

- x86 real-mode (512-byte MBR) is "out of reach — `.bad` emits 64-bit GAS
  only." The toolchain is fine; the grammar for writing the 16-bit body is
  not.
- The multiboot 32-bit prologue is "NOT expressible" — the failed attempt
  put `.code32` in an exception row. Raw blocks are the general fix.

The `.struct`/`.field`/`.end` pattern already establishes block-with-
terminator in the zero-braces grammar. `raw <target> ... .end` slots into
it.

## Part A — raw blocks

### Syntax

```
raw x86_64
    .code32
    cli
    movl $(gdt_end - gdt - 1), %eax
    lgdt gdt
    movl %cr0, %eax; orl $1, %eax; movl %eax, %cr0
    ...
    .code64
end
```

- `raw <target>` opens a block; every line until `end` is captured
  VERBATIM (unparsed — no mnemonic classification, no `unknown_mnemonic`).
- Emitted only when the active family matches `<target>`; skipped
  otherwise (a cross-target source carries its per-target raw text).
- `target` is a family prefix like the exception rows (`x86_64`,
  `thumb`, `riscv64`, ...).
- `.end` terminates (matching `.struct/.field/.end`). A `raw` block
  without `.end` before EOF is a loud error.

### Implementation

- AST: `BadTopLevel::RawBlock { target: String, lines: Vec<String>, span }`.
- Parser: `parse()` — when a top-level line is `raw <target>` (no colon,
  no `=>`), consume lines until a line whose trimmed content is `end`;
  error on EOF. The block is a top-level item (not a body item — it
  cannot nest inside a label/defn; like `.struct` it is its own item).
  Instruction-position defn invocation is unaffected.
- Lowerer pass 1: `collect` — no-op (nothing to collect; the block is
  self-contained).
- Lowerer pass 2: `emit` — if `family.starts_with(&target)`, push every
  line verbatim; else skip.
- `emit_directive` `.end`: already consumed in pass 1 — but `raw` is a
  top-level item, not a directive, so no clash.

### Ergonomics vs the old pattern

```
// OLD — one exception per line, directives impossible:
entry:
    nop
    x86_64 => cli
    nop
    x86_64 => movl $(gdt_end - gdt - 1), %eax

// NEW — one verbatim block:
raw x86_64
    cli
    movl $(gdt_end - gdt - 1), %eax
end
```

## Part B — `int $N` + the MBR

### `int $N` core op

`int vec, num`? No — a single immediate vector: `int N` → `int $N` on
x86_64. BIOS software interrupt (`int $0x13` disk, `int $0x10` video).
Data-driven row:

```
int: "1"; "x86_64:int $1|-";
```

Immediate-only (`|-` no reg form — a register vector is meaningless on
x86; other targets have no BIOS int → loud error). The `$1` imm prefix
(`$`) comes from the `imm` row.

### `examples/bad/boot_mbr.bad`

A real MBR boot sector in `.bad`:

```
import "boot.bad"   // boot-signature block (0x55AA)
section .text
raw x86_64
    .code16
    cli
    xorw %ax, %ax
    movw %ax, %ds
    // print 'B' via BIOS INT 10h (teletype), green
    movb $0x0E, %ah
    movb $66, %al        // 'B'
    movb $0x02, %bl
    int $0x10
    hlt
    .org 510
    .byte 0x55, 0xAA
end
```

Built with `brievc bad examples/bad/boot_mbr.bad --target x86_64 --raw-bin`
→ 512-byte flat image; gate boots it under `qemu-system-i386 -drive
file=...,format=raw,if=floppy`. (`.code16` + `as --64` is verified to
produce the correct flat bytes.)

### Multiboot retry

`examples/bad/boot_multiboot.bad`: the 32-bit prologue (GDT, PAE/LME,
long-mode jump) becomes a `raw x86_64` block; the 64-bit body stays
portable core ops. The `.multiboot` header is `.long` data.

## Work order

1. Part A: AST + parser block capture + lowerer emit/skip + tests.
2. Part B: `int` row + MBR example + gate.
3. Multiboot retry + gate if qemu accepts it.
4. Docs: bad-dialect.md grammar table + raw-block section, plan update.

## Undo

Part A: delete the AST node, the parser capture, the lowerer arm, the
grammar row. Part B: delete the `int` row, the examples, the gates.

## Doc updates

- `docs/architecture/bad-dialect.md`: grammar table row for `raw`, a
  Raw blocks section (verbatim, per-target, `.end`).
- `docs/plans/2026-09-22-bad-ack-tier-and-bootstrap-bad.md`: mark the
  boot-sector item DONE via raw blocks.
- `spec/SPEC.md` §20.1: raw-block sentence.