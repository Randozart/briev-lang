// 2026-07-12: Phase 0.2 — New architecture AST module.
// Replaces the old src/ast.rs with a clean split.
// Key differences from the old AST:
// - No Intrinsic enum (use #-name dispatch via execute_intrinsic())
// - No Expr::IntrinsicCall (use Expr::Call with # suffix)
// - InopDeclaration and TopLevel::Inop removed (2026-07-22)
// - No "feature" types (BinaryOpExpr, CallExpr, etc.) — unified Expr variants only
// - Added TopLevel::Export for export defn
// - Added Statement::Guarded (already existed in old AST)
// 2026-08-01 (Phase 2): Contract.is_entry removed (the [#] entry marker is
// replaced by the entry!/args! macros in Phase 3).

mod display;
mod canonical;
mod expr;
pub mod top;
pub mod bad;
mod types;

pub use canonical::*;
pub use display::*;
pub use expr::*;
pub use top::*;
pub use types::*;
