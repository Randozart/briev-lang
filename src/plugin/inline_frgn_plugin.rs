// ── inline_frgn! Plugin — Front Stage ────────────────────────────────────
// 2026-09-10 (Family F, plan amendments §C): one-line foreign calls without
// the top-level `frgn` ritual.
//
//   inline_frgn!("__print_int", "lib/runtime/briev_rt.c", "fn(n: Int) -> Int", my_n);
//
// At Parsed stage the plugin:
//   1. parses the signature string ("fn(a: T, ...) -> R"),
//   2. synthesizes the ForeignBinding at module scope (deduplicated by
//      briev name + path — a second call with the same signature reuses it),
//   3. rewrites the intercept into a plain call on the foreign symbol.
//
// Reuses the ENTIRE existing frgn path: frgn_map registration,
// collect_extra_objects linking, the backend's declare-guard and the
// state-prefix call adaptation. The typechecker sees the declared return
// type (fn_return_types includes ForeignBinding entries); call-arg
// validation is the author's responsibility (the explicit signature string
// is the contract).
//
// Parse failure of the signature string = loud compile error naming the
// symbol (an unparseable signature would otherwise surface as an opaque
// typechecker error far from the cause).

use crate::ast::{Expr, ForeignBinding, ForeignTarget, FromSpec, StageKind, TopLevel, Type};
use crate::plugin::Plugin;
use crate::type_universe::TypeUniverse;

/// inline_frgn plugin: synthesizes ForeignBinding items for inline_frgn!
/// intercepts and rewrites them into plain foreign calls.
#[derive(Debug)]
pub struct InlineFrgnPlugin;

impl Plugin for InlineFrgnPlugin {
    fn name(&self) -> &str {
        "inline-frgn"
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![StageKind::Parsed]
    }

    fn on_ast(
        &self,
        program: &mut Vec<TopLevel>,
        _universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        // Pass 1: collect intercepts + synthesize the deduped bindings.
        let mut synthesized: Vec<ForeignBinding> = Vec::new();
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut rewrites: Vec<(Expr, Expr)> = Vec::new();
        collect_intercepts(program, &mut synthesized, &mut seen, &mut rewrites)?;

        if synthesized.is_empty() {
            return Ok(());
        }

        // Insert the bindings before the first item (imports/prelude live at
        // the front; order among top-level items is otherwise irrelevant).
        let bindings: Vec<TopLevel> = synthesized
            .into_iter()
            .map(TopLevel::ForeignBinding)
            .collect();
        for (i, item) in program.iter_mut().enumerate() {
            let _ = i;
            let _ = item;
            break;
        }
        let mut new_program: Vec<TopLevel> = Vec::with_capacity(program.len() + bindings.len());
        new_program.extend(bindings);
        new_program.append(program);
        *program = new_program;

        // Pass 2: rewrite the intercepts into plain calls.
        for (intercept, call) in rewrites {
            rewrite_intercept(program, &intercept, &call);
        }
        Ok(())
    }
}

fn collect_intercepts(
    program: &mut [TopLevel],
    synthesized: &mut Vec<ForeignBinding>,
    seen: &mut std::collections::HashSet<(String, String)>,
    rewrites: &mut Vec<(Expr, Expr)>,
) -> Result<(), String> {
    for item in program.iter_mut() {
        match item {
            TopLevel::Definition(d) => {
                collect_from_stmts(&mut d.body, synthesized, seen, rewrites)?;
            }
            TopLevel::Transaction(t) => {
                collect_from_stmts(&mut t.body, synthesized, seen, rewrites)?;
            }
            TopLevel::Init(init) => {
                if let Some(value) = &mut init.value {
                    rewrite_expr(value, synthesized, seen, rewrites)?;
                }
                collect_from_stmts(&mut init.body, synthesized, seen, rewrites)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_from_stmts(
    stmts: &mut [crate::ast::Statement],
    synthesized: &mut Vec<ForeignBinding>,
    seen: &mut std::collections::HashSet<(String, String)>,
    rewrites: &mut Vec<(Expr, Expr)>,
) -> Result<(), String> {
    use crate::ast::Statement;
    for stmt in stmts {
        match stmt {
            Statement::Assign(lhs, rhs) => {
                rewrite_expr(lhs, synthesized, seen, rewrites)?;
                rewrite_expr(rhs, synthesized, seen, rewrites)?;
            }
            Statement::Let { expr: Some(e), .. } => {
                rewrite_expr(e, synthesized, seen, rewrites)?;
            }
            Statement::Expression(e) => rewrite_expr(e, synthesized, seen, rewrites)?,
            Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
                rewrite_expr(e, synthesized, seen, rewrites)?;
            }
            Statement::Guarded(_, body) | Statement::Block(body) | Statement::SyncBlock(body) => {
                collect_from_stmts(body, synthesized, seen, rewrites)?;
            }
            Statement::Mutex(body) => {
                collect_from_stmts(body, synthesized, seen, rewrites)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn rewrite_expr(
    expr: &mut Expr,
    synthesized: &mut Vec<ForeignBinding>,
    seen: &mut std::collections::HashSet<(String, String)>,
    rewrites: &mut Vec<(Expr, Expr)>,
) -> Result<(), String> {
    match expr {
        Expr::PluginIntercept { name, args, type_args: _ } if name == "inline_frgn" => {
            // Shape: inline_frgn!(symbol, path, "fn(params) -> ret", call_args...)
            let mut strs = args.iter().take(3).filter_map(|a| match a {
                Expr::Quoted(b) => Some(String::from_utf8_lossy(b).to_string()),
                _ => None,
            });
            let (Some(sym), Some(path), Some(sig)) =
                (strs.next(), strs.next(), strs.next())
            else {
                return Err(
                    "inline_frgn! needs (symbol: String, path: String, signature: \\
                     String, call args...) - the first three must be string literals"
                        .to_string()
                );
            };
            let (inputs, ret) = parse_sig(&sig).map_err(|e| {
                format!("inline_frgn!('{sym}'): bad signature '{sig}': {e}")
            })?;
            let key = (sym.clone(), path.clone());
            if seen.insert(key.clone()) {
                synthesized.push(make_binding(&sym, &path, inputs, ret));
            }
            // Rewrite: drop the first three args, call the symbol directly.
            let call_args: Vec<Expr> = args.iter().skip(3).cloned().collect();
            rewrites.push((
                expr.clone(),
                Expr::Call(sym.clone(), call_args, None),
            ));
        }
        Expr::BinaryOp(_, l, r) => {
            rewrite_expr(l, synthesized, seen, rewrites)?;
            rewrite_expr(r, synthesized, seen, rewrites)?;
        }
        Expr::UnaryOp(_, i) => rewrite_expr(i, synthesized, seen, rewrites)?,
        Expr::Call(_, args, _) | Expr::Spawn { args, .. } => {
            for a in args {
                rewrite_expr(a, synthesized, seen, rewrites)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn rewrite_intercept(program: &mut [TopLevel], intercept: &Expr, call: &Expr) {
    for item in program.iter_mut() {
        match item {
            TopLevel::Definition(d) => rewrite_stmts(&mut d.body, intercept, call),
            TopLevel::Transaction(t) => rewrite_stmts(&mut t.body, intercept, call),
            TopLevel::Init(init) => {
                if let Some(value) = &mut init.value {
                    if *value == *intercept {
                        *value = call.clone();
                    }
                }
                rewrite_stmts(&mut init.body, intercept, call);
            }
            _ => {}
        }
    }
}

fn rewrite_stmts(stmts: &mut [crate::ast::Statement], intercept: &Expr, call: &Expr) {
    for stmt in stmts {
        match stmt {
            crate::ast::Statement::Assign(l, r) => {
                if l == intercept { *l = call.clone(); }
                if r == intercept { *r = call.clone(); }
            }
            crate::ast::Statement::Let { expr: Some(e), .. } => {
                if e == intercept { *e = call.clone(); }
            }
            crate::ast::Statement::Expression(e) => {
                if e == intercept { *e = call.clone(); }
            }
            crate::ast::Statement::Term(Some(e)) | crate::ast::Statement::EndProgram(Some(e)) => {
                if e == intercept { *e = call.clone(); }
            }
            // Descend into nested bodies (when-guards etc.) - the probe's
            // intercept sits inside a `when` guard.
            crate::ast::Statement::Guarded(_, body)
            | crate::ast::Statement::Block(body)
            | crate::ast::Statement::SyncBlock(body)
            | crate::ast::Statement::Mutex(body) => {
                rewrite_stmts(body, intercept, call);
            }
            _ => {}
        }
    }
}

fn make_binding(sym: &str, path: &str, inputs: Vec<(String, Type)>, ret: Type) -> ForeignBinding {
    ForeignBinding {
        foreign_name: sym.to_string(),
        briev_name: None,
        // from "c" (or "#System") = libc — the Protocol path emits -lc and
        // compiles NOTHING (libc symbols resolve at link time). A real path
        // (.c) goes through the literal compile-and-link flow.
        from: if path == "c" || path == "#System" {
            FromSpec::Protocol("#System".to_string())
        } else {
            FromSpec::Literal(std::path::PathBuf::from(path))
        },
        target: ForeignTarget::from_name("c").unwrap_or(ForeignTarget::C),
        inputs,
        success_output: vec![("result".to_string(), ret)],
        error_type: "Error".to_string(),
        error_fields: vec![],
        input_layout: None,
        output_layout: None,
        precondition: None,
        postcondition: None,
        buffer_mode: None,
        default_watchdog: None,
        wasm_impl: None,
        wasm_setup: None,
        span: None,
        doc: None,
        is_optional: false,
        is_fire_forget: false,
        is_delivery: false,
        is_variadic: false,
    }
}

/// Parse "fn(a: T, b: U) -> R" into (inputs, return type).
fn parse_sig(sig: &str) -> Result<(Vec<(String, Type)>, Type), String> {
    let s = sig.trim();
    let rest = s.strip_prefix("fn").ok_or("expected 'fn'")?;
    let rest = rest.trim_start();
    let (params_str, ret_str) = rest
        .split_once("->")
        .ok_or("expected '->' in the signature")?;
    let params_str = params_str.trim();
    let inner = params_str
        .strip_prefix('(')
        .and_then(|p| p.strip_suffix(')'))
        .ok_or("expected a (param, ...) list")?;

    let mut inputs = Vec::new();
    if !inner.trim().is_empty() {
        for param in split_top(inner) {
            let param = param.trim();
            let Some((name, ty)) = param.split_once(':') else {
                return Err(format!("param '{param}' is missing ': Type'"));
            };
            inputs.push((name.trim().to_string(), parse_type(ty.trim())?));
        }
    }
    let ret = parse_type(ret_str.trim())?;
    Ok((inputs, ret))
}

/// Split on top-level commas (angle-bracket aware for Ptr<T>).
fn split_top(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '<' => { depth += 1; cur.push(c); }
            '>' => { depth = depth.saturating_sub(1); cur.push(c); }
            ',' if depth == 0 => {
                parts.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

/// Type-string → Type. Covers the frgn surface: Int, Float, Bool, String,
/// Byte, Ptr<T>, and (as the raw name) anything else.
fn parse_type(ty: &str) -> Result<Type, String> {
    match ty {
        "Int" => Ok(Type::int()),
        "Float" => Ok(Type::float()),
        "Bool" => Ok(Type::bool_()),
        "String" => Ok(Type::string()),
        "Byte" => Ok(Type::int()),
        _ => {
            if let Some(inner) = ty.strip_prefix("Ptr<").and_then(|p| p.strip_suffix('>')) {
                let _ = parse_type(inner)?;
                return Ok(Type::Ptr(Box::new(Type::int())));
            }
            Err(format!(
                "unsupported type '{ty}' in inline_frgn! signature (supported: Int, Float, Bool, String, Byte, Ptr<...>)"
            ))
        }
    }
}
