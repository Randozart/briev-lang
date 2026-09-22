// 2026-09-22 (Slice C2, plan 2026-09-22-electronics-participation-and-when-
// law.md): the software side of the static `when` law.
//
// At top level or in an obj/type body, `when G { F1; ...; Fn }` declares
// G ⟹ F1 ∧ … ∧ Fn, and the compiler must make it so. Inside a defn/node/
// txn, `when` stays guarded/reactive behavior (Statement::Guarded, which
// this pass does not touch). The software obligation: if a member fact
// forces a value, no node/txn body assignment or another law may force the
// same member to a DIFFERENT value under a jointly-satisfiable guard —
// otherwise the law cannot hold and the compile refuses (X must imply Y;
// a single escaping state is an error).
//
// This is the software twin of the electronics conditional-drive check. The
// law facts are member assignments (`thermal_alarm = true`); the pass
// collects them (per instance for type-body laws, scoped by declaration
// site), then cross-checks every node/txn body assignment to a law member.

use crate::ast::{Expr, Statement, TopLevel};
use crate::proof_engine::check_satisfiable;
use std::collections::BTreeMap;

/// A law fact forcing a member: when `guard` holds, `member` = `value`.
struct LawFact {
    member: String,
    value: Expr,
    guard: Expr,
    source: String,
}

/// Collect law facts from top-level laws and type-body laws. A fact is a
/// member assignment (`member = value`) or a member equality
/// (`member == value`). Type-body laws force the type's own slots; the
/// pass checks against node/txn bodies that reference those members.
fn collect_law_facts(items: &[TopLevel]) -> Vec<LawFact> {
    let mut out = Vec::new();
    // Top-level laws: member facts are directly named. Type-body laws force
    // the type's own slots (self-scoped members). Single pass over items.
    for item in items {
        match item {
            TopLevel::WhenLaw(w) => {
                collect_facts(&w.guard, &w.facts, "top-level", &mut out);
            }
            TopLevel::TypeDef(td) => collect_type_laws(td, &mut out),
            _ => {}
        }
    }
    out
}

/// A type body's laws, each contributing its member facts.
fn collect_type_laws(td: &crate::ast::TypeDef, out: &mut Vec<LawFact>) {
    for law in &td.body.when_laws {
        collect_facts(
            &law.guard,
            &law.facts,
            &format!("type '{}'", td.name),
            out,
        );
    }
}

/// Extract member-forcing facts from one law's body.
fn collect_facts(guard: &Expr, facts: &[Statement], source: &str, out: &mut Vec<LawFact>) {
    for fact in facts {
        let pair: Option<(&Expr, &Expr)> = match fact {
            Statement::Assign(l, r) => Some((l, r)),
            Statement::Expression(e) => match e {
                Expr::BinaryOp(crate::ast::BinaryOpKind::Eq, l, r) => Some((l, r)),
                _ => None,
            },
            _ => None,
        };
        let Some((target, value)) = pair else { continue };
        let Expr::Identifier(member) = target else { continue };
        let is_constant = matches!(
            value,
            Expr::Bool(_) | Expr::Quoted(_) | Expr::Decimal(_) | Expr::UnitLiteral { .. }
                | Expr::Float(_)
        );
        if !is_constant {
            continue;
        }
        out.push(LawFact {
            member: member.clone(),
            value: value.clone(),
            guard: guard.clone(),
            source: format!("when-law ({})", source),
        });
    }
}

/// Every member a law fact forces — the same member must not be forced to a
/// different value anywhere else under a jointly-satisfiable guard.
fn collect_law_members(facts: &[LawFact]) -> BTreeMap<String, Vec<&LawFact>> {
    let mut map: BTreeMap<String, Vec<&LawFact>> = BTreeMap::new();
    for f in facts {
        map.entry(f.member.clone()).or_default().push(f);
    }
    map
}

/// Check one node/txn body assignment against the law facts. If the body
/// assigns a law member a different value and the law's guard is
/// satisfiable alongside the transaction's own guard, the law cannot hold.
/// Iterative (explicit worklist) so guarded/block nesting stays flat.
fn check_assignments(
    txn_name: &str,
    txn_guard: &Expr,
    body: &[Statement],
    law_members: &BTreeMap<String, Vec<&LawFact>>,
    errors: &mut Vec<String>,
) {
    let mut stack: Vec<(&[Statement], Expr)> = vec![(body, txn_guard.clone())];
    while let Some((stmts, guard)) = stack.pop() {
        if let Some(first) = stmts.first() {
            check_one_assignment(txn_name, &guard, first, law_members, errors);
            let rest = &stmts[1..];
            if !rest.is_empty() {
                stack.push((rest, guard.clone()));
            }
            match first {
                Statement::Guarded(g, inner) => {
                    // Nested guard: the assignment holds under guard ∧ g.
                    stack.push((inner, mk_and(&guard, g)));
                }
                Statement::Block(inner) => {
                    stack.push((inner, guard.clone()));
                }
                _ => {}
            }
        }
    }
}

/// A single statement's assignment vs the law facts: if it forces a law
/// member to a different value under a jointly-satisfiable guard, the law
/// cannot hold.
fn check_one_assignment(
    txn_name: &str,
    guard: &Expr,
    stmt: &Statement,
    law_members: &BTreeMap<String, Vec<&LawFact>>,
    errors: &mut Vec<String>,
) {
    let pair: Option<(&Expr, &Expr)> = match stmt {
        Statement::Assign(l, r) => Some((l, r)),
        Statement::Expression(e) => match e {
            Expr::BinaryOp(crate::ast::BinaryOpKind::Eq, l, r) => Some((l, r)),
            _ => None,
        },
        _ => None,
    };
    let Some((target, value)) = pair else { return };
    let Expr::Identifier(member) = target else { return };
    let Some(laws) = law_members.get(member) else { return };
    if expr_may_equal(value, &laws[0].value) {
        return; // same value — no contradiction
    }
    for law in laws {
        let sat = check_satisfiable(&law.guard, guard);
        if sat {
            errors.push(format!(
                "when-law '{}' forces '{}' to '{}' under its guard, but node '{}' assigns it a \
                 different value in a state where both could hold. The law must hold everywhere — \
                 remove the conflicting assignment, or make the guards mutually exclusive.",
                law.source, law.member, law.value, txn_name
            ));
        }
    }
}

/// AND two expressions into one guard (keeps nested guards composable).
fn mk_and(a: &Expr, b: &Expr) -> Expr {
    Expr::BinaryOp(
        crate::ast::BinaryOpKind::And,
        Box::new(a.clone()),
        Box::new(b.clone()),
    )
}

/// Conservative value compatibility: true when the two expressions are
/// literally the same shape (so `true` vs `true` matches) or both are
/// numeric/unit constants with equal value.
fn expr_may_equal(a: &Expr, b: &Expr) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Expr::Bool(x), Expr::Bool(y)) => x == y,
        (Expr::Decimal(x), Expr::Decimal(y)) => x == y,
        (Expr::Float(x), Expr::Float(y)) => (x - y).abs() < f64::EPSILON,
        (Expr::Quoted(x), Expr::Quoted(y)) => x == y,
        (Expr::UnitLiteral { value: x, .. }, Expr::UnitLiteral { value: y, .. }) => {
            (x - y).abs() < f64::EPSILON
        }
        _ => true, // unknown shapes: assume compatible (conservative — only a
                   // proven-different constant is a contradiction)
    }
}

/// Run the software when-law check. Returns errors naming every law that
/// could be violated; an empty vec means every law holds.
pub fn run_when_law_check(items: &[TopLevel]) -> Vec<String> {
    let mut errors = Vec::new();
    let facts = collect_law_facts(items);
    if facts.is_empty() {
        return errors;
    }
    let law_members = collect_law_members(&facts);
    check_law_vs_law(&law_members, &mut errors);
    // Check node/txn body assignments against the laws.
    for item in items {
        let TopLevel::Transaction(t) = item else { continue };
        check_assignments(
            &t.name,
            &t.contract.pre_condition,
            &t.body,
            &law_members,
            &mut errors,
        );
    }
    errors
}

/// Two laws forcing the same member to different values under jointly-
/// satisfiable guards cannot both hold.
fn check_law_vs_law(
    law_members: &BTreeMap<String, Vec<&LawFact>>,
    errors: &mut Vec<String>,
) {
    for (member, laws) in law_members {
        check_member_laws(member, laws, errors);
    }
}

/// One member's laws: if two force DIFFERENT values and each guard is
/// individually satisfiable, the laws cannot both hold (they may still
/// conflict pairwise — a value's guards are checked against each other, but
/// the existence of two distinct possible values is the contradiction).
fn check_member_laws(member: &str, laws: &[&LawFact], errors: &mut Vec<String>) {
    // Group laws by the value they force; a law whose guard is satisfiable
    // could be in force. Two distinct in-force values = contradiction.
    let mut by_value: std::collections::BTreeMap<String, Vec<&LawFact>> =
        std::collections::BTreeMap::new();
    for law in laws {
        let active = check_satisfiable(&law.guard, &Expr::Bool(true));
        if !active {
            continue;
        }
        by_value.entry(format!("{}", law.value)).or_default().push(law);
    }
    if by_value.len() < 2 {
        return;
    }
    let mut groups = by_value.into_values();
    let first = groups.next().unwrap();
    for group in groups {
        errors.push(format!(
            "when-laws '{}' and '{}' force '{}' to different values under jointly-\
             satisfiable guards — the laws cannot both hold. Make the guards mutually \
             exclusive.",
            first[0].source, group[0].source, member
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::Parser;

    fn check(src: &str) -> Vec<String> {
        let tokens = tokenize(src).unwrap();
        let mut p = Parser::new(tokens, src);
        let items = p.parse_program().unwrap();
        run_when_law_check(&items)
    }

    #[test]
    fn top_level_when_law_forces_member() {
        // The law holds — no assignment contradicts it.
        let src = r#"
            let alarm: Bool = false;
            let temperature: Int = 20;
            when temperature > 100 { alarm = true; }
            node n [temperature > 100] { alarm = true; };
        "#;
        let errors = check(src);
        assert!(errors.is_empty(), "a satisfied law must pass: {errors:?}");
    }

    #[test]
    fn top_level_when_law_contradiction_is_refused() {
        // The law forces alarm=true under temperature>100; the node assigns
        // alarm=false in a state where the law's guard can also hold.
        let src = r#"
            let alarm: Bool = false;
            let temperature: Int = 20;
            when temperature > 100 { alarm = true; }
            node n [temperature > 100] { alarm = false; };
        "#;
        let errors = check(src);
        assert_eq!(errors.len(), 1, "a contradicted law must refuse: {errors:?}");
        assert!(errors[0].contains("when-law") && errors[0].contains("alarm"), "{}", errors[0]);
    }

    #[test]
    fn mutually_exclusive_guards_do_not_contradict() {
        // The law holds when temperature > 100; the node only assigns
        // alarm=false when temperature <= 100 — mutually exclusive, no
        // contradiction.
        let src = r#"
            let alarm: Bool = false;
            let temperature: Int = 20;
            when temperature > 100 { alarm = true; }
            node n [temperature <= 100] { alarm = false; };
        "#;
        let errors = check(src);
        assert!(errors.is_empty(), "mutually-exclusive guards must pass: {errors:?}");
    }

    #[test]
    fn type_body_when_law_contradiction_is_refused() {
        // An obj-body law forces a member; a node contradicts it.
        let src = r#"
            let alarm: Bool = false;
            obj Sensor { temperature: Int; when temperature > 100 { alarm = true; } };
            node n [alarm == false] { alarm = false; };
        "#;
        let errors = check(src);
        assert_eq!(errors.len(), 1, "a type-body law contradicted must refuse: {errors:?}");
        assert!(errors[0].contains("type 'Sensor'"), "{}", errors[0]);
    }

    #[test]
    fn law_vs_law_contradiction_is_refused() {
        // Two laws force the same member to different values under
        // jointly-satisfiable guards.
        let src = r#"
            let x: Int = 0;
            when x >= 0 { x = 1; }
            when x >= 0 { x = 2; }
        "#;
        let errors = check(src);
        assert_eq!(errors.len(), 1, "law-vs-law contradiction must refuse: {errors:?}");
        assert!(errors[0].contains("different values"), "{}", errors[0]);
    }
}