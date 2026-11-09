// ── Metadata Parser ────────────────────────────────────────────────────
// 2026-11-08 (attr plan): `attr [<owner>] key: value;` / `attr<owner> { … }`
// is the non-physical consumer metadata surface; `spec` owns physical layout.
// `!>` is retired (one diagnostic, `retired_exclaim_arrow`).

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
        while self.check(&Token::Attr) || self.check(&Token::ExclaimArrow)
            || self.check_identifier_prefix("#")
        {
            if self.check(&Token::ExclaimArrow) {
                return Err(self.retired_exclaim_arrow());
            }
            if self.eat(&Token::Attr) {
                let _owner = self.parse_attr_owner()?;
                let entries = self.parse_attr_entries()?;
                metadata.extend(entries);
                continue;
            }
            // #key value; or #key(value) — annotation syntax
            let key = self.expect_identifier()?; // e.g. "#inline"
            self.expect(Token::Semicolon)?;
            metadata.insert(key, PropertyValue::Bool(true));
        }
        Ok(metadata)
    }

    /// `!>` is retired (2026-11-08, attr plan). Kept as the one diagnostic so
    /// every former site names the replacement construct.
    pub(crate) fn retired_exclaim_arrow(&self) -> SyntaxError {
        self.error_at_current::<()>(
            "the `!>` metadata operator is retired. why: one operator mixed physical layout, consumer metadata, and policy, hiding which consumer owns a key. fix: write `attr key: value;` for consumer metadata, `attr<owner> { key: value; … }` for an owner schema, or `spec Name: value;` for physical layout."
        )
        .unwrap_err()
    }

    /// Optional `<owner>` after `attr`. The owner names the schema that claims
    /// the keys; it is a validation hint, not stored (keys are globally
    /// unique). Unknown owners are accepted (plugin domains).
    pub(crate) fn parse_attr_owner(&mut self) -> Result<Option<String>, SyntaxError> {
        if !self.eat(&Token::Lt) {
            return Ok(None);
        }
        // Owners may collide with keyword tokens (`accel`); read the
        // spelling like a metadata key so the diagnostic below is reachable.
        let owner = self.parse_metadata_key()?;
        self.expect(Token::Gt)?;
        if matches!(owner.as_str(), "accel" | "accel_report") {
            return Err(self
                .error_at_current::<()>(
                    "offload policy is a compiler mode, not an attr. fix: use `--accel=<keyword-only|try-all|force|try-all-force>` (verbose diagnostics replace the report key); `attr<accel>` does not exist.",
                )
                .unwrap_err());
        }
        Ok(Some(owner))
    }

    /// One `attr` declaration: either `key: value;` or `{ key: value; … }`.
    pub(crate) fn parse_attr_entries(&mut self) -> Result<HashMap<String, PropertyValue>, SyntaxError> {
        let mut entries = HashMap::new();
        if self.eat(&Token::LBrace) {
            while !self.check(&Token::RBrace) && !self.is_at_end() {
                let (key, val) = self.parse_attr_binding()?;
                entries.insert(key, val);
            }
            self.expect(Token::RBrace)?;
            self.eat(&Token::Semicolon);
            return Ok(entries);
        }
        let (key, val) = self.parse_attr_binding()?;
        entries.insert(key, val);
        Ok(entries)
    }

    /// `key: value;` — one attr binding.
    pub(crate) fn parse_attr_binding(&mut self) -> Result<(String, PropertyValue), SyntaxError> {
        let key = self.parse_metadata_key()?;
        self.expect(Token::Colon)?;
        let val = self.parse_metadata_value()?;
        self.expect(Token::Semicolon)?;
        Ok((key, val))
    }

    /// Parse a metadata key. Accepts plain identifiers and keyword tokens
    /// (e.g. `attr tbaa: "node";`). The keyword's
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
