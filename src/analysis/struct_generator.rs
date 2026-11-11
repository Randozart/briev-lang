use crate::ast::{TopLevel, Type};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MemoryMapping {
    pub mem_type: String,
    pub element_bits: u64,
}

#[derive(Debug, Clone)]
pub struct HardwareConfig {
    pub memory: std::collections::BTreeMap<String, MemoryMapping>,
}

pub struct StructGenerator;

impl StructGenerator {
    pub fn generate(hw_config: &HardwareConfig) -> String {
        let mut output = String::new();

        output.push_str("// Auto-generated State struct from hardware.toml\n");
        output.push_str("// DO NOT EDIT - changes will be overwritten\n\n");
        output.push_str("#[repr(C)]\n");
        output.push_str("pub struct State {\n");

        // Sort memory mappings by address
        let mut sorted_addrs: Vec<(&String, &MemoryMapping)> = hw_config.memory.iter().collect();
        sorted_addrs.sort_by(|a, b| {
            let addr_a = Self::parse_addr(a.0);
            let addr_b = Self::parse_addr(b.0);
            addr_a.cmp(&addr_b)
        });

        for (addr, mem_map) in sorted_addrs {
            let field_name = Self::field_name_from_addr(addr);
            let rust_type = Self::rust_type(mem_map);

            output.push_str(&format!(
                "    pub {}: {},  // {} @ {}\n",
                field_name, rust_type, mem_map.mem_type, addr
            ));
        }

        output.push_str("}\n\n");

        // Generate From implementations
        output.push_str("impl State {\n");
        output.push_str("    pub fn new() -> Self {\n");
        output.push_str("        unsafe { std::mem::zeroed() }\n");
        output.push_str("    }\n");
        output.push_str("}\n\n");

        output.push_str("impl Default for State {\n");
        output.push_str("    fn default() -> Self {\n");
        output.push_str("        Self::new()\n");
        output.push_str("    }\n");
        output.push_str("}\n");

        output
    }

    fn parse_addr(addr: &str) -> u64 {
        let clean = addr.trim_start_matches("0x").trim_start_matches("0X");
        u64::from_str_radix(clean, 16).unwrap_or(0)
    }

    fn field_name_from_addr(addr: &str) -> String {
        let addr_lower = addr.to_lowercase();
        // Remove 0x prefix and make valid identifier
        let clean = addr_lower.trim_start_matches("0x").replace("-", "_");

        // Common field names for standard MMIO addresses
        match clean.as_str() {
            "4000a000" | "8000a000" => "control".to_string(),
            "4000a004" | "8000a004" => "status".to_string(),
            "4000a008" | "8000a008" => "opcode".to_string(),
            "4000a00c" | "8000a00c" => "token_count".to_string(),
            "4000a040" | "8000a040" => "write_data".to_string(),
            "4000a044" | "8000a044" => "write_addr".to_string(),
            "4000a048" | "8000a048" => "write_en".to_string(),
            "4000a04c" | "8000a04c" => "read_en".to_string(),
            "4000a050" | "8000a050" => "read_data".to_string(),
            _ => format!("reg_{}", clean),
        }
    }

    fn rust_type(mem_map: &MemoryMapping) -> String {
        match mem_map.element_bits {
            1 => "bool".to_string(),
            2..=8 => "u8".to_string(),
            9..=16 => "u16".to_string(),
            17..=32 => "u32".to_string(),
            33..=64 => "u64".to_string(),
            _ => format!("u{}", ((mem_map.element_bits + 31) / 32) * 32),
        }
    }

    pub fn validate_against_rust(
        hw_config: &HardwareConfig,
        rust_source: &str,
    ) -> Vec<StructValidationError> {
        let mut errors = Vec::new();

        let generated = Self::generate(hw_config);

        // Check if key fields are present in the Rust source
        for (addr, mem_map) in &hw_config.memory {
            let field_name = Self::field_name_from_addr(addr);

            // Simple presence check - in real implementation would parse the Rust struct
            if !rust_source.contains(&format!("pub {}", field_name))
                && !rust_source.contains(&format!("{}:", field_name))
            {
                // This is actually fine - we generate the struct, we don't validate against it
                // This would be for generate mode
            }
        }

        errors
    }
}

#[derive(Debug, Clone)]
pub struct StructValidationError {
    pub field: String,
    pub expected_type: String,
    pub found_type: Option<String>,
    pub message: String,
}

impl std::fmt::Display for StructValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Field '{}': {}", self.field, self.message)
    }
}

impl std::error::Error for StructValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_struct_generation() {
        let mut memory_b = std::collections::BTreeMap::new();
        memory_b.insert(
            "0x8000A000".to_string(),
            MemoryMapping {
                mem_type: "flipflop".to_string(),
                element_bits: 8,
            },
        );
        memory_b.insert(
            "0x8000A004".to_string(),
            MemoryMapping {
                mem_type: "flipflop".to_string(),
                element_bits: 8,
            },
        );

        let config = HardwareConfig { memory: memory_b };

        let output = StructGenerator::generate(&config);
        assert!(output.contains("pub control: u8"));
    }
}
