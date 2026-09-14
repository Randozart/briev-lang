/* compiler_rt_arm.S — AEABI division entry points for Briev bare-metal ARM.
 *
 * 2026-09-14 (rv64-finish plan Phase 5): LLVM's ARM backend lowers 32/64-bit
 * integer division to __aeabi_* symbols using the AEABI REGISTER convention
 * (n in r0:r1, d in r2:r3; quotient in r0:r1, remainder in r2:r3). A C
 * definition returning a struct does NOT produce that ABI — AAPCS returns a
 * composite larger than 4 bytes via a hidden sret pointer — so the entries
 * are assembly. Each reshuffles into the convention of the scalar cores in
 * compiler_rt_arm.c, which do the actual shift-subtract math.
 *
 * The core-call convention below was DERIVED FROM THE COMPILED CORE, not
 * assumed (trace against qemu-arm user mode): for
 *     briev_sdiv64(long long *rem, long long n, long long d)
 * clang allocates rem → r0, n → r2:r3 (NOT r1:r2 — r1 stays free), and d →
 * the stack as a full 8-byte double-word slot at [sp+0]. Passing n in r1:r2
 * made the callee read n = 0 (division by zero). d.hi must be written too
 * ([sp+4]) — a bare 32-bit store leaves the high word as garbage.
 *
 * Undo: delete this file and the compile.rs arm branch.
 */

    .syntax unified
    .cpu cortex-m3
    .thumb
    .text

/* __aeabi_uldivmod: r0:r1 = n, r2:r3 = d → r0:r1 = q, r2:r3 = r */
    .thumb_func
    .global __aeabi_uldivmod
__aeabi_uldivmod:
    push {r4, lr}
    sub sp, #16
    mov r4, r0                  @ save n.lo
    mov r12, r1                 @ save n.hi
    strd r2, r3, [sp]           @ d fully on stack (8-byte slot)
    add r0, sp, #8              @ &rem
    mov r2, r4                  @ n.lo
    mov r3, r12                 @ n.hi
    bl briev_udiv64
    ldr r2, [sp, #8]            @ rem.lo
    ldr r3, [sp, #12]           @ rem.hi
    add sp, #16
    pop {r4, pc}

/* __aeabi_ldivmod: signed variant of the above. */
    .thumb_func
    .global __aeabi_ldivmod
__aeabi_ldivmod:
    push {r4, lr}
    sub sp, #16
    mov r4, r0
    mov r12, r1
    strd r2, r3, [sp]
    add r0, sp, #8
    mov r2, r4
    mov r3, r12
    bl briev_sdiv64
    ldr r2, [sp, #8]
    ldr r3, [sp, #12]
    add sp, #16
    pop {r4, pc}

/* __aeabi_uidiv: r0 = n, r1 = d → r0 = q (unsigned 32). */
    .thumb_func
    .global __aeabi_uidiv
__aeabi_uidiv:
    push {r4, lr}
    sub sp, #16
    mov r4, r0                  @ n.lo
    movs r12, #0                @ n.hi = 0
    strd r1, r12, [sp]          @ d = (r1 : 0)
    add r0, sp, #8              @ &rem
    mov r2, r4                  @ n.lo
    mov r3, r12                 @ n.hi = 0
    bl briev_udiv64
    add sp, #16
    pop {r4, pc}

/* __aeabi_uidivmod: r0 = n, r1 = d → r0 = q, r1 = r (unsigned 32). */
    .thumb_func
    .global __aeabi_uidivmod
__aeabi_uidivmod:
    push {r4, lr}
    sub sp, #16
    mov r4, r0
    movs r12, #0
    strd r1, r12, [sp]
    add r0, sp, #8
    mov r2, r4
    mov r3, r12
    bl briev_udiv64
    ldr r1, [sp, #8]            @ rem.lo
    add sp, #16
    pop {r4, pc}

/* __aeabi_idiv: r0 = n, r1 = d → r0 = q (signed 32, sign-extended). */
    .thumb_func
    .global __aeabi_idiv
__aeabi_idiv:
    push {r4, lr}
    sub sp, #16
    mov r4, r0                  @ n.lo
    asrs r0, r1, #31            @ r0 = d.hi = sign(d)
    strd r1, r0, [sp]           @ d = (r1 : sign(d))
    add r0, sp, #8              @ &rem
    mov r2, r4                  @ n.lo
    asrs r3, r4, #31            @ n.hi = sign(n)
    bl briev_sdiv64
    add sp, #16
    pop {r4, pc}

/* __aeabi_idivmod: r0 = n, r1 = d → r0 = q, r1 = r (signed 32). */
    .thumb_func
    .global __aeabi_idivmod
__aeabi_idivmod:
    push {r4, lr}
    sub sp, #16
    mov r4, r0
    asrs r0, r1, #31
    strd r1, r0, [sp]
    add r0, sp, #8
    mov r2, r4
    asrs r3, r4, #31
    bl briev_sdiv64
    ldr r1, [sp, #8]            @ rem.lo
    add sp, #16
    pop {r4, pc}