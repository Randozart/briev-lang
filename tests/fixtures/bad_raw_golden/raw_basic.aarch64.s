.section .text
.global _start
_start:
ldr x0, =753664
ldr x1, =66
L_start__loop:
ldrb w2, [x0]
cmp x2, x1
b.eq L_start__done
str x2, [x0]
add x0, x0, #1
b L_start__loop
L_start__done:
wfi
