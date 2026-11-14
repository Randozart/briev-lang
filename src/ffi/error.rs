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

//! FFI Error Handling
//!
//! Handles error conventions from TOML profiles:
//! - Error bounds (min/max values)
//! - Null pointer conventions
//! - Error return value conventions
//! - Err type generation for code generation

use std::collections::HashMap;

// The counsel contract layer: these are inert identity attributes, checked by
// `counsel-check` (the verification layer), not by brievc at runtime.
use counsel::{post, pre};

/// Error conventions from TOML profile
#[derive(Debug, Clone)]
pub struct ErrorConventions {
    /// Null pointer value (typically 0)
    pub null_pointer: i64,
    /// Valid error range minimum
    pub error_min: i64,
    /// Valid error range maximum
    pub error_max: i64,
    /// Error return value convention
    pub error_return: Option<i64>,
    /// Map of error code to error variant name
    pub error_variants: HashMap<i64, String>,
}

impl Default for ErrorConventions {
    fn default() -> Self {
        Self {
            null_pointer: 0,
            error_min: -1,
            error_max: -1,
            error_return: Some(-1),
            error_variants: HashMap::new(),
        }
    }
}

impl ErrorConventions {
    /// Parse from TOML table (from profile [conventions] section)
    pub fn from_toml(table: Option<&toml::Value>) -> Self {
        let mut conv = Self::default();

        if let Some(t) = table {
            if let Some(v) = t.get("null_pointer").and_then(|v| v.as_integer()) {
                conv.null_pointer = v;
            }
            if let Some(v) = t.get("error").and_then(|v| v.as_table()) {
                if let Some(min) = v.get("min").and_then(|v| v.as_integer()) {
                    conv.error_min = min;
                }
                if let Some(max) = v.get("max").and_then(|v| v.as_integer()) {
                    conv.error_max = max;
                }
                if let Some(ret) = v.get("return").and_then(|v| v.as_integer()) {
                    conv.error_return = Some(ret);
                }
            }
        }

        conv
    }

    /// Check if a return value indicates an error
    pub fn is_error(&self, value: i64) -> bool {
        // Check if value is in error range
        if self.error_min <= self.error_max {
            return value >= self.error_min && value <= self.error_max;
        }
        // Or matches error return convention
        if let Some(err_ret) = self.error_return {
            return value == err_ret;
        }
        // Default: negative values are errors
        value < 0
    }

    /// Check if a pointer is null
    #[pre(value == self.null_pointer)]
    #[post(term == 1)]
    pub fn is_null(&self, value: i64) -> bool {
        value == self.null_pointer
    }
}

/// Built-in Err type variants for Briev FFI
#[derive(Debug, Clone)]
pub enum ErrVariant {
    /// I/O error with code and message
    IoError { code: i64, message: String },
    /// Type mapping error
    MappingError { expected: String, got: String },
    /// Value out of bounds
    BoundsError { min: i64, max: i64, value: i64 },
    /// Void return when non-void expected
    VoidReturn { return_value: String },
    /// Generic error
    Generic { message: String },
}

impl ErrVariant {
    /// Generate C code for this error variant
    pub fn generate_c(&self) -> String {
        match self {
            ErrVariant::IoError { code, message } => {
                format!(
                    "(Err){{ .tag = IoError, .code = {}, .message = \"{}\" }}",
                    code, message
                )
            }
            ErrVariant::MappingError { expected, got } => {
                format!(
                    "(Err){{ .tag = MappingError, .expected = \"{}\", .got = \"{}\" }}",
                    expected, got
                )
            }
            ErrVariant::BoundsError { min, max, value } => {
                format!(
                    "(Err){{ .tag = BoundsError, .min = {}, .max = {}, .value = {} }}",
                    min, max, value
                )
            }
            ErrVariant::VoidReturn { return_value } => {
                format!(
                    "(Err){{ .tag = VoidReturn, .return_value = \"{}\" }}",
                    return_value
                )
            }
            ErrVariant::Generic { message } => {
                format!("(Err){{ .tag = Generic, .message = \"{}\" }}", message)
            }
        }
    }
}

/// Generate bounds checking code for an FFI call
pub fn generate_bounds_check(
    conventions: &ErrorConventions,
    result_var: &str,
    error_label: &str,
) -> String {
    // Generate: if (is_error(result)) goto error_label;
    let check = if conventions.error_min <= conventions.error_max {
        format!(
            "({} >= {} && {} <= {})",
            result_var, conventions.error_min, result_var, conventions.error_max
        )
    } else if let Some(err_ret) = conventions.error_return {
        format!("({} == {})", result_var, err_ret)
    } else {
        format!("({} < 0)", result_var)
    };

    format!("    if ({}) goto {};\n", check, error_label)
}

/// Generate null pointer check
pub fn generate_null_check(
    conventions: &ErrorConventions,
    ptr_var: &str,
    error_label: &str,
) -> String {
    format!(
        "    if ({} == {}) goto {};\n",
        ptr_var, conventions.null_pointer, error_label
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_conventions_default() {
        let conv = ErrorConventions::default();
        assert!(conv.is_error(-1));
        assert!(conv.is_null(0));
    }

    #[test]
    fn test_error_conventions_custom() {
        let conv = ErrorConventions {
            null_pointer: 0,
            error_min: 0,
            error_max: 0,
            error_return: Some(0),
            error_variants: HashMap::new(),
        };
        assert!(conv.is_error(0));
        assert!(!conv.is_error(1));
    }

    #[test]
    fn test_bounds_check_generation() {
        // Use error_return convention for cleaner test
        // Set error_min > error_max to disable range check and use error_return
        let conv = ErrorConventions {
            null_pointer: 0,
            error_min: 1, // > error_max to skip range check
            error_max: 0, //
            error_return: Some(-1),
            error_variants: HashMap::new(),
        };
        let check = generate_bounds_check(&conv, "result", "error_label");
        assert!(check.contains("result == -1"));
    }
}
