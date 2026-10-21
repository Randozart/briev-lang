.section .text
.global _start
_start:
li a0, 2
li a1, 3
add a2, a0, a1
addi a2, a2, 1
ret
