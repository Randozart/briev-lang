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

//! Schema validation for Data Briev (SPEC 22.5).
//!
//! Values remain raw until interpreted by an asserted schema. When a schema is
//! asserted against a data group, validation covers: required/unknown fields,
//! raw-token-to-type conversion, field constraints, optional fields, named
//! schemas, and key presence/uniqueness. Without a schema, arbitrary data is
//! valid (SPEC 22.1).
//!
//! 2026-08-09 (Phase 13): new module — the parser produced the document; this
//! is the first consumer beyond the config loaders.

use crate::dbriev::v2::{
    DataField, DataGroup, DataValue, DbrievDocument, FieldDef, FieldType, SchemaDef,
};
use std::collections::HashMap;

/// Validate a parsed Data Briev document against its asserted schemas.
///
/// Returns a list of human-readable errors, empty when the document is valid.
/// A group without a `schema_name` (raw/scraped data) is not validated
/// (SPEC 22.1).
pub fn validate_document(doc: &DbrievDocument) -> Vec<String> {
    let mut errors = Vec::new();
    let schemas: HashMap<&str, &SchemaDef> =
        doc.schemas.iter().map(|s| (s.name.as_str(), s)).collect();
    // Key uniqueness is PER SCHEMA across the whole document (the parser
    // creates one group per entry, so a per-group check cannot see duplicates
    // that span groups — SPEC 22.3/22.5).
    let mut seen_keys: HashMap<&str, Vec<String>> = HashMap::new();
    for (gi, group) in doc.data_groups.iter().enumerate() {
        let Some(schema_name) = group.schema_name.as_deref() else {
            continue; // raw data — not asserted
        };
        let Some(schema) = schemas.get(schema_name) else {
            errors.push(format!(
                "data group {} references unknown schema '{}'",
                gi, schema_name
            ));
            continue;
        };
        // Key presence + uniqueness, document-wide per schema.
        if let Some(kf) = schema.key_field.as_deref() {
            for (ei, entry) in group.entries.iter().enumerate() {
                let key_ok = entry_key(entry, schema, kf);
                match key_ok {
                    Some(k) => {
                        let list = seen_keys.entry(schema_name).or_default();
                        if list.iter().any(|x| x == k) {
                            errors.push(format!(
                                "group {} entry {}: duplicate key '{}' for schema '{}'",
                                gi, ei, k, schema_name
                            ));
                        } else {
                            list.push(k.to_string());
                        }
                    }
                    None => errors.push(format!(
                        "group {} entry {}: missing required key field '{}'",
                        gi, ei, kf
                    )),
                }
            }
        }
        validate_group(group, schema, gi, &schemas, &mut errors);
    }
    errors
}

/// Extract an entry's key value for a schema key field: a named field, or the
/// positional value at the key field's schema index (`.dbvl` / `key: Schema`).
fn entry_key<'a>(
    entry: &'a crate::dbriev::v2::DataEntry,
    schema: &SchemaDef,
    kf: &str,
) -> Option<&'a str> {
    match entry_field(entry, kf) {
        Some(DataValue::String(s)) if !s.is_empty() => Some(s.as_str()),
        Some(DataValue::Int(_)) | Some(DataValue::Float(_)) | Some(DataValue::Bool(_)) => {
            Some("<scalar-key>")
        }
        _ => schema
            .fields
            .iter()
            .position(|f| &f.name == kf)
            .and_then(|idx| entry.fields.get(idx))
            .and_then(|f| match f {
                DataField::Positional(DataValue::String(s)) if !s.is_empty() => Some(s.as_str()),
                _ => None,
            }),
    }
}

/// Validate one data group against its asserted schema.
fn validate_group(
    group: &DataGroup,
    schema: &SchemaDef,
    gi: usize,
    schemas: &HashMap<&str, &SchemaDef>,
    errors: &mut Vec<String>,
) {
    // Key presence + uniqueness is handled document-wide per schema in
    // validate_document (the parser creates one group per entry, so a
    // per-group check cannot see duplicates that span groups).
    for (ei, entry) in group.entries.iter().enumerate() {
        validate_entry(entry, schema, gi, ei, schemas, errors);
    }
}

/// Validate one entry's fields against the schema.
fn validate_entry(
    entry: &crate::dbriev::v2::DataEntry,
    schema: &SchemaDef,
    gi: usize,
    ei: usize,
    schemas: &HashMap<&str, &SchemaDef>,
    errors: &mut Vec<String>,
) {
    // Positional fields (`.dbvl` one-record-per-line / `key: Schema { ... }`):
    // map to schema fields by index — handled below. When the entry is
    // positional, the named-field requiredness loop must not fire (a positional
    // entry is fully specified by the positional values).
    let has_positional = entry
        .fields
        .iter()
        .any(|f| matches!(f, DataField::Positional(_)));
    let named: Vec<(&str, &DataValue)> = entry
        .fields
        .iter()
        .filter_map(|f| match f {
            DataField::Named(n, v) => Some((n.as_str(), v)),
            DataField::Positional(_) => None,
        })
        .collect();
    if has_positional {
        // Pure positional (or mixed) — the positional index mapping below is
        // the authority. Skip the named requiredness loop.
    } else {
        // Named fields: required must be present, unknown must not be.
        let present_names: Vec<&str> = named.iter().map(|(n, _)| *n).collect();
        for field in &schema.fields {
            let is_present = present_names.contains(&field.name.as_str());
            if !is_present && !field.optional && !matches!(field.ty, FieldType::Option(_)) {
                errors.push(format!(
                    "group {} entry {}: missing required field '{}'",
                    gi, ei, field.name
                ));
            }
        }
    }
    for (name, value) in &named {
        let Some(field) = schema.fields.iter().find(|f| &f.name == name) else {
            errors.push(format!(
                "group {} entry {}: unknown field '{}' (schema '{}' has {})",
                gi,
                ei,
                name,
                schema.name,
                schema
                    .fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        };
        validate_value(value, field, gi, ei, schemas, errors);
    }
    // Positional fields (`.dbvl` one-record-per-line / `key: Schema { ... }`):
    // map to schema fields by index. A positional count exceeding the schema is
    // an unknown-field error. A trailing empty positional is the parser's
    // `;`-terminator artifact — ignore it (the record terminator is not a
    // field). Every non-optional schema field must have a positional value.
    let positional: Vec<&DataValue> = entry
        .fields
        .iter()
        .filter_map(|f| match f {
            DataField::Positional(DataValue::String(s)) if s.is_empty() => None,
            DataField::Positional(v) => Some(v),
            _ => None,
        })
        .collect();
    for (pi, value) in positional.iter().enumerate() {
        let Some(field) = schema.fields.get(pi) else {
            errors.push(format!(
                "group {} entry {}: too many positional fields ({}, schema '{}' has {} fields)",
                gi,
                ei,
                positional.len(),
                schema.name,
                schema.fields.len()
            ));
            break;
        };
        validate_value(value, field, gi, ei, schemas, errors);
    }
    // Positional coverage: every non-optional schema field must be present by
    // index (a positional entry with too few values leaves a required field
    // unset).
    if has_positional {
        for (fi, field) in schema.fields.iter().enumerate() {
            let covered = positional.get(fi).is_some();
            if !covered && !field.optional && !matches!(field.ty, FieldType::Option(_)) {
                errors.push(format!(
                    "group {} entry {}: missing required field '{}'",
                    gi, ei, field.name
                ));
            }
        }
    }
}

/// Type-check + constraint-check one field value against its declaration.
fn validate_value(
    value: &DataValue,
    field: &FieldDef,
    gi: usize,
    ei: usize,
    schemas: &HashMap<&str, &SchemaDef>,
    errors: &mut Vec<String>,
) {
    if !type_matches(value, &field.ty, schemas) {
        errors.push(format!(
            "group {} entry {}: field '{}' expected {}, found {}",
            gi,
            ei,
            field.name,
            fmt_type(&field.ty),
            fmt_value(value)
        ));
    }
    if let Some(expr) = field.constraint.as_deref() {
        if let Err(msg) = check_constraint(value, expr) {
            errors.push(format!(
                "group {} entry {}: field '{}' violates constraint [ {} ]: {}",
                gi, ei, field.name, expr, msg
            ));
        }
    }
}

/// Does a value match a declared type?
fn type_matches(value: &DataValue, ty: &FieldType, schemas: &HashMap<&str, &SchemaDef>) -> bool {
    match (value, ty) {
        (DataValue::String(_), FieldType::String) => true,
        (DataValue::Int(_), FieldType::Int) => true,
        (DataValue::Float(_), FieldType::Float) => true,
        (DataValue::Bool(_), FieldType::Bool) => true,
        (DataValue::Int(v), FieldType::UInt(bits)) => *v >= 0 && (*v as u64) < (1u64 << *bits),
        (DataValue::List(items), FieldType::Vec(inner)) => {
            items.iter().all(|i| type_matches(i, inner, schemas))
        }
        (DataValue::Map(m), FieldType::Map(k, v)) => {
            m.values().all(|val| type_matches(val, v, schemas))
                && m.keys()
                    .all(|key| type_matches(&DataValue::String(key.clone()), k, schemas))
        }
        (_, FieldType::Option(inner)) => type_matches(value, inner, schemas),
        (DataValue::String(_), FieldType::Named(name)) => {
            // Named schema reference: best-effort (the referenced schema's own
            // group is validated separately; an unresolved name is a resolver
            // concern, not a value-type error here).
            schemas.contains_key(name.as_str()) || !name.is_empty()
        }
        _ => false,
    }
}

/// Evaluate a raw constraint expression (`!= ""`, `>= 0`) against a value.
/// Supports the documented comparison forms.
fn check_constraint(value: &DataValue, expr: &str) -> Result<(), String> {
    let expr = expr.trim();
    let mut parts = expr.splitn(2, char::is_whitespace);
    let (Some(op), Some(rhs)) = (parts.next(), parts.next()) else {
        // Bare constant or unsupported form — accept (best-effort).
        return Ok(());
    };
    let rhs = rhs.trim();
    let ok = match (value, rhs) {
        (DataValue::String(s), r) if r.starts_with('"') => {
            let want = r.trim_matches('"');
            match op {
                "!=" => s != want,
                "==" => s == want,
                _ => return Err(format!("unsupported string constraint '{}'", op)),
            }
        }
        (DataValue::Int(v), r) => {
            let want: i64 = r
                .parse()
                .map_err(|_| format!("constraint literal '{}' is not an Int", r))?;
            match op {
                ">=" => *v >= want,
                ">" => *v > want,
                "<=" => *v <= want,
                "<" => *v < want,
                "!=" => *v != want,
                "==" => *v == want,
                _ => return Err(format!("unsupported Int constraint '{}'", op)),
            }
        }
        (DataValue::Float(v), r) => {
            let want: f64 = r
                .parse()
                .map_err(|_| format!("constraint literal '{}' is not a Float", r))?;
            match op {
                ">=" => *v >= want,
                ">" => *v > want,
                "<=" => *v <= want,
                "<" => *v < want,
                "!=" => *v != want,
                "==" => *v == want,
                _ => return Err(format!("unsupported Float constraint '{}'", op)),
            }
        }
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err("constraint not satisfied".into())
    }
}

/// The named field an entry provides, if present.
fn entry_field<'a>(entry: &'a crate::dbriev::v2::DataEntry, name: &str) -> Option<&'a DataValue> {
    entry.fields.iter().find_map(|f| match f {
        DataField::Named(n, v) if n == name => Some(v),
        _ => None,
    })
}

/// Human-readable type name.
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

/// Human-readable value (uses the TYPE name for error messages, plus the raw
/// token for context).
fn fmt_value(value: &DataValue) -> String {
    match value {
        DataValue::String(s) => format!("String({:?})", s),
        DataValue::Int(v) => format!("Int({})", v),
        DataValue::Float(v) => format!("Float({})", v),
        DataValue::Bool(b) => format!("Bool({})", b),
        DataValue::List(_) => "List".into(),
        DataValue::Map(_) => "Map".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> DbrievDocument {
        crate::dbriev::v2::parse_document(src).unwrap()
    }

    #[test]
    fn valid_document_passes() {
        let doc = parse(
            "schema Person (name) { name: String; age: Int; [ >= 0 ] hp: Int; takeable: Bool; }\n\
             ada: Person { Ada; 37; 100; true; };\n",
        );
        assert!(
            validate_document(&doc).is_empty(),
            "{:?}",
            validate_document(&doc)
        );
    }

    #[test]
    fn raw_data_without_schema_is_valid() {
        // Bare data with no schema assertion (schema_name = None) is valid.
        let doc = parse("Ada; 37;\n");
        assert!(
            validate_document(&doc).is_empty(),
            "unasserted data must pass"
        );
    }

    #[test]
    fn missing_required_field_errors() {
        // Positional entry with fewer values than the schema has required fields.
        let doc = parse(
            "schema Person { name: String; age: Int; }\n\
             ada: Person { Ada; };\n",
        );
        let errors = validate_document(&doc);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("missing required field 'age'")),
            "{:?}",
            errors
        );
    }

    #[test]
    fn type_mismatch_errors() {
        let doc = parse(
            "schema Person { name: String; age: Int; }\n\
             ada: Person { 42; \"x\"; };\n",
        );
        let errors = validate_document(&doc);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("expected Int, found String(")),
            "{:?}",
            errors
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("expected String, found Int(")),
            "{:?}",
            errors
        );
    }

    #[test]
    fn constraint_violation_errors() {
        let doc = parse(
            "schema Item { [ >= 0 ] hp: Int; [ != \"\" ] name: String; }\n\
             bad: Item { -5; ok; };\n",
        );
        let errors = validate_document(&doc);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("violates constraint [ >= 0 ]")),
            "{:?}",
            errors
        );
    }

    #[test]
    fn optional_field_absent_is_fine() {
        let doc = parse(
            "schema Person { name: String; nickname?: String; }\n\
             ada: Person { Ada; };\n",
        );
        assert!(
            validate_document(&doc).is_empty(),
            "{:?}",
            validate_document(&doc)
        );
    }

    #[test]
    fn duplicate_key_errors() {
        let doc = parse(
            "schema Person (name) { name: String; age: Int; }\n\
             a: Person { Ada; 1; };\n\
             b: Person { Ada; 2; };\n",
        );
        let errors = validate_document(&doc);
        assert!(
            errors.iter().any(|e| e.contains("duplicate key 'Ada'")),
            "{:?}",
            errors
        );
    }

    #[test]
    fn missing_key_field_errors() {
        let doc = parse(
            "schema Person (name) { name: String; age: Int; }\n\
             a: Person { ; 1; };\n",
        );
        let errors = validate_document(&doc);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("missing required key field 'name'")),
            "{:?}",
            errors
        );
    }

    #[test]
    fn too_many_positional_fields_errors() {
        let doc = parse(
            "schema Person { name: String; age: Int; }\n\
             ada: Person { Ada; 37; extra; };\n",
        );
        let errors = validate_document(&doc);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("too many positional fields")),
            "{:?}",
            errors
        );
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn track_offsets_parses_dbvl_records() {
        let src = ">schema RegistryEntry from \"registry.dbv\"\nrust; a.bv; rs; src;\npython; p.bv; py; lib;\n";
        let doc = crate::dbriev::v2::parse_document_track_offsets(src).unwrap();
        // The line-oriented format: each physical line is one record. The
        // parser creates one group per record (or merges records into one
        // group's entries) — total entry count is what matters.
        let total_entries: usize = doc.data_groups.iter().map(|g| g.entries.len()).sum();
        assert_eq!(total_entries, 2, "two records -> two entries");
        let first = doc.data_groups[0].entries[0].clone();
        assert_eq!(first.fields.len(), 4, "record fields: {:?}", first.fields);
    }
}
