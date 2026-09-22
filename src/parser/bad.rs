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
            if content.starts_with('.') && is_local_label_shape(content) {
                match owner.as_ref() {
                    Some(Owner::Label(i)) => {
                        self.append_local(*i, content, span);
                    }
                    Some(Owner::Defn { idx, branch_rows, .. }) => {
                        self.append_defn_local(*idx, *branch_rows, content, span)?;
                    }
                    _ => {
                        return Err(BadParseError {
                            message: format!(
                                "local label `{content}` is outside any label - \
                                 local labels must follow a global label or sit inside \
                                 a defn body"
                            ),
                            line,
                            span,
                        });
                    }
                }
                self.pos += 1;
                continue;
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
            let instrs = self.parse_instr_line(content, line, span)?;
            let owner = owner.as_mut().ok_or_else(|| BadParseError {
                message: format!(
                    "instruction `{content}` has no owner - every instruction \
                     belongs to a label or defn declared above it"
                ),
                line,
                span,
            })?;
            let lctx = LineCtx { content, line, span };
            self.push_instruction(owner, instrs, &lctx)?;
            self.pos += 1;
        }
        let end = self.lines.last().map(|(o, c, _)| o + c.len()).unwrap_or(0);
        Ok(BadProgram { items: self.items, span: Span::new(0, end, 0, 0) })
    }


    fn append_local(&mut self, idx: usize, content: &str, span: Span) {
        let name = split_label(content).map(|(n, _)| n[1..].to_string()).unwrap_or_default();
        if let Some(BadTopLevel::Label(l)) = self.items.get_mut(idx) {
            l.body.push(BadBodyItem::Local(BadLocal { name, span }));
        }
    }

    /// `.name:` inside a defn body — hygienically renamed per expansion
    /// at lowering. Illegal in branch shapes (rows are one line each).
    fn defn_name_hint(&self) -> String {
        match self.items.last() {
            Some(BadTopLevel::Defn(d)) => d.name.clone(),
            _ => "?".to_string(),
        }
    }

    fn append_defn_local(
        &mut self, idx: usize, branch_rows: bool, content: &str, span: Span,
    ) -> Result<(), BadParseError> {
        let name = split_label(content).map(|(n, _)| n[1..].to_string()).unwrap_or_default();
        if let Some(BadTopLevel::Defn(d)) = self.items.get_mut(idx) {
            match &mut d.shape {
                BadDefnShape::Sequence(items) => {
                    items.push(BadBodyItem::Local(BadLocal { name, span }));
                    return Ok(());
                }
                BadDefnShape::Branch(rows) => {
                    // A provisional Branch shape with no rows yet is an
                    // unwritten sequence (the local came first) — convert
                    // it; real branch rows make locals an error.
                    if rows.is_empty() && !branch_rows {
                        d.shape = BadDefnShape::Sequence(vec![BadBodyItem::Local(BadLocal {
                            name,
                            span,
                        })]);
                        return Ok(());
                    }
                    return Err(BadParseError {
                        message: format!(
                            "local label `{content}` inside a branch defn - branch defns \
                             contain only `default => ...` / `target => ...` rows"
                        ),
                        line: span.line,
                        span,
                    });
                }
            }
        }
        Ok(())
    }

    /// Route a `target => ...` row to its owner (defn table or last
    /// instruction of a label).
    fn push_branch_row(
        &mut self, owner: &mut Owner, branch: BadBranch, lc: &LineCtx,
    ) -> Result<(), BadParseError> {
        let (content, line, span) = (lc.content, lc.line, lc.span);
        match owner {
            Owner::Defn { branch_rows, seq_body, .. } => {
                // After sequence lines, a `target =>` row is a
                // PER-INSTRUCTION exception (same as in label bodies) —
                // not a branch row. `default =>` here is a category
                // error: the body IS the default.
                if *seq_body {
                    if branch.is_default {
                        return Err(BadParseError {
                            message: format!(
                                "`default =>` rows belong in branch defns - the sequence \
                                 body of `{}` already lowers universally",
                                self.defn_name_hint()
                            ),
                            line,
                            span,
                        });
                    }
                    if let Some(BadTopLevel::Defn(d)) = self.items.last_mut() {
                        if let BadDefnShape::Sequence(items) = &mut d.shape {
                            match items.last_mut() {
                                Some(BadBodyItem::Instr(last)) => {
                                    last.exceptions.push(branch);
                                }
                                _ => {
                                    return Err(BadParseError {
                                        message: format!(
                                            "`{content}` follows a local label - exceptions \
                                             replace the nearest preceding instruction"
                                        ),
                                        line,
                                        span,
                                    });
                                }
                            }
                        }
                    }
                    return Ok(());
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
        &mut self, owner: &mut Owner, instrs: Vec<BadInstr>, lc: &LineCtx,
    ) -> Result<(), BadParseError> {
        match owner {
            Owner::Label(i) => {
                if let Some(BadTopLevel::Label(l)) = self.items.get_mut(*i) {
                    push_with_contract(instrs, &mut self.pending_contract, |i| {
                        l.body.push(BadBodyItem::Instr(i))
                    });
                }
            }
            Owner::Defn { seq_body, branch_rows, .. } => {
                if *branch_rows {
                    return Err(BadParseError {
                        message: format!(
                            "defn mixes branch rows with sequence body lines - \
                             a branch defn contains only `default => ...` / \
                             `target => ...` rows ('{}')",
                            lc.content
                        ),
                        line: lc.line,
                        span: lc.span,
                    });
                }
                *seq_body = true;
                let mut defn = match self.items.last_mut() {
                    Some(BadTopLevel::Defn(d)) => d.clone(),
                    _ => return Ok(()),
                };
                // First sequence line converts the provisional Branch
                // shape (see parse_defn) to Sequence.
                if matches!(defn.shape, BadDefnShape::Branch(_)) {
                    defn.shape = BadDefnShape::Sequence(Vec::new());
                }
                if let BadDefnShape::Sequence(body) = &mut defn.shape {
                    push_with_contract(instrs, &mut self.pending_contract, |i| {
                        body.push(BadBodyItem::Instr(i))
                    });
                }
                if let Some(BadTopLevel::Defn(d)) = self.items.last_mut() {
                    d.shape = defn.shape;
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
        // `section X` / `global X` / `export name` / `import "path"`
        if let Some(rest) = content.strip_prefix("section ")
            .or_else(|| content.strip_prefix("global "))
            .or_else(|| content.strip_prefix("export "))
            .or_else(|| content.strip_prefix("import "))
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
        let mut line_ack: Option<Ack> = None;
        for piece in split_semicolons(rest) {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let (off, _, _) = self.lines[self.pos];
            let (seg_ack, seg) = parse_ack_prefix(piece, off, line, span.clone())?;
            let ack = seg_ack.or_else(|| line_ack.clone());
            let mut instr = self.parse_instr_text(seg, off, line)?;
            if let Some(a) = &ack {
                if matches!(a.scope, AckScope::Line | AckScope::Override) {
                    line_ack = Some(a.clone());
                }
            }
            instr.ack = ack;
            body.push(instr);
        }
        Ok(Some(BadBranch { target: target.to_string(), is_default, body, span }))
    }

    /// `mnemonic ops` — a plain instruction line. A leading `^` / `^^` /
    /// `^^^` acknowledge prefix (optionally followed by a keyword tail
    /// and/or `W<n>` warning names) is stripped before parsing.
    fn parse_instr_line(
        &mut self, content: &str, line: usize, span: Span,
    ) -> Result<Vec<BadInstr>, BadParseError> {
        let (off, _, _) = self.lines[self.pos];
        let mut out = Vec::new();
        // A `^^` / `^^^` prefix scopes to the WHOLE line: the ack parsed
        // on the first segment propagates to every `;`-separated instr.
        let mut line_ack: Option<Ack> = None;
        for piece in split_semicolons(content) {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let (seg_ack, seg) = parse_ack_prefix(piece, off, line, span.clone())?;
            // The first segment's `^` is the line's acknowledge marker;
            // `^^`/`^^^` propagate to the remaining segments.
            let ack = seg_ack.or_else(|| line_ack.clone());
            let mut instr = self.parse_instr_text(seg, off, line)?;
            if let Some(a) = &ack {
                if matches!(a.scope, AckScope::Line | AckScope::Override) {
                    line_ack = Some(a.clone());
                }
            }
            instr.ack = ack;
            instr.span = span.clone();
            out.push(instr);
        }
        if out.is_empty() {
            return Err(BadParseError {
                message: format!("instruction line `{content}` contains no instruction"),
                line,
                span,
            });
        }
        Ok(out)
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
            operands.push(classify_operand(piece, line, off)?);
        }
        Ok(BadInstr {
            mnemonic: mnemonic.to_string(),
            operands,
            contract: None,
            exceptions: Vec::new(),
            ack: None,
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
    /// Positional label contracts: one group = postcondition (implied),
    /// two groups = precondition then postcondition. `[frame: N]` keeps
    /// its keyword (a different proof kind). No `pre:`/`post:` keywords —
    /// position is the whole story.
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
        match out.len() {
            1 => Ok(out),                            // single = post
            2 => Ok(out),                            // [pre, post] — order is the meaning
            0 => Ok(out),
            n => Err(BadParseError {
                message: format!(
                    "{n} contract groups on one label - a label takes at most two: \
                     `[pre] [post]` (a single group is the postcondition)"
                ),
                line,
                span,
            }),
        }
    }
}

// ── small scanners ─────────────────────────────────────────────────────

/// `.name:` shape test — ident after the dot, nothing after the colon
/// (directives never carry a colon, so the colon keeps the token-shape
/// disambiguation honest).
fn is_local_label_shape(content: &str) -> bool {
    match split_label(content) {
        Some((name, rest)) => is_ident(name) && rest.trim().is_empty(),
        None => false,
    }
}

/// Push a `;`-packed instruction run; the pending contract binds to the
/// FIRST of the run (exceptions from later lines attach to the LAST).
fn push_with_contract(
    instrs: Vec<BadInstr>, pending: &mut Option<BadContract>,
    mut push: impl FnMut(BadInstr),
) {
    let mut first = true;
    for mut instr in instrs {
        if first {
            instr.contract = pending.take();
            first = false;
        }
        push(instr);
    }
}

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

/// Strip a leading acknowledge prefix from an instruction segment.
///
/// `^ mov r5, 1` / `^^ mov r5, 1` / `^^^ mov r5, 1` — the caret count is
/// the scope (Instr / Line / Override). Optionally followed by a keyword
/// tail (`^ack`, the future expansion slot) and/or explicit `W<n>` warning
/// names (`^ W1 mov ...`, `^ack W1 mov ...`). Returns `(None, s)` when no
/// prefix is present.
fn parse_ack_prefix(
    s: &str, off: usize, line: usize, span: Span,
) -> Result<(Option<Ack>, &str), BadParseError> {
    let Some(scope) = parse_ack_scope(s, line, span)? else {
        return Ok((None, s));
    };
    let rest = s[carets_of(s)..].trim_start();
    // Optional keyword tail (`ack`, future `seq`/`vol`/...).
    let mut rest = rest;
    if let Some((kw, tail)) = rest.split_once(char::is_whitespace) {
        if kw == "ack" {
            rest = tail.trim_start();
        }
    }
    // Optional explicit warning names: `W1`, `W2`, ...
    let (warnings, rest) = parse_ack_warnings(rest);
    if warnings.is_empty() {
        if rest.is_empty() {
            return Err(BadParseError {
                message: format!(
                    "acknowledge prefix `{}` is not followed by an instruction",
                    &s[..carets_of(s).min(8)]
                ),
                line,
                span,
            });
        }
        return Ok((Some(Ack { scope, warnings: Vec::new(), span }), rest));
    }
    if rest.is_empty() {
        return Err(BadParseError {
            message: format!(
                "acknowledge prefix `{}` names {} warning(s) but no instruction follows",
                &s[..carets_of(s).min(8)],
                warnings.len()
            ),
            line,
            span,
        });
    }
    Ok((Some(Ack { scope, warnings, span }), rest))
}

/// Count the leading carets of an ack prefix.
fn carets_of(s: &str) -> usize {
    s.chars().take_while(|&c| c == '^').count()
}

/// Parse the caret count → scope; `None` when there is no prefix.
fn parse_ack_scope(
    s: &str, line: usize, span: Span,
) -> Result<Option<AckScope>, BadParseError> {
    let carets = carets_of(s);
    if carets == 0 {
        return Ok(None);
    }
    let scope = match carets {
        1 => AckScope::Instr,
        2 => AckScope::Line,
        3 => AckScope::Override,
        _ => {
            return Err(BadParseError {
                message: format!(
                    "acknowledge prefix `{}` has {carets} carets - use `^` (this instruction), \
                     `^^` (whole line), or `^^^` (full override of predicted errors)",
                    &s[..carets.min(8)]
                ),
                line,
                span,
            });
        }
    };
    Ok(Some(scope))
}

/// Consume leading `W<n>` warning names (space-separated).
fn parse_ack_warnings(rest: &str) -> (Vec<String>, &str) {
    let mut warnings = Vec::new();
    let mut rest = rest;
    loop {
        let head = rest.split_whitespace().next().unwrap_or("");
        if head.len() >= 2 && head.starts_with('W') && head[1..].chars().all(|c| c.is_ascii_digit()) {
            warnings.push(head.to_string());
            rest = rest[head.len()..].trim_start();
        } else {
            break;
        }
    }
    (warnings, rest)
}

/// `1.5`, `3.14e-2`, `2E10` — a decimal float literal (validated by the
/// caller's f64 parse).
/// One operand token → its AST kind. Int; float literal; Name (or raw
/// memory-ref text like `[sp, #-16]!`, resolved at lowering); or an
/// arithmetic Expr (`addr + 8`, `MAX * 4`) for the comptime pass.
fn classify_operand(
    piece: &str, line: usize, off: usize,
) -> Result<BadOperand, BadParseError> {
    if let Ok(n) = piece.parse::<i64>() {
        return Ok(BadOperand::Int(n));
    }
    if is_float_literal(piece) {
        return piece.parse::<f64>().map(|_| BadOperand::Float(piece.to_string())).map_err(|_| {
            BadParseError {
                message: format!("float literal `{piece}` does not parse"),
                line,
                span: Span::new(off, off + piece.len(), line, 0),
            }
        });
    }
    if is_ident(piece) || piece.starts_with('[') || piece.starts_with('(') {
        return Ok(BadOperand::Name(piece.to_string()));
    }
    Ok(BadOperand::Expr(piece.to_string()))
}

fn is_float_literal(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || !(b[0].is_ascii_digit()) {
        return false;
    }
    s.contains('.') || s.contains('e') || s.contains('E')
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

    pub(crate) fn parse_ok(src: &str) -> BadProgram {
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
    fn parses_label_contracts_positionally() {
        // Two groups: first = pre, second = post. Keywords are gone.
        let p = parse_ok("_start: [r0 valid] [r10 preserved]\n    ret\n");
        let BadTopLevel::Label(l) = &p.items[0] else { panic!("expected label") };
        assert_eq!(l.contracts.len(), 2);
        assert!(matches!(&l.contracts[0].preds[0], BadContractPred::Valid(r) if r == "r0"));
        assert!(matches!(&l.contracts[1].preds[0], BadContractPred::Preserved(r) if r == "r10"));
    }

    #[test]
    fn single_group_is_the_postcondition_and_keywords_are_rejected() {
        let p = parse_ok("_start: [r0 valid]\n    ret\n");
        let BadTopLevel::Label(l) = &p.items[0] else { panic!("expected label") };
        assert_eq!(l.contracts.len(), 1, "single group = post");
        let err = parse_bad("_start: [post: r0 == 0]\n    ret\n").unwrap_err();
        assert!(err.message.contains("positional"), "{}", err.message);
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
    fn seq_defn_inline_exceptions_attach_to_last() {
        // A `target =>` row after sequence lines is a per-instruction
        // exception, NOT a shape mix (the design-session gap, fixed).
        let p = parse_ok("defn f a\n    push a\n    x86_64 => pop a\n");
        let BadTopLevel::Defn(d) = &p.items[0] else { panic!("defn") };
        let BadDefnShape::Sequence(body) = &d.shape else { panic!("sequence") };
        let BadBodyItem::Instr(push) = &body[0] else { panic!("push") };
        assert_eq!(push.exceptions.len(), 1, "exception binds to push");
    }

    #[test]
    fn default_row_after_sequence_body_is_a_category_error() {
        let err = parse_bad("defn f a\n    push a\n    default => pop a\n").unwrap_err();
        assert!(err.message.contains("belong in branch defns"), "{}", err.message);
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
    for kw in ["pre:", "post:"] {
        if inner.starts_with(kw) {
            return Err(BadParseError {
                message: format!(
                    "contract keywords are gone - contracts are positional: one group is \
                     the postcondition, two groups are `[pre] [post]` ('{inner}')"
                ),
                line,
                span,
            });
        }
    }
    if let Some(body) = inner.strip_prefix("frame:") {
        return parse_frame_group(body, line, span);
    }
    parse_preds(inner, line, span)
}

/// `[frame: 32]` — the byte bound.
fn parse_frame_group(
    body: &str, line: usize, span: Span,
) -> Result<Vec<BadContractPred>, BadParseError> {
    let n: i64 = body.trim().parse().map_err(|_| BadParseError {
        message: format!("frame contract needs a byte bound: `[frame: 32]` ('{body}')"),
        line,
        span,
    })?;
    Ok(vec![BadContractPred::Frame(n)])
}

#[cfg(test)]
mod semicolon_tests {
    use super::tests::parse_ok;
    use super::*;

    #[test]
    fn packed_lines_split_in_every_context() {
        // Label body, defn sequence body, and branch row all accept the
        // same packed line — one rule everywhere.
        let p = parse_ok(
            "t:\n    mov r0, 1; mov r1, 2; ret\n\ndefn f x\n    push x; pop x\n\n\
             defn g x\n    default => mov r0, x; ret\n",
        );
        let BadTopLevel::Label(t) = &p.items[0] else { panic!("label") };
        assert_eq!(t.body.len(), 3, "label body: packed line = 3 instructions");
        let BadTopLevel::Defn(d) = &p.items[1] else { panic!("defn") };
        let BadDefnShape::Sequence(body) = &d.shape else { panic!("sequence") };
        assert_eq!(body.len(), 2, "defn body: packed line = 2 instructions");
        let BadTopLevel::Defn(g) = &p.items[2] else { panic!("defn g") };
        let BadDefnShape::Branch(rows) = &g.shape else { panic!("branch shape") };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body.len(), 2, "branch-row packed line = 2 instructions");
    }

    #[test]
    fn contract_binds_first_exception_binds_last() {
        let p = parse_ok(
            "t:\n    [sp % 16 == 0]\n    push r0; call f\n    x86_64 => nop\n    \
             ret\n\ndefn f\n    ret\n",
        );
        let BadTopLevel::Label(l) = &p.items[0] else { panic!("label") };
        let BadBodyItem::Instr(push) = &l.body[0] else { panic!("push") };
        let BadBodyItem::Instr(call) = &l.body[1] else { panic!("call") };
        assert!(push.contract.is_some(), "contract binds to the FIRST packed");
        assert!(call.contract.is_none());
        assert_eq!(call.exceptions.len(), 1, "exception binds to the LAST");
    }

    #[test]
    fn ack_prefix_scopes_by_caret_count() {
        let p = parse_ok(
            "t:\n    ^ mov r5, 1\n    ^^ mov r5, 1; call f\n    ^^^ mov r5, 1\n    \
             ret\n",
        );
        let BadTopLevel::Label(l) = &p.items[0] else { panic!("label") };
        let BadBodyItem::Instr(single) = &l.body[0] else { panic!("single") };
        let ack = single.ack.as_ref().expect("^ parses to Instr ack");
        assert_eq!(ack.scope, AckScope::Instr);
        assert!(ack.warnings.is_empty());
        let BadBodyItem::Instr(packed) = &l.body[1] else { panic!("packed") };
        let ack = packed.ack.as_ref().expect("^^ parses to Line ack");
        assert_eq!(ack.scope, AckScope::Line);
        let BadBodyItem::Instr(call) = &l.body[2] else { panic!("call") };
        let ack = call.ack.as_ref().expect("^^ propagates across ; segments");
        assert_eq!(ack.scope, AckScope::Line);
        let BadBodyItem::Instr(override_) = &l.body[3] else { panic!("override") };
        let ack = override_.ack.as_ref().expect("^^^ parses to Override ack");
        assert_eq!(ack.scope, AckScope::Override);
    }

    #[test]
    fn ack_parses_keyword_tail_and_named_warnings() {
        let p = parse_ok(
            "t:\n    ^ack W1 mov r5, 1\n    ^ W2 W3 mov r6, 2\n    ^ W1 mov r7, 3\n",
        );
        let BadTopLevel::Label(l) = &p.items[0] else { panic!("label") };
        let BadBodyItem::Instr(kw) = &l.body[0] else { panic!("kw") };
        let ack = kw.ack.as_ref().expect("^ack parses");
        assert_eq!(ack.scope, AckScope::Instr);
        assert_eq!(ack.warnings, vec!["W1".to_string()]);
        let BadBodyItem::Instr(warns) = &l.body[1] else { panic!("warns") };
        let ack = warns.ack.as_ref().expect("W2 W3 parses");
        assert_eq!(ack.warnings, vec!["W2".to_string(), "W3".to_string()]);
        let BadBodyItem::Instr(w1) = &l.body[2] else { panic!("w1") };
        let ack = w1.ack.as_ref().expect("W1 parses");
        assert_eq!(ack.warnings, vec!["W1".to_string()]);
    }

    #[test]
    fn ack_prefix_errors_are_loud() {
        let err = parse_bad("t:\n    ^^^^ mov r5, 1\n").unwrap_err();
        assert!(err.message.contains("carets"), "{}", err.message);
        let err = parse_bad("t:\n    ^\n").unwrap_err();
        assert!(err.message.contains("not followed by an instruction"), "{}", err.message);
    }
}
