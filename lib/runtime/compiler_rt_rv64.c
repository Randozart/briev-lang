// Minimal compiler-rt helpers for riscv64 bare-metal.
// Provides integer division/modulo helpers that LLVM emits for
// targets without hardware division (or when the compiler doesn't
// know the target has hardware div).

unsigned long long __udivdi3(unsigned long long a, unsigned long long b) {
    return a / b;
}

unsigned long long __umoddi3(unsigned long long a, unsigned long long b) {
    return a % b;
}

long long __divdi3(long long a, long long b) {
    return a / b;
}

long long __moddi3(long long a, long long b) {
    return a % b;
}
