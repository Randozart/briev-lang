/* MPS2-AN385 (ARM Cortex-M3) linker script — bare-metal.
 *
 * Cortex-M3 boots from address 0x0 (ZBT SSRAM1 on MPS2-AN385).
 * QEMU -kernel loads the ELF and the CPU reads the vector table
 * from IMAGE_BASE = 0x0.
 *
 * Memory map:
 *   0x00000000  ZBT SSRAM1 (512 KB, boot)
 *   0x20000000  ZBT SSRAM2/3 (512 KB)
 *   0x40004000  UART0 (PL011)
 *   0xE000E010  SysTick
 *   0xE000E100  NVIC
 */

ENTRY(Reset_Handler)

MEMORY
{
    /* Boot ROM / SSRAM1 — where the CPU starts. */
    FLASH (rx) : ORIGIN = 0x00000000, LENGTH = 0x00080000
    /* Main RAM for data + stack. */
    RAM (rwx)  : ORIGIN = 0x20000000, LENGTH = 0x00080000
}

/* Top of RAM — initial stack pointer (grows downward). */
_stack_top = ORIGIN(RAM) + LENGTH(RAM) - 0x4000;

SECTIONS
{
    /* Vector table + code in boot ROM — the CPU reads word 0 (SP) and
     * word 1 (Reset_Handler) from address 0x0 on power-up. */
    .text : ALIGN(4)
    {
        _text_start = .;
        *(.isr_vector)    /* interrupt vector table — must be first */
        *(.text.start)
        *(.text*)
        *(.rodata*)
    } > FLASH

    _data_init = LOADADDR(.data);
    .data : ALIGN(4)
    {
        _data_start = .;
        *(.data*)
        _data_end = .;
    } > RAM AT > FLASH

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
    _heap_end = ORIGIN(RAM) + LENGTH(RAM) - 0x4000;
}
