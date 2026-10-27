//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//

//! Proof-consumption ledger.
//!
//! Every proof verdict records the family that produced it and the consumer
//! that used it. `BRIEV_PROOF_LEDGER=1` prints the counts; a proof with only a
//! `typecheck-only` consumer is an explicit no-runtime-consumer record, not a
//! claim that codegen was optimized.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    static LEDGER: RefCell<BTreeMap<(String, String), usize>> =
        RefCell::new(BTreeMap::new());
}

pub fn enabled() -> bool {
    std::env::var_os("BRIEV_PROOF_LEDGER").is_some()
}

pub fn clear() {
    LEDGER.with(|l| l.borrow_mut().clear());
}

pub fn record(family: &str, consumer: &str) {
    if !enabled() {
        return;
    }
    LEDGER.with(|l| {
        *l.borrow_mut()
            .entry((family.to_string(), consumer.to_string()))
            .or_insert(0) += 1;
    });
}

pub fn dump() {
    if !enabled() {
        return;
    }
    LEDGER.with(|l| {
        for ((family, consumer), count) in l.borrow().iter() {
            eprintln!(
                "PROOF_LEDGER family={family} consumer={consumer} count={count}"
            );
        }
    });
}
