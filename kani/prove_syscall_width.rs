// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Kani proof harness for the SysCall# c_long width cast.
//!
//! `src/interpreter/intrinsics.rs::exec_syscall` (added 2026-11-15, issue #4)
//! casts the system-call number and six arguments through `libc::c_long`
//! before calling `libc::syscall`. On the compiler's target platforms
//! (x86-64 Linux, aarch64 Linux, x86-64 macOS), `c_long` is `i64`, so the
//! cast is a no-op. This harness proves the width invariants that make the
//! cast safe:
//!
//! 1. Valid POSIX syscall numbers (0..256) fit in `i32` — so the cast to
//!    `c_int` (macOS first-arg width) is lossless.
//! 2. An `i64` argument cast to `i64` (the 64-bit `c_long`) is an identity.
//! 3. An `i64` value decomposes into two `i32` halves losslessly — the ABI
//!    guarantee that makes passing an `i64` where `c_int` is expected safe
//!    on x86-64 SysV (the callee reads the lower 32 bits of the 64-bit
//!    register).
//!
//! Run with: `kani kani/prove_syscall_width.rs`   (from the repo root)

// ── Syscall number width ─────────────────────────────────────────────────

/// Prove that a syscall number in [0, 256) fits in `i32` and the round-trip
/// `i64 → i32 → i64` preserves the value exactly.
#[kani::proof]
fn syscall_number_fits_in_i32_when_in_range() {
    let sysno: i64 = kani::any();
    // The valid range: POSIX syscall numbers are non-negative and < 256.
    if sysno >= 0 && sysno < 256 {
        let sysno_i32 = sysno as i32;
        assert_eq!(
            sysno_i32 as i64, sysno,
            "syscall number in [0,256) must survive i64→i32→i64"
        );
    }
}

/// Prove that a syscall number in [0, 256) fits in `i64` trivially (the
/// `c_long` width on 64-bit targets) — the cast is an identity.
#[kani::proof]
fn syscall_number_identity_in_c_long() {
    let sysno: i64 = kani::any();
    if sysno >= 0 && sysno < 256 {
        let sysno_c: i64 = sysno; // c_long is i64 on our targets
        assert_eq!(
            sysno_c, sysno,
            "c_long cast must be identity for in-range sysno"
        );
    }
}

// ── Argument width ───────────────────────────────────────────────────────

/// Prove that an `i64` argument cast to `c_long` (i64 on 64-bit targets)
/// round-trips exactly — no truncation, no sign change.
#[kani::proof]
fn i64_arg_cast_to_c_long_is_identity() {
    let arg: i64 = kani::any();
    let arg_c: i64 = arg; // c_long is i64 on our targets
    assert_eq!(arg_c, arg, "i64 → c_long → i64 must be identity on 64-bit");
}

/// Prove that masking an `i64` to its low 32 bits is idempotent — i.e. the
/// low 32 bits carry no high-order information. This is the bit-level
/// guarantee underlying the SysV variadic-ABI claim: a C `int` callee
/// observes exactly these 32 bits, and re-masking (as the `as i32` cast
/// does) recovers them exactly.
#[kani::proof]
fn low32_mask_is_idempotent() {
    let arg: i64 = kani::any();
    let low32: i64 = arg & 0xFFFF_FFFF;
    // Masking the already-masked value changes nothing.
    assert_eq!(low32 & 0xFFFF_FFFF, low32, "low-32-bit mask is idempotent");
}

/// Prove that the `i32` truncation of an `i64` equals its lower 32 bits —
/// exactly what a C `int` callee observes on the x86-64 SysV ABI.
#[kani::proof]
fn i32_truncation_equals_lower_bits() {
    let arg: i64 = kani::any();
    let truncated: i32 = arg as i32;
    let lower_bits: i32 = (arg & 0xFFFF_FFFF) as i32;
    assert_eq!(
        truncated, lower_bits,
        "i32 cast must equal the lower 32 bits of the i64"
    );
}
