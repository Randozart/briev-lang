// ── Loop Emission: Module Root ──────────────────────────────────
//
// 2026-07-13: Split from monolithic loop_engine.rs (4398 lines)
// into four submodules: mod.rs (dispatch + exit eval), counter.rs
// (counter-based strategies), ssa.rs (SSA register pipeline), and
// analysis.rs (free-standing helpers). All old-style Expr/Statement
// patterns migrated to new-style unified variants.
//
// Loop strategies (documented per submodule):
//   counter.rs — Pure Counter Fold, Pure Counter Phi, Hybrid Countable
//   ssa.rs     — SSA Register Pipeline, Modulo Switch, Folded Multi-Txn

pub mod analysis;
pub mod counter;
pub mod ssa;

use crate::backend::llvm::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write;

impl LlvmBackend {
    // ═══════════════════════════════════════════════════════════════
    // Exit Expression Evaluator
    // ═══════════════════════════════════════════════════════════════

    /// Recursively evaluate a boolean expression for loop exit conditions.
    /// All values are emitted as `i64` for uniformity; comparisons are
    /// zext'd from `i1`. Handles both new-style unified Expr variants
    /// (`BinaryOp`, `UnaryOp`) and resolves state field references to
    /// GEP+load sequences.
    pub(crate) fn emit_exit_expr(
        &mut self,
        out: &mut String,
        expr: &Expr,
        indent: &str,
    ) -> String {
        let v = self.fun.next_reg();
        match expr {
            Expr::Decimal(n) => {
                return self.emit_expr(out, expr, indent).name;
            }
            Expr::Float(f) => {
                let bits = f.to_bits() as i64;
                writeln!(out, "{}{} = add i64 0, {}", indent, v, bits).ok();
                return v;
            }
            Expr::Bool(_) | Expr::Quoted(_) => {
                return self.emit_expr(out, expr, indent).name;
            }
            Expr::Identifier(name) => {
                return self.emit_exit_identifier(out, indent, name, &v);
            }
            Expr::UnaryOp(kind, inner) => {
                let op = self.emit_exit_expr(out, inner, indent);
                match kind {
                    crate::ast::UnaryOpKind::Not => {
                        let one = format!("%eno{}", self.fun.txn_counter);
                        self.fun.txn_counter += 1;
                        writeln!(out, "{}{} = xor i64 {}, 1", indent, one, op).ok();
                        writeln!(out, "{}{} = and i64 {}, 1", indent, v, one).ok();
                    }
                    _ => {
                        writeln!(out, "{}{} = add i64 0, {}", indent, v, op).ok();
                    }
                }
                return v;
            }
            Expr::BinaryOp(kind, l, r) => {
                return self.emit_exit_binop(out, indent, kind, l, r, &v);
            }
            _ => {
                return self.emit_expr(out, expr, indent).name;
            }
        }
    }

    /// Emit a state field load for an exit condition identifier.
    fn emit_exit_identifier(
        &mut self,
        out: &mut String,
        indent: &str,
        name: &str,
        v: &str,
    ) -> String {
        let Some(&idx) = self.ctx.field_index_map.get(name) else {
            return self.emit_expr(out, &Expr::Identifier(name.to_string()), indent).name;
        };
        // 2026-07-20: Intentionally hand-rolled — multi-type load with zext/ptrtoint normalization
        // to i64. The centralized emit_state_load_i64_by_idx assumes i64-only field type.
        let p = self.fun.next_reg_with_prefix("gep_exit");
        writeln!(out, "{}{} = getelementptr inbounds %State, ptr %state, i32 0, i32 {}",
            indent, p, idx).ok();
        let ft = &self.ctx.field_types[idx];
        match ft.as_str() {
            "i64" => {
                writeln!(out, "{}{} = load i64, ptr {}, align 8", indent, v, p).ok();
            }
            "i32" => {
                let l = self.fun.next_reg_with_prefix("exit_l");
                writeln!(out, "{}{} = load i32, ptr {}, align 4", indent, l, p).ok();
                writeln!(out, "{}{} = zext i32 {} to i64", indent, v, l).ok();
            }
            "i8" => {
                let l = self.fun.next_reg_with_prefix("exit_l");
                writeln!(out, "{}{} = load i8, ptr {}, align 1", indent, l, p).ok();
                writeln!(out, "{}{} = zext i8 {} to i64", indent, v, l).ok();
            }
            s if s == "i8*" || s == "ptr" => {
                let l = self.fun.next_reg_with_prefix("exit_l");
                writeln!(out, "{}{} = load ptr, ptr {}, align 8", indent, l, p).ok();
                writeln!(out, "{}{} = ptrtoint ptr {} to i64", indent, v, l).ok();
            }
            _ if ft.starts_with("float") => {
                let l = self.fun.next_reg_with_prefix("exit_l");
                writeln!(out, "{}{} = load i32, ptr {}, align 4", indent, l, p).ok();
                writeln!(out, "{}{} = zext i32 {} to i64", indent, v, l).ok();
            }
            _ => {
                writeln!(out, "{}{} = load i64, ptr {}, align 8", indent, v, p).ok();
            }
        }
        v.to_string()
    }

    /// Emit a binary operation for exit condition evaluation.
    fn emit_exit_binop(
        &mut self,
        out: &mut String,
        indent: &str,
        kind: &crate::ast::BinaryOpKind,
        l: &Expr,
        r: &Expr,
        v: &str,
    ) -> String {
        match kind {
            crate::ast::BinaryOpKind::Eq
            | crate::ast::BinaryOpKind::Neq
            | crate::ast::BinaryOpKind::Lt
            | crate::ast::BinaryOpKind::Gt
            | crate::ast::BinaryOpKind::Le
            | crate::ast::BinaryOpKind::Ge => {
                let (op, negate) = self.exit_comparison_op(kind);
                let left = self.emit_exit_expr(out, l, indent);
                let right = self.emit_exit_expr(out, r, indent);
                let cmp = self.fun.next_reg_with_prefix("ecmp");
                writeln!(out, "{}{} = icmp {} i64 {}, {}", indent, cmp, op, left, right).ok();
                if negate {
                    let not_cmp = self.fun.next_reg_with_prefix("enot");
                    writeln!(out, "{}{} = xor i1 {}, -1", indent, not_cmp, cmp).ok();
                    writeln!(out, "{}{} = zext i1 {} to i64", indent, v, not_cmp).ok();
                } else {
                    writeln!(out, "{}{} = zext i1 {} to i64", indent, v, cmp).ok();
                }
            }
            crate::ast::BinaryOpKind::And => {
                let left = self.emit_exit_expr(out, l, indent);
                let right = self.emit_exit_expr(out, r, indent);
                // 2026-07-26: Use i64 for all exit-typed values.
                // Bool fields go through zext i8 to i64 in emit_exit_identifier.
                writeln!(out, "{}{} = and i64 {}, {}", indent, v, left, right).ok();
            }
            crate::ast::BinaryOpKind::Or => {
                let left = self.emit_exit_expr(out, l, indent);
                let right = self.emit_exit_expr(out, r, indent);
                writeln!(out, "{}{} = or i64 {}, {}", indent, v, left, right).ok();
            }
            _ => {
                let left = self.emit_exit_expr(out, l, indent);
                let right = self.emit_exit_expr(out, r, indent);
                let op = self.exit_arith_op(kind);
                writeln!(out, "{}{} = {} i64 {}, {}", indent, v, op, left, right).ok();
            }
        }
        v.to_string()
    }

    /// Map a comparison BinaryOpKind to LLVM icmp predicate and negation flag.
    fn exit_comparison_op(&self, kind: &crate::ast::BinaryOpKind) -> (&'static str, bool) {
        match kind {
            crate::ast::BinaryOpKind::Eq => ("eq", false),
            crate::ast::BinaryOpKind::Neq => ("eq", true),
            crate::ast::BinaryOpKind::Lt => ("slt", false),
            crate::ast::BinaryOpKind::Gt => ("sgt", false),
            crate::ast::BinaryOpKind::Le => ("sle", false),
            crate::ast::BinaryOpKind::Ge => ("sge", false),
            _ => ("eq", false),
        }
    }

    /// Map an arithmetic BinaryOpKind to LLVM integer opcode.
    fn exit_arith_op(&self, kind: &crate::ast::BinaryOpKind) -> &'static str {
        match kind {
            crate::ast::BinaryOpKind::Add => "add",
            crate::ast::BinaryOpKind::Sub => "sub",
            crate::ast::BinaryOpKind::Mul => "mul",
            crate::ast::BinaryOpKind::Div => "sdiv",
            crate::ast::BinaryOpKind::Mod => "srem",
            crate::ast::BinaryOpKind::BitAnd => "and",
            crate::ast::BinaryOpKind::BitOr => "or",
            crate::ast::BinaryOpKind::BitXor => "xor",
            crate::ast::BinaryOpKind::Shl => "shl",
            crate::ast::BinaryOpKind::Shr => "lshr",
            _ => "add",
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // Main Loop Entry Points
    // ═══════════════════════════════════════════════════════════════

    /// 2026-07-14: Emit an exit condition check at a loop header.
    /// Evaluates exit_condition, truncates to i1, branches to .end on true.
    pub(crate) fn emit_exit_check(&mut self, out: &mut String) {
        let cond = match self.ctx.exit_condition.as_ref() {
            Some(c) => c.clone(),
            None => return,
        };
        let val = self.emit_exit_expr(out, &cond, "  ");
        let t = self.fun.gen_reg();
        writeln!(out, "  {} = trunc i64 {} to i1", t, val).ok();
        writeln!(out, "  br i1 {}, label %.end, label %.continue", t).ok();
        writeln!(out, ".continue:").ok();
    }

    /// Emit the main() function using a memcpy round-trip + reactor_tick.
    /// Used for multi-txn reactive programs with wake triggers.
    /// The memcpy round-trip saves/restores all state fields so each
    /// reactor tick sees a consistent snapshot.
pub(crate) fn emit_main(&mut self, out: &mut String, has_wake_triggers: bool) {
    self.emit_main_header(out, "#0", true);
    self.emit_state_base(out);
    self.emit_inline_init_stores(out, "%state");
    // 2026-09-07 (init-block phi predecessor fix): a state field's `op
    // Init` may emit blocks (HashMap.init's match) — emit_main_header
    // started a fresh function with cur_block = None, so after init the
    // insertion block is the init's FINAL block, not entry. Reset it:
    // this emitter's loop header (.loop) is reached by a single `br`
    // from entry, and any block-emitting init has already stored its
    // result to state — citing the stale block would make the header
    // phis name a block that is not their predecessor.
    self.fun.cur_block = None;
    // 2026-09-10 (async convergence fix): the exit check is the LOOP
    // HEADER. The old shape ran the check ONCE at entry, then .loop's
    // latch branched back to .loop — the convergence predicate was never
    // re-evaluated, so bounded async programs spun forever after
    // converging (prints landed, exit never did). Now: entry ->
    // .exit_check -> tick -> .exit_check -> ...
    writeln!(out, "  br label %.exit_check").ok();
    writeln!(out, ".exit_check:").ok();
    self.emit_exit_check(out);
    writeln!(out, "  %state_save = alloca %State, align 8").ok();
    writeln!(out, "  br label %.loop").ok();
    writeln!(out, ".loop:").ok();
    let state_bytes = self.compute_state_size_bytes() as i64;
    writeln!(out, "  call void @llvm.memcpy.p0p0i64(ptr %state_save, ptr %state, i64 {}, i1 false)",
        state_bytes).ok();
    // 2026-07-14: Use async phase for async programs
    if self.has_async_txns && !self.is_lightweight_async {
        self.emit_async_phase(out, "%state");
    } else {
        writeln!(out, "  call void @reactor_tick(ptr noalias nocapture %state)").ok();
    }
    // 2026-07-18: Convergence exit logic.
    // - Wake triggers: check @llvm.wake.any(); exit if none pending.
    // - Exit condition set (natural death): loop back to entry check.
    // - No wake triggers & no restartable txns: one-shot program. Check
    //   if any txn precondition is still true; exit if all converged.
    // - Otherwise: call __wait_for_trigger__() and loop (reactive system).
    let has_exit_cond = self.ctx.exit_condition.is_some();
    let is_one_shot = !has_wake_triggers && !self.has_async_txns;
    if has_wake_triggers {
        writeln!(out, "  %any_active = call i1 @llvm.wake.any()").ok();
        writeln!(out, "  br i1 %any_active, label %.exit_check, label %.end").ok();
    } else if has_exit_cond {
        writeln!(out, "  br label %.exit_check").ok();
    } else if is_one_shot {
        // 2026-07-18: One-shot program — no exit condition analysis available,
        // no wake triggers. After reactor_tick, any immediately-fireable txn
        // has run. Since no restart is possible, exit unconditionally.
        // Note: bounded-counter txns with exit_condition set are already
        // handled by the previous branch and run a proper loop.
        writeln!(out, "  br label %.end").ok();
    } else {
        writeln!(out, "  call i64 @__wait_for_trigger__(ptr %state)").ok();
        writeln!(out, "  br label %.exit_check").ok();
    }
    writeln!(out, ".end:").ok();
    writeln!(out, "  ret i32 0").ok();
    writeln!(out, "}}").ok();
    writeln!(out).ok();
}

    /// 2026-07-18: Emit IR to check if ANY reactive txn's precondition is
    /// still true. Returns the register name holding i1 (1 = active, 0 = all idle).
    /// Called when exit_condition is set (by natural death logic).
    pub(crate) fn emit_any_txn_active(&mut self, out: &mut String) -> String {
        // Precondition: exit_condition is set. Evaluate it, negate: if exit
        // condition is NOT yet met (value = 0), some txn is still active.
        let exit_cond = self.ctx.exit_condition.clone();
        if let Some(cond) = exit_cond {
            let val = self.emit_exit_expr(out, &cond, "  ");
            let not_done = self.fun.gen_reg();
            writeln!(out, "  {} = icmp eq i64 {}, 0", not_done, val).ok();
            return not_done;
        }
        // No exit condition — conservative: assume active.
        let one = self.fun.gen_reg();
        writeln!(out, "  {} = add i64 0, 1", one).ok();
        one
    }
}
