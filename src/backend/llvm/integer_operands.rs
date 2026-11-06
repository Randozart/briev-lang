//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::{LlvmBackend, TypedRegister};
use std::fmt::Write;

impl LlvmBackend {
    pub(super) fn emit_normalized_binary_op(
        &mut self,
        out: &mut String,
        location: (&str, &str),
        kind: &crate::ast::BinaryOpKind,
        operands: (&TypedRegister, &TypedRegister),
    ) -> TypedRegister {
        let (result, indent) = location;
        let (left, right) = operands;
        if let Some((left, right)) = self.normalize_integer_pair(out, indent, left, right) {
            return self.emit_binary_op(out, result, kind, &left, &right, indent);
        }
        self.emit_binary_op(out, result, kind, left, right, indent)
    }

    /// 2026-11-06: scalar operands can share a source protocol while carrying
    /// different SSA widths (native enum payload versus boxed parameter).
    /// Normalize before either config or fallback dispatch. Remove only when
    /// all producers enforce one physical representation at binary boundaries.
    pub(super) fn normalize_integer_pair(
        &mut self,
        out: &mut String,
        indent: &str,
        left: &TypedRegister,
        right: &TypedRegister,
    ) -> Option<(TypedRegister, TypedRegister)> {
        let left_ll = self.llvm_type(&left.ty);
        let right_ll = self.llvm_type(&right.ty);
        let left_bits = left_ll.strip_prefix('i')?.parse::<u32>().ok()?;
        let right_bits = right_ll.strip_prefix('i')?.parse::<u32>().ok()?;
        if left_bits == right_bits { return None; }
        if left_bits < right_bits {
            return Some((self.extend_integer_operand(out, indent, left, right), right.clone()));
        }
        Some((left.clone(), self.extend_integer_operand(out, indent, right, left)))
    }

    fn extend_integer_operand(
        &mut self,
        out: &mut String,
        indent: &str,
        narrow: &TypedRegister,
        wide: &TypedRegister,
    ) -> TypedRegister {
        let source = self.llvm_type(&narrow.ty);
        let target = self.llvm_type(&wide.ty);
        let signed = self.is_protocol_member(&narrow.ty, "Int");
        let opcode = if signed { "sext" } else { "zext" };
        let name = self.fun.gen_reg();
        writeln!(out, "{indent}{name} = {opcode} {source} {} to {target}", narrow.name).ok();
        TypedRegister { name, ty: wide.ty.clone() }
    }
}
