// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0b typed match-pattern skeleton. H1 replaces literal/range expressions
//! with fully typed projections.

use super::expr::HirExpr;
use super::ids::NodeId;
use crate::ast::Type;

#[derive(Debug, Clone, PartialEq)]
pub struct HirPattern {
    pub id: NodeId,
    pub kind: HirPatternKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HirPatternKind {
    Wildcard,
    Literal(HirExpr),
    Binding(String),
    TypedBinding(String, Type),
    EnumVariant(String, Vec<HirPattern>),
    Tuple(Vec<HirPattern>),
    Range(HirExpr, HirExpr),
    RangeInclusive(HirExpr, HirExpr),
    Multi(Vec<HirPattern>),
}
