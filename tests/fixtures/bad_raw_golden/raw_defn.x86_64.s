.section .text
.global _start
_start:
movq $2, %rax
movq $3, %rcx
leaq (%rax, %rcx), %rdx
addq $1, %rdx
ret
