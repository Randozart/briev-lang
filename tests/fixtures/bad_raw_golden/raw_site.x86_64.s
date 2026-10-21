.section .text
.global _start
_start:
lea %rax, [%rcx + 2]
ret
