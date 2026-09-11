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

//! Canonical language vocabulary.
//!
//! 2026-08-05 (normative spec Phase 1): one machine-readable source of truth
//! for keywords, reserved words, operators, sigils, file extensions/profiles,
//! casing conventions, and staged/removed language surface. The lexer, LSP,
//! TextMate grammar, formatter, and diagnostics must agree with this module.
//! See `spec/SPEC.md` §4, §23 and the implementation plan Phase 1.
//!
//! Status semantics:
//! - `Canonical`: valid, exact-spelling vocabulary in the normative grammar.
//! - `Removed`: no longer part of Briev; the parser must reject it (Phase 3)
//!   with migration guidance. Retained here only so diagnostics can explain.
//! - `Reserved`: intentionally unavailable to user identifiers for a future
//!   language contract (`sed`, `pvt`, `reg`).
//!
//! Compiler-known vocabulary (keywords, hashwords, intrinsics, operation
//! identities, stages) requires exact spelling/casing; violations are errors.
//! User-declared identifiers that violate casing conventions are only
//! informational/warning diagnostics.

use serde::{Deserialize, Serialize};

/// 2026-08-05: lifecycle status of a vocabulary entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VocabStatus {
    Canonical,
    Removed,
    Reserved,
}

/// 2026-08-05: broad grammar role used by the LSP/highlighter/manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeywordContext {
    Declaration,
    Statement,
    Modifier,
    Literal,
    Operator,
    CompileTime,
    Reactive,
    Foreign,
    Render,
    Ownership,
    Reserved,
}

/// 2026-08-05: one keyword entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyword {
    pub name: String,
    pub status: VocabStatus,
    pub context: KeywordContext,
}

/// 2026-08-05: identifier casing conventions. User-declared violations are
/// warning/info only; compiler-known vocabulary is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Casing {
    PascalCase,
    SnakeCase,
    PascalCaseHash,
}

/// 2026-08-05: the full canonical vocabulary, serializable for tooling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageVocab {
    pub version: String,
    pub keywords: Vec<Keyword>,
    pub operators: Vec<String>,
    pub sigils: Vec<String>,
    pub extensions: Vec<String>,
    pub profiles: Vec<String>,
    pub hashwords: Vec<String>,
    pub intrinsics: Vec<String>,
    pub operation_identities: Vec<String>,
    pub stages: Vec<String>,
    pub staged_features: Vec<String>,
    pub casing: Vec<(String, Casing)>,
}

impl Default for LanguageVocab {
    fn default() -> Self {
        Self::canonical()
    }
}

impl LanguageVocab {
    /// 2026-08-05: the canonical vocabulary defined here is the single source
    /// of truth. When a keyword/operator is added or removed, update this list
    /// and the Phase 1 parity tests.
    pub fn canonical() -> Self {
        let kw = |name: &str, status: VocabStatus, context: KeywordContext| Keyword {
            name: name.to_string(),
            status,
            context,
        };
        let ss = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        LanguageVocab {
            version: "2026-08-05.1".to_string(),
            keywords: vec![
                // Declarations
                kw("let", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("const", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("init", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("type", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("trait", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("proto", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("struct", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("seq", VocabStatus::Canonical, KeywordContext::Modifier),
                // 2026-08-13 (layout-keywords plan): `pack` — bit-contiguous,
                // zero-padding struct modifier (`pack struct`). Disclosed, like
                // `seq`; never a speed win over the default representation.
                kw("pack", VocabStatus::Canonical, KeywordContext::Modifier),
                // 2026-08-13 (layout-keywords plan Phase 4): `trap` — hardware
                // abort (statement, guard body, match-arm value). Never-type.
                kw("trap", VocabStatus::Canonical, KeywordContext::Statement),
                // 2026-09-06 (halt slice): `halt` — the bare-metal stop,
                // trap's deliberate sibling (statement). Never-type: control
                // flow past it is dead.
                kw("halt", VocabStatus::Canonical, KeywordContext::Statement),
                // 2026-08-13 (layout-keywords plan Phase 5): `atomic` —
                // per-field concurrency modifier (`atomic x: Int;`). Disclosed,
                // never a speed path; plain fields keep the default path.
                kw("atomic", VocabStatus::Canonical, KeywordContext::Modifier),
                // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md): atomic
                // ORDERING qualifiers — context-sensitive, only valid before
                // `atomic` (`relaxed atomic count: Int;`). `seq` (existing
                // keyword) is the default ordering; `bartered` = acq_rel (an
                // exchange of visibility between threads).
                kw("relaxed", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("acquire", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("release", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("bartered", VocabStatus::Canonical, KeywordContext::Modifier),
                // 2026-08-13 (layout-keywords plan Phase 6): `union` — untagged
                // overlay declaration (fields share storage at offset 0).
                kw("union", VocabStatus::Canonical, KeywordContext::Declaration),
                // 2026-08-15 (coll plan): `coll` — the native strategy keyword
                // for declaring collections. Prefix on `obj`/`struct`:
                // compiler-owned Length semantics, scaffolded op surface.
                kw("coll", VocabStatus::Canonical, KeywordContext::Declaration),
                // 2026-08-13 (layout-keywords plan): `spec` — physical-layout
                // metadata statement (`spec Bits: 64;`). Declared layout, the
                // disclosed sibling of the `!>` annotation form.
                kw("spec", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("enum", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("impl", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("obj", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("cell", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("defn", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("txn", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("node", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("asm", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("render", VocabStatus::Canonical, KeywordContext::Render),
                // 2026-08-27 (cbv-HW plan Slice A): foreign HARDWARE imports — the
                // cell-shaped sibling of frgn (software FFI).
                kw("extern", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("frgn", VocabStatus::Canonical, KeywordContext::Foreign),
                kw("optional", VocabStatus::Canonical, KeywordContext::Foreign),
                kw("export", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("import", VocabStatus::Canonical, KeywordContext::Declaration),
                kw("from", VocabStatus::Canonical, KeywordContext::Foreign),
                kw("as", VocabStatus::Canonical, KeywordContext::Operator),
                kw("op", VocabStatus::Canonical, KeywordContext::Declaration),
                // Statements and control flow
                kw("term", VocabStatus::Canonical, KeywordContext::Statement),
                kw("endprogram", VocabStatus::Canonical, KeywordContext::Statement),
                kw("beginprogram", VocabStatus::Canonical, KeywordContext::Statement),
                kw("program", VocabStatus::Canonical, KeywordContext::Statement),
                kw("defer", VocabStatus::Canonical, KeywordContext::Statement),
                kw("rollback", VocabStatus::Canonical, KeywordContext::Statement),
                kw("mutex", VocabStatus::Canonical, KeywordContext::Statement),
                kw("barrier", VocabStatus::Canonical, KeywordContext::Statement),
                kw("match", VocabStatus::Canonical, KeywordContext::Statement),
                kw("when", VocabStatus::Canonical, KeywordContext::Statement),
                kw("foreach", VocabStatus::Canonical, KeywordContext::Statement),
                kw("break", VocabStatus::Canonical, KeywordContext::Statement),
                kw("spawn", VocabStatus::Canonical, KeywordContext::Reactive),
                kw("await", VocabStatus::Canonical, KeywordContext::Reactive),
                kw("free", VocabStatus::Canonical, KeywordContext::Ownership),
                kw("keep", VocabStatus::Canonical, KeywordContext::Ownership),
                kw("trg", VocabStatus::Canonical, KeywordContext::Reactive),
                kw("within", VocabStatus::Canonical, KeywordContext::Reactive),
                // Concurrency/reactive classification
                kw("async", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("sync", VocabStatus::Canonical, KeywordContext::Modifier),
                // Modifiers
                kw("vol", VocabStatus::Canonical, KeywordContext::Modifier),
                // 2026-08-25 (seq-firmem plan): `mem let` / `reg let` —
                // array-lowering pins (memory macro vs register file).
                kw("mem", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("reg", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("out", VocabStatus::Canonical, KeywordContext::Modifier),
                // Storage/layout strategy (2026-08-09, Phase 5): `box` marks a
                // spawned value as per-instance-heap (not pooled) when the pool
                // decoder is ambiguous; `spill` allows growth into a growable
                // buffer when a static pool column can't hold the worst case.
                // Contextual (spawn position only) — both stay legal
                // identifiers elsewhere (the compiler backend's own .bv uses
                // `spill` as a register word).
                kw("box", VocabStatus::Canonical, KeywordContext::Modifier),
                kw("spill", VocabStatus::Canonical, KeywordContext::Modifier),
                // Ownership algebra (SPEC §14)
                kw("borrow", VocabStatus::Canonical, KeywordContext::Ownership),
                kw("consume", VocabStatus::Canonical, KeywordContext::Ownership),
                kw("owned", VocabStatus::Canonical, KeywordContext::Ownership),
                kw("shared", VocabStatus::Canonical, KeywordContext::Ownership),
                kw("borrowed", VocabStatus::Canonical, KeywordContext::Ownership),
                // Literals
                kw("true", VocabStatus::Canonical, KeywordContext::Literal),
                kw("false", VocabStatus::Canonical, KeywordContext::Literal),
                // Duration units (canonical abbreviations)
                kw("cyc", VocabStatus::Canonical, KeywordContext::Literal),
                kw("ms", VocabStatus::Canonical, KeywordContext::Literal),
                kw("s", VocabStatus::Canonical, KeywordContext::Literal),
                kw("min", VocabStatus::Canonical, KeywordContext::Literal),
                kw("ns", VocabStatus::Canonical, KeywordContext::Literal),
                // Compile-time
                kw("quote", VocabStatus::Canonical, KeywordContext::CompileTime),
                // Reserved for future contracts
                kw("sed", VocabStatus::Reserved, KeywordContext::Reserved),
                kw("pvt", VocabStatus::Reserved, KeywordContext::Reserved),
                // 2026-08-25 (seq-firmem plan): `reg` PROMOTED Reserved →
                // Canonical Modifier — the register-file lowering pin. The
                // reserved seat was always earmarked for this.
                // Removed surface (Phase 3 removes from lexer/parser)
                kw("sig", VocabStatus::Removed, KeywordContext::Declaration),
                kw("state", VocabStatus::Removed, KeywordContext::Declaration),
                kw("rstruct", VocabStatus::Removed, KeywordContext::Render),
                kw("uni", VocabStatus::Removed, KeywordContext::Statement),
                kw("is", VocabStatus::Removed, KeywordContext::Operator),
                kw("like", VocabStatus::Removed, KeywordContext::Operator),
                kw("prop", VocabStatus::Removed, KeywordContext::Declaration),
                kw("meld", VocabStatus::Removed, KeywordContext::Declaration),
                kw("syscall", VocabStatus::Removed, KeywordContext::Foreign),
                kw("escape", VocabStatus::Removed, KeywordContext::Statement),
                kw("term!", VocabStatus::Removed, KeywordContext::Statement),
                kw("trg!", VocabStatus::Removed, KeywordContext::Reactive),
                kw("cell!", VocabStatus::Removed, KeywordContext::Reactive),
                kw("sync!", VocabStatus::Removed, KeywordContext::Statement),
                kw("frgn!", VocabStatus::Removed, KeywordContext::Foreign),
                kw("syscall!", VocabStatus::Removed, KeywordContext::Foreign),
                kw("Ptr!", VocabStatus::Removed, KeywordContext::Declaration),
                kw("Ok", VocabStatus::Removed, KeywordContext::Declaration),
                kw("Err", VocabStatus::Removed, KeywordContext::Declaration),
                kw("Some", VocabStatus::Removed, KeywordContext::Declaration),
                kw("None", VocabStatus::Removed, KeywordContext::Declaration),
                kw("some", VocabStatus::Removed, KeywordContext::Declaration),
                kw("none", VocabStatus::Removed, KeywordContext::Declaration),
                // Deprecated duration-unit aliases (canonical abbreviations are
                // cyc/ns/ms/s/min; SPEC §16.1)
                kw("cycles", VocabStatus::Removed, KeywordContext::Literal),
                kw("seconds", VocabStatus::Removed, KeywordContext::Literal),
                kw("minute", VocabStatus::Removed, KeywordContext::Literal),
                kw("minutes", VocabStatus::Removed, KeywordContext::Literal),
                kw("nanoseconds", VocabStatus::Removed, KeywordContext::Literal),
            ],
            operators: ss(&[
                "+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">=", "&&", "||",
                "!", "&", "|", "^", "~", "<<", ">>", "->", "<-", "~<-", "=>", "..",
                "..=", "+=", "-=", "*=", "/=", "=", "@", "$",
            ]),
            sigils: ss(&[
                "#Category", "Intrinsic#", "$name", "name!(...)", "$(Stage)",
                ".^Field", ".^^Field", "!value", "?",
            ]),
            extensions: ss(&["bv", "ebv", "abv", "sbv", "rbv", "dbv", "dbvl"]),
            profiles: ss(&["s", "f"]),
            hashwords: ss(&[
                "Int", "Float", "Bool", "String", "Char", "Bits", "Ptr", "Void",
                "Bit", "Link", "System", "L", "R", "T", "Self", "r", "b",
            ]),
            intrinsics: ss(&[
                "Abs#", "BitReverse#", "Popcount#", "LeadingZeros#", "TrailingZeros#",
                "SysCall#", "Malloc#", "Free#", "Print#", "Sqrt#",
            ]),
            operation_identities: ss(&[
                "Add", "Sub", "Mul", "Div", "Rem", "Eq", "Neq", "Lt", "Le", "Gt", "Ge",
                "And", "Or", "Not", "BitAnd", "BitOr", "BitXor", "BitNot", "Shl", "Shr",
                "At", "Slice", "InsertAt", "ExtractFrom", "CopyFrom", "Append", "Prepend",
                // 2026-08-14 (UOL §6b): the iterable cursor ops were missing —
                // a vocab gap the generative OpName# dispatch needs filled.
                "Count", "Iter", "Step", "IsEnd", "Current",
                // 2026-08-15 (coll plan §3.6): the capacity intrinsics.
                "Capacity", "Resize", "EnsureCap", "TrimCap",
            ]),
            stages: ss(&[
                "PreLex", "Parsed", "Resolved", "Typed", "Normalized", "Verified",
                "Allocated", "Provenanced", "Generated", "Optimized", "Linked",
            ]),
            staged_features: ss(&[
                "dyn Trait", "const generics", "spawn/await handles",
                "rollback", "endprogram", "defer", "mutex", "barrier",
                ".f strict indentation", "generic semantic Value",
            ]),
            casing: vec![
                ("types".to_string(), Casing::PascalCase),
                ("traits".to_string(), Casing::PascalCase),
                ("structs".to_string(), Casing::PascalCase),
                ("enums".to_string(), Casing::PascalCase),
                ("objs".to_string(), Casing::PascalCase),
                ("cells".to_string(), Casing::PascalCase),
                ("protocol variants".to_string(), Casing::PascalCase),
                ("operation identities".to_string(), Casing::PascalCase),
                ("functions".to_string(), Casing::SnakeCase),
                ("fields".to_string(), Casing::SnakeCase),
                ("nodes".to_string(), Casing::SnakeCase),
                ("variables".to_string(), Casing::SnakeCase),
                ("macros".to_string(), Casing::SnakeCase),
                ("intrinsics".to_string(), Casing::PascalCaseHash),
            ],
        }
    }

    pub fn is_canonical_keyword(&self, name: &str) -> bool {
        self.keywords
            .iter()
            .any(|k| k.name == name && k.status == VocabStatus::Canonical)
    }

    pub fn is_removed_keyword(&self, name: &str) -> bool {
        self.keywords
            .iter()
            .any(|k| k.name == name && k.status == VocabStatus::Removed)
    }

    pub fn is_reserved(&self, name: &str) -> bool {
        self.keywords
            .iter()
            .any(|k| k.name == name && k.status == VocabStatus::Reserved)
    }

    pub fn keyword_status(&self, name: &str) -> Option<VocabStatus> {
        self.keywords.iter().find(|k| k.name == name).map(|k| k.status)
    }

    pub fn canonical_keywords(&self) -> impl Iterator<Item = &Keyword> {
        self.keywords.iter().filter(|k| k.status == VocabStatus::Canonical)
    }

    pub fn removed_keywords(&self) -> impl Iterator<Item = &Keyword> {
        self.keywords.iter().filter(|k| k.status == VocabStatus::Removed)
    }
}

/// 2026-08-05: serialize the canonical vocabulary for tooling consumption.
/// Emitted as TOML; the LSP/highlighter generators and CI parity tests read it.
pub fn serialize_vocab(vocab: &LanguageVocab) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(vocab)
}

/// 2026-08-05: regenerate the TextMate grammar keyword patterns from the
/// canonical vocab so the highlighter stops teaching removed syntax. Canonical
/// keywords become control/declaration keywords; intrinsics and bootstrap type
/// names are emitted as their own support categories. Removed/reserved words
/// are deliberately absent (they lex as ordinary identifiers).
pub fn regenerate_highlighter_grammar(grammar_path: &std::path::Path) -> Result<(), String> {
    let text = std::fs::read_to_string(grammar_path)
        .map_err(|e| format!("failed to read grammar '{}': {}", grammar_path.display(), e))?;
    let mut grammar: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse grammar JSON: {}", e))?;

    let vocab = LanguageVocab::canonical();
    let mut keywords: Vec<serde_json::Value> = Vec::new();

    let kw_pattern = |names: &[&str], scope: &str| {
        let joined = names.join("|");
        serde_json::json!({
            "name": scope,
            "match": format!("\\b({})\\b", joined)
        })
    };

    // Canonical declaration/statement/modifier/control keywords.
    let control: Vec<&str> = vocab
        .canonical_keywords()
        .map(|k| k.name.as_str())
        .filter(|n| *n != "true" && *n != "false" && *n != "cyc" && *n != "ms"
            && *n != "s" && *n != "min" && *n != "ns")
        .collect();
    keywords.push(kw_pattern(&control, "keyword.control.briev"));

    // Boolean and duration literals.
    keywords.push(kw_pattern(&["true", "false"], "constant.language.briev"));
    keywords.push(kw_pattern(&["cyc", "ns", "ms", "s", "min"], "keyword.other.time-unit.briev"));

    // Intrinsics: `Name#`.
    let intrinsics: Vec<&str> = vocab.intrinsics.iter().map(|s| s.as_str()).collect();
    if !intrinsics.is_empty() {
        let joined = intrinsics
            .iter()
            .map(|s| s.replace('#', "\\#"))
            .collect::<Vec<_>>()
            .join("|");
        keywords.push(serde_json::json!({
            "name": "support.function.intrinsic.briev",
            "match": format!("\\b({})\\b", joined)
        }));
    }

    // Bootstrap type names / primitive hashwords.
    let types: Vec<&str> = vec![
        "Int", "Float", "Bool", "String", "Char", "Void", "Ptr", "Bits",
    ];
    keywords.push(kw_pattern(&types, "support.type.primitive.briev"));

    // Guard: canonical keywords must be exact-lowercase; no uppercase aliases.
    assert!(
        control.iter().all(|k| *k == k.to_lowercase()),
        "highlighter regeneration: canonical keywords must be lowercase"
    );

    grammar["repository"]["keywords"]["patterns"] = serde_json::Value::Array(keywords);
    grammar["repository"]["types"]["patterns"] = serde_json::Value::Array(vec![
        serde_json::json!({
            "name": "support.type.primitive.briev",
            "match": format!("\\b({})\\b", types.join("|"))
        }),
        serde_json::json!({
            "name": "entity.name.type.custom.briev",
            "match": "\\b[A-Z][a-zA-Z0-9_]*\\b"
        }),
    ]);

    let out = serde_json::to_string_pretty(&grammar)
        .map_err(|e| format!("failed to serialize grammar: {}", e))?;
    std::fs::write(grammar_path, out + "\n")
        .map_err(|e| format!("failed to write grammar: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_keywords_are_lowercase() {
        let vocab = LanguageVocab::canonical();
        for kw in vocab.canonical_keywords() {
            assert_eq!(
                kw.name,
                kw.name.to_lowercase(),
                "canonical keyword '{}' must be lowercase (SPEC §4.1)",
                kw.name
            );
        }
    }

    #[test]
    fn no_duplicate_vocab_names() {
        let vocab = LanguageVocab::canonical();
        let mut seen = std::collections::HashSet::new();
        for kw in &vocab.keywords {
            assert!(seen.insert(&kw.name), "duplicate vocab keyword '{}'", kw.name);
        }
    }

    #[test]
    fn reserved_set_is_exactly_sed_pvt() {
        // 2026-08-25: `reg` promoted Reserved → Canonical (lowering pin).
        let vocab = LanguageVocab::canonical();
        let reserved: Vec<&str> = vocab
            .keywords
            .iter()
            .filter(|k| k.status == VocabStatus::Reserved)
            .map(|k| k.name.as_str())
            .collect();
        assert_eq!(reserved, vec!["sed", "pvt"]);
    }

    #[test]
    fn removed_surface_is_recorded() {
        let vocab = LanguageVocab::canonical();
        for name in ["sig", "state", "rstruct", "uni", "meld", "escape", "term!", "frgn!", "Ptr!"] {
            assert_eq!(
                vocab.keyword_status(name),
                Some(VocabStatus::Removed),
                "'{}' should be recorded as removed",
                name
            );
        }
    }

    #[test]
    fn vocab_serializes_and_round_trips() {
        let vocab = LanguageVocab::canonical();
        let text = serialize_vocab(&vocab).expect("vocab serialization must succeed");
        let parsed: LanguageVocab = toml::from_str(&text).expect("vocab round-trip must succeed");
        assert_eq!(parsed, vocab);
    }

    #[test]
    fn canonical_and_removed_are_disjoint() {
        let vocab = LanguageVocab::canonical();
        let mut canonical = std::collections::HashSet::new();
        let mut removed = std::collections::HashSet::new();
        for kw in &vocab.keywords {
            match kw.status {
                VocabStatus::Canonical => { canonical.insert(&kw.name); }
                VocabStatus::Removed => { removed.insert(&kw.name); }
                VocabStatus::Reserved => {}
            }
        }
        assert!(
            canonical.is_disjoint(&removed),
            "a name cannot be both canonical and removed"
        );
    }

    /// 2026-09-08: the code→doc completeness gate. Every canonical keyword
    /// (vocab) and every registered intrinsic (intrinsic_signatures.rs) MUST
    /// appear in docs/reference/MASTER-SYNTAX-REFERENCE.md. One-directional:
    /// new code breaks this test until the doc catches up; doc edits never do.
    /// The doc is included as a byte string so it can't silently drift from
    /// what the test actually checks.
    #[test]
    fn master_syntax_reference_covers_every_keyword_and_intrinsic() {
        let doc = include_str!("../docs/reference/MASTER-SYNTAX-REFERENCE.md");
        // A keyword/intrinsic is "covered" if it appears in the doc as a
        // word boundary (`  <name>` or `` `<name>` ``). Removed/reserved
        // surface is excluded — section 12 records it deliberately, and
        // section 1's `pvt`/`sed` table covers the reserved pair.
        let vocab = LanguageVocab::canonical();
        let mut missing: Vec<String> = Vec::new();
        for kw in &vocab.keywords {
            if kw.status != VocabStatus::Canonical {
                continue;
            }
            if !doc.contains(&format!("`{}`", kw.name))
                && !doc.contains(&format!("`{}", kw.name))
                && !doc.contains(&format!(" {} ", kw.name))
            {
                missing.push(format!("keyword `{}`", kw.name));
            }
        }
        for name in crate::intrinsic_signatures::REGISTERED_INTRINSICS {
            if !doc.contains(&format!("`{}`", name)) {
                missing.push(format!("intrinsic `{}`", name));
            }
        }
        assert!(
            missing.is_empty(),
            "MASTER-SYNTAX-REFERENCE.md is missing: {}",
            missing.join(", ")
        );
    }
}

// ── 2026-08-22 (spec-conformance plan Phase 2): did-you-mean support ─────
// SPEC §4.1: a wrong keyword spelling gives a suggested-correction error.
// These helpers power that hint; the parser consults them at declaration
// positions where a misspelled keyword otherwise dies as a generic
// "unexpected item" error.

/// Bounded Levenshtein edit distance. `None` once the distance provably
/// exceeds `max` (early-exit band so typo scanning stays cheap).
pub fn edit_distance_within(a: &str, b: &str, max: u8) -> Option<u8> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) as u8 > max {
        return None;
    }
    let mut prev: Vec<u16> = (0..=b.len() as u16).collect();
    let mut cur: Vec<u16> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i as u16 + 1;
        let mut row_min = cur[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = u16::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            row_min = row_min.min(cur[j + 1]);
        }
        if row_min > max as u16 {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let d = prev[b.len()];
    (d <= max as u16).then_some(d as u8)
}

/// The unique-closest candidate within `max_dist` of `input`, or `None`.
/// Deterministic: ties break by candidate order.
pub fn closest_keyword<'a>(input: &str, candidates: &[&'a str], max_dist: u8) -> Option<&'a str> {
    let mut best: Option<(&'a str, u8)> = None;
    for cand in candidates {
        if let Some(d) = edit_distance_within(input, cand, max_dist) {
            let better = match best {
                None => true,
                Some((_, bd)) => d < bd,
            };
            if better {
                best = Some((cand, d));
            }
        }
    }
    best.map(|(name, _)| name)
}

/// Full house-style hint line for a misspelled keyword, or `None`.
/// Removed and reserved words never fuzzy-suggest: they carry their own
/// diagnostics (removal notices, reserved-word errors), and a removed form
/// must not masquerade as a typo of an unrelated canonical keyword
/// (`meld` is distance-2 from `cell` — suggesting it would be absurd).
pub fn keyword_hint(vocab: &LanguageVocab, input: &str) -> Option<String> {
    if vocab.is_removed_keyword(input) || vocab.is_reserved(input) {
        return None;
    }
    let names: Vec<&str> = vocab.canonical_keywords().map(|k| k.name.as_str()).collect();
    // Distance 1 catches transposition-free typos (`nod`, `defn`); 2 catches
    // doubled/missing letters on longer words (`whn`, `matchh`) without
    // dragging unrelated short words into range.
    closest_keyword(input, &names, 2)
        .filter(|cand| cand.len() >= 3 || *cand == input)
        .map(|cand| format!("did you mean `{cand}`?"))
}

#[cfg(test)]
mod suggest_tests {
    use super::*;

    #[test]
    fn distance_bounds_respected() {
        assert_eq!(edit_distance_within("nod", "node", 2), Some(1));
        assert_eq!(edit_distance_within("xyzzy", "node", 2), None);
        assert_eq!(edit_distance_within("whn", "when", 2), Some(1));
    }

    #[test]
    fn closest_keyword_picks_minimum_and_is_deterministic() {
        let cands = ["node", "term", "foreach"];
        assert_eq!(closest_keyword("nod", &cands, 2), Some("node"));
        // Distance-2 deletion ("teeerm" → "term") is in range and correct.
        assert_eq!(closest_keyword("teeerm", &cands, 2), Some("term"));
        assert_eq!(closest_keyword("xyzzy", &cands, 2), None);
        assert_eq!(closest_keyword("forecah", &cands, 2), Some("foreach"));
    }

    #[test]
    fn vocab_hint_suggests_canonical_keywords_only() {
        let vocab = LanguageVocab::canonical();
        assert_eq!(
            keyword_hint(&vocab, "nod"),
            Some("did you mean `node`?".to_string())
        );
        // Removed forms must not be suggested as corrections — neither as
        // candidates (`meld` is distance-2 from `cell`) nor for removed
        // inputs themselves (they get their own removal diagnostic).
        assert_eq!(keyword_hint(&vocab, "meld"), None);
        assert_eq!(keyword_hint(&vocab, "celf"), Some("did you mean `cell`?".to_string()));
        assert_eq!(keyword_hint(&vocab, "counter"), None);
    }
}
