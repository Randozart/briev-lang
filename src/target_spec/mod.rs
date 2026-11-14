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

//! Target Spec TOML system for universal transpilation adapter.
//!
//! Defines the TargetSpec struct and related types that drive both FFI
//! call generation and code generation from declarative TOML files.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod loader;
pub use loader::TargetSpecLoader;

/// Main TargetSpec struct - can contain target metadata, FFI, Codegen, or all
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TargetSpec {
    #[serde(default)]
    pub target: Option<TargetSection>,
    #[serde(default)]
    pub ffi: Option<FfiSection>,
    #[serde(default)]
    pub codegen: Option<CodegenSection>,
    #[serde(default)]
    pub memory: Option<MemorySection>,
    #[serde(default)]
    pub bottlenecks: Option<BottleneckSection>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MemorySection {
    #[serde(default)]
    pub banks: HashMap<String, MemoryBank>,
    #[serde(default)]
    pub sections: HashMap<String, MemorySectionDef>,
}

/// Hardware bottleneck configuration for roofline analysis.
/// Can be loaded from a bottlenecks.dbv schema file.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct BottleneckSection {
    #[serde(default = "default_pcie_bw")]
    pub pcie_bandwidth_gbs: f64,
    #[serde(default = "default_ram_bw")]
    pub system_ram_bandwidth_gbs: f64,
    #[serde(default = "default_l1")]
    pub l1_cache_size_kb: u64,
    #[serde(default = "default_l2")]
    pub l2_cache_size_kb: u64,
    #[serde(default = "default_l3")]
    pub l3_cache_size_kb: u64,
    #[serde(default = "default_port_width")]
    pub memory_port_width: u64,
    #[serde(default)]
    pub fpga_clock_mhz: f64,
}

fn default_pcie_bw() -> f64 {
    15.75
}
fn default_ram_bw() -> f64 {
    40.0
}
fn default_l1() -> u64 {
    32
}
fn default_l2() -> u64 {
    256
}
fn default_l3() -> u64 {
    8192
}
fn default_port_width() -> u64 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryBank {
    pub start: u64,
    pub size: u64,
    #[serde(default)]
    pub usage: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemorySectionDef {
    pub at: u64,
    #[serde(default)]
    pub max_size: Option<u64>,
}

/// Target metadata: defines which backend and what capabilities are supported
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TargetSection {
    pub name: String,
    pub backend: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub import_ffi: Option<String>, // Phase 4: inherit FFI from profile
}

/// Errors for TargetSpec operations
#[derive(Debug, Clone)]
pub enum TargetError {
    SpecNotFound(String),
    ParseError(String),
    CapabilityMismatch {
        source_capability: String,
        target: String,
        missing: String,
    },
    BackendNotFound(String),
}

impl std::fmt::Display for TargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetError::SpecNotFound(s) => write!(f, "Target spec '{}' not found", s),
            TargetError::ParseError(s) => write!(f, "Parse error: {}", s),
            TargetError::CapabilityMismatch {
                source_capability,
                target,
                missing,
            } => {
                write!(
                    f,
                    "B4001: Target '{}' lacks '{}' capability required by source",
                    target, missing
                )
            }
            TargetError::BackendNotFound(s) => write!(f, "Backend '{}' not found", s),
        }
    }
}

impl std::error::Error for TargetError {}

impl TargetSpec {
    /// Get capabilities from target section, falling back to defaults
    pub fn capabilities(&self) -> Vec<String> {
        self.target
            .as_ref()
            .map(|t| t.capabilities.clone())
            .unwrap_or_else(|| vec!["logic".to_string()])
    }

    /// Check if target has required capability
    pub fn has_capability(&self, capability: &str) -> bool {
        // "logic" is always required, but specific capabilities need explicit support
        let caps = self.capabilities();
        caps.contains(&capability.to_string())
    }

    /// Validate source capabilities against target, returning error or warnings
    pub fn validate_capabilities(&self, source_caps: &[&str]) -> Result<Vec<String>, TargetError> {
        let target_caps = self.capabilities();
        let mut warnings = Vec::new();

        for sc in source_caps {
            if !target_caps.contains(&sc.to_string()) && *sc != "logic" {
                warnings.push(format!(
                    "B4005: Target '{}' lacks '{}'; feature may be stripped",
                    self.target
                        .as_ref()
                        .map(|t| &t.name)
                        .unwrap_or(&"default".to_string()),
                    sc
                ));
            }
        }

        if warnings.is_empty() {
            Ok(warnings)
        } else {
            Err(TargetError::CapabilityMismatch {
                source_capability: source_caps.join(", "),
                target: self
                    .target
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default(),
                missing: warnings.join("; "),
            })
        }
    }

    /// Get backend name from target section
    pub fn backend(&self) -> String {
        self.target
            .as_ref()
            .map(|t| t.backend.clone())
            .or_else(|| self.codegen.as_ref().map(|c| c.backend.clone()))
            .unwrap_or_else(|| "c".to_string())
    }
}

/// FFI section from TOML profile
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FfiSection {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub language: LanguageMeta,
    #[serde(default)]
    pub types: HashMap<String, TypeDef>,
    #[serde(default)]
    pub mapping: HashMap<String, String>,
    #[serde(default)]
    pub conventions: Conventions,
    #[serde(default)]
    pub overrides: Vec<TypeOverride>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LanguageMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub endianness: String,
    #[serde(default, rename = "pointer_size")]
    pub pointer_size: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TypeDef {
    #[serde(default)]
    pub representation: String,
    #[serde(default)]
    pub size: usize,
    #[serde(default)]
    pub signed: bool,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Conventions {
    #[serde(default)]
    pub alignment: usize,
    #[serde(default, rename = "call_conv")]
    pub call_conv: String,
    #[serde(default, rename = "pointer_size")]
    pub pointer_size: usize,
    #[serde(default)]
    pub error: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TypeOverride {
    pub name: String,
    pub value: String,
}

/// Codegen section from TOML profile
/// Note: `backend` is now also in `[target]` section for consistency, but we check both for compatibility
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CodegenSection {
    #[serde(default)]
    pub backend: String, // Kept for backward compatibility
    #[serde(default)]
    pub extension: String,
    #[serde(default, rename = "state_allocation")]
    pub state_allocation: StateAllocation,
    #[serde(default)]
    pub templates: CodegenTemplates,
    #[serde(default, rename = "entry_point")]
    pub entry_point: EntryPointConfig,
    #[serde(default)]
    pub validation: HashMap<String, String>,
    #[serde(default)]
    pub inference: HashMap<String, String>,
    #[serde(default, rename = "hardware_config")]
    pub hardware_config: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateAllocation {
    Static,
    Dynamic,
    ReachParticipant,
    WasmMemory,
    #[serde(other)]
    Other,
}

impl Default for StateAllocation {
    fn default() -> Self {
        StateAllocation::Other
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CodegenTemplates {
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub footer: Option<String>,
    #[serde(default, rename = "start_template")]
    pub start_template: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EntryPointConfig {
    #[serde(default)]
    pub style: String,
    #[serde(default, rename = "init_txn")]
    pub init_txn: Option<String>,
    #[serde(default, rename = "exit_txn")]
    pub exit_txn: Option<String>,
}

impl TargetSpec {
    /// Validate program against this spec's validation rules
    pub fn validate_program(&self, _program: &[crate::ast::TopLevel]) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Check for blocked types in [codegen.validation]
        if let Some(codegen) = &self.codegen {
            for (type_name, rule) in &codegen.validation {
                if rule.starts_with("error:") {
                    // In a real implementation, check if the program uses this type
                    // For now, just record that validation is active
                    errors.push(format!("Type '{}' validation rule: {}", type_name, rule));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Get the inference rule for a given key
    pub fn inference(&self, key: &str) -> Option<&str> {
        self.codegen
            .as_ref()
            .and_then(|c| c.inference.get(key))
            .map(|s| s.as_str())
    }
}

#[derive(Debug)]
pub enum LoadError {
    SpecNotFound(String),
    IoError(std::io::Error),
    ParseError(String),
}

impl From<std::io::Error> for LoadError {
    fn from(err: std::io::Error) -> Self {
        LoadError::IoError(err)
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::SpecNotFound(path) => write!(f, "Spec not found: {}", path),
            LoadError::IoError(e) => write!(f, "IO error: {}", e),
            LoadError::ParseError(e) => write!(f, "Parse error: {}", e),
        }
    }
}

impl std::error::Error for LoadError {}
