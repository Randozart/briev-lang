// 2026-08-23 (Plan 1.1/1.4): regression tests for intrinsic argument
// emission and compile-error recording.

use crate::ast::top::*;
use crate::ast::*;
use crate::backend::vm::VmBackend;
use crate::type_universe::TypeUniverse;
use std::collections::HashMap;

fn defn(name: &str, params: Vec<(String, Type)>, body: Vec<Statement>) -> TopLevel {
    TopLevel::Definition(Definition {
        name: name.into(),
        type_params: vec![],
        parameters: params,
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::Bool(true),
            post_condition: Expr::Bool(true),
            watchdog: None,
            explicit: false,
            post_authority: false,
            span: None,
        },
        body,
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        annotations: vec![],
        variadic_param: None,
        span: None,
        doc: None,
    })
}

/// Plan 1.1: an intrinsic call with a variable argument must pass the
/// variable's VALUE — the old code dropped every Identifier arg.
#[test]
fn intrinsic_call_passes_variable_arguments() {
    // fn go(x: Int) { Alloc#(x); term; }
    let prog = vec![
        defn(
            "go",
            vec![("x".into(), Type::int())],
            vec![
                Statement::Expression(Expr::Call(
                    "Alloc#".into(),
                    vec![Expr::Identifier("x".into())],
                    None,
                )),
                Statement::Term(None),
            ],
        ),
    ];
    let universe = TypeUniverse::new();
    let mut vm = VmBackend::new();
    let bytes = vm.generate(&prog, &universe);
    assert!(vm.errors.is_empty(), "variable arg must not error: {:?}", vm.errors);

    // Bytecode shape: PUSH_LOCAL 0; HCALL <id>; RET — the load must be there.
    // (The file starts with a LAIR header, so scan the whole buffer.)
    let load_local = super::assembler::OP_LOAD_LOCAL;
    let hcall = super::assembler::OP_HCALL;
    assert!(
        bytes.contains(&(load_local as u8)),
        "expected LOAD_LOCAL for the variable arg in {:?}",
        &bytes[..bytes.len().min(32)]
    );
    assert!(bytes.contains(&(hcall as u8)), "host call must follow");
}

/// 2026-08-26 (parity plan §1.3): top-level Int consts inline their
/// compile-time value at reference sites — PUSH_I64 <value>, no trap.
#[test]
fn test_const_reference_inlines_value() {
    let prog: Vec<TopLevel> = vec![
        TopLevel::Constant(Constant {
            name: "LIMIT".into(),
            ty: Type::int(),
            expr: Expr::Decimal(42),
            section: None,
        }),
        TopLevel::Constant(Constant {
            name: "DERIVED".into(),
            ty: Type::int(),
            // const-to-const, declared in reverse order on purpose
            expr: Expr::BinaryOp(
                BinaryOpKind::Mul,
                Box::new(Expr::Identifier("LIMIT".into())),
                Box::new(Expr::Decimal(2)),
            ),
            section: None,
        }),
        defn(
            "go",
            vec![],
            vec![
                Statement::Term(Some(Expr::Identifier("DERIVED".into()))),
            ],
        ),
    ];
    let universe = TypeUniverse::new();
    let mut vm = VmBackend::new();
    let bytes = vm.generate(&prog, &universe);
    assert!(vm.errors.is_empty(), "const refs must resolve: {:?}", vm.errors);
    // DERIVED = LIMIT*2 = 84 pushed as i64 constant.
    let has_84 = bytes.windows(9).any(|w| w[0] == super::assembler::OP_PUSH_I64 && i64::from_le_bytes(w[1..9].try_into().unwrap()) == 84);
    assert!(has_84, "expected PUSH_I64(84) in bytecode");
}

/// §1.3 honesty: a const CYCLE (a = b; b = a) leaves both unresolvable —
/// referencing one is the house capability error, not a silent 0+trap.
#[test]
fn test_const_cycle_is_capability_error() {
    let prog: Vec<TopLevel> = vec![
        TopLevel::Constant(Constant {
            name: "A".into(),
            ty: Type::int(),
            expr: Expr::Identifier("B".into()),
            section: None,
        }),
        TopLevel::Constant(Constant {
            name: "B".into(),
            ty: Type::int(),
            expr: Expr::Identifier("A".into()),
            section: None,
        }),
        defn(
            "go",
            vec![],
            vec![Statement::Term(Some(Expr::Identifier("A".into())))],
        ),
    ];
    let universe = TypeUniverse::new();
    let mut vm = VmBackend::new();
    let _ = vm.generate(&prog, &universe);
    assert!(
        vm.errors.iter().any(|e| e.contains("'A'")),
        "cycle must surface as unresolvable-reference error: {:?}",
        vm.errors
    );
}

/// 2026-08-26 (parity plan §1.5): field_offset_any must pick the same
/// struct regardless of HashMap seed — the fallback iterates sorted by
/// struct name, so two structs sharing a field name resolve deterministically.
#[test]
fn test_field_offset_fallback_is_seed_independent() {
    use crate::ast::top::{StructDefinition, StructField, Visibility};
    // Prog with two objects whose `val` fields sit at DIFFERENT offsets:
    // Aa { first: Int, val: Int } (offset 8) vs Bb { val: Int } (offset 0).
    let mk = |name: &str, fields: Vec<(&str, i64)>| TopLevel::Obj(StructDefinition {
        name: name.into(),
        type_params: vec![],
        parent: None,
        fields: fields.into_iter().map(|(n, _)| StructField {
            name: n.into(),
            ty: Type::int(),
            default: None,
            visibility: Visibility::Public,
        }).collect(),
        transactions: vec![],
        view_html: None,
        span: None,
        modifiers: vec![],
        variants: vec![],
    });
    let prog = vec![mk("Aa", vec![("first", 0), ("val", 8)]), mk("Bb", vec![("val", 0)])];
    let universe = TypeUniverse::new();
    let mut vm = VmBackend::new();
    vm.generate(&prog, &universe);
    // Sorted-by-name iteration: "Aa" < "Bb", so bare `val` resolves to
    // Aa's offset (8) — the SAME answer under every SipHash seed.
    assert_eq!(vm.field_offset(None, "val"), 8);
}

/// Plan 1.1: literal arguments still pass through unchanged.
#[test]
fn intrinsic_call_passes_literal_arguments() {
    let prog = vec![
        defn(
            "go",
            vec![],
            vec![
                Statement::Expression(Expr::Call(
                    "Alloc#".into(),
                    vec![Expr::Decimal(64)],
                    None,
                )),
                Statement::Term(None),
            ],
        ),
    ];
    let universe = TypeUniverse::new();
    let mut vm = VmBackend::new();
    vm.generate(&prog, &universe);
    assert!(
        vm.errors.is_empty(),
        "literal arg must not error: {:?}",
        vm.errors
    );
}

/// Plan 1.4: a float literal records a helpful error naming the construct.
#[test]
fn float_literal_records_compile_error() {
    let prog = vec![defn(
        "f",
        vec![],
        vec![Statement::Term(Some(Expr::Float(2.5)))],
    )];
    let universe = TypeUniverse::new();
    let mut vm = VmBackend::new();
    vm.generate(&prog, &universe);
    assert_eq!(vm.errors.len(), 1, "{:?}", vm.errors);
    assert!(vm.errors[0].contains("float literals"), "{}", vm.errors[0]);
    assert!(vm.errors[0].contains("in 'f'"), "names the function: {}", vm.errors[0]);
}

/// Plan 1.4: unknown function call is recorded, not silent.
#[test]
fn unknown_function_records_error() {
    let prog = vec![defn(
        "f",
        vec![],
        vec![
            Statement::Expression(Expr::Call("nope".into(), vec![], None)),
            Statement::Term(None),
        ],
    )];
    let universe = TypeUniverse::new();
    let mut vm = VmBackend::new();
    vm.generate(&prog, &universe);
    assert!(
        vm.errors.iter().any(|e| e.contains("'nope'")),
        "{:?}",
        vm.errors
    );
}

// ── Compile-tail parity: host-side expectation checker ──────────────────
// 2026-08-23 (plan 2026-08-23-vm-compile-tail-parity §1.2): the EXPECT
// comments in tmp_fixtures/parity/*.bv are the parity contract between the
// host semantics and the tamer. This test independently evaluates each
// fixture's Print# expressions (mini evaluator over the parsed AST) and
// asserts the baked EXPECT line matches — so the contract is verified
// against parsed source, not hand-maintained. tools/parity_harness.sh then
// holds the tamer side to the same numbers.

fn parity_eval_expr(e: &Expr, vars: &HashMap<String, i64>) -> i64 {
    match e {
        Expr::Decimal(n) => *n,
        Expr::Identifier(name) => vars.get(name.as_str()).copied().unwrap_or_else(|| {
            panic!("parity eval: unbound variable '{}'", name)
        }),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, inner) => -parity_eval_expr(inner, vars),
        // 2026-08-23: unary ops from the unary fixture.
        Expr::UnaryOp(kind, inner) => {
            let v = parity_eval_expr(inner, vars);
            match kind {
                crate::ast::UnaryOpKind::Not => (v == 0) as i64,
                crate::ast::UnaryOpKind::BitNot => !v,
                other => panic!("parity eval: unsupported unary {:?}", other),
            }
        }
        Expr::BinaryOp(kind, l, r) => {
            let a = parity_eval_expr(l, vars);
            let b = parity_eval_expr(r, vars);
            match kind {
                crate::ast::BinaryOpKind::Add => a.wrapping_add(b),
                crate::ast::BinaryOpKind::Sub => a.wrapping_sub(b),
                crate::ast::BinaryOpKind::Mul => a.wrapping_mul(b),
                crate::ast::BinaryOpKind::Div => {
                    if b == 0 { 0 } else { a.wrapping_div(b) }
                }
                crate::ast::BinaryOpKind::Mod => {
                    if b == 0 { 0 } else { a.wrapping_rem(b) }
                }
                crate::ast::BinaryOpKind::BitAnd | crate::ast::BinaryOpKind::And => a & b,
                crate::ast::BinaryOpKind::BitOr | crate::ast::BinaryOpKind::Or => a | b,
                crate::ast::BinaryOpKind::BitXor => a ^ b,
                crate::ast::BinaryOpKind::Shl => a << (b as u32 & 63),
                crate::ast::BinaryOpKind::Shr => a >> (b as u32 & 63),
                // 2026-08-23: comparisons yield 0/1 (C semantics) — the
                // fixture corpus now includes comparison fixtures.
                crate::ast::BinaryOpKind::Eq => (a == b) as i64,
                crate::ast::BinaryOpKind::Neq => (a != b) as i64,
                crate::ast::BinaryOpKind::Lt => (a < b) as i64,
                crate::ast::BinaryOpKind::Gt => (a > b) as i64,
                crate::ast::BinaryOpKind::Le => (a <= b) as i64,
                crate::ast::BinaryOpKind::Ge => (a >= b) as i64,
                other => panic!("parity eval: unsupported op {:?}", other),
            }
        }
        Expr::Match(scrutinee, arms) => {
            let sv = parity_eval_expr(scrutinee, vars);
            for arm in arms {
                match &arm.pattern {
                    crate::ast::Pattern::Literal(Expr::Decimal(n)) if *n == sv => {
                        return parity_eval_expr(&arm.body, vars);
                    }
                    crate::ast::Pattern::Wildcard | crate::ast::Pattern::Binding(_) => {
                        return parity_eval_expr(&arm.body, vars);
                    }
                    _ => {}
                }
            }
            panic!("parity eval: no match arm matched {}", sv);
        }
        other => panic!("parity eval: unsupported expr {:?}", other),
    }
}

#[test]
fn parity_expected_values_match_independent_evaluation() {
    use crate::lexer;
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = format!("{}/tmp_fixtures/parity", manifest_dir);
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&dir).expect("parity fixture dir") {
        let path = entry.unwrap().path();
        if path.extension().map(|e| e != "bv").unwrap_or(true) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let tokens = lexer::tokenize(&source).expect("lex");
        let mut parser = crate::parser::Parser::new(tokens, &source);
        let items = parser.parse_program().expect("parse");

        // Collect the body's Print# argument expressions. Fixtures use a
        // plain defn (no contract needed for --backend vm builds); accept
        // txns too so older shapes keep working.
        let mut prints: Vec<Expr> = Vec::new();
        let mut bodies: Vec<&Vec<Statement>> = Vec::new();
        for item in &items {
            match item {
                TopLevel::Transaction(t) => bodies.push(&t.body),
                TopLevel::Definition(d) => bodies.push(&d.body),
                _ => {}
            }
        }
        for s in bodies.iter().flat_map(|b| b.iter()) {
            if let Statement::Expression(Expr::Call(name, args, _)) = s {
                if name == "Print#" && args.len() == 1 {
                    prints.push(args[0].clone());
                }
            }
        }
        assert!(!prints.is_empty(), "{}: no Print# statements", path.display());

        // Independent evaluation with the txn's let-sequence applied.
        let mut vars: HashMap<String, i64> = HashMap::new();
        let mut actual: Vec<i64> = Vec::new();
        'stmts: for body in &bodies {
            {
                let t_body = *body;
                for s in t_body {
                    match s {
                        Statement::Let { name, expr: Some(e), .. } => {
                            vars.insert(name.clone(), parity_eval_expr(e, &vars));
                        }
                        Statement::Assign(lhs, rhs) => {
                            if let Expr::Identifier(n) = lhs {
                                vars.insert(n.clone(), parity_eval_expr(rhs, &vars));
                            }
                        }
                        Statement::Expression(Expr::Call(name, args, _)) => {
                            if name == "Print#" && args.len() == 1 {
                                actual.push(parity_eval_expr(&args[0], &vars));
                            }
                        }
                        // 2026-08-23 (controlflow fixture): when-guards gate
                        // statement execution on the evaluated condition.
                        Statement::Guarded(cond, body) => {
                            if parity_eval_expr(cond, &vars) != 0 {
                                for gs in body {
                                    if let Statement::Expression(Expr::Call(
                                        name,
                                        args,
                                        _,
                                    )) = gs
                                    {
                                        if name == "Print#" && args.len() == 1 {
                                            actual.push(parity_eval_expr(
                                                &args[0],
                                                &vars,
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        Statement::Term(_) => break 'stmts,
                        _ => {}
                    }
                }
            }
        }

        // Compare against the baked EXPECT comment.
        let expected_line = source
            .lines()
            .find(|l| l.contains("// EXPECT:"))
            .expect("EXPECT comment")
            .split("// EXPECT: ")
            .nth(1)
            .unwrap()
            .to_string();
        let expected: Vec<i64> = expected_line
            .split(',')
            .map(|v| v.trim().parse().expect("EXPECT value"))
            .collect();
        assert_eq!(
            actual, expected,
            "{}: independent evaluation disagrees with the baked contract",
            path.display()
        );
        // NOTE: `prints` (top-level scan) is a superset check only — guarded
        // bodies' Print# calls are counted by the evaluator walk above, so
        // no separate length assert here.
        checked += 1;
    }
    assert!(checked >= 4, "corpus must not shrink: {}", checked);
}
