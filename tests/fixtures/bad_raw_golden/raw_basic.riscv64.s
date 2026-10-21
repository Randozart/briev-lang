.section .text
.global _start
_start:
li a0, 753664
li a1, 66
L_start__loop:
lbu a2, 0(a0)
beq a2, a1, L_start__done
sd a2, 0(a0)
addi a0, a0, 1
j L_start__loop
L_start__done:
wfi
