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

use crate::manifest::{Dependency, Manifest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(test)]
use tempfile::TempDir;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ResolveError {
    #[error("Unresolved import '{0}'")]
    UnresolvedImport(String),
    #[error("Import '{0}' not found in manifest or search paths")]
    NotFound(String),
    #[error("Registry dependency '{0}' not yet supported")]
    RegistryNotSupported(String),
    #[error("Multiple candidates found for '{0}': {1:?}")]
    MultipleCandidates(String, Vec<PathBuf>),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct ResolvedImport {
    pub name: String,
    pub path: PathBuf,
    pub source: ImportSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportSource {
    Manifest,
    Discovered,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub items: Vec<String>,
    pub from: String,
}

impl Import {
    pub fn new(from: String, items: Vec<String>) -> Self {
        Self { from, items }
    }
}

pub struct Resolver {
    manifest: Option<Manifest>,
    manifest_path: Option<PathBuf>,
    search_paths: Vec<PathBuf>,
    resolved: HashMap<String, ResolvedImport>,
    discovered_paths: Vec<PathBuf>,
}

impl Resolver {
    pub fn new() -> Self {
        Resolver {
            manifest: None,
            manifest_path: None,
            search_paths: vec![
                PathBuf::from("lib"),
                PathBuf::from("imports"),
                PathBuf::from("."),
            ],
            resolved: HashMap::new(),
            discovered_paths: Vec::new(),
        }
    }

    pub fn with_manifest(mut self, manifest: Manifest, manifest_path: PathBuf) -> Self {
        self.manifest = Some(manifest);
        self.manifest_path = Some(manifest_path);
        self
    }

    pub fn add_search_path(&mut self, path: PathBuf) {
        self.search_paths.push(path);
    }

    pub fn resolve(&mut self, import: &Import) -> Result<ResolvedImport, ResolveError> {
        let name = &import.from;

        if let Some(existing) = self.resolved.get(name) {
            return Ok(existing.clone());
        }

        let project_root = self
            .manifest_path
            .as_ref()
            .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        if let Some(ref manifest) = self.manifest {
            if let Some(path) = manifest.resolve_path(name, &project_root) {
                let resolved = ResolvedImport {
                    name: name.clone(),
                    path,
                    source: ImportSource::Manifest,
                };
                self.resolved.insert(name.clone(), resolved.clone());
                return Ok(resolved);
            }
        }

        let candidates = self.search_import_file(name, &project_root);

        match candidates.len() {
            0 => Err(ResolveError::NotFound(name.clone())),
            1 => {
                let path = candidates.into_iter().next().unwrap();

                if let Some(ref mut manifest) = self.manifest {
                    manifest.add_dependency(
                        name.clone(),
                        Dependency::Path(crate::manifest::PathDependency {
                            path: Self::relative_to_project(&path, &project_root),
                        }),
                    );
                    if let Some(ref manifest_path) = self.manifest_path {
                        let _ = manifest.save(manifest_path);
                    }
                }

                let resolved = ResolvedImport {
                    name: name.clone(),
                    path,
                    source: ImportSource::Discovered,
                };
                self.resolved.insert(name.clone(), resolved.clone());
                Ok(resolved)
            }
            _ => Err(ResolveError::MultipleCandidates(name.clone(), candidates)),
        }
    }

    fn search_import_file(&self, name: &str, project_root: &Path) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        let file_name = format!("{}.bv", name);

        for search_dir in &self.search_paths {
            let full_path = project_root.join(search_dir).join(&file_name);
            if full_path.exists() {
                candidates.push(full_path);
            }
        }

        let direct_path = project_root.join(&file_name);
        if direct_path.exists() && !candidates.contains(&direct_path) {
            candidates.push(direct_path);
        }

        candidates
    }

    fn relative_to_project(path: &Path, project_root: &Path) -> PathBuf {
        if let Ok(rel) = path.strip_prefix(project_root) {
            rel.to_path_buf()
        } else {
            path.to_path_buf()
        }
    }

    pub fn get_resolved(&self, name: &str) -> Option<&ResolvedImport> {
        self.resolved.get(name)
    }

    pub fn all_resolved(&self) -> Vec<&ResolvedImport> {
        self.resolved.values().collect()
    }

    pub fn get_discovered(&self) -> Vec<&ResolvedImport> {
        self.resolved
            .values()
            .filter(|r| r.source == ImportSource::Discovered)
            .collect()
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }

    pub fn manifest_mut(&mut self) -> Option<&mut Manifest> {
        self.manifest.as_mut()
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_from_manifest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manifest_path = tmp.path().join("folio.toml");

        let manifest = Manifest {
            project: crate::manifest::Project::default(),
            dependencies: HashMap::from([(
                "auth".to_string(),
                Dependency::Path(crate::manifest::PathDependency {
                    path: PathBuf::from("lib/auth.bv"),
                }),
            )]),
            target: HashMap::new(),
        };
        manifest.save(&manifest_path).unwrap();

        let lib_dir = tmp.path().join("lib");
        std::fs::create_dir(&lib_dir).unwrap();
        std::fs::write(lib_dir.join("auth.bv"), "").unwrap();

        let mut resolver = Resolver::new().with_manifest(manifest, manifest_path);

        let import = Import::new("auth".to_string(), vec!["login".to_string()]);
        let resolved = resolver.resolve(&import).unwrap();

        assert_eq!(resolved.source, ImportSource::Manifest);
    }

    #[test]
    fn test_auto_discover() {
        let tmp = tempfile::TempDir::new().unwrap();
        let lib_dir = tmp.path().join("lib");
        std::fs::create_dir(&lib_dir).unwrap();
        std::fs::write(lib_dir.join("utils.bv"), "").unwrap();

        let manifest_path = tmp.path().join("folio.toml");
        let manifest = Manifest {
            project: crate::manifest::Project::default(),
            dependencies: std::collections::HashMap::new(),
            target: std::collections::HashMap::new(),
        };
        manifest.save(&manifest_path).unwrap();

        let mut resolver = Resolver::new().with_manifest(manifest, manifest_path);
        let import = Import::new("utils".to_string(), vec!["format".to_string()]);

        let resolved = resolver.resolve(&import).unwrap();
        assert!(resolved.path.ends_with("utils.bv"));
    }
}
