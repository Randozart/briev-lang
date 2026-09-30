// ── Optimization Strategy Selection ──────────────────────────────
//
// Phase 7: Extracted from optimizer.rs (now deleted as part of Phase 1
// heuristic bloat removal). This module retains only the structural
// dispatch decision tree — NOT any heuristic optimization pass.
//
// Phase 4 will simplify this to a 4-way structural decision tree.
// Until then, this code is preserved as-is for correctness.

use crate::ast::{BinaryOpKind, Expr, TopLevel, Transaction};
use crate::backend::llvm::DispatchMode;
use crate::backend::AnalysisResults;
use std::collections::{HashMap, HashSet};

use crate::backend::llvm::LlvmBackend;

/// Result of the optimization strategy selection.
pub struct OptimizationStrategy {
    pub dispatch_mode: DispatchMode,
    pub has_wake_triggers: bool,
    pub enumerable: Option<Vec<(String, Option<u64>)>>,
    pub enum_keys: HashMap<String, Vec<i64>>,
    pub enum_txn_names: HashSet<String>,
}

impl LlvmBackend {
    /// Select the optimization strategy: dispatch mode, txn categorization,
    /// and async/lightweight classification. Fills self.has_async_txns,
    /// self.is_lightweight_async, self.async_txn_names.
    pub fn select_optimization_strategy(
        &mut self,
        program: &[TopLevel],
        analysis: &AnalysisResults,
        txns: &[(String, &Transaction)],
    ) -> OptimizationStrategy {
        let dispatch_mode = Self::select_dispatch_mode(program, txns);
        // 2026-08-09 (Bug 2): a sync<group> node is a barrier, not async
        // parallelism — exclude its members from async auto-candidacy so they
        // stay on the sequential reactor (a group whose members went async
        // produced an empty reactor_tick and never fired).
        let sync_members: HashSet<String> = program
            .iter()
            .filter_map(|item| match item {
                TopLevel::SyncGroup { item: inner, .. } => match inner.as_ref() {
                    TopLevel::Transaction(t) => Some(t.name.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let (has_wake_triggers, enumerable, enum_keys, enum_txn_names) =
            Self::classify_txns(self, analysis, txns, &sync_members);
        OptimizationStrategy {
            dispatch_mode,
            has_wake_triggers,
            enumerable,
            enum_keys,
            enum_txn_names,
        }
    }

    /// Auto-select Parallel dispatch when all reactive transactions
    /// are proven conflict-free.
    fn select_dispatch_mode(program: &[TopLevel], txns: &[(String, &Transaction)]) -> DispatchMode {
        let reactive: Vec<&Transaction> = txns
            .iter()
            .filter(|(_, t)| t.is_reactive)
            .map(|(_, t)| *t)
            .collect();
        let mut cf = true;
        for i in 0..reactive.len() {
            for j in (i + 1)..reactive.len() {
                let a = reactive[i];
                let b = reactive[j];
                let a_writes: HashSet<String> =
                    crate::backend::collect_assigned_identifiers(&a.body)
                        .into_iter()
                        .collect();
                let b_writes: HashSet<String> =
                    crate::backend::collect_assigned_identifiers(&b.body)
                        .into_iter()
                        .collect();
                let a_reads = crate::backend::collect_read_identifiers(&a.body);
                let b_reads = crate::backend::collect_read_identifiers(&b.body);
                // 2026-07-30: Write-write AND read-write conflicts are checked
                // UNCONDITIONALLY. Briev's reactor design (per AGENTS.md) states:
                //   "If two nodes firing together would lead to a race condition
                //    due to one reading or one writing or both writing, deny
                //    compilation. Writing is a XOR condition."
                // Previously the read-write checks were gated behind the
                // precondition-identifier overlap — two nodes with disjoint
                // preconditions but overlapping read/write sets were wrongly
                // classified as Parallel, producing a race.
                if !a_writes.is_disjoint(&b_writes) {
                    cf = false;
                    break;
                }
                if !a_writes.is_disjoint(&b_reads) {
                    cf = false;
                    break;
                }
                if !b_writes.is_disjoint(&a_reads) {
                    cf = false;
                    break;
                }
            }
            if !cf {
                break;
            }
        }
        if cf {
            DispatchMode::Parallel
        } else {
            DispatchMode::Sequential
        }
    }

    /// Categorize reactive transactions into enum/async/sequential dispatch paths.
    fn classify_txns(
        &mut self,
        analysis: &AnalysisResults,
        txns: &[(String, &Transaction)],
        sync_members: &HashSet<String>,
    ) -> (
        bool,
        Option<Vec<(String, Option<u64>)>>,
        HashMap<String, Vec<i64>>,
        HashSet<String>,
    ) {
        let has_wake_triggers = self.ctx.triggers.values().any(|t| t.is_wake);

        let (enumerable, enum_keys): (
            Option<Vec<(String, Option<u64>)>>,
            HashMap<String, Vec<i64>>,
        ) = {
            let region = &analysis.region_analyzer;
            if !self.ctx.trigger_names.is_empty() {
                let mut sizes = Vec::new();
                let mut total: u64 = 1;
                let mut ok = true;
                let mut fallback_triggers = Vec::new();
                for tn in &self.ctx.trigger_names {
                    let sz = region.value_set_size_of(tn);
                    if let Some(s) = sz {
                        total = total.saturating_mul(s);
                        if total > self.ctx.optimize_budget {
                            ok = false;
                            break;
                        }
                        sizes.push((tn.clone(), sz));
                    } else {
                        fallback_triggers.push(tn.clone());
                    }
                }
                if ok && sizes.len() == self.ctx.trigger_names.len() {
                    (Some(sizes), HashMap::new())
                } else if !fallback_triggers.is_empty() {
                    let trigger_set: HashSet<&str> =
                        self.ctx.trigger_names.iter().map(|s| s.as_str()).collect();
                    let mut keys_map = HashMap::new();
                    for tn in &fallback_triggers {
                        for (_, txn) in txns {
                            if !txn.is_reactive {
                                continue;
                            }
                            if let Some(keys) =
                                extract_trigger_keys(&txn.contract.pre_condition, &trigger_set)
                            {
                                keys_map.insert(tn.clone(), keys);
                                break;
                            }
                        }
                    }
                    if !keys_map.is_empty() {
                        let mut combined_sizes = sizes;
                        let mut combined_total = total;
                        let mut all_ok = true;
                        for tn in &self.ctx.trigger_names {
                            if combined_sizes.iter().any(|(n, _)| n == tn) {
                                continue;
                            }
                            if let Some(keys) = keys_map.get(tn) {
                                let s = keys.len() as u64;
                                combined_total = combined_total.saturating_mul(s);
                                if combined_total > self.ctx.optimize_budget {
                                    all_ok = false;
                                    break;
                                }
                                combined_sizes.push((tn.clone(), Some(s)));
                            } else {
                                all_ok = false;
                                break;
                            }
                        }
                        if all_ok {
                            (Some(combined_sizes), keys_map)
                        } else {
                            (None, HashMap::new())
                        }
                    } else {
                        (None, HashMap::new())
                    }
                } else {
                    (None, HashMap::new())
                }
            } else {
                (None, HashMap::new())
            }
        };

        let enum_trigger_names: HashSet<&str> = enumerable
            .as_ref()
            .map(|en| en.iter().map(|(n, _)| n.as_str()).collect())
            .unwrap_or_default();
        let enum_txn_names: HashSet<String> = txns
            .iter()
            .filter(|(_, t)| {
                t.is_reactive && is_trigger_gated(&t.contract.pre_condition, &enum_trigger_names)
            })
            .map(|(n, _)| n.clone())
            .collect();

        let async_candidates: Vec<&Transaction> = txns
            .iter()
            .filter(|(n, t)| {
                t.is_reactive
                    && !enum_txn_names.contains(n.as_str())
                    // 2026-08-09 (Bug 2): a sync<group> member is explicitly
                    // grouped — the group is a barrier, NOT async parallelism.
                    // Excluding members keeps them on the sequential reactor
                    // (the group barrier holds their completion), so unequal
                    // firing schedules can't silently drop them to the async
                    // thread pool (which produced an empty reactor_tick).
                    && !sync_members.contains(n.as_str())
            })
            .map(|(_, t)| *t)
            .collect();
        let ac_writes: Vec<HashSet<String>> = async_candidates
            .iter()
            .map(|t| {
                crate::backend::collect_assigned_identifiers(&t.body)
                    .into_iter()
                    .collect()
            })
            .collect();
        let ac_reads: Vec<HashSet<String>> = async_candidates
            .iter()
            .map(|t| crate::backend::collect_read_identifiers(&t.body))
            .collect();
        let mut is_async_eligible: Vec<bool> = vec![true; async_candidates.len()];
        for i in 0..async_candidates.len() {
            for j in (i + 1)..async_candidates.len() {
                let has_conflict = !ac_writes[i].is_disjoint(&ac_writes[j])
                    || !ac_writes[i].is_disjoint(&ac_reads[j])
                    || !ac_writes[j].is_disjoint(&ac_reads[i]);
                if has_conflict {
                    is_async_eligible[i] = false;
                    is_async_eligible[j] = false;
                }
            }
        }
        let all_async_eligible =
            async_candidates.len() >= 2 && is_async_eligible.iter().all(|&x| x);
        let mut async_txn_names: HashSet<String> = HashSet::new();
        if all_async_eligible {
            for ac in &async_candidates {
                async_txn_names.insert(ac.name.clone());
            }
        }
        for txn in &async_candidates {
            if txn.is_async {
                async_txn_names.insert(txn.name.clone());
            }
        }

        self.has_async_txns = !async_txn_names.is_empty();
        // 2026-09-30 (BUGS.md: async call order nondeterminism): this
        // used to be a bare `HashSet::iter().collect()` — SipHash's
        // per-process seed randomized the async-body call order in main,
        // producing different .ll bytes run-to-run (A/B verification of
        // `async_counters`/`async_counters_idio` caught it; verified
        // pre-existing: 4 runs of the SAME pre-fix compiler → 2 distinct
        // hashes). House rule §4: HashMap/HashSet iteration producing
        // LLVM IR MUST be sorted. The language treats async bodies as
        // unordered (the sequential fallback is an arbitrary order), so
        // sorted names pick one stable order — alphabetical.
        self.async_txn_names = async_txn_names.iter().cloned().collect();
        self.async_txn_names.sort();
        self.async_thread_pool_size = self.async_txn_names.len() as u32;

        if !async_txn_names.is_empty() {
            let all_lightweight = async_txn_names.iter().all(|name| {
                analysis
                    .transition_graph
                    .nodes
                    .iter()
                    .find(|n| n.name == *name)
                    .map_or(false, |node| {
                        let is_pure = node.is_pure_body || node.is_effectively_pure;
                        if !is_pure {
                            return false;
                        }
                        if let Some(ref bp) = node.bounded_pre {
                            let is_const = self
                                .ctx
                                .field_initializers
                                .get(&bp.bound_var)
                                .and_then(|e| e.as_ref())
                                .map_or(false, |e| matches!(e, Expr::Decimal(_)))
                                || self
                                    .ctx
                                    .constants
                                    .get(&bp.bound_var)
                                    .map_or(false, |(_, e)| matches!(e, Expr::Decimal(_)));
                            !is_const
                        } else {
                            false
                        }
                    })
            });
            if all_lightweight {
                self.is_lightweight_async = true;
            }
        }

        (has_wake_triggers, enumerable, enum_keys, enum_txn_names)
    }
}

/// Check if a precondition is gated on any of the named triggers.
fn is_trigger_gated(pre: &Expr, trigger_names: &HashSet<&str>) -> bool {
    match pre {
        Expr::Identifier(name) => trigger_names.contains(name.as_str()),
        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => {
            matches!(l.as_ref(), Expr::Identifier(name) if trigger_names.contains(name.as_str()))
                || matches!(r.as_ref(), Expr::Identifier(name) if trigger_names.contains(name.as_str()))
        }
        Expr::BinaryOp(BinaryOpKind::And, l, r) => is_trigger_gated(l.as_ref(), trigger_names) || is_trigger_gated(r.as_ref(), trigger_names),
        _ => false,
    }
}

/// Extract trigger values from a precondition that match Eq(trigger, value).
fn extract_trigger_keys(pre: &Expr, trigger_names: &HashSet<&str>) -> Option<Vec<i64>> {
    let mut keys = Vec::new();
    match pre {
        Expr::BinaryOp(BinaryOpKind::Eq, l, r) => {
            let (ident, val) = if let (Expr::Identifier(name), Expr::Decimal(n)) =
                (l.as_ref(), r.as_ref())
            {
                (name.clone(), *n)
            } else if let (Expr::Decimal(n), Expr::Identifier(name)) = (l.as_ref(), r.as_ref()) {
                (name.clone(), *n)
            } else {
                return None;
            };
            if trigger_names.contains(ident.as_str()) {
                keys.push(val);
            } else {
                return None;
            }
        }
        Expr::BinaryOp(BinaryOpKind::Or, l, r) => {
            keys.extend(extract_trigger_keys(l, trigger_names)?);
            keys.extend(extract_trigger_keys(r, trigger_names)?);
        }
        Expr::BinaryOp(BinaryOpKind::And, l, r) => {
            if let Some(k) = extract_trigger_keys(l, trigger_names) {
                keys.extend(k);
            } else if let Some(k) = extract_trigger_keys(r, trigger_names) {
                keys.extend(k);
            } else {
                return None;
            }
        }
        _ => return None,
    }
    Some(keys)
}
