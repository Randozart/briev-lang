// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Two scalar models of cache-cap arithmetic, not proofs of the production
//! mutex/cache or its insertion function. Run: kani kani/prove_smt_cache.rs
//!
//! The former copied literal sanitizer examples have moved to production-code
//! tests in briev-verify/src/smt/symbols.rs and real-Z3 encoding regressions.
//! Actual byte-codec injectivity is checked by that module's Kani harness;
//! whole-string/caller refinement remains a separate obligation.

/// Prove that the cache insert guard respects the cap: at cap, no insert;
/// below cap, an insert is permitted.
#[kani::proof]
fn cache_cap_guard_respects_limit() {
    const CAP: usize = 10_000;
    let len: usize = kani::any();
    let would_insert = len < CAP;
    if len == CAP {
        assert!(!would_insert, "at cap: must not insert");
    }
    if len < CAP {
        assert!(would_insert, "below cap: may insert");
    }
}

/// Prove that after an insert below cap, the new length does not exceed the cap.
#[kani::proof]
fn cache_insert_stays_within_cap() {
    const CAP: usize = 10_000;
    let len: usize = kani::any();
    if len < CAP {
        let new_len = len + 1;
        assert!(new_len <= CAP, "insert below cap yields length <= cap");
    }
}
