//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//

use super::{LlvmBackend, TypedRegister};
use crate::ast::{Expr, Statement, Type};
use std::fmt::Write;
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct LogicalInputs<'a> { left: &'a Expr, right: &'a Expr, exit: bool }

struct LogicalScope {
    pointers: HashMap<String, String>,
    fields: HashMap<usize, (String, String)>,
    values: HashMap<String, String>,
}

impl LlvmBackend {
    fn logical_scope(&self) -> LogicalScope {
        LogicalScope { pointers: self.fun.canonical_ptrs.clone(),
            fields: self.fun.state_ptr_cache.clone(), values: self.fun.last_val_temps.clone() }
    }

    fn merge_logical_scope(&mut self, before: LogicalScope) {
        // 2026-10-22: RHS-only registers do not dominate the skipped edge.
        // Keep only unchanged incoming caches; invalidations stay invalidated.
        self.fun.canonical_ptrs.retain(|key, value| before.pointers.get(key) == Some(value));
        self.fun.state_ptr_cache.retain(|key, value| before.fields.get(key) == Some(value));
        self.fun.last_val_temps.retain(|key, value| before.values.get(key) == Some(value));
        self.fun.last_val_types.retain(|key, _| self.fun.last_val_temps.contains_key(key));
    }

    pub(super) fn parameter_is_reassigned(body: &[Statement], name: &str) -> bool {
        let mut found = false;
        let mut inspect = |lhs: &Expr| {
            if matches!(lhs, Expr::Identifier(value) if value == name) { found = true; }
        };
        crate::ast::visit::walk_assign_targets(body, &mut inspect);
        crate::ast::visit::walk_stmts(body, &mut |expression| {
            if let Expr::Block(statements) = expression {
                crate::ast::visit::walk_assign_targets(statements, &mut inspect);
            }
        });
        found
    }

    // 2026-10-22: assignment-site demotion loses the incoming value when a
    // lazy branch is skipped. Bind mutable parameters at entry in their ABI
    // type, including pointers and vectors; no machine-word boxing is needed.
    pub(super) fn bind_mutable_parameter(&mut self, out: &mut String, name: &str, ty: &Type, raw: &str) {
        let slot = self.fun.gen_reg();
        let physical = self.llvm_type(ty);
        writeln!(out, "  {} = alloca {}", slot, physical).ok();
        writeln!(out, "  store {} {}, ptr {}", physical, raw, slot).ok();
        self.fun.let_bindings.insert(name.to_string(), slot.clone());
        self.fun.let_binding_types.insert(name.to_string(), ty.clone());
        self.fun.let_original_types.insert(name.to_string(), ty.clone());
        self.fun.let_binding_allocas.insert(slot.clone());
        self.fun.native_parameter_slots.insert(slot);
    }

    // 2026-10-22: logical operators license evaluation of the RHS only on one
    // edge. Register-level and/or cannot implement that rule for calls, traps
    // or guarded reads. Dedicated predecessors keep nested control-flow phis
    // valid without stack slots. Retire only into an equivalent shared CFG IR.
    pub(super) fn emit_short_circuit(
        &mut self,
        out: &mut String,
        operands: (&Expr, &Expr),
        short_value: bool,
        indent: &str,
    ) -> TypedRegister {
        self.emit_logical_control(out, LogicalInputs { left: operands.0, right: operands.1, exit: false }, short_value, indent)
    }

    pub(super) fn emit_exit_short_circuit(
        &mut self, out: &mut String, operands: (&Expr, &Expr), short_value: bool, indent: &str,
    ) -> TypedRegister {
        self.emit_logical_control(out, LogicalInputs { left: operands.0, right: operands.1, exit: true }, short_value, indent)
    }

    fn emit_logical_control(
        &mut self, out: &mut String, inputs: LogicalInputs<'_>, short_value: bool, indent: &str,
    ) -> TypedRegister {
        let left = self.logical_operand(out, inputs.left, inputs.exit, indent);
        if self.fun.terminated {
            return TypedRegister { name: "0".into(), ty: Type::bool_() };
        }
        let condition = self.logical_truth(out, &left, indent);
        let identity = self.fun.gen_reg();
        let identity = identity.trim_start_matches('%');
        let short = format!("logical.short.{identity}");
        let rhs = format!("logical.rhs.{identity}");
        let rhs_done = format!("logical.value.{identity}");
        let merge = format!("logical.end.{identity}");
        let (on_true, on_false) = if short_value { (&short, &rhs) } else { (&rhs, &short) };
        writeln!(out, "{indent}br i1 {condition}, label %{on_true}, label %{on_false}").ok();
        writeln!(out, "{short}:\n{indent}br label %{merge}\n{rhs}:").ok();
        self.fun.cur_block = Some(rhs);
        let scope = self.logical_scope();
        let value = self.logical_rhs(out, inputs, indent);
        self.merge_logical_scope(scope);
        if value.is_some() {
            writeln!(out, "{indent}br label %{rhs_done}\n{rhs_done}:\n{indent}br label %{merge}").ok();
        }
        writeln!(out, "{merge}:").ok();
        let result = self.fun.gen_reg();
        let constant = u8::from(short_value);
        let incoming = value.map_or_else(String::new, |value| format!(", [ {value}, %{rhs_done} ]"));
        writeln!(out, "{indent}{result} = phi i8 [ {constant}, %{short} ]{incoming}").ok();
        self.fun.cur_block = Some(merge);
        self.fun.terminated = false;
        TypedRegister { name: result, ty: Type::bool_() }
    }

    fn logical_rhs(&mut self, out: &mut String, inputs: LogicalInputs<'_>, indent: &str) -> Option<String> {
        let value = self.logical_operand(out, inputs.right, inputs.exit, indent);
        if self.fun.terminated { return None; }
        let truth = self.logical_truth(out, &value, indent);
        let result = self.fun.gen_reg();
        writeln!(out, "{indent}{result} = zext i1 {truth} to i8").ok();
        Some(result)
    }

    fn logical_operand(&mut self, out: &mut String, expression: &Expr, exit: bool, indent: &str) -> TypedRegister {
        if !exit { return self.emit_expr(out, expression, indent); }
        // Exit fields intentionally read committed state rather than an earlier
        // SSA snapshot. Exit arithmetic/comparisons return i64; ordinary call
        // and literal fallbacks retain their actual type until Bool coercion.
        match expression {
            Expr::Identifier(name) if self.ctx.field_index_map.contains_key(name) =>
                TypedRegister { name: self.emit_exit_expr(out, expression, indent), ty: Type::bits(64) },
            Expr::BinaryOp(..) | Expr::UnaryOp(..) =>
                TypedRegister { name: self.emit_exit_expr(out, expression, indent), ty: Type::bits(64) },
            _ => self.emit_expr(out, expression, indent),
        }
    }

    fn logical_truth(&mut self, out: &mut String, value: &TypedRegister, indent: &str) -> String {
        let value = self.coerce_to_i8_bool(out, indent, value);
        let truth = self.fun.gen_reg();
        writeln!(out, "{indent}{truth} = icmp ne i8 {}, 0", value.name).ok();
        truth
    }
}
