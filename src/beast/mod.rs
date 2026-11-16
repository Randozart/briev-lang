// ── BEAST: Briev Virtual IR ──────────────────────────────────────────────
// 2026-07-14: S-expression format for the plugin mid-end.
// Serialize/deserialize between Rust AST and .beast text.

pub mod deserialize;
pub mod pattern;
pub mod serialize;
pub mod sexpr;

pub use deserialize::from_beast;
pub use serialize::to_beast;
