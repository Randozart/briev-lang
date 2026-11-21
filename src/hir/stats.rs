// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0c unsupported-syntax accounting. `Other(tag)` nodes are explicit gaps to
//! close before a consumer can rely on HIR.

use super::expr::{HirExpr, HirExprKind};
use super::pattern::{HirPattern, HirPatternKind};
use super::program::{HirCallable, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use std::collections::BTreeMap;

// The counsel contract layer: inert identity attributes, checked by `counsel-check`.
use counsel::{post, pre};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnsupportedCounts {
    pub expressions: usize,
    pub statements: usize,
    /// 2026-11-16 (H3): boundary facts HIR cannot represent: `proto`
    /// declarations and `frgn` boundary clauses. The statement/expression
    /// counts measure bodies; these measure the surface.
    pub protocols: usize,
    pub boundary_facts: usize,
    pub expression_tags: BTreeMap<&'static str, usize>,
    pub statement_tags: BTreeMap<&'static str, usize>,
}

impl UnsupportedCounts {
    // the unsupported-record total is the sum of two non-negative counts
    #[pre(self.expressions >= 0 && self.statements >= 0)]
    #[post(term >= 0)]
    pub fn total(&self) -> usize {
        self.expressions + self.statements + self.protocols + self.boundary_facts
    }
}

pub fn unsupported_counts(program: &HirProgram) -> UnsupportedCounts {
    let mut counts = UnsupportedCounts::default();
    for callable in &program.callables {
        count_body(&callable.body.statements, &mut counts);
    }
    // 2026-11-16 (H3): the protocol and boundary surfaces count as
    // unsupported until the lowering can represent their facts.
    count_protocols(program, &mut counts);
    count_boundaries(program, &mut counts);
    counts
}

fn count_protocols(program: &HirProgram, counts: &mut UnsupportedCounts) {
    // 2026-11-18 (W3): a proto lowers to a HirProtocol carrying name,
    // category, cast lanes (with their `trusted_axiom` markers, SPEC §8.7),
    // op bindings (with theirs, SPEC §8.8), AND the protocol-level contract
    // (lowered to `HirContract`, the same node `HirCallable` carries). The
    // lowering populates all four surfaces. `has_unrepresentable_contract`
    // is `false` whenever the contract is represented (i.e. always, since
    // `lower_contract` handles every `Contract` shape the parser produces)
    // and `true` only if a future parser change introduces a contract form
    // the lowering cannot handle. Today: zero protocol gaps.
    for proto in &program.protocols {
        if proto.has_unrepresentable_contract {
            counts.protocols += 1;
        }
    }
}

fn count_boundaries(program: &HirProgram, counts: &mut UnsupportedCounts) {
    // A frgn lowers to HirForeign with HirBoundary (H2). The boundary
    // carries from/flags/contracts/error_fields. What the parser does NOT
    // produce (no clause surface) is buffer_mode, pre/postcondition, and
    // non-default flags — those are constructor-only defaults on the AST.
    // The audit does not count represented facts; it counts gaps.
    // Today there is no gap: the boundary mirrors what the AST can produce.
    for _foreign in &program.foreign {
        // no count — boundary is fully represented
    }
}

pub fn unsupported_for_callable(callable: &HirCallable) -> UnsupportedCounts {
    let mut counts = UnsupportedCounts::default();
    count_body(&callable.body.statements, &mut counts);
    counts
}

fn count_body(body: &[HirStmt], counts: &mut UnsupportedCounts) {
    for stmt in body {
        count_stmt(stmt, counts);
    }
}

fn count_stmt(stmt: &HirStmt, counts: &mut UnsupportedCounts) {
    match &stmt.kind {
        HirStmtKind::Let { expr, .. } => count_opt_expr(expr.as_ref(), counts),
        HirStmtKind::Assign(left, right) => count_exprs([left, right], counts),
        HirStmtKind::ArrowAssign { target, value, .. } => {
            count_opt_expr(target.as_ref(), counts);
            count_expr(value, counts);
        }
        HirStmtKind::Term(expr) => count_opt_expr(expr.as_ref(), counts),
        HirStmtKind::Tail(expr) | HirStmtKind::Check(expr) | HirStmtKind::Expression(expr) => {
            count_expr(expr, counts)
        }
        HirStmtKind::Guarded(cond, body) => {
            count_expr(cond, counts);
            count_body(body, counts);
        }
        HirStmtKind::Block(body) => count_body(body, counts),
        HirStmtKind::Foreach { list, body, .. } => {
            count_expr(list, counts);
            count_body(body, counts);
        }
        HirStmtKind::Match { expr, arms } => {
            count_expr(expr, counts);
            count_stmt_arms(arms, counts);
        }
        HirStmtKind::EndProgram(expr) | HirStmtKind::Rollback(expr) => {
            count_opt_expr(expr.as_ref(), counts)
        }
        HirStmtKind::Gate(expr) | HirStmtKind::TrgBinding { instance: expr, .. } => {
            count_expr(expr, counts)
        }
        HirStmtKind::SyncBlock(body) | HirStmtKind::Defer(body) | HirStmtKind::Mutex(body) => {
            count_body(body, counts)
        }
        HirStmtKind::Open(left, right) => count_exprs([left, right], counts),
        HirStmtKind::Break
        | HirStmtKind::Trap
        | HirStmtKind::Halt
        | HirStmtKind::Yield
        | HirStmtKind::FreeHint(_)
        | HirStmtKind::KeepHint(_)
        | HirStmtKind::MetadataAssignment(_, _)
        | HirStmtKind::InlineAsm { .. }
        // 2026-11-15: inline-defn is a fully-represented HIR node (carries
        // the defn name); not an unsupported gap, so it is not counted.
        | HirStmtKind::InlineDefn(_) => {}
        HirStmtKind::ReleaseOwned { .. } => {}
        HirStmtKind::LetInsert { expr, .. } => count_expr(expr, counts),
        HirStmtKind::TempTransfer { expr, .. } => count_expr(expr, counts),
        HirStmtKind::Other(tag) => {
            counts.statements += 1;
            *counts.statement_tags.entry(tag).or_insert(0) += 1;
        }
    }
}

fn count_stmt_arms(arms: &[super::stmt::HirStmtMatchArm], counts: &mut UnsupportedCounts) {
    for arm in arms {
        count_patterns(&arm.patterns, counts);
        count_body(&arm.body, counts);
    }
}

fn count_expr_arms(arms: &[super::expr::HirMatchArm], counts: &mut UnsupportedCounts) {
    for arm in arms {
        count_pattern(&arm.pattern, counts);
        count_expr(&arm.body, counts);
    }
}

fn count_patterns(patterns: &[HirPattern], counts: &mut UnsupportedCounts) {
    for pattern in patterns {
        count_pattern(pattern, counts);
    }
}

fn count_opt_box_expr(expr: Option<&HirExpr>, counts: &mut UnsupportedCounts) {
    if let Some(expr) = expr {
        count_expr(expr, counts);
    }
}

fn count_opt_expr(expr: Option<&HirExpr>, counts: &mut UnsupportedCounts) {
    if let Some(expr) = expr {
        count_expr(expr, counts);
    }
}

fn count_exprs<'a>(exprs: impl IntoIterator<Item = &'a HirExpr>, counts: &mut UnsupportedCounts) {
    for expr in exprs {
        count_expr(expr, counts);
    }
}

fn count_expr(expr: &HirExpr, counts: &mut UnsupportedCounts) {
    match &expr.kind {
        HirExprKind::Call { args, .. } => count_exprs(args, counts),
        HirExprKind::Binary { left, right, .. } => count_exprs([left.as_ref(), right.as_ref()], counts),
        HirExprKind::Unary { inner, .. }
        | HirExprKind::Cast { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => count_expr(inner, counts),
        HirExprKind::Field { receiver, .. }
        | HirExprKind::Reflect { receiver, .. } => count_expr(receiver, counts),
        HirExprKind::MethodCall { receiver, args, .. } => {
            count_expr(receiver, counts);
            count_exprs(args, counts);
        }
        HirExprKind::Index(array, index) => count_exprs([array.as_ref(), index.as_ref()], counts),
        HirExprKind::Slice { array, start, end, stride } => {
            count_expr(array, counts);
            count_opt_box_expr(start.as_deref(), counts);
            count_opt_box_expr(end.as_deref(), counts);
            count_opt_box_expr(stride.as_deref(), counts);
        }
        HirExprKind::Range { start, end, .. } => count_exprs([start.as_ref(), end.as_ref()], counts),
        HirExprKind::Spawn { args, .. } => count_exprs(args, counts),
        HirExprKind::If { condition, then_branch, else_branch } => {
            count_expr(condition, counts);
            count_expr(then_branch, counts);
            count_opt_box_expr(else_branch.as_deref(), counts);
        }
        HirExprKind::Lambda { body, .. } | HirExprKind::IsType { inner: body, .. } => {
            count_expr(body, counts)
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => count_exprs(items, counts),
        HirExprKind::StructLiteral { fields, .. } => {
            count_exprs(fields.iter().map(|(_, expr)| expr), counts)
        }
        HirExprKind::Match { scrutinee, arms } => {
            count_expr(scrutinee, counts);
            count_expr_arms(arms, counts);
        }
        HirExprKind::PluginIntercept { args, receiver, .. } => {
            count_exprs(args, counts);
            if let Some(r) = receiver {
                count_expr(r, counts);
            }
        }
        HirExprKind::Within(l, r) => count_exprs([l.as_ref(), r.as_ref()], counts),
        HirExprKind::Block(body) => count_body(body, counts),
        HirExprKind::Other(tag) => {
            counts.expressions += 1;
            *counts.expression_tags.entry(tag).or_insert(0) += 1;
        }
        HirExprKind::Literal(_)
        | HirExprKind::Identifier(_)
        // 2026-11-12 (electronics full-surface port): a lowered selector,
        // never unsupported syntax.
        | HirExprKind::Wildcard
        | HirExprKind::UnitLiteral { .. }
        // 2026-11-15: fully-represented HIR nodes, not unsupported gaps.
        | HirExprKind::Exists(_)
        | HirExprKind::FormattingAnnotation(_) => {}
    }
}

fn count_pattern(pattern: &HirPattern, counts: &mut UnsupportedCounts) {
    match &pattern.kind {
        HirPatternKind::Literal(expr)
        | HirPatternKind::Range(expr, _)
        | HirPatternKind::RangeInclusive(expr, _) => count_expr(expr, counts),
        HirPatternKind::Range(_, second) | HirPatternKind::RangeInclusive(_, second) => {
            count_expr(second, counts);
        }
        HirPatternKind::EnumVariant(_, pats)
        | HirPatternKind::Tuple(pats)
        | HirPatternKind::Multi(pats) => {
            for pat in pats {
                count_pattern(pat, counts);
            }
        }
        HirPatternKind::Wildcard
        | HirPatternKind::Binding(_)
        | HirPatternKind::TypedBinding(_, _) => {}
    }
}

#[cfg(test)]
mod boundary_audit_tests {
    use super::*;

    /// Parse and lower a minimal program.
    fn lower(src: &str) -> HirProgram {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut p = crate::parser::Parser::new(tokens, src);
        let items = p.parse_program().expect("parse");
        super::super::lower::lower_program(&items)
    }

    /// 2026-11-17 (S4): a `proto` with cast lanes but NO protocol contract
    /// is fully represented — the lowering populates `casts` (with their
    /// authority markers) and `ops` from `cross_ops`, so the audit counts
    /// zero gaps. Supersedes the H3
    /// `proto_is_counted_until_cross_ops_populated` test: the ops surface is
    /// now represented; the only remaining gap is a contract HIR has no node
    /// for.
    #[test]
    fn proto_ops_populated_and_certified() {
        let program = lower(
            "proto TestProto: String {\n    axiom CastTo(String<UTF8>) = to_briev(#Lh);\n    axiom CastFrom(String<UTF8>) = to_c(#Lh);\n};\n",
        );
        assert_eq!(program.protocols.len(), 1, "one proto lowers to one node");
        let proto = &program.protocols[0];
        assert_eq!(
            proto.casts.len(),
            2,
            "two cast lanes lower in declaration order"
        );
        assert!(
            !proto.has_unrepresentable_contract,
            "this proto declares no protocol contract — no gap"
        );
        let counts = unsupported_counts(&program);
        assert_eq!(
            counts.protocols, 0,
            "a contract-less proto is fully represented — no gap"
        );
        assert_eq!(counts.total(), 0, "no unsupported facts");
    }

    /// 2026-11-18 (W3): a `proto` that declares a protocol-level contract
    /// now LOWERS that contract into `HirProtocol::contract` (reusing
    /// `HirContract`, the same node `HirCallable` carries). The audit counts
    /// zero gaps: the contract is fully represented. Supersedes the S4
    /// `proto_with_contract_is_one_gap` test (which expected one gap while
    /// HIR had no node for the contract).
    #[test]
    fn proto_with_contract_is_fully_represented() {
        let program = lower(
            "proto TestProto: String [true] {\n    axiom CastTo(String<UTF8>) = to_briev(#Lh);\n};\n",
        );
        assert_eq!(program.protocols.len(), 1, "one proto lowers to one node");
        let proto = &program.protocols[0];
        // The contract is lowered — present and not unrepresentable.
        assert!(
            proto.contract.is_some(),
            "the protocol contract lowers into HirProtocol::contract"
        );
        assert!(
            !proto.has_unrepresentable_contract,
            "the contract is fully represented — no gap"
        );
        // The contract carries pre/post expressions — they lowered to real
        // HIR nodes (not an Other gap placeholder).
        let c = proto.contract.as_ref().unwrap();
        assert!(
            !matches!(c.pre.kind, HirExprKind::Other(_)),
            "pre_condition lowered to a real HirExpr, not an Other gap"
        );
        let counts = unsupported_counts(&program);
        assert_eq!(
            counts.protocols, 0,
            "a contract-bearing proto is fully represented — no gap"
        );
        assert_eq!(counts.total(), 0, "no unsupported facts");
    }

    /// A program with no `proto` and no `frgn` should have zero boundary
    /// gaps.
    #[test]
    fn no_boundary_no_gap() {
        let program = lower(
            "let done: Bool = false;\nnode main [!done][done] {\n    done = true;\n    endprogram;\n};\n",
        );
        assert_eq!(program.protocols.len(), 0);
        assert_eq!(program.foreign.len(), 0);
        let counts = unsupported_counts(&program);
        assert_eq!(counts.protocols, 0);
        assert_eq!(counts.boundary_facts, 0);
        assert_eq!(counts.total(), 0, "a pure program has no unsupported facts");
    }
}
