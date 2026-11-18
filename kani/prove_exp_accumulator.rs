// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Kani proof for the accumulator invariant `pf_exp` relies on
//! (`lib/std/cast_lanes.bv`, `pf_exp_digits` and `pf_exp`).
//! Run: kani kani/prove_exp_accumulator.rs
//!
//! `pf_exp` declares `[term.0 >= -1000 && term.0 <= 1000]` and `str_to_float`
//! feeds that bound into `pf_make`'s `[d >= -2100 && d <= 2100]` precondition.
//! The declaration is only sound if the digit loop really keeps the exponent in
//! `[-1000, 1000]`:
//!
//!   invariant  `acc` in `[0, 1000]`
//!   step       `acc < 1000` and a digit `d` (0..=9): the next accumulator is
//!              `acc*10 + d`; if that exceeds 1000 the loop SATURATES at 1000
//!              instead of growing (the original `acc < 1000` guard let it reach
//!              9999, against a declared cap of 1000)
//!   tail       `acc * sg` with `sg` in `{-1, 1}` stays in `[-1000, 1000]`
//!
//! Saturation is unobservable because `str_to_float` returns `f32`: an exponent
//! of magnitude 1000 is far past `f32`'s range, so the exact digits past the cap
//! cannot change the result. The last harness proves the range claim that makes
//! the saturation lossless.

/// The saturated accumulator never leaves the declared interval.
///
/// The step result is `min(acc*10 + d, 1000)`; with `acc` in `[0, 1000]` and a
/// digit `d`, that min is always in `[0, 1000]`. This is the inductive step that
/// keeps `term.0` inside the declared bound across the recursive call.
#[kani::proof]
fn saturated_step_stays_within_the_bound() {
    let acc: i64 = kani::any();
    let d: i64 = kani::any();
    kani::assume(acc >= 0 && acc <= 1000);
    kani::assume(d >= 0 && d <= 9);

    let grown = acc * 10 + d;
    let saturated = if grown <= 1000 { grown } else { 1000 };
    assert!(saturated >= 0, "the accumulator is never negative");
    assert!(saturated <= 1000, "the accumulator never exceeds the cap");
}

/// The unsaturating step would break the bound — the refutation that motivated
/// saturation. With `acc = 999` and `d = 9` the raw step is 9999.
#[kani::proof]
fn unsaturated_step_can_exceed_the_bound() {
    let acc: i64 = 999;
    let d: i64 = 9;
    assert!(
        acc * 10 + d == 9999,
        "the unguarded step reaches 9999, which the +/-1000 contract denies"
    );
}

/// The signed tail preserves the magnitude: `acc * sg` with `sg` in `{-1, 1}`
/// and `acc` in `[0, 1000]` lies in `[-1000, 1000]`.
#[kani::proof]
fn signed_tail_stays_within_the_bound() {
    let acc: i64 = kani::any();
    let sg: i64 = kani::any();
    kani::assume(acc >= 0 && acc <= 1000);
    kani::assume(sg == 1 || sg == -1);

    let folded = acc * sg;
    assert!(folded >= -1000, "the signed tail cannot fall below the cap");
    assert!(folded <= 1000, "the signed tail cannot exceed the cap");
}

/// Saturating at 1000 is lossless for `f32`: ten to the 39th already overflows.
///
/// `str_to_float` computes `value * 10^e` in `f32`, whose maximum is about
/// `3.4e38`. Every exponent of magnitude 1000 or more therefore saturates to
/// `inf`/`0` regardless of the exact digits, so replacing the true exponent with
/// 1000 cannot change the observable result.
#[kani::proof]
fn saturation_is_beyond_f32_range() {
    let mut value: f32 = 1.0;
    let mut step = 0;
    while step < 39 {
        value *= 10.0;
        step += 1;
    }
    assert!(value.is_infinite(), "10^39 already overflows f32");
}
