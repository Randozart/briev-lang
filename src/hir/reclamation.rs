// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! R (2026-11-11): reclamation analysis over typed HIR — first slice.
//!
//! The full port translates the AST `Graph`/solver in
//! `analysis::reclamation` behind a parity gate. This module starts with the
//! closed-initializer predicate the entry-region decision reads; AST remains
//! authoritative until the port is complete.

use super::program::HirProgram;

/// True when every top-level initializer is a closed (literal/absent) value:
/// no state declaration and no non-literal top-level `let` initializer.
///
/// AST parity (`analysis::reclamation::walk::closed_initializers`): StateDecl
/// is closed=false; a top-level `let` is closed only for Int/Float/Bool/Char
/// literal or absent initializers; all other item kinds are closed=true.
pub fn closed_initializers(program: &HirProgram) -> bool {
    program.globals.iter().all(|global| {
        if global.is_state_decl {
            return false;
        }
        match &global.expr {
            None => true,
            Some(expr) => matches!(
                expr.kind,
                super::expr::HirExprKind::Literal(
                    super::expr::HirLiteral::Int(_)
                        | super::expr::HirLiteral::Float(_)
                        | super::expr::HirLiteral::Bool(_)
                        | super::expr::HirLiteral::Char(_)
                )
            ),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hir(src: &str) -> HirProgram {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        crate::hir::lower_program(&parser.parse_program().expect("parse"))
    }

    fn ast(src: &str) -> bool {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        crate::analysis::reclamation::closed_initializers_for_test(&items)
    }

    fn parity(src: &str) {
        assert_eq!(closed_initializers(&hir(src)), ast(src), "drift for:\n{src}");
    }

    #[test]
    fn literal_globals_are_closed() {
        parity("let a: Int = 1;\nlet b: Bool = false;\nlet c: Float = 1.5;\nlet d: Char = 'x';\n");
    }

    #[test]
    fn state_and_non_literal_globals_are_open() {
        parity("state let n: Int = 0;\n");
        parity("let xs: List<Int> = [1, 2];\n");
        parity("let n: Int = compute();\ndefn compute() -> Int { term 1; };\n");
    }

    #[test]
    fn no_globals_is_closed() {
        parity("defn f() -> Int { term 1; };\n");
    }
}
