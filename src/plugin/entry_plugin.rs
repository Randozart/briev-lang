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
// `args!("--flag")` becomes a snapshot state field bound from argv_has
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

    // 2026-10-12: the cli.bv import is injected UNCONDITIONALLY — it is the
    // PRELUDE ANCHOR. prelude-native's injection branches on `Tag$("import")`
    // matching; a unit with no user import and no txn (a bare `defn` file)
    // would otherwise get NO stdlib family at all (walker2-probe lesson: the
    // defn-only conformance corpus lost collections/cast_lanes/env and fell
    // to the Int fallback). cli.bv is deliberately import-free (see its
    // header: string.bv via cli.bv collided count_char_loop/len/to_string/
    // ends_with across the stdlib), so the anchor drags in nothing else.
    if !has_cli_import(program) {
        program.insert(0, TopLevel::Import(crate::ast::Import::literal("std/cli.bv", vec![])));
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
        // Snapshot initializer: argv_has("--flag") for Bool, argv_value
        // for String; Int/Float wrap the String value with the cli.bv parse
        // (arg_int/arg_float — the plugin targets stdlib, never Rust string
        // matches; Rule 3).
        let call = |name: &str| {
            Expr::Call(name.into(), vec![Expr::Quoted(flag.as_bytes().to_vec())], None)
        };
        let wrap1 = |name: &str, inner: Expr| {
            Expr::Call(name.into(), vec![inner], None)
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
        let init = match ty.as_deref() {
            None | Some("Bool") => call("argv_has"),
            Some("String") => call("argv_value"),
            Some("Int") => wrap1("arg_int", call("argv_value")),
            Some("Float") => wrap1("arg_float", call("argv_value")),
            Some(_) => unreachable!("field_ty match already rejected other types"),
        };
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
        | Statement::Tail(e)
        | Statement::EndProgram(Some(e)) => collect_from_expr(e, entry_commands, arg_flags),
        // 2026-10-12 (daily-driver quick wins): args! in an arrow-push
        // (`x <- args!("--flag")`) was never collected nor rewritten — the
        // raw intercept reached the typechecker ("expansion pass missed
        // this call position"). Collect + rewrite the arrow's value (and
        // target) like every other statement expression.
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                collect_from_expr(t, entry_commands, arg_flags);
            }
            collect_from_expr(value, entry_commands, arg_flags);
        }
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
        Expr::PluginIntercept { name, args, receiver, .. } => {
            collect_intercept(name, args, receiver.as_deref(), entry_commands, arg_flags);
        }
        Expr::Block(stmts) => {
            for s in stmts {
                collect_from_stmt(s, entry_commands, arg_flags);
            }
        }
        other => {
            for child in expr_child_refs(other) {
                collect_from_expr(child, entry_commands, arg_flags);
            }
        }
    }
}

/// The intercept itself (entry!/args! bookkeeping) PLUS a nested-intercept
/// walk of its args/receiver — `println!(args!("--name"))` hides the args!
/// intercept inside the outer intercept's arg list.
fn collect_intercept(
    name: &str,
    args: &[Expr],
    receiver: Option<&Expr>,
    entry_commands: &mut Vec<String>,
    arg_flags: &mut Vec<(String, Option<String>)>,
) {
    for a in args {
        collect_from_expr(a, entry_commands, arg_flags);
    }
    if let Some(r) = receiver {
        collect_from_expr(r, entry_commands, arg_flags);
    }
    match name {
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

/// Child expressions of every non-intercept, non-block shape. A missed
/// collect means the snapshot field is never created, so the rewritten
/// `arg_<flag>` identifier dies at typecheck with an unknown-name error.
fn expr_child_refs(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::BinaryOp(_, l, r) => vec![l, r],
        Expr::UnaryOp(_, e) | Expr::Cast(e, _) | Expr::Deref(e) | Expr::AddrOf(e)
        | Expr::Consume(e) | Expr::Await(e) | Expr::Field(e, _) | Expr::IsType(e, _)
        | Expr::Reflect(e, _, _) | Expr::Lambda(_, e) => vec![e],
        Expr::Call(_, args, _) | Expr::Spawn { args, .. } => args.iter().collect(),
        Expr::MethodCall(recv, _, args, _, _) => {
            args.iter().chain(std::iter::once(&**recv)).collect()
        }
        Expr::If(c, t, e) => {
            e.iter()
                .map(|b| &**b)
                .chain(std::iter::once(&**t))
                .chain(std::iter::once(&**c))
                .collect()
        }
        Expr::Match(scrutinee, arms) => {
            let mut out: Vec<&Expr> = vec![scrutinee];
            for arm in arms {
                if let Some(g) = &arm.guard {
                    out.push(g);
                }
                out.push(&arm.body);
            }
            out
        }
        Expr::Tuple(elems) | Expr::List(elems) => elems.iter().collect(),
        Expr::StructLiteral { fields, specs, .. } => {
            fields.iter().chain(specs.iter()).map(|(_, e)| e).collect()
        }
        Expr::Index(obj, idx) => vec![obj, idx],
        Expr::Slice { array, start, end, stride } => {
            [Some(&**array), start.as_deref(), end.as_deref(), stride.as_deref()]
                .into_iter()
                .flatten()
                .collect()
        }
        Expr::Range { start, end, .. } => vec![start, end],
        Expr::Within(a, b) => vec![a, b],
        _ => Vec::new(),
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
        variadic_param: None,
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
        Statement::Tail(e) => Statement::Tail(rewrite_expr(e, entries, arg_flags)?),
        Statement::EndProgram(Some(e)) => Statement::EndProgram(Some(rewrite_expr(e, entries, arg_flags)?)),
        // 2026-10-12 (daily-driver quick wins): the arrow-push arm — see
        // the collect-side note.
        Statement::ArrowAssign { target, value, consume } => Statement::ArrowAssign {
            target: target
                .map(|t| rewrite_expr(*t, entries, arg_flags).map(Box::new))
                .transpose()?,
            value: Box::new(rewrite_expr(*value, entries, arg_flags)?),
            consume,
        },
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
    if matches!(expr, Expr::PluginIntercept { .. }) {
        return rewrite_intercept(expr, entries, arg_flags);
    }
    if is_compound_shape(&expr) {
        return rewrite_expr_compound(expr, entries, arg_flags);
    }
    rewrite_expr_simple(expr, entries, arg_flags)
}

/// Statement-carrying / multi-element shapes. The PrintPlugin runs FIRST at
/// Parsed and rewrites `println!(args!("--x"))` into an Expr::Block of
/// Print# calls — the old fallthrough dropped Block (and every other
/// compound shape), so a nested args! intercept survived to codegen (panic
/// at emit_expr:1859). Statement-carrying shapes delegate to rewrite_stmt;
/// element lists map rewrite_expr over their items.
fn is_compound_shape(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Block(_)
            | Expr::If(_, _, _)
            | Expr::Match(_, _)
            | Expr::Tuple(_)
            | Expr::List(_)
            | Expr::StructLiteral { .. }
            | Expr::Lambda(_, _)
            | Expr::Slice { .. }
            | Expr::Range { .. }
            | Expr::Spawn { .. }
    )
}

/// entry!/args!/unknown intercepts. The receiver is prepended as the first
/// argument for entry!/args! only (UFCS chaining, 2026-09-16 Bug A); unknown
/// intercept names keep the receiver in its own field and walk their args.
fn rewrite_intercept(
    expr: Expr,
    entries: &mut Vec<String>,
    arg_flags: &[(String, Option<String>)],
) -> Result<Expr, String> {
    let Expr::PluginIntercept { name, args, type_args, receiver, chain_refs } = expr else {
        return Ok(expr);
    };
    let mut full_args = args.clone();
    if let Some(recv) = receiver.as_ref() {
        full_args.insert(0, rewrite_expr((**recv).clone(), entries, arg_flags)?);
    }
    match name.as_str() {
        "entry" => {
            let cmd = match full_args.first() {
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
            let flag = match full_args.first() {
                Some(Expr::Quoted(f)) => String::from_utf8_lossy(f).to_string(),
                _ => return Err("args!: expected a string literal flag".into()),
            };
            let _ = type_args;
            // Typed form: args!("--flag", T) → arg_<flag> (T-typed snapshot).
            // The field type is resolved from the second arg if present.
            let ty = full_args.get(1).and_then(|a| match a {
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
        _ => Ok(Expr::PluginIntercept {
            name,
            args: args
                .into_iter()
                .map(|a| rewrite_expr(a, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
            type_args,
            receiver: receiver
                .map(|r| rewrite_expr(*r, entries, arg_flags).map(Box::new))
                .transpose()?,
            chain_refs,
        }),
    }
}

fn rewrite_expr_compound(
    expr: Expr,
    entries: &mut Vec<String>,
    arg_flags: &[(String, Option<String>)],
) -> Result<Expr, String> {
    Ok(match expr {
        Expr::Block(stmts) => Expr::Block(
            stmts
                .into_iter()
                .map(|s| rewrite_stmt(s, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::If(c, t, e) => Expr::If(
            Box::new(rewrite_expr(*c, entries, arg_flags)?),
            Box::new(rewrite_expr(*t, entries, arg_flags)?),
            e.map(|el| rewrite_expr(*el, entries, arg_flags).map(Box::new))
                .transpose()?,
        ),
        Expr::Match(scrutinee, arms) => Expr::Match(
            Box::new(rewrite_expr(*scrutinee, entries, arg_flags)?),
            arms.into_iter()
                .map(|arm| -> Result<crate::ast::MatchArm, String> {
                    Ok(crate::ast::MatchArm {
                        pattern: arm.pattern,
                        guard: arm
                            .guard
                            .map(|g| rewrite_expr(g, entries, arg_flags))
                            .transpose()?,
                        body: Box::new(rewrite_expr(*arm.body, entries, arg_flags)?),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::Tuple(elems) => Expr::Tuple(
            elems
                .into_iter()
                .map(|e| rewrite_expr(e, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::List(elems) => Expr::List(
            elems
                .into_iter()
                .map(|e| rewrite_expr(e, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expr::StructLiteral { type_name, fields, specs } => Expr::StructLiteral {
            type_name,
            fields: fields
                .into_iter()
                .map(|(n, e)| -> Result<(String, Expr), String> {
                    Ok((n, rewrite_expr(e, entries, arg_flags)?))
                })
                .collect::<Result<Vec<_>, _>>()?,
            specs: specs
                .into_iter()
                .map(|(n, e)| -> Result<(String, Expr), String> {
                    Ok((n, rewrite_expr(e, entries, arg_flags)?))
                })
                .collect::<Result<Vec<_>, _>>()?,
        },
        Expr::Lambda(params, body) => Expr::Lambda(
            params,
            Box::new(rewrite_expr(*body, entries, arg_flags)?),
        ),
        Expr::Slice { array, start, end, stride } => Expr::Slice {
            array: Box::new(rewrite_expr(*array, entries, arg_flags)?),
            start: start
                .map(|s| rewrite_expr(*s, entries, arg_flags).map(Box::new))
                .transpose()?,
            end: end
                .map(|e| rewrite_expr(*e, entries, arg_flags).map(Box::new))
                .transpose()?,
            stride: stride
                .map(|s| rewrite_expr(*s, entries, arg_flags).map(Box::new))
                .transpose()?,
        },
        Expr::Range { start, end, inclusive } => Expr::Range {
            start: Box::new(rewrite_expr(*start, entries, arg_flags)?),
            end: Box::new(rewrite_expr(*end, entries, arg_flags)?),
            inclusive,
        },
        Expr::Spawn { type_name, args, storage } => Expr::Spawn {
            type_name,
            args: args
                .into_iter()
                .map(|a| rewrite_expr(a, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
            storage,
        },
        other => return rewrite_expr_simple(other, entries, arg_flags),
    })
}

fn rewrite_expr_simple(
    expr: Expr,
    entries: &mut Vec<String>,
    arg_flags: &[(String, Option<String>)],
) -> Result<Expr, String> {
    Ok(match expr {
        Expr::BinaryOp(kind, l, r) => Expr::BinaryOp(
            kind,
            Box::new(rewrite_expr(*l, entries, arg_flags)?),
            Box::new(rewrite_expr(*r, entries, arg_flags)?),
        ),
        Expr::UnaryOp(kind, e) => {
            Expr::UnaryOp(kind, Box::new(rewrite_expr(*e, entries, arg_flags)?))
        }
        Expr::Cast(e, t) => Expr::Cast(
            Box::new(rewrite_expr(*e, entries, arg_flags)?),
            t,
        ),
        Expr::Call(name, args, ty) => Expr::Call(
            name,
            args.into_iter()
                .map(|a| rewrite_expr(a, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
            ty,
        ),
        Expr::MethodCall(recv, name, args, aid, chain_refs) => Expr::MethodCall(
            Box::new(rewrite_expr(*recv, entries, arg_flags)?),
            name,
            args.into_iter()
                .map(|a| rewrite_expr(a, entries, arg_flags))
                .collect::<Result<Vec<_>, _>>()?,
            aid,
            chain_refs,
        ),
        Expr::Field(obj, name) => Expr::Field(
            Box::new(rewrite_expr(*obj, entries, arg_flags)?),
            name,
        ),
        Expr::Reflect(obj, name, kind) => Expr::Reflect(
            Box::new(rewrite_expr(*obj, entries, arg_flags)?),
            name,
            kind,
        ),
        Expr::IsType(inner, ty) => Expr::IsType(
            Box::new(rewrite_expr(*inner, entries, arg_flags)?),
            ty,
        ),
        Expr::Within(a, b) => Expr::Within(
            Box::new(rewrite_expr(*a, entries, arg_flags)?),
            Box::new(rewrite_expr(*b, entries, arg_flags)?),
        ),
        Expr::Index(obj, idx) => Expr::Index(
            Box::new(rewrite_expr(*obj, entries, arg_flags)?),
            Box::new(rewrite_expr(*idx, entries, arg_flags)?),
        ),
        Expr::Deref(inner) => {
            Expr::Deref(Box::new(rewrite_expr(*inner, entries, arg_flags)?))
        }
        Expr::AddrOf(inner) => {
            Expr::AddrOf(Box::new(rewrite_expr(*inner, entries, arg_flags)?))
        }
        Expr::Consume(inner) => {
            Expr::Consume(Box::new(rewrite_expr(*inner, entries, arg_flags)?))
        }
        Expr::Await(inner) => {
            Expr::Await(Box::new(rewrite_expr(*inner, entries, arg_flags)?))
        }
        other => other,
    })
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

