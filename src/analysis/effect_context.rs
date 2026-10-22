// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Canonical effect-context plan.
//!
//! 2026-10-21 (`docs/plans/2026-10-21-canonical-effect-abi.md`, S1): derive
//! the effect-context membership and per-defn ABI reach from the already
//! classified plain/plumbing storage set. This is the analysis consumed by
//! the backend; it performs no codegen and recognizes no effect-function
//! names. Every plain field is context-owned because plain storage is, by
//! definition, non-machine plumbing state.

use crate::analysis::export_abi::compute_defn_needs_plain;
use std::collections::{HashMap, HashSet};

/// Deterministic plan for the `%EffectCtx` aggregate and its ABI consumers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectContextPlan {
    /// Sorted plain-field names owned by the context.
    pub fields: Vec<String>,
    /// Defns that must receive `ptr %effect` because they transitively reach
    /// a plain field. Derived from the same least fixpoint as the existing
    /// plain-context ABI.
    pub defn_takes_effect: HashMap<String, bool>,
}

impl EffectContextPlan {
    /// ABI predicate for call-site marshaling.
    pub fn defn_takes_effect(&self, name: &str) -> bool {
        self.defn_takes_effect.get(name).copied().unwrap_or(false)
    }
}

/// Build the canonical effect-context plan.
///
/// `plain` is the storage-class classifier's plain set (inferred plus
/// explicitly asserted plain fields). The backend consumes this instead of
/// scattering plain-global accesses across the print chain.
pub fn build_effect_context_plan(
    items: &[crate::ast::TopLevel],
    plain: &HashSet<String>,
) -> EffectContextPlan {
    let mut fields: Vec<String> = plain.iter().cloned().collect();
    fields.sort();
    EffectContextPlan {
        fields,
        defn_takes_effect: compute_defn_needs_plain(items, plain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<crate::ast::TopLevel> {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        parser.parse_program().expect("parse")
    }

    #[test]
    fn effect_context_fields_are_sorted_and_defn_reach_is_transitive() {
        let items = parse(
            "plain let fill: Int = 0;\n             plain let buf: Int = 0;\n             let machine: Int = 0;\n             defn put(x: Int) -> Int { fill = fill + x; term fill; };\n             defn caller(x: Int) -> Int { term put(x); };\n             defn stateful(x: Int) -> Int { machine = machine + x; term machine; };\n",
        );
        let plain: HashSet<String> = ["buf".to_string(), "fill".to_string()]
            .into_iter()
            .collect();
        let plan = build_effect_context_plan(&items, &plain);
        assert_eq!(plan.fields, vec!["buf".to_string(), "fill".to_string()]);
        assert!(plan.defn_takes_effect("put"));
        assert!(plan.defn_takes_effect("caller"));
        assert!(!plan.defn_takes_effect("stateful"));
    }

    #[test]
    fn empty_plain_set_yields_empty_context() {
        let items = parse("let machine: Int = 0;\n             defn f(x: Int) -> Int { machine = machine + x; term machine; };\n");
        let plan = build_effect_context_plan(&items, &HashSet::new());
        assert!(plan.fields.is_empty());
        assert!(!plan.defn_takes_effect("f"));
    }
}
