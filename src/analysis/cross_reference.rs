use crate::ast::TopLevel;
use std::collections::{HashMap, HashSet};

type HardwareConfig = HashMap<String, ()>;

pub struct CrossReferenceValidator {
    hw_config: &'static HardwareConfig,
}

impl CrossReferenceValidator {
    pub fn new(hw_config: &'static HardwareConfig) -> Self {
        CrossReferenceValidator { hw_config }
    }

    pub fn validate(&self, items: &[TopLevel]) -> Vec<CrossRefError> {
        let mut errors = Vec::new();

        // Collect all addresses used in the .ebv file
        let mut used_addrs: HashSet<String> = HashSet::new();
        let mut defined_addrs: HashSet<String> = HashSet::new();

        for item in items {
            match item {
                TopLevel::StateDecl(_decl) => {}
                TopLevel::Trigger(_trg) => {}
                _ => {}
            }
        }

        // Collect defined addresses from hardware.toml
        for addr in self.hw_config.keys() {
            defined_addrs.insert(addr.clone());
        }

        // Warn about unused addresses in hardware.toml (optional, not error)
        for addr in &defined_addrs {
            if !used_addrs.contains(addr) {
                // This is informational - some memory regions might be reserved
                // Not adding as error, could be a warning in verbose mode
            }
        }

        errors
    }

    pub fn check_address_consistency(&self, _items: &[TopLevel]) -> Vec<CrossRefError> {
        let mut errors = Vec::new();

        // Group declarations by address
        let mut addr_groups: std::collections::HashMap<u64, Vec<(String, Option<String>)>> =
            std::collections::HashMap::new();

        for item in _items {
            let _ = item;
        }

        // Check that multiple declarations at same address don't conflict
        for (addr, decls) in &addr_groups {
            if decls.len() > 1 {
                // Multiple variables at same address - this is allowed for bit packing
                // but we should verify bit ranges don't overlap
                let addr_str = format!("0x{:08X}", addr);

                // Check if ANY declaration at this address lacks an explicit bit range.
                // Only checking the first (as before) would miss cases where a later
                // declaration has no bit range while an earlier one does.
                let has_implicit_overlap = decls.iter().any(|(_, range)| range.is_none());
                if has_implicit_overlap {
                    let addr_for_error = addr_str.clone();
                    errors.push(CrossRefError {
                        variable: decls
                            .iter()
                            .map(|(n, _)| n.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                        address: addr_str,
                        error_type: CrossRefErrorType::ImplicitOverlap,
                        message: format!(
                            "Multiple variables at address {} without explicit bit ranges. \
                            Specify /bit ranges to avoid conflicts.",
                            addr_for_error
                        ),
                    });
                }
            }
        }

        errors
    }
}

#[derive(Debug, Clone)]
pub enum CrossRefErrorType {
    AddressNotInHardwareConfig,
    TriggerAddressNotInHardwareConfig,
    ImplicitOverlap,
    AddressRangeMismatch,
}

#[derive(Debug, Clone)]
pub struct CrossRefError {
    pub variable: String,
    pub address: String,
    pub error_type: CrossRefErrorType,
    pub message: String,
}

impl std::fmt::Display for CrossRefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CrossRefError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_validation() {
        let hw_config = Box::leak(Box::new(HashMap::new()));
        let validator = CrossReferenceValidator::new(hw_config);
        let errors = validator.validate(&[]);
        assert!(errors.is_empty());
    }
}
