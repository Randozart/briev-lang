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

//! Frontend "No-Panic" Fuzzer
//!
//! Verifies that the lexer and parser never panic on any input.
//! Tests both garbage input and structured-but-invalid Briv syntax.

use briev_compiler::lexer::Token;
use briev_compiler::parser::Parser;
use logos::Logos;
use proptest::prelude::*;

/// Generate random garbage strings (bytes, unicode, etc.)
fn arb_garbage_input() -> impl Strategy<Value = String> {
    prop_oneof![
        // Random ASCII bytes
        proptest::string::string_regex("[\\x00-\\x7F]{0,200}").unwrap(),
        // Random printable ASCII
        proptest::string::string_regex("[ -~]{0,200}").unwrap(),
        // Mixed valid Briv keywords with garbage
        proptest::string::string_regex("[a-z_]{0,50}(let|txn|defn|sig|term|escape|true|false)[a-z_]{0,50}").unwrap(),
        // Repeated special characters
        proptest::string::string_regex("([!@#$%^&*()+=<>?/\\\\|~`]){1,100}").unwrap(),
        // Nested brackets without content
        proptest::string::string_regex("([{}\\[\\]()]){0,50}").unwrap(),
        // Numbers and operators
        proptest::string::string_regex("[0-9+\\-*/=<>!&|%^~]{0,100}").unwrap(),
    ]
}

/// Generate structured-but-invalid Briv syntax
fn arb_structured_invalid() -> impl Strategy<Value = String> {
    prop_oneof![
        // Missing semicolons
        Just("let x: Int = 5 txn foo [true][true] { term; }".to_string()),
        // Unmatched brackets
        Just("let x: Int = 5; txn foo [true][true { term; };".to_string()),
        // Invalid type names
        Just("let x: NotAType = 5;".to_string()),
        // Empty transaction body
        Just("txn empty [true][true] {};".to_string()),
        // Missing contract
        Just("txn no_contract { term; };".to_string()),
        // Invalid expressions
        Just("let x: Int = + - * /;".to_string()),
        // Nested without termination
        Just("txn nested [true][true] { [true] { [true] { term; }; }; };".to_string()),
        // Unicode edge cases
        Just("let 你好: Int = 5;".to_string()),
        // Very long identifiers
        Just(format!("let {}: Int = 0;", "a".repeat(1000))),
        // Mixed case keywords
        Just("LeT x: InT = 5; TxN foo [TrUe][FaLsE] { TeRm; };".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn test_lexer_never_panics(input in arb_garbage_input()) {
        // The lexer should never panic on any input
        let mut lexer = Token::lexer(&input);
        while let Some(result) = lexer.next() {
            // Just iterate through all tokens, don't panic on errors
            if let Err(_) = result {
                // Lexer errors are expected for garbage input
            }
        }
    }

    #[test]
    fn test_parser_never_panics_on_garbage(input in arb_garbage_input()) {
        // The parser should never panic, even on garbage input.
        // A lex error is an expected outcome for garbage — skip to the
        // parser only when the lexer succeeded.
        let tokens = match briev_compiler::lexer::tokenize(&input) {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };
        let mut parser = Parser::new(tokens, &input);
        let result = parser.parse_program();
        
        // Either Ok(Program) or Err(SyntaxError) is acceptable
        // The important thing is no panic
        match result {
            Ok(_) => {
                // Valid program parsed from garbage - that's fine
            }
            Err(_) => {
                // Expected error for garbage input
            }
        }
    }

    #[test]
    fn test_parser_never_panics_on_structured_invalid(input in arb_structured_invalid()) {
        // Structured-but-invalid input: lex errors are expected outcomes.
        let tokens = match briev_compiler::lexer::tokenize(&input) {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };
        let mut parser = Parser::new(tokens, &input);
        let result = parser.parse_program();
        
        match result {
            Ok(_) => {
                // Sometimes invalid-looking input is actually valid
            }
            Err(_) => {
                // Expected error for invalid input
            }
        }
    }

    #[test]
    fn test_lexer_parser_roundtrip(input in arb_garbage_input()) {
        // Test that lexer output can always be consumed by parser without panic
        let _lexer = Token::lexer(&input);
        
        // Now parse the same input (lex error = fine)
        let Ok(tokens) = briev_compiler::lexer::tokenize(&input) else { return Ok(()); };
        let mut parser = Parser::new(tokens, &input);
        let _ = parser.parse_program();
        
        // No panic = success
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        let mut parser = Parser::new(vec![], "");
        let result = parser.parse_program();
        assert!(result.is_ok(), "Empty input should parse to empty program");
    }

    #[test]
    fn test_whitespace_only() {
        let mut parser = Parser::new(briev_compiler::lexer::tokenize("   \n\t  \n  ").unwrap(), "   \n\t  \n  ");
        let result = parser.parse_program();
        assert!(result.is_ok(), "Whitespace-only input should parse to empty program");
    }

    #[test]
    fn test_comments_only() {
        let mut parser = Parser::new(briev_compiler::lexer::tokenize("// comment 1\n// comment 2\n").unwrap(), "// comment 1\n// comment 2\n");
        let result = parser.parse_program();
        assert!(result.is_ok(), "Comments-only input should parse to empty program");
    }

    #[test]
    fn test_null_bytes() {
        let input = "\0\0\0";
        let mut lexer = Token::lexer(input);
        // Should not panic
        while let Some(_) = lexer.next() {}
    }

    #[test]
    fn test_very_long_input() {
        let input = "let x: Int = ".to_string() + &"0 + ".repeat(10000) + "0;";
        let mut parser = Parser::new(briev_compiler::lexer::tokenize(&input).unwrap(), &input);
        let result = parser.parse_program();
        // Should not panic, may succeed or fail depending on parser limits
        let _ = result;
    }

    #[test]
    fn test_unicode_edge_cases() {
        let inputs = vec![
            "🦀🦀🦀",
            "let 日本語: Int = 5;",
            "txn 测试 [true][true] { term; };",
            "\u{200B}", // Zero-width space
            "\u{FEFF}", // BOM
        ];
        
        for input in inputs {
            let Ok(tokens) = briev_compiler::lexer::tokenize(input) else {
                continue; // lex error is a valid outcome for exotic input
            };
            let mut parser = Parser::new(tokens, input);
            let _ = parser.parse_program();
        }
    }
}
