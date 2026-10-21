.section .text
.global _start
_start:
ldr r0, =2
ldr r1, =3
adds r2, r0, r1
adds r2, r2, #1
bx lr
