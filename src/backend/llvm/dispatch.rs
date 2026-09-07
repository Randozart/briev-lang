
use crate::analysis::transition_graph;
use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, Type};
use crate::backend::llvm::emit_stmt::emit_statement;
use crate::backend::llvm::{
    find_perfect_hash, sparsity_ratio, FoldParam, FunctionGuard, LlvmBackend,
};
use std::collections::HashMap;
use std::fmt::Write;

impl LlvmBackend {
    // ── REACTOR LOOP ──────────────────────────────────────────
    //
    // Why two dispatch modes (sequential + parallel):
    //
    // Sequential is the fallback for any program where we cannot prove
    // conflict-freedom. It evaluates ALL preconditions first, then fires
    // each txn body. Ordering preconditions before bodies means that if
    // one txn fires and modifies state, subsequent txns still see the
    // pre-tick state for their pre-checks — a txn's decision to fire is
    // based on the state at tick start, not mid-tick modifications.
    //
    // Parallel adds a %fired_mask (bitmask) to track which fields have
    // been written by any previously-fired txn. Before firing, a txn
    // checks (fired_mask & write_mask) == 0 — if any of its written fields
    // were already written by a prior parallel txn, it skips. This avoids
    // the write-after-write hazard without locking.
    //
    // Both modes emit @cell_persistent_ticks(ptr %state) at the end of
    // every tick. This is a single call site for all dispatch paths that
    // ensures cell instances with persistent bodies tick once per reactor
    // cycle. See emit_toplevel.rs.
    pub(crate) fn emit_reactor(
        &mut self,
        out: &mut String,
        txns: &[(String, &crate::ast::Transaction)],
        fusable: &[(String, String)],
    ) {
        self.fused_to_first.clear();
        for (a, b) in fusable {
            let fn_ = format!("{}_{}_fused", a, b);
            self.fused_to_first.insert(fn_, a.clone());
        }
        let mut used_fused: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut dispatch: Vec<String> = Vec::new();
        let mut fused_txns: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (a, b) in fusable {
            let fn_ = format!("{}_{}_fused", a, b);
            if used_fused.contains(&fn_) {
                continue;
            }
            used_fused.insert(fn_.clone());
            fused_txns.insert(a.clone());
            fused_txns.insert(b.clone());
            dispatch.push(fn_);
        }
        for (n, t) in txns {
            if !fused_txns.contains(n) && t.is_reactive {
                dispatch.push(n.clone());
            }
        }

        // 2026-07-27: Select reactor_tick attribute based on whether any
        // reactive txn has unguarded FFI. Guarded FFI (inside `when` blocks)
        // is outlined into cold functions — the hot path stays argmem: readwrite.
        // Unguarded FFI (top-level prints, FFI calls outside guards) forces
        // memory(readwrite) because the call is in the hot path of every tick.
        // 2026-07-31: Phase 2 (§7.4) — the per-txn unguarded-FFI set is computed
        // once in the transition graph (has_unguarded_ffi) instead of re-walking
        // bodies here.
        let has_unguarded = self.ctx.transition_graph.as_ref()
            .map_or(false, |g| !g.has_unguarded_ffi.is_empty());
        let rct_attr = if has_unguarded { "#2" } else { "#12" };
        writeln!(
            out,
            "define void @reactor_tick(ptr noalias nocapture %state) local_unnamed_addr {} {{", rct_attr
        )
        .ok();
        writeln!(out, "  entry:").ok();
        // 2026-06-27: Clear ssa_old regs at reactor_tick entry — they may
        // contain stale entries from the main function emit (e.g., from
        // emit_ssa_main's per-field phi setup or emit_stmt's ssa_old reg
        // update). Without this, inline txn body identifier lookups find
        // stale register names not defined in this function.
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        // Arena init: shared arena for all txns in this tick.
        // Previously each @txn_name had its own 64KB arena (Approach 2),
        // but inlining shares one arena across all txns, saving memory.
        self.emit_arena_init(out, "  ");
        self.sampled_triggers.clear();
        let trigger_snapshot: Vec<(String, crate::ast::TriggerDeclaration)> = self
            .ctx
            .trigger_names
            .iter()
            .filter_map(|tn| self.ctx.triggers.get(tn).map(|t| (tn.clone(), t.clone())))
            .collect();
        for (tn, t) in &trigger_snapshot {
            let sz = format!("%sz_{}", tn);
            self.emit_trg_load(out, "  ", &sz, &t.address, &t.ty);
            self.sampled_triggers.insert(tn.clone(), sz);
        }

        if dispatch.is_empty() {
            self.emit_arena_fini(out, "  ");
            writeln!(out, "  call void @cell_persistent_ticks(ptr %state)").ok();
            writeln!(out, "  ret void").ok();
        } else if fusable.is_empty()
            && dispatch.len() >= 2
            && crate::analysis::transition_graph::is_uniform_body_group(txns)
        {
            // 2026-08-31 (plan abv-gpu-by-default): accel kernels dispatch
            // through their wrapper — see the per-node branch below.
            if self.accel_kernel_idx.contains_key(&dispatch[0]) {
                writeln!(out, "  call void @txn_{}(ptr %state)", dispatch[0]).ok();
            } else {
                self.emit_inline_txn_body(out, "  ", txns, &dispatch[0]);
            }
            writeln!(out, "  call void @cell_persistent_ticks(ptr %state)").ok();
            self.emit_arena_fini(out, "  ");
            writeln!(out, "  ret void").ok();
        } else {
            let mut pre_regs: Vec<String> = Vec::with_capacity(dispatch.len());
            for (i, txn_name) in dispatch.iter().enumerate() {
                let has_pre = self.dispatch_has_pre(txns, txn_name);
                if has_pre {
                    let reg = format!("%pr{}", i);
                    let txn = self.resolve_dispatch_first_txn(txn_name);
                    writeln!(out, "  {} = call i8 @pre_{}(ptr %state)", reg, txn).ok();
                    pre_regs.push(reg);
                } else {
                    pre_regs.push("true".to_string());
                }
            }

            for (i, txn_name) in dispatch.iter().enumerate() {
                let b = format!("b{}", i);
                let c = format!("ck{}", i);
                let pr = &pre_regs[i];
                let pr_i1 = self.fun.gen_reg();
                writeln!(out, "  {} = trunc i8 {} to i1", pr_i1, pr).ok();
                writeln!(out, "  br i1 {}, label %{}, label %{}", pr_i1, b, c).ok();
                writeln!(out, "{}:", b).ok();
                // 2026-09-07 (noalias via dispatch): for multi-txn ticks,
                // call @txn_<name> as a separate function instead of inlining.
                // The @txn_* function carries `noalias nocapture` on its %state
                // parameter (state_ptr_param), so LLVM can promote field loads
                // to SSA registers within the transaction body — impossible
                // when the body is inlined into reactor_tick.
                //
                // Single-txn ticks stay inlined: no benefit from a function
                // boundary when there's only one txn.
                //
                // Accel kernel nodes always dispatch through their wrapper
                // (@txn_<name>: runtime init + device gate + launch).
                if self.accel_kernel_idx.contains_key(txn_name) {
                    writeln!(out, "  call void @txn_{}(ptr %state)", txn_name).ok();
                } else if dispatch.len() >= 2 {
                    writeln!(out, "  call void @txn_{}(ptr %state)", txn_name).ok();
                } else {
                    self.emit_inline_txn_body(out, "  ", txns, txn_name);
                }
                writeln!(out, "  br label %{}", c).ok();
                writeln!(out, "{}:", c).ok();
            }
            // Tick persistent cell! instances every reactor cycle.
            // The @cell_persistent_ticks function is emitted unconditionally
            // in generate() (see emit_toplevel.rs). Single tick point for
            // all dispatch paths that use @reactor_tick.
            self.emit_arena_fini(out, "  ");
            writeln!(out, "  call void @cell_persistent_ticks(ptr %state)").ok();
            writeln!(out, "  ret void").ok();
        }
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    // ── RANGE EXTRACTION ──────────────────────────────────────
    //
    // Extracts [lo, hi) range bounds from precondition expressions for
    // !range metadata annotation. Only handles simple patterns:
    //   And(Lt(x, N), Ge(x, 0))  →  x: (0, N)
    //   Gt(x, M)                 →  x: (M+1, MAX)
    //
    // Why this lives in dispatch.rs instead of emit_toplevel.rs with the
    // !range emission: the range extraction happens during the same pass
    // that builds the dispatch mask, and both share the field_index_map.
    // The ranges are consumed by emit_precondition_check in emit_toplevel.rs.
    pub(crate) fn extract_ranges(pre: &Expr) -> HashMap<String, (i64, i64)> {
        let mut r = HashMap::new();
        // Empty constants map — extract_ranges is called from emit_transaction
        // where constants are resolved via ctx.constants if needed.
        // The actual constants lookup happens in the emit-to-plevel wrapper.
        Self::extract_ranges_inner(pre, &mut r, None);
        r
    }

    /// 2026-07-27: Extract ranges with constant resolution. When the RHS is an
    /// Expr::Identifier (e.g., `[ops < TOTAL]` where TOTAL is a const), resolve
    /// the constant value from the provided map. Falls back to no range if the
    /// constant isn't in the map.
    pub(crate) fn extract_ranges_with_constants(
        pre: &Expr,
        constants: &HashMap<String, (Type, Expr)>,
    ) -> HashMap<String, (i64, i64)> {
        let mut r = HashMap::new();
        Self::extract_ranges_inner(pre, &mut r, Some(constants));
        r
    }
    // 2026-07-04: Unwrap Cast(inner, Int) to find the underlying field name.
    // Ptr<T> fields use "ptr_field as Int" in contracts to constrain the
    // pointer's address range (e.g., [ptr as Int >= BASE && ptr as Int < END]).
    // Without unwrapping, these patterns fall through to @llvm.assume and
    // never produce !range metadata.
    // Other paths for range extraction:
    // - Bare Identifier(n): direct field name (Int, Bool, etc.)
    // - Cast(Identifier(n), Int): Ptr<T> field address range
    fn unwrap_cast_to_ident(e: &Expr) -> Option<&str> {
        match e {
            // 2026-07-31: Phase 3 (§8.4) — cast-to-Int matched against the
            // canonical Type::int() (a bootstrap primitive) instead of the
            // type-name string.
            Expr::Cast(inner, t) if *t == crate::ast::Type::int() => {
                Self::unwrap_cast_to_ident(inner)
            }
            Expr::Identifier(n) => Some(n.as_str()),
            _ => None,
        }
    }
    /// 2026-07-27: Resolve an Expr to a decimal value, either directly (Decimal)
    /// or via constant lookup (Identifier). Returns None if not resolvable.
    fn resolve_to_i64(expr: &Expr, constants: Option<&HashMap<String, (Type, Expr)>>) -> Option<i64> {
        match expr {
            Expr::Decimal(v) => Some(*v),
            Expr::Identifier(name) => {
                if let Some(cm) = constants {
                    if let Some((_, Expr::Decimal(v))) = cm.get(name.as_str()) {
                        return Some(*v);
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub(crate) fn extract_ranges_inner(
        expr: &Expr,
        r: &mut HashMap<String, (i64, i64)>,
        constants: Option<&HashMap<String, (Type, Expr)>>,
    ) {
        match expr {
            Expr::BinaryOp(BinaryOpKind::And, l, rgt) => {
                Self::extract_ranges_inner(l, r, constants);
                Self::extract_ranges_inner(rgt, r, constants);
            }
            Expr::BinaryOp(BinaryOpKind::Lt, l, rgt) => {
                if let Some(n) = Self::unwrap_cast_to_ident(l.as_ref()) {
                    if let Some(v) = Self::resolve_to_i64(rgt.as_ref(), constants) {
                        let e = r.entry(n.to_string()).or_insert((i64::MIN, i64::MAX));
                        if v < e.1 { e.1 = v; }
                    }
                }
            }
            Expr::BinaryOp(BinaryOpKind::Le, l, rgt) => {
                if let Some(n) = Self::unwrap_cast_to_ident(l.as_ref()) {
                    if let Some(v) = Self::resolve_to_i64(rgt.as_ref(), constants) {
                        let e = r.entry(n.to_string()).or_insert((i64::MIN, i64::MAX));
                        if v + 1 < e.1 { e.1 = v + 1; }
                    }
                }
            }
            Expr::BinaryOp(BinaryOpKind::Ge, l, rgt) => {
                if let Some(n) = Self::unwrap_cast_to_ident(l.as_ref()) {
                    if let Some(v) = Self::resolve_to_i64(rgt.as_ref(), constants) {
                        let e = r.entry(n.to_string()).or_insert((i64::MIN, i64::MAX));
                        if v > e.0 { e.0 = v; }
                    }
                }
            }
            Expr::BinaryOp(BinaryOpKind::Gt, l, rgt) => {
                if let Some(n) = Self::unwrap_cast_to_ident(l.as_ref()) {
                    if let Some(v) = Self::resolve_to_i64(rgt.as_ref(), constants) {
                        let e = r.entry(n.to_string()).or_insert((i64::MIN, i64::MAX));
                        if v + 1 > e.0 { e.0 = v + 1; }
                    }
                }
            }
            _ => {}
        }
    }

    // ── DISPATCH HELPERS ─────────────────────────────────────
    pub(crate) fn resolve_dispatch_first_txn(&self, name: &str) -> String {
        self.fused_to_first
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }
    pub(crate) fn dispatch_has_pre(
        &self,
        txns: &[(String, &crate::ast::Transaction)],
        name: &str,
    ) -> bool {
        let first = self.resolve_dispatch_first_txn(name);
        txns.iter()
            .find(|(n, _)| n == &first)
            .map(|(_, t)| !matches!(t.contract.pre_condition, Expr::Bool(true)))
            .unwrap_or(false)
    }

    // ── WRITE MASKS (Parallel Dispatch) ──────────────────────
    //
    // Precomputes a bitmask per transaction where bit N is set if the txn
    // writes to field at index N in field_index_map.
    //
    // 2026-07-31: Phase 3 (§8.3) — the mask is u128 and the EMITTED width is
    // i128 when field_index_map.len() > 64 (write_mask_type), so the 65th+
    // field is no longer silently dropped (the old `idx < 64` gate discarded
    // it). A u128 mask saturates any practical state size; a field index of
    // 128+ is a layout pathology that would need a wider mask (noted here).
    pub(crate) fn build_write_masks(&mut self, items: &[TopLevel]) {
        self.txn_write_masks.clear();
        for item in items {
            if let TopLevel::Transaction(t) = item {
                let writes = crate::backend::collect_assigned_identifiers(&t.body);
                let mut mask = 0u128;
                for w in &writes {
                    if let Some(&idx) = self.ctx.field_index_map.get(w.as_str()) {
                        if idx < 128 {
                            mask |= 1u128 << idx;
                        }
                    }
                }
                self.txn_write_masks.insert(t.name.clone(), mask);
            }
        }
    }

    /// LLVM integer type for write-mask arithmetic: i128 when the program has
    /// more than 64 state fields, else i64.
    ///
    /// 2026-07-31: Phase 3 (§8.3) — derived from field_index_map.len(), not a
    /// hardcoded 64-bit assumption. The `%fired_mask` alloca, and/or/icmp ops
    /// must all use the same width.
    fn write_mask_type(&self) -> &'static str {
        if self.ctx.field_index_map.len() > 64 {
            "i128"
        } else {
            "i64"
        }
    }

    // ── PARALLEL DISPATCH REACTOR ────────────────────────────
    //
    // Emits a reactor_tick that fires conflict-free transactions in parallel.
    // Unlike the sequential reactor (which chains every txn unconditionally),
    // this version tracks which fields have been written via a %fired_mask.
    //
    // Why a mask instead of locks or transactions: the compiler has complete
    // knowledge of every transaction's write set at compile time. A simple
    // bitmask check (load-and-test) is cheaper than any lock acquire or
    // transactional memory instruction. The mask is only 8 bytes on the stack
    // and the check is 3 ALU ops per txn.
    pub(crate) fn emit_parallel_reactor(
        &mut self,
        out: &mut String,
        txns: &[(String, &crate::ast::Transaction)],
        fusable: &[(String, String)],
    ) {
        self.fused_to_first.clear();
        for (a, b) in fusable {
            let fn_ = format!("{}_{}_fused", a, b);
            self.fused_to_first.insert(fn_, a.clone());
        }
        let mut used_fused: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut dispatch: Vec<String> = Vec::new();
        let mut fused_txns: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (a, b) in fusable {
            let fn_ = format!("{}_{}_fused", a, b);
            if used_fused.contains(&fn_) {
                continue;
            }
            used_fused.insert(fn_.clone());
            fused_txns.insert(a.clone());
            fused_txns.insert(b.clone());
            dispatch.push(fn_);
        }
        for (n, t) in txns {
            if !fused_txns.contains(n) && t.is_reactive {
                dispatch.push(n.clone());
            }
        }

        // 2026-07-27: Same FFI-aware attribute selection as emit_reactor.
        // 2026-07-31: Phase 2 (§7.4) — consumes the transition graph's
        // has_unguarded_ffi set instead of re-walking bodies.
        let has_unguarded = self.ctx.transition_graph.as_ref()
            .map_or(false, |g| !g.has_unguarded_ffi.is_empty());
        let rct_attr = if has_unguarded { "#2" } else { "#12" };
        writeln!(
            out,
            "define void @reactor_tick({}) local_unnamed_addr {} {{",
            self.ctx.state_ptr_param, rct_attr
        )
        .ok();
        writeln!(out, "  entry:").ok();
        // 2026-07-01: When the thread pool is active (async txns), the worker
        // threads execute the bodies in parallel on the correct state snapshot.
        // The main thread's reactor_tick must be a no-op — the workers handle
        // everything, synchronized via __barrier_release__/__barrier_wait__.
        if self.has_async_txns && !self.is_lightweight_async {
            writeln!(out, "  ret void").ok();
            writeln!(out, "}}").ok();
            writeln!(out).ok();
            return;
        }
        // 2026-06-27: Clear ssa_old regs at reactor_tick entry (same
        // rationale as the sequential reactor counterpart).
        self.fun.ssa_old_int_regs.clear();
        self.fun.ssa_old_float_regs.clear();
        // Arena init for parallel reactor — shared across all parallel txns.
        self.emit_arena_init(out, "  ");
        self.sampled_triggers.clear();
        let trigger_snapshot: Vec<(String, crate::ast::TriggerDeclaration)> = self
            .ctx
            .trigger_names
            .iter()
            .filter_map(|tn| self.ctx.triggers.get(tn).map(|t| (tn.clone(), t.clone())))
            .collect();
        for (tn, t) in &trigger_snapshot {
            let sz = format!("%sz_{}", tn);
            self.emit_trg_load(out, "  ", &sz, &t.address, &t.ty);
            self.sampled_triggers.insert(tn.clone(), sz);
        }

        // 2026-07-31: Phase 3 (§8.3) — fired-mask width derived from the state
        // field count (i128 when >64 fields) instead of a hardcoded i64.
        let wmt = self.write_mask_type();
        writeln!(out, "  %fired_mask = alloca {}, align 8", wmt).ok();
        writeln!(out, "  store {} 0, ptr %fired_mask", wmt).ok();

        if dispatch.is_empty() {
            self.emit_arena_fini(out, "  ");
            writeln!(out, "  ret void").ok();
        } else {
            let n = dispatch.len();
            for (i, txn_name) in dispatch.iter().enumerate() {
                let has_pre = self.dispatch_has_pre(txns, txn_name);
                if has_pre {
                    let first_txn = self.resolve_dispatch_first_txn(txn_name);
                    writeln!(out, "  %pr{} = call i8 @pre_{}(ptr %state)", i, first_txn).ok();
                } else {
                    writeln!(out, "  %pr{} = add i8 0, 1", i).ok();
                }
            }

            for i in 0..n {
                let txn_name = &dispatch[i];
                let b = format!("b{}", i);
                let next_c = format!("ck{}", i + 1);

                if i == 0 {
                    let pr0_i1 = self.fun.gen_reg();
                    writeln!(out, "  {} = trunc i8 %pr0 to i1", pr0_i1).ok();
                    writeln!(out, "  br i1 {}, label %b0, label %ck1", pr0_i1).ok();
                } else {
                    let c = format!("ck{}", i);
                    writeln!(out, "{}:", c).ok();
                    let wm = self.txn_write_masks.get(txn_name).copied().unwrap_or(0);
                    if wm == 0 {
                        let pr_i1 = self.fun.gen_reg();
                        writeln!(out, "  {} = trunc i8 %pr{} to i1", pr_i1, i).ok();
                        writeln!(out, "  br i1 {}, label %{}, label %{}", pr_i1, b, next_c).ok();
                    } else {
                        let fm = format!("%fm{}", i);
                        let ca = format!("%ca{}", i);
                        let nc = format!("%nc{}", i);
                        // 2026-07-31: Phase 3 (§8.3) — mask width derived from
                        // field count (i128 when >64 fields).
                        let wmt = self.write_mask_type();
                        writeln!(out, "  {} = load {}, ptr %fired_mask", fm, wmt).ok();
                        writeln!(out, "  {} = and {} {}, {}", ca, wmt, fm, wm).ok();
                        writeln!(out, "  {} = icmp eq {} {}, 0", nc, wmt, ca).ok();
                        let nc_i8 = self.fun.gen_reg();
                        writeln!(out, "  {} = zext i1 {} to i8", nc_i8, nc).ok();
                        writeln!(out, "  %can{} = and i8 %pr{}, {}", i, i, nc_i8).ok();
                        let can_i1 = self.fun.gen_reg();
                        writeln!(out, "  {} = trunc i8 %can{} to i1", can_i1, i).ok();
                        writeln!(out, "  br i1 {}, label %{}, label %{}", can_i1, b, next_c).ok();
                    }
                }
            }

            for i in 0..n {
                let txn_name = &dispatch[i];
                let b = format!("b{}", i);
                let next_c = format!("ck{}", i + 1);
                let wm = self.txn_write_masks.get(txn_name).copied().unwrap_or(0);
                writeln!(out, "{}:", b).ok();
                // Inline txn body — shares arena across parallel txns.
                self.emit_inline_txn_body(out, "  ", txns, txn_name);
                if wm != 0 {
                    let fm = format!("%fm{}a", i);
                    let fmu = format!("%fm{}b", i);
                    // 2026-07-31: Phase 3 (§8.3) — mask width derived from
                    // field count (i128 when >64 fields).
                    let wmt = self.write_mask_type();
                    writeln!(out, "  {} = load {}, ptr %fired_mask", fm, wmt).ok();
                    writeln!(out, "  {} = or {} {}, {}", fmu, wmt, fm, wm).ok();
                    writeln!(out, "  store {} {}, ptr %fired_mask", wmt, fmu).ok();
                }
                writeln!(out, "  br label %{}", next_c).ok();
            }

            writeln!(out, "ck{}:", n).ok();
            self.emit_arena_fini(out, "  ");
            writeln!(out, "  ret void").ok();
        }
        writeln!(out, "}}").ok();
        writeln!(out).ok();
    }

    // ── INLINE TXN BODY HELPER ──────────────────────────────────
    //
    // Emits a txn body inline within the reactor_tick function instead of
    // calling it as a separate @txn_name function. Uses FunctionGuard RAII
    // to save/restore all FunctionContext fields, preventing cross-txn
    // state contamination without manual per-field clone/restore.
    //
    // 2026-06-29: Replaced manual 7-field save/restore with FunctionGuard.
    // The guard clones the entire FunctionContext at scope entry and restores
    // it on drop. This ensures new FunctionContext fields are automatically
    // protected without editing this function.
    fn emit_inline_txn_body(
        &mut self,
        out: &mut String,
        indent: &str,
        txns: &[(String, &crate::ast::Transaction)],
        txn_name: &str,
    ) {
        let first_name = self.resolve_dispatch_first_txn(txn_name);
        if let Some((_, txn)) = txns.iter().find(|(n, _)| n == &first_name) {
            // 2026-06-29: FunctionGuard snapshots ALL FunctionContext state;
            // restore() at the end puts it back. No more forgetting fields.
            let guard = FunctionGuard::new(&self.fun);

            self.fun.terminated = false;
            self.fun.returns_i64 = false;

            // Emit precondition assume (for LLVM opt) — the br instruction
            // already guards execution, so this is just for metadata.
            if !matches!(txn.contract.pre_condition, crate::ast::Expr::Bool(true)) {
                self.emit_precondition_check(out, &txn.contract.pre_condition, indent);
            }
            for s in &txn.body {
                if self.fun.terminated {
                    break;
                }
                emit_statement(self, out, s, indent);
            }

            // 2026-07-01: Use restore_preserve_counters — SSA register counters
            // (txn_counter, arena_counter) must stay monotonically increasing
            // across inlined bodies to prevent duplicate %t{N}/%dab{N}/%aa{N}
            // registers. Full restore() would rewind counters.
            guard.restore_preserve_counters(&mut self.fun);
        }
    }

    // ── EXIT CONDITION EXPRESSION ────────────────────────────
    //
    // Validates that every identifier in a #!exit condition references an
    // existing state field, constant, or trigger. This prevents silent
    // reference-to-nowhere bugs where the exit condition always evaluates
    // to false because the field name was mistyped.
    //
    // Why this lives in dispatch.rs: the exit condition is evaluated in the
    // same pass as the main dispatch decision — both need the same
    // field_index_map and trigger_names visibility. Separating it would
    // require duplicating the lookup tables.
    pub(crate) fn check_exit_condition_idents(&self, expr: &Expr) -> Vec<String> {
        let mut errors = Vec::new();
        self.check_exit_condition_idents_inner(expr, &mut errors);
        errors
    }

    pub(crate) fn check_exit_condition_idents_inner(&self, expr: &Expr, errors: &mut Vec<String>) {
        match expr {
            Expr::Identifier(name) => {
                if !self.ctx.field_index_map.contains_key(name)
                    && !self.ctx.constants.contains_key(name)
                    && !self.ctx.trigger_names.contains(name)
                {
                    errors.push(format!(
                        "error: #!exit references unknown variable '{}'\n  note: '{}' is not a state field, constant, or trigger",
                        name, name
                    ));
                }
            }
            Expr::BinaryOp(_, l, r) => {
                self.check_exit_condition_idents_inner(l, errors);
                self.check_exit_condition_idents_inner(r, errors);
            }
            Expr::UnaryOp(_, e) => self.check_exit_condition_idents_inner(e, errors),
            _ => {}
        }
    }
}
