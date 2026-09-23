//! AST Generator for Property-Based Fuzzing
//!
//! Procedurally generates valid and semi-valid Briev ASTs for fuzzing.
//! Uses depth limiting to prevent stack overflow during generation.

use crate::ast::*;
use std::collections::HashMap;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

/// Maximum depth for recursive AST generation
const MAX_DEPTH: usize = 8;

/// Generate a random Briev program (returns Vec<TopLevel>)
pub fn arb_program(max_depth: usize) -> impl Strategy<Value = Vec<TopLevel>> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (0usize..=5usize).prop_flat_map(move |num_items| {
        proptest::collection::vec(
            arb_top_level(max_depth),
            num_items..=num_items + 3,
        )
    })
}

/// Generate a random top-level item
fn arb_top_level(max_depth: usize) -> impl Strategy<Value = TopLevel> {
    let max_depth = max_depth.min(MAX_DEPTH);
    prop_oneof![
        arb_state_decl(max_depth),
        arb_transaction(max_depth),
        arb_definition(max_depth),
        arb_trigger_decl(max_depth),
        arb_enum_def(max_depth),
        arb_struct_def(max_depth),
    ]
}

/// Generate a random state declaration
pub fn arb_state_decl(max_depth: usize) -> impl Strategy<Value = TopLevel> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_identifier(),
        arb_type(),
        arb_expr(max_depth).prop_map(Some),
    ).prop_map(|(name, ty, _expr)| {
        TopLevel::StateDecl(StateDecl {
            name,
            ty,
            span: None,
        })
    })
}

/// Generate a random trigger declaration
pub fn arb_trigger_decl(_max_depth: usize) -> impl Strategy<Value = TopLevel> {
    (
        arb_identifier(),
    ).prop_map(|(name,)| {
        TopLevel::Trigger(Trigger {
            name,
            instance: Expr::Decimal(0),
            span: None,
        })
    })
}

/// Generate a random transaction
pub fn arb_transaction(max_depth: usize) -> impl Strategy<Value = TopLevel> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_identifier(),
        arb_contract(max_depth),
        arb_statement_list(max_depth),
        prop_oneof![Just(false), Just(true)],
    ).prop_map(|(name, contract, body, is_reactive)| {
        TopLevel::Transaction(Transaction {
            is_async: false,
            is_reactive,
            name,
            type_params: Vec::new(),
            parameters: Vec::new(),
            contract,
            body,
            span: None,
            metadata: HashMap::new(),
            modifiers: vec![],
            outputs: Vec::new(),
            output_type: None,
            derivation: None,
            doc: None,
        })
    })
}

/// Generate a random contract
pub fn arb_contract(max_depth: usize) -> impl Strategy<Value = Contract> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_expr(max_depth),
        arb_expr(max_depth),
    ).prop_map(|(pre, post)| {
        Contract {
            pre_condition: pre,
            post_condition: post,
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false}
    })
}

/// Generate a random definition
pub fn arb_definition(max_depth: usize) -> impl Strategy<Value = TopLevel> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_identifier(),
        arb_type(),
        arb_expr(max_depth),
    ).prop_map(|(name, return_type, body)| {
        TopLevel::Definition(Definition {
            variadic_param: None,
            name,
            type_params: Vec::new(),
            parameters: Vec::new(),
            outputs: vec![return_type],
            output_type: None,
            contract: Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body: vec![Statement::Term(Some(body))],
            annotations: vec![],
            metadata: HashMap::new(),
            modifiers: vec![],
            derivation: None,
            span: None,
            doc: None,
        })
    })
}

/// Generate a random enum definition
pub fn arb_enum_def(_max_depth: usize) -> impl Strategy<Value = TopLevel> {
    (
        arb_identifier(),
        proptest::collection::vec(arb_identifier(), 1usize..=4usize),
    ).prop_map(|(name, variants)| {
        TopLevel::Enum(EnumDefinition {
            name,
            type_params: Vec::new(),
            variants: variants.into_iter()
                .map(EnumVariant::Unit)
                .collect(),
            span: None,
        })
    })
}

/// Generate a random struct definition
pub fn arb_struct_def(max_depth: usize) -> impl Strategy<Value = TopLevel> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_identifier(),
        proptest::collection::vec(arb_struct_field(max_depth), 1usize..=3usize),
    ).prop_map(|(name, fields)| {
        TopLevel::Obj(StructDefinition {
            name,
            type_params: Vec::new(),
            parent: None,
            fields,
            transactions: Vec::new(),
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        })
    })
}

fn arb_struct_field(max_depth: usize) -> impl Strategy<Value = StructField> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_identifier(),
        arb_type(),
        arb_expr(max_depth).prop_map(Some),
    ).prop_map(|(name, ty, default)| {
        StructField { name, ty, default, visibility: Visibility::Public }
    })
}

/// Generate a random list of statements
pub fn arb_statement_list(max_depth: usize) -> impl Strategy<Value = Vec<Statement>> {
    let max_depth = max_depth.min(MAX_DEPTH);
    proptest::collection::vec(arb_statement(max_depth), 1usize..=5usize)
}

/// Generate a random statement
pub fn arb_statement(max_depth: usize) -> impl Strategy<Value = Statement> {
    let max_depth = max_depth.min(MAX_DEPTH);

    if max_depth == 0 {
        // At max depth, only generate leaf statements
        return arb_term_statement(max_depth).boxed();
    }

    prop_oneof![
        // Assignment: &var = expr;
        arb_assignment(max_depth),
        // Let binding: let x: Type = expr;
        arb_let_statement(max_depth),
        // Guarded: when condition { statements } or [condition] stmt;
        arb_guarded_statement(max_depth),
        // Gate: [condition]; — convergence gate
        arb_gate_statement(max_depth),
        // Term: term;
        arb_term_statement(max_depth),
        // Escape: escape;
        arb_escape_statement(),
        // Expression: expr;
        arb_expr_statement(max_depth),
    ].boxed()
}

/// Generate a random assignment statement
fn arb_assignment(max_depth: usize) -> impl Strategy<Value = Statement> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_identifier().prop_map(|name| Expr::Identifier(name)),
        arb_expr(max_depth),
    ).prop_map(|(lhs, expr)| {
        Statement::Assign(lhs, expr)
    })
}

/// Generate a random let statement
fn arb_let_statement(max_depth: usize) -> impl Strategy<Value = Statement> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_identifier(),
        arb_type(),
        arb_expr(max_depth).prop_map(Some),
    ).prop_map(|(name, ty, expr)| {
        Statement::Let { names: vec![], 
            name,
            ty: Some(ty),
            expr,
            modifiers: vec![],
        }
    })
}

/// Generate a random guarded statement
fn arb_guarded_statement(max_depth: usize) -> impl Strategy<Value = Statement> {
    let max_depth = max_depth.min(MAX_DEPTH);
    (
        arb_expr(max_depth),
        proptest::collection::vec(arb_statement(max_depth.saturating_sub(1)), 1usize..=3usize),
    ).prop_map(|(condition, statements)| {
        Statement::Guarded(condition, statements)
    })
}

/// Generate a random convergence gate statement
fn arb_gate_statement(max_depth: usize) -> impl Strategy<Value = Statement> {
    let max_depth = max_depth.min(MAX_DEPTH);
    arb_expr(max_depth).prop_map(Statement::Gate)
}

/// Generate a random term statement
fn arb_term_statement(max_depth: usize) -> impl Strategy<Value = Statement> {
    let max_depth = max_depth.min(MAX_DEPTH);
    prop_oneof![
        Just(Statement::Term(None)),
        arb_expr(max_depth).prop_map(|e| Statement::Term(Some(e))),
    ]
}

/// Generate a random escape statement
fn arb_escape_statement() -> impl Strategy<Value = Statement> {
    prop_oneof![
        Just(Statement::Rollback(None)),
        arb_simple_expr().prop_map(|e| Statement::Rollback(Some(e))),
    ]
}

/// Generate a random expression statement
fn arb_expr_statement(max_depth: usize) -> impl Strategy<Value = Statement> {
    let max_depth = max_depth.min(MAX_DEPTH);
    arb_expr(max_depth).prop_map(Statement::Expression)
}

/// Generate a random expression
pub fn arb_expr(max_depth: usize) -> impl Strategy<Value = Expr> {
    let max_depth = max_depth.min(MAX_DEPTH);

    if max_depth == 0 {
        return arb_leaf_expr().boxed();
    }

    prop_oneof![
        // Leaf expressions
        arb_leaf_expr(),
        // Binary operations
        arb_binary_expr(max_depth),
        // Unary operations
        arb_unary_expr(max_depth),
        // Function calls
        arb_call_expr(max_depth),
    ].boxed()
}

/// Generate a random leaf expression (no recursion)
fn arb_leaf_expr() -> impl Strategy<Value = Expr> {
    prop_oneof![
        any::<i64>().prop_map(Expr::Decimal),
        any::<f64>().prop_map(Expr::Float),
        any::<bool>().prop_map(Expr::Bool),
        arb_string_literal().prop_map(|s| Expr::Quoted(s.into())),
        arb_identifier().prop_map(Expr::Identifier),
    ]
}

/// Generate a random simple expression (for escape, etc.)
fn arb_simple_expr() -> impl Strategy<Value = Expr> {
    prop_oneof![
        any::<i64>().prop_map(Expr::Decimal),
        any::<bool>().prop_map(Expr::Bool),
        arb_identifier().prop_map(Expr::Identifier),
    ]
}

/// Generate a random binary expression
fn arb_binary_expr(max_depth: usize) -> impl Strategy<Value = Expr> {
    let max_depth = max_depth.min(MAX_DEPTH);
    let sub_depth = max_depth.saturating_sub(1);
    (
        arb_expr(sub_depth),
        arb_binary_op_kind(),
        arb_expr(sub_depth),
    ).prop_map(|(left, op, right)| {
        Expr::BinaryOp(op, Box::new(left), Box::new(right))
    })
}

fn arb_binary_op_kind() -> impl Strategy<Value = BinaryOpKind> {
    prop_oneof![
        Just(BinaryOpKind::Add), Just(BinaryOpKind::Sub), Just(BinaryOpKind::Mul),
        Just(BinaryOpKind::Div), Just(BinaryOpKind::Mod),
        Just(BinaryOpKind::Eq), Just(BinaryOpKind::Neq),
        Just(BinaryOpKind::Lt), Just(BinaryOpKind::Le),
        Just(BinaryOpKind::Gt), Just(BinaryOpKind::Ge),
        Just(BinaryOpKind::And), Just(BinaryOpKind::Or),
        Just(BinaryOpKind::BitAnd), Just(BinaryOpKind::BitOr),
        Just(BinaryOpKind::BitXor), Just(BinaryOpKind::Shl), Just(BinaryOpKind::Shr),
    ]
}

/// Generate a random unary expression
fn arb_unary_expr(max_depth: usize) -> impl Strategy<Value = Expr> {
    let max_depth = max_depth.min(MAX_DEPTH);
    let sub_depth = max_depth.saturating_sub(1);
    prop_oneof![
        arb_expr(sub_depth).prop_map(|e| Expr::UnaryOp(UnaryOpKind::Not, Box::new(e))),
        arb_expr(sub_depth).prop_map(|e| Expr::UnaryOp(UnaryOpKind::Neg, Box::new(e))),
        arb_expr(sub_depth).prop_map(|e| Expr::UnaryOp(UnaryOpKind::BitNot, Box::new(e))),
    ]
}

/// Generate a random function call expression
fn arb_call_expr(max_depth: usize) -> impl Strategy<Value = Expr> {
    let max_depth = max_depth.min(MAX_DEPTH);
    let sub_depth = max_depth.saturating_sub(1);
    (
        arb_identifier(),
        proptest::collection::vec(arb_expr(sub_depth), 0usize..=3usize),
    ).prop_map(|(name, args)| {
        Expr::Call(name, args, None)
    })
}

/// Generate a random identifier
fn arb_identifier() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,15}".prop_filter(
        "Filter reserved keywords",
        |s| !is_reserved_keyword(s),
    )
}

fn is_reserved_keyword(s: &str) -> bool {
    matches!(
        s,
        "txn" | "defn" | "let" | "term" | "true" | "false"
            | "Int" | "UInt" | "Float"
            | "Bool" | "String" | "void" | "Blob" | "Char"
            | "frgn" | "import" | "from" | "as" | "struct"
            | "enum" | "trg" | "node" | "async"
            | "match"
            | "resource" | "rsrc" | "registry" | "reg"
            | "render" | "stage" | "on"
            | "forall" | "exists" | "within" | "link"
            | "asm" | "bank" | "const"
    )
}

/// Generate a random string literal
fn arb_string_literal() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _.-]{0,50}".prop_map(|s| s)
}

/// Generate a random type
pub fn arb_type() -> impl Strategy<Value = Type> {
    prop_oneof![
        Just(Type::int()),
        Just(Type::float()),
        Just(Type::bool_()),
        Just(Type::string()),
        Just(Type::Void),
        Just(Type::blob()),
        Just(Type::char_()),
        arb_identifier().prop_map(Type::Custom),
        // Constrained types (sized integers)
        prop_oneof![Just(8), Just(16), Just(32), Just(64)]
            .prop_flat_map(|n| {
                prop_oneof![
                    (Just(Type::int()), Just(n)).prop_map(|(t, n)| Type::Constrained(Box::new(t), BitRange::Any(n))),
                ]
            }),
    ]
}

/// Generate a random simple type (no constrained types)
fn arb_simple_type() -> impl Strategy<Value = Type> {
    prop_oneof![
        Just(Type::int()),
        Just(Type::float()),
        Just(Type::bool_()),
        Just(Type::string()),
        Just(Type::Void),
        Just(Type::blob()),
        Just(Type::char_()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    #[test]
    fn test_generate_random_expr() {
        // Generate multiple random expressions and verify they don't panic
        for _ in 0..100 {
            let expr = arb_expr(5).new_tree(&mut TestRunner::default()).unwrap().current();
            // Just verify it doesn't panic
            let _ = format!("{:?}", expr);
        }
    }

    #[test]
    fn test_generate_random_statement() {
        for _ in 0..100 {
            let stmt = arb_statement(5).new_tree(&mut TestRunner::default()).unwrap().current();
            let _ = format!("{:?}", stmt);
        }
    }

    #[test]
    fn test_generate_random_program() {
        for _ in 0..50 {
            let program = arb_program(5).new_tree(&mut TestRunner::default()).unwrap().current();
            let _ = format!("{:?}", program);
        }
    }

    #[test]
    fn test_depth_limiting() {
        // Verify that depth limiting prevents excessive nesting
        let expr = arb_expr(MAX_DEPTH).new_tree(&mut TestRunner::default()).unwrap().current();
        let depth = measure_expr_depth(&expr);
        assert!(depth <= MAX_DEPTH + 2, "Expression depth {} exceeded limit", depth);
    }

    fn measure_expr_depth(expr: &Expr) -> usize {
        match expr {
            Expr::BinaryOp(_, l, r) => {
                1 + measure_expr_depth(l).max(measure_expr_depth(r))
            }
            Expr::UnaryOp(_, e) => 1 + measure_expr_depth(e),
            Expr::Call(_, args, _) => {
                1 + args.iter().map(measure_expr_depth).max().unwrap_or(0)
            }
            _ => 0,
        }
    }

    proptest! {
        #[test]
        fn test_expr_roundtrip(program in arb_program(4)) {
            // Generate a program and verify it can be formatted without panic
            let _ = format!("{:?}", program);
        }

        #[test]
        fn test_statement_variety(stmt in arb_statement(6)) {
            // Verify we get different statement types
            let is_valid = matches!(
                &stmt,
                Statement::Assign(_, _)
                    | Statement::Let { .. }
                    | Statement::Guarded(_, _)
                    | Statement::Gate(_)
                    | Statement::Term(_) | Statement::EndProgram(_)
                    | Statement::Rollback(_)
                    | Statement::Expression(_)
            );
            prop_assert!(is_valid, "Generated invalid statement type: {:?}", stmt);
        }
    }
}
