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

//! Target Spec Loader
//!
//! Loads TargetSpec from TOML files, searching multiple directories.

use std::path::{Path, PathBuf};

use super::LoadError;
use super::TargetSpec;

/// Searches multiple directories for Target Spec TOML files
pub struct TargetSpecLoader {
    search_paths: Vec<PathBuf>,
}

impl TargetSpecLoader {
    /// Create a new loader with default search paths
    pub fn new() -> Self {
        Self {
            search_paths: vec![PathBuf::from("lib/targets"), PathBuf::from(".")],
        }
    }

    /// Try to find a target spec, checking multiple locations
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        let name = if name.ends_with(".toml") {
            name.to_string()
        } else {
            format!("{}.toml", name)
        };

        // Try each search path
        for base in &self.search_paths {
            let path = base.join(&name);
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// Create loader for project root (where binary is run)
    pub fn project_root() -> Self {
        Self {
            search_paths: vec![PathBuf::from("lib/targets"), PathBuf::from(".")],
        }
    }

    /// Add a custom search path
    pub fn add_path(&mut self, path: PathBuf) {
        self.search_paths.push(path);
    }

    /// Load a TargetSpec from a path or name
    pub fn load(&self, spec_path: &Path) -> Result<TargetSpec, LoadError> {
        // 1. If absolute path, load directly
        if spec_path.is_absolute() {
            return self.load_file(spec_path);
        }

        // 2. Try current directory first (most common case)
        if spec_path.exists() {
            return self.load_file(spec_path);
        }

        // 3. Try with .toml extension in current dir
        let with_ext = format!("{}.toml", spec_path.display());
        if std::path::Path::new(&with_ext).exists() {
            return self.load_file(std::path::Path::new(&with_ext));
        }

        // 4. Try each search path
        for base in &self.search_paths {
            // Try direct join
            let full_path = base.join(spec_path);
            if full_path.exists() {
                return self.load_file(&full_path);
            }

            // Try with .toml extension
            let with_ext = base.join(format!("{}.toml", spec_path.display()));
            if with_ext.exists() {
                return self.load_file(&with_ext);
            }
        }

        Err(LoadError::SpecNotFound(spec_path.display().to_string()))
    }

    /// Load and parse a TOML file
    fn load_file(&self, path: &Path) -> Result<TargetSpec, LoadError> {
        let content = std::fs::read_to_string(path).map_err(|e| LoadError::IoError(e))?;

        toml::from_str(&content).map_err(|e| LoadError::ParseError(e.to_string()))
    }
}

impl Default for TargetSpecLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_find_without_toml_ext() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("myspec.toml");
        fs::write(&file_path, "name = 'test'\n").unwrap();

        let mut loader = TargetSpecLoader::new();
        loader.add_path(dir.path().to_path_buf());
        let result = loader.find("myspec");
        assert!(result.is_some());
        assert_eq!(result.unwrap().file_name().unwrap(), "myspec.toml");
    }

    #[test]
    fn test_find_with_toml_ext() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("myspec.toml");
        fs::write(&file_path, "name = 'test'\n").unwrap();

        let mut loader = TargetSpecLoader::new();
        loader.add_path(dir.path().to_path_buf());
        let result = loader.find("myspec.toml");
        assert!(result.is_some());
    }

    #[test]
    fn test_find_not_found_returns_none() {
        let loader = TargetSpecLoader::new();
        let result = loader.find("nonexistent_spec_xyz");
        assert!(result.is_none());
    }

    #[test]
    fn test_project_root_default_paths() {
        let loader = TargetSpecLoader::project_root();
        assert_eq!(loader.search_paths.len(), 2);
        assert_eq!(loader.search_paths[0], PathBuf::from("lib/targets"));
        assert_eq!(loader.search_paths[1], PathBuf::from("."));
    }

    #[test]
    fn test_loader_with_custom_path() {
        let dir = TempDir::new().unwrap();
        let mut loader = TargetSpecLoader::new();
        loader.add_path(dir.path().to_path_buf());
        assert!(loader.search_paths.len() >= 1);
        assert_eq!(loader.search_paths.last().unwrap(), dir.path());
    }
}
