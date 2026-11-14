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

//! Python Analyzer
//!
//! Analyzes Python files to extract function signatures.
//! Supports:
//! - Regular Python files (.py) with type annotations
//! - Type stub files (.pyi) - easier to parse
//!
//! Python type mapping:
//! - int -> Int
//! - float -> Float
//! - str -> String
//! - bytes -> Data
//! - bool -> Bool
//! - list -> List<Value>
//! - dict -> Data
//! - tuple -> List<Value>
//! - set -> Data
//! - Optional[T] -> T | Void
//! - Union[T, U] -> T | U
//! - Callable[[args], ret] -> Value

use super::AnalyzedFunction;
use std::path::Path;

/// Analyze a Python file
pub fn analyze_python(path: &Path) -> Result<Vec<AnalyzedFunction>, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;

    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match extension {
        "pyi" => analyze_python_stub(&content),
        "py" => analyze_python_file(&content),
        _ => Err(format!("Unsupported Python file type: {}", extension)),
    }
}

/// Analyze Python type stub file (.pyi) - simpler parsing
fn analyze_python_stub(content: &str) -> Result<Vec<AnalyzedFunction>, String> {
    let mut functions = Vec::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Function: def name(params) -> return_type: ...
        if let Some(func) = parse_py_function(line, true) {
            functions.push(func);
        }

        // Async function: async def name(params) -> return_type: ...
        if let Some(func) = parse_py_async_function(line, true) {
            functions.push(func);
        }
    }

    Ok(functions)
}

/// Analyze regular Python file
fn analyze_python_file(content: &str) -> Result<Vec<AnalyzedFunction>, String> {
    let mut functions = Vec::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Function: def name(params) -> return_type: ...
        if let Some(func) = parse_py_function(line, false) {
            functions.push(func);
        }

        // Async function: async def name(params) -> return_type: ...
        if let Some(func) = parse_py_async_function(line, false) {
            functions.push(func);
        }
    }

    Ok(functions)
}

/// Parse Python function declaration
fn parse_py_function(line: &str, _is_stub: bool) -> Option<AnalyzedFunction> {
    let line = line.trim();

    // Remove decorators but keep function
    let line = line
        .strip_prefix("@")
        .and_then(|l| {
            l.find(|c: char| !c.is_alphanumeric() && c != '_')
                .and_then(|i| {
                    l[i..]
                        .find(|c: char| c.is_alphabetic())
                        .map(|j| &l[i + j..])
                })
        })
        .unwrap_or(line);

    // Must start with def
    let line = line.strip_prefix("def ")?;

    // Find function name
    let name_end = line.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    let name = line[..name_end].to_string();

    if name.is_empty() {
        return None;
    }

    let rest = line[name_end..].trim();

    // Find parameters
    let params_start = rest.find('(')?;
    let params_end = rest.find(')')?;
    let params_str = &rest[params_start + 1..params_end];

    let parameters = parse_py_parameters(params_str);

    // Find return type (if present)
    let return_type = if let Some(arrow_pos) = rest[params_end + 1..].find("->") {
        let ret_str = rest[params_end + 1 + arrow_pos + 2..].trim();
        // Remove trailing colon, body (": pass"), or comment
        let ret_str = ret_str
            .split(|c: char| c == ':' || c == '#' || c.is_whitespace())
            .next()
            .unwrap_or(ret_str)
            .trim();
        py_type_to_briev(ret_str)
    } else {
        "Void".to_string()
    };

    Some(AnalyzedFunction {
        name,
        return_type,
        parameters,
        is_variadic: params_str.contains("*"),
        comments: Vec::new(),
    })
}

/// Parse Python async function declaration
fn parse_py_async_function(line: &str, is_stub: bool) -> Option<AnalyzedFunction> {
    let line = line.trim();

    // Remove decorators
    let line = line
        .strip_prefix("@")
        .and_then(|l| {
            l.find(|c: char| !c.is_alphanumeric() && c != '_')
                .and_then(|i| {
                    l[i..]
                        .find(|c: char| c.is_alphabetic())
                        .map(|j| &l[i + j..])
                })
        })
        .unwrap_or(line);

    // Must start with async def
    let line = line.strip_prefix("async def ")?;

    // Everything after "async def " is same as regular function
    if let Some(mut func) = parse_py_function_internal(line, is_stub) {
        func.name = format!("async_{}", func.name);
        func.comments.push("async".to_string());
        Some(func)
    } else {
        None
    }
}

/// Internal function parser
fn parse_py_function_internal(line: &str, _is_stub: bool) -> Option<AnalyzedFunction> {
    // Find function name
    let name_end = line.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    let name = line[..name_end].to_string();

    if name.is_empty() {
        return None;
    }

    let rest = line[name_end..].trim();

    // Find parameters
    let params_start = rest.find('(')?;
    let params_end = rest.find(')')?;
    let params_str = &rest[params_start + 1..params_end];

    let parameters = parse_py_parameters(params_str);

    // Find return type
    let return_type = if let Some(arrow_pos) = rest[params_end + 1..].find("->") {
        let ret_str = rest[params_end + 1 + arrow_pos + 2..].trim();
        let ret_str = ret_str
            .trim_end_matches(':')
            .trim()
            .split('#')
            .next()
            .unwrap_or(ret_str)
            .trim();
        py_type_to_briev(ret_str)
    } else {
        "Void".to_string()
    };

    Some(AnalyzedFunction {
        name,
        return_type,
        parameters,
        is_variadic: params_str.contains("*"),
        comments: Vec::new(),
    })
}

/// Parse Python parameter list "(a: Type, b: Type = 1, *args, **kwargs)"
fn parse_py_parameters(params_str: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();

    if params_str.trim().is_empty() {
        return params;
    }

    let mut depth = 0;
    let mut param_start = 0;
    let mut seen_keyword_only = false;
    let mut seen_var_positional = false;
    let mut seen_var_keyword = false;

    for (i, c) in params_str.char_indices() {
        match c {
            '[' | '(' | '{' | '<' => depth += 1,
            ']' | ')' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => {
                if let Some(param) = parse_py_single_param(
                    &params_str[param_start..i],
                    seen_keyword_only,
                    seen_var_positional,
                    seen_var_keyword,
                ) {
                    params.push(param);
                }
                param_start = i + 1;
            }
            '*' if depth == 0 => {
                if params_str[param_start..].trim().starts_with("**") {
                    seen_var_keyword = true;
                } else if params_str[param_start..].trim() == "*" {
                    seen_keyword_only = true;
                } else {
                    seen_var_positional = true;
                }
            }
            _ => {}
        }
    }

    // Last parameter
    let last = params_str[param_start..].trim();
    if !last.is_empty() {
        if let Some(param) = parse_py_single_param(
            last,
            seen_keyword_only,
            seen_var_positional,
            seen_var_keyword,
        ) {
            params.push(param);
        }
    }

    params
}

/// Parse single Python parameter
fn parse_py_single_param(
    param: &str,
    _keyword_only: bool,
    _var_positional: bool,
    _var_keyword: bool,
) -> Option<(String, String)> {
    let param = param.trim();

    // Handle *args and **kwargs
    if param.starts_with("**") {
        let name = param.strip_prefix("**").unwrap_or(param).trim();
        if name.is_empty() {
            return Some(("kwargs".to_string(), "Blob".to_string()));
        }
        return Some((name.to_string(), "Blob".to_string()));
    }

    if param.starts_with("*") {
        let name = param.strip_prefix("*").unwrap_or(param).trim();
        if name.is_empty() {
            return Some(("args".to_string(), "List<Value>".to_string()));
        }
        return Some((name.to_string(), "List<Value>".to_string()));
    }

    // Handle default values: "name: Type = default"
    let (_name, type_str) = if let Some(eq_pos) = param.find('=') {
        let before_eq = param[..eq_pos].trim();
        let _after_eq = param[eq_pos + 1..].trim();

        // before_eq might be "name: Type" or just "name"
        if let Some(colon_pos) = before_eq.find(':') {
            let name = before_eq[..colon_pos].trim().to_string();
            let type_str = before_eq[colon_pos + 1..].trim().to_string();
            (name, type_str)
        } else {
            (before_eq.to_string(), "Int".to_string()) // Default to Int for untyped
        }
    } else {
        // No default value
        if let Some(colon_pos) = param.find(':') {
            let name = param[..colon_pos].trim().to_string();
            let type_str = param[colon_pos + 1..].trim().to_string();
            (name, type_str)
        } else {
            (param.to_string(), "Int".to_string()) // Default to Int for untyped
        }
    };

    Some((_name, py_type_to_briev(&type_str)))
}

/// Convert Python type annotation to Briev type
pub fn py_type_to_briev(py_type: &str) -> String {
    let t = py_type.trim();

    // Handle None
    if t == "None" {
        return "Void".to_string();
    }

    // Handle simple types
    match t {
        "int" | "Int" | "Integer" => "Int".to_string(),
        "float" | "Float" => "Float".to_string(),
        "str" | "Str" | "String" => "String".to_string(),
        "bytes" | "Bytes" => "Blob".to_string(),
        "bytearray" => "Blob".to_string(),
        "bool" | "Bool" | "Boolean" => "Bool".to_string(),
        "complex" => "Custom(Complex)".to_string(),
        "object" | "Object" => "Value".to_string(),
        "type" | "Type" => "Custom(Type)".to_string(),

        // Collection types
        "list" | "List" | "tuple" | "Tuple" => "List<Value>".to_string(),
        "dict" | "Dict" | "Dictionary" | "Mapping" | "MutableMapping" => "Blob".to_string(),
        "set" | "Set" | "FrozenSet" | "MutableSet" => "Blob".to_string(),

        // Optional types
        "optional" | "Optional" => "Value".to_string(), // Optional[X] needs special handling

        // Callable
        "callable" | "Callable" | "Function" => "Value".to_string(),

        // Iterators
        "iterator" | "Iterator" | "Iterable" | "Generator" | "AsyncGenerator" => {
            "List<Value>".to_string()
        }

        // Numeric
        "decimal" | "Decimal" | "Fraction" => "Float".to_string(),

        // Date/Time
        "datetime" | "date" | "time" | "timedelta" => "Custom(DateTime)".to_string(),

        // Errors/Exceptions
        "BaseException" | "Exception" | "Error" => "Error".to_string(),
        "TypeError" | "ValueError" | "RuntimeError" | "IOError" | "OSError"
        | "FileNotFoundError" => "Error".to_string(),

        // IO types
        "TextIO" | "BinaryIO" | "IO" | "FileIO" => "Custom(File)".to_string(),

        _ => {
            // Handle generic types: List[int], Dict[str, int], Optional[str], Union[int, str]
            if t.starts_with("List[") || t.starts_with("list[") {
                "List<Value>".to_string()
            } else if t.starts_with("Optional[") || t.starts_with("optional[") {
                // Strip Optional[] wrapper
                let inner = t
                    .strip_prefix("Optional[")
                    .or_else(|| t.strip_prefix("optional["))
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(t);
                py_type_to_briev(inner)
            } else if t.starts_with("Union[") || t.starts_with("union[") {
                // Union - take first type
                let inner = t
                    .strip_prefix("Union[")
                    .or_else(|| t.strip_prefix("union["))
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(t);
                inner
                    .split(',')
                    .next()
                    .map(|s| py_type_to_briev(s.trim()))
                    .unwrap_or_else(|| "Value".to_string())
            } else if t.starts_with("Dict[")
                || t.starts_with("dict[")
                || t.starts_with("Mapping[")
                || t.starts_with("mapping[")
            {
                "Blob".to_string()
            } else if t.starts_with("Tuple[") || t.starts_with("tuple[") {
                "List<Value>".to_string()
            } else if t.starts_with("Set[") || t.starts_with("set[") {
                "Blob".to_string()
            } else if t.starts_with("Callable[") || t.starts_with("Callable(") {
                "Value".to_string()
            } else if t.starts_with("Generator[") || t.starts_with("generator[") {
                "List<Value>".to_string()
            } else if t.starts_with("Iterator[") || t.starts_with("iterator[") {
                "List<Value>".to_string()
            } else if t.ends_with("Error") || t.ends_with("Exception") {
                "Error".to_string()
            } else {
                // Pass through unknown types as custom
                t.to_string()
            }
        }
    }
}

/// Generate frgn sig from analyzed function
pub fn py_func_to_frgn_sig(func: &AnalyzedFunction) -> String {
    let params: Vec<String> = func
        .parameters
        .iter()
        .map(|(n, t)| format!("{}: {}", n, t))
        .collect();

    format!(
        "frgn sig {}: {} -> {};",
        func.name,
        params.join(", "),
        func.return_type
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_py_type_to_briev() {
        assert_eq!(py_type_to_briev("int"), "Int");
        assert_eq!(py_type_to_briev("str"), "String");
        assert_eq!(py_type_to_briev("list"), "List<Value>");
        assert_eq!(py_type_to_briev("None"), "Void");
        assert_eq!(py_type_to_briev("Optional[str]"), "String");
        assert_eq!(py_type_to_briev("Union[int, str]"), "Int");
    }

    #[test]
    fn test_parse_py_function() {
        let result = parse_py_function("def add(a: int, b: int) -> int: pass", false);
        assert!(result.is_some());
        let func = result.unwrap();
        assert_eq!(func.name, "add");
        assert_eq!(func.return_type, "Int");
        assert_eq!(func.parameters.len(), 2);
    }

    #[test]
    fn test_parse_py_function_with_defaults() {
        let result = parse_py_function(
            "def greet(name: str, greeting: str = 'Hello') -> str: pass",
            false,
        );
        assert!(result.is_some());
        let func = result.unwrap();
        assert_eq!(func.parameters.len(), 2);
    }

    #[test]
    fn test_parse_py_function_varargs() {
        let result = parse_py_function("def sum(*args: int) -> int: pass", false);
        assert!(result.is_some());
        let func = result.unwrap();
        assert!(func.is_variadic);
    }
}
