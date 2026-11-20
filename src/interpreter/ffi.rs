// ── FFI Dispatch ───────────────────────────────────────────────────────
// 2026-07-12: Phase 3.4 — Foreign function call dispatch.
//
// 2026-08-06 (Phase 17, Slice I): the interpreter's FFI boundary. This module
// is the ONLY place (besides the derive engine) that may construct Product and
// Sum values from named stdlib/foreign types. marshal_value/unmarshal_value
// are the representation conversion across the boundary; dispatch_ffi routes
// the interpreter-expressible foreign surface (the `#`-suffixed intrinsic
// family: Env*/Dl*/Http*/SysCall* and the generic numeric set) to
// execute_intrinsic. No interpreter eval path matches stdlib type names.

use crate::errors::RuntimeError;
use crate::interpreter::{Atom, Value, VirtualHeap, execute_intrinsic};

/// A marshalled FFI payload — the boundary representation of a Value.
#[derive(Debug, Clone, PartialEq)]
pub enum Marshalled {
    Int(i64),
    Float(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    Seq(Vec<Marshalled>),
    Sum(String, Vec<Marshalled>),
    Void,
}

/// Marshal a semantic value to its FFI payload. Atoms map to their C-ABI
/// form (Char → code point), Bits to raw bytes, Product/Sum to tagged
/// sequences, Ref to its pointee. Closures and void have no payload form.
pub fn marshal_value(v: &Value) -> Marshalled {
    match v {
        Value::Data(bits) => Marshalled::Int(*bits as i64),
        Value::Atom(Atom::Int(n)) => Marshalled::Int(*n),
        Value::Atom(Atom::Float(f)) => Marshalled::Float(*f),
        Value::Atom(Atom::Float32(f)) => Marshalled::Float(*f as f64),
        Value::Atom(Atom::Bool(b)) => Marshalled::Bool(*b),
        Value::Atom(Atom::Char(c)) => Marshalled::Int(*c as i64),
        Value::Bits(bytes) => Marshalled::Bytes(bytes.clone()),
        // 2026-08-22 (Phase 5b): a dyn value crosses FFI as its payload.
        Value::Dyn { inner, .. } => marshal_value(inner),
        // 2026-08-22 (Phase 7b): ports marshal their current payload;
        // instances marshal as a sequence of field values.
        Value::EventQ(q) => match &q.borrow().payload {
            Some(p) => marshal_value(p),
            None => Marshalled::Bytes(vec![]),
        },
        Value::Instance { fields, .. } => {
            Marshalled::Seq(fields.borrow().values().map(marshal_value).collect())
        }
        Value::Product { fields, .. } => {
            Marshalled::Seq(fields.iter().map(marshal_value).collect())
        }
        Value::Sum { name, payload } => {
            Marshalled::Sum(name.clone(), payload.iter().map(marshal_value).collect())
        }
        Value::Ref(inner) => marshal_value(inner),
        Value::Closure { .. } => Marshalled::Void,
        Value::Range { .. } => Marshalled::Void,
        Value::Void => Marshalled::Void,
    }
}

/// Unmarshal an FFI payload back into semantic values — constructs Product and
/// Sum at the boundary (the designated compound-construction point).
pub fn unmarshal_value(m: Marshalled) -> Value {
    match m {
        Marshalled::Int(n) => Value::int(n),
        Marshalled::Float(f) => Value::float(f),
        Marshalled::Bool(b) => Value::bool(b),
        Marshalled::Bytes(bytes) => Value::bits(bytes),
        Marshalled::Seq(items) => Value::product(items.into_iter().map(unmarshal_value).collect()),
        Marshalled::Sum(name, payload) => {
            Value::sum(name, payload.into_iter().map(unmarshal_value).collect())
        }
        Marshalled::Void => Value::Void,
    }
}

/// Dispatch a foreign function call. Args are marshalled at the boundary, then
/// the interpreter-expressible foreign surface (every `#`-suffixed intrinsic,
/// which the typechecker resolves to a native entry point) executes through
/// execute_intrinsic with a fresh boundary heap. Anything else is an
/// undefined foreign function.
pub fn dispatch_ffi(name: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    let _payload: Vec<Marshalled> = args.iter().map(marshal_value).collect();
    if name.ends_with('#') {
        let mut heap = VirtualHeap::new();
        execute_intrinsic(name, args, &mut heap)
    } else {
        Err(RuntimeError::UndefinedForeignFunction {
            name: name.to_string(),
            source: "FFI".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_marshal_unmarshal_atoms() {
        assert_eq!(
            unmarshal_value(marshal_value(&Value::int(7))),
            Value::int(7)
        );
        assert_eq!(
            unmarshal_value(marshal_value(&Value::float(1.5))),
            Value::float(1.5)
        );
        assert_eq!(
            unmarshal_value(marshal_value(&Value::bool(true))),
            Value::bool(true)
        );
    }

    #[test]
    fn test_marshal_char_maps_to_int_payload() {
        // Char crosses the C-ABI boundary as its code point (Int).
        assert_eq!(marshal_value(&Value::char('A')), Marshalled::Int(65));
        assert_eq!(
            unmarshal_value(marshal_value(&Value::char('A'))),
            Value::int(65)
        );
    }

    #[test]
    fn test_marshal_unmarshal_bits() {
        let v = Value::bits(b"payload".to_vec());
        assert_eq!(unmarshal_value(marshal_value(&v)), v);
    }

    #[test]
    fn test_marshal_unmarshal_product() {
        let v = Value::product(vec![
            Value::int(1),
            Value::bool(true),
            Value::bits(b"x".to_vec()),
        ]);
        assert_eq!(unmarshal_value(marshal_value(&v)), v);
    }

    #[test]
    fn test_marshal_unmarshal_sum() {
        let v = Value::sum("Option::Some".into(), vec![Value::int(9)]);
        assert_eq!(unmarshal_value(marshal_value(&v)), v);
    }

    #[test]
    fn test_dispatch_ffi_unknown_is_error() {
        let err = dispatch_ffi("no_such_ffi", &[]).err().unwrap().to_string();
        assert!(err.contains("foreign function"), "got: {err}");
    }

    #[test]
    fn test_dispatch_ffi_routes_intrinsic_surface() {
        let r = dispatch_ffi("Abs#", &[Value::int(-3)]).unwrap();
        assert_eq!(r.as_i64(), Some(3));
    }
}
