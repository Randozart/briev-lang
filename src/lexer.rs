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
//
// Runtime Exception for Use as a Language:
// When the Work or any Derivative Work thereof is used to generate code
// ("generated code"), such generated code shall not be subject to the
// terms of this License, provided that the generated code itself is not
// a Derivative Work of the Work. This exception does not apply to code
// that is itself a compiler, interpreter, or similar tool that incorporates
// or embeds the Work.

// 2026-07-12: Phase 0.1 — Full rewrite for new architecture.
// Changes from old lexer:
// - # is now a valid identifier character (Sqrt# -> single token)
// - Standalone # token removed — [ # ] uses Identifier("#")
// - inop/inop! removed (intrinsic architecture: all ops are # intrinsics)
// - Added When, Input, Output tokens
// - Export, ColonEq already existed (kept as-is)

use logos::Logos;

#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\n\r]+")]
#[logos(skip r"//[^\n]*")]
#[logos(skip r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/")]
pub enum Token {
    // ── Doc comments (defined before //-skip to win ties) ─────────────
    #[regex(r"///[^\n]*", |lex| lex.slice()[3..].to_string())]
    DocComment(String),
    #[regex(r"//![^\n]*", |lex| lex.slice()[3..].to_string())]
    DocCommentBang(String),

    // ── Keywords ──────────────────────────────────────────────────────
    /// Phase 15: export defn — replaces #export pragma
    /// 2026-08-13 (layout-keywords plan): `spec` — physical-layout metadata
    /// assignment (PascalCase key): `spec Bits: 64;` `spec Align: 8;`
    /// `spec Bytes: 4;` `spec MaxBits: 16;` `spec Endian: Big;`. Declared layout
    /// is the modern, disclosed spelling of the `!>` annotation form (SPEC §8.9).
    #[token("spec")]
    Spec,

    /// 2026-09-11 (Part C, Electronics Briev): first-class component pin —
    /// `pin a = 1;` or `pin a;` (auto-number) inside a component type body.
    #[token("pin")]
    Pin,

    #[token("export")]
    Export,

    #[token("defn")]
    Defn,

    #[token("let")]
    Let,

    #[token("const")]
    Const,

    #[token("txn")]
    Txn,

    #[token("node")]
    Node,

    #[token("async")]
    Async,

    /// 2026-08-01 (E): `seq` — ordering/layout/sequence modifier (prefix).
    /// `seq struct` bypasses apply_field_modes; `seq node`/`seq txn` use
    /// sequential dispatch; `seq Int[x]`/`seq foreach` disable vectorization.
    #[token("seq")]
    Seq,

    /// 2026-08-13 (layout-keywords plan): `pack` — bit-contiguous, zero-padding
    /// struct modifier (prefix, combinable with `seq` in any order).
    #[token("pack")]
    Pack,

    /// 2026-08-13 (layout-keywords plan Phase 4): `trap` — hardware abort
    /// (statement, guard body, match-arm value).
    #[token("trap")]
    Trap,

    /// 2026-09-06 (halt slice): `halt` — the bare-metal stop (statement).
    /// The deliberate sibling of `trap`: trap = abort when something is
    /// wrong, halt = the work is done, wait for the world. Never-type —
    /// control flow past it is dead.
    #[token("halt")]
    Halt,

    /// 2026-08-13 (layout-keywords plan Phase 5): `atomic` — field modifier
    /// (prefix). `atomic x: Int;` marks a struct/obj/type slot as
    /// atomically-read/written (SPEC §8.2). A concurrency declaration, never
    /// a speed path — plain fields stay on the default (non-atomic) path.
    #[token("atomic")]
    Atomic,

    /// 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): atomic ordering
    /// qualifiers — context-sensitive, only valid before `atomic`.
    /// `relaxed atomic count: Int;` — memory_order_relaxed.
    #[token("relaxed")]
    Relaxed,
    /// `acquire atomic flag: Bool;` — memory_order_acquire.
    #[token("acquire")]
    Acquire,
    /// `release atomic payload: Int[256];` — memory_order_release.
    #[token("release")]
    Release,
    /// `bartered atomic ref_count: Int;` — memory_order_acq_rel (RMW exchange).
    #[token("bartered")]
    Bartered,

    /// 2026-08-13 (layout-keywords plan Phase 6): `union` — an untagged
    /// overlay declaration: `union Name { field: Type, … };` — all fields
    /// share storage at offset 0 (SPEC §8.2).
    #[token("union")]
    Union,

    /// 2026-08-01 (E): `vol` — memory-visibility modifier (prefix).
    /// `vol let x` emits volatile load/store.
    #[token("vol")]
    Vol,

    /// 2026-08-25 (seq-firmem plan): `mem let buf: Int[64] = ...` — pin a
    /// state array to the seq.firmem memory-macro lowering. Strategy
    /// keyword: carries intent (port limits accepted, element obligations
    /// unavailable), never acceleration.
    #[token("mem")]
    Mem,

    /// 2026-08-27 (cbv-HW plan Slice A): `extern Name(ports) -> outs
    /// from "path";` — a foreign HDL module import. Declares a bodyless
    /// cell-shaped record whose definition lives in the referenced source;
    /// the CIRCT target emits it as an `hw.module.extern` blackbox.
    #[token("extern")]
    Extern,

    /// 2026-08-15 (coll plan): `coll` — the native strategy keyword for
    /// declaring collections. Prefix on `obj`/`struct`: compiler-owned Length
    /// semantics (hidden cap/len slots), scaffolded op surface. See
    /// docs/plans/2026-08-15-coll-length-semantics.md.
    #[token("coll")]
    Coll,

    /// 2026-08-04 (out-observability plan): `out` — observability modifier
    /// (prefix). `out defn`/`out node`/`out txn` mark the callable's calls as
    /// liveness roots (the compiler must not eliminate them); `out let` marks
    /// the variable's reads/writes as live. A pin, never an acceleration.
    #[token("out")]
    Out,

    /// 2026-08-06 (accel plan): `accel` — GPU-deferral modifier (prefix).
    /// `accel node`/`accel txn` mark the body as a per-firing parallel map
    /// over work-items; the compiler defers execution to the GPU only when it
    /// verifies a speedup, else silent CPU fallback. See
    /// docs/plans/2026-08-06-accel-gpu-offload.md.
    #[token("accel")]
    Accel,

    #[token("await")]
    Await,

    #[token("spawn")]
    Spawn,

    #[token("term")]
    Term,

    #[token("endprogram")]
    EndProgram,

    #[token("beginprogram")]
    BeginProgram,

    #[token("rollback")]
    Rollback,

    /// 2026-08-09 (Phase 10): `defer { ... }` — cleanup registered for the
    /// current transaction/reactive firing; runs on `term`, `rollback`, and
    /// `endprogram`. Replaces the legacy `#on_exit`.
    #[token("defer")]
    Defer,

    /// 2026-08-09 (Phase 10): `mutex { ... }` — a serial section (replaces
    /// the legacy `sync {}`).
    #[token("mutex")]
    Mutex,

    #[token("import")]
    Import,

    #[token("from")]
    From,

    #[token("as")]
    As,

    #[token("frgn")]
    Frgn,

    // 2026-07-12: inop/inop! removed — all ops are # intrinsics
    #[token("meld")]
    Meld,

    #[token("reg")]
    Reg,

    #[token("op")]
    Op,

    #[token("type")]
    Type,

    #[token("trait")]
    Trait,

    #[token("impl")]
    Impl,

    #[token("cell")]
    Cell,

    #[token("obj")]
    Obj,
    #[token("struct")]
    Struct,

    #[token("render")]
    Render,

    /// 2026-09-23 (fab plan): the physical-layout section —
    /// `fab { board 30mm x 20mm; place u1 @ (20mm, 10mm); }`.
    #[token("fab")]
    Fab,

    #[token("enum")]
    Enum,

    #[token("trg")]
    Trg,





    #[token("within")]
    Within,

    #[token("match")]
    Match,

    // 2026-07-15: template/macro removed — replaced by $(Stage) blocks

    #[token("quote")]
    Quote,

    #[token("$")]
    Dollar,

    #[token("$!")]
    DollarBang,

    #[token("foreach")]
    Foreach,

    #[token("break")]
    Break,

    #[token("pvt")]
    Pvt,

    #[token("sed")]
    Sed,

    #[token("sync")]
    Sync,

    // 2026-09-14 (machine-entry plan): `bootstrap node` — the authored
    // program entry (pre-reactor machine beginning; SPEC §11.5, §13.2).
    #[token("bootstrap")]
    Bootstrap,

    #[token("true")]
    BoolTrue,

    #[token("false")]
    BoolFalse,

    // 2026-08-05 (Phase 3): canonical duration units are cyc/ns/ms/s/min
    // (SPEC §16.1). The alias tokens cycles/sec/seconds/minute/minutes/
    // nanoseconds are removed; s/ns/min are parsed contextually as
    // identifiers after a numeric bound.
    #[token("cyc")]
    Cyc,

    #[token("ms")]
    Ms,

    // ── Phase 16A/16D: Cell file keywords ─────────────────────
    /// 2026-07-12: .c.bv cell parameter declaration
    #[token("input")]
    Input,

    /// 2026-07-12: .c.bv cell output declaration
    #[token("output")]
    Output,

    // ── Phase 8.6: Guard/derivation keywords ─────────────────
    /// 2026-07-12: Guard statement keyword
    #[token("when")]
    When,

    // ── Operators ─────────────────────────────────────────────
    #[token("=>")]
    FatArrow,

    #[token("=")]
    Eq,

    #[token("&")]
    Ampersand,

    #[token("@")]
    At,

    #[token("==")]
    EqEq,

    #[token("!=")]
    Ne,

    #[token("<")]
    Lt,

    #[token("</")]
    LtSlash,

    #[token("<=")]
    Le,

    #[token(">")]
    Gt,

    #[token(">=")]
    Ge,

    #[token("<<")]
    Shl,

    #[token(">>")]
    Shr,

    #[token("|")]
    Pipe,

    #[token("||")]
    OrOr,

    #[token("&&")]
    AndAnd,

    #[token("!")]
    Not,

    #[token("?")]
    Question,

    #[token("-=")]
    MinusEq,
    #[token("-")]
    Minus,

    // ── Consumptive operators (Phase 3, 2026-08-01) ─────────────────
    // `~` prepends to a binary operator to consume/destroy the RHS after the
    // op. `~` ALONE stays unary bitwise NOT (the multi-char tokens win longest
    // match). `~?` (temporal fallback) was removed as dead.
    #[token("~<-")]
    TildeArrowLeft,
    #[token("~=")]
    TildeEq,
    #[token("~/")]
    TildeSlash,
    #[token("~*")]
    TildeStar,
    #[token("~-")]
    TildeMinus,
    #[token("~+")]
    TildePlus,

    #[token("~")]
    Tilde,

    #[token("+=")]
    PlusEq,

    #[token("+")]
    Plus,

    #[token("*=")]
    StarEq,
    #[token("*")]
    Star,

    #[token("/=")]
    SlashEq,
    #[token("/")]
    Slash,

    #[token("^")]
    BitXor,

    #[token("->")]
    Arrow,

    #[token("<-")]
    ArrowLeft,

/// !> — metadata assignment operator
#[token("!>")]
ExclaimArrow,

    #[token("_")]
    Underscore,

    // 2026-08-05 (Phase 3): legacy pragma/attribute tokens (#[, #![, #pragma,
    // #!pragma, #?, #!) removed — no parser used them and the SPEC forbids
    // legacy pragma syntax.

    // 2026-07-18: Compiler-internal hash words for strategy op bindings.
    // #Lh = left operand, #Rh = right operand, #T = type parameter.
    #[token("#Lh")]
    HashL,
    #[token("#Rh")]
    HashR,
    #[token("#T")]
    HashT,

    // 2026-07-23: #Self — self-reference hashword for protocol contracts.
    // Reserved for this use case. Matched before the identifier regex.
    #[token("#Self")]
    HashSelf,

    // ── Punctuation ───────────────────────────────────────────
    #[token(";")]
    Semicolon,

    /// `.^^` — compile-time reflection access (`x.^^Size`). Must lex before
    /// `.^` so logos longest-match handles the triple-char form; order is
    /// otherwise irrelevant (logos picks the longest match).
    #[token(".^^")]
    DotCaretCaret,

    /// `.^` — runtime reflection access (`x.^Length`, `x.^Ptr`).
    /// The caret alone remains bitwise XOR (`a ^ b`); the dot disambiguates.
    #[token(".^")]
    DotCaret,

    /// := — derivation / compile-time assertion block
    #[token(":=")]
    ColonEq,

    #[token(":")]
    Colon,

    #[token("{")]
    LBrace,

    #[token("}")]
    RBrace,

    #[token("(")]
    LParen,

    #[token(")")]
    RParen,

    #[token("[")]
    LBracket,

    #[token("]")]
    RBracket,

    #[token(",")]
    Comma,

    #[token("%")]
    Percent,

    #[token("...")]
    Ellipsis,

    #[token("..=")]
    DotDotEq,

    #[token("..")]
    DotDot,

    #[token(".")]
    Dot,

    // ── Literals ──────────────────────────────────────────────
    // 2026-08-05 (Phase 3): width-suffix literal tokens (i8/i16/i32/i64,
    // u8/u16/u32/u64, f32/f64) removed — physical width is expressed through
    // type annotation or cast (SPEC §16.1), never a lexer suffix family.
    #[regex(r"0x[0-9a-fA-F]+", |lex| i64::from_str_radix(&lex.slice()[2..], 16).ok())]
    #[regex(r"[0-9]+", |lex| lex.slice().parse().ok())]
    Integer(i64),

    // 2026-09-02 (plan exponent-notation): floats carry an optional
    // decimal exponent, C-style maximal munch — `1.0e-8`, and the BARE
    // form `1e5` (dot optional when an exponent follows; logos
    // longest-match picks it over Decimal). Fallbacks: `1e` fails the
    // exponent regex (digits required) → Decimal + Ident; `0x1e5` stays
    // hex (the bare regex cannot cross the `x`). f64::parse handles both
    // forms natively — no parser change.
    #[regex(r"[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?", |lex| lex.slice().parse().ok())]
    #[regex(r"[0-9]+[eE][+-]?[0-9]+", |lex| lex.slice().parse().ok())]
    Float(f64),

    #[regex(r#""([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        Some(unescape_string(&s[1..s.len()-1]))
    })]
    String(String),

    /// 2026-08-05 (Phase 7): raw string literal `#r"..."` — content is
    /// verbatim; escapes are NOT interpreted (SPEC §16.2).
    #[regex(r#"#r"([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        Some(s[3..s.len()-1].to_string())
    })]
    RawString(String),

    /// 2026-08-05 (Phase 7): byte literal `#b"..."` — escapes ARE interpreted
    /// (SPEC §16.2), producing the exact byte content.
    #[regex(r#"#b"([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        let inner = &s[3..s.len()-1];
        Some(unescape_bytes(inner))
    })]
    ByteString(Vec<u8>),

    #[regex(r"'([^'\\]|\\.)*'", |lex| {
        let s = lex.slice();
        let inner = &s[1..s.len()-1];
        if inner.is_empty() {
            return Some(' ');
        }
        if inner.len() == 1 {
            return Some(inner.chars().next().unwrap());
        }
        if inner == "\\0" { return Some('\0'); }
        if inner == "\\n" { return Some('\n'); }
        if inner == "\\t" { return Some('\t'); }
        if inner == "\\\\" { return Some('\\'); }
        if inner == "\\'" { return Some('\''); }
        if inner.len() == 4 && inner.starts_with("\\x") {
            if let Ok(h) = u8::from_str_radix(&inner[2..], 16) {
                return Some(h as char);
            }
        }
        if inner.starts_with("\\u{") && inner.ends_with('}') {
            if let Ok(cp) = u32::from_str_radix(&inner[3..inner.len()-1], 16) {
                return Some(char::from_u32(cp).unwrap_or('?'));
            }
        }
        Some(inner.chars().next().unwrap_or(' '))
    })]
    Char(char),

    // ── Identifiers (including PascalCase# intrinsics) ────────
    // 2026-07-12: # is a valid identifier character.
    // This allows Sqrt#, AddI64#, Print# as single tokens.
    // 2026-07-15: $ is also a valid identifier character.
    // This allows InsertRegistryImport$ as a single token.
    // $(Front) parses as Dollar LParen Identifier("Front") RParen
    // because $ standalone (with non-identifier char after) matches
    // the exact #[token("$")] before the identifier regex.
    // Specific multi-char hash tokens (#[, #![, #pragma, etc.)
    // are matched BEFORE this regex due to logos priority rules.
    #[regex(r"[a-zA-Z_#$][a-zA-Z0-9_#$]*", |lex| lex.slice().to_string())]
    Identifier(String),
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Export => write!(f, "export"),
            Token::Spec => write!(f, "spec"),
            Token::Pin => write!(f, "pin"),
            Token::Defn => write!(f, "defn"),
            Token::Let => write!(f, "let"),
            Token::Const => write!(f, "const"),
            Token::Txn => write!(f, "txn"),
            Token::Node => write!(f, "node"),
            Token::Async => write!(f, "async"),
            Token::Seq => write!(f, "seq"),
            Token::Pack => write!(f, "pack"),
            Token::Trap => write!(f, "trap"),
            Token::Halt => write!(f, "halt"),
            Token::Atomic => write!(f, "atomic"),
            Token::Relaxed => write!(f, "relaxed"),
            Token::Acquire => write!(f, "acquire"),
            Token::Release => write!(f, "release"),
            Token::Bartered => write!(f, "bartered"),
            Token::Union => write!(f, "union"),
            Token::Coll => write!(f, "coll"),
            Token::Vol => write!(f, "vol"),
            Token::Extern => write!(f, "extern"),
            Token::Mem => write!(f, "mem"),
            Token::Out => write!(f, "out"),
            Token::Accel => write!(f, "accel"),
            Token::Await => write!(f, "await"),
            Token::Spawn => write!(f, "spawn"),
            Token::Term => write!(f, "term"),
            Token::EndProgram => write!(f, "endprogram"),
            Token::BeginProgram => write!(f, "beginprogram"),
            Token::Rollback => write!(f, "rollback"),
            Token::Defer => write!(f, "defer"),
            Token::Mutex => write!(f, "mutex"),
            Token::Import => write!(f, "import"),
            Token::From => write!(f, "from"),
            Token::As => write!(f, "as"),
            Token::Frgn => write!(f, "frgn"),
            Token::Meld => write!(f, "meld"),
            Token::Reg => write!(f, "reg"),
            Token::Op => write!(f, "op"),
            Token::Type => write!(f, "type"),
            Token::Trait => write!(f, "trait"),
            Token::Impl => write!(f, "impl"),
            Token::Cell => write!(f, "cell"),
            Token::Obj => write!(f, "obj"),
            Token::Struct => write!(f, "struct"),
            Token::Render => write!(f, "render"),
            Token::Fab => write!(f, "fab"),
            Token::Enum => write!(f, "enum"),
            Token::Trg => write!(f, "trg"),
            Token::Within => write!(f, "within"),
            Token::Match => write!(f, "match"),
            // 2026-07-15: Template/Macro tokens removed

            Token::Quote => write!(f, "quote"),
            Token::Dollar => write!(f, "$"),
            Token::DollarBang => write!(f, "$!"),
            Token::Foreach => write!(f, "foreach"),
            Token::Break => write!(f, "break"),
            Token::Pvt => write!(f, "pvt"),
            Token::Sed => write!(f, "sed"),
            Token::Sync => write!(f, "sync"),
            Token::Bootstrap => write!(f, "bootstrap"),
            Token::BoolTrue => write!(f, "true"),
            Token::BoolFalse => write!(f, "false"),
            Token::Cyc => write!(f, "cyc"),
            Token::Ms => write!(f, "ms"),
            Token::Input => write!(f, "input"),
            Token::Output => write!(f, "output"),
            Token::When => write!(f, "when"),
            Token::FatArrow => write!(f, "=>"),
            Token::Eq => write!(f, "="),
            Token::EqEq => write!(f, "=="),
            Token::Ne => write!(f, "!="),
            Token::Lt => write!(f, "<"),
            Token::LtSlash => write!(f, "</"),
            Token::Le => write!(f, "<="),
            Token::Gt => write!(f, ">"),
            Token::Ge => write!(f, ">="),
            Token::Shl => write!(f, "<<"),
            Token::Shr => write!(f, ">>"),
            Token::Pipe => write!(f, "|"),
            Token::OrOr => write!(f, "||"),
            Token::AndAnd => write!(f, "&&"),
            Token::Not => write!(f, "!"),
            Token::Question => write!(f, "?"),
            Token::Minus => write!(f, "-"),
            Token::MinusEq => write!(f, "-="),
            Token::TildeArrowLeft => write!(f, "~<-"),
            Token::TildeEq => write!(f, "~="),
            Token::TildeSlash => write!(f, "~/"),
            Token::TildeStar => write!(f, "~*"),
            Token::TildeMinus => write!(f, "~-"),
            Token::TildePlus => write!(f, "~+"),
            Token::Tilde => write!(f, "~"),

            Token::PlusEq => write!(f, "+="),
            Token::Plus => write!(f, "+"),
            Token::StarEq => write!(f, "*="),
            Token::Star => write!(f, "*"),
            Token::SlashEq => write!(f, "/="),
            Token::BitXor => write!(f, "^"),
            Token::Arrow => write!(f, "->"),
            Token::ArrowLeft => write!(f, "<-"),
            Token::ExclaimArrow => write!(f, "!>"),
            Token::Underscore => write!(f, "_"),
            Token::Semicolon => write!(f, ";"),
            Token::DotCaretCaret => write!(f, ".^^"),
            Token::DotCaret => write!(f, ".^"),
            Token::ColonEq => write!(f, ":="),
            Token::Colon => write!(f, ":"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::Comma => write!(f, ","),
            Token::Percent => write!(f, "%"),
            Token::Ellipsis => write!(f, "..."),
            Token::DotDotEq => write!(f, "..="),
            Token::DotDot => write!(f, ".."),
            Token::Dot => write!(f, "."),
            Token::Ampersand => write!(f, "&"),
            Token::At => write!(f, "@"),
            Token::Integer(n) => write!(f, "{}", n),
            Token::Float(n) => write!(f, "{}", n),
            Token::String(s) => write!(f, "\"{}\"", s),
            Token::RawString(s) => write!(f, "#r\"{}\"", s),
            Token::ByteString(s) => write!(f, "#b\"{}\"", String::from_utf8_lossy(s)),
            Token::Char(c) => write!(f, "'{}'", c),
            Token::Identifier(s) => write!(f, "{}", s),
            Token::DocComment(s) => write!(f, "///{}", s),
            Token::DocCommentBang(s) => write!(f, "//!{}", s),
            Token::Slash => write!(f, "/"),
            Token::HashL => write!(f, "#Lh"),
            Token::HashR => write!(f, "#Rh"),
            Token::HashT => write!(f, "#T"),
            Token::HashSelf => write!(f, "#Self"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocab::{LanguageVocab, VocabStatus};

    /// 2026-08-05 (Phase 1 parity): every keyword the lexer recognizes as a
    /// dedicated token must be recorded in the canonical vocab as canonical,
    /// removed, or reserved. This prevents unaccounted language surface.
    #[test]
    fn lexer_keywords_are_accounted_in_vocab() {
        let vocab = LanguageVocab::canonical();
        // Keyword tokens: identifiers are excluded (handled by pattern),
        // literals/punctuation are covered by their own checks. This list is
        // the dedicated keyword tokens that Phase 3 will converge with the
        // vocab (removing Removed/Reserved tokens that are not canonical).
        let keyword_tokens: &[&str] = &[
            "export", "defn", "let", "const", "txn", "node", "async", "seq",
            "vol", "out", "spec", "pin", "pack", "trap", "halt", "atomic", "union", "coll", "await", "spawn", "term", "term!", "rollback", "import",
            "bootstrap",
            "mem", "relaxed", "acquire", "release", "bartered",
            "from", "as", "frgn", "meld", "reg", "op", "prop",
            "type", "trait", "impl", "cell", "obj", "struct", "render", "enum", "trg",
            "within", "match", "quote", "foreach", "pvt", "sed",
            "break",
            "sync", "true", "false", "cyc", "ms",
        ];
        for name in keyword_tokens {
            assert!(
                vocab.keyword_status(name).is_some(),
                "lexer keyword '{name}' is not recorded in the canonical vocab"
            );
        }
        // Canonical keywords must not be recorded as removed/reserved.
        for kw in vocab.canonical_keywords() {
            assert_ne!(kw.status, VocabStatus::Removed);
            assert_ne!(kw.status, VocabStatus::Reserved);
        }
    }

    /// 2026-09-02 (plan exponent-notation): float literals carry an
    /// optional decimal exponent — dot form and the C-style bare form —
    /// each a SINGLE Float token (maximal munch).
    #[test]
    fn exponent_float_literals_are_single_tokens() {
        for (src, want) in [
            ("1.0e-8", 1.0e-8),
            ("1.0e+8", 1.0e8),
            ("1.0E5", 1.0e5),
            ("2.5e0", 2.5),
            ("1e5", 1.0e5),
            ("1e-8", 1.0e-8),
            ("1e+8", 1.0e8),
            ("1E5", 1.0e5),
            ("65536e-16", 65536e-16),
        ] {
            let mut lexer = Token::lexer(src);
            match lexer.next() {
                Some(Ok(Token::Float(v))) => assert_eq!(
                    v, want,
                    "'{src}' must lex as Float({want}), got Float({v})"
                ),
                other => panic!("'{src}' must lex as a single Float token, got {other:?}"),
            }
            assert_eq!(lexer.next(), None, "'{src}' must consume the whole input");
        }
    }

    /// The fallbacks stay exactly as before the exponent change.
    #[test]
    fn exponent_fallbacks_unchanged() {
        // `1e` — no digits after the exponent marker → Decimal + Ident.
        let toks: Vec<Token> = Token::lexer("1e").filter_map(|t| t.ok()).collect();
        assert_eq!(toks.len(), 2, "1e must stay Decimal + Ident: {toks:?}");
        // Spaced `1e + 5` is arithmetic, never a literal (maximal munch
        // requires digits right after e/+-).
        let toks: Vec<Token> = Token::lexer("1e + 5").filter_map(|t| t.ok()).collect();
        assert_eq!(
            toks,
            vec![Token::Integer(1), Token::Identifier("e".into()), Token::Plus, Token::Integer(5)],
            "1e + 5 must stay 1e, +, 5"
        );
        // `1.0-8` is subtraction.
        let toks: Vec<Token> = Token::lexer("1.0-8").filter_map(|t| t.ok()).collect();
        assert_eq!(toks.len(), 3, "1.0-8 must stay 1.0, -, 8: {toks:?}");
        // Hex keeps the digits: 0x1e5 is one hex token.
        let toks: Vec<Token> = Token::lexer("0x1e5").filter_map(|t| t.ok()).collect();
        assert_eq!(toks.len(), 1, "0x1e5 must stay a single hex token: {toks:?}");
        // Identifier adjacency: `1.0expr` → Float + Ident (regex needs
        // digits after e; longest match on the dot form).
        let toks: Vec<Token> = Token::lexer("1.0expr").filter_map(|t| t.ok()).collect();
        assert_eq!(toks.len(), 2, "1.0expr must stay Float + Ident: {toks:?}");
    }

    #[test]
    fn test_lexer() {
        let mut lexer = Token::lexer("fetch: Int -> Int;");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("fetch".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::Colon)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("Int".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::Arrow)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("Int".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::Semicolon)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_char_literals() {
        let mut lexer = Token::lexer("'a'");
        assert_eq!(lexer.next(), Some(Ok(Token::Char('a'))));

        let mut lexer = Token::lexer("'\\n'");
        assert_eq!(lexer.next(), Some(Ok(Token::Char('\n'))));

        let mut lexer = Token::lexer("'\\t'");
        assert_eq!(lexer.next(), Some(Ok(Token::Char('\t'))));

        let mut lexer = Token::lexer("'\\\\'");
        assert_eq!(lexer.next(), Some(Ok(Token::Char('\\'))));

        let mut lexer = Token::lexer("'\\''");
        assert_eq!(lexer.next(), Some(Ok(Token::Char('\''))));

        let mut lexer = Token::lexer("'\\u{1F600}'");
        assert_eq!(lexer.next(), Some(Ok(Token::Char('😀'))));

        let mut lexer = Token::lexer("let c: Char = 'x';");
        assert_eq!(lexer.next(), Some(Ok(Token::Let)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("c".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::Colon)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("Char".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::Eq)));
        assert_eq!(lexer.next(), Some(Ok(Token::Char('x'))));
        assert_eq!(lexer.next(), Some(Ok(Token::Semicolon)));
    }

    #[test]
    fn test_nested_generic_tokens() {
        let mut lexer = Token::lexer("List<List<Int>>");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("List".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::Lt)));
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("List".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::Lt)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("Int".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::Shr)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_dollar_tokens() {
        // 2026-07-15: $ is a valid identifier character, so $unless is a
        // single Identifier token (longest match wins). $! is still a
        // single DollarBang token (exact token match beats identifier).
        let mut lexer = Token::lexer("$unless $!circular_buffer");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("$unless".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::DollarBang)));
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("circular_buffer".to_string())))
        );
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_dollar_bang_as_single_token() {
        let mut lexer = Token::lexer("$!x");
        assert_eq!(lexer.next(), Some(Ok(Token::DollarBang)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("x".to_string()))));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_hash_question_is_identifier_char() {
        // 2026-08-05 (Phase 3): the `#?` pragma token is removed. `#?inline`
        // lexes as identifier "#", then the question token, then "inline",
        // because `?` is not an identifier character.
        let mut lexer = Token::lexer("#?inline");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("#".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::Question)));
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("inline".to_string())))
        );
        assert_eq!(lexer.next(), None);
    }

    // 2026-07-12: Updated for new #-as-ident-char behavior.
    // #volatile is now a single identifier, not Hash + volatile.
    #[test]
    fn test_hash_as_identifier_char() {
        let mut lexer = Token::lexer("#volatile");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("#volatile".to_string())))
        );
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_exclaim_arrow_as_single_token() {
        let mut lexer = Token::lexer("x !> 5");
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("x".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::ExclaimArrow)));
        assert_eq!(lexer.next(), Some(Ok(Token::Integer(5))));
        assert_eq!(lexer.next(), None);

        let mut lexer2 = Token::lexer("x < 5");
        assert_eq!(lexer2.next(), Some(Ok(Token::Identifier("x".to_string()))));
        assert_eq!(lexer2.next(), Some(Ok(Token::Lt)));
        assert_eq!(lexer2.next(), Some(Ok(Token::Integer(5))));
        assert_eq!(lexer2.next(), None);
    }

    // ── New tests for Phase 0.1 features ──────────────────────

    #[test]
    fn test_intrinsic_identifier() {
        let mut lexer = Token::lexer("Sqrt#(x)");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("Sqrt#".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::LParen)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("x".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::RParen)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_entry_hash_bracket() {
        // 2026-08-01 (Phase 2): `[#]` is no longer a contract marker — the
        // entry!/args! macros (Phase 3) replace it. The lexer still produces
        // Identifier("#") inside brackets; the PARSER now rejects `[#]` as a
        // removed syntax. This test pins the tokenization only.
        let mut lexer = Token::lexer("[#]");
        assert_eq!(lexer.next(), Some(Ok(Token::LBracket)));
        // # is now an identifier character, so [#] -> LBracket, Identifier("#"), RBracket
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("#".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::RBracket)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_hash_in_identifier_middle() {
        let mut lexer = Token::lexer("foo#bar");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("foo#bar".to_string())))
        );
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_keyword_when() {
        let mut lexer = Token::lexer("when");
        assert_eq!(lexer.next(), Some(Ok(Token::When)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_keyword_input() {
        let mut lexer = Token::lexer("input port: Int;");
        assert_eq!(lexer.next(), Some(Ok(Token::Input)));
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("port".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::Colon)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("Int".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::Semicolon)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_keyword_output() {
        let mut lexer = Token::lexer("output status: Int;");
        assert_eq!(lexer.next(), Some(Ok(Token::Output)));
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("status".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::Colon)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("Int".to_string()))));
        assert_eq!(lexer.next(), Some(Ok(Token::Semicolon)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_inop_removed() {
        // inop should now lex as a regular identifier, not a keyword
        let mut lexer = Token::lexer("inop");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("inop".to_string())))
        );
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_export_keyword() {
        let mut lexer = Token::lexer("export defn add(a: Int) -> Int;");
        assert_eq!(lexer.next(), Some(Ok(Token::Export)));
        assert_eq!(lexer.next(), Some(Ok(Token::Defn)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("add".to_string()))));
    }

    #[test]
    fn test_hash_bracket_lexes_as_identifiers() {
        // 2026-08-05 (Phase 3): legacy `#[...]` attribute tokens are removed;
        // `#[inline]` lexes as identifier "#", then "[inline]".
        let mut lexer = Token::lexer("#[inline]");
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("#".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::LBracket)));
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("inline".to_string())))
        );
        assert_eq!(lexer.next(), Some(Ok(Token::RBracket)));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_raw_and_byte_literals() {
        // 2026-08-05 (Phase 7): `#r"..."` raw (verbatim) and `#b"..."` bytes
        // (escapes interpreted) lex as single tokens (SPEC §16.2).
        let mut lexer = Token::lexer(r#"#r"a\nb""#);
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::RawString("a\\nb".to_string())))
        );
        assert_eq!(lexer.next(), None);

        let mut lexer = Token::lexer(r#"#b"\x41\x42""#);
        assert_eq!(lexer.next(), Some(Ok(Token::ByteString(b"AB".to_vec()))));
        assert_eq!(lexer.next(), None);
    }

    #[test]
    fn test_colon_eq_derivation() {
        let mut lexer = Token::lexer("defn add(a: Int, b: Int) -> Int := { 1, 2 -> 3 };");
        assert_eq!(lexer.next(), Some(Ok(Token::Defn)));
        assert_eq!(lexer.next(), Some(Ok(Token::Identifier("add".to_string()))));
    }

    #[test]
    fn test_display_roundtrip() {
        let source = "defn main() -> Int { term 0; };";
        let mut lexer = Token::lexer(source);
        let mut found = Vec::new();
        while let Some(Ok(tok)) = lexer.next() {
            found.push(tok);
        }
        // Just verify that all tokens have Display impls that don't panic
        for tok in &found {
            let _ = format!("{}", tok);
        }
    }

    #[test]
    fn test_underscore_identifier() {
        let mut lexer = Token::lexer("_ _foo __bar");
        assert_eq!(lexer.next(), Some(Ok(Token::Underscore)));
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("_foo".to_string())))
        );
        assert_eq!(
            lexer.next(),
            Some(Ok(Token::Identifier("__bar".to_string())))
        );
        assert_eq!(lexer.next(), None);
    }
}

/// Convenience: tokenize a source string into (Token, Range) pairs.
/// 2026-08-05 (Phase 7): process string-literal escapes into a value.
/// Shared by the quoted-string token and the `#b` byte literal.
fn unescape_string(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(esc) = chars.next() else {
            out.push('\\');
            continue;
        };
        match esc {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '0' => out.push('\0'),
            'x' => out.push(decode_hex2(&mut chars)),
            'u' => out.push(decode_unicode(&mut chars)),
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// 2026-08-05 (Phase 7): decode `\xHH` into a char.
/// 2026-08-06 (Phase 7): decode `#b"..."` escapes into RAW bytes. `\xHH`
/// yields the exact byte HH (SPEC §16.2) — not the UTF-8 encoding of the
/// codepoint. Other escapes and literal chars map to their byte content.
fn unescape_bytes(inner: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend_from_slice(&decode_byte_escape(&mut chars));
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// Decode a single backslash escape after `\` into its byte content.
fn decode_byte_escape(chars: &mut std::str::Chars) -> Vec<u8> {
    let mut out = Vec::new();
    match chars.next() {
        Some('n') => out.push(b'\n'),
        Some('t') => out.push(b'\t'),
        Some('r') => out.push(b'\r'),
        Some('0') => out.push(0),
        Some('\\') => out.push(b'\\'),
        Some('"') => out.push(b'"'),
        Some('x') => {
            let mut hex = String::new();
            for _ in 0..2 {
                if let Some(h) = chars.next() {
                    hex.push(h);
                } else {
                    break;
                }
            }
            if let Ok(b) = u8::from_str_radix(&hex, 16) {
                out.push(b);
            }
        }
        Some(other) => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
        }
        None => out.push(b'\\'),
    }
    out
}

fn decode_hex2(chars: &mut std::str::Chars) -> char {    let hex_str: String = chars.by_ref().take(2).collect();
    match u8::from_str_radix(&hex_str, 16) {
        Ok(h) => h as char,
        Err(_) => '?',
    }
}

/// 2026-08-05 (Phase 7): decode `\u{HEX}` into a char.
fn decode_unicode(chars: &mut std::str::Chars) -> char {
    if chars.next() != Some('{') {
        return '{';
    }
    let mut hex = String::new();
    while let Some(h) = chars.next() {
        if h == '}' {
            break;
        }
        hex.push(h);
    }
    match u32::from_str_radix(&hex, 16) {
        Ok(cp) => char::from_u32(cp).unwrap_or('?'),
        Err(_) => '?',
    }
}

/// Returns Ok(tokens) on success, Err on lex failure.
/// 2026-07-15: Phase 2 — Added for system plugin discovery (plugin loader)
/// and other programmatic use outside the compile pipeline.
pub fn tokenize(source: &str) -> Result<Vec<(Token, std::ops::Range<usize>)>, String> {
    let mut lexer = Token::lexer(source);
    let mut tokens = Vec::new();
    while let Some(result) = lexer.next() {
        let token = result.map_err(|_| "lex error".to_string())?;
        let span = lexer.span();
        tokens.push((token, span));
    }
    Ok(tokens)
}

#[cfg(test)]
mod consumptive_tests {
    use super::*;

    #[test]
    fn test_consumptive_tokens_lex_longest_match() {
        // `~<-` wins over `~` + `<-`; `~=` over `~`; `~+` over `~`.
        let toks: Vec<_> = Token::lexer("a ~<- b ~= c ~+ d ~- e ~* f ~/ g ~ h ~? i")
            .map(|r| r.unwrap())
            .collect();
        let names: Vec<String> = toks.iter().map(|t| format!("{:?}", t)).collect();
        let joined = names.join(" ");
        assert!(joined.contains("TildeArrowLeft"), "~<-: {joined}");
        assert!(joined.contains("TildeEq"), "~=: {joined}");
        assert!(joined.contains("TildePlus"), "~+: {joined}");
        assert!(joined.contains("TildeMinus"), "~-: {joined}");
        assert!(joined.contains("TildeStar"), "~*: {joined}");
        assert!(joined.contains("TildeSlash"), "~/: {joined}");
        assert!(joined.contains("Tilde"), "bare ~: {joined}");
        assert!(!joined.contains("TildeQuestion"), "~? removed: {joined}");
    }
}

#[cfg(test)]
mod unescape_bytes_tests {
    use super::unescape_bytes;

    #[test]
    fn hex_escape_is_exact_byte() {
        assert_eq!(unescape_bytes(r"\x89PNG"), vec![0x89, b'P', b'N', b'G']);
    }
    #[test]
    fn common_escapes() {
        assert_eq!(unescape_bytes(r"a\nb"), vec![b'a', b'\n', b'b']);
        assert_eq!(unescape_bytes(r"a\\b"), vec![b'a', b'\\', b'b']);
    }
}
