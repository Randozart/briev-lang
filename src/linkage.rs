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

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct LinkageConfig {
    pub links: HashMap<String, LinkMapping>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinkMapping {
    pub sv: Option<String>,
    pub rust: Option<String>,
    pub c: Option<String>,
}

impl LinkageConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, LinkageError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| LinkageError::IoError(e.to_string()))?;
        toml::from_str(&content).map_err(|e| LinkageError::ParseError(e.to_string()))
    }

    pub fn resolve_sv(&self, name: &str) -> Option<&str> {
        self.links.get(name).and_then(|m| m.sv.as_deref())
    }

    pub fn resolve_rust(&self, name: &str) -> Option<&str> {
        self.links.get(name).and_then(|m| m.rust.as_deref())
    }

    pub fn resolve_c(&self, name: &str) -> Option<&str> {
        self.links.get(name).and_then(|m| m.c.as_deref())
    }
}

#[derive(Debug)]
pub enum LinkageError {
    IoError(String),
    ParseError(String),
    Unresolved(String),
}

impl std::fmt::Display for LinkageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkageError::IoError(msg) => write!(f, "IO error: {}", msg),
            LinkageError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            LinkageError::Unresolved(msg) => write!(f, "Unresolved link: {}", msg),
        }
    }
}

impl std::error::Error for LinkageError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linkage_parsing() {
        let config = LinkageConfig {
            links: HashMap::from([
                (
                    "weight_valid".to_string(),
                    LinkMapping {
                        sv: Some("fpga_weight_valid_wire".to_string()),
                        rust: Some("0x8000A040".to_string()),
                        c: Some("0x8000A040".to_string()),
                    },
                ),
                (
                    "result_data".to_string(),
                    LinkMapping {
                        sv: Some("fpga_result_data_wire".to_string()),
                        rust: Some("0x8000A050".to_string()),
                        c: Some("0x8000A050".to_string()),
                    },
                ),
            ]),
        };

        assert_eq!(
            config.resolve_sv("weight_valid"),
            Some("fpga_weight_valid_wire")
        );
        assert_eq!(config.resolve_rust("weight_valid"), Some("0x8000A040"));
        assert_eq!(config.resolve_c("result_data"), Some("0x8000A050"));
        assert_eq!(config.resolve_sv("nonexistent"), None);
    }
}
