/* QEMU virt board — aarch64 linker script (EL2, -bios none)
 *
 * QEMU loads the ELF at the entry point (0x40000000) and jumps there.
 * aarch64 needs no PMP (riscv-only), no vector table (thumb-only), no
 * mode switch (x86-only) — just set the UART base and run the portable
 * core.
 *
 * Memory map:
 *   0x09000000  PL011 UART
 *   0x40000000  RAM (128 MB default)
 */

ENTRY(_start)

MEMORY
{
    RAM (rwx) : ORIGIN = 0x40000000, LENGTH = 0x08000000
}

/* Top of usable RAM — initial stack pointer (grows downward).
 * Reserve 64 KB for the kernel stack. */
_stack_top = ORIGIN(RAM) + LENGTH(RAM) - 0x10000;

SECTIONS
{
    .text : ALIGN(4)
    {
        _text_start = .;
        *(.text.start)    /* boot code first */
        *(.text*)
        *(.rodata*)
    } > RAM

    _data_start = ALIGN(4);
    .data : ALIGN(4)
    {
        *(.data*)
    } > RAM

    .bss : ALIGN(8)
    {
        _bss_start = .;
        *(.bss*)
        *(COMMON)
        . = ALIGN(8);
        _bss_end = .;
    } > RAM

    /* Provide heap boundaries for the static bump arena (embedded mode). */
    _heap_start = .;
    _heap_end = ORIGIN(RAM) + LENGTH(RAM) - 0x10000;
}