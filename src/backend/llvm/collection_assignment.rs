//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::{LlvmBackend, TypedRegister};
use crate::ast::Expr;
use std::fmt::Write;

/// 2026-10-21: ordinary same-type collection assignment replaces the local
/// handle, as in the interpreter. The historical InsertAt fallback instead
/// inserted the replacement handle as an element, corrupting concat accumulators.
/// Explicit arrow insertion retains its own path. Remove this seam only when
/// all ordinary local stores share one lowering independent of collection ops.
pub(super) fn emit_collection_rebind(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    name: &str,
    value: &TypedRegister,
) -> bool {
    let Some(binding_type) = backend.fun.let_binding_types.get(name).cloned() else {
        return false;
    };
    if binding_type != value.ty {
        return false;
    }
    let Some(binding) = backend.fun.let_bindings.get(name).cloned() else {
        return false;
    };
    if backend.find_insert_strategy(&Expr::Identifier(name.to_owned())).is_none() {
        return false;
    }
    let llvm_type = backend.llvm_type(&binding_type);
    let allocated = backend.fun.let_binding_allocas.contains(&binding)
        || backend.fun.param_slots.values().any(|slot| slot == &binding);
    let slot = if allocated {
        binding
    } else {
        let slot = backend.fun.gen_reg();
        writeln!(out, "{indent}{slot} = alloca {llvm_type}, align 8").ok();
        backend.fun.let_bindings.insert(name.to_owned(), slot.clone());
        backend.fun.let_binding_allocas.insert(slot.clone());
        slot
    };
    let coerced = backend.coerce_register_value(out, indent, value, &binding_type);
    writeln!(out, "{indent}store {llvm_type} {}, ptr {slot}", coerced.name).ok();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Type;

    fn collection_backend(allocated: bool) -> LlvmBackend {
        let mut backend = LlvmBackend::new();
        let ty = Type::Custom("UserContainer".into());
        backend.fun.let_binding_types.insert("items".into(), ty.clone());
        backend.fun.let_original_types.insert("items".into(), ty);
        backend.fun.let_bindings.insert("items".into(), "%items".into());
        if allocated { backend.fun.let_binding_allocas.insert("%items".into()); }
        backend.ctx.operator_defs.insert("UserContainer".into(), vec![crate::ast::top::OperatorDef {
            op: "InsertAt".into(), params: vec![], pre: None, suf: None,
            impl_args: None, impl_name: "user_insert".into(), span: None, trusted_axiom: false,
        }]);
        backend
    }

    #[test]
    fn same_type_collection_assignment_replaces_handle() {
        for allocated in [true, false] {
            let mut backend = collection_backend(allocated);
            let mut output = String::new();
            let value = TypedRegister { name: "42".into(), ty: Type::Custom("UserContainer".into()) };
            assert!(emit_collection_rebind(&mut backend, &mut output, "", "items", &value));
            assert!(output.contains("store i64 42, ptr"), "{output}");
            assert!(!output.contains("user_insert"), "{output}");
        }
    }

    #[test]
    fn unknown_binding_preserves_assignment_fallback() {
        let mut backend = LlvmBackend::new();
        let mut output = String::new();
        let value = TypedRegister { name: "42".into(), ty: Type::int() };
        assert!(!emit_collection_rebind(&mut backend, &mut output, "", "absent", &value));
        assert!(output.is_empty());
    }

    #[test]
    fn different_rhs_type_preserves_insertion_fallback() {
        let mut backend = LlvmBackend::new();
        backend.fun.let_binding_types.insert("items".into(), Type::Custom("UserContainer".into()));
        backend.fun.let_bindings.insert("items".into(), "%items".into());
        let mut output = String::new();
        let value = TypedRegister { name: "42".into(), ty: Type::int() };
        assert!(!emit_collection_rebind(&mut backend, &mut output, "", "items", &value));
        assert!(output.is_empty());
    }
}
