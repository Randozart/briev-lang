// 2026-07-25: Beastpack round-trip tests.
// Tests serialize → deserialize cycle for structural equivalence.

use crate::ast::*;
use crate::ast::top::*;
use crate::beastpack::{deserialize, serialize};
use crate::type_universe::TypeUniverse;
use std::collections::HashMap;

/// Build a minimal TypeUniverse with just "Int" registered.
fn test_universe() -> TypeUniverse {
    TypeUniverse::new()
}

/// Build a simple `defn add(a: Int, b: Int) -> Int { term a + b; }`.
fn test_add_defn() -> TopLevel {
    TopLevel::Definition(Definition {
        variadic_param: None,
        name: "add".into(),
        type_params: vec![],
        parameters: vec![
            ("a".into(), Type::Custom("Int".into())),
            ("b".into(), Type::Custom("Int".into())),
        ],
        output_type: Some(OutputType::Single(Type::Custom("Int".into()))),
        outputs: vec![Type::Custom("Int".into())],
        contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
        body: vec![Statement::Term(Some(Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("a".into())),
            Box::new(Expr::Identifier("b".into())),
        )))],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        annotations: vec![],
        span: None,
        doc: None,
    })
}

#[test]
fn test_roundtrip_simple_defn() {
    let items = vec![test_add_defn()];
    let universe = test_universe();

    let bytes = serialize(&items, &universe, 0);
    assert!(bytes.len() > 10 + 32, "beastpack should have header + checksum");

    let (restored, _) = deserialize(&bytes).unwrap();
    assert_eq!(items.len(), restored.len(), "same number of items");

    // Verify the restored definition is structurally correct
    match &restored[0] {
        TopLevel::Definition(d) => {
            assert_eq!(d.name, "add");
            assert_eq!(d.parameters.len(), 2);
            assert_eq!(d.parameters[0].0, "a");
            assert_eq!(d.parameters[1].0, "b");
            assert_eq!(d.body.len(), 1);
        }
        other => panic!("expected Definition, got {:?}", other),
    }
}

#[test]
fn test_roundtrip_with_metadata() {
    let mut meta = HashMap::new();
    meta.insert("Source$".into(), PropertyValue::String("let x = 1;".into()));
    meta.insert("Something".into(), PropertyValue::Int(42));

    let mut d = match test_add_defn() {
        TopLevel::Definition(d) => d,
        _ => unreachable!(),
    };
    d.metadata = meta;
    let items = vec![TopLevel::Definition(d)];
    let universe = test_universe();

    let bytes = serialize(&items, &universe, 0);
    let (restored, _) = deserialize(&bytes).unwrap();

    // Source$ should be stripped
    if let TopLevel::Definition(restored_d) = &restored[0] {
        assert!(
            !restored_d.metadata.contains_key("Source$"),
            "Source$ metadata must be stripped"
        );
        assert_eq!(
            restored_d.metadata.get("Something"),
            Some(&PropertyValue::Int(42)),
            "non-sensitive metadata must be preserved"
        );
    } else {
        panic!("expected Definition");
    }
}

#[test]
fn test_checksum_validation() {
    let items = vec![test_add_defn()];
    let universe = test_universe();

    let bytes = serialize(&items, &universe, 0);

    // Corrupt a byte in the data section
    let header_size = 10 + 4 + 8 + 4 + 4 + 8;
    let mut corrupted = bytes.clone();
    if corrupted.len() > header_size + 10 {
        corrupted[header_size + 5] ^= 0xFF;
    }

    let result = deserialize(&corrupted);
    assert!(result.is_err(), "corrupted beastpack must fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("checksum") || err.contains("corrupted"),
        "error must mention checksum, got: {}",
        err
    );
}

#[test]
fn test_version_mismatch() {
    let items = vec![test_add_defn()];
    let universe = test_universe();
    let mut bytes = serialize(&items, &universe, 0);

    // Corrupt the version field (byte 10-13) and recompute checksum
    // Need to preserve the correct checksum after modification
    let checksum_start = bytes.len() - 32;
    bytes[10] = 99;  // Set version to 99
    bytes[11] = 0;
    bytes[12] = 0;
    bytes[13] = 0;

    // Recompute checksum for the corrupted data
    let new_checksum = blake3::hash(&bytes[..checksum_start]);
    bytes[checksum_start..].copy_from_slice(new_checksum.as_bytes());

    let result = deserialize(&bytes);
    assert!(result.is_err(), "version mismatch must fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("version"),
        "error must mention version, got: {}",
        err
    );
}

#[test]
fn test_invalid_magic() {
    let items = vec![test_add_defn()];
    let universe = test_universe();
    let mut bytes = serialize(&items, &universe, 0);

    // Corrupt the magic
    bytes[0] = 0;

    let result = deserialize(&bytes);
    assert!(result.is_err(), "invalid magic must fail");
}

#[test]
fn test_roundtrip_preserves_type_universe() {
    let items = vec![test_add_defn()];
    let universe = test_universe();

    // Universe should have primordial types
    let type_count_before = universe.types.len();
    assert!(type_count_before > 0, "universe should have primordial types");

    let bytes = serialize(&items, &universe, 0);
    let (_, restored_universe) = deserialize(&bytes).unwrap();

    // Both universes should have the same types
    assert_eq!(universe.types.len(), restored_universe.types.len());

    // Check that critical primordial types are present in restored
    for name in &["Int", "Float", "Bool", "String", "Void"] {
        assert!(
            restored_universe.types.contains_key(*name),
            "restored universe missing type '{}'",
            name
        );
    }
}
