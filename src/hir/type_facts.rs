// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H2c conservative enum/object facts keyed by HIR declaration identity.
//! Physical layout remains target-owned; these facts capture declared shape.

use super::program::{HirEnumVariant, HirProgram, HirTypeDefKind};
use crate::ast::Type;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct HirEnumFacts {
    pub variants: Vec<HirEnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HirObjectFacts {
    pub fields: Vec<(String, Type)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HirTypeFacts {
    pub enums: BTreeMap<String, HirEnumFacts>,
    pub objects: BTreeMap<String, HirObjectFacts>,
    pub structs: BTreeMap<String, HirObjectFacts>,
}

pub fn build_type_facts(program: &HirProgram) -> HirTypeFacts {
    let mut facts = HirTypeFacts::default();
    for def in &program.type_defs {
        match &def.kind {
            HirTypeDefKind::Enum { variants } => {
                facts.enums.insert(
                    def.name.clone(),
                    HirEnumFacts {
                        variants: variants.clone(),
                    },
                );
            }
            HirTypeDefKind::Object { fields } => {
                facts.objects.insert(
                    def.name.clone(),
                    HirObjectFacts {
                        fields: fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect(),
                    },
                );
            }
            HirTypeDefKind::Struct { fields } => {
                facts.structs.insert(
                    def.name.clone(),
                    HirObjectFacts {
                        fields: fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect(),
                    },
                );
            }
        }
    }
    facts
}
