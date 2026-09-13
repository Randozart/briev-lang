/* QEMU virt board — rv64 linker script (M-mode, -bios none)
 *
 * QEMU loads the ELF at the entry point (0x80000000) and jumps there
 * with a0 = hartid, a1 = DTB address.
 *
 * Memory map:
 *   0x02000000  CLINT (mtime/mtimecmp)
 *   0x0c000000  PLIC
 *   0x10000000  UART0 (NS16550A)
 *   0x80000000  RAM (128 MB default)
 */

ENTRY(_start)

MEMORY
{
    RAM (rwx) : ORIGIN = 0x80000000, LENGTH = 0x08000000
}

/* Top of usable RAM — initial stack pointer (grows downward).
 * Reserve 64 KB for the M-mode stack. */
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
