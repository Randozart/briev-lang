use crate::ast::TopLevel;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum AddressSpace {
    Ddr4,         // 0x00000000 - 0xFFFFFFFF: CPU accessible main memory
    Mmio(u64),    // MMIO range, CPU accessible via bus
    FpgaInternal, // 0x40A80000+: FPGA internal BRAM/URAM, NOT CPU accessible
    Unknown,
}

pub struct AddressSpaceAnalyzer {
    address_spaces: HashMap<String, AddressSpace>,
}

impl AddressSpaceAnalyzer {
    pub fn new() -> Self {
        AddressSpaceAnalyzer {
            address_spaces: HashMap::new(),
        }
    }

    fn classify_address(addr: u64) -> AddressSpace {
        // FPGA Internal BRAM: typically 0x40A80000+
        if addr >= 0x40A80000 && addr < 0x50000000 {
            return AddressSpace::FpgaInternal;
        }

        // UltraRAM: 0x40B00000+
        if addr >= 0x40B00000 && addr < 0x5000000 {
            return AddressSpace::FpgaInternal;
        }

        // MMIO ranges (common for AXI4-Lite)
        // 0x4000A000 - 0x4000AFFF (first AXI slave)
        // 0x8000A000 - 0x8000AFFF (second AXI slave - KV260)
        if (0x4000A000..=0x4000AFFF).contains(&addr) || (0x8000A000..=0x8000AFFF).contains(&addr) {
            return AddressSpace::Mmio(addr);
        }

        // Other MMIO ranges
        if (0x40000000..=0x4FFFFFFF).contains(&addr) || (0x80000000..=0x8FFFFFFF).contains(&addr) {
            return AddressSpace::Mmio(addr);
        }

        // Default: treat as DDR4 (CPU accessible)
        AddressSpace::Ddr4
    }

    pub fn classify(&self, addr_str: &str) -> AddressSpace {
        self.address_spaces
            .get(addr_str)
            .cloned()
            .unwrap_or(AddressSpace::Unknown)
    }

    pub fn is_cpu_accessible(&self, addr_str: &str) -> bool {
        match self.classify(addr_str) {
            AddressSpace::Ddr4 => true,
            AddressSpace::Mmio(_) => true,
            AddressSpace::FpgaInternal => false,
            AddressSpace::Unknown => true, // Assume accessible if unknown
        }
    }

    pub fn is_fpga_internal(&self, addr_str: &str) -> bool {
        matches!(self.classify(addr_str), AddressSpace::FpgaInternal)
    }

    pub fn validate_program(&self, _items: &[TopLevel]) -> Vec<AddressValidationError> {
        Vec::new()
    }
}

#[derive(Debug, Clone)]
pub struct AddressValidationError {
    pub variable: String,
    pub address: String,
    pub message: String,
}

impl std::fmt::Display for AddressValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Address violation for '{}' at {}: {}",
            self.variable, self.address, self.message
        )
    }
}

impl std::error::Error for AddressValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_address_classification() {
        // FPGA internal
        assert!(matches!(
            AddressSpaceAnalyzer::classify_address(0x40A80000),
            AddressSpace::FpgaInternal
        ));

        // MMIO
        assert!(matches!(
            AddressSpaceAnalyzer::classify_address(0x8000A000),
            AddressSpace::Mmio(0x8000A000)
        ));

        // DDR4 (default)
        assert!(matches!(
            AddressSpaceAnalyzer::classify_address(0x10000000),
            AddressSpace::Ddr4
        ));
    }
}
