// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H0a HIR invariants. Kept deliberately strict so later phases cannot admit
//! duplicate IDs, unnamed exports, or malformed callable metadata.

use super::program::HirProgram;
use std::collections::BTreeSet;

pub fn verify(program: &HirProgram) -> Result<(), String> {
    let mut defs = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut items = BTreeSet::new();
    for callable in &program.callables {
        verify_callable(callable, &mut defs, &mut nodes, &mut items)?;
    }
    Ok(())
}

fn verify_callable(
    callable: &super::program::HirCallable,
    defs: &mut BTreeSet<super::ids::DefId>,
    nodes: &mut BTreeSet<super::ids::NodeId>,
    items: &mut BTreeSet<super::ids::ItemId>,
) -> Result<(), String> {
    if callable.name.is_empty() {
        return Err(format!("callable {:?} has an empty name", callable.id));
    }
    verify_unique_ids(callable, defs, nodes, items)?;
    verify_export_metadata(callable)
}

fn verify_unique_ids(
    callable: &super::program::HirCallable,
    defs: &mut BTreeSet<super::ids::DefId>,
    nodes: &mut BTreeSet<super::ids::NodeId>,
    items: &mut BTreeSet<super::ids::ItemId>,
) -> Result<(), String> {
    if !defs.insert(callable.id) {
        return Err(format!("duplicate DefId {:?}", callable.id));
    }
    if !items.insert(callable.item) {
        return Err(format!("duplicate ItemId {:?}", callable.item));
    }
    if !nodes.insert(callable.node) || !nodes.insert(callable.body.node) {
        return Err(format!("duplicate NodeId in callable {}", callable.name));
    }
    Ok(())
}

fn verify_export_metadata(callable: &super::program::HirCallable) -> Result<(), String> {
    match (callable.exported, callable.export_symbol.as_deref()) {
        (true, Some(symbol)) if !symbol.is_empty() => Ok(()),
        (true, _) => Err(format!("exported callable '{}' has no symbol", callable.name)),
        (false, Some(_)) => Err(format!(
            "non-exported callable '{}' carries an export symbol",
            callable.name
        )),
        (false, None) => Ok(()),
    }
}
