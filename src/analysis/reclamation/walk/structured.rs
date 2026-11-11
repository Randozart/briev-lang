//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! 2026-11-05: retain declared payload types at destructuring sites so effects
//! resolve from the type universe. Unknown patterns retain the conservative
//! barrier. Replace with shared typed-AST facts when that interface exists.
use super::*;
use crate::ast::{EnumDefinition, EnumVariant, Type, TypeDef};

pub(super) fn register_variant(
    graph: &mut Graph,
    definition: &EnumDefinition,
    variant: &EnumVariant,
) {
    let (name, fields) = match variant {
        EnumVariant::Unit(name) => (name, vec![]),
        EnumVariant::Tuple(name, fields) => (name, fields.clone()),
        EnumVariant::Struct(name, fields) => {
            (name, fields.iter().map(|(_, ty)| ty.clone()).collect())
        }
    };
    graph.variant_types.insert(
        name.clone(),
        (
            definition
                .type_params
                .iter()
                .map(|param| param.name.clone())
                .collect(),
            fields,
        ),
    );
}

pub(super) fn register_slot(graph: &mut Graph, definition: &TypeDef, name: &str, ty: &Type) {
    let fields = match ty {
        Type::Tuple(fields) => fields.clone(),
        Type::Void => vec![],
        _ => vec![ty.clone()],
    };
    graph.variant_types.insert(
        name.into(),
        (
            definition
                .type_params
                .iter()
                .map(|param| param.name.clone())
                .collect(),
            fields,
        ),
    );
}

impl Builder<'_> {
    pub(super) fn pattern_types(&mut self, pattern: &Pattern, ty: Option<&Type>) {
        match pattern {
            Pattern::Binding(name) => {
                if let Some(ty) = ty {
                    self.types.insert(name.clone(), ty.clone());
                }
            }
            Pattern::TypedBinding(name, ty) => {
                self.types.insert(name.clone(), *ty.clone());
            }
            Pattern::EnumVariant(name, patterns) => self.variant_pattern_types(name, patterns, ty),
            Pattern::Tuple(patterns) => {
                if let Some(Type::Tuple(fields)) = ty {
                    for (pattern, field) in patterns.iter().zip(fields) {
                        self.pattern_types(pattern, Some(field));
                    }
                }
            }
            Pattern::Multi(patterns) => {
                for pattern in patterns {
                    self.pattern_types(pattern, ty);
                }
            }
            _ => {}
        }
    }

    fn variant_pattern_types(&mut self, name: &str, patterns: &[Pattern], ty: Option<&Type>) {
        let name = self
            .graph
            .constructor_key(name)
            .unwrap_or_else(|| name.into());
        let Some((params, fields)) = self.graph.variant_types.get(&name).cloned() else {
            return;
        };
        let args = match ty {
            Some(Type::Applied(_, args) | Type::Generic(_, args)) => args.as_slice(),
            _ => &[],
        };
        let substitution = params.into_iter().zip(args.iter().cloned()).collect();
        for (pattern, field) in patterns.iter().zip(fields) {
            self.pattern_types(
                pattern,
                Some(&crate::typechecker::substitute_type(&field, &substitution)),
            );
        }
    }
}
