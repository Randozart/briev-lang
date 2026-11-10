//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! 2026-11-10: grammar-aware generation of WELL-TYPED Briev programs.
//!
//! `semantic` generates one fixed Int-only operation chain. This module builds
//! a *typed expression tree* and emits source from it, so the same tree is the
//! oracle. Three properties are checked per generated program:
//!
//! 1. it is well-typed by construction, so the typechecker MUST accept it
//!    (a rejection is a false-negative in the typechecker);
//! 2. the reference interpreter MUST execute it; and
//! 3. the interpreter's results MUST equal the model's.
//!
//! Bounds (literals in [-8,8], args in [-20,20], depth <= 3, nonzero divisor
//! literals) keep all arithmetic well inside i64, so a disagreement is
//! actionable and never an overflow or divide-by-zero artifact.

use crate::ast::TopLevel;
use crate::interpreter::Interpreter;

/// Maximum expression nesting depth. Kept small so the model cannot overflow.
const MAX_DEPTH: u32 = 3;
/// Literal magnitude bound (`-LIT_MAX..=LIT_MAX`).
const LIT_MAX: i64 = 8;
/// Argument magnitude bound (`-ARG_MAX..=ARG_MAX`).
const ARG_MAX: i64 = 20;
/// At most this many top-level calls per generated program.
const MAX_CASES: usize = 3;

/// An Int-valued expression. Emitted to Briev and evaluated by the model.
#[derive(Clone, Debug)]
enum IExpr {
    Lit(i64),
    /// 0 = `s`, 1 = `e` (the two `f` parameters).
    Param(u8),
    /// The `let x` local.
    Local,
    Add(Box<IExpr>, Box<IExpr>),
    Sub(Box<IExpr>, Box<IExpr>),
    Mul(Box<IExpr>, Box<IExpr>),
    /// Division by a nonzero *literal* — never a runtime trap.
    Div(Box<IExpr>, i64),
    /// Remainder by a nonzero *literal*.
    Rem(Box<IExpr>, i64),
    Neg(Box<IExpr>),
    /// `pick(cond, a, b)` — the helper defn (Briev has no if/else).
    If(BExpr, Box<IExpr>, Box<IExpr>),
    /// `match scrutinee { 0 => a, _ => b }`.
    MatchZero(Box<IExpr>, Box<IExpr>, Box<IExpr>),
}

/// A Bool-valued expression (guards and `pick` conditions).
#[derive(Clone, Debug)]
enum BExpr {
    Lit(bool),
    Lt(Box<IExpr>, Box<IExpr>),
    Gt(Box<IExpr>, Box<IExpr>),
    Eq(Box<IExpr>, Box<IExpr>),
    Ne(Box<IExpr>, Box<IExpr>),
    And(Box<BExpr>, Box<BExpr>),
    Or(Box<BExpr>, Box<BExpr>),
    Not(Box<BExpr>),
}

/// Byte-cursor expression builder. Consumes bytes in order; a missing byte
/// reads as 0 so generation is total.
struct Builder<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Builder<'_> {
    fn byte(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    fn lit(&mut self) -> i64 {
        (self.byte() % (2 * LIT_MAX as u8 + 1)) as i64 - LIT_MAX
    }

    /// A nonzero divisor in `-9..=9`, excluding 0.
    fn divisor(&mut self) -> i64 {
        let magnitude = (self.byte() % 9) as i64 + 1;
        if self.byte() & 1 == 0 {
            magnitude
        } else {
            -magnitude
        }
    }

    fn arg(&mut self) -> i64 {
        (self.byte() % (2 * ARG_MAX as u8 + 1)) as i64 - ARG_MAX
    }

    fn leaf(&mut self, allow_local: bool) -> IExpr {
        let choices = if allow_local { 3 } else { 2 };
        match self.byte() % choices {
            0 => IExpr::Lit(self.lit()),
            1 => IExpr::Param(self.byte() & 1),
            _ => IExpr::Local,
        }
    }

    fn iexpr(&mut self, depth: u32, allow_local: bool) -> IExpr {
        if depth == 0 || self.byte() % 4 == 0 {
            return self.leaf(allow_local);
        }
        match self.byte() % 9 {
            0 => IExpr::Add(
                Box::new(self.iexpr(depth - 1, allow_local)),
                Box::new(self.iexpr(depth - 1, allow_local)),
            ),
            1 => IExpr::Sub(
                Box::new(self.iexpr(depth - 1, allow_local)),
                Box::new(self.iexpr(depth - 1, allow_local)),
            ),
            2 => IExpr::Mul(
                Box::new(self.iexpr(depth - 1, allow_local)),
                Box::new(self.iexpr(depth - 1, allow_local)),
            ),
            3 => IExpr::Div(Box::new(self.iexpr(depth - 1, allow_local)), self.divisor()),
            4 => IExpr::Rem(Box::new(self.iexpr(depth - 1, allow_local)), self.divisor()),
            5 => IExpr::Neg(Box::new(self.iexpr(depth - 1, allow_local))),
            6 => IExpr::If(
                self.bexpr(depth - 1),
                Box::new(self.iexpr(depth - 1, allow_local)),
                Box::new(self.iexpr(depth - 1, allow_local)),
            ),
            7 => IExpr::MatchZero(
                Box::new(self.iexpr(depth - 1, allow_local)),
                Box::new(self.iexpr(depth - 1, allow_local)),
                Box::new(self.iexpr(depth - 1, allow_local)),
            ),
            _ => self.leaf(allow_local),
        }
    }

    fn bexpr(&mut self, depth: u32) -> BExpr {
        if depth == 0 || self.byte() % 3 == 0 {
            if self.byte() & 1 == 0 {
                return BExpr::Lit(self.byte() & 1 == 0);
            }
            return self.cmp();
        }
        match self.byte() % 5 {
            0 => BExpr::And(Box::new(self.bexpr(depth - 1)), Box::new(self.bexpr(depth - 1))),
            1 => BExpr::Or(Box::new(self.bexpr(depth - 1)), Box::new(self.bexpr(depth - 1))),
            2 => BExpr::Not(Box::new(self.bexpr(depth - 1))),
            _ => self.cmp(),
        }
    }

    fn cmp(&mut self) -> BExpr {
        let l = self.iexpr(0, false);
        let r = self.iexpr(0, false);
        match self.byte() % 4 {
            0 => BExpr::Lt(Box::new(l), Box::new(r)),
            1 => BExpr::Gt(Box::new(l), Box::new(r)),
            2 => BExpr::Eq(Box::new(l), Box::new(r)),
            _ => BExpr::Ne(Box::new(l), Box::new(r)),
        }
    }
}

fn emit_i(e: &IExpr) -> String {
    match e {
        IExpr::Lit(n) => format!("({n})"),
        IExpr::Param(0) => "s".to_string(),
        IExpr::Param(_) => "e".to_string(),
        IExpr::Local => "x".to_string(),
        IExpr::Add(a, b) => format!("({} + {})", emit_i(a), emit_i(b)),
        IExpr::Sub(a, b) => format!("({} - {})", emit_i(a), emit_i(b)),
        IExpr::Mul(a, b) => format!("({} * {})", emit_i(a), emit_i(b)),
        IExpr::Div(a, d) => format!("({} / {d})", emit_i(a)),
        IExpr::Rem(a, d) => format!("({} % {d})", emit_i(a)),
        IExpr::Neg(a) => format!("(-{})", emit_i(a)),
        IExpr::If(c, a, b) => format!("pick({}, {}, {})", emit_b(c), emit_i(a), emit_i(b)),
        IExpr::MatchZero(v, a, b) => {
            format!("match {} {{ 0 => {}, _ => {} }}", emit_i(v), emit_i(a), emit_i(b))
        }
    }
}

fn emit_b(e: &BExpr) -> String {
    match e {
        BExpr::Lit(b) => if *b { "true".to_string() } else { "false".to_string() },
        BExpr::Lt(a, b) => format!("({} < {})", emit_i(a), emit_i(b)),
        BExpr::Gt(a, b) => format!("({} > {})", emit_i(a), emit_i(b)),
        BExpr::Eq(a, b) => format!("({} == {})", emit_i(a), emit_i(b)),
        BExpr::Ne(a, b) => format!("({} != {})", emit_i(a), emit_i(b)),
        BExpr::And(a, b) => format!("({} && {})", emit_b(a), emit_b(b)),
        BExpr::Or(a, b) => format!("({} || {})", emit_b(a), emit_b(b)),
        BExpr::Not(a) => format!("(!{})", emit_b(a)),
    }
}

fn eval_i(e: &IExpr, s: i64, arg_e: i64, x: i64) -> i64 {
    match e {
        IExpr::Lit(n) => *n,
        IExpr::Param(0) => s,
        IExpr::Param(_) => arg_e,
        IExpr::Local => x,
        IExpr::Add(a, b) => eval_i(a, s, arg_e, x).wrapping_add(eval_i(b, s, arg_e, x)),
        IExpr::Sub(a, b) => eval_i(a, s, arg_e, x).wrapping_sub(eval_i(b, s, arg_e, x)),
        IExpr::Mul(a, b) => eval_i(a, s, arg_e, x).wrapping_mul(eval_i(b, s, arg_e, x)),
        IExpr::Div(a, d) => eval_i(a, s, arg_e, x).wrapping_div(*d),
        IExpr::Rem(a, d) => eval_i(a, s, arg_e, x).wrapping_rem(*d),
        IExpr::Neg(a) => eval_i(a, s, arg_e, x).wrapping_neg(),
        IExpr::If(c, a, b) => {
            if eval_b(c, s, arg_e, x) {
                eval_i(a, s, arg_e, x)
            } else {
                eval_i(b, s, arg_e, x)
            }
        }
        IExpr::MatchZero(v, a, b) => {
            if eval_i(v, s, arg_e, x) == 0 {
                eval_i(a, s, arg_e, x)
            } else {
                eval_i(b, s, arg_e, x)
            }
        }
    }
}

fn eval_b(e: &BExpr, s: i64, arg_e: i64, x: i64) -> bool {
    match e {
        BExpr::Lit(b) => *b,
        BExpr::Lt(a, b) => eval_i(a, s, arg_e, x) < eval_i(b, s, arg_e, x),
        BExpr::Gt(a, b) => eval_i(a, s, arg_e, x) > eval_i(b, s, arg_e, x),
        BExpr::Eq(a, b) => eval_i(a, s, arg_e, x) == eval_i(b, s, arg_e, x),
        BExpr::Ne(a, b) => eval_i(a, s, arg_e, x) != eval_i(b, s, arg_e, x),
        BExpr::And(a, b) => eval_b(a, s, arg_e, x) && eval_b(b, s, arg_e, x),
        BExpr::Or(a, b) => eval_b(a, s, arg_e, x) || eval_b(b, s, arg_e, x),
        BExpr::Not(a) => !eval_b(a, s, arg_e, x),
    }
}

/// Generate a well-typed program and the model's expected results, in order.
/// Returns `None` when `data` is too short to build anything.
pub fn program(data: &[u8]) -> Option<(String, Vec<(String, i64)>)> {
    if data.len() < 6 {
        return None;
    }
    let mut b = Builder { data, pos: 0 };
    let init = b.iexpr(MAX_DEPTH, false);
    let cond = b.bexpr(MAX_DEPTH);
    let then_ = b.iexpr(MAX_DEPTH, true);
    let final_ = b.iexpr(MAX_DEPTH, true);
    let cases = 1 + (b.byte() as usize % MAX_CASES);

    let mut source = String::from(
        "defn pick(cond: Bool, a: Int, b: Int) -> Int {\n    when cond { term a; };\n    term b;\n};\n",
    );
    source.push_str("defn f(s: Int, e: Int) -> Int {\n");
    source.push_str(&format!("    let x: Int = {};\n", emit_i(&init)));
    source.push_str(&format!("    when {} {{ x = {}; }};\n", emit_b(&cond), emit_i(&then_)));
    source.push_str(&format!("    term {};\n}};\n", emit_i(&final_)));

    let mut expected = Vec::with_capacity(cases);
    for i in 0..cases {
        let s = b.arg();
        let e = b.arg();
        let name = format!("r{i}");
        source.push_str(&format!("let {name}: Int = f({s}, {e});\n"));
        let x = eval_i(&init, s, e, 0);
        let x = if eval_b(&cond, s, e, x) { eval_i(&then_, s, e, x) } else { x };
        expected.push((name, eval_i(&final_, s, e, x)));
    }
    Some((source, expected))
}

/// Run the generated program through the reference interpreter and return the
/// result values in `expected` order.
fn interpret(items: &[TopLevel], expected: &[(String, i64)]) -> Result<Vec<i64>, String> {
    let mut interpreter = Interpreter::new();
    interpreter.load_program(items);
    for item in items {
        if let TopLevel::Statement(stmt) = item {
            interpreter.exec_stmt(stmt).map_err(|e| e.to_string())?;
        }
    }
    expected
        .iter()
        .map(|(name, _)| {
            interpreter
                .state
                .get(name)
                .and_then(|v| v.as_i64())
                .ok_or_else(|| format!("missing integer result {name}"))
        })
        .collect()
}

/// Generate, typecheck (must accept), interpret, and compare to the model.
/// Panics on any disagreement — the fuzzer treats a panic as a finding.
pub fn check(data: &[u8]) {
    let Some((source, expected)) = program(data) else {
        return;
    };
    let items = match crate::library::parse_and_check("typed.bv", &source) {
        Ok((items, _)) => items,
        Err(e) => panic!("well-typed program rejected by the frontend: {e}\n{source}"),
    };
    let actual = interpret(&items, &expected)
        .unwrap_or_else(|e| panic!("reference interpreter failed: {e}\n{source}"));
    let want: Vec<i64> = expected.iter().map(|(_, v)| *v).collect();
    assert_eq!(actual, want, "model disagreement\n{source}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_inputs_generate_nothing() {
        assert!(program(&[]).is_none());
        assert!(program(&[1, 2, 3]).is_none());
    }

    #[test]
    fn generated_program_is_well_typed_and_agrees() {
        // Deterministic pseudo-random bytes; every generated program must
        // typecheck, interpret, and agree with the model.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        for _ in 0..400 {
            let mut bytes = Vec::with_capacity(64);
            for _ in 0..64 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                bytes.push((state >> 33) as u8);
            }
            check(&bytes);
        }
    }

    #[test]
    fn match_and_pick_shapes_are_emitted() {
        // A hand-picked byte string that exercises `pick` and `match`.
        let bytes: Vec<u8> = (0u8..64).collect();
        let (source, expected) = program(&bytes).unwrap();
        assert!(source.contains("defn pick"), "{source}");
        assert!(!expected.is_empty());
        check(&bytes);
    }
}
