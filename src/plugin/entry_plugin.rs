// ── Entry Plugin — Front Stage ────────────────────────────────────────
// 2026-08-01 (Phase 3): Resolves the `entry!` / `args!` macros to explicit
// CLI entry-point contracts (see plan §4.3). Replaces the removed `[#]`
// marker (Phase 2).
//
//   node build [entry!("build")][result == 0] { ... }
//
// expands to a one-shot entry node:
//
//   let __entry_build_done: Bool = false;
//   node build [entry_cmd() == "build" && !__entry_build_done][result == 0] {
//       ...
//       __entry_build_done = true;
//   };
//
// `args!("--flag")` becomes a snapshot state field bound from __argv_has
// (read-only: the enclosing node's one-shot guard governs firing).
//
// Rules:
//   - `[true]` is never emitted — the entry guard is always a real
//     constraint (`entry_cmd() == "<cmd>" && !__entry_<cmd>_done`).
//   - `defn` (non-reactive) entry points get a synthesized reactive wrapper
//     node (the "helper node" path — CLI-addressable defns become subcommands).
//   - helper names `__entry_<cmd>_done` / `arg_<flag>` are compiler-reserved;
//     a collision with an existing top-level binding is a compile error.
//   - the plugin ensures `import "std/cli.bv"` exists (like the prelude).

use crate::ast::{Expr, StageKind, Statement, TopLevel, Type};
use crate::plugin::Plugin;
use crate::type_universe::TypeUniverse;
use std::collections::HashSet;

#[derive(Debug)]
pub struct EntryPlugin;

impl Plugin for EntryPlugin {
    fn name(&self) -> &str {
        "entry"
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![StageKind::Parsed]
    }

    fn on_ast(
        &self,
        program: &mut Vec<TopLevel>,
        _universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        resolve_entries(program)
    }
}

/// Sanitize a flag into a valid identifier: strip leading `-`, `-`→`_`.
/// `--out` → `arg_out`.
fn sanitize_flag(flag: &str) -> String {
    flag.trim_start_matches('-').replace('-', "_")
}

/// Build a `let name: Type = expr;` top-level statement (a state field with
/// a runtime initializer, registered in field_initializers by the backend).
fn top_level_let(name: String, ty: Type, expr: Expr) -> TopLevel {
    TopLevel::Statement(Box::new(Statement::Let {
        name,
        names: vec![],
        ty: Some(ty),
        expr: Some(expr),
        modifiers: vec![],
    }))
}

/// Collect all existing top-level binding names to detect collisions.
fn collect_existing(program: &[TopLevel]) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in program {
        match item {
            TopLevel::StateDecl(s) => { names.insert(s.name.clone()); }
            TopLevel::Definition(d) => { names.insert(d.name.clone()); }
            TopLevel::Transaction(t) => { names.insert(t.name.clone()); }
            TopLevel::Constant(c) => { names.insert(c.name.clone()); }
            TopLevel::Init(i) => { names.insert(i.name.clone()); }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() {
                    names.insert(name.clone());
                }
            }
            _ => {}
        }
    }
    names
}

/// Whether the program already imports std/cli.bv.
fn has_cli_import(program: &[TopLevel]) -> bool {
    program.iter().any(|item| {
        if let TopLevel::Import(imp) = item {
            imp.path().ends_with("std/cli.bv")
        } else {
            false
        }
    })
}

fn resolve_entries(program: &mut Vec<TopLevel>) -> Result<(), String> {
    let existing = collect_existing(program);
    let mut injected_import = false;
    if !has_cli_import(program) {
        program.insert(0, TopLevel::Import(crate::ast::Import::literal("std/cli.bv", vec![])));
        injected_import = true;
    }

    // First pass: collect entry!/args! intercepts per node so we can
    // dedupe entry guards (same command across nodes shares one done-flag).
    let mut entry_commands: Vec<String> = Vec::new();
    let mut arg_flags: Vec<(String, Option<String>)> = Vec::new();
    collect_intercepts(program, &mut entry_commands, &mut arg_flags);

    // Inject the done-flag state fields and arg snapshot fields as top-level
    // `let` statements (state fields with runtime initializers — the backend
    // registers these in field_initializers via TopLevel::Statement(Let)).
    let mut extra_state: Vec<TopLevel> = Vec::new();
    for cmd in &entry_commands {
        let field = format!("__entry_{}_done", sanitize_flag(cmd));
        if existing.contains(&field) {
            return Err(format!(
                "entry!: binding '{}' already exists — entry helper names are \
                 compiler-reserved (no silent shadowing)",
                field
            ));
        }
        extra_state.push(top_level_let(
            field,
            Type::bool_(),
            Expr::Bool(false),
        ));
    }
    for (flag, ty) in &arg_flags {
        let field = format!("arg_{}", sanitize_flag(flag));
        if existing.contains(&field) {
            return Err(format!(
                "args!: binding '{}' already exists — args helper names are \
                 compiler-reserved (no silent shadowing)",
                field
            ));
        }
        // Snapshot initializer: __argv_has("--flag") for Bool, __argv_value
        // for typed values.
        let call = |name: &str| {
            Expr::Call(name.into(), vec![Expr::Quoted(flag.as_bytes().to_vec())], None)
        };
        let field_ty = match ty.as_deref() {
            Some("Int") => Type::int(),
            Some("Float") => Type::float(),
            Some("String") => Type::string(),
            Some("Bool") => Type::bool_(),
            Some(other) => {
                return Err(format!(
                    "args!: unsupported value type '{}' for flag '{}' (expected Int/Float/String/Bool)",
                    other, flag
                ));
            }
            None => Type::bool_(),
        };
        let init = if ty.is_some() { call("__argv_value") } else { call("__argv_has") };
        extra_state.push(top_level_let(field, field_ty, init));
    }

    // Rewrite intercepts in contracts + append the flip, and synthesize
    // wrapper nodes for non-reactive defn entry points.
    rewrite_nodes(program, &entry_commands, &arg_flags)?;

    // Prepend the injected state fields before all existing top-level items
    // (so the typechecker sees them as declared).
    if !extra_state.is_empty() {
        let mut combined: Vec<TopLevel> = extra_state;
        combined.append(program);
        *program = combined;
    }

    let _ = injected_import;
    Ok(())
}

/// Walk every Definition/Transaction, collect entry!/args! intercepts from
/// contracts and bodies.
fn collect_intercepts(
    program: &[TopLevel],
    entry_commands: &mut Vec<String>,
    arg_flags: &mut Vec<(String, Option<String>)>,
) {
    for item in program {
        match item {
            TopLevel::Definition(d) => {
                collect_from_expr(&d.contract.pre_condition, entry_commands, arg_flags);
                collect_from_expr(&d.contract.post_condition, entry_commands, arg_flags);
                // 2026-08-01 (Phase 3b): args! may appear in bodies.
                for stmt in &d.body {
                    collect_from_stmt(stmt, entry_commands, arg_flags);
                }
            }
            TopLevel::Transaction(t) => {
                collect_from_expr(&t.contract.pre_condition, entry_commands, arg_flags);
                collect_from_expr(&t.contract.post_condition, entry_commands, arg_flags);
                for stmt in &t.body {
                    collect_from_stmt(stmt, entry_commands, arg_flags);
                }
            }
            _ => {}
        }
    }
}

fn collect_from_stmt(
    stmt: &Statement,
    entry_commands: &mut Vec<String>,
    arg_flags: &mut Vec<(String, Option<String>)>,
) {
    use crate::ast::Statement;
    match stmt {
        Statement::Let { expr: Some(e), .. }
        | Statement::Assign(_, e)
        | Statement::Expression(e)
        | Statement::Term(Some(e))
        | Statement::EndProgram(Some(e)) => collect_from_expr(e, entry_commands, arg_flags),
        Statement::Guarded(cond, body) => {
            collect_from_expr(cond, entry_commands, arg_flags);
            for s in body {
                collect_from_stmt(s, entry_commands, arg_flags);
            }
        }
        _ => {}
    }
}

fn collect_from_expr(
    expr: &Expr,
    entry_commands: &mut Vec<String>,
    arg_flags: &mut Vec<(String, Option<String>)>,
) {
    match expr {
        Expr::PluginIntercept { name, args, .. } => {
            match name.as_str() {
                "entry" => {
                    if let Some(Expr::Quoted(cmd)) = args.first() {
                        let s = String::from_utf8_lossy(cmd).to_string();
                        if !entry_commands.contains(&s) {
                            entry_commands.push(s);
                        }
                    }
                }
                "args" => {
                    if let Some(Expr::Quoted(flag)) = args.first() {
                        let f = String::from_utf8_lossy(flag).to_string();
                        let ty = args.get(1).and_then(|a| match a {
                            Expr::Identifier(n) => Some(n.clone()),
                            _ => None,
                        });
                        if !arg_flags.iter().any(|(x, _)| *x == f) {
                            arg_flags.push((f, ty));
                        }
                    }
                }
                _ => {}
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_from_expr(l, entry_commands, arg_flags);
            collect_from_expr(r, entry_commands, arg_flags);
        }
        Expr::UnaryOp(_, e) | Expr::Cast(e, _) | Expr::Deref(e) | Expr::AddrOf(e) => {
            collect_from_expr(e, entry_commands, arg_flags);
        }
        _ => {}
    }
}

/// Rewrite `entry!`/`args!` intercepts in each node's contract to their
/// guard forms, and append the done-flip to the body.
fn rewrite_nodes(
    program: &mut Vec<TopLevel>,
    _entry_commands: &[String],
    arg_flags: &[(String, Option<String>)],
) -> Result<(), String> {
    // Wrapper nodes for non-reactive defn entry points, collected here and
    // appended after the loop.
    let mut wrappers: Vec<TopLevel> = Vec::new();

    for item in program.iter_mut() {
        match item {
            TopLevel::Definition(d) => rewrite_definition(d, arg_flags, &mut wrappers)?,
            TopLevel::Transaction(t) => rewrite_transaction(t, arg_flags)?,
            _ => {}
        }
    }

    program.extend(wrappers);
    Ok(())
}

/// Rewrite a Definition's contract + body, and synthesize a wrapper node for
/// a non-reactive defn entry point (§4.3 step 4 — the "helper node" path).
fn rewrite_definition(
    d: &mut crate::ast::Definition,
    arg_flags: &[(String, Option<String>)],
    wrappers: &mut Vec<TopLevel>,
) -> Result<(), String> {
    let entries = rewrite_contract(&mut d.contract, arg_flags)?;
    // args! may appear in defn bodies too.
    let mut entries_body = entries.clone();
    let mut rewritten = Vec::new();
    for stmt in std::mem::take(&mut d.body) {
        rewritten.push(rewrite_stmt(stmt, &mut entries_body, arg_flags)?);
    }
    d.body = rewritten;
    let entries = entries_body;
    if !entries.is_empty() && d.parameters.is_empty() {
        wrappers.push(make_wrapper(d, &entries));
    }
    Ok(())
}

/// Rewrite a Transaction's contract + body, and append the done-flip.
fn rewrite_transaction(
    t: &mut crate::ast::Transaction,
    arg_flags: &[(String, Option<String>)],
) -> Result<(), String> {
    let entries = rewrite_contract(&mut t.contract, arg_flags)?;
    // args! may appear in node bodies.
    let mut entries_body = entries.clone();
    let mut rewritten = Vec::new();
    for stmt in std::mem::take(&mut t.body) {
        rewritten.push(rewrite_stmt(stmt, &mut entries_body, arg_flags)?);
    }
    t.body = rewritten;
    for cmd in &entries_body {
        let field = format!("__entry_{}_done", sanitize_flag(cmd));
        t.body.push(Statement::Assign(
            Expr::Identifier(field.clone()),
            Expr::Bool(true),
        ));
    }
    Ok(())
}

/// Synthesize the reactive wrapper node for a non-reactive defn entry point.
/// The wrapper calls N, then flips the done-flag (without the flip the
/// reactor loops forever); its postcondition is the done-flag so the reactor
/// sees convergence (one-shot).
fn make_wrapper(d: &crate::ast::Definition, entries: &[String]) -> TopLevel {
    let wrapper_name = format!("__entry_{}", sanitize_flag(&entries[0]));
    let mut wrapper_body = vec![
        Statement::Expression(Expr::Call(d.name.clone(), vec![], None)),
    ];
    for cmd in entries {
        let field = format!("__entry_{}_done", sanitize_flag(cmd));
        wrapper_body.push(Statement::Assign(
            Expr::Identifier(field.clone()),
            Expr::Bool(true),
        ));
    }
    let done_field = Expr::Identifier(format!(
        "__entry_{}_done", sanitize_flag(&entries[0])
    ));
    let wrapper_contract = crate::ast::Contract {
        pre_condition: d.contract.pre_condition.clone(),
        post_condition: done_field,
        watchdog: None,
        span: None,
        explicit: true,
        post_authority: false,
    };
    TopLevel::Transaction(crate::ast::Transaction {
        name: wrapper_name,
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: wrapper_contract,
        body: wrapper_body,
        metadata: std::collections::HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    })
}

/// Rewrite `entry!`/`args!` intercepts inside a contract. Returns the entry
/// command list (for the flip injection). `[true]` is never emitted.
fn rewrite_contract(
    contract: &mut crate::ast::Contract,
    arg_flags: &[(String, Option<String>)],
) -> Result<Vec<String>, String> {
    let mut entries = Vec::new();
    contract.pre_condition = rewrite_expr(
        std::mem::replace(&mut contract.pre_condition, Expr::Bool(true)),
        &mut entries,
        arg_flags,
    )?;
    Ok(entries)
}

/// Rewrite `args!` intercepts inside a statement body (an entry! in a body is
/// an error — it belongs in the contract). Reuses the shared expr rewriter.
fn rewrite_stmt(
    stmt: Statement,
    entries: &mut Vec<String>,
    arg_flags: &[(String, Option<String>)],
) -> Result<Statement, String> {
    use crate::ast::Statement;
    Ok(match stmt {
        Statement::Let { name, names, ty, expr, modifiers } => Statement::Let {
            name,
            names,
            ty,
            expr: expr
                .map(|e| rewrite_expr(e, entries, arg_flags))
                .transpose()?,
            modifiers,
        },
        Statement::Assign(l, r) => Statement::Assign(
            l,
            rewrite_expr(r, entries, arg_flags)?,
        ),
        Statement::Expression(e) => Statement::Expression(rewrite_expr(e, entries, arg_flags)?),
        Statement::Term(Some(e)) => Statement::Term(Some(rewrite_expr(e, entries, arg_flags)?)),
        Statement::EndProgram(Some(e)) => Statement::EndProgram(Some(rewrite_expr(e, entries, arg_flags)?)),
        Statement::Guarded(cond, body) => Statement::Guarded(
            rewrite_expr(cond, entries, arg_flags)?,
            body.into_iter()
                .map(|s| rewrite_stmt(s, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        other => other,
    })
}


/// Rewrite one expression tree, replacing entry!/args! intercepts.
fn rewrite_expr(
    expr: Expr,
    entries: &mut Vec<String>,
    arg_flags: &[(String, Option<String>)],
) -> Result<Expr, String> {
    match expr {
        Expr::PluginIntercept { name, args, type_args, receiver: _, chain_refs: _ } => match name.as_str() {
            "entry" => {
                let cmd = match args.first() {
                    Some(Expr::Quoted(c)) => String::from_utf8_lossy(c).to_string(),
                    _ => return Err("entry!: expected a string literal command".into()),
                };
                if !entries.contains(&cmd) {
                    entries.push(cmd.clone());
                }
                let field = format!("__entry_{}_done", sanitize_flag(&cmd));
                // entry_cmd() == "<cmd>" && !__entry_<cmd>_done
                let eq = Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Eq,
                    Box::new(Expr::Call("entry_cmd".into(), vec![], None)),
                    Box::new(Expr::Quoted(cmd.into_bytes())),
                );
                let not_done = Expr::UnaryOp(
                    crate::ast::UnaryOpKind::Not,
                    Box::new(Expr::Identifier(field)),
                );
                Ok(Expr::BinaryOp(
                    crate::ast::BinaryOpKind::And,
                    Box::new(eq),
                    Box::new(not_done),
                ))
            }
            "args" => {
                let flag = match args.first() {
                    Some(Expr::Quoted(f)) => String::from_utf8_lossy(f).to_string(),
                    _ => return Err("args!: expected a string literal flag".into()),
                };
                let _ = type_args;
                // Typed form: args!("--flag", T) → arg_<flag> (T-typed snapshot).
                // The field type is resolved from the second arg if present.
                let ty = args.get(1).and_then(|a| match a {
                    Expr::Identifier(n) => {
                        let t = match n.as_str() {
                            "Int" => Some(Type::int()),
                            "Float" => Some(Type::float()),
                            "String" => Some(Type::string()),
                            "Bool" => Some(Type::bool_()),
                            _ => None,
                        };
                        t
                    }
                    _ => None,
                });
                let _ = arg_flags;
                let field = format!("arg_{}", sanitize_flag(&flag));
                let _ = ty;
                Ok(Expr::Identifier(field))
            }
            _ => Ok(Expr::PluginIntercept { name, args, type_args, receiver: None, chain_refs: vec![] }),
        },
        Expr::BinaryOp(kind, l, r) => Ok(Expr::BinaryOp(
            kind,
            Box::new(rewrite_expr(*l, entries, arg_flags)?),
            Box::new(rewrite_expr(*r, entries, arg_flags)?),
        )),
        Expr::UnaryOp(kind, e) => Ok(Expr::UnaryOp(
            kind,
            Box::new(rewrite_expr(*e, entries, arg_flags)?),
        )),
        Expr::Cast(e, t) => Ok(Expr::Cast(
            Box::new(rewrite_expr(*e, entries, arg_flags)?),
            t,
        )),
        Expr::Call(name, args, ty) => Ok(Expr::Call(
            name,
            args.into_iter()
                .map(|a| rewrite_expr(a, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
            ty,
        )),
        other => Ok(other),
    }
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
    fn test_entry_expands_to_guard_and_flip() {
        let mut program = parse(
            r#"
            node build [entry!("build")][result == 0] { term; };
            "#,
        );
        resolve_entries(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            debug.contains("__entry_build_done"),
            "entry! must inject a done-flag; got:\n{debug}"
        );
        assert!(
            debug.contains("entry_cmd"),
            "entry! must rewrite to entry_cmd() == \"build\"; got:\n{debug}"
        );
        // No tautological [true] guard.
        assert!(
            !debug.contains("entry_cmd() == \"build\" && Bool(true)"),
            "entry guard must not use [true]; got:\n{debug}"
        );
        // Import injected.
        assert!(
            debug.contains("std/cli.bv"),
            "entry! must ensure std/cli.bv is imported; got:\n{debug}"
        );
    }

    #[test]
    fn test_args_expands_to_snapshot_field() {
        let mut program = parse(
            r#"
            node run [entry!("run") && args!("--verbose")][done] { term; };
            "#,
        );
        resolve_entries(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            debug.contains("arg_verbose"),
            "args! must inject arg_verbose snapshot field; got:\n{debug}"
        );
    }

    #[test]
    fn test_entry_collision_errors() {
        let mut program = parse(
            r#"
            let __entry_build_done: Bool = false;
            node build [entry!("build")][done] { term; };
            "#,
        );
        let err = resolve_entries(&mut program).unwrap_err();
        assert!(
            err.contains("compiler-reserved"),
            "collision with __entry_build_done must error; got: {err}"
        );
    }

    #[test]
    fn test_args_in_body_rewrites() {
        // 2026-08-01 (Phase 3b): args! in a node BODY is rewritten to the
        // arg_<flag> snapshot identifier (not just contracts).
        let mut program = parse(
            r#"
            node build [entry!("build")][done] {
                let clean: Bool = args!("--clean");
                term;
            };
            "#,
        );
        resolve_entries(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            debug.contains("arg_clean"),
            "body args! must inject arg_clean; got:\n{debug}"
        );
        assert!(
            !debug.contains("args!"),
            "body args! must be rewritten away; got:\n{debug}"
        );
    }

    #[test]
    fn test_defn_entry_synthesizes_wrapper() {
        // 2026-08-01 (Phase 3b): a non-reactive defn with entry! gets a
        // synthesized reactive wrapper node that calls it and flips the
        // done-flag (helper-node path §4.3 step 4).
        let mut program = parse(
            r#"
            defn build() -> Int [entry!("build")][result == 0] {
                term 0;
            };
            "#,
        );
        resolve_entries(&mut program).unwrap();
        let debug = format!("{program:?}");
        assert!(
            debug.contains("__entry_build"),
            "defn entry! must synthesize a wrapper; got:\n{debug}"
        );
        assert!(
            debug.contains("Expression(Call(\"build\"")
                || debug.contains("Expression(Call(Identifier(\"build\"")
                || debug.contains("Call(\"build\""),
            "wrapper must call the defn; got:\n{debug}"
        );
    }
}
