//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Small executable language model: fuzz bytes select operations, not answers.
//! 2026-11-09: bounded states and nonzero divisors exclude undefined inputs so
//! disagreements are actionable. Extend the model and bounds together when
//! adding operations; replace this adapter only with an equally independent oracle.

use briev_compiler::{ast::TopLevel, interpreter::Interpreter, lexer::tokenize, parser::Parser};

/// Decode one initial byte followed by up to sixteen (operation, operand, flag)
/// triples. Incomplete trailing triples are ignored. All arithmetic fits i64.
pub fn program(data: &[u8], tuples: bool) -> Option<(String, i64, String)> {
    let (&initial, rest) = data.split_first()?;
    if rest.len() < 3 {
        return None;
    }
    let mut state = i64::from(initial as i8);
    let mut source = format!("let sample_0: Int = {state};\n");
    let mut last = String::from("sample_0");
    for (index, bytes) in rest.chunks_exact(3).take(16).enumerate() {
        let op = bytes[0] % if tuples { 8 } else { 6 };
        let operand = i64::from(bytes[1] as i8);
        let flag = bytes[2] & 1 != 0;
        let body = body(op, operand, flag);
        state = model(op, state, operand, flag) % 97;
        source.push_str(&format!("defn operation_{index}(s: Int) -> Int {{ {body} }};\n"));
        let next = format!("sample_{}", index + 1);
        source.push_str(&format!("let {next}: Int = operation_{index}({last}) % 97;\n"));
        last = next;
    }
    Some((source, state, last))
}

fn body(op: u8, operand: i64, flag: bool) -> String {
    let divisor = operand.abs() + 1;
    match op {
        0 => format!("term s + ({operand});"),
        1 => format!("term s - ({operand});"),
        2 => format!("term s * ({operand});"),
        3 => format!("term s / {divisor};"),
        4 => format!("term s % {divisor};"),
        5 => format!("when s > ({operand}) {{ term s + ({operand}); }}; term s - ({operand});"),
        6 => format!("let pair = (s + ({operand}), s - ({operand})); term pair.{};", u8::from(flag)),
        7 => format!("let pair = (s + ({operand}), s - ({operand})); let (left, right) = pair; term left - right;"),
        _ => panic!("invalid internal operation {op}"),
    }
}

fn model(op: u8, state: i64, operand: i64, flag: bool) -> i64 {
    match op {
        0 => state + operand,
        1 => state - operand,
        2 => state * operand,
        3 => state / (operand.abs() + 1),
        4 => state % (operand.abs() + 1),
        5 if state > operand => state + operand,
        5 => state - operand,
        6 if flag => state - operand,
        6 => state + operand,
        7 => 2 * operand,
        _ => panic!("invalid internal operation {op}"),
    }
}

fn interpret(source: &str, result: &str) -> Result<i64, String> {
    let tokens = tokenize(source).map_err(|e| format!("generated source lex error: {e}"))?;
    let items = Parser::new(tokens, source).parse_program()
        .map_err(|e| format!("generated source parse error: {e}"))?;
    let mut interpreter = Interpreter::new();
    interpreter.load_program(&items);
    for item in &items {
        if let TopLevel::Statement(stmt) = item {
            interpreter.exec_stmt(stmt).map_err(|e| format!("reference runtime error: {e}"))?;
        }
    }
    interpreter.state.get(result).and_then(|value| value.as_i64())
        .ok_or_else(|| format!("missing integer result {result}"))
}

/// A generated valid program must execute and agree with the independent model.
/// No catch_unwind: libFuzzer must see compiler panics and semantic failures.
pub fn check(data: &[u8], tuples: bool) {
    let Some((source, expected, result)) = program(data, tuples) else {
        return;
    };
    let actual = interpret(&source, &result);
    assert_eq!(actual, Ok(expected), "semantic disagreement; input={data:?}\n{source}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_inputs_are_not_programs() {
        for bytes in [&[][..], &[1], &[1, 2, 3]] {
            assert!(program(bytes, true).is_none());
        }
    }

    #[test]
    fn signed_boundaries_and_zero_operands_agree() {
        // 2026-11-09: flattened to stay within flat-control-flow (no nested loop).
        let operands = [0u8, 1, 127, 128, 255];
        let cases = (0u8..6).flat_map(|op| operands.map(move |operand| (op, operand)));
        for (op, operand) in cases {
            check(&[128, op, operand, 0, op, operand, 1], false);
        }
    }

    #[test]
    fn oracle_tracks_sequential_state_and_truncating_division() {
        let input = [249, 0, 2, 0, 3, 2, 0, 2, 4, 0];
        let (_, expected, _) = program(&input, false).unwrap();
        assert_eq!(expected, -4); // ((-7 + 2) / 3) * 4, truncating toward zero.
        check(&input, false);
    }

    #[test]
    fn tuple_oracle_distinguishes_projection_and_destructuring() {
        assert_eq!(program(&[7, 6, 2, 0], true).unwrap().1, 9);
        assert_eq!(program(&[7, 6, 2, 1], true).unwrap().1, 5);
        assert_eq!(program(&[7, 7, 2, 0], true).unwrap().1, 4);
    }
}
