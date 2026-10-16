// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Lifecycle laws + termination-reachability analysis (2026-10-15, plan
//! docs/plans/2026-10-15-lifecycle-laws-termination-warning.md).
//!
//! A top-level lifecycle law states the program's termination policy:
//!
//! - `when G { endprogram; };` — the law `G ⟹ ended`. Lowered to a
//!   synthetic reactive monitor txn whose body is `when G { endprogram; }`:
//!   the reactor evaluates `G` every tick and ends the process when it
//!   holds. Cost class = any reactive txn watching `G`.
//! - `when G { reside; };` — open-ended residency intent. Compile-time
//!   only (residency IS the equilibrium behavior); suppresses the
//!   no-terminator warning while `G` is satisfiable. The when-law pass
//!   errors on unsatisfiable guards, so suppression inherits that proof.
//!
//! The termination analysis is a MAY-reachability walk over the SAME live
//! set defn-liveness computes (reactive txns, beginprogram/bootstrap
//! nodes, ISR handlers, and their transitive callees): the program is
//! silent iff some live body reaches `endprogram`, `halt`, or `trap`, or
//! a reside law declares intent. Otherwise the program runs until the
//! process is killed — legal (stalling stays possible) but warned.

use crate::ast::{
    Contract, Expr, LifeCycleArm, Statement, TopLevel, Transaction,
};
use std::collections::HashSet;

/// Lower `when G { endprogram; }` laws into synthetic reactive monitor
/// txns (`__briev_lifecycle_N`, `__` = internal-symbol convention). Runs
/// on BOTH pipeline paths (build + check) before typecheck, so the
/// monitor typechecks, analyses, and emits exactly like an authored txn.
/// `Reside` laws are left in place — the termination pass reads them.
pub fn lower_lifecycle_laws(items: &mut Vec<TopLevel>) {
    let mut monitors: Vec<TopLevel> = Vec::new();
    let mut counter = 0usize;
    for item in items.iter_mut() {
        let TopLevel::WhenLaw(w) = item else { continue };
        if w.lifecycle != Some(LifeCycleArm::EndProgram) {
            continue;
        }
        let guard = w.guard.clone();
        monitors.push(TopLevel::Transaction(Transaction {
            name: format!("__briev_lifecycle_{counter}"),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
                post_authority: false,
            },
            body: vec![Statement::Guarded(
                guard,
                vec![Statement::EndProgram(None)],
            )],
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            variadic_param: None,
            span: None,
            doc: Some(
                "2026-10-15 (lifecycle laws): lowered from `when G { endprogram; }` \
                 — the reactor monitors G and ends the process when it holds."
                    .to_string(),
            ),
        }));
        counter += 1;
    }
    items.extend(monitors);
}

/// The termination verdict: does any live body reach a process terminator,
/// and did a reside law declare open-ended residency?
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminationReport {
    pub has_terminator: bool,
    pub has_reside: bool,
}

/// May-reachability (sound in the silent direction) over the defn-liveness
/// live set: reactive txns / beginprogram / bootstrap nodes / ISR handlers
/// are roots, and the set already includes their transitive callees (bodies
/// AND contracts — PR #18's member-body multimap). `live` is passed in so
/// the backend reuses the one analysis it already computed
/// (frontend-driven dispatch: the pass computes, the caller consumes).
pub fn analyze_termination(
    items: &[TopLevel],
    live: &HashSet<String>,
) -> TerminationReport {
    let has_reside = items.iter().any(|i| {
        matches!(
            i,
            TopLevel::WhenLaw(w) if w.lifecycle == Some(LifeCycleArm::Reside)
        )
    });
    let mut has_terminator = false;
    for item in items {
        match item {
            TopLevel::Transaction(t) => {
                if live.contains(&t.name) && body_reaches_terminator(&t.body) {
                    has_terminator = true;
                }
            }
            TopLevel::Definition(d) => {
                if live.contains(&d.name) && body_reaches_terminator(&d.body) {
                    has_terminator = true;
                }
            }
            // ISR handlers are machine-vectored: always live regardless of
            // the software call graph.
            TopLevel::IsrHandler(i) => {
                if body_reaches_terminator(&i.body) {
                    has_terminator = true;
                }
            }
            _ => {}
        }
    }
    TerminationReport {
        has_terminator,
        has_reside,
    }
}

/// The house termination warning text: what, proof, fix (errors.rs house
/// style lives in the caller — this is the payload).
pub fn termination_warning(report: &TerminationReport) -> Option<crate::diagnostics::Block> {
    if report.has_terminator || report.has_reside {
        return None;
    }
    Some(
        crate::diagnostics::Block::warning(
            "no path ends this program — it runs until the process is killed",
        )
        .why(
            "walked the live program (reactive txns, entry nodes, ISR handlers, \
             and their callees); no endprogram, halt, or trap is reachable",
        )
        .fix(
            "end it where it settles — `when <done> { endprogram; };` — or \
             declare open-ended residency: `when true { reside; };`",
        ),
    )
}

/// Does any statement in this body (recursively through the structural
/// forms) end the process? Contract expressions cannot contain statements,
/// so bodies only.
fn body_reaches_terminator(body: &[Statement]) -> bool {
    body.iter().any(stmt_reaches_terminator)
}

fn stmt_reaches_terminator(s: &Statement) -> bool {
    match s {
        Statement::EndProgram(_) | Statement::Halt | Statement::Trap => true,
        Statement::Guarded(_, b) | Statement::Block(b) | Statement::SyncBlock(b)
        | Statement::Mutex(b) => body_reaches_terminator(b),
        Statement::Foreach { body, .. } => body_reaches_terminator(body),
        Statement::Defer(b) => body_reaches_terminator(b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::top::WhenLawDecl;

    /// Parse real Briev source (behavioral tests — AGENTS: test the
    /// contract, not the implementation). Same helper shape as the other
    /// analysis-pass test modules.
    fn parse(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    fn law(guard: Expr, arm: LifeCycleArm) -> TopLevel {
        TopLevel::WhenLaw(WhenLawDecl {
            guard,
            facts: vec![],
            lifecycle: Some(arm),
            span: None,
        })
    }

    #[test]
    fn endprogram_law_lowers_to_reactive_monitor() {
        let mut items = parse(
            "let n: Int = 0;\nwhen n == 1 { endprogram; };\nnode main [n == 0][n == 1] { n = 1; term; };\n",
        );
        lower_lifecycle_laws(&mut items);
        let monitors: Vec<&Transaction> = items
            .iter()
            .filter_map(|i| match i {
                TopLevel::Transaction(t) if t.name.starts_with("__briev_lifecycle") => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(monitors.len(), 1, "one law → one monitor");
        let m = monitors[0];
        assert!(m.is_reactive, "the monitor must be a reactive txn (liveness root)");
        assert!(
            matches!(m.body.as_slice(), [Statement::Guarded(g, body)]
                if matches!(body.as_slice(), [Statement::EndProgram(None)])),
            "monitor body = `when G {{ endprogram; }}`"
        );
        // The law stays behind with its arm intact (the termination pass
        // and the when-law pass read it).
        assert!(
            items.iter().any(|i| matches!(i, TopLevel::WhenLaw(w)
                if w.lifecycle == Some(LifeCycleArm::EndProgram))),
            "the law item itself remains"
        );
    }

    #[test]
    fn zombie_program_warns_endprogram_and_reside_silence() {
        // A settled node with no terminator: warn.
        let zombie = parse(
            "let n: Int = 0;\nnode main [n == 0][n == 1] { n = 1; term; };\n",
        );
        let live = crate::analysis::defn_liveness::DefnLiveness::build(&zombie)
            .live;
        let r = analyze_termination(&zombie, &live);
        assert!(!r.has_terminator && !r.has_reside);
        assert!(termination_warning(&r).is_some(), "zombie warns");

        // + a reside law: silent (declared intent).
        let mut resident = parse(
            "let n: Int = 0;\nnode main [n == 0][n == 1] { n = 1; term; };\n",
        );
        resident.insert(0, law(Expr::Bool(true), LifeCycleArm::Reside));
        let live = crate::analysis::defn_liveness::DefnLiveness::build(&resident).live;
        let r = analyze_termination(&resident, &live);
        assert!(r.has_reside && termination_warning(&r).is_none());

        // + an endprogram LAW: after lowering, the monitor txn is a liveness
        // root whose body ends the process — silent.
        let mut ending = parse(
            "let n: Int = 0;\nnode main [n == 0][n == 1] { n = 1; term; };\n",
        );
        ending.insert(0, law(Expr::Bool(true), LifeCycleArm::EndProgram));
        lower_lifecycle_laws(&mut ending);
        let live = crate::analysis::defn_liveness::DefnLiveness::build(&ending).live;
        let r = analyze_termination(&ending, &live);
        assert!(r.has_terminator && termination_warning(&r).is_none());
    }

    #[test]
    fn dead_defn_endprogram_does_not_silence() {
        // Reachability matters: an endprogram inside a defn nobody calls is
        // NOT a termination path.
        let items = parse(
            "let n: Int = 0;\nnode main [n == 0][n == 1] { n = 1; term; };\ndefn gone() -> Void { term; };\n",
        );
        let live = crate::analysis::defn_liveness::DefnLiveness::build(&items).live;
        assert!(!live.contains("gone"), "the dead defn is not live");
        let r = analyze_termination(&items, &live);
        assert!(termination_warning(&r).is_some(), "dead terminator ≠ path");
    }
}
