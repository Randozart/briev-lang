// ── AST Navigation Macros ───────────────────────────────────────────────
// 2026-07-21: Selection engine and navigation DSL for compile-time AST
// manipulation inside $(Stage) blocks.

pub mod actions;
pub mod audit;
pub mod compile_time;
pub mod diff;
pub mod eval;
pub mod ir_eval;
pub mod lockfile;
pub mod memcheck;
pub mod pattern_live;
pub mod selection;
pub mod stage_target;
pub mod text_ops;

pub use pattern_live::*;
pub use selection::*;
