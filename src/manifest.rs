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
//
// Runtime Exception for Use as a Language:
// When the Work or any Derivative Work thereof is used to generate code
// ("generated code"), such generated code shall not be subject to the
// terms of this License, provided that the generated code itself is not
// a Derivative Work of the Work. This exception does not apply to code
// that is itself a compiler, interpreter, or similar tool that incorporates
// or embeds the Work.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("Failed to parse manifest: {0}")]
    ParseError(String),
    #[error("Manifest file not found at {0}")]
    NotFound(PathBuf),
    #[error("Invalid dependency specification for '{0}': {1}")]
    InvalidDependency(String, String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub project: Project,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dependencies: HashMap<String, Dependency>,
    /// 2026-07-23: Target profiles for multi-target compilation.
    /// Defined as `[target.<name>]` sections in folio.toml.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub target: HashMap<String, TargetProfile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_entry")]
    pub entry: String,
}

fn default_name() -> String {
    "unnamed-project".to_string()
}

fn default_version() -> String {
    "0.1.0".to_string()
}

fn default_entry() -> String {
    "main.bv".to_string()
}

/// A compilation target profile. Defines how SysQuery$ resolves hardware
/// queries and what overrides to apply.
/// 2026-07-23: Used by multi-target compilation (--target / --all-targets).
#[derive(Debug, Clone)]
pub struct TargetProfile {
    /// How to resolve SysQuery$ queries:
    ///   "host"       — query real hardware (default)
    ///   "inline"     — use key-value overrides from this struct
    ///   "file://..." — read overrides from a JSON/YAML/TOML file
    pub introspection: String,
    /// 2026-07-23: SysQuery$ key → value overrides.
    /// Applied before real host queries. Keys use dot notation
    /// (e.g., "cpu.cores", "cpu.arch").
    pub overrides: HashMap<String, OverrideValue>,
    /// 2026-09-06 (plan 2026-09-06-isr-handlers-and-sections.md): the ISR
    /// mechanism for this target — the configured default behind
    /// `isr handler @ vec: ...` when the declaration names none explicitly
    /// (e.g. `isr_mechanism = "arm_cortex_m"`). Must be a row of
    /// config/isr-targets.dbvl; absent + no explicit mechanism = compile
    /// error (the compiler never invents a vector table layout).
    pub isr_mechanism: Option<String>,
}

fn default_introspection() -> String {
    "host".into()
}

impl TargetProfile {
    /// Build a HashMap of SysQuery$ overrides with string values.
    pub fn sysquery_overrides(&self) -> HashMap<String, String> {
        self.overrides.iter().map(|(k, v)| (k.clone(), v.as_string())).collect()
    }
}

impl<'de> Deserialize<'de> for TargetProfile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        let raw: HashMap<String, OverrideValue> = HashMap::deserialize(deserializer)?;
        let introspection = raw.get("introspection")
            .map(|v| match v {
                OverrideValue::String(s) => s.clone(),
                _ => "host".to_string(),
            })
            .unwrap_or_else(default_introspection);
        let overrides: HashMap<String, OverrideValue> = raw.into_iter()
            .filter(|(k, _)| k != "introspection")
            .collect();
        let isr_mechanism = overrides.get("isr_mechanism")
            .map(|v| v.as_string());
        let overrides: HashMap<String, OverrideValue> = overrides
            .into_iter()
            .filter(|(k, _)| k != "isr_mechanism")
            .collect();
        Ok(TargetProfile { introspection, overrides, isr_mechanism })
    }
}

impl Serialize for TargetProfile {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let extra = self.isr_mechanism.is_some() as usize;
        let mut map = serializer.serialize_map(Some(self.overrides.len() + 1 + extra))?;
        map.serialize_entry("introspection", &self.introspection)?;
        if let Some(mech) = &self.isr_mechanism {
            map.serialize_entry("isr_mechanism", mech)?;
        }
        for (k, v) in &self.overrides {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// A value for a SysQuery$ override. Can be a string, integer, float, or bool.
/// 2026-07-23: Used by TargetProfile overrides in multi-target compilation.
#[derive(Debug, Clone)]
pub enum OverrideValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl OverrideValue {
    pub fn as_string(&self) -> String {
        match self {
            OverrideValue::String(s) => s.clone(),
            OverrideValue::Int(n) => n.to_string(),
            OverrideValue::Float(f) => f.to_string(),
            OverrideValue::Bool(b) => b.to_string(),
        }
    }
}

// Custom serde for OverrideValue — handles TOML scalar types
impl<'de> Deserialize<'de> for OverrideValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        use serde::de;
        struct OverrideVisitor;
        impl<'de> de::Visitor<'de> for OverrideVisitor {
            type Value = OverrideValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string, integer, float, or boolean")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<OverrideValue, E> {
                Ok(OverrideValue::String(v.to_string()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<OverrideValue, E> {
                Ok(OverrideValue::String(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<OverrideValue, E> {
                Ok(OverrideValue::Int(v))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<OverrideValue, E> {
                Ok(OverrideValue::Int(v as i64))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<OverrideValue, E> {
                Ok(OverrideValue::Float(v))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<OverrideValue, E> {
                Ok(OverrideValue::Bool(v))
            }
        }
        deserializer.deserialize_any(OverrideVisitor)
    }
}

impl Serialize for OverrideValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            OverrideValue::String(s) => serializer.serialize_str(s),
            OverrideValue::Int(n) => serializer.serialize_i64(*n),
            OverrideValue::Float(f) => serializer.serialize_f64(*f),
            OverrideValue::Bool(b) => serializer.serialize_bool(*b),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Path(PathDependency),
    Registry(RegistryDependency),
    /// 2026-10-06 (package/module v0, folio): a git-sourced dependency.
    /// Exactly one of `rev`/`tag`/`branch` may pin it; none = the default
    /// branch, whose resolved commit is recorded in `folio.lock`.
    Git(GitDependency),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathDependency {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryDependency {
    pub registry: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDependency {
    pub git: String,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

impl GitDependency {
    /// The git ref this dependency pins, if any. `rev` wins, then `tag`, then
    /// `branch`; `None` means the remote's default branch.
    pub fn pin(&self) -> Option<&str> {
        self.rev
            .as_deref()
            .or(self.tag.as_deref())
            .or(self.branch.as_deref())
    }
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        if !path.exists() {
            return Err(ManifestError::NotFound(path.to_path_buf()));
        }
        let content = fs::read_to_string(path)?;
        Self::parse(&content)
    }

    pub fn parse(content: &str) -> Result<Self, ManifestError> {
        toml::from_str(content).map_err(|e| ManifestError::ParseError(e.to_string()))
    }

    pub fn save(&self, path: &Path) -> Result<(), ManifestError> {
        let content =
            toml::to_string_pretty(self).map_err(|e| ManifestError::ParseError(e.to_string()))?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn find_dependency(&self, name: &str) -> Option<&Dependency> {
        self.dependencies.get(name)
    }

    pub fn add_dependency(&mut self, name: String, dep: Dependency) {
        self.dependencies.insert(name, dep);
    }

    pub fn remove_dependency(&mut self, name: &str) -> Option<Dependency> {
        self.dependencies.remove(name)
    }

    pub fn resolve_path(&self, name: &str, project_root: &Path) -> Option<PathBuf> {
        match self.find_dependency(name)? {
            Dependency::Path(p) => {
                let resolved = project_root.join(&p.path);
                if resolved.exists() {
                    Some(resolved)
                } else {
                    None
                }
            }
            Dependency::Registry(_) => None,
            Dependency::Git(_) => None,
        }
    }

    pub fn project_dir(&self, manifest_path: &Path) -> PathBuf {
        manifest_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

impl fmt::Display for Manifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} v{} ({})",
            self.project.name, self.project.version, self.project.entry
        )?;
        if !self.dependencies.is_empty() {
            write!(f, "\n\nDependencies:")?;
            for (name, dep) in &self.dependencies {
                write!(f, "\n  {}: ", name)?;
                match dep {
                    Dependency::Path(p) => write!(f, "{}", p.path.display())?,
                    Dependency::Registry(r) => {
                        write!(f, "{} v{}", r.registry, r.version.as_deref().unwrap_or("*"))?
                    }
                    Dependency::Git(g) => {
                        write!(f, "{}@{}", g.git, g.pin().unwrap_or("HEAD"))?
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn find_manifest(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();

    loop {
        let manifest_path = current.join("folio.toml");
        if manifest_path.exists() {
            return Some(manifest_path);
        }

        let parent = current.parent()?;
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }

    None
}

pub fn create_default_manifest(path: &Path) -> Result<Manifest, ManifestError> {
    let manifest = Manifest {
        project: Project {
            name: path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(default_name),
            version: default_version(),
            entry: default_entry(),
        },
        dependencies: HashMap::new(),
        target: HashMap::new(),
    };
    manifest.save(path)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_manifest() {
        let content = r#"
[project]
name = "test-project"
version = "0.2.0"
entry = "src/main.bv"

[dependencies]
auth = { path = "lib/auth.bv" }
utils = { path = "lib/utils.bv" }
"#;
        let manifest = Manifest::parse(content).unwrap();
        assert_eq!(manifest.project.name, "test-project");
        assert_eq!(manifest.project.version, "0.2.0");
        assert_eq!(manifest.dependencies.len(), 2);
    }

    #[test]
    fn test_find_manifest() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();

        fs::create_dir(project_dir.join("src")).unwrap();
        let manifest_path = project_dir.join("folio.toml");
        fs::write(&manifest_path, "").unwrap();

        let found = find_manifest(&project_dir.join("src").join("main.bv"));
        assert!(found.is_some());
        assert_eq!(found.unwrap(), manifest_path);
    }
}
