/* Multiboot2 x86_64 — QEMU `-kernel` loader.
 *
 * QEMU scans the ELF for a multiboot2 header in the first 32768 bytes,
 * then loads the LOAD segments and jumps to the ELF entry (`_start`) in
 * 32-bit protected mode with paging off. The kernel's own prologue
 * switches to long mode.
 *
 * Load at 1 MiB; the prologue identity-maps the low 2 MiB so the code,
 * VGA text memory (0xB8000), and page tables are all reachable.
 */

ENTRY(_entry)

SECTIONS
{
    . = 1M;

    .text : ALIGN(4K)
    {
        *(.text.start)
        *(.text*)
        *(.rodata*)
    }

    .data : ALIGN(4K)
    {
        *(.data*)
    }

    .bss : ALIGN(4K)
    {
        *(.bss*)
        *(COMMON)
    }

    /* Keep the flat image starting exactly at the multiboot header: drop
     * toolchain notes/properties so no orphan section lands before .text. */
    /DISCARD/ :
    {
        *(.note*)
        *(.comment)
        *(.eh_frame*)
    }
}
