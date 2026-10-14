// ── Variadic Specialization (2026-10-13) ────────────────────────────────
// Typed rest parameters on runtime `defn`/`txn` are specialized per
// call-site arity BEFORE typecheck (SPEC §9.1, 2026-10-13). Each distinct
// arity gets one monomorphized copy with the rest name rewritten to the
// element parameter names; the call site is retargeted at the
// instantiation. Zero runtime cost: the emitted body is a fixed-arity
// function taking the rest elements as ordinary parameters.
//
// Pipeline position: after composite expansion, before typecheck. It is
// ADDITIVE (Rule 6): no existing pass is modified — it appends
// instantiations to the item list and rewrites call sites.
//
// v1 scope (plan §Scope exclusions): defn + txn only (isr / op member
// reject the rest at parse); the rest name is rewritten at its USE SITES
// (`xs[i]`, `xs.Count#()`, bare `xs`) — no splice-apply into intrinsics.
use crate::ast::{Statement, Expr};
use crate::ast::top::{Definition, Transaction, TopLevel, VariadicParam};
use crate::ast::Type;
use std::collections::HashMap;

/// Default rest bound when no explicit bound is given (warn-the-default /
/// error-when-needed, keyword-taxonomy.md soundness rule).
pub const DEFAULT_MAX: usize = 16;

/// Instantiation name for (callable, arity): `name::arity`. The `::` is the
/// internal-delimiter convention (generic instantiation names use the same
/// `concrete::variant` form, src/backend/llvm/mod.rs:3181). It never appears
/// in an authored identifier, so an instantiation can never shadow a user
/// name. `::` is NOT a valid LLVM identifier, so the emitted IR symbol uses
/// `__` instead (see `emitted_instantiation_name`); the `::` form is the
/// Rust-side de-dup key.
pub fn instantiation_name(name: &str, arity: usize) -> String {
    format!("{name}::{arity}")
}

/// The name that reaches the LLVM IR. `::` is invalid in an LLVM identifier
/// (the assembler rejects `define i64 @sum::3`), so the emitted symbol uses
/// `__` — a valid identifier, matching the `__spawn_next_`/`__view_items_`
/// internal-symbol convention. The user's source name is unchanged.
pub fn emitted_instantiation_name(name: &str, arity: usize) -> String {
    format!("{name}__{arity}")
}

/// True when `n` is an emitted instantiation of one of the declared
/// variadics in `variadics` (i.e. `name__arity` for a declared `name`).
/// Used to skip reprocessing already-emitted instantiations. A user-authored
/// name can never match: it would have to equal a declared variadic's name
/// followed by `__` + a digit, which is not a valid plain identifier the
/// author writes.
pub fn is_instantiation_name(n: &str, variadics: &HashMap<String, (usize, bool)>) -> bool {
    if let Some(idx) = n.rfind("__") {
        let base = &n[..idx];
        let suffix = &n[idx + 2..];
        if suffix.chars().all(|c| c.is_ascii_digit()) && !suffix.is_empty() {
            return variadics.contains_key(base);
        }
    }
    false
}

/// True when the element type is a VALUE type (not `Expr`/`ExprItem`).
/// Composites keep the `Expr` rest and never reach this pass.
fn is_value_type(t: &Type) -> bool {
    !matches!(t, Type::Custom(n) if n == "Expr" || n == "ExprItem")
}

/// A declared callable may be wrapped in `export` (`export defn syscall(...)`
/// → `TopLevel::Export(Export { inner: Definition })`) — the variadic data
/// lives on the INNER decl.
fn unwrap_export(item: &TopLevel) -> &TopLevel {
    match item {
        TopLevel::Export(e) => &e.inner,
        other => other,
    }
}

fn unwrap_export_mut(item: &mut TopLevel) -> &mut TopLevel {
    match item {
        TopLevel::Export(e) => &mut e.inner,
        other => other,
    }
}

/// Specialize all typed-variadic `defn`/`txn` declarations in `items`.
/// Appends one instantiation per (callable, arity) pair and retargets the
/// call sites. The original declaration is retained (diagnostics + name
/// resolution) with `variadic_param` cleared so the typechecker sees a
/// fixed-arity signature.
pub fn specialize_variadic(
    items: &mut Vec<TopLevel>,
) -> Result<(), String> {
    // Index the variadic declarations by name → (item index, is_txn).
    let mut variadics: HashMap<String, (usize, bool)> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        // 2026-10-14: `export defn syscall(...)` parses to
        // TopLevel::Export(Export { inner: Definition }) — the rest lives on
        // the INNER decl. Without the unwrap, an exported variadic is
        // invisible to the pass: the original body (args.Count#()) reaches
        // the typechecker and every `brievc check` of the declaring module
        // fails (the conformance sweep caught it on posix/syscall.bv).
        match unwrap_export(item) {
            TopLevel::Definition(d) => {
                if d.variadic_param
                    .as_ref()
                    .is_some_and(|vp| is_value_type(&vp.element_type))
                {
                    variadics.insert(d.name.clone(), (i, false));
                }
            }
            TopLevel::Transaction(t) => {
                if t.variadic_param
                    .as_ref()
                    .is_some_and(|vp| is_value_type(&vp.element_type))
                {
                    variadics.insert(t.name.clone(), (i, true));
                }
            }
            _ => {}
        }
    }
    if variadics.is_empty() {
        return Ok(());
    }

    let mut ctx = Ctx {
        variadics,
        items,
        // rest name → element param names per instantiation
        elems: HashMap::new(),
        instantiations: Vec::new(),
    };

    let mut new_items: Vec<TopLevel> = Vec::new();
    for item in items.iter() {
        let mut rewritten = item.clone();
        match unwrap_export_mut(&mut rewritten) {
            TopLevel::Definition(d) => rewrite_definition(d, &mut ctx)?,
            TopLevel::Transaction(t) => rewrite_transaction(t, &mut ctx)?,
            _ => {}
        }
        // Clear the rest on the ORIGINAL variadic declaration AND empty its
        // body: the real work lives in the instantiations. The original is
        // retained for diagnostics + name resolution, but its body is never
        // typechecked or emitted (an empty body = a body-less signature,
        // which the typechecker accepts as a staged declaration).
        // GUARD: only items that ARE declared variadic get emptied — a
        // non-variadic defn/node/txn must keep its body (the unconditional
        // wipe here emptied `node main`'s body, dropping its statements).
        match unwrap_export_mut(&mut rewritten) {
            TopLevel::Definition(d) => {
                if d.variadic_param.is_some() {
                    d.variadic_param = None;
                    d.body = vec![];
                }
            }
            TopLevel::Transaction(t) => {
                if t.variadic_param.is_some() {
                    t.variadic_param = None;
                    t.body = vec![];
                }
            }
            _ => {}
        }
        new_items.push(rewritten);
    }
    new_items.append(&mut ctx.instantiations);
    *items = new_items;
    Ok(())
}

/// Context for the walk: the declared variadics, the ORIGINAL items (for
/// fixed-param counts and instantiation cloning), and the emitted
/// instantiations (accumulated in source order of first call-site sighting).
struct Ctx<'a> {
    variadics: HashMap<String, (usize, bool)>,
    items: &'a [TopLevel],
    elems: HashMap<String, Vec<String>>,
    instantiations: Vec<TopLevel>,
}

/// Fixed (non-rest) param count of a declared variadic. The parser records
/// the rest name inside `parameters` (definitions.rs:2509), so the true
/// fixed count is `parameters.len() - 1` when a rest is present — counting
/// the rest as a fixed param would shift the element positions and compute
/// the arity as `args.len() - 1` instead of `args.len()`.
fn fixed_count(ctx: &Ctx, name: &str) -> usize {
    let (i, is_txn) = ctx.variadics.get(name).copied().unwrap_or((0, false));
    let it = ctx.items.get(i);
    let (params, has_rest) = match (it, is_txn) {
        (Some(TopLevel::Definition(d)), false) => {
            (d.parameters.len(), d.variadic_param.is_some())
        }
        (Some(TopLevel::Transaction(t)), true) => {
            (t.parameters.len(), t.variadic_param.is_some())
        }
        _ => return 0,
    };
    if has_rest {
        params.saturating_sub(1)
    } else {
        params
    }
}

/// Ensure the (name, arity) instantiation exists: clone the ORIGINAL
/// declaration, rewrite the rest name at its use sites (`rest[i]` →
/// `__va_i`, `rest.Count#()` → N, bare `rest` → the element list literal),
/// and append it. Idempotent per (name, arity).
fn ensure_instantiation(
    ctx: &mut Ctx,
    name: &str,
    arity: usize,
) -> Result<(), String> {
    // `inst_name` is the internal de-dup key (`name::arity`); `emit_name` is
    // the symbol that reaches the LLVM IR (`name__arity` — `::` is invalid in
    // an LLVM identifier). The two must stay in sync: the item name, the
    // call-site retarget, and the de-dup key all derive from (name, arity).
    let inst_name = instantiation_name(name, arity);
    if ctx.elems.contains_key(&inst_name) {
        return Ok(());
    }
    let emit_name = emitted_instantiation_name(name, arity);
    let (i, is_txn) = ctx.variadics.get(name).copied().unwrap_or((0, false));
    let (rest_name, elem_type, min, max) = match unwrap_export(&ctx.items[i]) {
        TopLevel::Definition(d) => {
            let vp = d.variadic_param.as_ref().ok_or_else(|| {
                format!("'{name}' lost its rest parameter during specialization")
            })?;
            (vp.name.clone(), vp.element_type.clone(), vp.min, vp.max)
        }
        TopLevel::Transaction(t) => {
            let vp = t.variadic_param.as_ref().ok_or_else(|| {
                format!("'{name}' lost its rest parameter during specialization")
            })?;
            (vp.name.clone(), vp.element_type.clone(), vp.min, vp.max)
        }
        _ => return Err(format!("declaration '{name}' not found")),
    };
    if arity < min {
        return Err(format!(
            "call to '{name}' with {arity} rest argument(s) is below the \
             declared minimum of {min}"
        ));
    }
    let effective_max = max.unwrap_or(DEFAULT_MAX);
    if arity > effective_max {
        return Err(format!(
            "call to '{name}' with {arity} rest argument(s) exceeds the \
             declared maximum of {effective_max}; for runtime-determined \
             counts use a List<{elem_type}> parameter instead"
        ));
    }
    let elems: Vec<String> = (0..arity).map(|j| format!("__va_{j}")).collect();
    let ec = ElemCtx { rest: &rest_name, elems: &elems };

    // Append the element params to the instantiation's parameter list:
    // [fixed…, __va_0, __va_1, …]. The body's `rest[i]` → `__va_i` rewrite
    // references these params.
    let elem_params: Vec<(String, Type)> = elems
        .iter()
        .map(|n| (n.clone(), elem_type.clone()))
        .collect();

    // The parser recorded the rest name as the LAST entry in `parameters`
    // (definitions.rs:2509). In the instantiation the rest becomes N element
    // params, so the ghost rest entry is replaced by those elements:
    // [fixed…, __va_0, __va_1, …]. Keeping the ghost would leave a dead param
    // at position `fixed` and shift the element positions.
    let mut new_item: TopLevel = match unwrap_export(&ctx.items[i]) {
        TopLevel::Definition(d) => {
            let mut d2 = d.clone();
            d2.name = emit_name.clone();
            d2.variadic_param = None;
            if d2.parameters.last().map(|(n, _)| n == &rest_name) == Some(true) {
                d2.parameters.pop();
            }
            d2.parameters.extend(elem_params.clone());
            for stmt in d2.body.iter_mut() {
                rewrite_stmt_elems(stmt, &ec)?;
            }
            rewrite_expr_elems(&mut d2.contract.pre_condition, &ec)?;
            rewrite_expr_elems(&mut d2.contract.post_condition, &ec)?;
            TopLevel::Definition(d2)
        }
        TopLevel::Transaction(t) => {
            let mut t2 = t.clone();
            t2.name = emit_name.clone();
            t2.variadic_param = None;
            if t2.parameters.last().map(|(n, _)| n == &rest_name) == Some(true) {
                t2.parameters.pop();
            }
            t2.parameters.extend(elem_params.clone());
            for stmt in t2.body.iter_mut() {
                rewrite_stmt_elems(stmt, &ec)?;
            }
            rewrite_expr_elems(&mut t2.contract.pre_condition, &ec)?;
            rewrite_expr_elems(&mut t2.contract.post_condition, &ec)?;
            TopLevel::Transaction(t2)
        }
        _ => unreachable!("declaration found above"),
    };
    let _ = is_txn;
    ctx.elems.insert(inst_name, elems);
    ctx.instantiations.push(new_item);
    Ok(())
}

/// Rewrite context: rest name → element names (for the instantiation clone).
struct ElemCtx<'a> {
    rest: &'a str,
    elems: &'a [String],
}

/// Rewrite one `defn` in place: retarget its internal calls + clone its
/// instantiation body (the clone is emitted into ctx.instantiations on
/// first sighting).
fn rewrite_definition(
    d: &mut Definition,
    ctx: &mut Ctx,
) -> Result<(), String> {
    // An emitted instantiation (name__arity) — never reprocessed. The `__`
    // is the emitted delimiter (see emitted_instantiation_name); user names
    // never carry it, so this is a safe liveness marker.
    if is_instantiation_name(d.name.as_str(), &ctx.variadics) {
        return Ok(());
    }
    // Emit the per-arity instantiations lazily at call-site retargeting.
    for stmt in d.body.iter_mut() {
        rewrite_stmt_calls(stmt, ctx)?;
    }
    rewrite_expr_calls(&mut d.contract.pre_condition, ctx)?;
    rewrite_expr_calls(&mut d.contract.post_condition, ctx)?;
    if let Some(deriv) = &mut d.derivation {
        if let Some(e) = &mut deriv.postcondition {
            rewrite_expr_calls(e, ctx)?;
        }
        if let Some(e) = &mut deriv.precondition {
            rewrite_expr_calls(e, ctx)?;
        }
        if let Some(e) = &mut deriv.synthesized {
            rewrite_expr_calls(e, ctx)?;
        }
    }
    Ok(())
}

/// Rewrite one `txn`/`node` in place (same shape as the defn path).
fn rewrite_transaction(
    t: &mut Transaction,
    ctx: &mut Ctx,
) -> Result<(), String> {
    // An emitted instantiation (name__arity) — never reprocessed.
    if is_instantiation_name(t.name.as_str(), &ctx.variadics) {
        return Ok(());
    }
    for stmt in t.body.iter_mut() {
        rewrite_stmt_calls(stmt, ctx)?;
    }
    rewrite_expr_calls(&mut t.contract.pre_condition, ctx)?;
    rewrite_expr_calls(&mut t.contract.post_condition, ctx)?;
    if let Some(deriv) = &mut t.derivation {
        if let Some(e) = &mut deriv.postcondition {
            rewrite_expr_calls(e, ctx)?;
        }
        if let Some(e) = &mut deriv.precondition {
            rewrite_expr_calls(e, ctx)?;
        }
        if let Some(e) = &mut deriv.synthesized {
            rewrite_expr_calls(e, ctx)?;
        }
    }
    Ok(())
}

/// Retarget calls to declared variadics inside a statement.
fn rewrite_stmt_calls(stmt: &mut Statement, ctx: &mut Ctx) -> Result<(), String> {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                rewrite_expr_calls(e, ctx)?;
            }
        }
        Statement::Assign(l, r) => {
            rewrite_expr_calls(l, ctx)?;
            rewrite_expr_calls(r, ctx)?;
        }
        Statement::Term(e) => {
            if let Some(e) = e {
                rewrite_expr_calls(e, ctx)?;
            }
        }
        Statement::Guarded(c, body) => {
            rewrite_expr_calls(c, ctx)?;
            for s in body.iter_mut() {
                rewrite_stmt_calls(s, ctx)?;
            }
        }
        Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body) => {
            for s in body.iter_mut() {
                rewrite_stmt_calls(s, ctx)?;
            }
        }
        Statement::Expression(e) => rewrite_expr_calls(e, ctx)?,
        Statement::Foreach { list, body, .. } => {
            rewrite_expr_calls(list, ctx)?;
            for s in body.iter_mut() {
                rewrite_stmt_calls(s, ctx)?;
            }
        }
        Statement::Check(e) | Statement::Gate(e) => rewrite_expr_calls(e, ctx)?,
        Statement::EndProgram(e) | Statement::Rollback(e) => {
            if let Some(e) = e {
                rewrite_expr_calls(e, ctx)?;
            }
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                rewrite_expr_calls(t, ctx)?;
            }
            rewrite_expr_calls(value, ctx)?;
        }
        _ => {}
    }
    Ok(())
}

/// Retarget calls to declared variadics inside an expression: the
/// special form (a direct call to a declared variadic) plus child
/// recursion.
fn rewrite_expr_calls(expr: &mut Expr, ctx: &mut Ctx) -> Result<(), String> {
    rewrite_expr_calls_form(expr, ctx)?;
    rewrite_expr_calls_children(expr, ctx)
}

/// The special form: `name(args…)` where `name` is a declared variadic —
/// check the arity against the fixed count, ensure the (name, arity)
/// instantiation exists, and retarget the callee at the emitted symbol.
/// Positional args line up with [fixed…, __va_0…]: only the name changes.
fn rewrite_expr_calls_form(expr: &mut Expr, ctx: &mut Ctx) -> Result<(), String> {
    if let Expr::Call(name, args, _) = expr {
        if ctx.variadics.contains_key(name) {
            let fixed = fixed_count(ctx, name);
            if args.len() < fixed {
                return Err(format!(
                    "call to '{name}' with {} argument(s) is below the \
                     fixed parameter count of {fixed}",
                    args.len()
                ));
            }
            let arity = args.len() - fixed;
            ensure_instantiation(ctx, name, arity)?;
            // Retarget to the EMITTED symbol (name__arity) — it must match the
            // instantiation's item name (also emitted_instantiation_name).
            *name = emitted_instantiation_name(name, arity);
        }
    }
    Ok(())
}

/// Child recursion for the call retargeting walk.
fn rewrite_expr_calls_children(expr: &mut Expr, ctx: &mut Ctx) -> Result<(), String> {
    match expr {
        Expr::Call(_, args, _) => {
            for a in args.iter_mut() {
                rewrite_expr_calls(a, ctx)?;
            }
        }
        Expr::BinaryOp(_, l, r) => {
            rewrite_expr_calls(l, ctx)?;
            rewrite_expr_calls(r, ctx)?;
        }
        Expr::UnaryOp(_, e) => rewrite_expr_calls(e, ctx)?,
        Expr::Field(e, _) => rewrite_expr_calls(e, ctx)?,
        Expr::MethodCall(recv, _, args, _, _) => {
            rewrite_expr_calls(recv, ctx)?;
            for a in args.iter_mut() {
                rewrite_expr_calls(a, ctx)?;
            }
        }
        Expr::Reflect(e, _, _) => rewrite_expr_calls(e, ctx)?,
        Expr::Index(l, r) => {
            rewrite_expr_calls(l, ctx)?;
            rewrite_expr_calls(r, ctx)?;
        }
        Expr::Slice { array, start, end, stride } => {
            rewrite_expr_calls(array, ctx)?;
            if let Some(s) = start {
                rewrite_expr_calls(s, ctx)?;
            }
            if let Some(e) = end {
                rewrite_expr_calls(e, ctx)?;
            }
            if let Some(s) = stride {
                rewrite_expr_calls(s, ctx)?;
            }
        }
        Expr::Range { start, end, .. } => {
            rewrite_expr_calls(start, ctx)?;
            rewrite_expr_calls(end, ctx)?;
        }
        Expr::Block(body) => {
            for s in body.iter_mut() {
                rewrite_stmt_calls(s, ctx)?;
            }
        }
        Expr::If(c, t, e) => {
            rewrite_expr_calls(c, ctx)?;
            rewrite_expr_calls(t, ctx)?;
            if let Some(e) = e {
                rewrite_expr_calls(e, ctx)?;
            }
        }
        Expr::Match(s, arms) => {
            rewrite_expr_calls(s, ctx)?;
            for arm in arms.iter_mut() {
                if let Some(g) = &mut arm.guard {
                    rewrite_expr_calls(g, ctx)?;
                }
                rewrite_expr_calls(&mut arm.body, ctx)?;
            }
        }
        Expr::Tuple(items) | Expr::List(items) => {
            for e in items.iter_mut() {
                rewrite_expr_calls(e, ctx)?;
            }
        }
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, v) in fields.iter_mut() {
                rewrite_expr_calls(v, ctx)?;
            }
            for (_, v) in specs.iter_mut() {
                rewrite_expr_calls(v, ctx)?;
            }
        }
        Expr::Lambda(_, body) => rewrite_expr_calls(body, ctx)?,
        Expr::Cast(e, _) => rewrite_expr_calls(e, ctx)?,
        Expr::IsType(e, _) => rewrite_expr_calls(e, ctx)?,
        Expr::Spawn { args, .. } => {
            for a in args.iter_mut() {
                rewrite_expr_calls(a, ctx)?;
            }
        }
        Expr::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                rewrite_expr_calls(r, ctx)?;
            }
            for a in args.iter_mut() {
                rewrite_expr_calls(a, ctx)?;
            }
        }
        Expr::Capture { expr, .. } => rewrite_expr_calls(expr, ctx)?,
        Expr::Await(e) => rewrite_expr_calls(e, ctx)?,
        _ => {}
    }
    Ok(())
}

// ── Rest-name rewrite (instantiation clone) ─────────────────────────────
// Inside a cloned instantiation, the rest name is bound to the element
// parameters (`__va_i`). Rewrite the use sites:
//   - `rest[i]` (constant, in-bounds for THIS arity) → `__va_i`
//   - `rest[i]` (constant, out of bounds for THIS arity) → directed error
//     (the body demands more elements than this call supplies — emitting a
//     placeholder would compute silently wrong results)
//   - `rest.Count#()` → N (the arity literal)
//   - `foreach x in rest` → N unrolled body copies (one per element)
//   - bare `rest` / non-constant `rest[e]` → directed error (v1: the rest
//     has no list value to substitute; bind indices to constants or iterate)

/// Rewrite the rest name at use sites in a statement (instantiation body).
fn rewrite_stmt_elems(stmt: &mut Statement, ec: &ElemCtx) -> Result<(), String> {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                rewrite_expr_elems(e, ec)?;
            }
        }
        Statement::Assign(l, r) => {
            rewrite_expr_elems(l, ec)?;
            rewrite_expr_elems(r, ec)?;
        }
        Statement::Term(e) => {
            if let Some(e) = e {
                rewrite_expr_elems(e, ec)?;
            }
        }
        Statement::Guarded(c, body) => {
            rewrite_expr_elems(c, ec)?;
            for s in body.iter_mut() {
                rewrite_stmt_elems(s, ec)?;
            }
        }
        Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body) => {
            for s in body.iter_mut() {
                rewrite_stmt_elems(s, ec)?;
            }
        }
        Statement::Expression(e) => rewrite_expr_elems(e, ec)?,
        Statement::Foreach { item, body, .. } => {
            // Unroll the foreach over the rest list: each element gets a copy
            // of the body. This is the zero-cost specialization — the
            // compiler emits N sequential copies of the body, one per element.
            // The loop VARIABLE (`item`, e.g. the `x` in `foreach x in xs`)
            // is renamed to the element param in each copy; the iterable (the
            // rest name) is dropped with the statement itself.
            let from = Expr::Identifier(item.clone());
            let elems: Vec<String> = ec.elems.iter().cloned().collect();
            let mut new_body: Vec<Statement> = Vec::new();
            for elem_name in &elems {
                let elem_name = elem_name.clone();
                let mut body_copy = body.clone();
                // Replace the item variable with the element name.
                for stmt in body_copy.iter_mut() {
                    rename_item(stmt, &from, &elem_name);
                }
                new_body.extend(body_copy);
            }
            *stmt = Statement::Block(new_body);
        }
        Statement::Check(e) | Statement::Gate(e) => rewrite_expr_elems(e, ec)?,
        Statement::EndProgram(e) | Statement::Rollback(e) => {
            if let Some(e) = e {
                rewrite_expr_elems(e, ec)?;
            }
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                rewrite_expr_elems(t, ec)?;
            }
            rewrite_expr_elems(value, ec)?;
        }
        _ => {}
    }
    Ok(())
}

/// 2026-10-14 (syscall retirement): fold a match whose scrutinee is (now) a
/// CONSTANT to its live arm BEFORE the elems walk — the walk's out-of-bounds
/// rule would otherwise reject dead arms (a `syscall(nr, ...args)` body
/// dispatches on `args.Count#()` and indexes `args[0..5]` in arms 4..6,
/// which are out of bounds for an arity-3 instantiation but never executed
/// there). General constant-match folding: the scrutinee was folded to a
/// literal by the Count# rule, the arm pattern is a literal equal to it, and
/// the arm has no guard. Unguarded, unmatched, or non-literal: leave as-is
/// (the ordinary constant-fold pass still sees it). To undo: delete this fn
/// + the Match arm in rewrite_expr_elems_form.
fn rewrite_match_constant_fold(expr: &mut Expr, ec: &ElemCtx) -> Result<(), String> {
    // Fold the scrutinee FIRST (the Count# rule turns `args.Count#()` into
    // the arity literal), then look at it.
    if let Expr::Match(scrut, _) = expr {
        rewrite_expr_elems(scrut, ec)?;
    }
    let scrut = match expr {
        Expr::Match(scrut, _) => scrut.as_ref().clone(),
        _ => unreachable!("matched Match"),
    };
    if let Expr::Decimal(n) = scrut {
        let arms = match expr {
            Expr::Match(_, arms) => arms,
            _ => unreachable!("matched Match"),
        };
        let live = arms.iter().position(|arm| match &arm.pattern {
            crate::ast::Pattern::Literal(Expr::Decimal(j)) => *j == n,
            crate::ast::Pattern::Wildcard => true,
            _ => false,
        });
        if let Some(idx) = live {
            if arms[idx].guard.is_none() {
                let body = arms[idx].body.as_ref().clone();
                let _ = std::mem::replace(expr, body);
            }
        }
    }
    rewrite_expr_elems_children(expr, ec)
}

/// Rewrite the rest name at use sites in an expression (instantiation
/// body/contract): the rest-specific forms plus child recursion.
fn rewrite_expr_elems(expr: &mut Expr, ec: &ElemCtx) -> Result<(), String> {
    rewrite_expr_elems_form(expr, ec)?;
    rewrite_expr_elems_children(expr, ec)
}

/// The rest-specific forms. A replaced form (index → element param,
/// `Count#()` → the arity literal) contains no rest references, so the
/// subsequent child walk over it is a harmless no-op.
fn rewrite_expr_elems_form(expr: &mut Expr, ec: &ElemCtx) -> Result<(), String> {
    match expr {
        Expr::Identifier(n) if n == ec.rest => {
            // Bare rest use in expression position has no value to substitute
            // (the rest binding IS the element parameters). Direct the author
            // at the supported forms instead of emitting a dangling name.
            Err(format!(
                "rest parameter '{rest}' cannot be used as a value — \
                 index it (`{rest}[0]`), take its count (`{rest}.Count#()`), \
                 or iterate it (`foreach x in {rest}`)",
                rest = ec.rest
            ))
        }
        Expr::Index(base, idx) => {
            if let Expr::Identifier(n) = base.as_ref() {
                if n == ec.rest {
                    // A CONSTANT in-bounds index lowers to the element param
                    // directly. A constant OUT-OF-BOUNDS index (e.g. `xs[2]`
                    // at arity 2) means this call supplies fewer elements than
                    // the body demands — a directed error, never a placeholder
                    // (a placeholder would compute silently wrong results).
                    if let Expr::Decimal(j) = idx.as_ref() {
                        if *j >= 0 && (*j as usize) < ec.elems.len() {
                            let name = ec.elems[*j as usize].clone();
                            let _ = std::mem::replace(expr, Expr::Identifier(name));
                            return Ok(());
                        }
                        return Err(format!(
                            "the body indexes '{rest}[{j}]' but this call \
                             supplies only {n} rest argument(s) — every \
                             constant rest index must be below the call's \
                             rest count",
                            rest = ec.rest,
                            j = j,
                            n = ec.elems.len()
                        ));
                    }
                    // Non-constant index: there is no list value to index
                    // (the rest binding is erased). Direct at the supported
                    // forms instead of emitting a dangling name.
                    return Err(format!(
                        "rest parameter '{rest}' indexed with a \
                         non-constant expression — rest indexing needs a \
                         constant index (specialization is per-arity); \
                         iterate `foreach x in {rest}` for \
                         runtime-determined access",
                        rest = ec.rest
                    ));
                }
            }
            Ok(())
        }
        Expr::MethodCall(recv, method, _, _, _) if method == "Count#" => {
            if let Expr::Identifier(n) = recv.as_ref() {
                if n == ec.rest {
                    let n = ec.elems.len() as i64;
                    let _ = std::mem::replace(expr, Expr::Decimal(n));
                }
            }
            Ok(())
        }
        Expr::Match(_, _) => rewrite_match_constant_fold(expr, ec),
        _ => Ok(()),
    }
}

/// Child recursion for the rest-name rewrite walk.
fn rewrite_expr_elems_children(expr: &mut Expr, ec: &ElemCtx) -> Result<(), String> {
    match expr {
        Expr::Call(_, args, _) => {
            for a in args.iter_mut() {
                rewrite_expr_elems(a, ec)?;
            }
        }
        Expr::BinaryOp(_, l, r) => {
            rewrite_expr_elems(l, ec)?;
            rewrite_expr_elems(r, ec)?;
        }
        Expr::UnaryOp(_, e) => rewrite_expr_elems(e, ec)?,
        Expr::Field(e, _) => rewrite_expr_elems(e, ec)?,
        Expr::MethodCall(recv, _, args, _, _) => {
            rewrite_expr_elems(recv, ec)?;
            for a in args.iter_mut() {
                rewrite_expr_elems(a, ec)?;
            }
        }
        Expr::Reflect(e, _, _) => rewrite_expr_elems(e, ec)?,
        Expr::Index(l, r) => {
            rewrite_expr_elems(l, ec)?;
            rewrite_expr_elems(r, ec)?;
        }
        Expr::Slice { array, start, end, stride } => {
            rewrite_expr_elems(array, ec)?;
            if let Some(s) = start {
                rewrite_expr_elems(s, ec)?;
            }
            if let Some(e) = end {
                rewrite_expr_elems(e, ec)?;
            }
            if let Some(s) = stride {
                rewrite_expr_elems(s, ec)?;
            }
        }
        Expr::Range { start, end, .. } => {
            rewrite_expr_elems(start, ec)?;
            rewrite_expr_elems(end, ec)?;
        }
        Expr::Block(body) => {
            for s in body.iter_mut() {
                rewrite_stmt_elems(s, ec)?;
            }
        }
        Expr::If(c, t, e) => {
            rewrite_expr_elems(c, ec)?;
            rewrite_expr_elems(t, ec)?;
            if let Some(e) = e {
                rewrite_expr_elems(e, ec)?;
            }
        }
        Expr::Match(s, arms) => {
            rewrite_expr_elems(s, ec)?;
            for arm in arms.iter_mut() {
                if let Some(g) = &mut arm.guard {
                    rewrite_expr_elems(g, ec)?;
                }
                rewrite_expr_elems(&mut arm.body, ec)?;
            }
        }
        Expr::Tuple(items) | Expr::List(items) => {
            for e in items.iter_mut() {
                rewrite_expr_elems(e, ec)?;
            }
        }
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, v) in fields.iter_mut() {
                rewrite_expr_elems(v, ec)?;
            }
            for (_, v) in specs.iter_mut() {
                rewrite_expr_elems(v, ec)?;
            }
        }
        Expr::Lambda(_, body) => rewrite_expr_elems(body, ec)?,
        Expr::Cast(e, _) => rewrite_expr_elems(e, ec)?,
        Expr::IsType(e, _) => rewrite_expr_elems(e, ec)?,
        Expr::Spawn { args, .. } => {
            for a in args.iter_mut() {
                rewrite_expr_elems(a, ec)?;
            }
        }
        Expr::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                rewrite_expr_elems(r, ec)?;
            }
            for a in args.iter_mut() {
                rewrite_expr_elems(a, ec)?;
            }
        }
        Expr::Capture { expr, .. } => rewrite_expr_elems(expr, ec)?,
        Expr::Await(e) => rewrite_expr_elems(e, ec)?,
        _ => {}
    }
    Ok(())
}

/// Rename the foreach item variable to the element name in a statement
/// (unrolling the rest-foreach into sequential body copies).
fn rename_item(stmt: &mut Statement, from: &Expr, to: &str) {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                rename_expr(e, from, to);
            }
        }
        Statement::Assign(l, r) => {
            rename_expr(l, from, to);
            rename_expr(r, from, to);
        }
        Statement::Term(e) => {
            if let Some(e) = e {
                rename_expr(e, from, to);
            }
        }
        Statement::Guarded(c, body) => {
            rename_expr(c, from, to);
            for s in body.iter_mut() {
                rename_item(s, from, to);
            }
        }
        Statement::Block(body)
        | Statement::SyncBlock(body)
        | Statement::Defer(body)
        | Statement::Mutex(body) => {
            for s in body.iter_mut() {
                rename_item(s, from, to);
            }
        }
        Statement::Expression(e) => rename_expr(e, from, to),
        Statement::Foreach { list, body, .. } => {
            rename_expr(list, from, to);
            for s in body.iter_mut() {
                rename_item(s, from, to);
            }
        }
        Statement::Check(e) | Statement::Gate(e) => rename_expr(e, from, to),
        Statement::EndProgram(e) | Statement::Rollback(e) => {
            if let Some(e) = e {
                rename_expr(e, from, to);
            }
        }
        Statement::ArrowAssign { target, value, .. } => {
            if let Some(t) = target {
                rename_expr(t, from, to);
            }
            rename_expr(value, from, to);
        }
        _ => {}
    }
}

fn rename_expr(expr: &mut Expr, from: &Expr, to: &str) {
    // The special form: the loop variable itself. A replaced identifier
    // cannot match again, so the child walk below is a harmless no-op on it.
    if let Expr::Identifier(n) = expr {
        if as_identifier(from).is_some_and(|f| f == *n) {
            *expr = Expr::Identifier(to.to_string());
            return;
        }
    }
    rename_expr_children(expr, from, to)
}

/// Child recursion for the loop-variable rename walk.
fn rename_expr_children(expr: &mut Expr, from: &Expr, to: &str) {
    match expr {
        Expr::Call(_, args, _) => {
            for a in args.iter_mut() {
                rename_expr(a, from, to);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            rename_expr(l, from, to);
            rename_expr(r, from, to);
        }
        Expr::UnaryOp(_, e) => rename_expr(e, from, to),
        Expr::Field(e, _) => rename_expr(e, from, to),
        Expr::MethodCall(recv, _, args, _, _) => {
            rename_expr(recv, from, to);
            for a in args.iter_mut() {
                rename_expr(a, from, to);
            }
        }
        Expr::Reflect(e, _, _) => rename_expr(e, from, to),
        Expr::Index(l, r) => {
            rename_expr(l, from, to);
            rename_expr(r, from, to);
        }
        Expr::Slice { array, start, end, stride } => {
            rename_expr(array, from, to);
            if let Some(s) = start {
                rename_expr(s, from, to);
            }
            if let Some(e) = end {
                rename_expr(e, from, to);
            }
            if let Some(s) = stride {
                rename_expr(s, from, to);
            }
        }
        Expr::Range { start, end, .. } => {
            rename_expr(start, from, to);
            rename_expr(end, from, to);
        }
        Expr::Block(body) => {
            for s in body.iter_mut() {
                rename_item(s, from, to);
            }
        }
        Expr::If(c, t, e) => {
            rename_expr(c, from, to);
            rename_expr(t, from, to);
            if let Some(e) = e {
                rename_expr(e, from, to);
            }
        }
        Expr::Match(s, arms) => {
            rename_expr(s, from, to);
            for arm in arms.iter_mut() {
                if let Some(g) = &mut arm.guard {
                    rename_expr(g, from, to);
                }
                rename_expr(&mut arm.body, from, to);
            }
        }
        Expr::Tuple(items) | Expr::List(items) => {
            for e in items.iter_mut() {
                rename_expr(e, from, to);
            }
        }
        Expr::StructLiteral { fields, specs, .. } => {
            for (_, v) in fields.iter_mut() {
                rename_expr(v, from, to);
            }
            for (_, v) in specs.iter_mut() {
                rename_expr(v, from, to);
            }
        }
        Expr::Lambda(_, body) => rename_expr(body, from, to),
        Expr::Cast(e, _) => rename_expr(e, from, to),
        Expr::IsType(e, _) => rename_expr(e, from, to),
        Expr::Spawn { args, .. } => {
            for a in args.iter_mut() {
                rename_expr(a, from, to);
            }
        }
        Expr::PluginIntercept { args, receiver, .. } => {
            if let Some(r) = receiver {
                rename_expr(r, from, to);
            }
            for a in args.iter_mut() {
                rename_expr(a, from, to);
            }
        }
        Expr::Capture { expr, .. } => rename_expr(expr, from, to),
        Expr::Await(e) => rename_expr(e, from, to),
        _ => {}
    }
}

fn as_identifier(e: &Expr) -> Option<&str> {
    match e {
        Expr::Identifier(n) => Some(n.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOpKind;
    use crate::ast::top::Contract;
    use std::collections::HashMap;

    fn contract() -> Contract {
        Contract {
            pre_condition: Expr::Bool(true),
            post_condition: Expr::Bool(true),
            watchdog: None,
            span: None,
            explicit: true,
            post_authority: false,
        }
    }

    /// A variadic `defn name(...rest: Int[<=max])` with the parser's ghost
    /// rest entry as the LAST parameter (definitions.rs).
    fn var_defn(name: &str, rest: &str, min: usize, max: usize, body: Vec<Statement>) -> TopLevel {
        TopLevel::Definition(Definition {
            name: name.to_string(),
            type_params: vec![],
            parameters: vec![(rest.to_string(), Type::int())],
            output_type: None,
            outputs: vec![],
            contract: contract(),
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            variadic_param: Some(VariadicParam {
                name: rest.to_string(),
                element_type: Type::int(),
                min,
                max: Some(max),
            }),
            doc: None,
        })
    }

    /// A plain (non-variadic) transaction with the given body statements.
    fn plain_txn(name: &str, body: Vec<Statement>) -> TopLevel {
        TopLevel::Transaction(Transaction {
            name: name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: contract(),
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            variadic_param: None,
            doc: None,
        })
    }

    fn call(name: &str, args: Vec<i64>) -> Expr {
        Expr::Call(name.to_string(), args.into_iter().map(Expr::Decimal).collect(), None)
    }

    /// `total = total + rest[i]` accumulate chain over the given indices.
    fn sum_body(rest: &str, idxs: &[i64]) -> Vec<Statement> {
        let mut body = vec![Statement::Let {
            name: "total".to_string(),
            names: vec!["total".to_string()],
            ty: Some(Type::int()),
            expr: Some(Expr::Decimal(0)),
            modifiers: vec![],
        }];
        for j in idxs {
            body.push(Statement::Assign(
                Expr::Identifier("total".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("total".to_string())),
                    Box::new(Expr::Index(
                        Box::new(Expr::Identifier(rest.to_string())),
                        Box::new(Expr::Decimal(*j)),
                    )),
                ),
            ));
        }
        body.push(Statement::Term(Some(Expr::Identifier("total".to_string()))));
        body
    }

    fn inst_names(items: &[TopLevel]) -> Vec<String> {
        items
            .iter()
            .filter_map(|it| match it {
                TopLevel::Definition(d) => Some(d.name.clone()),
                TopLevel::Transaction(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn instantiation_naming_is_split_internal_vs_emitted() {
        assert_eq!(instantiation_name("sum", 3), "sum::3");
        assert_eq!(emitted_instantiation_name("sum", 3), "sum__3");
    }

    #[test]
    fn guard_only_matches_declared_variadics_with_digit_suffix() {
        let mut v = HashMap::new();
        v.insert("sum".to_string(), (0usize, false));
        assert!(is_instantiation_name("sum__3", &v));
        assert!(is_instantiation_name("sum__12", &v));
        assert!(!is_instantiation_name("sum", &v));
        assert!(!is_instantiation_name("other__3", &v));
        assert!(!is_instantiation_name("sum__", &v));
        assert!(!is_instantiation_name("sum__3x", &v));
        assert!(!is_instantiation_name("sum__x", &v));
    }

    #[test]
    fn single_arity_rewrites_body_and_call_site() {
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, sum_body("xs", &[0, 1, 2])),
            plain_txn(
                "main",
                vec![Statement::Term(Some(call("sum", vec![1, 2, 3])))],
            ),
        ];
        specialize_variadic(&mut items).unwrap();
        // Caller retargeted at the emitted symbol.
        match &items.iter().find(|it| matches!(it, TopLevel::Transaction(t) if t.name == "main")).unwrap() {
            TopLevel::Transaction(t) => match &t.body[0] {
                Statement::Term(Some(Expr::Call(n, _, _))) => assert_eq!(n, "sum__3"),
                other => panic!("expected retargeted call, got {other:?}"),
            },
            _ => unreachable!(),
        }
        // Instantiation: fixed params + __va_0..2, body fully rewritten.
        let inst = items
            .iter()
            .find_map(|it| match it {
                TopLevel::Definition(d) if d.name == "sum__3" => Some(d),
                _ => None,
            })
            .expect("instantiation sum__3 emitted");
        let params: Vec<&str> = inst.parameters.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(params, vec!["__va_0", "__va_1", "__va_2"]);
        assert!(inst.variadic_param.is_none());
        // Every rest use became the element param.
        let debug = format!("{:?}", inst.body);
        assert!(!debug.contains("\"xs\""), "rest name leaked: {debug}");
        assert!(debug.contains("__va_2"));
        // Original: wiped signature (no rest, no body).
        let orig = items
            .iter()
            .find_map(|it| match it {
                TopLevel::Definition(d) if d.name == "sum" => Some(d),
                _ => None,
            })
            .unwrap();
        assert!(orig.variadic_param.is_none());
        assert!(orig.body.is_empty());
    }

    #[test]
    fn same_arity_shares_one_instantiation() {
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, sum_body("xs", &[0])),
            plain_txn(
                "main",
                vec![
                    Statement::Term(Some(call("sum", vec![1, 2]))),
                    Statement::Term(Some(call("sum", vec![3, 4]))),
                ],
            ),
        ];
        specialize_variadic(&mut items).unwrap();
        assert_eq!(inst_names(&items).iter().filter(|n| *n == "sum__2").count(), 1);
    }

    #[test]
    fn two_arities_produce_two_instantiations() {
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, sum_body("xs", &[0, 1])),
            plain_txn(
                "main",
                vec![
                    Statement::Term(Some(call("sum", vec![1, 2]))),
                    Statement::Term(Some(call("sum", vec![1, 2, 3]))),
                ],
            ),
        ];
        specialize_variadic(&mut items).unwrap();
        let names = inst_names(&items);
        assert!(names.contains(&"sum__2".to_string()));
        assert!(names.contains(&"sum__3".to_string()));
    }

    #[test]
    fn below_min_call_is_a_directed_error() {
        let mut items = vec![
            var_defn("sum", "xs", 2, 8, sum_body("xs", &[0, 1])),
            plain_txn("main", vec![Statement::Term(Some(call("sum", vec![1])))]),
        ];
        let err = specialize_variadic(&mut items).unwrap_err();
        assert!(err.contains("below the declared minimum of 2"), "{err}");
    }

    #[test]
    fn over_max_call_is_a_directed_error() {
        let mut items = vec![
            var_defn("sum", "xs", 0, 2, sum_body("xs", &[0])),
            plain_txn(
                "main",
                vec![Statement::Term(Some(call("sum", vec![1, 2, 3])))],
            ),
        ];
        let err = specialize_variadic(&mut items).unwrap_err();
        assert!(err.contains("exceeds the declared maximum of 2"), "{err}");
        assert!(err.contains("List<Int>"), "{err}");
    }

    #[test]
    fn out_of_bounds_body_index_is_a_directed_error_not_a_placeholder() {
        // Body demands xs[2]; the call supplies 2 — the instantiation must
        // ERROR, never emit a placeholder that computes a wrong result.
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, sum_body("xs", &[0, 1, 2])),
            plain_txn("main", vec![Statement::Term(Some(call("sum", vec![1, 2])))]),
        ];
        let err = specialize_variadic(&mut items).unwrap_err();
        assert!(err.contains("xs[2]"), "{err}");
        assert!(err.contains("2 rest argument(s)"), "{err}");
    }

    #[test]
    fn non_variadic_node_body_is_preserved() {
        // Bug-1 regression: the body wipe must never touch non-variadic items.
        let body = vec![Statement::Term(Some(Expr::Decimal(7)))];
        let mut items = vec![plain_txn("main", body.clone())];
        specialize_variadic(&mut items).unwrap();
        match &items[0] {
            TopLevel::Transaction(t) => assert_eq!(t.body, body),
            _ => unreachable!(),
        }
    }

    #[test]
    fn count_hash_rewrites_to_the_arity_literal() {
        let mut body = sum_body("xs", &[0]);
        body.push(Statement::Term(Some(Expr::MethodCall(
            Box::new(Expr::Identifier("xs".to_string())),
            "Count#".to_string(),
            vec![],
            None,
            vec![],
        ))));
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, body),
            plain_txn("main", vec![Statement::Term(Some(call("sum", vec![1, 2])))]),
        ];
        specialize_variadic(&mut items).unwrap();
        let inst = items
            .iter()
            .find_map(|it| match it {
                TopLevel::Definition(d) if d.name == "sum__2" => Some(d),
                _ => None,
            })
            .unwrap();
        let debug = format!("{:?}", inst.body);
        assert!(debug.contains("Decimal(2)"), "Count#() not folded: {debug}");
    }

    #[test]
    fn foreach_over_rest_unrolls_per_element() {
        // `t = t + x` under `foreach x in xs` becomes one add per element;
        // the loop variable is renamed, the iterable is dropped.
        let body = vec![
            Statement::Let {
                name: "t".to_string(),
                names: vec!["t".to_string()],
                ty: Some(Type::int()),
                expr: Some(Expr::Decimal(0)),
                modifiers: vec![],
            },
            Statement::Foreach {
                item: "x".to_string(),
                list: Box::new(Expr::Identifier("xs".to_string())),
                body: vec![Statement::Assign(
                    Expr::Identifier("t".to_string()),
                    Expr::BinaryOp(
                        BinaryOpKind::Add,
                        Box::new(Expr::Identifier("t".to_string())),
                        Box::new(Expr::Identifier("x".to_string())),
                    ),
                )],
            },
        ];
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, body),
            plain_txn("main", vec![Statement::Term(Some(call("sum", vec![1, 2, 3])))]),
        ];
        specialize_variadic(&mut items).unwrap();
        let inst = items
            .iter()
            .find_map(|it| match it {
                TopLevel::Definition(d) if d.name == "sum__3" => Some(d),
                _ => None,
            })
            .expect("sum__3 emitted");
        let debug = format!("{:?}", inst.body);
        assert!(!debug.contains("\"x\""), "loop var leaked: {debug}");
        assert_eq!(debug.matches("__va_0").count() + debug.matches("__va_1").count() + debug.matches("__va_2").count() >= 3, true, "unroll missing: {debug}");
    }

    #[test]
    fn exported_variadic_is_specialized() {
        // 2026-10-14 (syscall retirement): `export defn` wraps the decl in
        // TopLevel::Export — the pass must see through it, or the declaring
        // module's own check typechecks the raw rest body.
        let inner = match var_defn("sum", "xs", 0, 8, sum_body("xs", &[0])) {
            TopLevel::Definition(d) => d,
            _ => unreachable!(),
        };
        let mut items = vec![
            TopLevel::Export(crate::ast::top::Export {
                inner: Box::new(TopLevel::Definition(inner)),
                export_name: None,
            }),
            plain_txn("main", vec![Statement::Term(Some(call("sum", vec![7])))]),
        ];
        specialize_variadic(&mut items).unwrap();
        // The export survives, wrapping the WIPED original; the instantiation
        // is a plain (internal, unexported) definition.
        let names = inst_names(&items);
        assert!(names.contains(&"sum__1".to_string()), "{names:?}");
        match items.iter().find(|it| matches!(it, TopLevel::Export(_))) {
            Some(TopLevel::Export(e)) => match e.inner.as_ref() {
                TopLevel::Definition(d) => {
                    assert_eq!(d.name, "sum");
                    assert!(d.body.is_empty());
                    assert!(d.variadic_param.is_none());
                }
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn constant_scrutinee_match_folds_to_the_live_arm() {
        // 2026-10-14 (syscall retirement): a body dispatching on
        // `xs.Count#()` must fold to the live arm BEFORE the elems walk —
        // dead arms index out of bounds for smaller arities. Mirrors the
        // syscall surface: match xs.Count#() { 0 => xs[0], 1 => xs[0]+xs[1] }.
        let body = vec![Statement::Term(Some(Expr::Match(
            Box::new(Expr::MethodCall(
                Box::new(Expr::Identifier("xs".to_string())),
                "Count#".to_string(),
                vec![],
                None,
                vec![],
            )),
            vec![
                crate::ast::MatchArm {
                    pattern: crate::ast::Pattern::Literal(Expr::Decimal(0)),
                    guard: None,
                    body: Box::new(Expr::Index(
                        Box::new(Expr::Identifier("xs".to_string())),
                        Box::new(Expr::Decimal(0)),
                    )),
                },
                crate::ast::MatchArm {
                    pattern: crate::ast::Pattern::Literal(Expr::Decimal(1)),
                    guard: None,
                    body: Box::new(Expr::BinaryOp(
                        BinaryOpKind::Add,
                        Box::new(Expr::Index(
                            Box::new(Expr::Identifier("xs".to_string())),
                            Box::new(Expr::Decimal(0)),
                        )),
                        Box::new(Expr::Index(
                            Box::new(Expr::Identifier("xs".to_string())),
                            Box::new(Expr::Decimal(0)),
                        )),
                    )),
                },
            ],
        )))];
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, body),
            plain_txn("main", vec![Statement::Term(Some(call("sum", vec![7])))]),
        ];
        specialize_variadic(&mut items).unwrap();
        let inst = items
            .iter()
            .find_map(|it| match it {
                TopLevel::Definition(d) if d.name == "sum__1" => Some(d),
                _ => None,
            })
            .expect("sum__1 emitted");
        // The fold replaced the whole match with arm 1's body; both `xs`
        // references became `__va_0`. No Match survives, no bare `xs`.
        let debug = format!("{:?}", inst.body);
        assert!(!debug.contains("Match("), "match not folded: {debug}");
        assert!(!debug.contains("\"xs\""), "rest name leaked: {debug}");
    }

    #[test]
    fn bare_rest_use_is_a_directed_error() {
        let body = vec![Statement::Term(Some(Expr::Identifier("xs".to_string())))];
        let mut items = vec![
            var_defn("sum", "xs", 0, 8, body),
            plain_txn("main", vec![Statement::Term(Some(call("sum", vec![1])))]),
        ];
        let err = specialize_variadic(&mut items).unwrap_err();
        assert!(err.contains("cannot be used as a value"), "{err}");
    }
}
