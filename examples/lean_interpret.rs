// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Sample the reference interpreter for the separate Lean execution gate.
use briev_compiler::interpreter::{Interpreter, Value};
use std::{error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: lean_interpret source.bv function arguments.json".into());
    }
    let source = fs::read_to_string(&args[0])?;
    let (items, _) = briev_compiler::library::parse_and_check(&args[0], &source)?;
    let inputs: Vec<Vec<i64>> = serde_json::from_str(&fs::read_to_string(&args[2])?)?;
    let mut interpreter = Interpreter::new();
    interpreter.load_program(&items);
    let mut results = Vec::with_capacity(inputs.len());
    for input in inputs {
        let values: Vec<Value> = input.into_iter().map(Value::int).collect();
        let result = interpreter
            .call_function(&args[1], &values)
            .map_err(|error| format!("interpreter evaluation failed: {error:?}"))?;
        results.push(
            result
                .as_i64()
                .ok_or("interpreter returned a non-word value")?,
        );
    }
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
