    .syntax unified
    .cpu cortex-m3
    .thumb

/* MPS2-AN385 board startup (rv64-finish plan Phase 5).
 *
 * The boot table at 0x0 carries Reset + spin defaults; Default_Handler is
 * NOT defined here — the compiler emits it alongside its vector table and
 * this table only references it.
 *
 * Reset_Handler does the canonical bare-metal init before entering the
 * compiler's _start:
 *   1. copy .data from its load address (_data_init) to its run address
 *      (_data_start.._data_end) — Briev globals like `briev_begin_boot`
 *      carry their initializers in .data and RAM starts as garbage;
 *   2. zero .bss (_bss_start.._bss_end).
 * Byte loops: section sizes here are tiny and the code stays correct for
 * any alignment.
 */

    .section .isr_vector, "a", %progbits
    .align 2
    .global __isr_vector
__isr_vector:
    .word _stack_top        @ 0: initial SP
    .word Reset_Handler     @ 1: Reset
    .word Default_Handler   @ 2: NMI
    .word Default_Handler   @ 3: HardFault
    .word Default_Handler   @ 4: MemManage
    .word Default_Handler   @ 5: BusFault
    .word Default_Handler   @ 6: UsageFault
    .word Default_Handler   @ 7: Reserved
    .word Default_Handler   @ 8: Reserved
    .word Default_Handler   @ 9: Reserved
    .word Default_Handler   @ 10: Reserved
    .word Default_Handler   @ 11: SVCall
    .word Default_Handler   @ 12: Debug Monitor
    .word Default_Handler   @ 13: Reserved
    .word Default_Handler   @ 14: PendSV
    .word Default_Handler   @ 15: SysTick

    .section .text.start, "ax", %progbits
    .align 2
    .global Reset_Handler
    .thumb_func
Reset_Handler:
    /* 1. .data init copy: LMA _data_init → VMA _data_start.._data_end */
    ldr r0, =_data_init
    ldr r1, =_data_start
    ldr r2, =_data_end
1:
    cmp r1, r2
    bcs 2f
    ldrb r3, [r0], #1
    strb r3, [r1], #1
    b 1b
2:
    /* 2. .bss zero: _bss_start.._bss_end */
    ldr r0, =_bss_start
    ldr r1, =_bss_end
    movs r3, #0
3:
    cmp r0, r1
    bcs 4f
    strb r3, [r0], #1
    b 3b
4:
    b _start

/* Default_Handler is defined WEAK here: the compiler also emits it (spin
 * loop) alongside its vector table when the program declares ISR handlers,
 * and that strong definition must win. Programs with no ISR handlers
 * (hello world) still need the boot table's spin reference resolved — the
 * weak definition covers that. 2026-09-14 (rv64-finish plan Phase 5). */
    .section .text, "ax", %progbits
    .align 2
    .weak Default_Handler
    .thumb_func
Default_Handler:
    b .
