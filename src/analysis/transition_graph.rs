use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, Transaction, Type, UnaryOpKind};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum ConvergeDirection {
    Increasing,
    Decreasing,
}

#[derive(Debug, Clone)]
pub struct BoundedPre {
    pub var: String,
    pub bound_var: String,
    pub direction: ConvergeDirection,
    pub bound_literal: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct IncrementInfo {
    pub var: String,
    pub delta: i64,
}

#[derive(Debug, Clone)]
pub struct ReactorNode {
    pub name: String,
    pub is_reactive: bool,
    pub precondition: Expr,
    pub body: Vec<Statement>,
    pub bounded_pre: Option<BoundedPre>,
    pub increments: Option<IncrementInfo>,
    pub is_pure_body: bool,
    pub write_set: HashSet<String>,
    pub is_effectively_pure: bool,
    pub lexicographic_vars: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ReactorTransitionGraph {
    pub nodes: Vec<ReactorNode>,
    pub has_triggers: bool,
    pub live_fields: HashSet<String>,
    /// 2026-07-31: Reactive txn names whose RAW body has an unguarded FFI
    /// statement (a top-level statement that is not `Statement::Guarded` and
    /// contains an FFI call). Computed once so the LLVM backend's reactor
    /// tick attribute selection (`#2` vs `#12`, dispatch.rs:68-73/357-362)
    /// no longer re-walks bodies. `Statement::Guarded` bodies are treated as
    /// guarded (their FFI is outlined into cold functions, so the hot path
    /// can stay `memory(argmem: readwrite)`).
    pub has_unguarded_ffi: HashSet<String>,
}

impl ReactorTransitionGraph {
    pub fn build(items: &[TopLevel], exit_condition: &Option<Box<Expr>>, out_pragmas: &[String]) -> Self {
        let mut nodes = Vec::new();
        let mut has_triggers = false;

        // 2026-08-09 (Bug 2): iterate the EFFECTIVE transactions (direct +
        // sync<group>-wrapped + export-wrapped) — a sync-group node is a real
        // reactive node whose fields must be live and whose bounded-pre must
        // reach the fold analysis. Skipping them left live_fields empty for
        // their state (dead-field elimination → undefined @field globals) and
        // the reactor dispatch empty (nothing fired).
        for txn in crate::analysis::effective_txns(items) {
            let body_no_term: Vec<Statement> = {
                        let mut filtered: Vec<&Statement> = txn.body.iter()
                            .filter(|s| !matches!(s, Statement::Term(None) | Statement::EndProgram(None)))
                            .collect();
                        while filtered.last().map_or(false, |s| {
                            if let Statement::Guarded(_, statements) = s {
                                statements.iter().any(|s| matches!(s, Statement::EndProgram(None)))
                            } else { false }
                        }) {
                            filtered.pop();
                        }
                        filtered.into_iter().cloned().collect()
                    };
                    let simplified_body = simplify_body(&body_no_term);
                    let increments = detect_increments(&simplified_body)
                        .or_else(|| detect_popcount_decay(&simplified_body));
                    // 2026-07-17: detect_increments returns the FIRST match,
                    // which may not be the bounded_pre's var (e.g. tail before ops).
                    // If the first increment doesn't match, collect ALL increments
                    // and try each one, updating increments to the matching one.
                    let (bounded_pre, increments) = {
                        let bp = extract_valid_bounded_pre(&txn.contract.pre_condition, &increments);
                        if let Some(ref bp_val) = bp {
                            (Some(bp_val.clone()), increments)
                        } else {
                            let all_incs = detect_all_increments(&simplified_body);
                            let matched = all_incs.iter().find_map(|inc| {
                                let bp = extract_valid_bounded_pre(&txn.contract.pre_condition, &Some(inc.clone()));
                                bp.map(|b| (b, inc.clone()))
                            });
                            match matched {
                                Some((bp_val, inc_val)) => (Some(bp_val), Some(inc_val)),
                                None => (None, increments),
                            }
                        }
                    };
                    // 2026-07-17: Collect state field names from both StateDecl
                    // (hand-constructed ASTs in tests) and Statement::Let (parser
                    // output for top-level `let name: Type = expr;`).
                    let state_field_names: HashSet<String> = items
                        .iter()
                        .filter_map(|i| {
                            match i {
                                TopLevel::StateDecl(s) => Some(s.name.clone()),
                                TopLevel::Statement(stmt) => {
                                    if let Statement::Let { name, .. } = stmt.as_ref() {
                                        Some(name.clone())
                                    } else { None }
                                }
                                _ => None,
                            }
                        })
                        .collect();
                    let is_pure = is_pure_body(&simplified_body, &state_field_names, &increments);
                    let write_set = extract_write_set(&simplified_body, &state_field_names);
                    let lexicographic_vars = detect_lexicographic_ranking(&txn.contract.pre_condition, &simplified_body);

                    nodes.push(ReactorNode {
                        name: txn.name.clone(),
                        is_reactive: txn.is_reactive,
                        precondition: txn.contract.pre_condition.clone(),
                        body: simplified_body,
                        bounded_pre,
                        increments,
                        is_pure_body: is_pure,
                        write_set,
                        is_effectively_pure: false,
                        lexicographic_vars,
                    });
        }

        // Trigger scan (kept separate — effective_txns yields only txns).
        for item in items {
            if let TopLevel::Trigger(_) = item {
                has_triggers = true;
            }
        }

        let live_fields = compute_live_fields(exit_condition, out_pragmas, &nodes);
        // 2026-08-04 (out-observability plan): `out let`/`vol let` top-level
        // state fields are observability pins — their writes are liveness
        // roots. Force them live so dead-field elimination cannot drop the
        // field (a field written only inside an observable context must keep
        // its slot even when nothing reads it).
        let mut live_fields = live_fields;
        for item in items {
            if let TopLevel::Statement(stmt) = item {
                if let Statement::Let { name, modifiers, .. } = stmt.as_ref() {
                    if modifiers.iter().any(|m| m.name == "out" || m.name == "vol") {
                        live_fields.insert(name.clone());
                    }
                }
            }
        }
        for node in &mut nodes {
            compute_effectively_pure(node, &live_fields);
        }
        // 2026-07-31: Phase 2 (§7.4) — scan RAW txn bodies (the backend's
        // dispatch.rs scans `t.body`, not the simplified body). A statement
        // is "unguarded" if it is not `Statement::Guarded`; Guarded bodies
        // are outlined to cold functions and do not force memory(readwrite).
        let mut has_unguarded_ffi: HashSet<String> = HashSet::new();
        for item in items {
            if let TopLevel::Transaction(t) = item {
                if t.is_reactive
                    && t.body.iter().any(|stmt| {
                        !matches!(stmt, Statement::Guarded(_, _)) && statement_contains_ffi(stmt)
                    })
                {
                    has_unguarded_ffi.insert(t.name.clone());
                }
            }
        }

        ReactorTransitionGraph {
            nodes,
            has_triggers,
            live_fields,
            has_unguarded_ffi,
        }
    }
}

fn extract_bounded_pre(pre: &Expr) -> Option<BoundedPre> {
    match pre {
        Expr::BinaryOp(BinaryOpKind::Lt, l, r) | Expr::BinaryOp(BinaryOpKind::Le, l, r) => {
            match (l.as_ref(), r.as_ref()) {
                (Expr::Identifier(var), Expr::Identifier(bound)) => Some(BoundedPre {
                    var: var.clone(),
                    bound_var: bound.clone(),
                    direction: ConvergeDirection::Increasing,
                    bound_literal: None,
                }),
                (Expr::Identifier(var), Expr::Decimal(n)) => Some(BoundedPre {
                    var: var.clone(),
                    bound_var: format!("__lit__{}", var),
                    direction: ConvergeDirection::Increasing,
                    bound_literal: Some(*n),
                }),
                (Expr::Identifier(var), Expr::UnaryOp(UnaryOpKind::Neg, bn)) if matches!(bn.as_ref(), Expr::Decimal(_)) => {
                    let n = match bn.as_ref() { Expr::Decimal(n) => -n, _ => 0 };
                    Some(BoundedPre {
                        var: var.clone(),
                        bound_var: format!("__lit__{}", var),
                        direction: ConvergeDirection::Increasing,
                        bound_literal: Some(n),
                    })
                }
                // len(list) < N — list drains toward full
                (Expr::Field(list, target), _) if target == "Size" => {
                    if let Some(name) = expr_name(list) {
                        Some(BoundedPre {
                            var: name,
                            bound_var: format!("__len_bound"),
                            direction: ConvergeDirection::Increasing,
                            bound_literal: None,
                        })
                    } else { None }
                }
                _ => None,
            }
        }
        Expr::BinaryOp(BinaryOpKind::Gt, l, r) | Expr::BinaryOp(BinaryOpKind::Ge, l, r) => {
            match (l.as_ref(), r.as_ref()) {
                (Expr::Identifier(var), Expr::Identifier(bound)) => Some(BoundedPre {
                    var: var.clone(),
                    bound_var: bound.clone(),
                    direction: ConvergeDirection::Decreasing,
                    bound_literal: None,
                }),
                (Expr::Identifier(var), Expr::Decimal(n)) => Some(BoundedPre {
                    var: var.clone(),
                    bound_var: format!("__lit__{}", var),
                    direction: ConvergeDirection::Decreasing,
                    bound_literal: Some(*n),
                }),
                (Expr::Identifier(var), Expr::UnaryOp(UnaryOpKind::Neg, bn)) if matches!(bn.as_ref(), Expr::Decimal(_)) => {
                    let n = match bn.as_ref() { Expr::Decimal(n) => -n, _ => 0 };
                    Some(BoundedPre {
                        var: var.clone(),
                        bound_var: format!("__lit__{}", var),
                        direction: ConvergeDirection::Decreasing,
                        bound_literal: Some(n),
                    })
                }
                // len(list) > 0 — list drains to empty (bound=0)
                (Expr::Field(list, target), Expr::Decimal(0)) if target == "Size" => {
                    if let Some(name) = expr_name(list) {
                        Some(BoundedPre {
                            var: name.clone(),
                            bound_var: format!("__lit__{}", name),
                            direction: ConvergeDirection::Decreasing,
                            bound_literal: Some(0),
                        })
                    } else { None }
                }
                // len(list) > N — list drains to N
                (Expr::Field(list, target), Expr::Decimal(n)) if target == "Size" && *n > 0 => {
                    if let Some(name) = expr_name(list) {
                        Some(BoundedPre {
                            var: name.clone(),
                            bound_var: format!("__lit__{}", name),
                            direction: ConvergeDirection::Decreasing,
                            bound_literal: Some(*n),
                        })
                    } else { None }
                }
                _ => None,
            }
        }
        // reg != N — treat as reg > N (decreasing toward N) or reg < N
        Expr::BinaryOp(BinaryOpKind::Neq, l, r) => {
            match (l.as_ref(), r.as_ref()) {
                (Expr::Identifier(var), Expr::Decimal(n)) => Some(BoundedPre {
                    var: var.clone(),
                    bound_var: format!("__lit__{}", var),
                    direction: ConvergeDirection::Decreasing,
                    bound_literal: Some(*n),
                }),
                _ => None,
            }
        }
        Expr::BinaryOp(BinaryOpKind::And, l, r) => {
            // 2026-07-17: Try both sides and prefer variable-bound comparison
            // over literal-bound. For `bound > 0 && ops < bound`, the left
            // `bound > 0` matches first (Gt + Decimal), but `ops < bound`
            // (Lt + Identifier) is the real counter loop condition. The
            // literal-guard side has bound_literal = Some(n); the real
            // counter loop has bound_literal = None. Prefer the latter so
            // extract_valid_bounded_pre sees the correct var/inc match.
            match (extract_bounded_pre(l), extract_bounded_pre(r)) {
                (Some(lp), Some(rp)) => {
                    if lp.bound_literal.is_some() { Some(rp) } else { Some(lp) }
                }
                (Some(lp), None) => Some(lp),
                (None, Some(rp)) => Some(rp),
                (None, None) => None,
            }
        }
        _ => None,
    }
}

fn extract_valid_bounded_pre(pre: &Expr, inc: &Option<IncrementInfo>) -> Option<BoundedPre> {
    let bp = extract_bounded_pre(pre)?;
    let is_mutated = inc.as_ref().map_or(false, |i| i.var == bp.var);
    if is_mutated { Some(bp) } else { None }
}

fn simplify_expr(expr: &Expr) -> Expr {
    let expr = match expr {
        Expr::BinaryOp(BinaryOpKind::Add, a, b) => Expr::BinaryOp(BinaryOpKind::Add,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Sub, a, b) => Expr::BinaryOp(BinaryOpKind::Sub,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Mul, a, b) => Expr::BinaryOp(BinaryOpKind::Mul,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Div, a, b) => Expr::BinaryOp(BinaryOpKind::Div,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Mod, a, b) => Expr::BinaryOp(BinaryOpKind::Mod,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Eq, a, b) => Expr::BinaryOp(BinaryOpKind::Eq,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Neq, a, b) => Expr::BinaryOp(BinaryOpKind::Neq,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Lt, a, b) => Expr::BinaryOp(BinaryOpKind::Lt,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Le, a, b) => Expr::BinaryOp(BinaryOpKind::Le,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Gt, a, b) => Expr::BinaryOp(BinaryOpKind::Gt,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Ge, a, b) => Expr::BinaryOp(BinaryOpKind::Ge,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::And, a, b) => Expr::BinaryOp(BinaryOpKind::And,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Or, a, b) => Expr::BinaryOp(BinaryOpKind::Or,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::BitAnd, a, b) => Expr::BinaryOp(BinaryOpKind::BitAnd,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::BitOr, a, b) => Expr::BinaryOp(BinaryOpKind::BitOr,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::BitXor, a, b) => Expr::BinaryOp(BinaryOpKind::BitXor,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Shl, a, b) => Expr::BinaryOp(BinaryOpKind::Shl,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Shr, a, b) => Expr::BinaryOp(BinaryOpKind::Shr,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::BinaryOp(BinaryOpKind::Concat, a, b) => Expr::BinaryOp(BinaryOpKind::Concat,
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::UnaryOp(UnaryOpKind::Not, a) => Expr::UnaryOp(UnaryOpKind::Not, Box::new(simplify_expr(a))),
        Expr::UnaryOp(UnaryOpKind::Neg, a) => Expr::UnaryOp(UnaryOpKind::Neg, Box::new(simplify_expr(a))),
        Expr::UnaryOp(UnaryOpKind::BitNot, a) => Expr::UnaryOp(UnaryOpKind::BitNot, Box::new(simplify_expr(a))),
        Expr::Cast(a, t) => Expr::Cast(Box::new(simplify_expr(a)), t.clone()),
        Expr::Index(a, b) => Expr::Index(
            Box::new(simplify_expr(a)),
            Box::new(simplify_expr(b)),
        ),
        Expr::Field(obj, f) => Expr::Field(
            Box::new(simplify_expr(obj)),
            f.clone(),
        ),
        Expr::List(elems) => Expr::List(
            elems.iter().map(|e| simplify_expr(e)).collect(),
        ),
        Expr::Tuple(elems) => Expr::Tuple(
            elems.iter().map(|e| simplify_expr(e)).collect(),
        ),
        Expr::Block(stmts) => Expr::Block(
            stmts.iter().map(|s| simplify_stmt(s)).collect(),
        ),
        other => other.clone(),
    };

    // Now apply algebraic rules to the simplified children.
    match &expr {
        Expr::BinaryOp(BinaryOpKind::Sub, sub_lhs, sub_rhs) => {
            // R1: (a + b) - a → b  and  (a + b) - b → a
            if let Expr::BinaryOp(BinaryOpKind::Add, add_lhs, add_rhs) = sub_lhs.as_ref() {
                if vars_match(add_lhs.as_ref(), sub_rhs.as_ref()) {
                    return add_rhs.as_ref().clone();
                }
                if vars_match(add_rhs.as_ref(), sub_rhs.as_ref()) {
                    return add_lhs.as_ref().clone();
                }
            }
            // R2: a - (a - b) → b
            if let Expr::BinaryOp(BinaryOpKind::Sub, inner_a, inner_b) = sub_rhs.as_ref() {
                if vars_match(sub_lhs.as_ref(), inner_a.as_ref()) {
                    return inner_b.as_ref().clone();
                }
            }
            // R3: (a + b) - (a + c) → b - c
            if let (Expr::BinaryOp(BinaryOpKind::Add, a1, b1), Expr::BinaryOp(BinaryOpKind::Add, a2, b2)) = (sub_lhs.as_ref(), sub_rhs.as_ref()) {
                if vars_match(a1.as_ref(), a2.as_ref()) && !vars_match(b1.as_ref(), b2.as_ref()) {
                    return Expr::BinaryOp(BinaryOpKind::Sub,
                        Box::new(b1.as_ref().clone()),
                        Box::new(b2.as_ref().clone()),
                    );
                }
            }
            // R5: (a - b) - (c - b) → a - c
            if let (Expr::BinaryOp(BinaryOpKind::Sub, sa, sb1), Expr::BinaryOp(BinaryOpKind::Sub, sc, sb2)) = (sub_lhs.as_ref(), sub_rhs.as_ref()) {
                if vars_match(sb1.as_ref(), sb2.as_ref()) {
                    return Expr::BinaryOp(BinaryOpKind::Sub,
                        Box::new(sa.as_ref().clone()),
                        Box::new(sc.as_ref().clone()),
                    );
                }
            }
            // R7: a - 0 → a
            if let Expr::Decimal(0) = sub_rhs.as_ref() {
                return sub_lhs.as_ref().clone();
            }
            expr
        }

        Expr::BinaryOp(BinaryOpKind::Add, add_lhs, add_rhs) => {
            // R4: (a - b) + b → a
            if let Expr::BinaryOp(BinaryOpKind::Sub, sub_a, sub_b) = add_lhs.as_ref() {
                if vars_match(sub_b.as_ref(), add_rhs.as_ref()) {
                    return sub_a.as_ref().clone();
                }
            }
            // R6: a + 0 → a
            if let Expr::Decimal(0) = add_rhs.as_ref() {
                return add_lhs.as_ref().clone();
            }
            // R6b: 0 + a → a
            if let Expr::Decimal(0) = add_lhs.as_ref() {
                return add_rhs.as_ref().clone();
            }
            expr
        }

        Expr::BinaryOp(BinaryOpKind::Mul, mul_lhs, mul_rhs) => {
            // R8: a * 1 → a
            if let Expr::Decimal(1) = mul_rhs.as_ref() {
                return mul_lhs.as_ref().clone();
            }
            // R8b: 1 * a → a
            if let Expr::Decimal(1) = mul_lhs.as_ref() {
                return mul_rhs.as_ref().clone();
            }
            // R10: a * 0 → 0
            if let Expr::Decimal(0) = mul_rhs.as_ref() {
                return Expr::Decimal(0);
            }
            // R10b: 0 * a → 0
            if let Expr::Decimal(0) = mul_lhs.as_ref() {
                return Expr::Decimal(0);
            }
            expr
        }

        Expr::BinaryOp(BinaryOpKind::Div, div_lhs, div_rhs) => {
            // R9: a / 1 → a
            if let Expr::Decimal(1) = div_rhs.as_ref() {
                return div_lhs.as_ref().clone();
            }
            expr
        }

        _ => expr,
    }
}

fn vars_match(a: &Expr, b: &Expr) -> bool {
    matches!((a, b), (Expr::Identifier(an), Expr::Identifier(bn)) if an == bn)
}

fn simplify_stmt(stmt: &Statement) -> Statement {
    match stmt {
        Statement::Assign(lhs, expr) => Statement::Assign(lhs.clone(), simplify_expr(expr)),
        Statement::Let { name, expr, .. } => Statement::Let { names: vec![],  
            name: name.clone(),
            ty: None,
            expr: expr.as_ref().map(|e| simplify_expr(e)),
            modifiers: vec![],
        },
        Statement::Expression(e) => Statement::Expression(simplify_expr(e)),
        Statement::Guarded(condition, statements) => Statement::Guarded(
            simplify_expr(condition),
            statements.iter().map(|s| simplify_stmt(s)).collect(),
        ),
        other => other.clone(),
    }
}

pub fn simplify_body(body: &[Statement]) -> Vec<Statement> {
    let mut current = body.to_vec();
    for _ in 0..5 {
        let next: Vec<Statement> = current.iter().map(|s| simplify_stmt(s)).collect();
        // Statement does not derive PartialEq; compare via Debug formatting
        if format!("{:?}", next) == format!("{:?}", current) {
            break;
        }
        current = next;
    }
    current
}

fn get_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Decimal(n) => Some(*n),
        _ => None,
    }
}

fn detect_increments(body: &[Statement]) -> Option<IncrementInfo> {
    for stmt in body {
        if let Statement::Assign(lhs, expr) = stmt {
            let Some(name) = lhs.as_var_name() else {
                continue;
            };
            if let Expr::BinaryOp(BinaryOpKind::Add, a, b) = expr {
                if let (Expr::Identifier(var), Some(delta)) = (a.as_ref(), get_int(b)) {
                    if *var == name && delta > 0 {
                        return Some(IncrementInfo { var: name.to_string(), delta });
                    }
                }
                if let (Expr::Identifier(var), Some(delta)) = (b.as_ref(), get_int(a)) {
                    if *var == name && delta > 0 {
                        return Some(IncrementInfo { var: name.to_string(), delta });
                    }
                }
            }
            // Decreasing counter: count = count - delta or count = count - 1
            if let Expr::BinaryOp(BinaryOpKind::Sub, a, b) = expr {
                if let (Expr::Identifier(var), Some(delta)) = (a.as_ref(), get_int(b)) {
                    if *var == name && delta > 0 {
                        return Some(IncrementInfo { var: name.to_string(), delta });
                    }
                }
            }
            // Interval bounds: (x + R1) - R2 where net step R1 - R2 ≥ 1
            if let Expr::BinaryOp(BinaryOpKind::Sub, inner, rhs) = expr {
                if let Expr::BinaryOp(BinaryOpKind::Add, lhs, rhs2) = inner.as_ref() {
                    let is_self_add = matches!(lhs.as_ref(), Expr::Identifier(v) if *v == name);
                    if is_self_add {
                        let r1 = get_int(rhs2);
                        let r2 = get_int(rhs);
                        if let (Some(r1_val), Some(r2_val)) = (r1, r2) {
                            let net = r1_val - r2_val;
                            if net > 0 {
                                return Some(IncrementInfo { var: name.to_string(), delta: net });
                            }
                        }
                        if r1.map_or(false, |v| v >= 1) && r2.is_none() {
                            return Some(IncrementInfo { var: name.to_string(), delta: 1 });
                        }
                    }
                }
            }
        }
    }
    None
}

fn detect_popcount_decay(body: &[Statement]) -> Option<IncrementInfo> {
    for stmt in body {
        if let Statement::Assign(lhs, expr) = stmt {
            let name = match lhs {
                Expr::Identifier(n) => n.clone(),
                _ => continue,
            };
            // reg & (reg - 1)
            if let Expr::BinaryOp(BinaryOpKind::BitAnd, a, b) = expr {
                let a_is_self = matches!(a.as_ref(), Expr::Identifier(v) if *v == name);
                let b_is_self_minus = if let Expr::BinaryOp(BinaryOpKind::Sub, inner, val) = b.as_ref() {
                    matches!(inner.as_ref(), Expr::Identifier(v) if *v == name)
                        && matches!(val.as_ref(), Expr::Decimal(1))
                } else {
                    false
                };
                if a_is_self && b_is_self_minus {
                    return Some(IncrementInfo { var: name.clone(), delta: 1 });
                }
                // (reg - 1) & reg
                let b_is_self = matches!(b.as_ref(), Expr::Identifier(v) if *v == name);
                let a_is_self_minus = if let Expr::BinaryOp(BinaryOpKind::Sub, inner, val) = a.as_ref() {
                    matches!(inner.as_ref(), Expr::Identifier(v) if *v == name)
                        && matches!(val.as_ref(), Expr::Decimal(1))
                } else {
                    false
                };
                if b_is_self && a_is_self_minus {
                    return Some(IncrementInfo { var: name.clone(), delta: 1 });
                }
            }
        }
    }
    None
}

/// Collect ALL increment patterns from a body. Unlike detect_increments which
/// returns the first match, this returns every variable that is self-incremented.
/// 2026-07-17: Fixes ring_buffer where tail = tail+1 (first match) shadows
/// ops = ops+1 (the actual bounded counter).
fn detect_all_increments(body: &[Statement]) -> Vec<IncrementInfo> {
    let mut results = Vec::new();
    for stmt in body {
        if let Statement::Assign(lhs, expr) = stmt {
            let Some(name) = lhs.as_var_name() else { continue; };
            if let Expr::BinaryOp(BinaryOpKind::Add, a, b) = expr {
                if let (Expr::Identifier(var), Some(delta)) = (a.as_ref(), get_int(b)) {
                    if *var == name && delta > 0 {
                        results.push(IncrementInfo { var: name.to_string(), delta });
                        continue;
                    }
                }
                if let (Expr::Identifier(var), Some(delta)) = (b.as_ref(), get_int(a)) {
                    if *var == name && delta > 0 {
                        results.push(IncrementInfo { var: name.to_string(), delta });
                        continue;
                    }
                }
            }
            if let Expr::BinaryOp(BinaryOpKind::Sub, a, b) = expr {
                if let (Expr::Identifier(var), Some(delta)) = (a.as_ref(), get_int(b)) {
                    if *var == name && delta > 0 {
                        results.push(IncrementInfo { var: name.to_string(), delta });
                        continue;
                    }
                }
            }
        }
    }
    results
}

fn detect_lexicographic_ranking(pre: &Expr, body: &[Statement]) -> Vec<String> {
    fn collect_or_vars(expr: &Expr, out: &mut Vec<String>) {
        match expr {
            Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
                collect_or_vars(l, out);
                collect_or_vars(r, out);
            }
            _ => {
                if let Expr::BinaryOp(BinaryOpKind::Gt, inner, val) = expr {
                    if let (Expr::Identifier(var), Expr::Decimal(n)) = (inner.as_ref(), val.as_ref()) {
                        if *n >= 0 && !out.contains(var) {
                            out.push(var.clone());
                        }
                    }
                }
                if let Expr::BinaryOp(BinaryOpKind::Ge, inner, val) = expr {
                    if let (Expr::Identifier(var), Expr::Decimal(n)) = (inner.as_ref(), val.as_ref()) {
                        if *n > 0 && !out.contains(var) {
                            out.push(var.clone());
                        }
                    }
                }
            }
        }
    }

    let mut vars = Vec::new();
    collect_or_vars(pre, &mut vars);
    if vars.len() < 2 {
        return Vec::new();
    }

    let decremented: HashSet<String> = body.iter().filter_map(|stmt| {
        if let Statement::Assign(lhs, expr) = stmt {
            let name = match lhs {
                Expr::Identifier(n) => Some(n.clone()),
                _ => None,
            }?;
            let is_decrement = if let Expr::BinaryOp(BinaryOpKind::Sub, a, d) = expr {
                            matches!(a.as_ref(), Expr::Identifier(v) if *v == name)
                                && matches!(d.as_ref(), Expr::Decimal(val) if *val >= 1)
                        } else { false };
            if is_decrement { Some(name) } else { None }
        } else { None }
    }).collect();

    vars.retain(|v| decremented.contains(v));
    if vars.len() < 2 { vec![] } else { vars }
}

fn is_pure_body(
    body: &[Statement],
    state_fields: &HashSet<String>,
    increments: &Option<IncrementInfo>,
) -> bool {
    let inc_var = increments.as_ref().map(|i| &i.var);
    for stmt in body {
        match stmt {
            Statement::Assign(lhs, expr) if inc_var.is_some() => {
                let name = match lhs {
                    Expr::Identifier(n) => n.clone(),
                    _ => return false,
                };
                if Some(&name) == inc_var {
                    if !matches!(expr, Expr::BinaryOp(BinaryOpKind::Add, _, _) | Expr::BinaryOp(BinaryOpKind::BitAnd, _, _)) {
                        return false;
                    }
                    continue;
                }
                if state_fields.contains(&name) {
                    return false;
                }
            }
            Statement::Let { expr, .. } => {
                if let Some(e) = expr {
                    if references_triggers_or_ffi_with_decls(e) {
                        return false;
                    }
                }
            }
            Statement::Assign(_, expr) => {
                if references_triggers_or_ffi_with_decls(expr) {
                    return false;
                }
            }
            Statement::Expression(e) => {
                if references_triggers_or_ffi_with_decls(e) {
                    return false;
                }
            }
            Statement::Term(_) | Statement::EndProgram(_) => {}
            Statement::Rollback(_) => return false,
            Statement::Guarded(condition, statements) => {
                if references_triggers_or_ffi_with_decls(condition) {
                    return false;
                }
                if statements.iter().any(|s| statement_contains_ffi_with_decls(s)) {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

fn references_triggers_or_ffi(expr: &Expr) -> bool {
    references_triggers_or_ffi_with_decls(expr)
}

fn references_triggers_or_ffi_with_decls(expr: &Expr) -> bool {
    match expr {
        Expr::Call(_, _, _) => true,
        Expr::Identifier(_) | Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Quoted(_) => false,
        Expr::BinaryOp(_, a, b) => {
            references_triggers_or_ffi_with_decls(a) || references_triggers_or_ffi_with_decls(b)
        }
        Expr::UnaryOp(_, a) => references_triggers_or_ffi_with_decls(a),
        Expr::Cast(a, _) => references_triggers_or_ffi_with_decls(a),
        Expr::Block(stmts) => stmts.iter().any(|s| statement_contains_ffi_with_decls(s)),
        Expr::List(elems) => elems.iter().any(|e| references_triggers_or_ffi_with_decls(e)),
        Expr::Index(list, idx) => references_triggers_or_ffi_with_decls(list) || references_triggers_or_ffi_with_decls(idx),
        Expr::Field(obj, _) => references_triggers_or_ffi_with_decls(obj),
        Expr::Tuple(elems) => elems.iter().any(|e| references_triggers_or_ffi_with_decls(e)),
        _ => false,
    }
}

pub fn is_uniform_body_group(txns: &[(String, &crate::ast::Transaction)]) -> bool {
    if txns.len() < 2 { return false; }
    let first_body = &txns[0].1.body;
    let first_debug = format!("{:?}", first_body);
    for (_, txn) in &txns[1..] {
        if format!("{:?}", &txn.body) != first_debug { return false; }
    }
    true
}

pub fn compute_live_fields(
    exit_condition: &Option<Box<Expr>>,
    out_pragmas: &[String],
    nodes: &[ReactorNode],
) -> HashSet<String> {
    let mut live = HashSet::new();
    if let Some(ec) = exit_condition {
        collect_identifiers(ec, &mut live);
    }
    for name in out_pragmas {
        live.insert(name.clone());
    }
    for node in nodes {
        collect_identifiers(&node.precondition, &mut live);
    }

    for node in nodes {
        for stmt in &node.body {
            scan_for_ffi_args(stmt, &mut live);
        }
    }
    loop {
        let mut changed = false;
        for node in nodes {
            let mut stmts: Vec<&Statement> = node.body.iter().collect();
            let mut i = 0;
            while i < stmts.len() {
                if let Statement::Guarded(_, statements) = stmts[i] {
                    stmts.extend(statements);
                }
                i += 1;
            }
            for stmt in stmts {
                let (target, expr) = match stmt {
                    Statement::Assign(lhs, expr) => {
                        (expr_name(lhs), expr)
                    }
                    Statement::Let { name, expr: Some(e), .. } => {
                        (Some(name.clone()), e)
                    }
                    _ => continue,
                };
                if let Some(ref t) = target {
                    if live.contains(t) {
                        let mut idents = HashSet::new();
                        collect_identifiers(expr, &mut idents);
                        for ident in idents {
                            if live.insert(ident) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if !changed { break; }
    }

    live
}

pub fn compute_projection_usage(items: &[TopLevel]) -> HashMap<String, HashSet<String>> {
    // 2026-07-17: Collect from both StateDecl and Statement::Let (parser output).
    let state_fields: HashSet<String> = items.iter()
        .filter_map(|item| match item {
            TopLevel::StateDecl(s) => Some(s.name.clone()),
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() { Some(name.clone()) } else { None }
            }
            _ => None,
        })
        .collect();
    let mut usage: HashMap<String, HashSet<String>> = HashMap::new();
    for item in items {
        if let TopLevel::Transaction(txn) = item {
            scan_for_projections_in_stmts(&txn.body, &state_fields, &mut usage);
        }
    }
    usage
}

pub fn projection_target_name(target: &str) -> String {
    target.to_string()
}

fn scan_for_projections_in_stmts(stmts: &[Statement], state_fields: &HashSet<String>, usage: &mut HashMap<String, HashSet<String>>) {
    for stmt in stmts {
        match stmt {
            Statement::Expression(expr) => {
                collect_projection_identifiers(expr, state_fields, usage);
            }
            Statement::Let { expr: Some(expr), .. } => {
                collect_projection_identifiers(expr, state_fields, usage);
            }
            Statement::Assign(_, expr) => {
                collect_projection_identifiers(expr, state_fields, usage);
            }
            Statement::Guarded(_, statements) => {
                scan_for_projections_in_stmts(statements, state_fields, usage);
            }
            Statement::Term(_) | Statement::EndProgram(_) => {}
            _ => {}
        }
    }
}

fn collect_projection_identifiers(expr: &Expr, state_fields: &HashSet<String>, usage: &mut HashMap<String, HashSet<String>>) {
    match expr {
        Expr::Field(source, target) => {
            if let Expr::Identifier(name) = source.as_ref() {
                if state_fields.contains(name) {
                    usage.entry(name.clone()).or_default().insert(projection_target_name(target));
                }
            }
            collect_projection_identifiers(source, state_fields, usage);
        }
        Expr::BinaryOp(_, l, r) => {
            collect_projection_identifiers(l, state_fields, usage);
            collect_projection_identifiers(r, state_fields, usage);
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_projection_identifiers(arg, state_fields, usage);
            }
        }
        Expr::Cast(inner, _) => {
            collect_projection_identifiers(inner, state_fields, usage);
        }
        Expr::Field(obj, _) => {
            collect_projection_identifiers(obj, state_fields, usage);
        }
        Expr::Block(stmts) => {
            for stmt in stmts {
                if let Statement::Expression(e) = stmt {
                    collect_projection_identifiers(e, state_fields, usage);
                }
            }
        }
        _ => {}
    }
}

pub fn compute_referenced_fields(items: &[TopLevel]) -> HashSet<String> {
    // 2026-07-16: Include both StateDecl items AND top-level Statement::Let items.
    // build_field_index registers both as state fields, but only StateDecl was
    // included here, causing fields from let x: T = expr; to be treated as dead.
    let state_fields: HashSet<String> = items.iter()
        .filter_map(|item| match item {
            TopLevel::StateDecl(s) => Some(s.name.clone()),
            TopLevel::Statement(stmt) => {
                if let crate::ast::Statement::Let { name, .. } = stmt.as_ref() {
                    Some(name.clone())
                } else { None }
            }
            _ => None,
        })
        .collect();
    let mut referenced: HashSet<String> = HashSet::new();

    for item in items {
        let body: Option<&[Statement]> = match item {
            TopLevel::Transaction(t) => Some(&t.body),
            TopLevel::Definition(d) => Some(&d.body),
            // 2026-09-06 (ISR plan): an ISR body is a HARDWARE-CALLED
            // liveness root — the handler fires outside the reactor, so a
            // field it touches is never dead. Without this, an ISR-only
            // counter is eliminated by apply_field_modes and the body
            // emits an undefined `@name` load.
            TopLevel::IsrHandler(h) => Some(&h.body),
            // 2026-08-03 (node bridge): an `export defn` referencing a state
            // field (`term saved;`) must count as a reference — without this
            // the field is eliminated as dead by apply_field_modes and the
            // export body emits an undefined `@saved` global.
            TopLevel::Export(e) => {
                if let TopLevel::Definition(d) = e.inner.as_ref() {
                    Some(&d.body)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(body) = body {
            scan_for_state_identifiers(body, &state_fields, &mut referenced);
        }
        if let TopLevel::Transaction(t) = item {
            collect_state_identifiers(&t.contract.pre_condition, &state_fields, &mut referenced);
            collect_state_identifiers(&t.contract.post_condition, &state_fields, &mut referenced);
        }
        // 2026-09-06 (ISR plan): ISR contracts are obligations over the
        // shared state — referenced fields are liveness roots like txn
        // contracts.
        if let TopLevel::IsrHandler(h) = item {
            collect_state_identifiers(&h.contract.pre_condition, &state_fields, &mut referenced);
            collect_state_identifiers(&h.contract.post_condition, &state_fields, &mut referenced);
        }
    }

    referenced
}

fn scan_for_state_identifiers(stmts: &[Statement], state_fields: &HashSet<String>, out: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Statement::Expression(expr)
            | Statement::Let { expr: Some(expr), .. } => {
                collect_state_identifiers(expr, state_fields, out);
            }
            Statement::Assign(lhs, expr) => {
                collect_state_identifiers(lhs, state_fields, out);
                collect_state_identifiers(expr, state_fields, out);
            }
            Statement::Guarded(_, statements) => {
                scan_for_state_identifiers(statements, state_fields, out);
            }
            Statement::Block(body) => {
                scan_for_state_identifiers(body, state_fields, out);
            }
            Statement::Gate(cond) => {
                collect_state_identifiers(cond, state_fields, out);
            }
            Statement::Term(Some(expr)) => {
                collect_state_identifiers(expr, state_fields, out);
            }
            Statement::EndProgram(Some(expr)) => {
                collect_state_identifiers(expr, state_fields, out);
            }
            Statement::Term(None) | Statement::EndProgram(None) => {}
            Statement::Rollback(Some(expr)) => {
                collect_state_identifiers(expr, state_fields, out);
            }
            Statement::Rollback(None) => {}
            Statement::SyncBlock(body) => {
                scan_for_state_identifiers(body, state_fields, out);
            }
            Statement::ArrowAssign { target, value, .. } => {
                // 2026-08-01: the arrow references BOTH the target (insert /
                // store destination) and the value (the collection being
                // inserted into or extracted from). Without this arm, a
                // collection referenced ONLY through `<-`/`~<-` was eliminated
                // by field-liveness, silently disabling the push/pop (the
                // queue_drain / stack_push_pop regression — the harness matched
                // only because their output was the counter, not the collection).
                if let Some(t) = target {
                    collect_state_identifiers(t, state_fields, out);
                }
                collect_state_identifiers(value, state_fields, out);
            }
            // 2026-08-07 (Phase 7): a foreach's iterable is a read — a range
            // bound (`foreach(i in 0..=n)`) or a collection iterated over
            // keeps its fields alive (previously skipped entirely, so the
            // bound field was pruned as Never → undefined `@n`).
            Statement::Foreach { list, body, .. } => {
                collect_state_identifiers(list, state_fields, out);
                scan_for_state_identifiers(body, state_fields, out);
            }
            _ => {}
        }
    }
}

fn collect_state_identifiers(expr: &Expr, state_fields: &HashSet<String>, out: &mut HashSet<String>) {
    match expr {
        Expr::Identifier(name) => {
            if state_fields.contains(name) {
                out.insert(name.clone());
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_state_identifiers(l, state_fields, out);
            collect_state_identifiers(r, state_fields, out);
        }
        Expr::UnaryOp(_, inner) => {
            collect_state_identifiers(inner, state_fields, out);
        }
        Expr::Index(obj, idx) => {
            collect_state_identifiers(obj, state_fields, out);
            collect_state_identifiers(idx, state_fields, out);
        }
        Expr::Field(obj, _) => {
            collect_state_identifiers(obj, state_fields, out);
        }
        // 2026-07-21: Handle &state_field and *state_field so arrow ops
        // (e.g. <- &queue, &queue <- count) don't hide state references
        // from field-liveness analysis (fixes queue_drain build failure).
        Expr::AddrOf(inner) | Expr::Deref(inner) | Expr::Consume(inner) => {
            collect_state_identifiers(inner, state_fields, out);
        }
        Expr::Cast(inner, _) => {
            collect_state_identifiers(inner, state_fields, out);
        }
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_state_identifiers(arg, state_fields, out);
            }
        }
        // 2026-07-31 (A5): MethodCall / Reflect receivers ARE uses — a
        // struct-typed state field that is a method-call receiver (or
        // reflected upon) must not be dead-eliminated.
        Expr::MethodCall(recv, _, args, _, _) => {
            collect_state_identifiers(recv, state_fields, out);
            for arg in args {
                collect_state_identifiers(arg, state_fields, out);
            }
        }
        Expr::Reflect(recv, _, _) => {
            collect_state_identifiers(recv, state_fields, out);
        }
        Expr::Block(stmts) => {
            for stmt in stmts {
                if let Statement::Expression(e) = stmt {
                    collect_state_identifiers(e, state_fields, out);
                }
            }
        }
        Expr::Tuple(exprs) | Expr::List(exprs) => {
            for e in exprs {
                collect_state_identifiers(e, state_fields, out);
            }
        }
        Expr::Match(scrutinee, arms) => {
            // 2026-08-06 (Phase 7): the SCRUTINEE is a read — a match on a
            // state field (`match tick { ... }`) must keep the field alive.
            // Previously discarded, so the field was marked Never and pruned,
            // leaving an undefined `@tick` in the match's load.
            collect_state_identifiers(scrutinee, state_fields, out);
            for arm in arms {
                // Guards may reference state fields too (`x when tick < 5`).
                if let Some(guard) = &arm.guard {
                    collect_state_identifiers(guard, state_fields, out);
                }
                collect_state_identifiers(&arm.body, state_fields, out);
            }
        }
        Expr::Within(inner, _) => {
            collect_state_identifiers(inner, state_fields, out);
        }
        Expr::If(cond, then, else_) => {
            collect_state_identifiers(cond, state_fields, out);
            collect_state_identifiers(then, state_fields, out);
            if let Some(else_) = else_ {
                collect_state_identifiers(else_, state_fields, out);
            }
        }
        Expr::Lambda(_, body) => {
            collect_state_identifiers(body, state_fields, out);
        }
        // 2026-08-07 (Phase 7): a range's bounds are reads — a foreach over
        // `0..=n` keeps the bound field alive (previously pruned as Never).
        Expr::Range { start, end, inclusive: _ } => {
            collect_state_identifiers(start, state_fields, out);
            collect_state_identifiers(end, state_fields, out);
        }
        _ => {}
    }
}

/// Collect the state-field identifiers a STATEMENT touches (reads or writes).
/// 2026-08-01 (D2): used by garbage scheduling (global_lifetime) to compute a
/// transaction's touch set — a write after a scheduled free is a use-after-free
/// too, so the scheduler frees after the LAST touch.
pub(crate) fn collect_statement_identifiers(
    stmt: &crate::ast::Statement,
    state_fields: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    use crate::ast::Statement;
    match stmt {
        Statement::Assign(lhs, rhs) => {
            collect_state_identifiers(lhs, state_fields, out);
            collect_state_identifiers(rhs, state_fields, out);
        }
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                collect_state_identifiers(e, state_fields, out);
            }
        }
        Statement::Expression(e) => {
            collect_state_identifiers(e, state_fields, out);
        }
        Statement::ArrowAssign { target, value, .. } => {
            // 2026-08-01: the arrow references the collection (the value for an
            // extract/discard, or the target for an insert) — see
            // scan_for_state_identifiers. Without this arm the garbage
            // scheduler's touch-set missed arrow-only fields (it never freed
            // them) and the field-liveness elimination hid them entirely.
            if let Some(t) = target {
                collect_state_identifiers(t, state_fields, out);
            }
            collect_state_identifiers(value, state_fields, out);
        }
        Statement::Guarded(cond, body) => {
            collect_state_identifiers(cond, state_fields, out);
            for s in body {
                collect_statement_identifiers(s, state_fields, out);
            }
        }
        Statement::Term(val) | Statement::EndProgram(val) => {
            if let Some(v) = val {
                collect_state_identifiers(v, state_fields, out);
            }
        }
        // 2026-08-07 (Phase 7): a foreach's iterable and body are reads —
        // a range bound (`0..=n`) or a collection iterated over keeps its
        // fields alive (previously the whole statement was skipped).
        Statement::Foreach { list, body, .. } => {
            collect_state_identifiers(list, state_fields, out);
            for s in body {
                collect_statement_identifiers(s, state_fields, out);
            }
        }
        Statement::Block(stmts) => {
            for s in stmts {
                collect_statement_identifiers(s, state_fields, out);
            }
        }
        _ => {}
    }
}

pub fn assign_field_modes(
    all_state_fields: &HashSet<String>,
    referenced_fields: &HashSet<String>,
    projection_usage: &HashMap<String, HashSet<String>>,
) -> HashMap<String, super::FieldMode> {
    let mut modes: HashMap<String, super::FieldMode> = HashMap::new();
    let mut next_cache_index: usize = 0;

    for field in all_state_fields {
        let usage = projection_usage.get(field);
        if let Some(targets) = usage {
            if targets.len() >= 2 {
                modes.insert(field.clone(), super::FieldMode::LazyCached {
                    cache_index: {
                        let ci = next_cache_index;
                        next_cache_index += 1;
                        ci
                    },
                });
                continue;
            }
        }

        if referenced_fields.contains(field) {
            modes.insert(field.clone(), super::FieldMode::Always);
        } else {
            modes.insert(field.clone(), super::FieldMode::Never);
        }
    }

    modes
}

fn scan_for_ffi_args(stmt: &Statement, out: &mut HashSet<String>) {
    match stmt {
        Statement::Expression(expr) | Statement::Let { expr: Some(expr), .. } => {
            collect_ffi_identifiers(expr, out);
        }
        Statement::Guarded(_, statements) => {
            for s in statements {
                scan_for_ffi_args(s, out);
            }
        }
        _ => {}
    }
}

fn collect_ffi_identifiers(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_identifiers(arg, out);
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_ffi_identifiers(l, out);
            collect_ffi_identifiers(r, out);
        }
        _ => collect_identifiers(expr, out),
    }
}

fn expr_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(n) => Some(n.clone()),
        _ => None,
    }
}

fn compute_effectively_pure(node: &mut ReactorNode, live_fields: &HashSet<String>) {
    if node.body.iter().any(|s| statement_contains_ffi_with_decls(s)) {
        return;
    }
    if let (Some(bp), Some(inc)) = (&node.bounded_pre, &node.increments) {
        if inc.var == bp.var && inc.delta > 0 && live_fields.contains(&inc.var) {
            let non_counter_writes: Vec<&String> = node.write_set.iter()
                .filter(|f| *f != &inc.var)
                .collect();
            if non_counter_writes.iter().all(|f| !live_fields.contains(*f)) {
                node.is_effectively_pure = true;
            }
        }
    }
}

pub(crate) fn statement_contains_ffi(stmt: &Statement) -> bool {
    statement_contains_ffi_with_decls(stmt)
}

pub(crate) fn statement_contains_ffi_with_decls(stmt: &Statement) -> bool {
    match stmt {
        Statement::Assign(_, expr) => references_triggers_or_ffi_with_decls(expr),
        Statement::Let { expr, .. } => expr.as_ref().map_or(false, |e| references_triggers_or_ffi_with_decls(e)),
        Statement::Expression(e) => references_triggers_or_ffi_with_decls(e),
        Statement::Term(Some(e)) => references_triggers_or_ffi_with_decls(e),
        Statement::EndProgram(Some(e)) => references_triggers_or_ffi_with_decls(e),
        Statement::Guarded(condition, statements) => {
            references_triggers_or_ffi_with_decls(condition)
                || statements.iter().any(|s| statement_contains_ffi_with_decls(s))
        }
        _ => false,
    }
}

fn is_self_identity(a: &Expr, b: &Expr) -> bool {
    matches!((a, b),
        (Expr::Identifier(n1), Expr::Identifier(n2)) if n1 == n2)
}

fn collect_identifiers(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Identifier(name) => {
            out.insert(name.clone());
        }
        // Self-identity operations (x == x, x >= x, x <= x) are tautologies that
        // don't actually observe the field's value. Skip them to avoid keeping
        // fields artificially alive in dead-field analysis.
        Expr::BinaryOp(BinaryOpKind::Eq, a, b) if is_self_identity(a, b) => {}
        Expr::BinaryOp(BinaryOpKind::Ge, a, b) if is_self_identity(a, b) => {}
        Expr::BinaryOp(BinaryOpKind::Le, a, b) if is_self_identity(a, b) => {}
        Expr::BinaryOp(_, a, b) => {
            collect_identifiers(a, out);
            collect_identifiers(b, out);
        }
        Expr::UnaryOp(_, a) => {
            collect_identifiers(a, out);
        }
        Expr::Cast(a, _) => collect_identifiers(a, out),
        Expr::Call(_, args, _) => {
            for arg in args {
                collect_identifiers(arg, out);
            }
        }
        Expr::List(elems) => {
            for elem in elems {
                collect_identifiers(elem, out);
            }
        }
        Expr::Index(list, idx) => {
            collect_identifiers(list, out);
            collect_identifiers(idx, out);
        }
        Expr::Field(obj, _) => {
            collect_identifiers(obj, out);
        }
        // 2026-08-01 (B3): reflection (`s.^Length`, `s.^^Bytes`, `s.^Ptr`)
        // READS its receiver — the field must stay live. Without this arm,
        // a String `let` used only via reflection was eliminated as a dead
        // state field, and `s.^Length` emitted `load i64, ptr @s` with @s
        // undefined (the field was dropped from %State). This is the same
        // liveness rule as FFI args: an observation keeps the value alive.
        Expr::Reflect(recv, _, _) => {
            collect_identifiers(recv, out);
        }
        Expr::Block(stmts) => {
            for stmt in stmts {
                if let Statement::Expression(e) = stmt {
                    collect_identifiers(e, out);
                }
            }
        }
        Expr::Tuple(elems) => {
            for elem in elems {
                collect_identifiers(elem, out);
            }
        }
        Expr::Match(_, arms) => {
            for arm in arms {
                if let Some(ref guard) = arm.guard {
                    collect_identifiers(guard, out);
                }
                collect_identifiers(&arm.body, out);
            }
        }
        Expr::If(cond, then, else_) => {
            collect_identifiers(cond, out);
            collect_identifiers(then, out);
            if let Some(else_) = else_ {
                collect_identifiers(else_, out);
            }
        }
        Expr::Lambda(_, body) => {
            collect_identifiers(body, out);
        }
        Expr::Within(inner, _) => {
            collect_identifiers(inner, out);
        }
        _ => {}
    }
}

/// Extract the set of state fields that a transaction body writes to.
/// 2026-07-18: Provenance-aware — uses infer_provenance to trace through
/// field accesses and index operations, yielding more precise write sets
/// than simple identifier matching.
fn extract_write_set(body: &[Statement], state_fields: &HashSet<String>) -> HashSet<String> {
    let mut writes = HashSet::new();
    for stmt in body {
        // 2026-08-11 (Phase 2a fix): recurse into nested statement containers.
        // The webstack flush batch covers exactly the txn's write_set — a
        // conditional write (`if count > 2 { show = true; }`) that was missed
        // meant the DOM never received a flush for `show`, so b-when/b-show
        // bindings on it stayed dead. Top-level Assign-only scanning was the
        // bug.
        match stmt {
            Statement::Assign(lhs, _) => {
                // 2026-07-18: Use provenance to extract the root variable name
                // from Expr::Field and Expr::Index chains.
                let root = extract_root_via_provenance(lhs);
                if let Some(name) = root {
                    if state_fields.contains(&name) {
                        writes.insert(name);
                    }
                }
            }
            Statement::Guarded(_, body) => {
                writes.extend(extract_write_set(body, state_fields));
            }
            Statement::Block(body) => {
                writes.extend(extract_write_set(body, state_fields));
            }
            Statement::Foreach { body, .. } => {
                writes.extend(extract_write_set(body, state_fields));
            }
            _ => {}
        }
    }
    writes
}

/// 2026-07-18: Walk the provenance chain of an expression to find the root
/// variable name. Handles Identifier and Field(base, _) by recursively
/// walking to the base until an Identifier is found.
/// 2026-07-21: Removed Index(base, _) arm. Pointer writes (data[idx] = val)
/// change the memory AT the pointer, not the pointer field itself. Including
/// Index in write_set creates redundant identity phi nodes for ptr fields.
fn extract_root_via_provenance(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(n) => Some(n.clone()),
        Expr::Field(base, _) => extract_root_via_provenance(base),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(name: &str, ty: Type) -> TopLevel {
        TopLevel::StateDecl(crate::ast::StateDecl {
            name: name.to_string(),
            ty,
            span: None,
        })
    }

    #[test]
    fn test_extract_bounded_pre_counter_lt_total() {
        let pre = Expr::BinaryOp(BinaryOpKind::Lt,
            Box::new(Expr::Identifier("count".to_string())),
            Box::new(Expr::Identifier("total".to_string())),
        );
        let bp = extract_bounded_pre(&pre).unwrap();
        assert_eq!(bp.var, "count");
        assert_eq!(bp.bound_var, "total");
    }

    #[test]
    fn test_detect_increments() {
        let body = vec![Statement::Assign(
            Expr::Identifier("count".to_string()),
            Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
        )];
        let inc = detect_increments(&body).unwrap();
        assert_eq!(inc.var, "count");
        assert_eq!(inc.delta, 1);
    }

    #[test]
    fn test_pure_counter_body() {
        let fields: HashSet<String> = ["count".to_string(), "total".to_string()].into();
        let body = vec![Statement::Assign(
            Expr::Identifier("count".to_string()),
            Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
        )];
        let inc = detect_increments(&body);
        assert!(is_pure_body(&body, &fields, &inc));
    }

    #[test]
    fn test_impure_body_with_state_write() {
        let fields: HashSet<String> = ["count".to_string(), "value".to_string()].into();
        let body = vec![
            Statement::Assign(
                Expr::Identifier("value".to_string()),
                Expr::Float(1.0),
            ),
            Statement::Assign(
                Expr::Identifier("count".to_string()),
                Expr::BinaryOp(BinaryOpKind::Add,
                    Box::new(Expr::Identifier("count".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
        ];
        let inc = detect_increments(&body);
        assert!(!is_pure_body(&body, &fields, &inc));
    }

    #[test]
    fn test_is_uniform_body_group_identical() {
        let body = vec![Statement::Assign(
            Expr::Identifier("count".to_string()),
            Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Decimal(1)),
            ),
        )];
        let txn1 = Transaction {
            name: "txn_a".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: Vec::new(),
            contract: crate::ast::Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: body.clone(),
            span: None,
            metadata: std::collections::HashMap::new(),
            modifiers: vec![],
            derivation: None,
            doc: None,
        };
        let txn2 = Transaction {
            name: "txn_b".to_string(),
            doc: None,
            .. txn1.clone()
        };
        let pairs: Vec<(String, &Transaction)> = vec![
            ("txn_a".to_string(), &txn1),
            ("txn_b".to_string(), &txn2),
        ];
        assert!(is_uniform_body_group(&pairs));
    }

    #[test]
    fn test_is_uniform_body_group_different() {
        let body_a = vec![Statement::Assign(
            Expr::Identifier("a".to_string()),
            Expr::Decimal(1),
        )];
        let body_b = vec![Statement::Assign(
            Expr::Identifier("b".to_string()),
            Expr::Decimal(2),
        )];
        let txn_a = Transaction {
            name: "txn_a".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: Vec::new(),
            contract: crate::ast::Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: body_a,
            span: None,
            metadata: std::collections::HashMap::new(),
            modifiers: vec![],
            derivation: None,
            doc: None,
        };
        let txn_b = Transaction {
            body: body_b,
            name: "txn_b".to_string(),
            doc: None,
            .. txn_a.clone()
        };
        let pairs: Vec<(String, &Transaction)> = vec![
            ("txn_a".to_string(), &txn_a),
            ("txn_b".to_string(), &txn_b),
        ];
        assert!(!is_uniform_body_group(&pairs));
    }

    #[test]
    fn test_is_uniform_body_group_single() {
        let body = vec![];
        let txn = Transaction {
            name: "only".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: Vec::new(),
            contract: crate::ast::Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            span: None,
            metadata: std::collections::HashMap::new(),
            modifiers: vec![],
            derivation: None,
            doc: None,
        };
        let pairs: Vec<(String, &Transaction)> = vec![("only".to_string(), &txn)];
        assert!(!is_uniform_body_group(&pairs));
    }

    #[test]
    fn test_graph_single_counter_txn() {
        let items = vec![
            make_state("count", Type::int()),
            make_state("total", Type::int()),
            TopLevel::Transaction(Transaction {
                name: "inc".to_string(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: Vec::new(),
                contract: crate::ast::Contract {
                    pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("count".to_string())),
                        Box::new(Expr::Identifier("total".to_string())),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![Statement::Assign(
                    Expr::Identifier("count".to_string()),
                    Expr::BinaryOp(BinaryOpKind::Add,
                        Box::new(Expr::Identifier("count".to_string())),
                        Box::new(Expr::Decimal(1)),
                    ),
                )],
                span: None,
                metadata: std::collections::HashMap::new(),
                modifiers: vec![],
                derivation: None,
                doc: None,
            }),
        ];
        let graph = ReactorTransitionGraph::build(&items, &None, &vec![]);
        assert_eq!(graph.nodes.len(), 1);
        assert!(!graph.has_triggers);
        let node = &graph.nodes[0];
        assert!(node.bounded_pre.is_some());
        assert!(node.increments.is_some());
        assert!(node.is_pure_body);
    }

    #[test]
    fn test_compute_projection_usage_none() {
        let items = vec![
            make_state("x", Type::int()),
            TopLevel::Transaction(Transaction {
                name: "t".into(),
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: Vec::new(),
                contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![Statement::Term(None)],
                is_async: false, is_reactive: false, span: None,
                metadata: std::collections::HashMap::new(),
                modifiers: vec![], derivation: None,
                doc: None,
            }),
        ];
        let usage = compute_projection_usage(&items);
        assert!(usage.is_empty(), "no projections → empty usage");
    }

    #[test]
    fn test_compute_projection_usage_single() {
        let items = vec![
            make_state("x", Type::int()),
            TopLevel::Transaction(Transaction {
                name: "t".into(),
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: Vec::new(),
                contract: crate::ast::Contract::new(Expr::Bool(true), Expr::Bool(true)),
                body: vec![
                    Statement::Expression(Expr::Field(
                        Box::new(Expr::Identifier("x".into())),
                        "Size".to_string(),
                    )),
                    Statement::Term(None),
                ],
                is_async: false, is_reactive: false, span: None,
                metadata: std::collections::HashMap::new(),
                modifiers: vec![], derivation: None,
                doc: None,
            }),
        ];
        let usage = compute_projection_usage(&items);
        assert_eq!(usage.len(), 1, "field x should have projection usage");
        let targets = usage.get("x").expect("x should be in usage");
        assert!(targets.contains("Size"), "x should have Size projection");
        assert_eq!(targets.len(), 1, "only one projection target");
    }

    #[test]
    fn test_assign_field_modes_single_lens() {
        let mut usage: HashMap<String, HashSet<String>> = HashMap::new();
        usage.insert("x".to_string(), {
            let mut s = HashSet::new();
            s.insert("Size".to_string());
            s
        });
        let all: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let referenced: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let modes = assign_field_modes(&all, &referenced, &usage);
        let mode = modes.get("x").expect("x should have a mode");
        assert_eq!(*mode, crate::analysis::FieldMode::Always, "single-lens + referenced → Always");
    }

    #[test]
    fn test_assign_field_modes_dual_lens() {
        let mut usage: HashMap<String, HashSet<String>> = HashMap::new();
        usage.insert("x".to_string(), {
            let mut s = HashSet::new();
            s.insert("Ptr".to_string());
            s.insert("Size".to_string());
            s
        });
        let all: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let referenced: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let modes = assign_field_modes(&all, &referenced, &usage);
        let mode = modes.get("x").expect("x should have a mode");
        assert_eq!(*mode, crate::analysis::FieldMode::LazyCached { cache_index: 0 }, "dual-lens → LazyCached");
    }

    #[test]
    fn test_assign_field_modes_no_usage_unreferenced() {
        let usage: HashMap<String, HashSet<String>> = HashMap::new();
        let all: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let referenced: HashSet<String> = HashSet::new();
        let modes = assign_field_modes(&all, &referenced, &usage);
        let mode = modes.get("x").expect("x should have a mode");
        assert_eq!(*mode, crate::analysis::FieldMode::Never, "unreferenced + no projection → Never");
    }

    #[test]
    fn test_collect_state_identifiers_match_scrutinee_is_a_read() {
        // 2026-08-06 (Phase 7): the field liveness collector must treat a
        // match SCRUTINEE as a read. Before the fix it only walked arm bodies,
        // so `match tick { ... }` marked `tick` Never and pruned it — leaving
        // an undefined `@tick` in the generated IR.
        let mut out = HashSet::new();
        let state_fields: HashSet<String> = ["tick"].iter().map(|s| s.to_string()).collect();
        let m = Expr::Match(
            Box::new(Expr::Identifier("tick".to_string())),
            vec![
                crate::ast::MatchArm {
                    pattern: crate::ast::Pattern::RangeInclusive(
                        Expr::Decimal(1),
                        Expr::Decimal(5),
                    ),
                    guard: None,
                    body: Box::new(Expr::Decimal(7)),
                },
                crate::ast::MatchArm {
                    pattern: crate::ast::Pattern::Wildcard,
                    guard: Some(Expr::Identifier("tick".to_string())),
                    body: Box::new(Expr::Decimal(0)),
                },
            ],
        );
        collect_state_identifiers(&m, &state_fields, &mut out);
        assert!(out.contains("tick"),
            "match scrutinee (and any guard) must count as a field read");
    }

    #[test]
    fn test_assign_field_modes_no_usage_referenced() {
        let usage: HashMap<String, HashSet<String>> = HashMap::new();
        let all: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let referenced: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let modes = assign_field_modes(&all, &referenced, &usage);
        let mode = modes.get("x").expect("x should have a mode");
        assert_eq!(*mode, crate::analysis::FieldMode::Always, "referenced + no projection → Always");
    }

    // ── Provenance-aware write set tests ───────────────────────────────

    #[test]
    fn test_extract_root_via_provenance_identifier() {
        let expr = Expr::Identifier("count".to_string());
        assert_eq!(extract_root_via_provenance(&expr), Some("count".to_string()));
    }

    #[test]
    fn test_extract_root_via_provenance_field() {
        let expr = Expr::Field(
            Box::new(Expr::Identifier("obj".to_string())),
            "Size".to_string(),
        );
        assert_eq!(extract_root_via_provenance(&expr), Some("obj".to_string()));
    }

    #[test]
    fn test_extract_root_via_provenance_decimal() {
        let expr = Expr::Decimal(42);
        assert_eq!(extract_root_via_provenance(&expr), None);
    }

    #[test]
    fn test_extract_write_set_provenance_field_assign() {
        let fields: HashSet<String> = ["obj".to_string()].into();
        let body = vec![Statement::Assign(
            Expr::Field(
                Box::new(Expr::Identifier("obj".to_string())),
                "Size".to_string(),
            ),
            Expr::Decimal(42),
        )];
        let writes = extract_write_set(&body, &fields);
        assert!(writes.contains("obj"), "field assign to obj.Size should detect obj as written");
        assert_eq!(writes.len(), 1);
    }

    /// 2026-08-11 (Phase 2a fix): a write inside an `if` body is still a
    /// write — the webstack flush batch (sized to the write_set) must cover
    /// it, or the DOM never learns the field changed. Regression for the
    /// top-level-Assign-only scan.
    #[test]
    fn test_extract_write_set_recurses_into_conditionals() {
        let fields: HashSet<String> = ["count".to_string(), "show".to_string()].into();
        let body = vec![
            Statement::Assign(
                Expr::Identifier("count".to_string()),
                Expr::Decimal(1),
            ),
            Statement::Guarded(
                Expr::BinaryOp(
                    BinaryOpKind::Gt,
                    Box::new(Expr::Identifier("count".to_string())),
                    Box::new(Expr::Decimal(2)),
                ),
                vec![Statement::Assign(
                    Expr::Identifier("show".to_string()),
                    Expr::Bool(true),
                )],
            ),
        ];
        let writes = extract_write_set(&body, &fields);
        assert!(writes.contains("count"));
        assert!(
            writes.contains("show"),
            "conditional write must land in the write_set: {writes:?}"
        );
        assert_eq!(writes.len(), 2);
    }

    /// Phase 2 (§7.4): a top-level FFI call (not inside a guard) marks the
    /// txn as having unguarded FFI → the reactor tick forces memory(readwrite).
    #[test]
    fn test_has_unguarded_ffi_top_level_call() {
        let items = vec![
            make_state("count", Type::int()),
            make_state("total", Type::int()),
            TopLevel::Transaction(Transaction {
                name: "work".to_string(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: Vec::new(),
                contract: crate::ast::Contract {
                    pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("count".to_string())),
                        Box::new(Expr::Identifier("total".to_string())),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    Statement::Expression(Expr::Call("PrintLn#".to_string(), vec![], None)),
                    Statement::Term(None),
                ],
                span: None,
                metadata: std::collections::HashMap::new(),
                modifiers: vec![],
                derivation: None,
                doc: None,
            }),
        ];
        let g = ReactorTransitionGraph::build(&items, &None, &[]);
        assert!(g.has_unguarded_ffi.contains("work"),
            "top-level FFI call must be flagged as unguarded");
    }

    /// Phase 2 (§7.4): FFI inside a `when` guard is OUTLINED into a cold
    /// function — it does not force the hot path to memory(readwrite).
    #[test]
    fn test_guarded_ffi_not_unguarded() {
        let items = vec![
            make_state("count", Type::int()),
            make_state("total", Type::int()),
            TopLevel::Transaction(Transaction {
                name: "work".to_string(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: Vec::new(),
                contract: crate::ast::Contract {
                    pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("count".to_string())),
                        Box::new(Expr::Identifier("total".to_string())),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    Statement::Guarded(
                        Expr::BinaryOp(BinaryOpKind::Eq,
                            Box::new(Expr::BinaryOp(BinaryOpKind::Mod,
                                Box::new(Expr::Identifier("count".to_string())),
                                Box::new(Expr::Decimal(5))),
                            ),
                            Box::new(Expr::Decimal(0)),
                        ),
                        vec![Statement::Expression(Expr::Call("PrintLn#".to_string(), vec![], None))],
                    ),
                    Statement::Term(None),
                ],
                span: None,
                metadata: std::collections::HashMap::new(),
                modifiers: vec![],
                derivation: None,
                doc: None,
            }),
        ];
        let g = ReactorTransitionGraph::build(&items, &None, &[]);
        assert!(!g.has_unguarded_ffi.contains("work"),
            "FFI inside a `when` guard is guarded (outlined) — not unguarded");
    }

    /// 2026-08-04 (out-observability plan): a top-level `out let` state field
    /// written but never read must stay live — `out` marks its writes as
    /// liveness roots, so dead-field elimination cannot drop the field.
    #[test]
    fn test_out_let_field_forced_live() {
        use crate::ast::Annotation;
        let items = vec![
            make_state("count", Type::int()),
            make_state("total", Type::int()),
            TopLevel::Statement(Box::new(Statement::Let {
                name: "pinned".to_string(),
                names: vec![],
                ty: Some(Type::int()),
                expr: Some(Expr::Decimal(0)),
                modifiers: vec![Annotation { name: "out".to_string(), value: None }],
            })),
            TopLevel::Transaction(Transaction {
                name: "work".to_string(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: Vec::new(),
                contract: crate::ast::Contract {
                    pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("count".to_string())),
                        Box::new(Expr::Identifier("total".to_string())),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    Statement::Assign(Expr::Identifier("count".to_string()),
                        Expr::BinaryOp(BinaryOpKind::Add,
                            Box::new(Expr::Identifier("count".to_string())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                    // pinned is written but never read — only `out` keeps it live.
                    Statement::Assign(Expr::Identifier("pinned".to_string()), Expr::Decimal(7)),
                    Statement::Term(None),
                ],
                span: None,
                metadata: std::collections::HashMap::new(),
                modifiers: vec![],
                derivation: None,
                doc: None,
            }),
        ];
        let g = ReactorTransitionGraph::build(&items, &None, &[]);
        assert!(g.live_fields.contains("pinned"),
            "out-let field must stay live even though never read");
    }

    /// 2026-08-09 (Bug 2): a `sync<group> node` is a real reactive node — it
    /// must produce a graph node with a bounded-pre and its fields live.
    /// Previously sync-group nodes were skipped by the graph builder, so
    /// dead-field elimination dropped their state → undefined `@field` globals
    /// and an empty reactor dispatch.
    #[test]
    fn test_sync_group_node_enters_graph() {
        fn make_sync_node(name: &str, var: &str) -> TopLevel {
            TopLevel::SyncGroup {
                domains: vec!["step".to_string()],
                item: Box::new(TopLevel::Transaction(Transaction {
                    name: name.to_string(),
                    type_params: vec![],
                    parameters: vec![],
                    output_type: None,
                    outputs: Vec::new(),
                    contract: crate::ast::Contract::new(
                        Expr::BinaryOp(
                            BinaryOpKind::Lt,
                            Box::new(Expr::Identifier(var.to_string())),
                            Box::new(Expr::Decimal(4)),
                        ),
                        Expr::BinaryOp(
                            BinaryOpKind::Eq,
                            Box::new(Expr::Identifier(var.to_string())),
                            Box::new(Expr::Decimal(4)),
                        ),
                    ),
                    body: vec![
                        Statement::Assign(
                            Expr::Identifier(var.to_string()),
                            Expr::BinaryOp(
                                BinaryOpKind::Add,
                                Box::new(Expr::Identifier(var.to_string())),
                                Box::new(Expr::Decimal(1)),
                            ),
                        ),
                        Statement::Term(None),
                    ],
                    span: None,
                    metadata: std::collections::HashMap::new(),
                    modifiers: vec![],
                    derivation: None,
                    doc: None,
                    is_reactive: true,
                    is_async: false,
                })),
            }
        }
        let items = vec![
            make_state("i", Type::int()),
            make_state("count", Type::int()),
            make_sync_node("force", "i"),
            make_sync_node("step", "count"),
        ];
        let graph = ReactorTransitionGraph::build(&items, &None, &[]);
        assert_eq!(graph.nodes.len(), 2, "sync-group nodes must enter the graph");
        assert!(
            graph.nodes.iter().all(|n| n.bounded_pre.is_some()),
            "each sync-group node must get a bounded-pre"
        );
        assert!(
            graph.live_fields.contains("i") && graph.live_fields.contains("count"),
            "sync-group node fields must be live: {:?}",
            graph.live_fields
        );
    }
}
