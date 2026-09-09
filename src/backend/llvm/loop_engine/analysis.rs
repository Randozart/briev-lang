// ── Loop Emission: Free-Standing Analysis Helpers ──────────────
//
// 2026-07-13: Extracted from monolithic loop_engine.rs. Contains
// free-standing helper functions (not in `impl LlvmBackend`) for
// liveness analysis, parallel safety, rotation detection, and
// trigger event dispatch.

use crate::backend::llvm::*;
use std::collections::HashMap;
use std::collections::HashSet;

/// Check if a block of statements contains a `Term` or `TermBang` with
/// an expression — meaning the block always exits.
pub fn terminating_guard(statements: &[Statement]) -> bool {
    statements.iter().any(|s| {
        matches!(s, Statement::Term(Some(_)) | Statement::EndProgram(Some(_)))
    })
}

/// Collect field names that are exempt from parallel-safety checks.
pub fn collect_parallel_safe_exemptions(
    body: &[Statement],
    exempt_fields: &mut HashSet<String>,
    guard_exempt_fields: &mut HashSet<String>,
    field_index_map: &HashMap<String, usize>,
) {
    for stmt in body {
        match stmt {
            Statement::Assign(lhs, expr) => {
                if let Expr::Identifier(n) = lhs {
                    if !field_index_map.contains_key(n) {
                        exempt_fields.insert(n.clone());
                    }
                }
                exempt_side_effect_args(expr, exempt_fields, field_index_map);
            }
            Statement::Let { expr: Some(e), .. } => {
                exempt_side_effect_args(e, exempt_fields, field_index_map);
            }
            _ => {}
        }
    }
}

/// Extract side-effect argument references for exemption tracking.
pub fn exempt_side_effect_args(
    expr: &Expr,
    fields: &mut HashSet<String>,
    field_index_map: &HashMap<String, usize>,
) {
    match expr {
        Expr::Call(_name, args, _) => {
            for arg in args {
                collect_field_ids_from_expr(arg, fields, field_index_map);
            }
        }
        _ => {}
    }
}

/// Collect field IDs referenced in an expression into a set.
pub fn collect_field_ids_from_expr(
    e: &Expr,
    fields: &mut HashSet<String>,
    field_index_map: &HashMap<String, usize>,
) {
    match e {
        Expr::Identifier(name) => {
            if field_index_map.contains_key(name) {
                fields.insert(name.clone());
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_field_ids_from_expr(l, fields, field_index_map);
            collect_field_ids_from_expr(r, fields, field_index_map);
        }
        Expr::UnaryOp(_, inner) => {
            collect_field_ids_from_expr(inner, fields, field_index_map);
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_field_ids_from_expr(arg, fields, field_index_map);
            }
        }
        Expr::Field(obj, _) => {
            collect_field_ids_from_expr(obj, fields, field_index_map);
        }
        Expr::Cast(inner, _) => {
            collect_field_ids_from_expr(inner, fields, field_index_map);
        }
        _ => {}
    }
}

/// Extract side-effect reads from a statement.
pub fn extract_side_effect_reads(
    stmt: &Statement,
    field_index_map: &HashMap<String, usize>,
) -> Vec<String> {
    let mut result = Vec::new();
    match stmt {
        Statement::Assign(_, expr) => {
            let mut ids = HashSet::new();
            collect_field_ids_from_expr(expr, &mut ids, field_index_map);
            result.extend(ids.into_iter());
        }
        _ => {}
    }
    result
}

/// Check if a body is parallel-safe (currently always returns true).
pub fn is_body_parallel_safe(_body: &[Statement]) -> bool {
    true
}

/// Collect expression field references into a set (no field_index_map).
pub fn collect_expr_field_refs_for_set(e: &Expr, refs: &mut HashSet<String>) {
    match e {
        Expr::Identifier(name) => {
            refs.insert(name.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            collect_expr_field_refs_for_set(l, refs);
            collect_expr_field_refs_for_set(r, refs);
        }
        Expr::UnaryOp(_, inner) => {
            collect_expr_field_refs_for_set(inner, refs);
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_expr_field_refs_for_set(arg, refs);
            }
        }
        _ => {}
    }
}

/// Collect field references from a statement.
pub fn collect_field_refs(
    stmt: &Statement,
    fields: &mut HashSet<String>,
    field_index_map: &HashMap<String, usize>,
) {
    match stmt {
        Statement::Assign(_, expr) => {
            collect_expr_field_refs(expr, fields, field_index_map);
        }
        Statement::Let { expr: Some(e), .. } => {
            collect_expr_field_refs(e, fields, field_index_map);
        }
        Statement::Guarded(cond, stmts) => {
            collect_expr_field_refs(cond, fields, field_index_map);
            for s in stmts {
                collect_field_refs(s, fields, field_index_map);
            }
        }
        Statement::Block(stmts) => {
            for s in stmts {
                collect_field_refs(s, fields, field_index_map);
            }
        }
        Statement::Expression(e) => {
            collect_expr_field_refs(e, fields, field_index_map);
        }
        Statement::Foreach { list, body, .. } => {
            collect_expr_field_refs(list, fields, field_index_map);
            for s in body {
                collect_field_refs(s, fields, field_index_map);
            }
        }
        _ => {}
    }
}

/// Collect field references from an expression.
pub fn collect_expr_field_refs(
    e: &Expr,
    fields: &mut HashSet<String>,
    field_index_map: &HashMap<String, usize>,
) {
    match e {
        Expr::Identifier(name) => {
            if field_index_map.contains_key(name) {
                fields.insert(name.clone());
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_expr_field_refs(l, fields, field_index_map);
            collect_expr_field_refs(r, fields, field_index_map);
        }
        Expr::UnaryOp(_, inner) => {
            collect_expr_field_refs(inner, fields, field_index_map);
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_expr_field_refs(arg, fields, field_index_map);
            }
        }
        Expr::Field(obj, _) => {
            collect_expr_field_refs(obj, fields, field_index_map);
        }
        Expr::Cast(inner, _) => {
            collect_expr_field_refs(inner, fields, field_index_map);
        }
        Expr::Index(obj, idx) => {
            collect_expr_field_refs(obj, fields, field_index_map);
            collect_expr_field_refs(idx, fields, field_index_map);
        }
        _ => {}
    }
}

/// Collect all identifiers from an expression into a set.
pub fn collect_all_idents(e: &Expr, idents: &mut HashSet<String>) {
    match e {
        Expr::Identifier(name) => {
            idents.insert(name.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            collect_all_idents(l, idents);
            collect_all_idents(r, idents);
        }
        Expr::UnaryOp(_, inner) => {
            collect_all_idents(inner, idents);
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_all_idents(arg, idents);
            }
        }
        Expr::Field(obj, _) => {
            collect_all_idents(obj, idents);
        }
        Expr::Index(obj, idx) => {
            collect_all_idents(obj, idents);
            collect_all_idents(idx, idents);
        }
        Expr::Cast(inner, _) => {
            collect_all_idents(inner, idents);
        }
        _ => {}
    }
}

/// Build a map from let-bound variable to the set of state fields it reads.
pub fn build_let_field_refs(
    body: &[Statement],
    field_index_map: &HashMap<String, usize>,
) -> HashMap<String, HashSet<String>> {
    let mut result = HashMap::new();
    for stmt in body {
        if let Statement::Let { name, expr: Some(e), .. } = stmt {
            let mut fields = HashSet::new();
            collect_expr_field_refs(e, &mut fields, field_index_map);
            result.insert(name.clone(), fields);
        }
    }
    result
}

/// Check if an expression is an output-related FFI call.
pub fn is_output_call(expr: &Expr) -> bool {
    match expr {
        // 2026-09-08 (anti-pattern audit): Print#/Println# are the ONLY
        // canonical output intrinsics — print! / println! lower to them via
        // the print plugin. The raw __print_* frgn names were a second
        // hardcoded copy; benchmark call sites migrated to println!().
        Expr::Call(name, _, _) if name == "Print#" || name == "Println#" => true,
        _ => false,
    }
}

/// Returns the set of observable (printed/externally visible) field refs.
pub fn observable_field_refs(
    stmt: &Statement,
    field_index_map: &HashMap<String, usize>,
) -> HashSet<String> {
    let mut result = HashSet::new();
    match stmt {
        Statement::EndProgram(Some(e)) | Statement::Term(Some(e)) => {
            collect_expr_field_refs(e, &mut result, field_index_map);
        }
        // 2026-07-19: Detect output calls in Expression statements.
        // These come from !Print/!PrintLn which resolve to __print_*
        // frgn calls. The optimizer must not eliminate these.
        Statement::Expression(e) => {
            if contains_output_call(e) {
                collect_expr_field_refs(e, &mut result, field_index_map);
            }
        }
        _ => {}
    }
    result
}

/// Check if an expression contains a Print#/Println# call (observable output).
fn contains_output_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(name, _, _) => name == "Print#" || name == "Println#",
        Expr::Block(stmts) => stmts.iter().any(|s| matches!(s, Statement::Expression(e) if contains_output_call(e))),
        _ => false,
    }
}

/// Extract the target field name from an assignment LHS expression.
pub fn target_field_name(lhs: &Expr) -> Option<String> {
    match lhs {
        Expr::Identifier(n) => Some(n.clone()),
        _ => None,
    }
}

/// Seed observable identifiers from Term/TermBang statements.
pub fn seed_observable_idents(
    stmt: &Statement,
    let_fields: &HashMap<String, HashSet<String>>,
    field_index_map: &HashMap<String, usize>,
    live: &mut HashSet<String>,
) {
    let obs = observable_field_refs(stmt, field_index_map);
    for f in obs {
        live.insert(f.clone());
    }
    match stmt {
        Statement::EndProgram(Some(e)) | Statement::Term(Some(e)) => {
            seed_from_expr(e, let_fields, live);
        }
        // 2026-07-19: Output calls in Expression statements (from !Print/!PrintLn)
        // also mark their referenced fields as live.
        Statement::Expression(e) if contains_output_call(e) => {
            seed_from_expr(e, let_fields, live);
        }
        _ => {}
    }
}

/// Recursively expand observable seeds through let-bound expressions.
fn seed_from_expr(
    e: &Expr,
    let_fields: &HashMap<String, HashSet<String>>,
    live: &mut HashSet<String>,
) {
    match e {
        Expr::Identifier(name) => {
            if let Some(fields) = let_fields.get(name) {
                for f in fields {
                    live.insert(f.clone());
                }
            }
        }
        Expr::BinaryOp(_, l, r) => {
            seed_from_expr(l, let_fields, live);
            seed_from_expr(r, let_fields, live);
        }
        Expr::UnaryOp(_, inner) => {
            seed_from_expr(inner, let_fields, live);
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                seed_from_expr(arg, let_fields, live);
            }
        }
        // 2026-07-19: Handle Block expressions from !PrintLn!() resolution.
        Expr::Block(stmts) => {
            for stmt in stmts {
                if let crate::ast::Statement::Expression(e) = stmt {
                    seed_from_expr(e, let_fields, live);
                }
            }
        }
        _ => {}
    }
}

/// Trace which fields are live (observable) through a block of statements.
/// Starting from observable seeds, traces backwards through assignments
/// to find all transitively-live fields.
pub fn trace_live_fields(
    body: &[Statement],
    field_index_map: &HashMap<String, usize>,
) -> HashSet<String> {
    let let_fields = build_let_field_refs(body, field_index_map);
    // Forward pass: seed observable idents
    let mut live = HashSet::new();
    for stmt in body {
        seed_observable_idents(stmt, &let_fields, field_index_map, &mut live);
    }
    // Backward pass: trace field writes that produce observable values
    for stmt in body.iter().rev() {
        match stmt {
            Statement::Assign(lhs, expr) => {
                let lhs_name = target_field_name(lhs);
                if let Some(ref n) = lhs_name {
                    if live.contains(n) {
                        collect_expr_field_refs(expr, &mut live, field_index_map);
                    }
                }
            }
            Statement::Guarded(cond, stmts) => {
                collect_expr_field_refs(cond, &mut live, field_index_map);
                let sub_live = trace_live_fields(stmts, field_index_map);
                for f in sub_live {
                    live.insert(f);
                }
            }
            _ => {}
        }
    }
    live
}

/// Filter out assignments to dead (non-observable) fields.
pub fn filter_dead_assignments(
    body: &[Statement],
    live_fields: &HashSet<String>,
) -> Vec<Statement> {
    let mut result = Vec::new();
    for stmt in body {
        match stmt {
            Statement::Assign(lhs, _) if target_field_name(lhs)
                .map_or(false, |n| !live_fields.contains(&n)) => {
                // Skip dead assignment
            }
            Statement::Guarded(cond, stmts) => {
                let filtered = filter_dead_assignments(stmts, live_fields);
                if !filtered.is_empty() {
                    result.push(Statement::Guarded(cond.clone(), filtered));
                }
            }
            _ => {
                result.push(stmt.clone());
            }
        }
    }
    result
}

/// Find permutation cycles in a field index permutation.
pub fn find_permutation_cycles(
    perm: &HashMap<usize, usize>,
    n: usize,
) -> Vec<Vec<usize>> {
    let mut visited = vec![false; n];
    let mut cycles = Vec::new();
    for start in 0..n {
        if visited[start] || !perm.contains_key(&start) {
            continue;
        }
        let mut cycle = Vec::new();
        let mut cur = start;
        while !visited[cur] {
            visited[cur] = true;
            cycle.push(cur);
            if let Some(&next) = perm.get(&cur) {
                cur = next;
            } else {
                break;
            }
        }
        if cycle.len() > 1 {
            cycles.push(cycle);
        }
    }
    cycles
}

/// Find the optimal step for a cycle of a given length.
pub fn optimal_step_for_cycle_length(len: usize) -> usize {
    if len <= 1 {
        return 1;
    }
    // Use half the cycle length (or midpoint)
    let half = len / 2;
    // Find a divisor of len close to half for clean rotation
    let mut best = 1;
    for d in 1..=half {
        if len % d == 0 {
            best = d;
        }
    }
    best.max(1)
}

/// GCD of two numbers.
pub fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Detect field rotation pattern in a block of statements.
pub fn detect_rotation_ast(
    body: &[Statement],
    field_index_map: &HashMap<String, usize>,
) -> (usize, Vec<String>) {
    let mut perm: HashMap<usize, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for stmt in body {
        let (lhs, rhs) = match stmt {
            Statement::Assign(l, r) => (l, r),
            _ => continue,
        };
        let src = match rhs {
            Expr::Identifier(n) => n.clone(),
            _ => continue,
        };
        let dst = match lhs {
            Expr::Identifier(n) => n.clone(),
            _ => continue,
        };
        let src_idx = match field_index_map.get(&src) {
            Some(i) => *i,
            None => continue,
        };
        let dst_idx = match field_index_map.get(&dst) {
            Some(i) => *i,
            None => continue,
        };
        if src_idx != dst_idx {
            perm.insert(dst_idx, src_idx);
        }
        if !order.contains(&dst) {
            order.push(dst);
        }
    }
    let cycles = find_permutation_cycles(&perm, field_index_map.len());
    let step = if cycles.is_empty() {
        1
    } else {
        let max_cycle_len = cycles.iter().map(|c| c.len()).max().unwrap_or(1);
        optimal_step_for_cycle_length(max_cycle_len)
    };
    (step, order)
}

/// Build vector phi groups from field index map.
/// Groups fields by their type for vectorized emission.
pub fn build_vector_phi_groups(
    field_index_map: &HashMap<String, usize>,
    field_types: &[String],
) -> HashMap<String, Vec<String>> {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    let mut sorted: Vec<(String, usize)> = field_index_map.iter()
        .map(|(k, v)| (k.clone(), *v)).collect();
    sorted.sort_by_key(|(_, v)| *v);
    for (name, idx) in &sorted {
        if idx >= &field_types.len() {
            continue;
        }
        let ft = &field_types[*idx];
        groups.entry(ft.clone()).or_default().push(name.clone());
    }
    groups
}
