.section .text
.global _start
_start:
movq $753664, %rax
movq $66, %rcx
L_start__loop:
movzbq (%rax), %rdx
cmpq %rcx, %rdx
je L_start__done
movq %rdx, (%rax)
addq $1, %rax
jmp L_start__loop
L_start__done:
hlt
