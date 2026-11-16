use briev_compiler::ast::*;
use briev_compiler::interpreter::{Atom, Interpreter, Value};
use briev_compiler::lexer::tokenize;
use briev_compiler::parser::Parser;

fn parse_and_init(source: &str) -> (Interpreter, Vec<TopLevel>) {
    let tokens = tokenize(source).unwrap();
    let mut parser = Parser::new(tokens, source);
    let program = parser.parse_program().expect("Parse should succeed");
    let mut interp = Interpreter::new();

    for item in &program {
        if let TopLevel::Constant(c) = item {
            let val = interp.eval_expr(&c.expr).expect("Constant eval");
            interp.state.insert(c.name.clone(), val);
        }
    }

    (interp, program)
}

/// Truthiness across the interpreter's current value representations.
/// 2026-08-03: Comparisons return Value::Bool and bitwise ops return
/// Value::Bits since the Bits thesis; both must read as truthy/nonzero.
fn is_truthy(v: &Value) -> bool {
    v.is_true()
}

#[test]
fn test_ptr_arithmetic_e2e() {
    let source = r#"
        const UART_DR: Ptr<Int> = 0x40011000 as Ptr<Int>;
        const UART_FR: Ptr<Int> = 0x40011004 as Ptr<Int>;
    "#;

    let (mut interp, _) = parse_and_init(source);

    let dr = interp.state.get("UART_DR").unwrap().clone();
    let fr = interp.state.get("UART_FR").unwrap().clone();
    assert_eq!(dr, Value::Atom(Atom::Int(0x40011000)));
    assert_eq!(fr, Value::Atom(Atom::Int(0x40011004)));

    let computed = interp
        .eval_expr(&Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("UART_DR".into())),
            Box::new(Expr::Decimal(4)),
        ))
        .expect("Ptr add");
    assert_eq!(computed, Value::Atom(Atom::Int(0x40011004)), "DR + 4 == FR");

    let eq_result = interp
        .eval_expr(&Expr::BinaryOp(
            BinaryOpKind::Eq,
            Box::new(Expr::Identifier("UART_DR".into())),
            Box::new(Expr::Identifier("UART_FR".into())),
        ))
        .expect("Ptr == Ptr");
    assert!(!is_truthy(&eq_result), "DR != FR");

    let addr = interp
        .eval_expr(&Expr::Cast(
            Box::new(Expr::Identifier("UART_DR".into())),
            Type::Custom("Int".to_string()),
        ))
        .expect("Ptr to Int cast");
    assert_eq!(addr, Value::Atom(Atom::Int(0x40011000)));
}

#[test]
fn test_ptr_type_punning_e2e() {
    let ptr_int_ty = Type::Applied("Ptr".into(), vec![Type::Custom("Int".to_string())]);
    let ptr_char_ty = Type::Applied("Ptr".into(), vec![Type::Custom("Char".to_string())]);

    let mut interp = Interpreter::new();

    let ptr_int = interp
        .eval_expr(&Expr::Cast(Box::new(Expr::Decimal(0x100)), ptr_int_ty))
        .expect("Ptr<Int>");
    assert_eq!(ptr_int, Value::Atom(Atom::Int(0x100)));

    let ptr_char = interp
        .eval_expr(&Expr::Cast(Box::new(Expr::Decimal(0x100)), ptr_char_ty))
        .expect("Ptr<Char>");
    assert_eq!(ptr_char, Value::Atom(Atom::Int(0x100)));

    let advanced = interp
        .eval_expr(&Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Cast(
                Box::new(Expr::Decimal(0x100)),
                Type::Applied("Ptr".into(), vec![Type::Custom("Char".to_string())]),
            )),
            Box::new(Expr::Decimal(4)),
        ))
        .expect("Ptr<Char> + 4");
    assert_eq!(advanced, Value::Atom(Atom::Int(0x104)));
}

#[test]
fn test_ptr_contract_bounds_e2e() {
    let mut interp = Interpreter::new();
    let ptr_addr = 0x40011004i64;

    let cast_addr = Expr::Cast(
        Box::new(Expr::Decimal(ptr_addr)),
        Type::Custom("Int".to_string()),
    );
    let addr_int = interp.eval_expr(&cast_addr).expect("Cast Int to Int");
    assert_eq!(addr_int, Value::Atom(Atom::Int(ptr_addr)));

    let check_ge = interp
        .eval_expr(&Expr::BinaryOp(
            BinaryOpKind::Ge,
            Box::new(cast_addr),
            Box::new(Expr::Decimal(0x40011000)),
        ))
        .expect("Contract GE");
    assert!(is_truthy(&check_ge), "ptr addr >= base");

    let cast_addr2 = Expr::Cast(
        Box::new(Expr::Decimal(ptr_addr)),
        Type::Custom("Int".to_string()),
    );
    let check_lt = interp
        .eval_expr(&Expr::BinaryOp(
            BinaryOpKind::Lt,
            Box::new(cast_addr2),
            Box::new(Expr::Decimal(0x40011020)),
        ))
        .expect("Contract LT");
    assert!(is_truthy(&check_lt), "ptr addr < end");
}

#[test]
fn test_ptr_bitwise_arithmetic_e2e() {
    let mut interp = Interpreter::new();
    let ptr_ty = Type::Applied("Ptr".into(), vec![Type::Custom("Int".to_string())]);

    let ptr = Expr::Cast(Box::new(Expr::Decimal(0x40011007)), ptr_ty.clone());

    let aligned = interp
        .eval_expr(&Expr::BinaryOp(
            BinaryOpKind::BitAnd,
            Box::new(ptr.clone()),
            Box::new(Expr::Decimal(!7i64)),
        ))
        .expect("Ptr align");
    assert_eq!(aligned.as_i64(), Some(0x40011000), "Ptr & !7 aligns down");

    let toggled = interp
        .eval_expr(&Expr::BinaryOp(
            BinaryOpKind::BitXor,
            Box::new(ptr),
            Box::new(Expr::Decimal(0xFFF)),
        ))
        .expect("Ptr XOR");
    assert_eq!(
        toggled.as_i64(),
        Some(0x40011FF8),
        "Ptr ^ 0xFFF toggles low bits"
    );
}
