use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

pub mod handoff;

#[derive(Debug, Clone, Deserialize)]
pub struct HardwareLib {
    pub targets: HashMap<String, TargetProfile>,
    pub interfaces: HashMap<String, InterfaceProfile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TargetProfile {
    pub vendor: String,
    pub family: String,
    #[serde(default)]
    pub part_prefix: String,
    #[serde(default)]
    pub memory_pragmas: HashMap<String, String>,
    #[serde(default)]
    pub logic_pragmas: HashMap<String, String>,
    #[serde(default)]
    pub synthesis: SynthesisConfig,
    #[serde(default)]
    pub constraints: ConstraintsConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SynthesisConfig {
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub script_ext: String,
    #[serde(default)]
    pub default_part: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ConstraintsConfig {
    #[serde(default)]
    pub max_frequency_hz: u64,
    #[serde(default)]
    pub timing_units: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InterfaceProfile {
    pub name: String,
    #[serde(default)]
    pub interface_type: String,
    #[serde(default)]
    pub address_width: u32,
    #[serde(default)]
    pub data_width: u32,
    #[serde(default)]
    pub port_map: HashMap<String, String>,
    #[serde(default)]
    pub axi4_full: Option<Axi4FullConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Axi4FullConfig {
    #[serde(default)]
    pub async_read: bool,
    #[serde(default)]
    pub async_write: bool,
    #[serde(default)]
    pub max_burst_length: u32,
}

impl HardwareLib {
    pub fn load<P: AsRef<Path>>(base_path: P) -> Result<Self, HardwareLibError> {
        let mut targets = HashMap::new();
        let mut interfaces = HashMap::new();

        let base = base_path.as_ref();
        let targets_dir = base.join("targets");
        let interfaces_dir = base.join("interfaces");

        // Load targets
        if targets_dir.exists() {
            for entry in std::fs::read_dir(&targets_dir)
                .map_err(|e| HardwareLibError::IoError(e.to_string()))?
            {
                let entry = entry.map_err(|e| HardwareLibError::IoError(e.to_string()))?;
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "toml") {
                    let content = std::fs::read_to_string(&path)
                        .map_err(|e| HardwareLibError::IoError(e.to_string()))?;
                    let target: TargetProfile = toml::from_str(&content)
                        .map_err(|e| HardwareLibError::ParseError(e.to_string()))?;
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    targets.insert(name, target);
                }
            }
        }

        // Load interfaces
        if interfaces_dir.exists() {
            for entry in std::fs::read_dir(&interfaces_dir)
                .map_err(|e| HardwareLibError::IoError(e.to_string()))?
            {
                let entry = entry.map_err(|e| HardwareLibError::IoError(e.to_string()))?;
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "toml") {
                    let content = std::fs::read_to_string(&path)
                        .map_err(|e| HardwareLibError::IoError(e.to_string()))?;
                    let interface: InterfaceProfile = toml::from_str(&content)
                        .map_err(|e| HardwareLibError::ParseError(e.to_string()))?;
                    interfaces.insert(interface.name.clone(), interface);
                }
            }
        }

        Ok(HardwareLib {
            targets,
            interfaces,
        })
    }

    pub fn get_memory_pragma(&self, target: &str, mem_type: &str) -> Option<String> {
        self.targets
            .get(target)
            .and_then(|t| t.memory_pragmas.get(mem_type))
            .cloned()
    }

    pub fn get_interface(&self, name: &str) -> Option<&InterfaceProfile> {
        self.interfaces.get(name)
    }
}

#[derive(Debug)]
pub enum HardwareLibError {
    IoError(String),
    ParseError(String),
    NotFound(String),
}

impl std::fmt::Display for HardwareLibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HardwareLibError::IoError(msg) => write!(f, "IO error: {}", msg),
            HardwareLibError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            HardwareLibError::NotFound(msg) => write!(f, "Not found: {}", msg),
        }
    }
}

impl std::error::Error for HardwareLibError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let lib = HardwareLib {
            targets: HashMap::new(),
            interfaces: HashMap::new(),
        };

        assert!(lib.get_memory_pragma("nonexistent", "bram").is_none());
        assert!(lib.get_interface("nonexistent").is_none());
    }
}
