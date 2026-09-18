// ── ExecuteMany Plugin — Front Stage ────────────────────────────────────
// 2026-09-18 (plan 2026-09-18-execute-many-macro): `execute_many!(callee,
// (a), (b, c), …)` expands at the Parsed stage to sequential applications
// `callee(a); callee(b, c); …` — one per parameter block.
//
// Purpose: heterogeneous per-call literal blocks that a runtime `foreach`
// cannot express — document-fill lines, and repeated `Asm#` invocations
// whose immediate operands must be compile-time constants.
//
// Delimiter contract (SPEC §18.2, Rule 21): `()` is the APPLICATION
// delimiter at both levels — the outer `!()` invokes the macro, each
// inner `()` is one application spine written without its callee. A
// parenthesized tuple `(a, b)` is a multi-arg block, any other
// expression `(x)` is a single-arg block, `()` is the empty block. At
// least one block is required (a zero-invocation call site is a
// mistake, not a no-op). Statement-only: as an expression it is an
// error with the fix — bind the last call explicitly.

use crate::ast::{Expr, Statement, TopLevel};
use crate::plugin::StageKind;
use crate::plugin::Plugin;
use crate::type_universe::TypeUniverse;

#[derive(Debug)]
pub struct ExecuteManyPlugin;

impl Plugin for ExecuteManyPlugin {
    fn name(&self) -> &str {
        "execute-many"
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![StageKind::Parsed]
    }

    fn on_ast(
        &self,
        program: &mut Vec<TopLevel>,
        _universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        for item in program.iter_mut() {
            match item {
                TopLevel::Definition(d) => expand_stmts(&mut d.body)?,
                TopLevel::Transaction(t) => expand_stmts(&mut t.body)?,
                TopLevel::Statement(stmt) => {
                    let mut one = vec![(**stmt).clone()];
                    expand_stmts(&mut one)?;
                    *stmt = Box::new(one.into_iter().next().unwrap_or(Statement::Break));
                }
                TopLevel::Init(init) => {
                    expand_stmts(&mut init.body)?;
                }
                // Constants and metadata cannot host statements — the
                // typechecker reports the position error if someone tries.
                _ => {}
            }
        }
        Ok(())
    }
}

/// Parse one parameter block: Tuple → its elements; any other expression →
/// a single-argument block (the grouping parser already unwrapped `(x)`).
fn block_args(e: &Expr) -> Vec<Expr> {
    match e {
        Expr::Tuple(elems) => elems.clone(),
        other => vec![other.clone()],
    }
}

fn expand_call(name: &str, args: &[Expr]) -> Result<Vec<Statement>, String> {
    let Some(Expr::Identifier(callee)) = args.first() else {
        return Err(format!(
            "execute_many! needs a callee name as its first argument\n  \
             why: the macro expands to `callee(block)` per block, so it must \
             know what to call\n  \
             fix: write the function or intrinsic name first, e.g. \
             `execute_many!(PrintInt#, (1), (2));`"
        ));
    };
    if args.len() < 2 {
        return Err(format!(
            "execute_many! requires at least one invocation block\n  \
             why: a zero-invocation call site is a mistake, not a no-op\n  \
             fix: drop the call, or add blocks: `execute_many!({name}, (…));`"
        ));
    }
    let callee = callee.clone();
    Ok(args[1..]
        .iter()
        .map(|block| Statement::Expression(Expr::Call(callee.clone(), block_args(block), None)))
        .collect())
}

/// Does this expression hold an execute_many! anywhere?
fn expr_has_execute_many(e: &Expr) -> bool {
    match e {
        Expr::PluginIntercept { name, .. } => name == "execute_many",
        Expr::BinaryOp(_, l, r) => expr_has_execute_many(l) || expr_has_execute_many(r),
        Expr::UnaryOp(_, x) => expr_has_execute_many(x),
        Expr::Index(o, i) => expr_has_execute_many(o) || expr_has_execute_many(i),
        Expr::Call(_, args, _) => args.iter().any(expr_has_execute_many),
        Expr::Cast(x, _) => expr_has_execute_many(x),
        _ => false,
    }
}

/// execute_many! in a NON-statement position (let initializer, guard
/// condition, foreach list): reject with the fix rather than silently
/// discarding values.
fn reject_misplaced(stmt: &Statement) -> Result<(), String> {
    let hostile = match stmt {
        Statement::Let { expr: Some(e), .. } => expr_has_execute_many(e),
        Statement::Guarded(g, body) => {
            expr_has_execute_many(g) || body.iter().any(|s| {
                matches!(s, Statement::Expression(Expr::PluginIntercept { name, .. }) if name == "execute_many")
            })
        }
        Statement::Foreach { list, .. } => expr_has_execute_many(list),
        _ => false,
    };
    if hostile {
        return Err(
            "execute_many! is a statement construct — it keeps the side \
             effects and discards every result\n  \
             why: as an expression its value semantics would be the last \
             call's, inviting accidental use\n  \
             fix: write it as a statement, or bind calls explicitly: \
             `let v = callee(…);`"
                .to_string(),
        );
    }
    Ok(())
}

fn expand_stmts(stmts: &mut Vec<Statement>) -> Result<(), String> {
    let mut i = 0;
    while i < stmts.len() {
        reject_misplaced(&stmts[i])?;
        // Recurse into nested statement lists first (kernels nest foreach
        // bodies; guards and blocks nest too).
        match &mut stmts[i] {
            Statement::Foreach { body, .. } => expand_stmts(body)?,
            Statement::Guarded(_, body) => expand_stmts(body)?,
            Statement::Block(body) => expand_stmts(body)?,
            _ => {}
        }
        let repl = match &stmts[i] {
            Statement::Expression(Expr::PluginIntercept { name, args, .. })
                if name == "execute_many" =>
            {
                Some(expand_call(name, args)?)
            }
            _ => None,
        };
        if let Some(repl) = repl {
            let n = repl.len();
            stmts.splice(i..i + 1, repl);
            i += n;
        } else {
            i += 1;
        }
    }
    Ok(())
}
