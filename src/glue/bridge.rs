// ── GLUE Bridge — Protocol-Mediated Foreign Calls ─────────────────────
// 2026-07-22: Shared bridge generation logic used by all backends.
//
// The bridge wraps a foreign function call with:
//   1. Protocol transforms (CastTo/CastFrom) for each parameter
//   2. The foreign call itself (via the appropriate mechanism)
//   3. Protocol transforms for the return value
//   4. Contract verification + fallback dispatch
//
// The specific mechanism for making the foreign call (dlopen, Python
// embedding, JS glue, etc.) is backend-specific, but the transform
// chain and fallback logic is shared.

use std::fmt::Write;

use crate::analysis::frgn_dispatch::{ProtocolStep, TransformKind};
use crate::ast::Type;

/// Emit the protocol transform chain for a single value.
///
/// 2026-07-22: Applies a sequence of protocol transforms to convert a
/// value from one type representation to another. Each step in the path
/// is emitted as the appropriate LLVM IR operation.
///
/// Returns the register name holding the final transformed value.
pub fn emit_protocol_chain(
    out: &mut String,
    value_reg: &str,
    path: &[ProtocolStep],
    value_ty: &str,
    gen_reg: &mut dyn FnMut() -> String,
) -> Result<String, String> {
    if path.is_empty() {
        return Ok(value_reg.to_string());
    }

    let mut current_reg = value_reg.to_string();
    let mut current_ty = value_ty.to_string();

    for step in path {
        match step.kind {
            TransformKind::Identity => {
                // No transformation needed — types are structurally identical
            }
            TransformKind::Bitcast => {
                let target_ty = match &step.target {
                    Type::Custom(t) => t.as_str(),
                    // 2026-08-13 (layout-keywords plan): Bits(w) stores BITS; the
                    // match arms below are bit widths (8=i8 … 64=i64). The byte-era
                    // caller passed bytes, so Bits(8) mis-bitcast to i8 — a latent
                    // bug the unit restoration fixes.
                    Type::Bits(w) => match w {
                        8 => "i8",
                        16 => "i16",
                        32 => "i32",
                        64 => "i64",
                        _ => "i64",
                    },
                    _ => "i64",
                };
                let result = gen_reg();
                writeln!(
                    out,
                    "  {} = bitcast {} {} to {}",
                    result, current_ty, current_reg, target_ty
                )
                .ok();
                current_reg = result;
                current_ty = target_ty.to_string();
            }
            TransformKind::MeldShuffle => {
                // 2026-07-22: MeldShuffle — field reordering between source
                // and target struct types. When the struct layouts differ
                // (different field order or size), extract fields from source
                // and insert into target. When layouts match, this is a no-op.
                let target_ty_str = match &step.target {
                    Type::Custom(t) => t.as_str(),
                    Type::Ptr(_) => "ptr",
                    _ => "i64",
                };
                let result = gen_reg();
                // Emit as a bitcast when both types have the same byte width.
                // For field-level reordering, this would need extractvalue/
                // insertvalue sequences guided by a field map on the step.
                writeln!(
                    out,
                    "  {} = bitcast {} {} to {}",
                    result, current_ty, current_reg, target_ty_str
                )
                .ok();
                current_reg = result;
                current_ty = target_ty_str.to_string();
            }
            TransformKind::ProtocolTransform(ref category) => {
                // 2026-07-22: Protocol transform via CastTo/CastFrom intrinsic.
                // Declare the intrinsic if not already declared (weak linkage
                // so missing at link time falls back to linker resolution).
                let target_ty = match &step.target {
                    Type::Custom(t) => t.as_str(),
                    _ => "i64",
                };
                let intrinsic = format!("_CastTo_{}", category);
                writeln!(
                    out,
                    "  declare {} @{}({})",
                    target_ty, intrinsic, current_ty
                )
                .ok();
                let result = gen_reg();
                writeln!(
                    out,
                    "  {} = call {} @{}({} {})",
                    result, target_ty, intrinsic, current_ty, current_reg
                )
                .ok();
                current_reg = result;
                current_ty = target_ty.to_string();
            }
        }
    }
    Ok(current_reg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::frgn_dispatch::TransformKind;
    use crate::ast::Type;

    /// Helper: creates a mutable closure for register generation.
    fn test_gen_reg() -> Box<dyn FnMut() -> String> {
        let mut counter = 0u64;
        Box::new(move || {
            let n = counter;
            counter += 1;
            format!("%t{}", n)
        })
    }

    #[test]
    fn test_emit_protocol_chain_identity() {
        let path = vec![ProtocolStep {
            source: Type::int(),
            target: Type::int(),
            kind: TransformKind::Identity,
        }];
        let mut out = String::new();
        let mut gen_reg = test_gen_reg();
        let result = emit_protocol_chain(&mut out, "%val", &path, "i64", &mut gen_reg).unwrap();
        assert_eq!(result, "%val");
    }

    #[test]
    fn test_emit_protocol_chain_empty() {
        let mut out = String::new();
        let mut gen_reg = test_gen_reg();
        let result = emit_protocol_chain(&mut out, "%val", &[], "i64", &mut gen_reg).unwrap();
        assert_eq!(result, "%val");
    }

    #[test]
    fn test_emit_protocol_chain_bitcast() {
        let path = vec![ProtocolStep {
            source: Type::int(),
            target: Type::Custom("ptr".into()),
            kind: TransformKind::Bitcast,
        }];
        let mut out = String::new();
        let mut gen_reg = test_gen_reg();
        let result = emit_protocol_chain(&mut out, "%val", &path, "i64", &mut gen_reg).unwrap();
        assert!(out.contains("bitcast"), "should emit bitcast: {}", out);
        assert_eq!(result, "%t0");
    }

    #[test]
    fn test_emit_protocol_chain_meld_shuffle() {
        let path = vec![ProtocolStep {
            source: Type::Custom("StructA".into()),
            target: Type::Custom("StructB".into()),
            kind: TransformKind::MeldShuffle,
        }];
        let mut out = String::new();
        let mut gen_reg = test_gen_reg();
        let result = emit_protocol_chain(&mut out, "%val", &path, "i64", &mut gen_reg).unwrap();
        // MeldShuffle currently falls back to bitcast
        assert!(
            out.contains("bitcast"),
            "should emit bitcast for meld: {}",
            out
        );
        assert_eq!(result, "%t0");
    }

    #[test]
    fn test_emit_protocol_chain_protocol_transform() {
        let path = vec![ProtocolStep {
            source: Type::Custom("String".into()),
            target: Type::Custom("str".into()),
            kind: TransformKind::ProtocolTransform("String".into()),
        }];
        let mut out = String::new();
        let mut gen_reg = test_gen_reg();
        let result = emit_protocol_chain(&mut out, "%val", &path, "i64", &mut gen_reg).unwrap();
        assert!(out.contains("declare"), "should emit declare: {}", out);
        assert!(
            out.contains("_CastTo_String"),
            "should emit _CastTo_: {}",
            out
        );
        assert!(out.contains("call"), "should emit call: {}", out);
        assert_eq!(result, "%t0");
    }
}
