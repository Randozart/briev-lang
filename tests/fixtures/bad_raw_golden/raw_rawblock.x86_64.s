.section .text
.global _start
_start:
movq $1, %rax
addq $1, %rax
after:
ret
