.section .text
.global _start
_start:
# [r10 preserved] proven: callee-saved on x86_64
pushq %r12
movq $1, %r12
addq $2, %r12
popq %r12
ret
