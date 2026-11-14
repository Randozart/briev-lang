//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::emit_expr::ArgsView;
use super::{LlvmBackend, TypedRegister};
use crate::ast::Expr;
use std::fmt::Write;

pub(super) struct IndexStore<'a> {
    pub receiver: &'a TypedRegister,
    pub index: &'a Expr,
    pub value: &'a TypedRegister,
}

/// 2026-11-03: preserve indexed collection writes through an explicit SetAt
/// capability. Bind evaluated operands once so nested receivers and RHS calls
/// keep their effects. Remove when all lvalues share capability dispatch.
pub(super) fn emit_collection_index_store(
    backend: &mut LlvmBackend,
    out: &mut String,
    indent: &str,
    operands: IndexStore<'_>,
) -> bool {
    let IndexStore { receiver, index, value } = operands;
    let Some(key) = backend.resolve_obj_key(&receiver.ty) else { return false; };
    let supported = backend.ctx.obj_members.get(&key).is_some_and(|members| {
        members.iter().any(|member| matches!(member,
            crate::ast::TopLevel::TypeDefOperator(definition) if definition.name == "SetAt"))
    });
    if !supported { return false; }
    let receiver_name = format!("__index_receiver_{}", backend.fun.gen_reg());
    let value_name = format!("__index_value_{}", backend.fun.gen_reg());
    for (name, register) in [(&receiver_name, receiver), (&value_name, value)] {
        backend.fun.let_bindings.insert(name.clone(), register.name.clone());
        backend.fun.let_binding_types.insert(name.clone(), register.ty.clone());
    }
    let result = backend.fun.gen_reg();
    backend.emit_method_call(out, &result, &Expr::Identifier(receiver_name.clone()),
        "SetAt", ArgsView::Ast(&[index.clone(), Expr::Identifier(value_name.clone())]), indent);
    for name in [&receiver_name, &value_name] {
        backend.fun.let_bindings.remove(name);
        backend.fun.let_binding_types.remove(name);
    }
    true
}

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

    #[test]
    fn narrow_pointer_storage_preserves_address_bits() {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
        let mut backend = LlvmBackend::new();
        backend.ctx.int_bits = 32;
        let mut body = String::new();
        let stored = backend.adapt_to_i64(&mut body, "  ", &TypedRegister {
            name: "%value".into(), ty: Type::Ptr(Box::new(Type::int())),
        });
        let ir = format!("define i64 @box(i32 %value) {{\nentry:\n{body}  ret i64 {stored}\n}}\n\
            define i32 @main() {{\nentry:\n  %got = call i64 @box(i32 3735928559)\n\
            %wrong = icmp ne i64 %got, 3735928559\n  %status = zext i1 %wrong to i32\n\
            ret i32 %status\n}}\n");
        super::super::logical_tests::clang_ir(&ir, "-O2", true);
    }

    #[test]
    fn narrow_float_storage_preserves_its_bits() {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
        let (items, universe) = crate::library::parse_and_check(
            "scratch/float_storage.bv", "let value: Float = 0.5;",
        ).expect("registered numeric protocols");
        let mut backend = LlvmBackend::new().with_type_universe(universe);
        backend.generate(&items, None);
        let mut body = String::new();
        let stored = backend.ensure_typed_value(&mut body, "  ", "i64", "%value",
            Some(Type::float()), None);
        let ir = format!("define i64 @box(float %value) {{\nentry:\n{body}  ret i64 {stored}\n}}\n\
            define i32 @main() {{\nentry:\n  %got = call i64 @box(float 5.000000e-01)\n\
            %wrong = icmp ne i64 %got, 1056964608\n  %status = zext i1 %wrong to i32\n\
            ret i32 %status\n}}\n");
        super::super::logical_tests::clang_ir(&ir, "-O2", true);
    }

    #[test]
    fn collection_field_index_writes_reach_storage() {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
        let source = r#"
            coll struct Samples<T> { data: T[2]; };
            obj Buckets { values: Samples<Int>; };
            defn update(buckets: Buckets) -> Int {
                buckets.values[1] = 7;
                term 13;
            };
            let done: Bool = false;
            node main [!done][done] {
                let initial: Samples<Int> = [0,0];
                let buckets: Buckets = Buckets { values: initial };
                when update(buckets) != 13 { SysCall#(60,3); };
                when buckets.values[1] != 7 { SysCall#(60,1); };
                let fractions: Samples<Float> = [0.5,1.5];
                fractions[1] = 2.5;
                when fractions[0] != 0.5 || fractions[1] != 2.5 { SysCall#(60,2); };
                done = true;
                endprogram;
            };
        "#;
        let (items, mut universe) = crate::library::parse_and_check("scratch/index_store.bv", source)
            .expect("checked indexed assignment");
        crate::backend::register_types::register_typedefs(&items, &mut universe, 64)
            .expect("registered collection fields");
        let ir = LlvmBackend::new().with_type_universe(universe).generate(&items, None);
        for optimization in ["-O0", "-O2"] {
            super::super::logical_tests::clang_ir(&ir, optimization, true);
        }
    }

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
