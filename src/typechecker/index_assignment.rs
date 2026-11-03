//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use super::{infer_type_only, operator_member, TypecheckContext};
use crate::ast::{Expr, Type};
use crate::errors::TypeError;

/// 2026-11-03: indexed reads do not imply writable storage. Require an
/// explicit collection writer so unsupported assignments cannot disappear.
/// Retire this gate when a shared place checker validates every assignment.
pub(super) fn validate(lhs: &Expr, rhs: &Expr, ctx: &mut TypecheckContext<'_>) -> Result<(), TypeError> {
    let Expr::Index(receiver, index) = lhs else { return Ok(()); };
    let ty = infer_type_only(receiver, ctx)?;
    if matches!(ty, Type::Ptr(_) | Type::Vector(_, _)) { return Ok(()); }
    let key = match &ty {
        Type::Custom(name) | Type::Applied(name, _) => Some(name),
        _ => None,
    };
    let writable = key.and_then(|name| ctx.type_members.get(name))
        .and_then(|members| operator_member(members, "SetAt")).is_some();
    if writable {
        let call = Expr::MethodCall(receiver.clone(), "SetAt".into(),
            vec![(**index).clone(), rhs.clone()], None, vec![]);
        infer_type_only(&call, ctx)?;
        return Ok(());
    }
    Err(TypeError::InvalidOperation {
        operation: "indexed assignment requires writable storage; declare op SetAt(index, value) on the collection or use a mutable pointer or array".into(),
        type_name: ty.to_string(),
    })
}

#[cfg(test)]
mod tests {
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
