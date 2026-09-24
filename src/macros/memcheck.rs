//! `brievc memcheck <file.bv>` — the garbage-scheduler diagnostics subcommand.
//!
//! 2026-08-01 (Phase 5): reports, per heap-backed state field, whether the
//! garbage scheduler proved a last use and scheduled a free (and after which
//! transaction), or fell back to "lives for the program" (a potential leak).
//! Also reports the effect of every `free x;` / `keep x;` hint.

use crate::analysis::global_lifetime::GlobalLifetime;
use std::collections::HashMap;

/// Run the memcheck analysis on a parsed program.
pub fn run_memcheck(items: &[crate::ast::TopLevel]) -> MemcheckReport {
    // Field initializers mirror analyze_program (StateDecl + top-level let).
    let mut field_inits: HashMap<String, crate::ast::Expr> = HashMap::new();
    for item in items {
        if let crate::ast::TopLevel::StateDecl(s) = item {
            field_inits.entry(s.name.clone()).or_insert(crate::ast::Expr::Decimal(0));
        } else if let crate::ast::TopLevel::Statement(stmt) = item {
            if let crate::ast::Statement::Let { name, expr, .. } = stmt.as_ref() {
                if let Some(e) = expr {
                    field_inits.entry(name.clone()).or_insert_with(|| e.clone());
                }
            }
        }
    }
    // The reactor firing order is the transition-graph node order.
    let transition_graph = crate::analysis::transition_graph::ReactorTransitionGraph::build(
        items, &None, &vec![],
    );
    let node_order: Vec<String> = transition_graph.nodes.iter().map(|n| n.name.clone()).collect();
    let foldable: std::collections::HashSet<String> = transition_graph
        .nodes
        .iter()
        .filter(|n| n.bounded_pre.is_some())
        .map(|n| n.name.clone())
        .collect();
    let lifetime = crate::analysis::global_lifetime::analyze(items, &field_inits, &node_order, &foldable);
    // Only heap-backed fields are schedulable; scalars are never freed (a
    // "lives for the program" report on them would be misleading).
    let heap_fields: Vec<String> = field_inits
        .iter()
        .filter(|(_, init)| crate::analysis::global_lifetime::contains_heap_alloc(init))
        .map(|(name, _)| name.clone())
        .collect();

    // 2026-08-09 (init kind, Phase 2): an init-bound pool is SEALED — its
    // capacity is the bound-set max (phase 4 sizes the pool from it), so it is
    // provably inexhaustible rather than "lives for the program (unprovable)".
    // A field is sealed when its initializer references an `init` name (the
    // pool's capacity reads the seeded value).
    let init_names: std::collections::HashSet<String> = items
        .iter()
        .filter_map(|item| match item {
            crate::ast::TopLevel::Init(i) => Some(i.name.clone()),
            _ => None,
        })
        .collect();
    let sealed_fields: Vec<String> = field_inits
        .iter()
        .filter(|(name, init)| {
            heap_fields.contains(name)
                && expr_references_any(init, &init_names)
        })
        .map(|(name, _)| name.clone())
        .collect();

    MemcheckReport {
        lifetime,
        field_names: heap_fields,
        sealed_fields,
    }
}

/// Does the expression reference any of the given names (recursively)? Used to
/// detect init-bound pool fields for the sealed-field report.
fn expr_references_any(expr: &crate::ast::Expr, names: &std::collections::HashSet<String>) -> bool {
    use crate::ast::Expr;
    match expr {
        Expr::Identifier(n) => names.contains(n),
        Expr::Decimal(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Char(_)
        | Expr::Quoted(_) | Expr::TaggedLiteral(_, _) | Expr::TaggedQuotedLiteral(_, _) => false,
        Expr::Call(_, args, _) => args.iter().any(|a| expr_references_any(a, names)),
        Expr::BinaryOp(_, l, r) => expr_references_any(l, names) || expr_references_any(r, names),
        Expr::UnaryOp(_, e) | Expr::Deref(e) | Expr::AddrOf(e) | Expr::Cast(e, _)
        | Expr::Field(e, _) => expr_references_any(e, names),
        Expr::Index(obj, idx) => expr_references_any(obj, names) || expr_references_any(idx, names),
        Expr::List(items) | Expr::Tuple(items) => items.iter().any(|i| expr_references_any(i, names)),
        Expr::Block(stmts) => stmts.iter().any(|s| stmt_references_any(s, names)),
        Expr::Consume(inner) => match inner.as_ref() {
            crate::ast::Expr::Identifier(n) => names.contains(n),
            e => expr_references_any(e, names),
        },
        Expr::MethodCall(..) | Expr::Match(..) => false,
        _ => false,
    }
}

/// Does a seeding-body statement reference any of the given names?
fn stmt_references_any(
    stmt: &crate::ast::Statement,
    names: &std::collections::HashSet<String>,
) -> bool {
    use crate::ast::Statement;
    match stmt {
        Statement::Let { expr, .. } => expr.as_ref().map_or(false, |e| expr_references_any(e, names)),
        Statement::Assign(lhs, rhs) => {
            expr_references_any(lhs, names) || expr_references_any(rhs, names)
        }
        Statement::ArrowAssign { target, value, .. } => {
            target.as_ref().map_or(false, |t| expr_references_any(t, names))
                || expr_references_any(value, names)
        }
        Statement::Term(val) => val.as_ref().map_or(false, |e| expr_references_any(e, names)),
        Statement::Expression(e) => expr_references_any(e, names),
        Statement::Guarded(_, body) => body.iter().any(|s| stmt_references_any(s, names)),
        _ => false,
    }
}

/// The report: the scheduler's decisions + the hint outcomes.
pub struct MemcheckReport {
    pub lifetime: GlobalLifetime,
    pub field_names: Vec<String>,
    /// 2026-08-09 (init kind, Phase 2): heap-backed fields whose capacity is
    /// bound by a runtime-seeded `init` — provably inexhaustible (sealed),
    /// not "lives for the program (unprovable)".
    pub sealed_fields: Vec<String>,
}

/// Print the report to stdout.
pub fn print_memcheck(report: &MemcheckReport) {
    let mut fields = report.field_names.clone();
    fields.sort();
    println!("=== memcheck — garbage-scheduler decisions ===");
    if fields.is_empty() {
        println!("  (no state fields)");
    }
    for f in &fields {
        println!("{}", field_decision_line(report, f));
    }
    if !report.lifetime.redundant_keeps.is_empty() {
        println!("  redundant `keep` hints (the scheduler would not free these anyway):");
        for k in &report.lifetime.redundant_keeps {
            println!("    keep {};", k);
        }
    }
    println!("=== end memcheck ===");
}

/// One field's scheduling decision as a single line: freed after a txn,
/// sealed by an init bound, or "lives for the program".
/// 2026-08-22 (Phase 9): shared by `memcheck` output and the `.s`
/// verification report (analysis::strict::render_report) — one wording.
pub fn field_decision_line(report: &MemcheckReport, f: &str) -> String {
    let scheduled: Vec<&String> = report
        .lifetime
        .free_after
        .iter()
        .filter_map(|(txn, fields)| {
            if fields.iter().any(|x| x == f) {
                Some(txn)
            } else {
                None
            }
        })
        .collect();
    if !scheduled.is_empty() {
        let txn_names: Vec<&str> = scheduled.iter().map(|t| t.as_str()).collect();
        format!("  {}: freed after {}", f, txn_names.join(", "))
    } else if report.sealed_fields.iter().any(|x| x == f) {
        // 2026-08-09 (init kind, Phase 2): an init-bound pool is sealed —
        // capacity is the bound-set max, provably inexhaustible. Not a leak.
        format!("  {}: sealed (capacity bound by an init — provably inexhaustible)", f)
    } else {
        format!("  {}: lives for the program (unprovable — potential leak; add `free`/`keep` or a refcount)", f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Statement, TopLevel};

    fn parse_program(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    /// 2026-08-09 (init kind, Phase 2): a heap-backed pool whose capacity is
    /// bound by a runtime-seeded init is reported sealed (provably
    /// inexhaustible), not "lives for the program (unprovable)".
    #[test]
    fn init_bound_pool_is_sealed() {
        let items = parse_program(
            "init PoolCap: Int = 64;\n\
             let pool: Blob = Malloc#(PoolCap);\n\
             let done: Bool = false;\n\
             node go [done == false][done == true] { done = true; term; };\n",
        );
        let report = run_memcheck(&items);
        assert!(
            report.sealed_fields.contains(&"pool".to_string()),
            "init-bound pool must be sealed, got sealed={:?} fields={:?}",
            report.sealed_fields,
            report.field_names
        );
    }

    /// A non-init heap field stays unsealed (falls through to the normal
    /// "lives for the program" or scheduled report).
    #[test]
    fn non_init_heap_field_is_not_sealed() {
        let items = parse_program(
            "let pool: Blob = Malloc#(100);\n\
             let done: Bool = false;\n\
             node go [done == false][done == true] { done = true; term; };\n",
        );
        let report = run_memcheck(&items);
        assert!(
            !report.sealed_fields.contains(&"pool".to_string()),
            "a literal-bounded pool is not init-sealed, got sealed={:?}",
            report.sealed_fields
        );
    }
}
