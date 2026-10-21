.section .text
.global _start
_start:
@ [r10 preserved] proven: callee-saved on thumbv7m
push { r10 }
ldr r10, =1
adds r10, r10, #2
pop { r10 }
bx lr
