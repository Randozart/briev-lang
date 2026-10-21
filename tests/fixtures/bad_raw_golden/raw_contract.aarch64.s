.section .text
.global _start
_start:
// [r10 preserved] proven: 1 balanced push/pop pair(s)
str x10, [sp, #-16]!
ldr x10, =1
add x10, x10, #2
ldr x10, [sp], #16
ret
