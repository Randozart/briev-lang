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
use crate::ast::{BinaryOpKind, Expr, MatchArm, Pattern, UnaryOpKind};
use crate::ast::{Dimension, ReflectKind, Type};
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
    matches!(t, crate::ast::Type::Custom(n) if n == "Expr" || n == "ExprItem")
}

/// The composite's signature: (substitution parameters in declaration
/// order, exposed binder names). `None` when the composite mixes in value
/// parameters (v1 restriction — fail closed).
fn composite_signature(
    def: &Definition,
) -> Option<(Vec<String>, HashSet<String>, Option<String>)> {
    let mut subst = Vec::new();
    let mut exposed: HashSet<String> = HashSet::new();
    for (n, t) in &def.parameters {
        match t {
            crate::ast::Type::Custom(k) if k == "Expr" => {
                if Some(n) == def.variadic_param.as_ref() {
                    // The rest parameter is NOT substituted positionally — it
                    // binds ALL trailing arguments as a compile-time list.
                    continue;
                }
                subst.push(n.clone());
            }
            crate::ast::Type::Custom(k) if k == "ExprItem" => {
                exposed.insert(n.clone());
            }
            _ => return None,
        }
    }
    Some((subst, exposed, def.variadic_param.clone()))
}

/// Validate call-site arity against the fixed params and the optional
/// `...` rest param (2026-09-22 unified-metaprogramming plan). Variadic:
/// fixed params must all be present; zero rest args is a mistake, not a
/// no-op. Non-variadic: exact arity.
fn check_arity(
    name: &str,
    params: &[String],
    rest_name: Option<&str>,
    args: &[Expr],
) -> Result<(), String> {
    if rest_name.is_some() {
        if args.len() < params.len() {
            return Err(format!(
                "composite '{name}' expects at least {} expression argument(s) \
                 before the `...` rest ({}, got {} — supply one expression per \
                 fixed `expr` parameter plus zero-or-more rest arguments",
                params.len(),
                params.join(", "),
                args.len()
            ));
        }
        if args.len() == params.len() {
            return Err(format!(
                "composite '{name}': a `...` rest call site with zero rest \
                 arguments is a mistake, not a no-op — drop the call or pass \
                 one or more trailing arguments to emit per element"
            ));
        }
        Ok(())
    } else if args.len() != params.len() {
        Err(format!(
            "composite '{name}' expects {} expression arguments ({}), got {} — \
             supply one expression per `Expr` parameter (`ExprItem` binders \
             are bound by the body, never passed)",
            params.len(),
            params.join(", "),
            args.len()
        ))
    } else {
        Ok(())
    }
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
    state_types: &HashMap<String, Type>,
) -> Result<Vec<Statement>, String> {
    let (body, _) = expand_composite_body(def, args, comptime, state_types)?;
    Ok(body)
}

/// Expand a composite in EXPRESSION position to a value (2026-09-22
/// unified-metaprogramming plan, C2): the folded body becomes an
/// `Expr::Block` whose value is the composite's `-> Type` return. The
/// trailing `term v` (or a trailing value `Expression(v)`) becomes the
/// block's value statement; the block types as `v`'s type (the typechecker
/// types a block ending in `Expression(e)` as `e`'s type — matching the
/// interpreter's `eval_block` and the backend's last-register return). A
/// value-position call of a composite WITHOUT a value term is an error.
pub fn expand_composite_value(
    def: &Definition,
    args: &[Expr],
    comptime: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Result<Expr, String> {
    let (mut body, value) = expand_composite_body(def, args, comptime, state_types)?;
    let value = value.ok_or_else(|| {
        format!(
            "composite '{}' is used in value position but its body yields no \
             value — a value composite must end in `term v;` or a trailing \
             value expression",
            def.name
        )
    })?;
    // Convert the trailing value statement into the block's value. If it's a
    // `term v`, drop the term marker — the block ends in `Expression(v)` so
    // eval/emit produce v as the block value (never a function return).
    if let Some(last) = body.last_mut() {
        if matches!(last, Statement::Term(Some(_))) {
            *last = Statement::Expression(value);
        }
    }
    Ok(Expr::Block(body))
}

/// The shared core of composite expansion (2026-09-22 unified-metaprogramming
/// plan): validate signature/arity/hygiene, generate, substitute, splice the
/// rest-foreach, fold. Returns the folded statement body and, when the
/// composite declares a `-> Type`, the value expression the body yields (the
/// trailing `term v`'s value) — statement-position expansion ignores it,
/// expression-position expansion wraps it in an `Expr::Block`.
fn expand_composite_body(
    def: &Definition,
    args: &[Expr],
    comptime: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Result<(Vec<Statement>, Option<Expr>), String> {
    let name = &def.name;
    let Some((params, exposed, rest)) = composite_signature(def) else {
        return Err(format!(
            "composite '{name}' mixes `Expr`/`ExprItem` and value parameters — \
             declare all parameters as `name: Expr` or `name: ExprItem` (v1 \
             supports expression parameters only)"
        ));
    };
    let rest_name = rest.as_deref();
    check_arity(name, &params, rest_name, args)?;
    check_hygiene(def, &params, &exposed, args)?;
    let trivial = Expr::Bool(true);
    let mut out: Vec<Statement> = Vec::new();
    if def.contract.pre_condition != trivial {
        out.push(Statement::Gate(def.contract.pre_condition.clone()));
    }
    let mut env: HashMap<String, ComptimeVal> = comptime.clone();
    // Generation FIRST (static text, consts only), then substitution, then
    // rest-foreach splice (the rest list is the sanctioned compile-time
    // iteration channel — see splice_rest_foreach).
    let generated = unroll_static(&def.body, comptime, state_types);
    let mut body_stmts: Vec<Statement> = Vec::new();
    for s in &generated {
        let mut cloned = s.clone();
        for (param, arg) in params.iter().zip(args) {
            substitute_param(&mut cloned, param, arg);
        }
        body_stmts.push(cloned);
    }
    // Rest-foreach splice AFTER fixed-param substitution: a body
    // `foreach c in calls { … }` where `calls` is the rest param becomes one
    // body copy per trailing arg, each `c` substituted with the ORIGINAL arg
    // expression (emission, not folding — `execute_many!(f(a), f(b))` emits
    // `f(a); f(b);`).
    if let Some(rest_name) = rest_name {
        body_stmts = splice_rest_foreach(body_stmts, rest_name, &args[params.len()..]);
    }
    let ctx = FoldCtx { state_types, composite: name };
    let folded = fold_stmt_list(body_stmts, &mut env, &ctx)?;
    out.extend(folded);
    if def.contract.post_condition != trivial {
        out.push(Statement::Gate(def.contract.post_condition.clone()));
    }
    let value = composite_value_expr(&out);
    Ok((out, value))
}

/// The value a composite's folded body yields: the expression of its trailing
/// `term v` statement, when the composite declares a `-> Type` (2026-09-22).
/// A body without a value term yields `None` — statement-position expansion
/// only. Expression-position expansion requires a value and errors if absent.
fn composite_value_expr(body: &[Statement]) -> Option<Expr> {
    match body.last() {
        Some(Statement::Term(Some(e))) | Some(Statement::Expression(e)) => Some(e.clone()),
        _ => None,
    }
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
    /// 2026-09-22 (unified-metaprogramming plan, C3): a compile-time string
    /// (from `Expr::Quoted("...")` or a bridged `NavValue::Str`). Closes the
    /// one-value-domain gap: a composite body may gate a `match`/`when` on a
    /// string literal the same way it gates on an Int — `match s { "abc" => … }`
    /// folds when `s` substitutes to a literal. String comparisons in
    /// comptime conditions fold too.
    Str(String),
    /// Comptime-generated sequence (plan 2026-09-21 §B2): the target of a
    /// comptime `foreach`. Scalars only — the fold never guesses at
    /// nested structures.
    List(Vec<ComptimeVal>),
}

/// Evaluate one expression against the fold env. `None` = not
/// comptime-known (fold declines; the caller keeps the runtime form).
/// Checked arithmetic: a comptime overflow or division by zero declines
/// the fold rather than changing semantics or panicking the compiler.
/// `state_types` maps declared state-variable names to their static types
/// (2026-09-22 reflection-conditions plan): `.^^Size` / `.^^Element` on a
/// declared array/iterable receiver fold from the type at expansion time.
fn eval_const(
    e: &Expr,
    env: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Option<ComptimeVal> {
    match e {
        Expr::Decimal(n) => Some(ComptimeVal::Int(*n)),
        Expr::Float(f) => Some(ComptimeVal::Float(*f)),
        Expr::Bool(b) => Some(ComptimeVal::Bool(*b)),
        // 2026-09-22 (C3): a string literal folds to ComptimeVal::Str — the
        // one-value-domain closure. Tagged literals fold only for the plain
        // string tag; any other tagged literal declines.
        Expr::Quoted(bytes) => Some(ComptimeVal::Str(String::from_utf8_lossy(bytes).into_owned())),
        Expr::TaggedQuotedLiteral(bytes, tag) if tag == "str" || tag == "String" => {
            Some(ComptimeVal::Str(String::from_utf8_lossy(bytes).into_owned()))
        }
        Expr::Identifier(n) => env.get(n).cloned(),
        Expr::UnaryOp(kind, a) => {
            let v = eval_const(a, env, state_types)?;
            match (kind, v) {
                (UnaryOpKind::Neg, ComptimeVal::Int(i)) => Some(ComptimeVal::Int(-i)),
                (UnaryOpKind::Neg, ComptimeVal::Float(f)) => Some(ComptimeVal::Float(-f)),
                (UnaryOpKind::Not, ComptimeVal::Bool(b)) => Some(ComptimeVal::Bool(!b)),
                _ => None,
            }
        }
        Expr::BinaryOp(kind, a, b) => {
            let l = eval_const(a, env, state_types)?;
            let r = eval_const(b, env, state_types)?;
            apply_binop_const(*kind, l, r)
        }
        Expr::List(xs) => {
            let mut out = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(eval_const(x, env, state_types)?);
            }
            Some(ComptimeVal::List(out))
        }
        // 2026-09-22 (reflection-conditions plan): `.^^Size` / `.^^Element`
        // on a declared state variable fold from its static type. The
        // receiver must be a bare state-decl name; any other receiver
        // declines (fail-open). Mirrors the LLVM backend's
        // vector_element_count / type_category_code so the fold and codegen
        // agree (rule #4 parity).
        Expr::Reflect(recv, target, kind) => {
            let Expr::Identifier(name) = recv.as_ref() else {
                return None;
            };
            let ty = state_types.get(name)?;
            reflect_type_value(ty, target, *kind)
        }
        _ => None,
    }
}

/// The folded value of a compile-time reflection target on a static type:
/// `^^Size` → the element count of a vector (product of Anonymous dims);
/// `^^Element` → the element category code. Mirrors the LLVM backend
/// (`vector_element_count`, `type_category_code`) — the fold and codegen
/// must agree. Any other target/kind/receiver declines (`None`).
fn reflect_type_value(
    ty: &Type,
    target: &str,
    kind: crate::ast::ReflectKind,
) -> Option<ComptimeVal> {
    if !matches!(kind, crate::ast::ReflectKind::CompileTime) {
        return None;
    }
    match target {
        "Size" => {
            let count: u64 = match ty {
                Type::Vector(_, dims) => dims
                    .iter()
                    .map(|d| match d {
                        Dimension::Anonymous(n) => *n as u64,
                        _ => 1,
                    })
                    .product(),
                _ => 1,
            };
            Some(ComptimeVal::Int(count as i64))
        }
        "Element" => {
            let elem = match ty {
                Type::Vector(inner, _) => (**inner).clone(),
                Type::Custom(n) if n == "String" => Type::Custom("Char".to_string()),
                _ => return None,
            };
            Some(ComptimeVal::Int(type_category_code(&elem)))
        }
        _ => None,
    }
}

/// The `.^^Type` / `.^^Element` frozen category code. MUST match
/// `emit_expr.rs::type_category_code` and the interpreter's
/// `reflect_type_code` (rule #4 parity).
fn type_category_code(ty: &Type) -> i64 {
    match ty {
        Type::Custom(n)
            if n == "Float" || n == "Float64" || n == "Double" || n == "Float32" =>
        {
            1
        }
        Type::Custom(n) if n == "Bool" || n == "UInt8" || n == "Int8" => 2,
        Type::Custom(n) if n == "Char" || n == "Byte" => 3,
        Type::Custom(n) if n == "String" || matches!(ty, Type::Bits(_)) => 4,
        Type::Ptr(_) | Type::PtrConst(_) | Type::LayoutPtr(_) => 7,
        _ => 0,
    }
}

/// Collect the declared static types of top-level state variables
/// (2026-09-22 reflection-conditions plan): `let x: T` statements and
/// explicit `StateDecl` items. The map feeds the reflection fold so a
/// composite can gate on a receiver's shape at expansion time.
fn collect_state_types(items: &[TopLevel]) -> HashMap<String, Type> {
    let mut out = HashMap::new();
    for item in items {
        match item {
            TopLevel::Statement(stmt) => {
                if let Statement::Let {
                    name,
                    ty: Some(ty),
                    ..
                } = stmt.as_ref()
                {
                    out.insert(name.clone(), ty.clone());
                }
            }
            TopLevel::StateDecl(d) => {
                out.insert(d.name.clone(), d.ty.clone());
            }
            _ => {}
        }
    }
    out
}

/// The float projection of a folded value (comparisons promote).
fn as_f(v: &ComptimeVal) -> f64 {
    match v {
        ComptimeVal::Int(i) => *i as f64,
        ComptimeVal::Float(f) => *f,
        ComptimeVal::Bool(_) => f64::NAN,
        ComptimeVal::Str(_) => f64::NAN,
        ComptimeVal::List(_) => f64::NAN,
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
                // 2026-09-22 (C3): string equality folds — a comptime
                // `match s { "abc" => … }` decides when `s` is a literal.
                (ComptimeVal::Str(a), ComptimeVal::Str(b)) => a == b,
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
        BinaryOpKind::Shl | BinaryOpKind::Shr => match (l, r) {
            (Int(a), Int(b)) if b >= 0 && b < 64 => {
                if kind == BinaryOpKind::Shl {
                    a.checked_shl(b as u32).map(Int)
                } else {
                    Some(Int(a >> b))
                }
            }
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
        ComptimeVal::Str(s) => Expr::Quoted(s.as_bytes().to_vec()),
        // Lists never reach this — fold_let_stmt keeps the source literal.
        ComptimeVal::List(_) => Expr::List(vec![]),
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
        // 2026-09-22 (C3): a string-literal pattern matches a folded string.
        (Pattern::Literal(Expr::Quoted(b)), ComptimeVal::Str(x)) => {
            Some(String::from_utf8_lossy(b) == x.as_str())
        }
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
    state_types: &HashMap<String, Type>,
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
        expr.as_ref().and_then(|e| eval_const(e, env, state_types))
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
            if already_literal || matches!(v, ComptimeVal::List(_)) {
                // A literal keeps its exact shape (downstream matchers
                // pattern-match Neg/Float nodes); a comptime list has no
                // scalar literal — the source list literal IS its form.
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
    ctx: &FoldCtx,
    out: &mut Vec<Statement>,
) -> Result<(), String> {
    let mut arms = arms;
    let scrut = eval_const(&expr, env, ctx.state_types);
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
            out.extend(fold_stmt_list(body, env, ctx)?);
            return Ok(());
        }
        // No arm takes the value: keep verbatim — the typechecker
        // diagnoses exhaustiveness, unchanged.
    }
    let mut folded_arms = Vec::with_capacity(arms.len());
    for mut a in arms {
        let mut clone = env.clone();
        a.body = fold_stmt_list(a.body, &mut clone, ctx)?;
        env_kill_tree(&a.body, env);
        folded_arms.push(a);
    }
    out.push(Statement::Match {
        expr: Box::new(expr),
        arms: folded_arms,
    });
    Ok(())
}

/// Fold an expression-form `match` used as a statement (the F1 unified
/// dispatch shape). Splices the taken arm's block statements on a comptime
/// scrutinee; otherwise pushes the kept runtime form.
fn fold_expr_form_match(
    m: Expr,
    env: &mut HashMap<String, ComptimeVal>,
    ctx: &FoldCtx,
    out: &mut Vec<Statement>,
) -> Result<(), String> {
    let Expr::Match(scrut_e, mut m_arms) = m else {
        unreachable!("caller matched Expr::Match");
    };
    let scrut = eval_const(&scrut_e, env, ctx.state_types);
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
                    out.extend(fold_stmt_list(body, env, ctx)?);
                    return Ok(());
                }
            }
        }
    }
    for a in &m_arms {
        if let Expr::Block(body) = a.body.as_ref() {
            env_kill_tree(body, env);
        }
    }
    out.push(Statement::Expression(Expr::Match(scrut_e, m_arms)));
    Ok(())
}


/// - a comptime-known `let` stays (its binder may be referenced by runtime
///   statements) — its init is rewritten to the folded literal and the
///   value feeds later condition folding;
/// - every runtime bind/mutate kills the name in the env;
/// - spliced taken arms execute deterministically in sequence — they fold
///   under the SAME env; every kept nested body (0+ or unknown iterations)
///   folds under a CLONE (mutations must not leak out).
/// Comptime generation cap: an unroll beyond this is an authoring bug
/// (runaway generation), not a big loop — fail the expansion with the fix.
const COMPTIME_UNROLL_CAP: i64 = 4096;

/// Comptime-known iteration values for a STATIC (pre-substitution) loop
/// list: ranges bounded by literals or seed consts, list literals of
/// literals, or seed-const lists. A range whose bound mentions ANY other
/// identifier (a caller span, a body let) is a runtime loop — generation
/// only comes from the declaration's own static text.
fn comptime_iters(
    list: &Expr,
    consts: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Option<Vec<ComptimeVal>> {
    match list {
        Expr::Range {
            start,
            end,
            inclusive,
        } => {
            let static_end = match end.as_ref() {
                Expr::Decimal(_) => true,
                Expr::Identifier(n) => consts.contains_key(n),
                _ => false,
            };
            if !static_end {
                return None;
            }
            let a = match eval_const(start, consts, state_types)? {
                ComptimeVal::Int(i) => i,
                _ => return None,
            };
            let b = match eval_const(end, consts, state_types)? {
                ComptimeVal::Int(i) => i,
                _ => return None,
            };
            let stop = if *inclusive {
                b.checked_add(1)?
            } else {
                b
            };
            if a >= stop {
                return Some(Vec::new());
            }
            let n = stop.checked_sub(a)?;
            if n > COMPTIME_UNROLL_CAP {
                return None;
            }
            Some((a..stop).map(ComptimeVal::Int).collect())
        }
        Expr::List(xs) => {
            if xs.len() as i64 > COMPTIME_UNROLL_CAP {
                return None;
            }
            let mut out = Vec::with_capacity(xs.len());
            for x in xs {
                match x {
                    Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) => {}
                    _ => return None,
                }
                out.push(eval_const(x, consts, state_types)?);
            }
            Some(out)
        }
        Expr::Identifier(n) => match consts.get(n) {
            Some(ComptimeVal::List(xs)) => Some(xs.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Pre-substitution generation pass (plan 2026-09-21 §B1/B2): unroll
/// `foreach` loops whose lists are the declaration's OWN static text —
/// literal ranges, list literals, seed consts. Caller spans never
/// generate: an `expr` parameter is a runtime quantity even when a
/// particular call passes a literal (that literal is POLICY, not
/// structure). Recurses through nesting; item substitution reuses the
/// parameter machinery so arithmetic on the item folds downstream.
fn unroll_static(
    body: &[Statement],
    consts: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Vec<Statement> {
    let mut out = Vec::with_capacity(body.len());
    for s in body {
        match s {
            Statement::Foreach { item, list, body } => {
                match comptime_iters(list, consts, state_types) {
                    Some(iters) => {
                        // Splice the body once per iteration value,
                        // substituting the item's literal (the recursion
                        // re-generates nested static loops).
                        for val in &iters {
                            let mut iter_body = unroll_static(body, consts, state_types);
                            for b in iter_body.iter_mut() {
                                substitute_param(b, item, &literal_expr(val));
                            }
                            out.extend(iter_body);
                        }
                    }
                    None => {
                        let mut st = s.clone();
                        if let Statement::Foreach { body, .. } = &mut st {
                            *body = unroll_static(body, consts, state_types);
                        }
                        out.push(st);
                    }
                }
            }
            Statement::Guarded(_, body)
            | Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Mutex(body)
            | Statement::Defer(body) => {
                let mut st = s.clone();
                let inner = match &mut st {
                    Statement::Guarded(_, b)
                    | Statement::Block(b)
                    | Statement::SyncBlock(b)
                    | Statement::Mutex(b)
                    | Statement::Defer(b) => b,
                    _ => unreachable!("matched above"),
                };
                *inner = unroll_static(body, consts, state_types);
                out.push(st);
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// Kill every comptime env entry a KEPT (may-run-zero-times) body
/// rebinds or mutates — Lets, assignments to identifiers, nested loop
/// items. The clone that folded the body never propagates its kills, so
/// the outer env would otherwise keep a stale value the runtime overwrites
/// (found by conformance: `s_` surviving as Int(0) let the in-place fold
/// rewrite the normalize tail into acc/0). Over-killing is safe: the fold
/// merely declines.
fn env_kill_tree(body: &[Statement], env: &mut HashMap<String, ComptimeVal>) {
    for s in body {
        match s {
            Statement::Let { name, names, .. } => {
                env.remove(name);
                for n in names {
                    env.remove(n);
                }
            }
            Statement::Assign(Expr::Identifier(n), _) => {
                env.remove(n);
            }
            Statement::Foreach { item, body, .. } => {
                env.remove(item);
                env_kill_tree(body, env);
            }
            Statement::Guarded(_, body)
            | Statement::Block(body)
            | Statement::SyncBlock(body)
            | Statement::Mutex(body)
            | Statement::Defer(body) => env_kill_tree(body, env),
            _ => {}
        }
    }
}

/// Fold comptime-known SUBexpressions in place (post-order): pure scalar
/// arithmetic that became known — typically through item substitution in
/// an unrolled body — is replaced by its literal at every position
/// (assign sides, kept loop lists). Already-literal nodes are untouched.
fn fold_expr_in_place(
    e: &mut Expr,
    env: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) {
    match e {
        Expr::BinaryOp(_, a, b) => {
            fold_expr_in_place(a, env, state_types);
            fold_expr_in_place(b, env, state_types);
        }
        Expr::UnaryOp(_, a) => fold_expr_in_place(a, env, state_types),
        Expr::Index(a, i) => {
            fold_expr_in_place(a, env, state_types);
            fold_expr_in_place(i, env, state_types);
        }
        Expr::Range { start, end, .. } => {
            fold_expr_in_place(start, env, state_types);
            fold_expr_in_place(end, env, state_types);
        }
        _ => {}
    }
    if matches!(
        e,
        Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_)
    ) {
        return;
    }
    if let Some(v) = eval_const(e, env, state_types) {
        if !matches!(v, ComptimeVal::List(_)) {
            *e = literal_expr(&v);
        }
    }
}

fn fold_stmt_list(
    stmts: Vec<Statement>,
    env: &mut HashMap<String, ComptimeVal>,
    ctx: &FoldCtx,
) -> Result<Vec<Statement>, String> {
    let mut out: Vec<Statement> = Vec::with_capacity(stmts.len());
    for s in stmts {
        match s {
            stmt @ Statement::Let { .. } => fold_let_stmt(stmt, env, ctx.state_types, &mut out),
            Statement::Assign(mut l, mut r) => {
                if let Expr::Identifier(n) = &l {
                    env.remove(n);
                }
                fold_expr_in_place(&mut l, env, ctx.state_types);
                fold_expr_in_place(&mut r, env, ctx.state_types);
                out.push(Statement::Assign(l, r));
            }
            Statement::Match { expr, arms } => {
                fold_stmt_form_match(*expr, arms, env, ctx, &mut out)?;
            }
            // 2026-08-23 (F1 unified dispatch): the parser routes ALL match
            // to the EXPRESSION form — `Statement::Expression(Expr::Match)`.
            // A comptime scrutinee with block-bodied arms splices the taken
            // arm's statements (same env — the arm executes); anything else
            // stays an ordinary runtime match (fail-open).
            Statement::Expression(e @ Expr::Match(_, _)) => {
                fold_expr_form_match(e, env, ctx, &mut out)?;
            }
            // 2026-09-21 (comptime generation, §B3): a check that folds
            // true is proven — spliced out; false fails the expansion with
            // the composite and expression named; a non-foldable check
            // stays on the ordinary runtime path (fail-open).
            Statement::Check(e) => match eval_const(&e, env, ctx.state_types) {
                Some(ComptimeVal::Bool(true)) => {}
                Some(ComptimeVal::Bool(false)) => {
                    return Err(format!(
                        "composite '{}': comptime check `{e}` is \
                         FALSE for this instantiation — the call arguments \
                         violate a requirement the composite declares; \
                         adjust the arguments or weaken the check",
                        ctx.composite
                    ));
                }
                _ => out.push(Statement::Check(e)),
            },
            Statement::Guarded(cond, body) => match eval_const(&cond, env, ctx.state_types) {
                Some(ComptimeVal::Bool(true)) => {
                    out.extend(fold_stmt_list(body, env, ctx)?);
                }
                Some(ComptimeVal::Bool(false)) => {}
                _ => {
                    let mut clone = env.clone();
                    let folded = fold_stmt_list(body.clone(), &mut clone, ctx)?;
                    env_kill_tree(&body, env);
                    out.push(Statement::Guarded(cond, folded));
                }
            },
            Statement::Foreach { item, mut list, body } => {
                // Generation happened in unroll_static (PRE-substitution):
                // by the time a composite body reaches the fold, every
                // loop range is caller-derived — a RUNTIME quantity by
                // contract. The loop stays; its body folds under a clone.
                env.remove(&item);
                let mut clone = env.clone();
                let folded = fold_stmt_list(body.clone(), &mut clone, ctx)?;
                env_kill_tree(&body, env);
                fold_expr_in_place(&mut list, env, ctx.state_types);
                out.push(Statement::Foreach {
                    item,
                    list,
                    body: folded,
                });
            }
            Statement::Block(body) => {
                let mut clone = env.clone();
                out.push(Statement::Block(fold_nested(body, &mut clone, ctx)?));
            }
            Statement::SyncBlock(body) => {
                let mut clone = env.clone();
                out.push(Statement::SyncBlock(fold_nested(body, &mut clone, ctx)?));
            }
            Statement::Mutex(body) => {
                let mut clone = env.clone();
                out.push(Statement::Mutex(fold_nested(body, &mut clone, ctx)?));
            }
            Statement::Defer(body) => {
                let mut clone = env.clone();
                out.push(Statement::Defer(fold_nested(body, &mut clone, ctx)?));
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Fold a nested statement list under a cloned env (the clone is local to
/// the enclosing block and discarded — names bound inside do not escape).
fn fold_nested(
    body: Vec<Statement>,
    clone: &mut HashMap<String, ComptimeVal>,
    ctx: &FoldCtx,
) -> Result<Vec<Statement>, String> {
    fold_stmt_list(body, clone, ctx)
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
                     binder (exposed `ExprItem` binders may be referenced)",
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
    // 2026-09-22 (reflection-conditions plan): the declared STATIC TYPE of
    // each top-level state variable seeds the reflection fold. A composite
    // may gate on `x.^^Size` / `x.^^Element` where `x` is a state decl
    // (`let buf: Float[4]`); the receiver's type resolves the target.
    // A receiver that is not a state name declines (fail-open).
    let state_types: HashMap<String, Type> = collect_state_types(items);
    // Top-level `const` declarations are comptime by definition — their
    // values seed the fold env in declaration order (a const init may
    // reference an earlier const). A non-foldable init simply contributes
    // nothing (fail-open: dependent spans degrade to runtime).
    for item in items.iter() {
        if let TopLevel::Constant(k) = item {
            if let Some(v) = eval_const(&k.expr, &comptime, &state_types) {
                comptime.insert(k.name.clone(), v);
            }
        }
    }
    let mut total = 0;
    for depth in 0..8 {
        let mut n = 0;
        for item in items.iter_mut() {
            n += expand_top_level(item, &registry, &comptime, &state_types)?;
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

/// Read-only context threaded through the fold (2026-09-22
/// reflection-conditions plan): the declared static types of state vars
/// and the composite name (error context). Bundled so the fold functions
/// stay within the param budget as the feature grows.
struct FoldCtx<'a> {
    state_types: &'a HashMap<String, Type>,
    composite: &'a str,
}

/// The comptime-foldable projection of a stage-evaluator value (plan
/// 2026-09-21): scalars only — the fold never guesses at structures.
fn nav_comptime(v: &crate::macros::eval::NavValue) -> Option<ComptimeVal> {
    match v {
        crate::macros::eval::NavValue::Int(i) => Some(ComptimeVal::Int(*i)),
        crate::macros::eval::NavValue::Bool(b) => Some(ComptimeVal::Bool(*b)),
        crate::macros::eval::NavValue::Count(c) => Some(ComptimeVal::Int(*c as i64)),
        crate::macros::eval::NavValue::Str(s) => Some(ComptimeVal::Str(s.clone())),
        crate::macros::eval::NavValue::List(xs) => {
            let mut out = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(nav_comptime(x)?);
            }
            Some(ComptimeVal::List(out))
        }
        _ => None,
    }
}

/// Expand composite invocations in one top-level item (2026-09-22
/// unified-metaprogramming plan, C4): the uniform walker covers EVERY
/// statement-bearing TopLevel — reactive txns, compile-time defns, runtime
/// defns, operator members, cells (their member txns/defns), top-level
/// statements, and triggers. Replaces the old Transaction-only walk so a
/// composite call is expanded in any body it appears in.
fn expand_top_level(
    item: &mut TopLevel,
    registry: &HashMap<&String, &Definition>,
    comptime: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Result<usize, String> {
    match item {
        TopLevel::Definition(d) | TopLevel::TypeDefOperator(d) => {
            expand_stmt_list(&mut d.body, registry, comptime, state_types)
        }
        TopLevel::Transaction(t) => {
            expand_stmt_list(&mut t.body, registry, comptime, state_types)
        }
        TopLevel::CompileTimeDefn(d) => {
            expand_stmt_list(&mut d.body, registry, comptime, state_types)
        }
        TopLevel::Statement(stmt) => {
            let mut one = vec![(**stmt).clone()];
            let n = expand_stmt_list(&mut one, registry, comptime, state_types)?;
            if n > 0 {
                // The expansion may have spliced multiple statements in place
                // of one; a single-statement slot can hold at most one. Keep
                // the first spliced statement (composite calls in top-level
                // statement position are statement composites — their splice
                // is one statement).
                *stmt = Box::new(one.into_iter().next().unwrap_or(Statement::Break));
            }
            Ok(n)
        }
        TopLevel::Cell(cell) => {
            let mut n = 0;
            for t in cell.transactions.iter_mut() {
                n += expand_stmt_list(&mut t.body, registry, comptime, state_types)?;
            }
            for d in cell.definitions.iter_mut() {
                n += expand_stmt_list(&mut d.body, registry, comptime, state_types)?;
            }
            Ok(n)
        }
        _ => Ok(0),
    }
}

/// Expand statement-position composite invocations in one statement list.
fn expand_stmt_list(
    stmts: &mut Vec<Statement>,
    registry: &HashMap<&String, &Definition>,
    comptime: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Result<usize, String> {
    let mut n = 0;
    let mut i = 0;
    while i < stmts.len() {
        // Depth first — nested bodies expand before their parents, so an
        // enclosing splice never hides an inner call site.
        expand_nested(&mut stmts[i], registry, comptime, state_types)?;
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
                let expanded = expand_composite_invocation(def, &args, comptime, state_types)?;
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

/// Recurse into the statement kinds that carry nested statement lists, and
/// expand value-position composite calls inside expressions.
fn expand_nested(
    s: &mut Statement,
    registry: &HashMap<&String, &Definition>,
    comptime: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Result<(), String> {
    match s {
        Statement::Foreach { body, .. }
        | Statement::Guarded(_, body)
        | Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body) => {
            expand_stmt_list(body, registry, comptime, state_types)?;
        }
        // 2026-09-22 (unified-metaprogramming plan, C2): expression-position
        // composites (`let r = f!(x)`, `r = f!(x)`, `f!(x);`) expand to a
        // value block. Nested value calls are resolved depth-first by the
        // expression walker's recursion.
        Statement::Let { expr: Some(e), .. } => {
            expand_expr_values(e, registry, comptime, state_types)?;
        }
        Statement::Assign(_, r) => {
            expand_expr_values(r, registry, comptime, state_types)?;
        }
        Statement::Expression(e) => {
            // A bare `name!(...)` statement is the statement-position path —
            // expand_stmt_list splices it. Only walk NESTED value calls
            // (e.g. `f!(x).field`, `g(f!(x))`).
            if !matches!(&*e, Expr::PluginIntercept { receiver: None, .. }) {
                expand_expr_values(e, registry, comptime, state_types)?;
            }
        }
        Statement::Term(Some(e)) => {
            expand_expr_values(e, registry, comptime, state_types)?;
        }
        Statement::Check(e) | Statement::Gate(e) => {
            expand_expr_values(e, registry, comptime, state_types)?;
        }
        _ => {}
    }
    Ok(())
}

/// Expand value-position composite invocations throughout an expression
/// (2026-09-22 unified-metaprogramming plan, C2). A `name!(args...)` whose
/// name resolves to a value composite becomes `Expr::Block` of the expanded
/// body; the walk recurses into the expansion result so a composite body's
/// nested calls also resolve (the fixpoint loop re-walks statements, this
/// recursion covers nested expressions inside one call).
fn expand_expr_values(
    e: &mut Expr,
    registry: &HashMap<&String, &Definition>,
    comptime: &HashMap<String, ComptimeVal>,
    state_types: &HashMap<String, Type>,
) -> Result<(), String> {
    match e {
        Expr::PluginIntercept {
            name,
            args,
            receiver: None,
            ..
        } => {
            if let Some(def) = registry.get(name) {
                let block = expand_composite_value(def, args, comptime, state_types)?;
                // Recurse into the expanded block so nested value calls in
                // the composite body resolve before the block is spliced.
                let mut inner = block;
                expand_expr_values(&mut inner, registry, comptime, state_types)?;
                *e = inner;
            }
        }
        Expr::Call(_, a, _) => {
            for x in a.iter_mut() {
                expand_expr_values(x, registry, comptime, state_types)?;
            }
        }
        Expr::MethodCall(r, _, a, _, _) => {
            expand_expr_values(r, registry, comptime, state_types)?;
            for x in a.iter_mut() {
                expand_expr_values(x, registry, comptime, state_types)?;
            }
        }
        Expr::BinaryOp(_, l, r) => {
            expand_expr_values(l, registry, comptime, state_types)?;
            expand_expr_values(r, registry, comptime, state_types)?;
        }
        Expr::UnaryOp(_, x) => expand_expr_values(x, registry, comptime, state_types)?,
        Expr::Index(o, i) => {
            expand_expr_values(o, registry, comptime, state_types)?;
            expand_expr_values(i, registry, comptime, state_types)?;
        }
        Expr::Cast(x, _) | Expr::Deref(x) | Expr::AddrOf(x) | Expr::Consume(x)
        | Expr::Await(x) => expand_expr_values(x, registry, comptime, state_types)?,
        Expr::Range { start, end, .. } => {
            expand_expr_values(start, registry, comptime, state_types)?;
            expand_expr_values(end, registry, comptime, state_types)?;
        }
        Expr::Tuple(xs) | Expr::List(xs) => {
            for x in xs.iter_mut() {
                expand_expr_values(x, registry, comptime, state_types)?;
            }
        }
        Expr::Match(scrut, arms) => {
            expand_expr_values(scrut, registry, comptime, state_types)?;
            for a in arms.iter_mut() {
                if let Some(g) = a.guard.as_mut() {
                    expand_expr_values(g, registry, comptime, state_types)?;
                }
                expand_expr_values(a.body.as_mut(), registry, comptime, state_types)?;
            }
        }
        Expr::Block(stmts) => {
            for s in stmts.iter_mut() {
                expand_nested(s, registry, comptime, state_types)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// 2026-09-22 (unified-metaprogramming plan): splice a `foreach item in
/// <rest>` loop whose list is the composite's `...` rest parameter. The rest
/// list is the sanctioned compile-time iteration channel: the loop becomes
/// ONE body copy per trailing argument, each `item` substituted with the
/// ORIGINAL argument expression (emission — `execute_many!(f(a), f(b))`
/// emits `f(a); f(b);`). The splice happens AFTER fixed-param substitution,
/// so the rest name is still a bare identifier in the body. Any other
/// `foreach` (runtime list) is kept verbatim.
fn splice_rest_foreach(
    stmts: Vec<Statement>,
    rest_name: &str,
    rest_args: &[Expr],
) -> Vec<Statement> {
    let mut out: Vec<Statement> = Vec::with_capacity(stmts.len());
    for s in stmts {
        match s {
            Statement::Foreach {
                item,
                list,
                body,
            } => {
                let is_rest = match list.as_ref() {
                    Expr::Identifier(n) => n == rest_name,
                    _ => false,
                };
                if is_rest {
                    out.extend(splice_rest_iteration(&item, &body, rest_args));
                } else {
                    out.push(Statement::Foreach {
                        item,
                        list,
                        body: splice_nested(body, rest_name, rest_args),
                    });
                }
            }
            Statement::Guarded(cond, body) => {
                out.push(Statement::Guarded(
                    cond,
                    splice_nested(body, rest_name, rest_args),
                ));
            }
            Statement::Block(body) => {
                out.push(Statement::Block(splice_nested(
                    body,
                    rest_name,
                    rest_args,
                )));
            }
            Statement::SyncBlock(body) => {
                out.push(Statement::SyncBlock(splice_nested(
                    body,
                    rest_name,
                    rest_args,
                )));
            }
            Statement::Mutex(body) => {
                out.push(Statement::Mutex(splice_nested(
                    body,
                    rest_name,
                    rest_args,
                )));
            }
            Statement::Defer(body) => {
                out.push(Statement::Defer(splice_nested(
                    body,
                    rest_name,
                    rest_args,
                )));
            }
            other => out.push(other),
        }
    }
    out
}

/// One body copy per rest argument, each `item` substituted with the arg
/// expression. Empty rest is defensive (caller errors first).
fn splice_rest_iteration(
    item: &str,
    body: &[Statement],
    rest_args: &[Expr],
) -> Vec<Statement> {
    let mut out = Vec::new();
    for arg in rest_args {
        let mut copy = body.to_vec();
        for st in copy.iter_mut() {
            substitute_param(st, item, arg);
        }
        out.extend(copy);
    }
    out
}

/// Recurse into a nested statement list so a rest-foreach inside a
/// guarded/block/defer body also splices.
fn splice_nested(body: Vec<Statement>, rest_name: &str, rest_args: &[Expr]) -> Vec<Statement> {
    splice_rest_foreach(body, rest_name, rest_args)
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
        | Statement::Mutex(body) => {
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
        Expr::Call(name, args, _) => {
            // 2026-09-22 (unified-metaprogramming plan): a parameter used as a
            // CALLABLE (`tag(c)` where `tag` is a fixed expr param bound to a
            // callee expression) must substitute the callee too — not just the
            // argument spine. `Sink#`-style callees flow through fixed params.
            if name == param {
                *name = match arg {
                    Expr::Identifier(n) => n.clone(),
                    _ => return,
                };
            }
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
            subst_match_arms(scrut, arms, param, arg);
        }
        Expr::Block(stmts) => {
            for s in stmts.iter_mut() {
                substitute_param(s, param, arg);
            }
        }
        // 2026-09-22 (unified-metaprogramming plan): a nested `name!(...)`
        // inside a composite body carries parameter references in its args —
        // substitute them so `execute_many!(f, (x), (x+1))`-style nesting
        // resolves the parameter through the composite boundary.
        Expr::PluginIntercept {
            name: _,
            args,
            type_args: _,
            receiver,
            chain_refs: _,
        } => {
            subst_intercept_args(args, receiver, param, arg);
        }
        _ => {}
    }
}

/// Substitute inside a nested `name!(...)` intercept's args and receiver —
/// a composite parameter flows through the nested macro boundary
/// (2026-09-22 unified-metaprogramming plan).
fn subst_intercept_args(
    args: &mut [Expr],
    receiver: &mut Option<Box<Expr>>,
    param: &str,
    arg: &Expr,
) {
    for a in args.iter_mut() {
        subst_expr(a, param, arg);
    }
    if let Some(r) = receiver.as_mut() {
        subst_expr(r, param, arg);
    }
}

/// Substitute inside a statement-form `match` expression (F1 unified
/// dispatch): scrutinee, arm patterns, guards, and block bodies all carry
/// parameter references.
fn subst_match_arms(
    scrut: &mut Expr,
    arms: &mut [MatchArm],
    param: &str,
    arg: &Expr,
) {
    subst_expr(scrut, param, arg);
    for a in arms.iter_mut() {
        subst_pattern(&mut a.pattern, param, arg);
        if let Some(g) = a.guard.as_mut() {
            subst_expr(g, param, arg);
        }
        subst_expr(a.body.as_mut(), param, arg);
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
        | Statement::Mutex(body) => {
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
            "$defn f(x: Expr, y: Expr) { \n z = x + y; \n}; \nlet q: Int = 0;",
        );
        let d = defn_of(&items, "f");
        assert!(is_composite(&d), "`expr` params mark a composite");
        let (subst, exposed, _rest) = composite_signature(&d).unwrap();
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
            "$defn f(fill: Expr, n: Expr, i: ExprItem) { \n\
             \x20 foreach i in 0..n { \n\
             \x20  buf[i] = fill; \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let (subst, exposed, _rest) = composite_signature(&d).unwrap();
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
            &HashMap::new(),
        )
        .expect("exposed-binder reference is legal");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn expansion_substitutes_nested_positions() {
        let items = parse_program(
            "$defn f(p: Expr, q: Expr) { \n\
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
            expand_composite_invocation(&d, &[arg_p, arg_q], &HashMap::new(), &HashMap::new())
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
            "$defn f(p: Expr) { let t = p * 2; res = t; };",
        );
        let d = defn_of(&items, "f");
        // The arg mentions `t` — the body binds `t`. Capture => error.
        let arg = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("t".into())),
            Box::new(Expr::Decimal(1)),
        );
        let err =
            expand_composite_invocation(&d, &[arg], &HashMap::new(), &HashMap::new()).unwrap_err();
        assert!(err.contains("captures") || err.contains("capture"), "{err}");
    }

    #[test]
    fn arg_referencing_another_param_is_rejected() {
        let items = parse_program("$defn f(p: Expr, q: Expr) { res = p + q; };");
        let d = defn_of(&items, "f");
        let arg_p = Expr::Decimal(1);
        let arg_q = Expr::BinaryOp(
            BinaryOpKind::Add,
            Box::new(Expr::Identifier("p".into())),
            Box::new(Expr::Decimal(2)),
        );
        let err = expand_composite_invocation(&d, &[arg_p, arg_q], &HashMap::new(), &HashMap::new())
            .unwrap_err();
        assert!(err.contains("parameter 'p'"), "{err}");
    }

    #[test]
    fn contract_gates_are_spliced() {
        let items = parse_program(
            "$defn f(p: Expr) [i < 8] [i >= 0] { res = p; };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("v".into())],
            &HashMap::new(),
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
$defn scale_into(dst: Expr, srcv: Expr) { dst = srcv * 2; };
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
            "$defn f(p: Expr, n: Int) { res = p + n; };",
        );
        let d = defn_of(&items, "f");
        let err = expand_composite_invocation(
            &d,
            &[Expr::Decimal(1), Expr::Decimal(2)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(err.contains("mixes"), "{err}");
    }

    #[test]
    fn arg_count_mismatch_diagnoses_params() {
        let items = parse_program("$defn f(p: Expr, q: Expr) { res = p + q; };");
        let d = defn_of(&items, "f");
        let err = expand_composite_invocation(
            &d,
            &[Expr::Decimal(1)],
            &HashMap::new(),
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
            "$defn f(n: Expr) { \n\
             \x20 match n <= 32 { \n\
             \x20  true => { small = 1; }, \n\
             \x20  false => { large = 1; }, \n\
             \x20 }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        expand_composite_invocation(&d, &[span], &HashMap::new(), &HashMap::new()).expect("expands")
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
            "$defn f(n: Expr) { \n\
             \x20 let tile: Int = n / 4; \n\
             \x20 match tile > 8 { \n\
             \x20  true => { wide = tile; }, \n\
             \x20  false => { narrow = tile; }, \n\
             \x20 }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(64)], &HashMap::new(), &HashMap::new())
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
            "$defn f(n: Expr) { match n <= 32 { true => { s = 1; }, false => { l = 1; }, }; };",
        );
        let d = defn_of(&items, "f");
        let mut seed = HashMap::new();
        seed.insert("NKV".to_string(), ComptimeVal::Int(4096));
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("NKV".into())],
            &seed,
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("\"l\"") && !dump.contains("\"s\""), "{dump}");
    }

    #[test]
    fn top_level_const_seeds_the_driver_fold() {
        let src = "\
$defn f(n: Expr) { match n <= 32 { true => { s = 1; }, false => { l = 1; }, } };
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
            "$defn f(n: Expr) { \n\
             \x20 when n > 100 { big = 1; }; \n\
             \x20 when n > 1000 { huge = 1; }; \n\
             \x20 when n > 0 { pos = 1; }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(500)], &HashMap::new(), &HashMap::new())
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
            "$defn f(n: Expr, w: Expr) { \n\
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
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Match"), "t mutated at runtime: {dump}");
    }

    #[test]
    fn comptime_range_unrolls_and_prunes_per_iteration() {
        // LITERAL range in the declaration = generation (static text).
        let items = parse_program(
            "$defn f(p: Expr) { \n\
             \x20 foreach j in 0..16 { \n\
             \x20  match j < 4 { true => { lo = 1; }, false => { hi = 1; }, } \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(0)], &HashMap::new(), &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        let los = dump.matches("\"lo\"").count();
        let his = dump.matches("\"hi\"").count();
        assert_eq!(los, 4, "j=0..3 take the true arm: {dump}");
        assert_eq!(his, 12, "j=4..15 take the false arm: {dump}");
        assert!(!dump.contains("Foreach"), "unrolled: {dump}");
        assert!(!dump.contains("Match"), "per-iteration pruning: {dump}");
    }

    #[test]
    fn param_derived_range_stays_a_runtime_loop() {
        // A range over an expr PARAMETER is a runtime quantity even when
        // this call passes a literal — caller spans never generate.
        let items = parse_program(
            "$defn f(n: Expr) { \n\
             \x20 foreach j in 0..n { \n\
             \x20  work = 1; \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(16)], &HashMap::new(), &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Foreach"), "param range stays runtime: {dump}");
    }

    #[test]
    fn runtime_list_keeps_the_runtime_foreach() {
        let items = parse_program(
            "$defn f(rows: Expr) { \n\
             \x20 foreach j in rows { \n\
             \x20  match j < 4 { true => { lo = 1; }, false => { hi = 1; }, } \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("R".into())],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Foreach"), "R is runtime: {dump}");
        assert!(dump.contains("Match"), "j unknown: {dump}");
    }

    #[test]
    fn comptime_list_literal_unrolls_per_element() {
        let items = parse_program(
            "$defn f(p: Expr) { \n\
             \x20 foreach w in [1, 2, 4] { \n\
             \x20  acc[p + w] = w; \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(0)], &HashMap::new(), &HashMap::new())
            .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(1)") && dump.contains("Decimal(2)")
            && dump.contains("Decimal(4)"), "each element spliced: {dump}");
        assert!(!dump.contains("Foreach"), "unrolled: {dump}");
        assert!(!dump.contains("\"w\""), "item substituted: {dump}");
    }

    #[test]
    fn comptime_const_list_unrolls_per_element() {
        let items = parse_program(
            "$defn f() { \n\
             \x20 foreach w in PHASES { \n\
             \x20  acc[0] = acc[0] + w; \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let mut seed = HashMap::new();
        seed.insert(
            "PHASES".to_string(),
            ComptimeVal::List(vec![
                ComptimeVal::Int(1),
                ComptimeVal::Int(2),
                ComptimeVal::Int(4),
                ComptimeVal::Int(8),
            ]),
        );
        let out =
            expand_composite_invocation(&d, &[], &seed, &HashMap::new()).expect("expands");
        let dump = format!("{out:?}");
        assert_eq!(dump.matches("Decimal(8)").count(), 1, "w=8 spliced: {dump}");
        assert!(!dump.contains("Foreach"), "unrolled: {dump}");
    }

    #[test]
    fn comptime_check_true_splices_false_errors() {
        let items = parse_program(
            "$defn f(n: Expr) { \n\
             \x20 check n <= 32; \n\
             \x20 mark = n; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let ok = expand_composite_invocation(&d, &[Expr::Decimal(16)], &HashMap::new(), &HashMap::new())
            .expect("16 <= 32 proven");
        let dump = format!("{ok:?}");
        assert!(!dump.contains("Check"), "proven check spliced out: {dump}");
        assert!(dump.contains("\"mark\""), "body present: {dump}");
        let err = expand_composite_invocation(&d, &[Expr::Decimal(64)], &HashMap::new(), &HashMap::new())
            .unwrap_err();
        assert!(err.contains("comptime check") && err.contains("FALSE"), "{err}");
        // Non-foldable check degrades to the runtime path.
        let rt = expand_composite_invocation(
            &d,
            &[Expr::Identifier("N".into())],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("runtime check kept");
        assert!(format!("{rt:?}").contains("Check"), "{rt:?}");
    }

    #[test]
    fn arithmetic_on_unrolled_item_folds() {
        let items = parse_program(
            "$defn f() { \n\
             \x20 foreach k in 0..3 { \n\
             \x20  stage[(1 << k)] = k; \n\
             \x20 } \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[], &HashMap::new(), &HashMap::new()).expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(1)") && dump.contains("Decimal(2)")
            && dump.contains("Decimal(4)"), "1<<k folded per iteration: {dump}");
        assert!(!dump.contains("Shl"), "shift folded away: {dump}");
    }

    #[test]
    fn nested_adaptive_arms_fold_recursively() {
        let items = parse_program(
            "$defn f(n: Expr) { \n\
             \x20 match n <= 32 { \n\
             \x20  true => { match n <= 8 { true => { tiny = 1; }, false => { small = 1; }, }; }, \n\
             \x20  false => { large = 1; }, \n\
             \x20 }; \n\
             };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(4)], &HashMap::new(), &HashMap::new())
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
            "$defn f(n: Expr) { match n { x => { any = x; }, }; };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(7)], &HashMap::new(), &HashMap::new())
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
            "$defn f(n: Expr) { let t: Int = n * 2; match t > 8 { true => { a = 1; }, false => { b = 1; }, }; };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Decimal(i64::MAX)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Match"), "overflow declines: {dump}");
    }

#[test]
    fn contracts_survive_folding() {
        let items = parse_program(
            "$defn f(n: Expr) [n < 64] [n > 0] { match n <= 32 { true => { s = 1; }, false => { l = 1; }, } };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(&d, &[Expr::Decimal(16)], &HashMap::new(), &HashMap::new())
            .expect("expands");
        assert_eq!(out.len(), 3, "pre gate + spliced arm + post gate");
        assert!(matches!(&out[0], Statement::Gate(_)));
        assert!(matches!(&out[2], Statement::Gate(_)));
    }

    // ── 2026-09-22 (reflection-conditions plan) ────────────────────────
    // `.^^Size` / `.^^Element` on a declared state variable fold from its
    // static type at expansion time.

    fn state_types_of(items: &[TopLevel]) -> HashMap<String, Type> {
        collect_state_types(items)
    }

    #[test]
    fn reflect_size_folds_from_state_decl() {
        // A composite gates on `x.^^Size`; the arg names a `let buf: Float[4]`.
        let items = parse_program(
            "let buf: Float[4]; \n\
             $defn pick(x: Expr) { match x.^^Size { 4 => { r = 1; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "pick");
        let state_types = state_types_of(&items);
        assert_eq!(state_types.get("buf"), Some(&Type::Vector(
            Box::new(Type::Custom("Float".to_string())),
            vec![crate::ast::Dimension::Anonymous(4)]
        )));
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("buf".into())],
            &HashMap::new(),
            &state_types,
        )
        .expect("expands");
        // Only the `4` arm splices — the `_` arm is gone.
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(1)"), "taken arm spliced: {dump}");
        assert!(!dump.contains("Decimal(0)"), "_ arm pruned: {dump}");
    }

    #[test]
    fn reflect_size_mismatch_splices_other_arm() {
        let items = parse_program(
            "let buf: Float[8]; \n\
             $defn pick(x: Expr) { match x.^^Size { 4 => { r = 1; }, 8 => { r = 8; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "pick");
        let state_types = state_types_of(&items);
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("buf".into())],
            &HashMap::new(),
            &state_types,
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(8)"), "size-8 arm spliced: {dump}");
        assert!(!dump.contains("Decimal(1)"), "size-4 arm pruned: {dump}");
        assert!(!dump.contains("Decimal(0)"), "_ arm pruned: {dump}");
    }

    #[test]
    fn reflect_element_folds_category_code() {
        // `.^^Element` on Float[16] folds to category 1 (Float).
        let items = parse_program(
            "let buf: Float[16]; \n\
             $defn kind(x: Expr) { match x.^^Element { 1 => { r = 1; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "kind");
        let state_types = state_types_of(&items);
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("buf".into())],
            &HashMap::new(),
            &state_types,
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(1)"), "Float category arm spliced: {dump}");
        assert!(!dump.contains("Decimal(0)"), "_ arm pruned: {dump}");
    }

    #[test]
    fn reflect_unknown_receiver_stays_runtime() {
        // A receiver that is NOT a state decl declines the fold — the match
        // stays a runtime branch (fail-open).
        let items = parse_program(
            "$defn pick(x: Expr) { match x.^^Size { 4 => { r = 1; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "pick");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("runtime_val".into())],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Match"), "runtime match kept: {dump}");
    }

    #[test]
    fn reflect_string_element_is_char_category() {
        // `.^^Element` on a String state var folds to the Char category (3).
        let items = parse_program(
            "let s: String; \n\
             $defn ch(x: Expr) { match x.^^Element { 3 => { r = 1; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "ch");
        let state_types = state_types_of(&items);
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("s".into())],
            &HashMap::new(),
            &state_types,
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(1)"), "Char category arm spliced: {dump}");
        assert!(!dump.contains("Decimal(0)"), "_ arm pruned: {dump}");
    }

    // ── 2026-09-22 (unified-metaprogramming plan) ────────────────────────
    // Variadic composites: `$defn f(...calls: Expr)` binds ALL trailing args
    // as the sanctioned compile-time iteration channel — a `foreach c in
    // calls` splices one body copy per arg, `c` substituted with the arg.

    #[test]
    fn rest_param_foreach_splices_one_call_per_arg() {
        let items = parse_program(
            "$defn execute_many(...calls: Expr) { foreach c in calls { c; } };",
        );
        let d = defn_of(&items, "execute_many");
        let out = expand_composite_invocation(
            &d,
            &[
                Expr::Call("emit".into(), vec![Expr::Decimal(1)], None),
                Expr::Call("emit".into(), vec![Expr::Decimal(2)], None),
                Expr::Call("emit".into(), vec![Expr::Decimal(3)], None),
            ],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        assert_eq!(out.len(), 3, "one emission per arg: {out:?}");
        for (i, st) in out.iter().enumerate() {
            let Statement::Expression(Expr::Call(name, args, _)) = st else {
                panic!("expected a call emission, got {st:?}");
            };
            assert_eq!(name, "emit");
            assert!(matches!(&args[0], Expr::Decimal(n) if *n == (i as i64) + 1));
        }
    }

    #[test]
    fn rest_param_with_fixed_params_binds_trailing() {
        // A fixed `expr` param plus a `...` rest: the first arg binds the
        // fixed param, the rest iterate.
        let items = parse_program(
            "$defn wrap(tag: Expr, ...calls: Expr) { foreach c in calls { tag(c); } };",
        );
        let d = defn_of(&items, "wrap");
        let out = expand_composite_invocation(
            &d,
            &[
                Expr::Identifier("Sink#".into()),
                Expr::Decimal(1),
                Expr::Decimal(2),
            ],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert_eq!(out.len(), 2, "two rest args → two emissions: {dump}");
        assert!(dump.contains("Sink#"), "fixed tag substituted: {dump}");
    }

    #[test]
    fn zero_rest_args_is_an_error() {
        let items = parse_program(
            "$defn execute_many(...calls: Expr) { foreach c in calls { c; } };",
        );
        let d = defn_of(&items, "execute_many");
        let err = expand_composite_invocation(
            &d,
            &[],
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(
            err.contains("zero rest arguments"),
            "expected zero-rest diagnostic, got: {err}"
        );
    }

    #[test]
    fn rest_param_stays_unsubstituted_before_splice() {
        // The rest name is NOT positionally substituted (it is a list, not a
        // single expr) — the splice consumes the foreach over it.
        let items = parse_program(
            "$defn execute_many(...calls: Expr) { foreach c in calls { c; } };",
        );
        let d = defn_of(&items, "execute_many");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Call("f".into(), vec![Expr::Decimal(7)], None)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        assert_eq!(out.len(), 1);
        let Statement::Expression(Expr::Call(name, _, _)) = &out[0] else {
            panic!("expected call, got {:?}", out[0]);
        };
        assert_eq!(name, "f");
    }

    #[test]
    fn runtime_foreach_inside_composite_is_not_spliced() {
        // A `foreach` over a NON-rest list stays a runtime loop.
        let items = parse_program(
            "$defn f(x: Expr) { foreach k in 0..x { emit(k); } };",
        );
        let d = defn_of(&items, "f");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Decimal(4)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Foreach"), "runtime loop kept: {dump}");
    }

    // ── 2026-09-22 (unified-metaprogramming plan, C2) ───────────────────
    // Value-returning composites: `$defn f(...) -> T { ...; term v; }` used
    // in EXPRESSION position expands to an `Expr::Block` whose value is the
    // composite's return. The trailing `term` becomes the block's value
    // statement (never a function return).

    #[test]
    fn value_composite_expands_to_value_block() {
        let items = parse_program(
            "$defn aligned_size(n: Expr) -> Int { term (n + 15) & ~15; };",
        );
        let d = defn_of(&items, "aligned_size");
        let block = expand_composite_value(
            &d,
            &[Expr::Decimal(4)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands to value");
        let Expr::Block(stmts) = block else {
            panic!("expected Expr::Block, got {block:?}");
        };
        assert!(!stmts.is_empty());
        // The trailing term became a value expression — the block yields it.
        assert!(
            matches!(stmts.last(), Some(Statement::Expression(_))),
            "trailing value statement: {stmts:?}"
        );
    }

    #[test]
    fn value_composite_without_term_is_an_error() {
        let items = parse_program(
            "$defn no_value(x: Expr) { let y: Int = x; };",
        );
        let d = defn_of(&items, "no_value");
        let err = expand_composite_value(
            &d,
            &[Expr::Decimal(1)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(
            err.contains("value position"),
            "expected value-position diagnostic, got: {err}"
        );
    }

    #[test]
    fn statement_composite_still_expands_to_statements() {
        // A statement composite (no -> Type) keeps statement expansion;
        // its body's trailing term is NOT converted to a value statement.
        let items = parse_program(
            "$defn execute_many(...calls: Expr) { foreach c in calls { c; } };",
        );
        let d = defn_of(&items, "execute_many");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Call("f".into(), vec![Expr::Decimal(1)], None)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        assert_eq!(out.len(), 1);
        let Statement::Expression(Expr::Call(name, _, _)) = &out[0] else {
            panic!("expected a call, got {:?}", out[0]);
        };
        assert_eq!(name, "f");
    }

    // ── 2026-09-22 (unified-metaprogramming plan, C4) ───────────────────
    // Uniform walker: composite calls expand in EVERY statement-bearing
    // TopLevel, not just reactive txns — runtime defn bodies, operator
    // members, cell members.

    #[test]
    fn composite_expands_in_runtime_defn_body() -> Result<(), String> {
        let items = parse_program(
            "$defn twice(x: Expr) { let r2: Int = x * 2; }; \
             \x20defn compute(v: Int) -> Int { twice!(v); term r2; };",
        );
        // Build a registry from the $defn, then walk the runtime defn.
        let comp = defn_of(&items, "twice");
        let mut registry: HashMap<&String, &Definition> = HashMap::new();
        registry.insert(&comp.name, &comp);
        let mut items = items;
        let n = if let Some(TopLevel::Definition(d)) = items.iter_mut().find(|it| {
            matches!(it, TopLevel::Definition(def) if def.name == "compute")
        }) {
            expand_stmt_list(&mut d.body, &registry, &HashMap::new(), &HashMap::new())?
        } else {
            panic!("compute defn not found");
        };
        assert_eq!(n, 1, "one composite call expanded in the defn body");
        let TopLevel::Definition(d) = &items[1] else {
            panic!("compute defn");
        };
        let dump = format!("{:?}", d.body);
        assert!(!dump.contains("twice!"), "call spliced: {dump}");
        assert!(dump.contains("Mul"), "twice body spliced: {dump}");
        Ok(())
    }

    #[test]
    fn composite_expands_in_operator_member_body() -> Result<(), String> {
        // TypeDefOperator is Defn-shaped — the walker covers it. (In a real
        // parse the op member nests inside `TopLevel::TypeDef`; the driver
        // walk covers it once surfaced, and expand_top_level has the arm.)
        let comp = defn_of(
            &parse_program("$defn twice(x: Expr) { let r2: Int = x * 2; };"),
            "twice",
        );
        let mut registry: HashMap<&String, &Definition> = HashMap::new();
        registry.insert(&comp.name, &comp);
        let mut op_member = crate::ast::top::Definition {
            name: "Count".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: crate::ast::top::Contract::new(Expr::Bool(true), Expr::Bool(true)),
            body: vec![
                Statement::Expression(Expr::PluginIntercept {
                    name: "twice".into(),
                    args: vec![Expr::Decimal(3)],
                    type_args: vec![],
                    receiver: None,
                    chain_refs: vec![],
                }),
                Statement::Term(Some(Expr::Identifier("r2".into()))),
            ],
            metadata: Default::default(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            variadic_param: None,
            span: None,
            doc: None,
        };
        let mut item = TopLevel::TypeDefOperator(op_member);
        let n = expand_top_level(&mut item, &registry, &HashMap::new(), &HashMap::new())?;
        assert_eq!(n, 1, "one composite call expanded in the operator member");
        Ok(())
    }

    // ── 2026-09-22 (unified-metaprogramming plan, C3) ───────────────────
    // One value domain: ComptimeVal gains Str, so a composite body may gate
    // a match/when on a string literal (`match s { "abc" => … }`) and string
    // equality folds. NavValue::Str bridges into the fold env.

    #[test]
    fn string_literal_match_folds_to_taken_arm() {
        let items = parse_program(
            "$defn pick(s: Expr) { match s { \"abc\" => { r = 1; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "pick");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Quoted("abc".into())],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(1)"), "taken arm spliced: {dump}");
        assert!(!dump.contains("Decimal(0)"), "_ arm pruned: {dump}");
    }

    #[test]
    fn string_neq_folds_to_other_arm() {
        let items = parse_program(
            "$defn pick(s: Expr) { match s { \"abc\" => { r = 1; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "pick");
        let out = expand_composite_invocation(
            &d,
            &[Expr::Quoted("xyz".into())],
            &HashMap::new(),
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(0)"), "other arm spliced: {dump}");
        assert!(!dump.contains("Decimal(1)"), "abc arm pruned: {dump}");
    }

    #[test]
    fn nav_str_bridges_into_fold_env() {
        // A NavValue::Str (from a $let) seeds the fold env and gates a match.
        let items = parse_program(
            "$let mode = \"fast\";\n\
             $defn pick(s: Expr) { match s { \"fast\" => { r = 1; }, _ => { r = 0; }, } };",
        );
        let d = defn_of(&items, "pick");
        let mut comptime: HashMap<String, ComptimeVal> = HashMap::new();
        comptime.insert(
            "mode".to_string(),
            ComptimeVal::Str("fast".to_string()),
        );
        let out = expand_composite_invocation(
            &d,
            &[Expr::Identifier("mode".into())],
            &comptime,
            &HashMap::new(),
        )
        .expect("expands");
        let dump = format!("{out:?}");
        assert!(dump.contains("Decimal(1)"), "fast arm spliced: {dump}");
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
