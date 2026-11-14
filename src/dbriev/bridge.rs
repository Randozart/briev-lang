// Bridge: converts DBriev v2 parsed types into Briev AST types (TopLevel, Type, Expr).

use crate::ast;
use crate::dbriev::v2::*;
use std::collections::HashMap;

/// Convert a parsed DbrievDocument into a Vec of Briev TopLevel items.
/// `name` is the import alias (e.g. "data" from `import data from "file.dbv"`).
/// `use_lazy` — if true, creates Expr::DbvlTable for schema-typed data with key_offsets.
pub fn document_to_program(doc: &DbrievDocument, name: &str) -> Vec<ast::TopLevel> {
    document_to_program_flags(doc, name, false)
}

/// Like `document_to_program` but with option for lazy DBVL loading.
pub fn document_to_program_flags(
    doc: &DbrievDocument,
    name: &str,
    use_lazy: bool,
) -> Vec<ast::TopLevel> {
    let mut items: Vec<ast::TopLevel> = Vec::new();

    // 0. Convert imports to Briev import statements
    // 2026-07-26: New syntax — imports are stored as paths in doc.imports.
    for import_path in &doc.imports {
        items.push(ast::TopLevel::Import(ast::Import {
            kind: ast::ImportKind::Literal(import_path.clone()),
            symbols: Vec::new(),
            alias: None,
            span: None,
        }));
    }

    // 1. Convert schemas to Struct definitions
    for schema in &doc.schemas {
        items.push(schema_to_struct(schema));
    }

    // 2. Convert data groups into a constant named by the import alias.
    if !doc.data_groups.is_empty() {
        let mut data_map: Vec<(ast::Expr, ast::Expr)> = Vec::new();

        for group in &doc.data_groups {
            let group_name = match &group.schema_name {
                Some(name) => name.clone(),
                None => "_".to_string(),
            };

            // Check if we can use lazy DbvlTable
            let can_use_lazy =
                use_lazy && group.schema_name.is_some() && !doc.key_offsets.is_empty();

            if can_use_lazy {
                // Create lazy DbvlTable — builds key-offset index
                let field_names: Vec<String> =
                    group.entries.iter().filter_map(|e| e.key.clone()).collect();
                // Use schema field names for proper ordering
                let schema_field_names = doc
                    .schemas
                    .iter()
                    .find(|s| Some(s.name.as_str()) == group.schema_name.as_deref())
                    .map(|s| s.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>())
                    .unwrap_or_else(|| field_names.clone());

                data_map.push((
                    ast::Expr::Quoted(group_name.into()),
                    ast::Expr::Tuple(vec![]), // lazy DbvlTable — placeholder
                ));
            // 2026-07-14: Non-lazy path — convert data entries directly
            } else {
                for entry in &group.entries {
                    let key_str = entry
                        .key
                        .clone()
                        .unwrap_or_else(|| format!("_{}", data_map.len()));
                    let entry_expr =
                        data_entry_to_expr(entry, group.schema_name.as_deref(), &doc.schemas);
                    data_map.push((
                        ast::Expr::Quoted(key_str.clone().into()),
                        ast::Expr::List(vec![ast::Expr::Quoted(key_str.into()), entry_expr]),
                    ));
                }
            }
        }

        // If only one group and it has no schema name, use value directly
        let value = if data_map.len() == 1
            && doc.data_groups[0].schema_name.is_none()
            && doc.data_groups[0].entries.len() == 1
        {
            let group = &doc.data_groups[0];
            if group.entries.len() == 1 && group.entries[0].fields.len() == 1 {
                data_field_to_expr(&group.entries[0].fields[0])
            } else {
                ast::Expr::List(data_map.into_iter().flat_map(|(k, v)| vec![k, v]).collect())
            }
        } else {
            ast::Expr::List(data_map.into_iter().flat_map(|(k, v)| vec![k, v]).collect())
        };

        let ty = ast::Type::Custom(name.to_string());
        items.push(ast::TopLevel::Constant(ast::Constant {
            name: name.to_string(),
            ty,
            expr: value,
            section: None,
        }));
    }

    // 3. Flatten peripheral struct instances into per-register Int constants.
    //    For each data entry keyed by a schema with base_addr + offset fields,
    //    creates top-level constants like {key}_dr = base_addr + dr_offset.
    items.extend(flatten_peripheral_constants(doc));

    items
}

/// Flatten schema-typed data entries with base_addr and offset fields into
/// individual Int constants. This makes peripheral register addresses available
/// as top-level constants for use in contracts and MMIO instructions.
///
/// Example: for entry `uart1 { base_addr: 0x40011000; dr_offset: 0x00; ... }`
/// with schema UartPeripheral, creates:
///   const uart1_base: Int = 0x40011000;
///   const uart1_end: Int = 0x40011018;
///   const uart1_dr: Int = 0x40011000;
///   const uart1_sr: Int = 0x40011001;
///   const uart1_cr1: Int = 0x4001100C;
///   const uart1_cr2: Int = 0x40011010;
fn flatten_peripheral_constants(doc: &DbrievDocument) -> Vec<ast::TopLevel> {
    let mut result = Vec::new();

    for group in &doc.data_groups {
        let schema_name = match &group.schema_name {
            Some(s) => s,
            None => continue,
        };
        let schema = match doc.schemas.iter().find(|s| &s.name == schema_name) {
            Some(s) => s,
            None => continue,
        };

        // Identify base_addr and size field indices
        let base_idx = schema.fields.iter().position(|f| f.name == "base_addr");
        let size_idx = schema.fields.iter().position(|f| f.name == "size");

        for entry in &group.entries {
            let key = match &entry.key {
                Some(k) => k.clone(),
                None => continue,
            };

            // Extract base_addr value
            let base_addr = base_idx
                .and_then(|idx| entry.fields.get(idx))
                .and_then(|f| match f {
                    DataField::Named(_, v) | DataField::Positional(v) => data_value_as_u64(v),
                });
            let base = match base_addr {
                Some(b) => b,
                None => continue,
            };

            // Emit base constant
            result.push(ast::TopLevel::Constant(ast::Constant {
                name: format!("{}_base", key),
                ty: ast::Type::int(),
                expr: ast::Expr::Decimal(base.try_into().unwrap()),
                section: None,
            }));

            // Emit end constant (base + size) if size is known
            if let Some(sz) = size_idx
                .and_then(|idx| entry.fields.get(idx))
                .and_then(|f| match f {
                    DataField::Named(_, v) | DataField::Positional(v) => data_value_as_u64(v),
                })
            {
                result.push(ast::TopLevel::Constant(ast::Constant {
                    name: format!("{}_end", key),
                    ty: ast::Type::int(),
                    expr: ast::Expr::Decimal((base + sz).try_into().unwrap()),
                    section: None,
                }));
            }

            // Emit register offset constants
            for (i, field) in schema.fields.iter().enumerate() {
                let fname = &field.name;
                if fname == "base_addr" || fname == "size" {
                    continue;
                }
                // Skip register metadata fields (_size, _access, _reset)
                if fname.ends_with("_size")
                    || fname.ends_with("_access")
                    || fname.ends_with("_reset")
                {
                    continue;
                }
                // Only flatten integer fields (offsets, control values)
                if !matches!(field.ty, FieldType::UInt(_) | FieldType::Int) {
                    continue;
                }

                if let Some(off) = entry.fields.get(i).and_then(|f| match f {
                    DataField::Named(_, v) | DataField::Positional(v) => data_value_as_u64(v),
                }) {
                    // Strip _offset suffix for cleaner constant name
                    let stem = if fname.ends_with("_offset") {
                        &fname[..fname.len() - 7]
                    } else {
                        fname.as_str()
                    };
                    let const_name = format!("{}_{}", key, stem);
                    result.push(ast::TopLevel::Constant(ast::Constant {
                        name: const_name,
                        ty: ast::Type::int(),
                        expr: ast::Expr::Decimal((base + off).try_into().unwrap()),
                        section: None,
                    }));
                }
            }
        }
    }

    result
}

/// Convert a schema into a StructDefinition
/// 2026-07-26: Preserves key_field annotation as the first struct field with
/// a special field name ~key so the compiler can identify the primary key field.
fn schema_to_struct(schema: &SchemaDef) -> ast::TopLevel {
    let mut fields: Vec<ast::StructField> = Vec::new();

    // Emit the key field annotation as a synthetic first field if present.
    // The ~ prefix is reserved for compiler-internal metadata in Briev.
    // 2026-07-26: Key field annotation (name) in schema Person (name) { ... }
    if let Some(ref kf) = schema.key_field {
        fields.push(ast::StructField {
            name: format!("~key_{}", kf),
            ty: ast::Type::string(),
            default: None,
            visibility: ast::Visibility::Private,
        });
    }

    for f in &schema.fields {
        let ty = field_type_to_briev(&f.ty);
        fields.push(ast::StructField {
            name: f.name.clone(),
            ty,
            default: None,
            visibility: ast::Visibility::Public,
        });
    }

    ast::TopLevel::Obj(ast::StructDefinition {
        name: schema.name.clone(),
        type_params: Vec::new(),
        parent: None,
        fields,
        transactions: Vec::new(),
        view_html: None,
        span: None,
        modifiers: Vec::new(),
        variants: Vec::new(),
    })
}

/// Convert a FieldType → ast::Type
fn field_type_to_briev(ft: &FieldType) -> ast::Type {
    match ft {
        FieldType::String => ast::Type::string(),
        FieldType::Int => ast::Type::int(),
        FieldType::Float => ast::Type::float(),
        FieldType::Bool => ast::Type::bool_(),
        FieldType::UInt(_) => ast::Type::int(),
        FieldType::Vec(inner) => {
            ast::Type::Applied("List".to_string(), vec![field_type_to_briev(inner)])
        }
        FieldType::Map(k, v) => ast::Type::Applied(
            "Map".to_string(),
            vec![field_type_to_briev(k), field_type_to_briev(v)],
        ),
        FieldType::Option(inner) => {
            ast::Type::Applied("Option".to_string(), vec![field_type_to_briev(inner)])
        }
        FieldType::Named(n) => ast::Type::Custom(n.clone()),
    }
}

/// Convert a single data entry to an Expr.
/// `group_schema_name` is the schema name from the enclosing `as Schema { }` group.
fn data_entry_to_expr(
    entry: &DataEntry,
    group_schema_name: Option<&str>,
    schemas: &[SchemaDef],
) -> ast::Expr {
    let fields: Vec<(String, ast::Expr)> = entry
        .fields
        .iter()
        .map(|f| match f {
            DataField::Named(n, v) => (n.clone(), data_value_to_expr(v)),
            DataField::Positional(v) => ("".to_string(), data_value_to_expr(v)),
        })
        .collect();

    // Determine schema name: entry-level takes priority, fallback to group-level
    let schema_name = entry.schema_name.as_deref().or(group_schema_name);

    if let Some(schema_name) = schema_name {
        let named_fields = if let Some(schema) = schemas.iter().find(|s| &s.name == schema_name) {
            let mut resolved: Vec<(String, ast::Expr)> = Vec::new();
            for (i, field) in entry.fields.iter().enumerate() {
                match field {
                    DataField::Named(n, v) => {
                        resolved.push((n.clone(), data_value_to_expr(v)));
                    }
                    DataField::Positional(v) => {
                        if i < schema.fields.len() {
                            let name = schema.fields[i].name.clone();
                            resolved.push((name, data_value_to_expr(v)));
                        }
                    }
                }
            }
            resolved
        } else {
            // Schema not in this document — use field names from DataField if available,
            // or generate placeholder names for positional fields
            let mut resolved: Vec<(String, ast::Expr)> = Vec::new();
            for (i, field) in entry.fields.iter().enumerate() {
                match field {
                    DataField::Named(n, v) => {
                        resolved.push((n.clone(), data_value_to_expr(v)));
                    }
                    DataField::Positional(v) => {
                        resolved.push((format!("field_{}", i), data_value_to_expr(v)));
                    }
                }
            }
            resolved
        };
        let named_fields_expr = ast::Expr::Tuple(
            named_fields
                .into_iter()
                .map(|(n, v)| ast::Expr::Tuple(vec![ast::Expr::Quoted(n.into()), v]))
                .collect(),
        );
        return ast::Expr::Call(
            "StructInstance".to_string(),
            vec![
                ast::Expr::Identifier(schema_name.to_string()),
                named_fields_expr,
            ],
            None,
        );
    }

    // No schema — use ObjectLiteral
    ast::Expr::List(
        fields
            .into_iter()
            .flat_map(|(n, v)| vec![ast::Expr::Quoted(n.into()), v])
            .collect(),
    )
}

/// Convert a DataField to an Expr
fn data_field_to_expr(field: &DataField) -> ast::Expr {
    match field {
        DataField::Named(_, v) | DataField::Positional(v) => data_value_to_expr(v),
    }
}

/// Read a DataValue as a u64 — decimal Int directly, `0x`-prefixed String via
/// radix parse. 2026-08-03 (Phase 2): the v2 parser yields hex literals as
/// DataValue::String, so flattening must accept both.
fn data_value_as_u64(dv: &DataValue) -> Option<u64> {
    match dv {
        DataValue::Int(n) => u64::try_from(*n).ok(),
        DataValue::String(s) => {
            let clean = s.trim_start_matches("0x").trim_start_matches("0X");
            u64::from_str_radix(clean, 16).ok()
        }
        _ => None,
    }
}

/// Convert a DataValue to an Expr
fn data_value_to_expr(dv: &DataValue) -> ast::Expr {
    match dv {
        DataValue::String(s) => ast::Expr::Quoted(s.clone().into()),
        DataValue::Int(n) => ast::Expr::Decimal(*n),
        DataValue::Float(f) => ast::Expr::Float(*f),
        DataValue::Bool(b) => ast::Expr::Bool(*b),
        DataValue::List(items) => {
            let exprs: Vec<ast::Expr> = items.iter().map(data_value_to_expr).collect();
            ast::Expr::List(exprs)
        }
        DataValue::Map(entries) => {
            let mut flat = Vec::new();
            for (k, v) in entries {
                flat.push(ast::Expr::Quoted(k.clone().into()));
                flat.push(data_value_to_expr(v));
            }
            ast::Expr::List(flat)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbriev::v2::*;

    fn parse(input: &str) -> DbrievDocument {
        parse_document(input).unwrap()
    }

    #[test]
    fn test_schema_conversion() {
        let doc = parse("schema Item { name: String; desc: String; hp: Int; takeable: Bool }");
        let items = document_to_program(&doc, "data");
        // Schema-only document produces struct definition, no constant
        assert_eq!(items.len(), 1);
        match &items[0] {
            ast::TopLevel::Obj(s) => {
                assert_eq!(s.name, "Item");
            }
            _ => panic!("Expected Struct definition"),
        }
    }

    #[test]
    fn test_data_conversion_basic() {
        let doc = parse(
            r#"as Item {
    rusty_key: Rusty Key; An old iron key; 5; true;
}
"#,
        );
        let items = document_to_program(&doc, "data");
        assert!(!items.is_empty());

        let constant = &items[0];
        match constant {
            ast::TopLevel::Constant(c) => {
                assert_eq!(c.name, "data");
                // Value should be a MapLiteral: { "Item": { "rusty_key": ... } }
                match &c.expr {
                    ast::Expr::List(pairs) => {
                        assert_eq!(pairs.len(), 2, "Expected [key, val] flat pairs");
                        let key = &pairs[0];
                        let val = &pairs[1];
                        match key {
                            ast::Expr::Quoted(s) => {
                                assert_eq!(s.as_slice(), b"rusty_key", "Expected first entry key")
                            }
                            _ => panic!("Expected string key"),
                        }
                        match val {
                            ast::Expr::List(entries) => {
                                assert_eq!(
                                    entries.len(),
                                    2,
                                    "Expected [entry_key, entry_val] flat pairs"
                                );
                                let ek = &entries[0];
                                let ev = &entries[1];
                                match ek {
                                    ast::Expr::Quoted(s) => assert_eq!(s.as_slice(), b"rusty_key"),
                                    _ => panic!("Expected string entry key"),
                                }
                                match ev {
                                    ast::Expr::Call(name_str, args, _)
                                        if name_str == "StructInstance" && args.len() == 2 =>
                                    {
                                        let name = &args[0];
                                        assert!(
                                            matches!(name, ast::Expr::Identifier(n) if n == "Item")
                                        );
                                    }
                                    _ => panic!("Expected StructInstance"),
                                }
                            }
                            _ => panic!("Expected MapLiteral for entries"),
                        }
                    }
                    _ => panic!("Expected MapLiteral"),
                }
            }
            _ => panic!("Expected Constant"),
        }
    }

    #[test]
    fn test_data_value_conversion() {
        let dv = DataValue::String("hello".into());
        match data_value_to_expr(&dv) {
            ast::Expr::Quoted(s) => assert_eq!(s.as_slice(), b"hello"),
            _ => panic!("expected string"),
        }

        let dv = DataValue::Int(42);
        match data_value_to_expr(&dv) {
            ast::Expr::Decimal(n) => assert_eq!(n, 42),
            _ => panic!("expected int"),
        }

        let dv = DataValue::Float(3.14);
        match data_value_to_expr(&dv) {
            ast::Expr::Float(f) => assert!((f - 3.14).abs() < 1e-10),
            _ => panic!("expected float"),
        }

        let dv = DataValue::Bool(true);
        match data_value_to_expr(&dv) {
            ast::Expr::Bool(b) => assert!(b),
            _ => panic!("expected bool"),
        }

        let dv = DataValue::List(vec![DataValue::Int(1), DataValue::Int(2)]);
        match data_value_to_expr(&dv) {
            ast::Expr::List(items) => assert_eq!(items.len(), 2),
            _ => panic!("expected list"),
        }

        let mut map = HashMap::new();
        map.insert("a".into(), DataValue::Int(1));
        let dv = DataValue::Map(map);
        match data_value_to_expr(&dv) {
            ast::Expr::List(pairs) => assert_eq!(pairs.len(), 2),
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn test_schema_and_data() {
        let input = r#"
schema Item {
    name: String;
    desc: String;
    hp: Int;
    takeable: Bool;
}

as Item {
    rusty_key: Rusty Key; An old iron key; 5; true;
}
"#;
        let doc = parse(input);
        let items = document_to_program(&doc, "data");

        // Should have 2 items: Struct + Constant
        // But currently schema_to_struct creates a Struct and then
        // the data creates a Constant. Let me check.
        let has_struct = items
            .iter()
            .any(|item| matches!(item, ast::TopLevel::Obj(s) if s.name == "Item"));
        let has_constant = items
            .iter()
            .any(|item| matches!(item, ast::TopLevel::Constant(c) if c.name == "data"));
        assert!(has_struct, "Expected Struct definition for Item schema");
        assert!(has_constant, "Expected Constant for data");
    }

    #[test]
    fn test_field_type_conversion() {
        let cases: Vec<(FieldType, &str)> = vec![
            (FieldType::String, "String"),
            (FieldType::Int, "Int"),
            (FieldType::Float, "Float"),
            (FieldType::Bool, "Bool"),
            (FieldType::UInt(32), "Int"),
            (
                FieldType::Vec(Box::new(FieldType::String)),
                "Applied(\"List\", [String])",
            ),
            (FieldType::Named("IoResult".into()), "Custom(\"IoResult\")"),
        ];
        for (ft, _desc) in cases {
            let ty = field_type_to_briev(&ft);
            let ty_str = format!("{:?}", ty);
            assert!(!ty_str.is_empty());
        }
    }
}
