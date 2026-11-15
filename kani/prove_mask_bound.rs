// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Kani proof for the mask-bound rule in `briev-verify`'s interval reasoning
//! (`verify.rs::mask_bound`). Run: kani kani/prove_mask_bound.rs
//!
//! The prover infers `[0, M]` for `x & M` whenever `M >= 0`, and declines to
//! infer anything for a negative mask. Both halves are proved here on the exact
//! i64 operation the analysis models, so the rule is validated rather than
//! assumed.

/// A non-negative mask bounds the result: `0 <= x & M <= M`.
///
/// This is the fact that discharges `term <= MASK32` obligations (`sha1_f`,
/// `rotl32`) and `term <= 0xFFFFFFFF` (`ip_v4`) without a solver.
#[kani::proof]
fn masked_value_lies_within_the_mask() {
    let x: i64 = kani::any();
    let mask: i64 = kani::any();
    kani::assume(mask >= 0);

    let masked = x & mask;
    assert!(masked >= 0, "a non-negative mask keeps the sign bit clear");
    assert!(masked <= mask, "AND cannot set a bit above the mask");
}

/// A negative mask is not a range: `x & -1 == x`, so no upper bound exists.
///
/// The analysis must therefore return no bound here — inferring `[0, M]` from a
/// negative mask would prove false postconditions.
#[kani::proof]
fn negative_mask_is_the_identity() {
    let x: i64 = kani::any();
    assert!(x & -1 == x, "all-ones mask is the identity");
}

/// The zero mask collapses the value to zero (the `[0, 0]` instance).
#[kani::proof]
fn zero_mask_collapses_to_zero() {
    let x: i64 = kani::any();
    assert!(x & 0 == 0);
}
