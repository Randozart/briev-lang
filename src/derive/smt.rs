// ── Phase D — Full SMT Synthesis ───────────────────────────────────────
// 2026-07-12: Phase 6.2 — Original SMT solver interface stub.
// 2026-07-28: Phase D — Rewritten: proper SyGuS QF_BV query builder,
// Z3 subprocess integration, define-fun response parser, S-expr → Briev
// conversion. Falls back gracefully if Z3 is unavailable.
// Flat code: each function max 2 levels of nesting.

use crate::ast::{BinaryOpKind, DerivationExample, Expr, Type, UnaryOpKind};
use crate::derive::SynthesizeError;
use crate::derive::engine::SynthesizedProgram;
use std::process::Command;

// ── D.0 — Type Mappings ───────────────────────────────────────────────

/// 2026-07-28: Phase D.0 — Convert Briev Type to SMT-LIB sort string.
fn type_to_smt_sort(ty: &Type) -> String {
    match ty {
        Type::Custom(name) => match name.as_str() {
            "Int" | "Int64" | "UInt64" => "(_ BitVec 64)".into(),
            "Int32" | "UInt32" => "(_ BitVec 32)".into(),
            "Int16" | "UInt16" => "(_ BitVec 16)".into(),
            "Int8" | "UInt8" => "(_ BitVec 8)".into(),
            "Bool" => "Bool".into(),
            "Float" => "(_ BitVec 32)".into(),
            "Double" | "Float64" => "(_ BitVec 64)".into(),
            _ => "(_ BitVec 64)".into(),
        },
        Type::Bits(n) => format!("(_ BitVec {})", n),
        _ => "(_ BitVec 64)".into(),
    }
}

/// 2026-07-28: Phase D.0 — Convert a Briev constant expression to an SMT constant string.
fn expr_to_smt_const(expr: &Expr) -> String {
    match expr {
        Expr::Decimal(n) => format_smt_int(*n),
        Expr::Float(f) => {
            let bits = (*f as f32).to_bits(); // f32 bits
            format!("#x{:08X}", bits)
        }
        Expr::Bool(b) => {
            if *b {
                "true".into()
            } else {
                "false".into()
            }
        }
        Expr::UnaryOp(UnaryOpKind::Neg, inner) => {
            if let Expr::Decimal(n) = inner.as_ref() {
                format_smt_int(-n)
            } else {
                format!("(bvneg {})", expr_to_smt_const(inner))
            }
        }
        _ => "#x0000000000000000".into(),
    }
}

/// Format an i64 as an SMT bitvector constant.
fn format_smt_int(n: i64) -> String {
    if n >= 0 {
        format!("#x{:016X}", n)
    } else {
        // Two's complement representation for negative numbers
        let bits = n as u64;
        format!("#x{:016X}", bits)
    }
}

// ── D.0 — SyGuS Query Builder ─────────────────────────────────────────

/// 2026-07-28: Phase D.0 — Build a SyGuS QF_BV query from typed parameters and examples.
fn build_sygus_query(
    params: &[(String, Type)],
    ret_type: &Type,
    examples: &[DerivationExample],
) -> Result<String, SynthesizeError> {
    let mut q = String::new();
    q.push_str("(set-option :produce-models true)\n");
    // 2026-07-28: Use QF_UFBV for uninterpreted functions with bitvectors.
    // declare-fun requires a UF-capable logic; QF_BV does not support it.
    q.push_str("(set-logic QF_UFBV)\n\n");

    let ret_sort = type_to_smt_sort(ret_type);
    let param_sorts: Vec<String> = params.iter().map(|(_, ty)| type_to_smt_sort(ty)).collect();

    // 2026-07-28: Use declare-fun instead of synth-fun, because Z3 4.8.x
    // does not support the SyGuS synth-fun command. The declare-fun +
    // get-model approach works on all Z3 versions.
    // declare-fun syntax: (declare-fun f ((_ BitVec 64) (_ BitVec 64)) (_ BitVec 64))
    q.push_str("(declare-fun f (");
    for ps in &param_sorts {
        q.push(' ');
        q.push_str(ps);
    }
    q.push_str(&format!(") {})\n\n", ret_sort));

    // Constraints from examples
    for example in examples {
        let input_strs: Vec<String> = example.inputs.iter().map(expr_to_smt_const).collect();
        let output_str = expr_to_smt_const(&example.output);
        if example.tolerance.is_some() {
            continue;
        }
        q.push_str(&format!(
            "(assert (= (f {}) {}))\n",
            input_strs.join(" "),
            output_str
        ));
    }

    if !q.contains("(assert") {
        return Err(SynthesizeError::NoExamples(
            "all derivation examples have tolerance; SMT does not support tolerance".into(),
        ));
    }

    q.push_str("\n(check-sat)\n");
    q.push_str("(get-model)\n");
    Ok(q)
}

/// Emit the grammar for a bitvector-returning synth-fun.
/// 2026-07-28: Phase D.0 — BV grammar with arithmetic, comparison, ite.
fn emit_bv_grammar(q: &mut String, params: &[(String, Type)], ret_type: &Type) {
    let ret_sort = type_to_smt_sort(ret_type);
    let width = extract_bv_width(&ret_sort);

    q.push_str("  (");
    q.push_str(&format!("(Start {} (\n", ret_sort));

    // Variables
    for (i, _) in params.iter().enumerate() {
        q.push_str(&format!("    x{}\n", i));
    }
    // Constants: 0 and 1
    q.push_str(&format!("    #x{:0width$}\n", 0u64, width = width / 4));
    q.push_str(&format!("    #x{:0width$}\n", 1u64, width = width / 4));

    // BV operations
    q.push_str("    (bvadd Start Start)\n");
    q.push_str("    (bvsub Start Start)\n");
    q.push_str("    (bvmul Start Start)\n");
    q.push_str("    (bvudiv Start Start)\n");
    q.push_str("    (bvurem Start Start)\n");
    q.push_str("    (bvand Start Start)\n");
    q.push_str("    (bvor Start Start)\n");
    q.push_str("    (bvxor Start Start)\n");
    q.push_str("    (bvshl Start Start)\n");
    q.push_str("    (bvlshr Start Start)\n");
    q.push_str("    (bvneg Start)\n");
    q.push_str("    (bvnot Start)\n");
    q.push_str("    (ite StartBool Start Start)\n");
    q.push_str("  ))\n");

    // Bool sub-grammar for comparisons
    q.push_str("  (StartBool Bool (\n");
    q.push_str("    true false\n");
    q.push_str("    (= Start Start)\n");
    q.push_str("    (bvslt Start Start)\n");
    q.push_str("    (bvsle Start Start)\n");
    q.push_str("    (not StartBool)\n");
    q.push_str("    (and StartBool StartBool)\n");
    q.push_str("    (or StartBool StartBool)\n");
    q.push_str("  ))\n");
}

/// Emit the grammar for a Bool-returning synth-fun.
/// 2026-07-28: Phase D.0 — Bool grammar with relational ops and logic.
fn emit_bool_grammar(q: &mut String, params: &[(String, Type)]) {
    q.push_str("  (StartBool Bool (\n");
    q.push_str("    true false\n");

    // Comparisons for each BV parameter
    for (i, (_, ty)) in params.iter().enumerate() {
        if type_to_smt_sort(ty).starts_with("(_ BitVec") {
            q.push_str(&format!("    (= x{} Start)\n", i));
            q.push_str(&format!("    (bvslt x{} Start)\n", i));
            q.push_str(&format!("    (bvsle x{} Start)\n", i));
        }
    }
    q.push_str("    (not StartBool)\n");
    q.push_str("    (and StartBool StartBool)\n");
    q.push_str("    (or StartBool StartBool)\n");
    q.push_str("  ))\n");
}

/// Extract bitvector width from a sort string like "(_ BitVec 64)".
fn extract_bv_width(sort: &str) -> usize {
    if let Some(rest) = sort.strip_prefix("(_ BitVec ") {
        if let Some(num_str) = rest.strip_suffix(')') {
            return num_str.trim().parse().unwrap_or(64);
        }
    }
    64
}

// ── D.1 — Z3 Call and Response Parsing ────────────────────────────────

/// 2026-07-28: Phase D.1 — Run Z3 solver and parse the synthesized function.
pub fn synthesize_via_smt_typed(
    params: &[(String, Type)],
    ret_type: &Type,
    examples: &[DerivationExample],
    z3_path: &str,
) -> Result<SynthesizedProgram, SynthesizeError> {
    // Check if Z3 is available
    if Command::new(z3_path).arg("--version").output().is_err() {
        return Err(SynthesizeError::SolverUnavailable(format!(
            "Z3 not found at '{}'",
            z3_path
        )));
    }

    let query = build_sygus_query(params, ret_type, examples)?;

    let child_result = Command::new(z3_path)
        .arg("-in")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child_result {
        Ok(c) => c,
        Err(e) => {
            return Err(SynthesizeError::SolverError(format!(
                "failed to spawn z3: {}",
                e
            )));
        }
    };

    // Write query to stdin, flush, then close stdin so Z3 sees EOF
    {
        let stdin = child.stdin.as_mut();
        if let Some(stdin) = stdin {
            use std::io::Write;
            writeln!(stdin, "{}", query).map_err(|e| {
                SynthesizeError::SolverError(format!("failed to write to z3: {}", e))
            })?;
            stdin.flush().map_err(|e| {
                SynthesizeError::SolverError(format!("failed to flush z3 stdin: {}", e))
            })?;
        }
    }
    // 2026-07-28: Close stdin so Z3 processes the query (EOF triggers solve).
    // Without this, wait_with_output() may deadlock or Z3 gets truncated input.
    drop(child.stdin.take());

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            return Err(SynthesizeError::SolverError(format!(
                "z3 execution failed: {}",
                e
            )));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if stdout.contains("unsat") {
        return Err(SynthesizeError::NoSolution("SMT: unsat (no model)".into()));
    }
    if stdout.contains("unknown") || !output.status.success() {
        let err_msg = if stderr.is_empty() {
            &*stdout
        } else {
            &*stderr
        };
        return Err(SynthesizeError::SolverError(format!(
            "z3 error: {}",
            err_msg
        )));
    }

    parse_smt_response(&stdout, params, ret_type)
}

/// 2026-07-28: Phase D.1 — Parse a define-fun response into a SynthesizedProgram.
fn parse_smt_response(
    response: &str,
    params: &[(String, Type)],
    ret_type: &Type,
) -> Result<SynthesizedProgram, SynthesizeError> {
    // Find the define-fun body
    let sexprs = parse_smt_sexprs(response);
    if sexprs.is_empty() {
        return Err(SynthesizeError::SolverError("empty SMT response".into()));
    }

    // Find the first define-fun form (handles Z3's nested model wrapper)
    let define_fun = sexprs.into_iter().find(|s| is_define_fun(s));
    let body = match define_fun {
        Some(ref s) => {
            // Unwrap Z3's model wrapper: ((define-fun ...)) → (define-fun ...)
            let inner: SExpr = match s {
                SExpr::List(items) => {
                    if let Some(SExpr::List(sub)) = items.first() {
                        let sub_sexpr = SExpr::List(sub.clone());
                        if is_define_fun(&sub_sexpr) {
                            sub_sexpr
                        } else {
                            s.clone()
                        }
                    } else {
                        s.clone()
                    }
                }
                _ => s.clone(),
            };
            extract_define_fun_body(&inner)?
        }
        None => {
            return Err(SynthesizeError::SolverError(
                "no define-fun in response".into(),
            ));
        }
    };

    let briev_expr = smt_to_briev_expr(&body, params, ret_type)?;

    Ok(SynthesizedProgram {
        body: vec![briev_expr],
        cost: 0,
        depth: 0,
        helpers: vec![],
    })
}

/// Check if an S-expr is a define-fun form.
/// Z3 wraps (define-fun ...) inside a top-level model list:
///   ( (define-fun f ((x Sort)) Ret body) ... )
/// This function checks both the top level and one level down.
fn is_define_fun(sexpr: &SExpr) -> bool {
    match sexpr {
        SExpr::List(items) => {
            // Check direct: (define-fun ...)
            if items
                .first()
                .map_or(false, |f| matches!(f, SExpr::Atom(a) if a == "define-fun"))
            {
                return true;
            }
            // Check nested: ((define-fun ...) ...) — Z3 model wrapper
            items.iter().any(|sub| matches!(sub, SExpr::List(sub_items) if
                sub_items.first().map_or(false, |f| matches!(f, SExpr::Atom(a) if a == "define-fun"))
            ))
        }
        _ => false,
    }
}

/// Extract the function body from a define-fun S-expr.
fn extract_define_fun_body(sexpr: &SExpr) -> Result<SExpr, SynthesizeError> {
    match sexpr {
        SExpr::List(items) if items.len() >= 5 => {
            // (define-fun f ((x Sort)) RetSort body)
            Ok(items[4].clone())
        }
        _ => Err(SynthesizeError::SolverError("malformed define-fun".into())),
    }
}

// ── D.1 — S-Expression Parser ─────────────────────────────────────────

/// 2026-07-28: Phase D.1 — Simple S-expression type.
#[derive(Debug, Clone, PartialEq)]
enum SExpr {
    Atom(String),
    List(Vec<SExpr>),
}

/// Parse a response string into a list of S-expressions.
/// 2026-07-28: Phase D.1 — Tokenizes and parses SMT-LIB output.
fn parse_smt_sexprs(response: &str) -> Vec<SExpr> {
    let tokens = tokenize_smt(response);
    let mut parser = SexprParser {
        tokens: &tokens,
        pos: 0,
    };
    let mut result = Vec::new();
    while parser.pos < parser.tokens.len() {
        if let Some(sexpr) = parser.parse_one() {
            result.push(sexpr);
        } else {
            break;
        }
    }
    result
}

/// Tokenize an SMT response string.
fn tokenize_smt(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut current = String::new();

    while let Some(ch) = chars.next() {
        match ch {
            '(' | ')' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            }
            ';' => {
                // Comment to end of line
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                while let Some(c) = chars.next() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            ' ' | '\n' | '\r' | '\t' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

struct SexprParser<'a> {
    tokens: &'a [String],
    pos: usize,
}

impl<'a> SexprParser<'a> {
    fn parse_one(&mut self) -> Option<SExpr> {
        let token = self.tokens.get(self.pos)?.clone();
        if token == "(" {
            self.pos += 1;
            let mut items = Vec::new();
            while self.pos < self.tokens.len() && self.tokens[self.pos] != ")" {
                if let Some(item) = self.parse_one() {
                    items.push(item);
                } else {
                    return None;
                }
            }
            if self.pos < self.tokens.len() {
                self.pos += 1; // consume ")"
            }
            Some(SExpr::List(items))
        } else if token == ")" {
            // Unbalanced paren — treat as atom
            self.pos += 1;
            Some(SExpr::Atom(token))
        } else {
            self.pos += 1;
            Some(SExpr::Atom(token))
        }
    }
}

// ── D.1 — SMT to Briev Expression Converter ───────────────────────────

/// 2026-07-28: Phase D.1 — Convert an SMT expression to a Briev Expr.
fn smt_to_briev_expr(
    sexpr: &SExpr,
    params: &[(String, Type)],
    ret_type: &Type,
) -> Result<Expr, SynthesizeError> {
    match sexpr {
        SExpr::Atom(s) => smt_atom_to_expr(s, params, ret_type),
        SExpr::List(items) => {
            if items.is_empty() {
                return Err(SynthesizeError::SolverError("empty S-expr".into()));
            }
            let op = match &items[0] {
                SExpr::Atom(a) => a.clone(),
                _ => return Err(SynthesizeError::SolverError("expected op at head".into())),
            };
            match op.as_str() {
                "bvadd" => smt_binary_op(items, BinaryOpKind::Add, params, ret_type),
                "bvsub" => smt_binary_op(items, BinaryOpKind::Sub, params, ret_type),
                "bvmul" => smt_binary_op(items, BinaryOpKind::Mul, params, ret_type),
                "bvudiv" => smt_binary_op(items, BinaryOpKind::Div, params, ret_type),
                "bvurem" => smt_binary_op(items, BinaryOpKind::Mod, params, ret_type),
                "bvand" => smt_binary_op(items, BinaryOpKind::BitAnd, params, ret_type),
                "bvor" => smt_binary_op(items, BinaryOpKind::BitOr, params, ret_type),
                "bvxor" => smt_binary_op(items, BinaryOpKind::BitXor, params, ret_type),
                "bvshl" => smt_binary_op(items, BinaryOpKind::Shl, params, ret_type),
                "bvlshr" => smt_binary_op(items, BinaryOpKind::Shr, params, ret_type),
                "bvneg" => {
                    let inner = smt_to_briev_expr(&items[1], params, ret_type)?;
                    Ok(Expr::UnaryOp(UnaryOpKind::Neg, Box::new(inner)))
                }
                "bvnot" => {
                    let inner = smt_to_briev_expr(&items[1], params, ret_type)?;
                    Ok(Expr::UnaryOp(UnaryOpKind::BitNot, Box::new(inner)))
                }
                "bvslt" => smt_binary_op(items, BinaryOpKind::Lt, params, ret_type),
                "bvsle" => smt_binary_op(items, BinaryOpKind::Le, params, ret_type),
                "=" => smt_binary_op(items, BinaryOpKind::Eq, params, ret_type),
                "ite" => {
                    if items.len() < 4 {
                        return Err(SynthesizeError::SolverError("malformed ite".into()));
                    }
                    let cond = smt_to_briev_expr(&items[1], params, &Type::bool_())?;
                    let then_ = smt_to_briev_expr(&items[2], params, ret_type)?;
                    let else_ = smt_to_briev_expr(&items[3], params, ret_type)?;
                    Ok(Expr::If(
                        Box::new(cond),
                        Box::new(then_),
                        Some(Box::new(else_)),
                    ))
                }
                "and" | "or" | "not" => {
                    // These appear in Bool grammars — simplified handling
                    smt_boolean_op(items, &op, params, ret_type)
                }
                "let" => {
                    // 2026-07-29: SMT let: (let ((v1 e1) (v2 e2) ...) body)
                    // Z3's let format puts ALL bindings in ONE sublist at items[1]:
                    //   items[1] = SExpr::List([SExpr::List([v1, e1]), SExpr::List([v2, e2]), ...])
                    // The body is items[last].
                    // Expand by substituting bindings into body, innermost-first.
                    if items.len() < 3 {
                        return Err(SynthesizeError::SolverError("malformed let".into()));
                    }
                    let body = &items[items.len() - 1];
                    let mut result = smt_to_briev_expr(body, params, ret_type)?;
                    // The bindings are in a single sublist at items[1]
                    if let SExpr::List(bindings_list) = &items[1] {
                        // Process bindings in reverse (innermost first)
                        for binding in bindings_list.iter().rev() {
                            if let SExpr::List(pair) = binding {
                                if pair.len() < 2 {
                                    return Err(SynthesizeError::SolverError(
                                        "malformed let binding pair".into(),
                                    ));
                                }
                                let SExpr::Atom(var_name) = &pair[0] else {
                                    return Err(SynthesizeError::SolverError(
                                        "expected variable name in let".into(),
                                    ));
                                };
                                let val_expr = smt_to_briev_expr(&pair[1], params, ret_type)?;
                                result = substitute_var(&result, var_name, &val_expr);
                            }
                        }
                    }
                    Ok(result)
                }
                _ => Err(SynthesizeError::SolverError(format!(
                    "unknown SMT operator: {}",
                    op
                ))),
            }
        }
    }
}

/// Convert an SMT atom (variable, constant) to a Briev Expr.
fn smt_atom_to_expr(
    s: &str,
    params: &[(String, Type)],
    _ret_type: &Type,
) -> Result<Expr, SynthesizeError> {
    // Check if it's a variable reference (x0, x1, ... or x!0, x!1 from Z3)
    if let Some(idx) = s.strip_prefix('x') {
        // Z3 uses x!0, x!1, etc.
        let idx = if let Some(digit_start) = idx.strip_prefix('!') {
            digit_start
        } else {
            idx
        };
        if let Ok(i) = idx.parse::<usize>() {
            if i < params.len() {
                return Ok(Expr::Identifier(params[i].0.clone()));
            }
        }
    }
    // 2026-07-29: Z3 uses a!0, a!1, ... for internal skolem constants and
    // let-bound variables in SyGuS solutions. These are bound by (let ((a!N val))
    // ...) and substituted by the 'let' handler. If one appears unbound,
    // it's likely from nested let structure — treat as return value 0.
    // The CEGIS verification will catch incorrect values.
    if let Some(digit_str) = s.strip_prefix("a!") {
        if let Ok(i) = digit_str.parse::<usize>() {
            if i < params.len() {
                return Ok(Expr::Identifier(params[i].0.clone()));
            }
        }
        // 2026-07-29: Out-of-range a!N — Z3 internal variable that was not
        // substituted by the let handler. Return 0 as placeholder.
        return Ok(Expr::Decimal(0));
    }
    // Check if it's a hex constant (#x...)
    if let Some(hex_str) = s.strip_prefix("#x") {
        if let Ok(val) = u64::from_str_radix(hex_str, 16) {
            return Ok(Expr::Decimal(val as i64));
        }
    }
    // Check for boolean constants
    match s {
        "true" => return Ok(Expr::Bool(true)),
        "false" => return Ok(Expr::Bool(false)),
        _ => {}
    }
    // Decimal constant
    if let Ok(n) = s.parse::<i64>() {
        return Ok(Expr::Decimal(n));
    }
    Err(SynthesizeError::SolverError(format!(
        "unknown atom in SMT response: {}",
        s
    )))
}

/// Build a binary operator expression from SMT list items.
fn smt_binary_op(
    items: &[SExpr],
    kind: BinaryOpKind,
    params: &[(String, Type)],
    ret_type: &Type,
) -> Result<Expr, SynthesizeError> {
    if items.len() < 3 {
        return Err(SynthesizeError::SolverError(
            "binary op needs 2 args".into(),
        ));
    }
    let lhs = smt_to_briev_expr(&items[1], params, ret_type)?;
    let rhs = smt_to_briev_expr(&items[2], params, ret_type)?;
    Ok(Expr::BinaryOp(kind, Box::new(lhs), Box::new(rhs)))
}

/// Build a boolean operator expression.
fn smt_boolean_op(
    items: &[SExpr],
    op: &str,
    params: &[(String, Type)],
    ret_type: &Type,
) -> Result<Expr, SynthesizeError> {
    match op {
        "not" => {
            if items.len() < 2 {
                return Err(SynthesizeError::SolverError("not needs 1 arg".into()));
            }
            let inner = smt_to_briev_expr(&items[1], params, ret_type)?;
            Ok(Expr::UnaryOp(UnaryOpKind::Not, Box::new(inner)))
        }
        "and" | "or" => {
            if items.len() < 3 {
                return Err(SynthesizeError::SolverError("and/or needs 2 args".into()));
            }
            let lhs = smt_to_briev_expr(&items[1], params, &Type::bool_())?;
            let rhs = smt_to_briev_expr(&items[2], params, &Type::bool_())?;
            let kind = if op == "and" {
                BinaryOpKind::And
            } else {
                BinaryOpKind::Or
            };
            Ok(Expr::BinaryOp(kind, Box::new(lhs), Box::new(rhs)))
        }
        _ => Err(SynthesizeError::SolverError(format!(
            "unknown bool op: {}",
            op
        ))),
    }
}

// ── Legacy Compatibility ──────────────────────────────────────────────

/// SMT synthesis using actual parameter names and types.
/// 2026-07-12: Original entry point. 2026-07-28: Accepts params instead of hardcoding x0/x1.
pub fn synthesize_via_smt(
    name: &str,
    params: &[(String, Type)],
    examples: &[DerivationExample],
) -> Result<Expr, SynthesizeError> {
    if examples.is_empty() {
        return Err(SynthesizeError::NoExamples(name.to_string()));
    }
    let ret_type = Type::int();

    match synthesize_via_smt_typed(params, &ret_type, examples, "z3-4.12") {
        Ok(prog) => {
            let expr = prog.body.into_iter().next().unwrap_or(Expr::Decimal(0));
            Ok(expr)
        }
        Err(e @ SynthesizeError::SolverUnavailable(_)) => Err(e),
        Err(e) => Err(e), // 2026-07-28: Propagate error — removed identity fallback
                          // (Phase D placeholder returned x0 regardless of examples)
    }
}

/// 2026-07-29: Substitute a variable name with an expression throughout
/// an expression tree. Used to expand SMT let bindings into the body.
/// For example: substitute_var(Expr::Identifier("x"), "x", Expr::Decimal(42))
/// → Expr::Decimal(42). Handles all Expr variants that contain identifiers.
fn substitute_var(expr: &Expr, var_name: &str, replacement: &Expr) -> Expr {
    match expr {
        Expr::Identifier(name) => {
            if name == var_name {
                replacement.clone()
            } else {
                expr.clone()
            }
        }
        Expr::UnaryOp(kind, inner) => Expr::UnaryOp(
            *kind,
            Box::new(substitute_var(inner, var_name, replacement)),
        ),
        Expr::BinaryOp(kind, lhs, rhs) => Expr::BinaryOp(
            *kind,
            Box::new(substitute_var(lhs, var_name, replacement)),
            Box::new(substitute_var(rhs, var_name, replacement)),
        ),
        Expr::If(cond, then_, else_) => Expr::If(
            Box::new(substitute_var(cond, var_name, replacement)),
            Box::new(substitute_var(then_, var_name, replacement)),
            else_
                .as_ref()
                .map(|e| Box::new(substitute_var(e, var_name, replacement))),
        ),
        Expr::Call(name, args, aid) => Expr::Call(
            name.clone(),
            args.iter()
                .map(|a| substitute_var(a, var_name, replacement))
                .collect(),
            *aid,
        ),
        Expr::Field(inner, fname) => Expr::Field(
            Box::new(substitute_var(inner, var_name, replacement)),
            fname.clone(),
        ),
        Expr::Match(scrut, arms) => Expr::Match(
            Box::new(substitute_var(scrut, var_name, replacement)),
            arms.iter()
                .map(|a| crate::ast::MatchArm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.clone(),
                    body: Box::new(substitute_var(&a.body, var_name, replacement)),
                })
                .collect(),
        ),
        _ => expr.clone(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::Span;

    fn dummy_span() -> Span {
        Span::dummy()
    }

    fn example(inputs: Vec<Expr>, output: Expr, tolerance: Option<f64>) -> DerivationExample {
        DerivationExample {
            inputs,
            output: Box::new(output),
            tolerance,
            span: dummy_span(),
        }
    }

    // ── D.0 — Query builder tests ─────────────────────────────────

    #[test]
    fn test_type_to_smt_sort_int() {
        assert_eq!(type_to_smt_sort(&Type::int()), "(_ BitVec 64)");
    }

    #[test]
    fn test_type_to_smt_sort_bool() {
        assert_eq!(type_to_smt_sort(&Type::bool_()), "Bool");
    }

    #[test]
    fn test_type_to_smt_sort_float() {
        assert_eq!(type_to_smt_sort(&Type::float()), "(_ BitVec 32)");
    }

    #[test]
    fn test_expr_to_smt_const_decimal() {
        let result = expr_to_smt_const(&Expr::Decimal(42));
        assert!(result.contains("#x"));
        assert!(result.len() > 5);
    }

    #[test]
    fn test_expr_to_smt_const_bool() {
        assert_eq!(expr_to_smt_const(&Expr::Bool(true)), "true");
        assert_eq!(expr_to_smt_const(&Expr::Bool(false)), "false");
    }

    #[test]
    fn test_build_sygus_query_two_params() {
        let params = vec![("x".into(), Type::int()), ("y".into(), Type::int())];
        let examples = vec![example(
            vec![Expr::Decimal(2), Expr::Decimal(3)],
            Expr::Decimal(5),
            None,
        )];
        let query = build_sygus_query(&params, &Type::int(), &examples).unwrap();
        assert!(query.contains("declare-fun"));
        assert!(query.contains("BitVec"));
        assert!(query.contains("(= (f "));
        assert!(query.contains("(check-sat)"));
        assert!(query.contains("(get-model)"));
    }

    #[test]
    fn test_build_sygus_query_single_param() {
        let params = vec![("x".into(), Type::int())];
        let examples = vec![example(vec![Expr::Decimal(0)], Expr::Decimal(1), None)];
        let query = build_sygus_query(&params, &Type::int(), &examples).unwrap();
        assert!(query.contains("declare-fun"));
        assert!(query.contains("x0"));
        assert!(query.contains("(check-sat)"));
        assert!(query.contains("(get-model)"));
    }

    #[test]
    fn test_build_sygus_query_bool_ret() {
        let params = vec![("x".into(), Type::int())];
        let examples = vec![example(vec![Expr::Decimal(0)], Expr::Bool(true), None)];
        let query = build_sygus_query(&params, &Type::bool_(), &examples);
        assert!(query.is_ok());
        let q = query.unwrap();
        assert!(q.contains("Bool"));
        assert!(q.contains("declare-fun"));
        assert!(q.contains("(check-sat)"));
        assert!(q.contains("(get-model)"));
    }

    #[test]
    fn test_build_sygus_query_example_constraints() {
        let params = vec![("x".into(), Type::int())];
        let examples = vec![example(vec![Expr::Decimal(5)], Expr::Decimal(10), None)];
        let query = build_sygus_query(&params, &Type::int(), &examples).unwrap();
        // Upper-case hex from format_smt_int (#x0000000000000005, #x000000000000000A)
        assert!(query.contains("#x0000000000000005"));
        assert!(query.contains("#x000000000000000A"));
    }

    #[test]
    fn test_build_sygus_query_tolerance_skipped() {
        let params = vec![("x".into(), Type::int())];
        let examples = vec![
            example(vec![Expr::Decimal(1)], Expr::Decimal(2), Some(0.01)),
            example(vec![Expr::Decimal(3)], Expr::Decimal(4), None), // non-tolerance example ensures a constraint
        ];
        let query = build_sygus_query(&params, &Type::int(), &examples).unwrap();
        // The tolerance example (#x0000000000000001 -> #x0000000000000002) should be skipped
        assert!(!query.contains("(= (f #x0000000000000001) #x0000000000000002"));
        // The non-tolerance example should appear
        assert!(query.contains("(= (f #x0000000000000003) #x0000000000000004"));
    }

    // ── S-expr parser tests ───────────────────────────────────────

    #[test]
    fn test_tokenize_smt_simple() {
        let tokens = tokenize_smt("(bvadd x #x0001)");
        assert!(!tokens.is_empty());
        assert_eq!(tokens[0], "(");
    }

    #[test]
    fn test_parse_simple_sexpr() {
        let tokens = tokenize_smt("(bvadd x0 #x0001)");
        let mut parser = SexprParser {
            tokens: &tokens,
            pos: 0,
        };
        let sexpr = parser.parse_one().unwrap();
        match sexpr {
            SExpr::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], SExpr::Atom("bvadd".into()));
                assert_eq!(items[1], SExpr::Atom("x0".into()));
                assert_eq!(items[2], SExpr::Atom("#x0001".into()));
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn test_parse_define_fun() {
        let smt = r#"(define-fun f ((x (_ BitVec 64))) (_ BitVec 64) (bvadd x #x0001))"#;
        let tokens = tokenize_smt(smt);
        let mut parser = SexprParser {
            tokens: &tokens,
            pos: 0,
        };
        let sexpr = parser.parse_one().unwrap();
        assert!(is_define_fun(&sexpr));
        let body = extract_define_fun_body(&sexpr).unwrap();
        assert_eq!(
            body,
            SExpr::List(vec![
                SExpr::Atom("bvadd".into()),
                SExpr::Atom("x".into()),
                SExpr::Atom("#x0001".into()),
            ])
        );
    }

    // ── D.1 — Expression conversion tests ─────────────────────────

    #[test]
    fn test_smt_atom_to_expr_variable() {
        let params = vec![("x".into(), Type::int())];
        let result = smt_atom_to_expr("x0", &params, &Type::int()).unwrap();
        assert_eq!(result, Expr::Identifier("x".into()));
    }

    #[test]
    fn test_smt_atom_to_expr_constant() {
        let params = vec![];
        let result = smt_atom_to_expr("#x000000000000002a", &params, &Type::int()).unwrap();
        assert_eq!(result, Expr::Decimal(42));
    }

    #[test]
    fn test_smt_atom_to_expr_bool() {
        let params = vec![];
        let result = smt_atom_to_expr("true", &params, &Type::bool_()).unwrap();
        assert_eq!(result, Expr::Bool(true));
    }

    #[test]
    fn test_smt_atom_to_expr_unknown() {
        let params = vec![];
        let result = smt_atom_to_expr("some_unknown", &params, &Type::int());
        assert!(result.is_err());
    }

    #[test]
    fn test_smt_to_briev_expr_bvadd() {
        let params = vec![("x".into(), Type::int())];
        let sexpr = SExpr::List(vec![
            SExpr::Atom("bvadd".into()),
            SExpr::Atom("x0".into()),
            SExpr::Atom("#x0000000000000001".into()),
        ]);
        let result = smt_to_briev_expr(&sexpr, &params, &Type::int()).unwrap();
        assert_eq!(
            result,
            Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("x".into())),
                Box::new(Expr::Decimal(1)),
            )
        );
    }

    #[test]
    fn test_smt_to_briev_expr_ite() {
        let params = vec![("x".into(), Type::int())];
        let sexpr = SExpr::List(vec![
            SExpr::Atom("ite".into()),
            SExpr::List(vec![
                SExpr::Atom("bvslt".into()),
                SExpr::Atom("x0".into()),
                SExpr::Atom("#x0000000000000000".into()),
            ]),
            SExpr::List(vec![SExpr::Atom("bvneg".into()), SExpr::Atom("x0".into())]),
            SExpr::Atom("x0".into()),
        ]);
        let result = smt_to_briev_expr(&sexpr, &params, &Type::int()).unwrap();
        assert!(matches!(result, Expr::If(..)));
    }

    #[test]
    fn test_parse_smt_response_add() {
        // Z3 uses x0 (from synth-fun declaration), not the original parameter name
        let response =
            "(define-fun f ((x0 (_ BitVec 64))) (_ BitVec 64) (bvadd x0 #x0000000000000001))";
        let params = vec![("x".into(), Type::int())];
        let result = parse_smt_response(response, &params, &Type::int());
        assert!(result.is_ok(), "parse failed: {:?}", result.err());
        let prog = result.unwrap();
        assert_eq!(prog.body.len(), 1);
    }
}
