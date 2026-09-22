// ── Parser Module Entry Point ──────────────────────────────────────────
// 2026-07-12: Phase 1.0 — Parse pipeline entry, Parser struct, re-exports.
// The Parser struct is defined in helpers.rs; methods are added via impl blocks
// in the submodules. This file re-exports the public API.

pub mod bad;
mod definitions;
mod expressions;
mod helpers;
mod metadata;
mod statements;
mod types;

pub use bad::{parse_bad, BadParseError};
pub use definitions::*;
pub use expressions::*;
pub use helpers::Parser;
pub use metadata::*;
pub use statements::*;
pub use types::*;

