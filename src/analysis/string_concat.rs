// ── String Concat Resolution (`+` is concat for String/Blob) ─────────
// 2026-08-03: `+` reads naturally as string concatenation, so `"a" + "b"` and
// `a + b` on String/Blob operands mean the same thing as the `++`/Concat
// operator. The typechecker resolves the Concat binding for `+` on strings
// (operators.rs protocol_binding), and THIS pass rewrites the AST kind
// Add → Concat so the backend dispatches the concat emitter. The backend
// cannot see String operands at the binary-op site (they are boxed to i64
// registers), so the decision must be made on the typed AST before codegen.
//
// Undo: if `+` is ever allowed to be a distinct overloadable operator, remove
// this pass and let `+` resolve like any other overloaded rune.

use crate::ast::{Expr, Statement, TopLevel, Type};
use crate::type_universe::TypeUniverse;

/// Rewrite `BinaryOp(Add, …)` → `BinaryOp(Concat, …)` when an operand is a
/// String/Blob value. Runs after typechecking, before codegen.
pub fn rewrite_plus_concat(items: &mut [TopLevel], universe: &TypeUniverse) {
    for item in items {
        match item {
            TopLevel::Definition(d) => {
                let mut env = param_env(&d.parameters);
                rewrite_body(&mut d.body, &mut env, universe);
            }
            TopLevel::Transaction(t) => {
                let mut env = param_env(&t.parameters);
                rewrite_body(&mut t.body, &mut env, universe);
            }
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_mut() {
                    let mut env = param_env(&d.parameters);
                    rewrite_body(&mut d.body, &mut env, universe);
                }
            }
            _ => {}
        }
    }
}

fn param_env(params: &[(String, Type)]) -> std::collections::HashMap<String, Type> {
    params.iter().cloned().collect()
}

fn rewrite_body(
    body: &mut [Statement],
    env: &mut std::collections::HashMap<String, Type>,
    universe: &TypeUniverse,
) {
    for stmt in body {
        match stmt {
            Statement::Term(opt)
            | Statement::EndProgram(opt)
            | Statement::Rollback(opt) => {
                if let Some(expr) = opt.as_mut() {
                    rewrite_expr(expr, env, universe);
                }
            }
            Statement::Expression(expr) => rewrite_expr(expr, env, universe),
            Statement::Let { name, expr, ty, .. } => {
                if let Some(e) = expr.as_mut() {
                    rewrite_expr(e, env, universe);
                }
                // Track the let binding's declared type for later identifiers
                // (`let s: String = …; term s + "x"`).
                if let Some(t) = ty.as_ref() {
                    env.insert(name.clone(), t.clone());
                }
            }
            Statement::Assign(_, expr) => rewrite_expr(expr, env, universe),
            Statement::Guarded(_, body) => rewrite_body(body, env, universe),
            Statement::Foreach { body, .. } => rewrite_body(body, env, universe),
            Statement::Block(body) => rewrite_body(body, env, universe),
            _ => {}
        }
    }
}

fn rewrite_expr(
    expr: &mut Expr,
    env: &mut std::collections::HashMap<String, Type>,
    universe: &TypeUniverse,
) {
    match expr {
        Expr::BinaryOp(kind, l, r) => {
            rewrite_expr(l, env, universe);
            rewrite_expr(r, env, universe);
            if matches!(kind, crate::ast::BinaryOpKind::Add)
                && (expr_is_string(l, env, universe) || expr_is_string(r, env, universe))
            {
                *kind = crate::ast::BinaryOpKind::Concat;
            }
        }
        Expr::UnaryOp(_, inner) => rewrite_expr(inner, env, universe),
        Expr::List(items) => {
            for item in items {
                rewrite_expr(item, env, universe);
            }
        }
        _ => {}
    }
}

/// Conservative "is this expression a String/Blob value?" — literal, cast,
/// bound identifier, or the result of a string-producing binary op.
fn expr_is_string(
    expr: &Expr,
    env: &std::collections::HashMap<String, Type>,
    universe: &TypeUniverse,
) -> bool {
    match expr {
        Expr::Quoted(_) => true,
        Expr::Cast(_, ty) => is_string_category(ty, universe),
        Expr::Identifier(name) => env
            .get(name)
            .map(|t| is_string_category(t, universe))
            .unwrap_or(false),
        Expr::BinaryOp(_, l, r) => {
            expr_is_string(l, env, universe) || expr_is_string(r, env, universe)
        }
        Expr::UnaryOp(_, inner) => expr_is_string(inner, env, universe),
        _ => false,
    }
}

/// Is a type a String/Blob-category value? Mirrors the casting graph's
/// base-chain walk (no graph needed — checks the universe's Cast. properties
/// and the declared base). The bootstrap String/Data entries carry
/// Cast.String/Cast.Blob, so no type names are matched (rule 18).
pub fn is_string_category(ty: &Type, universe: &TypeUniverse) -> bool {
    match ty {
        Type::Custom(name) => {
            universe.get(name).map(|rt| {
                rt.properties.contains_key("Cast.String")
                    || rt.properties.contains_key("Cast.Blob")
                    // 2026-09-02 (de-hashtag sweep): category comparison on
                    // the trimmed base — legacy registrations may still
                    // spell the base "String"; the law is plain names.
                    || rt.base.trim_start_matches('#').starts_with("String")
                    || rt.base.trim_start_matches('#').starts_with("Blob")
            }).unwrap_or(false)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOpKind, Contract, Definition, OutputType, Statement};

    fn string_defn(name: &str, param_ty: Type, plus_rhs: Expr) -> Definition {
        use crate::ast::{Definition, Statement};
        Definition {
                variadic_param: None,
            name: name.to_string(),
            type_params: vec![],
            parameters: vec![("a".to_string(), param_ty.clone())],
            output_type: Some(OutputType::Single(param_ty)),
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body: vec![Statement::Term(Some(Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("a".to_string())),
                Box::new(plus_rhs),
            )))],
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        }
    }

    fn term_kind(defn: &Definition) -> BinaryOpKind {
        let Statement::Term(Some(expr)) = &defn.body[0] else { panic!("term expected") };
        let Expr::BinaryOp(kind, _, _) = expr else { panic!("binary expected") };
        *kind
    }

    #[test]
    fn plus_on_string_becomes_concat() {
        let universe = TypeUniverse::new();
        let mut items = vec![TopLevel::Definition(string_defn(
            "f",
            Type::Custom("String".to_string()),
            Expr::Quoted(b"x".to_vec()),
        ))];
        rewrite_plus_concat(&mut items, &universe);
        let TopLevel::Definition(d) = &items[0] else { panic!() };
        assert_eq!(term_kind(d), BinaryOpKind::Concat);
    }

    #[test]
    fn plus_on_int_stays_add() {
        let universe = TypeUniverse::new();
        let mut items = vec![TopLevel::Definition(string_defn(
            "f",
            Type::Custom("Int".to_string()),
            Expr::Decimal(1),
        ))];
        rewrite_plus_concat(&mut items, &universe);
        let TopLevel::Definition(d) = &items[0] else { panic!() };
        assert_eq!(term_kind(d), BinaryOpKind::Add);
    }

    #[test]
    fn concat_with_literal() {
        let universe = TypeUniverse::new();
        let mut items = vec![TopLevel::Definition(string_defn(
            "g",
            Type::Custom("String".to_string()),
            Expr::Quoted(b"literal".to_vec()),
        ))];
        rewrite_plus_concat(&mut items, &universe);
        let TopLevel::Definition(d) = &items[0] else { panic!() };
        assert_eq!(term_kind(d), BinaryOpKind::Concat);
    }
}
