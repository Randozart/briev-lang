// ── IR-stage plugin evaluation (Generated / Optimized) ─────────────────
// 2026-11-10: `$(Generated)` / `$(Optimized)` blocks operate on the backend
// IR TEXT, mirroring the live-AST stage blocks. Before this the
// StageBlockPlugin `on_ir` was a no-op, so the documented `Find$` /
// `ReplaceWith$` surface silently did nothing.
//
// Supported shapes (one statement per line in the block):
//   Find$("needle").ReplaceWith$("text");
//   Find$("needle").InsertBefore$("text"); / .InsertAfter$("text"); / .Delete$();
//   Find$("needle").First$().Count$();        (query values)
//   Ir$.Find$("needle").Count$();
//   let s = Find$("needle"); s.InsertBefore$("text");
// Unsupported statements are an ERROR (never a silent no-op).

use crate::ast::{Expr, Statement};
use crate::macros::text_ops::TextSelection;
use std::collections::HashMap;

#[derive(Debug, Clone)]
enum Value {
    Selection(TextSelection),
    Text(String),
    Count(usize),
}

type Env = HashMap<String, Value>;

/// Evaluate one IR-stage `$(Stage)` block against the emitted IR text.
pub fn evaluate_ir_block(body: &[Statement], ir: &mut String) -> Result<(), String> {
    let mut env = Env::new();
    for stmt in body {
        eval_stmt(ir, stmt, &mut env)?;
    }
    Ok(())
}

fn eval_stmt(ir: &mut String, stmt: &Statement, env: &mut Env) -> Result<(), String> {
    match stmt {
        Statement::Expression(expr) => {
            eval(ir, expr, env)?;
            Ok(())
        }
        Statement::Let { name, expr: Some(expr), .. } => {
            let value = eval(ir, expr, env)?;
            env.insert(name.clone(), value);
            Ok(())
        }
        // The block body is wrapped in a defn for parsing; its terminator is
        // not an IR operation.
        Statement::Term(_) | Statement::Tail(_) | Statement::EndProgram(_) => Ok(()),
        other => Err(format!(
            "IR plugin: unsupported statement ({:?}); use Find$/ReplaceWith$/InsertBefore$/InsertAfter$/Delete$/First$/Last$/Nth$/Count$/Text$ chains",
            std::mem::discriminant(other)
        )),
    }
}

fn eval(ir: &mut String, expr: &Expr, env: &mut Env) -> Result<Value, String> {
    match expr {
        Expr::Quoted(bytes) => Ok(Value::Text(String::from_utf8_lossy(bytes).into_owned())),
        Expr::Decimal(n) => Ok(Value::Count(*n as usize)),
        Expr::Identifier(name) if name == "Ir$" => Ok(Value::Text(ir.clone())),
        Expr::Identifier(name) => env
            .get(name)
            .cloned()
            .ok_or_else(|| format!("IR plugin: unknown binding '{name}'")),
        Expr::Call(name, args, _) => eval_call(ir, name, args, env),
        Expr::MethodCall(receiver, name, args, _) => eval_method(ir, receiver, name, args, env),
        other => Err(format!(
            "IR plugin: unsupported expression '{other}'; use string literals and Find$/ReplaceWith$ chains"
        )),
    }
}

fn eval_call(ir: &mut String, name: &str, args: &[Expr], env: &mut Env) -> Result<Value, String> {
    match name {
        "Find$" => Ok(Value::Selection(text_selection(ir, args, env, "Find$")?)),
        "EmitInfo$" => {
            eprintln!("{}", string_arg(args, env, "EmitInfo$")?);
            Ok(Value::Count(0))
        }
        other => Err(format!("IR plugin: unsupported call '{other}$'")),
    }
}

fn eval_method(
    ir: &mut String,
    receiver: &Expr,
    name: &str,
    args: &[Expr],
    env: &mut Env,
) -> Result<Value, String> {
    // `Ir$.Find$("x")` — the receiver is the text buffer itself.
    if matches!(receiver, Expr::Identifier(id) if id == "Ir$") {
        return eval_call(ir, name, args, env);
    }
    let value = eval(ir, receiver, env)?;
    let selection = match value {
        Value::Selection(sel) => sel,
        other => {
            return Err(format!(
                "IR plugin: '{name}' needs a Find$ selection, got {other:?}"
            ))
        }
    };
    match name {
        "First$" => Ok(Value::Selection(selection.first(1))),
        "Last$" => Ok(Value::Selection(selection.last(1))),
        "Nth$" => {
            let Some(Expr::Decimal(n)) = args.first() else {
                return Err("IR plugin: Nth$ requires a numeric literal".into());
            };
            Ok(Value::Selection(selection.nth(*n as usize)))
        }
        "Count$" => Ok(Value::Count(selection.count())),
        "Text$" => Ok(Value::Text(selection_text(ir, &selection))),
        "ReplaceWith$" => {
            let replacement = string_arg(args, env, "ReplaceWith$")?;
            selection.replace_with(ir, &replacement);
            Ok(Value::Count(selection.count()))
        }
        "InsertBefore$" => {
            let content = string_arg(args, env, "InsertBefore$")?;
            selection.insert_before(ir, &content);
            Ok(Value::Count(selection.count()))
        }
        "InsertAfter$" => {
            let content = string_arg(args, env, "InsertAfter$")?;
            selection.insert_after(ir, &content);
            Ok(Value::Count(selection.count()))
        }
        "Delete$" => {
            selection.delete(ir);
            Ok(Value::Count(selection.count()))
        }
        other => Err(format!("IR plugin: unsupported method '{other}$'")),
    }
}

fn text_selection(ir: &str, args: &[Expr], env: &mut Env, op: &str) -> Result<TextSelection, String> {
    let needle = string_arg(args, env, op)?;
    Ok(TextSelection::find(ir, &needle))
}

fn string_arg(args: &[Expr], env: &mut Env, op: &str) -> Result<String, String> {
    let Some(arg) = args.first() else {
        return Err(format!("IR plugin: {op} requires a string argument"));
    };
    match arg {
        Expr::Quoted(bytes) => Ok(String::from_utf8_lossy(bytes).into_owned()),
        Expr::Identifier(name) => match env.get(name) {
            Some(Value::Text(text)) => Ok(text.clone()),
            _ => Err(format!("IR plugin: '{op}' argument '{name}' is not text")),
        },
        other => Err(format!("IR plugin: '{op}' requires a string argument, got '{other}'")),
    }
}

fn selection_text(ir: &str, selection: &TextSelection) -> String {
    let mut out = String::new();
    for (start, end) in &selection.ranges {
        out.push_str(&ir[*start..*end]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(src: &str) -> Vec<Statement> {
        let wrapped = format!("defn __ir_plugin__() -> Int {{\n{src}\nterm 0;\n}};");
        let tokens = crate::lexer::tokenize(&wrapped).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, &wrapped);
        let items = parser.parse_program().expect("parse");
        items
            .into_iter()
            .find_map(|item| match item {
                crate::ast::TopLevel::Definition(def) => Some(def.body.clone()),
                _ => None,
            })
            .expect("defn body")
    }

    #[test]
    fn find_replace_and_query() {
        let mut ir = "target triple = \"x86_64\"\n".to_string();
        let body = block(r#"Find$("x86_64").ReplaceWith$("riscv64");"#);
        evaluate_ir_block(&body, &mut ir).unwrap();
        assert!(ir.contains("riscv64"));
        assert!(!ir.contains("x86_64"));
    }

    #[test]
    fn insert_before_and_after() {
        let mut ir = "middle".to_string();
        evaluate_ir_block(&block(r#"Find$("middle").InsertBefore$("a");"#), &mut ir).unwrap();
        evaluate_ir_block(&block(r#"Find$("middle").InsertAfter$("b");"#), &mut ir).unwrap();
        assert_eq!(ir, "amiddleb");
    }

    #[test]
    fn let_bound_selection_and_count() {
        let mut ir = "a a a".to_string();
        let body = block(r#"let s = Find$("a"); s.ReplaceWith$("b");"#);
        evaluate_ir_block(&body, &mut ir).unwrap();
        assert_eq!(ir, "b b b");
    }

    #[test]
    fn unsupported_statement_is_an_error() {
        let mut ir = "x".to_string();
        let body = block(r#"when true { Find$("x").Delete$(); };"#);
        assert!(evaluate_ir_block(&body, &mut ir).is_err());
    }
}
