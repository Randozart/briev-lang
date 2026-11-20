// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! G2 census gate (2026-11-19, wave-4 D).
//!
//! The G2 diagnostic (a `when` guard ending in bare `term;` with statements
//! after it) is advisory by the wave-3 decision: bare `term;` IS the only
//! checkpoint spelling (it ends the current reactive step, not the program —
//! `term! ->` would end the program and change behavior), and every corpus
//! site the census classifies is a deliberate step-machine checkpoint.
//! Promotion to an error requires an explicit step-exit marker in the
//! language (a keyword-taxonomy decision), so until then the dangerous class
//! is TRAILING: a guard-final `term;` where NOTHING follows — observably
//! identical to the exit spelling, i.e. dead confusion with zero signal.
//!
//! This gate pins the corpus at **0 trailing sites**: if one appears, the
//! author almost certainly meant an exit, and the census (and this test)
//! fail loudly with the site named.

use std::path::Path;
use std::process::Command;

/// Run the reference census (scripts/g2_census.py) and return its summary
/// line ("G2 census: N checkpoint(s), M trailing").
fn census_summary(root: &Path) -> String {
    let out = Command::new("python3")
        .arg(root.join("scripts/g2_census.py"))
        .current_dir(root)
        .output()
        .expect("failed to spawn scripts/g2_census.py");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with("G2 census:"))
        .unwrap_or("")
        .to_string()
}

#[test]
fn g2_census_zero_trailing_guard_terms() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let summary = census_summary(root);
    eprintln!("{summary}");
    assert!(
        summary.contains(", 0 trailing"),
        "trailing guard-final `term;` sites appeared — each is observationally \
         identical to the exit spelling and should be written as `term! ->` \
         (or given a following statement if the checkpoint is intended). \
         Census said: {summary:?} — run scripts/g2_census.py -v for the sites"
    );
}
