// 2026-07-25: VM backend — expression bytecode emission.
// Each expression pushes its result onto the operand stack.

use super::assembler::Assembler;
use super::{VmBackend, canonical_host_id};
use crate::ast::Expr;
use crate::ast::*;

impl VmBackend {
    /// Emit VM bytecode for an expression. Result is left on the stack.
    pub(crate) fn emit_expr(&mut self, expr: &Expr) {
        let expr = expr.clone();
        self.emit_expr_inner(&expr);
    }

    fn emit_expr_inner(&mut self, expr: &Expr) {
        match expr {
            // ── Literals ───────────────────────────────────────────────
            Expr::Decimal(n) => {
                let v = *n;
                if v >= i16::MIN as i64 && v <= i16::MAX as i64 {
                    self.asm.emit_push_i16(v as i16);
                } else if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
                    self.asm.emit_push_i32(v as i32);
                } else {
                    self.asm.emit_push_i64(v);
                }
            }

            Expr::Bool(b) => {
                self.asm.emit_push_i8(if *b { 1 } else { 0 });
            }

            Expr::Float(_) => {
                // 2026-08-23 (Plan 0.2): float support does not exist in the
                // tamer opcode set — record a compile error instead of the
                // old silent push-0 + trap (wrong value at install time).
                self.record_unsupported("float literals", &self.current_fn.clone());
                self.asm.emit_push_i64(0);
                self.asm.emit_trap();
            }

            // ── Identifiers (variable access) ──────────────────────────
            Expr::Identifier(name) => {
                match self.local_slots.get(name.as_str()) {
                    Some(slot) => {
                        self.asm.emit_load_local(*slot);
                    }
                    None => {
                        // 2026-08-26 (parity plan §1.3): a top-level
                        // const reference inlines its compile-time
                        // value — the corpus (tamer host dispatch)
                        // compares against HOST_ID_* consts, and an
                        // integer stack machine has no global load;
                        // PUSH_I64 of the resolved value IS the
                        // semantically-correct lowering for an
                        // immutable Int const.
                        if let Some(val) = self.const_values.get(name.as_str()) {
                            self.asm.emit_push_i64(*val);
                        } else {
                            // 2026-08-23 (Plan 1.4): unresolvable
                            // identifier (state field, global, or
                            // strategy tag) records a compile error —
                            // the old silent push-0+trap shipped wrong
                            // values to install time.
                            self.record_unsupported(
                                &format!("reference '{}' (not a local/param)", name),
                                &self.current_fn.clone(),
                            );
                            self.asm.emit_push_i64(0);
                            self.asm.emit_trap();
                        }
                    }
                }
            }

            // ── Binary operations ──────────────────────────────────────
            Expr::BinaryOp(kind, lhs, rhs) => {
                // 2026-07-25: Ptr<Int> + Int — scale Int by 8 before adding.
                // In Briev, Ptr arithmetic is scaled by element size.
                // The VM's add is unscaled, so we multiply the offset by 8.
                if matches!(kind, BinaryOpKind::Add) {
                    if let Expr::Identifier(name) = lhs.as_ref() {
                        if let Some(&slot) = self.local_slots.get(name.as_str()) {
                            if self.ptr_slots.contains(&slot) {
                                // Ptr<Int> + Int: emit ptr, emit offset, mul 8, add
                                self.emit_expr(lhs);
                                self.emit_expr(rhs);
                                self.asm.emit_push_i8(3); // shift left 3 = multiply by 8
                                self.asm.emit_shl();
                                self.asm.emit_add();
                                return;
                            }
                        }
                    }
                }
                self.emit_expr(lhs);
                self.emit_expr(rhs);
                match kind {
                    BinaryOpKind::Add => self.asm.emit_add(),
                    BinaryOpKind::Sub => self.asm.emit_sub(),
                    BinaryOpKind::Mul => self.asm.emit_mul(),
                    BinaryOpKind::Div => self.asm.emit_div_s(),
                    BinaryOpKind::Mod => self.asm.emit_rem_s(),
                    BinaryOpKind::Eq => self.asm.emit_eq(),
                    BinaryOpKind::Neq => {
                        self.asm.emit_eq();
                        self.asm.emit_push_i8(1);
                        self.asm.emit_xor();
                    }
                    BinaryOpKind::Lt => self.asm.emit_lt_s(),
                    BinaryOpKind::Gt => self.asm.emit_gt_s(),
                    BinaryOpKind::Le => self.asm.emit_le_s(),
                    BinaryOpKind::Ge => self.asm.emit_ge_s(),
                    BinaryOpKind::And | BinaryOpKind::BitAnd => self.asm.emit_and(),
                    BinaryOpKind::Or | BinaryOpKind::BitOr => self.asm.emit_or(),
                    BinaryOpKind::BitXor => self.asm.emit_xor(),
                    BinaryOpKind::Shl => self.asm.emit_shl(),
                    BinaryOpKind::Shr => self.asm.emit_shr_s(),
                    BinaryOpKind::Concat => {
                        // 2026-08-23 (Plan 1.4): string concat needs the
                        // string opcode family — compile error, not a
                        // silent wrong-value trap.
                        self.record_unsupported(
                            "string concatenation (Concat)",
                            &self.current_fn.clone(),
                        );
                        self.asm.emit_trap();
                    }
                }
            }

            Expr::UnaryOp(kind, inner) => {
                self.emit_expr(inner);
                match kind {
                    UnaryOpKind::Neg => {
                        self.asm.emit_push_i64(0);
                        self.asm.emit_swap();
                        self.asm.emit_sub();
                    }
                    UnaryOpKind::Not => {
                        // 2026-07-26: Logical NOT — emit NOT which is now
                        // a logical NOT (0→1, else→0) in both C and Briev VM.
                        self.asm.emit_not();
                    }
                    UnaryOpKind::BitNot => {
                        // 2026-07-26: Bitwise NOT — emit BNOT for ~a.
                        self.asm.emit_bnot();
                    }
                }
            }

            // ── Control flow expressions ───────────────────────────────
            Expr::If(cond, then_branch, else_branch) => {
                self.emit_expr(cond);
                let else_label = self.fresh_label("if_else");
                let end_label = self.fresh_label("if_end");

                self.asm.emit_jz(&else_label);
                self.emit_expr(then_branch);
                self.asm.emit_jmp(&end_label);

                self.asm.define_label(&else_label);
                if let Some(els) = else_branch {
                    self.emit_expr(els);
                } else {
                    self.asm.emit_push_i64(0);
                }

                self.asm.define_label(&end_label);
            }

            Expr::Match(scrutinee, arms) => {
                self.emit_expr(scrutinee);
                let end_label = self.fresh_label("match_end");
                for arm in arms {
                    let next_label = self.fresh_label("match_next");
                    // Emit pattern matching
                    match &arm.pattern {
                        Pattern::Wildcard => {
                            // Always matches — emit body
                        }
                        Pattern::Literal(lit_expr) => {
                            // Extract the literal value from the Expr wrapper
                            if let Expr::Decimal(val) = lit_expr {
                                self.asm.emit_dup();
                                if *val >= i16::MIN as i64 && *val <= i16::MAX as i64 {
                                    self.asm.emit_push_i16(*val as i16);
                                } else if *val >= i32::MIN as i64 && *val <= i32::MAX as i64 {
                                    self.asm.emit_push_i32(*val as i32);
                                } else {
                                    self.asm.emit_push_i64(*val);
                                }
                                self.asm.emit_eq();
                                self.asm.emit_jz(&next_label);
                            } else {
                                // Non-integer literal pattern — fall through for MVP
                            }
                        }
                        _ => {
                            // Complex patterns: fall through to body for MVP
                        }
                    }
                    self.emit_expr(&arm.body);
                    self.asm.emit_jmp(&end_label);
                    self.asm.define_label(&next_label);
                }
                // Drop scrutinee (no arm matched — should not happen with wildcard)
                self.asm.emit_drop();
                self.asm.emit_push_i64(0);
                self.asm.define_label(&end_label);
            }

            // ── Function calls ─────────────────────────────────────────
            Expr::Call(name, args, _analysis_id) => {
                // Check if this is a host function (frgn call)
                // 2026-07-25: Check intrinsic (#) BEFORE host_fn_ids, so that
                // Alloc#/SysCall# etc. always take the PascalCase-skip path
                // even after being registered in host_fn_ids by a prior call.
                if name.ends_with('#') {
                    // 2026-07-25: Intrinsic calls (Alloc#, SysCall#, ShellCmd#, etc.)
                    // are handled as host calls.
                    // 2026-08-23 (Plan 1 HCALL slice): ids are CANONICAL —
                    // canonical_host_id assigns a stable number per service
                    // so the tamer dispatches by id without parsing names
                    // from the string table. Unknown services take ids above
                    // HOST_ID_CANONICAL_MAX, still table-recorded with their
                    // name (the tamer rejects them by id with a diagnostic).
                    if !self.host_fn_ids.contains_key(name.as_str()) {
                        let id = match canonical_host_id(name) {
                            Some(cid) => cid,
                            None => {
                                crate::backend::vm::HOST_ID_CANONICAL_MAX
                                    + self.host_fn_ids.len() as u32
                            }
                        };
                        self.asm
                            .register_host_fn_with_arity(name, id, args.len() as u32);
                        self.host_fn_ids.insert(name.clone(), id);
                    }
                    let host_id = self.host_fn_ids[name.as_str()];
                    // 2026-08-23 (Plan 1.1 bugfix): emit EVERY argument
                    // right-to-left. The old code skipped ALL Identifier
                    // args claiming they were "PascalCase strategy tags" —
                    // it actually dropped real variable arguments, so e.g.
                    // Alloc#(buf) called the host with no buffer. An
                    // unresolvable identifier is diagnosed by the
                    // Identifier arm itself (record_unsupported), never by
                    // shape-guessing here.
                    for arg in args.iter().rev() {
                        self.emit_expr(arg);
                    }
                    self.asm.emit_hcall(host_id);
                } else if let Some(&host_id) = self.host_fn_ids.get(name.as_str()) {
                    // Emit arguments right-to-left
                    for arg in args.iter().rev() {
                        self.emit_expr(arg);
                    }
                    self.asm.emit_hcall(host_id);
                } else {
                    // Regular function call — emit args, then call
                    for arg in args.iter().rev() {
                        self.emit_expr(arg);
                    }
                    // Look up function index by name
                    match self.fn_indices.get(name.as_str()) {
                        Some(idx) => self.asm.emit_call(*idx),
                        None => {
                            // 2026-08-23 (Plan 1.4): unknown function is a
                            // compile error, not a silent trap.
                            self.record_unsupported(
                                &format!("call to unknown function '{}'", name),
                                &self.current_fn.clone(),
                            );
                            self.asm.emit_trap();
                        }
                    }
                }
            }

            // ── Block expression ───────────────────────────────────────
            Expr::Block(stmts) => {
                for stmt in stmts {
                    self.emit_stmt(stmt);
                }
            }

            // ── Array index ───────────────────────────────────────────
            Expr::Index(obj, idx) => {
                // data[sl] → push base, push idx, mul 8, add, LOAD
                self.emit_expr(obj);
                self.emit_expr(idx);
                self.asm.emit_push_i8(3); // scale by 8 (Int size)
                self.asm.emit_shl();
                self.asm.emit_add();
                self.asm.emit_load();
            }

            // ── Field access ───────────────────────────────────────────
            Expr::Field(obj, field_name) => {
                // 2026-07-30: Determine the struct type name to compute
                // the correct field byte offset. If the object is an
                // identifier, look it up in local_types.
                let struct_name = match obj.as_ref() {
                    Expr::Identifier(name) => self.local_types.get(name).map(|s| s.as_str()),
                    _ => None,
                };
                let offset = self.field_offset(struct_name, field_name) as i64;
                self.emit_expr(obj);
                self.asm.emit_push_i64(offset);
                self.asm.emit_add();
            }

            // ── Cast ───────────────────────────────────────────────────
            Expr::Cast(inner, _target_type) => {
                // For MVP: no-op (cast is a type system annotation).
                // The VM is untyped, so casts are identity.
                self.emit_expr(inner);
            }

            // ── Pointer dereference ──────────────────────────────────
            Expr::Deref(inner) => {
                // Emit the pointer expression, then LOAD from that address.
                // *(bc + index) → emit ptr, load
                self.emit_expr(inner);
                self.asm.emit_load();
            }

            // ── Address-of ────────────────────────────────────────────
            Expr::AddrOf(inner) => {
                // For MVP: no-op. AddrOf in the VM is just the value itself
                // (the VM is untyped, all values are Int handles).
                self.emit_expr(inner);
            }

            // ── Remaining expressions ───────────────────────────────────
            other => {
                // 2026-08-23 (Plan 0.2): unsupported expressions record a
                // compile error alongside the trap — never a silent drop.
                let kind = format!("{:?}", other)
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or("this")
                    .to_string();
                self.record_unsupported(&format!("{} expressions", kind), &self.current_fn.clone());
                self.asm.emit_push_i64(0);
                self.asm.emit_trap();
            }
        }
    }

    pub(crate) fn fresh_label(&mut self, prefix: &str) -> String {
        let id = self.label_counter;
        self.label_counter += 1;
        format!("{}_{}", prefix, id)
    }
}
