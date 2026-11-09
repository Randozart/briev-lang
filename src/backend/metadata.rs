// ── Phase G — Metadata Registry ──────────────────────────────────────
// 2026-07-28: Phase G.2 — DBV-backed metadata registry.
// Loads config/meta-vocab.dbv at compile time and provides typed lookup
// functions for each backend. Replaces hardcoded key→attribute matching.
// Flat code: max 2 nesting levels.

use crate::dbriev::v2::{parse_document_quoted, DataEntry, DataField, DataValue};
use std::collections::HashMap;

/// A metadata field definition from the vocabulary.
#[derive(Debug, Clone)]
pub struct MetaFieldDef {
    pub name: String,
    pub field_type: MetaType,
    pub description: String,
}

/// Supported metadata value types.
#[derive(Debug, Clone, PartialEq)]
pub enum MetaType {
    Bool,
    Int,
    Float,
    String,
    List,
}

impl MetaType {
    fn from_str(s: &str) -> Self {
        match s {
            "Bool" => MetaType::Bool,
            "Int" => MetaType::Int,
            "Float" => MetaType::Float,
            "String" => MetaType::String,
            "String[]" => MetaType::List,
            _ => MetaType::String,
        }
    }
}

/// A backend mapping rule from the vocabulary.
/// Parsed positionally from as BackendMapping { > backend; meta_key; value_pattern; attr; scope; };
#[derive(Debug, Clone)]
struct BackendMapping {
    backend: String,
    metadata_key: String,
    value_pattern: String,
    ir_attribute: String,
    applies_to: String,
}

/// Compiled metadata registry loaded from config/meta-vocab.dbv.
/// Provides O(n) lookup per backend (n = number of mapping rules for that backend),
/// which is acceptable for metadata that is evaluated once per function/instruction
/// during codegen (n ≤ 20).
#[derive(Debug, Clone)]
pub struct MetadataRegistry {
    fields: HashMap<String, MetaFieldDef>,
    mappings: Vec<BackendMapping>,
    llvm_idx: Vec<usize>,
    webstack_idx: Vec<usize>,
    circt_idx: Vec<usize>,
}

impl MetadataRegistry {
    /// Load the metadata vocabulary from the baked config file.
    /// Panics on parse failure — the .dbv file is a compile-time invariant.
    pub fn load() -> Self {
        let content = include_str!("../../config/meta-vocab.dbv");
        let doc = parse_document_quoted(content)
            .expect("config/meta-vocab.dbv: parse failed — check .dbv syntax");

        let mut fields: HashMap<String, MetaFieldDef> = HashMap::new();
        let mut mappings: Vec<BackendMapping> = Vec::new();

        for group in &doc.data_groups {
            let schema_name = match &group.schema_name {
                Some(n) => n.as_str(),
                None => continue,
            };
            match schema_name {
                "MetaField" => {
                    for entry in &group.entries {
                        if let Some(field) = Self::parse_meta_field(entry) {
                            fields.insert(field.name.clone(), field);
                        }
                    }
                }
                "BackendMapping" => {
                    for entry in &group.entries {
                        if let Some(mapping) = Self::parse_backend_mapping(entry) {
                            mappings.push(mapping);
                        }
                    }
                }
                _ => {}
            }
        }

        let mut llvm_idx = Vec::new();
        let mut webstack_idx = Vec::new();
        let mut circt_idx = Vec::new();
        for (i, m) in mappings.iter().enumerate() {
            match m.backend.as_str() {
                "llvm" => llvm_idx.push(i),
                "webstack" => webstack_idx.push(i),
                "circt" => circt_idx.push(i),
                _ => {}
            }
        }

        MetadataRegistry { fields, mappings, llvm_idx, webstack_idx, circt_idx }
    }

    /// Look up a metadata field definition by name.
    pub fn field_def(&self, name: &str) -> Option<&MetaFieldDef> {
        self.fields.get(name)
    }

    /// Look up an LLVM IR attribute for a (key, value) metadata pair.
    pub fn llvm_attr(&self, key: &str, value: &str) -> Option<&str> {
        self.match_mapping(&self.llvm_idx, key, value)
    }

    /// Look up a Webstack option for a (key, value) metadata pair.
    pub fn webstack_option(&self, key: &str, value: &str) -> Option<&str> {
        self.match_mapping(&self.webstack_idx, key, value)
    }

    /// Look up a CIRCT option for a (key, value) metadata pair.
    pub fn circt_option(&self, key: &str, value: &str) -> Option<&str> {
        self.match_mapping(&self.circt_idx, key, value)
    }

    /// Number of registered field definitions.
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// Number of registered mapping rules.
    pub fn mapping_count(&self) -> usize {
        self.mappings.len()
    }

    fn match_mapping(&self, indices: &[usize], key: &str, value: &str) -> Option<&str> {
        for &idx in indices {
            let m = &self.mappings[idx];
            if m.metadata_key == key && (m.value_pattern == "*" || m.value_pattern == value) {
                return Some(m.ir_attribute.as_str());
            }
        }
        None
    }

    fn parse_meta_field(entry: &DataEntry) -> Option<MetaFieldDef> {
        // Keyed entry: key = name, positional fields: [type, description]
        let name = entry.key.as_ref()?.clone();
        let fields = &entry.fields;
        if fields.len() < 2 {
            return None;
        }
        let type_str = value_as_string(&fields[0])?;
        let description = value_as_string(&fields[1])?;
        Some(MetaFieldDef {
            name,
            field_type: MetaType::from_str(&type_str),
            description,
        })
    }

    fn parse_backend_mapping(entry: &DataEntry) -> Option<BackendMapping> {
        // Positional entry: fields = [backend, meta_key, value_pattern, attr, scope]
        // (entry.key is None for positional > entries)
        let fields = &entry.fields;
        if fields.len() < 5 {
            return None;
        }
        Some(BackendMapping {
            backend: value_as_string(&fields[0])?,
            metadata_key: value_as_string(&fields[1])?,
            value_pattern: value_as_string(&fields[2])?,
            ir_attribute: value_as_string(&fields[3])?,
            applies_to: value_as_string(&fields[4])?,
        })
    }
}

fn value_as_string(df: &DataField) -> Option<String> {
    match df {
        DataField::Positional(DataValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Apply LLVM function-level attributes from attr metadata.
pub fn apply_llvm_function_metadata(
    fn_attrs: &mut Vec<String>,
    metadata: &[(&str, &str)],
    registry: &MetadataRegistry,
) {
    for (key, value) in metadata {
        if let Some(attr) = registry.llvm_attr(key, value) {
            fn_attrs.push(attr.to_string());
        }
    }
}

/// Emit fast-math flags string for LLVM float operations.
pub fn emit_fast_math_flags(
    metadata: &[(&str, &str)],
    registry: &MetadataRegistry,
) -> String {
    let mut flags = String::new();
    for (key, value) in metadata {
        if key == &"overflow" {
            continue;
        }
        if let Some(attr) = registry.llvm_attr(key, value) {
            flags.push(' ');
            flags.push_str(attr);
        }
    }
    flags
}

/// Apply Webstack options from attr metadata.
pub fn apply_webstack_metadata<'a>(
    metadata: &[(&str, &str)],
    registry: &'a MetadataRegistry,
) -> Vec<(&'a str, String)> {
    let mut opts = Vec::new();
    for (key, value) in metadata {
        if let Some(option) = registry.webstack_option(key, value) {
            opts.push((option, value.to_string()));
        }
    }
    opts
}

/// Apply CIRCT options from attr metadata.
pub fn apply_circt_metadata<'a>(
    metadata: &[(&str, &str)],
    registry: &'a MetadataRegistry,
) -> Vec<(&'a str, String)> {
    let mut opts = Vec::new();
    for (key, value) in metadata {
        if let Some(option) = registry.circt_option(key, value) {
            opts.push((option, value.to_string()));
        }
    }
    opts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> MetadataRegistry {
        MetadataRegistry::load()
    }

    #[test]
    fn test_registry_loads() {
        let reg = test_registry();
        assert!(reg.field_count() >= 10, "expected >=10 field defs, got {}", reg.field_count());
        assert!(reg.mapping_count() >= 10, "expected >=10 mappings, got {}", reg.mapping_count());
    }

    #[test]
    fn test_llvm_overflow_wrapping() {
        let reg = test_registry();
        let result = reg.llvm_attr("overflow", "wrapping");
        assert_eq!(result, Some("nuw nsw"));
    }

    #[test]
    fn test_llvm_fp_math_fast() {
        let reg = test_registry();
        let result = reg.llvm_attr("fp_math", "fast");
        assert_eq!(result, Some("fast"));
    }

    #[test]
    fn test_llvm_readonly() {
        let reg = test_registry();
        let result = reg.llvm_attr("readonly", "true");
        assert_eq!(result, Some("readonly"));
    }

    #[test]
    fn test_llvm_inline_hint_always() {
        let reg = test_registry();
        let result = reg.llvm_attr("inline_hint", "always");
        assert_eq!(result, Some("alwaysinline"));
    }

    #[test]
    fn test_llvm_inline_hint_never() {
        let reg = test_registry();
        let result = reg.llvm_attr("inline_hint", "never");
        assert_eq!(result, Some("noinline"));
    }

    #[test]
    fn test_llvm_unknown_key() {
        let reg = test_registry();
        let result = reg.llvm_attr("nonexistent_key", "val");
        assert_eq!(result, None);
    }

    #[test]
    fn test_llvm_overflow_checked() {
        let reg = test_registry();
        let result = reg.llvm_attr("overflow", "checked");
        assert_eq!(result, Some("nsw"));
    }

    #[test]
    fn test_llvm_wildcard() {
        let reg = test_registry();
        // unroll_hint uses "*" wildcard — any value matches
        let result = reg.llvm_attr("unroll_hint", "4");
        assert_eq!(result, Some("unroll"));
        let result = reg.llvm_attr("unroll_hint", "8");
        assert_eq!(result, Some("unroll"));
    }

    #[test]
    fn test_webstack_stack_alloc() {
        let reg = test_registry();
        let result = reg.webstack_option("alloc_scope", "stack");
        assert_eq!(result, Some("stack_allocation"));
    }

    #[test]
    fn test_webstack_default() {
        let reg = test_registry();
        let result = reg.webstack_option("alloc_scope", "heap");
        assert_eq!(result, None);
    }

    #[test]
    fn test_circt_convergence_tight() {
        let reg = test_registry();
        let result = reg.circt_option("convergence", "tight");
        assert_eq!(result, Some("single_cycle"));
    }

    #[test]
    fn test_circt_unroll_hint() {
        let reg = test_registry();
        let result = reg.circt_option("unroll_hint", "4");
        assert_eq!(result, Some("unroll_factor"));
    }

    #[test]
    fn test_field_def_exists() {
        let reg = test_registry();
        let field = reg.field_def("overflow");
        assert!(field.is_some());
        assert_eq!(field.unwrap().name, "overflow");
    }

    #[test]
    fn test_field_def_nonexistent() {
        let reg = test_registry();
        assert!(reg.field_def("nope").is_none());
    }

    #[test]
    fn test_emit_fast_math_flags_fp_math_fast() {
        let reg = test_registry();
        let metadata = vec![("fp_math", "fast"), ("fp_contract", "fast")];
        let flags = emit_fast_math_flags(&metadata, &reg);
        assert!(flags.contains("fast"));
        assert!(flags.contains("contract"));
    }

    #[test]
    fn test_emit_fast_math_flags_empty() {
        let reg = test_registry();
        let metadata: Vec<(&str, &str)> = vec![("nonexistent", "val")];
        let flags = emit_fast_math_flags(&metadata, &reg);
        assert_eq!(flags, "");
    }

    #[test]
    fn test_apply_llvm_function_metadata() {
        let reg = test_registry();
        let mut attrs = Vec::new();
        let metadata = vec![("readonly", "true"), ("inline_hint", "always")];
        apply_llvm_function_metadata(&mut attrs, &metadata, &reg);
        assert!(attrs.contains(&"readonly".to_string()));
        assert!(attrs.contains(&"alwaysinline".to_string()));
    }

    #[test]
    fn test_apply_webstack_metadata() {
        let reg = test_registry();
        let metadata = vec![("alloc_scope", "stack")];
        let opts = apply_webstack_metadata(&metadata, &reg);
        assert!(!opts.is_empty());
        assert_eq!(opts[0].0, "stack_allocation");
    }

    #[test]
    fn test_apply_circt_metadata() {
        let reg = test_registry();
        let metadata = vec![("convergence", "tight")];
        let opts = apply_circt_metadata(&metadata, &reg);
        assert!(!opts.is_empty());
        assert_eq!(opts[0].0, "single_cycle");
    }
}
