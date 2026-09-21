// ── .bad (Briev Assembly Dialect) parser ──────────────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-09-21 (bad-dialect plan): standalone line-oriented parser for .bad
// programs. NOT woven into the .bv lexer/parser pipeline — .bad is a
// separate dialect with a line-shaped grammar, so this module scans lines
// directly and classifies them by token shape (plan §Grammar). No
// indentation sensitivity, no braces: a label or defn owns every
// instruction line that follows it until the next top-level line.
//
// Disambiguation is pure token shape:
//   `ident: ...`            → label (code) or data line (if `.dir` follows)
//   `defn ident params`     → defn head
//   `section/global/.x ...` → top-level directive
//   `alias x = r0`          → register alias
//   `target => ...`         → exception/branch row (attaches to previous
//                             instruction, or forms branch-defn rows)
//   `[preds]`               → inline contract for the NEXT instruction
//   anything else           → instruction owned by the nearest label/defn
//
// To undo: remove this file and src/ast/bad.rs, revert the ast/mod.rs
// `pub mod bad;` line, and drop the backend (src/backend/bad/).

use crate::ast::bad::*;
use crate::errors::Span;

/// Parse failure with a house-style what/why/fix message.
#[derive(Debug, Clone)]
pub struct BadParseError {
    pub message: String,
    pub line: usize,
    pub span: Span,
}

impl std::fmt::Display for BadParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (line {})", self.message, self.line)
    }
}

/// Parse a whole .bad source file.
pub fn parse_bad(source: &str) -> Result<BadProgram, BadParseError> {
    Parser::new(source).parse()
}

struct Parser<'s> {
    /// (byte offset, trimmed content, 1-based line number)
    lines: Vec<(usize, &'s str, usize)>,
    pos: usize,
    items: Vec<BadTopLevel>,
    /// Inline contract waiting for the next instruction line.
    pending_contract: Option<BadContract>,
}

/// Identification of the line being pushed — bundles the diagnostic
/// triple so the push helpers stay at ≤4 params.
struct LineCtx<'a> {
    content: &'a str,
    line: usize,
    span: Span,
}

/// Who currently owns instruction lines.
enum Owner {
    Label(usize),
    Defn { idx: usize, branch_rows: bool, seq_body: bool },
}

impl<'s> Parser<'s> {
    fn new(source: &'s str) -> Self {
        let mut lines = Vec::new();
        let mut offset = 0;
        for (i, raw) in source.lines().enumerate() {
            let content = strip_comment(raw).trim();
            if !content.is_empty() {
                lines.push((offset, content, i + 1));
            }
            offset += raw.len() + 1;
        }
        Parser { lines, pos: 0, items: Vec::new(), pending_contract: None }
    }

    fn parse(mut self) -> Result<BadProgram, BadParseError> {
        let mut owner: Option<Owner> = None;
        while self.pos < self.lines.len() {
            let (off, content, line) = self.lines[self.pos];
            let span = Span::new(off, off + content.len(), line, 0);

            // Local label `.name:` — nests into the owning label's body
            // (directives never carry a colon, so the colon keeps the
            // token-shape disambiguation honest).
            if content.starts_with('.') {
                if let Some((name, rest)) = split_label(content) {
                    if is_ident(name) && rest.trim().is_empty() {
                        let local = BadLocal { name: name[1..].to_string(), span };
                        match owner.as_mut() {
                            Some(Owner::Label(i)) => {
                                if let Some(BadTopLevel::Label(l)) = self.items.get_mut(*i) {
                                    l.body.push(BadBodyItem::Local(local));
                                }
                                self.pos += 1;
                                continue;
                            }
                            _ => {
                                return Err(BadParseError {
                                    message: format!(
                                        "local label `{content}` is outside any label - \
                                         local labels must follow a global label"
                                    ),
                                    line,
                                    span,
                                });
                            }
                        }
                    }
                }
            }

            // Top-level shapes always close the current owner.
            if let Some(item) = self.try_top_level(content, line, span)? {
                match &item {
                    BadTopLevel::Label(_) => {
                        owner = Some(Owner::Label(self.items.len()));
                    }
                    BadTopLevel::Defn(_) => {
                        owner = Some(Owner::Defn {
                            idx: self.items.len(),
                            branch_rows: false,
                            seq_body: false,
                        });
                    }
                    _ => owner = None,
                }
                self.items.push(item);
                self.pos += 1;
                continue;
            }

            // `target => ...` / `default => ...`
            if let Some(branch) = self.try_branch_row(content, line, span)? {
                let owner = owner.as_mut().ok_or_else(|| BadParseError {
                    message: format!(
                        "`{content}` is a target exception row but no instruction \
                         precedes it - exceptions replace the nearest preceding \
                         instruction, or form a defn's branch table"
                    ),
                    line,
                    span,
                })?;
                let lctx = LineCtx { content, line, span };
                self.push_branch_row(owner, branch, &lctx)?;
                self.pos += 1;
                continue;
            }

            // Inline contract line `[preds]`.
            if content.starts_with('[') {
                let contract = self.parse_contract_line(content, line, span)?;
                self.pending_contract = Some(contract);
                self.pos += 1;
                continue;
            }

            // Instruction line — must have an owner.
            let instr = self.parse_instr_line(content, line, span)?;
            let owner = owner.as_mut().ok_or_else(|| BadParseError {
                message: format!(
                    "instruction `{content}` has no owner - every instruction \
                     belongs to a label or defn declared above it"
                ),
                line,
                span,
            })?;
                let lctx = LineCtx { content, line, span };
                self.push_instruction(owner, instr, &lctx)?;
            self.pos += 1;
        }
        let end = self.lines.last().map(|(o, c, _)| o + c.len()).unwrap_or(0);
        Ok(BadProgram { items: self.items, span: Span::new(0, end, 0, 0) })
    }


    /// Route a `target => ...` row to its owner (defn table or last
    /// instruction of a label).
    fn push_branch_row(
        &mut self, owner: &mut Owner, branch: BadBranch, lc: &LineCtx,
    ) -> Result<(), BadParseError> {
        let (content, line, span) = (lc.content, lc.line, lc.span);
        match owner {
            Owner::Defn { branch_rows, seq_body, .. } => {
                if *seq_body {
                    return Err(BadParseError {
                        message: format!(
                            "defn mixes sequence body lines with branch rows - \
                             pick one shape: instruction lines are the default body, \
                             `target => ...` rows are the target table ('{content}')"
                        ),
                        line,
                        span,
                    });
                }
                *branch_rows = true;
                if let Some(BadTopLevel::Defn(d)) = self.items.last_mut() {
                    if let BadDefnShape::Branch(rows) = &mut d.shape {
                        rows.push(branch);
                    }
                }
            }
            Owner::Label(i) => {
                let attached = match self.items.get_mut(*i) {
                    Some(BadTopLevel::Label(l)) => l.body.last_mut().and_then(|last| {
                        match last {
                            BadBodyItem::Instr(i) => {
                                i.exceptions.push(branch);
                                Some(())
                            }
                            BadBodyItem::Local(_) => None,
                        }
                    }),
                    _ => None,
                };
                if attached.is_none() {
                    return Err(BadParseError {
                        message: format!(
                            "`{content}` follows a label with no instruction - \
                             exceptions replace the nearest preceding instruction"
                        ),
                        line,
                        span,
                    });
                }
            }
        }
        Ok(())
    }

    /// Route an instruction line to its owner (label body or defn
    /// sequence body).
    fn push_instruction(
        &mut self, owner: &mut Owner, mut instr: BadInstr, lc: &LineCtx,
    ) -> Result<(), BadParseError> {
        let (content, line, span) = (lc.content, lc.line, lc.span);
        match owner {
            Owner::Label(i) => {
                if let Some(BadTopLevel::Label(l)) = self.items.get_mut(*i) {
                    instr.contract = self.pending_contract.take();
                    l.body.push(BadBodyItem::Instr(instr));
                }
            }
            Owner::Defn { seq_body, branch_rows, .. } => {
                if *branch_rows {
                    return Err(BadParseError {
                        message: format!(
                            "defn mixes branch rows with sequence body lines - \
                             a branch defn contains only `default => ...` / \
                             `target => ...` rows ('{content}')"
                        ),
                        line,
                        span,
                    });
                }
                *seq_body = true;
                if let Some(BadTopLevel::Defn(d)) = self.items.last_mut() {
                    // First sequence line converts the provisional
                    // Branch shape (see parse_defn) to Sequence.
                    if matches!(d.shape, BadDefnShape::Branch(_)) {
                        d.shape = BadDefnShape::Sequence(Vec::new());
                    }
                    if let BadDefnShape::Sequence(body) = &mut d.shape {
                        instr.contract = self.pending_contract.take();
                        body.push(instr);
                    }
                }
            }
        }
        Ok(())
    }

    /// Try the top-level shapes; `Ok(None)` = not top-level.
    fn try_top_level(
        &mut self, content: &str, line: usize, span: Span,
    ) -> Result<Option<BadTopLevel>, BadParseError> {
        // `defn name params`
        if let Some(rest) = content.strip_prefix("defn ") {
            return Ok(Some(self.parse_defn(rest, line, span)?));
        }
        // `alias x = r0`
        if let Some(rest) = content.strip_prefix("alias ") {
            return Ok(Some(self.parse_alias(rest, line, span)?));
        }
        // `section X` / `global X` / `export name`
        if let Some(rest) = content.strip_prefix("section ")
            .or_else(|| content.strip_prefix("global "))
            .or_else(|| content.strip_prefix("export "))
        {
            let name = content.split_whitespace().next().unwrap_or("").to_string();
            return Ok(Some(BadTopLevel::Directive(BadDirective {
                name,
                args: rest.trim().to_string(),
                span,
            })));
        }
        // `.directive args`
        if content.starts_with('.') {
            let (name, args) = split_ws(content);
            return Ok(Some(BadTopLevel::Directive(BadDirective {
                name: name.to_string(),
                args: args.to_string(),
                span,
            })));
        }
        // `name: ...` — data line or code label.
        if let Some((name, rest)) = split_label(content) {
            return self.parse_name_colon(name.trim(), rest.trim(), line, span);
        }
        Ok(None)
    }

    /// The `name: ...` tail: data line (`.dir` follows) or code label
    /// with optional trailing contracts (`_start: [post: ...]`).
    fn parse_name_colon(
        &mut self, name: &str, rest: &str, line: usize, span: Span,
    ) -> Result<Option<BadTopLevel>, BadParseError> {
        validate_ident(name, line, span)?;
        if rest.starts_with('.') || rest.starts_with("section ") || rest.starts_with("global ") {
            return Ok(Some(self.parse_data_label(name, rest, span)?));
        }
        let contracts = if rest.starts_with('[') {
            self.parse_contract_list(rest, line, span)?
        } else {
            Vec::new()
        };
        Ok(Some(BadTopLevel::Label(BadLabel {
            name: name.to_string(),
            local: false,
            contracts,
            body: Vec::new(),
            span,
        })))
    }

    fn parse_alias(
        &mut self, rest: &str, line: usize, span: Span,
    ) -> Result<BadTopLevel, BadParseError> {
        let (name, reg) = rest.split_once('=').ok_or_else(|| BadParseError {
            message: format!("alias needs `alias name = register` form ('alias {rest}')"),
            line,
            span,
        })?;
        let (name, reg) = (name.trim().to_string(), reg.trim().to_string());
        validate_ident(&name, line, span)?;
        Ok(BadTopLevel::Alias(BadAlias { name, register: reg, span }))
    }

    fn parse_data_label(
        &mut self, name: &str, rest: &str, span: Span,
    ) -> Result<BadTopLevel, BadParseError> {
        // Data: `msg: .asciz "hi"` — label + directive pair.
        let (dname, dargs) = split_ws(rest);
        Ok(BadTopLevel::Data(BadDataLabel {
            name: name.to_string(),
            directive: BadDirective { name: dname.to_string(), args: dargs.to_string(), span },
            span,
        }))
    }

    fn parse_defn(
        &mut self, rest: &str, _line: usize, span: Span,
    ) -> Result<BadTopLevel, BadParseError> {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").trim().to_string();
        let params = parts
            .next()
            .map(|p| {
                p.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Ok(BadTopLevel::Defn(BadDefn {
            name,
            params,
            // Shape resolves as lines arrive: branch rows → Branch, else Sequence.
            shape: BadDefnShape::Branch(Vec::new()),
            span,
        }))
    }

    /// `x86_64 => instr; instr` / `default => ...`
    fn try_branch_row(
        &mut self, content: &str, line: usize, span: Span,
    ) -> Result<Option<BadBranch>, BadParseError> {
        let Some((target, rest)) = content.split_once("=>") else {
            return Ok(None);
        };
        let target = target.trim();
        let is_default = target == "default";
        if !is_default {
            validate_ident(target, line, span)?;
        }
        let mut body = Vec::new();
        for piece in split_semicolons(rest) {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let (off, _, _) = self.lines[self.pos];
            body.push(self.parse_instr_text(piece, off, line)?);
        }
        Ok(Some(BadBranch { target: target.to_string(), is_default, body, span }))
    }

    /// `mnemonic ops` — a plain instruction line.
    fn parse_instr_line(
        &mut self, content: &str, line: usize, span: Span,
    ) -> Result<BadInstr, BadParseError> {
        let (off, _, _) = self.lines[self.pos];
        let mut instr = self.parse_instr_text(content, off, line)?;
        instr.span = span;
        Ok(instr)
    }

    fn parse_instr_text(
        &self, text: &str, off: usize, line: usize,
    ) -> Result<BadInstr, BadParseError> {
        let (mnemonic, rest) = split_ws(text);
        let mut operands = Vec::new();
        for piece in split_operands(rest) {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            if let Ok(n) = piece.parse::<i64>() {
                operands.push(BadOperand::Int(n));
            } else if is_ident(piece) || piece.starts_with('[') || piece.starts_with('(') {
                // Name or raw operand text (memory refs like `[sp, #-16]!`):
                // resolved at lowering — params first, then the register
                // table, with identifier-boundary replacement inside raw
                // operand text.
                operands.push(BadOperand::Name(piece.to_string()));
            } else {
                // Arithmetic shape (`addr + 8`, `MAX * 4 - 1`) — evaluated
                // at lowering through the comptime pass.
                operands.push(BadOperand::Expr(piece.to_string()));
            }
        }
        Ok(BadInstr {
            mnemonic: mnemonic.to_string(),
            operands,
            contract: None,
            exceptions: Vec::new(),
            span: Span::new(off, off + text.len(), line, 0),
        })
    }

    fn parse_contract_line(
        &mut self, content: &str, line: usize, span: Span,
    ) -> Result<BadContract, BadParseError> {
        let inner = content
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .ok_or_else(|| BadParseError {
                message: format!("inline contract needs `[expr]` form ('{content}')"),
                line,
                span,
            })?;
        Ok(BadContract { preds: parse_preds(inner, line, span)?, span })
    }

    /// `[pre: ...] [post: ...]` trailing a label line.
    fn parse_contract_list(
        &mut self, content: &str, line: usize, span: Span,
    ) -> Result<Vec<BadContract>, BadParseError> {
        let mut out = Vec::new();
        let mut rest = content.trim();
        while let Some((start, end_rel)) = next_group(rest) {
            let inner = rest[start + 1..start + 1 + end_rel].trim();
            out.push(BadContract { preds: parse_pred_group(inner, line, span)?, span });
            rest = rest[start + 1 + end_rel + 1..].trim();
        }
        Ok(out)
    }
}

// ── small scanners ─────────────────────────────────────────────────────

/// Strip a `//` comment, respecting string literals.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'/' if !in_str && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                return &line[..i];
            }
            _ => {}
        }
        i += 1;
    }
    line
}

fn split_ws(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
}

/// `name: rest` → Some((name, rest)); requires the colon to be the first
/// `:` and the name to be an identifier.
fn split_label(s: &str) -> Option<(&str, &str)> {
    let i = s.find(':')?;
    let name = &s[..i];
    if name.is_empty() || !name.chars().all(is_ident_char) {
        return None;
    }
    Some((name, &s[i + 1..]))
}

/// Split on `;` respecting quotes and brackets.
fn split_semicolons(s: &str) -> Vec<&str> {
    split_top(s, b';')
}

/// Split on commas respecting quotes, `[...]`, `(...)`.
fn split_operands(s: &str) -> Vec<&str> {
    split_top(s, b',')
}

fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_str = !in_str,
            b'[' | b'(' if !in_str => depth += 1,
            b']' | b')' if !in_str => depth -= 1,
            _ if !in_str && depth == 0 && b == sep => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().map(|c| c.is_alphabetic() || c == '_' || c == '.').unwrap_or(false)
        && s.chars().all(is_ident_char)
}

fn validate_ident(s: &str, line: usize, span: Span) -> Result<(), BadParseError> {
    if is_ident(s) {
        Ok(())
    } else {
        Err(BadParseError {
            message: format!("`{s}` is not a valid identifier"),
            line,
            span,
        })
    }
}

// ── contract predicate parsing ────────────────────────────────────────

/// `r0 == 0 && sp % 16 == 0` / `r0 preserved` / `r0 valid`.
fn parse_preds(s: &str, line: usize, span: Span) -> Result<Vec<BadContractPred>, BadParseError> {
    let mut preds = Vec::new();
    for part in s.split("&&") {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        preds.push(parse_one_pred(part, line, span)?);
    }
    if preds.is_empty() {
        return Err(BadParseError {
            message: format!("contract `[{s}]` has no predicates"),
            line,
            span,
        });
    }
    Ok(preds)
}

fn parse_one_pred(s: &str, line: usize, span: Span) -> Result<BadContractPred, BadParseError> {
    let mut toks = s.split_whitespace();
    let lhs = toks.next().unwrap_or("").to_string();
    if lhs.is_empty() {
        return Err(BadParseError {
            message: format!("contract predicate `{s}` is empty"),
            line,
            span,
        });
    }
    match toks.next() {
        Some("preserved") => return Ok(BadContractPred::Preserved(lhs)),
        Some("valid") => return Ok(BadContractPred::Valid(lhs)),
        _ => {}
    }
    // Chained compares: `r0 % 16 == 0`
    let mut ops = Vec::new();
    let mut tail = s.splitn(2, char::is_whitespace).nth(1).unwrap_or("").trim();
    while let Some((op, after)) = take_cmp_op(tail) {
        let num_part = after.trim_start();
        let num_end = num_part
            .find(|c: char| !(c.is_ascii_digit() || c == '-'))
            .unwrap_or(num_part.len());
        let val: i64 = num_part[..num_end].parse().map_err(|_| BadParseError {
            message: format!("contract predicate `{s}` needs a number after the operator"),
            line,
            span,
        })?;
        ops.push((op, val));
        tail = num_part[num_end..].trim_start();
        if tail.is_empty() {
            break;
        }
    }
    if ops.is_empty() {
        return Err(BadParseError {
            message: format!(
                "contract predicate `{s}` is not one of `name preserved`, `name valid`, \
                 or a comparison like `name % 16 == 0`"
            ),
            line,
            span,
        });
    }
    Ok(BadContractPred::Compare { lhs, ops })
}

fn take_cmp_op(s: &str) -> Option<(BadCmpOp, &str)> {
    for (tok, op) in [
        ("==", BadCmpOp::Eq),
        ("!=", BadCmpOp::Ne),
        ("<=", BadCmpOp::Le),
        (">=", BadCmpOp::Ge),
        ("<", BadCmpOp::Lt),
        (">", BadCmpOp::Gt),
        ("%", BadCmpOp::Mod),
    ] {
        if let Some(rest) = s.strip_prefix(tok) {
            return Some((op, rest));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> BadProgram {
        parse_bad(src).unwrap_or_else(|e| panic!("parse failed: {}", e))
    }

    #[test]
    fn parses_label_instructions_and_data() {
        let p = parse_ok(
            "section .text\nglobal _start\n\n_start:\n    mov r0, 1\n    syscall\n\n\
             section .data\nmsg: .asciz \"hi\\n\"\n",
        );
        assert_eq!(p.items.len(), 5);
        assert!(matches!(&p.items[0], BadTopLevel::Directive(d) if d.name == "section"));
        assert!(matches!(&p.items[1], BadTopLevel::Directive(d) if d.name == "global"));
        let BadTopLevel::Label(l) = &p.items[2] else { panic!("expected label") };
        assert_eq!(l.name, "_start");
        assert_eq!(l.body.len(), 2);
        let BadBodyItem::Instr(first) = &l.body[0] else { panic!("expected instr") };
        assert_eq!(first.mnemonic, "mov");
        assert_eq!(first.operands, vec![BadOperand::Name("r0".into()), BadOperand::Int(1)]);
        let BadTopLevel::Data(d) = &p.items[4] else { panic!("expected data") };
        assert_eq!(d.name, "msg");
        assert_eq!(d.directive.name, ".asciz");
    }

    #[test]
    fn parses_sequence_defn_and_branch_defn() {
        let p = parse_ok(
            "defn push2 a, b\n    push a\n    push b\n\ndefn store_pair x, addr\n    \
             default => store x, addr; store x, addr + 8\n    \
             x86_64 => movq [addr], x\n",
        );
        let BadTopLevel::Defn(d1) = &p.items[0] else { panic!("expected defn") };
        assert_eq!(d1.name, "push2");
        assert_eq!(d1.params, vec!["a", "b"]);
        assert!(matches!(&d1.shape, BadDefnShape::Sequence(b) if b.len() == 2));
        let BadTopLevel::Defn(d2) = &p.items[1] else { panic!("expected defn") };
        let BadDefnShape::Branch(rows) = &d2.shape else { panic!("expected branch shape") };
        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_default);
        assert_eq!(rows[0].body.len(), 2, "semicolons split into two instructions");
        assert_eq!(rows[1].target, "x86_64");
    }

    #[test]
    fn parses_inline_exception_and_contract() {
        let p = parse_ok(
            "_start:\n    [sp % 16 == 0]\n    add r0, r0, 1\n    \
             x86_64 => lea r0, [r1 + 1]\n    ret\n",
        );
        let BadTopLevel::Label(l) = &p.items[0] else { panic!("expected label") };
        assert_eq!(l.body.len(), 2);
        let BadBodyItem::Instr(add) = &l.body[0] else { panic!("expected instr") };
        assert!(add.contract.is_some(), "inline contract attaches to next instruction");
        assert_eq!(add.exceptions.len(), 1);
        assert_eq!(add.exceptions[0].target, "x86_64");
        assert_eq!(add.exceptions[0].body[0].mnemonic, "lea");
        let BadBodyItem::Instr(ret_i) = &l.body[1] else { panic!("expected instr") };
        assert_eq!(ret_i.mnemonic, "ret");
        assert!(ret_i.exceptions.is_empty(), "exception binds to the add, not ret");
    }

    #[test]
    fn parses_label_contracts() {
        let p = parse_ok("_start: [pre: r0 valid] [post: r10 preserved]\n    ret\n");
        let BadTopLevel::Label(l) = &p.items[0] else { panic!("expected label") };
        assert_eq!(l.contracts.len(), 2);
        assert!(matches!(&l.contracts[0].preds[0], BadContractPred::Valid(r) if r == "r0"));
        assert!(matches!(&l.contracts[1].preds[0], BadContractPred::Preserved(r) if r == "r10"));
    }

    #[test]
    fn comment_respects_strings() {
        let p = parse_ok("d: .asciz \"a//b\" // trailing\n");
        let BadTopLevel::Data(d) = &p.items[0] else { panic!("expected data") };
        assert_eq!(d.directive.args, "\"a//b\"");
    }

    #[test]
    fn errors_on_orphan_exception() {
        let err = parse_bad("x86_64 => lea r0, [r1]\n").unwrap_err();
        assert!(err.message.contains("no instruction precedes it"), "{}", err.message);
    }

    #[test]
    fn errors_on_ownerless_instruction() {
        let err = parse_bad("mov r0, 1\n").unwrap_err();
        assert!(err.message.contains("no owner"), "{}", err.message);
    }

    #[test]
    fn errors_on_defn_shape_mix() {
        let err = parse_bad("defn f a\n    push a\n    x86_64 => pop a\n").unwrap_err();
        assert!(err.message.contains("mixes sequence body"), "{}", err.message);
    }

    #[test]
    fn errors_on_exception_after_empty_label() {
        let err = parse_bad("l:\n    x86_64 => nop\n").unwrap_err();
        assert!(err.message.contains("no instruction"), "{}", err.message);
    }
}

/// `..[...]..` → (start of `[`, relative end of `]`).
fn next_group(s: &str) -> Option<(usize, usize)> {
    let start = s.find('[')?;
    let end_rel = s[start + 1..].find(']')?;
    Some((start, end_rel))
}

/// `pre: r0 valid` / `post: ...` / bare preds → one contract's preds.
fn parse_pred_group(
    inner: &str, line: usize, span: Span,
) -> Result<Vec<BadContractPred>, BadParseError> {
    if let Some(body) = inner.strip_prefix("pre:") {
        parse_preds(body.trim(), line, span)
    } else if let Some(body) = inner.strip_prefix("post:") {
        parse_preds(body.trim(), line, span)
    } else if let Some(body) = inner.strip_prefix("frame:") {
        let n: i64 = body.trim().parse().map_err(|_| BadParseError {
            message: format!(
                "frame contract needs a byte bound: `[frame: 32]` ('{inner}')"
            ),
            line,
            span,
        })?;
        Ok(vec![BadContractPred::Frame(n)])
    } else {
        parse_preds(inner, line, span)
    }
}
