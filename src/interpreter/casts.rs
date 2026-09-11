// ── Type Cast / Is / Like ──────────────────────────────────────────────
// 2026-07-12: Phase 3.5 — Rewritten for Bits-only Value.
// Type casts reinterpret Value::Bits bytes between type interpretations.
// IsType checks the byte width for type compatibility.

use crate::ast::{Expr, Type};
use crate::errors::RuntimeError;
use crate::interpreter::{Atom, f64_to_bits, i64_to_bits, Value};

/// 2026-08-06 (Slice B): Whether a value is a member of a target type.
/// Membership is decided by the value's semantic category, not by byte width:
/// an Int atom is a member of the Int family, a Float atom of the Float
/// family, raw Bits of `Bit`/String/Data, a `Product` of tuple/collection
/// types, a `Ref` of pointer types. `Union` is a member when any variant is.
/// Returns an Atom::Bool.
pub fn eval_is_type(val: &Value, target: &Type) -> Result<Value, RuntimeError> {
    let is_member = match target {
        Type::Custom(name) => match name.as_str() {
            "Int" | "UInt" | "Int64" | "UInt64" | "Int32" | "UInt32" | "Int16" | "UInt16"
            | "Int8" | "UInt8" => matches!(val, Value::Atom(Atom::Int(_))),
            "Float" | "Float64" | "Float32" | "Double" => matches!(val, Value::Atom(Atom::Float(_))),
            "Bool" => matches!(val, Value::Atom(Atom::Bool(_))),
            "Char" => matches!(val, Value::Atom(Atom::Char(_))),
            "String" | "Blob" => matches!(val, Value::Bits(_)),
            _ => false,
        },
        Type::Bits(_) => matches!(val, Value::Bits(_)),
        // 2026-09-11 (fundamentals doctrine, Phase A4): the Bit spellings
        // are retired; the Bits content view remains via Type::Bits.
        Type::Bits(_) => matches!(val, Value::Bits(_)),
        Type::Tuple(_) | Type::Applied(_, _) | Type::Generic(_, _) => {
            matches!(val, Value::Product { .. })
        }
        Type::Ptr(_) | Type::PtrConst(_) => matches!(val, Value::Ref(_)),
        Type::Void => matches!(val, Value::Void),
        Type::Union(variants) => variants
            .iter()
            .any(|t| eval_is_type(val, t).map_or(false, |r| r.as_bool().unwrap_or(false))),
        _ => false,
    };
    Ok(Value::Atom(Atom::Bool(is_member)))
}

/// Evaluate a type cast: reinterpret Value::Bits bytes between types.
pub fn eval_cast(val: Value, target: &Type) -> Result<Value, RuntimeError> {
    match target {
        Type::Custom(name) if name == "Float" || name == "Float64" => {
            let n = val.as_i64().unwrap_or(0);
            Ok(f64_to_bits(n as f64))
        }
        Type::Custom(name) if name == "Int" || name == "Int64" || name == "Bool" => {
            let bytes = val.bits_to_vec(8);
            Ok(Value::Bits(bytes))
        }
        Type::Custom(name) if name == "String" => {
            let s = bits_to_string(&val);
            Ok(Value::Bits(s.into_bytes()))
        }
        // 2026-09-11 (Phase A4): the Bit cast target is retired; casting to
        // Bits yields the bytes unchanged (the backend's ptrtoint content-view
        // cast is the address of those same bytes).
        Type::Bits(_) => Ok(val),
        Type::Custom(name) if name == "Char" => {
            let code = val.as_i64().unwrap_or(0) as u32;
            let ch = char::from_u32(code).unwrap_or('\0');
            Ok(Value::Bits((ch as u32).to_le_bytes().to_vec()))
        }
        _ => Ok(val),
    }
}

/// Structural equality check.
pub fn eval_like(lhs: &Value, rhs: &Value) -> Result<Value, RuntimeError> {
    let result = match (lhs, rhs) {
        (Value::Bits(a), Value::Bits(b)) => a == b,
        _ => false,
    };
    Ok(Value::Bits(vec![if result { 1u8 } else { 0u8 }]))
}

/// Convert value to Vec<u8> of given minimum size (padding with zeros).
fn value_bits_to_vec(v: &Value, min_len: usize) -> Vec<u8> {
    match v {
        Value::Bits(bytes) => {
            let mut result = bytes.clone();
            while result.len() < min_len {
                result.push(0);
            }
            result
        }
        _ => vec![0u8; min_len],
    }
}

/// Convert Bits to a human-readable string for casting.
fn bits_to_string(val: &Value) -> String {
    match val {
        Value::Bits(bytes) => {
            if bytes.len() == 8 {
                val.as_i64()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| String::from_utf8_lossy(bytes).to_string())
            } else {
                String::from_utf8_lossy(bytes).to_string()
            }
        }
        _ => String::new(),
    }
}

// Helper methods on Value for this module
trait ValueExt {
    fn bits_to_vec(&self, min_len: usize) -> Vec<u8>;
}

impl ValueExt for Value {
    fn bits_to_vec(&self, min_len: usize) -> Vec<u8> {
        value_bits_to_vec(self, min_len)
    }
}
