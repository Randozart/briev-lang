// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! H1d frontend -> HIR boundary. All syntax-expanding stages run before
//! checked HIR, so no plugin/composite/desugar marker reaches the semantic IR.

use super::lower::{lower_program_checked, HirLowerIssue};
use super::program::HirProgram;
use crate::ast::{StageKind, TopLevel};
use crate::plugin::PluginManager;
use crate::type_universe::TypeUniverse;

/// Run the syntax-expanding frontend and lower checked HIR.
///
/// This mirrors the pre-typecheck half of the build pipeline. Type-dependent
/// plugins run later; H1d covers Parsed/Resolved syntax expansion only.
pub fn lower_program_after_frontend(
    items: &mut Vec<TopLevel>,
    pm: &PluginManager,
    universe: &mut TypeUniverse,
) -> Result<HirProgram, String> {
    pm.run_ast(StageKind::Parsed, items, universe)?;
    pm.run_ast(StageKind::Resolved, items, &mut TypeUniverse::new())?;
    crate::pipeline::resolve_comptime_refs(pm, items)?;
    crate::plugin::composite::expand_composites(items, pm)?;
    crate::analysis::variadic_specialize::specialize_variadic(items)?;
    crate::analysis::desugar::rewrite_enum_variant_access(items)?;
    crate::analysis::desugar::rewrite_multi_index(items)?;
    crate::analysis::desugar::rewrite_wildcard_lift(items)?;
    lower_program_checked(items).map_err(format_issues)
}

fn format_issues(issues: Vec<HirLowerIssue>) -> String {
    let rendered: Vec<String> = issues
        .into_iter()
        .map(|issue| format!("{} x{}", issue.tag, issue.count))
        .collect();
    format!("frontend left unresolved HIR nodes: {}", rendered.join(", "))
}
