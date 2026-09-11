// ── Loop Emission: Counter-Based Strategies ────────────────────
//
// 2026-07-13: Extracted from monolithic loop_engine.rs (4398 lines).
// Implements strategies 1–3 from the loop emission architecture:
//
//   1. PURE COUNTER FOLD (emit_folded_pure_counter):
//      Pure bodies with compile-time constant bound. O(1) single store.
//
//   2. PURE COUNTER PHI (emit_folded_loop, use_phi=true):
//      Pure bodies with runtime-variable bound. Counter-only phi node,
//      no body emission (body precomputed).
//
//   3. HYBRID COUNTER-PHI + MEMORY FIELDS (emit_countable_main, EmitHybridCounterPhi):
//      Non-pure foldable single-txn programs. Single counter phi + per-field
//      load/store. LLVM SROA converts to closed-SSA phis, avoiding the
//      phi-escape problem that blocks the vectorizer.

use crate::backend::llvm::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write;

/// Configuration for batch-loop mode.
/// When set, the loop is split into an outer structural loop and an inner
/// pure-compute loop. The inner loop runs for `batch_size` iterations without
/// any branch guards, enabling LLVM's if-conversion. The outer loop handles
/// the guard checks (prints, termination) between batches.
///
/// See docs/plans/2026-07-29-loop-peeling-automatic.md
//
// 2026-07-31: BatchInfo removed — the composite-node decomposition
// (emit_version_dag_main) supersedes the heuristic batch-loop. See
// docs/plans/2026-07-30-flat-node-decomposition.md §11.

impl LlvmBackend {
    // ═══════════════════════════════════════════════════════════════
    // Strategy 1: Pure Counter Fold
    // ═══════════════════════════════════════════════════════════════

    /// 2026-08-10: Widen a narrow loop counter to i64 for the bound comparison.
    /// The loop bound is always i64 (emit_countable_load_bound); a counter whose
    /// %State slot is `i{int_bits}` (i32 on wasm32) must be sext'd to i64 before
    /// `icmp slt/sgt i64`. Mirrors emit_countable_main's `counter_ty != "i64"`
    /// arm. x86_64 (counter_ty == i64) returns the counter unchanged.
    fn narrow_counter_for_bound(
        &mut self,
        out: &mut String,
        prefix: &str,
        counter_ty: &str,
        counter_name: &str,
    ) -> String {
        if counter_ty != "i64" {
            let w = self.fun.next_reg_with_prefix(prefix);
            writeln!(out, "  {} = sext {} {} to i64", w, counter_ty, counter_name).ok();
            w
        } else {
            counter_name.to_string()
        }
    }

    /// Emit a pure counter fold: single `store i64` with the final counter
    /// value. No runtime loop. The body was fully precomputed at compile
    /// time within `--optimize-budget`.
    pub(crate) fn emit_folded_pure_counter(
        &mut self,
        out: &mut String,
        counter_idx: usize,
        total_value: i64,
    ) {
        let store_val = format!("{}", total_value);
        self.emit_state_store_i64_by_idx(out, "  ", counter_idx, &store_val);
    }

    // ═══════════════════════════════════════════════════════════════
    // Strategy 2: Folded Loop (Counter Phi, Pure)
    // ═══════════════════════════════════════════════════════════════

    /// Emit a folded loop with counter-only phi. When `use_phi=true`, the
    /// body is precomputed — only the counter phi and backedge are emitted.
    /// When `use_phi=false` with a body, the body statements are emitted
    /// inline with SSA registers.
    ///
    /// 2026-08-03 (loop-guard state-store fix): `counter_var` is the guard
    /// field's NAME. It is registered in `phi_field_regs` → the counter phi so
    /// body READS of the guard field resolve to the live per-iteration phi
    /// value (0,1,2,…) instead of a stale `%State` load (which always returned
    /// the initial 0). This mirrors emit_countable_main's PerFieldPhi mapping.
    pub(crate) fn emit_folded_loop(
        &mut self,
        out: &mut String,
        txn_name: &str,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        label_prefix: &str,
        use_phi: bool,
        body: Option<&[Statement]>,
        unroll_factor: usize,
        is_decreasing: bool,
        bound_literal: Option<i64>,
        counter_var: Option<&str>,
    ) {
        let c0 = self.fun.txn_counter;
        let bound_reg = self.fun.next_reg_with_prefix("flb");
        self.emit_countable_load_bound(out, &bound_reg, total_idx, total_const_name, bound_literal, c0);
        let (init_name, _) = self.emit_state_load_i64_by_idx(out, "  ", counter_idx);
        // 2026-09-07 (init-block phi predecessor fix): cite the block the
        // init load + `br` emit into (self.fun.cur_block, or "entry" when no
        // block-emitting init ran) as the header's init predecessor.
        let init_pred = self.fun.cur_block.clone()
            .unwrap_or_else(|| "entry".to_string());
        // 2026-07-18: Preallocate push targets before the loop body.
        // Collects all Assign(Ident, _) targets from the body and allocates
        // (bound + 2) * 8 bytes per target from the arena (or @malloc if
        // no arena active). This converts per-iteration push overhead from
        // O(N) malloc+memcpy to O(1) direct store.
        if let Some(stmts) = body {
            let mut push_targets: Vec<String> = Vec::new();
            crate::backend::llvm::collect_push_targets(stmts, &mut push_targets);
            push_targets.sort();
            push_targets.dedup();
            if !push_targets.is_empty() {
                self.emit_prealloc_for_targets(out, "  ", &push_targets, &bound_reg);
            }
        }
        // 2026-07-17: Pre-generate backedge register name (forward reference
        // from phi header to latch definition — valid in LLVM IR).
        let next = self.fun.next_reg_with_prefix("fln");
        let exit_label = format!("{}.end", label_prefix);
        writeln!(out, "  br label %{}.header", label_prefix).ok();
        writeln!(out, "{}.header:", label_prefix).ok();
        let counter_name = self.fun.next_reg_with_prefix("flc");
        let done_reg = self.fun.next_reg_with_prefix("fld");
        // 2026-08-10: the counter phi uses the field's actual LLVM type
        // (i{int_bits} for flexible Int on wasm32). The bound is always i64;
        // a narrow counter is sext'd to i64 for the comparison (mirrors
        // emit_countable_main's counter_ty != "i64" arm).
        let counter_ty = self.ctx.field_types.get(counter_idx)
            .cloned().unwrap_or_else(|| "i64".to_string());
        if is_decreasing {
            writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %{}.latch ]",
                counter_name, counter_ty, init_name, init_pred, next, label_prefix).ok();
            // 2026-07-17: Fixed comparison direction. For decreasing counters we
            // want `counter > 0` (continue while still above the bound), not
            // `counter < bound` (which would exit immediately for decreasing).
            let cmp = self.narrow_counter_for_bound(out, "flc", &counter_ty, &counter_name);
            writeln!(out, "  {} = icmp sgt i64 {}, {}", done_reg, cmp, bound_reg).ok();
        } else {
            writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %{}.latch ]",
                counter_name, counter_ty, init_name, init_pred, next, label_prefix).ok();
            // 2026-07-17: Fixed comparison direction. For increasing counters we
            // want `counter < bound` (continue while below the bound), not
            // `counter > bound` (which would exit immediately).
            let cmp = self.narrow_counter_for_bound(out, "flc", &counter_ty, &counter_name);
            writeln!(out, "  {} = icmp slt i64 {}, {}", done_reg, cmp, bound_reg).ok();
        }
        writeln!(out, "  br i1 {}, label %{}.body, label %{}", done_reg, label_prefix, exit_label).ok();
        writeln!(out, "{}.body:", label_prefix).ok();

        // 2026-08-03 (loop-guard state-store fix): register the guard field →
        // counter phi so body reads of `count` resolve to the live phi value,
        // not a stale %State load. Without this, a `println!(count)` in the
        // body prints 0 every iteration (the %State slot is only stored once,
        // after the loop). Cleared before body emission, mirrors
        // emit_countable_main.
        self.fun.phi_field_regs.clear();
        self.fun.backedge_field_regs.clear();
        if let Some(cv) = counter_var {
            self.fun.phi_field_regs.insert(cv.to_string(), counter_name.clone());
            self.fun.backedge_field_regs.insert(cv.to_string(), next.clone());
        }

        if use_phi {
            // Pure phi — no body emission, counter only
        } else if let Some(stmts) = body {
            let write_set: HashSet<String> = HashSet::new();
            let mut hoisted = Vec::new();
            self.emit_countable_body(out, stmts, &write_set, &mut hoisted);
        } else {
            writeln!(out, "  call void @txn_{}(ptr %state)", txn_name).ok();
        }

        writeln!(out, "  br label %{}.latch", label_prefix).ok();
        writeln!(out, "{}.latch:", label_prefix).ok();
        if is_decreasing {
            writeln!(out, "  {} = sub nuw nsw {} {}, 1", next, counter_ty, counter_name).ok();
        } else {
            writeln!(out, "  {} = add nuw nsw {} {}, 1", next, counter_ty, counter_name).ok();
        }
        let disable_fold = body.map_or(false, |b| self.loop_has_observable(b));
        if let Some(b) = body {

        }
        emit_loop_metadata(out, "  ", &format!("{}.header", label_prefix),
            &mut self.fun.metadata_counter, &mut self.fun.pending_metadata, disable_fold);
        writeln!(out, "{}:", exit_label).ok();
        // 2026-08-09 (Phase 10): a folded loop exit runs registered defers.
        self.flush_defer_cleanup(out, "  ");
        self.emit_state_store_i64_by_idx(out, "  ", counter_idx, &counter_name);
    }

    /// Emit a folded loop wrapped in a main() function.
    /// For pure bodies with a known bound: counter-phi loop that stores
    /// the final counter value and returns.
    pub(crate) fn emit_folded_main(
        &mut self,
        out: &mut String,
        txn_name: &str,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        use_phi: bool,
        body: Option<&[Statement]>,
        counter_var: Option<&str>,
    ) {
        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        self.emit_folded_loop(out, txn_name, counter_idx, total_idx, total_const_name,
            ".fmain", use_phi, body, 1, false, bound_literal, counter_var);
        // 2026-09-11 (buffered stdout): flush the stdlib buffer before main
        // returns — the tail bytes reach the fd. Gated: bare/no-stdlib
        // programs have no __stdout_flush and get neither call nor declare.
        self.emit_stdout_flush_tail(out);
        writeln!(out, "  ret i32 0").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    // 2026-07-29: emit_folded_memory_main and emit_while_main removed — dead code
    // after Phase 4 dispatch simplification. PerFieldPhi (emit_countable_main)
    // handles all cases with better SROA characteristics.

    // ═══════════════════════════════════════════════════════════════
    // Strategy 3: Hybrid Countable Loop (EmitHybridCounterPhi)
    // ═══════════════════════════════════════════════════════════════

/// Configuration for batch-loop mode.
/// When set, the loop is split into an outer structural loop and an inner
/// pure-compute loop. The inner loop runs for `batch_size` iterations without
/// any branch guards, enabling LLVM's if-conversion. The outer loop handles
/// the guard checks (prints, termination) between batches.
    /// Emit a countable main() with per-field phi nodes (EmitPerFieldPhi/EmitHybridCounterPhi).
    ///
    /// 2026-07-17: Each state field in write_set gets its own phi node in
    /// the loop header. The body reads from phi registers and writes to
    /// pending_phi_backedge. The latch computes the backedge value for each
    /// field (identity for unwritten, written value for modified).
    ///
    /// Path A (no post-loop hoists): Zero stores in the hot loop body — phi
    /// registers carry all values. Enables LLVM SROA to decompose the loop
    /// into closed-SSA form.
    ///
    /// Path B (post-loop hoists exist): GEP+store emitted for fields the
    /// done: block reads. Ensures hoisted post-loop prints see final values.
    ///
    /// 2026-07-29: Batch mode (batch_info is Some) emits a nested loop
    /// structure with two levels of phi nodes. The outer phis track values
    /// across batches; the inner phis track values within a single batch.
    /// The inner loop has no branches or function calls, enabling LLVM's
    /// if-conversion.
    ///
    /// 2026-07-13: Extracted into a single function with max 2-level
    /// nesting. Loop setup, body, and latch are delegated to helpers.
     pub(crate) fn emit_countable_main(
        &mut self,
        out: &mut String,
        txn_name: &str,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        body: &[Statement],
        write_set: &HashSet<String>,
        is_decreasing: bool,
        counter_var: Option<&str>,
        watchdog: Option<&crate::ast::top::WatchdogSpec>,
    ) {
        self.emit_countable_loop_wrapped(out, txn_name, counter_idx, total_idx, total_const_name,
            bound_literal, body, write_set, is_decreasing, counter_var, watchdog, true);
    }

    /// The PerFieldPhi countdown loop, emitted as the whole `main()` (the
    /// single-node fold) or inside a `define void @txn_<name>(ptr %state)`
    /// function (2026-08-16, multi-node internal fold — Direction 3). In txn
    /// mode the caller has already emitted the function header and `%state` is
    /// the parameter; the main mode emits the header, alloca, and init stores.
    pub(crate) fn emit_countable_loop_wrapped(
        &mut self,
        out: &mut String,
        txn_name: &str,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        body: &[Statement],
        write_set: &HashSet<String>,
        is_decreasing: bool,
        counter_var: Option<&str>,
        watchdog: Option<&crate::ast::top::WatchdogSpec>,
        is_main: bool,
    ) {
        if is_main {
            self.emit_main_header(out, "#0", true);
            self.emit_state_base(out);
            self.emit_inline_init_stores(out, "%state");
            // 2026-09-07 (init-block phi predecessor fix): a state field's
            // `op Init` may emit blocks (HashMap.init's match). In MAIN mode
            // the loop header's single predecessor is `entry` (reached by a
            // plain `br`), so reset the stale block — the init has already
            // stored its result to state. In TXN mode (is_main=false) the
            // caller's cur_block is the loop's true predecessor and is kept.
            self.fun.cur_block = None;
        }
        // 2026-08-13 (reactor fix): buffer the loop construction so the
        // deferred struct-literal allocas can be flushed into the PREHEADER
        // (before the loop). An alloca inside a loop body makes clang -O3 peel
        // the loop and emit a bogus exit assumption — reactive nodes with
        // struct-typed state slots fired exactly once.
        let mut loop_buf = String::new();
        {
        let out = &mut loop_buf;
        let c0 = self.fun.txn_counter;
        let bound_reg = self.fun.next_reg_with_prefix("cmb");
        self.emit_countable_load_bound(out, &bound_reg, total_idx, total_const_name, bound_literal, c0);
        let (init_name, _) = self.emit_state_load_i64_by_idx(out, "  ", counter_idx);

        // 2026-07-17: Pre-load all field initial values from state for per-field phis.
        // Sort deterministically to avoid HashMap iteration non-determinism.
        let mut sorted_fields: Vec<&String> = write_set.iter().collect();
        sorted_fields.sort();
        let mut phi_field_init: HashMap<String, String> = HashMap::new();
        for fname in &sorted_fields {
            let idx = match self.ctx.field_index_map.get(fname.as_str()) {
                Some(&i) => i,
                None => continue,
            };
            let (init_f, _) = self.emit_state_load_i64_by_idx(out, "  ", idx);
            phi_field_init.insert((*fname).clone(), init_f);
        }
        // 2026-09-07 (init-block phi predecessor fix): the init loads emit
        // into self.fun.cur_block — the block the loop `br` targets. When a
        // state field's `op Init` emits blocks (HashMap.init's match), that
        // is the match's .match_end_N, not entry. Cite it in the header phis.
        // Falls back to "entry" when no block-emitting init ran (byte-identical
        // IR).
        let init_pred = self.fun.cur_block.clone()
            .unwrap_or_else(|| "entry".to_string());

        // 2026-07-29: Clear vector phi state — disabled inside emit_countable_main.
        // The dispatch-level detection in mod.rs still checks for vector phi groups,
        // but the actual emission is deferred until the vector phi infrastructure
        // handles all edge cases (duplicate fields, let-binding groups, power-of-2
        // widths, backedge register naming conflicts).
        self.fun.active_vector_groups.clear();
        self.fun.field_to_phi.clear();
        self.fun.field_to_lane.clear();
        self.fun.vector_phi_current.clear();

        // 2026-07-17: Pre-generate backedge register names for per-field phis
        // (forward reference from header phi to latch definition).
        let next = self.fun.next_reg_with_prefix("cmn");
        let mut be_field_regs: HashMap<String, String> = HashMap::new();
        for fname in &sorted_fields {
            let be_f = self.fun.next_reg_with_prefix("pbf");
            be_field_regs.insert((*fname).clone(), be_f);
        }

        let exit_label = format!(".cm_end_{}", self.fun.txn_counter);
        self.fun.txn_counter += 1;
        // 2026-08-01 (D2): a `within N ms` deadline captures the monotonic
        // clock at loop entry; the .cmwd_ check fires when the elapsed time
        // exceeds the deadline even if the liveliness condition still holds.
        let cm_start = if let Some(wd) = watchdog {
            if wd.deadline_ns.is_some() {
                let s = self.fun.gen_reg();
                writeln!(out, "  {} = call i64 @__briev_now(%state)", s).ok();
                Some(s)
            } else {
                None
            }
        } else {
            None
        };
        writeln!(out, "  br label %.cm_header").ok();
        writeln!(out, ".cm_header:").ok();
        let counter_name = self.fun.next_reg_with_prefix("cmc");
        let done_reg = self.fun.next_reg_with_prefix("cmd");

        // Counter phi — use the field's native LLVM type from field_types.
        let counter_ty = self.ctx.field_types.get(counter_idx)
            .cloned().unwrap_or_else(|| "i64".to_string());
        if is_decreasing {
            writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %.cm_latch ]",
                counter_name, counter_ty, init_name, init_pred, next).ok();
        } else {
            writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %.cm_latch ]",
                counter_name, counter_ty, init_name, init_pred, next).ok();
        }

        // 2026-07-17: Per-field phi nodes — one per written field.
        // 2026-07-26: Phi type is read from field_types to match the native
        // LLVM type stored by push_field_type (float/double for Float types,
        // iN for exact ints, i64 for flexible Int and everything else).
        self.fun.phi_field_regs.clear();
        self.fun.backedge_field_regs.clear();

        for fname in &sorted_fields {
            // Check if this field duplicates the counter variable
            if let Some(cv) = counter_var {
                if fname.as_str() == cv {
                    self.fun.phi_field_regs.insert((*fname).clone(), counter_name.clone());
                    self.fun.backedge_field_regs.insert((*fname).clone(), next.clone());
                    continue;
                }
            }
            let phi_f = self.fun.next_reg_with_prefix("ppf");
            let be_f = be_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| format!("%be_{}", fname));
            let init_f = phi_field_init.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            let phi_ty = self.ctx.field_index_map.get(fname.as_str())
                .and_then(|idx| self.ctx.field_types.get(*idx))
                .cloned().unwrap_or_else(|| "i64".to_string());
            writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %.cm_latch ]",
                phi_f, phi_ty, init_f, init_pred, be_f).ok();
            self.fun.phi_field_regs.insert((*fname).clone(), phi_f);
            self.fun.backedge_field_regs.insert((*fname).clone(), be_f);
        }

        // 2026-07-17: Exit check. For increasing counters we want
        // `counter < bound` (continue while below the bound); for decreasing
        // counters we want `counter > 0` (continue while above the bound).
        // The br branches to .cm_body if done_reg is true.
        // 2026-07-26: Counter may be narrower than 64 (native int width from
        // field_types). sext to i64 for comparison with bound (always i64).
        let cmp_counter = if counter_ty != "i64" {
            let w = self.fun.next_reg_with_prefix("cmw");
            writeln!(out, "  {} = sext {} {} to i64", w, counter_ty, counter_name).ok();
            w
        } else {
            counter_name.clone()
        };
        if is_decreasing {
            writeln!(out, "  {} = icmp sgt i64 {}, {}", done_reg, cmp_counter, bound_reg).ok();
        } else {
            writeln!(out, "  {} = icmp slt i64 {}, {}", done_reg, cmp_counter, bound_reg).ok();
        }
        // 2026-08-01 (C2/C3): liveliness watchdog for the memory-counter loop —
        // continue while `?[condition]` holds; on false, fire the handler with
        // the last computed value and exit (mirrors the countdown path).
        if let Some(wd) = watchdog {
            writeln!(out, "  br i1 {}, label %.cmwd_{}, label %{}", done_reg, self.fun.txn_counter, exit_label).ok();
            let wd_c0 = self.fun.txn_counter;
            self.fun.txn_counter += 1;
            writeln!(out, ".cmwd_{}:", wd_c0).ok();
            self.fun.cur_block = Some(format!(".cmwd_{}", wd_c0));
            let cond_reg = self.emit_expr(out, &wd.condition, "  ");
            let bool_reg = self.as_bool_reg(out, "  ", &cond_reg);
            // 2026-08-01 (D2): the `within N` deadline (mirrors the countdown
            // path). Continue = cond AND counter < N AND Now#() - start < N.
            let mut continue_cond = bool_reg;
            if let Some(cyc) = wd.cycles_bound {
                let exp = self.fun.gen_reg();
                writeln!(out, "  {} = icmp slt i64 {}, {}", exp, counter_name, cyc as i64).ok();
                let andr = self.fun.gen_reg();
                writeln!(out, "  {} = and i1 {}, {}", andr, continue_cond, exp).ok();
                continue_cond = andr;
            }
            if let Some(secs) = wd.deadline_ns {
                if let Some(start) = &cm_start {
                    let now = self.fun.gen_reg();
                    writeln!(out, "  {} = call i64 @__briev_now(%state)", now).ok();
                    let el = self.fun.gen_reg();
                    writeln!(out, "  {} = sub i64 {}, {}", el, now, start).ok();
                    let db = self.fun.gen_reg();
                    writeln!(out, "  {} = add i64 0, {}", db, secs as i64).ok();
                    let exp = self.fun.gen_reg();
                    writeln!(out, "  {} = icmp slt i64 {}, {}", exp, el, db).ok();
                    let andr = self.fun.gen_reg();
                    writeln!(out, "  {} = and i1 {}, {}", andr, continue_cond, exp).ok();
                    continue_cond = andr;
                }
            }
            writeln!(out, "  br i1 {}, label %.cm_body, label %.cmwdf_{}", continue_cond, wd_c0).ok();
            writeln!(out, ".cmwdf_{}:", wd_c0).ok();
            self.fun.cur_block = Some(format!(".cmwdf_{}", wd_c0));
            if let Some(on_fire) = &wd.on_fire {
                let call_reg = self.fun.gen_reg();
                let args: Vec<crate::ast::Expr> = match &on_fire.arg {
                    Some(name) => vec![crate::ast::Expr::Identifier(name.clone())],
                    None => Vec::new(),
                };
                self.emit_user_call(out, &call_reg, &on_fire.handler, &args, "  ");
            } else if wd.is_required {
                // 2026-09-10 (Family I): pure-Briev defn takes the hidden %state and
                // returns i64 (defns cannot be void).
                writeln!(out, "  call i64 @__watchdog_fail(ptr %state)").ok();
            }
            writeln!(out, "  br label %{}", exit_label).ok();
        } else {
            writeln!(out, "  br i1 {}, label %.cm_body, label %{}", done_reg, exit_label).ok();
        }
        writeln!(out, ".cm_body:").ok();
        // 2026-08-22 (two-guard fix): the header ALWAYS branches here — the
        // body starts unterminated regardless of what a PREVIOUS emission
        // region left behind (a stale `terminated` skipped my guard blocks'
        // fall-through branch and produced empty predecessor blocks).
        self.fun.terminated = false;
        self.fun.cur_block = Some(".cm_body".to_string());

        // 2026-07-17: Initialize pending_phi_backedge with identity values.
        // Body writes will overwrite entries for modified fields.
        self.fun.pending_phi_backedge.clear();
        for fname in &sorted_fields {
            if let Some(phi_f) = self.fun.phi_field_regs.get(fname.as_str()) {
                self.fun.pending_phi_backedge.insert((*fname).clone(), phi_f.clone());
            }
        }

        // 2026-07-17: Path B (stores in body) for post-loop hoisted prints.
        // 2026-07-21: Also enabled by dispatch when phi-capped fields need stores
        // (float_math_nonzero p22 fix). Use OR to preserve pre-existing value.
        if !self.fun.pending_post_hoist.is_empty() {
            self.fun.needs_state_stores_in_body = true;
            self.collect_swan_song_locals();
        }

        // 2026-07-17: pending_post_hoist (provided by the frontend swan-song
        // hoist, analysis/swan_song.rs) is emitted AFTER the loop closes, not
        // inside the body. The hoisted swan song reads final accumulator values
        // from %State (stored by Path B — needs_state_stores_in_body). Clone to
        // satisfy borrow checker (self.emit_expr needs &mut self;
        // pending_post_hoist is behind &self.fun).
        let hoist = self.fun.pending_post_hoist.clone();
        let mut empty = Vec::new();
        // 2026-08-13 (reactor fix): defer struct-literal allocas in the loop
        // body so they flush to the loop PREHEADER (emit_countable_main's
        // trailing flush) — an in-loop alloca makes clang -O3 peel the loop.
        self.fun.defer_struct_allocas = true;
        self.emit_countable_body(out, body, write_set, &mut empty);
        self.fun.defer_struct_allocas = false;
        writeln!(out, "  br label %.cm_latch").ok();
        writeln!(out, ".cm_latch:").ok();
        // 2026-07-26: Counter increment uses the field's native type, not i64.
        if is_decreasing {
            writeln!(out, "  {} = sub nuw nsw {} {}, 1", next, counter_ty, counter_name).ok();
        } else {
            writeln!(out, "  {} = add nuw nsw {} {}, 1", next, counter_ty, counter_name).ok();
        }

        // 2026-07-17: Per-field scalar backedges. Modified fields use the written value;
        // unwritten fields use identity (phi self-ref). LLVM peephole eliminates
        // the `add i64 0, %val` copy in both cases.
        // 2026-07-21: Skip the counter variable — its backedge is already the
        // latch increment (next). Creating another identity would redefine next.
        // For float fields, the backedge value is already the native float type
        // (skip adapt_to_i64), so the identity would be a type mismatch.
        for fname in sorted_fields.iter().filter(|f| {
            counter_var.map_or(true, |cv| f.as_str() != cv)
        }) {
            if let Some(be_f) = self.fun.backedge_field_regs.get(fname.as_str()) {
                let val = self.fun.pending_phi_backedge.get(fname.as_str())
                    .cloned().unwrap_or_else(|| {
                        self.fun.phi_field_regs.get(fname.as_str())
                            .cloned().unwrap_or_else(|| "0".to_string())
                    });
                // Check if this field is a float type — skip i64 identity
                let field_ty = self.ctx.field_index_map.get(fname.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .cloned().unwrap_or_else(|| "i64".to_string());
                // 2026-07-26: The backedge identity must match the phi type.
                // Float fields use fadd, integer fields use the field's native width.
                if field_ty == "float" || field_ty == "double" {
                    writeln!(out, "  {} = fadd fast {} 0.0, {}", be_f, field_ty, val).ok();
                } else {
                    writeln!(out, "  {} = add {} 0, {}", be_f, field_ty, val).ok();
                }
            }
        }

        let disable_fold = self.loop_has_observable(body);
        emit_loop_metadata(out, "  ", ".cm_header",
            &mut self.fun.metadata_counter, &mut self.fun.pending_metadata, disable_fold);
        writeln!(out, "{}:", exit_label).ok();
        // 2026-07-17: Emit hoisted post-loop prints (swan song) AFTER the loop
        // closes, so they read the final accumulator values from %State. The
        // guard condition was removed by the frontend swan-song hoist
        // (analysis/swan_song.rs); at this point the loop postcondition
        // guarantees it holds.
        // Clear the float cache to prevent reusing fpext registers from the
        // loop body (which may be defined in non-dominating conditional blocks
        // like periodic prints). Without this, the swan song reuses a register
        // defined inside a conditional that never fires for small BOUND values,
        // producing 0.0 from LLVM's undefined value handling — nbody_newton bug.
        self.fun.reg_float_cache.clear();
        // 2026-07-19: Clear last-value temps before hoisted post-loop prints.
        // Without this, identifier resolution uses SSA registers from the loop
        // body which don't dominate the exit block — SSA dominance violation.
        // The hoisted prints must resolve via phi registers or %State loads.
        self.fun.last_val_temps.clear();
        self.fun.last_val_types.clear();
        // 2026-09-07 (swan-song dominance fix): clear the loop SSA maps too.
        // pending_phi_backedge carries body-defined registers (assigns and
        // guard merges register their written value there), phi_field_regs
        // carries the header phis. A body register referenced from the exit
        // block is invalid on the zero-trip path (dominance violation —
        // async-events/nbody_newton), and a header phi holds the pre-loop
        // value there while the watchdog path can exit early. Post-loop field
        // reads must be %State loads: the body stored final values via Path B
        // (needs_state_stores_in_body is set whenever a hoist is pending, see
        // the body prologue). Same rationale as the two clears above — the
        // exit block is outside the loop's SSA domain.
        self.fun.pending_phi_backedge.clear();
        self.fun.phi_field_regs.clear();
        let hoist = self.fun.pending_post_hoist.clone();
        self.emit_hoisted_post_loop_prints(out, &hoist);
        self.emit_state_store_i64_by_idx(out, "  ", counter_idx, &counter_name);
        // 2026-08-06 (Phase 9): garbage scheduling for the PerFieldPhi
        // countable-loop path — free the fields the scheduler PROVED this txn
        // is the last consumer of, AFTER the loop closes (the loop-exit block,
        // so a free never fires inside a body that iterates again). Mirrors the
        // countdown + non-loop paths.
        if let Some(fields) = self.ctx.global_free_after.get(txn_name).cloned() {
            self.emit_scheduled_frees(out, &fields);
        }
        if is_main {
            // 2026-09-11 (buffered stdout): flush the stdlib buffer before main
        // returns — the tail bytes reach the fd. Gated: bare/no-stdlib
        // programs have no __stdout_flush and get neither call nor declare.
        self.emit_stdout_flush_tail(out);
        writeln!(out, "  ret i32 0").ok();
            writeln!(out, "}}").ok();
        } else {
            writeln!(out, "  ret void").ok();
            writeln!(out, "}}").ok();
        }        writeln!(out).ok();
        }
        self.flush_pending_struct_allocas(out);
        out.push_str(&loop_buf);
    }

    // ── Garbage-scheduler free emission ───────────────────────────────

    /// 2026-08-06 (Phase 9): emit `__briev_free` for each scheduled state
    /// field whose reactor-ordered last consumer is the enclosing fold. The
    /// handle is the field's STORED value (the ptrtoint of the allocation),
    /// loaded from %State — re-evaluating the initializer would re-malloc.
    /// Shared by the countdown, PerFieldPhi, and version-DAG fold paths.
    pub(crate) fn emit_scheduled_frees(&mut self, out: &mut String, fields: &[String]) {        for f in fields {
            let Some(&fidx) = self.ctx.field_index_map.get(f) else { continue; };
            let (handle, _) = self.emit_state_load_i64_by_idx(out, "  ", fidx);
            let ptr = self.fun.gen_reg();
            writeln!(out, "  {} = inttoptr i64 {} to ptr", ptr, handle).ok();
            // 2026-09-10 (Family I): pure-Briev defn — hidden %state + i64 return.
            writeln!(out, "  call i64 @__briev_free(ptr %state, ptr {})", ptr).ok();
        }
    }

    // ── Batch-Loop Emission ───────────────────────────────────────────
    //
    // 2026-07-31: Rebuilt from the Phase-6-removed emit_countable_batched_main    // (docs/plans/2026-07-30-flat-node-decomposition.md §4), now consuming the
    // frontend BatchShape (analysis/batch_shape.rs) instead of the
    // extract_batch_size / split_hoistable heuristics. The io boundary is the
    // guard precondition's interval (`count % N == 0`), derived structurally.
    //
    // Only POST-increment guards are batched (the counter is incremented BEFORE
    // the guard, e.g. kalman/float_math) — for them the structure is EXACT:
    // the inner loop runs `batch_size` pure-compute iterations and the guard
    // fires at the boundary after the same number of computes as the composite.
    //
    // Structure (one @main):
    //   entry → .oh (outer header: phis + next boundary) → .inner (inner
    //   header: phis + exit check) → .il (inner pure body + latch) → inner_exit
    //   (fire io guard, store to %State) → .ox (bound check) → .done (post-loop
    //   hoist, ret) / .ol (outer latch: reload, loop).
    pub(crate) fn emit_countable_batched_main(
        &mut self,
        out: &mut String,
        txn_name: &str,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        write_set: &HashSet<String>,
        is_decreasing: bool,
        counter_var: &str,
        batch: &crate::analysis::batch_shape::BatchShape,
    ) {
        let batch_size = batch.batch_size as i64;
        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        // 2026-09-07 (init-block phi predecessor fix): a state field's `op
        // Init` may emit blocks (HashMap.init's match). The outer header's
        // single predecessor is `entry`, so reset the stale block.
        self.fun.cur_block = None;
        // 2026-08-13 (reactor fix): buffer the batched loop so deferred
        // struct-literal allocas flush into the PREHEADER (before the loop) —
        // an in-loop alloca makes clang -O3 peel the loop + emit a bogus exit
        // assumption (nodes with struct-typed state slots fired once).
        let mut batch_buf = String::new();
        {
        let out = &mut batch_buf;
        self.fun.defer_struct_allocas = true;
        let c0 = self.fun.txn_counter;
        self.fun.txn_counter += 1;
        let bound_reg = self.fun.next_reg_with_prefix("obb");
        self.emit_countable_load_bound(out, &bound_reg, total_idx, total_const_name, bound_literal, c0);

        // Pre-load initial values from %State for the outer phis.
        let mut sorted_fields: Vec<&String> = write_set.iter().collect();
        sorted_fields.sort();
        let mut phi_field_init: HashMap<String, String> = HashMap::new();
        for fname in &sorted_fields {
            if let Some(&idx) = self.ctx.field_index_map.get(fname.as_str()) {
                let (init_f, _) = self.emit_state_load_i64_by_idx(out, "  ", idx);
                phi_field_init.insert((*fname).clone(), init_f);
            }
        }
        let init_name = phi_field_init.get(counter_var)
            .cloned().unwrap_or_else(|| "0".to_string());
        // 2026-09-07 (init-block phi predecessor fix): cite the block the init
        // loads + `br` emit into (self.fun.cur_block, or "entry" when no
        // block-emitting init ran) as the outer header's init predecessor.
        let init_pred = self.fun.cur_block.clone()
            .unwrap_or_else(|| "entry".to_string());

        let exit_label = format!(".oexit_{}", c0);
        let inner_exit_label = format!(".inner_exit_{}", c0);
        writeln!(out, "  br label %.oh_{}", c0).ok();

        // ── Outer Header ──────────────────────────────────────────
        // Phis track values across batches. Updated from entry (first batch)
        // or from the outer latch (subsequent batches).
        writeln!(out, ".oh_{}:", c0).ok();
        let oh_counter = self.fun.next_reg_with_prefix("ohc");
        let oh_bound = self.fun.next_reg_with_prefix("ohb");
        let next_oh = self.fun.next_reg_with_prefix("ohn");
        let counter_ty = self.ctx.field_types.get(counter_idx)
            .cloned().unwrap_or_else(|| "i64".to_string());
        writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %.ol_{} ]",
            oh_counter, counter_ty, init_name, init_pred, next_oh, c0).ok();
        writeln!(out, "  {} = phi i64 [ {}, %{} ], [ {}, %.ol_{} ]",
            oh_bound, bound_reg, init_pred, bound_reg, c0).ok();

        let mut oh_field_regs: HashMap<String, String> = HashMap::new();
        let mut oh_latch_regs: HashMap<String, String> = HashMap::new();
        for fname in &sorted_fields {
            if fname.as_str() == counter_var {
                continue;
            }
            let oh_f = self.fun.next_reg_with_prefix("ohs");
            let ol_f = self.fun.next_reg_with_prefix("olf");
            let init_f = phi_field_init.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            let phi_ty = self.ctx.field_index_map.get(fname.as_str())
                .and_then(|idx| self.ctx.field_types.get(*idx))
                .cloned().unwrap_or_else(|| "i64".to_string());
            writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %.ol_{} ]",
                oh_f, phi_ty, init_f, init_pred, ol_f, c0).ok();
            oh_field_regs.insert((*fname).clone(), oh_f);
            oh_latch_regs.insert((*fname).clone(), ol_f);
        }

        // inner_end = min(bound, next_print_boundary)
        // next_print_boundary = ((counter / batch_size) + 1) * batch_size
        let bsize_reg = self.fun.next_reg_with_prefix("bsz");
        writeln!(out, "  {} = add i64 0, {}", bsize_reg, batch_size).ok();
        let div_reg = self.fun.next_reg_with_prefix("bdi");
        writeln!(out, "  {} = udiv i64 {}, {}", div_reg, oh_counter, bsize_reg).ok();
        let add_reg = self.fun.next_reg_with_prefix("bad");
        writeln!(out, "  {} = add i64 {}, 1", add_reg, div_reg).ok();
        let mul_reg = self.fun.next_reg_with_prefix("bmu");
        writeln!(out, "  {} = mul i64 {}, {}", mul_reg, add_reg, bsize_reg).ok();
        let inner_end = self.fun.next_reg_with_prefix("bie");
        writeln!(out, "  {} = call i64 @llvm.umin.i64(i64 {}, i64 {})",
            inner_end, oh_bound, mul_reg).ok();
        writeln!(out, "  br label %.inner_{}", c0).ok();

        // ── Inner Header ──────────────────────────────────────────
        // Phis fed by the outer phis (first iteration) then the inner latch.
        writeln!(out, ".inner_{}:", c0).ok();
        let i_counter = self.fun.next_reg_with_prefix("icc");
        let next_i = self.fun.next_reg_with_prefix("icn");
        writeln!(out, "  {} = phi {} [ {}, %.oh_{} ], [ {}, %.il_{} ]",
            i_counter, counter_ty, oh_counter, c0, next_i, c0).ok();

        self.fun.phi_field_regs.clear();
        self.fun.backedge_field_regs.clear();
        self.fun.phi_field_regs.insert(counter_var.to_string(), i_counter.clone());
        self.fun.backedge_field_regs.insert(counter_var.to_string(), next_i.clone());

        for fname in &sorted_fields {
            if fname.as_str() == counter_var {
                continue;
            }
            let i_f = self.fun.next_reg_with_prefix("ifs");
            let be_f = self.fun.next_reg_with_prefix("ibf");
            let init_f = oh_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            let phi_ty = self.ctx.field_index_map.get(fname.as_str())
                .and_then(|idx| self.ctx.field_types.get(*idx))
                .cloned().unwrap_or_else(|| "i64".to_string());
            writeln!(out, "  {} = phi {} [ {}, %.oh_{} ], [ {}, %.il_{} ]",
                i_f, phi_ty, init_f, c0, be_f, c0).ok();
            self.fun.phi_field_regs.insert((*fname).clone(), i_f);
            self.fun.backedge_field_regs.insert((*fname).clone(), be_f);
        }

        // Inner exit check — continue while counter < inner_end.
        let cmp_i_counter = if counter_ty != "i64" {
            let w = self.fun.next_reg_with_prefix("icw");
            writeln!(out, "  {} = sext {} {} to i64", w, counter_ty, i_counter).ok();
            w
        } else {
            i_counter.clone()
        };
        let exit_reg = self.fun.next_reg_with_prefix("iex");
        if is_decreasing {
            writeln!(out, "  {} = icmp sgt i64 {}, {}", exit_reg, cmp_i_counter, inner_end).ok();
        } else {
            writeln!(out, "  {} = icmp slt i64 {}, {}", exit_reg, cmp_i_counter, inner_end).ok();
        }
        writeln!(out, "  br i1 {}, label %.il_{}, label %{}", exit_reg, c0, inner_exit_label).ok();

        // ── Inner Body + Latch ────────────────────────────────────
        // Pure compute (guard removed). Field reads resolve to the inner phis;
        // writes go to pending_phi_backedge (no per-iteration %State traffic).
        writeln!(out, ".il_{}:", c0).ok();
        self.fun.pending_phi_backedge.clear();
        for fname in &sorted_fields {
            let init_val = self.fun.phi_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            self.fun.pending_phi_backedge.insert((*fname).clone(), init_val.clone());
        }
        let mut empty = Vec::new();
        self.emit_countable_body(out, &batch.inner_body, write_set, &mut empty);
        // Counter increment (native width) — the counter's backedge.
        if is_decreasing {
            writeln!(out, "  {} = sub nuw nsw {} {}, 1", next_i, counter_ty, i_counter).ok();
        } else {
            writeln!(out, "  {} = add nuw nsw {} {}, 1", next_i, counter_ty, i_counter).ok();
        }
        self.fun.pending_phi_backedge.insert(counter_var.to_string(), next_i.clone());
        // Field backedges (skip the counter — its backedge is next_i above).
        for fname in sorted_fields.iter().filter(|f| f.as_str() != counter_var) {
            if let Some(be_f) = self.fun.backedge_field_regs.get(fname.as_str()) {
                let val = self.fun.pending_phi_backedge.get(fname.as_str())
                    .cloned().unwrap_or_else(|| {
                        self.fun.phi_field_regs.get(fname.as_str())
                            .cloned().unwrap_or_else(|| "0".to_string())
                    });
                let field_ty = self.ctx.field_index_map.get(fname.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .cloned().unwrap_or_else(|| "i64".to_string());
                if field_ty == "float" || field_ty == "double" {
                    writeln!(out, "  {} = fadd fast {} 0.0, {}", be_f, field_ty, val).ok();
                } else {
                    writeln!(out, "  {} = add {} 0, {}", be_f, field_ty, val).ok();
                }
            }
        }
        writeln!(out, "  br label %.inner_{}", c0).ok();

        // ── Inner Exit ────────────────────────────────────────────
        // The inner loop completed one batch. Fire the io guard — re-evaluating
        // `count % N == 0` here reads the counter phi (a boundary multiple), so
        // it is true and the body runs. Let-bindings referenced by the guard are
        // remapped to their stored state fields (they are not live here).
        writeln!(out, "{}:", inner_exit_label).ok();
        let mut let_to_field: HashMap<String, String> = HashMap::new();
        for stmt in &batch.inner_body {
            if let Statement::Assign(lhs, Expr::Identifier(let_name)) = stmt {
                if let Some(field_name) = lhs.as_var_name() {
                    // 2026-08-01 (A9b): a field-to-field assignment (`queue = count`,
                    // the `<-` push's lowered AST) is NOT a let-alias — `count` is a
                    // field, so the guard must keep reading the count field, not the
                    // queue. Only a genuine non-field local (`field = local`, the
                    // `sum = acc` hoist pattern) creates an alias.
                    if self.ctx.field_index_map.contains_key(field_name)
                        && !self.ctx.field_index_map.contains_key(let_name)
                    {
                        let_to_field.insert(let_name.clone(), field_name.to_string());
                    }
                }
            }
        }
        let mut guard = batch.guard.clone();
        crate::analysis::swan_song::remap_stmt_identifiers(&mut guard, &let_to_field);
        self.fun.last_val_temps.clear();
        self.fun.last_val_types.clear();
        let mut empty2 = Vec::new();
        self.emit_countable_body(out, std::slice::from_ref(&guard), write_set, &mut empty2);
        // Store final values to %State for the outer latch (per-batch — once per
        // `batch_size` iterations, negligible). Use the inner HEADER phis (they
        // dominate inner_exit; the latch backedge registers do not). The guard
        // above reads these same phis, so the stored values reflect the batch's
        // final state.
        for fname in &sorted_fields {
            let val = self.fun.phi_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            if let Some(&idx) = self.ctx.field_index_map.get(fname.as_str()) {
                self.emit_state_store_i64_by_idx(out, "  ", idx, &val);
            }
        }
        // The counter's header phi is the boundary value (e.g. 5M) at inner_exit.
        let final_counter = i_counter.clone();
        self.emit_state_store_i64_by_idx(out, "  ", counter_idx, &final_counter);
        writeln!(out, "  br label %.ox_{}", c0).ok();

        // ── Outer Body (Termination Check) ────────────────────────
        writeln!(out, ".ox_{}:", c0).ok();
        let (final_count_load, _) = self.emit_state_load_i64_by_idx(out, "  ", counter_idx);
        let done_reg = self.fun.next_reg_with_prefix("odn");
        if is_decreasing {
            writeln!(out, "  {} = icmp sle i64 {}, {}", done_reg, final_count_load, oh_bound).ok();
        } else {
            writeln!(out, "  {} = icmp sge i64 {}, {}", done_reg, final_count_load, oh_bound).ok();
        }
        writeln!(out, "  br i1 {}, label %.done_{}, label %.ol_{}", done_reg, c0, c0).ok();

        // ── Done / Exit ───────────────────────────────────────────
        writeln!(out, ".done_{}:", c0).ok();
        self.fun.reg_float_cache.clear();
        self.fun.last_val_temps.clear();
        self.fun.last_val_types.clear();
        let pending: Vec<Vec<Statement>> = self.fun.pending_post_hoist.clone();
        if !pending.is_empty() {
            for group in &pending {
                let mut empty3 = Vec::new();
                self.emit_countable_body(out, group, &HashSet::new(), &mut empty3);
            }
        }
        // 2026-09-11 (buffered stdout): flush the stdlib buffer before main
        // returns — the tail bytes reach the fd. Gated: bare/no-stdlib
        // programs have no __stdout_flush and get neither call nor declare.
        self.emit_stdout_flush_tail(out);
        writeln!(out, "  ret i32 0").ok();

        // ── Outer Latch ───────────────────────────────────────────
        writeln!(out, ".ol_{}:", c0).ok();
        writeln!(out, "  {} = add {} 0, {}", next_oh, counter_ty, final_counter).ok();
        for fname in &sorted_fields {
            if fname.as_str() == counter_var {
                continue;
            }
            let ol_f = oh_latch_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| self.fun.next_reg_with_prefix("olf"));
            if let Some(&idx) = self.ctx.field_index_map.get(fname.as_str()) {
                let (val, _) = self.emit_state_load_i64_by_idx(out, "  ", idx);
                let field_ty = self.ctx.field_index_map.get(fname.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .cloned().unwrap_or_else(|| "i64".to_string());
                if field_ty == "float" || field_ty == "double" {
                    writeln!(out, "  {} = fadd fast {} 0.0, {}", ol_f, field_ty, val).ok();
                } else {
                    writeln!(out, "  {} = add {} 0, {}", ol_f, field_ty, val).ok();
                }
            }
        }
        writeln!(out, "  br label %.oh_{}", c0).ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
        let _ = txn_name;
        self.fun.defer_struct_allocas = false;
        }
        self.flush_pending_struct_allocas(out);
        out.push_str(&batch_buf);
    }

    // 2026-07-29: emit_countable_memory_main removed — dead code after Phase 4.
    // PerFieldPhi (emit_countable_main) handles all cases.

    // ── Version-DAG Emission ─────────────────────────────────────────
    //
    // ── Countdown-Loop Emission ───────────────────────────────────────
    //
    // 2026-07-31: Single tight loop for periodic post-increment io guards
    // (`when count % N == 0` AFTER count++). Instead of the batch's outer/inner
    // structure, a loop-carried `%rem` counter decrements each iteration; when
    // it reaches 0, a COLD guard block prints and resets `%rem = N`.
    //
    // WHY this shape (plan 2026-07-31-fmn-countdown-vs-batch-and-new-benchmarks):
    // the version-DAG's guard-in-loop costs a modulo + body-split (~5 extra
    // instructions vs C) AND the batch's PURE inner loop lets LLVM's vectorizer
    // mis-vectorize cross-indexed matrix bodies (fmn: 14 shuffle-heavy
    // instructions, slower than 29 scalar). The countdown keeps the loop in ONE
    // block (no body-split), replaces the modulo with `sub;cmp` (2 instructions),
    // and its `%fire` conditional naturally blocks the bad vectorization.
    //
    // Structure:
    //   entry → .cd (header: phis %count/%rem/%fields, bound check)
    //         → .cdb (body ONE block: compute + count++ + rem--)
    //         → .cdg (COLD guard block: print, rem = N)  /  .cdl (latch)
    //         → .cde (done: post-loop hoist, ret)
    pub(crate) fn emit_countable_countdown_main(
        &mut self,
        out: &mut String,
        txn_name: &str,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        write_set: &HashSet<String>,
        counter_var: &str,
        batch: &crate::analysis::batch_shape::BatchShape,
        watchdog: Option<&crate::ast::top::WatchdogSpec>,
        free_after: &[String],
    ) {
        let batch_size = batch.batch_size as i64;
        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        // 2026-09-07 (swan-song dominance fix): the post-hoist (.cde_ block)
        // reads body let-locals (e.g. hash_ops_idio's `sum`) whose SSA
        // registers live in .cdb_ and do NOT dominate .cde_ (a header
        // sibling). Route them through entry-block allocas — same mechanism
        // as PerFieldPhi and version-DAG.
        if !self.fun.pending_post_hoist.is_empty() {
            self.fun.needs_state_stores_in_body = true;
            self.collect_swan_song_locals();
        }
        let c0 = self.fun.txn_counter;
        self.fun.txn_counter += 1;
        let bound_reg = self.fun.next_reg_with_prefix("cdb");
        self.emit_countable_load_bound(out, &bound_reg, total_idx, total_const_name, bound_literal, c0);

        // Pre-load initial values for the header phis.
        // 2026-07-31 (A4): aggregate (array) fields are excluded from phis —
        // they are memory-resident and accessed via the %State GEP path.
        let mut sorted_fields: Vec<&String> = write_set
            .iter()
            .filter(|f| !self.is_aggregate_field(f))
            .collect();
        sorted_fields.sort();
        let mut phi_field_init: HashMap<String, String> = HashMap::new();
        for fname in &sorted_fields {
            if let Some(&idx) = self.ctx.field_index_map.get(fname.as_str()) {
                let (init_f, _) = self.emit_state_load_i64_by_idx(out, "  ", idx);
                phi_field_init.insert((*fname).clone(), init_f);
            }
        }
        let init_name = phi_field_init.get(counter_var)
            .cloned().unwrap_or_else(|| "0".to_string());
        let counter_ty = self.ctx.field_types.get(counter_idx)
            .cloned().unwrap_or_else(|| "i64".to_string());
        // 2026-09-07 (init-block phi predecessor fix): the init loads above
        // emit into self.fun.cur_block — the block the loop `br` targets.
        // When a state field's `op Init` body emits blocks (HashMap.init's
        // `match capacity`), cur_block is the match's `.match_end_N`, NOT
        // entry. The header's true predecessor is cur_block, so the phis
        // must cite it, not `%entry`. When no block-emitting init ran,
        // cur_block is None and init_pred falls back to "entry" (identical
        // IR to today).
        let init_pred = self.fun.cur_block.clone()
            .unwrap_or_else(|| "entry".to_string());
        // 2026-08-01 (D2): a `within N ms` watchdog deadline captures the
        // monotonic clock at loop entry; the .cdw_ check fires when the
        // elapsed time exceeds the deadline even if the liveliness condition
        // still holds.
        let wd_start = if let Some(wd) = watchdog {
            if wd.deadline_ns.is_some() {
                let s = self.fun.gen_reg();
                writeln!(out, "  {} = call i64 @__briev_now(%state)", s).ok();
                Some(s)
            } else {
                None
            }
        } else {
            None
        };
        // 2026-09-07 (swan-song dominance fix): buffer the loop text so the
        // deferred swan-song-local allocas flush into the still-open ENTRY
        // block before the loop — same shape as the PerFieldPhi and
        // version-DAG folds. The body's let-bindings (e.g. hash_ops_idio's
        // `sum`) are read by the post-hoist; their SSA registers live in
        // .cdb_ and do not dominate .cde_ (a header sibling).
        let mut cd_buf = String::new();
        let real_out = out;
        {
        let out = &mut cd_buf;
        self.fun.defer_struct_allocas = true;
        writeln!(out, "  br label %.cd_{}", c0).ok();

        // ── Header ──────────────────────────────────────────────
        writeln!(out, ".cd_{}:", c0).ok();
        let c_counter = self.fun.next_reg_with_prefix("cdc");
        let c_next = self.fun.next_reg_with_prefix("cdn");
        let c_rem = self.fun.next_reg_with_prefix("cdr");
        let c_rem_latch = self.fun.next_reg_with_prefix("cdl");
        // 2026-09-07 (init-block phi predecessor fix): cite init_pred (the
        // block the init loads + `br` emit into) instead of hardcoded %entry.
        // init_pred == "entry" when no block-emitting init ran (byte-identical
        // IR); it is the match's .match_end_N when HashMap.init's match ran.
        writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %.cdl_{} ]",
            c_counter, counter_ty, init_name, init_pred, c_next, c0).ok();
        writeln!(out, "  {} = phi i64 [ {}, %{} ], [ {}, %.cdl_{} ]",
            c_rem, batch_size, init_pred, c_rem_latch, c0).ok();

        self.fun.phi_field_regs.clear();
        self.fun.backedge_field_regs.clear();
        self.fun.phi_field_regs.insert(counter_var.to_string(), c_counter.clone());
        self.fun.backedge_field_regs.insert(counter_var.to_string(), c_next.clone());
        for fname in &sorted_fields {
            if fname.as_str() == counter_var {
                continue;
            }
            let f_reg = self.fun.next_reg_with_prefix("cdf");
            let f_be = self.fun.next_reg_with_prefix("cbe");
            let init_f = phi_field_init.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            let phi_ty = self.ctx.field_index_map.get(fname.as_str())
                .and_then(|idx| self.ctx.field_types.get(*idx))
                .cloned().unwrap_or_else(|| "i64".to_string());
            writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %.cdl_{} ]",
                f_reg, phi_ty, init_f, init_pred, f_be, c0).ok();
            self.fun.phi_field_regs.insert((*fname).clone(), f_reg);
            self.fun.backedge_field_regs.insert((*fname).clone(), f_be);
        }

        // Exit check — continue while count < bound.
        let cmp_counter = if counter_ty != "i64" {
            let w = self.fun.next_reg_with_prefix("cdw");
            writeln!(out, "  {} = sext {} {} to i64", w, counter_ty, c_counter).ok();
            w
        } else {
            c_counter.clone()
        };
        let done_reg = self.fun.next_reg_with_prefix("cdd");
        writeln!(out, "  {} = icmp slt i64 {}, {}", done_reg, cmp_counter, bound_reg).ok();
        // 2026-08-01 (C2/C3): liveliness watchdog — the loop continues while
        // `?[condition]` holds; when it stops, fire the on-fire handler with
        // the last computed value and exit. The check sits between the header
        // and the body (per-iteration), branching to a cold `.wdf_` fire block.
        if let Some(wd) = watchdog {
            writeln!(out, "  br i1 {}, label %.cdw_{}, label %.cde_{}", done_reg, c0, c0).ok();
            writeln!(out, ".cdw_{}:", c0).ok();
            self.fun.cur_block = Some(format!(".cdw_{}", c0));
            let cond_reg = self.emit_expr(out, &wd.condition, "  ");
            let bool_reg = self.as_bool_reg(out, "  ", &cond_reg);
            // 2026-08-01 (D2): the `within N` deadline. The loop CONTINUES
            // while the liveliness condition holds AND no deadline has expired;
            // it FIRES when the condition stops holding OR a deadline expires:
            //   continue = cond AND counter < N  (cycles_bound)
            //             AND Now#() - start < N  (seconds_bound)
            //   cycles_bound: the loop counter reaching N.
            //   seconds_bound: Now#() - start >= N seconds (N * 1e9 ns).
            let mut continue_cond = bool_reg;
            if let Some(cyc) = wd.cycles_bound {
                // !(counter >= N) = counter < N.
                let exp = self.fun.gen_reg();
                writeln!(out, "  {} = icmp slt i64 {}, {}", exp, c_counter, cyc as i64).ok();
                let andr = self.fun.gen_reg();
                writeln!(out, "  {} = and i1 {}, {}", andr, continue_cond, exp).ok();
                continue_cond = andr;
            }
            if let Some(secs) = wd.deadline_ns {
                if let Some(start) = &wd_start {
                    let now = self.fun.gen_reg();
                    writeln!(out, "  {} = call i64 @__briev_now(%state)", now).ok();
                    let el = self.fun.gen_reg();
                    writeln!(out, "  {} = sub i64 {}, {}", el, now, start).ok();
                    let db = self.fun.gen_reg();
                    writeln!(out, "  {} = add i64 0, {}", db, secs as i64).ok();
                    let exp = self.fun.gen_reg();
                    writeln!(out, "  {} = icmp slt i64 {}, {}", exp, el, db).ok();
                    let andr = self.fun.gen_reg();
                    writeln!(out, "  {} = and i1 {}, {}", andr, continue_cond, exp).ok();
                    continue_cond = andr;
                }
            }
            writeln!(out, "  br i1 {}, label %.cdb_{}, label %.wdf_{}", continue_cond, c0, c0).ok();
            // ── Watchdog fired (COLD) ──────────────────────────
            writeln!(out, ".wdf_{}:", c0).ok();
            self.fun.cur_block = Some(format!(".wdf_{}", c0));
            if let Some(on_fire) = &wd.on_fire {
                let call_reg = self.fun.gen_reg();
                let args: Vec<crate::ast::Expr> = match &on_fire.arg {
                    Some(name) => vec![crate::ast::Expr::Identifier(name.clone())],
                    None => Vec::new(),
                };
                self.emit_user_call(out, &call_reg, &on_fire.handler, &args, "  ");
            } else if wd.is_required {
                // Required watchdog with no handler: error exit.
                writeln!(out, "  call void @__watchdog_fail()").ok();
            }
            writeln!(out, "  br label %.cde_{}", c0).ok();
        } else {
            writeln!(out, "  br i1 {}, label %.cdb_{}, label %.cde_{}", done_reg, c0, c0).ok();
        }

        // ── Body (ONE block) ────────────────────────────────────
        writeln!(out, ".cdb_{}:", c0).ok();
        self.fun.cur_block = Some(format!(".cdb_{}", c0));
        self.fun.pending_phi_backedge.clear();
        for fname in &sorted_fields {
            let init_val = self.fun.phi_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            self.fun.pending_phi_backedge.insert((*fname).clone(), init_val.clone());
        }
        let mut empty = Vec::new();
        self.emit_countable_body(out, &batch.inner_body, write_set, &mut empty);
        // Countdown: remaining-- (loop-carried, independent of the counter).
        // 2026-08-01 (B): the rem/fire instructions land in the inner body's
        // FINAL block (cur_block) — an if-ended body leaves the emitter in the
        // if's merge block, so `.cdb_`'s terminator is the if's br. The latch
        // phis below use that block as the `.cdb_` predecessor.
        let body_final = self.fun.cur_block.clone()
            .unwrap_or_else(|| format!(".cdb_{}", c0));
        let c_rem_next = self.fun.next_reg_with_prefix("cdm");
        writeln!(out, "  {} = sub i64 {}, 1", c_rem_next, c_rem).ok();
        let fire = self.fun.next_reg_with_prefix("cdf");
        writeln!(out, "  {} = icmp eq i64 {}, 0", fire, c_rem_next).ok();
        writeln!(out, "  br i1 {}, label %.cdg_{}, label %.cdl_{}", fire, c0, c0).ok();

        // Fields the guard WRITES (e.g. accumulator_flush's `sum = 0` reset)
        // need a latch phi merging the body's value (.cdb) with the guard's
        // (.cdg) — the guard's write register does not dominate the latch.
        // Print-only guards have an empty set and take the plain backedge path.
        let mut guard_writes: HashSet<String> = HashSet::new();
        for stmt in &batch.guard_body {
            if let Statement::Assign(lhs, _) = stmt {
                if let Some(field_name) = lhs.as_var_name() {
                    if self.ctx.field_index_map.contains_key(field_name) {
                        guard_writes.insert(field_name.to_string());
                    }
                }
            }
        }
        // Save the body's per-field values BEFORE the guard overwrites them, so
        // the latch's .cdb phi entry sees the body's compute.
        let body_backedges = self.fun.pending_phi_backedge.clone();

        // ── Guard (COLD — 1 in N iterations) ────────────────────
        // The io guard fires here (remaining == 0 is known true), so only the
        // guard BODY is emitted — no conditional branch structure, keeping
        // .cdl's predecessors exactly {.cdb, .cdg}. The body reads the header
        // phis (post-compute state); let-bindings referenced by the guard are
        // remapped to their stored state fields.
        writeln!(out, ".cdg_{}:", c0).ok();
        self.fun.cur_block = Some(format!(".cdg_{}", c0));
        let mut let_to_field: HashMap<String, String> = HashMap::new();
        for stmt in &batch.inner_body {
            if let Statement::Assign(lhs, Expr::Identifier(let_name)) = stmt {
                if let Some(field_name) = lhs.as_var_name() {
                    // 2026-08-01 (A9b): a field-to-field assignment (`queue = count`,
                    // the `<-` push's lowered AST) is NOT a let-alias — `count` is a
                    // field, so the guard must keep reading the count field, not the
                    // queue. Only a genuine non-field local (`field = local`, the
                    // `sum = acc` hoist pattern) creates an alias.
                    if self.ctx.field_index_map.contains_key(field_name)
                        && !self.ctx.field_index_map.contains_key(let_name)
                    {
                        let_to_field.insert(let_name.clone(), field_name.to_string());
                    }
                }
            }
        }
        let mut guard_body = batch.guard_body.clone();
        for s in &mut guard_body {
            crate::analysis::swan_song::remap_stmt_identifiers(s, &let_to_field);
        }

        // 2026-07-31: Do NOT clear last_val_temps here. The guard fires mid-loop
        // (before the latch), so the header phis still hold the PRE-body values;
        // the current iteration's computed state lives in last_val_temps (the
        // body's assigns, defined in .cdb which dominates .cdg). Clearing it
        // would make the guard print the previous iteration's state — the
        // 5M+1-compute bug (kalman printed 8.188e12 instead of 8.139e12).
        // 2026-08-01 (B): the rem reset is emitted BEFORE the guard body so it
        // is defined in .cdg_ and dominates the guard's control flow. A guard
        // body that ends in a `when` leaves the emitter in the when's
        // next_label; the latch br below lands there, and the latch phis use
        // cur_block (the final block) as the guard predecessor — hardcoding
        // .cdg_ broke the phi's predecessor set for when-ended guards.
        let rem_reset = self.fun.next_reg_with_prefix("cdz");
        writeln!(out, "  {} = add i64 0, {}", rem_reset, batch_size).ok();
        let mut empty2 = Vec::new();
        self.emit_countable_body(out, &guard_body, write_set, &mut empty2);
        writeln!(out, "  br label %.cdl_{}", c0).ok();

        // ── Latch ──────────────────────────────────────────────
        // %rem_latch = phi [remaining-1, body], [N, guard]. All phis (rem +
        // guard-written fields) are grouped at the TOP of the block per LLVM
        // rules; non-phi backedges follow. The guard predecessor is the
        // guard's FINAL block (cur_block) — a when-ended guard branches to
        // .cdl_ from its next_label, not from .cdg_ itself.
        let guard_pred = self.fun.cur_block.clone()
            .unwrap_or_else(|| format!(".cdg_{}", c0));
        writeln!(out, ".cdl_{}:", c0).ok();
        writeln!(out, "  {} = phi i64 [ {}, %{} ], [ {}, %{} ]",
            c_rem_latch, c_rem_next, body_final, rem_reset, guard_pred).ok();
        for fname in sorted_fields.iter().filter(|f| {
            f.as_str() != counter_var && guard_writes.contains(f.as_str())
        }) {
            if let Some(be_f) = self.fun.backedge_field_regs.get(fname.as_str()) {
                let field_ty = self.ctx.field_index_map.get(fname.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .cloned().unwrap_or_else(|| "i64".to_string());
                let body_val = body_backedges.get(fname.as_str())
                    .cloned().unwrap_or_else(|| {
                        self.fun.phi_field_regs.get(fname.as_str())
                            .cloned().unwrap_or_else(|| "0".to_string())
                    });
                let guard_val = self.fun.pending_phi_backedge.get(fname.as_str())
                    .cloned().unwrap_or_else(|| body_val.clone());
                writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %{} ]",
                    be_f, field_ty, body_val, body_final, guard_val, guard_pred).ok();
            }
        }
        // Counter increment (native width) — the counter's backedge.
        writeln!(out, "  {} = add nuw nsw {} {}, 1", c_next, counter_ty, c_counter).ok();
        self.fun.pending_phi_backedge.insert(counter_var.to_string(), c_next.clone());
        // Non-guard-written field backedges (skip the counter — its backedge is
        // c_next above).
        for fname in sorted_fields.iter().filter(|f| {
            f.as_str() != counter_var && !guard_writes.contains(f.as_str())
        }) {
            if let Some(be_f) = self.fun.backedge_field_regs.get(fname.as_str()) {
                let val = self.fun.pending_phi_backedge.get(fname.as_str())
                    .cloned().unwrap_or_else(|| {
                        self.fun.phi_field_regs.get(fname.as_str())
                            .cloned().unwrap_or_else(|| "0".to_string())
                    });
                let field_ty = self.ctx.field_index_map.get(fname.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .cloned().unwrap_or_else(|| "i64".to_string());
                // 2026-08-16 (sweep parity): `fast` on the backedge copy is
                // REQUIRED — a bare `0.0 + x` cannot fold to `x` under strict
                // IEEE (the `-0.0`/signaling-NaN edge), so LLVM kept it as a
                // real floating add on the loop-carried critical path. In the
                // vectorized countdown this surfaced as a live `vaddps <reg>,
                // <zero>` per iteration (the sweep-family loss: sparse 1.37x,
                // mid 1.08x, dense 1.48x). With `fast`, instcombine folds it to
                // the value. The copy's semantic is a value rename (the field's
                // new value), so folding is exact — see
                // docs/plans/2026-08-16-sweep-family-investigation.md §5 P4.
                // Undo: drop the `fast` at all 6 `fadd ... 0.0` copy sites.
                if field_ty == "float" || field_ty == "double" {
                    writeln!(out, "  {} = fadd fast {} 0.0, {}", be_f, field_ty, val).ok();
                } else {
                    writeln!(out, "  {} = add {} 0, {}", be_f, field_ty, val).ok();
                }
            }
        }
        writeln!(out, "  br label %.cd_{}", c0).ok();

        // ── Done / Exit ─────────────────────────────────────────
        writeln!(out, ".cde_{}:", c0).ok();
        self.fun.reg_float_cache.clear();
        self.fun.last_val_temps.clear();
        self.fun.last_val_types.clear();
        // Store final phi values so a post-loop hoist can read them from %State.
        for fname in &sorted_fields {
            let val = self.fun.phi_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            if let Some(&idx) = self.ctx.field_index_map.get(fname.as_str()) {
                self.emit_state_store_i64_by_idx(out, "  ", idx, &val);
            }
        }
        let pending: Vec<Vec<Statement>> = self.fun.pending_post_hoist.clone();
        if !pending.is_empty() {
            for group in &pending {
                let mut empty3 = Vec::new();
                self.emit_countable_body(out, group, &HashSet::new(), &mut empty3);
            }
        }
        // 2026-08-01 (D2): garbage scheduling — emit the `Free#` for each
        // heap-backed state field whose reactor-ordered last consumer is this
        // countdown transaction. The free fires exactly once, after the whole
        // loop completes (a per-iteration free would be a use-after-free). The
        // handle is the field's STORED value (the ptrtoint of the allocation),
        // loaded from %State — re-evaluating the initializer would re-malloc.
        // Routed through __briev_free so the benchmark can assert frees ==
        // allocs (no leak).
        self.emit_scheduled_frees(out, free_after);
        // 2026-09-11 (buffered stdout): flush the stdlib buffer before main
        // returns — the tail bytes reach the fd. Gated: bare/no-stdlib
        // programs have no __stdout_flush and get neither call nor declare.
        self.emit_stdout_flush_tail(out);
        writeln!(out, "  ret i32 0").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
        }
        self.fun.defer_struct_allocas = false;
        self.flush_pending_struct_allocas(real_out);
        real_out.push_str(&cd_buf);
        let _ = txn_name;
    }

    // ── Version-DAG Emission ─────────────────────────────────────────
    //
    // 2026-07-31: Emit the composite-node decomposition for a transaction
    // body containing ONE runtime `when` guard. The body is split at the
    // guard into [pre], [guard], [post] (see analysis/node_decompose.rs).
    // Two versions are emitted:
    //
    //   guard-absent loop:  [pre] → check predicate → [post] (self-terminating)
    //   guard-present block: [pre] → [guard] → [post] (fires when predicate holds)
    //
    // The guard predicate is evaluated BETWEEN [pre] and [post], at the split
    // point — this captures whether the guard observes the counter pre- or
    // post-increment naturally (no position scanning, no counter-name matching).
    //
    // Returns false if the body has no runtime guard or more than one — the
    // caller falls back to PerFieldPhi (emit_countable_main).
    //
    // See docs/plans/2026-07-30-flat-node-decomposition.md §11.

    /// 2026-09-07 (swan-song dominance fix): collect the let-local names the
    /// pending post-hoist reads. State fields and constants resolve at the
    /// emission site; everything else is a body local whose SSA register does
    /// not dominate the loop-exit block. Shared by every fold engine that
    /// emits the post-hoist (PerFieldPhi, version-DAG).
    fn collect_swan_song_locals(&mut self) {
        let fields = self.ctx.field_index_map.clone();
        let constants = self
            .ctx
            .constants
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let mut locals = std::collections::HashSet::new();
        for block in &self.fun.pending_post_hoist {
            collect_hoist_identifiers_expr_stmts(block, &fields, &constants, &mut locals);
        }
        self.fun.swan_song_locals = locals;
    }

    pub(crate) fn emit_version_dag_main(
        &mut self,
        out: &mut String,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        body: &[Statement],
        write_set: &HashSet<String>,
        is_decreasing: bool,
        counter_var: Option<&str>,
        free_after: &[String],
    ) -> bool {
        use crate::analysis::node_decompose::{PredicateClass, Segment, split_into_segments};
        // 2026-08-23 (vd-phi repro fix): a body that ENDS THE PROGRAM mid-loop
        // (endprogram inside a guard) emits `ret` inside the present/latch
        // blocks — the header phis then cite predecessors whose terminators
        // are dead and the whole fold emits invalid IR. The general
        // PerFieldPhi path handles early termination proven-correctly, so
        // DECLINE the fold when any segment body terminates.
        let body_terminates = body.iter().any(|st| {
            matches!(
                st,
                Statement::EndProgram(_)
            ) || {
                let mut found = false;
                if let Statement::Guarded(_, inner) = st {
                    found = inner.iter().any(|s2| {
                        matches!(s2, Statement::EndProgram(_))
                            || matches!(s2, Statement::Guarded(_, deeper)
                                if deeper.iter().any(|s3| matches!(s3, Statement::EndProgram(_))))
                    });
                }
                found
            }
        });
        if body_terminates {
            return false;
        }
        // 2026-09-07 (swan-song dominance fix): the exit block references the
        // post-hoist, whose let-locals must bind through ENTRY-block allocas
        // (entry dominates the guard-present block and the end block). Body
        // allocas defer to pending_struct_allocas and flush into the entry
        // block before the loop text — same shape as emit_countable_main's
        // loop_buf (the split happens inside, at the entry→header boundary).
        if !self.fun.pending_post_hoist.is_empty() {
            self.collect_swan_song_locals();
            self.fun.defer_struct_allocas = true;
        }
        let wrote = self.emit_version_dag_main_inner(
            out, counter_idx, total_idx, total_const_name, bound_literal,
            body, write_set, is_decreasing, counter_var, free_after,
        );
        self.fun.defer_struct_allocas = false;
        wrote
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_version_dag_main_inner(
        &mut self,
        out: &mut String,
        counter_idx: usize,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        body: &[Statement],
        write_set: &HashSet<String>,
        is_decreasing: bool,
        counter_var: Option<&str>,
        free_after: &[String],
    ) -> bool {
        use crate::analysis::node_decompose::{PredicateClass, Segment, split_into_segments};
        let segments = split_into_segments(body);

        // Locate the single runtime guard and collect [pre] / [post] statements.
        let mut pre: Vec<Statement> = Vec::new();
        let mut post: Vec<Statement> = Vec::new();
        let mut runtime_guard: Option<(&Expr, &Vec<Statement>)> = None;
        let mut seen_guard = false;
        for seg in &segments {
            match seg {
                Segment::Compute(stmts) => {
                    if runtime_guard.is_none() {
                        pre.extend(stmts.clone());
                    } else {
                        post.extend(stmts.clone());
                    }
                }
                Segment::Guard { condition, body, classification, .. } => {
                    if seen_guard {
                        return false; // multiple guards — fall back to PerFieldPhi
                    }
                    seen_guard = true;
                    match classification {
                        PredicateClass::Runtime => {
                            runtime_guard = Some((condition, body));
                        }
                        // 2026-07-31: Static predicates are handled by inlining
                        // (always-true) or dropping (always-false). Both mean no
                        // runtime version split is needed — the guard body is
                        // either always executed or never, so we fold it into
                        // [pre]/[post] and let PerFieldPhi emit the single loop.
                        PredicateClass::AlwaysTrue | PredicateClass::AlwaysFalse => {
                            return false;
                        }
                    }
                }
            }
        }
        let Some((guard_cond, guard_body)) = runtime_guard else {
            return false; // no runtime guard — PerFieldPhi
        };

        // ── Emit @main with the guard-absent loop + guard-present block ──
        let c0 = self.fun.txn_counter;
        let vd_prefix = format!("vd{}", c0);
        self.fun.txn_counter += 1;
        let header_label = format!(".{}_header", vd_prefix);
        let absent_label = format!(".{}_absent", vd_prefix);
        let latch_label = format!(".{}_latch", vd_prefix);
        let present_label = format!(".{}_present", vd_prefix);
        let end_label = format!(".{}_end", vd_prefix);

        self.emit_main_header(out, "#0", true);
        self.emit_state_base(out);
        self.emit_inline_init_stores(out, "%state");
        // 2026-09-07 (init-block phi predecessor fix): a state field's `op
        // Init` may emit blocks (HashMap.init's match). The version-DAG
        // header's single predecessor is `entry`, so reset the stale block.
        self.fun.cur_block = None;
        let bound_reg = self.fun.next_reg_with_prefix("vdb");
        self.emit_countable_load_bound(out, &bound_reg, total_idx, total_const_name, bound_literal, c0);
        let (init_name, _) = self.emit_state_load_i64_by_idx(out, "  ", counter_idx);

        let mut sorted_fields: Vec<&String> = write_set.iter().collect();
        sorted_fields.sort();
        let mut phi_field_init: HashMap<String, String> = HashMap::new();
        for fname in &sorted_fields {
            let idx = match self.ctx.field_index_map.get(fname.as_str()) {
                Some(&i) => i,
                None => continue,
            };
            let (init_f, _) = self.emit_state_load_i64_by_idx(out, "  ", idx);
            phi_field_init.insert((*fname).clone(), init_f);
        }
        // 2026-09-07 (init-block phi predecessor fix): cite the block the
        // init loads + `br` emit into (self.fun.cur_block, or "entry" when no
        // block-emitting init ran) as the header's init predecessor.
        let init_pred = self.fun.cur_block.clone()
            .unwrap_or_else(|| "entry".to_string());

        // Pre-generate backedge register names: one set for the latch, one for
        // the present block (both are header predecessors). The counter's
        // backedge registers live in these maps too (indexed by counter_var).
        let mut be_latch_regs: HashMap<String, String> = HashMap::new();
        let mut be_present_regs: HashMap<String, String> = HashMap::new();
        for fname in &sorted_fields {
            be_latch_regs.insert((*fname).clone(), self.fun.next_reg_with_prefix("bl"));
            be_present_regs.insert((*fname).clone(), self.fun.next_reg_with_prefix("bp"));
        }

        let counter_ty = self.ctx.field_types.get(counter_idx)
            .cloned().unwrap_or_else(|| "i64".to_string());
        // 2026-09-07 (swan-song dominance fix): the loop text (header → ret)
        // buffers so the deferred swan-song-local allocas flush into the still
        // open ENTRY block before the loop text — identical to
        // emit_countable_main's loop_buf pattern.
        let mut vd_loop = String::new();
        let real_out = out;
        let out = &mut vd_loop;
        writeln!(out, "  br label %{}", header_label).ok();

        // ── Header: per-field phis ───────────────────────────────────
        // 2026-07-31: Minimal-state classification (Phase 7). Fields never
        // written in the loop are hoisted (no phi); fields written but never
        // read are dropped. Only loop-carried fields get phis. The body
        // includes the guard, so a field read only by the guard is carried.
        let hoist_flat: Vec<Statement> = self.fun.pending_post_hoist.iter()
            .flat_map(|g| g.clone()).collect();
        let observables: Vec<&[Statement]> = vec![guard_body, &hoist_flat];
        let field_classes = crate::analysis::loop_carried::classify_fields(
            &write_set, body, &[], &observables,
        );
        writeln!(out, "{}:", header_label).ok();
        self.fun.phi_field_regs.clear();
        self.fun.backedge_field_regs.clear();
        let counter_name = self.fun.next_reg_with_prefix("vdc");
        let counter_key = counter_var.map(|s| s.to_string()).unwrap_or_else(|| "count".to_string());
        let be_l_count = be_latch_regs.get(&counter_key)
            .cloned().unwrap_or_else(|| self.fun.next_reg_with_prefix("bl"));
        let be_p_count = be_present_regs.get(&counter_key)
            .cloned().unwrap_or_else(|| self.fun.next_reg_with_prefix("bp"));
        writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %{} ], [ {}, %{} ]",
            counter_name, counter_ty, init_name, init_pred, be_l_count, latch_label,
            be_p_count, present_label).ok();
        for fname in &sorted_fields {
            if let Some(cv) = counter_var {
                if fname.as_str() == cv {
                    self.fun.phi_field_regs.insert((*fname).clone(), counter_name.clone());
                    continue;
                }
            }
            let phi_f = self.fun.next_reg_with_prefix("vdf");
            let be_l = be_latch_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| self.fun.next_reg_with_prefix("bl"));
            let be_p = be_present_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| self.fun.next_reg_with_prefix("bp"));
            let init_f = phi_field_init.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            let phi_ty = self.ctx.field_index_map.get(fname.as_str())
                .and_then(|idx| self.ctx.field_types.get(*idx))
                .cloned().unwrap_or_else(|| "i64".to_string());
            // 2026-07-31: Minimal-state — loop-invariant fields are hoisted
            // (their entry load is the hoisted value, no phi); dead fields are
            // dropped; loop-carried fields get a phi.
            match field_classes.get(fname.as_str()) {
                Some(crate::analysis::loop_carried::FieldClass::LoopInvariant) => {
                    self.fun.phi_field_regs.insert((*fname).clone(), init_f);
                }
                Some(crate::analysis::loop_carried::FieldClass::Dead) => {
                    // Skipped — no phi, no backedge, body writes dropped.
                }
                _ => {
                    writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %{} ], [ {}, %{} ]",
                        phi_f, phi_ty, init_f, init_pred, be_l, latch_label, be_p, present_label).ok();
                    self.fun.phi_field_regs.insert((*fname).clone(), phi_f);
                }
            }
        }
        // 2026-07-31: Save the header phi registers — the present block must
        // read them (they dominate it), not the absent body's post-[pre] regs.
        let header_phi_regs: HashMap<String, String> = self.fun.phi_field_regs.clone();
        // 2026-07-31: Exit check AT THE HEADER — evaluated before the guard
        // predicate, so the present block never fires at count == bound.
        // count < bound → absent_body; count >= bound → end.
        let cmp_counter_h = if counter_ty != "i64" {
            let w = self.fun.next_reg_with_prefix("vdw");
            writeln!(out, "  {} = sext {} {} to i64", w, counter_ty, counter_name).ok();
            w
        } else {
            counter_name.clone()
        };
        let done_h = self.fun.next_reg_with_prefix("vdd");
        if is_decreasing {
            writeln!(out, "  {} = icmp sgt i64 {}, {}", done_h, cmp_counter_h, bound_reg).ok();
        } else {
            writeln!(out, "  {} = icmp slt i64 {}, {}", done_h, cmp_counter_h, bound_reg).ok();
        }
        writeln!(out, "  br i1 {}, label %{}, label %{}", done_h, absent_label, end_label).ok();

        // ── Guard-absent body: [pre], predicate check ───────────────
        writeln!(out, "{}:", absent_label).ok();
        self.fun.pending_phi_backedge.clear();
        for fname in &sorted_fields {
            let init_val = self.fun.phi_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            self.fun.pending_phi_backedge.insert((*fname).clone(), init_val);
        }
        self.emit_countable_body(out, &pre, write_set, &mut vec![]);
        // Update phi_field_regs to post-[pre] values so the predicate reads them.
        for fname in &sorted_fields {
            if let Some(v) = self.fun.pending_phi_backedge.get(fname.as_str()) {
                self.fun.phi_field_regs.insert((*fname).clone(), v.clone());
            }
        }
        // Evaluate the guard predicate at the split point.
        let pred_reg = self.emit_expr(out, guard_cond, "  ");
        let pred_bool = self.fun.next_reg_with_prefix("vdb");
        writeln!(out, "  {} = trunc i8 {} to i1", pred_bool, pred_reg.name).ok();
        writeln!(out, "  br i1 {}, label %{}, label %{}",
            pred_bool, present_label, latch_label).ok();

        // ── Latch: [post] + backedge to header ──────────────────────
        writeln!(out, "{}:", latch_label).ok();
        self.emit_countable_body(out, &post, write_set, &mut vec![]);
        // 2026-07-31: The counter increment lives in [pre] or [post] (the
        // source's `count = count + 1` statement). After [post],
        // pending_phi_backedge[count] holds the incremented value. The loop
        // below emits an identity copy to be_latch_regs[count] — the register
        // the header's counter phi references from the latch predecessor.
        // Emit latch backedges for fields (including the counter).
        for fname in &sorted_fields {
            if let Some(be_f) = be_latch_regs.get(fname.as_str()) {
                let val = self.fun.pending_phi_backedge.get(fname.as_str())
                    .cloned().unwrap_or_else(|| {
                        self.fun.phi_field_regs.get(fname.as_str())
                            .cloned().unwrap_or_else(|| "0".to_string())
                    });
                let field_ty = self.ctx.field_index_map.get(fname.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .cloned().unwrap_or_else(|| "i64".to_string());
                if field_ty == "float" || field_ty == "double" {
                    writeln!(out, "  {} = fadd fast {} 0.0, {}", be_f, field_ty, val).ok();
                } else {
                    writeln!(out, "  {} = add {} 0, {}", be_f, field_ty, val).ok();
                }
            }
        }
        // 2026-07-31: Exit check is at the header; the latch just backs to it.
        writeln!(out, "  br label %{}", header_label).ok();

        // ── Guard-present block: [pre] [guard] [post] ───────────────
        writeln!(out, "{}:", present_label).ok();
        // 2026-07-31: Restore phi_field_regs to the header phis so the
        // present block's [pre] reads the loop-carried values (which dominate
        // it), not the absent body's post-[pre] registers (sibling block).
        self.fun.phi_field_regs = header_phi_regs.clone();
        self.fun.last_val_temps.clear();
        self.fun.last_val_types.clear();
        self.fun.pending_phi_backedge.clear();
        for fname in &sorted_fields {
            let init_val = self.fun.phi_field_regs.get(fname.as_str())
                .cloned().unwrap_or_else(|| "0".to_string());
            self.fun.pending_phi_backedge.insert((*fname).clone(), init_val);
        }
        self.emit_countable_body(out, &pre, write_set, &mut vec![]);
        self.emit_countable_body(out, guard_body, write_set, &mut vec![]);
        self.emit_countable_body(out, &post, write_set, &mut vec![]);
        // Present-backedges to the header. The counter increment is in
        // [pre]/[post]; the loop below emits be_present_regs[count] from
        // pending_phi_backedge — the register the header phi references
        // from the present predecessor.
        for fname in &sorted_fields {
            if let Some(be_f) = be_present_regs.get(fname.as_str()) {
                let val = self.fun.pending_phi_backedge.get(fname.as_str())
                    .cloned().unwrap_or_else(|| {
                        self.fun.phi_field_regs.get(fname.as_str())
                            .cloned().unwrap_or_else(|| "0".to_string())
                    });
                let field_ty = self.ctx.field_index_map.get(fname.as_str())
                    .and_then(|idx| self.ctx.field_types.get(*idx))
                    .cloned().unwrap_or_else(|| "i64".to_string());
                if field_ty == "float" || field_ty == "double" {
                    writeln!(out, "  {} = fadd fast {} 0.0, {}", be_f, field_ty, val).ok();
                } else {
                    writeln!(out, "  {} = add {} 0, {}", be_f, field_ty, val).ok();
                }
            }
        }
        writeln!(out, "  br label %{}", header_label).ok();

        // ── End: post-loop prints ───────────────────────────────────
        writeln!(out, "{}:", end_label).ok();
        // 2026-07-31: Materialize ALL written fields' final values to %State.
        // The end block is a successor of the header, so it references the
        // header phi registers (which dominate it), NOT the absent body's
        // post-[pre] registers (sibling block). The post-loop swan song reads
        // these from %State — boundary-only fields (e.g. `escapes`) must be
        // stored here or the print reads the initial value.
        self.fun.phi_field_regs = header_phi_regs;
        for fname in &sorted_fields {
            if let Some(&idx) = self.ctx.field_index_map.get(fname.as_str()) {
                let phi = self.fun.phi_field_regs.get(fname.as_str())
                    .cloned().unwrap_or_else(|| "0".to_string());
                self.emit_state_store_i64_by_idx(out, "  ", idx, &phi);
            }
        }
        let hoist = self.fun.pending_post_hoist.clone();
        if !hoist.is_empty() {
            // 2026-09-07 (swan-song dominance fix): clear pending_phi_backedge
            // with the other loop SSA maps — it carries guard-present/absent
            // body registers (e.g. mandelbrot's %t207 from .vd3_present) that
            // do NOT dominate this end block (a sibling of the header). The
            // final values were just stored to %State above; hoisted reads
            // resolve via %State loads or the entry-block swan-song slots.
            self.fun.pending_phi_backedge.clear();
            self.fun.phi_field_regs.clear();
            self.fun.last_val_temps.clear();
            for group in &hoist {
                self.emit_countable_body(out, group, &HashSet::new(), &mut vec![]);
            }
        }
        // 2026-08-06 (Phase 9): garbage scheduling for the version-DAG fold
        // path — free after the loop closes, like the other fold emitters.
        self.emit_scheduled_frees(out, free_after);
        // 2026-09-11 (buffered stdout): flush the stdlib buffer before main
        // returns — the tail bytes reach the fd. Gated: bare/no-stdlib
        // programs have no __stdout_flush and get neither call nor declare.
        self.emit_stdout_flush_tail(out);
        writeln!(out, "  ret i32 0").ok();
        writeln!(out, "}}").ok();
        writeln!(out).ok();
        // 2026-09-07: splice — deferred swan-song-local allocas land in the
        // still-open entry block, then the buffered loop text follows.
        self.flush_pending_struct_allocas(real_out);
        real_out.push_str(&vd_loop);
        true
    }


    // ═══════════════════════════════════════════════════════════════
    // Countable Loop Helpers
    // ═══════════════════════════════════════════════════════════════

    /// Load the loop bound into a register: from a state field, a const
    /// name, or 0.
    fn emit_countable_load_bound(
        &mut self,
        out: &mut String,
        bound_reg: &str,
        total_idx: Option<usize>,
        total_const_name: Option<&str>,
        bound_literal: Option<i64>,
        _c0: usize,
    ) {
        // 2026-07-20: Pre-allocated bound_reg — use hand-rolled GEP+load
        // because the centralized helper creates its own register name.
        if let Some(lit) = bound_literal {
            // 2026-08-08 (countdown-loop bound fix): a `[ticks < N]` countdown
            // with a compile-time LITERAL N emitted the loop bound as
            // `add i64 0, 1` (the final else fallback), so the loop ran once
            // regardless of N — every literal-bound countdown (spawn pools
            // sized by the analysis included) silently under-ran. Emit the
            // literal directly.
            writeln!(out, "  {} = add i64 0, {}", bound_reg, lit).ok();
        } else if let Some(ti) = total_idx {
            let gep = self.fun.next_reg_with_prefix("clb");
            writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
                gep, ti).ok();
            writeln!(out, "  {} = load i64, ptr {}, align 8", bound_reg, gep).ok();
        } else if let Some(tcn) = total_const_name {
            // 2026-07-17: Resolve bound from compile-time constant value first.
            if let Some((_, Expr::Decimal(val))) = self.ctx.constants.get(tcn) {
                writeln!(out, "  {} = add i64 0, {}", bound_reg, val).ok();
            } else if let Some(&idx) = self.ctx.field_index_map.get(tcn) {
                let gep = self.fun.next_reg_with_prefix("clb");
                writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
                    gep, idx).ok();
                writeln!(out, "  {} = load i64, ptr {}, align 8", bound_reg, gep).ok();
            } else if self.ctx.inits.contains_key(tcn) {
                // 2026-08-09 (init kind, Phase 3): the bound is a runtime-seeded
                // invariant — load its seeded global. Provably invariant, so the
                // fold is sound; no Unknown `add i64 0, 1` fallback.
                writeln!(out, "  {} = load i64, ptr @{}, align 8", bound_reg, tcn).ok();
            } else {
                writeln!(out, "  {} = add i64 0, 1", bound_reg).ok();
            }
        } else {
            writeln!(out, "  {} = add i64 0, 1", bound_reg).ok();
        }
    }

    /// Emit the body of a countable loop. Converts each Statement to the
    /// appropriate SSA load + op + store sequence.
    /// 2026-07-29: SLP gating removed — proven counterproductive. LLVM's SLP
    /// vectorizer has its own cost model. See docs/plans/2026-07-29-full-recovery-plan.md §7.
    fn emit_countable_body(
        &mut self,
        out: &mut String,
        body: &[Statement],
        write_set: &HashSet<String>,
        hoisted: &mut Vec<Vec<Statement>>,
    ) {
        let mut i = 0;
        while i < body.len() {
            let stmt = &body[i];
            match stmt {
                Statement::Let { name, expr: Some(e), .. } => {
                    let reg = self.emit_expr(out, e, "  ");
                    // 2026-09-07 (swan-song dominance fix): a let-local read by
                    // the pending post-hoist binds through a preheader-flushed
                    // alloca (pending_struct_allocas) instead of its body SSA
                    // register — the exit-block hoisted print loads the slot,
                    // which dominates the exit and holds the last-iteration
                    // value. Body-defined registers referenced from the exit
                    // block are a dominance violation (async-ready-gate:
                    // `produced` from briev_await). This arm mirrors the
                    // emit_stmt.rs Let hook — the countable body has its own
                    // statement walk and never routes through emit_statement.
                    let is_struct_ty = match &reg.ty {
                        crate::ast::Type::Custom(n) | crate::ast::Type::Applied(n, _) => {
                            self.ctx.struct_types.contains_key(n)
                        }
                        _ => false,
                    };
                    let reg = if self.fun.swan_song_locals.contains(name)
                        && !self.fun.let_binding_allocas.contains(&reg.name)
                        && !is_struct_ty
                        && !self.is_coll_type(&reg.ty)
                    {
                        let slot_ty = self.llvm_type(&reg.ty);
                        let slot = self.fun.next_reg_with_prefix("sslv");
                        self.fun.pending_struct_allocas.push(
                            format!("  {} = alloca {}, align 8", slot, slot_ty),
                        );
                        let store_val = self.ensure_typed_value(
                            out, "  ", &slot_ty, &reg.name,
                            Some(reg.ty.clone()), None,
                        );
                        writeln!(out, "  store {} {}, ptr {}", slot_ty, store_val, slot).ok();
                        self.fun.let_binding_allocas.insert(slot.clone());
                        crate::backend::llvm::TypedRegister { name: slot, ty: reg.ty.clone() }
                    } else {
                        reg
                    };
                    self.fun.last_val_temps.insert(name.clone(), reg.name.clone());
                    self.fun.last_val_types.insert(name.clone(), reg.ty.clone());
                    // 2026-08-26 (async Phase C): track defn-spawn / handle-
                    // move bindings so a later `free` cancels through the
                    // scheduler (the standard emitter's FreeHint arm reads
                    // this set). Mirrors emit_stmt's Let hook.
                    match e {
                        crate::ast::Expr::Spawn { type_name, .. }
                            if self.ctx.defn_params.contains_key(type_name.as_str()) =>
                        {
                            self.fun.task_handle_names.insert(name.clone());
                        }
                        crate::ast::Expr::Identifier(src)
                            if self.fun.task_handle_names.contains(src) =>
                        {
                            self.fun.task_handle_names.insert(name.clone());
                        }
                        _ => {}
                    }
                    // 2026-08-07 (instance pools): a spawned handle local
                    // (`let h: Counter = spawn ...`) must bind through
                    // let_bindings/let_binding_types too — the member-call /
                    // field resolution (instance_prefix_for) reads those, not
                    // last_val_temps. Without them h.inc() fell back to the
                    // boxed self (inttoptr the row) and dereferenced address 1.
                    self.fun.let_bindings.insert(name.clone(), reg.name.clone());
                    // 2026-08-26 (async Phase D): a declared Event<T> pin wins
                    // over the column-read register type. A port projection
                    // (`let wire: Event<P> = bus.evt`) loads an i64 wire id
                    // whose column row types as Int; without the pin the
                    // spawn-site wrapper re-wraps the arg into a PRIVATE slot
                    // and payload projections deref address 0.
                    if let Statement::Let { ty: Some(dt), .. } = stmt {
                        if crate::backend::llvm::emit_stmt::is_event_type(dt) {
                            self.fun.let_binding_types.insert(name.clone(), dt.clone());
                            self.fun.let_original_types.insert(name.clone(), dt.clone());
                        } else {
                            self.fun.let_binding_types.insert(name.clone(), reg.ty.clone());
                            self.fun.let_original_types.insert(name.clone(), reg.ty.clone());
                        }
                    } else {
                        self.fun.let_binding_types.insert(name.clone(), reg.ty.clone());
                        self.fun.let_original_types.insert(name.clone(), reg.ty.clone());
                    }
                }
                Statement::Assign(lhs, expr) => {
                    let lhs_name = Self::assign_target_name(lhs);
                     let val = self.emit_expr(out, expr, "  ");
                      // 2026-08-01 (A9b): `<-` op dispatch — when the LHS is a
                      // collection with an InsertAt op binding (`queue <- count`),
                      // emit the self-bound member call (push) instead of a scalar
                      // field backedge. The collection field is aggregate (excluded
                      // from the phis); its data write is memory-resident in %State.
                      let insert_strat = self.find_insert_strategy(lhs).cloned();
                      if let Some(op_def) = &insert_strat {
                          if super::emit_stmt::emit_strategy_member_call(self, out, "  ", lhs, op_def, Some(&val.name)).is_none() {
                              super::emit_stmt::emit_strategy_fn_call(self, out, "  ", lhs, op_def, Some(&val.name));
                          }
                          i += 1;
                          continue;
                      }
                       if let Some(ref n) = lhs_name {
                         if write_set.contains(n) {
                              // 2026-07-29: Vector phi routing — if field belongs to
                              // a vector group, record_field_update instead of scalar backedge.
                              // Clone lookup data to avoid borrow conflicts with &mut self.fun.
                              let is_vector_grouped = self.fun.field_to_phi.contains_key(n.as_str());
                              let (groups_clone, lane_map_clone) = if is_vector_grouped {
                                  (Some(self.fun.active_vector_groups.clone()),
                                   Some(self.fun.field_to_lane.clone()))
                              } else {
                                  (None, None)
                              };
                              if let (Some(ref g), Some(ref l)) = (groups_clone, lane_map_clone) {
                                  crate::backend::llvm::vector_phi::record_field_update(
                                      &mut self.fun, n, &val.name, g, l,
                                  );
                              } else {
                                  // 2026-07-21: Float fields use native type in backend —
                                  // skip adapt_to_i64 and store the float value directly.
                                  let field_ty = self.ctx.field_index_map.get(n)
                                      .and_then(|idx| self.ctx.field_types.get(*idx))
                                      .cloned().unwrap_or_else(|| "i64".to_string());
                                  if field_ty == "float" || field_ty == "double" {
                                      self.fun.pending_phi_backedge.insert(n.clone(), val.name.clone());
                                  } else if field_ty == "i64" {
                                      // 2026-08-10: Bool/String/Data/Ptr slots are
                                      // i64 — box the value (adapt_to_i64 widens
                                      // i32→i64, ptrtoints, etc.) so the backedge
                                      // phi (typed field_ty=i64) matches.
                                      let boxed = self.adapt_to_i64(out, "  ", &val);
                                      self.fun.pending_phi_backedge.insert(n.clone(), boxed);
                                  } else {
                                      // 2026-08-10: flexible Int/UInt slots are
                                      // i{int_bits} (i32 wasm32) — the body value
                                      // is already that width (binop_int_type),
                                      // so the backedge phi gets it directly.
                                      // adapt_to_i64 here would emit
                                      // `sext i32 <i32 val> to i64`, mismatching
                                      // the `phi i32` backedge.
                                      self.fun.pending_phi_backedge.insert(n.clone(), val.name.clone());
                                  }
                              }
                          }
                        // 2026-07-17: When post-loop hoisted prints need final values,
                        // emit state stores for ALL fields, not just phi-tracked ones.
                        // Without this, fields outside the capped write_set (max 6)
                        // silently lose their values between iterations — the body
                        // computes the new value, but it's never stored back to %State.
                        if self.fun.needs_state_stores_in_body {
                            if let Some(&idx) = self.ctx.field_index_map.get(n) {
                                // 2026-07-20: Intentionally hand-rolled — needs adapt_to_i64
                                // fallback when val_ty != field_ty (float→i64 store).
                                // 2026-07-19: Store with native type for %State struct
                                // compatibility. Phi backedge uses i64, but the state
                                // store matches the field's LLVM type (float/double).
                                let field_ty = &self.ctx.field_types[idx];
                                let val_ty = self.llvm_type(&val.ty);
                                let gep = self.fun.next_reg_with_prefix("cms");
                                writeln!(out, "  {} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
                                    gep, idx).ok();
                                if val_ty == *field_ty {
                                    writeln!(out, "  store {} {}, ptr {}, align 8", field_ty, val.name, gep).ok();
                                } else {
                                    let boxed = self.adapt_to_i64(out, "  ", &val);
                                    writeln!(out, "  store i64 {}, ptr {}, align 8", boxed, gep).ok();
                                }
                            }
                        }
                        self.fun.last_val_temps.insert(n.clone(), val.name.clone());
                        self.fun.last_val_types.insert(n.clone(), val.ty.clone());
                    }
                    // 2026-07-21: Handle pointer-indexed stores (data[idx] = val)
                    // and deref stores (*ptr = val) inside convergence loops.
                    // Without this, emit_countable_body silently drops these
                    // assignments (assign_target_name returns None for non-Ident).
                    match lhs {
                        Expr::Index(obj, idx) => {
                            // 2026-08-01 (B): array-state field store
                            // (`f[i] = v` for Float[16]) — route through
                            // emit_array_state_store like the normal path. The
                            // countdown previously only handled Ptr-indexed
                            // stores, silently DROPPING array-state writes
                            // (the seed + the loop's f[j]=n[j] both vanished).
                            if super::emit_stmt::emit_array_state_store(self, out, "  ", obj, idx, &val) {
                                i += 1;
                                continue;
                            }
                            let obj_reg = self.emit_expr(out, obj, "  ");
                            if matches!(obj_reg.ty, Type::Ptr(_)) {
                                let idx_reg = self.emit_expr(out, idx, "  ");
                                let ptr = self.fun.gen_reg();
                                writeln!(out, "  {} = inttoptr i64 {} to ptr", ptr, obj_reg.name).ok();
                                let gep = self.fun.gen_reg();
                                let offset = self.fun.gen_reg();
                                // Only List/tuple literals have a length header at slot 0.
                                if matches!(obj.as_ref(), Expr::List(_) | Expr::Tuple(_)) {
                                    writeln!(out, "  {} = add i64 {}, 1", offset, idx_reg.name).ok();
                                } else {
                                    writeln!(out, "  {} = add i64 {}, 0", offset, idx_reg.name).ok();
                                }
                                writeln!(out, "  {} = getelementptr i64, ptr {}, i64 {}", gep, ptr, offset).ok();
                                writeln!(out, "  store i64 {}, ptr {}", val.name, gep).ok();
                            }
                        }
                        Expr::Deref(inner) => {
                            let ptr_reg = self.emit_expr(out, inner, "  ");
                            // 2026-07-30: Ptr values are stored as i64 internally;
                            // convert back to LLVM ptr before storing through.
                            let store_ptr = if matches!(ptr_reg.ty, Type::Ptr(_)) {
                                let p = self.fun.gen_reg();
                                writeln!(out, "  {} = inttoptr i64 {} to ptr", p, ptr_reg.name).ok();
                                p.to_string()
                            } else {
                                ptr_reg.name.clone()
                            };
                            writeln!(out, "  store i64 {}, ptr {}", val.name, store_ptr).ok();
                        }
                        _ => {}
                    }
                }
                Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
                    let val = self.emit_expr(out, e, "  ");
                    let name = format!("%t{}", self.fun.txn_counter);
                    self.fun.txn_counter += 1;
                    writeln!(out, "  {} = add i64 0, {}", name, val.name).ok();
                }
                Statement::Defer(body) => {
                    // 2026-08-09 (Phase 10): register cleanup on the fold's
                    // defer stack; flush_defer_cleanup emits it at loop exit.
                    self.fun.defer_bodies.push(body.clone());
                    i += 1;
                    continue;
                }
                Statement::Mutex(body) | Statement::Barrier { body, .. } => {
                    self.emit_countable_body(out, body, write_set, hoisted);
                    i += 1;
                    continue;
                }
                Statement::Guarded(cond, stmts) => {
                    // 2026-08-22 (Phase 6b dominance fix): a field written ONLY
                    // inside the guard yields a register defined on the then-
                    // path; the latch backedge and later guards must not cite
                    // it directly (clang: "does not dominate all uses"). At the
                    // merge block we insert one phi per conditionally-written
                    // field — [written, body] / [pre-value, fall-through] — and
                    // re-point pending_phi_backedge at the phi. Conditional
                    // last_val_temps entries are dropped instead (cross-guard
                    // reads reload via the normal paths; intra-guard chaining
                    // is untouched because the map is snapshotted, not cleared).
                    // 2026-08-22 (two-guard repro fix): the fall-through
                    // predecessor is a FRESHLY LABELED condition block — never
                    // the inherited cur_block, which may name a pre-loop guard
                    // merge from an earlier emission region (the two-guard task
                    // repro cited %guard.end49 as a loop-internal predecessor).
                    let pre_guard_block = format!(".cmgc{}", self.fun.txn_counter);
                    self.fun.txn_counter += 1;
                    // LLVM has no implicit fall-through: branch into the
                    // condition block unless the previous statement already
                    // terminated its block.
                    if !self.fun.terminated {
                        writeln!(out, "  br label %{}", pre_guard_block).ok();
                    }
                    self.fun.terminated = false;
                    writeln!(out, "{}:", pre_guard_block).ok();
                    self.fun.cur_block = Some(pre_guard_block.clone());
                    let lvt_before = self.fun.last_val_temps.clone();
                    let pending_before = self.fun.pending_phi_backedge.clone();
                    let cond_reg = self.emit_expr(out, cond, "  ");
                    let bool_reg = self.as_bool_reg(out, "  ", &cond_reg);
                    let body_label = format!(".cmgb{}", self.fun.txn_counter);
                    let next_label = format!(".cmgn{}", self.fun.txn_counter);
                    self.fun.txn_counter += 1;
                    writeln!(out, "  br i1 {}, label %{}, label %{}", bool_reg, body_label, next_label).ok();
                    writeln!(out, "{}:", body_label).ok();
                    self.emit_countable_body(out, stmts, write_set, hoisted);
                    // Snapshot what the BODY wrote, keyed by field, BEFORE the
                    // merge phis (their inputs come from this block).
                    let mut written: Vec<(String, String)> = Vec::new();
                    for (f, v) in &self.fun.pending_phi_backedge {
                        if pending_before.get(f).map(|old| old != v).unwrap_or(true) {
                            written.push((f.clone(), v.clone()));
                        }
                    }
                    written.sort();
                    writeln!(out, "  br label %{}", next_label).ok();
                    writeln!(out, "{}:", next_label).ok();
                    for (f, v) in &written {
                        let Some(&idx) = self.ctx.field_index_map.get(f.as_str()) else { continue };
                        let ty = self.ctx.field_types.get(idx)
                            .cloned().unwrap_or_else(|| "i64".to_string());
                        let old_v = pending_before.get(f).cloned()
                            .unwrap_or_else(|| "0".to_string());
                        let mrg = self.fun.next_reg_with_prefix("cmgm");
                        writeln!(out, "  {} = phi {} [ {}, %{} ], [ {}, %{} ]",
                            mrg, ty, v, body_label, old_v, pre_guard_block).ok();
                        self.fun.pending_phi_backedge.insert(f.clone(), mrg);
                    }
                    for (f, v) in &self.fun.last_val_temps {
                        if lvt_before.get(f).map(|old| old != v).unwrap_or(true) {
                            // Conditionally-defined temp: drop so later reads
                            // don't cite a register from a non-dominating block.
                            let _ = (f, v);
                        }
                    }
                    self.fun.last_val_temps = lvt_before;
                    self.fun.cur_block = Some(next_label);
                }
                Statement::Block(stmts) => {
                    self.emit_countable_body(out, stmts, write_set, hoisted);
                }
                // 2026-08-22 (two-guard repro audit): free/keep fell into
                // `_ => {}` here — SILENTLY DROPPED in countable bodies. The
                // eager model has nothing to free at runtime, but the hint
                // must still flow through the standard emitter so any future
                // lowering sees it; silence would hide real semantics later.
                Statement::FreeHint(_) | Statement::KeepHint(_) | Statement::Yield => {
                    super::emit_stmt::emit_statement(self, out, stmt, "  ");
                }
                Statement::Expression(e) => {
                    // 2026-08-01 (A10): `<- &collection` discard — dispatch the
                    // ExtractFrom member call (self-bound pop), not just emit the
                    // address. Without it the pop never runs: a Stack's len never
                    // decrements and the next push overflows the buffer.
                    if let Expr::AddrOf(source) = e {
                        let strat = self.find_extract_strategy(source)
                            .or_else(|| self.find_extract_strategy(e)).cloned();
                        if let Some(op_def) = &strat {
                            if super::emit_stmt::emit_strategy_member_call(self, out, "  ", source, op_def, None).is_none() {
                                super::emit_stmt::emit_strategy_fn_call(self, out, "  ", source, op_def, None);
                            }
                        }
                        let _ = self.fun.gen_reg();
                    } else {
                        self.emit_expr(out, e, "  ");
                    }
                }
                Statement::ArrowAssign { .. } => {
                    // 2026-08-01 (Phase 4): the arrow (stream write, collection
                    // insert/extract, discard) — delegate to the standard emitter;
                    // the loop engine's hand-rolled body walker must not drop it.
                    super::emit_stmt::emit_statement(self, out, stmt, "  ");
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Emit a single guard statement (when cond { body } or term! -> print)
    /// as a simple if-block, without phi backedge tracking or field write sets.
    /// Suitable for outer loop guards and post-loop termination prints.
    fn emit_guard_block(&mut self, out: &mut String, stmt: &Statement, indent: &str) {
        match stmt {
            Statement::Guarded(cond, body) => {
                let cond_reg = self.emit_expr(out, cond, indent);
                let bool_reg = self.as_bool_reg(out, indent, &cond_reg);
                let body_label = format!(".ogb{}", self.fun.txn_counter);
                let next_label = format!(".ogn{}", self.fun.txn_counter);
                self.fun.txn_counter += 1;
                writeln!(out, "{}br i1 {}, label %{}, label %{}", indent, bool_reg, body_label, next_label).ok();
                writeln!(out, "{}:", body_label).ok();
                for s in body {
                    self.emit_guard_body_stmt(out, s, indent);
                }
                writeln!(out, "{}br label %{}", indent, next_label).ok();
                writeln!(out, "{}:", next_label).ok();
            }
            Statement::Term(Some(e)) | Statement::Expression(e) | Statement::EndProgram(Some(e)) => {
                self.emit_expr(out, e, indent);
            }
            Statement::ArrowAssign { .. } => {
                super::emit_stmt::emit_statement(self, out, stmt, indent);
            }
            _ => {}
        }
    }

    /// Emit a single statement inside a guard body (Let → compute, Expression → call).
    fn emit_guard_body_stmt(&mut self, out: &mut String, stmt: &Statement, indent: &str) {
        match stmt {
            Statement::ArrowAssign { .. } => {
                // 2026-08-01 (Phase 4): the arrow — delegate to the standard
                // emitter so guard-body stream writes / collection ops survive.
                super::emit_stmt::emit_statement(self, out, stmt, indent);
            }
            Statement::Let { name, expr: Some(e), .. } => {
                let reg = self.emit_expr(out, e, indent);
                self.fun.last_val_temps.insert(name.clone(), reg.name.clone());
                self.fun.last_val_types.insert(name.clone(), reg.ty.clone());
                self.fun.let_bindings.insert(name.clone(), reg.name.clone());
                self.fun.let_binding_types.insert(name.clone(), reg.ty.clone());
                self.fun.let_original_types.insert(name.clone(), reg.ty.clone());
            }
            Statement::Expression(e) => {
                self.emit_expr(out, e, indent);
            }
            Statement::Guarded(cond, body) => {
                self.emit_guard_block(out, stmt, indent);
            }
            Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
                self.emit_expr(out, e, indent);
            }
            Statement::Assign(lhs, expr) => {
                let val = self.emit_expr(out, expr, indent);
                if let Expr::Identifier(n) = lhs {
                    self.fun.last_val_temps.insert(n.clone(), val.name.clone());
                    self.fun.last_val_types.insert(n.clone(), val.ty.clone());
                }
            }
            _ => {}
        }
    }

    /// Extract the field name from an assignment left-hand side.
    fn assign_target_name(lhs: &Expr) -> Option<String> {
        match lhs {
            Expr::Identifier(n) => Some(n.clone()),
            _ => None,
        }
    }

}

// ── 2026-09-07 (swan-song dominance fix) ────────────────────────────
//
// Collect the free identifiers of a pending post-hoist. Names that are state
// fields or constants resolve at the emission site (%State load / immediate);
// everything else is a body-local whose SSA register does not dominate the
// exit block — those names must bind through preheader-flushed allocas
// (FunctionContext.swan_song_locals consulted by the Statement::Let emitter).

fn collect_hoist_identifiers_expr_stmts(
    stmts: &[Statement],
    fields: &HashMap<String, usize>,
    constants: &std::collections::HashSet<String>,
    out: &mut std::collections::HashSet<String>,
) {
    for s in stmts {
        collect_hoist_identifiers_stmt(s, fields, constants, out);
    }
}

fn collect_hoist_identifiers_stmt(
    s: &Statement,
    fields: &HashMap<String, usize>,
    constants: &std::collections::HashSet<String>,
    out: &mut std::collections::HashSet<String>,
) {
    match s {
        Statement::Expression(e) | Statement::EndProgram(Some(e)) | Statement::Term(Some(e)) => {
            collect_hoist_identifiers(e, fields, constants, out);
        }
        Statement::Let { expr: Some(e), .. } => {
            collect_hoist_identifiers(e, fields, constants, out);
        }
        _ => {}
    }
}

fn collect_hoist_identifiers(
    e: &Expr,
    fields: &HashMap<String, usize>,
    constants: &std::collections::HashSet<String>,
    out: &mut std::collections::HashSet<String>,
) {
    match e {
        Expr::Identifier(n) => {
            if !fields.contains_key(n) && !constants.contains(n) {
                out.insert(n.clone());
            }
        }
        Expr::BinaryOp(_, l, r) | Expr::Index(l, r) => {
            collect_hoist_identifiers(l, fields, constants, out);
            collect_hoist_identifiers(r, fields, constants, out);
        }
        Expr::UnaryOp(_, x) => collect_hoist_identifiers(x, fields, constants, out),
        Expr::Call(_, args, _) => {
            for a in args {
                collect_hoist_identifiers(a, fields, constants, out);
            }
        }
        Expr::Field(base, _) => collect_hoist_identifiers(base, fields, constants, out),
        Expr::MethodCall(recv, _, args, _) => {
            collect_hoist_identifiers(recv, fields, constants, out);
            for a in args {
                collect_hoist_identifiers(a, fields, constants, out);
            }
        }
        Expr::Reflect(recv, _, _) => collect_hoist_identifiers(recv, fields, constants, out),
        // 2026-09-07: the frontend swan-song hoist wraps the guard body as an
        // Expression(Block([...])) — walk the inner statements too
        // (async-ready-gate repro).
        Expr::Block(stmts) => {
            for s in stmts {
                collect_hoist_identifiers_stmt(s, fields, constants, out);
            }
        }
        _ => {}
    }
}

impl LlvmBackend {
    /// 2026-09-11 (buffered stdout): gated epilogue flush — see the
    /// has_stdout_flush field on the backend.
    fn emit_stdout_flush_tail(&mut self, out: &mut String) {
        if self.has_stdout_flush {
            writeln!(out, "  %__flush = call i64 @__stdout_flush(ptr %state)").ok();
        }
    }
}
