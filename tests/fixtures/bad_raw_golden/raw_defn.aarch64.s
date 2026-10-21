.section .text
.global _start
_start:
ldr x0, =2
ldr x1, =3
add x2, x0, x1
add x2, x2, #1
ret
