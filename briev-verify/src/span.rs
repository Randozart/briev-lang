// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//!
//! Source positions.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, column: usize) -> Self {
        Span {
            start,
            end,
            line,
            column,
        }
    }

    pub fn dummy() -> Self {
        Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        }
    }

    /// Render a caret diagnostic pointing into `source`.
    pub fn format(&self, source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        if self.line > 0 && self.line <= lines.len() {
            let line_content = lines[self.line - 1];
            let pointer = " ".repeat(self.column.saturating_sub(1)) + "^";
            format!(
                " --> {}:{}:{}\n  |\n{} | {}\n{} | {}",
                "file", self.line, self.column, self.line, line_content, self.line, pointer
            )
        } else {
            format!(" --> {}:{}:{}", "file", self.line, self.column)
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}
