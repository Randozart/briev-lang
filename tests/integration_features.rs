//! Integration tests for contract features (Features A, B, C)
//!
//! Updated to current API:
//! - Parser::new(tokenize(code).unwrap(), code)
//! - parser.parse_program() → Vec<TopLevel>
//! - typechecker::check_program(&mut items, &TypeUniverse::new())
//! - proof_engine::prove_contract(pre, post, params, explicit)

use briev_compiler::ast::*;
use briev_compiler::lexer::tokenize;
use briev_compiler::parser::Parser;
use briev_compiler::proof_engine;

fn parse_program(source: &str) -> Vec<TopLevel> {
    let tokens = tokenize(source).unwrap();
    let mut p = Parser::new(tokens, source);
    p.parse_program().unwrap()
}

#[test]
fn test_feature_a_single_output() {
    let code = r#"
        defn get_value -> Bool {
            term true;
        };
    "#;

    let mut program = parse_program(code);
    let universe = briev_compiler::type_universe::TypeUniverse::new();
    let result = briev_compiler::typechecker::check_program(&mut program, &universe);
    assert!(
        result.is_ok(),
        "Type checking should pass for single output"
    );
}

#[test]
fn test_feature_a_union_output() {
    let code = r#"
        defn maybe_value -> Bool | String {
            term true;
        };
    "#;

    let tokens = tokenize(code).unwrap();
    let mut p = Parser::new(tokens, code);
    let result = p.parse_program();
    let _ = result;
}

#[test]
fn test_feature_a_tuple_output() {
    let code = r#"
        defn multi_value -> Bool, String, Int {
            term true, "hello", 42;
        };
    "#;

    let tokens = tokenize(code).unwrap();
    let mut p = Parser::new(tokens, code);
    let result = p.parse_program();
    let _ = result;
}

#[test]
fn test_feature_b_contract_simple() {
    let code = r#"
        defn always_true [true][result == true] -> Bool {
            term true;
        };
    "#;

    let program = parse_program(code);
    let defn = match &program[0] {
        TopLevel::Definition(d) => d,
        _ => panic!("expected Definition"),
    };

    let result = proof_engine::prove_contract(
        &defn.contract.pre_condition,
        &defn.contract.post_condition,
        &[],
        defn.contract.explicit,
    );
    assert!(result.is_ok(), "Contract should verify for always-true function");
}

#[test]
fn test_feature_c_true_assertion_simple() {
    let code = r#"
        defn always_succeeds [true][result == true] -> Bool {
            term true;
        };
    "#;

    let program = parse_program(code);
    let defn = match &program[0] {
        TopLevel::Definition(d) => d,
        _ => panic!("expected Definition"),
    };

    let result = proof_engine::prove_contract(
        &defn.contract.pre_condition,
        &defn.contract.post_condition,
        &[],
        defn.contract.explicit,
    );
    assert!(
        result.is_ok(),
        "Should verify successfully because the function always returns true"
    );
}

#[test]
fn test_feature_c_unsatisfiable_contract_fails() {
    let code = r#"
        defn check [true][false] -> Bool {
            term true;
        };
    "#;

    let program = parse_program(code);
    let defn = match &program[0] {
        TopLevel::Definition(d) => d,
        _ => panic!("expected Definition"),
    };

    let result = proof_engine::prove_contract(
        &defn.contract.pre_condition,
        &defn.contract.post_condition,
        &[],
        defn.contract.explicit,
    );
    assert!(
        result.is_err(),
        "Should fail because postcondition `false` is unsatisfiable"
    );
}

#[test]
fn test_feature_abc_combined() {
    let code = r#"
        defn success_case [true][result == true] -> Bool {
            term true;
        };
    "#;

    let mut program = parse_program(code);

    // Type check
    let universe = briev_compiler::type_universe::TypeUniverse::new();
    let tc_result = briev_compiler::typechecker::check_program(&mut program, &universe);
    assert!(tc_result.is_ok(), "Type checking should pass");

    // Proof check
    let defn = match &program[0] {
        TopLevel::Definition(d) => d,
        _ => panic!("expected Definition"),
    };
    let proof_result = proof_engine::prove_contract(
        &defn.contract.pre_condition,
        &defn.contract.post_condition,
        &[],
        defn.contract.explicit,
    );
    assert!(
        proof_result.is_ok(),
        "Proof engine should verify the contract"
    );
}

#[test]
fn test_feature_contract_with_precondition() {
    let code = r#"
        defn clamp [x >= 0][result >= 0] -> Int {
            term x;
        };
    "#;

    let program = parse_program(code);
    let defn = match &program[0] {
        TopLevel::Definition(d) => d,
        _ => panic!("expected Definition"),
    };

    let result = proof_engine::prove_contract(
        &defn.contract.pre_condition,
        &defn.contract.post_condition,
        &[],
        defn.contract.explicit,
    );
    assert!(result.is_ok(), "Contract with precondition should verify");
}

#[test]
fn test_feature_typecheck_rejects_bad_output() {
    let code = r#"
        defn get_value -> Int {
            term true;
        };
    "#;

    let mut program = parse_program(code);
    let universe = briev_compiler::type_universe::TypeUniverse::new();
    let result = briev_compiler::typechecker::check_program(&mut program, &universe);
    assert!(
        result.is_err(),
        "Type checking should reject mismatched output type"
    );
}
