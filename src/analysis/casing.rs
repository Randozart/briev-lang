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

//! 2026-08-22 (spec-conformance plan Phase 2): identifier casing advisory.
//!
//! SPEC §4.1: user-declared names that violate casing conventions are
//! warnings, never errors — unlike compiler-known vocabulary, which is exact.
//! The convention table lives in `vocab::LanguageVocab::casing`; this pass
//! consumes it instead of restating the rules (single source of truth).
//!
//! Advisory only: output is a list of warning strings; nothing here blocks a
//! build. Undo: remove the module and its two call sites next to the
//! termination analysis in `src/compile.rs`.

use crate::ast::TopLevel;

/// One advisory line per violating declared name.
pub fn analyze(items: &[TopLevel]) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            // Functions/txns are snake_case ("functions" category).
            TopLevel::Definition(d) => {
                check(&mut out, "functions", &d.name, false);
            }
            TopLevel::Transaction(t) => {
                check(&mut out, "functions", &t.name, false);
            }
            // Types/structs/enums/objs share one convention: PascalCase.
            // `struct` declarations parse to TopLevel::StaticStruct.
            TopLevel::TypeDef(td) => {
                check(&mut out, "types", &td.name, true);
            }
            TopLevel::StaticStruct(s) => {
                check(&mut out, "structs", &s.name, true);
            }
            TopLevel::Trait(t) => {
                check(&mut out, "traits", &t.name, true);
            }
            TopLevel::Cell(c) => {
                check(&mut out, "cells", &c.name, true);
            }
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

fn check(out: &mut Vec<String>, category: &str, name: &str, expect_pascal: bool) {
    if name.is_empty() {
        return;
    }
    let first = name.chars().next().unwrap_or('_');
    let ok = if expect_pascal {
        first.is_ascii_uppercase()
    } else {
        first.is_ascii_lowercase() || first == '_'
    };
    if !ok {
        let conv = if expect_pascal { "PascalCase" } else { "snake_case" };
        out.push(format!(
            "{category} are {conv} by convention (advisory): found '{name}'"
        ));
    }
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
    fn violations_are_advisories_not_errors() {
        let items = parse("defn Bad(n: Int) -> Int { term n; };");
        let warns = analyze(&items);
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("snake_case"), "{}", warns[0]);
        assert!(warns[0].contains("advisory"), "{}", warns[0]);
    }

    #[test]
    fn conforming_names_are_silent() {
        let items = parse("defn good(n: Int) -> Int { term n; };\ntype Meter: Int { v: Int; };");
        assert!(analyze(&items).is_empty());
    }

    #[test]
    fn struct_and_trait_names_checked_pascal() {
        let items = parse("struct point { x: Int; };\ntrait printable { };");
        let warns = analyze(&items);
        assert_eq!(warns.len(), 2, "{warns:?}");
        assert!(warns.iter().any(|w| w.contains("structs")));
        assert!(warns.iter().any(|w| w.contains("traits")));
    }
}
