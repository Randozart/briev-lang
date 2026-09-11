// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

//! 2026-08-23 (Phase 10): the compiler FRONTEND pipeline, now in the LIB.
//!
//! Extracted verbatim from the binary's `compile` module so library
//! consumers — the SPEC §23.4 conformance sweep, integration tests, and
//! future tooling — run the REAL parse/elaborate/typecheck path instead of
//! a shallow reimplementation. Codegen and linking stay in the binary.
//!
//! Undo: move these functions back into src/compile.rs and drop this
//! module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::ast::{Expr, StageKind, TopLevel};
use crate::lexer::Token;
use crate::plugin::loader::{discover_system_plugins, extract_inline_stage_blocks};
use crate::plugin::PluginManager;
use crate::target::{BackendKind, get_extension};
use crate::type_universe::TypeUniverse;
pub use crate::backend::llvm::TrgUnresolvedAction;
use crate::{macros, dbriev, analysis};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeastStage {
    Parse,
    Resolve,
    TypeCheck,
    Normalize,
    Verify,
    Alloc,
    Provenance,
    Codegen,
    Optimize,
}

impl BeastStage {
    /// Stages that have AST data (get .beast files).
    pub fn has_ast(&self) -> bool {
        matches!(self, BeastStage::Parse | BeastStage::Resolve
            | BeastStage::TypeCheck | BeastStage::Normalize | BeastStage::Verify
            | BeastStage::Alloc | BeastStage::Provenance)
    }

    /// Stages that have IR text data (get .ir files).
    pub fn has_ir(&self) -> bool {
        matches!(self, BeastStage::Codegen | BeastStage::Optimize)
    }
}

impl std::str::FromStr for BeastStage {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "parse" => Ok(BeastStage::Parse),
            "resolve" => Ok(BeastStage::Resolve),
            "type-check" => Ok(BeastStage::TypeCheck),
            "normalize" => Ok(BeastStage::Normalize),
            "verify" => Ok(BeastStage::Verify),
            "alloc" => Ok(BeastStage::Alloc),
            "provenance" => Ok(BeastStage::Provenance),
            "codegen" => Ok(BeastStage::Codegen),
            "optimize" => Ok(BeastStage::Optimize),
            "all" => Err("use multiple --emit-beast flags for individual stages".to_string()),
            _ => Err(format!("unknown BEAST stage '{}'. Use one of: parse, resolve, type-check, \
                             normalize, verify, alloc, provenance, codegen, optimize", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeastPosition {
    Before,
    After,
}

impl BeastPosition {
    pub fn priority(self) -> u32 {
        match self {
            BeastPosition::Before => 999,
            BeastPosition::After => 0,
        }
    }
}

#[derive(Debug)]
pub struct CompiledView {
    pub bindings: Vec<crate::view_compiler::Binding>,
    /// ID-injected HTML — the dom-shim's getElementById() calls resolve
    /// against this, never the raw markup. None when the build has no view.
    pub modified_html: Option<String>,
    /// 2026-08-12 (Iterable protocol, slice 4): the `b-each` iterable FIELDS
    /// whose Briev type is a generic collection (`Applied(base, args)`) — the
    /// backend emits a `__view_items_<field>()` snapshot materializer for
    /// these (driving op Count/op At), and the dom-shim renders from the
    /// snapshot instead of vector layout bytes.
    pub collection_iterables: std::collections::HashSet<String>,
    /// 2026-08-12 (slice 4): the subset whose ELEMENT type is String — the
    /// shim decodes each snapshot word as a `[len][bytes]` string pointer.
    pub collection_string_iterables: std::collections::HashSet<String>,
    pub warnings: Vec<String>,
}

/// 2026-08-12 (2b3): the view-compiler mount specs — HTML-side pool specs
/// (component type → per-mount specs) and Briev-side instance specs (instance
/// var → spec). Bundled so compile_view stays at five parameters.
pub struct ViewMountSpecs {
    /// Component type → per-mount pool specs (`<Counter />` anonymous spawns).
    pub pools: std::collections::HashMap<
        String,
        Vec<crate::analysis::component_instances::MountSpec>,
    >,
    /// Briev-side instance var → spec (`<c1 />` mounts `let c1: Counter`).
    pub instances: std::collections::HashMap<
        String,
        crate::analysis::component_instances::MountSpec,
    >,
}

#[derive(Debug, Clone)]
pub struct BeastFilter {
    pub stage: BeastStage,
    pub position: Option<BeastPosition>,
}

impl std::str::FromStr for BeastFilter {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(dot) = s.find('.') {
            let stage_str = &s[..dot];
            let pos_str = &s[dot + 1..];
            let stage: BeastStage = stage_str.parse()?;
            let position = match pos_str {
                "before" => BeastPosition::Before,
                "after" => BeastPosition::After,
                _ => return Err(format!("unknown beast position '{}'. Use before, after, or omit", pos_str)),
            };
            Ok(BeastFilter { stage, position: Some(position) })
        } else {
            let stage: BeastStage = s.parse()?;
            Ok(BeastFilter { stage, position: None })
        }
    }
}

#[derive(Clone)]
pub struct BuildOptions {
    pub config_dir: Option<String>,
    pub file_path: String,
    pub emit_ir_only: bool,
    pub out_dir: Option<String>,
    pub optimize_budget: u64,
    /// BEAST snapshot stages to emit (--emit-beast). Empty = no emission.
    pub emit_beast_stages: Vec<BeastFilter>,
    /// Selected backend (resolved from extension + --backend flag).
    pub backend: BackendKind,
    /// Disable automatic stdlib import (for bare-metal/no-OS targets).
    pub no_stdlib: bool,
    /// Override stdlib search path.
    pub stdlib_path: Option<String>,
    /// Plugin names to disable (CLI --disable-plugin).
    pub disable_plugins: Vec<String>,
    /// Plugin names to enable exclusively (CLI --enable-plugin).
    pub enable_plugins: Vec<String>,
    /// Action on unresolved dynamic trigger target (--error-unresolved-trg).
    pub trg_unresolved_action: TrgUnresolvedAction,
    /// 2026-07-16: P4 — Pre-compiled .o / .so / .a objects linked into the binary.
    pub extra_objects: Vec<PathBuf>,
    /// 2026-07-18: Build a shared library (.so) instead of an executable.
    pub shared: bool,
    /// 2026-09-01 (plan gpu-backend-hardening Track A): `brievc run x.abv` —
    /// after kernel emission, drive the GPU runtime IN-PROCESS (the RT is
    /// linked into brievc) instead of writing a runner .c file. The SPIR-V
    /// backend arm branches to the run phase machine when this is set.
    pub run: bool,
    /// 2026-08-03: Build a linkable static library (.a) — the `extern "C"`
    /// on-ramp. Runs the full backend in library_mode (emit_library_shim
    /// with __briev_init_state/__glue_release), packages .o + runtime into
    /// `ar rcs lib<name>.a`, and a PIC .so for c_abi hosts.
    pub library_mode: bool,
    /// 2026-07-18: SVO (Small Vector Optimization) — inline storage for
    /// small List<T> elements (≤ N where N is from svo <~ N metadata).
    /// 2026-07-22: Override path for the GLUE config (config/glue.dbv). None = use compiler-shipped default.
    pub glue_config: Option<String>,
    /// 2026-07-18: Maximum size in bytes for stack allocation (alloca).
    /// Allocations exceeding this threshold fall back to heap (malloc).
    /// Used by the runtime fallback check in emit_dynamic_alloc.
    /// Default 4096 (4KB) — safe for most stack frames.
    pub stack_threshold: u64,
    /// 2026-07-25: Native integer width for #Int protocol (default 64).
    /// WASM targets should set to 32 to avoid BigInt in JavaScript.
    pub int_bits: u64,
    /// 2026-07-26: Phase 6b — CSS content from <style> block in .rbv files.
    /// Written as app.css alongside the compiled output for webstack backend.
    pub style_css: Option<String>,
    /// 2026-07-26: Phase 6b — Raw HTML from <view> block in .rbv files.
    /// Wrapped in index.html boilerplate for webstack backend.
    pub view_html: Option<String>,
    /// 2026-07-26: Phase 6b — View bindings from processed <html> template.
    /// Passed to GlueWebGenerator for DOM binding table generation.
    pub view_bindings: Vec<crate::view_compiler::Binding>,
    /// 2026-07-26: Item 3 — Enable SSR (Server-Side Rendering).
    /// Pre-renders initial state into the HTML at compile time.
    /// Only meaningful for webstack backend.
    pub ssr: bool,
    /// 2026-07-26: Item 4 — Enable dev mode (HMR support).
    /// Uses dev-shim.mjs instead of dom-shim.mjs.
    /// Only meaningful for webstack backend.
    pub dev: bool,
    /// 2026-07-23: Allow macros to read files (FileRead$).
    pub allow_read: bool,
    /// 2026-07-23: Allow macros to write files (FileWrite$).
    pub allow_write: bool,
    /// 2026-07-23: Allow macros to execute shell commands (ShellCmd$).
    pub allow_run: bool,
    /// 2026-07-23: Allow macros to query host hardware (SysQuery$).
    pub allow_sys_query: bool,
    /// 2026-07-23: Allow macros network access (HttpFetch$).
    pub allow_net: bool,
    /// 2026-07-23: Instruction budget for macro execution (0 = unlimited).
    pub macro_budget: u64,
    /// 2026-07-23: Print virtual filesystem contents after compilation.
    pub dump_vfs: bool,
    /// 2026-07-23: Regenerate macro-lock.toml from current plugin files.
    pub update_lockfile: bool,
    /// 2026-07-23: Print macro expansion traces after compilation.
    pub dump_traces: bool,
    /// 2026-07-23: Diff mode — show changes macros make to the AST without
    /// writing output. Acts as a dry-run: shows added/removed/modified items.
    pub diff_mode: bool,
    /// 2026-07-23: Overrides for SysQuery$ results.
    /// Populated by --sysquery <key=value> and --sysquery-file <path> flags.
    /// Also populated by --target <name> from briev.toml profiles.
    /// Empty = query real host. Later values override earlier ones.
    pub sysquery_overrides: HashMap<String, String>,
    /// 2026-07-23: Target profile name (--target). None = default/single build.
    /// Overrides are resolved from briev.toml and merged into sysquery_overrides.
    pub target: Option<String>,
    /// 2026-07-23: Raw --sysquery flag pairs (unresolved, for run_build).
    pub sysquery_pairs: Vec<(String, String)>,
    /// 2026-07-23: Raw --sysquery-file paths (unresolved, for run_build).
    pub sysquery_files: Vec<String>,
    /// 2026-09-02: Minimum work-item count for GPU dispatch. Shapes with
    /// fewer work items fall to the CPU loop (--accel-cpu-fallback N).
    /// None = no fallback (current default). Only applies to .bv accel paths.
    pub accel_cpu_fallback: Option<u64>,
    /// 2026-09-06 (ISR plan): the active target profile's ISR mechanism —
    /// the configured default behind mechanism-less `isr` declarations.
    /// Populated from briev.toml [target.<name>] isr_mechanism.
    pub isr_mechanism: Option<String>,
}

pub struct PreprocessedSource {
    pub briev_source: String,
    pub style_css: Option<String>,
    pub view_html: Option<String>,
}

pub fn evaluate_pending_comptime(
    pm: &mut PluginManager,
    program: &mut Vec<TopLevel>,
    universe: &mut TypeUniverse,
) -> Result<(), String> {
    let pending: Vec<(String, crate::ast::Expr, bool)> = pm.pending_comptime.drain()
        .map(|(k, (e, c))| (k, e, c))
        .collect();
    // 2026-07-25: Use a fresh sandbox cloned from pm for evaluation, then
    // merge back to preserve capability tracking.
    let mut sandbox = pm.sandbox.clone();
    for (name, expr, is_const) in pending {
        let val = {
            let mut pm_opt: Option<&mut PluginManager> = Some(pm);
            crate::macros::eval::eval_nav_chain(
                &expr, program, universe, StageKind::Parsed,
                &std::collections::HashMap::new(),
                &mut sandbox, &mut pm_opt,
            )?
        };
        pm.comptime_vars.insert(name, (val, is_const));
    }
    pm.sandbox = sandbox;
    Ok(())
}

pub fn resolve_comptime_refs(
    pm: &PluginManager,
    program: &mut Vec<TopLevel>,
) -> Result<(), String> {
    for item in program.iter_mut() {
        match item {
            TopLevel::Constant(c) => {
                if let Expr::Identifier(name) = &c.expr {
                    if let Some((val, _)) = pm.comptime_vars.get(name) {
                        c.expr = nav_value_to_expr(val)?;
                    }
                }
            }
            TopLevel::Trigger(trg) => {
                if let Expr::Identifier(name) = &trg.instance {
                    if let Some((val, _)) = pm.comptime_vars.get(name) {
                        trg.instance = nav_value_to_expr(val)?;
                    }
                }
            }
            // 2026-08-09: `init` seeding may reference $let/$const names —
            // resolve them the same way const initializers do.
            TopLevel::Init(init) => {
                if let Some(Expr::Identifier(name)) = &init.value {
                    if let Some((val, _)) = pm.comptime_vars.get(name) {
                        init.value = Some(nav_value_to_expr(val)?);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn nav_value_to_expr(val: &crate::macros::eval::NavValue) -> Result<Expr, String> {
    match val {
        crate::macros::eval::NavValue::Int(n) => Ok(Expr::Decimal(*n)),
        crate::macros::eval::NavValue::Bool(b) => Ok(Expr::Bool(*b)),
        crate::macros::eval::NavValue::Str(s) => Ok(Expr::Quoted(s.as_bytes().to_vec())),
        _ => Err(format!("cannot convert {:?} to Expr literal", val)),
    }
}

pub fn preprocess_source_for_path(file_path: &str, source: &str) -> Result<PreprocessedSource, String> {
    if get_extension(file_path) != ".rbv" {
        return Ok(PreprocessedSource {
            briev_source: source.to_string(),
            style_css: None,
            view_html: None,
        });
    }

    // Support logic-only `.rbv` files: if no RBV markup/script tags are
    // present, treat the file as plain Briev source.
    let has_rbv_markup = source.contains("<view>")
        || source.contains("<style>")
        || source.contains("<script")
        || source.contains("</script>");
    if !has_rbv_markup {
        return Ok(PreprocessedSource {
            briev_source: source.to_string(),
            style_css: None,
            view_html: None,
        });
    }

    let rbv = crate::rbv::RbvFile::parse(source)
        .map_err(|e| format!("{}: rbv parse error: {}", file_path, e))?;
    Ok(PreprocessedSource {
        briev_source: rbv.briev_source,
        style_css: rbv.style_css,
        view_html: Some(rbv.view_html),
    })
}

pub fn compile_view(
    file_path: &str,
    items: &[crate::ast::TopLevel],
    opts: &BuildOptions,
    preprocessed: &PreprocessedSource,
    specs: &ViewMountSpecs,
) -> Result<CompiledView, String> {
    let raw_view = effective_view_html(opts, preprocessed, items);
    let Some(html) = raw_view else {
        return Ok(CompiledView {
            bindings: opts.view_bindings.clone(),
            modified_html: opts.view_html.clone(),
            collection_iterables: std::collections::HashSet::new(),
            collection_string_iterables: std::collections::HashSet::new(),
            warnings: Vec::new(),
        });
    };

    let mut vc = crate::view_compiler::ViewCompiler::new();
    // 2026-08-11 (Phase 2b, SPEC 21.3): `render Name { ... }` blocks are
    // reusable view fragments — `<Name />` mounts them at compile time. The
    // analysis supplies the per-mount rewrite SPECS (decisions); the view
    // layer formats the raw fragment per mount (instance-qualified slots +
    // txn variants + data-instance marker).
    let raw_blocks: std::collections::HashMap<String, String> = items
        .iter()
        .filter_map(|item| match item {
            crate::ast::TopLevel::RenderBlock(rb) => {
                Some((rb.struct_name.clone(), rb.view_html.clone()))
            }
            _ => None,
        })
        .collect();
    vc.set_render_blocks(raw_blocks);
    vc.set_component_specs(specs.pools.clone());
    vc.set_instance_specs(specs.instances.clone());
    for item in items {
        match item {
            crate::ast::TopLevel::StateDecl(sd) => {
                vc.register_signal(&sd.name, 0);
            }
            crate::ast::TopLevel::Transaction(t) => {
                vc.register_transaction(&t.name, 0);
            }
            crate::ast::TopLevel::Definition(d) => {
                vc.register_transaction(&d.name, 0);
            }
            _ => {}
        }
    }
    let (bindings, modified_html, diagnostics) = vc.compile(&html);
    // 2026-08-11: compile() surfaces validation_errors first (SPEC 21.4: a
    // rejected directive is never silently ignored) — split the merged list on
    // that count.
    let validation_count = vc.validation_errors.len();
    // 2026-08-11 (housekeeping 1b): a `b-each` iterable must be a STATIC
    // vector field (`Int[N]`/`Bool[N]` — the layout's i{int_bits} slot
    // array). A heap `List`/`String` iterable cannot be indexed by the slot
    // renderer; warn (the generator skips it — never a wrong render) instead
    // of silently rendering garbage.
    let field_types: std::collections::HashMap<String, crate::ast::Type> = items
        .iter()
        .filter_map(|item| match item {
            crate::ast::TopLevel::StateDecl(sd) => Some((sd.name.clone(), sd.ty.clone())),
            crate::ast::TopLevel::Statement(stmt) => {
                if let crate::ast::Statement::Let { name, ty, .. } = stmt.as_ref() {
                    ty.clone().map(|t| (name.clone(), t))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    let mut warnings: Vec<String> = diagnostics.iter().skip(validation_count).cloned().collect();
    let mut collection_iterables: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut collection_string_iterables: std::collections::HashSet<String> = std::collections::HashSet::new();
    for binding in &bindings {
        if let crate::view_compiler::Directive::Each { iterable, .. } = &binding.directive {
            let ty = field_types.get(iterable);
            let is_vector = ty.map(|t| matches!(t, crate::ast::Type::Vector(..))).unwrap_or(false);
            if !is_vector {
                // 2026-08-12 (Iterable protocol, slice 4): a generic collection
                // iterable gets a snapshot materializer; the vector-only skip
                // warning is replaced by the materializer path.
                let is_collection = ty.map(|t| {
                    matches!(
                        t,
                        crate::ast::Type::Applied(..)
                    )
                }).unwrap_or(false);
                if is_collection {
                    collection_iterables.insert(iterable.clone());
                    // 2026-08-12 (slice 4, String elements): a collection whose
                    // element type is String materializes string POINTERS in the
                    // snapshot — the shim decodes them as `[len][bytes]`.
                    let is_string_elem = ty.map(|t| match t {
                        crate::ast::Type::Applied(_, args) => args
                            .first()
                            .map(|a| *a == crate::ast::Type::string())
                            .unwrap_or(false),
                        _ => false,
                    }).unwrap_or(false);
                    if is_string_elem {
                        collection_string_iterables.insert(iterable.clone());
                    }
                } else {
                    warnings.push(format!(
                        "b-each iterable '{}' is neither a static vector field (Int[N]/Bool[N]) \
                         nor a collection — the each is skipped",
                        iterable
                    ));
                }
            }
        }
    }
    let directive_errors: Vec<String> =
        diagnostics.iter().take(validation_count).cloned().collect();
    if !directive_errors.is_empty() {
        return Err(format!(
            "{}: view directive errors:\n{}",
            file_path,
            directive_errors.join("\n")
        ));
    }
    // 2026-08-11: SRBV view-state verification applies only to the `.s` strict
    // profile (SPEC §3.2: `ui.s.rbv`) — strict changes ACCEPTANCE criteria.
    // Plain `.rbv`/`.bv` builds surface ViewCompiler diagnostics as warnings.
    if crate::conformance::is_strict(std::path::Path::new(file_path)) {
        let srbv = crate::view_compiler::verify_srbv(&bindings, items);
        if !srbv.is_empty() {
            return Err(format!(
                "{}: view reference errors:\n{}",
                file_path,
                srbv.join("\n")
            ));
        }
    }
    Ok(CompiledView {
        bindings,
        modified_html: Some(modified_html),
        collection_iterables,
        collection_string_iterables,
        warnings,
    })
}

pub fn view_root_signals(
    bindings: &[crate::view_compiler::Binding],
) -> std::collections::HashSet<String> {
    use crate::view_compiler::Directive;
    let mut set = std::collections::HashSet::new();
    for b in bindings {
        match &b.directive {
            Directive::Text { signal } => {
                set.insert(crate::view_compiler::root_signal(signal).0.to_string());
            }
            Directive::Show { expr } | Directive::Hide { expr } => {
                set.insert(crate::view_compiler::root_signal(expr).0.to_string());
            }
            Directive::When { expr } => {
                set.insert(
                    crate::view_compiler::condition_root_signal(expr)
                        .0
                        .to_string(),
                );
            }
            Directive::Class { pairs } => {
                for (_, v) in pairs {
                    set.insert(crate::view_compiler::root_signal(v).0.to_string());
                }
            }
            Directive::Attr { value, .. } => {
                set.insert(crate::view_compiler::root_signal(value).0.to_string());
            }
            Directive::Style { value, .. } => {
                set.insert(crate::view_compiler::root_signal(value).0.to_string());
            }
            Directive::Each { iterable, .. } => {
                set.insert(crate::view_compiler::root_signal(iterable).0.to_string());
            }
            Directive::Bind { target } => {
                // 2026-08-11 (Phase 2a2): b-bind WRITES the target — the field
                // must stay live so its slot exists for the transaction's
                // write + flush.
                set.insert(crate::view_compiler::root_signal(target).0.to_string());
            }
            Directive::Trigger { .. } => {}
        }
    }
    set
}

pub fn resolve_bind_routes(
    graph: &Option<crate::analysis::transition_graph::ReactorTransitionGraph>,
    items: &[crate::ast::TopLevel],
    universe: &TypeUniverse,
) -> std::collections::HashMap<String, Result<crate::glue::web_generator::BindRoute, String>>
{
    use crate::glue::web_generator::{BindRoute, ParamKind, TypeTag};
    use std::collections::HashMap;

    let mut writers: HashMap<String, Vec<String>> = HashMap::new();
    // Flat scan of every (field, txn) write pair — the total write entries,
    // not nodes × fields (single logical pass; grouping below is linear).
    // 2026-08-12 (2b3 slice 3): compiler-generated lifecycle txns (`__reset_*`,
    // the b-when unmount resets) are not user routes — a b-bind:value must
    // resolve to the single USER writer, never the reset.
    let write_pairs: Vec<(String, String)> = graph
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| !n.name.starts_with("__reset_"))
        .flat_map(|n| n.write_set.iter().map(move |f| (f.clone(), n.name.clone())))
        .collect();
    for (field, txn) in write_pairs {
        writers.entry(field).or_default().push(txn);
    }

    // Transaction → sole parameter Briev type (type-driven marshalling).
    let mut param_ty: HashMap<String, crate::ast::Type> = HashMap::new();
    for item in items {
        let (name, params) = match item {
            crate::ast::TopLevel::Transaction(t) => (&t.name, &t.parameters),
            crate::ast::TopLevel::Definition(d) => (&d.name, &d.parameters),
            _ => continue,
        };
        if let Some((_, ty)) = params.first() {
            param_ty.insert(name.clone(), ty.clone());
        }
    }

    let mut out: HashMap<String, Result<BindRoute, String>> = HashMap::new();
    for (field, mut txns) in writers {
        txns.sort_unstable();
        if txns.len() > 1 {
            out.insert(
                field.clone(),
                Err(format!(
                    "ambiguous — transactions {} write '{}'; b-bind:value needs exactly one writer",
                    txns.join(", "),
                    field
                )),
            );
            continue;
        }
        let txn = txns[0].clone();
        match param_ty.get(&txn) {
            Some(ty) => {
                let cat = crate::type_universe::protocol_category(universe, ty);
                let kind = ParamKind::from_type_tag(TypeTag::from_protocol_category(cat.as_deref()));
                out.insert(field.clone(), Ok(BindRoute { txn, param_kind: kind }));
            }
            None => {
                out.insert(
                    field.clone(),
                    Err(format!(
                        "transaction '{}' takes no parameters; b-bind:value must pass the input value",
                        txn
                    )),
                );
            }
        }
    }
    out
}

/// 2026-08-23 (Phase 10): moved with the frontend pipeline — target config
/// resolution reads only lib-side machinery (config_db + target_spec).
pub fn load_target_config(opts: &BuildOptions) -> crate::target::TargetConfig {
    match &opts.config_dir {
        Some(dir) => match crate::dbriev::config_db::resolve_config_file(
            std::path::Path::new(dir),
            "targets",
        ) {
            Some(path) => crate::target::TargetConfig::load_from(&path).unwrap_or_else(|e| {
                eprintln!(
                    "warning: cannot load '{}': {} — using baked fallback",
                    path.display(),
                    e
                );
                crate::target::TargetConfig::load()
            }),
            None => {
                eprintln!(
                    "warning: no targets config found in '{}' — using baked fallback",
                    dir
                );
                crate::target::TargetConfig::load()
            }
        },
        None => crate::target::TargetConfig::load(),
    }
}

pub fn check_source(file_path: &str, source: &str) -> Result<(), String> {
    let default_opts = BuildOptions {
        run: false,
        config_dir: None,
        file_path: file_path.to_string(),
        emit_ir_only: false,
        out_dir: None,
        optimize_budget: 256,
        emit_beast_stages: vec![],
        backend: BackendKind::Llvm,
        no_stdlib: false,
        stdlib_path: None,
        disable_plugins: vec![],
        enable_plugins: vec![],
        trg_unresolved_action: TrgUnresolvedAction::Warn,
        extra_objects: vec![],
        shared: false,
        library_mode: false,
        int_bits: 64,
        glue_config: None,
        stack_threshold: 4096,
        allow_read: false,
        allow_write: false,
        allow_run: false,
        allow_sys_query: false,
        allow_net: false,
        macro_budget: 0,
        dump_vfs: false,
        update_lockfile: false,
        dump_traces: false,
        diff_mode: false,
        sysquery_overrides: HashMap::new(),
        target: None,
        sysquery_pairs: vec![],
        sysquery_files: vec![],
        style_css: None,
        view_html: None,
        view_bindings: vec![],
        ssr: false,
        dev: false,
        accel_cpu_fallback: None,
        isr_mechanism: None,
    };
    let (_items, _universe) = parse_and_check(file_path, source, &default_opts)?;
    println!("OK");
    Ok(())
}

pub fn check_data_source(file_path: &str, source: &str) -> Result<(), String> {
    let is_dbvl = file_path.ends_with(".dbvl");
    let mut doc = if is_dbvl {
        crate::dbriev::v2::parse_document_track_offsets(source)
    } else {
        crate::dbriev::v2::parse_document(source)
    }
    .map_err(|e| format!("{}: {}", file_path, e))?;
    // Resolve `schema X from "file.dbv"` imports: load the referenced schema
    // files (relative to the checking file's directory) and merge their
    // schemas so the data groups can be validated against them.
    let base_dir = std::path::Path::new(file_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let imports: Vec<String> = doc.imports.clone();
    for imp in imports {
        let candidate = base_dir.join(&imp);
        let Ok(import_src) = std::fs::read_to_string(&candidate) else {
            return Err(format!(
                "{}: cannot resolve schema import '{}' (wanted {})",
                file_path, imp, candidate.display()
            ));
        };
        let imported = crate::dbriev::v2::parse_document(&import_src)
            .map_err(|e| format!("{}: schema import '{}': {}", file_path, imp, e))?;
        doc.schemas.extend(imported.schemas);
    }
    let errors = crate::dbriev::validate::validate_document(&doc);
    if errors.is_empty() {
        println!("OK ({} schema{}, {} data groups)",
            doc.schemas.len(),
            if doc.schemas.len() == 1 { "" } else { "s" },
            doc.data_groups.len());
        Ok(())
    } else {
        Err(format!("schema validation failed for '{}':\n  {}", file_path, errors.join("\n  ")))
    }
}

pub fn build_plugin_manager(file_path: &str, opts: &BuildOptions) -> PluginManager {
    let mut pm = PluginManager::new();

    // Discover system plugins from plugins/{front,mid,post,back}/
    discover_system_plugins(&mut pm);

    // Register built-in Rust plugins
    pm.register(Box::new(crate::plugin::env_plugin::EnvPlugin));
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.register(Box::new(crate::plugin::entry_plugin::EntryPlugin));
    pm.register(Box::new(crate::plugin::inline_frgn_plugin::InlineFrgnPlugin));
    pm.register(Box::new(crate::plugin::script_plugin::ScriptPlugin));

    // Apply per-extension filtering from config/targets.dbvl
    let ext = get_extension(file_path);
    let config = load_target_config(opts);
    pm.filter_for_extension(&ext, &config);

    // Apply CLI overrides
    if !opts.enable_plugins.is_empty() {
        pm = pm.with_enabled_only(opts.enable_plugins.clone());
    }
    if !opts.disable_plugins.is_empty() {
        pm = pm.with_disabled(opts.disable_plugins.clone());
    }
    // --no-std is equivalent to --disable-plugin prelude
    if opts.no_stdlib {
        pm = pm.with_disabled(vec!["prelude".to_string()]);
    }

    // Apply sandbox from CLI flags
    use crate::macros::eval::Sandbox;
    let sandbox = Sandbox {
        allow_read: opts.allow_read,
        allow_write: opts.allow_write,
        allow_run: opts.allow_run,
        allow_sys_query: opts.allow_sys_query,
        allow_net: opts.allow_net,
        budget: opts.macro_budget,
        remaining: opts.macro_budget,
        sysquery_overrides: opts.sysquery_overrides.clone(),
    };
    pm = pm.with_sandbox(sandbox);

    pm
}

pub fn compile_to_typed(file_path: &str, source: &str, opts: &BuildOptions) -> Result<(Vec<TopLevel>, TypeUniverse), String> {
    let mut pm = build_plugin_manager(file_path, opts);
    let project_root = std::env::current_dir()
        .map_err(|e| format!("cannot determine project root: {}", e))?;
    let project_root_str = project_root.to_string_lossy().to_string();
    if opts.update_lockfile {
        let granted = crate::macros::lockfile::cli_granted_set(
            opts.allow_read, opts.allow_write, opts.allow_run,
            opts.allow_sys_query, opts.allow_net,
        );
        let lock = crate::macros::lockfile::generate_lockfile(&granted, None)?;
        crate::macros::lockfile::save_lockfile(&project_root_str, &lock)?;
    } else if let Some(lock) = crate::macros::lockfile::load_lockfile(&project_root_str)? {
        crate::macros::lockfile::validate_and_apply(&lock, &mut pm, None)?;
    }
    let preprocessed = preprocess_source_for_path(file_path, source)?;
    let mut source = preprocessed.briev_source;
    pm.run_source(StageKind::PreLex, &mut source)?;
    let tokens = lex_for_path(file_path, &source)?;
    let mut items = parse(file_path, &tokens, &source)?;
    extract_inline_stage_blocks(&mut items, &mut pm);
    {
        let mut eval_universe = TypeUniverse::new();
        evaluate_pending_comptime(&mut pm, &mut items, &mut eval_universe)?;
    }
    {
        let mut parsed_universe = TypeUniverse::new();
        pm.run_ast(StageKind::Parsed, &mut items, &mut parsed_universe)?;
    }
    let mut resolver = crate::import_resolver::ImportResolver::new();
    if let Some(ref stdlib_path) = opts.stdlib_path {
        resolver = resolver.with_stdlib_path(Some(std::path::PathBuf::from(stdlib_path)));
    }
    items = resolver.resolve_imports(items, &std::path::PathBuf::from(file_path))?;
    extract_inline_stage_blocks(&mut items, &mut pm);
    {
        let mut eval_universe = TypeUniverse::new();
        evaluate_pending_comptime(&mut pm, &mut items, &mut eval_universe)?;
    }
    pm.run_ast(StageKind::Resolved, &mut items, &mut TypeUniverse::new())?;
    resolve_comptime_refs(&pm, &mut items)?;
    let mut universe = TypeUniverse::new();
    check_types(&mut items, &universe, opts.isr_mechanism.as_deref())?;
    pm.run_ast(StageKind::Typed, &mut items, &mut universe)?;
    Ok((items, universe))
}

pub fn effective_view_html(
    opts: &BuildOptions,
    preprocessed: &PreprocessedSource,
    items: &[crate::ast::TopLevel],
) -> Option<String> {
    if let Some(h) = opts.view_html.as_ref().or(preprocessed.view_html.as_ref()) {
        return Some(h.clone());
    }
    let root = items.iter().find_map(|item| match item {
        crate::ast::TopLevel::RenderBlock(rb) if rb.struct_name == "Root" => {
            Some(rb.view_html.clone())
        }
        _ => None,
    });
    if root.is_some() {
        return root;
    }
    let parts: Vec<String> = items
        .iter()
        .filter_map(|item| match item {
            crate::ast::TopLevel::RenderBlock(rb) => Some(rb.view_html.clone()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn parse_and_check(file_path: &str, source: &str, opts: &BuildOptions) -> Result<(Vec<crate::ast::TopLevel>, TypeUniverse), String> {
    // 2026-08-18 (check/build divergence): `brievc check` MUST run the SAME
    // pipeline stages as `brievc build` before check_types — plugin manager +
    // lockfile, PreLex, inline stage blocks, comptime evaluation, the Parsed
    // and Resolved plugin stages, import resolution, and comptime-ref
    // resolution. The check path was a stale LEAN pipeline (parse → resolve →
    // check) predating those stages; it silently diverged — e.g. `brievc check`
    // over-reported "expected List<K> for arrow assignment, found K" on
    // imported generic collection scans while `build` was clean (the resolver
    // now walks member OUTPUT types so `import { HashMap }` brings `List`; the
    // divergence class is closed by sharing the pipeline).
    let preprocessed = preprocess_source_for_path(file_path, source)?;
    let mut source = preprocessed.briev_source;
    let mut pm = build_plugin_manager(file_path, opts);
    let project_root = std::env::current_dir()
        .map_err(|e| format!("cannot determine project root: {}", e))?;
    let project_root_str = project_root.to_string_lossy().to_string();
    if opts.update_lockfile {
        let granted = crate::macros::lockfile::cli_granted_set(
            opts.allow_read,
            opts.allow_write,
            opts.allow_run,
            opts.allow_sys_query,
            opts.allow_net,
        );
        let lock = crate::macros::lockfile::generate_lockfile(&granted, None)?;
        crate::macros::lockfile::save_lockfile(&project_root_str, &lock)?;
    } else if let Some(lock) = crate::macros::lockfile::load_lockfile(&project_root_str)? {
        crate::macros::lockfile::validate_and_apply(&lock, &mut pm, None)?;
    }
    pm.run_source(StageKind::PreLex, &mut source)?;
    let tokens = lex_for_path(file_path, &source)?;
    let mut items = parse(file_path, &tokens, &source)?;
    extract_inline_stage_blocks(&mut items, &mut pm);
    {
        let mut eval_universe = TypeUniverse::new();
        evaluate_pending_comptime(&mut pm, &mut items, &mut eval_universe)?;
    }
    {
        let mut parsed_universe = TypeUniverse::new();
        pm.run_ast(StageKind::Parsed, &mut items, &mut parsed_universe)?;
    }
    let mut resolver = crate::import_resolver::ImportResolver::new();
    if let Some(ref stdlib_path) = opts.stdlib_path {
        resolver = resolver.with_stdlib_path(Some(std::path::PathBuf::from(stdlib_path)));
    }
    items = resolver.resolve_imports(items, &std::path::PathBuf::from(file_path))?;
    extract_inline_stage_blocks(&mut items, &mut pm);
    {
        let mut eval_universe = TypeUniverse::new();
        evaluate_pending_comptime(&mut pm, &mut items, &mut eval_universe)?;
    }
    {
        pm.run_ast(StageKind::Resolved, &mut items, &mut TypeUniverse::new())?;
    }
    resolve_comptime_refs(&pm, &mut items)?;

    let universe = TypeUniverse::new();
    check_types(&mut items, &universe, opts.isr_mechanism.as_deref())?;
    // 2026-08-01 (C4): watchdog contract checks also run on the `check` path
    // (parse_and_check) — `brievc check` must catch trigger/handler violations
    // and missing on-fire handlers the same way `brievc build` does.
    let watchdog_errors = crate::analysis::watchdog::analyze(&items);
    if !watchdog_errors.is_empty() {
        let msgs: Vec<String> = watchdog_errors.iter().map(|e| e.to_string()).collect();
        return Err(format!("watchdog errors:\n{}", msgs.join("\n")));
    }
    crate::analysis::watchdog::check_on_fire_handlers(&items)
        .map_err(|e| format!("watchdog error:\n{}", e))?;
    // 2026-08-04: term termination diagnostics — `check` must catch
    // unreachable code after a terminating `term <value>`/`term! <value>`
    // exactly like `build` does, or the two paths silently diverge.
    let (term_errors, term_warnings) = crate::analysis::termination::analyze(&items);
    for w in &term_warnings {
        eprintln!("warning: {w}");
    }
    if !term_errors.is_empty() {
        return Err(format!("termination errors:\n{}", term_errors.join("\n")));
    }
    // 2026-08-22 (spec-conformance plan Phase 2): casing advisory — `check`
    // reports the same advisories as `build` so the two paths never diverge.
    for w in crate::analysis::casing::analyze(&items) {
        eprintln!("warning: {w}");
    }
    // 2026-08-22 (Phase 9): `.s` strict gate in `check` too — acceptance
    // criteria must not diverge between check and build (2026-08-18 lesson).
    if crate::conformance::is_strict(std::path::Path::new(file_path)) {
        let mc = crate::macros::memcheck::run_memcheck(&items);
        crate::analysis::strict::enforce(&items, &mc)?;
    }
    // 2026-08-22 (Phase 8): linearity gate in `check` too — acceptance never
    // diverges between paths.
    let task_errors = crate::analysis::task_linear::analyze(&items);
    if !task_errors.is_empty() {
        return Err(format!(
            "task handle errors:\n{}",
            task_errors.join("\n")
        ));
    }
    // 2026-08-07 (object instance pools): `check` must reject unprovable
    // spawn counts exactly like `build` (Briev has no runtime errors).
    let (_, _, spawn_errors, _) = crate::analysis::spawn_pool::analyze(&items);
    if !spawn_errors.is_empty() {
        return Err(format!(
            "spawn pool errors:\n{}",
            spawn_errors.join("\n")
        ));
    }
    Ok((items, universe))
}

fn lex(source: &str) -> Result<Vec<(Token, std::ops::Range<usize>)>, String> {
    use logos::Logos;
    let mut lexer = Token::lexer(source);
    let mut tokens = Vec::new();
    while let Some(result) = lexer.next() {
        let token = result.map_err(|_| "lex error".to_string())?;
        let span = lexer.span();
        tokens.push((token, span));
    }
    Ok(tokens)
}

pub fn lex_for_path(
    file_path: &str,
    source: &str,
) -> Result<Vec<(Token, std::ops::Range<usize>)>, String> {
    if crate::conformance::is_formatted(std::path::Path::new(file_path)) {
        crate::layout::layout_process(source).map_err(|e| {
            format!("{}: formatted-source error: {}", file_path, e)
        })
    } else {
        lex(source)
    }
}

pub fn parse(file_path: &str, tokens: &[(Token, std::ops::Range<usize>)], source: &str) -> Result<Vec<crate::ast::TopLevel>, String> {
    let mut parser = crate::parser::Parser::new(tokens.to_vec(), source);
    parser.parse_program().map_err(|e| format!("{}: parse error: {}", file_path, e))
}

pub fn validate_constraints(items: &[crate::ast::TopLevel]) -> Result<(), String> {
    for item in items {
        let crate::ast::TopLevel::TypeDef(td) = item else { continue; };
        for tp in &td.type_params {
            let crate::ast::top::TypeParam { name, bound: Some(bound) } = tp else { continue; };
            let bound_category = match bound {
                crate::ast::Type::HashWord(c) => c.as_str(),
                crate::ast::Type::HashWordVariant(c, _) => c.as_str(),
                _ => continue,
            };
            // Check at least one operator references this hashword in its params
            let has_op = td.body.operators.iter().any(|op| {
                op.params.iter().any(|p| {
                    matches!(p,
                        crate::ast::Type::HashWord(c) if c == bound_category
                    ) || matches!(p,
                        crate::ast::Type::HashWordVariant(c, _) if c == bound_category
                    )
                })
            });
            if !has_op {
                return Err(format!(
                    "constraint '{}: {}' in type '{}' is unsatisfiable — \
                     no operator references {} in its parameters. \
                     Add an op declaration like 'op ...({}, ...)' to use this constraint.",
                    name, bound, td.name, bound, bound
                ));
            }
        }
    }
    Ok(())
}

pub fn check_types(items: &mut [crate::ast::TopLevel], universe: &TypeUniverse, isr_mechanism: Option<&str>) -> Result<(), String> {
    validate_constraints(items)?;
    crate::typechecker::check_program_with_target(items, universe, isr_mechanism)
        .map_err(|errors| {
            let msgs: Vec<String> = errors.iter().map(|e| format!("{}", e)).collect();
            format!("type errors:\n  {}", msgs.join("\n  "))
        })
}
