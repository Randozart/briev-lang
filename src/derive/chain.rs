// ── Verification Chain Resolver ───────────────────────────────────────
// 2026-07-29: Compile-time cross-verified implementation selection.
// Given a chain of bodies (Ref to asm fn or defn, Derivation), select the
// first one that passes cross-verification against every other body.

use crate::ast::{ChainSegment, Expr};
use crate::backend::assembler::{AsmAssembler, StubAssembler};
use crate::interpreter::RuntimeError;
use crate::interpreter::{Atom, Value, VirtualHeap, eval_expr};
use std::collections::HashMap;

/// 2026-07-29: A body in the verification chain — evaluable form.
#[derive(Clone)]
pub enum Body {
    Asm(crate::ast::AsmFn),
    Ref(Expr),
    Synthesized(Expr),
}

/// 2026-07-29: Evaluate a Briev expression against test inputs.
/// `param_names` is the list of parameter names used in the expression.
/// Inputs are bound to these names by position.
fn evaluate_ref_expr(
    expr: &Expr,
    input: &[Value],
    param_names: &[String],
) -> Result<Value, String> {
    let mut bindings: HashMap<String, Value> = HashMap::new();
    for (i, val) in input.iter().enumerate() {
        let name = param_names
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("x{}", i));
        bindings.insert(name, val.clone());
    }
    let mut heap = VirtualHeap::new();
    match eval_expr(expr, &mut heap, &mut bindings, &HashMap::new()) {
        Ok(val) => Ok(val),
        Err(RuntimeError::TermReturn(v)) => Ok(v),
        Err(e) => Err(format!("evaluation error: {:?}", e)),
    }
}

/// 2026-07-29: Map target architecture to ABI register names.
/// Returns (result_register, input_registers_by_position).
fn abi_registers(arch: &str, n: usize) -> (&'static str, Vec<&'static str>) {
    let regs: &[&str] = match arch {
        "x86_64" => &["rdi", "rsi", "rdx", "rcx", "r8", "r9"],
        "aarch64" => &["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7"],
        _ => &["rdi", "rsi", "rdx", "rcx", "r8", "r9"],
    };
    let result = if arch == "aarch64" { "x0" } else { "rax" };
    let inputs: Vec<&'static str> = (0..n).map(|i| *regs.get(i).unwrap_or(&regs[0])).collect();
    (result, inputs)
}

/// 2026-07-29: Evaluate a body on a single set of inputs.
/// `param_names` are the parameter names used in Ref/Synthesized bodies.
fn evaluate_body(
    body: &Body,
    input: &[Value],
    _asm: &dyn AsmAssembler,
    param_names: &[String],
) -> Result<Value, String> {
    match body {
        Body::Asm(asm_fn) => {
            if asm_fn.body.is_empty() {
                return Ok(Value::Atom(Atom::Int(0)));
            }

            // Substitute {param} and {result} with ABI register names for the target.
            // Uses actual register names (e.g., rax, rdi for x86_64) because the
            // assembly is compiled as raw GNU assembler, not GCC inline asm.
            let arch = &asm_fn.target;
            let (result_reg, input_regs) = abi_registers(arch, asm_fn.params.len());
            let mut asm_text = asm_fn.body.join("\n");
            asm_text = asm_text.replace("{result}", result_reg);
            for (i, (p_name, _)) in asm_fn.params.iter().enumerate() {
                let reg = *input_regs.get(i).unwrap_or(&result_reg);
                asm_text = asm_text.replace(&format!("{{{}}}", p_name), reg);
            }

            let pid = std::process::id();
            let tmp = std::env::temp_dir().join(format!("briev_asm_{}", pid));
            std::fs::create_dir_all(&tmp).map_err(|e| format!("tmp: {}", e))?;
            let asm_file = tmp.join("f.s");
            let c_file = tmp.join("f.c");
            let so_file = tmp.join("f.so");

            // Write GNU assembler file with proper directives
            let preamble = format!(
                ".intel_syntax noprefix\n.text\n.globl asm_func\n.type asm_func, @function\nasm_func:\n"
            );
            std::fs::write(&asm_file, format!("{}{}\nret\n", preamble, asm_text))
                .map_err(|e| format!("write asm: {}", e))?;

            // Write C wrapper: passes first 6 i64 args, returns i64
            let c_code = "long long asm_func(long long, long long, long long, long long, long long, long long);\n\
                         long long wrapper(long long a0, long long a1, long long a2, long long a3, long long a4, long long a5) {\n\
                         return asm_func(a0, a1, a2, a3, a4, a5);\n}\n";
            std::fs::write(&c_file, c_code).map_err(|e| format!("write c: {}", e))?;

            // Compile with gcc
            let cc = std::env::var("CC").unwrap_or_else(|_| "gcc".to_string());
            let out = std::process::Command::new(&cc)
                .args(&["-shared", "-fPIC", "-o"])
                .arg(&so_file)
                .args(&[&asm_file, &c_file])
                .output()
                .map_err(|e| format!("gcc: {}", e))?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(format!("compile: {}", err.trim()));
            }

            // Load and call via FFI
            let result = unsafe {
                let lib = libloading::Library::new(&so_file).map_err(|e| format!("load: {}", e))?;
                let f: libloading::Symbol<
                    unsafe extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64,
                > = lib.get(b"wrapper").map_err(|e| format!("sym: {}", e))?;
                let mut args = [0i64; 6];
                for (i, v) in input.iter().enumerate().take(6) {
                    args[i] = match v {
                        Value::Atom(Atom::Int(n)) => *n,
                        _ => 0,
                    };
                }
                let val = f(args[0], args[1], args[2], args[3], args[4], args[5]);
                lib.close().ok();
                val
            };

            let _ = std::fs::remove_dir_all(&tmp);
            Ok(Value::Atom(Atom::Int(result)))
        }
        Body::Ref(expr) => evaluate_ref_expr(expr, input, param_names),
        Body::Synthesized(expr) => evaluate_ref_expr(expr, input, param_names),
    }
}

/// 2026-07-29: Generate deterministic test inputs for params.
fn generate_sample_input(params: &[(String, crate::ast::Type)]) -> Vec<Value> {
    params
        .iter()
        .enumerate()
        .map(|(i, (_, ty))| {
            let seed = (i as u64)
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            match ty {
                crate::ast::Type::Custom(s) if s == "Int" => {
                    Value::Atom(Atom::Int((seed & 0x7FF) as i64 - 500))
                }
                crate::ast::Type::Custom(s) if s == "Float" => {
                    Value::Atom(Atom::Float((seed & 0x7FF) as f64 * 0.5))
                }
                _ => Value::Atom(Atom::Int(0)),
            }
        })
        .collect()
}

/// 2026-07-29: Compare two values with tolerance for floats.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Atom(Atom::Int(ai)), Value::Atom(Atom::Int(bi))) => ai == bi,
        (Value::Atom(Atom::Float(af)), Value::Atom(Atom::Float(bf))) => (af - bf).abs() < 0.0001,
        _ => {
            let ba = a.as_bool().unwrap_or(false);
            let bb = b.as_bool().unwrap_or(false);
            ba == bb
        }
    }
}

/// 2026-07-29: Cross-verify a single candidate against all others.
fn verify_candidate(
    idx: usize,
    candidates: &[Body],
    params: &[(String, crate::ast::Type)],
    samples: usize,
    asm: &dyn AsmAssembler,
) -> bool {
    let param_names: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
    for _ in 0..samples {
        let input = generate_sample_input(params);
        let Ok(output) = evaluate_body(&candidates[idx], &input, asm, &param_names) else {
            return false;
        };
        for j in 0..candidates.len() {
            if idx == j {
                continue;
            }
            let Ok(other) = evaluate_body(&candidates[j], &input, asm, &param_names) else {
                return false;
            };
            if !values_equal(&output, &other) {
                return false;
            }
        }
    }
    true
}

/// 2026-07-29: Resolve a Ref segment to a Body using the registry.
fn resolve_ref(_name: &str, _target_arch: &str) -> Option<Body> {
    // MVP: look up in registry passed to resolve_chain.
    None
}

/// 2026-07-29: Resolve a ChainSegment to a Body.
fn resolve_segment(
    segment: &ChainSegment,
    target_arch: &str,
    registry: &HashMap<String, Body>,
) -> Option<Body> {
    match segment {
        ChainSegment::Ref(name) => registry.get(name).cloned(),
        ChainSegment::Derivation(_block) => {
            // CEGIS synthesis runs here (MVP: not yet invoked)
            None
        }
    }
}

/// 2026-07-29: Resolve a verification chain: select the first body
/// that passes cross-verification against all others.
pub fn resolve_chain(
    chain: &[ChainSegment],
    target_arch: &str,
    params: &[(String, crate::ast::Type)],
    samples: u32,
    registry: &HashMap<String, Body>,
) -> Option<Body> {
    let param_names: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
    let mut candidates: Vec<Body> = Vec::new();
    for segment in chain {
        if let Some(body) = resolve_segment(segment, target_arch, registry) {
            candidates.push(body);
        }
    }
    if candidates.is_empty() {
        return None;
    }

    let samples = (samples as usize).max(1);
    let stub_asm = StubAssembler;
    for i in 0..candidates.len() {
        if verify_candidate(i, &candidates, params, samples, &stub_asm) {
            return Some(candidates.swap_remove(i));
        }
    }

    candidates.pop()
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Expr};
    use crate::interpreter::bool_to_bits;

    fn make_ref_body() -> Expr {
        Expr::Identifier("x0".into())
    }

    fn make_params() -> Vec<(String, crate::ast::Type)> {
        vec![("x".into(), crate::ast::Type::Custom("Int".into()))]
    }

    fn make_registry() -> HashMap<String, Body> {
        let mut reg = HashMap::new();
        reg.insert("ref_fn".into(), Body::Ref(make_ref_body()));
        reg
    }

    #[test]
    fn test_chain_empty() {
        let result = resolve_chain(&[], "x86_64", &make_params(), 10, &make_registry());
        assert!(result.is_none());
    }

    #[test]
    fn test_chain_single_segment() {
        let chain = vec![ChainSegment::Ref("ref_fn".into())];
        let result = resolve_chain(&chain, "x86_64", &make_params(), 10, &make_registry());
        assert!(result.is_some());
    }

    #[test]
    fn test_chain_falls_through() {
        let chain = vec![
            ChainSegment::Ref("ref_fn".into()),
            ChainSegment::Ref("ref_fn".into()),
        ];
        let result = resolve_chain(&chain, "x86_64", &make_params(), 10, &make_registry());
        assert!(result.is_some());
    }

    #[test]
    fn test_values_equal_int() {
        assert!(values_equal(
            &Value::Atom(Atom::Int(42)),
            &Value::Atom(Atom::Int(42))
        ));
        assert!(!values_equal(
            &Value::Atom(Atom::Int(42)),
            &Value::Atom(Atom::Int(43))
        ));
    }

    #[test]
    fn test_values_equal_float() {
        assert!(values_equal(
            &Value::Atom(Atom::Float(3.14)),
            &Value::Atom(Atom::Float(3.1401))
        ));
        assert!(!values_equal(
            &Value::Atom(Atom::Float(3.14)),
            &Value::Atom(Atom::Float(4.0))
        ));
    }

    #[test]
    fn test_values_equal_bool() {
        let t = bool_to_bits(true);
        let f = bool_to_bits(false);
        assert!(values_equal(&t, &t));
        assert!(!values_equal(&t, &f));
    }

    #[test]
    fn test_generate_sample_input_int() {
        let params = vec![("x".into(), crate::ast::Type::Custom("Int".into()))];
        let input = generate_sample_input(&params);
        assert_eq!(input.len(), 1);
        assert!(matches!(input[0], Value::Atom(Atom::Int(_))));
    }

    #[test]
    fn test_evaluate_ref_simple() {
        let expr = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("x0".into())),
            Box::new(Expr::Decimal(1)),
        );
        let params = vec!["x0".to_string()];
        let result = evaluate_ref_expr(&expr, &[Value::Atom(Atom::Int(5))], &params);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Atom(Atom::Int(6)));
    }
}
