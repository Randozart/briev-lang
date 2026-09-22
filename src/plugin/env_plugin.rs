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

fn walk_item(item: &mut TopLevel) {
    match item {
        TopLevel::Definition(d) => walk_stmts(&mut d.body),
        TopLevel::Transaction(t) => walk_stmts(&mut t.body),
        TopLevel::Constant(c) => walk_expr(&mut c.expr),
        TopLevel::Statement(stmt) => walk_stmt(stmt),
        // 2026-08-09: `init` seeding (`get_env_int!` etc.) is runtime input —
        // walk the value expr and any body statements like a const/let.
        TopLevel::Init(init) => {
            if let Some(value) = &mut init.value {
                walk_expr(value);
            }
            walk_stmts(&mut init.body);
        }
        TopLevel::StateDecl(_) | TopLevel::Trigger(_) => {}
        TopLevel::ForeignBinding(_) => {}
        _ => {}
    }
}

fn walk_stmts(stmts: &mut [crate::ast::Statement]) {
    for stmt in stmts.iter_mut() {
        walk_stmt(stmt);
    }
}

fn walk_stmt(stmt: &mut crate::ast::Statement) {
    match stmt {
        crate::ast::Statement::Assign(_, expr)
        | crate::ast::Statement::Let { expr: Some(expr), .. } => {
            walk_expr(expr);
        }
        crate::ast::Statement::Expression(expr)
        | crate::ast::Statement::Term(Some(expr))
        | crate::ast::Statement::EndProgram(Some(expr)) => {
            walk_expr(expr);
        }
        crate::ast::Statement::Guarded(_, body) => {
            walk_stmts(body);
        }
        _ => {}
    }
}

fn walk_expr(expr: &mut Expr) {
    match expr {
        Expr::PluginIntercept { name, args, type_args: _, receiver, chain_refs: _ } => {
            // 2026-09-16 (Bug A): walk the receiver for nested intercepts, then
            // prepend it as the first argument (UFCS-style chaining) so it is
            // not silently dropped.
            let mut full_args = args.clone();
            if let Some(recv) = receiver {
                walk_expr(recv);
                full_args.insert(0, (**recv).clone());
            }
            if let Some(replacement) = resolve_intercept(name, &full_args) {
                *expr = replacement;
            }
        }
        Expr::BinaryOp(_, lhs, rhs) => {
            walk_expr(lhs);
            walk_expr(rhs);
        }
        Expr::UnaryOp(_, inner) => walk_expr(inner),
        Expr::Consume(inner) => walk_expr(inner),
        Expr::Char(_) => {}
        Expr::Decimal(_) | Expr::Bool(_) | Expr::BeginProgram | Expr::Float(_) => {}
        Expr::Call(_, args, _) | Expr::Spawn { args, .. } => {
            for a in args { walk_expr(a); }
        }
        Expr::If(cond, then, else_) => {
            walk_expr(cond);
            walk_expr(then);
            if let Some(el) = else_ { walk_expr(el); }
        }
        Expr::Match(_, arms) => {
            for arm in arms { walk_expr(&mut arm.body); }
        }
        Expr::Block(stmts) => walk_stmts(stmts),
        Expr::Tuple(elems) | Expr::List(elems) => {
            for e in elems { walk_expr(e); }
        }
        Expr::Field(obj, _) | Expr::Index(obj, _) => walk_expr(obj),
        Expr::Cast(inner, _) | Expr::IsType(inner, _) | Expr::Deref(inner) | Expr::Await(inner)
        | Expr::AddrOf(inner) => walk_expr(inner),
        Expr::Within(body, _) => walk_expr(body),
        Expr::Lambda(_, body) => walk_expr(body),
        Expr::DerivationBlock(db) => {
            for ex in &mut db.examples {
                for inp in &mut ex.inputs { walk_expr(inp); }
                walk_expr(&mut ex.output);
            }
        }
        // Leaves — no sub-expressions
        Expr::Decimal(_) | Expr::TaggedLiteral(_, _) | Expr::Bool(_) | Expr::Float(_) | Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _)
        | Expr::Identifier(_)
        | Expr::FormattingAnnotation(_)
        | Expr::UnitLiteral { .. } => {}
        Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => {
            walk_expr(recv);
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            walk_expr(recv);
            for a in args { walk_expr(a); }
        }
        Expr::StructLiteral { .. } => {}
        Expr::Exists(_) => { unreachable!("fn? only in stage eval") },
        Expr::Slice { .. } => {},
Expr::Slice { .. } => {},
        Expr::Range { .. } => {},
        Expr::Capture { expr, .. } => walk_expr(expr),

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
