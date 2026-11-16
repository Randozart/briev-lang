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

//! C Header Analyzer - Parses C header files to extract function signatures

use super::{AnalyzedFunction, c_type_is_pointer, c_type_to_briev, parse_c_signature};
use std::fs;
use std::path::Path;

/// Analyze a C header file and extract function declarations
pub fn analyze_c_header(header_path: &Path) -> Result<Vec<AnalyzedFunction>, String> {
    let content =
        fs::read_to_string(header_path).map_err(|e| format!("Failed to read header: {}", e))?;

    let mut functions = Vec::new();
    let mut in_macro_block = false;
    let mut current_comment = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Track multi-line comments
        if trimmed.starts_with("/*") {
            in_macro_block = true;
            if let Some(end) = trimmed.find("*/") {
                let comment = &trimmed[2..end];
                current_comment.push(comment.to_string());
                in_macro_block = false;
            }
            continue;
        }

        if in_macro_block {
            if let Some(end) = trimmed.find("*/") {
                let comment = &trimmed[..end];
                current_comment.push(comment.to_string());
                in_macro_block = false;
            }
            continue;
        }

        // Single-line comments
        if trimmed.starts_with("//") {
            current_comment.push(trimmed[2..].trim().to_string());
            continue;
        }

        // End of comment block
        if !current_comment.is_empty() && !trimmed.starts_with("/*") && !trimmed.starts_with("//") {
            current_comment.clear();
        }

        // Skip preprocessor directives except function-like macros
        if trimmed.starts_with('#') {
            continue;
        }

        // Skip struct/enum/typedef definitions
        if trimmed.starts_with("typedef")
            || trimmed.starts_with("struct")
            || trimmed.starts_with("enum")
        {
            continue;
        }

        // Skip extern "C" blocks
        if trimmed.contains("extern") && trimmed.contains("\"C\"") {
            continue;
        }

        // Try to parse as function signature
        if let Some(mut func) = parse_c_signature(trimmed) {
            // Attach collected comments
            func.comments = current_comment.clone();
            current_comment.clear();

            // Skip declarations without implementation hints (just prototypes)
            if !trimmed.contains('{') && !trimmed.contains(';') {
                continue;
            }

            functions.push(func);
        }
    }

    Ok(functions)
}

/// Convert C function to Briev frgn sig format
pub fn c_func_to_frgn_sig(func: &AnalyzedFunction) -> String {
    let mut params = Vec::new();

    for (name, c_type) in &func.parameters {
        let briev_type = c_type_to_briev(c_type);
        params.push(format!("{}: {}", name, briev_type));
    }

    let return_type = c_type_to_briev(&func.return_type);

    if params.is_empty() {
        format!("frgn sig {}() -> {};", func.name, return_type)
    } else {
        format!(
            "frgn sig {}({}) -> {};",
            func.name,
            params.join(", "),
            return_type
        )
    }
}

/// Generate suggested preconditions based on C function parameters
pub fn suggest_preconditions(func: &AnalyzedFunction) -> Vec<String> {
    let mut preconditions = Vec::new();

    for (name, c_type) in &func.parameters {
        if c_type_is_pointer(c_type) {
            // Pointer should not be null
            preconditions.push(format!("{} != null", name));
        }

        // Size parameters should be positive
        if name.to_lowercase().contains("size") || name.to_lowercase().contains("len") {
            preconditions.push(format!("{} > 0", name));
        }
    }

    if preconditions.is_empty() {
        preconditions.push("true".to_string());
    }

    preconditions
}

/// Generate suggested postconditions based on C function return type
pub fn suggest_postconditions(func: &AnalyzedFunction) -> Vec<String> {
    let mut postconditions = Vec::new();

    let return_type = func.return_type.to_lowercase();

    // Check return type
    if return_type.contains("int") || return_type.contains("size_t") {
        // Return value often indicates success/error
        postconditions.push(format!("result >= 0"));
    } else if return_type.contains("char*") || return_type.contains("void*") {
        // Pointer return type - check for null
        postconditions.push("result != null".to_string());
    } else if return_type == "void" {
        postconditions.push("true".to_string());
    }

    if postconditions.is_empty() {
        postconditions.push("true".to_string());
    }

    postconditions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_signature() {
        let sig = "int add(int a, int b)";
        let func = parse_c_signature(sig).unwrap();
        assert_eq!(func.name, "add");
        assert_eq!(func.return_type, "int");
        assert_eq!(func.parameters.len(), 2);
    }

    #[test]
    fn test_c_type_to_briev() {
        assert_eq!(c_type_to_briev("int"), "Int");
        assert_eq!(c_type_to_briev("float"), "Float");
        assert_eq!(c_type_to_briev("char*"), "String");
        assert_eq!(c_type_to_briev("void*"), "Blob");
    }
}
