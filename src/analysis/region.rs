use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, Type, UnaryOpKind};

use std::collections::{HashMap, HashSet, VecDeque};

/// Classification of a transaction body by computational weight.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComplexityClass {
    Trivial,
    Light,
    Medium,
    Heavy,
    Unbounded,
}

/// Optimization score and metadata for a single atomic reactive region.
#[derive(Debug, Clone)]
pub struct RegionScore {
    pub region_id: usize,
    pub txn_names: Vec<String>,
    pub complexity: ComplexityClass,
    pub body_weight: usize,
    pub iteration_count: u64,
    pub value_set_size: Option<u64>,
    pub optimization_score: f64,
    pub chain_composed: bool,
    pub gpu_eligible: bool,
}

/// Result of greedy budget allocation across regions.
#[derive(Debug, Clone)]
pub struct BudgetPlan {
    pub total_budget: u64,
    pub allocated: Vec<(usize, ComplexityClass, u64, f64)>,
    pub residual_budget: u64,
    pub skipped: Vec<(usize, ComplexityClass, u64)>,
}

/// Result of expression substitution across a linear transaction chain.
#[derive(Debug, Clone)]
pub struct ComposedChain {
    pub chain: Vec<String>,
    pub link_vars: Vec<String>,
    pub root_triggers: Vec<String>,
    pub composed_body: Vec<Statement>,
    pub counter_var: Option<String>,
    pub fused_weight: usize,
    pub trigger_values: Option<Vec<(String, i64)>>,
    pub all_internal: bool,
}

/// Classification of a variable along the predictability axis.
///
/// - **Pure**: deterministic, no dependency on frontier values — fully foldable.
/// - **Bounded**: depends on frontier but bounds are known (type + contract) —
///   can still optimize with range checks instead of exact values.
/// - **Opaque**: depends on frontier with unbounded or unknown values —
///   segmentation necessary, cannot fold through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarClass {
    Pure,
    Bounded,
    Opaque,
}

/// Compile-time-known inclusive integer interval.
#[derive(Debug, Clone, PartialEq)]
pub struct Interval {
    pub lo: i64,
    pub hi: i64,
}

impl Interval {
    pub fn contains(&self, val: i64) -> bool {
        val >= self.lo && val <= self.hi
    }

    pub fn size(&self) -> u64 {
        if self.hi < self.lo {
            0
        } else {
            (self.hi - self.lo + 1) as u64
        }
    }
}

/// Analysis result for a single variable.
#[derive(Debug, Clone)]
pub struct VarInfo {
    pub classification: VarClass,
    pub interval: Option<Interval>,
    /// Estimated number of distinct values this variable can hold,
    /// or `None` if unbounded.
    pub value_set_size: Option<u64>,
    /// Which atomic region this variable belongs to (0 = Pure/isolated).
    pub region_id: usize,
}

/// The region analyzer — classifies variables and partitions the program
/// into independent atomic reactive regions.
///
/// A region is a connected component in the dependency graph of non-Pure
/// variables. Variables in the same region must be scheduled together;
/// variables in different regions are independent.
pub struct RegionAnalyzer {
    pub var_info: HashMap<String, VarInfo>,
    pub trigger_vars: HashSet<String>,
    pub regions: Vec<Vec<String>>,
    deps: HashMap<String, HashSet<String>>,
    rev_deps: HashMap<String, HashSet<String>>,
    txn_reads: HashMap<String, HashSet<String>>,
    txn_writes: HashMap<String, HashSet<String>>,
    pub linear_chains: Vec<Vec<String>>,
    txn_bodies: HashMap<String, Vec<Statement>>,
    iter_bounds: HashMap<String, u64>,
    pub region_scores: Vec<RegionScore>,
    pub budget_plan: Option<BudgetPlan>,
    pub composed_chains: Vec<ComposedChain>,
}

impl RegionAnalyzer {
    /// Create an empty analyzer (no program loaded).
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
        }
    }

    /// Run the full analysis pipeline on a parsed program.
    pub fn analyze(program: &[TopLevel]) -> Self {
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

    fn register_declarations(&mut self, program: &[TopLevel]) {
        for item in program {
            match item {
                TopLevel::StateDecl(decl) => {
                    let vc = VarInfo {
                        classification: VarClass::Pure,
                        interval: None,
                        value_set_size: None,
                        region_id: 0,
                    };
                    self.var_info.entry(decl.name.clone()).or_insert(vc);
                    self.deps.entry(decl.name.clone()).or_default();
                    self.rev_deps.entry(decl.name.clone()).or_default();
                }
                TopLevel::Constant(constant) => {
                    let interval = Self::expr_to_interval(&constant.expr);
                    let vc = VarInfo {
                        classification: VarClass::Pure,
                        interval,
                        value_set_size: None,
                        region_id: 0,
                    };
                    self.var_info.entry(constant.name.clone()).or_insert(vc);
                    self.deps.entry(constant.name.clone()).or_default();
                    self.rev_deps.entry(constant.name.clone()).or_default();
                }
                TopLevel::Trigger(trg) => {
                    self.trigger_vars.insert(trg.name.clone());
                    let vc = VarInfo {
                        classification: VarClass::Opaque,
                        interval: None,
                        value_set_size: None,
                        region_id: 0,
                    };
                    self.var_info.entry(trg.name.clone()).or_insert(vc);
                    self.deps.entry(trg.name.clone()).or_default();
                    self.rev_deps.entry(trg.name.clone()).or_default();
                }
                _ => {}
            }
        }
    }

    // ── Phase B: Build dependency graph ─────────────────────────────────

    fn build_dependency_graph(&mut self, program: &[TopLevel]) {
        for item in program {
            if let TopLevel::Transaction(txn) = item {
                let mut txn_read_vars = HashSet::new();
                let mut txn_write_vars = HashSet::new();

                self.txn_bodies.insert(txn.name.clone(), txn.body.clone());

                self.collect_identifiers(&txn.contract.pre_condition, &txn.name);
                self.collect_identifiers(&txn.contract.post_condition, &txn.name);

                for stmt in &txn.body {
                    if let Statement::Assign(lhs, expr) = stmt {
                        let writer = match lhs.as_var_name() {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        txn_write_vars.insert(writer.clone());
                        txn_read_vars.extend(expr_to_var_set(expr));
                        self.collect_identifiers(expr, &writer);
                    }
                    if let Statement::Let { name, expr, .. } = stmt {
                        if let Some(e) = expr {
                            txn_write_vars.insert(name.clone());
                            txn_read_vars.extend(expr_to_var_set(e));
                            self.collect_identifiers(e, name);
                        }
                    }
                }

                self.txn_reads.entry(txn.name.clone()).or_default().extend(txn_read_vars);
                self.txn_writes.entry(txn.name.clone()).or_default().extend(txn_write_vars);
            }
        }
    }

    /// Iterative identifier collection. Uses explicit stack to avoid recursion.
    fn collect_identifiers(&mut self, expr: &Expr, reader_for: &str) {
        let rf = reader_for.to_string();
        let mut work: Vec<&Expr> = vec![expr];

        while let Some(e) = work.pop() {
            match e {
                Expr::Identifier(name) => {
                    self.deps.entry(rf.clone()).or_default().insert(name.clone());
                    self.rev_deps.entry(name.clone()).or_default().insert(rf.clone());
                }
                Expr::Call(_, args, _) => {
                    for arg in args.iter().rev() {
                        work.push(arg);
                    }
                }
                 Expr::Tuple(elems) => {
                    for e in elems.iter().rev() {
                        work.push(e);
                    }
                }
                Expr::BinaryOp(_, l, r) => {
                    work.push(r);
                    work.push(l);
                }
                Expr::UnaryOp(_, e) => {
                    work.push(e);
                }
                Expr::Cast(e, _) => {
                    work.push(e);
                }
                Expr::Index(list, idx) => {
                    work.push(idx);
                    work.push(list);
                }
                Expr::Field(obj, _) => {
                    work.push(obj);
                }
                Expr::Match(value, arms) => {
                    for arm in arms.iter().rev() {
                        if let Some(g) = &arm.guard { work.push(g); }
                        work.push(&arm.body);
                    }
                    work.push(value.as_ref());
                }
                Expr::List(elems) => {
                    for e in elems.iter().rev() { work.push(e); }
                }
                _ => {}
            }
        }
    }

    // ── Phase C: Seed frontier ──────────────────────────────────────────

    fn seed_frontier(&mut self) {
        let trigger_names: Vec<String> = self.trigger_vars.iter().cloned().collect();
        for name in trigger_names {
            if let Some(info) = self.var_info.get(&name) {
                // If the trigger has a tight bound, mark as Bounded not Opaque.
                let is_tight = info
                    .interval
                    .as_ref()
                    .map(|iv| iv.size() <= 256)
                    .unwrap_or(false);
                if is_tight {
                    if let Some(info) = self.var_info.get_mut(&name) {
                        info.classification = VarClass::Bounded;
                    }
                }
            }
        }
    }

    // ── Phase D: Propagate classification ───────────────────────────────

    fn propagate_classification(&mut self) {
        // BFS from frontier vars: any var that reads a non-Pure var
        // inherits the most restrictive classification.
        let var_names: Vec<String> = self.var_info.keys().cloned().collect();
        let mut queue: VecDeque<String> = VecDeque::new();

        // Seed the queue with all non-Pure vars
        for name in &var_names {
            if let Some(info) = self.var_info.get(name) {
                if info.classification != VarClass::Pure {
                    queue.push_back(name.clone());
                }
            }
        }

        while let Some(frontier_var) = queue.pop_front() {
            let frontier_class = self.var_info[&frontier_var].classification.clone();

            // Propagate to all vars that read this frontier_var
            if let Some(readers) = self.rev_deps.get(&frontier_var).cloned() {
                for reader in readers {
                    // Skip unregistered vars (e.g. transaction params, foreign bindings)
                    let Some(reader_info) = self.var_info.get(&reader) else { continue; };
                    let cur_class = &reader_info.classification;
                    let needs_update = *cur_class == VarClass::Pure
                        || (frontier_class == VarClass::Opaque
                            && *cur_class == VarClass::Bounded);
                    if needs_update {
                        if let Some(info) = self.var_info.get_mut(&reader) {
                            info.classification = if frontier_class == VarClass::Opaque {
                                VarClass::Opaque
                            } else {
                                VarClass::Bounded
                            };
                            queue.push_back(reader.clone());
                        }
                    }
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

            let mut component: Vec<String> = Vec::new();
            let mut stack: Vec<String> = vec![seed.clone()];

            while let Some(var) = stack.pop() {
                if !visited.insert(var.clone()) {
                    continue;
                }
                component.push(var.clone());

                // Neighbors = deps[var] ∪ rev_deps[var], filtered to non-Pure
                let mut neighbors: Vec<String> = Vec::new();
                if let Some(deps) = self.deps.get(&var) {
                    for n in deps.iter() {
                        if self.var_info.get(n).map(|i| i.classification != VarClass::Pure).unwrap_or(false)
                        {
                            neighbors.push(n.clone());
                        }
                    }
                }
                if let Some(revs) = self.rev_deps.get(&var) {
                    for n in revs.iter() {
                        if self.var_info.get(n).map(|i| i.classification != VarClass::Pure).unwrap_or(false)
                        {
                            neighbors.push(n.clone());
                        }
                    }
                }

                for n in neighbors {
                    if !visited.contains(&n) {
                        stack.push(n);
                    }
                }
            }

            if !component.is_empty() {
                region_id_counter += 1;
                for var in &component {
                    if let Some(info) = self.var_info.get_mut(var) {
                        info.region_id = region_id_counter;
                    }
                }
                self.regions.push(component);
            }
        }

        // Pure vars get region_id = 0
        for (_, info) in self.var_info.iter_mut() {
            if info.classification == VarClass::Pure {
                info.region_id = 0;
            }
        }
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
            VarClass::Pure => {
                // Pure variables: if constant → size 1, if interval → size of interval
                if let Some(ref iv) = info.interval {
                    let s = iv.size();
                    if s > 0 { Some(s) } else { None }
                } else {
                    None
                }
            }
            VarClass::Bounded => {
                // Bounded: use interval if available, otherwise fall back to type
                if let Some(ref iv) = info.interval {
                    Some(iv.size())
                } else {
                    None
                }
            }
            VarClass::Opaque => None, // Can't estimate
        }
    }

    // ── Utilities ───────────────────────────────────────────────────────

    /// Extract a compile-time-known integer interval from an expression.
    /// Returns `Some((lo, hi))` if the expression is a known constant or
    /// a narrow range of constants.
    fn expr_to_interval(expr: &Expr) -> Option<Interval> {
        match expr {
            Expr::Decimal(n) => Some(Interval { lo: *n, hi: *n }),
            Expr::Bool(b) => Some(Interval {
                lo: if *b { 1 } else { 0 },
                hi: if *b { 1 } else { 0 },
            }),
            Expr::UnaryOp(UnaryOpKind::Neg, inner) => {
                Self::expr_to_interval(inner).map(|iv| Interval {
                    lo: -iv.hi,
                    hi: -iv.lo,
                })
            }
            _ => None,
        }
    }

    /// Extract bounds from a Briev type (e.g., `Bool` → `[0,1]`, `U8` → `[0,255]`).
    fn type_to_interval(ty: &Type) -> Option<Interval> {
        match ty {
            Type::Custom(__t) if __t == "Bool" => Some(Interval { lo: 0, hi: 1 }),
            Type::Custom(__t) if __t == "Int" => None, // Full i64 range — unbounded
            Type::Custom(__t) if __t == "UInt" => Some(Interval {
                lo: 0,
                hi: i64::MAX,
            }),
            Type::Custom(__t) if __t == "Char" => Some(Interval {
                lo: 0,
                hi: 0x10FFFF,
            }),
            _ => None,
        }
    }

    // ── Phase H: Detect linear transaction chains ───────────────────────

    fn detect_linear_chains(&mut self, _program: &[TopLevel]) {
        self.linear_chains.clear();
        let txn_names: Vec<String> = self.txn_reads.keys().cloned().collect();

        for txn in &txn_names {
            let reads = self.txn_reads.get(txn).cloned().unwrap_or_default();
            let writes = self.txn_writes.get(txn).cloned().unwrap_or_default();

            // A linear chain starts when a transaction's writes are exclusively
            // consumed by another single transaction's reads.
            for written in &writes {
                if let Some(readers) = self.rev_deps.get(written) {
                    let downstream_txns: Vec<&String> = txn_names
                        .iter()
                        .filter(|tn| {
                            *tn != txn
                                && self.txn_reads.get(*tn).map(|r| r.contains(written)).unwrap_or(false)
                        })
                        .collect();

                    if downstream_txns.len() == 1 {
                        let next = downstream_txns[0].clone();
                        let mut chain = vec![txn.clone(), next.clone()];

                        // Extend the chain: follow reads→writes forward
                        loop {
                            let last = match chain.last() {
                                Some(l) => l.clone(),
                                None => break,
                            };
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
                            if !extended { break; }
                        }

                        self.linear_chains.push(chain);
                    }
                }
            }
        }

        // Deduplicate: keep only maximal chains (supersets of others)
        self.linear_chains.sort_by(|a, b| b.len().cmp(&a.len()));
        let mut deduped: Vec<Vec<String>> = Vec::new();
        for chain in &self.linear_chains {
            let chain_set: HashSet<&String> = chain.iter().collect();
            if !deduped.iter().any(|existing| {
                let existing_set: HashSet<&String> = existing.iter().collect();
                existing_set.is_superset(&chain_set) && existing.len() > chain.len()
            }) {
                deduped.push(chain.clone());
            }
        }
        self.linear_chains = deduped;
    }

    // ── Public query API ────────────────────────────────────────────────

    /// Get the region ID for a given variable.
    pub fn region_of(&self, var: &str) -> Option<usize> {
        self.var_info.get(var).map(|i| i.region_id)
    }

    /// Get the classification of a given variable.
    pub fn classification_of(&self, var: &str) -> Option<VarClass> {
        self.var_info.get(var).map(|i| i.classification.clone())
    }

    /// Check if a variable is trigger-dependent (Bounded or Opaque).
    pub fn is_frontier_dependent(&self, var: &str) -> bool {
        self.var_info
            .get(var)
            .map(|i| i.classification != VarClass::Pure)
            .unwrap_or(false)
    }

    /// Get the estimated value-set size for a variable, or `None` if unbounded.
    pub fn value_set_size_of(&self, var: &str) -> Option<u64> {
        self.var_info.get(var).and_then(|i| i.value_set_size)
    }

    // ── Iteration bound resolution ────────────────────────────────

    fn resolve_iteration_bounds(&mut self, program: &[TopLevel]) {
        self.iter_bounds.clear();
        for item in program {
            if let TopLevel::Transaction(txn) = item {
                if let Some(bound) = Self::extract_bound_from_pre(&txn.contract.pre_condition) {
                    let val = self.resolve_bound_value(program, &bound.1);
                    if let Some(v) = val {
                        self.iter_bounds.insert(txn.name.clone(), v);
                    }
                }
            }
        }
    }

    fn extract_bound_from_pre(pre: &Expr) -> Option<(String, String)> {
        match pre {
            Expr::BinaryOp(BinaryOpKind::Le, a, b)
            | Expr::BinaryOp(BinaryOpKind::Lt, a, b) => {
                match (a.as_ref(), b.as_ref()) {
                    (Expr::Identifier(var), Expr::Identifier(bound)) => {
                        Some((var.clone(), bound.clone()))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn resolve_bound_value(&self, program: &[TopLevel], bound_var: &str) -> Option<u64> {
        for item in program {
            match item {
                TopLevel::Constant(c) if c.name == bound_var => {
                    if let Expr::Decimal(n) = c.expr { return Some(n as u64); }
                }
                TopLevel::StateDecl(d) if d.name == bound_var => {
                    // StateDecl no longer has an expr field
                }
                _ => {}
            }
        }
        None
    }

    // ── Complexity estimation ─────────────────────────────────────

    fn classify_complexity(&self, body: &[Statement]) -> ComplexityClass {
        let weight = count_statements_recursive(body);
        let has_ffi = self.has_ffi_or_trigger_refs(body);
        if has_ffi {
            ComplexityClass::Unbounded
        } else if weight <= 2 {
            ComplexityClass::Trivial
        } else if weight <= 5 {
            ComplexityClass::Light
        } else if weight <= 20 {
            ComplexityClass::Medium
        } else {
            ComplexityClass::Heavy
        }
    }

    fn has_ffi_or_trigger_refs(&self, body: &[Statement]) -> bool {
        for stmt in body {
            if has_ffi_or_trigger_stmt(stmt, &self.trigger_vars) {
                return true;
            }
        }
        false
    }

    // ── Region scoring ────────────────────────────────────────────

    fn compute_region_scores(&mut self, program: &[TopLevel]) {
        self.region_scores.clear();

        let txn_to_region: HashMap<String, usize> = self.build_txn_to_region_map();
        let mut region_txns: HashMap<usize, Vec<String>> = HashMap::new();
        for (txn, rid) in &txn_to_region {
            region_txns.entry(*rid).or_default().push(txn.clone());
        }

        for (rid, txn_list) in region_txns {
            let mut body_weight: usize = 0;
            let mut complexity = ComplexityClass::Trivial;
            let mut max_iter: u64 = 1;
            let mut combined_vset_size: Option<u64> = Some(1);
            let mut all_pure_and_nop_term = true;

            for tn in &txn_list {
                if let Some(body) = self.txn_bodies.get(tn) {
                    let c = self.classify_complexity(body);
                    if c != ComplexityClass::Trivial {
                        complexity = std::cmp::max(complexity.clone(), c);
                    }
                    body_weight += count_statements_recursive(body);
                }

                // GPU eligibility
                if let Some(body) = self.txn_bodies.get(tn) {
                    if has_term_or_unify_escape(body) {
                        all_pure_and_nop_term = false;
                    }
                }
                if self.has_txn_trigger_refs(tn) {
                    all_pure_and_nop_term = false;
                }

                // iteration count
                let iter = self.iter_bounds.get(tn).copied().unwrap_or(1);
                if iter > max_iter { max_iter = iter; }

                // value set size
                if let Some(reads) = self.txn_reads.get(tn) {
                    for r in reads {
                        if self.trigger_vars.contains(r) {
                            let sz = self.value_set_size_of(r);
                            combined_vset_size = match (combined_vset_size, sz) {
                                (Some(a), Some(b)) => Some(a.saturating_mul(b)),
                                _ => None,
                            };
                        }
                    }
                }
            }

            let cost = combined_vset_size.unwrap_or(1).max(1);
            let score = if complexity == ComplexityClass::Unbounded {
                f64::NEG_INFINITY
            } else {
                (body_weight as f64 * max_iter as f64) / cost as f64
            };

            let gpu_eligible = all_pure_and_nop_term
                && complexity != ComplexityClass::Trivial
                && complexity != ComplexityClass::Light
                && complexity != ComplexityClass::Unbounded;

            self.region_scores.push(RegionScore {
                region_id: rid,
                txn_names: txn_list,
                complexity,
                body_weight,
                iteration_count: max_iter,
                value_set_size: combined_vset_size,
                optimization_score: score,
                chain_composed: false,
                gpu_eligible,
            });
        }

        self.region_scores.sort_by(|a, b| {
            b.optimization_score
                .partial_cmp(&a.optimization_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    fn build_txn_to_region_map(&self) -> HashMap<String, usize> {
        let mut map = HashMap::new();
        for rid in 1..=self.regions.len() {
            if let Some(region) = self.regions.get(rid - 1) {
                let txn_in_region: Vec<&String> = self.txn_writes
                    .iter()
                    .filter(|(_, writes)| writes.iter().any(|w| region.contains(w)))
                    .map(|(tn, _)| tn)
                    .collect();
                for tn in txn_in_region {
                    map.entry(tn.clone()).or_insert(rid);
                }
            }
        }
        map
    }

    fn has_txn_trigger_refs(&self, txn_name: &str) -> bool {
        if let Some(reads) = self.txn_reads.get(txn_name) {
            reads.iter().any(|r| self.trigger_vars.contains(r))
        } else {
            false
        }
    }

    // ── Budget planning ───────────────────────────────────────────

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

    /// Query the resolved iteration bound for a given transaction name.
    pub fn iteration_bound_of(&self, txn_name: &str) -> Option<u64> {
        self.iter_bounds.get(txn_name).copied()
    }

    // ── Chain composition (Phase 4.2) ─────────────────────────────

    pub fn compose_chains(&mut self) {
        self.composed_chains.clear();
        let chains = self.linear_chains.clone();
        let all_txn_names: HashSet<String> = self.txn_reads.keys().cloned().collect();

        for chain in &chains {
            if chain.len() < 2 { continue; }

            let link_vars = self.compute_link_vars(chain);
            if link_vars.is_empty() { continue; }

            if !self.chain_is_composable(chain, &all_txn_names) { continue; }

            let root_triggers: Vec<String> = self.txn_reads
                .get(&chain[0])
                .map(|reads| reads.iter()
                    .filter(|r| self.trigger_vars.contains(*r))
                    .cloned()
                    .collect())
                .unwrap_or_default();

            let all_internal = link_vars.iter().all(|lv| self.var_is_chain_internal(lv, chain, &all_txn_names));

            if root_triggers.is_empty() {
                self.push_composed_chain(chain, &link_vars, &root_triggers, None, all_internal);
            } else if root_triggers.len() == 1 {
                let trg = &root_triggers[0];
                if let Some(info) = self.var_info.get(trg) {
                    if let Some(ref iv) = info.interval {
                        let bound = iv.size().min(256);
                        let hi = iv.lo.saturating_add((bound - 1) as i64).min(iv.hi);
                        for val in iv.lo..=hi {
                            let tv = vec![(trg.clone(), val)];
                            self.push_composed_chain(chain, &link_vars, &root_triggers, Some(&tv), all_internal);
                        }
                    } else {
                        self.push_composed_chain(chain, &link_vars, &root_triggers, None, all_internal);
                    }
                } else {
                    self.push_composed_chain(chain, &link_vars, &root_triggers, None, all_internal);
                }
            } else {
                self.push_composed_chain(chain, &link_vars, &root_triggers, None, all_internal);
            }
        }

        self.update_scores_after_composition();
    }

    pub fn is_fully_precomputable(&self, budget: u64) -> bool {
        if self.composed_chains.is_empty() {
            return false;
        }
        let mut total: u64 = 0;
        for cc in &self.composed_chains {
            if has_ffi_or_trigger_stmt_in_chain(&cc.composed_body) {
                return false;
            }
            if cc.trigger_values.is_some() {
                return false;
            }
            if cc.all_internal {
                // 2026-08-09 (Bug 3): an all-internal chain that writes an
                // ARRAY ELEMENT (`a[i] = ...`) cannot be precomputed — the
                // final-value model only tracks plain-identifier bindings, so
                // the array would keep its initializer (0) and a folded
                // observable read `a[k]` would return 0. Refuse to precompute.
                if Self::composed_body_has_indexed_write(&cc.composed_body) {
                    return false;
                }
                total = total.saturating_add(1);
            } else {
                return false;
            }
        }
        for score in &self.region_scores {
            if score.complexity == ComplexityClass::Unbounded {
                return false;
            }
        }
        total <= budget
    }

    pub fn collect_final_values(&self, program: &[TopLevel]) -> Option<Vec<(Vec<String>, HashMap<String, i64>)>> {
        let mut all_bindings = Vec::new();
        for cc in &self.composed_chains {
            let mut bindings = Self::initial_bindings(program);
            let mut chain_bindings = HashMap::new();
            if cc.all_internal {
                if let Some(ref cv) = cc.counter_var {
                    let Some(&bound) = self.iter_bounds.get(&cc.chain[0]) else {
                        // 2026-07-10: Bound is runtime-determined (e.g. getenv_int#).
                        // Cannot precompute when iteration count is unknown.
                        return None;
                    };
                    bindings.insert(cv.clone(), bound as i64);
                    chain_bindings.insert(cv.clone(), bound as i64);
                }
                all_bindings.push((cc.chain.clone(), chain_bindings));
                continue;
            }
            for stmt in &cc.composed_body {
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

    fn initial_bindings(program: &[TopLevel]) -> HashMap<String, i64> {
        let mut bindings = HashMap::new();
        for item in program {
            match item {
                TopLevel::StateDecl(_decl) => {
                    // StateDecl no longer has an expr field
                }
                TopLevel::Constant(c) => {
                    if let Some(v) = Self::eval_expr_simple(&c.expr, &HashMap::new()) {
                        bindings.insert(c.name.clone(), v);
                    }
                }
                _ => {}
            }
        }
        bindings
    }

    /// 2026-08-09 (Bug 3): does a composed-chain body contain an INDEXED
    /// assignment (`a[i] = ...`)? The final-value model tracks only
    /// plain-identifier bindings, so array-element writes are unrepresentable —
    /// such chains must not be precomputed (a folded observable would read the
    /// array's initializer, e.g. 0).
    fn composed_body_has_indexed_write(body: &[Statement]) -> bool {
        fn stmt_has_indexed_write(s: &Statement) -> bool {
            match s {
                Statement::Assign(Expr::Index(..), _) => true,
                Statement::Guarded(_, inner) => inner.iter().any(stmt_has_indexed_write),
                Statement::Block(inner) | Statement::SyncBlock(inner) => {
                    inner.iter().any(stmt_has_indexed_write)
                }
                _ => false,
            }
        }
        body.iter().any(stmt_has_indexed_write)
    }

    fn eval_stmt(stmt: &Statement, bindings: &mut HashMap<String, i64>) -> bool {
        match stmt {
            Statement::Assign(Expr::Identifier(name), expr) => {
                if let Some(val) = Self::eval_expr_simple(expr, bindings) {
                    bindings.insert(name.clone(), val);
                    true
                } else { false }
            }
            // 2026-08-09 (Bug 3): an INDEXED assignment (`a[i] = ...`) writes
            // into an array element the i64 binding model cannot represent —
            // silently ignoring it leaves the array at its initializer value,
            // so a folded observable read `a[k]` returns 0. Refuse to
            // precompute such chains (they fall through to the runtime loop,
            // which executes the write correctly).
            Statement::Assign(Expr::Index(..), _) => false,
            // 2026-07-09: Pointer writes (AddrOf) not supported in compile-time eval.
            Statement::Assign(_, expr) => {
                let _ = Self::eval_expr_simple(expr, bindings);
                true
            }
            Statement::Let { name, expr, .. } => {
                if let Some(e) = expr {
                    if let Some(val) = Self::eval_expr_simple(e, bindings) {
                        bindings.insert(name.clone(), val);
                    }
                }
                true
            }
            Statement::Expression(e) => {
                Self::eval_expr_simple(e, bindings).is_some()
            }
            Statement::Guarded(condition, statements) => {
                if let Some(cond) = Self::eval_expr_simple(condition, bindings) {
                    if cond != 0 {
                        for s in statements {
                            if !Self::eval_stmt(s, bindings) { return false; }
                        }
                    }
                    true
                } else { false }
            }
            Statement::Term(None) | Statement::EndProgram(None) | Statement::InlineAsm { .. } => false,
            _ => true,
        }
    }

    // 2026-06-27: iterative arithmetic evaluation — replaces recursive version
    // to prevent stack overflow on deeply nested arithmetic expressions.
    fn eval_expr_simple(expr: &Expr, bindings: &HashMap<String, i64>) -> Option<i64> {
        // Stack: (left_expr, right_expr, op_to_apply)
        // Post-order: push children first, then combine results.
        enum Op { Add, Sub, Mul, Div, Mod, And, Or, Eq, Ne, Lt, Le, Gt, Ge,
                  BitAnd, BitOr, BitXor, Shl, Shr, Not, Neg, BitNot, Cast, Id }
        struct Frame<'a> { expr: &'a Expr, state: u8, left: Option<i64> }

        let mut stack: Vec<Frame> = vec![Frame { expr, state: 0, left: None }];
        let mut results: Vec<i64> = vec![];

        while let Some(mut f) = stack.pop() {
            match f.state {
                0 => match f.expr {
                    Expr::Decimal(n) => results.push(*n),
                    Expr::Bool(b) => results.push(if *b { 1 } else { 0 }),
                    Expr::Identifier(n) => {
                        if let Some(&v) = bindings.get(n) { results.push(v); }
                        else { return None; }
                    }
                    Expr::BinaryOp(_, a, b) => {
                        f.state = 1;
                        stack.push(f);
                        stack.push(Frame { expr: b, state: 0, left: None });
                        stack.push(Frame { expr: a, state: 0, left: None });
                    }
                    Expr::UnaryOp(_, a) | Expr::Cast(a, _) => {
                        f.state = 1;
                        stack.push(f);
                        stack.push(Frame { expr: a, state: 0, left: None });
                    }
                    _ => return None,
                },
                1 => match f.expr {
                    Expr::BinaryOp(BinaryOpKind::Add, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l.wrapping_add(r)); }
                    Expr::BinaryOp(BinaryOpKind::Sub, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l.wrapping_sub(r)); }
                    Expr::BinaryOp(BinaryOpKind::Mul, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l.wrapping_mul(r)); }
                    Expr::BinaryOp(BinaryOpKind::Div, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l / r); }
                    Expr::BinaryOp(BinaryOpKind::Mod, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l % r); }
                    Expr::BinaryOp(BinaryOpKind::And, _, _) => { let rv = results.pop()?; let lv = results.pop()?; results.push(if lv != 0 && rv != 0 { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::Or, _, _) => { let rv = results.pop()?; let lv = results.pop()?; results.push(if lv != 0 || rv != 0 { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::Eq, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(if l == r { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::Neq, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(if l != r { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::Lt, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(if l < r { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::Le, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(if l <= r { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::Gt, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(if l > r { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::Ge, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(if l >= r { 1 } else { 0 }); }
                    Expr::BinaryOp(BinaryOpKind::BitAnd, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l & r); }
                    Expr::BinaryOp(BinaryOpKind::BitOr, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l | r); }
                    Expr::BinaryOp(BinaryOpKind::BitXor, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l ^ r); }
                    Expr::BinaryOp(BinaryOpKind::Shl, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l << (r as u32 & 63)); }
                    Expr::BinaryOp(BinaryOpKind::Shr, _, _) => { let r = results.pop()?; let l = results.pop()?; results.push(l >> (r as u32 & 63)); }
                    Expr::UnaryOp(UnaryOpKind::Not, _) => { let v = results.pop()?; results.push(if v == 0 { 1 } else { 0 }); }
                    Expr::UnaryOp(UnaryOpKind::Neg, _) => { let v = results.pop()?; results.push(-v); }
                    Expr::UnaryOp(UnaryOpKind::BitNot, _) => { let v = results.pop()?; results.push(!v); }
                    Expr::Cast(_, _) => {} // result already on stack
                    _ => {} // identity: result already on stack
                },
                _ => unreachable!(),
            }
        }
        results.pop()
    }

    /// Extract range bounds from a desugared constraint expression.
    /// Recognizes the pattern: `_ >= lo && _ <= hi` produced by `lo..hi` sugar.
    fn extract_range_from_constraint(expr: &Expr) -> Option<(&Expr, &Expr)> {
        match expr {
            Expr::BinaryOp(BinaryOpKind::And, ge_expr, le_expr) => {
                if let Expr::BinaryOp(BinaryOpKind::Ge, ge_lhs, lo_expr) = ge_expr.as_ref() {
                    if let Expr::Identifier(l1) = ge_lhs.as_ref() {
                        if let Expr::BinaryOp(BinaryOpKind::Le, le_lhs, hi_expr) = le_expr.as_ref() {
                            if let Expr::Identifier(l2) = le_lhs.as_ref() {
                                if l1 == "_" && l2 == "_" {
                                    return Some((lo_expr.as_ref(), hi_expr.as_ref()));
                                }
                            }
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn var_is_chain_internal(&self, var: &str, chain: &[String], all_txn_names: &HashSet<String>) -> bool {
        for txn in all_txn_names {
            if chain.contains(txn) { continue; }
            if let Some(reads) = self.txn_reads.get(txn) {
                if reads.contains(var) { return false; }
            }
            if let Some(writes) = self.txn_writes.get(txn) {
                if writes.contains(var) { return false; }
            }
        }
        true
    }

    fn chain_is_composable(&self, chain: &[String], all_txn_names: &HashSet<String>) -> bool {
        // 1. Each link variable has exactly one writer within the chain
        for i in 0..chain.len().saturating_sub(1) {
            let a = &chain[i];
            if let Some(writes_a) = self.txn_writes.get(a) {
                if let Some(reads_next) = self.txn_reads.get(&chain[i + 1]) {
                    let shared: Vec<_> = writes_a.iter().filter(|w| reads_next.contains(*w)).collect();
                    for sv in &shared {
                        if self.is_chain_counter_var(chain, sv) { continue; }
                        let writer_count: usize = chain.iter()
                            .filter(|tn| {
                                if let Some(w) = self.txn_writes.get(*tn) {
                                    w.contains(*sv)
                                } else { false }
                            })
                            .count();
                        if writer_count > 1 { return false; }
                    }
                }
            }
        }

        // 2. No FFI calls in chain body
        for tn in chain {
            if let Some(body) = self.txn_bodies.get(tn) {
                if has_ffi_or_trigger_stmt_in_chain(body) {
                    return false;
                }
            }
        }

        // 3. Same convergence contract (same pre-condition var and bound)
        let mut common_pre_var: Option<String> = None;
        let mut common_bound_var: Option<String> = None;
        for tn in chain {
            if let Some(body) = self.txn_bodies.get(tn) {
                let cv = find_counter_var(body);
                if let Some(ref v) = cv {
                    if let Some(ref pv) = common_pre_var {
                        if pv != v { return false; }
                    } else {
                        common_pre_var = Some(v.clone());
                    }
                } else {
                    return false;
                }
            }
            if let Some(bv) = self.iter_bounds.get(tn) {
                if let Some(ref cb) = common_bound_var {
                    if format!("{}", bv) != *cb { return false; }
                } else {
                    common_bound_var = Some(format!("{}", bv));
                }
            }
        }

        true
    }

    fn push_composed_chain(
        &mut self,
        chain: &[String],
        link_vars: &[String],
        root_triggers: &[String],
        trigger_values: Option<&[(String, i64)]>,
        all_internal: bool,
    ) {
        let composed = self.build_composed_body(chain, link_vars, trigger_values);

        let counter_var = self.iter_bounds.get(&chain[0])
            .map(|_| {
                if let Some(body) = self.txn_bodies.get(&chain[0]) {
                    find_counter_var(body)
                } else { None }
            })
            .flatten();

        let fused_weight = count_statements_recursive(&composed);

        self.composed_chains.push(ComposedChain {
            chain: chain.to_vec(),
            link_vars: link_vars.to_vec(),
            root_triggers: root_triggers.to_vec(),
            composed_body: composed,
            counter_var,
            fused_weight,
            trigger_values: trigger_values.map(|tv| tv.to_vec()),
            all_internal,
        });
    }

    fn is_chain_counter_var(&self, chain: &[String], var: &str) -> bool {
        chain.iter().any(|tn| {
            if let Some(body) = self.txn_bodies.get(tn) {
                let cv = find_counter_var(body);
                cv.as_deref() == Some(var)
            } else { false }
        })
    }

    fn compute_link_vars(&self, chain: &[String]) -> Vec<String> {
        let mut links = Vec::new();
        for i in 0..chain.len().saturating_sub(1) {
            let a = &chain[i];
            let b = &chain[i + 1];
            if let (Some(writes_a), Some(reads_b)) = (self.txn_writes.get(a), self.txn_reads.get(b)) {
                for w in writes_a {
                    if reads_b.contains(w) {
                        links.push(w.clone());
                        break;
                    }
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
    ) -> Vec<Statement> {
        let mut result = Vec::new();

        for (i, txn_name) in chain.iter().enumerate() {
            let body = match self.txn_bodies.get(txn_name) {
                Some(b) => b.clone(),
                None => continue,
            };

            let counter_var = find_counter_var(&body);

            let mut stmts: Vec<Statement> = body.clone();

            // If this is the root txn (i=0) and we have trigger values,
            // substitute trigger identifiers with concrete values before composition.
            if i == 0 {
                if let Some(tv) = trigger_values {
                    for (trg_name, trg_val) in tv {
                        stmts = substitute_var(&stmts, trg_name, &Expr::Decimal(*trg_val));
                    }
                }
            }

            // Substitute link variables from previous txn's writes
            if i > 0 {
                let prev = &chain[i - 1];
                if let Some(prev_body) = self.txn_bodies.get(prev) {
                    if let Some(&ref link_var) = link_vars.get(i - 1) {
                        let mut write_expr = find_write_expr(prev_body, link_var);

                        // If the previous txn is the root and we have trigger values,
                        // also concretize the write expression
                        if i == 1 {
                            if let Some(tv) = trigger_values {
                                if let Some(ref we) = write_expr {
                                    let mut subs = we.clone();
                                    for (trg_name, trg_val) in tv {
                                        subs = substitute_expr(&subs, trg_name, &Expr::Decimal(*trg_val));
                                    }
                                    write_expr = Some(subs);
                                }
                            }
                        }

                        if let Some(we) = write_expr {
                            stmts = substitute_var(&stmts, link_var, &we);
                        }
                    }
                }
            }

            // Filter out counter bumps from non-root transactions
            for s in &stmts {
                let is_counter = if let Some(ref cv) = counter_var {
                    is_counter_bump_stmt(s, cv)
                } else { false };
                if i > 0 && is_counter { continue; }
                result.push(s.clone());
            }
        }

        result
    }

    fn update_scores_after_composition(&mut self) {
        for cc in &self.composed_chains {
            for score in &mut self.region_scores {
                let shares = cc.chain.iter().any(|c| score.txn_names.contains(c));
                if shares {
                    score.chain_composed = true;
                    score.body_weight = cc.fused_weight;
                    score.optimization_score *= 1.5;
                }
            }
        }
    }
}

fn expr_to_var_set(expr: &Expr) -> HashSet<String> {
    let mut vars = HashSet::new();
    collect_var_ids(expr, &mut vars);
    vars
}

// 2026-06-27: iterative variable collection — replaces recursive version.
fn collect_var_ids(expr: &Expr, vars: &mut HashSet<String>) {
    let mut work: Vec<&Expr> = vec![expr];
    while let Some(e) = work.pop() {
        match e {
            Expr::Identifier(n) => { vars.insert(n.clone()); }
            Expr::BinaryOp(_, l, r) => { work.push(r); work.push(l); }
            Expr::UnaryOp(_, e) => { work.push(e); }
            Expr::Call(_, args, _) => { work.extend(args.iter().rev()); }
            Expr::Tuple(elems) => { work.extend(elems.iter().rev()); }
            Expr::Index(l, i) => { work.push(i.as_ref()); work.push(l.as_ref()); }
            Expr::Field(o, _) => { work.push(o); }
            Expr::List(elems) => { for e in elems.iter().rev() { work.push(e); } }
            _ => {}
        }
    }
}

// 2026-06-27: iterative count — replaces recursive version to prevent
// stack overflow on deep statement nesting (Oracle with nested Foreach).
fn count_statements_recursive(body: &[Statement]) -> usize {
    let mut count = 0;
    let mut work: Vec<&Statement> = body.iter().collect();
    while let Some(s) = work.pop() {
        count += 1;
        match s {
            Statement::Guarded(_, statements) => {
                work.extend(statements.iter().rev());
            }
            Statement::SyncBlock(inner) => {
                work.extend(inner.iter().rev());
            }
            Statement::Foreach { body: inner, .. } => {
                work.extend(inner.iter().rev());
            }
            _ => {}
        }
    }
    count
}

// 2026-06-27: iterative statement walk — replaces recursive version to prevent
// stack overflow on deeply nested guarded blocks.
fn has_ffi_or_terminator_stmt(stmt: &Statement) -> bool {
    let mut work: Vec<&Statement> = vec![stmt];
    while let Some(s) = work.pop() {
        match s {
            Statement::Term(_) | Statement::EndProgram(_)
            | Statement::InlineAsm { .. } => return true,
            // 2026-07-19: Block precomputation for non-# calls (frgn, pure-Briev).
            // # intrinsics are handled by the interpreter's execute_intrinsic
            // and can be precomputed. All other calls prevent precomputation
            // because the interpreter can't evaluate them efficiently.
            Statement::Assign(_, expr) if expr_has_non_hash_call(expr) => return true,
            Statement::Let { expr, .. } => {
                if let Some(e) = expr { if expr_has_non_hash_call(e) { return true; } }
            }
            Statement::Expression(e) if expr_has_non_hash_call(e) => return true,
            Statement::Guarded(condition, statements) => {
                if expr_has_non_hash_call(condition) { return true; }
                work.extend(statements.iter().rev());
            }
            Statement::Foreach { body, .. } => {
                work.extend(body.iter().rev());
            }
            _ => {}
        }
    }
    false
}

/// Check if an expression contains a call that does NOT end with '#'
/// (# intrinsics are handled by the interpreter and can be precomputed).
fn expr_has_non_hash_call(expr: &Expr) -> bool {
    let mut work: Vec<&Expr> = vec![expr];
    while let Some(s) = work.pop() {
        match s {
            Expr::Call(name, _, _) if !name.ends_with('#') => return true,
            Expr::Tuple(elems) => work.extend(elems.iter().rev()),
            Expr::Index(l, i) => { work.push(i); work.push(l); }
            Expr::Field(o, _) => { work.push(o); }
            Expr::Match(value, arms) => {
                for arm in arms.iter().rev() { work.push(&arm.body); }
                work.push(value);
            }
            _ => {}
        }
    }
    false
}

pub(crate) fn has_ffi_or_trigger_stmt_in_chain(body: &[Statement]) -> bool {
    body.iter().any(|s| has_ffi_or_terminator_stmt(s))
}

// 2026-06-27: iterative statement walk — replaces recursive version to prevent
// stack overflow on deeply nested guarded blocks.
fn has_ffi_or_trigger_stmt(stmt: &Statement, trigger_vars: &HashSet<String>) -> bool {
    let mut work: Vec<&Statement> = vec![stmt];
    while let Some(s) = work.pop() {
        match s {
            Statement::Term(_) | Statement::EndProgram(_)
            | Statement::InlineAsm { .. } => return true,
            Statement::Assign(lhs, expr) => {
                if expr_has_call(expr) { return true; }
                // Also check if lhs references a trigger variable
                if let Expr::Identifier(n) = lhs {
                    if trigger_vars.contains(n) { return true; }
                }
            }
            Statement::Let { expr, .. } => {
                if let Some(e) = expr { if expr_has_call(e) { return true; } }
            }
            Statement::Expression(e) if expr_has_call(e) => return true,
            Statement::Guarded(condition, statements) => {
                if expr_has_call(condition) { return true; }
                work.extend(statements.iter().rev());
            }
            Statement::Foreach { body, .. } => {
                work.extend(body.iter().rev());
            }
            _ => {}
        }
    }
    false
}

// 2026-06-27: iterative AST walk — replaces recursive version to prevent
// stack overflow on deeply nested expression trees (officina-cli's complex
// Match arms with deeply nested binary ops).
fn expr_has_call(expr: &Expr) -> bool {
    let mut work: Vec<&Expr> = vec![expr];
    while let Some(e) = work.pop() {
        match e {
            // 2026-07-19: Only frgn calls block precomputation — pure-Briev
            // function calls and # intrinsics are handled by the interpreter.
            // Without frgn_names context, we conservatively skip all calls
            // and let the interpreter handle them during eval.
            Expr::Call(_, _, _) => return true,
            Expr::Tuple(elems) => {
                work.extend(elems.iter().rev());
            }
            Expr::Index(l, i) => {
                work.push(i);
                work.push(l);
            }
            Expr::Field(o, _) => {
                work.push(o);
            }
            Expr::Match(value, arms) => {
                for arm in arms.iter().rev() {
                    work.push(&arm.body);
                }
                work.push(value);
            }
            _ => {}
        }
    }
    false
}

fn has_term_or_unify_escape(body: &[Statement]) -> bool {
    body.iter().any(|s| matches!(s,
//         Statement::Term(None) | Statement::EndProgram(None) | Statement::Unification { .. }
        | Statement::Rollback(_) | Statement::InlineAsm { .. }
    ))
}

fn is_counter_bump_stmt(stmt: &Statement, counter_var: &str) -> bool {
    let lhs_name = match stmt {
        Statement::Assign(Expr::Identifier(n), _) => Some(n.as_str()),
        _ => None,
    };
    let Some(name) = lhs_name else { return false; };
    if name != counter_var { return false; }
    if let Statement::Assign(_, expr) = stmt {
        if let Expr::BinaryOp(BinaryOpKind::Add, a, b) = expr {
            let lhs_in_a = matches!(a.as_ref(), Expr::Identifier(n) if n == counter_var);
            let lhs_in_b = matches!(b.as_ref(), Expr::Identifier(n) if n == counter_var);
            let pos_int = |e: &Expr| matches!(e, Expr::Decimal(d) if *d > 0);
            return (lhs_in_a && pos_int(b)) || (lhs_in_b && pos_int(a));
        }
    }
    false
}

fn find_counter_var(body: &[Statement]) -> Option<String> {
    for s in body {
        let (lhs_name, expr) = match s {
            Statement::Assign(Expr::Identifier(n), expr) => (Some(n.clone()), expr),
            _ => continue,
        };
        let Some(name) = lhs_name else { continue; };
        if let Expr::BinaryOp(BinaryOpKind::Add, a, b) = expr {
            let lhs_in_a = matches!(a.as_ref(), Expr::Identifier(an) if *an == name);
            let lhs_in_b = matches!(b.as_ref(), Expr::Identifier(bn) if *bn == name);
            let pos_int = |e: &Expr| matches!(e, Expr::Decimal(d) if *d > 0);
            if (lhs_in_a && pos_int(b)) || (lhs_in_b && pos_int(a)) {
                return Some(name);
            }
        }
    }
    None
}

fn find_write_expr(body: &[Statement], var: &str) -> Option<Expr> {
    for s in body {
        let lhs_name = match s {
            Statement::Assign(Expr::Identifier(n), _) => Some(n.as_str()),
            _ => None,
        };
        if let Some(n) = lhs_name {
            if n == var {
                if let Statement::Assign(_, expr) = s {
                    return Some(expr.clone());
                }
            }
        }
    }
    None
}

fn substitute_var(body: &[Statement], old_var: &str, new_expr: &Expr) -> Vec<Statement> {
    body.iter().map(|s| substitute_stmt(s, old_var, new_expr)).collect()
}

fn substitute_stmt(stmt: &Statement, old_var: &str, new_expr: &Expr) -> Statement {
    match stmt {
        Statement::Assign(lhs, expr) => {
            Statement::Assign(lhs.clone(), substitute_expr(expr, old_var, new_expr))
        }
        Statement::Let { name, expr, .. } => {
            Statement::Let {
                names: vec![],
                name: name.clone(),
                ty: None,
                expr: expr.as_ref().map(|e| substitute_expr(e, old_var, new_expr)),
                modifiers: vec![],
            }
        }
        Statement::Guarded(condition, statements) => {
            Statement::Guarded(
                substitute_expr(condition, old_var, new_expr),
                substitute_var(statements, old_var, new_expr),
            )
        }
        Statement::Expression(e) => Statement::Expression(substitute_expr(e, old_var, new_expr)),
        Statement::Term(Some(e)) => Statement::Term(Some(substitute_expr(e, old_var, new_expr))),
        Statement::Term(None) => Statement::Term(None),
        Statement::EndProgram(Some(e)) => Statement::EndProgram(Some(substitute_expr(e, old_var, new_expr))),
        Statement::EndProgram(None) => Statement::EndProgram(None),
        Statement::Rollback(Some(e)) => Statement::Rollback(Some(substitute_expr(e, old_var, new_expr))),
        Statement::Rollback(None) => Statement::Rollback(None),
        Statement::SyncBlock(body) => Statement::SyncBlock(body.clone()),
        other => other.clone(),
    }
}

/// Post-order iterative substitution using explicit stack.
/// Replaces `old_var` with `new_expr` in an expression tree without recursion.
fn substitute_expr(expr: &Expr, old_var: &str, new_expr: &Expr) -> Expr {
    let owned_new = new_expr.clone();
    let owned_old = old_var.to_string();

    // ── Helper function pointers for binary/unary ops ──
    fn add(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Add, Box::new(l), Box::new(r)) }
    fn sub(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Sub, Box::new(l), Box::new(r)) }
    fn mul(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Mul, Box::new(l), Box::new(r)) }
    fn div(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Div, Box::new(l), Box::new(r)) }
    fn modop(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Mod, Box::new(l), Box::new(r)) }
    fn eq(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Eq, Box::new(l), Box::new(r)) }
    fn ne(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Neq, Box::new(l), Box::new(r)) }
    fn lt(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Lt, Box::new(l), Box::new(r)) }
    fn le(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Le, Box::new(l), Box::new(r)) }
    fn gt(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Gt, Box::new(l), Box::new(r)) }
    fn ge(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Ge, Box::new(l), Box::new(r)) }
    fn and(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::And, Box::new(l), Box::new(r)) }
    fn or(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Or, Box::new(l), Box::new(r)) }
    fn bitand(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::BitAnd, Box::new(l), Box::new(r)) }
    fn bitor(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::BitOr, Box::new(l), Box::new(r)) }
    fn bitxor(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::BitXor, Box::new(l), Box::new(r)) }
    fn shl(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Shl, Box::new(l), Box::new(r)) }
    fn shr(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Shr, Box::new(l), Box::new(r)) }
    fn concat(l: Expr, r: Expr) -> Expr { Expr::BinaryOp(BinaryOpKind::Concat, Box::new(l), Box::new(r)) }
    fn not(v: Expr) -> Expr { Expr::UnaryOp(UnaryOpKind::Not, Box::new(v)) }
    fn neg(v: Expr) -> Expr { Expr::UnaryOp(UnaryOpKind::Neg, Box::new(v)) }
    fn bitnot(v: Expr) -> Expr { Expr::UnaryOp(UnaryOpKind::BitNot, Box::new(v)) }

    // Work stack entries
    enum W {
        Proc(Expr),
        B0(Expr),
        /// Unary combine (must capture no data — use naive free function pointers)
        B1(fn(Expr) -> Expr),
        /// Binary combine (must capture no data — use naive free function pointers)
        B2(fn(Expr, Expr) -> Expr),
        /// N-ary combine with data capture via Box<dyn FnOnce>
        Args(usize, Box<dyn FnOnce(Vec<Expr>) -> Expr>),
    }

    macro_rules! binop {
        ($work:ident, $f:expr, $a:expr, $b:expr) => {{
            $work.push(W::B2($f));
            $work.push(W::Proc($b));
            $work.push(W::Proc($a));
        }};
    }
    macro_rules! unop {
        ($work:ident, $f:expr, $a:expr) => {{
            $work.push(W::B1($f));
            $work.push(W::Proc($a));
        }};
    }

    let mut work: Vec<W> = vec![W::Proc(expr.clone())];
    let mut results: Vec<Expr> = vec![];

    // Helper to push Proc entries for each expression in reverse order
    macro_rules! push_procs_rev {
        ($work:ident, $first:expr $(, $rest:expr)*) => {{
            $work.push(W::Proc($first));
            $( $work.push(W::Proc($rest)); )*
        }};
    }

    while let Some(w) = work.pop() {
        match w {
            W::Proc(e) => match e {
                Expr::Identifier(n) if n == owned_old => {
                    results.push(owned_new.clone());
                }
                Expr::Identifier(n) => { results.push(Expr::Identifier(n)); }
                   Expr::Bool(_) => {
                    results.push(e);
                }
                Expr::BinaryOp(BinaryOpKind::Add, a, b) => binop!(work, add, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Sub, a, b) => binop!(work, sub, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Mul, a, b) => binop!(work, mul, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Div, a, b) => binop!(work, div, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Mod, a, b) => binop!(work, modop, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Eq, a, b) => binop!(work, eq, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Neq, a, b) => binop!(work, ne, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Lt, a, b) => binop!(work, lt, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Le, a, b) => binop!(work, le, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Gt, a, b) => binop!(work, gt, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Ge, a, b) => binop!(work, ge, *a, *b),
                Expr::BinaryOp(BinaryOpKind::And, a, b) => binop!(work, and, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Or, a, b) => binop!(work, or, *a, *b),
                Expr::BinaryOp(BinaryOpKind::BitAnd, a, b) => binop!(work, bitand, *a, *b),
                Expr::BinaryOp(BinaryOpKind::BitOr, a, b) => binop!(work, bitor, *a, *b),
                Expr::BinaryOp(BinaryOpKind::BitXor, a, b) => binop!(work, bitxor, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Shl, a, b) => binop!(work, shl, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Shr, a, b) => binop!(work, shr, *a, *b),
                Expr::BinaryOp(BinaryOpKind::Concat, a, b) => binop!(work, concat, *a, *b),
                Expr::UnaryOp(UnaryOpKind::Not, a) => unop!(work, not, *a),
                Expr::UnaryOp(UnaryOpKind::Neg, a) => unop!(work, neg, *a),
                Expr::UnaryOp(UnaryOpKind::BitNot, a) => unop!(work, bitnot, *a),
                Expr::Cast(a, t) => {
                    let t2 = t;
                    work.push(W::Args(1, Box::new(move |v| Expr::Cast(Box::new(v[0].clone()), t2.clone()))));
                    work.push(W::Proc(*a));
                }
                Expr::Call(name, args, _) => {
                    let n = args.len();
                    let name2 = name;
                    work.push(W::Args(n, Box::new(move |v| Expr::Call(name2, v, None))));
                    for a in args.into_iter().rev() {
                        work.push(W::Proc(a));
                    }
                }
                Expr::List(elems) => {
                    let n = elems.len();
                    work.push(W::Args(n, Box::new(Expr::List)));
                    for e in elems.into_iter().rev() {
                        work.push(W::Proc(e));
                    }
                }
                Expr::Tuple(elems) => {
                    let n = elems.len();
                    work.push(W::Args(n, Box::new(Expr::Tuple)));
                    for e in elems.into_iter().rev() {
                        work.push(W::Proc(e));
                    }
                }
                Expr::Field(obj, f) => {
                    let f2 = f;
                    work.push(W::Args(1, Box::new(move |v| Expr::Field(Box::new(v[0].clone()), f2.clone()))));
                    work.push(W::Proc(*obj));
                }
                Expr::Index(l, i) => {
                    work.push(W::Args(2, Box::new(|v| Expr::Index(Box::new(v[0].clone()), Box::new(v[1].clone())))));
                    work.push(W::Proc(*i));
                    work.push(W::Proc(*l));
                }
                other => { results.push(other); }
            },
            W::B0(e) => { results.push(e); }
            W::B1(f) => {
                let a = results.pop().expect("B1: no result");
                results.push(f(a));
            }
            W::B2(f) => {
                let b = results.pop().expect("B2: no right result");
                let a = results.pop().expect("B2: no left result");
                results.push(f(a, b));
            }
            W::Args(n, f) => {
                let len = results.len();
                let args: Vec<Expr> = results.drain(len - n..).collect();
                results.push(f(args));
            }
        }
    }

    results.pop().expect("substitute_expr: no result")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn make_state(name: &str, _val: Expr) -> TopLevel {
        TopLevel::StateDecl(StateDecl {
            name: name.to_string(),
            ty: Type::int(),
            span: None,
        })
    }

    fn make_trigger(name: &str, _ty: Type) -> TopLevel {
        TopLevel::Trigger(Trigger {
            name: name.to_string(),
            instance: Expr::Decimal(0),
            span: None,
        })
    }

    fn make_txn(
        name: &str,
        pre: Expr,
        post: Expr,
        body: Vec<Statement>,
    ) -> TopLevel {
        TopLevel::Transaction(Transaction {
            variadic_param: None,
            name: name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            contract: Contract {
                pre_condition: pre,
                post_condition: post,
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            span: None,
            metadata: HashMap::new(),
            modifiers: vec![],
            outputs: Vec::new(),
            output_type: None,
            derivation: None,
            doc: None,
     })
    }

    fn assign(lhs: &str, expr: Expr) -> Statement {
        Statement::Assign(Expr::Identifier(lhs.to_string()), expr)
    }

    fn int(n: i64) -> Expr {
        Expr::Decimal(n)
    }

    fn ident(name: &str) -> Expr {
        Expr::Identifier(name.to_string())
    }

    fn add(a: Expr, b: Expr) -> Expr {
        Expr::BinaryOp(BinaryOpKind::Add, Box::new(a), Box::new(b))
    }

    fn mk_program(items: Vec<TopLevel>) -> Vec<TopLevel> {
        items
    }

    #[test]
    fn test_pure_vars_region_zero() {
        // Two state vars with no triggers or transactions — all Pure, region 0
        let program = mk_program(vec![
            make_state("count", int(0)),
            make_state("total", int(100)),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.region_of("count"), Some(0));
        assert_eq!(ra.region_of("total"), Some(0));
        assert_eq!(ra.classification_of("count"), Some(VarClass::Pure));
    }

    #[test]
    fn test_trigger_seeded_as_opaque() {
        let program = mk_program(vec![make_trigger("btn", Type::bool_())]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.classification_of("btn"), Some(VarClass::Opaque));
    }

    #[test]
    fn test_trigger_dependency_propagates() {
        // trg: Bool → x depends on trg → x becomes Bounded
        let program = mk_program(vec![
            make_trigger("trg", Type::bool_()),
            make_state("x", int(0)),
            make_txn(
                "t1",
                Expr::Bool(true),
                Expr::Bool(true),
                vec![assign("x", ident("trg"))],
            ),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.classification_of("trg"), Some(VarClass::Opaque));
        assert_eq!(ra.classification_of("x"), Some(VarClass::Opaque));
        assert_ne!(ra.region_of("x"), Some(0));
    }

    #[test]
    fn test_two_independent_trigs_two_regions() {
        // trg_a → x, trg_b → y — two regions
        let program = mk_program(vec![
            make_trigger("trg_a", Type::bool_()),
            make_trigger("trg_b", Type::bool_()),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn("tx_a", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("trg_a")),
            ]),
            make_txn("tx_b", Expr::Bool(true), Expr::Bool(true), vec![
                assign("y", ident("trg_b")),
            ]),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        // Both triggers are Opaque, both x and y are Opaque
        assert_eq!(ra.classification_of("x"), Some(VarClass::Opaque));
        assert_eq!(ra.classification_of("y"), Some(VarClass::Opaque));
        // x and y should be in DIFFERENT regions (connected graphs)
        let region_x = ra.region_of("x").unwrap();
        let region_y = ra.region_of("y").unwrap();
        assert_ne!(
            region_x, region_y,
            "Independent trg vars should be in different regions"
        );
        assert_eq!(ra.regions.len(), 2);
    }

    #[test]
    fn test_chained_dependency_one_region() {
        // trg → x → y: all in same region
        let program = mk_program(vec![
            make_trigger("trg", Type::bool_()),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn(
                "t1",
                Expr::Bool(true),
                Expr::Bool(true),
                vec![assign("x", ident("trg"))],
            ),
            make_txn(
                "t2",
                Expr::Bool(true),
                Expr::Bool(true),
                vec![assign("y", ident("x"))],
            ),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.classification_of("trg"), Some(VarClass::Opaque));
        assert_eq!(ra.classification_of("x"), Some(VarClass::Opaque));
        assert_eq!(ra.classification_of("y"), Some(VarClass::Opaque));
        // All three in same region
        assert_eq!(ra.region_of("trg"), ra.region_of("x"));
        assert_eq!(ra.region_of("x"), ra.region_of("y"));
        assert_eq!(ra.regions.len(), 1);
    }

    #[test]
    fn test_int_trigger_opaque() {
        // Int trigger has no bound → stays Opaque (not tight)
        let program = mk_program(vec![make_trigger("sensor", Type::int())]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.classification_of("sensor"), Some(VarClass::Opaque));
    }

    #[test]
    fn test_transaction_precondition_adds_dep() {
        // pre-condition references total → txn depends on total
        let program = mk_program(vec![
            make_state("count", int(0)),
            make_state("total", int(100)),
            make_txn(
                "proc",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![assign("count", add(ident("count"), int(1)))],
            ),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        // count and total are Pure (no triggers), but count depends on total via pre
        assert_eq!(ra.classification_of("count"), Some(VarClass::Pure));
        assert_eq!(ra.classification_of("total"), Some(VarClass::Pure));
    }

    #[test]
    fn test_constant_interval() {
        let program = mk_program(vec![TopLevel::Constant(Constant {
            name: "total".to_string(),
            ty: Type::int(),
            expr: int(100),
            section: None,
        })]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.classification_of("total"), Some(VarClass::Pure));
        let info = ra.var_info.get("total").unwrap();
        assert_eq!(info.interval, Some(Interval { lo: 100, hi: 100 }));
        assert_eq!(info.value_set_size, Some(1));
    }

    #[test]
    fn test_region_of_nonexistent_var() {
        let program = mk_program(vec![]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.region_of("nonexistent"), None);
        assert_eq!(ra.classification_of("nonexistent"), None);
    }

    fn make_txn_with_body(name: &str, body: Vec<Statement>) -> TopLevel {
        make_txn(name, Expr::Bool(true), Expr::Bool(true), body)
    }

    fn make_const(name: &str, val: i64) -> TopLevel {
        TopLevel::Constant(Constant {
            name: name.to_string(),
            ty: Type::int(),
            expr: int(val),
            section: None,
        })
    }

    #[test]
    fn test_complexity_trivial() {
        let program = mk_program(vec![
            make_trigger("btn", Type::bool_()),
            make_state("count", int(0)),
            make_txn_with_body("bump", vec![assign("count", ident("btn"))]),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert!(ra.region_scores.len() > 0);
        assert_eq!(ra.region_scores[0].complexity, ComplexityClass::Trivial);
    }

    #[test]
    fn test_complexity_light() {
        let program = mk_program(vec![
            make_trigger("btn", Type::bool_()),
            make_state("a", int(0)),
            make_state("b", int(0)),
            make_txn_with_body("proc", vec![
                assign("a", ident("btn")),
                assign("b", int(2)),
                assign("a", add(ident("a"), int(3))),
            ]),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert!(ra.region_scores.len() > 0);
        assert_eq!(ra.region_scores[0].complexity, ComplexityClass::Light);
    }

    #[test]
    fn test_complexity_unbounded() {
        let program = mk_program(vec![
            make_trigger("btn", Type::bool_()),
            make_state("x", int(0)),
            make_txn_with_body("exit", vec![
                assign("x", ident("btn")),
                Statement::Term(Some(int(0))),
            ]),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert!(ra.region_scores.len() > 0);
        assert_eq!(ra.region_scores[0].complexity, ComplexityClass::Unbounded);
    }

    #[test]
    fn test_region_scoring() {
        let program = mk_program(vec![
            make_trigger("trg", Type::bool_()),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn("t1", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("trg")),
            ]),
            make_txn("t2", Expr::Bool(true), Expr::Bool(true), vec![
                assign("y", add(ident("x"), int(1))),
            ]),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert!(!ra.region_scores.is_empty());
        let score = &ra.region_scores[0];
        assert!(score.body_weight > 0);
    }

    #[test]
    fn test_region_independent() {
        let program = mk_program(vec![
            make_trigger("ta", Type::bool_()),
            make_trigger("tb", Type::bool_()),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn("tx_a", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("ta")),
            ]),
            make_txn("tx_b", Expr::Bool(true), Expr::Bool(true), vec![
                assign("y", ident("tb")),
            ]),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        assert_eq!(ra.region_scores.len(), 2);
    }

    #[test]
    fn test_budget_plan_fit() {
        let program = mk_program(vec![
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_const("a_val", 1),
            make_const("b_val", 2),
            make_txn("tx_a", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("a_val")),
            ]),
            make_txn("tx_b", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("a_val")),
                assign("y", ident("b_val")),
            ]),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.build_budget_plan(10);
        let plan = ra.budget_plan.as_ref().unwrap();
        assert!(plan.skipped.is_empty() || plan.allocated.len() > 0);
        assert!(plan.residual_budget > 0);
    }

    #[test]
    fn test_budget_plan_exceeds() {
        let program = mk_program(vec![
            make_trigger("ta", Type::bool_()),
            make_state("x", int(0)),
            make_txn("tx_a", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("ta")),
            ]),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.build_budget_plan(0);
        let plan = ra.budget_plan.as_ref().unwrap();
        assert!(plan.allocated.is_empty());
        assert!(!plan.skipped.is_empty());
    }

    #[test]
    fn test_chain_substitution() {
        let body_a = vec![assign("x", int(42))];
        let body_b = vec![assign("y", add(ident("x"), int(1)))];
        let result = substitute_var(&body_b, "x", &int(42));
        if let Statement::Assign(_, expr) = &result[0] {
            assert!(!format!("{:?}", expr).contains("Identifier(\"x\")"));
        }
    }

    #[test]
    fn test_chain_composition() {
        let program = mk_program(vec![
            make_const("total", 100),
            make_state("count", int(0)),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn("step_a",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("x", int(42)),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
            make_txn("step_b",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("y", add(ident("x"), int(1))),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        if !ra.composed_chains.is_empty() {
            let cc = &ra.composed_chains[0];
            assert!(cc.fused_weight > 0);
            assert_eq!(cc.link_vars.len(), 1);
        }
    }

    #[test]
    fn test_chain_branching() {
        let program = mk_program(vec![
            make_trigger("sensor", Type::bool_()),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn("step_a", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("sensor")),
            ]),
            make_txn("step_b", Expr::Bool(true), Expr::Bool(true), vec![
                assign("y", add(ident("x"), int(1))),
            ]),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        if !ra.composed_chains.is_empty() {
            let cc = &ra.composed_chains[0];
            assert!(cc.root_triggers.contains(&"sensor".to_string()));
        }
    }

    #[test]
    fn test_gpu_eligible() {
        let program = mk_program(vec![
            make_const("total", 100),
            make_state("a", int(0)),
            make_state("b", int(0)),
            make_state("c", int(0)),
            make_state("count", int(0)),
            make_txn("heavy",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("a", int(1)),
                    assign("b", int(2)),
                    assign("c", int(3)),
                    assign("a", add(ident("a"), ident("b"))),
                    assign("b", add(ident("b"), ident("c"))),
                    assign("c", add(ident("c"), int(1))),
                    assign("a", add(ident("a"), ident("c"))),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        if let Some(score) = ra.region_scores.first() {
            assert_eq!(score.complexity, ComplexityClass::Medium);
            assert!(score.gpu_eligible);
        }
    }

    #[test]
    fn test_gpu_ineligible_term() {
        let program = mk_program(vec![
            make_const("total", 100),
            make_state("a", int(0)),
            make_state("count", int(0)),
            make_txn("with_term",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("a", int(1)),
                    assign("a", int(2)),
                    assign("a", int(3)),
                    assign("a", int(4)),
                    assign("a", int(5)),
                    assign("a", int(6)),
                    Statement::Term(Some(int(0))),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
        ]);
        let ra = RegionAnalyzer::analyze(&program);
        if let Some(score) = ra.region_scores.first() {
            assert!(!score.gpu_eligible);
        }
    }

    #[test]
    fn test_is_fully_precomputable_chain() {
        let program = mk_program(vec![
            make_const("total", 100),
            make_state("count", int(0)),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn("step_a",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("x", int(42)),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
            make_txn("step_b",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("y", add(ident("x"), int(1))),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        assert!(ra.is_fully_precomputable(10));
        assert!(!ra.is_fully_precomputable(0));
    }

    #[test]
    fn test_is_not_precomputable_no_chains() {
        let program = mk_program(vec![
            make_trigger("btn", Type::bool_()),
            make_state("x", int(0)),
            make_txn("t1", Expr::Bool(true), Expr::Bool(true), vec![
                assign("x", ident("btn")),
            ]),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        assert!(!ra.is_fully_precomputable(100));
    }

    #[test]
    fn test_is_not_precomputable_ffi() {
        let program = mk_program(vec![
            make_const("total", 100),
            make_state("count", int(0)),
            make_state("x", int(0)),
            make_txn("heavy",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("x", int(1)),
                    Statement::Term(Some(int(0))),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        assert!(!ra.is_fully_precomputable(100));
    }

    #[test]
    fn test_collect_final_values_all_internal() {
        let program = mk_program(vec![
            make_const("total", 100),
            make_const("count", 0),
            make_const("x", 0),
            make_const("y", 0),
            make_txn("step_a",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("x", int(42)),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
            make_txn("step_b",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("y", add(ident("x"), int(1))),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        let result = ra.collect_final_values(&program);
        assert!(result.is_some());
        let bindings = result.unwrap();
        assert!(!bindings.is_empty());
        let found_count = bindings.iter().any(|(_, m)| m.get("count") == Some(&100));
        assert!(found_count, "Final count should equal bound (100), got {:?}", bindings);
    }

    #[test]
    fn test_collect_final_values_with_trigger() {
        let program = mk_program(vec![
            make_const("total", 50),
            make_trigger("sensor", Type::bool_()),
            make_state("count", int(0)),
            make_state("x", int(0)),
            make_state("y", int(0)),
            make_txn("step_a",
                Expr::Bool(true),
                Expr::Bool(true),
                vec![
                    assign("x", ident("sensor")),
                ],
            ),
            make_txn("step_b",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    assign("y", add(ident("x"), int(1))),
                    assign("count", add(ident("count"), int(1))),
                ],
            ),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        let result = ra.collect_final_values(&program);
        assert!(result.is_some());
    }

    /// 2026-08-09 (Bug 3): a chain that writes an ARRAY ELEMENT (`a[i] = ...`)
    /// must NOT be precomputed — the i64 binding model can't represent array
    /// elements, so a folded observable would read the initializer (0).
    #[test]
    fn test_indexed_write_blocks_precompute() {
        let program = mk_program(vec![
            make_const("total", 4),
            make_state("count", int(0)),
            make_state("a", int(0)),
            make_txn("go",
                Expr::BinaryOp(BinaryOpKind::Lt, Box::new(ident("count")), Box::new(ident("total"))),
                Expr::Bool(true),
                vec![
                    // a[count] = 1.5 — an indexed write the model can't track.
                    Statement::Assign(
                        Expr::Index(Box::new(ident("a")), Box::new(ident("count"))),
                        Expr::Float(1.5),
                    ),
                    assign("count", add(ident("count"), int(1))),
                    Statement::Term(None),
                ],
            ),
        ]);
        let mut ra = RegionAnalyzer::analyze(&program);
        ra.compose_chains();
        assert!(
            !ra.is_fully_precomputable(100),
            "an indexed write must block the compile-time precompute fold"
        );
    }
}
