// ── Environment Plugin — Front Stage ────────────────────────────────────
// 2026-07-19: Resolves !GetEnv(name) and !GetEnvInt(name) to their stdlib
// equivalents: getenv(name) and get_env_int(name). Runs at Front stage so
// replacements are visible to the typechecker.
//
// 2026-08-01: Phase 1 of the plugin-macro rework — the entry points are the
// lowercase macros `get_env!` / `get_env_int!` (and the stdlib-backed
// `get_env_or_default!`). The old PascalCase names are rejected by the
// typechecker with a rename hint (see src/typechecker/mod.rs).

use crate::ast::{Expr, StageKind, TopLevel};
use crate::plugin::Plugin;
use crate::type_universe::TypeUniverse;

/// Environment plugin: replaces PluginIntercept calls for env var access.
#[derive(Debug)]
pub struct EnvPlugin;

impl Plugin for EnvPlugin {
    fn name(&self) -> &str {
        "env"
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![StageKind::Parsed]
    }

    fn on_ast(
        &self,
        program: &mut Vec<TopLevel>,
        _universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        // 2026-07-19: First pass — resolve const-level !GetEnv/!GetEnvInt to
        // literal values. This allows `const N = !GetEnvInt("BOUND")` to work
        // even though get_env_int is a regular function (not an intrinsic).
        resolve_const_env_vars(program);
        // Second pass — normal resolution for non-const contexts
        for item in program.iter_mut() {
            walk_item(item);
        }
        Ok(())
    }
}

/// Evaluate get_env!/get_env_int! inside const declarations at compile time.
fn resolve_const_env_vars(program: &mut [TopLevel]) {
    for item in program.iter_mut() {
        let TopLevel::Constant(c) = item else { continue };
        let Expr::PluginIntercept { name, args, .. } = &c.expr else { continue };
        let key = extract_string_arg(args).unwrap_or_default();
        let val = std::env::var(&key).unwrap_or_default();
        c.expr = match name.as_str() {
            "get_env_int" => Expr::Decimal(val.parse::<i64>().unwrap_or(0)),
            "get_env"     => Expr::Quoted(val.as_bytes().to_vec()),
            _             => continue,
        };
    }
}

/// Extract a string literal argument from a PluginIntercept call.
fn extract_string_arg(args: &[Expr]) -> Option<String> {
    if let Some(Expr::Quoted(bytes)) = args.first() {
        String::from_utf8(bytes.clone()).ok()
    } else {
        None
    }
}

// 2026-11-04: nested environment macros in collector loop/operand positions
// escaped the partial walker and reached codegen unresolved. Use the shared
// postorder visitor so children are lowered before their enclosing intercept.
// Preserve the 2026-09-16 receiver-first fix and 2026-09-28 sync-group
// descent (including 2026-08-09 init seeding). Retire this callback if expansion moves
// into a common plugin dispatcher, retaining the exhaustive traversal contract.
fn walk_item(item: &mut TopLevel) {
    crate::ast::visit_mut::walk_toplevel(item, &mut resolve_expr);
}

fn resolve_expr(expr: &mut Expr) {
    let Expr::PluginIntercept { name, args, receiver, .. } = expr else { return };
    let mut full_args = Vec::with_capacity(args.len() + usize::from(receiver.is_some()));
    if let Some(recv) = receiver {
        full_args.push((**recv).clone());
    }
    full_args.extend(args.iter().cloned());
    if let Some(replacement) = resolve_intercept(name, &full_args) {
        *expr = replacement;
    }
}
/// 2026-08-01: Resolve a plugin-intercept call to a typed stdlib function
/// call. Only lowercase macro names are rewritten; PascalCase legacy names
/// fall through to the typechecker, which rejects them with a rename hint.
fn resolve_intercept(name: &str, args: &[Expr]) -> Option<Expr> {
    let call_args = args.to_vec();
    match name {
        "get_env" => Some(Expr::Call("get_env".to_string(), call_args, None)),
        "get_env_int" => Some(Expr::Call("get_env_int".to_string(), call_args, None)),
        "get_env_or_default" => Some(Expr::Call("get_env_or_default".to_string(), call_args, None)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(source: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(source).expect("tokens");
        let mut program = crate::parser::Parser::new(tokens, source)
            .parse_program().expect("syntax");
        resolve_const_env_vars(&mut program);
        program.iter_mut().for_each(walk_item);
        program
    }

    #[test]
    fn expands_environment_calls_in_nested_control_flow_and_operands() {
        let program = expand(r#"
            defn check(values: List<Int>) -> Int {
                foreach index in 0..get_env_int!("READS") {
                    when get_env_int!("MODE") > 0 {
                        let value: Int = values[get_env_int!("INDEX")];
                    };
                };
                term get_env_int!(get_env_or_default!("KEY", "READS"));
            };
        "#);
        let mut calls = Vec::new();
        for item in &program {
            crate::ast::visit::walk_toplevel(item, &mut |expr| match expr {
                Expr::PluginIntercept { .. } => panic!("environment intercept survived expansion"),
                Expr::Call(name, _, _) => calls.push(name.clone()),
                _ => {}
            });
        }
        assert_eq!(calls.iter().filter(|name| *name == "get_env_int").count(), 4);
        assert_eq!(calls.iter().filter(|name| *name == "get_env_or_default").count(), 1);
    }

    #[test]
    fn expands_children_of_unknown_plugins_without_rewriting_the_parent() {
        let program = expand(r#"
            defn check() -> Int {
                term another_plugin!(get_env_int!("READS"));
            };
        "#);
        let TopLevel::Definition(definition) = &program[0] else { panic!("definition") };
        let crate::ast::Statement::Term(Some(Expr::PluginIntercept { name, args, .. })) =
            &definition.body[0] else { panic!("unknown plugin must remain") };
        assert_eq!(name, "another_plugin");
        assert!(matches!(&args[0], Expr::Call(name, _, _) if name == "get_env_int"));
    }

    #[test]
    fn chained_environment_call_keeps_receiver_before_expanded_arguments() {
        let mut expression = Expr::PluginIntercept {
            name: "get_env_or_default".into(),
            args: vec![Expr::PluginIntercept {
                name: "get_env".into(),
                args: vec![Expr::Quoted(b"DEFAULT".to_vec())],
                type_args: vec![], receiver: None,
            }],
            type_args: vec![],
            receiver: Some(Box::new(Expr::Quoted(b"KEY".to_vec()))),
        };
        crate::ast::visit_mut::walk_expr(&mut expression, &mut resolve_expr);
        let Expr::Call(name, args, _) = expression else { panic!("expanded call") };
        assert_eq!(name, "get_env_or_default");
        assert_eq!(args[0], Expr::Quoted(b"KEY".to_vec()));
        assert!(matches!(&args[1], Expr::Call(name, _, _) if name == "get_env"));
    }
}
