// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Reactive-region analysis over typed HIR.
//!
//! This is a faithful port of `analysis::region` (AST) onto `HirProgram`.
//! Every scan, dependence fact, classification, clustering, scoring, and
//! chain-composition decision reads HIR statements/expressions only. The one
//! AST crossing is at the result boundary: a composed chain body is lowered
//! back with `hir::to_ast` because `ComposedChain` (the public result type the
//! backend re-emits) stores AST statements. Shape/region facts never come from
//! the AST.
//!
//! Semantics mirror the AST implementation exactly, including the lossy
//! substitution rules (only `Guarded` recurses; `Let` drops annotation data;
//! `SyncBlock`/`Block`/`Foreach` are copied verbatim) and the
//! `HashMap`-iteration-driven ordering. Test-only canonical dumps normalise
//! region ids and map order so parity can be asserted exactly.
//!
//! Coverage notes:
//! - Direct, non-exported, non-`sync<>` transactions are analyzed (the AST
//!   walker only visits `TopLevel::Transaction`); exported/sync-wrapped/
//!   compile-time and member transactions are not.
//! - A composed body containing an HIR `Other` statement (for example an
//!   inline compile-time definition) has no AST round-trip representation;
//!   that chain is skipped rather than crossing a guessed node.
//! - A transaction writing into more than one region maps to the first region
//!   encountered, as in the AST; the underlying region-vector order is
//!   `HashMap`-derived in both analyses.

use crate::analysis::region::{
    BudgetPlan, ComplexityClass, ComposedChain, Interval, RegionScore, VarClass, VarInfo,
};
use crate::ast::{BinaryOpKind, Type, UnaryOpKind};
use crate::hir::expr::{HirExpr, HirExprKind, HirLiteral};
use crate::hir::program::{HirCallable, HirCallableKind, HirProgram};
use crate::hir::stmt::{HirStmt, HirStmtKind};

use std::collections::{HashMap, HashSet, VecDeque};

/// Region analyzer over typed HIR. Mirrors `analysis::region::RegionAnalyzer`.
pub struct RegionAnalyzer {
    pub var_info: HashMap<String, VarInfo>,
    pub trigger_vars: HashSet<String>,
    pub regions: Vec<Vec<String>>,
    deps: HashMap<String, HashSet<String>>,
    rev_deps: HashMap<String, HashSet<String>>,
    txn_reads: HashMap<String, HashSet<String>>,
    txn_writes: HashMap<String, HashSet<String>>,
    pub linear_chains: Vec<Vec<String>>,
    txn_bodies: HashMap<String, Vec<HirStmt>>,
    iter_bounds: HashMap<String, u64>,
    pub region_scores: Vec<RegionScore>,
    pub budget_plan: Option<BudgetPlan>,
    /// AST-facing result: `ComposedChain` carries AST statements.
    pub composed_chains: Vec<ComposedChain>,
    /// HIR bodies aligned index-for-index with `composed_chains`; used by
    /// analysis queries so the HIR side never re-reads its own AST crossing.
    composed_body_hir: Vec<Vec<HirStmt>>,
}

impl RegionAnalyzer {
    pub fn empty() -> Self {
        RegionAnalyzer {
            var_info: HashMap::new(),
            trigger_vars: HashSet::new(),
            regions: Vec::new(),
            deps: HashMap::new(),
            rev_deps: HashMap::new(),
            txn_reads: HashMap::new(),
            txn_writes: HashMap::new(),
            linear_chains: Vec::new(),
            txn_bodies: HashMap::new(),
            iter_bounds: HashMap::new(),
            region_scores: Vec::new(),
            budget_plan: None,
            composed_chains: Vec::new(),
            composed_body_hir: Vec::new(),
        }
    }

    pub fn analyze(program: &HirProgram) -> Self {
        let mut analyzer = RegionAnalyzer::empty();
        analyzer.register_declarations(program);
        analyzer.build_dependency_graph(program);
        analyzer.seed_frontier();
        analyzer.propagate_classification();
        analyzer.compute_regions();
        analyzer.estimate_value_sets();
        analyzer.detect_linear_chains(program);
        analyzer.resolve_iteration_bounds(program);
        analyzer.compute_region_scores(program);
        analyzer
    }

    // ── Phase A: Collect declarations ──────────────────────────────────

    fn register_declarations(&mut self, program: &HirProgram) {
        for global in &program.globals {
            if !global.is_state_decl {
                continue;
            }
            self.register_var(&global.name, VarClass::Pure, None);
        }
        for (name, expr) in &program.const_exprs {
            let interval = Self::expr_to_interval(expr);
            self.register_var(name, VarClass::Pure, interval);
        }
        for name in &program.trigger_names {
            self.trigger_vars.insert(name.clone());
            self.register_var(name, VarClass::Opaque, None);
        }
    }

    fn register_var(&mut self, name: &str, class: VarClass, interval: Option<Interval>) {
        self.var_info.entry(name.to_string()).or_insert(VarInfo {
            classification: class,
            interval,
            value_set_size: None,
            region_id: 0,
        });
        self.deps.entry(name.to_string()).or_default();
        self.rev_deps.entry(name.to_string()).or_default();
    }

    // ── Phase B: Build dependency graph ─────────────────────────────────

    fn build_dependency_graph(&mut self, program: &HirProgram) {
        for txn in transaction_callables(program) {
            let mut txn_read_vars = HashSet::new();
            let mut txn_write_vars = HashSet::new();
            self.txn_bodies.insert(txn.name.clone(), txn.body.statements.clone());

            if let Some(contract) = &txn.contract {
                self.collect_identifiers(&contract.pre, &txn.name);
                self.collect_identifiers(&contract.post, &txn.name);
            }

            for stmt in &txn.body.statements {
                match &stmt.kind {
                    HirStmtKind::Assign(lhs, expr) => {
                        let Some(writer) = as_var_name(lhs) else { continue };
                        txn_write_vars.insert(writer.to_string());
                        txn_read_vars.extend(expr_to_var_set(expr));
                        self.collect_identifiers(expr, writer);
                    }
                    HirStmtKind::Let { name, expr: Some(e), .. } => {
                        txn_write_vars.insert(name.clone());
                        txn_read_vars.extend(expr_to_var_set(e));
                        self.collect_identifiers(e, name);
                    }
                    _ => {}
                }
            }

            self.txn_reads.entry(txn.name.clone()).or_default().extend(txn_read_vars);
            self.txn_writes.entry(txn.name.clone()).or_default().extend(txn_write_vars);
        }
    }

    /// Iterative identifier collection (AST: `collect_identifiers`). Only the
    /// same expression kinds the AST walker descends into are visited.
    fn collect_identifiers(&mut self, expr: &HirExpr, reader_for: &str) {
        let rf = reader_for.to_string();
        let mut work: Vec<&HirExpr> = vec![expr];

        while let Some(e) = work.pop() {
            match &e.kind {
                HirExprKind::Identifier(name) => {
                    self.deps.entry(rf.clone()).or_default().insert(name.clone());
                    self.rev_deps.entry(name.clone()).or_default().insert(rf.clone());
                }
                HirExprKind::Call { args, .. } => work.extend(args.iter().rev()),
                HirExprKind::Tuple(elems) | HirExprKind::List(elems) => {
                    work.extend(elems.iter().rev());
                }
                HirExprKind::Binary { left, right, .. } => {
                    work.push(right);
                    work.push(left);
                }
                HirExprKind::Unary { inner, .. } => work.push(inner),
                HirExprKind::Cast { inner, .. } => work.push(inner),
                HirExprKind::Index(list, idx) => {
                    work.push(idx);
                    work.push(list);
                }
                HirExprKind::Field { receiver, .. } => work.push(receiver),
                HirExprKind::Match { scrutinee, arms } => {
                    for arm in arms.iter().rev() {
                        if let Some(guard) = &arm.guard {
                            work.push(guard);
                        }
                        work.push(&arm.body);
                    }
                    work.push(scrutinee);
                }
                _ => {}
            }
        }
    }

    // ── Phase C: Seed frontier ──────────────────────────────────────────

    fn seed_frontier(&mut self) {
        let trigger_names: Vec<String> = self.trigger_vars.iter().cloned().collect();
        for name in trigger_names {
            let is_tight = self
                .var_info
                .get(&name)
                .and_then(|info| info.interval.as_ref())
                .map(|iv| iv.size() <= 256)
                .unwrap_or(false);
            if is_tight {
                if let Some(info) = self.var_info.get_mut(&name) {
                    info.classification = VarClass::Bounded;
                }
            }
        }
    }

    // ── Phase D: Propagate classification ───────────────────────────────

    fn propagate_classification(&mut self) {
        let var_names: Vec<String> = self.var_info.keys().cloned().collect();
        let mut queue: VecDeque<String> = var_names
            .into_iter()
            .filter(|name| self.var_info[name].classification != VarClass::Pure)
            .collect();

        while let Some(frontier_var) = queue.pop_front() {
            let frontier_class = self.var_info[&frontier_var].classification.clone();
            let Some(readers) = self.rev_deps.get(&frontier_var).cloned() else { continue };
            for reader in readers {
                let Some(reader_info) = self.var_info.get(&reader) else { continue };
                let Some(updated) = tighten(&reader_info.classification, &frontier_class) else {
                    continue;
                };
                if let Some(info) = self.var_info.get_mut(&reader) {
                    info.classification = updated;
                    queue.push_back(reader.clone());
                }
            }
        }
    }

    // ── Phase E: Find connected components (regions) ────────────────────

    fn compute_regions(&mut self) {
        let mut visited: HashSet<String> = HashSet::new();
        let mut region_id_counter: usize = 0;

        let non_pure: Vec<String> = self
            .var_info
            .iter()
            .filter(|(_, info)| info.classification != VarClass::Pure)
            .map(|(name, _)| name.clone())
            .collect();

        for seed in &non_pure {
            if visited.contains(seed) {
                continue;
            }
            let component = self.collect_component(seed, &mut visited);
            if component.is_empty() {
                continue;
            }
            region_id_counter += 1;
            for var in &component {
                if let Some(info) = self.var_info.get_mut(var) {
                    info.region_id = region_id_counter;
                }
            }
            self.regions.push(component);
        }

        for info in self.var_info.values_mut() {
            if info.classification == VarClass::Pure {
                info.region_id = 0;
            }
        }
    }

    fn collect_component(&self, seed: &str, visited: &mut HashSet<String>) -> Vec<String> {
        let mut component: Vec<String> = Vec::new();
        let mut stack: Vec<String> = vec![seed.to_string()];
        while let Some(var) = stack.pop() {
            if !visited.insert(var.clone()) {
                continue;
            }
            component.push(var.clone());
            for n in self.region_neighbors(&var) {
                if !visited.contains(&n) {
                    stack.push(n);
                }
            }
        }
        component
    }

    /// Neighbors = deps[var] ∪ rev_deps[var], filtered to non-Pure.
    fn region_neighbors(&self, var: &str) -> Vec<String> {
        let mut neighbors: Vec<String> = Vec::new();
        for table in [&self.deps, &self.rev_deps] {
            if let Some(edges) = table.get(var) {
                for n in edges {
                    let non_pure = self
                        .var_info
                        .get(n)
                        .map(|i| i.classification != VarClass::Pure)
                        .unwrap_or(false);
                    if non_pure {
                        neighbors.push(n.clone());
                    }
                }
            }
        }
        neighbors
    }

    // ── Phase G: Estimate value set sizes ───────────────────────────────

    fn estimate_value_sets(&mut self) {
        let names: Vec<String> = self.var_info.keys().cloned().collect();
        for name in names {
            let size = self.compute_value_set_size(&name);
            if let Some(info) = self.var_info.get_mut(&name) {
                info.value_set_size = size;
            }
        }
    }

    fn compute_value_set_size(&self, name: &str) -> Option<u64> {
        let info = self.var_info.get(name)?;
        match info.classification {
            VarClass::Pure => match info.interval.as_ref() {
                Some(iv) if iv.size() > 0 => Some(iv.size()),
                _ => None,
            },
            VarClass::Bounded => info.interval.as_ref().map(|iv| iv.size()),
            VarClass::Opaque => None,
        }
    }

    // ── Utilities ───────────────────────────────────────────────────────

    fn expr_to_interval(expr: &HirExpr) -> Option<Interval> {
        match &expr.kind {
            HirExprKind::Literal(HirLiteral::Int(n)) => Some(Interval { lo: *n, hi: *n }),
            HirExprKind::Literal(HirLiteral::Bool(b)) => {
                let v = if *b { 1 } else { 0 };
                Some(Interval { lo: v, hi: v })
            }
            HirExprKind::Unary { op: UnaryOpKind::Neg, inner } => {
                Self::expr_to_interval(inner).map(|iv| Interval { lo: -iv.hi, hi: -iv.lo })
            }
            _ => None,
        }
    }

    /// AST parity helper retained for completeness; the AST copy is currently
    /// unused. Kept so the port covers every AST utility.
    #[allow(dead_code)]
    fn type_to_interval(ty: &Type) -> Option<Interval> {
        match ty {
            Type::Custom(name) if name == "Bool" => Some(Interval { lo: 0, hi: 1 }),
            Type::Custom(name) if name == "Int" => None,
            Type::Custom(name) if name == "UInt" => Some(Interval { lo: 0, hi: i64::MAX }),
            Type::Custom(name) if name == "Char" => Some(Interval { lo: 0, hi: 0x10FFFF }),
            _ => None,
        }
    }

    // ── Phase H: Detect linear transaction chains ───────────────────────

    fn detect_linear_chains(&mut self, _program: &HirProgram) {
        self.linear_chains.clear();
        let txn_names: Vec<String> = self.txn_reads.keys().cloned().collect();
        for txn in &txn_names {
            let writes = self.txn_writes.get(txn).cloned().unwrap_or_default();
            for written in &writes {
                if !self.rev_deps.contains_key(written) {
                    continue;
                }
                let downstream = self.downstream_txns(txn, written, &txn_names);
                if downstream.len() == 1 {
                    let mut chain = vec![txn.clone(), downstream[0].clone()];
                    self.extend_chain(&mut chain, &txn_names);
                    self.linear_chains.push(chain);
                }
            }
        }
        self.dedupe_chains();
    }

    fn downstream_txns(&self, txn: &str, written: &str, txn_names: &[String]) -> Vec<String> {
        txn_names
            .iter()
            .filter(|tn| {
                *tn != txn
                    && self
                        .txn_reads
                        .get(*tn)
                        .map(|r| r.contains(written))
                        .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    fn extend_chain(&self, chain: &mut Vec<String>, txn_names: &[String]) {
        loop {
            let Some(last) = chain.last().cloned() else { break };
            let cur_writes = self.txn_writes.get(&last).cloned().unwrap_or_default();
            let mut extended = false;
            for cw in &cur_writes {
                let further: Vec<String> = txn_names
                    .iter()
                    .filter(|tn| {
                        !chain.contains(tn)
                            && self.txn_reads.get(*tn).map(|r| r.contains(cw)).unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                if further.len() == 1 {
                    chain.push(further[0].clone());
                    extended = true;
                    break;
                }
            }
            if !extended {
                break;
            }
        }
    }

    fn dedupe_chains(&mut self) {
        self.linear_chains.sort_by(|a, b| b.len().cmp(&a.len()));
        let mut deduped: Vec<Vec<String>> = Vec::new();
        for chain in &self.linear_chains {
            let chain_set: HashSet<&String> = chain.iter().collect();
            let covered = deduped.iter().any(|existing| {
                let existing_set: HashSet<&String> = existing.iter().collect();
                existing_set.is_superset(&chain_set) && existing.len() > chain.len()
            });
            if !covered {
                deduped.push(chain.clone());
            }
        }
        self.linear_chains = deduped;
    }

    // ── Public query API ────────────────────────────────────────────────

    pub fn region_of(&self, var: &str) -> Option<usize> {
        self.var_info.get(var).map(|i| i.region_id)
    }

    pub fn classification_of(&self, var: &str) -> Option<VarClass> {
        self.var_info.get(var).map(|i| i.classification.clone())
    }

    pub fn is_frontier_dependent(&self, var: &str) -> bool {
        self.var_info
            .get(var)
            .map(|i| i.classification != VarClass::Pure)
            .unwrap_or(false)
    }

    pub fn value_set_size_of(&self, var: &str) -> Option<u64> {
        self.var_info.get(var).and_then(|i| i.value_set_size)
    }

    // ── Iteration bound resolution ──────────────────────────────────────

    fn resolve_iteration_bounds(&mut self, program: &HirProgram) {
        self.iter_bounds.clear();
        for txn in transaction_callables(program) {
            let Some(contract) = &txn.contract else { continue };
            let Some((_, bound)) = Self::extract_bound_from_pre(&contract.pre) else {
                continue;
            };
            if let Some(v) = self.resolve_bound_value(program, &bound) {
                self.iter_bounds.insert(txn.name.clone(), v);
            }
        }
    }

    fn extract_bound_from_pre(pre: &HirExpr) -> Option<(String, String)> {
        let HirExprKind::Binary { op, left, right } = &pre.kind else { return None };
        if !matches!(op, BinaryOpKind::Le | BinaryOpKind::Lt) {
            return None;
        }
        match (&left.kind, &right.kind) {
            (HirExprKind::Identifier(var), HirExprKind::Identifier(bound)) => {
                Some((var.clone(), bound.clone()))
            }
            _ => None,
        }
    }

    fn resolve_bound_value(&self, program: &HirProgram, bound_var: &str) -> Option<u64> {
        for (name, expr) in &program.const_exprs {
            if name != bound_var {
                continue;
            }
            if let HirExprKind::Literal(HirLiteral::Int(n)) = &expr.kind {
                return Some(*n as u64);
            }
        }
        None
    }

    // ── Complexity estimation ───────────────────────────────────────────

    fn classify_complexity(&self, body: &[HirStmt]) -> ComplexityClass {
        if self.has_ffi_or_trigger_refs(body) {
            return ComplexityClass::Unbounded;
        }
        weight_class(count_statements_recursive(body))
    }

    fn has_ffi_or_trigger_refs(&self, body: &[HirStmt]) -> bool {
        body.iter().any(|stmt| has_ffi_or_trigger_stmt(stmt, &self.trigger_vars))
    }

    // ── Region scoring ──────────────────────────────────────────────────

    fn compute_region_scores(&mut self, _program: &HirProgram) {
        self.region_scores.clear();
        let txn_to_region = self.build_txn_to_region_map();
        let mut region_txns: HashMap<usize, Vec<String>> = HashMap::new();
        for (txn, rid) in &txn_to_region {
            region_txns.entry(*rid).or_default().push(txn.clone());
        }

        for (rid, txn_list) in region_txns {
            let score = self.score_region(rid, txn_list);
            self.region_scores.push(score);
        }

        self.region_scores.sort_by(|a, b| {
            b.optimization_score
                .partial_cmp(&a.optimization_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    fn score_region(&self, rid: usize, txn_list: Vec<String>) -> RegionScore {
        let mut acc = ScoreAcc::new();
        for tn in &txn_list {
            self.accumulate_txn(tn, &mut acc);
        }
        acc.finish(rid, txn_list)
    }

    fn accumulate_txn(&self, tn: &str, acc: &mut ScoreAcc) {
        if let Some(body) = self.txn_bodies.get(tn) {
            let class = self.classify_complexity(body);
            if class != ComplexityClass::Trivial {
                acc.complexity = std::cmp::max(acc.complexity.clone(), class);
            }
            acc.body_weight += count_statements_recursive(body);
            if has_term_or_unify_escape(body) {
                acc.all_pure_and_nop_term = false;
            }
        }
        if self.has_txn_trigger_refs(tn) {
            acc.all_pure_and_nop_term = false;
        }
        acc.max_iter = acc.max_iter.max(self.iter_bounds.get(tn).copied().unwrap_or(1));
        let Some(reads) = self.txn_reads.get(tn) else { return };
        for r in reads.iter().filter(|r| self.trigger_vars.contains(*r)) {
            let size = self.value_set_size_of(r);
            acc.combined_vset_size = match (acc.combined_vset_size, size) {
                (Some(a), Some(b)) => Some(a.saturating_mul(b)),
                _ => None,
            };
        }
    }

    fn build_txn_to_region_map(&self) -> HashMap<String, usize> {
        let mut map = HashMap::new();
        for (idx, region) in self.regions.iter().enumerate() {
            let rid = idx + 1;
            for (tn, writes) in &self.txn_writes {
                if writes.iter().any(|w| region.contains(w)) {
                    map.entry(tn.clone()).or_insert(rid);
                }
            }
        }
        map
    }

    fn has_txn_trigger_refs(&self, txn_name: &str) -> bool {
        self.txn_reads
            .get(txn_name)
            .map(|reads| reads.iter().any(|r| self.trigger_vars.contains(r)))
            .unwrap_or(false)
    }

    // ── Budget planning ─────────────────────────────────────────────────

    pub fn build_budget_plan(&mut self, budget: u64) {
        let mut allocated = Vec::new();
        let mut skipped = Vec::new();
        let mut remaining = budget;

        for score in &self.region_scores {
            if score.complexity == ComplexityClass::Unbounded {
                skipped.push((score.region_id, score.complexity.clone(), 0));
                continue;
            }
            let cost = score.value_set_size.unwrap_or(u64::MAX);
            if cost == u64::MAX || cost > remaining {
                skipped.push((score.region_id, score.complexity.clone(), cost.min(remaining)));
            } else {
                remaining = remaining.saturating_sub(cost);
                allocated.push((
                    score.region_id,
                    score.complexity.clone(),
                    cost,
                    score.optimization_score,
                ));
            }
        }

        self.budget_plan = Some(BudgetPlan {
            total_budget: budget,
            allocated,
            residual_budget: remaining,
            skipped,
        });
    }

    pub fn iteration_bound_of(&self, txn_name: &str) -> Option<u64> {
        self.iter_bounds.get(txn_name).copied()
    }
}

/// Direct (non-exported, non-sync-wrapped) transactions — the AST region only
/// walks `TopLevel::Transaction` items.
fn transaction_callables(program: &HirProgram) -> impl Iterator<Item = &HirCallable> {
    program.callables.iter().filter(|c| {
        c.kind == HirCallableKind::Transaction
            && !c.exported
            && c.owner.is_none()
            && c.sync_domains.is_empty()
            && !c.is_impl_fn
    })
}

/// Classification tightening during frontier propagation.
fn tighten(current: &VarClass, frontier: &VarClass) -> Option<VarClass> {
    let needs = *current == VarClass::Pure
        || (*frontier == VarClass::Opaque && *current == VarClass::Bounded);
    if !needs {
        return None;
    }
    Some(if *frontier == VarClass::Opaque {
        VarClass::Opaque
    } else {
        VarClass::Bounded
    })
}

/// Merge a contract fact into a common slot; false on disagreement.
fn merge_common(slot: &mut Option<String>, value: String) -> bool {
    match slot {
        Some(existing) => existing == &value,
        None => {
            *slot = Some(value);
            true
        }
    }
}

fn weight_class(weight: usize) -> ComplexityClass {
    match weight {
        0..=2 => ComplexityClass::Trivial,
        3..=5 => ComplexityClass::Light,
        6..=20 => ComplexityClass::Medium,
        _ => ComplexityClass::Heavy,
    }
}

#[derive(Clone, Copy)]
struct ComposedPlan<'a> {
    chain: &'a [String],
    link_vars: &'a [String],
    root_triggers: &'a [String],
}

#[derive(Clone, Copy)]
struct LinkSpec<'a> {
    chain: &'a [String],
    link_vars: &'a [String],
    index: usize,
}

struct ScoreAcc {
    body_weight: usize,
    complexity: ComplexityClass,
    max_iter: u64,
    combined_vset_size: Option<u64>,
    all_pure_and_nop_term: bool,
}

impl ScoreAcc {
    fn new() -> Self {
        ScoreAcc {
            body_weight: 0,
            complexity: ComplexityClass::Trivial,
            max_iter: 1,
            combined_vset_size: Some(1),
            all_pure_and_nop_term: true,
        }
    }

    fn finish(self, rid: usize, txn_names: Vec<String>) -> RegionScore {
        let cost = self.combined_vset_size.unwrap_or(1).max(1);
        let score = if self.complexity == ComplexityClass::Unbounded {
            f64::NEG_INFINITY
        } else {
            (self.body_weight as f64 * self.max_iter as f64) / cost as f64
        };
        let gpu_eligible = self.all_pure_and_nop_term
            && self.complexity != ComplexityClass::Trivial
            && self.complexity != ComplexityClass::Light
            && self.complexity != ComplexityClass::Unbounded;
        RegionScore {
            region_id: rid,
            txn_names,
            complexity: self.complexity,
            body_weight: self.body_weight,
            iteration_count: self.max_iter,
            value_set_size: self.combined_vset_size,
            optimization_score: score,
            chain_composed: false,
            gpu_eligible,
        }
    }
}

fn as_var_name(expr: &HirExpr) -> Option<&str> {
    match &expr.kind {
        HirExprKind::Identifier(name) => Some(name),
        _ => None,
    }
}

fn expr_to_var_set(expr: &HirExpr) -> HashSet<String> {
    let mut vars = HashSet::new();
    collect_var_ids(expr, &mut vars);
    vars
}

/// Iterative variable collection. Matches the AST walker's covered kinds
/// exactly (notably not `Match`/`If`/`Slice`/`MethodCall`).
fn collect_var_ids(expr: &HirExpr, vars: &mut HashSet<String>) {
    let mut work: Vec<&HirExpr> = vec![expr];
    while let Some(e) = work.pop() {
        match &e.kind {
            HirExprKind::Identifier(n) => {
                vars.insert(n.clone());
            }
            HirExprKind::Binary { left, right, .. } => {
                work.push(right);
                work.push(left);
            }
            HirExprKind::Unary { inner, .. } => work.push(inner),
            HirExprKind::Call { args, .. } => work.extend(args.iter().rev()),
            HirExprKind::Tuple(elems) | HirExprKind::List(elems) => {
                work.extend(elems.iter().rev());
            }
            HirExprKind::Index(list, idx) => {
                work.push(idx);
                work.push(list);
            }
            HirExprKind::Field { receiver, .. } => work.push(receiver),
            _ => {}
        }
    }
}

/// Iterative statement count. Recurses only Guarded/SyncBlock/Foreach, exactly
/// like the AST helper (Block is intentionally not descended).
fn count_statements_recursive(body: &[HirStmt]) -> usize {
    let mut count = 0;
    let mut work: Vec<&HirStmt> = body.iter().collect();
    while let Some(s) = work.pop() {
        count += 1;
        match &s.kind {
            HirStmtKind::Guarded(_, statements) => work.extend(statements.iter().rev()),
            HirStmtKind::SyncBlock(inner) => work.extend(inner.iter().rev()),
            HirStmtKind::Foreach { body: inner, .. } => work.extend(inner.iter().rev()),
            _ => {}
        }
    }
    count
}

/// Any terminator/asm/call blocks compile-time precomputation.
fn has_ffi_or_terminator_stmt(stmt: &HirStmt) -> bool {
    let mut work: Vec<&HirStmt> = vec![stmt];
    while let Some(s) = work.pop() {
        match &s.kind {
            HirStmtKind::Term(_)
            | HirStmtKind::EndProgram(_)
            | HirStmtKind::Tail(_)
            | HirStmtKind::InlineAsm { .. } => return true,
            HirStmtKind::Assign(_, expr) if expr_has_call(expr) => return true,
            HirStmtKind::Let { expr: Some(e), .. } if expr_has_call(e) => return true,
            HirStmtKind::Expression(e) if expr_has_call(e) => return true,
            HirStmtKind::Guarded(condition, statements) => {
                if expr_has_call(condition) {
                    return true;
                }
                work.extend(statements.iter().rev());
            }
            HirStmtKind::Foreach { body, .. } => work.extend(body.iter().rev()),
            _ => {}
        }
    }
    false
}

pub(crate) fn has_ffi_or_trigger_stmt_in_chain(body: &[HirStmt]) -> bool {
    body.iter().any(has_ffi_or_terminator_stmt)
}

/// Trigger-reference / call scan used by complexity classification.
fn has_ffi_or_trigger_stmt(stmt: &HirStmt, trigger_vars: &HashSet<String>) -> bool {
    let mut work: Vec<&HirStmt> = vec![stmt];
    while let Some(s) = work.pop() {
        match &s.kind {
            HirStmtKind::Term(_)
            | HirStmtKind::EndProgram(_)
            | HirStmtKind::Tail(_)
            | HirStmtKind::InlineAsm { .. } => return true,
            HirStmtKind::Assign(lhs, expr) => {
                if expr_has_call(expr) {
                    return true;
                }
                if let HirExprKind::Identifier(n) = &lhs.kind {
                    if trigger_vars.contains(n) {
                        return true;
                    }
                }
            }
            HirStmtKind::Let { expr: Some(e), .. } if expr_has_call(e) => return true,
            HirStmtKind::Expression(e) if expr_has_call(e) => return true,
            HirStmtKind::Guarded(condition, statements) => {
                if expr_has_call(condition) {
                    return true;
                }
                work.extend(statements.iter().rev());
            }
            HirStmtKind::Foreach { body, .. } => work.extend(body.iter().rev()),
            _ => {}
        }
    }
    false
}

/// Exhaustive HIR walk; any call-shaped node conservatively blocks folding.
fn expr_has_call(expr: &HirExpr) -> bool {
    let mut found = false;
    crate::hir::expr_walk::walk_expr_pub(expr, &mut |e| {
        if matches!(
            &e.kind,
            HirExprKind::Call { .. }
                | HirExprKind::MethodCall { .. }
                | HirExprKind::Spawn { .. }
                | HirExprKind::PluginIntercept { .. }
        ) {
            found = true;
        }
    });
    found
}

fn has_term_or_unify_escape(body: &[HirStmt]) -> bool {
    body.iter().any(|s| {
        matches!(
            &s.kind,
            HirStmtKind::Rollback(_) | HirStmtKind::InlineAsm { .. }
        )
    })
}

fn is_counter_bump_stmt(stmt: &HirStmt, counter_var: &str) -> bool {
    let HirStmtKind::Assign(lhs, expr) = &stmt.kind else { return false };
    let Some(name) = as_var_name(lhs) else { return false };
    if name != counter_var {
        return false;
    }
    let HirExprKind::Binary { op: BinaryOpKind::Add, left, right } = &expr.kind else {
        return false;
    };
    bump_shape(left, right, counter_var)
}

fn find_counter_var(body: &[HirStmt]) -> Option<String> {
    for s in body {
        let HirStmtKind::Assign(lhs, expr) = &s.kind else { continue };
        let Some(name) = as_var_name(lhs) else { continue };
        let HirExprKind::Binary { op: BinaryOpKind::Add, left, right } = &expr.kind else {
            continue;
        };
        if bump_shape(left, right, name) {
            return Some(name.to_string());
        }
    }
    None
}

fn bump_shape(left: &HirExpr, right: &HirExpr, counter_var: &str) -> bool {
    let lhs_in_left = matches!(&left.kind, HirExprKind::Identifier(n) if n == counter_var);
    let lhs_in_right = matches!(&right.kind, HirExprKind::Identifier(n) if n == counter_var);
    let pos_int = |e: &HirExpr| matches!(&e.kind, HirExprKind::Literal(HirLiteral::Int(d)) if *d > 0);
    (lhs_in_left && pos_int(right)) || (lhs_in_right && pos_int(left))
}

fn find_write_expr(body: &[HirStmt], var: &str) -> Option<HirExpr> {
    for s in body {
        if let HirStmtKind::Assign(lhs, expr) = &s.kind {
            if as_var_name(lhs) == Some(var) {
                return Some(expr.clone());
            }
        }
    }
    None
}

fn apply_trigger_values(
    mut stmts: Vec<HirStmt>,
    trigger_values: Option<&[(String, i64)]>,
) -> Vec<HirStmt> {
    let Some(values) = trigger_values else { return stmts };
    for (name, value) in values {
        stmts = substitute_var(&stmts, name, &int_expr(*value));
    }
    stmts
}

fn filter_counter(stmts: Vec<HirStmt>, index: usize, counter_var: Option<&str>) -> Vec<HirStmt> {
    let Some(counter) = counter_var else { return stmts };
    if index == 0 {
        return stmts;
    }
    stmts
        .into_iter()
        .filter(|stmt| !is_counter_bump_stmt(stmt, counter))
        .collect()
}

fn substitute_var(body: &[HirStmt], old_var: &str, new_expr: &HirExpr) -> Vec<HirStmt> {
    body.iter().map(|s| substitute_stmt(s, old_var, new_expr)).collect()
}

fn substitute_stmt(stmt: &HirStmt, old_var: &str, new_expr: &HirExpr) -> HirStmt {
    let kind = match &stmt.kind {
        HirStmtKind::Assign(lhs, expr) => {
            HirStmtKind::Assign(lhs.clone(), substitute_expr(expr, old_var, new_expr))
        }
        HirStmtKind::Let { name, expr, .. } => HirStmtKind::Let {
            name: name.clone(),
            names: vec![],
            ty: None,
            expr: expr
                .as_ref()
                .map(|e| substitute_expr(e, old_var, new_expr)),
            modifiers: vec![],
        },
        HirStmtKind::Guarded(condition, statements) => HirStmtKind::Guarded(
            substitute_expr(condition, old_var, new_expr),
            substitute_var(statements, old_var, new_expr),
        ),
        HirStmtKind::Expression(e) => {
            HirStmtKind::Expression(substitute_expr(e, old_var, new_expr))
        }
        HirStmtKind::Term(Some(e)) => {
            HirStmtKind::Term(Some(substitute_expr(e, old_var, new_expr)))
        }
        HirStmtKind::Tail(e) => HirStmtKind::Tail(substitute_expr(e, old_var, new_expr)),
        HirStmtKind::Term(None) => HirStmtKind::Term(None),
        HirStmtKind::EndProgram(Some(e)) => {
            HirStmtKind::EndProgram(Some(substitute_expr(e, old_var, new_expr)))
        }
        HirStmtKind::EndProgram(None) => HirStmtKind::EndProgram(None),
        HirStmtKind::Rollback(Some(e)) => {
            HirStmtKind::Rollback(Some(substitute_expr(e, old_var, new_expr)))
        }
        HirStmtKind::Rollback(None) => HirStmtKind::Rollback(None),
        HirStmtKind::SyncBlock(body) => HirStmtKind::SyncBlock(body.clone()),
        other => other.clone(),
    };
    HirStmt { id: stmt.id, span: stmt.span, kind }
}

/// Post-order substitution work item. Module-scoped so the n-ary helpers can
/// push onto the same stack.
enum SubstWork {
    Visit(HirExpr),
    Binary(HirExpr, BinaryOpKind),
    Unary(HirExpr, UnaryOpKind),
    Cast(HirExpr, Type),
    Call(HirExpr, String, usize),
    Nary(HirExpr, bool, usize),
    Field(HirExpr, String),
    Index(HirExpr),
}

/// Post-order iterative substitution. Mirrors the AST walker: only the same
/// structural expression kinds are rebuilt; everything else is cloned.
fn substitute_expr(expr: &HirExpr, old_var: &str, new_expr: &HirExpr) -> HirExpr {
    let mut work: Vec<SubstWork> = vec![SubstWork::Visit(expr.clone())];
    let mut results: Vec<HirExpr> = Vec::new();

    while let Some(w) = work.pop() {
        match w {
            SubstWork::Visit(e) => match e.kind.clone() {
                HirExprKind::Identifier(n) if n == old_var => results.push(new_expr.clone()),
                HirExprKind::Identifier(_) => results.push(e),
                HirExprKind::Binary { op, left, right } => {
                    work.push(SubstWork::Binary(e, op));
                    work.push(SubstWork::Visit(*right));
                    work.push(SubstWork::Visit(*left));
                }
                HirExprKind::Unary { op, inner } => {
                    work.push(SubstWork::Unary(e, op));
                    work.push(SubstWork::Visit(*inner));
                }
                HirExprKind::Cast { inner, to, .. } => {
                    work.push(SubstWork::Cast(e, to));
                    work.push(SubstWork::Visit(*inner));
                }
                HirExprKind::Call { callee, args, .. } => {
                    let n = args.len();
                    work.push(SubstWork::Call(e, callee, n));
                    for a in args.into_iter().rev() {
                        work.push(SubstWork::Visit(a));
                    }
                }
                HirExprKind::List(items) => push_nary(&mut work, e, true, items),
                HirExprKind::Tuple(items) => push_nary(&mut work, e, false, items),
                HirExprKind::Field { receiver, name } => {
                    work.push(SubstWork::Field(e, name));
                    work.push(SubstWork::Visit(*receiver));
                }
                HirExprKind::Index(list, idx) => {
                    work.push(SubstWork::Index(e));
                    work.push(SubstWork::Visit(*idx));
                    work.push(SubstWork::Visit(*list));
                }
                _ => results.push(e),
            },
            SubstWork::Binary(e, op) => {
                let right = results.pop().expect("binary: right");
                let left = results.pop().expect("binary: left");
                results.push(HirExpr {
                    kind: HirExprKind::Binary { op, left: Box::new(left), right: Box::new(right) },
                    ..e
                });
            }
            SubstWork::Unary(e, op) => {
                let inner = results.pop().expect("unary: operand");
                results.push(HirExpr {
                    kind: HirExprKind::Unary { op, inner: Box::new(inner) },
                    ..e
                });
            }
            SubstWork::Cast(e, to) => {
                let inner = results.pop().expect("cast: operand");
                results.push(HirExpr {
                    kind: HirExprKind::Cast { inner: Box::new(inner), to, via: None },
                    ..e
                });
            }
            SubstWork::Call(e, callee, n) => {
                let args = drain_n(&mut results, n);
                results.push(HirExpr {
                    kind: HirExprKind::Call { callee, args, analysis_id: None },
                    ..e
                });
            }
            SubstWork::Nary(e, is_list, n) => {
                let items = drain_n(&mut results, n);
                let kind = if is_list { HirExprKind::List(items) } else { HirExprKind::Tuple(items) };
                results.push(HirExpr { kind, ..e });
            }
            SubstWork::Field(e, name) => {
                let receiver = results.pop().expect("field: receiver");
                results.push(HirExpr {
                    kind: HirExprKind::Field { receiver: Box::new(receiver), name },
                    ..e
                });
            }
            SubstWork::Index(e) => {
                let index = results.pop().expect("index: index");
                let list = results.pop().expect("index: list");
                results.push(HirExpr {
                    kind: HirExprKind::Index(Box::new(list), Box::new(index)),
                    ..e
                });
            }
        }
    }
    results.pop().expect("substitute_expr: no result")
}

fn push_nary(work: &mut Vec<SubstWork>, e: HirExpr, is_list: bool, items: Vec<HirExpr>) {
    let n = items.len();
    work.push(SubstWork::Nary(e, is_list, n));
    for item in items.into_iter().rev() {
        work.push(SubstWork::Visit(item));
    }
}

fn drain_n(results: &mut Vec<HirExpr>, n: usize) -> Vec<HirExpr> {
    let len = results.len();
    results.drain(len - n..).collect()
}

// ── Chain composition ───────────────────────────────────────────────────

impl RegionAnalyzer {

    /// 2026-11-11: deterministic fingerprint of the consumer-visible facts —
    /// the debug oracle for the HIR consumer flip (HashMaps are sorted).
    pub fn consumer_fingerprint(&self) -> String {
        let mut regions = self.regions.clone();
        regions.sort();
        let mut chains = self.linear_chains.clone();
        chains.sort();
        let mut triggers: Vec<&String> = self.trigger_vars.iter().collect();
        triggers.sort();
        let bounds: std::collections::BTreeMap<&str, u64> =
            self.iter_bounds.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        format!(
            "{regions:?}|{chains:?}|{triggers:?}|{bounds:?}|{:?}|{:?}|{:?}",
            self.region_scores, self.budget_plan, self.composed_chains
        )
    }

    pub fn compose_chains(&mut self) {
        self.composed_chains.clear();
        self.composed_body_hir.clear();
        let chains = self.linear_chains.clone();
        let all_txn_names: HashSet<String> = self.txn_reads.keys().cloned().collect();

        for chain in &chains {
            if chain.len() < 2 {
                continue;
            }
            let link_vars = self.compute_link_vars(chain);
            if link_vars.is_empty() || !self.chain_is_composable(chain) {
                continue;
            }
            let root_triggers: Vec<String> = self
                .txn_reads
                .get(&chain[0])
                .map(|reads| {
                    reads
                        .iter()
                        .filter(|r| self.trigger_vars.contains(*r))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let all_internal = link_vars
                .iter()
                .all(|lv| self.var_is_chain_internal(lv, chain, &all_txn_names));
            let plan = ComposedPlan { chain, link_vars: &link_vars, root_triggers: &root_triggers };
            self.emit_chain_variants(plan, all_internal);
        }

        self.update_scores_after_composition();
    }

    fn emit_chain_variants(&mut self, plan: ComposedPlan<'_>, all_internal: bool) {
        if plan.root_triggers.len() != 1 {
            self.push_composed_chain(plan, None, all_internal);
            return;
        }
        let trg = &plan.root_triggers[0];
        let interval = self.var_info.get(trg).and_then(|info| info.interval.clone());
        let Some(iv) = interval else {
            self.push_composed_chain(plan, None, all_internal);
            return;
        };
        let bound = iv.size().min(256);
        let hi = iv.lo.saturating_add((bound - 1) as i64).min(iv.hi);
        for val in iv.lo..=hi {
            let tv = vec![(trg.clone(), val)];
            self.push_composed_chain(plan, Some(&tv), all_internal);
        }
    }

    pub fn is_fully_precomputable(&self, budget: u64) -> bool {
        if self.composed_chains.is_empty() {
            return false;
        }
        let mut total: u64 = 0;
        for (cc, body) in self.composed_chains.iter().zip(&self.composed_body_hir) {
            if has_ffi_or_trigger_stmt_in_chain(body) || cc.trigger_values.is_some() {
                return false;
            }
            if !cc.all_internal || Self::composed_body_has_indexed_write(body) {
                return false;
            }
            total = total.saturating_add(1);
        }
        if self
            .region_scores
            .iter()
            .any(|s| s.complexity == ComplexityClass::Unbounded)
        {
            return false;
        }
        total <= budget
    }

    pub fn collect_final_values(
        &self,
        program: &HirProgram,
    ) -> Option<Vec<(Vec<String>, HashMap<String, i64>)>> {
        let mut all_bindings = Vec::new();
        for (cc, body) in self.composed_chains.iter().zip(&self.composed_body_hir) {
            let mut bindings = Self::initial_bindings(program);
            let mut chain_bindings = HashMap::new();
            if cc.all_internal {
                if let Some(ref cv) = cc.counter_var {
                    let bound = *self.iter_bounds.get(&cc.chain[0])?;
                    bindings.insert(cv.clone(), bound as i64);
                    chain_bindings.insert(cv.clone(), bound as i64);
                }
                all_bindings.push((cc.chain.clone(), chain_bindings));
                continue;
            }
            for stmt in body {
                if !Self::eval_stmt(stmt, &mut bindings) {
                    return None;
                }
            }
            for (k, v) in &bindings {
                chain_bindings.insert(k.clone(), *v);
            }
            all_bindings.push((cc.chain.clone(), chain_bindings));
        }
        Some(all_bindings)
    }

    fn initial_bindings(program: &HirProgram) -> HashMap<String, i64> {
        let mut bindings = HashMap::new();
        for (name, expr) in &program.const_exprs {
            if let Some(v) = Self::eval_expr_simple(expr, &HashMap::new()) {
                bindings.insert(name.clone(), v);
            }
        }
        bindings
    }

    /// Indexed writes are unrepresentable in the i64 binding model.
    fn composed_body_has_indexed_write(body: &[HirStmt]) -> bool {
        fn stmt_has_indexed_write(s: &HirStmt) -> bool {
            match &s.kind {
                HirStmtKind::Assign(lhs, _) => matches!(&lhs.kind, HirExprKind::Index(..)),
                HirStmtKind::Guarded(_, inner) => inner.iter().any(stmt_has_indexed_write),
                HirStmtKind::Block(inner) | HirStmtKind::SyncBlock(inner) => {
                    inner.iter().any(stmt_has_indexed_write)
                }
                _ => false,
            }
        }
        body.iter().any(stmt_has_indexed_write)
    }

    fn eval_stmt(stmt: &HirStmt, bindings: &mut HashMap<String, i64>) -> bool {
        match &stmt.kind {
            HirStmtKind::Assign(lhs, expr) => match &lhs.kind {
                HirExprKind::Identifier(name) => {
                    match Self::eval_expr_simple(expr, bindings) {
                        Some(val) => {
                            bindings.insert(name.clone(), val);
                            true
                        }
                        None => false,
                    }
                }
                HirExprKind::Index(..) => false,
                _ => {
                    let _ = Self::eval_expr_simple(expr, bindings);
                    true
                }
            },
            HirStmtKind::Let { name, expr: Some(e), .. } => {
                if let Some(val) = Self::eval_expr_simple(e, bindings) {
                    bindings.insert(name.clone(), val);
                }
                true
            }
            HirStmtKind::Let { expr: None, .. } => true,
            HirStmtKind::Expression(e) => Self::eval_expr_simple(e, bindings).is_some(),
            HirStmtKind::Guarded(condition, statements) => {
                let Some(cond) = Self::eval_expr_simple(condition, bindings) else {
                    return false;
                };
                if cond != 0 {
                    for s in statements {
                        if !Self::eval_stmt(s, bindings) {
                            return false;
                        }
                    }
                }
                true
            }
            HirStmtKind::Term(None) | HirStmtKind::EndProgram(None) | HirStmtKind::InlineAsm { .. } => false,
            _ => true,
        }
    }

    /// Iterative arithmetic evaluation (AST: `eval_expr_simple`).
    fn eval_expr_simple(expr: &HirExpr, bindings: &HashMap<String, i64>) -> Option<i64> {
        enum Frame<'a> {
            Visit(&'a HirExpr),
            Combine(&'a HirExpr),
        }
        let mut stack = vec![Frame::Visit(expr)];
        let mut results: Vec<i64> = Vec::new();

        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Visit(e) => match &e.kind {
                    HirExprKind::Literal(HirLiteral::Int(n)) => results.push(*n),
                    HirExprKind::Literal(HirLiteral::Bool(b)) => {
                        results.push(if *b { 1 } else { 0 });
                    }
                    HirExprKind::Identifier(n) => results.push(*bindings.get(n)?),
                    HirExprKind::Binary { left, right, .. } => {
                        stack.push(Frame::Combine(e));
                        stack.push(Frame::Visit(right));
                        stack.push(Frame::Visit(left));
                    }
                    HirExprKind::Unary { inner, .. } | HirExprKind::Cast { inner, .. } => {
                        stack.push(Frame::Combine(e));
                        stack.push(Frame::Visit(inner));
                    }
                    _ => return None,
                },
                Frame::Combine(e) => {
                    if !fold_frame(e, &mut results) {
                        return None;
                    }
                }
            }
        }
        results.pop()
    }

    fn chain_is_composable(&self, chain: &[String]) -> bool {
        if !self.chain_links_single_writer(chain) {
            return false;
        }
        let has_ffi = chain
            .iter()
            .filter_map(|tn| self.txn_bodies.get(tn))
            .any(|body| body.iter().any(has_ffi_or_terminator_stmt));
        if has_ffi {
            return false;
        }
        self.chain_contracts_agree(chain)
    }

    fn chain_links_single_writer(&self, chain: &[String]) -> bool {
        for i in 0..chain.len().saturating_sub(1) {
            let a = &chain[i];
            let Some(writes_a) = self.txn_writes.get(a) else { continue };
            let Some(reads_next) = self.txn_reads.get(&chain[i + 1]) else { continue };
            for sv in writes_a.iter().filter(|w| reads_next.contains(*w)) {
                if self.is_chain_counter_var(chain, sv) {
                    continue;
                }
                let writer_count = chain
                    .iter()
                    .filter(|tn| {
                        self.txn_writes
                            .get(*tn)
                            .map(|w| w.contains(sv))
                            .unwrap_or(false)
                    })
                    .count();
                if writer_count > 1 {
                    return false;
                }
            }
        }
        true
    }

    fn chain_contracts_agree(&self, chain: &[String]) -> bool {
        let mut common_pre_var: Option<String> = None;
        let mut common_bound_var: Option<String> = None;
        for tn in chain {
            if let Some(body) = self.txn_bodies.get(tn) {
                let Some(cv) = find_counter_var(body) else { return false };
                if !merge_common(&mut common_pre_var, cv) {
                    return false;
                }
            }
            if let Some(bound) = self.iter_bounds.get(tn) {
                if !merge_common(&mut common_bound_var, bound.to_string()) {
                    return false;
                }
            }
        }
        true
    }

    fn push_composed_chain(
        &mut self,
        plan: ComposedPlan<'_>,
        trigger_values: Option<&[(String, i64)]>,
        all_internal: bool,
    ) {
        let hir_body = self.build_composed_body(plan.chain, plan.link_vars, trigger_values);
        let Some(composed_body) = crate::hir::to_ast::stmt_body(&hir_body) else {
            // An unsupported HIR statement (`Other`, e.g. an inline compile-time
            // definition) cannot cross to the AST result boundary. The AST
            // analyzer has no equivalent HIR-free body for it, so the chain is
            // not surfaced until HIR represents that statement.
            return;
        };
        let counter_var = self.iter_bounds.get(&plan.chain[0]).and_then(|_| {
            self.txn_bodies
                .get(&plan.chain[0])
                .and_then(|body| find_counter_var(body))
        });
        let fused_weight = count_statements_recursive(&hir_body);

        self.composed_chains.push(ComposedChain {
            chain: plan.chain.to_vec(),
            link_vars: plan.link_vars.to_vec(),
            root_triggers: plan.root_triggers.to_vec(),
            composed_body,
            counter_var,
            fused_weight,
            trigger_values: trigger_values.map(|tv| tv.to_vec()),
            all_internal,
        });
        self.composed_body_hir.push(hir_body);
    }

    fn is_chain_counter_var(&self, chain: &[String], var: &str) -> bool {
        chain.iter().any(|tn| {
            self.txn_bodies
                .get(tn)
                .map(|body| find_counter_var(body).as_deref() == Some(var))
                .unwrap_or(false)
        })
    }

    fn compute_link_vars(&self, chain: &[String]) -> Vec<String> {
        let mut links = Vec::new();
        for i in 0..chain.len().saturating_sub(1) {
            let (a, b) = (&chain[i], &chain[i + 1]);
            if let (Some(writes_a), Some(reads_b)) = (self.txn_writes.get(a), self.txn_reads.get(b)) {
                let mut shared: Vec<&String> =
                    writes_a.iter().filter(|w| reads_b.contains(*w)).collect();
                shared.sort();
                if let Some(w) = shared.first() {
                    links.push((*w).clone());
                }
            }
        }
        links
    }

    fn build_composed_body(
        &self,
        chain: &[String],
        link_vars: &[String],
        trigger_values: Option<&[(String, i64)]>,
    ) -> Vec<HirStmt> {
        let mut result = Vec::new();
        for (i, txn_name) in chain.iter().enumerate() {
            let Some(body) = self.txn_bodies.get(txn_name) else { continue };
            let counter_var = find_counter_var(body);
            let spec = LinkSpec { chain, link_vars, index: i };
            let stmts = self.composed_stmts(body, spec, trigger_values);
            result.extend(filter_counter(stmts, i, counter_var.as_deref()));
        }
        result
    }

    fn composed_stmts(
        &self,
        body: &[HirStmt],
        spec: LinkSpec<'_>,
        trigger_values: Option<&[(String, i64)]>,
    ) -> Vec<HirStmt> {
        let stmts = body.to_vec();
        if spec.index == 0 {
            return apply_trigger_values(stmts, trigger_values);
        }
        self.substitute_link(stmts, spec, trigger_values)
    }

    fn substitute_link(
        &self,
        stmts: Vec<HirStmt>,
        spec: LinkSpec<'_>,
        trigger_values: Option<&[(String, i64)]>,
    ) -> Vec<HirStmt> {
        let Some(prev_body) = self.txn_bodies.get(&spec.chain[spec.index - 1]) else {
            return stmts;
        };
        let Some(link_var) = spec.link_vars.get(spec.index - 1) else { return stmts };
        let mut write_expr = find_write_expr(prev_body, link_var);
        if spec.index == 1 {
            if let (Some(tv), Some(we)) = (trigger_values, write_expr.as_ref()) {
                let mut subs = we.clone();
                for (trg_name, trg_val) in tv {
                    subs = substitute_expr(&subs, trg_name, &int_expr(*trg_val));
                }
                write_expr = Some(subs);
            }
        }
        match write_expr {
            Some(we) => substitute_var(&stmts, link_var, &we),
            None => stmts,
        }
    }

    fn var_is_chain_internal(
        &self,
        var: &str,
        chain: &[String],
        all_txn_names: &HashSet<String>,
    ) -> bool {
        for txn in all_txn_names {
            if chain.contains(txn) {
                continue;
            }
            if self.txn_reads.get(txn).map(|r| r.contains(var)).unwrap_or(false) {
                return false;
            }
            if self.txn_writes.get(txn).map(|w| w.contains(var)).unwrap_or(false) {
                return false;
            }
        }
        true
    }

    fn update_scores_after_composition(&mut self) {
        for cc in &self.composed_chains {
            for score in &mut self.region_scores {
                if cc.chain.iter().any(|c| score.txn_names.contains(c)) {
                    score.chain_composed = true;
                    score.body_weight = cc.fused_weight;
                    score.optimization_score *= 1.5;
                }
            }
        }
    }
}

fn int_expr(value: i64) -> HirExpr {
    HirExpr {
        id: crate::hir::ids::NodeId(0),
        span: None,
        ty: None,
        kind: HirExprKind::Literal(HirLiteral::Int(value)),
    }
}

/// Combine one post-order expression frame. Returns false when evaluation is
/// not possible (division-by-zero is preserved as an AST-parity panic path).
fn fold_frame(e: &HirExpr, results: &mut Vec<i64>) -> bool {
    match &e.kind {
        HirExprKind::Binary { op: BinaryOpKind::Concat, .. } => true,
        HirExprKind::Binary { op, .. } => {
            let Some(right) = results.pop() else { return false };
            let Some(left) = results.pop() else { return false };
            let value = match op {
                BinaryOpKind::Add => left.wrapping_add(right),
                BinaryOpKind::Sub => left.wrapping_sub(right),
                BinaryOpKind::Mul => left.wrapping_mul(right),
                BinaryOpKind::Div => left / right,
                BinaryOpKind::Mod => left % right,
                BinaryOpKind::And => i64::from(left != 0 && right != 0),
                BinaryOpKind::Or => i64::from(left != 0 || right != 0),
                BinaryOpKind::Eq => i64::from(left == right),
                BinaryOpKind::Neq => i64::from(left != right),
                BinaryOpKind::Lt => i64::from(left < right),
                BinaryOpKind::Le => i64::from(left <= right),
                BinaryOpKind::Gt => i64::from(left > right),
                BinaryOpKind::Ge => i64::from(left >= right),
                BinaryOpKind::BitAnd => left & right,
                BinaryOpKind::BitOr => left | right,
                BinaryOpKind::BitXor => left ^ right,
                BinaryOpKind::Shl => left << (right as u32 & 63),
                BinaryOpKind::Shr => left >> (right as u32 & 63),
                BinaryOpKind::Concat => 0,
            };
            results.push(value);
            true
        }
        HirExprKind::Unary { op, .. } => {
            let Some(v) = results.pop() else { return false };
            results.push(match op {
                UnaryOpKind::Not => i64::from(v == 0),
                UnaryOpKind::Neg => -v,
                UnaryOpKind::BitNot => !v,
            });
            true
        }
        HirExprKind::Cast { .. } => true,
        _ => true,
    }
}

/// Test-only HIR dump. Uses the shared canonicalisation in `analysis::region`
/// so AST and HIR render byte-for-byte identically.
#[cfg(test)]
pub(crate) fn canonical_dump(
    analyzer: &RegionAnalyzer,
) -> std::collections::BTreeMap<String, String> {
    crate::analysis::region::canonical_dump_parts(crate::analysis::region::CanonicalParts {
        var_info: &analyzer.var_info,
        trigger_vars: &analyzer.trigger_vars,
        regions: &analyzer.regions,
        linear_chains: &analyzer.linear_chains,
        iter_bounds: &analyzer.iter_bounds,
        region_scores: &analyzer.region_scores,
        budget_plan: &analyzer.budget_plan,
        composed_chains: &analyzer.composed_chains,
    })
}

#[cfg(test)]
mod tests;
