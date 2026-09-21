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

use crate::ast::top::{Definition, Statement, StmtMatchArm, TopLevel};
use crate::ast::{BinaryOpKind, Expr, Pattern, UnaryOpKind};
use crate::plugin::{FnDef, PluginManager};
use std::collections::{HashMap, HashSet};

/// True when this `$defn` declares composite parameters — `expr`
/// (substituted expression) or `expr_item` (an exposed binder: the body
/// binds a loop/item with this name and arguments MAY reference it).
/// Ordinary `$defn`s (value parameters) are executed by the macro
/// evaluator, never expanded.
pub fn is_composite(def: &Definition) -> bool {
    def.parameters.iter().any(|(_, t)| is_expr_param(t))
}

fn is_expr_param(t: &crate::ast::Type) -> bool {
    matches!(t, crate::ast::Type::Custom(n) if n == "expr" || n == "expr_item")
}

/// The composite's signature: (substitution parameters in declaration
/// order, exposed binder names). `None` when the composite mixes in value
/// parameters (v1 restriction — fail closed).
fn composite_signature(def: &Definition) -> Option<(Vec<String>, HashSet<String>)> {
    let mut subst = Vec::new();
    let mut exposed: HashSet<String> = HashSet::new();
    for (n, t) in &def.parameters {
        match t {
            crate::ast::Type::Custom(k) if k == "expr" => subst.push(n.clone()),
            crate::ast::Type::Custom(k) if k == "expr_item" => {
                exposed.insert(n.clone());
            }
            _ => return None,
        }
    }
    Some((subst, exposed))
}

/// Expand one `name!(args...)` invocation against its declared composite:
/// clone the body, substitute every parameter identifier with its
/// argument expression (one pass — inserted arguments are never
/// re-substituted), wrap with the composite's `[pre]` / `[post]` gates so
/// every instantiation is proof-checked through the normal contract path,
/// then comptime-fold the substituted body (shape-adaptive expansion, plan
/// 2026-09-21-comptime-fold-expansion). `comptime` seeds the fold env with
/// the program's `$let`/`$const` values so a span argument may be a named
/// comptime constant.
pub fn expand_composite_invocation(
    def: &Definition,
    args: &[Expr],
    comptime: &HashMap<String, ComptimeVal>,
) -> Result<Vec<Statement>, String> {
    let name = &def.name;
    let Some((params, exposed)) = composite_signature(def) else {
        return Err(format!(
            "composite '{name}' mixes `expr`/`expr_item` and value parameters — \
             declare all parameters as `name: expr` or `name: expr_item` (v1 \
             supports expression parameters only)"
        ));
    };
    if args.len() != params.len() {
        return Err(format!(
            "composite '{name}' expects {} expression arguments ({}), got {} — \
             supply one expression per `expr` parameter (`expr_item` binders \
             are bound by the body, never passed)",
            params.len(),
            params.join(", "),
            args.len()
        ));
    }
    check_hygiene(def, &params, &exposed, args)?;
    let trivial = Expr::Bool(true);
    let mut out: Vec<Statement> = Vec::new();
    if def.contract.pre_condition != trivial {
        out.push(Statement::Gate(def.contract.pre_condition.clone()));
    }
    let mut env: HashMap<String, ComptimeVal> = comptime.clone();
    let mut body_stmts: Vec<Statement> = Vec::new();
    for s in &def.body {
        let mut cloned = s.clone();
        for (param, arg) in params.iter().zip(args) {
            substitute_param(&mut cloned, param, arg);
        }
        body_stmts.push(cloned);
    }
    out.extend(fold_stmt_list(body_stmts, &mut env));
    if def.contract.post_condition != trivial {
        out.push(Statement::Gate(def.contract.post_condition.clone()));
    }
    Ok(out)
}

// ── Comptime fold (plan 2026-09-21-comptime-fold-expansion) ────────────
//
// The language is the metaprogramming layer: a composite body may contain
// ordinary `match` / `when` whose scrutinee is comptime-known AFTER
// substitution (literal spans, `$let`/`$const` names, arithmetic over
// them). Expansion evaluates the scrutinee and splices only the taken
// future; a scrutinee that is NOT comptime-known stays an ordinary runtime
// conditional — identical semantics to hand-written code, just not
// specialized (fail-open; never an error). Zero new syntax, zero compiler
// knowledge of any algorithm: the shape policy lives in the `.bv` file
// that declares the composite.

/// A comptime-known scalar during folding (the foldable subset of Briev's
/// constant expressions — Int/Float/Bool literals and arithmetic).
#[derive(Clone, Debug, PartialEq)]
pub enum ComptimeVal {
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// Evaluate one expression against the fold env. `None` = not
/// comptime-known (fold declines; the caller keeps the runtime form).
/// Checked arithmetic: a comptime overflow or division by zero declines
/// the fold rather than changing semantics or panicking the compiler.
fn eval_const(e: &Expr, env: &HashMap<String, ComptimeVal>) -> Option<ComptimeVal> {
    match e {
        Expr::Decimal(n) => Some(ComptimeVal::Int(*n)),
        Expr::Float(f) => Some(ComptimeVal::Float(*f)),
        Expr::Bool(b) => Some(ComptimeVal::Bool(*b)),
        Expr::Identifier(n) => env.get(n).cloned(),
        Expr::UnaryOp(kind, a) => {
            let v = eval_const(a, env)?;
            match (kind, v) {
                (UnaryOpKind::Neg, ComptimeVal::Int(i)) => Some(ComptimeVal::Int(-i)),
                (UnaryOpKind::Neg, ComptimeVal::Float(f)) => Some(ComptimeVal::Float(-f)),
                (UnaryOpKind::Not, ComptimeVal::Bool(b)) => Some(ComptimeVal::Bool(!b)),
                _ => None,
            }
        }
        Expr::BinaryOp(kind, a, b) => {
            let l = eval_const(a, env)?;
            let r = eval_const(b, env)?;
            apply_binop_const(*kind, l, r)
        }
        _ => None,
    }
}

/// The float projection of a folded value (comparisons promote).
fn as_f(v: &ComptimeVal) -> f64 {
    match v {
        ComptimeVal::Int(i) => *i as f64,
        ComptimeVal::Float(f) => *f,
        ComptimeVal::Bool(_) => f64::NAN,
    }
}

fn apply_binop_const(kind: BinaryOpKind, l: ComptimeVal, r: ComptimeVal) -> Option<ComptimeVal> {
    use ComptimeVal::{Bool, Float, Int};
    /// Int arithmetic with overflow decline; Float promotion otherwise.
    fn arith(
        l: ComptimeVal,
        r: ComptimeVal,
        fi: fn(i64, i64) -> Option<i64>,
        ff: fn(f64, f64) -> f64,
    ) -> Option<ComptimeVal> {
        let promoted = matches!(l, Float(_)) || matches!(r, Float(_));
        match (l, r) {
            (Int(a), Int(b)) if !promoted => fi(a, b).map(Int),
            (l, r) => Some(Float(ff(as_f(&l), as_f(&r)))),
        }
    }
    match kind {
        BinaryOpKind::Add => arith(l, r, |a, b| a.checked_add(b), |a, b| a + b),
        BinaryOpKind::Sub => arith(l, r, |a, b| a.checked_sub(b), |a, b| a - b),
        BinaryOpKind::Mul => arith(l, r, |a, b| a.checked_mul(b), |a, b| a * b),
        BinaryOpKind::Div => arith(l, r, |a, b| a.checked_div(b), |a, b| a / b),
        BinaryOpKind::Mod => match (l, r) {
            (Int(a), Int(b)) => a.checked_rem(b).map(Int),
            _ => None,
        },
        BinaryOpKind::Eq | BinaryOpKind::Neq => {
            let eq = match (&l, &r) {
                (Int(a), Int(b)) => a == b,
                (Bool(a), Bool(b)) => a == b,
                _ => as_f(&l) == as_f(&r),
            };
            Some(Bool(if kind == BinaryOpKind::Eq { eq } else { !eq }))
        }
        BinaryOpKind::Lt => Some(Bool(as_f(&l) < as_f(&r))),
        BinaryOpKind::Gt => Some(Bool(as_f(&l) > as_f(&r))),
        BinaryOpKind::Le => Some(Bool(as_f(&l) <= as_f(&r))),
        BinaryOpKind::Ge => Some(Bool(as_f(&l) >= as_f(&r))),
        BinaryOpKind::And => match (l, r) {
            (Bool(a), Bool(b)) => Some(Bool(a && b)),
            _ => None,
        },
        BinaryOpKind::Or => match (l, r) {
            (Bool(a), Bool(b)) => Some(Bool(a || b)),
            _ => None,
        },
        _ => None,
    }
}

/// The literal expression carrying a folded value (spliced into a let's
/// init so the binder stays bound for runtime statements).
fn literal_expr(v: &ComptimeVal) -> Expr {
    match v {
        ComptimeVal::Int(i) => Expr::Decimal(*i),
        ComptimeVal::Float(f) => Expr::Float(*f),
        ComptimeVal::Bool(b) => Expr::Bool(*b),
    }
}

/// Does a pattern decide for a comptime value? `None` = undecidable
/// (binding, range, enum patterns — the match is kept verbatim).
fn pattern_const_match(p: &Pattern, v: &ComptimeVal) -> Option<bool> {
    match (p, v) {
        (Pattern::Wildcard, _) => Some(true),
        (Pattern::Literal(Expr::Bool(b)), ComptimeVal::Bool(x)) => Some(b == x),
        (Pattern::Literal(Expr::Decimal(n)), ComptimeVal::Int(x)) => Some(n == x),
        (Pattern::Literal(Expr::Float(f)), ComptimeVal::Float(x)) => Some(f == x),
        _ => None,
    }
}

/// Fold one `let`. A comptime init is recorded in the env; the tree is
/// rewritten to the folded literal only when folding COLLAPSED something
/// (an already-literal init keeps its exact shape — downstream matchers
/// pattern-match Neg/Float nodes). A non-comptime init kills the binder in
/// the env.
fn fold_let_stmt(
    s: Statement,
    env: &mut HashMap<String, ComptimeVal>,
    out: &mut Vec<Statement>,
) {
    let Statement::Let {
        name,
        names,
        ty,
        expr,
        modifiers,
    } = s
    else {
        unreachable!("caller matched Statement::Let");
    };
    // Single-binder let (the parser also lists the name in `names`) with a
    // comptime init folds in place; multi-lets never enter the env.
    let single = names.is_empty() || (names.len() == 1 && &names[0] == &name);
    let folded = if single {
        expr.as_ref().and_then(|e| eval_const(e, env))
    } else {
        None
    };
    let already_literal = matches!(
        expr.as_ref(),
        Some(Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_))
    );
    let init = match folded {
        Some(v) => {
            env.insert(name.clone(), v.clone());
            if already_literal {
                expr
            } else {
                Some(literal_expr(&v))
            }
        }
        None => {
            env.remove(&name);
            for n in &names {
                env.remove(n);
            }
            expr
        }
    };
    out.push(Statement::Let {
        name,
        names,
        ty,
        expr: init,
        modifiers,
    });
}

/// Fold a statement-form `match`: splices the taken arm on a comptime,
/// fully-decidable scrutinee; otherwise splices the kept runtime form
/// (each arm body folds under a clone of the env — which arm runs is
/// unknown).
fn fold_stmt_form_match(
    expr: Expr,
    arms: Vec<StmtMatchArm>,
    env: &mut HashMap<String, ComptimeVal>,
    out: &mut Vec<Statement>,
) {
    let mut arms = arms;
    let scrut = eval_const(&expr, env);
    let decidable = scrut.is_some()
        && arms.iter().all(|a| {
            a.patterns
                .iter()
                .all(|p| pattern_const_match(p, scrut.as_ref().unwrap()).is_some())
        });
    if let (Some(v), true) = (scrut, decidable) {
        let taken_idx = arms.iter().position(|a| {
            a.patterns
                .iter()
                .any(|p| pattern_const_match(p, &v) == Some(true))
        });
        if let Some(idx) = taken_idx {
            let StmtMatchArm { body, .. } = arms.swap_remove(idx);
            out.extend(fold_stmt_list(body, env));
            return;
        }
        // No arm takes the value: keep verbatim — the typechecker
        // diagnoses exhaustiveness, unchanged.
    }
    let arms = arms
        .into_iter()
        .map(|a| {
            let mut clone = env.clone();
            StmtMatchArm {
                patterns: a.patterns,
                body: fold_stmt_list(a.body, &mut clone),
            }
        })
        .collect();
    out.push(Statement::Match { expr: Box::new(expr), arms });
}

/// Fold an expression-form `match` used as a statement (the F1 unified
/// dispatch shape). Splices the taken arm's block statements on a comptime
/// scrutinee; otherwise pushes the kept runtime form.
fn fold_expr_form_match(
    m: Expr,
    env: &mut HashMap<String, ComptimeVal>,
    out: &mut Vec<Statement>,
) {
    let Expr::Match(scrut_e, mut m_arms) = m else {
        unreachable!("caller matched Expr::Match");
    };
    let scrut = eval_const(&scrut_e, env);
    let decidable = scrut.is_some()
        && m_arms.iter().all(|a| {
            a.guard.is_none()
                && pattern_const_match(&a.pattern, scrut.as_ref().unwrap()).is_some()
        });
    if let (Some(v), true) = (scrut, decidable) {
        let taken_idx = m_arms
            .iter()
            .position(|a| pattern_const_match(&a.pattern, &v) == Some(true));
        if let Some(idx) = taken_idx {
            if matches!(m_arms[idx].body.as_ref(), Expr::Block(_)) {
                let arm = m_arms.swap_remove(idx);
                if let Expr::Block(body) = *arm.body {
                    out.extend(fold_stmt_list(body, env));
                    return;
                }
            }
        }
    }
    out.push(Statement::Expression(Expr::Match(scrut_e, m_arms)));
}


/// - a comptime-known `let` stays (its binder may be referenced by runtime
///   statements) — its init is rewritten to the folded literal and the
///   value feeds later condition folding;
/// - every runtime bind/mutate kills the name in the env;
/// - spliced taken arms execute deterministically in sequence — they fold
///   under the SAME env; every kept nested body (0+ or unknown iterations)
///   folds under a CLONE (mutations must not leak out).
fn fold_stmt_list(
    stmts: Vec<Statement>,
    env: &mut HashMap<String, ComptimeVal>,
) -> Vec<Statement> {
    let mut out: Vec<Statement> = Vec::with_capacity(stmts.len());
    for s in stmts {
        match s {
            stmt @ Statement::Let { .. } => fold_let_stmt(stmt, env, &mut out),
            Statement::Assign(l, r) => {
                if let Expr::Identifier(n) = &l {
                    env.remove(n);
                }
                out.push(Statement::Assign(l, r));
            }
            Statement::Match { expr, arms } => {
                fold_stmt_form_match(*expr, arms, env, &mut out);
            }
            // 2026-08-23 (F1 unified dispatch): the parser routes ALL match
            // to the EXPRESSION form — `Statement::Expression(Expr::Match)`.
            // A comptime scrutinee with block-bodied arms splices the taken
            // arm's statements (same env — the arm executes); anything else
            // stays an ordinary runtime match (fail-open).
            Statement::Expression(e @ Expr::Match(_, _)) => {
                fold_expr_form_match(e, env, &mut out);
            }
            Statement::Guarded(cond, body) => match eval_const(&cond, env) {
                Some(ComptimeVal::Bool(true)) => {
                    out.extend(fold_stmt_list(body, env));
                }
                Some(ComptimeVal::Bool(false)) => {}
                _ => {
                    let mut clone = env.clone();
                    out.push(Statement::Guarded(
                        cond,
                        fold_stmt_list(body, &mut clone),
                    ));
                }
            },
            Statement::Foreach { item, list, body } => {
                env.remove(&item);
                let mut clone = env.clone();
                out.push(Statement::Foreach {
                    item,
                    list,
                    body: fold_stmt_list(body, &mut clone),
                });
            }
            Statement::Block(body) => {
                let mut clone = env.clone();
                out.push(Statement::Block(fold_stmt_list(body, &mut clone)));
            }
            Statement::SyncBlock(body) => {
                let mut clone = env.clone();
                out.push(Statement::SyncBlock(fold_stmt_list(body, &mut clone)));
            }
            Statement::Mutex(body) => {
                let mut clone = env.clone();
                out.push(Statement::Mutex(fold_stmt_list(body, &mut clone)));
            }
            Statement::Defer(body) => {
                let mut clone = env.clone();
                out.push(Statement::Defer(fold_stmt_list(body, &mut clone)));
            }
            Statement::Barrier { groups, body } => {
                let mut clone = env.clone();
                out.push(Statement::Barrier {
                    groups,
                    body: fold_stmt_list(body, &mut clone),
                });
            }
            other => out.push(other),
        }
    }
    out
}

/// Hygiene: identifiers the caller's arguments carry must not collide with
/// names the body binds PRIVATELY (exposed `expr_item` binders are the
/// sanctioned exception — arguments reference them deliberately), and must
/// not reference the composite's own `expr` parameters. Either violation
/// would silently change meaning — both fail closed, naming both sides.
fn check_hygiene(
    def: &Definition,
    params: &[String],
    exposed: &HashSet<String>,
    args: &[Expr],
) -> Result<(), String> {
    let mut binders: HashSet<String> = HashSet::new();
    for s in &def.body {
        collect_binders(s, &mut binders);
    }
    for p in params {
        binders.remove(p);
    }
    for b in exposed {
        binders.remove(b);
    }
    for (param, arg) in params.iter().zip(args) {
        let mut idents = HashSet::new();
        collect_idents(arg, &mut idents);
        for id in &idents {
            if binders.contains(id) {
                return Err(format!(
                    "composite '{}': argument for '{}' mentions '{}', which the \
                     composite body binds privately — capture would silently \
                     change meaning; rename the caller's '{}' or the composite's \
                     binder (exposed `expr_item` binders may be referenced)",
                    def.name, param, id, id
                ));
            }
            if params.contains(id) {
                return Err(format!(
                    "composite '{}': argument for '{}' references parameter '{}' — \
                     a parameter is not visible inside another argument; inline \
                     the intended expression",
                    def.name, param, id
                ));
            }
        }
    }
    Ok(())
}

/// Driver: expand every statement-position `composite!(...)` in the
/// program whose name resolves to a declared composite. Runs to fixpoint
/// (composites may invoke composites) with a depth cap. Returns the
/// number of expansions performed. Unknown names are left for the
/// typechecker to diagnose. The program's `$let`/`$const` values seed the
/// fold env, so a span argument may be a named comptime constant.
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
    let mut comptime: HashMap<String, ComptimeVal> = pm
        .comptime_vars
        .iter()
        .filter_map(|(n, (v, _))| nav_comptime(v).map(|c| (n.clone(), c)))
        .collect();
    // Top-level `const` declarations are comptime by definition — their
    // values seed the fold env in declaration order (a const init may
    // reference an earlier const). A non-foldable init simply contributes
    // nothing (fail-open: dependent spans degrade to runtime).
    for item in items.iter() {
        if let TopLevel::Constant(k) = item {
            if let Some(v) = eval_const(&k.expr, &comptime) {
                comptime.insert(k.name.clone(), v);
            }
        }
    }
    let mut total = 0;
    for depth in 0..8 {
        let mut n = 0;
        for item in items.iter_mut() {
            match item {
                TopLevel::Transaction(t) => {
                    n += expand_stmt_list(&mut t.body, &registry, &comptime)?;
                }
                TopLevel::CompileTimeDefn(d) => {
                    n += expand_stmt_list(&mut d.body, &registry, &comptime)?;
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

/// The comptime-foldable projection of a stage-evaluator value (plan
/// 2026-09-21): scalars only — the fold never guesses at structures.
fn nav_comptime(v: &crate::macros::eval::NavValue) -> Option<ComptimeVal> {
    match v {
        crate::macros::eval::NavValue::Int(i) => Some(ComptimeVal::Int(*i)),
        crate::macros::eval::NavValue::Bool(b) => Some(ComptimeVal::Bool(*b)),
        crate::macros::eval::NavValue::Count(c) => Some(ComptimeVal::Int(*c as i64)),
        _ => None,
    }
}

/// Expand statement-position composite invocations in one statement list.
fn expand_stmt_list(
    stmts: &mut Vec<Statement>,
    registry: &HashMap<&String, &Definition>,
    comptime: &HashMap<String, ComptimeVal>,
) -> Result<usize, String> {
    let mut n = 0;
    let mut i = 0;
    while i < stmts.len() {
        // Depth first — nested bodies expand before their parents, so an
        // enclosing splice never hides an inner call site.
        expand_nested(&mut stmts[i], registry, comptime)?;
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
                let expanded = expand_composite_invocation(def, &args, comptime)?;
                let tail = stmts.split_off(i + 1);
                stmts.truncate(i);
                stmts.extend(expanded);
                stmts.extend(tail);
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
    comptime: &HashMap<String, ComptimeVal>,
) -> Result<(), String> {
    match s {
        Statement::Foreach { body, .. }
        | Statement::Guarded(_, body)
        | Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body) => {
            expand_stmt_list(body, registry, comptime)?;
        }
        Statement::Barrier { body, .. } => {
            expand_stmt_list(body, registry, comptime)?;
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
        // 2026-08-23 (F1 unified dispatch): statement-position match parses
        // as the EXPRESSION form — its scrutinee, guards, patterns, and
        // block bodies carry parameter references too.
        Expr::Match(scrut, arms) => {
            subst_expr(scrut, param, arg);
            for a in arms.iter_mut() {
                subst_pattern(&mut a.pattern, param, arg);
                if let Some(g) = a.guard.as_mut() {
                    subst_expr(g, param, arg);
                }
                subst_expr(a.body.as_mut(), param, arg);
            }
        }
        Expr::Block(stmts) => {
            for s in stmts.iter_mut() {
                substitute_param(s, param, arg);
            }
        }
        _ => {}
    }
}

/// Substitute inside a match PATTERN (literal and range patterns carry
/// expressions; binder patterns do not reference parameters).
fn subst_pattern(p: &mut Pattern, param: &str, arg: &Expr) {
    match p {
        Pattern::Literal(e) => subst_expr(e, param, arg),
        Pattern::Range(a, b) => {
            subst_expr(a, param, arg);
            subst_expr(b, param, arg);
        }
        Pattern::EnumVariant(_, xs) => {
            for x in xs.iter_mut() {
                subst_pattern(x, param, arg);
            }
        }
        Pattern::Tuple(xs) => {
            for x in xs.iter_mut() {
                subst_pattern(x, param, arg);
            }
        }
        Pattern::Multi(ps) => {
            for x in ps.iter_mut() {
                subst_pattern(x, param, arg);
            }
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
        let (subst, exposed) = composite_signature(&d).unwrap();
        assert_eq!(subst, vec!["x".to_string(), "y".to_string()]);
        assert!(exposed.is_empty());
        let plain = defn_of(
            &parse_program("$defn g(v: Int) -> Int { v; };"),
            "g",
        );
        assert!(!is_composite(&plain), "value params are not a composite");
    }

    #[test]
    fn expr_item_params_expose_binders() {
        // `expr_item` names a binder the body binds; arguments may reference
        // it, and it is never passed at the call site.
        let items = parse_program(
            "$defn f(fill: expr, n: expr, i: expr_item) { \n\
             \x20 foreach i in 0..n { \n\
             \x20  buf[i] = fill; \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let (subst, exposed) = composite_signature(&d).unwrap();
        assert_eq!(subst, vec!["fill".to_string(), "n".to_string()]);
        assert!(exposed.contains("i"));
        // The argument references the exposed binder `i` — allowed.
        let fill = Expr::BinaryOp(
            BinaryOpKind::Mul,
            Box::new(Expr::Identifier("w".into())),
            Box::new(Expr::Identifier("i".into())),
        );
        let out = expand_composite_invocation(
            &d,
            &[fill, Expr::Identifier("N".into())],
            &HashMap::new(),
        )
        .expect("exposed-binder reference is legal");
        assert_eq!(out.len(), 1);
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
        let out =
            expand_composite_invocation(&d, &[arg_p, arg_q], &HashMap::new())
                .expect("expands");
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
        let err =
            expand_composite_invocation(&d, &[arg], &HashMap::new()).unwrap_err();
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
        let err = expand_composite_invocation(&d, &[arg_p, arg_q], &HashMap::new())
            .unwrap_err();
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
            &HashMap::new(),
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
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(err.contains("mixes"), "{err}");
    }

    #[test]
    fn arg_count_mismatch_diagnoses_params() {
        let items = parse_program("$defn f(p: expr, q: expr) { res = p + q; };");
        let d = defn_of(&items, "f");
        let err = expand_composite_invocation(
            &d,
            &[Expr::Decimal(1)],
            &HashMap::new(),
        )
        .unwrap_err();
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

#[cfg(test)]
mod comptime_fold {
    use super::*;

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

    /// Expand `f!(span)` where the composite branches `match n <= 32` on a
    /// literal small arm / large arm. Returns the spliced statements.
    fn expand_adaptive(span: Expr) -> Vec<Statement> {
        let items = parse_program(
            "$defn f(n: expr) { \n\
             \x20 match n <= 32 { \n\
             \x20  true => { small = 1; }, \n\
             \x20  false => { large = 1; }, \n\
             \x20 }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        expand_composite_invocation(&d, &[span], &HashMap::new()).expect("expands")
    }

    #[test]
    fn comptime_match_splices_taken_arm_only() {
        let out = expand_adaptive(Expr::Decimal(16));
        let dump = format!("{out:?}");
        assert!(dump.contains("\"small\""), "taken arm spliced: {dump}");
        assert!(!dump.contains("\"large\""), "dead arm pruned: {dump}");
        assert!(!dump.contains("Match"), "no runtime match remains: {dump}");
    }

    #[test]
    fn comptime_match_takes_other_arm() {
        let out = expand_adaptive(Expr::Decimal(128));
        let dump = format!("{out:?}");
        assert!(dump.contains("\"large\"") && !dump.contains("\"small\""), "{dump}");
    }

    #[test]
    fn runtime_scrutinee_degrades_to_runtime_match() {
        let out = expand_adaptive(Expr::Identifier("N".into()));
        let dump = format!("{out:?}");
        assert!(dump.contains("Match"), "kept for runtime: {dump}");
        assert!(
            dump.contains("\"small\"") && dump.contains("\"large\""),
            "both futures present: {dump}"
        );
    }

    #[test]
    fn comptime_let_feeds_condition_and_stays_bound() {
        let items = parse_program(
            "$defn f(n: expr) { \n\
             \x20 let tile: Int = n / 4; \n\
             \x20 match tile > 8 { \n\
             \x20  true => { wide = tile; }, \n\
             \x20  false => { narrow = tile; }, \n\
             \x20 }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(64)], &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("\"wide\""), "64/4=16 > 8: {dump}");
        assert!(!dump.contains("\"narrow\""), "{dump}");
        assert!(
            dump.contains("Decimal(16)"),
            "the let stays, folded to its value: {dump}"
        );
    }

    #[test]
    fn named_comptime_constant_seeds_the_fold() {
        let items = parse_program(
            "$defn f(n: expr) { match n <= 32 { true => { s = 1; }, false => { l = 1; }, }; };",
        );
        let d = defn_of(&items, "f");
        let mut seed = HashMap::new();
        seed.insert("NKV".to_string(), ComptimeVal::Int(4096));
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("NKV".into())],
            &seed,
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("\"l\"") && !dump.contains("\"s\""), "{dump}");
    }

    #[test]
    fn top_level_const_seeds_the_driver_fold() {
        let src = "\
$defn f(n: expr) { match n <= 32 { true => { s = 1; }, false => { l = 1; }, } };
const D: Int = 128;
let i: Int = 0;
let buf: Float[64];
async node k [i < 1][i == 1] {
    f!(D);
    i = i + 1;
    term;
};";
        let mut items = parse_program(src);
        let mut pm = PluginManager::new();
        crate::plugin::loader::extract_inline_stage_blocks(&mut items, &mut pm);
        expand_composites(&mut items, &pm).expect("expansion runs");
        let TopLevel::Transaction(t) = items
            .iter()
            .find(|it| matches!(it, TopLevel::Transaction(tx) if tx.name == "k"))
            .unwrap()
        else {
            panic!("node k");
        };
        let dump = format!("{:?}", t.body);
        assert!(
            dump.contains("\"l\"") && !dump.contains("\"s\"") && !dump.contains("Match"),
            "const D=128 folds to the large arm: {dump}"
        );
    }

    #[test]
    fn when_folds_both_polarities() {
        let items = parse_program(
            "$defn f(n: expr) { \n\
             \x20 when n > 100 { big = 1; }; \n\
             \x20 when n > 1000 { huge = 1; }; \n\
             \x20 when n > 0 { pos = 1; }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(500)], &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("\"big\""), "{dump}");
        assert!(!dump.contains("\"huge\""), "false when dropped: {dump}");
        assert!(dump.contains("\"pos\""), "{dump}");
        assert!(!dump.contains("Guarded"), "no runtime guard remains: {dump}");
    }

    #[test]
    fn runtime_rebind_kills_comptime_value() {
        let items = parse_program(
            "$defn f(n: expr, w: expr) { \n\
             \x20 let t: Int = n; \n\
             \x20 t = w; \n\
             \x20 match t > 8 { true => { a = 1; }, false => { b = 1; }, }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Decimal(1), Expr::Identifier("runtime".into())],
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Match"), "t mutated at runtime: {dump}");
    }

    #[test]
    fn loop_binder_is_never_comptime() {
        let items = parse_program(
            "$defn f(n: expr) { \n\
             \x20 foreach j in 0..n { \n\
             \x20  match j < 4 { true => { lo = 1; }, false => { hi = 1; }, }; \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(16)], &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Match"), "j is a runtime binder: {dump}");
    }

    #[test]
    fn nested_adaptive_arms_fold_recursively() {
        let items = parse_program(
            "$defn f(n: expr) { \n\
             \x20 match n <= 32 { \n\
             \x20  true => { match n <= 8 { true => { tiny = 1; }, false => { small = 1; }, }; }, \n\
             \x20  false => { large = 1; }, \n\
             \x20 }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(4)], &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("\"tiny\""), "{dump}");
        assert!(
            !dump.contains("\"small\"") && !dump.contains("\"large\""),
            "outer and inner dead arms pruned: {dump}"
        );
        assert!(!dump.contains("Match"), "fully specialized: {dump}");
    }

    #[test]
    fn binding_pattern_scrutinee_degrades() {
        let items = parse_program(
            "$defn f(n: expr) { match n { x => { any = x; }, }; };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(7)], &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        assert!(
            dump.contains("Match"),
            "binding patterns are not comptime-decidable: {dump}"
        );
    }

    #[test]
    fn comptime_overflow_declines_fold() {
        let items = parse_program(
            "$defn f(n: expr) { let t: Int = n * 2; match t > 8 { true => { a = 1; }, false => { b = 1; }, }; };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Decimal(i64::MAX)],
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Match"), "overflow declines: {dump}");
    }

    #[test]
    fn contracts_survive_folding() {
        let items = parse_program(
            "$defn f(n: expr) [n < 64] [n > 0] { match n <= 32 { true => { s = 1; }, false => { l = 1; }, } };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(16)], &HashMap::new())
            .expect("expands");
        assert_eq!(out.len(), 3, "pre gate + spliced arm + post gate");
        assert!(matches!(&out[0], Statement::Gate(_)));
        assert!(matches!(&out[2], Statement::Gate(_)));
    }
}

#[cfg(test)]
mod frontb {
    use super::*;
    use crate::ast::top::TopLevel as TL;

    fn parse_program(src: &str) -> Vec<TL> {
        let tokens = crate::pipeline::lex_for_path("t.bv", src).expect("lex");
        crate::pipeline::parse("t.bv", &tokens, src).expect("parse")
    }

    #[test]
    fn softmax_fused_composite_expands() {
        let lib = std::fs::read_to_string("lib/std/numeric.bv").expect("read numeric.bv");
        let user = "\
let h: Int = 0;
let q: Float[1024];
async node s [h < 8][h == 8] {
    softmax_fused!(
        q[h * 128 + d],
        v[h * 32768 + j * 128 + d],
        256, 128,
        acc, a_out,
        h * 128, h * 128
    );
    h = h + 1;
    term;
};";
        let mut items = parse_program(user);
        items.extend(parse_program(&lib));
        let mut pm = PluginManager::new();
        crate::plugin::loader::extract_inline_stage_blocks(&mut items, &mut pm);
        println!("registry keys: {:?}", pm.fn_registry.keys().collect::<Vec<_>>());
        let n = expand_composites(&mut items, &pm);
        match n {
            Ok(k) => println!("expanded {k}"),
            Err(e) => panic!("expansion failed: {e}"),
        }
        assert_eq!(n.unwrap(), 1);
    }
}
