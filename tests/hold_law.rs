// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! `hold` law surface (2026-11-09): static obligations use `hold`; `when`
//! stays the runtime guard. The optional `through <Type>` strategy clause
//! belongs to a mechanism fact body and is spelled `through` (`thru` is
//! retired with a fix).

use briev_compiler::ast::{format_program, TopLevel};
use briev_compiler::lexer::{tokenize, Token};

fn parse(src: &str) -> Vec<TopLevel> {
    let tokens = tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse")
}

fn parse_err(src: &str) -> String {
    let tokens = tokenize(src).expect("tokenize");
    let mut parser = briev_compiler::parser::Parser::new(tokens, src);
    parser
        .parse_program()
        .expect_err("expected a parse error")
        .to_string()
}

fn top_level_laws(items: &[TopLevel]) -> usize {
    items
        .iter()
        .filter(|i| matches!(i, TopLevel::HoldLaw(_)))
        .count()
}

#[test]
fn lexer_recognizes_hold() {
    let tokens = |src: &str| -> Vec<Token> {
        tokenize(src).unwrap().into_iter().map(|(t, _)| t).collect()
    };
    assert_eq!(tokens("hold"), vec![Token::Hold]);
    assert_eq!(tokens("when"), vec![Token::When]);
}

#[test]
fn top_level_hold_declares_a_law() {
    let items = parse("let usb: Bool = true;\nhold usb { usb = true; }\n");
    assert_eq!(top_level_laws(&items), 1);
}

#[test]
fn obj_body_bare_hold_declares_a_law() {
    let items = parse("obj Sensor { temperature: Int; hold temperature > 100 { alarm = true; } };\n");
    // obj bodies are TypeDef bodies (parse_obj_like), so the law lands in
    // `body.hold_laws` like any component clause.
    let TopLevel::TypeDef(o) = &items[0] else {
        panic!("obj expected");
    };
    assert_eq!(o.body.hold_laws.len(), 1);
}

#[test]
fn type_body_bare_hold_declares_a_law() {
    let items = parse(
        "type Regulator { pin vin: Power; pin vout: Power; pin gnd: Ground; reference \"R\";\n\
         hold vin.voltage >= 5V { vout.voltage = 3.3V; } };\n",
    );
    let TopLevel::TypeDef(t) = &items[0] else {
        panic!("type expected");
    };
    assert_eq!(t.body.hold_laws.len(), 1);
}

#[test]
fn lifecycle_arm_uses_hold() {
    let items = parse("let n: Int = 0;\nhold n == 1 { endprogram; };\n");
    let TopLevel::HoldLaw(w) = &items[1] else {
        panic!("law expected");
    };
    assert!(w.lifecycle.is_some());
}

#[test]
fn mechanism_body_uses_hold_and_through() {
    let items = parse(
        "let u1: Mcu = Mcu { value: \"x\" };\n\
         let d1: Led = Led { value: \"d\" };\n\
         node n [true][true] {\n\
             hold u1.gpio0.voltage == 3.3V { d1.a = u1.gpio0; } through Fet;\n\
         };\n",
    );
    let TopLevel::Transaction(t) = &items[2] else {
        panic!("node expected");
    };
    assert!(matches!(
        &t.body[0],
        briev_compiler::ast::Statement::Guarded(_, _)
    ));
    let canonical = format_program(&items[2..3]);
    assert!(canonical.contains("hold"), "{canonical}");
    assert!(canonical.contains("through Fet;"), "{canonical}");
}

#[test]
fn top_level_when_is_rejected_with_fix() {
    let err = parse_err("let x: Bool = true;\nwhen x { x = true; }\n");
    assert!(err.contains("hold"), "{err}");
    assert!(err.contains("runtime guard"), "{err}");
}

#[test]
fn obj_body_when_is_rejected_with_fix() {
    let err = parse_err("obj Sensor { temperature: Int; when temperature > 100 { alarm = true; } };\n");
    assert!(err.contains("hold"), "{err}");
}

#[test]
fn body_when_stays_a_runtime_guard() {
    parse("defn f(x: Int) -> Int { when x > 0 { term x; }; term 0; };\n");
}

#[test]
fn when_with_strategy_clause_is_rejected() {
    let err = parse_err(
        "node n [true][true] { when true { x = 1; } through Fet; };\n",
    );
    assert!(err.contains("hold"), "{err}");
}

#[test]
fn retired_thru_is_rejected_with_through_fix() {
    let err = parse_err(
        "node n [true][true] { hold true { x = 1; } thru Fet; };\n",
    );
    assert!(err.contains("through"), "{err}");
}

const BODY_HOLD: &str =
    "let x: Int = 0;\ndefn f() -> Int { hold x == 0 { term 1; }; term 0; };\n";

#[test]
fn body_hold_is_rejected_on_the_software_surface() {
    let mut items = parse(BODY_HOLD);
    let universe = briev_compiler::type_universe::TypeUniverse::new();
    let errors = briev_compiler::typechecker::check_program_with_target(
        &mut items,
        &universe,
        None,
        false,
    )
    .expect_err("hold in a reactive body must be rejected");
    assert!(
        errors.iter().any(|e| e.to_string().contains("static law")),
        "{errors:?}"
    );
}

#[test]
fn body_hold_is_accepted_on_the_electronics_surface() {
    let mut items = parse(BODY_HOLD);
    let universe = briev_compiler::type_universe::TypeUniverse::new();
    briev_compiler::typechecker::check_program_with_target(
        &mut items,
        &universe,
        None,
        true,
    )
    .expect("electronics body facts are legal");
}
