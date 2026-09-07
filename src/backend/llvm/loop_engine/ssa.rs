// ── Loop Emission: SSA Register Pipeline ───────────────────────
//
// 2026-07-13: Extracted from monolithic loop_engine.rs (4398 lines).
// Implements Strategy 4 (SSA Register Pipeline) for multi-txn reactive
// programs, modulo-switch dispatch, and folded multi-txn dispatch.
//
// Strategies:
//   a) SSA canonical loop (single txn, simple bound)
//   b) SSA with precondition check (node with pre/post)
//   c) SSA no precondition (node with [true][post])
//   d) Modulo-rotated dispatch (K ≤ 8 reactive txns)
//   e) Modulo-switch dispatch (K > 8 reactive txns)
//   f) Folded multi-txn (enum-key dispatch for folded transactions)

use crate::backend::llvm::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write;

impl LlvmBackend {
    // ═══════════════════════════════════════════════════════════════
    // Modulo-Switch Dispatch
    // ═══════════════════════════════════════════════════════════════

    /// Try to detect if the reactive transaction set can be dispatched
    /// via modulo-switch on the counter register.
    ///
    /// 2026-07-31: Phase 2 (plan §7.2) — the partition (counter, divisor,
    /// residue→txn cases) is computed ONCE in the frontend
    /// (src/analysis/modulo_partition.rs), replacing the per-dispatch
    /// `extract_mod_info` / `extract_mod_guard` body re-walks. The backend
    /// only verifies the dispatch set matches the partition and chooses the
    /// emitter structurally:
    ///   - rotated loop whenever the txn set has a bounded counter
    ///     precondition — the ONLY form that handles a bounded counter (the
    ///     one-shot switch can't loop; see comment below),
    ///   - one-shot switch only when no txn increments a counter
    ///     (self-terminating: without counter advancement the set cannot
    ///     require repeated ticks),
    ///   - otherwise fall through to the generic SSA pipeline.
    fn try_modulo_switch_dispatch(
        &mut self,
        out: &mut String,
        reactive_txns: &[&(String, &crate::ast::Transaction)],
    ) -> bool {
        let partition = match self.ctx.modulo_partition.clone() {
            Some(p) => p,
            None => return false,
        };
        if partition.cases.len() < 2 {
            return false;
        }
        for (_, txn) in reactive_txns {
            if !txn.is_reactive {
                return false;
            }
        }
        let reactive_names: Vec<&str> = reactive_txns.iter().map(|(n, _)| n.as_str()).collect();
        if partition.cases.len() != reactive_names.len() {
            return false;
        }
        // Every txn in the dispatch set must appear exactly once in the cases.
        for (_, case_name) in &partition.cases {
            if !reactive_names.contains(&case_name.as_str()) {
                return false;
            }
        }
        let graph = self.ctx.transition_graph.as_ref();
        let node_of = |name: &str| {
            graph.and_then(|g| g.nodes.iter().find(|n| n.name == name))
        };
        let any_bounded = reactive_names
            .iter()
            .any(|n| node_of(n).map_or(false, |node| node.bounded_pre.is_some()));
        let any_increments = reactive_names
            .iter()
            .any(|n| node_of(n).map_or(false, |node| node.increments.is_some()));
        // 2026-07-17: Use modulo-rotated loop for bounded counter sets.
        // emit_modulo_switch_main is one-shot (no loop) and doesn't handle
        // the bounded counter — it was designed for the old pre-check path.
        // 2026-07-31: The bound variable comes from the transition graph's
        // bounded_pre (plan §7.2 fixes ssa.rs:183's hardcoded "total").
        if any_bounded {
            let bound_var = match reactive_names.iter().find_map(|n| {
                node_of(n).and_then(|node| node.bounded_pre.as_ref().map(|bp| bp.bound_var.clone()))
            }) {
                Some(b) => b,
                None => return false,
            };
            let cases_ref: Vec<(i64, &str)> = partition.cases.iter()
                .map(|(r, n)| (*r, n.as_str())).collect();
            let txns_ref: Vec<(String, &crate::ast::Transaction)> = reactive_txns.iter()
                .map(|(n, t)| (n.clone(), *t)).collect();
            self.emit_modulo_rotated(out, &txns_ref, &partition.counter, partition.divisor, &cases_ref, &bound_var);
            return true;
        }
        if !any_increments {
            let cases_ref: Vec<(i64, &str)> = partition.cases.iter()
                .map(|(r, n)| (*r, n.as_str())).collect();
            self.emit_modulo_switch_main(out, reactive_txns, &partition.counter, partition.divisor, &cases_ref);
            return true;
        }
        false
    }

    /// Emit a modulo-switch dispatch: check counter % divisor and switch
    /// to the matching transaction body.
    fn emit_modulo_switch_main(
        &mut self,
        out: &mut String,
        _txns: &[&(String, &crate::ast::Transaction)],
        counter_name: &str,
        divisor: i64,
        cases: &[(i64, &str)],
    ) {
        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        let counter_idx = self.ctx.field_index_map.get(counter_name).copied().unwrap_or(0);
        let (c_val, _) = self.emit_state_load_i64_by_idx(out, "  ", counter_idx);
        let mod_val = self.fun.next_reg_with_prefix("msm");
        writeln!(out, "  {} = srem i64 {}, {}", mod_val, c_val, divisor).ok();
        writeln!(out, "  switch i64 {}, label %.end [", mod_val).ok();
        for (val, name) in cases {
            writeln!(out, "    i64 {}, label %{}", val, name).ok();
        }
        writeln!(out, "  ]").ok();
        for (_val, name) in cases {
            writeln!(out, "{}:", name).ok();
            writeln!(out, "  call void @txn_{}(ptr %state)", name).ok();
            writeln!(out, "  br label %.end").ok();
        }
        writeln!(out, ".end:").ok();
        writeln!(out, "  ret i32 0").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    // ═══════════════════════════════════════════════════════════════
    // Modulo-Rotated Dispatch (bounded counter sets)
    // ═══════════════════════════════════════════════════════════════

    /// Emit a modulo-rotated dispatch loop. Used for reactive txn sets whose
    /// preconditions are `counter % divisor == residue` AND that have a
    /// bounded counter (a `count < total` bound in the precondition). The
    /// loop body checks `counter % divisor` and branches to the matching case.
    ///
    /// 2026-07-31: Phase 2 (plan §7.2) — the bound variable comes from the
    /// transition graph's `bounded_pre.bound_var`, fixing the hardcoded
    /// `"total"` lookup at the old ssa.rs:183 (`counter_idx + 1` fallback
    /// assumed the bound field immediately follows the counter).
    pub(crate) fn emit_modulo_rotated(
        &mut self,
        out: &mut String,
        _txns: &[(String, &crate::ast::Transaction)],
        counter_name: &str,
        divisor: i64,
        cases: &[(i64, &str)],
        bound_var: &str,
    ) {
        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        writeln!(out, "  br label %.mr_loop").ok();
        writeln!(out, ".mr_loop:").ok();
        let counter_idx = self.ctx.field_index_map.get(counter_name).copied().unwrap_or(0);
        // 2026-07-20: Keep GEP for counter (reused by store at mr_latch).
        let c_gep = self.fun.next_reg_with_prefix("mrp");
        writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            c_gep, counter_idx).ok();
        let c_val = self.fun.next_reg_with_prefix("mrv");
        writeln!(out, "  {} = load i64, ptr {}, align 8", c_val, c_gep).ok();
        // 2026-07-17: Bound check — exit when counter >= bound.
        // The bound field name may not be directly after counter; find by name.
        let total_idx = self.ctx.field_index_map.get(bound_var).copied().unwrap_or(counter_idx + 1);
        let (bound_val, _) = self.emit_state_load_i64_by_idx(out, "  ", total_idx);
        let done = self.fun.next_reg_with_prefix("mrd");
        writeln!(out, "  {} = icmp sge i64 {}, {}", done, c_val, bound_val).ok();
        writeln!(out, "  br i1 {}, label %.mr_end, label %.mr_cont", done).ok();
        writeln!(out, ".mr_cont:").ok();
        let mod_val = self.fun.next_reg_with_prefix("mrm");
        writeln!(out, "  {} = srem i64 {}, {}", mod_val, c_val, divisor).ok();
        for (i, (val, name)) in cases.iter().enumerate() {
            let eq = self.fun.next_reg_with_prefix("mre");
            let next_label = format!(".mr_next_{}", i);
            writeln!(out, "  {} = icmp eq i64 {}, {}", eq, mod_val, val).ok();
            writeln!(out, "  br i1 {}, label %{}, label %{}", eq, name, next_label).ok();
            writeln!(out, "{}:", name).ok();
            writeln!(out, "  call void @txn_{}(ptr %state)", name).ok();
            writeln!(out, "  br label %.mr_latch").ok();
            writeln!(out, "{}:", next_label).ok();
        }
        writeln!(out, "  br label %.mr_latch").ok();
        writeln!(out, ".mr_latch:").ok();
        let next = self.fun.next_reg_with_prefix("mrn");
        writeln!(out, "  {} = add nuw nsw i64 {}, 1", next, c_val).ok();
        writeln!(out, "  store i64 {}, ptr {}, align 8", next, c_gep).ok();
        writeln!(out, "  br label %.mr_loop").ok();
        writeln!(out, ".mr_end:").ok();
        writeln!(out, "  ret i32 0").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    // ═══════════════════════════════════════════════════════════════
    // SSA Canonical Loop Setup (Single Txn)
    // ═══════════════════════════════════════════════════════════════

    /// Set up a canonical SSA loop for a single transaction.
    /// Emits phi nodes for counter + field state, header check, and latch.
    fn emit_ssa_canonical_loop_setup(
        &mut self,
        out: &mut String,
        _txn: &crate::ast::Transaction,
        bound_name: &str,
        b_idx: usize,
        cname: &str,
    ) {
        let c_idx = self.ctx.field_index_map.get(cname).copied().unwrap_or(0);
        let (init, _) = self.emit_state_load_i64_by_idx(out, "  ", c_idx);
        let (bound, _) = self.emit_state_load_i64_by_idx(out, "  ", b_idx);
        writeln!(out, "  br label %.ss_loop").ok();
        writeln!(out, ".ss_loop:").ok();
        let counter = self.fun.next_reg_with_prefix("ssc");
        writeln!(out, "  {} = phi i64 [ {}, %.ss_loop ], [ {}, %.ss_latch ]",
            counter, init, bound).ok();
        let done = self.fun.next_reg_with_prefix("ssd");
        writeln!(out, "  {} = icmp sgt i64 {}, {}", done, counter, bound).ok();
        writeln!(out, "  br i1 {}, label %.ss_body, label %.ss_end", done).ok();
        writeln!(out, ".ss_body:").ok();
    }

    /// Emit the body of a canonical SSA transaction.
    fn emit_ssa_txn_canonical_body(
        &mut self,
        out: &mut String,
        body_stmts: &[&Statement],
        _post_hoist: &[Vec<Statement>],
    ) {
        for stmt in body_stmts {
            self.emit_statement(out, stmt, "  ");
        }
    }

    /// Emit a transaction body with precondition check.
    fn emit_ssa_txn_with_precond(
        &mut self,
        out: &mut String,
        pre: &Expr,
        _name: &str,
        body_stmts: &[&Statement],
        _post_hoist: &[Vec<Statement>],
    ) {
        let cond_val = self.emit_expr(out, pre, "  ");
        let bool_reg = self.as_bool_reg(out, "  ", &cond_val);
        let body_label = format!(".spb{}", self.fun.txn_counter);
        let next_label = format!(".spn{}", self.fun.txn_counter);
        self.fun.txn_counter += 1;
        writeln!(out, "  br i1 {}, label %{}, label %{}", bool_reg, body_label, next_label).ok();
        writeln!(out, "{}:", body_label).ok();
        for stmt in body_stmts {
            self.emit_statement(out, stmt, "  ");
        }
        writeln!(out, "  br label %{}", next_label).ok();
        writeln!(out, "{}:", next_label).ok();
    }

    /// Emit a transaction body without precondition check.
    fn emit_ssa_txn_no_precond(
        &mut self,
        out: &mut String,
        body_stmts: &[&Statement],
        _post_hoist: &[Vec<Statement>],
    ) {
        for stmt in body_stmts {
            self.emit_statement(out, stmt, "  ");
        }
    }

    /// Pre-allocate SSA registers for multi-txn state fields.
    fn emit_ssa_mt_prealloc(
        &mut self,
        out: &mut String,
        _txns: &[(String, &crate::ast::Transaction)],
    ) {
        // Emit alloca for each state field at function entry
        for (name, idx) in &self.ctx.field_index_map {
            let alloca = self.fun.next_reg_with_prefix("sa");
            let ft = &self.ctx.field_types[*idx];
            let llvm_type = match ft.as_str() {
                "float" => "float",
                "i32" => "i32",
                "i8" => "i8",
                s if s == "i8*" || s == "ptr" => "ptr",
                _ => "i64",
            };
            writeln!(out, "  {} = alloca {}, align 8", alloca, llvm_type).ok();
            // 2026-07-17: Do NOT insert the alloca pointer into last_val_temps.
            // Alloca registers are LLVM ptr type. When emit_expr looks up a
            // field in last_val_temps, it expects a value register (i64) but
            // gets a ptr — type mismatch at the icmp/cmp instruction.
            // load_last_val_temps (called later in emit_ssa_main) properly
            // loads state values into last_val_temps.
        }
    }

    /// Emit the main SSA register pipeline for multi-txn reactive programs.
    pub(crate) fn emit_ssa_main(
        &mut self,
        out: &mut String,
        txns: &[(String, &crate::ast::Transaction)],
        has_wake_triggers: bool,
    ) {
        // 2026-07-17: Early-return for modulo-gated dispatch (sparse_dispatch).
        // Checks if all reactive txns have counter % K == N preconditions and
        // emits an optimized modulo-rotated loop that branches directly to the
        // correct body without evaluating K preconditions per iteration.
        let reactive_txns: Vec<&(String, &crate::ast::Transaction)> = txns.iter()
            .filter(|(_, t)| t.is_reactive).collect();
        if self.try_modulo_switch_dispatch(out, &reactive_txns) {
            return;
        }

        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        self.emit_ssa_mt_prealloc(out, txns);
        // 2026-07-18: Convergence check — if no wake triggers and no async,
        // the program is one-shot. Exit when all txns have converged
        // (precondition false for all).
        let is_one_shot = !has_wake_triggers && !self.has_async_txns;
        let has_exit_cond = self.ctx.exit_condition.is_some();
        let is_terminating = has_exit_cond || is_one_shot;

        let active_slot = if is_terminating {
            let slot = format!("%any_active_{}", self.fun.txn_counter);
            self.fun.txn_counter += 1;
            Some(slot)
        } else {
            None
        };
        // 2026-09-07 (init-block cur_block fix): the init stores above may have
        // emitted blocks (a HashMap.init `match` leaves the emitter in its
        // .match_end_N). The loop's `br` + `.ss_main_loop:` header emit into
        // that block — valid for the br (entry's successor), but a STALE
        // cur_block would make the loop body's swan-song lets bind through a
        // flush slot that lands AFTER the `br` (the flush happens before the
        // loop text is appended), which the loop body then references without
        // dominance. Reset so the body's allocas flush into the header's own
        // preheader (entry, before the `br`).
        self.fun.cur_block = None;
        // 2026-07-18: Allocate convergence tracking slot in entry (not in loop)
        if let Some(ref slot) = active_slot {
            writeln!(out, "  {} = alloca i64, align 8", slot).ok();
        }
        // 2026-08-17 (Phase B reactor fix): buffer the loop construction so a
        // constant-element tuple arg to a member call in a reactive body can be
        // flushed into the PREHEADER (before `.ss_main_loop`) via
        // flush_pending_struct_allocas. Without it, clang -O3 collapses the
        // post-first-statement body (the m2/m3i/m2_letget grid). Mirrors
        // emit_countable_main's loop_buf + trailing flush (2026-08-13).
        let prev_defer = self.fun.defer_struct_allocas;
        self.fun.defer_struct_allocas = true;
        let mut loop_buf = String::new();
        {
        let out = &mut loop_buf;
        writeln!(out, "  br label %.ss_main_loop").ok();
        writeln!(out, ".ss_main_loop:").ok();
        // Reset active flag each iteration
        if let Some(ref slot) = active_slot {
            writeln!(out, "  store i64 0, ptr {}", slot).ok();
        }
        // Check each txn's precondition, execute body if true
        for (name, txn) in txns {
            if txn.is_reactive {
                let pre = &txn.contract.pre_condition;
                // 2026-08-06 (beginprogram plan): the precondition may read the
                // node's `@briev_begin_<name>` entry flag — bind the txn name.
                self.fun.txn_name = name.clone();
                let cond_val = self.emit_expr(out, pre, "  ");
                let bool_reg = self.as_bool_reg(out, "  ", &cond_val);
                let body_label = format!(".ssb_{}", name);
                let next_label = format!(".ssn_{}", name);
                self.fun.txn_counter += 1;
                writeln!(out, "  br i1 {}, label %{}, label %{}", bool_reg, body_label, next_label).ok();
                writeln!(out, "{}:", body_label).ok();
                // 2026-08-04 (term-termination-diagnostics): a value-form term
                // in this txn's body unwinds to the next txn's label, skipping
                // the rest of THIS body — interpreter TermReturn semantics.
                // Reset per-txn so a terminated previous txn does not leak into
                // this one's body loop.
                self.fun.terminated = false;
                self.fun.void_txn_abort_label = Some(next_label.clone());
                if let Some(ref slot) = active_slot {
                    writeln!(out, "  store i64 1, ptr {}", slot).ok();
                }
                // 2026-08-16 (multi-node internal fold, Direction 3): a node
                // whose whole bounded pass is folded into @txn_<name> (a
                // noinline countdown) is CALLED once per pass — the pass runs
                // internally (the counter lives in a phi register) instead of
                // this inline per-firing body. The precondition branch above
                // still gates the call.
                if self.ctx.internal_fold_txns.contains(name) {
                    writeln!(out, "  call void @txn_{}(ptr %state)", name).ok();
                    if !self.fun.terminated {
                        self.emit_beginprogram_goal_check(out, txn);
                        writeln!(out, "  br label %{}", next_label).ok();
                    }
                    writeln!(out, "{}:", next_label).ok();
                    continue;
                }
                // 2026-08-31 (plan abv-gpu-by-default): an ACCEL kernel node
                // dispatches through its wrapper (@txn_<name> — lazy runtime
                // init + device/verdict gate + one launch of N work-items +
                // counter fast-forward), never an inlined per-firing body.
                // Inlining made every kernel CPU-only with the GPU lane dead
                // in the module. The pre-condition branch above still gates
                // the call; after the launch the fast-forwarded counter makes
                // this pre false, so the loop advances exactly like the
                // internal fold below.
                if self.accel_kernel_idx.contains_key(name) {
                    writeln!(out, "  call void @txn_{}(ptr %state)", name).ok();
                    if !self.fun.terminated {
                        self.emit_beginprogram_goal_check(out, txn);
                        writeln!(out, "  br label %{}", next_label).ok();
                    }
                    writeln!(out, "{}:", next_label).ok();
                    continue;
                }
                for stmt in &txn.body {
                    if self.fun.terminated { break; }
                    self.emit_statement(out, stmt, "  ");
                }
                self.fun.void_txn_abort_label = None;
                if !self.fun.terminated {
                    // 2026-08-06 (beginprogram plan): clear the entry flag when
                    // the entry loop's goal is met.
                    self.emit_beginprogram_goal_check(out, txn);
                    writeln!(out, "  br label %{}", next_label).ok();
                }
                writeln!(out, "{}:", next_label).ok();
            }
        }
        if let Some(ref slot) = active_slot {
            // Check if any txn ran this iteration
            let check_reg = self.fun.gen_reg();
            writeln!(out, "  {} = load i64, ptr {}", check_reg, slot).ok();
            let done_reg = self.fun.gen_reg();
            writeln!(out, "  {} = icmp eq i64 {}, 0", done_reg, check_reg).ok();
            writeln!(out, "  br i1 {}, label %.end, label %.ss_main_loop", done_reg).ok();
        } else {
            writeln!(out, "  br label %.ss_main_loop").ok();
        }
        writeln!(out, ".end:").ok();
        writeln!(out, "  ret i32 0").ok();
        } // end loop_buf scope
        self.fun.defer_struct_allocas = prev_defer;
        self.flush_pending_struct_allocas(out);
        out.push_str(&loop_buf);
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    // ═══════════════════════════════════════════════════════════════
    // Folded Multi-Txn Dispatch
    // ═══════════════════════════════════════════════════════════════

    /// Emit a folded multi-txn main — dispatch between multiple folded
    /// transactions via an enum key. Used when multiple transactions
    /// share a single main() and are dispatched on a counter/state field.
    pub(crate) fn emit_folded_multi_main(
        &mut self,
        out: &mut String,
        txns: &[(String, &crate::ast::Transaction)],
        _enum_sizes: &[(String, Option<u64>)],
        _enum_keys: &HashMap<String, Vec<i64>>,
        _fold_params: &HashMap<String, crate::backend::llvm::FoldParam>,
        _fold_pure: &HashMap<String, (bool, Option<i64>)>,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        _composed_fn: Option<&str>,
        _composed_trig_map: Option<&HashMap<String, Vec<(i64, String)>>>,
        _all_internal_map: Option<&HashMap<String, (usize, i64)>>,
        _has_wake: bool,
    ) {
        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        let bound_reg = if let Some(lit) = bound_literal {
            // 2026-08-08: a shared compile-time literal bound — the fold loop
            // must run to the literal, not the `counter < 1` fallback.
            let r = self.fun.next_reg_with_prefix("fmb");
            writeln!(out, "  {} = add i64 0, {}", r, lit).ok();
            r
        } else if let Some(ti) = total_idx {
            let (br, _) = self.emit_state_load_i64_by_idx(out, "  ", ti);
            br
        } else if let Some(tcn) = total_const_name {
            if let Some(&idx) = self.ctx.field_index_map.get(tcn) {
                let (br, _) = self.emit_state_load_i64_by_idx(out, "  ", idx);
                br
            } else {
                let r = self.fun.next_reg_with_prefix("fmb");
                writeln!(out, "  {} = add i64 0, 1", r).ok();
                r
            }
        } else {
            let r = self.fun.next_reg_with_prefix("fmb");
            writeln!(out, "  {} = add i64 0, 1", r).ok();
            r
        };
        // 2026-07-18: Preallocate push targets for all txn bodies in the
        // multi-txn main loop. Each body's push targets are collected via
        // over-approximation and deduplicated, then preallocated once with
        // capacity = bound + 2 per target. Since all txns share the same
        // arena and bound, one preallocation block covers all bodies.
        {
            let mut push_targets: Vec<String> = Vec::new();
            for (_name, txn) in txns {
                crate::backend::llvm::collect_push_targets(&txn.body, &mut push_targets);
            }
            push_targets.sort();
            push_targets.dedup();
            if !push_targets.is_empty() {
                self.emit_prealloc_for_targets(out, "  ", &push_targets, &bound_reg);
            }
        }
        writeln!(out, "  br label %.fm_loop").ok();
        writeln!(out, ".fm_loop:").ok();
        // 2026-07-20: Intentionally hand-rolled — single GEP serves both load (above) and store (fm_loop latch).
        let c_gep = self.fun.next_reg_with_prefix("fmg");
        writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            c_gep, counter_idx).ok();
        let c_val = self.fun.next_reg_with_prefix("fmv");
        writeln!(out, "  {} = load i64, ptr {}, align 8", c_val, c_gep).ok();
        let done = self.fun.next_reg_with_prefix("fmd");
        writeln!(out, "  {} = icmp slt i64 {}, {}", done, c_val, bound_reg).ok();
        writeln!(out, "  br i1 {}, label %.fm_body, label %.fm_end", done).ok();
        writeln!(out, ".fm_body:").ok();
        for (name, txn) in txns {
            writeln!(out, "  call void @txn_{}(ptr %state)", name).ok();
        }
        let next = self.fun.next_reg_with_prefix("fmn");
        writeln!(out, "  {} = add nuw nsw i64 {}, 1", next, c_val).ok();
        writeln!(out, "  store i64 {}, ptr {}, align 8", next, c_gep).ok();
        writeln!(out, "  br label %.fm_loop").ok();
        writeln!(out, ".fm_end:").ok();
        writeln!(out, "  ret i32 0").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    // ═══════════════════════════════════════════════════════════════
    // Post-Loop Print Handling
    // ═══════════════════════════════════════════════════════════════

    /// Load last-value temps into registers for post-loop printing.
    /// State fields are now stored with their native LLVM type in %State.
    /// Float fields are loaded as float directly — no unboxing needed.
    pub(crate) fn load_last_val_temps(&mut self, out: &mut String) {
        let mut sorted_keys: Vec<String> = self.fun.last_val_temps.keys().cloned().collect();
        sorted_keys.sort();
        for name in &sorted_keys {
            if let Some(&idx) = self.ctx.field_index_map.get(name) {
                let (val, briev_ty) = self.emit_state_load_i64_by_idx(out, "  ", idx);
                self.fun.last_val_temps.insert(name.clone(), val.clone());
                self.fun.last_val_types.insert(name.clone(), briev_ty);
            }
        }
    }

    /// Emit hoisted post-loop print calls.
    pub(crate) fn emit_hoisted_post_loop_prints(
        &mut self,
        out: &mut String,
        hoisted: &[Vec<Statement>],
    ) {
        for block in hoisted {
            for stmt in block {
                match stmt {
                    // 2026-07-17: The frontend swan-song hoist (analysis/swan_song.rs)
                    // wraps the swan song as Statement::Expression(swan_song_expr).
                    // Also handle TermBang/Term in case other paths produce those variants.
                    Statement::EndProgram(Some(e)) | Statement::Term(Some(e)) | Statement::Expression(e) => {
                        self.emit_expr(out, e, "  ");
                    }
                    // 2026-07-19: Emit let bindings inside the hoisted guard
                    // body so downstream identifier lookups resolve correctly.
                    // Without this, let bindings (e.g. nbody's `let energy:
                    // Float32 = ...`) are silently skipped, and the identifier
                    // fallback creates an undefined global reference (@energy).
                    Statement::Let { .. } => {
                        self.emit_statement(out, stmt, "  ");
                    }
                    _ => {}
                }
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // Trigger Step Function
    // ═══════════════════════════════════════════════════════════════

    /// Emit the trigger step function: check dirty flags and recompute
    /// transactions whose trigger conditions are met.
    pub(crate) fn emit_trg_step(
        &mut self,
        out: &mut String,
        _dep_graph: &crate::analysis::dependency_graph::DependencyGraph,
        trigger_names: &[String],
    ) {
        writeln!(out, "define void @trg_step(ptr %state) local_unnamed_addr {{").ok();
        for trg_name in trigger_names {
            let trg_name_clone = trg_name.clone();
            // 2026-08-27 (cbv-HW plan Slice B): a VALUE-PIN trigger (numeric
            // @-address) is not an event — its body txn never exists and its
            // value is read directly by bodies (volatile load). Excluding it
            // from event dispatch removes the dangling @txn_<pin> call.
            if self.ctx.trg_addresses.contains_key(trg_name_clone.as_str()) {
                continue;
            }
            if let Some(trg) = self.ctx.triggers.get(&trg_name_clone) {
                let trg_name = trg.name.clone();
                let cond_val = self.emit_expr(out, &Expr::Identifier(trg_name.clone()), "  ");
                let bool_reg = self.as_bool_reg(out, "  ", &cond_val);
                let body_label = format!(".trg_{}", trg_name);
                let next_label = format!(".trg_next_{}", trg_name);
                writeln!(out, "  br i1 {}, label %{}, label %{}", bool_reg, body_label, next_label).ok();
                writeln!(out, "  {}:", body_label).ok();
                writeln!(out, "  call void @txn_{}(ptr %state)", trg_name).ok();
                writeln!(out, "  br label %{}", next_label).ok();
                writeln!(out, "  {}:", next_label).ok();
            }
        }
        writeln!(out, "  ret void").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    /// Emit a trigger event via epoll_wait.
    pub(crate) fn emit_trg_event_epoll_wait(
        _backend: &mut LlvmBackend,
        _out: &mut String,
    ) {
        // Placeholder: epoll-based trigger wait is platform-specific
    }

    /// Emit a cycle count increment.
    pub(crate) fn emit_cycle_count_increment(
        _backend: &mut LlvmBackend,
        _out: &mut String,
    ) {
        // Placeholder: cycle counting is architecture-specific
    }

}
