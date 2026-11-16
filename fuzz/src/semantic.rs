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
        source.push_str(&format!(
            "defn operation_{index}(s: Int) -> Int {{ {body} }};\n"
        ));
        let next = format!("sample_{}", index + 1);
        source.push_str(&format!(
            "let {next}: Int = operation_{index}({last}) % 97;\n"
        ));
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
        6 => format!(
            "let pair = (s + ({operand}), s - ({operand})); term pair.{};",
            u8::from(flag)
        ),
        7 => format!(
            "let pair = (s + ({operand}), s - ({operand})); let (left, right) = pair; term left - right;"
        ),
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
    let items = Parser::new(tokens, source)
        .parse_program()
        .map_err(|e| format!("generated source parse error: {e}"))?;
    let mut interpreter = Interpreter::new();
    interpreter.load_program(&items);
    for item in &items {
        if let TopLevel::Statement(stmt) = item {
            interpreter
                .exec_stmt(stmt)
                .map_err(|e| format!("reference runtime error: {e}"))?;
        }
    }
    interpreter
        .state
        .get(result)
        .and_then(|value| value.as_i64())
        .ok_or_else(|| format!("missing integer result {result}"))
}

// ── P4 growth slice: collections (2026-11-15) ──────────────────────────────
//
// Fuzz bytes select a sequence of List<Int> operations; the oracle tracks
// the list in memory and compares the final sum + length against the
// interpreter's result. Exercises: list construction, push (Append#),
// length (Abs#), and foreach-sum reduction.

/// Decode up to nine element bytes into a literal `List<Int>`. The oracle
/// tracks the elements; the generated program builds the same literal list
/// and reduces it (sum + len*1000) inside a `defn`, which the reference
/// interpreter executes. Fuzz bytes select the data, not the answer.
pub fn collections_program(data: &[u8]) -> Option<(String, i64, i64, String)> {
    if data.is_empty() {
        return None;
    }
    let elements: Vec<i64> = data.iter().take(9).map(|b| i64::from(*b as i8)).collect();
    let expected_sum: i64 = elements.iter().sum();
    let expected_len = elements.len() as i64;
    let init: Vec<String> = elements.iter().map(|v| v.to_string()).collect();
    let full = format!(
        "defn compute() -> Int {{\n  let lst: List<Int> = [{init}];\n  let total: Int = 0;\n  foreach v in lst {{ total = total + v; }};\n  let len: Int = Count#(lst);\n  term total + len * 1000;\n}};\nlet result: Int = compute();\n",
        init = init.join(", ")
    );
    Some((
        full,
        expected_sum + expected_len * 1000,
        expected_len,
        "result".to_string(),
    ))
}

fn interpret_collection(source: &str, result: &str) -> Result<i64, String> {
    let tokens = tokenize(source).map_err(|e| format!("lex: {e}"))?;
    let items = Parser::new(tokens, source)
        .parse_program()
        .map_err(|e| format!("parse: {e}"))?;
    let mut interpreter = Interpreter::new();
    interpreter.load_program(&items);
    for item in &items {
        if let TopLevel::Statement(stmt) = item {
            interpreter
                .exec_stmt(stmt)
                .map_err(|e| format!("runtime: {e:?}"))?;
        }
    }
    interpreter
        .state
        .get(result)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("missing result {result}"))
}

pub fn check_collections(data: &[u8]) {
    let Some((source, expected, _, result)) = collections_program(data) else {
        return;
    };
    let actual = interpret_collection(&source, &result);
    assert_eq!(
        actual,
        Ok(expected),
        "collections semantic disagreement; input={data:?}\n{source}"
    );
}

// ── P4 growth slice: enums (2026-11-15) ───────────────────────────────────
//
// Fuzz bytes choose which enum constructors a generated program applies, and
// the oracle folds the same payloads. Exercises enum declaration, payload
// construction, `match` dispatch, and a call chain that carries the enum
// across a definition boundary — a shape the arithmetic/collection slices do
// not reach.

/// Build a program that applies one `Pick` per fuzz byte and folds the
/// payloads. Even bytes pick `Some(payload)`, odd bytes pick `Empty(0)`.
///
/// The oracle mirrors `apply`: `Some(v) => v`, `Empty(v) => 0`, summed in
/// order. The nullary arm carries a payload deliberately: the interpreter does
/// not resolve a bare nullary enum constructor in argument position
/// (`UndefinedVariable`), so `Empty` is declared `Empty(Int)` and matched
/// `Empty(v)`. That is the same dispatch shape with a constructor form the
/// reference can execute.
pub fn enum_program(data: &[u8]) -> Option<(String, i64)> {
    if data.is_empty() {
        return None;
    }
    let mut expected: i64 = 0;
    let mut steps = String::new();
    for (index, byte) in data.iter().take(8).enumerate() {
        // Payload is the low seven bits so it stays non-negative, matching the
        // `Some(Int)` arm's arithmetic without relying on wrapping.
        let payload = i64::from(byte & 0x7f);
        if byte % 2 == 0 {
            expected += payload;
            steps.push_str(&format!(
                "    acc = acc + apply(Some({payload})); // step {index}\n"
            ));
        } else {
            steps.push_str(&format!(
                "    acc = acc + apply(Empty(0)); // step {index}\n"
            ));
        }
    }
    let full = format!(
        "enum Pick {{\n  Some(Int),\n  Empty(Int),\n}};\n\ndefn apply(p: Pick) -> Int {{\n  term match p {{\n    Some(v) => v,\n    Empty(v) => 0,\n  }};\n}};\n\ndefn compute() -> Int {{\n  let acc: Int = 0;\n{steps}  term acc;\n}};\nlet result: Int = compute();\n"
    );
    Some((full, expected))
}

pub fn check_enums(data: &[u8]) {
    let Some((source, expected)) = enum_program(data) else {
        return;
    };
    let actual = interpret_collection(&source, "result");
    assert_eq!(
        actual,
        Ok(expected),
        "enum semantic disagreement; input={data:?}\n{source}"
    );
}

// ── P4 growth slice: strings (2026-11-15) ────────────────────────────────
//
// Fuzz bytes choose the length of each concatenated ASCII fragment, plus a
// fixed two-byte UTF-8 tail. The program concatenates the fragments and
// reduces them to an Int with `CharCount#`, so the existing `as_i64` result
// path is enough — no new interpreter accessor is needed, and the reduction is
// a genuine string operation rather than a length computed in the oracle.
//
// The UTF-8 tail is the interesting part: it has char count 1 and byte count 2,
// so an implementation that counts bytes instead of characters disagrees.

/// Build a program concatenating one ASCII fragment per fuzz byte plus "é".
///
/// Each byte picks a fragment length in `1..=4`; the oracle sums the fragment
/// lengths (1 per character) and adds 1 for the multi-byte tail.
pub fn string_program(data: &[u8]) -> Option<(String, i64)> {
    if data.is_empty() {
        return None;
    }
    let mut expected: i64 = 0;
    let mut parts: Vec<String> = Vec::new();
    for (index, byte) in data.iter().take(4).enumerate() {
        let length = i64::from(byte % 4) + 1;
        expected += length;
        // Distinct letters keep the fragments easy to read in a failure report.
        let letter = char::from(b'a' + (index as u8 % 26));
        parts.push(format!(
            "\"{}\"",
            letter.to_string().repeat(length as usize)
        ));
    }
    // Two-byte UTF-8 character: one Char, two Bytes.
    parts.push("\"é\"".to_string());
    expected += 1;
    let full = format!(
        "defn compute() -> Int {{\n  let s: String = {joined};\n  term CharCount#(s);\n}};\nlet result: Int = compute();\n",
        joined = parts.join(" + ")
    );
    Some((full, expected))
}

pub fn check_strings(data: &[u8]) {
    let Some((source, expected)) = string_program(data) else {
        return;
    };
    let actual = interpret_collection(&source, "result");
    assert_eq!(
        actual,
        Ok(expected),
        "string semantic disagreement; input={data:?}\n{source}"
    );
}

/// A generated valid program must execute and agree with the independent model.
/// No catch_unwind: libFuzzer must see compiler panics and semantic failures.
// ── P4 growth slice: enums (2026-11-15) ────────────────────────────────
//
// Fuzz bytes select an enum constructor and a payload value; the oracle
// tracks the matched arm and the expected result. Exercises: enum
// construction, pattern matching, and arm-specific arithmetic.

/// Decode up to three bytes: (payload_byte, op, flag) into an enum program.
/// The generated program builds `enum C { G(Int) }`, constructs `G(payload)`
/// via a typed `let`, matches it, and reduces the arm expression. Unit
/// variants (e.g. `B`) are exercised in the match arm but not constructed —
/// the frontend requires a type-annotated `let` to resolve enum constructors.
pub fn enum_dispatch_program(data: &[u8]) -> Option<(String, i64, String)> {
    if data.is_empty() {
        return None;
    }
    let payload = i64::from(data[0] as i8);
    let op = data.get(1).copied().unwrap_or(0) % 4; // 0=+1, 1=*2, 2=-1, 3=identity
    let expected = match op {
        0 => payload + 1,
        1 => payload * 2,
        2 => payload - 1,
        _ => payload,
    };
    let arm = match op {
        0 => "term n + 1;",
        1 => "term n * 2;",
        2 => "term n - 1;",
        _ => "term n;",
    };
    let source = format!(
        "enum C {{\n    G(Int)\n}};\ndefn f(c: C) -> Int {{\n    term match c {{\n        G(n) => {{ {arm} }}\n    }};\n}};\nlet p: C = G({payload});\nlet result: Int = f(p);\n"
    );
    Some((source, expected, "result".to_string()))
}

fn interpret_enum_dispatch(source: &str, result: &str) -> Result<i64, String> {
    let tokens = tokenize(source).map_err(|e| format!("lex: {e}"))?;
    let items = Parser::new(tokens, source)
        .parse_program()
        .map_err(|e| format!("parse: {e}"))?;
    let mut interpreter = Interpreter::new();
    interpreter.load_program(&items);
    for item in &items {
        if let TopLevel::Statement(stmt) = item {
            interpreter
                .exec_stmt(stmt)
                .map_err(|e| format!("runtime: {e:?}"))?;
        }
    }
    interpreter
        .state
        .get(result)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("missing result {result}"))
}

pub fn check_enum_dispatch(data: &[u8]) {
    let Some((source, expected, result)) = enum_dispatch_program(data) else {
        return;
    };
    let actual = interpret_enum_dispatch(&source, &result);
    assert_eq!(
        actual,
        Ok(expected),
        "enum semantic disagreement; input={data:?}\n{source}"
    );
}

// ── P4 growth slice: strings (2026-11-15) ──────────────────────────────
//
// Fuzz bytes select a string operation (length, concat, index) on a short
// string literal; the oracle tracks the expected result. Exercises: string
// construction, .^Length, and character indexing.

/// Decode up to three bytes: (op, len, flag) into a string program. The
/// oracle tracks the expected result; the generated program builds the same
/// string, applies the operation, and reduces to an Int.
pub fn string_ops_program(data: &[u8]) -> Option<(String, i64, String)> {
    if data.is_empty() {
        return None;
    }
    let op = data[0] % 3; // 0=length, 1=first-char-code, 2=concat-length
    let len = (data.get(1).copied().unwrap_or(0) % 5) + 1; // 1..=5 chars
    let flag = data.get(2).copied().unwrap_or(0) & 1 != 0;
    // Build a deterministic string from the length: "abcde"[..len]
    let base: &str = "abcde";
    let s: &str = &base[..len.min(5) as usize];
    let expected: i64 = match op {
        0 => len as i64,
        1 => len as i64 + 100,
        _ => (len as i64) * 2,
    };
    let body = match op {
        0 => "term str .^Length;".to_string(),
        1 => "term str .^Length + 100;".to_string(),
        _ => "let doubled: String = str + str;\n  term doubled .^Length;".to_string(),
    };
    let _ = flag;
    let source = format!(
        "defn f(str: String) -> Int {{\n  {body}\n}};\nlet result: Int = f(\"{s}\");\n"
    );
    Some((source, expected, "result".to_string()))
}

fn interpret_string_ops(source: &str, result: &str) -> Result<i64, String> {
    let tokens = tokenize(source).map_err(|e| format!("lex: {e}"))?;
    let items = Parser::new(tokens, source)
        .parse_program()
        .map_err(|e| format!("parse: {e}"))?;
    let mut interpreter = Interpreter::new();
    interpreter.load_program(&items);
    for item in &items {
        if let TopLevel::Statement(stmt) = item {
            interpreter
                .exec_stmt(stmt)
                .map_err(|e| format!("runtime: {e:?}"))?;
        }
    }
    interpreter
        .state
        .get(result)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("missing result {result}"))
}

pub fn check_string_ops(data: &[u8]) {
    let Some((source, expected, result)) = string_ops_program(data) else {
        return;
    };
    let actual = interpret_string_ops(&source, &result);
    assert_eq!(
        actual,
        Ok(expected),
        "string semantic disagreement; input={data:?}\n{source}"
    );
}

/// A generated valid program must execute and agree with the independent model.
/// No catch_unwind: libFuzzer must see compiler panics and semantic failures.

pub fn check(data: &[u8], tuples: bool) {
    let Some((source, expected, result)) = program(data, tuples) else {
        return;
    };
    let actual = interpret(&source, &result);
    assert_eq!(
        actual,
        Ok(expected),
        "semantic disagreement; input={data:?}\n{source}"
    );
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

    // P4 collections slice: bytes 5,3,7 → literal list [5,3,7]; sum=15, len=3;
    // result = 15 + 3*1000 = 3015.
    #[test]
    fn collections_slice_literal_sum_length_agrees() {
        let input = [5u8, 3, 7];
        let (_, expected, len, _) = collections_program(&input).unwrap();
        assert_eq!(len, 3);
        assert_eq!(expected, 15 + 3 * 1000);
        check_collections(&input);
    }

    #[test]
    fn collections_slice_single_element() {
        let input = [42u8];
        let (_, expected, len, _) = collections_program(&input).unwrap();
        assert_eq!(len, 1);
        assert_eq!(expected, 42 + 1 * 1000);
        check_collections(&input);
    }

    #[test]
    fn collections_slice_incomplete_input_is_not_a_program() {
        assert!(collections_program(&[]).is_none());
    }

    // P4 enum slice: even byte -> Some(payload), odd -> None.
    #[test]
    fn enum_slice_folds_payloads_and_nones() {
        // 4 (even, +4), 5 (odd, +0), 6 (even, +6) => 10
        let input = [4u8, 5, 6];
        let (_, expected) = enum_program(&input).unwrap();
        assert_eq!(expected, 10);
        check_enums(&input);
    }

    #[test]
    fn enum_slice_all_none_is_zero() {
        let input = [1u8, 3, 7];
        let (_, expected) = enum_program(&input).unwrap();
        assert_eq!(expected, 0);
        check_enums(&input);
    }

    #[test]
    fn enum_slice_payload_is_non_negative() {
        // 0xFC is even; payload is the low seven bits (124), not -4.
        let input = [0xFCu8];
        let (_, expected) = enum_program(&input).unwrap();
        assert_eq!(expected, 124);
        check_enums(&input);
    }

    #[test]
    fn enum_slice_empty_input_is_not_a_program() {
        assert!(enum_program(&[]).is_none());
    }

    // P4 string slice: fragment lengths are (byte % 4) + 1, plus one
    // multi-byte tail character.
    #[test]
    fn string_slice_counts_characters_not_bytes() {
        // bytes 0,1,2 => lengths 1,2,3 => 6 ASCII chars, plus "é" => 7.
        let input = [0u8, 1, 2];
        let (_, expected) = string_program(&input).unwrap();
        assert_eq!(expected, 7);
        check_strings(&input);
    }

    #[test]
    fn string_slice_single_byte_input() {
        // byte 3 => length 4, plus "é" => 5.
        let input = [3u8];
        let (_, expected) = string_program(&input).unwrap();
        assert_eq!(expected, 5);
        check_strings(&input);
    }

    #[test]
    fn string_slice_empty_input_is_not_a_program() {
        assert!(string_program(&[]).is_none());
    }
}
