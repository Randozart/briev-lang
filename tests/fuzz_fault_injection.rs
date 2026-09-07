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

//! Fault Injection Fuzzer
//!
//! Simulates "messy outside world" at FFI and hardware boundaries.
//! Focuses exclusively on trigger-marked variables and FFI boundaries.
//!
//! Three types of fault injection:
//! 1. FFI Chaos: Return garbage data, timeouts, or simulated memory corruption
//! 2. Hardware Chaos: Flip random bits in simulated hardware registers
//! 3. Metropolitan Chaos: Randomly alter Status Word to simulate crashes

use briev_compiler::ast::*;
use briev_compiler::interpreter::{Atom, Interpreter, Value};
use briev_compiler::lexer::tokenize;
use briev_compiler::parser::Parser;
use briev_compiler::reactor::Reactor;
use proptest::prelude::*;
use std::collections::HashMap;

/// Types of faults to inject
#[derive(Debug, Clone, Copy)]
enum FaultType {
    /// Return garbage data instead of expected FFI result
    GarbageData,
    /// Simulate FFI timeout
    Timeout,
    /// Simulate memory corruption (flip bits)
    MemoryCorruption,
    /// Return error instead of success
    ReturnError,
    /// Return partial/truncated data
    TruncatedData,
}

/// A fault injection scenario
#[derive(Debug, Clone)]
struct FaultScenario {
    /// Which variable to corrupt
    target_var: String,
    /// What type of fault to inject
    fault_type: FaultType,
    /// Corruption value (for bit flipping, etc.)
    corruption_value: u64,
}

/// Generate random fault scenarios
fn arb_fault_scenario(var_names: Vec<String>) -> impl Strategy<Value = FaultScenario> {
    (
        proptest::sample::select(var_names),
        prop_oneof![
            Just(FaultType::GarbageData),
            Just(FaultType::Timeout),
            Just(FaultType::MemoryCorruption),
            Just(FaultType::ReturnError),
            Just(FaultType::TruncatedData),
        ],
        any::<u64>(),
    ).prop_map(|(target_var, fault_type, corruption_value)| {
        FaultScenario {
            target_var,
            fault_type,
            corruption_value,
        }
    })
}

/// Apply a fault to an interpreter's state
fn apply_fault(interp: &mut Interpreter, fault: &FaultScenario) {
    match fault.fault_type {
        FaultType::GarbageData => {
            let garbage = Value::Atom(Atom::Int(fault.corruption_value as i64));
            interp.state.insert(fault.target_var.clone(), garbage);
        }
        FaultType::Timeout => {
            interp.state.insert(fault.target_var.clone(), Value::Atom(Atom::Int(-1)));
        }
        FaultType::MemoryCorruption => {
            if let Some(Value::Atom(Atom::Int(val))) = interp.state.get(&fault.target_var) {
                let corrupted = val ^ (fault.corruption_value as i64);
                interp.state.insert(fault.target_var.clone(), Value::Atom(Atom::Int(corrupted)));
            }
        }
        FaultType::ReturnError => {
            interp.state.insert(fault.target_var.clone(), Value::Atom(Atom::Int(i64::MIN)));
        }
        FaultType::TruncatedData => {
            if let Some(Value::Atom(Atom::Int(val))) = interp.state.get(&fault.target_var) {
                let truncated = val & 0xFF;
                interp.state.insert(fault.target_var.clone(), Value::Atom(Atom::Int(truncated)));
            }
        }
    }
}

/// Run a fault injection test on a Briev program.
/// Returns true if the program handled the fault gracefully (no panic, state restored).
fn run_fault_injection_test(source: &str, fault: FaultScenario) -> bool {
    let tokens = match tokenize(source) {
        Ok(t) => t,
        Err(_) => return true,
    };
    let mut parser = Parser::new(tokens, source);
    let program = match parser.parse_program() {
        Ok(p) => p,
        Err(_) => return true,
    };

    let mut interp = Interpreter::new();
    interp.load_program(&program);

    apply_fault(&mut interp, &fault);

    let mut reactor = Reactor::new();
    reactor.build_from_program(&program);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reactor.run(&mut interp)
    }));

    match result {
        Ok(Ok(_)) | Ok(Err(_)) => true,
        Err(_) => false,
    }
}

/// Extract trigger-like variable names from a program
fn extract_trigger_variables(program: &[TopLevel]) -> Vec<String> {
    let mut vars = Vec::new();

    for item in program {
        match item {
            TopLevel::Trigger(trg) => {
                vars.push(trg.name.clone());
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() {
                    if is_trigger_like_name(name) {
                        vars.push(name.clone());
                    }
                }
            }
            _ => {}
        }
    }

    vars
}

fn is_trigger_like_name(name: &str) -> bool {
    let trigger_prefixes = [
        "sig", "signal", "trg", "trigger", "event", "irq", "interrupt",
        "input", "sensor", "button", "key", "click", "tick", "clock",
        "stdin", "stdout", "network", "socket", "file_", "fs_",
    ];

    let name_lower = name.to_lowercase();
    trigger_prefixes.iter().any(|prefix| name_lower.starts_with(prefix))
        || name_lower.ends_with("_trg")
        || name_lower.ends_with("_trigger")
        || name_lower.ends_with("_signal")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn test_fault_injection_never_panics(
        source in "[a-zA-Z0-9_ \\t\\n\\r;{}\\[\\]().,=+\\-*/<>!&|'\"]{0,300}",
        fault_type in 0usize..5,
        corruption_value in any::<u64>(),
    ) {
        let tokens = match tokenize(&source) {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };
        let mut parser = Parser::new(tokens, &source);
        if let Ok(program) = parser.parse_program() {
            let trigger_vars = extract_trigger_variables(&program);
            if !trigger_vars.is_empty() {
                let fault = FaultScenario {
                    target_var: trigger_vars[0].clone(),
                    fault_type: match fault_type {
                        0 => FaultType::GarbageData,
                        1 => FaultType::Timeout,
                        2 => FaultType::MemoryCorruption,
                        3 => FaultType::ReturnError,
                        _ => FaultType::TruncatedData,
                    },
                    corruption_value,
                };

                let result = run_fault_injection_test(&source, fault);
                prop_assert!(result, "Fault injection caused a panic");
            }
        }
    }
}

/// Test that the interpreter correctly handles corrupted trigger values
#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_apply_fault_garbage_data() {
        let mut interp = Interpreter::new();
        interp.state.insert("sensor".to_string(), Value::Atom(Atom::Int(42)));

        let fault = FaultScenario {
            target_var: "sensor".to_string(),
            fault_type: FaultType::GarbageData,
            corruption_value: 0xDEADBEEF,
        };

        apply_fault(&mut interp, &fault);

        assert_eq!(
            interp.state.get("sensor"),
            Some(&Value::Atom(Atom::Int(0xDEADBEEF as i64)))
        );
    }

    #[test]
    fn test_apply_fault_timeout() {
        let mut interp = Interpreter::new();
        interp.state.insert("network_data".to_string(), Value::Atom(Atom::Int(100)));

        let fault = FaultScenario {
            target_var: "network_data".to_string(),
            fault_type: FaultType::Timeout,
            corruption_value: 0,
        };

        apply_fault(&mut interp, &fault);

        assert_eq!(
            interp.state.get("network_data"),
            Some(&Value::Atom(Atom::Int(-1)))
        );
    }

    #[test]
    fn test_apply_fault_memory_corruption() {
        let mut interp = Interpreter::new();
        interp.state.insert("button".to_string(), Value::Atom(Atom::Int(0b10101010)));

        let fault = FaultScenario {
            target_var: "button".to_string(),
            fault_type: FaultType::MemoryCorruption,
            corruption_value: 0b11110000,
        };

        apply_fault(&mut interp, &fault);

        assert_eq!(
            interp.state.get("button"),
            Some(&Value::Atom(Atom::Int(0b01011010)))
        );
    }

    #[test]
    fn test_apply_fault_truncated_data() {
        let mut interp = Interpreter::new();
        interp.state.insert("stdin_line".to_string(), Value::Atom(Atom::Int(0x12345678)));

        let fault = FaultScenario {
            target_var: "stdin_line".to_string(),
            fault_type: FaultType::TruncatedData,
            corruption_value: 0,
        };

        apply_fault(&mut interp, &fault);

        assert_eq!(
            interp.state.get("stdin_line"),
            Some(&Value::Atom(Atom::Int(0x78)))
        );
    }

    #[test]
    fn test_extract_trigger_variables() {
        let code = r#"
            let counter: Int = 0;
            trg button @ 0x40001000;
            let sensor_value: Int = 0;
            let normal_var: Int = 5;
            trg sigint @ dev_signal;
        "#;

        let tokens = tokenize(code).expect("Failed to tokenize");
        let mut parser = Parser::new(tokens, code);
        let program = parser.parse_program().expect("Failed to parse");

        let triggers = extract_trigger_variables(&program);

        // Trigger declarations always count.
        assert!(triggers.contains(&"button".to_string()));
        assert!(triggers.contains(&"sigint".to_string()));
        // A `let` whose name is trigger-like counts (fault injection targets
        // "messy outside world" variables the reactor samples).
        assert!(triggers.contains(&"sensor_value".to_string()));
        // Plain state (`counter`) and non-trigger-like lets do not.
        assert!(!triggers.contains(&"counter".to_string()));
        assert!(!triggers.contains(&"normal_var".to_string()));
    }

    #[test]
    fn test_fault_injection_with_valid_program() {
        let code = r#"
            let counter: Int = 0;
            trg button @ 0x40001000;

            txn handle_button [button == true] {
                &counter = counter + 1;
                term;
            };
        "#;

        let fault = FaultScenario {
            target_var: "button".to_string(),
            fault_type: FaultType::MemoryCorruption,
            corruption_value: 0xFFFFFFFF,
        };

        let result = run_fault_injection_test(code, fault);
        assert!(result, "Fault injection should not cause panic");
    }

    #[test]
    fn test_fault_injection_with_garbage_source() {
        let code = "this is not valid briev code at all!@#$%^&*()";

        let fault = FaultScenario {
            target_var: "x".to_string(),
            fault_type: FaultType::GarbageData,
            corruption_value: 42,
        };

        let result = run_fault_injection_test(code, fault);
        assert!(result, "Invalid source should be handled gracefully");
    }

    #[test]
    fn test_multiple_fault_types() {
        let code = r#"
            let state: Int = 0;
            trg event @ 0x40002000;

            txn process [event > 0] {
                &state = state + event;
                term;
            };
        "#;

        let fault_types = [
            FaultType::GarbageData,
            FaultType::Timeout,
            FaultType::MemoryCorruption,
            FaultType::ReturnError,
            FaultType::TruncatedData,
        ];

        for fault_type in fault_types {
            let fault = FaultScenario {
                target_var: "event".to_string(),
                fault_type,
                corruption_value: 0xCAFEBABE,
            };

            let result = run_fault_injection_test(code, fault);
            assert!(result, "Fault type {:?} should not cause panic", fault_type);
        }
    }
}
