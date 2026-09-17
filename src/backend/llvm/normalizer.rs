// ── LLVM Normalizer — AST Annotation Pass ─────────────────────────────
//
// 2026-07-14: Walks the AST and attaches llvm_type to every type reference.
// Backend never reads config files or matches on primitive/bytes.
//
// 2026-07-20: Simplified for hashword protocol architecture.
// llvm_type is derived from structure only (fields + bytes), never from
// CTD, ALU, or TOML config. Types don't belong to categories — they
// interact with categories through hashword op signatures.

use crate::ast::*;
use crate::backend::normalizer;
use crate::backend::register_types::register_typedefs;
use crate::type_universe::TypeUniverse;
use std::collections::HashSet;

// 2026-07-30: Walks the AST and registers types in the TypeUniverse with
// their protocol membership and metadata. The casting graph resolves LLVM
// types from (protocol, metadata) at codegen time — no llvm_type needed.
// 2026-08-10: TypeDef registration + layout field attachment moved to the
// shared backend::register_types::register_typedefs (used by every backend).

/// Register type definitions in the universe and process meld declarations.
///
/// 2026-07-30: Protocol-driven type registration only. LLVM type resolution
/// is deferred to the casting graph's `resolve_llvm_type()`.
/// - Flexible types (Int, UInt, Bit) have no baked-in width — resolved per-target.
/// - Fixed-width types (Int32, Float) carry explicit !> bits metadata.
/// - Struct types derive LLVM type from field shapes at codegen time.
/// - Protocol (Cast.) properties are retained for graph-based membership checks.
/// - Explicit user `llvm <~` is validated against known LLVM type strings.
pub fn normalize(items: &mut Vec<TopLevel>, universe: &mut TypeUniverse, int_bits: u64) -> Result<(), String> {
    // ── Register all TopLevel::TypeDef items into the TypeUniverse ─────────
    // After registration, the casting graph resolves LLVM types from
    // (protocol, metadata) at codegen time — no llvm_type derivation needed.
    // 2026-07-31: Phase 3 (§8.6) — int_bits threaded through so a type with no
    // width metadata falls back to the TARGET default width, not a hardcoded 64.
    // 2026-08-10: shared backend::register_types::register_typedefs.
    register_typedefs(items, universe, int_bits)?;

    // 2026-08-09 (Phase 12, SPEC §19.6): the `meld` declaration pass is
    // removed — foreign shapes adapt through GLUE/Data Briev descriptors,
    // explicit protocol cast edges, ownership contracts, and effects.
    // (2026-07-16: P0+P6 — Process meld layout declarations in a single pass.)

    // Validate intrinsics against supported set
    let errors = normalizer::validate_intrinsics(items, &build_supported_ops());
    if !errors.is_empty() {
        return Err(format!("LLVM normalizer:\n  {}", errors.join("\n  ")));
    }

    // 2026-07-29: Validate that no TypeDef overrides non-overridable ops.
    // Bitwise operations (BitAnd, BitOr, BitXor, BitNot, Shl, Shr) are axioms
    // of the Bit protocol — they must never be semantically overloaded.
    // Parsing/lexing operations (Parse, Lex) are compile-time structural phases —
    // types inherit their parent protocol's parsing rules.
    let mut forbidden_names: HashSet<&str> = [
        "BitAnd", "BitOr", "BitXor", "BitNot", "Shl", "Shr",
    ].iter().cloned().collect();
    for item in items.iter() {
        if let TopLevel::TypeDef(td) = item {
            for op in &td.body.op_bindings {
                if forbidden_names.contains(op.name.as_str()) {
                    return Err(format!(
                        "{} '{}' cannot be overridden — it is an axiom of the Bit protocol",
                        op.name, td.name,
                    ));
                }
                if op.name == "Parse" || op.name == "Lex" {
                    return Err(format!(
                        "'{}' is a built-in compile-time operation and cannot be overridden (in type '{}')",
                        op.name, td.name,
                    ));
                }
            }
        }
    }

    // Strip metadata LLVM doesn't use
        // 2026-07-30: Keep Cast. properties for protocol membership,
        // plus width metadata (bits, maxbits, minbits) for resolve_llvm_type(),
        // and alignment for align_of().
        for rt in universe.types.values_mut() {
            rt.properties.retain(|k, _| {
                k.starts_with("Cast.")
                    || k == "bits" || k == "maxbits" || k == "minbits"
                    || k == "alignment"
            });
        }

    Ok(())
}

/// 2026-07-19: Validate that a user-provided LLVM type string is valid.
/// Returns Ok(()) if the type string is syntactically valid, Err with
/// a descriptive message otherwise.
fn validate_explicit_llvm(llvm_val: &str) -> Result<(), String> {
    match llvm_val {
        "half" | "bfloat" | "float" | "double" | "fp128"
        | "x86_fp80" | "ppc_fp128" => return Ok(()),
        _ => {}
    }
    if llvm_val.starts_with('i') {
        let bits = &llvm_val[1..];
        if bits.parse::<u64>().is_ok() {
            return Ok(());
        }
    }
    if llvm_val == "ptr" || llvm_val == "void" {
        return Ok(());
    }
    if llvm_val.starts_with('{') && llvm_val.ends_with('}') {
        return Ok(());
    }
    if llvm_val.starts_with('<') && llvm_val.contains(" x ") && llvm_val.ends_with('>') {
        return Ok(());
    }
    Err(format!(
        "invalid LLVM type '{}': expected a known LLVM type (float, double, half, bfloat, iN, ptr, ...)",
        llvm_val
    ))
}

fn build_supported_ops() -> HashSet<String> {
    let mut set = HashSet::new();
    for op_name in STANDARD_OPS {
        set.insert(format!("{}#", op_name));
    }
    for name in &["GetEnv#", "GetEnvInt#", "GetGlobalId#", "GetGlobalSize#", "GetLocalId#",
                   "ToInt#", "ToFloat#", "ToString#", "Concat#", "Length#",
                   "AddressOf#", "SysCall#", "SysConf#",
                   "Load#", "Store#", "Copy#", "Fill#",
                   // 2026-08-27 (Slice C): typed volatile MMIO access.
                   "VolatileLoad#", "VolatileStore#",
                   "AtomicLoad#", "AtomicStore#", "AtomicCas#", "AtomicXchg#",
                   "AtomicAdd#", "Fence#",
                   // 2026-09-10 (Family F, Asm#): the two-mode asm escape
                   // hatch — abstract ops lower through the lowering table,
                   // raw mode is dialect-specific text. Both emit inline asm.
                   "Asm#",
                   // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md):
                   // RMW family completion + width-parameterized access
                   // + pointer arithmetic.
                   "AtomicSub#", "AtomicOr#", "AtomicAnd#", "AtomicXor#",
                   "AtomicLoadN#", "AtomicStoreN#",
                   "PtrAdd#", "PtrSub#", "PtrDiff#", "PtrEq#", "PtrLt#",
                   // 2026-09-06 (plan 2026-09-06-cpp-expressiveness.md):
                   // portable SIMD — memory-to-memory element-wise family.
                   "SimdAdd#", "SimdSub#", "SimdMul#", "SimdFma#",
                   "DlOpen#", "DlSym#", "DlClose#",
                    "Backtrace#", "WorkgroupSize#",
                    // 2026-08-12 (Iterable protocol): `CharCount#` is the
                    // computed UTF8 char count intrinsic (SPEC §17.1) — the
                    // stdlib's string.text functions call it from imported
                    // modules. Whitelisted with the other computed intrinsics.
                    "CharCount#",
                    // 2026-08-01: The print/println macros expand to these
                    // direct runtime calls. They were previously invisible to
                    // this validator because the old PrintLn! always wrapped
                    // them in an Expr::Block, which the validator does not
                    // descend into; the bare println!() form exposes them.
                    "Print#",
                    // 2026-08-14 (UOL §6b): the generative collection-op
                    // intrinsic forms (`OpName#` → op-member dispatch). The
                    // normalizer must accept them so the generative codegen
                    // path is reachable; the typechecker validates the op is
                    // actually declared on the receiver.
                    "At#", "Slice#", "InsertAt#", "ExtractFrom#", "CopyFrom#",
                    "Append#", "Prepend#", "Count#", "Iter#", "Step#",
                    "IsEnd#", "Current#",
                    // 2026-08-15 (coll plan §3.6): the capacity intrinsics —
                    // compiler-owned coll capacity control (`Capacity#`,
                    // `Resize#`, `EnsureCap#`, `TrimCap#`).
                    "Capacity#", "Resize#", "EnsureCap#", "TrimCap#"] {
        set.insert(name.to_string());
    }
    set
}

/// Standard generic operation names (without the # suffix).
const STANDARD_OPS: &[&str] = &[
    "Add", "Sub", "Mul", "Div", "Rem",
    "Eq", "Neq", "Lt", "Gt", "Le", "Ge",
    "Neg", "Abs",
    "BitAnd", "BitOr", "BitXor", "Shl", "Shr",
    "Sqrt", "Sin", "Cos", "Fabs", "Ceil", "Floor", "Exp", "Fma", "Pow",
    "Print",
    "Malloc", "Free",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_explicit_llvm_float() {
        assert!(validate_explicit_llvm("float").is_ok());
    }

    #[test]
    fn test_validate_explicit_llvm_double() {
        assert!(validate_explicit_llvm("double").is_ok());
    }

    #[test]
    fn test_validate_explicit_llvm_invalid() {
        assert!(validate_explicit_llvm("not_a_type").is_err());
    }

    #[test]
    fn test_validate_explicit_llvm_i64() {
        assert!(validate_explicit_llvm("i64").is_ok());
    }

    #[test]
    fn test_validate_explicit_llvm_ptr() {
        assert!(validate_explicit_llvm("ptr").is_ok());
    }
}
