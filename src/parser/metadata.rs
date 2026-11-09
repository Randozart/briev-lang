// ── Metadata Parser ────────────────────────────────────────────────────
// 2026-07-12: Phase 1.5 — Parse `<~ expr;` metadata declarations.
// Flat code: each function is max 2 levels of nesting.

use super::helpers::Parser;
use crate::ast::PropertyValue;
use crate::errors::SyntaxError;
use crate::lexer::Token;
use std::collections::HashMap;

impl<'a> Parser<'a> {
    /// Parse metadata declarations inside a function body.
    /// Returns a map of key -> value.
    /// Stops when it encounters a non-metadata token.
    pub fn parse_body_metadata(&mut self) -> Result<HashMap<String, PropertyValue>, SyntaxError> {
        let mut metadata = HashMap::new();
        while self.check(&Token::ExclaimArrow) || self.check_identifier_prefix("#") {
            let key = if self.eat(&Token::ExclaimArrow) {
                // !> key: value; — inline metadata
                let key = self.parse_metadata_key()?;
                self.expect(Token::Colon)?;
                self.parse_metadata_value().map(|val| (key, val))?
            } else {
                // #key value; or #key(value) — annotation syntax
                let key = self.expect_identifier()?; // e.g. "#inline"
                let val = PropertyValue::Bool(true);
                (key, val)
            };
            self.expect(Token::Semicolon)?;
            metadata.insert(key.0, key.1);
        }
        Ok(metadata)
    }

    /// Parse a metadata key. Accepts plain identifiers and keyword tokens
    /// (e.g. `!> tbaa: "node";` — `tbaa` requires backticks/string form). The keyword's
    /// spelling comes from `Token::Display`, so the emitted key matches the
    /// source spelling.
    pub(crate) fn parse_metadata_key(&mut self) -> Result<String, SyntaxError> {
        match self.peek() {
            Some(Token::Identifier(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            Some(t) => {
                let s = t.to_string();
                if !s.is_empty() && s.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
                    self.pos += 1;
                    Ok(s)
                } else {
                    self.error_at_current("expected metadata key (identifier or keyword)")
                }
            }
            None => self.error_at_current("expected metadata key (identifier or keyword)"),
        }
    }

    /// Parse a single metadata value after `<~`.
    pub(crate) fn parse_metadata_value(&mut self) -> Result<PropertyValue, SyntaxError> {
        match self.peek() {
            Some(Token::Identifier(s)) => {
                // formatting <~ Quoted — identifier value
                let s = s.clone();
                self.pos += 1;
                Ok(PropertyValue::Identifier(s))
            }
            Some(Token::Integer(n)) => {
                let n = *n;
                self.pos += 1;
                Ok(PropertyValue::Int(n))
            }
            Some(Token::BoolTrue) => {
                self.pos += 1;
                Ok(PropertyValue::Bool(true))
            }
            Some(Token::BoolFalse) => {
                self.pos += 1;
                Ok(PropertyValue::Bool(false))
            }
            Some(Token::String(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(PropertyValue::String(s))
            }
            Some(Token::LBracket) => {
                // List value: [val1, val2, ...]
                self.pos += 1;
                let mut items = Vec::new();
                if !self.check(&Token::RBracket) {
                    loop {
                        items.push(self.parse_metadata_value()?);
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                }
                self.expect(Token::RBracket)?;
                Ok(PropertyValue::List(items))
            }
            _ => self.error_at_current(
                "expected metadata value (identifier, int, bool, string, or list)",
            ),
        }
    }

    /// Check if the current token is an identifier starting with '#'.
    fn check_identifier_prefix(&self, prefix: &str) -> bool {
        self.peek().map_or(
            false,
            |t| matches!(t, Token::Identifier(s) if s.starts_with(prefix)),
        )
    }
}
