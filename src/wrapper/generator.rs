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

//! File Generator - Generates Briev FFI files from analysis results

use super::c_analyzer::{c_func_to_frgn_sig, suggest_postconditions, suggest_preconditions};
use super::js_analyzer::js_func_to_frgn_sig;
use super::python_analyzer::py_func_to_frgn_sig;
use super::rust_analyzer::rust_func_to_frgn_sig;
use super::wasm_analyzer::wasm_func_to_frgn_sig;
use super::{AnalysisResult, AnalyzedFunction, c_type_to_briev};
use std::fs;
use std::path::Path;

/// Generate lib.bv content from analysis results
pub fn generate_lib_bv(result: &AnalysisResult) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "// Auto-generated wrapper for {}\n// Mapper: {}\n// Generated functions: {}\n\n",
        result.library_name,
        result.mapper,
        result.functions.len()
    ));

    output.push_str("// Foreign function declarations (frgn sig)\n");

    for func in &result.functions {
        let frgn_sig = match result.mapper.as_str() {
            "c" => c_func_to_frgn_sig(func),
            "rust" => rust_func_to_frgn_sig(func),
            "wasm" => wasm_func_to_frgn_sig(func),
            "js" => js_func_to_frgn_sig(func),
            "python" => py_func_to_frgn_sig(func),
            _ => format!("// Unknown mapper: {}", result.mapper),
        };

        // Add comments if present
        for comment in &func.comments {
            output.push_str(&format!("// {}\n", comment));
        }

        output.push_str(&frgn_sig);
        output.push_str("\n\n");
    }

    output.push_str(
        "// =============================================================================\n",
    );
    output.push_str("// User-defined implementations\n");
    output.push_str(
        "// =============================================================================\n\n",
    );

    // Generate template defns with suggested contracts
    for func in &result.functions {
        let preconditions = match result.mapper.as_str() {
            "c" => suggest_preconditions(func),
            _ => vec!["true".to_string()],
        };

        let postconditions = match result.mapper.as_str() {
            "c" => suggest_postconditions(func),
            _ => vec!["true".to_string()],
        };

        let params: Vec<String> = func
            .parameters
            .iter()
            .map(|(n, t)| format!("{}: {}", n, t))
            .collect();

        let params_str = if params.is_empty() {
            "".to_string()
        } else {
            format!(", {}", params.join(", "))
        };

        output.push_str(&format!(
            "// {}Implementation for {}\ndefn {}({}) -> {} [\n  {}  // precondition\n][\n  {}  // postcondition\n] {{\n  __raw_{}({})\n}};\n\n",
            if func.comments.is_empty() { "" } else { "\n" },
            func.name,
            func.name,
            params_str.trim_start_matches(", "),
            func.return_type,
            preconditions.join(" && "),
            postconditions.join(" && "),
            func.name,
            if params.is_empty() { "".to_string() } else { params_str.trim_start_matches(", ").to_string() }
        ));
    }

    output
}

/// Generate bindings.toml content from analysis results
pub fn generate_bindings_toml(result: &AnalysisResult) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "# Auto-generated bindings for {}\n# Mapper: {}\n\n",
        result.library_name, result.mapper,
    ));

    for func in &result.functions {
        output.push_str(&format!("[[functions]]\n"));
        output.push_str(&format!("name = \"{}\"\n", func.name));
        output.push_str(&format!(
            "location = \"{}::{}\"\n",
            result.library_name.replace('-', "_"),
            func.name
        ));
        output.push_str(&format!("target = \"{}\"\n", detect_target(&result.mapper)));
        output.push_str(&format!("mapper = \"{}\"\n", result.mapper));

        if let Some(desc) = func.comments.first() {
            output.push_str(&format!("description = \"{}\"\n", desc));
        }

        output.push_str("\n[functions.input]\n");
        for (name, briev_type) in &func.parameters {
            output.push_str(&format!("{} = \"{}\"\n", name, briev_type));
        }

        output.push_str("\n[functions.output.success]\n");
        if func.return_type != "Void" {
            output.push_str(&format!("result = \"{}\"\n", func.return_type));
        }

        output.push_str("\n[functions.output.error]\n");
        output.push_str(&format!("type = \"{}Error\"\n", func.name));
        output.push_str("code = \"Int\"\n");
        output.push_str("message = \"String\"\n");

        output.push('\n');
    }

    output
}

/// Detect target from mapper
fn detect_target(mapper: &str) -> &str {
    match mapper {
        "wasm" => "wasm",
        "c" => "native",
        "rust" => "native",
        "js" => "native",     // JavaScript uses native FFI via Node.js or WASM
        "python" => "native", // Python uses native FFI via CPython
        _ => "native",
    }
}

/// Write generated files to directory
pub fn write_generated_files(
    result: &AnalysisResult,
    output_dir: &Path,
    force: bool,
) -> Result<(), String> {
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;
    }

    let lib_bv_path = output_dir.join("lib.bv");
    let toml_path = output_dir.join("bindings.toml");

    if lib_bv_path.exists() && !force {
        return Err(format!("lib.bv already exists (use --force to overwrite)"));
    }

    if toml_path.exists() && !force {
        return Err(format!(
            "bindings.toml already exists (use --force to overwrite)"
        ));
    }

    let lib_bv_content = generate_lib_bv(result);
    let toml_content = generate_bindings_toml(result);

    fs::write(&lib_bv_path, lib_bv_content)
        .map_err(|e| format!("Failed to write lib.bv: {}", e))?;

    fs::write(&toml_path, toml_content)
        .map_err(|e| format!("Failed to write bindings.toml: {}", e))?;

    Ok(())
}

/// Preview generated content without writing files
pub fn preview_generated(result: &AnalysisResult) -> String {
    let mut output = String::new();

    output.push_str("=== lib.bv (preview) ===\n\n");
    output.push_str(&generate_lib_bv(result));
    output.push_str("\n=== bindings.toml (preview) ===\n\n");
    output.push_str(&generate_bindings_toml(result));

    output
}

/// Generate .dbv bindings from analysis results (replaces TOML)
pub fn generate_bindings_dbv(result: &AnalysisResult) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "// Auto-generated .dbv bindings for {}\n// Mapper: {}\n\n",
        result.library_name, result.mapper,
    ));

    for func in &result.functions {
        let params_str: Vec<String> = func
            .parameters
            .iter()
            .map(|(_, t)| c_type_to_briev(t))
            .collect();
        let return_briev = c_type_to_briev(&func.return_type);

        output.push_str(&format!(
            "register 0x{:02X} as \"{}\" {{\n",
            result
                .functions
                .iter()
                .position(|f| f.name == func.name)
                .unwrap_or(0),
            func.name
        ));
        output.push_str(&format!(
            "    type: Fn({}) -> {};\n",
            params_str.join(", "),
            return_briev
        ));
        output.push_str(&format!(
            "    location: \"{}::{}\";\n",
            result.library_name, func.name
        ));
        output.push_str(&format!("    target: {};\n", detect_target(&result.mapper)));
        if let Some(desc) = func.comments.first() {
            output.push_str(&format!("    description: \"{}\";\n", desc));
        }
        output.push_str("    check: [true];\n");
        output.push_str("}\n\n");
    }

    output
}

/// Generate a Metropolitan IDL .dbv service definition
pub fn generate_service_dbv(result: &AnalysisResult) -> String {
    let mut output = String::new();
    let lib = &result.library_name;

    output.push_str(&format!(
        "// Auto-generated metropipe service definition for {}\n// Mapper: {}\n\n",
        lib, result.mapper,
    ));
    output.push_str(&format!("SERVICE {} {{\n", lib));

    for func in &result.functions {
        for (pname, ptype) in &func.parameters {
            let t = c_type_to_briev(ptype);
            output.push_str(&format!("    INPUT {}: {};\n", pname, t));
        }
        let rt = c_type_to_briev(&func.return_type);
        output.push_str(&format!("    OUTPUT {}_result: {};\n", func.name, rt));
    }

    output.push_str("}\n");
    output
}

/// Generate a memory-spec.json for schema-aware clients
pub fn generate_memory_spec(result: &AnalysisResult) -> String {
    use serde_json::map::Map;

    let lib = &result.library_name;
    let mut spec = Map::new();
    let mut channel = Map::new();
    channel.insert(
        "address".to_string(),
        serde_json::Value::String(format!("/dev/shm/metro_{}", lib)),
    );
    channel.insert(
        "payload_offset".to_string(),
        serde_json::Value::Number(serde_json::Number::from(32)),
    );
    channel.insert(
        "capacity".to_string(),
        serde_json::Value::Number(serde_json::Number::from(4096)),
    );

    let mut input_fields: Vec<Map<String, serde_json::Value>> = Vec::new();
    let mut output_fields: Vec<Map<String, serde_json::Value>> = Vec::new();
    let mut input_offset = 0u64;

    for func in &result.functions {
        for (pname, ptype) in &func.parameters {
            let mut field = Map::new();
            field.insert("name".to_string(), serde_json::Value::String(pname.clone()));
            field.insert("type".to_string(), serde_json::Value::String(ptype.clone()));
            field.insert(
                "offset".to_string(),
                serde_json::Value::Number(serde_json::Number::from(input_offset)),
            );
            let size = type_size(ptype);
            field.insert(
                "size".to_string(),
                serde_json::Value::Number(serde_json::Number::from(size)),
            );
            input_fields.push(field);
            input_offset += size;
        }
        let mut field = Map::new();
        field.insert(
            "name".to_string(),
            serde_json::Value::String(format!("{}_result", func.name)),
        );
        field.insert(
            "type".to_string(),
            serde_json::Value::String(func.return_type.clone()),
        );
        field.insert(
            "offset".to_string(),
            serde_json::Value::Number(serde_json::Number::from(input_offset)),
        );
        let size = type_size(&func.return_type);
        field.insert(
            "size".to_string(),
            serde_json::Value::Number(serde_json::Number::from(size)),
        );
        output_fields.push(field);
        input_offset += size;
    }

    channel.insert(
        "input_fields".to_string(),
        serde_json::Value::Array(
            input_fields
                .into_iter()
                .map(|m| serde_json::Value::Object(m))
                .collect(),
        ),
    );
    channel.insert(
        "output_fields".to_string(),
        serde_json::Value::Array(
            output_fields
                .into_iter()
                .map(|m| serde_json::Value::Object(m))
                .collect(),
        ),
    );

    spec.insert("channel".to_string(), serde_json::Value::Object(channel));
    serde_json::to_string_pretty(&spec).unwrap_or_default()
}

fn type_size(ty: &str) -> u64 {
    match ty {
        "Bool" | "bool" | "uint8_t" => 1,
        "Int16" | "int16_t" | "UInt16" | "uint16_t" => 2,
        "Int" | "int" | "int32_t" | "UInt" | "uint32_t" | "Float" | "float" => 4,
        "Int64" | "int64_t" | "UInt64" | "uint64_t" | "Double" | "double" | "Float64" => 8,
        "String" | "string" | "Blob" => 256,
        "Void" | "void" => 0,
        _ => 8,
    }
}

/// Generate bridge.bv with a pre-initialized wrapper function using alka! polling
pub fn generate_bridge_bv(result: &AnalysisResult) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "// Auto-generated bridge for {}\n// Mapper: {}\n// Import this file to call {} functions from Briev\n\n",
        result.library_name, result.mapper, result.library_name
    ));
    output.push_str("import \"std/metro_bridge\";\n\n");

    // Generate frgn declarations
    for func in &result.functions {
        let params: Vec<String> = func
            .parameters
            .iter()
            .map(|(n, t)| format!("{}: {}", n, c_type_to_briev(t)))
            .collect();
        let return_briev = if func.return_type == "Void" || func.return_type == "void" {
            "Void".to_string()
        } else {
            c_type_to_briev(&func.return_type)
        };
        output.push_str(&format!(
            "frgn __raw_{}({}) -> Result<{}, String>;\n\n",
            func.name,
            params.join(", "),
            return_briev
        ));
    }

    // Generate pre-initialized wrapper defn
    for func in &result.functions {
        let params: Vec<String> = func
            .parameters
            .iter()
            .map(|(n, t)| format!("{}: {}", n, c_type_to_briev(t)))
            .collect();
        let return_briev = if func.return_type == "Void" || func.return_type == "void" {
            "Void".to_string()
        } else {
            c_type_to_briev(&func.return_type)
        };
        let params_names: Vec<String> = func.parameters.iter().map(|(n, _)| n.clone()).collect();
        let args_list = if params_names.is_empty() {
            "[]".to_string()
        } else {
            format!("List({})", params_names.join(", "))
        };

        output.push_str(&format!(
            "// Pre-initialized wrapper for {}\n\
             // Usage: let result = call_{}({});\n\
             defn call_{}(\n\
             \tchannel_id: String,\n\
             \t{}\n\
             ) -> Result<{}, String> [\n\
             \ttrue\n\
             ][\n\
             \tresult.is_ok() || result.is_err()\n\
             ] {{\n\
             \tlet request: List<Int> = {};\n\
             \tterm metropolitan_rpc(channel_id, request, 5000);\n\
             }};\n\n",
            func.name,
            func.name,
            params_names.join(", "),
            func.name,
            params.join(", "),
            return_briev,
            args_list
        ));
    }

    output
}

/// Generate foreign stub for C/Python/JS using MetropolitanHub
pub fn generate_foreign_stub(
    hub: &crate::ffi::metropipe::MetropolitanHub,
    channel_id: &str,
    lang: &str,
) -> Result<String, String> {
    match lang {
        // metropipe 32-byte protocol is the default
        "c" => hub.generate_metropipe_c_header(channel_id),
        "python" => hub.generate_metropipe_python_module(channel_id),
        "js" | "javascript" => hub.generate_metropipe_js_module(channel_id),
        "rust" => hub.generate_rust_module(channel_id),
        // Legacy 3-region protocol (fallback)
        "legacy-c" => hub.generate_c_header(channel_id),
        "legacy-python" => hub.generate_python_module(channel_id),
        _ => Err(format!("Unsupported stub language: {}", lang)),
    }
}

/// Write bind files to output directory
pub fn write_bind_files(
    result: &AnalysisResult,
    output_dir: &Path,
    force: bool,
) -> Result<(), String> {
    if !output_dir.exists() {
        fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;
    }

    let dbv_path = output_dir.join("bindings.dbv");
    let bridge_path = output_dir.join("bridge.bv");

    if dbv_path.exists() && !force {
        return Err(format!(
            "bindings.dbv already exists (use --force to overwrite)"
        ));
    }
    if bridge_path.exists() && !force {
        return Err(format!(
            "bridge.bv already exists (use --force to overwrite)"
        ));
    }

    let dbv_content = generate_bindings_dbv(result);
    let bridge_content = generate_bridge_bv(result);

    fs::write(&dbv_path, dbv_content)
        .map_err(|e| format!("Failed to write bindings.dbv: {}", e))?;

    fs::write(&bridge_path, bridge_content)
        .map_err(|e| format!("Failed to write bridge.bv: {}", e))?;

    Ok(())
}

/// Preview bind files without writing
pub fn preview_bind(result: &AnalysisResult) -> String {
    let mut output = String::new();
    output.push_str("=== bindings.dbv (preview) ===\n\n");
    output.push_str(&generate_bindings_dbv(result));
    output.push_str("\n=== bridge.bv (preview) ===\n\n");
    output.push_str(&generate_bridge_bv(result));
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_bindings_toml() {
        let result = AnalysisResult {
            library_name: "test".to_string(),
            mapper: "c".to_string(),
            functions: vec![AnalyzedFunction {
                name: "add".to_string(),
                return_type: "Int".to_string(),
                parameters: vec![
                    ("a".to_string(), "Int".to_string()),
                    ("b".to_string(), "Int".to_string()),
                ],
                is_variadic: false,
                comments: vec![],
            }],
        };

        let toml = generate_bindings_toml(&result);
        assert!(toml.contains("[[functions]]"));
        assert!(toml.contains("name = \"add\""));
    }
}
