// Copyright 2026 Randy Smits-Schreuder Goedheijt
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

//! Loop-field materialization plan.
//!
//! 2026-10-21 (canonical Local/effect model): the frontend records, per
//! bounded transaction, which loop-carried fields are Local (loop-only SSA,
//! no post-loop `%State` commitment) and which remain Machine state. The loop
//! engine consumes this plan instead of re-deriving Local membership.

use crate::analysis::loop_shape::LoopShape;
use std::collections::{BTreeMap, HashMap};

/// Semantic storage class. `Plain` and `Volatile` are represented here so the
/// plan is extensible, but the loop-field planner currently emits only
/// `Local` and `Machine`; broader storage inference owns the other classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    Local,
    Plain,
    Machine,
    Volatile,
}

/// One transaction's field materialization plan, deterministic by field name.
#[derive(Debug, Clone, Default)]
pub struct LoopMaterializationPlan {
    pub fields: BTreeMap<String, StorageClass>,
}

impl LoopMaterializationPlan {
    /// Fields committed to `%State` after the loop.
    pub fn machine_fields(&self) -> impl Iterator<Item = &String> {
        self.fields
            .iter()
            .filter(|(_, class)| **class != StorageClass::Local)
            .map(|(name, _)| name)
    }

    /// Fields that stay loop-local SSA and are not committed at loop exit.
    pub fn local_fields(&self) -> impl Iterator<Item = &String> {
        self.fields
            .iter()
            .filter(|(_, class)| **class == StorageClass::Local)
            .map(|(name, _)| name)
    }
}

/// Build the per-transaction materialization plan from the loop shapes.
/// `local_fields` is the only source of Local membership; every other
/// carried field remains Machine here. Plain/Volatile classification is a
/// separate storage-inference pass.
pub fn build_materialization_plans(
    shapes: &HashMap<String, LoopShape>,
) -> HashMap<String, LoopMaterializationPlan> {
    let mut plans = HashMap::new();
    for (txn, shape) in shapes {
        let local: std::collections::HashSet<&str> =
            shape.local_fields.iter().map(String::as_str).collect();
        let mut fields = BTreeMap::new();
        for field in &shape.carried_fields {
            let class = if local.contains(field.as_str()) {
                StorageClass::Local
            } else {
                StorageClass::Machine
            };
            fields.insert(field.clone(), class);
        }
        plans.insert(txn.clone(), LoopMaterializationPlan { fields });
    }
    plans
}
