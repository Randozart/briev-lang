//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::{declares_collection_ops, infer_type_only, operator_member, TypecheckContext};
use crate::ast::{Expr, Type};
use crate::errors::TypeError;

/// 2026-11-03: indexed reads do not imply writable storage. Require an
/// explicit collection writer so unsupported assignments cannot disappear.
/// Retire this gate when a shared place checker validates every assignment.
pub(super) fn validate(lhs: &Expr, rhs: &Expr, ctx: &mut TypecheckContext<'_>) -> Result<(), TypeError> {
    let Expr::Index(receiver, index) = lhs else { return Ok(()); };
    let ty = infer_type_only(receiver, ctx)?;
    if matches!(ty, Type::PtrConst(_)) {
        return Err(TypeError::InvalidOperation {
            operation: "indexed write through a const pointer; use a mutable pointer".into(),
            type_name: ty.to_string(),
        });
    }
    if matches!(ty, Type::Ptr(_) | Type::Vector(_, _)) { return Ok(()); }
    let key = ty.universe_key().unwrap_or_default();
    let members = ctx.type_members.get(key).map(Vec::as_slice).unwrap_or_default();
    if operator_member(members, "SetAt").is_some() {
        let call = Expr::MethodCall(receiver.clone(), "SetAt".into(),
            vec![(**index).clone(), rhs.clone()], None);
        infer_type_only(&call, ctx)?;
        return Ok(());
    }
    // 2026-11-04: only a declared collection requires a collection writer.
    // Pin wiring and shaped storage use the ordinary indexed-place rules
    // below this check. Retire this split with a shared place classifier.
    if !ctx.coll_types.contains(key) && !declares_collection_ops(ctx, key)
        && operator_member(members, "At").is_none()
        && has_storage_index(receiver, &ty, ctx)? {
        return Ok(());
    }
    Err(TypeError::InvalidOperation {
        operation: "indexed assignment requires writable storage; declare op SetAt(index, value) on the collection or use a mutable pointer or array".into(),
        type_name: ty.to_string(),
    })
}

/// 2026-11-04: keep the existing indexed-place domains distinct from member
/// dispatch. An undeclared applied constructor is checked parametrically
/// (see index inference); a concrete nominal record needs storage evidence.
/// Retire alongside the generic index-inference fallback.
fn has_storage_index(receiver: &Expr, ty: &Type, ctx: &mut TypecheckContext<'_>) -> Result<bool, TypeError> {
    if ctx.universe.matrix_shape(ty).is_some() { return Ok(true); }
    if has_unresolved_element(ty, ctx) { return Ok(true); }
    let Expr::Field(owner, field) = receiver else { return Ok(false); };
    let owner_ty = infer_type_only(owner, ctx)?;
    let Some(base) = owner_ty.universe_key() else { return Ok(false); };
    let prefix = format!("{field}[");
    let pins = ctx.type_pins.get(base).into_iter().chain(
        ctx.component_pins.get(base).filter(|_| ctx.surface_electronics));
    Ok(pins.flatten().any(|pin| pin.name.starts_with(&prefix)))
}

fn has_unresolved_element(ty: &Type, ctx: &TypecheckContext<'_>) -> bool {
    let Type::Applied(base, args) = ty else { return false; };
    let Some(element) = args.first().and_then(Type::universe_key) else { return false; };
    let parameters = ctx.current_fn_name.as_ref().and_then(|owner| ctx.fn_type_params.get(owner));
    ctx.universe.get(base).is_none() && !ctx.type_slots.contains_key(base)
        && parameters.is_some_and(|names| names.iter().any(|name| name == element))
}

#[cfg(test)]
mod tests {
    #[test]
    fn data_with_shape_metadata_does_not_require_a_collection_writer() {
        let source = r#"
            type Grid<T, R, C> { spec Rows: R; spec Cols: C; };
            let values: Grid<Float, 4, 4>;
            let position: Int = 0;
            async node scale [position < 16][position == 16] {
                values[position] = values[position] * 0.5;
                position = position + 1;
                term;
            };
        "#;
        crate::library::parse_and_check("scratch/shaped_store.abv", source)
            .expect("shape-backed indexed storage has no collection operator");
    }

    #[test]
    fn parametric_storage_uses_the_enclosing_element_parameter() {
        let source = "txn copy_one<T>(input: Vector<T,N>, output: Vector<T,N>, position: Int) [position < N][position == N] -> Vector<T,N> { output[position] = input[position]; position = position + 1; term output; };";
        crate::library::parse_and_check("scratch/parametric_storage.bv", source)
            .expect("indexed storage with an in-scope generic element");
    }

    #[test]
    fn const_pointer_index_is_read_only() {
        use super::*;
        let universe = crate::type_universe::TypeUniverse::new();
        let mut ctx = TypecheckContext::new(&universe);
        ctx.bindings.insert("source".into(), Type::PtrConst(Box::new(Type::int())));
        let index = Expr::Index(Box::new(Expr::Identifier("source".into())), Box::new(Expr::Decimal(0)));
        let error = validate(&index, &Expr::Decimal(9), &mut ctx).unwrap_err();
        assert!(error.to_string().contains("const pointer"), "{error}");
    }

    #[test]
    fn a_plain_scalar_or_record_has_no_writable_index() {
        for receiver in ["Int", "Plain", "Boxed<Int>", "Opaque<Undeclared, N>"] {
            let source = format!("obj Plain {{ value: Int; }}; obj Boxed<T> {{ value: T; }}; defn write(target: {receiver}) -> Int {{ target[0] = 9; term 0; }};");
            let error = crate::library::parse_and_check("scratch/no_storage.bv", &source)
                .err().expect("ordinary values do not acquire indexed storage");
            assert!(error.to_string().contains("indexed assignment"), "{error}");
        }
    }

    #[test]
    fn indexed_write_checks_writer_arguments() {
        for parameters in ["index: Int", "index: Int, value: Bool"] {
            let source = format!(r#"
                obj Writable {{
                    op At(index: Int) -> Int {{ term 0; }};
                    op SetAt({parameters}) -> Void {{ term; }};
                }};
                defn replace(value: Writable) -> Int {{ value[0] = 9; term 0; }};
            "#);
            let result = crate::library::parse_and_check("scratch/writer_signature.bv", &source);
            let error = result.err().expect("the write operator must accept both arguments");
            assert!(error.to_string().contains("SetAt"), "{error}");
        }
    }

    #[test]
    fn indexed_write_requires_declared_write_capability() {
        let source = r#"
            obj ReadOnly {
                op Count() -> Int { term 1; };
                op At(index: Int) -> Int { term 0; };
            };
            defn replace(value: ReadOnly) -> Int { value[0] = 9; term 0; };
        "#;
        let result = crate::library::parse_and_check("scratch/read_only.bv", source);
        let error = result.err().expect("read capability alone must not permit a write");
        assert!(error.to_string().contains("SetAt"), "{error}");
    }
}
