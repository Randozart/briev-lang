// ── Stage$ Target — Plugin Injection ────────────────────────────────────
// 2026-07-21: Stage$.Insert$/Remove$/List$ for forward-only plugin
// injection. A $(Stage) block can register new plugins for later stages.
// Forward-only: N → >N only, enforced at registration time.

use crate::ast::{StageBlock, StageKind, TopLevel};
use crate::plugin::PluginManager;
use crate::plugin::loader::StageBlockPlugin;

/// Names of all registered plugins (for Stage$.List$).
pub fn list_plugins(pm: &PluginManager) -> Vec<String> {
    pm.list_names()
}

/// Remove a plugin by name (Stage$.Remove$).
pub fn remove_plugin(pm: &mut PluginManager, name: &str) {
    pm.disable_plugin(name);
}

/// Register new plugins from a $(Stage) block file (Stage$.Insert$(path)).
/// Parses the file, extracts all $(Stage) blocks, and registers each as a
/// StageBlockPlugin. Forward-only: only stages > current_stage are accepted.
pub fn insert_plugin_from_file(
    pm: &mut PluginManager,
    path: &str,
    current_stage: StageKind,
) -> Result<(), String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("Stage$.Insert$: cannot read '{}': {}", path, e))?;

    let tokens = crate::lexer::tokenize(&source)
        .map_err(|e| format!("Stage$.Insert$: tokenize error in '{}': {}", path, e))?;

    let mut parser = crate::parser::Parser::new(tokens, &source);
    let items = parser
        .parse_program()
        .map_err(|e| format!("Stage$.Insert$: parse error in '{}': {}", path, e))?;

    let file_stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("injected")
        .to_string();

    let mut count = 0u32;
    for item in &items {
        if let TopLevel::StageBlock(block) = item {
            if block.stage <= current_stage {
                eprintln!(
                    "warning: Stage$.Insert$ skipping block at stage {:?} — \
                     must be > current stage {:?}",
                    block.stage, current_stage
                );
                continue;
            }
            let plugin_name = format!("{}-inject-{}", file_stem, count);
            let plugin = StageBlockPlugin::new(plugin_name, block.clone());
            let result = pm.register_during_stage(Box::new(plugin), block.priority, current_stage);
            match result {
                Ok(()) => count += 1,
                Err(e) => eprintln!("warning: Stage$.Insert$: {}", e),
            }
        }
    }
    if count == 0 {
        eprintln!(
            "warning: Stage$.Insert$: no valid $(Stage) blocks found in '{}'",
            path
        );
    }
    Ok(())
}

/// Register a plugin from an inline $(Stage) block (Stage$.Insert$(block)).
pub fn insert_inline_block(
    pm: &mut PluginManager,
    block: StageBlock,
    current_stage: StageKind,
) -> Result<(), String> {
    if block.stage <= current_stage {
        return Err(format!(
            "Stage$.Insert$: cannot register at stage {:?} from stage {:?} — forward-only",
            block.stage, current_stage
        ));
    }
    let priority = block.priority;
    let plugin_name = format!("inline-{:?}-{}", block.stage, pm.len());
    let plugin = StageBlockPlugin::new(plugin_name, block);
    pm.register_during_stage(Box::new(plugin), priority, current_stage)
}
