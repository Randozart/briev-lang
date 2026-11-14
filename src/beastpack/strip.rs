// 2026-07-25: Strip Source$ and Comment$ metadata before beastpack serialization.
// Source$ contains original source text; Comment$ contains doc comments.
// Both must be removed for IP protection in distributed .beastpack files.

use crate::ast::top::*;
use crate::ast::*;
use std::collections::HashMap;

fn strip_from_map(map: &HashMap<String, PropertyValue>) -> HashMap<String, PropertyValue> {
    map.iter()
        .filter(|(k, _)| k.as_str() != "Source$" && k.as_str() != "Comment$")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn strip_definition(d: &Definition) -> Definition {
    Definition {
        metadata: strip_from_map(&d.metadata),
        ..d.clone()
    }
}

fn strip_transaction(t: &Transaction) -> Transaction {
    Transaction {
        metadata: strip_from_map(&t.metadata),
        ..t.clone()
    }
}

fn strip_typedef(td: &TypeDef) -> TypeDef {
    TypeDef {
        body: TypeDefBody {
            metadata: strip_from_map(&td.body.metadata),
            ..td.body.clone()
        },
        ..td.clone()
    }
}

pub fn strip_items(items: &[TopLevel]) -> Vec<TopLevel> {
    items
        .iter()
        .map(|item| match item {
            TopLevel::Definition(d) => TopLevel::Definition(strip_definition(d)),
            TopLevel::Transaction(t) => TopLevel::Transaction(strip_transaction(t)),
            TopLevel::TypeDef(td) => TopLevel::TypeDef(Box::new(strip_typedef(td))),
            other => other.clone(),
        })
        .collect()
}
