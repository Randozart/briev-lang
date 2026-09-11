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

//! Memory Spec Output
//!
//! Collects all variable/register/address allocations during compilation
//! and outputs a JSON/TOML spec for foreign language consumption.

use crate::ast::{BitRange, Expr, LinkRef, StateDecl, TopLevel, Transaction, Trigger, Type};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct MemorySpec {
    pub target: String,
    pub compiler_version: String,
    pub allocations: BTreeMap<String, Allocation>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metropolitan_ffi: BTreeMap<String, FfiRegion>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub triggers: BTreeMap<String, TriggerInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Allocation {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub size_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    pub is_trigger: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FfiRegion {
    pub address: String,
    pub size_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerInfo {
    #[serde(rename = "type")]
    pub trigger_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

impl MemorySpec {
    pub fn new(target: &str) -> Self {
        MemorySpec {
            target: target.to_string(),
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            allocations: BTreeMap::new(),
            metropolitan_ffi: BTreeMap::new(),
            triggers: BTreeMap::new(),
        }
    }

    /// Collect allocations from a parsed Briev program
    pub fn collect_from_program(&mut self, items: &[TopLevel]) {
        for item in items {
            match item {
                TopLevel::StateDecl(decl) => {
                    self.add_state_decl(decl);
                }
                TopLevel::Trigger(trg) => {
                    self.add_trigger_decl(trg);
                }
                TopLevel::Transaction(txn) => {
                    self.add_transaction(txn);
                }
                _ => {}
            }
        }
    }

    fn add_state_decl(&mut self, decl: &StateDecl) {
        let type_name = format_type(&decl.ty);
        let size = estimate_type_size(&decl.ty);

        self.allocations.insert(
            decl.name.clone(),
            Allocation {
                type_name,
                address: None,
                size_bytes: size,
                bit_range: None,
                stage: None,
                is_trigger: false,
            },
        );
    }

    fn add_trigger_decl(&mut self, trg: &Trigger) {
        let type_name = "trg".to_string();
        let size = 8usize;

        self.allocations.insert(
            trg.name.clone(),
            Allocation {
                type_name,
                address: None,
                size_bytes: size,
                bit_range: None,
                stage: None,
                is_trigger: true,
            },
        );

        self.triggers.insert(
            trg.name.clone(),
            TriggerInfo {
                trigger_type: "hardware".to_string(),
                binding: None,
                mode: None,
            },
        );
    }

    fn add_transaction(&mut self, txn: &Transaction) {
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Serialize to TOML string
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

fn format_type(ty: &Type) -> String {
    match ty {
        Type::Dyn(inner) => format!("dyn {}", inner),
        Type::Task(inner) => format!("Task<{}>", inner),
        Type::Number(n) => n.to_string(),
        Type::Custom(__t) if __t == "Int" => "Int".to_string(),
        Type::Custom(__t) if __t == "Int8" => "Int8".to_string(),
        Type::Custom(__t) if __t == "Int16" => "Int16".to_string(),
        Type::Custom(__t) if __t == "Int32" => "Int32".to_string(),
        Type::Custom(__t) if __t == "UInt" => "UInt".to_string(),
        Type::Custom(__t) if __t == "UInt8" => "UInt8".to_string(),
        Type::Custom(__t) if __t == "UInt16" => "UInt16".to_string(),
        Type::Custom(__t) if __t == "UInt32" => "UInt32".to_string(),
        Type::Bits(width) => format!("Bit<{}>", width),
        Type::Width(n) => format!("Width({})", n),
        Type::Custom(__t) if __t == "Float" => "Float".to_string(),
        Type::Custom(__t) if __t == "Float64" => "Float64".to_string(),
        Type::Custom(__t) if __t == "Bool" => "Bool".to_string(),
        Type::Custom(__t) if __t == "String" => "String".to_string(),
        Type::Void => "void".to_string(),
        Type::Custom(__t) if __t == "Blob" => "Blob".to_string(),
        Type::Custom(__t) if __t == "Char" => "Char".to_string(),
        Type::Custom(name) => name.clone(),
        Type::Union(types) => {
            let inner: Vec<_> = types.iter().map(format_type).collect();
            inner.join(" | ")
        }
        Type::Tuple(types) => {
            let inner: Vec<_> = types.iter().map(format_type).collect();
            format!("({})", inner.join(", "))
        }
        Type::TypeVar(name) => name.clone(),
        Type::Generic(name, params) => {
            let inner: Vec<_> = params.iter().map(format_type).collect();
            format!("{}<{}>", name, inner.join(", "))
        }
        Type::Applied(name, args) => {
            let inner: Vec<_> = args.iter().map(format_type).collect();
            format!("{}<{}>", name, inner.join(", "))
        }
        Type::Vector(elem, dims) => {
            let total_size: usize = dims.iter().map(|d| match d {
                crate::ast::Dimension::Anonymous(s) => *s,
                crate::ast::Dimension::Named(_, s) => *s,
            }).product();
            format!("Vec<{}; {}>", format_type(elem), total_size)
        }
        Type::Constrained(inner, bit_range) => {
            format!("{}@/{}", format_type(inner), format_bit_range(bit_range))
        }
        // 2026-07-03: Layout-constrained pointer — show canonical form
        Type::LayoutPtr(lc) => format!("Ptr<Bits @/0..{}>", lc.bytes * 8 - 1),
        Type::Ptr(inner) => format!("Ptr<{}>", format_type(inner)),
        Type::PtrConst(inner) => format!("Ptr<const {}>", format_type(inner)),
        Type::Function(params, ret) => {
            let inner: Vec<_> = params.iter().map(format_type).collect();
            format!("({}) -> {}", inner.join(", "), format_type(ret))
        },
    }
}

fn format_bit_range(br: &BitRange) -> String {
    match br {
        BitRange::Single(n) => format!("{}", n),
        BitRange::Any(n) => format!("x{}", n),
        BitRange::Range(start, end) => format!("{}..{}", start, end),
    }
}

fn estimate_type_size(ty: &Type) -> usize {
    match ty {
        // A trait object is a fat pointer; the payload dominates layout
        // estimates until the thunk-table ABI lands (Phase 5c).
        Type::Dyn(inner) => estimate_type_size(inner).max(8),
        // A task handle is one i64 slot (the eager result-handle model).
        Type::Task(_) => 8,
        Type::Number(n) => (*n).max(0) as usize,
        Type::Custom(__t) if __t == "Int" || __t == "UInt" => 8,
        Type::Custom(__t) if __t == "Int8" || __t == "UInt8" => 1,
        Type::Custom(__t) if __t == "Int16" || __t == "UInt16" => 2,
        Type::Custom(__t) if __t == "Int32" || __t == "UInt32" => 4,
        Type::Custom(__t) if __t == "Float" => 8,
        Type::Custom(__t) if __t == "Float64" => 8,
        Type::Custom(__t) if __t == "Bool" => 1,
        // 2026-07-18: String is 16 bytes (2 × i64 struct). Data stays rough.
        Type::Custom(__t) if __t == "String" => 16,
        Type::Void => 0,
        Type::Custom(__t) if __t == "Blob" => 16,
        Type::Custom(__t) if __t == "Char" => 4,
        Type::Custom(_) => 8,
        Type::Union(types) => types.iter().map(estimate_type_size).max().unwrap_or(8),
        Type::Tuple(types) => types.iter().map(estimate_type_size).sum(),
        Type::TypeVar(_) => 8,
        Type::Generic(_, _) => 8,
        Type::Applied(_, _) => 8,
        Type::Vector(elem, dims) => {
            let total_size: usize = dims.iter().map(|d| match d {
                crate::ast::Dimension::Anonymous(s) => *s,
                crate::ast::Dimension::Named(_, s) => *s,
            }).product();
            estimate_type_size(elem) * total_size
        }
        Type::Constrained(_, BitRange::Single(_)) => 1,
        Type::Constrained(_, BitRange::Any(n)) => (*n + 7) / 8,
        Type::Constrained(_, BitRange::Range(start, end)) => (end - start + 1 + 7) / 8,
        // 2026-07-03: Layout-constrained pointer — value is always pointer-width (8 bytes on x86_64)
        Type::LayoutPtr(_) => 8,
        // 2026-08-13 (layout-keywords plan): Bits stores bits; storage rounds up.
        Type::Bits(n) => (*n as usize).div_ceil(8),
        Type::Width(_) => 8,
        Type::Ptr(_) | Type::PtrConst(_) => 8,
        Type::Function(_, _) => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_spec_empty() {
        let spec = MemorySpec::new("test");
        assert_eq!(spec.target, "test");
        assert!(spec.allocations.is_empty());
    }

    #[test]
    fn test_memory_spec_json_output() {
        let mut spec = MemorySpec::new("aarch64");
        spec.allocations.insert(
            "counter".to_string(),
            Allocation {
                type_name: "Int".to_string(),
                address: Some("0x1000".to_string()),
                size_bytes: 8,
                bit_range: None,
                stage: None,
                is_trigger: false,
            },
        );
        let json = spec.to_json().unwrap();
        assert!(json.contains("counter"));
        assert!(json.contains("0x1000"));
    }

    #[test]
    fn test_format_bit_range() {
        assert_eq!(format_bit_range(&BitRange::Any(16)), "x16");
        assert_eq!(format_bit_range(&BitRange::Range(3, 7)), "3..7");
    }

    #[test]
    fn test_estimate_type_size() {
        assert_eq!(estimate_type_size(&Type::bool_()), 1);
        assert_eq!(estimate_type_size(&Type::int()), 8);
        assert_eq!(
            estimate_type_size(&Type::Constrained(
                Box::new(Type::Custom("UInt".to_string())),
                BitRange::Any(8)
            )),
            1
        );
        assert_eq!(
            estimate_type_size(&Type::Constrained(
                Box::new(Type::Custom("UInt".to_string())),
                BitRange::Any(32)
            )),
            4
        );
    }
}
