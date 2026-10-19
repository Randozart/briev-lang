.section .text
.global _start
_start:
ldr r0, =753664
ldr r1, =66
L_start__loop:
ldrb r2, [r0]
cmp r2, r1
beq L_start__done
str r2, [r0]
adds r0, r0, #1
b L_start__loop
L_start__done:
wfi
