// ── Anti-Unification Abstraction — Ephemeral Helper Library ─────────────
// 2026-07-29: Abstraction discovery for depth-bounded enumerative synthesis
// using anti-unification (Plotkin 1970, Reynolds 1970) adapted from Feser
// et al. λ² (PLDI 2015) §4. Instead of frequency-based extraction, anti-unify
// pairs of expressions in the LevelCache, extract the common sub-structure as
// a helper, and REPLACE the originals with helper calls. This shrinks the
// search space rather than growing it. Helpers are ephemeral: discovered after
// depth N, registered into the LevelCache, consumed at depth N+1, and
// garbage-collected if unused.

use crate::ast::{BinaryOpKind, Expr, UnaryOpKind};
use crate::derive::engine::{CostModel, LevelCache, is_commutative_op};
use std::collections::{HashMap, HashSet};

// ── Configuration ────────────────────────────────────────────────────

/// 2026-07-29: Configuration for anti-unification discovery.
/// Default conservative: min savings = 1 (must strictly reduce total size).
#[derive(Debug, Clone)]
pub struct DiscoverConfig {
    /// Minimum savings to extract a helper (in cost units)
    pub min_savings: i64,
    /// Maximum helpers per return type
    pub max_helpers_per_type: usize,
}

impl Default for DiscoverConfig {
    fn default() -> Self {
        DiscoverConfig {
            min_savings: 1,
            max_helpers_per_type: 20,
        }
    }
}

/// 2026-07-29: Global default used by synthesize_enumerative.
pub static DISCOVER_CONFIG: DiscoverConfig = DiscoverConfig {
    min_savings: 1,
    max_helpers_per_type: 20,
};

// ── Helper Type ──────────────────────────────────────────────────────

/// 2026-07-29: A helper function discovered via anti-unification.
/// Represents the common sub-expression pattern extracted from a pair
/// of LevelCache expressions. Params include the original params plus
/// any placeholder variables introduced by anti-unification.
#[derive(Debug, Clone)]
pub struct HelperFunction {
    pub name: String,
    pub params: Vec<String>,
    pub param_types: Vec<String>,
    pub body: Expr,
    pub ret_type: String,
    pub body_cost: u64,
    pub call_cost: u64,
    pub use_count: usize,
}

// ── Expression Size ──────────────────────────────────────────────────

/// 2026-07-29: Count AST nodes in an expression (each Expr variant = 1).
/// Used by the savings function to compare size before/after abstraction.
fn expr_size(expr: &Expr) -> u64 {
    match expr {
        Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Identifier(_) => 1,
        Expr::UnaryOp(_, inner) => 1 + expr_size(inner),
        Expr::BinaryOp(_, lhs, rhs) => 1 + expr_size(lhs) + expr_size(rhs),
        Expr::If(cond, then, else_) => {
            1 + expr_size(cond) + expr_size(then) + else_.as_ref().map_or(0, |e| expr_size(e))
        }
        Expr::Call(_, args, _) => 1 + args.iter().map(|a| expr_size(a)).sum::<u64>(),
        Expr::Field(inner, _) => 1 + expr_size(inner),
        Expr::Match(scrut, arms) => {
            1 + expr_size(scrut) + arms.iter().map(|a| expr_size(&a.body)).sum::<u64>()
        }
        _ => 1,
    }
}

// ── Placeholder Variable Management ──────────────────────────────────

/// 2026-07-29: Counter for generating unique placeholder variable names.
/// Placeholders are named "_t0", "_t1", etc. and appear in the anti-unified
/// pattern where two expressions differ. They become parameters of the helper.
struct PlaceholderState {
    counter: usize,
}

impl PlaceholderState {
    fn new() -> Self {
        PlaceholderState { counter: 0 }
    }

    fn fresh(&mut self) -> String {
        let name = format!("_t{}", self.counter);
        self.counter += 1;
        name
    }
}

// ── Anti-Unification ─────────────────────────────────────────────────

/// 2026-07-29: Anti-unify two expressions, producing the least general
/// generalization (the anti-unifier) with placeholder variables where
/// they differ, plus two substitution maps reconstructing the originals.
///
/// Returns None when anti-unification fails (completely different structure).
///
/// Algorithm (Plotkin 1970, Reynolds 1970):
///   antiunify(c, c) = c                                      (identical leaves)
///   antiunify(f(e1..en), f(e1'..en')) = f(antiunify(ei, ei'))  (same functor)
///   antiunify(a, b) = _ti                                    (different → placeholder)
///
/// Placeholders are tracked by (address_of_a, address_of_b) to share the same
/// placeholder when the same pair is encountered in different contexts.
pub(crate) fn anti_unify(
    a: &Expr,
    b: &Expr,
) -> Option<(Expr, HashMap<String, Expr>, HashMap<String, Expr>)> {
    let mut state = PlaceholderState::new();
    let mut subst_a: HashMap<String, Expr> = HashMap::new();
    let mut subst_b: HashMap<String, Expr> = HashMap::new();
    // Shared placeholder key: a pointer pair is used as key so the same
    // diff pair gets the same placeholder across recursive calls.
    // We use a simple HashMap keyed by (ptr_a, ptr_b).
    let mut placeholder_map: HashMap<(usize, usize), String> = HashMap::new();

    let result = anti_unify_expr(
        a,
        b,
        &mut state,
        &mut placeholder_map,
        &mut subst_a,
        &mut subst_b,
    )?;
    Some((result, subst_a, subst_b))
}

/// 2026-07-29: Recursive anti-unification step. Walks two expression trees
/// in parallel, comparing nodes and creating placeholders where they differ.
fn anti_unify_expr(
    a: &Expr,
    b: &Expr,
    state: &mut PlaceholderState,
    placeholder_map: &mut HashMap<(usize, usize), String>,
    subst_a: &mut HashMap<String, Expr>,
    subst_b: &mut HashMap<String, Expr>,
) -> Option<Expr> {
    // Handle identical expressions (same variant and equal contents)
    if a == b {
        return Some(a.clone());
    }

    match (a, b) {
        // 2026-07-29: Same operator → anti-unify children pairwise.
        // For commutative BinaryOps, try both orderings to maximize
        // structural match.
        (Expr::BinaryOp(k1, l1, r1), Expr::BinaryOp(k2, l2, r2)) if k1 == k2 => {
            // 2026-07-29: Anti-unify children pairwise.
            // For commutative ops, try (l1,r1)↔(l2,r2) only.
            // We DON'T try swapped orderings because that would create
            // different placeholder patterns for the same pair and
            // inflate the substitution set.
            Some(Expr::BinaryOp(
                *k1,
                Box::new(anti_unify_expr(
                    l1,
                    l2,
                    state,
                    placeholder_map,
                    subst_a,
                    subst_b,
                )?),
                Box::new(anti_unify_expr(
                    r1,
                    r2,
                    state,
                    placeholder_map,
                    subst_a,
                    subst_b,
                )?),
            ))
        }
        (Expr::UnaryOp(k1, i1), Expr::UnaryOp(k2, i2)) if k1 == k2 => Some(Expr::UnaryOp(
            *k1,
            Box::new(anti_unify_expr(
                i1,
                i2,
                state,
                placeholder_map,
                subst_a,
                subst_b,
            )?),
        )),
        (Expr::If(c1, t1, e1), Expr::If(c2, t2, e2)) => {
            let cond = anti_unify_expr(c1, c2, state, placeholder_map, subst_a, subst_b)?;
            let then = anti_unify_expr(t1, t2, state, placeholder_map, subst_a, subst_b)?;
            let else_expr = match (e1, e2) {
                (Some(ee1), Some(ee2)) => Some(Box::new(anti_unify_expr(
                    ee1,
                    ee2,
                    state,
                    placeholder_map,
                    subst_a,
                    subst_b,
                )?)),
                (None, None) => None,
                _ => {
                    // One has else, other doesn't → need placeholder
                    let var = placeholder_map
                        .entry(ptr_pair(a, b))
                        .or_insert_with(|| state.fresh());
                    subst_a.insert(var.clone(), a.clone());
                    subst_b.insert(var.clone(), b.clone());
                    return Some(Expr::Identifier(var.clone()));
                }
            };
            Some(Expr::If(Box::new(cond), Box::new(then), else_expr))
        }
        (Expr::Call(n1, args1, _), Expr::Call(n2, args2, _))
            if n1 == n2 && args1.len() == args2.len() =>
        {
            let args: Vec<Expr> = args1
                .iter()
                .zip(args2.iter())
                .map(|(a1, a2)| anti_unify_expr(a1, a2, state, placeholder_map, subst_a, subst_b))
                .collect::<Option<Vec<_>>>()?;
            Some(Expr::Call(n1.clone(), args, None))
        }
        // 2026-07-29: Different operators or types → create placeholder.
        // The placeholder captures both entire sub-expressions, allowing
        // the anti-unifier to continue at higher levels.
        _ => {
            let key = ptr_pair(a, b);
            let var = placeholder_map.entry(key).or_insert_with(|| state.fresh());
            subst_a.insert(var.clone(), a.clone());
            subst_b.insert(var.clone(), b.clone());
            Some(Expr::Identifier(var.clone()))
        }
    }
}

/// 2026-07-29: Get a pointer-pair key for the placeholder map.
fn ptr_pair(a: &Expr, b: &Expr) -> (usize, usize) {
    (a as *const _ as usize, b as *const _ as usize)
}

// ── Savings Computation ──────────────────────────────────────────────

/// 2026-07-29: Compute the size savings of extracting a helper from a
/// pair of expressions. Positive savings means the helper reduces total size.
///
/// savings = (size(e1) + size(e2)) - (size(common) + size(diff1) + size(diff2))
///
/// A positive value means the abstraction reduces total expression size
/// (and thus improves the search space). Zero means it's break-even.
pub(crate) fn compute_savings(
    e1: &Expr,
    e2: &Expr,
    common: &Expr,
    sigma1: &HashMap<String, Expr>,
    sigma2: &HashMap<String, Expr>,
) -> i64 {
    let size_e1 = expr_size(e1) as i64;
    let size_e2 = expr_size(e2) as i64;
    let size_common = expr_size(common) as i64;
    let size_diff1: i64 = sigma1.values().map(|e| expr_size(e) as i64).sum();
    let size_diff2: i64 = sigma2.values().map(|e| expr_size(e) as i64).sum();

    (size_e1 + size_e2) - (size_common + size_diff1 + size_diff2)
}

// ── Free Variables ───────────────────────────────────────────────────

/// 2026-07-29: Collect the set of parameter names referenced in an expression.
fn free_variables(expr: &Expr, param_names: &[String]) -> Vec<String> {
    let mut vars = Vec::new();
    collect_free_vars(expr, param_names, &mut vars);
    let mut seen = HashSet::new();
    vars.retain(|v| seen.insert(v.clone()));
    vars
}

fn collect_free_vars(expr: &Expr, param_names: &[String], vars: &mut Vec<String>) {
    match expr {
        Expr::Identifier(name) => {
            if param_names.iter().any(|p| p == name) && !vars.contains(name) {
                vars.push(name.clone());
            }
        }
        Expr::UnaryOp(_, inner) => collect_free_vars(inner, param_names, vars),
        Expr::BinaryOp(_, lhs, rhs) => {
            collect_free_vars(lhs, param_names, vars);
            collect_free_vars(rhs, param_names, vars);
        }
        Expr::If(cond, then, else_) => {
            collect_free_vars(cond, param_names, vars);
            collect_free_vars(then, param_names, vars);
            if let Some(e) = else_ {
                collect_free_vars(e, param_names, vars);
            }
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_free_vars(arg, param_names, vars);
            }
        }
        Expr::Field(inner, _) => collect_free_vars(inner, param_names, vars),
        Expr::Match(scrut, arms) => {
            collect_free_vars(scrut, param_names, vars);
            for arm in arms {
                collect_free_vars(&arm.body, param_names, vars);
            }
        }
        _ => {}
    }
}

// ── Return Type Inference ───────────────────────────────────────────

/// 2026-07-29: Infer the return type of an expression.
fn infer_return_type(expr: &Expr) -> String {
    match expr {
        Expr::Decimal(_) => "Int".to_string(),
        Expr::Float(_) => "Float".to_string(),
        Expr::Bool(_) => "Bool".to_string(),
        Expr::Identifier(_) => "Int".to_string(),
        Expr::UnaryOp(UnaryOpKind::Neg, _) => "Int".to_string(),
        Expr::UnaryOp(UnaryOpKind::Not, _) => "Bool".to_string(),
        Expr::UnaryOp(UnaryOpKind::BitNot, _) => "Int".to_string(),
        Expr::BinaryOp(op, _, _) => match op {
            BinaryOpKind::Add
            | BinaryOpKind::Sub
            | BinaryOpKind::Mul
            | BinaryOpKind::Div
            | BinaryOpKind::Mod
            | BinaryOpKind::BitAnd
            | BinaryOpKind::BitOr
            | BinaryOpKind::BitXor
            | BinaryOpKind::Shl
            | BinaryOpKind::Shr => "Int".to_string(),
            BinaryOpKind::Eq
            | BinaryOpKind::Neq
            | BinaryOpKind::Lt
            | BinaryOpKind::Gt
            | BinaryOpKind::Le
            | BinaryOpKind::Ge
            | BinaryOpKind::And
            | BinaryOpKind::Or => "Bool".to_string(),
            BinaryOpKind::Concat => "Int".to_string(),
        },
        Expr::If(_, _, _) => "Int".to_string(),
        _ => "Int".to_string(),
    }
}

// ── Anti-Unification with Replacement Discovery ─────────────────────

/// 2026-07-29: Discover and register helpers via anti-unification.
///
/// For each pair of expressions in the LevelCache:
/// 1. Anti-unify to find common sub-structure
/// 2. Compute size savings
/// 3. If savings > 0, create a helper and REPLACE the originals
///
/// Returns the discovered helpers (with use_count=0 for GC tracking).
///
/// Adapted from Feser et al. λ² (PLDI 2015) §4: anti-unification of
/// conditional branches produces lambda abstractions that capture common
/// computation. Here we generalize to anti-unification of any pair of
/// expressions in the LevelCache.
pub(crate) fn discover_and_register_helpers(
    cache: &mut LevelCache,
    param_names: &[String],
    param_types: &[String],
    config: &DiscoverConfig,
    name_counter: &mut usize,
) -> Vec<HelperFunction> {
    // Step 1: Collect all expressions from the LevelCache by type
    let mut by_type: HashMap<String, Vec<Expr>> = HashMap::new();
    for e in &cache.int_exprs {
        by_type
            .entry("Int".to_string())
            .or_default()
            .push(e.clone());
    }
    for e in &cache.float_exprs {
        by_type
            .entry("Float".to_string())
            .or_default()
            .push(e.clone());
    }
    for e in &cache.bool_exprs {
        by_type
            .entry("Bool".to_string())
            .or_default()
            .push(e.clone());
    }
    for (ty, list) in &cache.compound_exprs {
        for e in list {
            by_type.entry(ty.clone()).or_default().push(e.clone());
        }
    }

    let mut helpers: Vec<HelperFunction> = Vec::new();
    // Track which expressions have been replaced (by index in the original vec)
    // so we don't process or replace the same expression twice.
    let mut replaced: HashSet<(String, usize)> = HashSet::new();

    // 2026-07-29: Skip anti-unification for Bool expressions at depth < 3.
    // Bool expressions are used as IF conditions at depth 3+. Replacing them
    // with helper calls disrupts the IF-generation because helper calls are
    // at the end of bool_exprs and might not be reached by the beam.
    // Bool expressions are also typically small (comparisons) and don't
    // need compression.
    // This is a heuristic: at higher depths, Bool expressions are complex
    // enough that abstraction is safe.
    for (ret_type, exprs) in &by_type {
        if exprs.len() < 2 {
            continue;
        }
        // 2026-07-29: Skip Bool at depth 2 to preserve IF generation.
        // A more principled fix: only anti-unify types that have enough
        // complexity (expr_size > threshold) to benefit from abstraction.
        if ret_type == "Bool" && exprs.iter().any(|e| expr_size(e) <= 3) {
            continue;
        }
        let max_h = config
            .max_helpers_per_type
            .saturating_sub(helpers.iter().filter(|h| h.ret_type == *ret_type).count());
        if max_h == 0 {
            continue;
        }

        // Step 2: Iterate over all pairs in this type group
        for i in 0..exprs.len() {
            if replaced.contains(&(ret_type.clone(), i)) {
                continue;
            }
            if helpers.len() >= config.max_helpers_per_type {
                break;
            }

            for j in (i + 1)..exprs.len() {
                if replaced.contains(&(ret_type.clone(), j)) {
                    continue;
                }
                if helpers.len() >= config.max_helpers_per_type {
                    break;
                }
                if helpers.iter().filter(|h| h.ret_type == *ret_type).count() >= max_h {
                    break;
                }

                // Skip identical expressions (no abstraction possible)
                if exprs[i] == exprs[j] {
                    continue;
                }

                let result = anti_unify(&exprs[i], &exprs[j]);
                let (common, sigma_a, sigma_b) = match result {
                    Some(r) => r,
                    None => continue,
                };

                // 2026-07-29: Skip anti-unifiers that are just a single placeholder
                // (completely different structure → no useful abstraction).
                if matches!(&common, Expr::Identifier(n) if n.starts_with("_t")) {
                    continue;
                }

                let savings = compute_savings(&exprs[i], &exprs[j], &common, &sigma_a, &sigma_b);
                if savings < config.min_savings {
                    continue;
                }

                // 2026-07-29: Verify that the DIFFERENCES between expressions
                // are only in CONSTANT values, not in variable/param references.
                // If a variable becomes a placeholder, the helper gets an extra
                // parameter for each variable difference, which bloats the call
                // and reduces the abstraction's value. Constants-as-params is
                // fine (e.g., _h0(x, 1) for x+1).
                let all_subs_const = sigma_a
                    .values()
                    .chain(sigma_b.values())
                    .all(|e| matches!(e, Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_)));
                if !all_subs_const {
                    continue;
                }

                // Step 3: Determine helper parameters.
                // Parameters = (common's free vars) + (placeholder vars that are params)
                let common_free_vars = free_variables(&common, param_names);
                // Placeholder variables referenced in the common pattern
                let placeholder_vars: Vec<String> = {
                    let mut pv = Vec::new();
                    collect_placeholders(&common, &mut pv);
                    let mut seen = HashSet::new();
                    pv.retain(|v| seen.insert(v.clone()));
                    pv
                };

                let all_params: Vec<String> = {
                    let mut p = common_free_vars.clone();
                    for pv in &placeholder_vars {
                        if !p.contains(pv) {
                            p.push(pv.clone());
                        }
                    }
                    p
                };

                let param_types_resolved: Vec<String> = all_params
                    .iter()
                    .map(|p| {
                        // Try to find type from original param_names
                        if let Some(idx) = param_names.iter().position(|n| n == p) {
                            param_types
                                .get(idx)
                                .cloned()
                                .unwrap_or_else(|| "Int".to_string())
                        } else {
                            // Placeholder — infer from substitution expressions
                            if let Some(sub_expr) = sigma_a.get(p) {
                                infer_return_type(sub_expr)
                            } else {
                                "Int".to_string()
                            }
                        }
                    })
                    .collect();

                let name = format!("_h{}", *name_counter);
                *name_counter += 1;

                let body_cost = CostModel::default().cost_of_expr(&common);
                let call_cost = 3 + all_params.len() as u64;

                let helper = HelperFunction {
                    name: name.clone(),
                    params: all_params,
                    param_types: param_types_resolved,
                    body: common,
                    ret_type: ret_type.clone(),
                    body_cost,
                    call_cost,
                    use_count: 0,
                };

                // Step 4: Build replacement calls.
                // For each placeholder, substitute the actual expression from sigma_a/sigma_b.
                let call_a = build_helper_call(&name, &helper.params, &sigma_a, &exprs[i]);
                let call_b = build_helper_call(&name, &helper.params, &sigma_b, &exprs[j]);

                // Step 5: Replace originals in the LevelCache.
                // We mark these as replaced and add the calls.
                replaced.insert((ret_type.clone(), i));
                replaced.insert((ret_type.clone(), j));

                // Register the helper in helper_names and helper_info
                if !cache.helper_names.contains(&name) {
                    cache.helper_names.push(name.clone());
                }
                cache
                    .helper_info
                    .entry(name.clone())
                    .or_insert_with(|| (helper.params.clone(), helper.ret_type.clone()));

                // Add the replacement calls to the LevelCache
                match ret_type.as_str() {
                    "Int" => {
                        cache.int_exprs.push(call_a);
                        cache.int_exprs.push(call_b);
                    }
                    "Float" => {
                        cache.float_exprs.push(call_a);
                        cache.float_exprs.push(call_b);
                    }
                    "Bool" => {
                        cache.bool_exprs.push(call_a);
                        cache.bool_exprs.push(call_b);
                    }
                    _ => {
                        cache
                            .compound_exprs
                            .entry(ret_type.clone())
                            .or_default()
                            .push(call_a);
                        cache
                            .compound_exprs
                            .entry(ret_type.clone())
                            .or_default()
                            .push(call_b);
                    }
                }

                helpers.push(helper);
            }
        }

        // Step 6: Remove replaced expressions from the LevelCache.
        // We rebuild each bucket keeping only non-replaced expressions.
        match ret_type.as_str() {
            "Int" => {
                let mut kept: Vec<Expr> = Vec::new();
                for (idx, e) in cache.int_exprs.drain(..).enumerate() {
                    if !replaced.contains(&(ret_type.clone(), idx)) {
                        kept.push(e);
                    }
                }
                cache.int_exprs = kept;
            }
            "Float" => {
                let mut kept: Vec<Expr> = Vec::new();
                for (idx, e) in cache.float_exprs.drain(..).enumerate() {
                    if !replaced.contains(&(ret_type.clone(), idx)) {
                        kept.push(e);
                    }
                }
                cache.float_exprs = kept;
            }
            "Bool" => {
                let mut kept: Vec<Expr> = Vec::new();
                for (idx, e) in cache.bool_exprs.drain(..).enumerate() {
                    if !replaced.contains(&(ret_type.clone(), idx)) {
                        kept.push(e);
                    }
                }
                cache.bool_exprs = kept;
            }
            _ => {
                if let Some(list) = cache.compound_exprs.get_mut(ret_type) {
                    let mut kept: Vec<Expr> = Vec::new();
                    for (idx, e) in list.drain(..).enumerate() {
                        if !replaced.contains(&(ret_type.clone(), idx)) {
                            kept.push(e);
                        }
                    }
                    *list = kept;
                }
            }
        }
    }

    helpers
}

/// 2026-07-29: Build a helper call expression, substituting placeholder
/// variables with the actual expressions from sigma.
fn build_helper_call(
    name: &str,
    params: &[String],
    sigma: &HashMap<String, Expr>,
    original: &Expr,
) -> Expr {
    let args: Vec<Expr> = params
        .iter()
        .map(|p| {
            // If this param is a placeholder with a substitution, use the sub expression
            if let Some(sub) = sigma.get(p) {
                sub.clone()
            } else {
                // Otherwise it's a regular param — keep it as identifier
                Expr::Identifier(p.clone())
            }
        })
        .collect();
    Expr::Call(name.to_string(), args, None)
}

/// 2026-07-29: Collect placeholder variable names referenced in an expression.
fn collect_placeholders(expr: &Expr, vars: &mut Vec<String>) {
    match expr {
        Expr::Identifier(name) if name.starts_with("_t") => {
            if !vars.contains(name) {
                vars.push(name.clone());
            }
        }
        Expr::UnaryOp(_, inner) => collect_placeholders(inner, vars),
        Expr::BinaryOp(_, lhs, rhs) => {
            collect_placeholders(lhs, vars);
            collect_placeholders(rhs, vars);
        }
        Expr::If(cond, then, else_) => {
            collect_placeholders(cond, vars);
            collect_placeholders(then, vars);
            if let Some(e) = else_ {
                collect_placeholders(e, vars);
            }
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_placeholders(arg, vars);
            }
        }
        Expr::Field(inner, _) => collect_placeholders(inner, vars),
        Expr::Match(scrut, arms) => {
            collect_placeholders(scrut, vars);
            for arm in arms {
                collect_placeholders(&arm.body, vars);
            }
        }
        _ => {}
    }
}

// ── Garbage Collection ───────────────────────────────────────────────

/// 2026-07-29: Garbage-collect unused helpers after each depth level.
pub(crate) fn gc_helpers(cache: &mut LevelCache, helpers: &mut Vec<HelperFunction>) {
    let active: HashSet<String> = helpers
        .iter()
        .filter(|h| h.use_count > 0)
        .map(|h| h.name.clone())
        .collect();

    helpers.retain(|h| h.use_count > 0);
    cache.helper_names.retain(|name| active.contains(name));
    cache.helper_info.retain(|name, _| active.contains(name));
}

/// 2026-07-29: Increment use counts for helpers referenced in candidates.
pub fn count_helper_uses(helpers: &mut Vec<HelperFunction>, candidates: &[Expr]) {
    for helper in helpers.iter_mut() {
        helper.use_count = 0;
    }
    for candidate in candidates {
        count_uses_in_expr(candidate, helpers);
    }
}

fn count_uses_in_expr(expr: &Expr, helpers: &mut [HelperFunction]) {
    match expr {
        Expr::Call(name, _, _) => {
            if let Some(helper) = helpers.iter_mut().find(|h| h.name == *name) {
                helper.use_count += 1;
            }
        }
        Expr::UnaryOp(_, inner) => count_uses_in_expr(inner, helpers),
        Expr::BinaryOp(_, lhs, rhs) => {
            count_uses_in_expr(lhs, helpers);
            count_uses_in_expr(rhs, helpers);
        }
        Expr::If(cond, then, else_) => {
            count_uses_in_expr(cond, helpers);
            count_uses_in_expr(then, helpers);
            if let Some(e) = else_ {
                count_uses_in_expr(e, helpers);
            }
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                count_uses_in_expr(arg, helpers);
            }
        }
        Expr::Field(inner, _) => count_uses_in_expr(inner, helpers),
        Expr::Match(scrut, arms) => {
            count_uses_in_expr(scrut, helpers);
            for arm in arms {
                count_uses_in_expr(&arm.body, helpers);
            }
        }
        _ => {}
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::engine::LevelCache;

    fn empty_cache() -> LevelCache {
        LevelCache {
            int_exprs: vec![],
            float_exprs: vec![],
            bool_exprs: vec![],
            compound_exprs: std::collections::HashMap::new(),
            helper_names: vec![],
            helper_info: std::collections::HashMap::new(),
        }
    }

    fn int_var(name: &str) -> Expr {
        Expr::Identifier(name.to_string())
    }
    fn int_const(n: i64) -> Expr {
        Expr::Decimal(n)
    }
    fn bool_const(b: bool) -> Expr {
        Expr::Bool(b)
    }
    fn binop(kind: BinaryOpKind, lhs: Expr, rhs: Expr) -> Expr {
        Expr::BinaryOp(kind, Box::new(lhs), Box::new(rhs))
    }
    fn uneg(inner: Expr) -> Expr {
        Expr::UnaryOp(UnaryOpKind::Neg, Box::new(inner))
    }

    // ── Anti-Unification Tests ────────────────────────────────────

    #[test]
    fn test_anti_unify_identical() {
        let expr = binop(BinaryOpKind::Add, int_var("x"), int_const(1));
        let result = anti_unify(&expr, &expr);
        assert!(result.is_some());
        let (common, sigma_a, sigma_b) = result.unwrap();
        assert_eq!(
            common, expr,
            "identical exprs should produce same anti-unifier"
        );
        assert!(sigma_a.is_empty(), "no substitutions for identical");
        assert!(sigma_b.is_empty(), "no substitutions for identical");
    }

    #[test]
    fn test_anti_unify_same_op_diff_constant() {
        let e1 = binop(BinaryOpKind::Add, int_var("x"), int_const(1));
        let e2 = binop(BinaryOpKind::Add, int_var("x"), int_const(2));
        let result = anti_unify(&e1, &e2);
        assert!(result.is_some(), "same op + diff constant should unify");
        let (common, sigma_a, sigma_b) = result.unwrap();
        // Common: x + _t0
        assert!(matches!(&common, Expr::BinaryOp(BinaryOpKind::Add, _, _)));
        let has_placeholder = placeholder_count(&common);
        assert_eq!(has_placeholder, 1, "should have exactly 1 placeholder");
        // Placeholder should map to 1 for e1, 2 for e2
        assert_eq!(sigma_a.len(), 1);
        assert_eq!(sigma_b.len(), 1);
    }

    #[test]
    fn test_anti_unify_same_op_diff_var() {
        let e1 = binop(BinaryOpKind::Add, int_var("x"), int_const(1));
        let e2 = binop(BinaryOpKind::Add, int_var("y"), int_const(1));
        let result = anti_unify(&e1, &e2);
        assert!(result.is_some());
        let (common, sigma_a, sigma_b) = result.unwrap();
        // Common: _t0 + 1
        assert!(has_placeholder(&common));
        assert_eq!(sigma_a.len(), 1);
        assert_eq!(sigma_b.len(), 1);
    }

    #[test]
    fn test_anti_unify_diff_op() {
        let e1 = binop(BinaryOpKind::Add, int_var("x"), int_const(1));
        let e2 = binop(BinaryOpKind::Mul, int_var("x"), int_const(2));
        let result = anti_unify(&e1, &e2);
        // Different ops: should still unify at the top level with placeholder
        assert!(result.is_some());
        let (common, sigma_a, sigma_b) = result.unwrap();
        assert_eq!(sigma_a.len(), 1, "should have 1 substitution for e1");
        assert_eq!(sigma_b.len(), 1, "should have 1 substitution for e2");
    }

    #[test]
    fn test_anti_unify_nested() {
        // (x + 1) * (x + 2) and (x + 1) * (x + 3)
        let sub1 = binop(BinaryOpKind::Add, int_var("x"), int_const(2));
        let sub2 = binop(BinaryOpKind::Add, int_var("x"), int_const(3));
        let e1 = binop(
            BinaryOpKind::Mul,
            binop(BinaryOpKind::Add, int_var("x"), int_const(1)),
            sub1,
        );
        let e2 = binop(
            BinaryOpKind::Mul,
            binop(BinaryOpKind::Add, int_var("x"), int_const(1)),
            sub2,
        );
        // (x+1) is shared, (x+2)/(x+3) differ → should produce (x+1) * (x + _t0)
        let result = anti_unify(&e1, &e2);
        assert!(result.is_some());
        let (common, sigma_a, sigma_b) = result.unwrap();
        // Common should be (x + 1) * (x + _t0)
        let has_ph = placeholder_count(&common);
        assert!(has_ph >= 1, "nested should have at least 1 placeholder");
        assert!(sigma_a.len() >= 1);
    }

    #[test]
    fn test_anti_unify_commutative_swap() {
        // x + y and y + x — since we don't normalize for commutativity,
        // the anti-unifier produces placeholders for each pair:
        // anti-unify(x, y) → _t0, anti-unify(y, x) → _t1 → _t0 + _t1
        let e1 = binop(BinaryOpKind::Add, int_var("x"), int_var("y"));
        let e2 = binop(BinaryOpKind::Add, int_var("y"), int_var("x"));
        let result = anti_unify(&e1, &e2);
        assert!(result.is_some());
        let (common, sigma_a, sigma_b) = result.unwrap();
        // Both children differ → 2 placeholders
        assert!(sigma_a.len() >= 1);
        assert!(sigma_b.len() >= 1);
        // Make sure savings are computed correctly
        let savings = compute_savings(&e1, &e2, &common, &sigma_a, &sigma_b);
        // savings = (3+3) - (1+3+3) = 6-7 = -1 → negative (expected: no savings for swap)
        assert!(savings < 0, "swap should have negative savings");
    }

    // ── Savings Tests ─────────────────────────────────────────────

    #[test]
    fn test_savings_positive() {
        let e1 = binop(BinaryOpKind::Add, int_var("x"), int_const(1)); // size 3
        let e2 = binop(BinaryOpKind::Add, int_var("x"), int_const(2)); // size 3
        let (common, sigma_a, sigma_b) = anti_unify(&e1, &e2).unwrap();
        let savings = compute_savings(&e1, &e2, &common, &sigma_a, &sigma_b);
        // common = x + _t0 (size 3), sigma_a: _t0→1 (size 1), sigma_b: _t0→2 (size 1)
        // savings = (3+3) - (3+1+1) = 6 - 5 = 1
        assert!(savings > 0, "savings should be positive: got {}", savings);
    }

    #[test]
    fn test_savings_zero_identical() {
        let e1 = binop(BinaryOpKind::Add, int_var("x"), int_const(1));
        let e2 = binop(BinaryOpKind::Add, int_var("x"), int_const(1)); // identical
        let (common, sigma_a, sigma_b) = anti_unify(&e1, &e2).unwrap();
        let savings = compute_savings(&e1, &e2, &common, &sigma_a, &sigma_b);
        // identical → common = e1 = e2, sigma_empty. savings = (3+3) - (3+0+0) = 3
        assert!(savings >= 0, "identical should have non-negative savings");
    }

    #[test]
    fn test_savings_negative_different_ops() {
        let e1 = binop(BinaryOpKind::Add, int_var("x"), int_const(1)); // size 3
        let e2 = binop(BinaryOpKind::Sub, int_var("x"), int_var("y")); // size 3
        let (common, sigma_a, sigma_b) = anti_unify(&e1, &e2).unwrap();
        let savings = compute_savings(&e1, &e2, &common, &sigma_a, &sigma_b);
        // Different ops → top-level placeholder. common = _t0 (size 1),
        // sigma_a: _t0→e1 (size 3), sigma_b: _t0→e2 (size 3)
        // savings = (3+3) - (1+3+3) = 6 - 7 = -1
        assert!(
            savings < 0,
            "different ops should have negative savings: got {}",
            savings
        );
    }

    // ── discover_and_register_helpers Tests ───────────────────────

    #[test]
    fn test_discover_anti_unify_empty_cache() {
        let mut cache = empty_cache();
        let mut counter = 0;
        let helpers = discover_and_register_helpers(
            &mut cache,
            &[],
            &[],
            &DiscoverConfig::default(),
            &mut counter,
        );
        assert!(helpers.is_empty(), "empty cache should produce no helpers");
    }

    #[test]
    fn test_discover_anti_unify_single_expr() {
        let mut cache = empty_cache();
        cache.int_exprs.push(int_var("x"));
        let mut counter = 0;
        let helpers = discover_and_register_helpers(
            &mut cache,
            &["x".into()],
            &["Int".into()],
            &DiscoverConfig::default(),
            &mut counter,
        );
        assert!(helpers.is_empty(), "single expr should produce no helpers");
    }

    #[test]
    fn test_discover_anti_unify_reusable_add() {
        let mut cache = empty_cache();
        // Use deeper expressions to get savings >= 2
        // (x+1)*y and (x+2)*y → common = (x+_t0)*y, savings = (5+5) - (5+1+1) = 3
        let e1 = binop(
            BinaryOpKind::Mul,
            binop(BinaryOpKind::Add, int_var("x"), int_const(1)),
            int_var("y"),
        );
        let e2 = binop(
            BinaryOpKind::Mul,
            binop(BinaryOpKind::Add, int_var("x"), int_const(2)),
            int_var("y"),
        );
        cache.int_exprs.push(e1);
        cache.int_exprs.push(e2);
        let mut counter = 0;
        let helpers = discover_and_register_helpers(
            &mut cache,
            &["x".into(), "y".into()],
            &["Int".into(), "Int".into()],
            &DiscoverConfig::default(),
            &mut counter,
        );
        // (x+1)*y and (x+2)*y → common = (x+_t0)*y → helper with savings >= 2
        assert!(!helpers.is_empty(), "should discover at least one helper");
        assert!(cache.helper_names.contains(&"_h0".to_string()));
    }

    #[test]
    fn test_discover_anti_unify_no_savings() {
        let mut cache = empty_cache();
        // x+1 and y-2 have different structures, so anti-unification
        // produces a top-level placeholder with negative savings.
        cache
            .int_exprs
            .push(binop(BinaryOpKind::Add, int_var("x"), int_const(1)));
        cache
            .int_exprs
            .push(binop(BinaryOpKind::Sub, int_var("y"), int_const(2)));
        let mut counter = 0;
        let helpers = discover_and_register_helpers(
            &mut cache,
            &["x".into(), "y".into()],
            &["Int".into(), "Int".into()],
            &DiscoverConfig::default(),
            &mut counter,
        );
        // Savings should be negative → no helper
        assert!(
            helpers.is_empty(),
            "no helper for structurally different exprs"
        );
    }

    #[test]
    fn test_replacement_removes_originals() {
        let mut cache = empty_cache();
        // Use deeper expressions (savings >= 2):
        // (x+1)*y and (x+2)*y → common = (x+_t0)*y, savings = 3
        let e1 = binop(
            BinaryOpKind::Mul,
            binop(BinaryOpKind::Add, int_var("x"), int_const(1)),
            int_var("y"),
        );
        let e2 = binop(
            BinaryOpKind::Mul,
            binop(BinaryOpKind::Add, int_var("x"), int_const(2)),
            int_var("y"),
        );
        let e3 = int_var("x"); // third unrelated expression
        cache.int_exprs.push(e1);
        cache.int_exprs.push(e2);
        cache.int_exprs.push(e3);

        let before = cache.int_exprs.len();
        let mut counter = 0;
        discover_and_register_helpers(
            &mut cache,
            &["x".into(), "y".into()],
            &["Int".into(), "Int".into()],
            &DiscoverConfig::default(),
            &mut counter,
        );

        // After replacement: 2 originals removed, 2 helper calls added
        // But e3 remains → total = 2 helper calls + e3 = 3
        assert_eq!(
            cache.int_exprs.len(),
            before,
            "replaced 2 exprs with 2 calls + 1 original"
        );
        // The helper calls should be Expr::Call nodes
        let call_count = cache
            .int_exprs
            .iter()
            .filter(|e| matches!(e, Expr::Call(_, _, _)))
            .count();
        assert!(call_count >= 1, "should have at least one replacement call");
    }

    #[test]
    fn test_discover_anti_unify_filters_by_savings() {
        let mut cache = empty_cache();
        // Two structurally equal expressions (x+1 and x+1) but we skip identical
        cache
            .int_exprs
            .push(binop(BinaryOpKind::Add, int_var("x"), int_const(1)));
        cache
            .int_exprs
            .push(binop(BinaryOpKind::Add, int_var("x"), int_const(1)));
        // With an intervening expression that differs
        cache
            .int_exprs
            .push(binop(BinaryOpKind::Add, int_var("x"), int_const(3)));

        let strict = DiscoverConfig {
            min_savings: 10,
            ..Default::default()
        };
        let mut counter = 0;
        let helpers = discover_and_register_helpers(
            &mut cache,
            &["x".into()],
            &["Int".into()],
            &strict,
            &mut counter,
        );
        // savings = (3+3) - (3+1+1) = 1 < 10 → no helpers
        assert!(helpers.is_empty(), "strict savings filter should reject");
    }

    // ── GC Tests ──────────────────────────────────────────────────

    #[test]
    fn test_gc_unused_helper() {
        let mut cache = empty_cache();
        let mut helpers = vec![HelperFunction {
            name: "_h0".to_string(),
            params: vec!["x".into()],
            param_types: vec!["Int".into()],
            body: binop(BinaryOpKind::Add, int_var("x"), int_const(1)),
            ret_type: "Int".to_string(),
            body_cost: 5,
            call_cost: 4,
            use_count: 0,
        }];
        cache.helper_names.push("_h0".into());
        cache
            .helper_info
            .insert("_h0".into(), (vec!["x".into()], "Int".into()));

        gc_helpers(&mut cache, &mut helpers);

        assert!(helpers.is_empty(), "unused helper should be GC'd");
        assert!(!cache.helper_names.contains(&"_h0".to_string()));
        assert!(!cache.helper_info.contains_key("_h0"));
    }

    #[test]
    fn test_gc_keeps_used() {
        let mut cache = empty_cache();
        let mut helpers = vec![HelperFunction {
            name: "_h0".to_string(),
            params: vec!["x".into()],
            param_types: vec!["Int".into()],
            body: binop(BinaryOpKind::Add, int_var("x"), int_const(1)),
            ret_type: "Int".to_string(),
            body_cost: 5,
            call_cost: 4,
            use_count: 3,
        }];
        cache.helper_names.push("_h0".into());
        cache
            .helper_info
            .insert("_h0".into(), (vec!["x".into()], "Int".into()));

        gc_helpers(&mut cache, &mut helpers);

        assert_eq!(helpers.len(), 1, "used helper should survive GC");
    }

    // ── Helper Functions for Tests ────────────────────────────────

    /// Count placeholder variables (starting with "_t") in an expression.
    fn placeholder_count(expr: &Expr) -> usize {
        let mut vars = Vec::new();
        collect_placeholders(expr, &mut vars);
        vars.len()
    }

    /// Check if expression has any placeholder variables.
    fn has_placeholder(expr: &Expr) -> bool {
        placeholder_count(expr) > 0
    }
}
