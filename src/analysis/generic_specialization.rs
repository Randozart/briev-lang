//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Shared generic identity and binding, used by analysis and code generation.
//! 2026-11-04: keep ownership facts attached to the same concrete callable
//! that lowering emits. Retire only with a unified specialization IR.
use crate::ast::Type;

/// 2026-10-13 (static trait-constrained generics, SPEC §8.6, S6): bind a
/// generic defn's type params from a (declared param type, concrete arg type)
/// pair. A declared type that IS a type param binds to the concrete arg;
/// structured types recurse (`List<T>` vs `List<Int>` binds `T=Int`). Only
/// the type params in `params` are bound; everything else is left alone.
pub(crate) fn unify_generic_binding(
    declared: &Type,
    concrete: &Type,
    params: &[String],
    subst: &mut std::collections::HashMap<String, Type>,
) {
    if let Type::Custom(declared_name) = declared {
        if params.iter().any(|p| p == declared_name) {
            subst
                .entry(declared_name.clone())
                .or_insert_with(|| concrete.clone());
            return;
        }
    }
    match (declared, concrete) {
        (Type::Applied(_, dps), Type::Applied(_, cps)) if dps.len() == cps.len() => {
            for (dp, cp) in dps.iter().zip(cps.iter()) {
                unify_generic_binding(dp, cp, params, subst);
            }
        }
        (Type::Ptr(dp), Type::Ptr(cp)) | (Type::PtrConst(dp), Type::PtrConst(cp)) => {
            unify_generic_binding(dp, cp, params, subst);
        }
        _ => {}
    }
}

/// 2026-10-13 (static trait-constrained generics, SPEC §8.6, S6): the mangled
/// LLVM symbol for a specialization — `bump_twice$Tally` for one concrete arg,
/// multiple joined by `$`. The `$` is a legal LLVM identifier character, so no
/// quoting is needed; a nominal type name is already identifier-safe.
///
/// 2026-10-14 (fix): COMPOUND concrete args (`List<String>`, nested `Result`)
/// are encoded recursively — the old Display fallback produced
/// `List<String>`-shaped suffixes (`<`, `>`, `,` are invalid in an LLVM
/// identifier), which is why the Custom-only guard below rejected every
/// compound instantiation and its call sites fell back to the plain
/// (never-emitted) name. To undo: revert to the Display fallback + guard.
pub(crate) fn mangle_specialization(name: &str, concrete: &[Type]) -> String {
    let args = concrete
        .iter()
        .map(mangle_type_suffix)
        .collect::<Vec<_>>()
        .join("$");
    format!("{}${}", name, args)
}

/// Identifier-safe encoding of one concrete type for `mangle_specialization`:
/// nominal names pass through, applied types flatten recursively
/// (`List<String>` → `List$String`), everything else keeps only its
/// alphanumeric runs (deterministic, no ambiguity in practice — the shapes
/// that reach here are nominal or applied).
fn mangle_type_suffix(ty: &Type) -> String {
    match ty {
        Type::Custom(n) => n.clone(),
        Type::Applied(n, args) | Type::Generic(n, args) => {
            if args.is_empty() {
                n.clone()
            } else {
                format!(
                    "{}${}",
                    n,
                    args.iter()
                        .map(mangle_type_suffix)
                        .collect::<Vec<_>>()
                        .join("$")
                )
            }
        }
        other => {
            let disp = format!("{other}");
            disp.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                .collect()
        }
    }
}
