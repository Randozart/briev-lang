//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! §12 idiom-ledger gate (W2).
//!
//! Every idiom matcher must carry a coverage-ledger row naming its
//! why-not path and its retirement condition (§12.2 gates 4–5, §12.5).
//! This gate scans the source for idiom-matcher function names and fails
//! if a matcher has no qualifying ledger row — adding a matcher without
//! recording it is a test failure, not a review note.
//!
//! What counts as a matcher (mirrors the §12 vocabulary):
//!   fn detect_*_fold        — structure detection
//!   fn *_reduce_lane        — the routed lane predicate
//!   fn *_idiom_lane         — general idiom-lane predicates
//!
//! The gate is intentionally textual: the ledger lives in the doctrine
//! doc, and the doctrine is the contract.

use std::path::Path;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn read(rel: &str) -> String {
    let path = Path::new(PROJECT_ROOT).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Matcher function names declared in the analysis module.
fn idiom_matcher_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line
            .trim_start()
            .strip_prefix("pub(crate) fn ")
            .or_else(|| line.trim_start().strip_prefix("pub fn "))
            .or_else(|| line.trim_start().strip_prefix("fn "));
        let Some(rest) = t else { continue };
        let name = rest.split('(').next().unwrap_or("").trim();
        let is_fold = name.starts_with("detect_") && name.ends_with("_fold");
        let is_lane = name.ends_with("_reduce_lane") || name.ends_with("_idiom_lane");
        if is_fold || is_lane {
            out.push(name.to_string());
        }
    }
    out
}

#[test]
fn every_idiom_matcher_has_a_qualifying_ledger_row() {
    let accel = read("src/analysis/accel.rs");
    let doc = read("docs/architecture/derivation-not-recognition.md");

    let matchers = idiom_matcher_names(&accel);
    assert!(
        !matchers.is_empty(),
        "expected at least one §12 idiom matcher (the W3 sum fold); if the \
         scanner stopped matching, the gate would be vacuous"
    );

    // The coverage ledger section (§8).
    let section = doc
        .split("## 8. The coverage ledger")
        .nth(1)
        .and_then(|s| s.split("## 9.").next())
        .expect("doctrine must keep a §8 coverage ledger");

    for matcher in &matchers {
        let row = section
            .lines()
            .find(|l| l.starts_with('|') && l.contains(matcher.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "§12 ledger row missing for matcher '{matcher}' — record the idiom in \
                     docs/architecture/derivation-not-recognition.md §8 (gates 4–5)"
                )
            });
        let lower = row.to_lowercase();
        assert!(
            lower.contains("why-not"),
            "ledger row for '{matcher}' must name its why-not path (gate 4): {row}"
        );
        assert!(
            lower.contains("retire"),
            "ledger row for '{matcher}' must carry its retirement condition (gate 5): {row}"
        );
    }

    // The why-not path named by the W3 row must actually exist in the
    // analysis (a ledger row naming a ghost is worse than none).
    assert!(
        section.contains("sum_lane_outcome"),
        "the W3 ledger row must name the why-not entry point `sum_lane_outcome`"
    );
    assert!(
        accel.contains("fn sum_lane_outcome"),
        "the why-not entry point `sum_lane_outcome` named in the ledger must exist"
    );
}

#[test]
fn idiom_doctrine_plan_exists() {
    // Gates 4–5 are only meaningful with the recorded plan that owns the
    // sequencing; keep the pointer honest.
    let plan = read("docs/plans/2026-11-06-idiom-recognition-doctrine.md");
    assert!(plan.contains("W3"), "plan must keep the W3 work item");
    assert!(
        plan.contains("retire"),
        "plan must keep the retirement/parity obligation"
    );
}
