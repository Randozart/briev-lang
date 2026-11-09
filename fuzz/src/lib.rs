//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! In-process fuzz adapters. Native execution remains in deterministic_transitions.
pub mod semantic;

/// Invalid programs are normal results; panics, hangs and sanitizer errors are not.
pub fn frontend(data: &[u8]) {
    if data.len() > 4096 {
        return;
    }
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(tokens) = briev_compiler::lexer::tokenize(source) else {
        return;
    };
    let _ = briev_compiler::parser::Parser::new(tokens, source).parse_program();
}
