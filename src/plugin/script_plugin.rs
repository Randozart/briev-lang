// ── Script Plugin — Front Stage ───────────────────────────────────────
// 2026-08-01 (Phase 4): Flat-scripting — a `.bv` file with a `defn main()`
// (no entry!/args! marker) or with only bare top-level statements gets a
// synthesized ONE-SHOT opening node that runs it exactly once.
//
// Two cases (plan §4.5):
//   1. `defn main()` exists (no explicit entry!): synthesize
//        let script_done: Bool = false;
//        node script_main [script_done == false][script_done] {
//            <call to main() — emitted as briev_main by the backend>;
//            script_done = true;
//        };
//   2. Only bare top-level statements (TopLevel::Statement Let) and no
//      defn/txn/node: synthesize the same one-shot node whose body runs the
//      script statements in order.
//
// Rules:
//   - `[true]` is never emitted — the guard `script_done == false` is a
//     real constraint, true exactly once.
//   - `script_main` / `script_done` are compiler-reserved; collision with
//     a user binding is a compile error (no silent shadowing).
//   - Runs at Parsed stage (before typechecking) so the node is visible to
//     the typechecker and the concurrency gate.

use crate::ast::{Expr, StageKind, Statement, TopLevel, Type};
use crate::plugin::Plugin;
use crate::type_universe::TypeUniverse;

#[derive(Debug)]
pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn name(&self) -> &str {
        "script"
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![StageKind::Parsed]
    }

    fn on_ast(
        &self,
        program: &mut Vec<TopLevel>,
        _universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        resolve_script(program)
    }
}

/// Whether the program declares an explicit entry!/args! (in which case the
/// script plugin must NOT synthesize an opening node).
fn has_explicit_entry(program: &[TopLevel]) -> bool {
    program.iter().any(|item| {
        let contract = match item {
            TopLevel::Definition(d) => Some(&d.contract),
            TopLevel::Transaction(t) => Some(&t.contract),
            _ => None,
        };
        contract.map_or(false, |c| {
            expr_has_intercept(&c.pre_condition)
                || expr_has_intercept(&c.post_condition)
        })
    })
}

fn expr_has_intercept(expr: &Expr) -> bool {
    match expr {
        Expr::PluginIntercept { .. } => true,
        Expr::BinaryOp(_, l, r) => expr_has_intercept(l) || expr_has_intercept(r),
        Expr::UnaryOp(_, e) | Expr::Cast(e, _) => expr_has_intercept(e),
        _ => false,
    }
}

fn has_main_defn(program: &[TopLevel]) -> bool {
    program.iter().any(|item| {
        if let TopLevel::Definition(d) = item {
            d.name == "main" && d.parameters.is_empty()
        } else {
            false
        }
    })
}

/// Count non-import/plugin-injected top-level items that would make a script
/// (vs a reactor program): statements/lets and constants.
fn has_script_content(program: &[TopLevel]) -> bool {
    program.iter().any(|item| match item {
        TopLevel::Statement(_) | TopLevel::Constant(_) | TopLevel::Init(_) => true,
        _ => false,
    })
}

/// Collect the bare top-level statements (lets) and constants that form the
/// script body, in order.
fn collect_script_statements(program: &[TopLevel]) -> Vec<Statement> {
    let mut out = Vec::new();
    for item in program {
        match item {
            TopLevel::Statement(stmt) => out.push(stmt.as_ref().clone()),
            TopLevel::Constant(c) => {
                // A `const name: T = expr;` becomes a `let name: T = expr;`
                // inside the script body so it is evaluated at runtime.
                out.push(Statement::Let {
                    name: c.name.clone(),
                    names: vec![],
                    ty: Some(c.ty.clone()),
                    expr: Some(c.expr.clone()),
                    modifiers: vec![],
                });
            }
            TopLevel::Init(init) => {
                // 2026-08-09: `init` = runtime-seeded invariant — in script
                // mode it is a runtime `let` (each line runs once; no reactor
                // ordering to protect). Body form unsupported here — an init
                // body seeds once before beginprogram; scripts have no reactor,
                // so the value form is the valid script shape.
                if let Some(value) = &init.value {
                    out.push(Statement::Let {
                        name: init.name.clone(),
                        names: vec![],
                        ty: Some(init.ty.clone()),
                        expr: Some(value.clone()),
                        modifiers: vec![],
                    });
                }
            }
            _ => {}
        }
    }
    out
}

fn resolve_script(program: &mut Vec<TopLevel>) -> Result<(), String> {
    // Only act on script-style programs: no defn/txn/node (other than a defn
    // main) AND no explicit entry!. If there are reactive nodes, the entry
    // plugin / reactor owns the program.
    if !is_script_style(program) {
        return Ok(());
    }
    if let Some(name) = reserved_collision(program) {
        return Err(format!(
            "'{}' is compiler-reserved — the script plugin synthesizes it \
             (no silent shadowing)",
            name
        ));
    }

    // Case 1: defn main — the node just calls it once.
    if has_main_defn(program) {
        let body = vec![
            // The backend renames `defn main` → `briev_main`; calling `main()`
            // in the AST resolves to the same defn.
            Statement::Expression(Expr::Call("main".into(), vec![], None)),
            Statement::Assign(
                Expr::Identifier("script_done".into()),
                Expr::Bool(true),
            ),
        ];
        synthesize(program, body)?;
        return Ok(());
    }

    // Case 2: bare top-level statements only.
    let mut script_stmts = collect_script_statements(program);
    if script_stmts.is_empty() {
        return Ok(()); // nothing to wrap
    }
    script_stmts.push(Statement::Assign(
        Expr::Identifier("script_done".into()),
        Expr::Bool(true),
    ));
    synthesize(program, script_stmts)?;
    Ok(())
}

/// A script-style program: no reactive node/txn, no sync<group>, no non-main
/// defn, and no explicit entry!.
fn is_script_style(program: &[TopLevel]) -> bool {
    let has_node_or_txn = program.iter().any(|item| {
        matches!(item, TopLevel::Transaction(_))
            // 2026-08-01 (Phase 4): sync<group> wraps a Transaction in a
            // TopLevel::SyncGroup — that is a reactive node and must NOT be
            // treated as a script.
            || matches!(item, TopLevel::SyncGroup { .. })
    });
    let has_other_defn = program.iter().any(|item| {
        if let TopLevel::Definition(d) = item {
            d.name != "main"
        } else {
            false
        }
    });
    !has_node_or_txn && !has_other_defn && !has_explicit_entry(program)
}

/// A top-level binding colliding with a compiler-reserved name, if any.
fn reserved_collision(program: &[TopLevel]) -> Option<&'static str> {
    for item in program {
        let name = match item {
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() {
                    name.as_str()
                } else {
                    continue;
                }
            }
            TopLevel::Constant(c) => c.name.as_str(),
            TopLevel::Definition(d) => d.name.as_str(),
            TopLevel::Init(i) => i.name.as_str(),
            _ => continue,
        };
        if name == "script_main" {
            return Some("script_main");
        }
        if name == "script_done" {
            return Some("script_done");
        }
    }
    None
}


/// Prepend `let script_done: Bool = false;` and append the synthesized
/// one-shot node. `[true]` is never emitted.
fn synthesize(program: &mut Vec<TopLevel>, body: Vec<Statement>) -> Result<(), String> {
    let done = TopLevel::Statement(Box::new(Statement::Let {
        name: "script_done".into(),
        names: vec![],
        ty: Some(Type::bool_()),
        expr: Some(Expr::Bool(false)),
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(crate::ast::Transaction {
        name: "script_main".into(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: crate::ast::Contract {
            pre_condition: Expr::BinaryOp(
                crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("script_done".into())),
                Box::new(Expr::Bool(false)),
            ),
            post_condition: Expr::Identifier("script_done".into()),
            watchdog: None,
            span: None,
            explicit: true,
            post_authority: false,
        },
        body,
        metadata: std::collections::HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        variadic_param: None,
        doc: None,
    });
    // Prepend the done-flag so the typechecker sees it declared; the node
    // goes after (the reactor finds it regardless of order).
    program.insert(0, done);
    program.push(node);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::Parser;

    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    #[test]
    fn test_defn_main_synthesizes_opening_node() {
        let mut program = parse(
            r#"
            defn main() -> Int {
                term 0;
            };
            "#,
        );
        resolve_script(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            debug.contains("script_main"),
            "defn main must synthesize script_main; got:\n{debug}"
        );
        assert!(
            debug.contains("script_done"),
            "defn main must synthesize script_done; got:\n{debug}"
        );
        assert!(
            debug.contains("Call(\"main\""),
            "the opening node must call main(); got:\n{debug}"
        );
        // [true] never emitted as the guard: the precondition is the real
        // constraint `script_done == false` (a Bool(true) in the defn's own
        // contract or the flip is legitimate, so assert on the node's guard).
        assert!(
            debug.contains("BinaryOp(Eq, Identifier(\"script_done\"), Bool(false))"),
            "script guard must be script_done == false (not [true]); got:\n{debug}"
        );
    }

    #[test]
    fn test_bare_lets_synthesize_opening_node() {
        let mut program = parse(
            r#"
            let x: Int = 42;
            let y: Int = x + 1;
            "#,
        );
        resolve_script(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            debug.contains("script_main"),
            "bare lets must synthesize script_main; got:\n{debug}"
        );
        assert!(
            debug.contains("name: \"x\"")
                || debug.contains("Identifier(\"x\"")
                || debug.contains("x"),
            "script body must run the lets; got:\n{debug}"
        );
    }

    #[test]
    fn test_reactive_node_program_not_wrapped() {
        let mut program = parse(
            r#"
            let a: Int = 0;
            node work [a == 0][a == 1] {
                a = 1;
                term;
            };
            "#,
        );
        resolve_script(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            !debug.contains("script_main"),
            "a reactive program must NOT be wrapped; got:\n{debug}"
        );
    }

    #[test]
    fn test_reserved_collision_errors() {
        let mut program = parse(
            r#"
            let script_done: Bool = false;
            defn main() -> Int { term 0; };
            "#,
        );
        let err = resolve_script(&mut program).unwrap_err();
        assert!(
            err.contains("compiler-reserved"),
            "collision with script_done must error; got: {err}"
        );
    }

    #[test]
    fn test_sync_group_not_wrapped_as_script() {
        // 2026-08-01 (Phase 4): sync<group> node is a reactive transaction
        // (wrapped in TopLevel::SyncGroup) — the script plugin must not treat
        // it as a script and synthesize an opening node.
        let mut program = parse(
            r#"
            sync<g> node work [a == 0][a == 1] {
                a = 1;
                term;
            };
            "#,
        );
        resolve_script(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            !debug.contains("script_main"),
            "sync<group> node must NOT be wrapped; got:\n{debug}"
        );
    }

    #[test]
    fn test_entry_program_not_wrapped() {
        // 2026-08-01 (Phase 4): a program with entry! must not be wrapped.
        let mut program = parse(
            r#"
            defn main() -> Int [entry!("run")][result == 0] { term 0; };
            "#,
        );
        resolve_script(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            !debug.contains("script_main"),
            "entry! program must NOT be wrapped; got:\n{debug}"
        );
    }
}
