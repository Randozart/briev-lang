//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! 2026-11-10: grammar-aware generation of WELL-TYPED Briev programs.
//!
//! `semantic` generates one fixed Int-only operation chain. This module builds
//! a *typed expression tree* and emits source from it, so the same tree is the
//! oracle. Every generated program must:
//!
//! 1. typecheck (it is well-typed by construction — a rejection is a
//!    false-negative in the typechecker);
//! 2. execute in the reference interpreter; and
//! 3. agree with the model, on both `Int` and `Float` results.
//!
//! Coverage: typed `let`, `when` guards, `match` on `Int` and on an `enum`,
//! `List<Int>` indexing, and `Float` arithmetic. Bounds (literals in [-8,8],
//! args in [-20,20], depth <= 3, nonzero divisor literals, exactly
//! representable floats) keep a disagreement actionable rather than an
//! overflow, trap, or rounding artifact.

use crate::ast::TopLevel;
use crate::interpreter::Interpreter;

const MAX_DEPTH: u32 = 3;
const LIT_MAX: i64 = 8;
const ARG_MAX: i64 = 20;
const MAX_CASES: usize = 3;
const LIST_LEN: usize = 3;
/// Float operands are drawn from this exactly-representable integer range.
const FLOAT_MAX: i64 = 8;

/// The single `enum E` variant chosen for a program's `k`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    A,
    B,
    C,
}

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Variant::A => "A",
            Variant::B => "B",
            Variant::C => "C",
        }
    }
}

/// Which bindings are in scope at a generation point (declaration order).
#[derive(Clone, Copy)]
struct Scope {
    local: bool,
    list: bool,
    enum_: bool,
}

impl Scope {
    const PARAMS_ONLY: Scope = Scope { local: false, list: false, enum_: false };
    const FULL: Scope = Scope { local: true, list: true, enum_: true };
}

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
    Div(Box<IExpr>, i64),
    Rem(Box<IExpr>, i64),
    Neg(Box<IExpr>),
    If(BExpr, Box<IExpr>, Box<IExpr>),
    MatchZero(Box<IExpr>, Box<IExpr>, Box<IExpr>),
    /// `v[k]` — index into the `List<Int>` local.
    ListIndex(usize),
    /// `match k { A => a, B => b, C => c, }`.
    EnumMatch(Box<IExpr>, Box<IExpr>, Box<IExpr>),
}

/// A Bool-valued expression.
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

/// A Float-valued expression for the `g` defn.
#[derive(Clone, Debug)]
enum FExpr {
    Lit(f64),
    /// 0 = `a`, 1 = `b`.
    Param(u8),
    Add(Box<FExpr>, Box<FExpr>),
    Sub(Box<FExpr>, Box<FExpr>),
    Mul(Box<FExpr>, Box<FExpr>),
    Div(Box<FExpr>, f64),
    Neg(Box<FExpr>),
}

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

    fn float_lit(&mut self) -> f64 {
        ((self.byte() % (2 * FLOAT_MAX as u8 + 1)) as i64 - FLOAT_MAX) as f64
    }

    fn float_divisor(&mut self) -> f64 {
        ((self.byte() % 9) as i64 + 1) as f64
    }

    fn leaf(&mut self, scope: Scope) -> IExpr {
        let choices = if scope.local { 3 } else { 2 };
        match self.byte() % choices {
            0 => IExpr::Lit(self.lit()),
            1 => IExpr::Param(self.byte() & 1),
            _ => IExpr::Local,
        }
    }

    fn iexpr(&mut self, depth: u32, scope: Scope) -> IExpr {
        if depth == 0 || self.byte() % 4 == 0 {
            return self.leaf(scope);
        }
        let mut variants = 9u8;
        if scope.list {
            variants += 1;
        }
        if scope.enum_ {
            variants += 1;
        }
        match self.byte() % variants {
            0 => IExpr::Add(
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
            ),
            1 => IExpr::Sub(
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
            ),
            2 => IExpr::Mul(
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
            ),
            3 => IExpr::Div(Box::new(self.iexpr(depth - 1, scope)), self.divisor()),
            4 => IExpr::Rem(Box::new(self.iexpr(depth - 1, scope)), self.divisor()),
            5 => IExpr::Neg(Box::new(self.iexpr(depth - 1, scope))),
            6 => IExpr::If(
                self.bexpr(depth - 1, scope),
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
            ),
            7 => IExpr::MatchZero(
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
            ),
            8 if scope.list => IExpr::ListIndex((self.byte() as usize) % LIST_LEN),
            9 if scope.enum_ => IExpr::EnumMatch(
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
                Box::new(self.iexpr(depth - 1, scope)),
            ),
            _ => self.leaf(scope),
        }
    }

    fn bexpr(&mut self, depth: u32, scope: Scope) -> BExpr {
        if depth == 0 || self.byte() % 3 == 0 {
            if self.byte() & 1 == 0 {
                return BExpr::Lit(self.byte() & 1 == 0);
            }
            return self.cmp(scope);
        }
        match self.byte() % 5 {
            0 => BExpr::And(
                Box::new(self.bexpr(depth - 1, scope)),
                Box::new(self.bexpr(depth - 1, scope)),
            ),
            1 => BExpr::Or(
                Box::new(self.bexpr(depth - 1, scope)),
                Box::new(self.bexpr(depth - 1, scope)),
            ),
            2 => BExpr::Not(Box::new(self.bexpr(depth - 1, scope))),
            _ => self.cmp(scope),
        }
    }

    fn cmp(&mut self, scope: Scope) -> BExpr {
        let l = self.iexpr(0, scope);
        let r = self.iexpr(0, scope);
        match self.byte() % 4 {
            0 => BExpr::Lt(Box::new(l), Box::new(r)),
            1 => BExpr::Gt(Box::new(l), Box::new(r)),
            2 => BExpr::Eq(Box::new(l), Box::new(r)),
            _ => BExpr::Ne(Box::new(l), Box::new(r)),
        }
    }

    fn fexpr(&mut self, depth: u32) -> FExpr {
        if depth == 0 || self.byte() % 3 == 0 {
            if self.byte() & 1 == 0 {
                return FExpr::Lit(self.float_lit());
            }
            return FExpr::Param(self.byte() & 1);
        }
        match self.byte() % 5 {
            0 => FExpr::Add(Box::new(self.fexpr(depth - 1)), Box::new(self.fexpr(depth - 1))),
            1 => FExpr::Sub(Box::new(self.fexpr(depth - 1)), Box::new(self.fexpr(depth - 1))),
            2 => FExpr::Mul(Box::new(self.fexpr(depth - 1)), Box::new(self.fexpr(depth - 1))),
            3 => FExpr::Div(Box::new(self.fexpr(depth - 1)), self.float_divisor()),
            _ => FExpr::Neg(Box::new(self.fexpr(depth - 1))),
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
            format!("match {} {{ 0 => {}, _ => {}, }}", emit_i(v), emit_i(a), emit_i(b))
        }
        IExpr::ListIndex(k) => format!("v[{k}]"),
        IExpr::EnumMatch(a, b, c) => format!(
            "match k {{ E.A => {}, E.B => {}, E.C => {}, }}",
            emit_i(a),
            emit_i(b),
            emit_i(c)
        ),
    }
}

fn emit_b(e: &BExpr) -> String {
    match e {
        BExpr::Lit(true) => "true".to_string(),
        BExpr::Lit(false) => "false".to_string(),
        BExpr::Lt(a, b) => format!("({} < {})", emit_i(a), emit_i(b)),
        BExpr::Gt(a, b) => format!("({} > {})", emit_i(a), emit_i(b)),
        BExpr::Eq(a, b) => format!("({} == {})", emit_i(a), emit_i(b)),
        BExpr::Ne(a, b) => format!("({} != {})", emit_i(a), emit_i(b)),
        BExpr::And(a, b) => format!("({} && {})", emit_b(a), emit_b(b)),
        BExpr::Or(a, b) => format!("({} || {})", emit_b(a), emit_b(b)),
        BExpr::Not(a) => format!("(!{})", emit_b(a)),
    }
}

fn emit_f(e: &FExpr) -> String {
    match e {
        FExpr::Lit(v) => format!("{v:?}"),
        FExpr::Param(0) => "a".to_string(),
        FExpr::Param(_) => "b".to_string(),
        FExpr::Add(a, b) => format!("({} + {})", emit_f(a), emit_f(b)),
        FExpr::Sub(a, b) => format!("({} - {})", emit_f(a), emit_f(b)),
        FExpr::Mul(a, b) => format!("({} * {})", emit_f(a), emit_f(b)),
        FExpr::Div(a, d) => format!("({} / {d:?})", emit_f(a)),
        FExpr::Neg(a) => format!("(-{})", emit_f(a)),
    }
}

/// Model environment: parameter and local values at one evaluation point.
#[derive(Clone, Copy)]
struct Env {
    s: i64,
    arg_e: i64,
    x: i64,
    v: [i64; LIST_LEN],
    variant: Variant,
}

fn eval_i(e: &IExpr, env: &Env) -> i64 {
    match e {
        IExpr::Lit(n) => *n,
        IExpr::Param(0) => env.s,
        IExpr::Param(_) => env.arg_e,
        IExpr::Local => env.x,
        IExpr::Add(a, b) => eval_i(a, env).wrapping_add(eval_i(b, env)),
        IExpr::Sub(a, b) => eval_i(a, env).wrapping_sub(eval_i(b, env)),
        IExpr::Mul(a, b) => eval_i(a, env).wrapping_mul(eval_i(b, env)),
        IExpr::Div(a, d) => eval_i(a, env).wrapping_div(*d),
        IExpr::Rem(a, d) => eval_i(a, env).wrapping_rem(*d),
        IExpr::Neg(a) => eval_i(a, env).wrapping_neg(),
        IExpr::If(c, a, b) => {
            if eval_b(c, env) {
                eval_i(a, env)
            } else {
                eval_i(b, env)
            }
        }
        IExpr::MatchZero(v, a, b) => {
            if eval_i(v, env) == 0 {
                eval_i(a, env)
            } else {
                eval_i(b, env)
            }
        }
        IExpr::ListIndex(k) => env.v[*k % LIST_LEN],
        IExpr::EnumMatch(a, b, c) => match env.variant {
            Variant::A => eval_i(a, env),
            Variant::B => eval_i(b, env),
            Variant::C => eval_i(c, env),
        },
    }
}

fn eval_b(e: &BExpr, env: &Env) -> bool {
    match e {
        BExpr::Lit(b) => *b,
        BExpr::Lt(a, b) => eval_i(a, env) < eval_i(b, env),
        BExpr::Gt(a, b) => eval_i(a, env) > eval_i(b, env),
        BExpr::Eq(a, b) => eval_i(a, env) == eval_i(b, env),
        BExpr::Ne(a, b) => eval_i(a, env) != eval_i(b, env),
        BExpr::And(a, b) => eval_b(a, env) && eval_b(b, env),
        BExpr::Or(a, b) => eval_b(a, env) || eval_b(b, env),
        BExpr::Not(a) => !eval_b(a, env),
    }
}

fn eval_f(e: &FExpr, a: f64, b: f64) -> f64 {
    match e {
        FExpr::Lit(v) => *v,
        FExpr::Param(0) => a,
        FExpr::Param(_) => b,
        FExpr::Add(l, r) => eval_f(l, a, b) + eval_f(r, a, b),
        FExpr::Sub(l, r) => eval_f(l, a, b) - eval_f(r, a, b),
        FExpr::Mul(l, r) => eval_f(l, a, b) * eval_f(r, a, b),
        FExpr::Div(l, d) => eval_f(l, a, b) / d,
        FExpr::Neg(l) => -eval_f(l, a, b),
    }
}

/// A generated program plus the model's expected results.
pub struct Generated {
    pub source: String,
    pub ints: Vec<(String, i64)>,
    pub floats: Vec<(String, f64)>,
}

/// Generate a well-typed program and the model's expected results.
pub fn program(data: &[u8]) -> Option<Generated> {
    if data.len() < 6 {
        return None;
    }
    let mut b = Builder { data, pos: 0 };
    // Declaration order: x, then v, then k. So `init` sees only params, list
    // elements see x, and guards/arms see everything.
    let init = b.iexpr(MAX_DEPTH, Scope::PARAMS_ONLY);
    let list_elems = [
        b.iexpr(MAX_DEPTH, Scope { local: true, list: false, enum_: false }),
        b.iexpr(MAX_DEPTH, Scope { local: true, list: false, enum_: false }),
        b.iexpr(MAX_DEPTH, Scope { local: true, list: false, enum_: false }),
    ];
    let variant = match b.byte() % 3 {
        0 => Variant::A,
        1 => Variant::B,
        _ => Variant::C,
    };
    let cond = b.bexpr(MAX_DEPTH, Scope::FULL);
    let then_ = b.iexpr(MAX_DEPTH, Scope::FULL);
    let final_ = b.iexpr(MAX_DEPTH, Scope::FULL);
    let fexpr = b.fexpr(MAX_DEPTH);
    let cases = 1 + (b.byte() as usize % MAX_CASES);
    let float_cases = b.byte() as usize % (MAX_CASES + 1);

    let mut source = String::from("enum E { A, B, C };\n");
    source.push_str(
        "defn pick(cond: Bool, a: Int, b: Int) -> Int {\n    when cond { term a; };\n    term b;\n};\n",
    );
    source.push_str("defn f(s: Int, e: Int) -> Int {\n");
    source.push_str(&format!("    let x: Int = {};\n", emit_i(&init)));
    source.push_str(&format!(
        "    let v: List<Int> = [{}, {}, {}];\n",
        emit_i(&list_elems[0]),
        emit_i(&list_elems[1]),
        emit_i(&list_elems[2])
    ));
    source.push_str(&format!("    let k: E = E.{};\n", variant.name()));
    source.push_str(&format!("    when {} {{ x = {}; }};\n", emit_b(&cond), emit_i(&then_)));
    source.push_str(&format!("    term {};\n}};\n", emit_i(&final_)));
    source.push_str(&format!(
        "defn g(a: Float, b: Float) -> Float {{\n    term {};\n}};\n",
        emit_f(&fexpr)
    ));

    let mut ints = Vec::with_capacity(cases);
    for i in 0..cases {
        let s = b.arg();
        let e = b.arg();
        let name = format!("r{i}");
        source.push_str(&format!("let {name}: Int = f({s}, {e});\n"));
        // Model: x, then v, then guard, then final.
        let base = Env { s, arg_e: e, x: 0, v: [0; LIST_LEN], variant };
        let x = eval_i(&init, &base);
        let with_x = Env { x, ..base };
        let v = [
            eval_i(&list_elems[0], &with_x),
            eval_i(&list_elems[1], &with_x),
            eval_i(&list_elems[2], &with_x),
        ];
        let env = Env { v, ..with_x };
        let x = if eval_b(&cond, &env) { eval_i(&then_, &env) } else { x };
        let env = Env { x, ..env };
        ints.push((name, eval_i(&final_, &env)));
    }

    let mut floats = Vec::with_capacity(float_cases);
    for i in 0..float_cases {
        let a = ((b.byte() % (2 * FLOAT_MAX as u8 + 1)) as i64 - FLOAT_MAX) as f64;
        let c = ((b.byte() % (2 * FLOAT_MAX as u8 + 1)) as i64 - FLOAT_MAX) as f64;
        let name = format!("fr{i}");
        source.push_str(&format!("let {name}: Float = g({a:?}, {c:?});\n"));
        floats.push((name, eval_f(&fexpr, a, c)));
    }

    Some(Generated { source, ints, floats })
}

/// Run the generated program through the reference interpreter.
fn interpret(
    items: &[TopLevel],
    ints: &[(String, i64)],
    floats: &[(String, f64)],
) -> Result<(Vec<i64>, Vec<f64>), String> {
    let mut interpreter = Interpreter::new();
    interpreter.load_program(items);
    for item in items {
        if let TopLevel::Statement(stmt) = item {
            interpreter.exec_stmt(stmt).map_err(|e| e.to_string())?;
        }
    }
    let got_ints = ints
        .iter()
        .map(|(name, _)| {
            interpreter
                .state
                .get(name)
                .and_then(|v| v.as_i64())
                .ok_or_else(|| format!("missing integer result {name}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let got_floats = floats
        .iter()
        .map(|(name, _)| {
            interpreter
                .state
                .get(name)
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("missing float result {name}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((got_ints, got_floats))
}

/// Generate, typecheck (must accept), interpret, and compare to the model.
/// Panics on any disagreement — the fuzzer treats a panic as a finding.
pub fn check(data: &[u8]) {
    let Some(generated) = program(data) else {
        return;
    };
    let source = &generated.source;
    let items = match crate::library::parse_and_check("typed.bv", source) {
        Ok((items, _)) => items,
        Err(e) => panic!("well-typed program rejected by the frontend: {e}\n{source}"),
    };
    let (got_ints, got_floats) = interpret(&items, &generated.ints, &generated.floats)
        .unwrap_or_else(|e| panic!("reference interpreter failed: {e}\n{source}"));
    let want_ints: Vec<i64> = generated.ints.iter().map(|(_, v)| *v).collect();
    assert_eq!(got_ints, want_ints, "int model disagreement\n{source}");
    let want_floats: Vec<f64> = generated.floats.iter().map(|(_, v)| *v).collect();
    assert_eq!(got_floats, want_floats, "float model disagreement\n{source}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_inputs_generate_nothing() {
        assert!(program(&[]).is_none());
        assert!(program(&[1, 2, 3]).is_none());
    }

    fn pseudo_random_bytes(state: &mut u64, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| {
                *state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (*state >> 33) as u8
            })
            .collect()
    }

    #[test]
    fn generated_programs_typecheck_and_agree() {
        // Deterministic pseudo-random bytes; every generated program must
        // typecheck, interpret, and agree with the model (ints and floats).
        let mut state: u64 = 0x9E3779B97F4A7C15;
        for _ in 0..600 {
            let bytes = pseudo_random_bytes(&mut state, 96);
            check(&bytes);
        }
    }

    #[test]
    fn constructs_are_exercised() {
        // A byte string chosen so the emitted source uses every construct.
        let bytes: Vec<u8> = (0u8..128).cycle().take(128).collect();
        let g = program(&bytes).unwrap();
        for needle in ["enum E", "defn pick", "let v: List<Int>", "let k: E = E.", "defn g"] {
            assert!(g.source.contains(needle), "missing {needle}\n{}", g.source);
        }
        check(&bytes);
    }

    #[test]
    fn negative_float_literal_is_a_float() {
        // 2026-11-10 (found by this generator): the interpreter collapsed a
        // negated Float to `Int(0)` because `Neg` used `as_i64().unwrap_or(0)`.
        // Fixed in `eval_unary_op`; this is the minimal regression.
        let src = "let fr0: Float = -1.5;\n";
        let items = crate::library::parse_and_check("p.bv", src).unwrap().0;
        let mut interp = Interpreter::new();
        interp.load_program(&items);
        for it in &items {
            if let TopLevel::Statement(s) = it {
                interp.exec_stmt(s).unwrap();
            }
        }
        assert_eq!(interp.state.get("fr0").and_then(|v| v.as_f64()), Some(-1.5));
    }
}
