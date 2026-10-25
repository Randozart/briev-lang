// ── Print Plugin — Front Stage ────────────────────────────────────────
// 2026-07-19: Resolves !Print(x) and !PrintLn(x) — today via the
// lowercase `print!`/`println!` macros to the `Print#` intrinsic, whose
// typed dispatch lives in the backend (emit_intrinsic_print → the plain
// cast-lane helpers print_int/print_str/print_float/put_char; renamed
// 2026-10-15, core-usable purge).
//
// Runs before typechecking and again after typing. The early implementation
// collected annotations and defaulted unknown values to integers. Since typed
// dispatch moved to Print#, that table was unused; 2026-10-22 removes it and
// uses the exhaustive mutable visitor for type-independent macro expansion.
//
// 2026-08-01: Phase 1 of the plugin-macro rework — the entry points are now
// the lowercase macros `print!` / `println!`. Two forms are supported:
//   - print!("literal {0} {1}", a, b) — Rust-style format string with
//     `{{`/`}}` escapes, `{}` sequential placeholders, and `{n}` positional
//     placeholders. Out-of-range `{n}` is a compile error; surplus trailing
//     arguments produce a warning (Rust-compatible).
//   - print!(value) / println!(value) — the legacy single-value form,
//     dispatch derived from the value's protocol category.
// `println!()` with no arguments prints a newline alone.
// The old PascalCase `Print!`/`PrintLn!` names are rejected by the
// typechecker with a rename hint (see src/typechecker/mod.rs).
//
// 2026-10-25: exhaustive postorder traversal adopted from Otto Westerlund
// (@odecode); see docs/contributors/otto-westerlund.md.

use crate::ast::{Expr, StageKind, Statement, TopLevel};
use crate::plugin::Plugin;
use crate::type_universe::TypeUniverse;

#[derive(Debug)]
pub struct PrintPlugin;

impl Plugin for PrintPlugin {
    fn name(&self) -> &str {
        "print"
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![StageKind::Parsed, StageKind::Typed]
    }

    fn on_ast(
        &self,
        program: &mut Vec<TopLevel>,
        _universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        resolve_prints(program)
    }
}

/// 2026-08-01: One segment of a parsed format string.
/// `Literal` is raw bytes ({{/}} escapes already unfolded);
/// `Next` is a sequential `{}` placeholder;
/// `Position(n)` is a positional `{n}` placeholder.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FmtPart {
    Literal(Vec<u8>),
    Next,
    Position(usize),
}

/// 2026-08-01: Parse a Rust-style format string into segments.
/// `{{` and `}}` are escapes for a literal brace; `{}` is a sequential
/// placeholder; `{n}` is a positional placeholder. Any other placeholder
/// body is a compile error. Shared with the interpreter so the reference
/// evaluator and the codegen path parse identically.
pub(crate) fn parse_format(fmt: &[u8]) -> Result<Vec<FmtPart>, String> {
    let mut parts = Vec::new();
    let mut literal = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        match fmt[i] {
            b'{' => {
                if i + 1 < fmt.len() && fmt[i + 1] == b'{' {
                    literal.push(b'{');
                    i += 2;
                    continue;
                }
                if !literal.is_empty() {
                    parts.push(FmtPart::Literal(std::mem::take(&mut literal)));
                }
                let mut j = i + 1;
                while j < fmt.len() && fmt[j] != b'}' {
                    j += 1;
                }
                if j >= fmt.len() {
                    return Err("unmatched '{' in format string".to_string());
                }
                let inner = &fmt[i + 1..j];
                i = j + 1;
                if inner.is_empty() {
                    parts.push(FmtPart::Next);
                } else {
                    let spec = std::str::from_utf8(inner).map_err(|_| {
                        format!("invalid placeholder in format string")
                    })?;
                    let idx = spec.parse::<usize>().map_err(|_| {
                        format!("invalid placeholder '{{{}}}' in format string", spec)
                    })?;
                    parts.push(FmtPart::Position(idx));
                }
            }
            b'}' => {
                if i + 1 < fmt.len() && fmt[i + 1] == b'}' {
                    literal.push(b'}');
                    i += 2;
                    continue;
                }
                return Err("unmatched '}' in format string".to_string());
            }
            c => {
                literal.push(c);
                i += 1;
            }
        }
    }
    if !literal.is_empty() {
        parts.push(FmtPart::Literal(literal));
    }
    Ok(parts)
}

// 2026-10-22: one exhaustive mutable postorder walk replaces partial print
// walkers. Foreach diagnostics and nested operand/container macros previously
// escaped expansion. Print# remains the sole typed dispatch authority.
fn resolve_prints(program: &mut [TopLevel]) -> Result<(), String> {
    let mut failure = None;
    let mut resolve = |expression: &mut Expr| {
        if failure.is_some() { return; }
        if let Err(error) = resolve_print_expr(expression) { failure = Some(error); }
    };
    for item in program {
        crate::ast::visit_mut::walk_toplevel(item, &mut resolve);
    }
    failure.map_or(Ok(()), Err)
}

fn resolve_print_expr(expression: &mut Expr) -> Result<(), String> {
    let Expr::PluginIntercept { name, args, receiver, .. } = expression else { return Ok(()); };
    // 2026-09-16 (Bug A), preserved: receiver precedes explicit arguments.
    // Postorder traversal has already expanded nested receiver/argument macros.
    let mut full_args = args.clone();
    if let Some(receiver) = receiver { full_args.insert(0, (**receiver).clone()); }
    if let Some(replacement) = resolve_print(name, &full_args)? { *expression = replacement; }
    Ok(())
}

fn print_call(value: &Expr) -> Expr {
    Expr::Call("Print#".to_string(), vec![value.clone()], None)
}

/// A newline-printing call — `Print#('\n')`, a Char literal routed through the
/// generic Print# convenience intrinsic (line termination is the macro's job;
/// the printing is the intrinsic's).
fn newline_call() -> Expr {
    Expr::Call("Print#".to_string(), vec![Expr::Char('\n')], None)
}

/// 2026-08-01: Expand a format string against its value arguments into a
/// sequence of print statements. Returns the statements and how many
/// arguments were consumed (for the surplus-argument warning).
fn expand_format(
    fmt: &[u8],
    args: &[Expr],
) -> Result<(Vec<Statement>, usize), String> {
    let parts = parse_format(fmt)?;
    let mut stmts = Vec::new();
    let mut next = 0usize;
    let mut max_used = 0usize;
    for part in &parts {
        match part {
            FmtPart::Literal(seg) => {
                if !seg.is_empty() {
                    stmts.push(Statement::Expression(Expr::Call(
                        "Print#".to_string(),
                        vec![Expr::Quoted(seg.clone())],
                        None,
                    )));
                }
            }
            FmtPart::Next => {
                if next >= args.len() {
                    return Err(format!(
                        "format string references argument {} but only {} were supplied",
                        next,
                        args.len()
                    ));
                }
                stmts.push(Statement::Expression(print_call(&args[next])));
                next += 1;
            }
            FmtPart::Position(n) => {
                if *n >= args.len() {
                    return Err(format!(
                        "format string argument {{{}}} is out of range ({} were supplied)",
                        n,
                        args.len()
                    ));
                }
                stmts.push(Statement::Expression(print_call(&args[*n])));
                max_used = max_used.max(*n + 1);
            }
        }
    }
    Ok((stmts, next.max(max_used)))
}

/// Resolve a `print!` or `println!` invocation to typed runtime calls.
/// Returns Ok(None) when `name` is not one of ours (e.g. a PascalCase
/// legacy name — the typechecker rejects those with a rename hint).
fn resolve_print(
    name: &str,
    args: &[Expr],
) -> Result<Option<Expr>, String> {
    let is_println = name == "println";
    if !is_println && name != "print" {
        return Ok(None);
    }

    // println!() — newline only.
    if args.is_empty() {
        if is_println {
            return Ok(Some(newline_call()));
        }
        return Err("print! requires a value or a format-string argument".to_string());
    }

    let mut stmts = Vec::new();
    if let Expr::Quoted(fmt) = &args[0] {
        // Rust-style format string: value arguments follow the literal.
        let (mut parts, used) = expand_format(fmt, &args[1..])?;
        let supplied = args.len() - 1;
        if supplied > used {
            eprintln!(
                "print plugin: warning: {} format-string argument(s) unused in {:?}",
                supplied - used,
                String::from_utf8_lossy(fmt)
            );
        }
        stmts.append(&mut parts);
    } else {
        // Legacy value form: every non-literal argument printed directly.
        // 2026-09-16 (Bug A): all args, not just the first — a chained
        // `obj.print!(x)` passes the receiver as args[0] and must print it too.
        for a in args {
            stmts.push(Statement::Expression(print_call(a)));
        }
    }

    if is_println {
        stmts.push(Statement::Expression(newline_call()));
    }

    // A lone call stands alone (preserves value/expression semantics);
    // multiple calls form a block.
    if stmts.len() == 1 {
        if let Some(Statement::Expression(e)) = stmts.pop() {
            return Ok(Some(e));
        }
    }
    Ok(Some(Expr::Block(stmts)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(source: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(source).expect("print fixture tokens");
        crate::parser::Parser::new(tokens, source).parse_program().expect("print fixture syntax")
    }

    #[test]
    fn nested_statement_prints_expand_without_intercepts() {
        let mut program = parsed(r#"
            defn diagnostics() {
                foreach item in 0..3 { when item > 0 { println!("item={}",item); }; };
                match 1 { 1 => { println!("one"); }, _ => { println!("other"); }, };
                let nested: Int = { println!("initializer"); 1; };
            };
            obj Console { defn show() { println!("member"); }; };
        "#);
        resolve_prints(&mut program).expect("expand all statement positions");
        let mut unresolved = 0;
        let mut calls = 0;
        for item in &program {
            crate::ast::visit::walk_toplevel(item, &mut |expression| match expression {
                Expr::PluginIntercept { .. } => unresolved += 1,
                Expr::Call(name, _, _) if name == "Print#" => calls += 1,
                _ => {}
            });
        }
        assert_eq!(unresolved, 0);
        assert!(calls >= 10, "each diagnostic retains its print and newline");
    }

    #[test]
    fn nested_unknown_plugin_arguments_still_expand_prints() {
        let mut program = parsed("defn nested() { custom!(println!(\"nested\")); };");
        resolve_prints(&mut program).expect("nested argument expansion");
        let mut names = Vec::new();
        for item in &program {
            crate::ast::visit::walk_toplevel(item, &mut |expression| {
                if let Expr::PluginIntercept { name, .. } = expression { names.push(name.clone()); }
            });
        }
        assert_eq!(names, ["custom"]);
    }

    #[test]
    fn exported_definition_prints_expand() {
        let mut program = parsed(r#"export defn say() { println!("exported"); };"#);
        resolve_prints(&mut program).expect("export body expansion");
        let mut unresolved = 0;
        for item in &program {
            crate::ast::visit::walk_toplevel(item, &mut |expression| {
                if let Expr::PluginIntercept { .. } = expression { unresolved += 1; }
            });
        }
        assert_eq!(unresolved, 0);
    }

    #[test]
    fn malformed_nested_format_is_an_error() {
        let mut program = parsed("defn nested() { foreach item in 0..2 { println!(\"{2}\",item); }; };");
        let error = resolve_prints(&mut program).expect_err("nested format error");
        assert!(error.contains("out of range"), "{error}");
    }

    #[test]
    fn parse_plain_literal() {
        assert_eq!(parse_format(b"hello").unwrap(), vec![FmtPart::Literal(b"hello".to_vec())]);
    }

    #[test]
    fn parse_sequential_placeholder() {
        assert_eq!(
            parse_format(b"x={}").unwrap(),
            vec![FmtPart::Literal(b"x=".to_vec()), FmtPart::Next]
        );
    }

    #[test]
    fn parse_positional_placeholder() {
        assert_eq!(
            parse_format(b"{1}:{0}").unwrap(),
            vec![FmtPart::Position(1), FmtPart::Literal(b":".to_vec()), FmtPart::Position(0)]
        );
    }

    #[test]
    fn parse_escaped_braces() {
        assert_eq!(
            parse_format(b"a{{b}}c").unwrap(),
            vec![FmtPart::Literal(b"a{b}c".to_vec())]
        );
    }

    #[test]
    fn parse_unmatched_brace_errors() {
        assert!(parse_format(b"oops {").is_err());
        assert!(parse_format(b"oops }").is_err());
    }

    #[test]
    fn parse_bad_placeholder_errors() {
        assert!(parse_format(b"{abc}").is_err());
        assert!(parse_format(b"{-1}").is_err());
    }

    #[test]
    fn expand_out_of_range_positional_errors() {
        let err = expand_format(b"{1}", &[]).unwrap_err();
        assert!(err.contains("out of range"), "err was: {err}");
    }

    #[test]
    fn expand_too_few_sequential_errors() {
        let err = expand_format(b"{}", &[]).unwrap_err();
        assert!(err.contains("only 0 were supplied"), "err was: {err}");
    }

    #[test]
    fn expand_value_kinds() {
        let (stmts, used) = expand_format(b"{}, {1}, {0}", &[Expr::Decimal(7), Expr::Float(1.5)]).unwrap();
        // {} -> Print#(7), ", " -> literal, {1} -> Print#(1.5),
        // ", " -> literal, {0} -> Print#(1.5).
        assert_eq!(stmts.len(), 5);
        assert_eq!(used, 2);
        match &stmts[0] {
            Statement::Expression(Expr::Call(name, args, _)) => {
                assert_eq!(name, "Print#");
                assert_eq!(args[0], Expr::Decimal(7));
            }
            other => panic!("expected call, got {other:?}"),
        }
        match &stmts[1] {
            Statement::Expression(Expr::Call(name, args, _)) => {
                assert_eq!(name, "Print#");
                assert_eq!(args[0], Expr::Quoted(b", ".to_vec()));
            }
            other => panic!("expected call, got {other:?}"),
        }
        match &stmts[2] {
            Statement::Expression(Expr::Call(name, args, _)) => {
                assert_eq!(name, "Print#");
                assert_eq!(args[0], Expr::Float(1.5));
            }
            other => panic!("expected call, got {other:?}"),
        }
        match &stmts[4] {
            Statement::Expression(Expr::Call(name, args, _)) => {
                assert_eq!(name, "Print#");
                assert_eq!(args[0], Expr::Decimal(7));
            }
            other => panic!("expected call, got {other:?}"),
        }
    }
}
