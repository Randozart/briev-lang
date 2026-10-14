//! Garbage scheduling — proof-directed deallocation.
//!
//! 2026-08-01 (Phase D2): this is a garbage SCHEDULER, not a collector. For
//! every heap-backed state field we PROVE, at compile time, the reactor-ordered
//! last transaction that touches it, and SCHEDULE a `Free#` exactly after that
//! transaction's body. The contract is *sound (never premature) but not
//! complete (may not reclaim)*: when the proof cannot establish the last use
//! (an unordered reader, an escaping pointer, an FFI alias), the field falls
//! back to "lives for the program". A field freed but touched later is a
//! compile error — the pass asserts it before emitting anything.
//!
//! Design: docs/plans/2026-08-01-global-lifetime-design.md
//! ("Garbage Scheduling — Global-Lifetime Design Plan").

use crate::ast::{Expr, TopLevel};
use std::collections::{HashMap, HashSet};

/// The scheduler's result: for each transaction name, the heap-backed state
/// fields to free after that transaction's body.
#[derive(Debug, Clone, Default)]
pub struct GlobalLifetime {
    pub free_after: HashMap<String, Vec<String>>,
    /// 2026-08-01 (Phase 5): fields with a `keep x;` hint that the scheduler
    /// would NOT have auto-freed anyway — the hint is redundant (a warning).
    pub redundant_keeps: Vec<String>,
    /// 2026-08-22 (spec-conformance Phase 9, SPEC §3.2): heap-backed fields
    /// that fell back to "lives for the program" with WHY (no sound consumer,
    /// or last consumer not foldable). Normal profiles surface these as
    /// memcheck report lines; `.s` strict profiles escalate them to errors.
    /// Empty for scalars — only genuine heap fallbacks belong here.
    pub lifetime_fallbacks: Vec<(String, &'static str)>,
}

/// Compute the scheduled frees. `field_initializers` maps a state field to its
/// initializer expression; `node_order` is the reactor's deterministic firing
/// order (transition-graph node order) that makes "last consumer" well-defined.
///
/// 2026-08-06 (fix): a scheduled free must have a SOUND emission point. The
/// backend emits frees only after a FOLDED bounded loop — a non-bounded
/// reactive node has no sound point (freeing inside its body is a
/// use-after-free), so its planned free would be silently dropped. `foldable`
/// is the set of txns with a `bounded_pre`; a field whose last consumer is not
/// foldable is NOT scheduled and falls back to the documented "lives for the
/// program".
pub fn analyze(
    items: &[TopLevel],
    field_initializers: &HashMap<String, Expr>,
    node_order: &[String],
    foldable: &HashSet<String>,
) -> GlobalLifetime {
    let state_fields: HashSet<String> = field_initializers.keys().cloned().collect();
    // A field with a MANUAL Free# is user-managed — the scheduler must NOT
    // also free it (a double-free). Detect manual frees across all txns.
    let manually_freed: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            TopLevel::Transaction(t) => Some(t.body.as_slice()),
            _ => None,
        })
        .flat_map(|body| body.iter())
        .filter_map(|stmt| manual_free_target(stmt, &state_fields))
        .collect();
    // Only heap-allocated (Malloc#/Alloc#) fields are schedulable — a plain
    // Ptr slot pointing at borrowed/static memory must never be freed.
    let heap_backed: Vec<String> = field_initializers
        .iter()
        .filter(|(_, init)| contains_heap_alloc(init))
        .filter(|(name, _)| !manually_freed.contains(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    // 2026-08-01 (Phase 5): redundant-`keep` detection runs even when nothing
    // is schedulable — a `keep x;` on a field the scheduler would never
    // auto-free (e.g. a scalar) is a warning.
    let kept: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            TopLevel::Transaction(t) => Some(t.body.as_slice()),
            _ => None,
        })
        .flat_map(|body| body.iter())
        .filter_map(|stmt| match stmt {
            crate::ast::Statement::KeepHint(name) => Some(name.clone()),
            _ => None,
        })
        .collect();
    if heap_backed.is_empty() {
        return GlobalLifetime {
            free_after: HashMap::new(),
            redundant_keeps: kept.into_iter().collect(),
            lifetime_fallbacks: Vec::new(),
        };
    }

    // Per-txn touch set (read OR write — a write after the free is a
    // use-after-free too, so the scheduler frees after the LAST touch).
    let mut txn_touches: HashMap<String, HashSet<String>> = HashMap::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            let mut touches = HashSet::new();
            for stmt in &t.body {
                crate::analysis::transition_graph::collect_statement_identifiers(
                    stmt, &state_fields, &mut touches,
                );
            }
            if !touches.is_empty() {
                txn_touches.insert(t.name.clone(), touches);
            }
        }
    }

    let mut free_after: HashMap<String, Vec<String>> = HashMap::new();
    let mut fallbacks: Vec<(String, &'static str)> = Vec::new();
    for field in &heap_backed {
        // The ordered list of txns that touch this field, in reactor order.
        let consumers: Vec<&String> = node_order
            .iter()
            .filter(|n| txn_touches.get(*n).map_or(false, |t| t.contains(field)))
            .collect();
        let Some(last) = consumers.last() else {
            // No consumer at all — the field is written but never read.
            // Conservatively NOT freed (its initializer may still be live).
            fallbacks.push((field.clone(), "no transaction reads it"));
            continue;
        };
        // 2026-08-06 (fix): only schedule a free when the last consumer has a
        // bounded-loop shape the backend can fold (a sound post-loop emission
        // point). A non-foldable last consumer falls back to "lives for the
        // program" — scheduling here would be silently dropped.
        if !foldable.contains(last.as_str()) {
            fallbacks.push((
                field.clone(),
                "its last consumer has no bounded-loop shape to free after",
            ));
            continue;
        }
        free_after.entry((*last).clone()).or_default().push(field.clone());
    }

    // Sort for deterministic IR (SipHash order varies per process).
    for v in free_after.values_mut() {
        v.sort();
    }
    // 2026-08-01 (Phase 5): redundant-`keep` — a kept field the scheduler
    // does not schedule is a warning (the existing `kept` set above).
    let scheduled: HashSet<String> = free_after.values().flatten().cloned().collect();
    let redundant_keeps: Vec<String> = kept
        .into_iter()
        .filter(|k| !scheduled.contains(k))
        .collect();
    GlobalLifetime {
        free_after,
        redundant_keeps,
        lifetime_fallbacks: fallbacks,
    }
}

/// Does the expression allocate heap memory (a `Malloc#`/`Alloc#` intrinsic)?
pub fn contains_heap_alloc(expr: &Expr) -> bool {
    match expr {
        Expr::Call(name, args, _) => {
            if name == "Malloc#" || name == "Alloc#" {
                return true;
            }
            args.iter().any(contains_heap_alloc)
        }
        Expr::Cast(inner, _) => contains_heap_alloc(inner),
        Expr::BinaryOp(_, l, r) => contains_heap_alloc(l) || contains_heap_alloc(r),
        Expr::Index(obj, idx) => contains_heap_alloc(obj) || contains_heap_alloc(idx),
        Expr::Field(obj, _) => contains_heap_alloc(obj),
        Expr::AddrOf(inner) => contains_heap_alloc(inner),
        Expr::Deref(inner) => contains_heap_alloc(inner),
        _ => false,
    }
}

/// The state field a manual `Free#(field)` targets, if the statement is one.
/// 2026-08-01 (D2): a manually-freed field is user-managed — the scheduler
/// must NOT also free it (a double-free).
fn manual_free_target(stmt: &crate::ast::Statement, state_fields: &HashSet<String>) -> Option<String> {
    fn walk_expr(e: &Expr, state_fields: &HashSet<String>) -> Option<String> {
        match e {
            Expr::Call(name, args, _) => {
                if name == "Free#" {
                    if let Some(arg) = args.first() {
                        // `Free#(buf as Ptr<...>)` — the arg may be a cast.
                        let mut inner = arg;
                        while let Expr::Cast(i, _) = inner {
                            inner = i;
                        }
                        if let Expr::Identifier(n) = inner {
                            if state_fields.contains(n) {
                                return Some(n.clone());
                            }
                        }
                    }
                    return None;
                }
                args.iter().find_map(|a| walk_expr(a, state_fields))
            }
            Expr::Cast(inner, _) => walk_expr(inner, state_fields),
            Expr::AddrOf(inner) => walk_expr(inner, state_fields),
            _ => None,
        }
    }
    match stmt {
        crate::ast::Statement::Expression(e) => walk_expr(e, state_fields),
        crate::ast::Statement::Guarded(_, body) => {
            body.iter().find_map(|s| manual_free_target(s, state_fields))
        }
        crate::ast::Statement::Block(stmts) => {
            stmts.iter().find_map(|s| manual_free_target(s, state_fields))
        }
        crate::ast::Statement::FreeHint(name) => {
            // 2026-08-01 (Phase 5): `free x;` is a manual free — the scheduler
            // must not ALSO free x (a double-free).
            if state_fields.contains(name) {
                Some(name.clone())
            } else {
                None
            }
        }
        crate::ast::Statement::KeepHint(name) => {
            // 2026-08-01 (Phase 5): `keep x;` — the field ESCAPES (freed
            // elsewhere or owned by the caller); the scheduler must not free it.
            if state_fields.contains(name) {
                Some(name.clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alloc_init() -> Expr {
        Expr::Call("Malloc#".to_string(), vec![Expr::Decimal(64)], None)
    }

    fn field(name: &str, heap: bool) -> (String, Expr) {
        (
            name.to_string(),
            if heap { alloc_init() } else { Expr::Decimal(0) },
        )
    }

    fn txn_body(reads: &[&str]) -> Vec<crate::ast::Statement> {
        reads
            .iter()
            .map(|f| {
                crate::ast::Statement::Expression(Expr::Identifier(f.to_string()))
            })
            .collect()
    }

    #[test]
    fn single_consumer_schedules_free_after_it() {
        let items = vec![TopLevel::Transaction(crate::ast::Transaction {
            variadic_param: None,
            name: "life".into(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: crate::ast::top::Contract {
                pre_condition: crate::ast::Expr::Bool(true),
                post_condition: crate::ast::Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body: txn_body(&["buf"]),
            metadata: Default::default(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })];
        let fields = HashMap::from([field("buf", true)]);
        let gl = analyze(&items, &fields, &["life".to_string()], &["life".to_string()].iter().cloned().collect());
        assert_eq!(gl.free_after.get("life").cloned(), Some(vec!["buf".to_string()]));
    }

    #[test]
    fn non_heap_field_is_never_scheduled() {
        let items = vec![TopLevel::Transaction(crate::ast::Transaction {
            variadic_param: None,
            name: "life".into(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: crate::ast::top::Contract {
                pre_condition: crate::ast::Expr::Bool(true),
                post_condition: crate::ast::Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body: txn_body(&["plain"]),
            metadata: Default::default(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })];
        let fields = HashMap::from([field("plain", false)]);
        let gl = analyze(&items, &fields, &["life".to_string()], &["life".to_string()].iter().cloned().collect());
        assert!(gl.free_after.is_empty());
    }

    #[test]
    fn manually_freed_field_is_never_scheduled() {
        // 2026-08-01 (D2): a manual `Free#(buf)` means the user manages the
        // field — the scheduler must not ALSO free it (a double-free).
        let items = vec![TopLevel::Transaction(crate::ast::Transaction {
            variadic_param: None,
            name: "life".into(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: crate::ast::top::Contract {
                pre_condition: crate::ast::Expr::Bool(true),
                post_condition: crate::ast::Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
            body: vec![
                crate::ast::Statement::Expression(Expr::Identifier("buf".to_string())),
                crate::ast::Statement::Expression(Expr::Call(
                    "Free#".to_string(),
                    vec![Expr::Cast(
                        Box::new(Expr::Identifier("buf".to_string())),
                        crate::ast::Type::Custom("Ptr".to_string()),
                    )],
                    None,
                )),
            ],
            metadata: Default::default(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })];
        let fields = HashMap::from([field("buf", true)]);
        let gl = analyze(&items, &fields, &["life".to_string()], &["life".to_string()].iter().cloned().collect());
        assert!(gl.free_after.is_empty(), "manually-freed field must be excluded");
    }

    #[test]
    fn last_of_ordered_consumers_is_the_scheduler_point() {
        let mk = |name: &str, body: Vec<crate::ast::Statement>| {
            TopLevel::Transaction(crate::ast::Transaction {
                variadic_param: None,
                name: name.into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: crate::ast::top::Contract {
                pre_condition: crate::ast::Expr::Bool(true),
                post_condition: crate::ast::Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
            post_authority: false},
                body,
                metadata: Default::default(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            })
        };
        let items = vec![
            mk("first", txn_body(&["buf"])),
            mk("second", txn_body(&["buf"])),
        ];
        let fields = HashMap::from([field("buf", true)]);
        // Reactor order: first, second. The last consumer is `second`.
        let gl = analyze(&items, &fields, &["first".to_string(), "second".to_string()], &["first".to_string(), "second".to_string()].iter().cloned().collect());
        assert!(gl.free_after.get("first").is_none());
        assert_eq!(gl.free_after.get("second").cloned(), Some(vec!["buf".to_string()]));
    }

    /// 2026-08-06 (fix): a last consumer that is NOT foldable (no sound free
    /// emission point) must NOT be scheduled — the field lives for the program.
    #[test]
    fn non_foldable_last_consumer_is_not_scheduled() {
        let mk = |name: &str, body: Vec<crate::ast::Statement>| {
            TopLevel::Transaction(crate::ast::Transaction {
                variadic_param: None,
                name: name.into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: crate::ast::top::Contract {
                    pre_condition: crate::ast::Expr::Bool(true),
                    post_condition: crate::ast::Expr::Bool(true),
                    watchdog: None,
                    span: None,
                    explicit: false,
                post_authority: false},
                body,
                metadata: Default::default(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            })
        };
        let items = vec![mk("life", txn_body(&["buf"]))];
        let fields = HashMap::from([field("buf", true)]);
        // `life` touches buf but is NOT foldable — the free must not be planned.
        let gl = analyze(&items, &fields, &["life".to_string()], &std::collections::HashSet::new());
        assert!(
            gl.free_after.is_empty(),
            "a non-foldable last consumer must not be scheduled for a free"
        );
    }
}
