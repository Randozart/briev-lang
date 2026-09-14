/* compiler_rt_arm.c — ARM (thumbv7m/armv6-m+) compiler-rt shims for Briev
 * bare-metal targets. Mirror of compiler_rt_rv64.c (2026-09-13): LLVM emits
 * calls to the AEABI helpers for targets without hardware divide; the
 * freestanding link has no libgcc/libc to satisfy them.
 *
 * 2026-09-14 (rv64-finish plan Phase 5): first ARM consumer — QEMU
 * MPS2-AN385 (Cortex-M3, no hardware divide).
 *
 * The DIVISION entry points live in compiler_rt_arm.S, not here: LLVM's
 * ARM backend calls __aeabi_ldivmod/__aeabi_idivmod with the AEABI
 * REGISTER convention (quotient/remainder in r0-r3), which a C
 * struct-returning definition does not produce (AAPCS returns a >4-byte
 * composite via sret). This file provides the scalar cores that the .S
 * entries call after reshuffling into the normal AAPCS; the cores use
 * shift-subtract loops because expressing them with C `/`/`%` would make
 * clang recurse into the very helper being defined.
 *
 * Undo: delete this file, compiler_rt_arm.S, and the compile.rs arm branch
 * to return to linking failures (the pre-Phase-5 state).
 */

/* Shift-subtract unsigned 64-bit divide. d==0 returns 0/0 (hardware div
 * faults; the helper convention is to give the caller deterministic junk
 * rather than trap — matches libgcc's "no trap" stance for UDIV users). */
unsigned long long briev_udiv64(unsigned long long *rem,
                                unsigned long long n,
                                unsigned long long d) {
    unsigned long long q = 0;
    unsigned long long bit = 1;
    if (d == 0) {
        *rem = 0;
        return 0;
    }
    while (d < n && !(d >> 63)) {
        d <<= 1;
        bit <<= 1;
    }
    while (bit != 0) {
        if (n >= d) {
            n -= d;
            q |= bit;
        }
        bit >>= 1;
        d >>= 1;
    }
    *rem = n;
    return q;
}

/* Signed 64-bit divide: magnitudes through the unsigned core, then apply
 * the C99 truncation-toward-zero sign rule (remainder takes the dividend's
 * sign). */
long long briev_sdiv64(long long *rem, long long n, long long d) {
    int neg_q = 0;
    int neg_r = 0;
    unsigned long long un;
    unsigned long long ud;
    unsigned long long r = 0;
    if (n < 0) { un = (unsigned long long)(-n); neg_q = 1; neg_r = 1; }
    else { un = (unsigned long long)n; }
    if (d < 0) { ud = (unsigned long long)(-d); neg_q = !neg_q; }
    else { ud = (unsigned long long)d; }
    unsigned long long q = briev_udiv64(&r, un, ud);
    long long sq = (long long)q;
    long long sr = (long long)r;
    if (neg_q) { sq = -sq; }
    if (neg_r) { sr = -sr; }
    *rem = sr;
    return sq;
}

/* Memory-clear family. size_t is 4 bytes on every AAPCS32 target this file
 * serves; byte-loop is correct for any alignment. */
void __aeabi_memclr8(void *dest, unsigned n) {
    unsigned char *p = (unsigned char *)dest;
    while (n-- != 0) { *p++ = 0; }
}

void __aeabi_memclr4(void *dest, unsigned n)  { __aeabi_memclr8(dest, n); }
void __aeabi_memclr(void *dest, unsigned n)   { __aeabi_memclr8(dest, n); }
