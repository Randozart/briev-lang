// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-10 (H5 port): identifier-casing advisories over typed HIR.
//!
//! Mirror of `analysis::casing` (AST remains the fallback/oracle): SPEC §4.1
//! conventions are advisories, never errors. Categories and order match the
//! AST walker's declared-name surfaces; the final sort+dedup makes the output
//! independent of traversal order.

use super::program::{HirCallableKind, HirProgram};
use crate::analysis::casing::check_name;

pub fn analyze(program: &HirProgram) -> Vec<String> {
    let mut out = Vec::new();
    for callable in &program.callables {
        // AST checks TopLevel::Definition / TopLevel::Transaction items only:
        // not exports, members, ISR handlers, or compile-time callables.
        if callable.owner.is_some() || callable.exported || callable.is_isr {
            continue;
        }
        if matches!(
            callable.kind,
            HirCallableKind::Defn | HirCallableKind::Transaction
        ) {
            check_name(&mut out, "functions", &callable.name, false);
        }
    }
    for name in &program.type_names {
        check_name(&mut out, "types", name, true);
    }
    for name in &program.static_struct_names {
        check_name(&mut out, "structs", name, true);
    }
    for name in &program.trait_names {
        check_name(&mut out, "traits", name, true);
    }
    for name in &program.cell_names {
        check_name(&mut out, "cells", name, true);
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(src: &str) -> (Vec<String>, Vec<String>) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (
            analyze(&program),
            crate::analysis::casing::analyze(&items),
        )
    }

    #[test]
    fn conforming_program_is_silent_in_both() {
        let (h, a) = both("defn good(n: Int) -> Int { term n; };\ntype Meter: Int { v: Int; };");
        assert_eq!(h, a);
        assert!(h.is_empty(), "{h:?}");
    }

    #[test]
    fn each_category_matches_ast() {
        let src = "defn Bad(n: Int) -> Int { term n; };\n\
                   struct point { x: Int; };\n\
                   trait printable { };\n";
        let (h, a) = both(src);
        assert_eq!(h, a, "casing drift");
        assert_eq!(h.len(), 3, "{h:?}");
    }
}
