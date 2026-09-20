//! Expression-parameterized composites (Front A, plan
//! 2026-09-20-metaprogrammed-composites).
//!
//! A `$defn` whose parameters are typed `expr` is a DECLARED COMPOSITE:
//! its body is the canonical kernel structure, its parameters are
//! expressions, and every `name!(args...)` call site expands — before
//! typecheck — into the substituted body wrapped with the composite's
//! contract gates. One IR afterwards: typecheck, contracts, accel
//! analysis, and both backends see the expanded body as if hand-written.
//!
//! Doctrine (Golden Rule 24): composites live in stdlib `.bv` files — the
//! compiler carries the EXPANSION MACHINERY (this module), never a
//! specific composite. Hygiene fails closed: a caller identifier that a
//! body binder would capture is an error, not a silent capture.

use crate::ast::top::{Definition, Statement, TopLevel};
use crate::ast::Expr;
use crate::plugin::{FnDef, PluginManager};
use std::collections::{HashMap, HashSet};

/// True when this `$defn` declares at least one `expr` parameter — the
/// composite marker. Ordinary `$defn`s (value parameters) are executed by
/// the macro evaluator, never expanded.
pub fn is_composite(def: &Definition) -> bool {
    def.parameters
        .iter()
        .any(|(_, t)| matches!(t, crate::ast::Type::Custom(n) if n == "expr"))
}

/// The composite's expression-parameter names, in declaration order.
/// `None` when the composite mixes `expr` and value parameters (v1
/// restriction — fail closed).
fn expr_params(def: &Definition) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for (n, t) in &def.parameters {
        match t {
            crate::ast::Type::Custom(k) if k == "expr" => names.push(n.clone()),
            _ => return None,
        }
    }
    Some(names)
}

/// Expand one `name!(args...)` invocation against its declared composite:
/// clone the body, substitute every parameter identifier with its
/// argument expression (one pass — inserted arguments are never
/// re-substituted), wrap with the composite's `[pre]` / `[post]` gates so
/// every instantiation is proof-checked through the normal contract path.
pub fn expand_composite_invocation(
    def: &Definition,
    args: &[Expr],
) -> Result<Vec<Statement>, String> {
    let name = &def.name;
    let Some(params) = expr_params(def) else {
        return Err(format!(
            "composite '{name}' mixes `expr` and value parameters — declare all \
             parameters as `name: expr` (v1 supports expression parameters only)"
        ));
    };
    if args.len() != params.len() {
        return Err(format!(
            "composite '{name}' expects {} expression arguments ({}), got {} — \
             supply one expression per `expr` parameter",
            params.len(),
            params.join(", "),
            args.len()
        ));
    }
    // Hygiene: identifiers the caller's arguments carry must not collide
    // with names the body binds, and must not reference the composite's
    // own parameters. Either case would silently change meaning.
    let mut binders: HashSet<String> = HashSet::new();
    for s in &def.body {
        collect_binders(s, &mut binders);
    }
    for p in &params {
        binders.remove(p);
    }
    for (param, arg) in params.iter().zip(args) {
        let mut idents = HashSet::new();
        collect_idents(arg, &mut idents);
        for id in &idents {
            if binders.contains(id) {
                return Err(format!(
                    "composite '{name}': argument for '{param}' mentions '{id}', \
                     which the composite body binds — capture would silently \
                     change meaning; rename the caller's '{id}' or the \
                     composite's binder"
                ));
            }
            if params.contains(id) {
                return Err(format!(
                    "composite '{name}': argument for '{param}' references \
                     parameter '{id}' — a parameter is not visible inside \
                     another argument; inline the intended expression"
                ));
            }
        }
    }
    let mut out: Vec<Statement> = Vec::new();
    let trivial = Expr::Bool(true);
    if def.contract.pre_condition != trivial {
        out.push(Statement::Gate(def.contract.pre_condition.clone()));
    }
    for s in &def.body {
        let mut cloned = s.clone();
        for (param, arg) in params.iter().zip(args) {
            substitute_param(&mut cloned, param, arg);
        }
        out.push(cloned);
    }
    if def.contract.post_condition != trivial {
        out.push(Statement::Gate(def.contract.post_condition.clone()));
    }
    Ok(out)
}

/// Driver: expand every statement-position `composite!(...)` in the
/// program whose name resolves to a declared composite. Runs to fixpoint
/// (composites may invoke composites) with a depth cap. Returns the
/// number of expansions performed. Unknown names are left for the
/// typechecker to diagnose.
pub fn expand_composites(
    items: &mut Vec<TopLevel>,
    pm: &PluginManager,
) -> Result<usize, String> {
    let registry: HashMap<&String, &Definition> = pm
        .fn_registry
        .iter()
        .filter_map(|(n, f)| match f {
            FnDef::Defn(d) if is_composite(d) => Some((n, d)),
            _ => None,
        })
        .collect();
    if registry.is_empty() {
        return Ok(0);
    }
    let mut total = 0;
    for depth in 0..8 {
        let mut n = 0;
        for item in items.iter_mut() {
            match item {
                TopLevel::Transaction(t) => {
                    n += expand_stmt_list(&mut t.body, &registry)?;
                }
                TopLevel::CompileTimeDefn(d) => {
                    n += expand_stmt_list(&mut d.body, &registry)?;
                }
                _ => {}
            }
        }
        total += n;
        if n == 0 {
            return Ok(total);
        }
        let _ = depth;
    }
    Err(
        "composite expansion did not converge within 8 rounds — a composite \
         likely invokes itself (directly or through another composite)"
            .to_string(),
    )
}

/// Expand statement-position composite invocations in one statement list.
fn expand_stmt_list(
    stmts: &mut Vec<Statement>,
    registry: &HashMap<&String, &Definition>,
) -> Result<usize, String> {
    let mut n = 0;
    let mut i = 0;
    while i < stmts.len() {
        // Depth first — nested bodies expand before their parents, so an
        // enclosing splice never hides an inner call site.
        expand_nested(&mut stmts[i], registry)?;
        let call = match &stmts[i] {
            Statement::Expression(Expr::PluginIntercept {
                name,
                args,
                receiver: None,
                ..
            }) => Some((name.clone(), args.clone())),
            _ => None,
        };
        if let Some((name, args)) = call {
            if let Some(def) = registry.get(&name) {
                let expanded = expand_composite_invocation(def, &args)?;
                let before = stmts.len();
                let tail = stmts.split_off(i + 1);
                stmts.truncate(i);
                stmts.extend(expanded);
                stmts.extend(tail);
                let _ = before;
                n += 1;
                // Do not advance: the spliced body may itself end with a
                // nested list needing the walk (already handled above? no —
                // spliced statements were substituted but their NESTED
                // lists were not re-walked for invocations; the fixpoint
                // loop in expand_composites catches them).
            }
        }
        i += 1;
    }
    Ok(n)
}

/// Recurse into the statement kinds that carry nested statement lists.
fn expand_nested(
    s: &mut Statement,
    registry: &HashMap<&String, &Definition>,
) -> Result<(), String> {
    match s {
        Statement::Foreach { body, .. }
        | Statement::Guarded(_, body)
        | Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body) => {
            expand_stmt_list(body, registry)?;
        }
        Statement::Barrier { body, .. } => {
            expand_stmt_list(body, registry)?;
        }
        _ => {}
    }
    Ok(())
}

/// Substitute `param` → `arg` throughout one statement (one pass; the
/// inserted argument expressions are never re-walked).
fn substitute_param(s: &mut Statement, param: &str, arg: &Expr) {
    match s {
        Statement::Let { expr: Some(e), .. } => subst_expr(e, param, arg),
        Statement::Assign(l, r) => {
            subst_expr(l, param, arg);
            subst_expr(r, param, arg);
        }
        Statement::Expression(e)
        | Statement::Term(Some(e))
        | Statement::Check(e)
        | Statement::Gate(e)
        | Statement::Rollback(Some(e)) => subst_expr(e, param, arg),
        Statement::EndProgram(Some(e)) => subst_expr(e, param, arg),
        Statement::Foreach { list, body, .. } => {
            subst_expr(list, param, arg);
            for b in body.iter_mut() {
                substitute_param(b, param, arg);
            }
        }
        Statement::Guarded(cond, body) => {
            subst_expr(cond, param, arg);
            for b in body.iter_mut() {
                substitute_param(b, param, arg);
            }
        }
        Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body)
        | Statement::Barrier { body, .. } => {
            for b in body.iter_mut() {
                substitute_param(b, param, arg);
            }
        }
        _ => {}
    }
}

/// Replace `Identifier(param)` with `arg.clone()` in one expression tree.
fn subst_expr(e: &mut Expr, param: &str, arg: &Expr) {
    let hit = matches!(e, Expr::Identifier(n) if n == param);
    if hit {
        *e = arg.clone();
        return;
    }
    match e {
        Expr::Index(a, i) => {
            subst_expr(a, param, arg);
            subst_expr(i, param, arg);
        }
        Expr::BinaryOp(_, a, b) => {
            subst_expr(a, param, arg);
            subst_expr(b, param, arg);
        }
        Expr::UnaryOp(_, a) => subst_expr(a, param, arg),
        Expr::Call(_, args, _) => {
            for a in args.iter_mut() {
                subst_expr(a, param, arg);
            }
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            subst_expr(recv, param, arg);
            for a in args.iter_mut() {
                subst_expr(a, param, arg);
            }
        }
        Expr::Cast(a, _) | Expr::Deref(a) | Expr::AddrOf(a) | Expr::Consume(a)
        | Expr::Await(a) => subst_expr(a, param, arg),
        Expr::Range { start, end, .. } => {
            subst_expr(start, param, arg);
            subst_expr(end, param, arg);
        }
        Expr::Slice {
            array,
            start,
            end,
            stride,
        } => {
            subst_expr(array, param, arg);
            if let Some(x) = start.as_mut() {
                subst_expr(x, param, arg);
            }
            if let Some(x) = end.as_mut() {
                subst_expr(x, param, arg);
            }
            if let Some(x) = stride.as_mut() {
                subst_expr(x, param, arg);
            }
        }
        Expr::Field(a, _) | Expr::Reflect(a, _, _) => subst_expr(a, param, arg),
        Expr::Tuple(xs) | Expr::List(xs) => {
            for x in xs.iter_mut() {
                subst_expr(x, param, arg);
            }
        }
        Expr::Within(a, b) => {
            subst_expr(a, param, arg);
            subst_expr(b, param, arg);
        }
        _ => {}
    }
}

/// Names the statement tree binds (lets — including multi-lets — and
/// foreach items).
fn collect_binders(s: &Statement, out: &mut HashSet<String>) {
    match s {
        Statement::Let { name, names, .. } => {
            out.insert(name.clone());
            for n in names {
                out.insert(n.clone());
            }
        }
        Statement::Foreach { item, body, .. } => {
            out.insert(item.clone());
            for b in body {
                collect_binders(b, out);
            }
        }
        Statement::Guarded(_, body)
        | Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body)
        | Statement::Barrier { body, .. } => {
            for b in body {
                collect_binders(b, out);
            }
        }
        _ => {}
    }
}

/// Every identifier appearing anywhere in the expression (conservative:
/// lambda-bound names count as free — fail-closed for hygiene).
fn collect_idents(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Identifier(n) => {
            out.insert(n.clone());
        }
        Expr::Index(a, i) => {
            collect_idents(a, out);
            collect_idents(i, out);
        }
        Expr::BinaryOp(_, a, b) => {
            collect_idents(a, out);
            collect_idents(b, out);
        }
        Expr::UnaryOp(_, a) => collect_idents(a, out),
        Expr::Call(_, args, _) => {
            for a in args {
                collect_idents(a, out);
            }
        }
        Expr::MethodCall(recv, _, args, _, _) => {
            collect_idents(recv, out);
            for a in args {
                collect_idents(a, out);
            }
        }
        Expr::Cast(a, _) | Expr::Deref(a) | Expr::AddrOf(a) | Expr::Consume(a)
        | Expr::Await(a) => collect_idents(a, out),
        Expr::Range { start, end, .. } => {
            collect_idents(start, out);
            collect_idents(end, out);
        }
        Expr::Slice {
            array,
            start,
            end,
            stride,
        } => {
            collect_idents(array, out);
            if let Some(x) = start {
                collect_idents(x, out);
            }
            if let Some(x) = end {
                collect_idents(x, out);
            }
            if let Some(x) = stride {
                collect_idents(x, out);
            }
        }
        Expr::Field(a, _) | Expr::Reflect(a, _, _) => collect_idents(a, out),
        Expr::Tuple(xs) | Expr::List(xs) => {
            for x in xs {
                collect_idents(x, out);
            }
        }
        Expr::Within(a, b) => {
            collect_idents(a, out);
            collect_idents(b, out);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::Transaction;
    use crate::ast::{BinaryOpKind, Type};

    fn parse_program(src: &str) -> Vec<TopLevel> {
        let tokens = crate::pipeline::lex_for_path("test.bv", src).expect("lex");
        crate::pipeline::parse("test.bv", &tokens, src).expect("parse")
    }

    fn defn_of(items: &[TopLevel], name: &str) -> Definition {
        for it in items {
            if let TopLevel::CompileTimeDefn(d) = it {
                if d.name == name {
                    return d.clone();
                }
            }
        }
        panic!("no defn '{name}'");
    }

    #[test]
    fn expr_typed_params_are_composites() {
        let items = parse_program(
            "$defn f(x: expr, y: expr) { \n z = x + y; \n}; \nlet q: Int = 0;",
        );
        let d = defn_of(&items, "f");
        assert!(is_composite(&d), "`expr` params mark a composite");
        assert_eq!(
            expr_params(&d).unwrap(),
            vec!["x".to_string(), "y".to_string()]
        );
        let plain = defn_of(
            &parse_program("$defn g(v: Int) -> Int { v; };"),
            "g",
        );
        assert!(!is_composite(&plain), "value params are not a composite");
    }

    #[test]
    fn expansion_substitutes_nested_positions() {
        let items = parse_program(
            "$defn f(p: expr, q: expr) { \n\
             \x20 let t = p * 2; \n\
             \x20 buf[i] = t + q; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let arg_p = Expr::Index(
            Box::new(Expr::Identifier("src".into())),
            Box::new(Expr::Identifier("k".into())),
        );
        let arg_q = Expr::Call("Exp#".into(), vec![Expr::Identifier("w".into())], None);
        let out = expand_composite_invocation(&d, &[arg_p, arg_q]).expect("expands");
        assert_eq!(out.len(), 2, "no contract gates on a trivial contract");
        // `let t = src[k] * 2;`
        let Statement::Let { expr: Some(e), .. } = &out[0] else {
            panic!("let expected");
        };
        let dump = format!("{e:?}");
        assert!(dump.contains("\"src\"") && dump.contains("\"k\""), "{dump}");
        // `buf[i] = t + Exp#(w);`
        let Statement::Assign(_, rhs) = &out[1] else {
            panic!("assign expected");
        };
        let dump = format!("{rhs:?}");
        assert!(dump.contains("Exp#") && dump.contains("\"w\""), "{dump}");
        assert!(!dump.contains("\"q\""), "param q must be gone: {dump}");
    }

    #[test]
    fn hygiene_capture_is_rejected() {
        let items = parse_program(
            "$defn f(p: expr) { let t = p * 2; res = t; };",
        );
        let d = defn_of(&items, "f");
        // The arg mentions `t` — the body binds `t`. Capture => error.
        let arg = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("t".into())),
            Box::new(Expr::Decimal(1)),
        );
        let err = expand_composite_invocation(&d, &[arg]).unwrap_err();
        assert!(err.contains("captures") || err.contains("capture"), "{err}");
    }

    #[test]
    fn arg_referencing_another_param_is_rejected() {
        let items = parse_program("$defn f(p: expr, q: expr) { res = p + q; };");
        let d = defn_of(&items, "f");
        let arg_p = Expr::Decimal(1);
        let arg_q = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("p".into())),
            Box::new(Expr::Decimal(2)),
        );
        let err = expand_composite_invocation(&d, &[arg_p, arg_q]).unwrap_err();
        assert!(err.contains("parameter 'p'"), "{err}");
    }

    #[test]
    fn contract_gates_are_spliced() {
        let items = parse_program(
            "$defn f(p: expr) [i < 8] [i >= 0] { res = p; };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("v".into())],
        )
        .expect("expands");
        assert_eq!(out.len(), 3, "pre gate + body + post gate");
        assert!(matches!(&out[0], Statement::Gate(_)), "pre gate spliced");
        assert!(matches!(&out[2], Statement::Gate(_)), "post gate spliced");
    }

    #[test]
    fn driver_expands_call_sites_in_node_bodies() {
        let src = "\
$defn scale_into(dst: expr, srcv: expr) { dst = srcv * 2; };
let i: Int = 0;
let buf: Float[16];
let inp: Float[16];
async node k [i < 16][i == 16] {
    scale_into!(buf[i], inp[i]);
    i = i + 1;
    term;
};";
        let mut items = parse_program(src);
        let mut pm = PluginManager::new();
        crate::plugin::loader::extract_inline_stage_blocks(&mut items, &mut pm);
        let n = expand_composites(&mut items, &pm).expect("expansion runs");
        assert_eq!(n, 1, "one call site expanded");
        let TopLevel::Transaction(t) = &items
            .iter()
            .find(|it| matches!(it, TopLevel::Transaction(tx) if tx.name == "k"))
            .unwrap()
        else {
            panic!("node k");
        };
        let dump = format!("{:?}", t.body);
        assert!(!dump.contains("PluginIntercept"), "call site gone: {dump}");
        assert!(dump.contains("scale_into") == false, "defn stripped");
        assert!(dump.contains("Mul"), "substituted body present: {dump}");
    }

    #[test]
    fn mixed_params_fail_closed() {
        let items = parse_program(
            "$defn f(p: expr, n: Int) { res = p + n; };",
        );
        let d = defn_of(&items, "f");
        let err = expand_composite_invocation(
            &d,
            &[Expr::Decimal(1), Expr::Decimal(2)],
        )
        .unwrap_err();
        assert!(err.contains("mixes"), "{err}");
    }

    #[test]
    fn arg_count_mismatch_diagnoses_params() {
        let items = parse_program("$defn f(p: expr, q: expr) { res = p + q; };");
        let d = defn_of(&items, "f");
        let err =
            expand_composite_invocation(&d, &[Expr::Decimal(1)]).unwrap_err();
        assert!(err.contains("expects 2") && err.contains("p, q"), "{err}");
    }

    #[test]
    fn value_param_defn_is_never_expanded_by_driver() {
        let src = "\
$defn triple(v: Int) -> Int { v * 3; };
let i: Int = 0;
async node k [i < 1][i == 1] {
    i = i + 1;
    term;
};";
        let mut items = parse_program(src);
        let mut pm = PluginManager::new();
        crate::plugin::loader::extract_inline_stage_blocks(&mut items, &mut pm);
        let n = expand_composites(&mut items, &pm).expect("runs");
        assert_eq!(n, 0, "ordinary $defn calls are not composite expansions");
        let _ = Type::int();
    }
}
