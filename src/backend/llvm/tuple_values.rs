//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Tuple values retain semantic element types independently of handle width.
use super::{LlvmBackend, TypedRegister};
use crate::ast::Type;
use std::fmt::Write;

impl LlvmBackend {
    /// 2026-08-17: tuples store `[len, element0, ...]` in fixed i64 words.
    /// 2026-10-19: Float slots invert adapt_to_i64's IEEE bit boxing, rather
    /// than treating the loaded integer as a floating register.
    /// 2026-10-05: String/Data pointer elements must regain pointer registers
    /// before equality, slicing or length operations consume them.
    /// 2026-11-05: share decoding between destructuring and numeric projection;
    /// both previously decoded only some representations. Handles still follow
    /// target width, including nested tuples on wasm32. Retire this helper only
    /// with a shared typed aggregate-slot consumer preserving those contracts.
    pub(crate) fn emit_tuple_element(
        &mut self,
        out: &mut String,
        indent: &str,
        tuple: &TypedRegister,
        element: (usize, &Type),
    ) -> TypedRegister {
        let (index, element_type) = element;
        let base = self.fun.gen_reg();
        let slot = self.fun.gen_reg();
        let word = self.fun.gen_reg();
        let width = self.ctx.int_bits;
        writeln!(out, "{indent}{base} = inttoptr i{width} {} to ptr", tuple.name).ok();
        writeln!(out, "{indent}{slot} = getelementptr i64, ptr {base}, i64 {}", index + 1).ok();
        writeln!(out, "{indent}{word} = load i64, ptr {slot}").ok();
        let name = self.decode_tuple_word(out, indent, &word, element_type);
        if self.is_protocol_member(element_type, "Float") {
            self.fun.reg_float_cache.insert(word, name.clone());
            self.fun.reg_float_cache.insert(name.clone(), name.clone());
        }
        TypedRegister { name, ty: element_type.clone() }
    }

    /// 2026-11-05: raw Ptr values use integer handles internally, unlike
    /// String/Data pointer registers. Preserve that ABI when unpacking slots;
    /// retire this distinction with a unified value-register representation.
    fn decode_tuple_word(
        &mut self, out: &mut String, indent: &str, word: &str, ty: &Type,
    ) -> String {
        if !matches!(ty, Type::Ptr(_) | Type::PtrConst(_)) {
            return self.adapt_from_i64(out, indent, word, ty);
        }
        if self.ctx.int_bits == 64 { return word.to_string(); }
        let handle = self.fun.gen_reg();
        writeln!(out, "{indent}{handle} = trunc i64 {word} to i{}", self.ctx.int_bits).ok();
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_hoisted_products_keep_semantic_types() {
        for hoisted in [false, true] {
            let mut backend = LlvmBackend::new();
            backend.fun.defer_struct_allocas = hoisted;
            let mut output = String::new();
            let empty = backend.emit_expr(&mut output, &crate::ast::Expr::Tuple(vec![]), "  ");
            assert_eq!(empty.ty, Type::Tuple(vec![]));
            let pair = backend.emit_expr(&mut output, &crate::ast::Expr::Tuple(vec![
                crate::ast::Expr::Decimal(17), crate::ast::Expr::Bool(true),
            ]), "  ");
            assert_eq!(pair.ty, Type::Tuple(vec![Type::int(), Type::bool_()]));
        }
    }
}
