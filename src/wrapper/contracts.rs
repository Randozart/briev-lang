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

//! Contract Inference - Suggests pre/post conditions based on function patterns

use super::AnalyzedFunction;

/// Inference rule for contract conditions
#[derive(Debug, Clone)]
pub struct ContractRule {
    pub pattern: String,
    pub precondition: String,
    pub postcondition: String,
    pub description: String,
}

/// Get all contract inference rules
pub fn get_contract_rules() -> Vec<ContractRule> {
    vec![
        // Read functions
        ContractRule {
            pattern: "read".to_string(),
            precondition: "buffer != null && size > 0".to_string(),
            postcondition: "result >= 0 && result <= size".to_string(),
            description: "Read function: returns bytes read, limited by buffer size".to_string(),
        },
        // Write functions
        ContractRule {
            pattern: "write".to_string(),
            precondition: "buffer != null && size > 0".to_string(),
            postcondition: "result >= 0 && result <= size".to_string(),
            description: "Write function: returns bytes written".to_string(),
        },
        // Parse functions
        ContractRule {
            pattern: "parse".to_string(),
            precondition: "input.len() > 0".to_string(),
            postcondition: "result.is_ok() || result.is_err()".to_string(),
            description: "Parse function: returns Result".to_string(),
        },
        // Alloc functions
        ContractRule {
            pattern: "alloc".to_string(),
            precondition: "size > 0".to_string(),
            postcondition: "result != null".to_string(),
            description: "Allocate function: returns pointer".to_string(),
        },
        // Free functions
        ContractRule {
            pattern: "free".to_string(),
            precondition: "ptr != null".to_string(),
            postcondition: "true".to_string(),
            description: "Free function: frees memory".to_string(),
        },
        // Init functions
        ContractRule {
            pattern: "init".to_string(),
            precondition: "true".to_string(),
            postcondition: "result != null".to_string(),
            description: "Init function: returns initialized handle".to_string(),
        },
        // Close/Destroy functions
        ContractRule {
            pattern: "close".to_string(),
            precondition: "handle != null".to_string(),
            postcondition: "true".to_string(),
            description: "Close function: releases resources".to_string(),
        },
        ContractRule {
            pattern: "destroy".to_string(),
            precondition: "handle != null".to_string(),
            postcondition: "true".to_string(),
            description: "Destroy function: frees resources".to_string(),
        },
        // Get functions (accessors)
        ContractRule {
            pattern: "get".to_string(),
            precondition: "true".to_string(),
            postcondition: "result != null".to_string(),
            description: "Getter function: returns value".to_string(),
        },
        // Set functions (mutators)
        ContractRule {
            pattern: "set".to_string(),
            precondition: "true".to_string(),
            postcondition: "true".to_string(),
            description: "Setter function: sets value".to_string(),
        },
        // Len/Size functions
        ContractRule {
            pattern: "len".to_string(),
            precondition: "true".to_string(),
            postcondition: "result >= 0".to_string(),
            description: "Length function: returns size".to_string(),
        },
        ContractRule {
            pattern: "size".to_string(),
            precondition: "true".to_string(),
            postcondition: "result >= 0".to_string(),
            description: "Size function: returns size".to_string(),
        },
        // Copy functions
        ContractRule {
            pattern: "copy".to_string(),
            precondition: "src != null && dst != null && len > 0".to_string(),
            postcondition: "result == len".to_string(),
            description: "Copy function: copies data".to_string(),
        },
    ]
}

/// Find matching rule for a function
pub fn find_matching_rule(func_name: &str) -> Option<ContractRule> {
    let rules = get_contract_rules();
    let name_lower = func_name.to_lowercase();

    for rule in rules {
        if name_lower.contains(&rule.pattern) {
            return Some(rule);
        }
    }

    None
}

/// Generate preconditions for a function based on its name and parameters
pub fn infer_preconditions(func: &AnalyzedFunction) -> Vec<String> {
    let mut preconditions = Vec::new();

    // First try to find a matching rule by name
    if let Some(rule) = find_matching_rule(&func.name) {
        // Parse and adapt the rule's precondition
        for cond in rule.precondition.split("&&") {
            let cond = cond.trim();

            // Check if condition applies to this function's parameters
            let param_names: Vec<&str> = func.parameters.iter().map(|(n, _)| n.as_str()).collect();

            // Adapt condition to actual parameter names if needed
            let adapted = adapt_condition(cond, &param_names);
            if !adapted.is_empty() {
                preconditions.push(adapted);
            }
        }
    }

    // Add parameter-based checks not covered by rules
    for (name, c_type) in &func.parameters {
        let type_lower = c_type.to_lowercase();

        // Pointer parameters should not be null
        if type_lower.contains('*') || type_lower.contains("ptr") || type_lower.contains("buffer") {
            if !preconditions
                .iter()
                .any(|p| p.contains(&format!("{} != null", name)))
            {
                preconditions.push(format!("{} != null", name));
            }
        }

        // Size/length parameters should be positive
        let name_lower = name.to_lowercase();
        if name_lower.contains("size")
            || name_lower.contains("len")
            || name_lower.contains("length")
        {
            if !preconditions
                .iter()
                .any(|p| p.contains(&format!("{} > 0", name)))
            {
                preconditions.push(format!("{} > 0", name));
            }
        }
    }

    if preconditions.is_empty() {
        preconditions.push("true".to_string());
    }

    preconditions
}

/// Generate postconditions for a function based on its name and return type
pub fn infer_postconditions(func: &AnalyzedFunction) -> Vec<String> {
    let mut postconditions = Vec::new();

    let return_type = func.return_type.to_lowercase();
    let _name_lower = func.name.to_lowercase();

    // First try to find a matching rule by name
    if let Some(rule) = find_matching_rule(&func.name) {
        for cond in rule.postcondition.split("&&") {
            let cond = cond.trim();

            if cond != "true" {
                postconditions.push(cond.to_string());
            }
        }
    }

    // Add return type based checks
    if return_type.contains("int") || return_type.contains("size") || return_type.contains("len") {
        if !postconditions.iter().any(|p| p.contains("result >= 0")) {
            postconditions.push("result >= 0".to_string());
        }
    } else if return_type.contains("char*")
        || return_type.contains("void*")
        || return_type.contains("ptr")
    {
        if !postconditions.iter().any(|p| p.contains("result != null")) {
            postconditions.push("result != null".to_string());
        }
    } else if return_type == "void" {
        postconditions.push("true".to_string());
    }

    if postconditions.is_empty() {
        postconditions.push("true".to_string());
    }

    postconditions
}

/// Adapt a condition template to actual parameter names
fn adapt_condition(condition: &str, param_names: &[&str]) -> String {
    let mut result = condition.to_string();

    // Common parameter name mappings
    let replacements = vec![
        ("buffer", param_names.first().copied().unwrap_or("buffer")),
        ("ptr", param_names.first().copied().unwrap_or("ptr")),
        (
            "size",
            param_names
                .iter()
                .find(|n| n.contains("size"))
                .copied()
                .unwrap_or("size"),
        ),
        (
            "len",
            param_names
                .iter()
                .find(|n| n.contains("len"))
                .copied()
                .unwrap_or("len"),
        ),
    ];

    for (from, to) in replacements {
        if result.contains(from) && !result.contains(to) {
            result = result.replace(from, to);
        }
    }

    // If condition still contains placeholder, skip it
    if result.contains("buffer")
        || result.contains("ptr")
        || result.contains("size")
        || result.contains("len")
    {
        if param_names.is_empty() {
            return String::new();
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_matching_rule() {
        let rule = find_matching_rule("read_file").unwrap();
        assert_eq!(rule.pattern, "read");
    }

    #[test]
    fn test_infer_preconditions() {
        let func = AnalyzedFunction {
            name: "read".to_string(),
            return_type: "Int".to_string(),
            parameters: vec![
                ("buf".to_string(), "Blob".to_string()),
                ("len".to_string(), "Int".to_string()),
            ],
            is_variadic: false,
            comments: vec![],
        };

        let pre = infer_preconditions(&func);
        assert!(!pre.is_empty());
    }
}
