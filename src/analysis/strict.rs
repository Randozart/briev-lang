// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 2026-08-22 (spec-conformance plan Phase 9, SPEC §3.2): `.s` strict
//! profile enforcement.
//!
//! A dotted-profile source (`main.s.bv`, `ui.s.rbv` — NEVER compound
//! `.sbv`/`.srbv`; `conformance::classify` already rejects those) demands
//! the compiler PROVE everything it can: representation fallbacks that a
//! normal profile tolerates (a heap field whose lifetime proof fell back to
//! "lives for the program") become hard errors citing the decision and the
//! fix. Proof obligations, trivial contracts, and concurrency classification
//! are ALREADY global hard errors — strict adds, never relaxes.
//!
//! Strict also emits a trust-boundary report next to the artifact listing
//! every foreign symbol the compiler takes on faith (frgn declarations),
//! satisfying §3.2's visible verification report.
//!
//! Undo: remove this module + its two call sites in `src/compile.rs`
//! (enforcement gate and report write).

use crate::ast::TopLevel;
use crate::macros::memcheck::MemcheckReport;

/// Enforce strict acceptance. Returns the house-style error listing every
/// fallback decision with its reason and fix when any exist.
pub fn enforce(items: &[TopLevel], mc: &MemcheckReport) -> Result<(), String> {
    let mut lines: Vec<String> = Vec::new();
    let mut fallbacks = mc.lifetime.lifetime_fallbacks.clone();
    fallbacks.sort();
    for (field, reason) in &fallbacks {
        lines.push(format!(
            "  {} lives for the program (unprovable — {}) under `.s`: \
             prove the last use or add `free x;` / an init capacity bound",
            field, reason
        ));
    }
    if lines.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "strict profile rejections:\n{}",
            lines.join("\n")
        ))
    }
}

/// The trust boundaries: every foreign symbol the compiler takes on faith
/// (frgn declarations). asm blocks are inline machine code — listed too.
pub fn trusted_axioms(items: &[TopLevel]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        match item {
            TopLevel::ForeignBinding(f) => {
                let shown = f.briev_name.as_ref().unwrap_or(&f.foreign_name);
                out.push(format!("frgn {}", shown));
            }
            TopLevel::AsmFn(a) => out.push(format!("asm {}", a.name)),
            TopLevel::BadFn(bf) => out.push(format!("bad {}", bf.name)),
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Render the verification report body (trust boundaries + memory decisions).
pub fn render_report(items: &[TopLevel], mc: &MemcheckReport) -> String {
    let mut s = String::from("=== .s verification report ===\n");
    s.push_str("trust boundaries (compiler takes these on faith):\n");
    let axioms = trusted_axioms(items);
    if axioms.is_empty() {
        s.push_str("  (none)\n");
    }
    for a in &axioms {
        s.push_str(&format!("  {}\n", a));
    }
    s.push_str("memory decisions:\n");
    let mut fields = mc.field_names.clone();
    fields.sort();
    if fields.is_empty() {
        s.push_str("  (no state fields)\n");
    }
    // 2026-08-22 (Phase 9): one decision wording, two surfaces (DRY with
    // memcheck::field_decision_line).
    for f in &fields {
        s.push_str(&crate::macros::memcheck::field_decision_line(mc, f));
        s.push('\n');
    }
    s.push_str("=== end report ===\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    #[test]
    fn heap_field_without_consumer_is_a_strict_violation() {
        let items = parse("let buf: Ptr<Int> = Malloc#(64);");
        let mc = crate::macros::memcheck::run_memcheck(&items);
        assert!(mc.lifetime.lifetime_fallbacks.is_empty() == false || !mc.field_names.is_empty());
        // The scheduler records WHY it fell back; strict escalates it.
        let err = enforce(&items, &mc).expect_err("fallback must fail under .s");
        assert!(err.contains("buf"), "{}", err);
        assert!(err.contains("prove the last use"), "{}", err);
    }

    #[test]
    fn manually_freed_field_passes_strict() {
        let items = parse(
            "let buf: Ptr<Int> = Malloc#(64);\ntxn drop [true][true] { Free#(buf); term; };",
        );
        let mc = crate::macros::memcheck::run_memcheck(&items);
        assert!(enforce(&items, &mc).is_ok(), "manual free = user-managed");
    }

    #[test]
    fn scalar_fields_never_violate() {
        let items = parse("let n: Int = 3;");
        let mc = crate::macros::memcheck::run_memcheck(&items);
        assert!(enforce(&items, &mc).is_ok(), "scalars are not heap fallbacks");
    }

    #[test]
    fn trust_report_lists_frgn_and_asm() {
        let items = parse(
            "frgn puts(x: Int) -> Int from #System;\nasm <x86_64> blink() -> Void [true][true] {\"nop\"};",
        );
        let axioms = trusted_axioms(&items);
        assert_eq!(axioms.len(), 2, "{axioms:?}");
        assert!(axioms.iter().any(|a| a.starts_with("frgn puts")));
        assert!(axioms.iter().any(|a| a.starts_with("asm blink")));
    }

    #[test]
    fn report_renders_decisions() {
        let items = parse("let buf: Ptr<Int> = Malloc#(64);");
        let mc = crate::macros::memcheck::run_memcheck(&items);
        let r = render_report(&items, &mc);
        assert!(r.contains("trust boundaries"));
        assert!(r.contains("lives for the program"));
    }
}
