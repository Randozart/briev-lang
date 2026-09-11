// ── Plugin Loader — Stage Block Plugin & System Discovery ───────────
//
// 2026-07-21: Stage directories updated from {front,mid,post,back} to
// {parsed,resolved,typed,normalized,verified,allocated,provenanced,generated,
// optimized,linked} as part of the granular pipeline expansion.
// 2026-07-15: Phase 2 — Provides StageBlockPlugin (wraps $(Stage) blocks
// into Plugin trait) and discover_system_plugins() which scans
// plugins/{stage}/ directories for .bv files and extracts their $(Stage) blocks.
//
// Native (.so) and WASM (.wasm) loaders from Phase 7 (2026-07-11) are
// retained but will be replaced by the stage-based architecture in
// Phase 5.

use super::{Plugin, PluginManager, PluginEntry};
use crate::ast::{StageBlock, StageKind, TopLevel};
use crate::macros;
use crate::parser::Parser;
use crate::target::TargetConfig;
use crate::type_universe::TypeUniverse;
use std::path::Path;

// ── StageBlockPlugin ──────────────────────────────────────────────────

/// Wraps a parsed $(Stage) block into a Plugin.
///
/// The plugin runs at the stage specified by the block. Its body is a
/// sequence of statements that may call compiler-known $ intrinsics
/// (e.g., InsertRegistryImport$). The $ intrinsics are evaluated when
/// the plugin's on_ast / on_ir method is called.
///
/// 2026-07-15: Phase 2 — StageBlockPlugin wraps parsed AST blocks.
/// Phase 3 will implement the $ intrinsic dispatch.
#[derive(Debug)]
pub struct StageBlockPlugin {
    name: String,
    stage: StageKind,
    priority: u32,
    body: Vec<crate::ast::Statement>,
    /// 2026-07-21: Raw pointer to PluginManager for Stage$.Insert$/List$/Remove$.
    /// Set before evaluate_body is called. Safe because PluginManager outlives
    /// the plugin evaluation and we only borrow mutably in the eval chain.
    pm_ptr: std::cell::Cell<usize>,
}

impl StageBlockPlugin {
    pub fn new(name: String, block: StageBlock) -> Self {
        StageBlockPlugin {
            name,
            stage: block.stage,
            priority: block.priority,
            body: block.body,
            pm_ptr: std::cell::Cell::new(0),
        }
    }

    /// Set the raw PluginManager pointer for Stage$ operations.
    pub fn set_pm_ptr(&self, ptr: usize) {
        self.pm_ptr.set(ptr);
    }

    /// Retrieve the PluginManager as &mut, if set.
    fn get_pm(&self) -> Option<&mut PluginManager> {
        let ptr = self.pm_ptr.get();
        if ptr == 0 { return None; }
        Some(unsafe { &mut *(ptr as *mut PluginManager) })
    }

    /// Evaluate the block's body statements using the new navigation engine.
    /// 2026-07-21: Replaced intrinsics::evaluate_statement with
    /// macros::eval::evaluate_stage_block for full navigation DSL support.
    fn evaluate_body(
        &self,
        program: &mut Vec<TopLevel>,
        universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        let mut pm = self.get_pm();
        macros::eval::evaluate_stage_block(
            &self.body, program, universe, self.stage, &mut pm,
        )
    }
}

impl Plugin for StageBlockPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![self.stage]
    }

    fn on_ast(
        &self,
        program: &mut Vec<TopLevel>,
        universe: &mut TypeUniverse,
    ) -> Result<(), String> {
        self.evaluate_body(program, universe)
    }

    fn on_ir(&self, _ir: &mut String) -> Result<(), String> {
        Ok(())
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

// ── System Plugin Discovery ───────────────────────────────────────────

/// Stage directory names under the plugins/ root.
/// 2026-07-21: Will be updated to granular directories as part of the
/// granular pipeline implementation:
///   ("parsed", StageKind::Parsed),
///   ("typed", StageKind::Typed),
///   ("verified", StageKind::Verified),
///   etc.
/// See docs/plans/2026-07-21-granular-pipeline-and-ast-navigation.md.
const STAGE_DIRS: &[(&str, StageKind)] = &[
    ("prelex", StageKind::PreLex),
    ("parsed", StageKind::Parsed),
    ("resolved", StageKind::Resolved),
    ("typed", StageKind::Typed),
    ("normalized", StageKind::Normalized),
    ("verified", StageKind::Verified),
    ("allocated", StageKind::Allocated),
    ("provenanced", StageKind::Provenanced),
    ("generated", StageKind::Generated),
    ("optimized", StageKind::Optimized),
    ("linked", StageKind::Linked),
];

/// Discover system plugins from the compiler's plugins/ directory.
///
/// Scans `plugins/{front,mid,post,back}/` for `.bv` files, parses each,
/// extracts all `$(Stage)` blocks, and wraps each as a `StageBlockPlugin`
/// registered into the provided `PluginManager`.
///
/// 2026-07-15: Phase 2 — System plugins ship with the compiler. Each
/// `.bv` file in the stage directories can contain one or more $(Stage)
/// blocks. The plugin name is derived from the filename and a block
/// index to ensure uniqueness.
pub fn discover_system_plugins(mgr: &mut PluginManager) {
    let base = Path::new("plugins");
    for (dir_name, stage) in STAGE_DIRS {
        let dir = base.join(dir_name);
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bv") {
                continue;
            }
            let source = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // Parse the file to extract $(Stage) blocks.
            // We create a temporary parse context — if parsing fails,
            // skip the file with a warning.
            let tokens = match crate::lexer::tokenize(&source) {
                Ok(t) => t,
                Err(_) => {
                    eprintln!("warning: system plugin '{}' failed to tokenize, skipping", path.display());
                    continue;
                }
            };
            let mut parser = Parser::new(tokens, &source);
            let items = match parser.parse_program() {
                Ok(items) => items,
                Err(e) => {
                    eprintln!("warning: system plugin '{}' parse error: {}, skipping", path.display(), e);
                    continue;
                }
            };

            // Extract StageBlock top-levels and register each.
            let file_stem = path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let mut block_idx = 0u32;
            for item in &items {
                if let TopLevel::StageBlock(block) = item {
                    // Plugin name is the filestem (e.g. "prelude", "prelude-hw").
                    // Multi-block files get suffixes ("prelude-1", etc).
                    let plugin_name = if block_idx == 0 {
                        file_stem.clone()
                    } else {
                        format!("{}-{}", file_stem, block_idx)
                    };
                    let plugin = StageBlockPlugin::new(
                        plugin_name,
                        block.clone(),
                    );
                    mgr.register_with_priority(
                        Box::new(plugin),
                        block.priority,
                    );
                    block_idx += 1;
                }
            }
        }
    }
}

// ── Inline Plugin Extraction ──────────────────────────────────────────

/// Extract inline $(Stage) blocks from a parsed program and register
/// them as plugins on the manager.
///
/// Inline blocks are those written directly in user source files. They
/// are removed from the program AST after extraction so they do not
/// reach codegen.
///
/// 2026-07-15: Phase 2 — Inline $(Stage) blocks become StageBlockPlugin
/// instances. The block is removed from the AST after registration.
pub fn extract_inline_stage_blocks(
    program: &mut Vec<TopLevel>,
    mgr: &mut PluginManager,
) {
    let mut blocks: Vec<(usize, StageBlock)> = Vec::new();
    for (i, item) in program.iter().enumerate() {
        if let TopLevel::StageBlock(block) = item {
            blocks.push((i, block.clone()));
        }
    }

    // Extract in reverse order to preserve indices during removal.
    let mut block_idx = 0u32;
    for (i, block) in blocks.into_iter().rev() {
        let plugin_name = format!("inline:{}", block_idx);
        let priority = block.priority;
        let plugin = StageBlockPlugin::new(plugin_name.clone(), block);
        mgr.register_with_priority(Box::new(plugin), priority);
        // 2026-07-23: Inline plugins must also be added to enabled_only when it
        // is set (by filter_for_extension), otherwise they are silently filtered out.
        if !mgr.enabled_only().is_empty() {
            mgr.enabled_only_mut().push(plugin_name);
        }
        program.remove(i);
        block_idx += 1;
    }

    // 2026-07-23: Extract $defn/$txn compile-time functions.
    // These are top-level items, registered in fn_registry and removed
    // before codegen so they never reach the LLVM backend.
    let mut fn_indices: Vec<usize> = Vec::new();
    for (i, item) in program.iter().enumerate() {
        match item {
            TopLevel::CompileTimeDefn(d) => {
                mgr.fn_registry.insert(d.name.clone(), crate::plugin::FnDef::Defn(d.clone()));
                fn_indices.push(i);
            }
            TopLevel::CompileTimeTxn(t) => {
                mgr.fn_registry.insert(t.name.clone(), crate::plugin::FnDef::Txn(t.clone()));
                fn_indices.push(i);
            }
            _ => {}
        }
    }
    // Remove in reverse order to preserve indices
    for i in fn_indices.into_iter().rev() {
        program.remove(i);
    }

    // 2026-07-25: Extract $let/$const compile-time variables.
    // Stored in pending_comptime — evaluated in compile.rs before stage execution.
    let mut comptime_indices: Vec<usize> = Vec::new();
    for (i, item) in program.iter().enumerate() {
        match item {
            TopLevel::CompileTimeLet(name, expr) => {
                mgr.pending_comptime.insert(name.clone(), (expr.clone(), false));
                comptime_indices.push(i);
            }
            TopLevel::CompileTimeConst(name, expr) => {
                mgr.pending_comptime.insert(name.clone(), (expr.clone(), true));
                comptime_indices.push(i);
            }
            _ => {}
        }
    }
    for i in comptime_indices.into_iter().rev() {
        program.remove(i);
    }
}

// ── ValidationPlugin (kept from Phase 7) ──────────────────────────────

/// A simple validations plugin that checks program invariants.
/// Retained as a built-in example and for testing.
/// 2026-07-11: Phase 7. 2026-07-15: Adapted to new Plugin trait.
#[derive(Debug)]
pub struct ValidationPlugin {
    name: String,
}

impl ValidationPlugin {
    pub fn new() -> Self {
        ValidationPlugin { name: "builtin:validation".into() }
    }
}

impl Plugin for ValidationPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn stages(&self) -> Vec<StageKind> {
        vec![StageKind::Parsed, StageKind::Typed, StageKind::Generated, StageKind::Optimized]
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Statement;

    #[test]
    fn test_validation_plugin_name() {
        let plugin = ValidationPlugin::new();
        assert_eq!(plugin.name(), "builtin:validation");
    }

    #[test]
    fn test_stage_block_plugin_creation() {
        let block = StageBlock {
            stage: StageKind::Parsed,
            priority: 100,
            body: vec![],
            span: None,
        };
        let plugin = StageBlockPlugin::new("test:block".to_string(), block);
        assert_eq!(plugin.name(), "test:block");
        assert!(plugin.stages().contains(&StageKind::Parsed));
    }

    #[test]
    fn test_stage_block_plugin_on_ast_ok() {
        let block = StageBlock {
            stage: StageKind::Parsed,
            priority: 100,
            body: vec![],
            span: None,
        };
        let plugin = StageBlockPlugin::new("test:midblock".to_string(), block);
        let mut program = vec![];
        let mut universe = TypeUniverse::new();
        assert!(plugin.on_ast(&mut program, &mut universe).is_ok());
    }

    #[test]
    fn test_stage_block_plugin_on_ir_ok() {
        let block = StageBlock {
            stage: StageKind::Generated,
            priority: 0,
            body: vec![],
            span: None,
        };
        let plugin = StageBlockPlugin::new("test:postblock".to_string(), block);
        let mut ir = "define i32 @main() { ret i32 0 }".to_string();
        assert!(plugin.on_ir(&mut ir).is_ok());
    }

    #[test]
    fn test_extract_inline_stage_blocks_empty() {
        let mut program: Vec<TopLevel> = vec![];
        let mut mgr = PluginManager::new();
        extract_inline_stage_blocks(&mut program, &mut mgr);
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_extract_inline_stage_blocks_removes_from_ast() {
        use crate::ast::Statement;
        let block = StageBlock {
            stage: StageKind::Parsed,
            priority: 100,
            body: vec![],
            span: None,
        };
        let mut program = vec![TopLevel::StageBlock(block)];
        let mut mgr = PluginManager::new();
        extract_inline_stage_blocks(&mut program, &mut mgr);
        assert!(program.is_empty(), "StageBlock should be removed from AST");
        assert_eq!(mgr.len(), 1, "StageBlock should be registered as plugin");
    }

    #[test]
    fn test_discover_system_plugins_no_dir() {
        // No plugins/ directory exists yet — should not panic.
        let mut mgr = PluginManager::new();
        discover_system_plugins(&mut mgr);
        // Just check no crash. Plugins may or may not be found.
    }

    #[test]
    fn test_filter_for_extension_prelude() {
        let mut mgr = PluginManager::new();
        let prelude = StageBlockPlugin::new(
            "prelude-native".to_string(),
            StageBlock {
                stage: StageKind::Parsed,
                priority: 0,
                body: vec![],
                span: None,
            },
        );
        mgr.register(Box::new(prelude));
        mgr.register(Box::new(ValidationPlugin::new()));

        let config = TargetConfig::load();
        mgr.filter_for_extension(".bv", &config);

        let names = mgr.enabled_names(None);
        // .bv in config has plugins = ["prelude-native", ...] (2026-09-09,
        // Family A/B: the pure-Briev runtime prelude).
        assert!(names.contains(&"prelude-native".to_string()));
        // builtin:validation is NOT in .bv's plugin list, so should be excluded
        assert!(!names.contains(&"builtin:validation".to_string()));
    }

    #[test]
    fn test_filter_for_extension_sbv() {
        let mut mgr = PluginManager::new();
        let prelude_hw = StageBlockPlugin::new(
            "prelude-hw".to_string(),
            StageBlock {
                stage: StageKind::Parsed,
                priority: 0,
                body: vec![],
                span: None,
            },
        );
        mgr.register(Box::new(prelude_hw));
        mgr.register(Box::new(ValidationPlugin::new()));

        let config = TargetConfig::load();
        mgr.filter_for_extension(".sbv", &config);

        let names = mgr.enabled_names(None);
        assert!(names.contains(&"prelude-hw".to_string()));
        assert!(!names.contains(&"builtin:validation".to_string()));
    }

    #[test]
    fn test_missing_extension_defaults_to_prelude() {
        let mut mgr = PluginManager::new();
        let p = StageBlockPlugin::new(
            "prelude".to_string(),
            StageBlock {
                stage: StageKind::Parsed,
                priority: 0,
                body: vec![],
                span: None,
            },
        );
        mgr.register(Box::new(p));
        let config = TargetConfig::load();
        // .xhv doesn't exist in config — should default to ["prelude"]
        mgr.filter_for_extension(".xhv", &config);
        let names = mgr.enabled_names(None);
        assert!(names.contains(&"prelude".to_string()));
    }
}
