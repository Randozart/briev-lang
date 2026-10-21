.section .text
.global _start
_start:
# [r10 preserved] proven: 1 balanced push/pop pair(s)
addi sp, sp, -16
sd t2, 0(sp)
li t2, 1
addi t2, t2, 2
ld t2, 0(sp)
addi sp, sp, 16
ret
