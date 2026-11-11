use crate::errors::{Diagnostic, Severity};
use std::collections::{HashMap, HashSet};

pub fn cross_validate(
    schema_alias_names: &HashSet<String>,
    target_addresses: &HashMap<String, u64>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let schema_names: HashSet<&String> = schema_alias_names.iter().collect();
    let target_names: HashSet<&String> = target_addresses.keys().collect();

    for name in schema_names.difference(&target_names) {
        diagnostics.push(
            Diagnostic::new("HW008", Severity::Error, "missing target binding")
                .with_explanation(&format!(
                    "alias '{}' is declared in schema but has no address binding in target DBV",
                    name
                ))
                .with_hint(&format!(
                    "add: ALIAS {}: Type = @0x...; to the target .dbv file",
                    name
                )),
        );
    }

    for name in target_names.difference(&schema_names) {
        diagnostics.push(
            Diagnostic::new("HW009", Severity::Warning, "unreferenced target alias")
                .with_explanation(&format!(
                    "alias '{}' is bound in target DBV but not declared in any imported schema",
                    name
                ))
                .with_note(
                    "this may be intentional (reserved for future use) or indicate a stale binding",
                ),
        );
    }

    let mut addr_by_name: Vec<(&String, u64)> =
        target_addresses.iter().map(|(n, a)| (n, *a)).collect();
    addr_by_name.sort_by_key(|(_, a)| *a);

    let mut base_addrs: Vec<(String, u64)> = Vec::new();
    for (name, addr) in &addr_by_name {
        if !schema_alias_names.contains(*name) {
            continue;
        }
        for (existing_name, existing_addr) in &base_addrs {
            if *addr == *existing_addr {
                diagnostics.push(
                    Diagnostic::new("HW010", Severity::Error, "address overlap")
                        .with_explanation(&format!(
                            "alias '{}' and '{}' both map to address 0x{:X}",
                            name, existing_name, addr
                        ))
                        .with_hint("each alias must have a unique base address in the target DBV"),
                );
            }
        }
        base_addrs.push(((*name).clone(), *addr));
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_aliases_have_targets() {
        let mut schema = HashSet::new();
        schema.insert("gpio0".to_string());
        let mut target = HashMap::new();
        target.insert("gpio0".to_string(), 0xA0000000);
        let d = cross_validate(&schema, &target);
        assert!(d.is_empty(), "expected no diagnostics, got {:?}", d);
    }

    #[test]
    fn test_missing_target_binding_error() {
        let mut schema = HashSet::new();
        schema.insert("uart_debug".to_string());
        let target: HashMap<String, u64> = HashMap::new();
        let d = cross_validate(&schema, &target);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "HW008");
        assert!(d[0].explanation[0].contains("uart_debug"));
    }

    #[test]
    fn test_unreferenced_target_warning() {
        let schema: HashSet<String> = HashSet::new();
        let mut target = HashMap::new();
        target.insert("gic_distributor".to_string(), 0xF9010000);
        let d = cross_validate(&schema, &target);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "HW009");
        assert!(d[0].explanation[0].contains("gic_distributor"));
    }

    #[test]
    fn test_address_overlap_error() {
        let mut schema = HashSet::new();
        schema.insert("gpio0".to_string());
        schema.insert("gpio1".to_string());
        let mut target = HashMap::new();
        target.insert("gpio0".to_string(), 0xA0000000);
        target.insert("gpio1".to_string(), 0xA0000000);
        let d = cross_validate(&schema, &target);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "HW010");
        assert!(d[0].explanation[0].contains("gpio0") || d[0].explanation[0].contains("gpio1"));
    }
}
