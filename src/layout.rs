// ── .f Formatted-Source Layout Frontend ────────────────────────────────
// 2026-08-06: Rewrite of the indentation frontend for the `.f` dotted
// profile (SPEC §3.2). Token-aware: it operates on the lexed token stream
// (not raw text), so multi-line raw strings, comments, and literals are
// handled by the lexer. It inserts synthetic brace/semicolon/comma tokens so
// the existing parser runs unchanged and produces the SAME AST as canonical
// brace syntax — the SPEC §25 equivalence contract.
//
// Rules (pure indent-delta; statement braces and semicolons are forbidden in
// the source and synthesized here):
//   * A fresh line followed by a deeper line is a block header: a synthetic
//     `{` is inserted after it and a block is pushed.
//   * Same-indent lines are siblings and get a synthetic terminator (`;`, or
//     `,` inside an `enum` body).
//   * A shallower line pops blocks; each pop emits `}`.
//   * Continuations (no terminator, no block): open bracket `(`/`[`, a line
//     starting with `.` (method chain), or a line ending in a binary/trailing
//     operator. A `=>`-terminated line followed by a deeper line opens an arm
//     body block.
//   * Contract clauses `[...]` on deeper bracket-shaped lines are absorbed
//     into a preceding declaration header (canonical `defn f()\n    [pre]\n{`).
//   * Source `{`, `}`, and `;` tokens are rejected with a helpful error.

use crate::errors::{Span, SyntaxError};
use crate::lexer::{tokenize, Token};
use std::ops::Range;

/// A run of tokens on one physical line.
struct LogicalLine {
    tokens: Vec<(Token, Range<usize>)>,
    indent: usize,
    line: usize,
    inert: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Sep {
    Semi,
    Comma,
}

struct Block {
    body_indent: usize,
    sep: Sep,
}

/// Run the `.f` layout frontend over `source`, producing a token stream the
/// canonical parser accepts. Spans of real tokens are preserved verbatim;
/// synthetic tokens carry a zero-width span at the end of the line they close.
pub fn layout_process(source: &str) -> Result<Vec<(Token, Range<usize>)>, SyntaxError> {
    let tokens = tokenize(source).map_err(|e| SyntaxError::InvalidExpression {
        reason: format!("lex error in formatted source: {}", e),
        span: Span::dummy(),
    })?;
    let line_starts = compute_line_starts(source);
    let lines = group_lines(tokens, source, &line_starts)?;
    walk_lines(lines)
}

/// Byte offset of the start of each physical line (0 = first line).
fn compute_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn line_and_col(offset: usize, line_starts: &[usize]) -> (usize, usize) {
    let line = match line_starts.binary_search(&offset) {
        Ok(l) => l,
        Err(l) => l - 1,
    };
    (line, offset - line_starts[line])
}

fn span_at(tok_span: &Range<usize>, line_starts: &[usize]) -> Span {
    let (line, col) = line_and_col(tok_span.start, line_starts);
    Span::new(tok_span.start, tok_span.end, line + 1, col + 1)
}

/// Reject forbidden tokens and group the stream into logical lines.
fn group_lines(
    tokens: Vec<(Token, Range<usize>)>,
    source: &str,
    line_starts: &[usize],
) -> Result<Vec<LogicalLine>, SyntaxError> {
    let mut lines: Vec<LogicalLine> = Vec::new();
    for (tok, span) in tokens {
        if matches!(tok, Token::LBrace | Token::RBrace | Token::Semicolon) {
            let what = match tok {
                Token::LBrace => "statement braces ('{')",
                Token::RBrace => "statement braces ('}')",
                Token::Semicolon => "semicolon terminators (';')",
                _ => unreachable!(),
            };
            return Err(SyntaxError::InvalidExpression {
                reason: format!(
                    "{} are forbidden in formatted (.f) sources; delimit blocks with indentation instead",
                    what
                ),
                span: span_at(&span, line_starts),
            });
        }
        let (line, col) = line_and_col(span.start, line_starts);
        let line_start = line_starts[line];
        if let Some(last) = lines.last_mut() {
            if last.line == line {
                last.tokens.push((tok, span));
                continue;
            }
        }
        let indent = col;
        if indent > 0
            && source[line_start..span.start].contains('\t')
        {
            return Err(SyntaxError::InvalidExpression {
                reason: "tabs are not allowed in formatted (.f) indentation; use spaces".into(),
                span: span_at(&span, line_starts),
            });
        }
        lines.push(LogicalLine {
            tokens: vec![(tok, span)],
            indent,
            line,
            inert: false,
        });
    }
    for l in &mut lines {
        l.inert = l
            .tokens
            .iter()
            .all(|(t, _)| matches!(t, Token::DocComment(_) | Token::DocCommentBang(_)));
    }
    Ok(lines)
}

fn is_trailing_op(tok: &Token) -> bool {
    matches!(
        tok,
        Token::Plus
            | Token::Minus
            | Token::Star
            | Token::Slash
            | Token::Percent
            | Token::Eq
            | Token::EqEq
            | Token::Ne
            | Token::Lt
            | Token::Le
            | Token::Gt
            | Token::Ge
            | Token::Shl
            | Token::Shr
            | Token::Pipe
            | Token::OrOr
            | Token::AndAnd
            | Token::BitXor
            | Token::Dot
            | Token::Arrow
            | Token::ArrowLeft
            | Token::TildeArrowLeft
            | Token::Comma
            | Token::DotDot
            | Token::DotDotEq
            | Token::Ellipsis
            | Token::ColonEq
            | Token::Ampersand
            | Token::Question
            | Token::Tilde
            | Token::TildePlus
            | Token::TildeMinus
            | Token::TildeStar
            | Token::TildeSlash
            | Token::TildeEq
            | Token::PlusEq
            | Token::MinusEq
            | Token::StarEq
            | Token::SlashEq
            | Token::PipeEq
            | Token::AmpEq
            | Token::CaretEq
            | Token::ShlEq
            | Token::ShrEq
    )
}

/// Declaration headers that may carry `[...]` contract clauses on deeper lines.
fn is_declaration_header(first: &Token) -> bool {
    matches!(
        first,
        Token::Defn
            | Token::Node
            | Token::Txn
            | Token::Trg
            | Token::Reg
            | Token::Trait
            | Token::Impl
            | Token::Type
            | Token::Struct
            | Token::Enum
            | Token::Obj
            | Token::Cell
            | Token::Within
            | Token::Async
            | Token::Sync
            | Token::Frgn
    )
}

fn first_token(line: &LogicalLine) -> Option<&Token> {
    line.tokens.first().map(|(t, _)| t)
}

fn last_token(line: &LogicalLine) -> Option<&Token> {
    line.tokens.last().map(|(t, _)| t)
}

fn last_span_end(line: &LogicalLine) -> usize {
    line.tokens.last().map_or(0, |(_, s)| s.end)
}

fn next_non_inert(lines: &[LogicalLine], from: usize) -> Option<usize> {
    (from..lines.len()).find(|&k| !lines[k].inert)
}

/// A `[...]`-shaped line: starts and ends with a bracket clause. Contract
/// clauses are bracket-shaped; a guarded statement `[c] foo()` is not (it ends
/// with the statement's last token).
fn is_bracket_shaped(line: &LogicalLine) -> bool {
    matches!(first_token(line), Some(Token::LBracket)) && matches!(last_token(line), Some(Token::RBracket))
}

/// Whether the next line after `next` starts a method chain (`.foo`).
fn next_line_starts_chain(lines: &[LogicalLine], next: Option<usize>) -> bool {
    next.is_some_and(|k| matches!(first_token(&lines[k]), Some(Token::Dot)))
}

/// Whether the line at `next` sits deeper than `indent` (opens a block).
fn next_line_deeper(lines: &[LogicalLine], next: Option<usize>, indent: usize) -> bool {
    next.is_some_and(|k| lines[k].indent > indent)
}

fn line_ends_fat_arrow(line: &LogicalLine) -> bool {
    matches!(last_token(line), Some(Token::FatArrow))
}

/// A line ending in `=>`: a deeper next line opens an arm-body block,
/// otherwise the expression continues. Returns true when the line ended in
/// `=>` and the caller must continue without terminating.
fn handle_fat_arrow(
    ctx: &mut LayoutCtx,
    line: &LogicalLine,
    lines: &[LogicalLine],
    next_before: Option<usize>,
    indent: usize,
) -> bool {
    if !line_ends_fat_arrow(line) {
        return false;
    }
    if next_line_deeper(lines, next_before, indent) {
        ctx.open_block(line, next_before.unwrap(), lines, last_span_end(line));
    }
    true
}

/// Absorb deeper bracket-shaped contract clauses into a declaration header and
/// return `(first line after the header, closing-artifact offset)`.
fn absorb_decl_header(
    ctx: &mut LayoutCtx,
    lines: &[LogicalLine],
    i: usize,
    indent: usize,
    at: usize,
) -> (usize, usize) {
    let mut k = i + 1;
    if is_declaration_header(first_token(&lines[i]).unwrap_or(&Token::Identifier(String::new()))) {
        k = ctx.absorb_contracts(lines, k, indent);
    }
    let header_end = lines[i + 1..k]
        .iter()
        .filter(|l| !l.inert)
        .map(last_span_end)
        .max()
        .unwrap_or(at);
    (k, header_end)
}

/// Shared walk state: the emitted token stream, the open-block stack, and the
/// running bracket depth (open `(`/`[`). Encapsulated so the walk loop stays
/// flat and each layout rule is a named helper.
struct LayoutCtx {
    out: Vec<(Token, Range<usize>)>,
    stack: Vec<Block>,
    bracket_depth: usize,
}

impl LayoutCtx {
    fn new() -> Self {
        LayoutCtx { out: Vec::new(), stack: Vec::new(), bracket_depth: 0 }
    }

    /// Append a synthetic zero-width token at `at`.
    fn emit_synth(&mut self, tok: Token, at: usize) {
        self.out.push((tok, at..at));
    }

    /// Append a line's real tokens, tracking bracket depth. Returns the
    /// byte offset of the line's final token.
    fn emit_line_tokens(&mut self, line: &LogicalLine) -> usize {
        for (tok, span) in &line.tokens {
            match tok {
                Token::LParen | Token::LBracket => self.bracket_depth += 1,
                Token::RParen | Token::RBracket => self.bracket_depth = self.bracket_depth.saturating_sub(1),
                _ => {}
            }
            self.out.push((tok.clone(), span.clone()));
        }
        last_span_end(line)
    }

    /// Absorb deeper bracket-shaped contract lines (`[pre][post]`) into a
    /// preceding declaration header. Returns the index of the first line after
    /// the absorbed contracts (skipping inert lines).
    fn absorb_contracts(&mut self, lines: &[LogicalLine], mut k: usize, indent: usize) -> usize {
        while let Some(ni) = next_non_inert(lines, k) {
            if lines[ni].indent > indent && is_bracket_shaped(&lines[ni]) {
                self.emit_line_tokens(&lines[ni]);
                k = ni + 1;
            } else {
                break;
            }
        }
        k
    }

/// Close the top block, emitting `}` and (at bracket depth zero) the
    /// owning statement's trailing `;`.
    fn close_block(&mut self, at: usize) {
        self.stack.pop();
        self.emit_synth(Token::RBrace, at);
        // The statement/declaration that opened this block is terminated after
        // its closing brace: `defn f() { .. };`, `term match x { .. };`.
        // Inside parentheses/brackets the closing is an expression
        // (`foo(Person { .. })`) — the enclosing statement supplies the `;`.
        if self.bracket_depth == 0 {
            self.emit_synth(Token::Semicolon, at);
        }
    }

    /// Close blocks whose body indent is deeper than `target`.
    fn close_blocks_to(&mut self, target: usize, at: usize) {
        while let Some(top) = self.stack.last() {
            if top.body_indent <= target {
                break;
            }
            self.close_block(at);
        }
    }

    /// Close every open block (end of file).
    fn close_all(&mut self, at: usize) {
        while self.stack.last().is_some() {
            self.close_block(at);
        }
    }

    /// Push a block opened by `line`, whose first body line is `lines[ni]`.
    /// `enum` bodies are comma-separated; every other block is `;`-separated.
    fn open_block(&mut self, line: &LogicalLine, ni: usize, lines: &[LogicalLine], at: usize) {
        let sep = if matches!(first_token(line), Some(Token::Enum)) {
            Sep::Comma
        } else {
            Sep::Semi
        };
        self.stack.push(Block { body_indent: lines[ni].indent, sep });
        self.emit_synth(Token::LBrace, at);
    }

    /// Terminate a plain statement, then close blocks shallower than the next
    /// line. A non-zero next line that matches no open block is an indent
    /// inconsistency.
    fn terminate_statement(
        &mut self,
        lines: &[LogicalLine],
        next: Option<usize>,
        indent: usize,
        at: usize,
    ) -> Result<(), SyntaxError> {
        let sep = self.stack.last().map_or(Sep::Semi, |b| b.sep);
        self.emit_synth(
            match sep {
                Sep::Semi => Token::Semicolon,
                Sep::Comma => Token::Comma,
            },
            at,
        );
        match next {
            Some(ni) => {
                self.close_blocks_to(lines[ni].indent, at);
                match self.stack.last() {
                    Some(top) if top.body_indent != lines[ni].indent => {
                        Err(inconsistent_indent(&lines[ni]))
                    }
                    None if lines[ni].indent != 0 => Err(inconsistent_indent(&lines[ni])),
                    _ => Ok(()),
                }
            }
            None => {
                self.close_all(at);
                Ok(())
            }
        }
    }
}

fn walk_lines(lines: Vec<LogicalLine>) -> Result<Vec<(Token, Range<usize>)>, SyntaxError> {
    let mut ctx = LayoutCtx::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if line.inert {
            ctx.emit_line_tokens(line);
            i += 1;
            continue;
        }
        let indent = line.indent;
        let at = ctx.emit_line_tokens(line);

        // In-bracket continuation: no terminator, no block.
        if ctx.bracket_depth > 0 {
            i += 1;
            continue;
        }

        let next_before = next_non_inert(&lines, i + 1);

        // Method-chain continuation: the next line starts with `.`.
        if next_line_starts_chain(&lines, next_before) {
            i += 1;
            continue;
        }

        // `=>` at end of line: an arm-body block opens on a deeper next line;
        // either way the expression continues across the boundary.
        if handle_fat_arrow(&mut ctx, line, &lines, next_before, indent) {
            i += 1;
            continue;
        }

        // Trailing-operator continuation.
        if last_token(line).map_or(false, is_trailing_op) {
            i += 1;
            continue;
        }

        // Absorb contract clauses into a declaration header.
        let (k, header_end) = absorb_decl_header(&mut ctx, &lines, i, indent, at);
        let next = next_non_inert(&lines, k);

        // Fresh line: a deeper next line opens a block (pure indent-delta).
        if next_line_deeper(&lines, next, indent) {
            ctx.open_block(line, next.unwrap(), &lines, header_end);
            i = k;
            continue;
        }

        // Plain statement: terminate, then pop blocks for a shallower next line.
        ctx.terminate_statement(&lines, next, indent, at)?;
        i += 1;
    }
    Ok(ctx.out)
}

fn inconsistent_indent(line: &LogicalLine) -> SyntaxError {
    let span = match line.tokens.first() {
        Some((_, s)) => Span::new(s.start, s.end, line.line + 1, line.indent + 1),
        None => Span::dummy(),
    };
    SyntaxError::InvalidExpression {
        reason: format!(
            "inconsistent indentation: column {} does not match any open block",
            line.indent + 1
        ),
        span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Result<Vec<(Token, Range<usize>)>, String> {
        layout_process(src).map_err(|e| e.to_string())
    }

    fn layout(src: &str) -> String {
        toks(src)
            .map(|v| {
                v.iter()
                    .map(|(t, _)| format!("{:?}", t))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|e| format!("ERROR: {}", e))
    }

    #[test]
    fn simple_node_block() {
        let src = "node start [a][b]\n    foo()\n    term\n";
        let out = layout(src);
        assert!(out.contains("LBrace"), "missing block open: {out}");
        assert!(out.contains("Semicolon"), "missing terminator: {out}");
        assert!(out.contains("RBrace"), "missing block close: {out}");
    }

    #[test]
    fn contracts_absorbed_into_header() {
        let src = "defn f(x: Int) -> Int\n    [x > 0][true]\n    term x\n";
        let out = toks(src).unwrap();
        let idx_brace = out.iter().position(|(t, _)| *t == Token::LBrace).unwrap();
        let idx_int = out.iter().position(|(t, _)| *t == Token::Integer(0)).unwrap();
        assert!(idx_brace > idx_int, "block must open after contracts");
    }

    #[test]
    fn semicolon_forbidden() {
        let src = "let x = 1;\n";
        assert!(toks(src).is_err());
    }

    #[test]
    fn brace_forbidden() {
        let src = "defn f()\n{\n}\n";
        assert!(toks(src).is_err());
    }

    #[test]
    fn continuation_in_brackets() {
        let src = "let x = foo(\n    a,\n    b\n)\n";
        let out = toks(src).unwrap();
        assert_eq!(out.iter().filter(|(t, _)| *t == Token::Semicolon).count(), 1);
        assert!(!out.iter().any(|(t, _)| *t == Token::LBrace));
    }

    #[test]
    fn method_chain_continuation() {
        let src = "let x = foo\n    .bar()\n    .baz()\n";
        let out = toks(src).unwrap();
        assert_eq!(out.iter().filter(|(t, _)| *t == Token::Semicolon).count(), 1);
        assert!(!out.iter().any(|(t, _)| *t == Token::LBrace));
    }

    #[test]
    fn nested_blocks_close_at_dedent() {
        let src = "node a [t][f]\n    foo()\n        bar()\n    baz()\n";
        let out = toks(src).unwrap();
        assert_eq!(out.iter().filter(|(t, _)| *t == Token::LBrace).count(), 2);
        assert_eq!(out.iter().filter(|(t, _)| *t == Token::RBrace).count(), 2);
    }

    #[test]
    fn enum_variants_use_comma() {
        let src = "enum Color\n    Red\n    Green\n    Blue\n";
        let out = toks(src).unwrap();
        // Variants are comma-separated; only the trailing declaration `;`
        // (after the closing brace) may appear.
        let commas = out.iter().filter(|(t, _)| *t == Token::Comma).count();
        let semis = out.iter().filter(|(t, _)| *t == Token::Semicolon).count();
        assert_eq!(commas, 3, "3 variants must be comma-separated");
        assert_eq!(semis, 1, "only the trailing declaration ';'");
    }

    #[test]
    fn raw_string_with_braces_untouched() {
        let src = "let s = #r\"{a}; b\"\nfoo()\n";
        let out = layout(src);
        assert!(out.contains("RawString"), "raw string must lex as one token: {out}");
    }

    #[test]
    fn match_arms_terminate() {
        let src = "term match n\n    _ when n < 0 => -1\n    0 => 0\n    _ => 1\n";
        let out = layout(src);
        assert!(out.contains("Semicolon"), "match arms need terminators: {out}");
        assert!(out.contains("LBrace"), "match needs an opening brace: {out}");
    }

    #[test]
    fn arm_body_block_after_fat_arrow() {
        let src = "match x\n    \"a\" =>\n        foo()\n    \"b\" =>\n        bar()\n";
        let out = layout(src);
        assert_eq!(out.matches("LBrace").count(), 3, "match + 2 arm bodies: {out}");
        assert_eq!(out.matches("RBrace").count(), 3, "close all: {out}");
    }

    #[test]
    fn top_level_statements_terminate() {
        let src = "let a = 1\nlet b = 2\n";
        let out = toks(src).unwrap();
        assert_eq!(out.iter().filter(|(t, _)| *t == Token::Semicolon).count(), 2);
    }

    #[test]
    fn tabs_rejected() {
        let src = "defn f()\n\tfoo()\n";
        assert!(toks(src).is_err());
    }

    #[test]
    fn inconsistent_dedent_rejected() {
        let src = "defn f()\n    foo()\n        bar()\n      baz()\n";
        assert!(toks(src).is_err());
    }

    #[test]
    fn doc_comment_lines_inert() {
        let src = "/// doc for node\nnode start [a][b]\n    foo()\n";
        let out = toks(src).unwrap();
        assert!(out.iter().any(|(t, _)| matches!(t, Token::DocComment(_))));
        assert!(out.iter().any(|(t, _)| *t == Token::LBrace));
    }

    #[test]
    fn guard_statement_own_line() {
        let src = "foo()\n[x > 0] bar()\n";
        let out = layout(src);
        assert!(out.contains("Semicolon"), "foo() must terminate: {out}");
        assert!(out.contains("LBracket"), "guard retained: {out}");
    }

    /// SPEC §25 equivalence: a `.f` source and its canonical brace twin must
    /// produce the identical AST.
    fn assert_ast_equivalent(formatted: &str, canonical: &str) {
        let f_tokens = layout_process(formatted).expect("formatted layout failed");
        let c_tokens = crate::lexer::tokenize(canonical).expect("canonical lex failed");
        let mut fp = crate::parser::Parser::new(f_tokens, formatted);
        let mut cp = crate::parser::Parser::new(c_tokens, canonical);
        let f_ast = fp
            .parse_program()
            .unwrap_or_else(|e| panic!("formatted parse failed: {e}\nsource:\n{formatted}"));
        let c_ast = cp.parse_program().expect("canonical parse failed");
        // Behavioral comparison via the canonical formatter: identical ASTs
        // format identically (the formatter is round-trip stable, Phase 2).
        let f_fmt = crate::ast::format_program(&f_ast);
        let c_fmt = crate::ast::format_program(&c_ast);
        assert_eq!(f_fmt, c_fmt, "AST mismatch!\nformatted:\n{formatted}\ncanonical:\n{canonical}");
    }

    #[test]
    fn ast_equiv_node_block() {
        assert_ast_equivalent(
            "node start [done == false][done == true]\n    let x: Int = 1\n    done = true\n    term\n",
            "node start [done == false][done == true] {\n    let x: Int = 1;\n    done = true;\n    term;\n};",
        );
    }

    #[test]
    fn ast_equiv_defn_with_contracts() {
        assert_ast_equivalent(
            "defn f(x: Int) -> Int\n    [x > 0][true]\n    term x\n",
            "defn f(x: Int) -> Int\n    [x > 0][true]\n{\n    term x;\n};",
        );
    }

    #[test]
    fn ast_equiv_enum() {
        assert_ast_equivalent(
            "enum Color\n    Red\n    Green\n    Blue\n",
            "enum Color {\n    Red,\n    Green,\n    Blue,\n}",
        );
    }

    #[test]
    fn ast_equiv_match_expression() {
        assert_ast_equivalent(
            "defn classify(n: Int) -> Int\n    term match n\n        _ when n < 0 => -1\n        0 => 0\n        _ => 1\n",
            "defn classify(n: Int) -> Int {\n    term match n { _ when n < 0 => -1, 0 => 0, _ => 1 };\n};",
        );
    }

    #[test]
    fn ast_equiv_continuations() {
        assert_ast_equivalent(
            "let x = foo(\n    a,\n    b\n)\nlet y = bar\n    .baz()\n",
            "let x = foo(a, b);\nlet y = bar.baz();",
        );
    }
}
