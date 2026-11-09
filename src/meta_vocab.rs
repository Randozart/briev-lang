// ── Metadata vocabulary (frontend view) ────────────────────────────────
// 2026-11-08 (attr plan): the language-level metadata vocabulary lives in
// `config/meta-vocab.dbv`. The backend already loads it per-LLVM-context for
// mapping lookups; the frontend needs the same table to validate
// `attr<owner>` keys, so this module exposes one process-wide instance.

use std::sync::OnceLock;

/// The process-wide metadata vocabulary (fields, owners, backend mappings).
///
/// Panics if the baked config is malformed — the `.dbv` file is a
/// compile-time invariant (`include_str!`).
pub fn vocab() -> &'static crate::backend::metadata::MetadataRegistry {
    static VOCAB: OnceLock<crate::backend::metadata::MetadataRegistry> = OnceLock::new();
    VOCAB.get_or_init(crate::backend::metadata::MetadataRegistry::load)
}
