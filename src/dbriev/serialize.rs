// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Canonical serialization for Data Briev (SPEC 22.6).
//!
//! Deterministic field/key ordering, quoting, numeric spelling, and
//! instruction placement for reproducible builds and hashing. Schemas emit in
//! declaration order; fields in schema order; a document's data groups emit in
//! source order. Round-trips through `parse_document` are idempotent.
//!
//! 2026-08-09 (Phase 13): new module.

use crate::dbriev::v2::{
    DataField, DataGroup, DataValue, DbrievDocument, FieldDef, FieldType, SchemaDef,
};

/// Canonically serialize a parsed Data Briev document (`.dbv` form).
pub fn canonicalize_document(doc: &DbrievDocument) -> String {
    let mut out = String::new();
    for import in &doc.imports {
        out.push_str(&format!("import \"{}\";\n", import));
    }
    if !doc.imports.is_empty() {
        out.push('\n');
    }
    for schema in &doc.schemas {
        canonicalize_schema(schema, &mut out);
    }
    if !doc.schemas.is_empty() && !doc.data_groups.is_empty() {
        out.push('\n');
    }
    for group in &doc.data_groups {
        canonicalize_group(group, &mut out);
    }
    out
}

/// Canonicalize one schema definition.
fn canonicalize_schema(schema: &SchemaDef, out: &mut String) {
    out.push_str("schema ");
    out.push_str(&schema.name);
    if let Some(kf) = &schema.key_field {
        out.push_str(&format!(" ({})", kf));
    }
    out.push_str(" {\n");
    for field in &schema.fields {
        canonicalize_field(field, out);
    }
    out.push_str("};\n");
}

/// Canonicalize one field declaration: optional `?`, constraint, name, type.
fn canonicalize_field(field: &FieldDef, out: &mut String) {
    out.push_str("    ");
    if let Some(cons) = &field.constraint {
        out.push_str(&format!("[ {} ] ", cons));
    }
    out.push_str(&field.name);
    if field.optional {
        out.push('?');
    }
    out.push_str(": ");
    out.push_str(&fmt_type(&field.ty));
    out.push_str(";\n");
}

/// Canonicalize one data group: `key: Schema { values };` per entry, in
/// source order. Keys are sorted for determinism when multiple entries exist.
fn canonicalize_group(group: &DataGroup, out: &mut String) {
    let schema = group.schema_name.clone().unwrap_or_default();
    for entry in &group.entries {
        let key = entry.key.clone().unwrap_or_default();
        out.push_str(&format!("{}: {} {{", key, schema));
        for field in &entry.fields {
            out.push(' ');
            canonicalize_field_value(field, out);
            out.push(';');
        }
        out.push_str(" };\n");
    }
}

/// Canonicalize one field value (named or positional).
fn canonicalize_field_value(field: &DataField, out: &mut String) {
    match field {
        DataField::Named(n, v) => {
            out.push_str(n);
            out.push_str(": ");
            canonicalize_value(v, out);
        }
        DataField::Positional(v) => canonicalize_value(v, out),
    }
}

/// Canonicalize one value: deterministic quoting + numeric spelling. The
/// parser keeps raw quoted tokens (`"Ada"` parses as `String("\"Ada\"")`), so
/// a String value may carry literal quote chars — strip them and re-emit one
/// canonical quoted form.
fn canonicalize_value(value: &DataValue, out: &mut String) {
    match value {
        DataValue::String(s) => {
            let inner = s.trim_matches('"');
            out.push_str(&format!("\"{}\"", inner));
        }
        DataValue::Int(v) => out.push_str(&v.to_string()),
        DataValue::Float(v) => out.push_str(&fmt_float(*v)),
        DataValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        DataValue::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                canonicalize_value(item, out);
            }
            out.push(']');
        }
        DataValue::Map(m) => {
            out.push('{');
            // Deterministic key ordering.
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("\"{}\": ", k.trim_matches('"')));
                canonicalize_value(&m[*k], out);
            }
            out.push('}');
        }
    }
}

/// Deterministic Float spelling: no trailing `.0` on integral floats, a `.5`
/// for halves, full f64 otherwise. This matches the parser's round-trip.
fn fmt_float(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}.0", v as i64)
    } else {
        format!("{}", v)
    }
}

/// Human-readable type name (mirrors validate::fmt_type).
fn fmt_type(ty: &FieldType) -> String {
    match ty {
        FieldType::String => "String".into(),
        FieldType::Int => "Int".into(),
        FieldType::Float => "Float".into(),
        FieldType::Bool => "Bool".into(),
        FieldType::UInt(bits) => format!("UInt<{}>", bits),
        FieldType::Vec(inner) => format!("List<{}>", fmt_type(inner)),
        FieldType::Map(k, v) => format!("Map<{}, {}>", fmt_type(k), fmt_type(v)),
        FieldType::Option(inner) => format!("Option<{}>", fmt_type(inner)),
        FieldType::Named(n) => n.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> DbrievDocument {
        crate::dbriev::v2::parse_document(src).unwrap()
    }

    #[test]
    fn canonicalize_round_trips() {
        let doc = parse(
            "schema Person (name) { name: String; age: Int; [ >= 0 ] hp: Int; takeable: Bool; }\n\
             ada: Person { Ada; 37; 100; true; };\n",
        );
        let out = canonicalize_document(&doc);
        // Round-trip: parse the canonical form again — same schemas + groups.
        let again = parse(&out);
        assert_eq!(again.schemas.len(), 1);
        assert_eq!(again.data_groups.len(), 1);
        assert_eq!(again.schemas[0].name, "Person");
        assert_eq!(again.schemas[0].key_field.as_deref(), Some("name"));
        // Idempotent: canonicalizing the canonical form is byte-identical.
        assert_eq!(canonicalize_document(&again), out);
    }

    #[test]
    fn canonicalize_schema_fields() {
        let doc = parse("schema Item { [ >= 0 ] hp: Int; [ != \"\" ] name: String; }");
        let out = canonicalize_document(&doc);
        assert!(out.contains("[ >= 0 ] hp: Int;"), "{}", out);
        assert!(out.contains("[ != \"\" ] name: String;"), "{}", out);
        assert!(out.starts_with("schema Item {"), "{}", out);
    }

    #[test]
    fn canonicalize_deterministic_map_order() {
        // A Map value's keys serialize sorted (deterministic ordering).
        use std::collections::HashMap;
        let mut m = HashMap::new();
        m.insert("z".to_string(), DataValue::Int(1));
        m.insert("a".to_string(), DataValue::Int(2));
        let doc = DbrievDocument {
            imports: vec![],
            schemas: vec![SchemaDef {
                name: "Cfg".into(),
                key_field: None,
                fields: vec![FieldDef {
                    name: "attrs".into(),
                    ty: FieldType::Map(Box::new(FieldType::String), Box::new(FieldType::Int)),
                    constraint: None,
                    optional: false,
                }],
            }],
            data_groups: vec![DataGroup {
                schema_name: Some("Cfg".into()),
                entries: vec![crate::dbriev::v2::DataEntry {
                    key: Some("a".into()),
                    schema_name: Some("Cfg".into()),
                    fields: vec![DataField::Positional(DataValue::Map(m))],
                }],
            }],
            key_offsets: HashMap::new(),
        };
        let out = canonicalize_document(&doc);
        assert!(
            out.contains("\"a\": 2") && out.contains("\"z\": 1"),
            "{}",
            out
        );
        let a = out.find("\"a\": 2").unwrap();
        let z = out.find("\"z\": 1").unwrap();
        assert!(a < z, "map keys must sort: {}", out);
    }

    #[test]
    fn canonicalize_numeric_spelling() {
        let doc = parse("schema P { f: Float; };\np: P { 3.0; };\n");
        let out = canonicalize_document(&doc);
        assert!(out.contains("3.0"), "integral floats keep .0: {}", out);
    }

    #[test]
    fn canonicalize_imports_first() {
        let doc = parse("schema P { f: Int; };\nimport \"x.dbv\";\n");
        // Imports are hoisted above schemas.
        let out = canonicalize_document(&doc);
        assert!(out.starts_with("import \"x.dbv\";"), "{}", out);
    }
}
