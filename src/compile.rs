// ── Compilation Pipeline ──────────────────────────────────────────────
// 2026-07-12: Phase 7 — Compile a Briev source file end-to-end.
// Pipeline: lex -> parse -> typecheck -> codegen -> output.
// 2026-07-14: Wire real LlvmBackend instead of stub codegen.
//             Add binary compilation via clang. Add --out / --optimize-budget flags.
// 2026-07-14: Plugin path — serialize to BEAST, run external plugins, deserialize.
// 2026-07-15: Phase 2 — Wire per-stage plugin dispatch into pipeline.
//             Front: on_ast after parse, Mid: on_ast after typecheck,
//             Post/Back: on_ir after codegen. Per-extension plugin selection
//             from config/targets.dbvl. System plugin discovery from
//             plugins/{front,mid,post,back}/.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use briev_compiler::backend::llvm::LlvmBackend;
use briev_compiler::ast::StageKind;
use briev_compiler::plugin::loader::extract_inline_stage_blocks;
use briev_compiler::plugin::PluginManager;
use briev_compiler::target::{BackendKind, get_extension};
use briev_compiler::type_universe::TypeUniverse;

/// Re-export the LLVM backend's TrgUnresolvedAction for CLI flag parsing.
/// 2026-07-15: Phase 7i — Defined in the backend to avoid circular deps.
pub use briev_compiler::backend::llvm::TrgUnresolvedAction;
// 2026-08-23 (Phase 10): frontend pipeline moved into the LIB so the sweep
// and library consumers run the real path.
pub use briev_compiler::pipeline::{
    check_data_source, check_source, compile_to_typed, BuildOptions, BeastFilter,
    BeastPosition, BeastStage,
};
use briev_compiler::pipeline::{
    build_plugin_manager, check_types, compile_view, effective_view_html,
    evaluate_pending_comptime, lex_for_path, load_target_config, parse,
    preprocess_source_for_path, resolve_bind_routes, resolve_comptime_refs,
    view_root_signals, CompiledView, ViewMountSpecs,
};

/// Pipeline stage at which to emit a BEAST snapshot or IR snapshot.
/// 2026-07-21: Expanded to granular stages matching the pipeline.
/// AST stages emit .beast files; IR stages emit .ir files.



/// Snapshot position relative to plugin execution.
/// 2026-07-23: Used by --emit-beast for pre/post plugin snapshots.


/// A BEAST snapshot filter: emit at a specific (stage, position) pair.
/// 2026-07-23: --emit-beast accepts stage.position (before/after) or plain stage (both).


/// Options parsed from the `briev-compiler build` CLI flags.

/// Compile a Briev source file: produce an executable binary (or `.ll` with `--llvm`).
/// 2026-07-25: Evaluate pending $let/$const compile-time variable initializers.
/// Called after both extract_inline_stage_blocks calls, before any stage blocks
/// execute. This ensures $let/$const values are available to all stage blocks.

/// 2026-07-25: Resolve comptime variable references in const initializers and
/// trg instance expressions. Replaces Expr::Identifier references to $let/$const
/// names with their evaluated NavValue literals before type checking / codegen.

/// 2026-07-25: Convert a NavValue to its corresponding Expr literal.


/// Normalize source text by file kind before lexing.
///
/// `.rbv` files carry `<view>`/`<style>` markup that must not reach the lexer.
/// The Briev parser consumes only extracted Briev source, while webstack output
/// still receives the extracted HTML/CSS payload.


/// 2026-08-11 (Phase 1 view wiring): compile the view markup with the
/// ViewCompiler — element IDs injected, b-* bindings extracted, directives
/// validated per SPEC 21.4 — and, for the `.s` strict profile, run the SRBV
/// view-state verification. Runs BEFORE codegen so the returned view-referenced
/// fields can protect state slots from dead-field elimination (the DOM consumes
/// them — observability-as-liveness).

/// 2026-08-11: the state fields a view actually references (root names of
/// directive signals). Cache-slots and dead-field elimination must keep them.

/// 2026-08-11 (Phase 2a2, SPEC 21.4): resolve `b-bind:value` input routes.
/// A field's route is the UNIQUE transaction whose write_set contains it (the
/// write-contract proof), and the JS marshalling category of that transaction's
/// SOLE parameter. Resolved from the transition graph — the same write sets
/// the webstack flush batch covers, so a resolved route is guaranteed to flush
/// back to the DOM. Returns field → Ok(route) or Err(reason):
/// - no writer → "no transaction writes '<field>'";
/// - multiple writers → "ambiguous — transactions ... write '<field>'";
/// - the sole writer takes no/several params → "transaction '<txn>' takes N
///   parameter(s); b-bind requires exactly one".


/// 2026-08-27 (cbv-HW plan Slice A): copy every `extern` cell's referenced
/// HDL source beside the output artifacts so verilator/Vivado link lines
/// resolve blackbox references without user setup. Missing file -> hard
/// error naming symbol + path.
pub fn copy_extern_companions(
    items: &[briev_compiler::ast::TopLevel],
    main_file: &str,
) -> Result<(), String> {
    let base_dir = std::path::Path::new(main_file)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let base_out = main_file.strip_suffix(".bv").unwrap_or(main_file);
    for item in items {
        let briev_compiler::ast::TopLevel::Cell(c) = item else { continue };
        let Some(rel) = &c.extern_source else { continue };
        let src_path: std::path::PathBuf = base_dir.to_path_buf().join(rel);
        if !src_path.exists() {
            return Err(format!(
                "extern cell '{}' references '{}' which does not exist — \
                 create the HDL source or fix the path",
                c.name, rel
            ));
        }
        let fname = rel.rsplit('/').next().unwrap_or(rel);
        let dest = format!("{}.extern.{}", base_out, fname);
        std::fs::copy(&src_path, &dest)
            .map_err(|e| format!("cannot copy '{}': {}", src_path.display(), e))?;
        eprintln!("copied extern source: {}", dest);
    }
    Ok(())
}

/// 2026-08-31 (plan abv-gpu-by-default B3): an Accelerator Briev Volume
/// (`.abv`) assumes GPU by default — the extension IS the accel intent, so
/// the module accel policy defaults to `try_all` with no `!> accel:`
/// metadata and no per-node `accel` keyword required. An explicit
/// `!> accel:` binding always wins (the default is only injected when no
/// module sets the key). Eligibility proofs still gate: a non-kernel-shaped
/// body is skipped with reasons, and a file with no eligible kernel errors
/// helpfully in the SPIR-V backend. Non-`.abv` sources are untouched.
/// To undo: delete this fn and its call site in compile_source.
fn apply_abv_accel_default(items: &mut Vec<briev_compiler::ast::TopLevel>, file_path: &str) {
    if get_extension(file_path) != ".abv"
        || items.iter().any(
            |i| matches!(i, briev_compiler::ast::TopLevel::ModuleMetadata(m) if m.contains_key("accel")),
        )
    {
        return;
    }
    // Insert into the LAST metadata node (merge semantics: last binding
    // wins), or append a new one when the module declares no metadata.
    let injection = || {
        let mut meta = std::collections::HashMap::new();
        meta.insert(
            "accel".into(),
            briev_compiler::ast::PropertyValue::Identifier("try_all".into()),
        );
        briev_compiler::ast::TopLevel::ModuleMetadata(meta)
    };
    match items
        .iter()
        .rposition(|i| matches!(i, briev_compiler::ast::TopLevel::ModuleMetadata(_)))
    {
        Some(idx) => {
            if let briev_compiler::ast::TopLevel::ModuleMetadata(m) = &mut items[idx] {
                m.insert(
                    "accel".into(),
                    briev_compiler::ast::PropertyValue::Identifier("try_all".into()),
                );
            }
        }
        None => items.push(injection()),
    }
}

pub fn compile_source(file_path: &str, source: &str, opts: &BuildOptions) -> Result<(), String> {
    // ── Macro lockfile handling ────────────────────────────────────
    // 2026-07-23: If --update-lockfile, regenerate macro-lock.toml from
    // current plugin files and --allow-* flags. Otherwise validate the
    // lockfile against loaded plugins and apply approved capabilities.
    let mut pm = build_plugin_manager(file_path, opts);
    let project_root = std::env::current_dir()
        .map_err(|e| format!("cannot determine project root: {}", e))?;
    let project_root_str = project_root.to_string_lossy().to_string();
    if opts.update_lockfile {
        let granted = briev_compiler::macros::lockfile::cli_granted_set(
            opts.allow_read,
            opts.allow_write,
            opts.allow_run,
            opts.allow_sys_query,
            opts.allow_net,
        );
        let lock = briev_compiler::macros::lockfile::generate_lockfile(&granted, None)?;
        briev_compiler::macros::lockfile::save_lockfile(&project_root_str, &lock)?;
    } else {
        if let Some(lock) = briev_compiler::macros::lockfile::load_lockfile(&project_root_str)? {
            briev_compiler::macros::lockfile::validate_and_apply(&lock, &mut pm, None)?;
        }
    }

    // ── Source normalization + PreLex transformation ─────────────────
    let preprocessed = preprocess_source_for_path(file_path, source)?;
    // 2026-08-11: clone — preprocessed (view_html/style_css) is consumed again
    // by the early view compilation; a partial move would forbid that borrow.
    let mut source = preprocessed.briev_source.clone();
    pm.run_source(StageKind::PreLex, &mut source)?;

    // ── Parse ─────────────────────────────────────────────────────────
    let tokens = lex_for_path(file_path, &source)?;
    let mut items = parse(file_path, &tokens, &source)?;

    // ── .abv assumes GPU (2026-08-31, plan abv-gpu-by-default B3) ────
    // The extension IS the accel intent; see apply_abv_accel_default.
    apply_abv_accel_default(&mut items, file_path);

    // Extract inline $(Stage) blocks from the AST — they are plugins,
    // not runtime code.
    extract_inline_stage_blocks(&mut items, &mut pm);

    // 2026-07-25: Evaluate $let/$const initializers before Parsed stage.
    {
        let mut eval_universe = TypeUniverse::new();
        evaluate_pending_comptime(&mut pm, &mut items, &mut eval_universe)?;
    }

    // 2026-07-23: Snapshot the program before any macro evaluation for --diff.
    let pre_macro_items = if opts.diff_mode {
        Some(items.clone())
    } else {
        None
    };

    // ── Parsed stage: AST transformation (before import resolution) ───
    {
        let mut parsed_universe = TypeUniverse::new();
        emit_beast_snapshot(file_path, BeastStage::Parse, BeastPosition::Before, &items, &TypeUniverse::new(), opts)?;
                pm.run_ast(StageKind::Parsed, &mut items, &mut parsed_universe)?;
    }

    // BEAST snapshot at Parse stage
    {
        let snapshot_universe = TypeUniverse::new();
        emit_beast_snapshot(file_path, BeastStage::Parse, BeastPosition::After, &items, &snapshot_universe, opts)?;
    }

    // 2026-08-04 (Phase 4, .ebv heap reframe): the embedded mode backend
    // emits int_to_str and the other cast-lane symbols directly as LLVM
    // define functions using the static bump arena (see mod.rs generate fn,
    // after the embedded_heap global). The compiler provides the runtime.
    // 2026-09-11 (Part B): bare targets (.b.bv) use the same .bv stdlib —
    // no separate .ebv stdlib variant remains.

    // ── Resolved stage (after import resolution) ──────────────────────
    let mut resolver = briev_compiler::import_resolver::ImportResolver::new();
    if let Some(ref stdlib_path) = opts.stdlib_path {
        resolver = resolver.with_stdlib_path(Some(std::path::PathBuf::from(stdlib_path)));
    }
    items = resolver.resolve_imports(items, &std::path::PathBuf::from(file_path))?;

    // 2026-07-24: Extract stage blocks from imported files. The first
    // extract_inline_stage_blocks ran before import resolution, so stage
    // blocks in imported modules were not captured.
    extract_inline_stage_blocks(&mut items, &mut pm);

    // 2026-07-25: Evaluate any new $let/$const from imported modules.
    {
        let mut eval_universe = TypeUniverse::new();
        evaluate_pending_comptime(&mut pm, &mut items, &mut eval_universe)?;
    }

    {
        emit_beast_snapshot(file_path, BeastStage::Resolve, BeastPosition::Before, &items, &TypeUniverse::new(), opts)?;
                pm.run_ast(StageKind::Resolved, &mut items, &mut TypeUniverse::new())?;
    }
    emit_beast_snapshot(file_path, BeastStage::Resolve, BeastPosition::After, &items, &TypeUniverse::new(), opts)?;

    // ── Type check ────────────────────────────────────────────────────
    // 2026-07-25: Resolve comptime var references in const initializers
    // and trg instance expressions before type checking.
    resolve_comptime_refs(&pm, &mut items)?;
    // 2026-09-20 (Front A, plan metaprogrammed-composites): expand
    // declared expression-parameterized composites before the multi-index
    // desugar and typecheck — ONE IR: every analysis and backend sees the
    // expanded body as if hand-written.
    briev_compiler::plugin::composite::expand_composites(&mut items, &pm)?;
    // 2026-09-17 (plan 2026-09-17-row-2d-index-desugar): `a[i, j]` markers
    // → 1D row-major arithmetic. MUST run before check_types (the marker is
    // a parse artifact, not a resolvable call) and before every analysis —
    // the shape detectors and backends only ever see plain Expr::Index.
    briev_compiler::analysis::desugar::rewrite_multi_index(&mut items)?;
    let mut universe = TypeUniverse::new();
    // 2026-09-14 (machine-entry plan): effective mechanism — CLI override,
    // else the target profile's row (keyed by triple, longest prefix).
    let check_isr_mechanism = opts.isr_mechanism.clone()
        .or_else(|| briev_compiler::config_tuning::target_settings_for(
            opts.triple_override.as_deref().unwrap_or("")).isr_mechanism)
        .or_else(|| briev_compiler::config_tuning::target_settings_for(
            &get_extension(&opts.file_path))
            .isr_mechanism.take())
        .or_else(|| {
            load_target_config(opts).lookup(&get_extension(&opts.file_path))
                .and_then(|e| e.target_triple.clone())
                .and_then(|t| briev_compiler::config_tuning::target_settings_for(&t).isr_mechanism)
        });
    check_types(&mut items, &universe, check_isr_mechanism.as_deref())?;
    // 2026-08-04: term termination diagnostics — unreachable code after a
    // terminating `term <value>`/`term! <value>` and the bare-term-guard
    // hint. Runs here (typed AST, pre-normalizer) so the backend never sees
    // a body whose semantics the interpreter would unwind early.
    {
        let (term_errors, term_warnings) = briev_compiler::analysis::termination::analyze(&items);
        for w in &term_warnings {
            eprintln!("warning: {w}");
        }
        if !term_errors.is_empty() {
            return Err(format!("termination errors:\n{}", term_errors.join("\n")));
        }
    }
    // 2026-08-22 (spec-conformance plan Phase 2): identifier casing advisory
    // (SPEC §4.1 — user-declared violations warn, never error).
    for w in briev_compiler::analysis::casing::analyze(&items) {
        eprintln!("warning: {w}");
    }
    // 2026-08-22 (spec-conformance plan Phase 9, SPEC §3.2): `.s` strict
    // profile — representation fallbacks become hard errors. The dotted-flag
    // forms only (`.s.bv`, `.s.rbv`); classify() already rejects compound
    // `.sbv`/`.srbv`. Proof obligations, trivial contracts, and concurrency
    // classification are global gates already; strict adds the memory-
    // decision tier. The trust-boundary report is written when strict passes.
    let strict_profile =
        briev_compiler::conformance::is_strict(std::path::Path::new(file_path));
    if strict_profile {
        let mc = briev_compiler::macros::memcheck::run_memcheck(&items);
        briev_compiler::analysis::strict::enforce(&items, &mc)?;
        let report = briev_compiler::analysis::strict::render_report(&items, &mc);
        let report_path = std::path::Path::new(file_path).with_extension("report.txt");
        if let Err(e) = std::fs::write(&report_path, &report) {
            return Err(format!(
                "cannot write the `.s` verification report to {}: {}",
                report_path.display(), e
            ));
        }
        eprintln!("[.s] verification report: {}", report_path.display());
    }
    // 2026-08-22 (spec-conformance plan Phase 8, SPEC §12.2): task-handle
    // linearity — every spawn handle consumed exactly once; `free` proves a
    // cancellation point in the spawned body.
    let task_errors = briev_compiler::analysis::task_linear::analyze(&items);
    if !task_errors.is_empty() {
        return Err(format!(
            "task handle errors:\n{}",
            task_errors.join("\n")
        ));
    }
    // 2026-08-07 (object instance pools): spawn pools must be predictably
    // inexhaustible — the spawn-count analysis rejects any spawn whose
    // multiplicity cannot be statically bounded (Briev has no runtime errors).
    {
        let (_, _, spawn_errors, _) = briev_compiler::analysis::spawn_pool::analyze(&items);
        if !spawn_errors.is_empty() {
            return Err(format!(
                "spawn pool errors:\n{}",
                spawn_errors.join("\n")
            ));
        }
    }
    // 2026-08-26 (async Phase C): compiled tasks carry args/results as i64
    // argv slots — a spawn target outside that ABI must fail at compile time,
    // never miscompile silently. LLVM-family backends only: the reference
    // interpreter (and `brievc check`) legitimately support Event<T>
    // parameters for blocking port reads (async Phase B).
    if matches!(opts.backend, BackendKind::Llvm | BackendKind::Webstack) {
        let abi_errors = briev_compiler::analysis::task_segments::collect_task_abi_errors(
            &items, &universe,
        );
        if !abi_errors.is_empty() {
            return Err(format!(
                "task ABI errors:\n{}",
                abi_errors.join("\n")
            ));
        }
    }
    // 2026-08-03: `+` is string concat for String/Blob operands — rewrite
    // BinaryOp(Add) → Concat on the typed AST so the backend dispatches the
    // concat emitter (String operands are boxed to i64 before the binary op).
    briev_compiler::analysis::string_concat::rewrite_plus_concat(&mut items, &universe);
    // 2026-08-03: same-category representation casts (`CStr as String`) become
    // the graph-resolved binding calls (cstr_to_briev / str_to_c) — Briev's
    // boxing loses the boundary type at codegen, so the marshalling decision
    // (the casting graph's minimal path) is made on the typed AST.
    briev_compiler::analysis::boundary_marshalling::rewrite_boundary_marshalling(&mut items, &universe);

    // ── Concurrency gate (Phase 3c, rule #21: no implicit concurrency) ──
    // Any pair of reactive txns that can fire together must be classified
    // (async on both, or sync<group> on both). Runs after typechecking so the
    // AST is stable; frontend-computed per the frontend-driven-dispatch pillar.
    // ── Causal DAG (2026-09-12, plan 2026-09-12-dynamics-causal-dag.md) ──
    // Liveness refusal: cyclic reactive components with no declared
    // completion. The graph itself rides on the context for the report
    // and future consumers.
    let causal = briev_compiler::analysis::causality::run(&items);
    if opts.explain_causality {
        for line in briev_compiler::analysis::causality::explain(&causal) {
            eprintln!("causality: {line}");
        }
    }
    if !causal.refusals.is_empty() {
        return Err(format!(
            "reactive liveness:\n  {}",
            causal.refusals.join("\n  ")
        ));
    }

    let gate_errors = briev_compiler::analysis::concurrency_gate::run_concurrency_gate(&items);
    if !gate_errors.is_empty() {
        return Err(format!(
            "concurrency gate:\n  {}",
            gate_errors.join("\n  ")
        ));
    }

    // ── Static when-law gate (2026-09-22, Slice C2) ──────────────────────
    // Software `when G { F }` at top level / in obj / in type bodies is a
    // forced fact the compiler must make hold everywhere. A member fact that
    // any node/txn body or another law contradicts under a jointly-
    // satisfiable guard is a refusal.
    let when_errors = briev_compiler::analysis::when_law::run_when_law_check(&items);
    if !when_errors.is_empty() {
        return Err(format!(
            "when-law gate:\n  {}",
            when_errors.join("\n  ")
        ));
    }

    // ── Typed stage: AST transformation (after type check) ────────────
    emit_beast_snapshot(file_path, BeastStage::TypeCheck, BeastPosition::Before, &items, &universe, opts)?;
    pm.run_ast(StageKind::Typed, &mut items, &mut universe)?;
    emit_beast_snapshot(file_path, BeastStage::TypeCheck, BeastPosition::After, &items, &universe, opts)?;

    // ── Normalizer pass ───────────────────────────────────────────────
    // 2026-07-29: Pass int_bits for protocol-driven llvm_type resolution.
    let int_bits = opts.int_bits;
    match opts.backend {
        BackendKind::Llvm | BackendKind::Gpu => {
            briev_compiler::backend::llvm::normalizer::normalize(&mut items, &mut universe, int_bits)?;
        }
        BackendKind::Circt => {
            briev_compiler::backend::circt::normalizer::normalize(&mut items, &mut universe, int_bits)?;
        }
        BackendKind::Electronics => {
            // Electronics backend has no normalization pass — AST is consumed directly.
        }
        BackendKind::Webstack => {
            // Webstack is always wasm32 (32-bit pointers)
            briev_compiler::backend::webstack::normalizer::normalize(&mut items, &mut universe, 32)?;
        }
        BackendKind::Spirv => {
            briev_compiler::backend::spirv::normalizer::normalize(&mut items, &mut universe, int_bits)?;
        }
        BackendKind::Ptx => {
            // The PTX tier reuses the SPIR-V normalizer + accel analysis:
            // same frontend-driven kernel selection, different emitter.
            briev_compiler::backend::spirv::normalizer::normalize(&mut items, &mut universe, int_bits)?;
        }
        BackendKind::Vm => {
            // 2026-08-10: VM is untyped but the universe must be populated
            // uniformly — minimal shared registration, nothing backend-specific.
            briev_compiler::backend::vm::normalizer::normalize(&mut items, &mut universe, int_bits)?;
        }
        BackendKind::Bad => {
            // 2026-09-21 (bad-dialect plan): .bad never enters the .bv
            // pipeline — pure assembly, own parser/backend. Compiled via
            // `brievc bad <file.bad>`; this arm exists only to keep the
            // BackendKind match exhaustive.
        }
    }

    emit_beast_snapshot(file_path, BeastStage::Normalize, BeastPosition::Before, &items, &universe, opts)?;
    pm.run_ast(StageKind::Normalized, &mut items, &mut universe)?;
    emit_beast_snapshot(file_path, BeastStage::Normalize, BeastPosition::After, &items, &universe, opts)?;

    // ── Build protocol graph from protocol declarations ────────────────
    // ── Protocol contract enforcement via SMT ──────────────────────────
    // 2026-07-23: For each protocol declaration with a contract, prove
    // the invariant holds using the SMT solver. If unprovable, deny.
    // Also validate that all CastTo/CastFrom have bindings.
    for item in &items {
        if let briev_compiler::ast::TopLevel::ProtocolDef(pd) = item {
            // Validate bindings exist on all CastTo/CastFrom edges
            for edge in &pd.cast_edges {
                if edge.binding.is_none() {
                    return Err(format!(
                        "protocol '{}': {} must have a binding (e.g., CastTo(#target) = fn(#L))",
                        pd.name,
                        match edge.direction {
                            briev_compiler::ast::top::CastDirection::CastTo => "CastTo",
                            briev_compiler::ast::top::CastDirection::CastFrom => "CastFrom",
                        }
                    ));
                }
            }
            // Validate contract if present
            if let Some(ref contract) = pd.contract {
                let pre = &contract.pre_condition;
                let post = &contract.post_condition;
                let params = vec![("Self".to_string(), briev_compiler::ast::Type::int())];
                if let Err(errs) = briev_compiler::proof_engine::prove_contract(pre, post, &params, contract.explicit) {
                    return Err(format!("protocol contract violation in '{}': {:?}", pd.name, errs));
                }
            }
            // 2026-07-23: Round-trip proof — CastFrom(CastTo(x)) == x
            if let Err(msg) = briev_compiler::analysis::protocol_graph::verify_protocol_roundtrip(pd, &items) {
                return Err(msg);
            }
            // 2026-07-23: Cross-op equivalence proof
            if let Err(msg) = briev_compiler::analysis::protocol_graph::verify_crossop_equivalence(pd, &items) {
                return Err(msg);
            }
        }
    }

    // ── frgn? guard safety check ────────────────────────────────────
    // 2026-07-25: Verify every frgn?/frgn!/frgn?! call is guarded by fn?.
    briev_compiler::analysis::frgn_guard::check_frgn_guards(&items)
        .map_err(|e| format!("frgn guard error:\n{}", e))?;

    // ── Tautology check (Phase 4) ─────────────────────────────────────
    // 2026-07-31: Reject functionally-always-true contracts at proof time.
    // `[true][true]` and `0 == 0` constrain nothing and provide no
    // optimization leverage. Parser stays permissive; proof is the gate.
    for item in &items {
        let contract: Option<&briev_compiler::ast::Contract> = match item {
            briev_compiler::ast::TopLevel::Transaction(t) => Some(&t.contract),
            briev_compiler::ast::TopLevel::Definition(d) => Some(&d.contract),
            _ => None,
        };
        if let Some(c) = contract {
            if let Some(err) = briev_compiler::proof_engine::detect_tautology(
                &c.pre_condition,
                &c.post_condition,
                c.explicit,
            ) {
                let name = match item {
                    briev_compiler::ast::TopLevel::Transaction(t) => t.name.clone(),
                    briev_compiler::ast::TopLevel::Definition(d) => d.name.clone(),
                    _ => "<unknown>".into(),
                };
                return Err(format!("tautological contract on '{}': {:?}", name, err));
            }
        }
    }

    // ── Watchdog contract checks (Phase C4) ──────────────────────────
    // 2026-08-01: wire the trigger->handler watchdog analysis into the
    // pipeline, and validate the `-> handler(val)` on-fire callback (the
    // handler must exist and be callable with the last computed value).
    let watchdog_errors = briev_compiler::analysis::watchdog::analyze(&items);
    if !watchdog_errors.is_empty() {
        let msgs: Vec<String> = watchdog_errors.iter().map(|e| e.to_string()).collect();
        return Err(format!("watchdog errors:\n{}", msgs.join("\n")));
    }
    briev_compiler::analysis::watchdog::check_on_fire_handlers(&items)
        .map_err(|e| format!("watchdog error:\n{}", e))?;

    // ── Protocol round-trip verification ──────────────────────────────
    briev_compiler::protocol_verify::verify_roundtrips(&items, &universe)?;

    emit_beast_snapshot(file_path, BeastStage::Verify, BeastPosition::Before, &items, &universe, opts)?;
    pm.run_ast(StageKind::Verified, &mut items, &mut universe)?;
    emit_beast_snapshot(file_path, BeastStage::Verify, BeastPosition::After, &items, &universe, opts)?;

    // ── Slice narrowing ───────────────────────────────────────────────
    // 2026-07-26: Convert constant-bounds Expr::Slice to direct array access.
    briev_compiler::analysis::narrow_slice::narrow_slices(&mut items);

    // ── Allocation strategy analysis ──────────────────────────────────
    let alloc_strategies = briev_compiler::analysis::allocation::analyze_alloc_strategies(&mut items);
    // 2026-07-27: Compute transitive arena need from the same allocation walk.
    // This determines which functions need the 64KB arena buffer. When empty,
    // arena fields in %State and all arena init/fini calls are skipped — saving
    // 64KB malloc and 24 bytes of %State for benchmarks with no Alloc# calls.
    let needs_arena = briev_compiler::analysis::allocation::analyze_arena_need(&mut items);
    emit_beast_snapshot(file_path, BeastStage::Alloc, BeastPosition::Before, &items, &universe, opts)?;
    pm.run_ast(StageKind::Allocated, &mut items, &mut universe)?;
    emit_beast_snapshot(file_path, BeastStage::Alloc, BeastPosition::After, &items, &universe, opts)?;

    // ── Dangling pointer detection ────────────────────────────────────
    // 2026-07-31: provenance warning → HARD compile error (memory-by-proof,
    // Phase D). The type system already rejects `&local` → Ptr<Int> escapes
    // (PtrConst), but this layer is the defense-in-depth: if a provenance gap
    // appears (a future pointer form that slips past PtrConst inference), the
    // program is denied at compile time instead of dereferencing a dead stack
    // address.
    use briev_compiler::analysis::provenance::{check_dangling_ptrs, collect_local_names};
    for item in &items {
        if let briev_compiler::ast::TopLevel::Transaction(txn) = item {
            let local_names = collect_local_names(&txn.body, &txn.parameters);
            let warnings = check_dangling_ptrs(&txn.body, &local_names);
            if !warnings.is_empty() {
                return Err(format!(
                    "dangling pointer error in '{}':\n{}",
                    txn.name,
                    warnings.join("\n")
                ));
            }
        }
    }

    emit_beast_snapshot(file_path, BeastStage::Provenance, BeastPosition::Before, &items, &universe, opts)?;
    pm.run_ast(StageKind::Provenanced, &mut items, &mut universe)?;
    emit_beast_snapshot(file_path, BeastStage::Provenance, BeastPosition::After, &items, &universe, opts)?;

    // 2026-07-16: P4 — Collect extra objects from ForeignBinding FromSpec paths
    // for linking into the final binary.
    let mut extra_objects = collect_extra_objects(&items, &resolver, briev_compiler::conformance::is_bare(std::path::Path::new(file_path)))?;

    // ── Frgn dispatch resolution ──────────────────────────────────────
    // 2026-07-22: Resolve each frgn declaration's dispatch strategy before
    // codegen. The backend receives the resolved strategies and does not
    // re-implement dispatch logic.
    let glue_targets = briev_compiler::glue::config::load_glue_config(
        opts.glue_config.as_deref().map(Path::new),
    )?;
    let mut resolved_frgns: std::collections::HashMap<
        String, briev_compiler::analysis::frgn_dispatch::ResolvedFrgn,
    > = std::collections::HashMap::new();
    for item in &items {
        let briev_compiler::ast::TopLevel::ForeignBinding(fb) = item else { continue; };
        let ext = fb.from.extension().unwrap_or_default();
        let dispatch = briev_compiler::analysis::frgn_dispatch::resolve_single_frgn(
            fb, &ext, &glue_targets, opts.backend, Some(&universe),
        )?;
        resolved_frgns.insert(fb.effective_briev_name().to_string(), dispatch);
    }

    // 2026-07-26: Collect protocol library names from resolved frgns
    // for passing as -l<lib> flags to clang during linking.
    let protocol_libs: Vec<String> = resolved_frgns.values().filter_map(|rf| {
        if let briev_compiler::analysis::frgn_dispatch::ResolvedFrgn::Inline { protocol_lib: Some(lib), .. } = rf {
            Some(lib.clone())
        } else {
            None
        }
    }).collect();

    // ── Layout optimization (frgn/export boundary) ─────────────────────
    // 2026-07-22: Propose adopting foreign type layouts to minimize
    // protocol transform costs. Only applies to bridge-path frgns.
    // This is additive — removing this pass does not affect correctness.
    let layout_changes = briev_compiler::analysis::layout_optimizer::optimize_layouts(
        &items, &universe, &resolved_frgns, &glue_targets,
    )?;
    for change in &layout_changes {
        briev_compiler::analysis::layout_optimizer::apply_layout_change(&mut items, change)?;
    }
    if !layout_changes.is_empty() {
        eprintln!("layout optimizer: {} change(s) applied", layout_changes.len());
    }

    // ── Diff mode / dry-run ─────────────────────────────────────────────
    // 2026-07-23: If --diff was specified, show what macros changed and exit
    // before codegen/writing. No output file is produced.
    if let Some(ref pre_macro) = pre_macro_items {
        let diff = briev_compiler::macros::diff::compute_diff(pre_macro, &items);
        if diff.is_empty() {
            println!("(no changes)");
        } else {
            println!("\n=== Macro Changes ({} change(s)) ===", diff.len());
            briev_compiler::macros::diff::print_diff(&diff);
            println!("=== End Macro Changes ===");
        }
        return Ok(());
    }

    // ── Derivation assertion verification (Phase B.0) ──────────────────
    // 2026-07-28: For every definition/txn that has BOTH a body and a
    // derivation block, evaluate each example through the interpreter and
    // compare to expected output. A mismatch is a fatal build error.
    {
        let mut interp = briev_compiler::interpreter::Interpreter::new();
        interp.load_program(&items);
        if let Err(errors) = briev_compiler::derive::verify_derivation_assertions(&items, &mut interp) {
            for e in &errors {
                eprintln!("error: derivation assertion: {}", e);
            }
            return Err("derivation assertion failed".to_string());
        }
    }

    // ── View compilation (webstack) ────────────────────────────────────
    // 2026-08-11 (Phase 1 view wiring): compile the view late enough that the
    // program is type-checked (SRBV is meaningful) but BEFORE codegen, so the
    // view-referenced fields protect their %State slots from dead-field
    // elimination. Output block below reuses the cached result.
    // 2026-08-11 (2b2 slice 2a): expand component instances first — each
    // `<Name />` mount gains its own instance-qualified state slots and txn
    // variants; the per-mount fragments drive the view compiler.
    let mut component_specs: std::collections::HashMap<
        String,
        Vec<briev_compiler::analysis::component_instances::MountSpec>,
    > = std::collections::HashMap::new();
    let mut component_initializers: std::collections::HashMap<
        String,
        briev_compiler::ast::Expr,
    > = std::collections::HashMap::new();
    let mut instance_specs: std::collections::HashMap<
        String,
        briev_compiler::analysis::component_instances::MountSpec,
    > = std::collections::HashMap::new();
    if opts.backend == BackendKind::Webstack {
        let view_html = effective_view_html(opts, &preprocessed, &items).unwrap_or_default();
        match briev_compiler::analysis::component_instances::expand_component_instances(
            &mut items,
            &view_html,
        ) {
            Ok(plan) => {
                component_specs = plan.mounts;
                component_initializers = plan.initializers;
                instance_specs = plan.instance_specs;
            }
            Err(msg) => return Err(format!("{}: component instance error: {}", file_path, msg)),
        }
    }
    let compiled_view: CompiledView = if opts.backend == BackendKind::Webstack {
        compile_view(file_path, &items, opts, &preprocessed, &ViewMountSpecs {
            pools: component_specs.clone(),
            instances: instance_specs.clone(),
        })?
    } else {
        CompiledView {
            bindings: Vec::new(),
            modified_html: None,
            collection_iterables: std::collections::HashSet::new(),
            collection_string_iterables: std::collections::HashSet::new(),
            warnings: Vec::new(),
        }
    };
    let view_warnings = compiled_view.warnings.clone();
    let view_bindings = compiled_view.bindings.clone();
    let modified_view_html = compiled_view.modified_html.clone();
    let collection_iterables = compiled_view.collection_iterables.clone();
    let collection_string_iterables = compiled_view.collection_string_iterables.clone();
    let view_signals = view_root_signals(&view_bindings);

    // ── Code generation ───────────────────────────────────────────────
    // 2026-07-23: Check if any glue target requests native module init.
    let enable_module_init = glue_targets.values().any(|t| t.module_init);

    // 2026-08-10: real state layout captured from the webstack codegen path,
    // consumed by the GlueWebGenerator below (falls back to the hardcoded
    // stub when no webstack codegen ran, e.g. --emit-ir-only).
    let mut web_layout: Option<briev_compiler::glue::web_generator::StateLayout> = None;
    // 2026-08-11 (Phase 2a2): b-bind routes resolved during codegen from the
    // transition graph; surfaced here so unresolvable routes are hard errors.
    let mut bind_routes: Option<std::collections::HashMap<
        String,
        Result<briev_compiler::glue::web_generator::BindRoute, String>,
    >> = None;

    let (codegen_output, ext) = codegen(&items, &mut universe, &pm, opts, alloc_strategies, needs_arena, resolved_frgns, enable_module_init, &mut web_layout, &view_signals, &collection_iterables, &mut bind_routes, &component_initializers)?;

    // BEAST/IR snapshot at Codegen stage
    emit_beast_snapshot(file_path, BeastStage::Codegen, BeastPosition::After, &items, &universe, opts)?;

    // ── Generated stage: IR text manipulation ──────────────────────────
    let mut output = codegen_output;
    pm.run_ir(StageKind::Generated, &mut output)?;

    // ── Write output ──────────────────────────────────────────────────
    let out_path = determine_out_path(file_path, opts.out_dir.as_deref())?;
    let out_path = out_path.replace(".ll", ext);

    // 2026-07-15: SPIR-V writes inside codegen (binary format), skip outer write
    // 2026-07-25: Vm backend also writes inside codegen (.lair is binary)
    // 2026-09-08: PTX writes inside codegen (.ptx is text but artifacts are
    // managed by the arm, like the .spv path).
    if opts.backend != BackendKind::Spirv && opts.backend != BackendKind::Vm && opts.backend != BackendKind::Ptx {
        if let Some(parent) = std::path::Path::new(&out_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create output dir '{}': {}", parent.display(), e))?;
            }
        }
        std::fs::write(&out_path, &output)
            .map_err(|e| format!("cannot write '{}': {}", out_path, e))?;
        println!("wrote {}", out_path);
    }

    // ── Optimized stage: final IR validation ──────────────────────────
    pm.run_ir(StageKind::Optimized, &mut output)?;
    emit_beast_snapshot(file_path, BeastStage::Optimize, BeastPosition::After, &items, &universe, opts)?;

    // 2026-09-21: Compile `bad fn` bodies through the bad backend and
    // collect their .o files for linking. Each bad fn body is a standalone
    // .bad program compiled per-target; the LLVM IR references the symbols
    // via `declare`.
    let bad_triple = opts.triple_override.clone()
        .or_else(|| load_target_config(opts)
            .lookup(&get_extension(&opts.file_path))
            .and_then(|e| e.target_triple.clone()))
        .unwrap_or_else(|| "x86_64-unknown-linux-gnu".to_string());
    let bad_fn_objects = compile_bad_fn_objects(&items, &bad_triple)?;
    extra_objects.extend(bad_fn_objects);

    if !opts.emit_ir_only {
        let binary_base = out_path.strip_suffix(ext).unwrap_or(&out_path);
        // 2026-08-03: --library — package a static .a (+ PIC .so) instead of
        // a linked executable. The archive bundles the bridge .o and the
        // briev_rt runtime so a host links `-l<name>` standalone.
        if opts.library_mode && opts.backend == BackendKind::Llvm {
            // Merge CLI-provided extra_objects with ones collected from frgn
            // declarations (frgn .c/.cpp sources are auto-compiled to .o).
            let mut all_objects = opts.extra_objects.clone();
            all_objects.extend(extra_objects);
            all_objects.sort();
            all_objects.dedup();
            compile_ll_to_library(&out_path, binary_base, &all_objects)?;
            return Ok(());
        }
        let binary_path = if opts.shared {
            format!("{}.so", binary_base)
        } else {
            binary_base.to_string()
        };
        if opts.backend == BackendKind::Llvm || opts.backend == BackendKind::Gpu {
            // Merge CLI-provided extra_objects with ones collected from frgn
            // declarations (frgn .c/.cpp sources are auto-compiled to .o).
            // 2026-07-26: Deduplicate — multiple frgns may reference the same
            // .c source (e.g., briev_rt.c), producing identical cached .o paths.
            let mut all_objects = opts.extra_objects.clone();
            all_objects.extend(extra_objects);
            // 2026-09-21 (Family K): the accel orchestration is the Rust
            // staticlib (src/accel_rt.rs, built by build.rs alongside the
            // driver archive); --gc-sections drops it when the program has
            // no accel kernels.
            let accel_lib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/compiler-in-briv/libbriev_accel_rt.a");
            let driver_lib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/compiler-in-briv/libbriev_gpu_rt.a");
            if accel_lib.exists() && driver_lib.exists() {
                all_objects.push(accel_lib.clone());
                all_objects.push(driver_lib.clone());
            } else {
                // Headless bootstrap (no cc/rustc for the drivers): the
                // program still links; accel launches no-op to CPU.
                eprintln!("[briev] accel runtime archives not built — CPU lane only");
            }
            all_objects.sort();
            all_objects.dedup();
            compile_ll_to_binary(&out_path, &binary_path, &all_objects, &protocol_libs, opts.shared)?;
        }
        // 2026-07-26: Phase 5 — Compile LLVM IR to WASM binary for webstack backend.
        // Uses llc to compile the .ll (emitted with wasm32 target triple) to .wasm.
        // Skips C runtime linking — WASM modules are self-contained pure logic.
        if opts.backend == BackendKind::Webstack {
            let wasm_path = format!("{}.wasm", binary_base);
            // 2026-08-11 (Phase 2a3 fix): wasm-ld exports NOTHING by default
            // (`--no-entry` + no `--export`) — the generated module exported
            // only `memory`. The shim calls `exports.state_layout()` and
            // `exports["<txn>"]()` on flush/trigger/bind, all of which were
            // undefined → the page never initialized. Export every
            // transaction/definition (the reactive entry points) + the
            // state_layout table the shim reads at init.
            let mut exports: Vec<String> = items.iter()
                .filter_map(|item| match item {
                    // 2026-08-11 (Phase 2a3): a callable txn emits `@<name>`; a
                    // reactive txn emits `@txn_<name>`. Export both forms —
                    // wasm-ld ignores names without a matching symbol, and the
                    // shim's `_txn()` resolver tries both.
                    briev_compiler::ast::TopLevel::Transaction(t) => {
                        Some(vec![t.name.clone(), format!("txn_{}", t.name)])
                    }
                    briev_compiler::ast::TopLevel::Definition(d) => {
                        Some(vec![d.name.clone(), format!("txn_{}", d.name)])
                    }
                    _ => None,
                })
                .flatten()
                .collect();
            exports.push("state_layout".to_string());
            // 2026-08-12 (Iterable protocol, slice 4): the b-each snapshot
            // materializers — `__view_items_<field>()` per collection iterable.
            for field in &collection_iterables {
                exports.push(format!("__view_items_{}", field));
            }
            // 2026-08-12 (Iterable protocol, slice 4): the state-pointer + boot
            // + render-frame exports — the shim passes __briev_state_ptr() to
            // every txn export and ticks render_frame each frame.
            exports.push("__briev_state_ptr".to_string());
            exports.push("__web_boot".to_string());
            exports.push("render_frame".to_string());
            exports.sort_unstable();
            exports.dedup();
            compile_wasm(&out_path, &wasm_path, &exports)?;

            // 2026-07-26: Phase 6b — Write app.css from <style> block content.
            let style_css = opts.style_css.as_ref().or(preprocessed.style_css.as_ref());
            if let Some(css) = style_css {
                let css_path = format!("{}.css", binary_base);
                std::fs::write(&css_path, css)
                    .map_err(|e| format!("cannot write '{}': {}", css_path, e))?;
                println!("wrote {}", css_path);
            }

            // 2026-08-11 (Phase 1 view wiring): the view was compiled BEFORE
            // codegen — bindings + ID-injected HTML cached in view_bindings /
            // modified_view_html / view_warnings (see the webstack arm above).
            // The injected IDs are load-bearing: the dom-shim's
            // getElementById(el) calls resolve against the MODIFIED html,
            // never the raw markup.

            // 2026-07-26: Phase 6b — Write index.html from the compiled view.
            // Wraps the ID-injected HTML in a minimal HTML5 boilerplate that
            // links app.css and loads dom-shim.mjs via ES module import.
            if let Some(html) = modified_view_html.as_ref() {
                let index_path = format!("{}.html", binary_base);
                let index_content = format!(
                    "<!DOCTYPE html>\n\
                     <html lang=\"en\">\n\
                     <head>\n\
                     <meta charset=\"UTF-8\">\n\
                     <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n\
                     <link rel=\"stylesheet\" href=\"app.css\">\n\
                     <script type=\"module\" src=\"dom-shim.mjs\"></script>\n\
                     </head>\n\
                     <body>\n\
                     {}\n\
                     <script type=\"module\">\n\
                     import {{ createApp }} from './dom-shim.mjs';\n\
                     fetch('{}.wasm').then(r => r.arrayBuffer())\n\
                       .then(bytes => createApp(new Uint8Array(bytes)));\n\
                     </script>\n\
                     </body>\n\
                     </html>\n",
                    html,
                    binary_base,
                );
                std::fs::write(&index_path, &index_content)
                    .map_err(|e| format!("cannot write '{}': {}", index_path, e))?;
                println!("wrote {}", index_path);

                // 2026-07-26: Item 3 — SSR pass. If --ssr is set, replace
                // the standard app.html with an SSR-enabled version that
                // embeds initial state as JSON and pre-renders the view.
                if opts.ssr {
                    let ssr_out = briev_compiler::ssr::render_ssr(
                        html,
                        &items,
                        style_css.map(|s| s.as_str()),
                        binary_base,
                        opts.dev,
                    );
                    std::fs::write(&index_path, &ssr_out.full_html)
                        .map_err(|e| format!("cannot write SSRed '{}': {}", index_path, e))?;
                    println!("ssr {}", index_path);
                }
            }
            for w in &view_warnings {
                eprintln!("warning: {}", w);
            }

            // 2026-08-11 (Phase 2a2, SPEC 21.4): every `b-bind:value` must
            // resolve to exactly one writer transaction (the write-contract
            // proof). Unresolvable routes are hard errors, never inert inputs.
            {
                let routes = bind_routes.as_ref();
                for binding in &view_bindings {
                    if let briev_compiler::view_compiler::Directive::Bind { target } =
                        &binding.directive
                    {
                        let (root, _) = briev_compiler::view_compiler::root_signal(target);
                        let resolution = routes
                            .and_then(|r| r.get(root))
                            .cloned()
                            .unwrap_or_else(|| {
                                Err(format!(
                                    "no transaction writes '{}' — b-bind:value needs a proven write contract (SPEC 21.4)",
                                    root
                                ))
                            });
                        if let Err(reason) = resolution {
                            return Err(format!(
                                "{}: b-bind:value=\"{}\": {}",
                                file_path, target, reason
                            ));
                        }
                    }
                }
            }

            // 2026-07-26: Phase 6c — Generate dom-shim.mjs + .d.ts from frgn decls.
            let frgn_decls: Vec<briev_compiler::ast::ForeignBinding> = items.iter()
                .filter_map(|item| {
                    if let briev_compiler::ast::TopLevel::ForeignBinding(fb) = item {
                        if matches!(fb.from, briev_compiler::ast::FromSpec::Protocol(ref p) if p == "#Web") {
                            Some(fb.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();
            if !frgn_decls.is_empty() || !view_bindings.is_empty() {
                // 2026-08-10: use the real layout captured from the webstack
                // codegen path when available; fall back to the historical
                // hardcoded stub (empty fields) for paths that skipped codegen.
                let state_layout = web_layout.clone().unwrap_or_else(|| {
                    briev_compiler::glue::web_generator::StateLayout {
                        app_name: binary_base.to_string(),
                        generation_offset: 0,
                        flush_buffer_offset: 64,
                        max_flush_entries: 16,
                        fields: vec![],
                    }
                });
                // 2026-08-11 (Phase 2a2): unwrap the resolved b-bind routes for
                // the generator — the Ok entries are the wired inputs; Err
                // entries were already rejected above as hard errors.
                let resolved_routes: std::collections::HashMap<_, _> = bind_routes
                    .iter()
                    .flat_map(|r| r.iter())
                    .filter_map(|(field, res)| res.clone().ok().map(|route| (field.clone(), route)))
                    .collect();
                let web_gen = briev_compiler::glue::web_generator::GlueWebGenerator::new(
                    Vec::new(), // wasm bytes not needed for stub generation
                    view_bindings.clone(),
                    state_layout,
                    HashMap::new(),
                    frgn_decls,
                )
                .with_bind_routes(resolved_routes)
                .with_collection_iterables(collection_iterables.clone())
                .with_collection_string_iterables(collection_string_iterables.clone());
                match web_gen.generate() {
                    Ok(output) => {
                        let mjs_path = format!("{}.mjs", binary_base);
                        std::fs::write(&mjs_path, &output.dom_shim)
                            .map_err(|e| format!("cannot write '{}': {}", mjs_path, e))?;
                        println!("wrote {}", mjs_path);
                        let dts_path = format!("{}.d.ts", binary_base);
                        std::fs::write(&dts_path, &output.dts)
                            .map_err(|e| format!("cannot write '{}': {}", dts_path, e))?;
                        println!("wrote {}", dts_path);
                    }
                    Err(e) => {
                        return Err(format!("GlueWebGenerator failed: {}", e));
                    }
                }
            }
        }

        // ── Linked stage: binary processing ───────────────────────────
        let bin_path = std::path::Path::new(&binary_path);
        pm.run_bin(bin_path)?;
    }

    // ── VFS dump / flush ────────────────────────────────────────────
    // 2026-07-23: If --dump-vfs was specified, print virtual filesystem contents.
    if opts.dump_vfs && !pm.vfs.is_empty() {
        println!("\n=== Virtual Filesystem Contents ===");
        let mut sorted: Vec<&String> = pm.vfs.keys().collect();
        sorted.sort();
        for path in &sorted {
            let content = &pm.vfs[*path];
            println!("  {} ({} bytes)", path, content.len());
            if let Some(first_line) = content.lines().next() {
                let preview = if first_line.len() > 80 { &first_line[..77] } else { first_line };
                println!("    -> {}", preview);
            }
        }
        println!("=== End VFS ===");
    }

    // ── Expansion traces dump ─────────────────────────────────────────
    // 2026-07-23: If --dump-traces was specified, print macro expansion traces.
    if opts.dump_traces && !pm.expansion_traces.is_empty() {
        println!("\n=== Macro Expansion Traces ===");
        let mut sorted: Vec<(usize, String)> = pm.expansion_traces.iter()
            .map(|(k, v)| (*k, v.clone())).collect();
        sorted.sort_by_key(|(k, _)| *k);
        for (idx, desc) in &sorted {
            println!("  [{}] {}", idx, desc);
        }
        println!("=== End Expansion Traces ===");
    }

    Ok(())
}

/// Type-check only: don't generate code.

/// 2026-08-09 (Phase 13, SPEC 22.6): `briev check file.dbv|file.dbvl` — parse a
/// Data Briev document and validate it against its asserted schemas. A `.dbvl`
/// file uses the line-oriented parser (offsets tracked); `.dbv` the structured
/// parser. The document is not a Briev program — it never enters the .bv
/// pipeline.

/// Build the plugin manager for a given file and opts.
/// 2026-07-15: Phase 2 — Discovers system plugins, applies per-extension
/// filtering, and applies CLI overrides. The caller then runs stages at
/// the appropriate pipeline points.

/// Code generation: dispatch to the selected backend, run Post/Back
/// plugin IR stages, and return (output_text, extension).
/// 2026-07-15: Phase 2 — Extracted from compile_source for flat flow.
fn codegen(
    items: &[briev_compiler::ast::TopLevel],
    universe: &mut TypeUniverse,
    _pm: &PluginManager,
    opts: &BuildOptions,
    alloc_strategies: std::collections::HashMap<usize, briev_compiler::backend::llvm::AllocStrategy>,
    needs_arena: std::collections::HashSet<String>,
    resolved_frgns: std::collections::HashMap<String, briev_compiler::analysis::frgn_dispatch::ResolvedFrgn>,
    enable_module_init: bool,
    web_layout: &mut Option<briev_compiler::glue::web_generator::StateLayout>,
    view_signals: &std::collections::HashSet<String>,
    collection_iterables: &std::collections::HashSet<String>,
    bind_routes: &mut Option<std::collections::HashMap<
        String,
        Result<briev_compiler::glue::web_generator::BindRoute, String>,
    >>,
    component_initializers: &std::collections::HashMap<String, briev_compiler::ast::Expr>,
) -> Result<(String, &'static str), String> {
    // 2026-07-20: Extract operator definitions from AST for backend dispatch.
    let mut operator_defs: std::collections::HashMap<String, Vec<briev_compiler::ast::top::OperatorDef>> = std::collections::HashMap::new();
    let mut cast_from_bit_overrides: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for item in items.iter() {
        if let briev_compiler::ast::TopLevel::TypeDef(td) = item {
            let mut all_ops = td.body.operators.clone();
            // 2026-08-15 (coll plan §3.4): a `coll` type gets the default
            // construction/mutation op bindings synthesized — `op InitEmpty`/
            // `op Init` (literal + `let` construction), `op InsertAt` (`<-`
            // push), `op ExtractFrom`/`op CopyFrom` (pop/read). The member
            // bodies are synthesized into obj_members by the LLVM backend
            // (coll_scaffold); here we wire the bindings the dispatch paths
            // consult. Only added when the type doesn't declare its own
            // binding for the same op (a user override wins).
            let mut coll_bindings: Vec<briev_compiler::ast::top::OperatorBinding> = Vec::new();
            if td.coll {
                for (op, impl_name, arg_form) in [
                    ("InitEmpty", "init_empty", "Lh"),
                    ("Init", "init", "Lh,Rh"),
                    ("InsertAt", "push", "Lh,Rh"),
                    ("ExtractFrom", "pop", "Rh"),
                    ("CopyFrom", "get", "Rh"),
                ] {
                    let already = td.body.op_bindings.iter().any(|b| b.name == op);
                    if already {
                        continue;
                    }
                    let args: Vec<briev_compiler::ast::Expr> = arg_form
                        .split(',')
                        .map(|h| {
                            briev_compiler::ast::Expr::Identifier(format!("#{}", h))
                        })
                        .collect();
                    coll_bindings.push(briev_compiler::ast::top::OperatorBinding {
                        name: op.to_string(),
                        protocol_variant: None,
                        pre: None,
                        suf: None,
                        reg: None,
                        expr: briev_compiler::ast::Expr::Call(
                            impl_name.to_string(),
                            args,
                            None,
                        ),
                        trusted_axiom: false,
                        span: None,
                    });
                }
            }
            let bindings_iter = td.body.op_bindings.iter().chain(coll_bindings.iter());
            // 2026-07-30: Convert op_bindings (new-style) to OperatorDef format.
            // CastFrom(Bit) goes to the casting graph (sole user-extensible cast edge).
            // CastTo(Bit) is banned (hardcoded representation guarantee).
            // Other CastTo/CastFrom remain in operator_defs as type-level lane overrides.
            for b in bindings_iter {
                let pv = b.protocol_variant.as_deref().unwrap_or("");
                let is_bit_target = pv == "Bit";

                if b.name == "CastTo" && is_bit_target {
                    return Err(format!(
                        "CastTo(Bit) is hardcoded on type '{}' — \
                         use x as Bit or Cast#(x, target) for bitcasts. \
                         CastTo(Bit) is a compiler-guaranteed mechanical operation \
                         (bitcast/extractvalue/ptrtoint) and cannot be overridden.",
                        td.name
                    ));
                }

                if b.name == "CastFrom" && is_bit_target {
                    // Register in casting graph as the sole user-extensible cast edge
                    if let briev_compiler::ast::Expr::Call(fn_name, _, _) = &b.expr {
                        cast_from_bit_overrides.insert(td.name.clone(), fn_name.clone());
                    }
                    continue; // skip operator_defs — handled by casting graph
                }

                // 2026-07-31 (A6/A7): every op-binding reaches operator_defs —
                // CastTo/CastFrom (type-level lane overrides) AND the
                // collection op bindings (InsertAt / ExtractFrom / Init). The
                // '<-' dispatch and `op Init` construction look these up.
                let params = match &b.protocol_variant {
                    Some(pv) if pv.starts_with('#') => {
                        vec![briev_compiler::ast::Type::Custom(pv.clone())]
                    }
                    Some(pv) => vec![briev_compiler::ast::Type::Custom(pv.clone())],
                    None => vec![],
                };
                let impl_args = if let briev_compiler::ast::Expr::Call(fn_name, _, _) = &b.expr {
                    Some(briev_compiler::ast::PropertyValue::Identifier(fn_name.clone()))
                } else {
                    None
                };
                all_ops.push(briev_compiler::ast::top::OperatorDef {
                    op: b.name.clone(),
                    params,
                    pre: b.pre.clone(),
                    suf: b.suf.clone(),
                    impl_args,
                    impl_name: b.name.clone(),
                    trusted_axiom: false,
                    span: b.span.clone(),
                });
            }
            if !all_ops.is_empty() {
                operator_defs.insert(td.name.clone(), all_ops);
            }
        }
    }

    // ── Shared frontend analysis (Plan 0.1, 2026-08-23) ────────────────
    // Computed ONCE here so every backend consumes identical frontend
    // decisions (frontend-driven-dispatch pillar) instead of re-deriving
    // them per backend. The tuning triple replicates each backend's own
    // triple resolution (LLVM/Gpu default x86_64-unknown-linux-gnu from
    // CompilerContext::new(); Webstack forces wasm32-unknown-wasi; a
    // config/targets.dbvl entry overrides either) so the vector-phi gate
    // matches what the backend would have derived itself.
    // To undo: delete this block, restore per-backend analyze_program calls
    // (LLVM: llvm/mod.rs generate(); CIRCT: circt/mod.rs generate()).
    let default_triple = match opts.backend {
        BackendKind::Webstack => "wasm32-unknown-wasi",
        _ => "x86_64-unknown-linux-gnu",
    };
    let tuning_triple = opts.triple_override.clone()
        .or_else(|| load_target_config(opts)
            .lookup(&get_extension(&opts.file_path))
            .and_then(|e| e.target_triple.clone()))
        .unwrap_or_else(|| default_triple.to_string());
    // 2026-09-14 (machine-entry plan): the effective ISR mechanism — CLI
    // override, else the target profile's row (the inference source for
    // mechanism-less `node @ vector` declarations).
    let effective_isr_mechanism = opts.isr_mechanism.clone()
        .or_else(|| briev_compiler::config_tuning::target_settings_for(&tuning_triple).isr_mechanism);
    let analysis = briev_compiler::backend::analyze_program(
        items,
        false,
        briev_compiler::config_tuning::target_settings_for(&tuning_triple).vector_min_width,
        Some(&*universe),
    );

    // ── Capability validation (Plan 0.2, 2026-08-23) ───────────────────
    // Backends with a partial surface declare it (backend::*::CAPABILITIES);
    // programs reaching beyond are rejected here with what/why/fix instead
    // of silently dropping constructs mid-codegen (the old CIRCT `None`
    // fallbacks / VM trap-on-drop behavior). LLVM has the full surface and
    // skips this gate.
    if matches!(
        opts.backend,
        BackendKind::Llvm | BackendKind::Circt | BackendKind::Electronics | BackendKind::Spirv | BackendKind::Vm | BackendKind::Ptx
    ) {
        let caps = match opts.backend {
            // 2026-08-22 (Phase 7c): LLVM joins the gate — its surface is
            // full EXCEPT the staged port/cell execution.
            BackendKind::Llvm => briev_compiler::backend::llvm::CAPABILITIES,
            BackendKind::Circt => briev_compiler::backend::circt::CirctBackend::CAPABILITIES,
            // 2026-09-11 (Part C): Electronics — the strictest surface in the
            // family: closed component universe, static netlist, no runtime.
            BackendKind::Electronics => briev_compiler::backend::electronics::CAPABILITIES,
            BackendKind::Spirv | BackendKind::Ptx => briev_compiler::backend::spirv::CAPABILITIES,
            _ => briev_compiler::backend::vm::CAPABILITIES,
        };
        // 2026-08-31 (plan abv-gpu-by-default): the SPIR-V gate is
        // KERNEL-SCOPED — the .spv contains only the eligible accel bodies'
        // proven kernel_stmts (plus state/metadata surface), so exactly
        // those validate against the kernel capability table. The module's
        // remaining items — stdlib defns the prelude injects, host-only
        // transactions — never reach the backend and must not fail its
        // surface gate. Other backends lower whole programs: full scope.
        // 2026-09-08: PTX shares this — same frontend kernel selection.
        let cap_errors = if opts.backend == BackendKind::Spirv || opts.backend == BackendKind::Ptx {
            let kernel_items: Vec<briev_compiler::ast::TopLevel> = items
                .iter()
                .filter_map(|item| match item {
                    briev_compiler::ast::TopLevel::StateDecl(_)
                    | briev_compiler::ast::TopLevel::ModuleMetadata(_) => Some(item.clone()),
                    briev_compiler::ast::TopLevel::Transaction(t) => {
                        let entry = analysis.accel.get(&t.name)?;
                        if !entry.shape.eligible {
                            return None;
                        }
                        let mut kernel_txn = t.clone();
                        kernel_txn.body = entry.shape.kernel_stmts.clone();
                        Some(briev_compiler::ast::TopLevel::Transaction(kernel_txn))
                    }
                    _ => None,
                })
                .collect();
            briev_compiler::backend::capabilities::validate_program(&kernel_items, &caps)
        } else {
            briev_compiler::backend::capabilities::validate_program(items, &caps)
        };
        if !cap_errors.is_empty() {
            return Err(cap_errors.join("\n"));
        }
    }

    let output;
    let ext: &str = match opts.backend {
        BackendKind::Llvm => {
            let mut b = LlvmBackend::new()
                .with_int_bits(opts.int_bits)
                .with_alloc_strategies(alloc_strategies)
                .with_needs_arena(needs_arena.clone())
                .with_shared_lib(opts.shared)
                .with_library_mode(opts.library_mode)
                .with_force_emit_all(opts.keep_all_defns)
                .with_stack_threshold(opts.stack_threshold)
                .with_accel_cpu_fallback(opts.accel_cpu_fallback)
                .with_optimize_budget(opts.optimize_budget)
                .with_type_universe(universe.clone())
                .with_operator_defs(operator_defs)
                .with_cast_from_bit_overrides(cast_from_bit_overrides)
                .with_resolved_frgns(resolved_frgns.clone())
                .with_trg_unresolved_action(opts.trg_unresolved_action)
                .with_module_init(enable_module_init)
                // 2026-08-23 (Plan 0.1): pipeline-computed analysis, shared
                // across backends — see the block above the dispatch.
                .with_analysis(analysis);
            // Apply target config if available
            let ext = get_extension(&opts.file_path);
            // 2026-08-04 (Phase 4): a bare target (.b.bv) activates the
            // restricted embedded mode (check_embedded_restrictions, term! ->
            // wfi) — the freestanding bare-metal path.
            if briev_compiler::conformance::is_bare(std::path::Path::new(&opts.file_path)) {
                b = b.with_embedded_mode(true);
            }
            // 2026-09-06 (ISR plan): the profile's ISR mechanism — the
            // configured default for mechanism-less `isr` declarations.
            if let Some(ref mech) = effective_isr_mechanism {
                b = b.with_isr_mechanism(Some(mech.clone()));
            }
            let target_config = load_target_config(opts);
            if let Some(entry) = target_config.lookup(&ext) {
                if let Some(ref triple) = entry.target_triple {
                    b = b.with_target_triple(triple);
                }
                if let Some(ref dl) = entry.data_layout {
                    b = b.with_data_layout(dl);
                }
                // 2026-09-13 (rv64 capability kernel): linker script from target profile.
                if entry.linker_script.is_some() {
                    b = b.with_linker_script(entry.linker_script.clone());
                }
            }
            // 2026-09-13 (rv64 capability kernel): CLI overrides for triple
            // and linker script take precedence over dbvl settings.
            if let Some(ref triple) = opts.triple_override {
                b = b.with_target_triple(triple);
            }
            if opts.linker_script_override.is_some() {
                b = b.with_linker_script(opts.linker_script_override.clone());
            }
            // Register proto declarations on the casting graph
            if let Some(ref mut graph) = b.ctx.casting_graph {
                for item in items.iter() {
                    if let briev_compiler::ast::TopLevel::ProtocolDef(pd) = item {
                        graph.register_protocol_def(pd);
                    }
                }
                // 2026-08-03 (P1.5): prove cross-type inverse pairs
                // (b.CastFrom(base)(a.CastTo(base)(x)) == x) so the delta
                // collapse in find_path can make them zero-cost.
                graph.register_inverse_pairs_from(items);
            }
            output = b.generate(items, None);
            // 2026-08-01: surface the backend's warnings (redundant-keep hints,
            // GPU-info, target-triple notes) — they were test-only.
            for w in b.warnings() {
                eprintln!("{}", w);
            }
            // 2026-08-10: capture the real state layout (field names + handles)
            // so the JS shim can map view bindings to state fields.
            let stem = std::path::Path::new(&opts.file_path)
                .file_stem().map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "app".to_string());
            *web_layout = Some(b.web_state_layout(&stem));
            ".ll"
        }
        BackendKind::Webstack => {
            // 2026-07-26: Phase 4 — Webstack uses LlvmBackend(wasm32) + with_webstack().
            // The old TS emitter path is deprecated. Phase 6 will also invoke
            // GlueWebGenerator to produce the JS shim from view bindings.
            // Phase 5: Extension is .ll — compile_wasm will produce .wasm from it.
            let mut b = LlvmBackend::new()
                .with_webstack(true)
                .with_int_bits(32)
                .with_target_triple("wasm32-unknown-wasi")
                .with_type_universe(universe.clone())
                .with_alloc_strategies(alloc_strategies)
                .with_needs_arena(needs_arena)
                .with_stack_threshold(opts.stack_threshold)
                .with_accel_cpu_fallback(opts.accel_cpu_fallback)
                .with_optimize_budget(opts.optimize_budget)
                .with_operator_defs(operator_defs)
                .with_cast_from_bit_overrides(cast_from_bit_overrides)
                .with_resolved_frgns(resolved_frgns.clone())
                .with_trg_unresolved_action(opts.trg_unresolved_action)
                .with_module_init(enable_module_init)
                .with_component_initializers(component_initializers.clone())
                // 2026-08-23 (Plan 0.1): pipeline-computed analysis, shared
                // across backends — see the block above the dispatch.
                .with_analysis(analysis);
            // 2026-08-11 (view wiring): view-bound fields are observability —
            // the DOM consumes them, so dead-field elimination must keep them.
            b.ctx.view_bound_fields = view_signals.clone();
            b.ctx.collection_iterables = collection_iterables.clone();
            // Apply target config if available
            let ext = get_extension(&opts.file_path);
            // 2026-08-04 (Phase 4): a bare target (.b.bv) activates the
            // restricted embedded mode (check_embedded_restrictions, term! ->
            // wfi) — the freestanding bare-metal path.
            if briev_compiler::conformance::is_bare(std::path::Path::new(&opts.file_path)) {
                b = b.with_embedded_mode(true);
            }
            // 2026-09-06 (ISR plan): the profile's ISR mechanism — the
            // configured default for mechanism-less `isr` declarations.
            if let Some(ref mech) = effective_isr_mechanism {
                b = b.with_isr_mechanism(Some(mech.clone()));
            }
            let target_config = load_target_config(opts);
            if let Some(entry) = target_config.lookup(&ext) {
                if let Some(ref triple) = entry.target_triple {
                    b = b.with_target_triple(triple);
                }
                if let Some(ref dl) = entry.data_layout {
                    b = b.with_data_layout(dl);
                }
            }
            output = b.generate(items, None);
            // 2026-08-01: surface the backend's warnings (redundant-keep hints,
            // GPU-info, target-triple notes) — they were test-only.
            for w in b.warnings() {
                eprintln!("{}", w);
            }
            // 2026-08-10: capture the real state layout (field names + handles)
            // so the JS shim can map view bindings to state fields.
            let stem = std::path::Path::new(&opts.file_path)
                .file_stem().map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "app".to_string());
            *web_layout = Some(b.web_state_layout(&stem));
            // 2026-08-11 (Phase 2a2): resolve `b-bind:value` input routes from
            // the transition-graph write sets (the SAME source the flush batch
            // covers) — a field's route is the UNIQUE transaction that writes
            // it. Compile_source surfaces unresolvable routes (zero / ambiguous
            // writers, wrong arity) as hard errors.
            *bind_routes = Some(resolve_bind_routes(&b.ctx.transition_graph, items, universe));
            ".ll"
        }
        BackendKind::Circt => {
            let mut b = briev_compiler::backend::circt::CirctBackend::new()
                // 2026-08-23 (Plan 3.1): normalized universe for rule-19
                // type lowering (protocol categories, never names).
                .with_universe(universe.clone());
            // 2026-08-23 (Plan 0.1): consume the shared dependency graph from
            // the pipeline analysis instead of re-deriving it.
            output = b.generate_with_dep_graph_universe(items, &analysis.dependency_graph, universe);
            // 2026-08-23 (Plan 3.3): recorded unsupported constructs are hard
            // errors — hardware targets never silently drop logic.
            let errs = b.errors.borrow().clone();
            if !errs.is_empty() {
                return Err(errs.join("\n"));
            }
            // 2026-08-25 (seq-firmem plan): memory macros ship with reference
            // implementation companions next to the .mlir — SeqToSV lowers
            // firmem to an EXTERNALLY-generated module, and the harness links
            // these at verilator/Vivado time.
            for (fname, content) in b.memory_companions() {
                let base = opts.file_path.strip_suffix(".bv").unwrap_or(&opts.file_path);
                let path = format!("{}.{}", base, fname);
                std::fs::write(&path, &content)
                    .map_err(|e| format!("cannot write '{}': {}", path, e))?;
                eprintln!("wrote memory companion: {}", path);
            }
            // 2026-08-27 (cbv-HW plan Slice A): ship foreign HDL sources next
            // to the .mlir like firmem companions; missing source errors.
            copy_extern_companions(items, &opts.file_path)?;
            // THE aggregated disambiguation note (one per compile; explicit
            // pins silence their arrays).
            if let Some(note) = b.take_disambiguation_note() {
                eprintln!("{}", note);
            }
            ".mlir"
        }
        BackendKind::Electronics => {
            // 2026-09-11 (Part C, Electronics skeleton): the frontend-derived
            // netlist (analysis.electronics) is emitted as a KiCad 7
            // schematic. Dangling pins fail the compile — no partial boards.
            match briev_compiler::backend::electronics::ElectronicsBackend::generate(&analysis.electronics) {
                Ok(sch) => output = sch,
                Err(errs) => return Err(errs.join("\n")),
            }
            // 2026-09-23 (fab plan): a `fab` section asks for a board — the
            // .kicad_pcb is written as a companion to the schematic.
            match briev_compiler::backend::electronics::ElectronicsBackend::generate_board(
                &analysis.electronics,
                items,
            ) {
                Ok(Some(board)) => {
                    let pcb_path = determine_out_path(&opts.file_path, opts.out_dir.as_deref())?
                        .replace(".ll", ".kicad_pcb");
                    if let Some(parent) = std::path::Path::new(&pcb_path).parent() {
                        if !parent.as_os_str().is_empty() {
                            std::fs::create_dir_all(parent).map_err(|e| {
                                format!("cannot create output dir '{}': {}", parent.display(), e)
                            })?;
                        }
                    }
                    std::fs::write(&pcb_path, &board)
                        .map_err(|e| format!("cannot write '{}': {}", pcb_path, e))?;
                    println!("wrote {}", pcb_path);
                }
                Ok(None) => {}
                Err(errs) => return Err(errs.join("\n")),
            }
            ".kicad_sch"
        }
        BackendKind::Gpu => {
            let mut b = LlvmBackend::new()
                .with_int_bits(opts.int_bits)
                .with_alloc_strategies(alloc_strategies)
                .with_shared_lib(opts.shared)
                .with_library_mode(opts.library_mode)
                .with_force_emit_all(opts.keep_all_defns)
                .with_stack_threshold(opts.stack_threshold)
                .with_accel_cpu_fallback(opts.accel_cpu_fallback)
                .with_optimize_budget(opts.optimize_budget)
                .with_type_universe(universe.clone())
                .with_resolved_frgns(resolved_frgns)
                .with_trg_unresolved_action(opts.trg_unresolved_action)
                // 2026-08-23 (Plan 0.1): pipeline-computed analysis, shared
                // across backends — see the block above the dispatch.
                .with_analysis(analysis);
            // Apply target config (same logic as Llvm)
            let ext = get_extension(&opts.file_path);
            // 2026-08-04 (Phase 4): a bare target (.b.bv) activates the
            // restricted embedded mode (check_embedded_restrictions, term! ->
            // wfi) — the freestanding bare-metal path.
            if briev_compiler::conformance::is_bare(std::path::Path::new(&opts.file_path)) {
                b = b.with_embedded_mode(true);
            }
            // 2026-09-06 (ISR plan): the profile's ISR mechanism — the
            // configured default for mechanism-less `isr` declarations.
            if let Some(ref mech) = effective_isr_mechanism {
                b = b.with_isr_mechanism(Some(mech.clone()));
            }
            let target_config = load_target_config(opts);
            if let Some(entry) = target_config.lookup(&ext) {
                if let Some(ref triple) = entry.target_triple {
                    b = b.with_target_triple(triple);
                }
                if let Some(ref dl) = entry.data_layout {
                    b = b.with_data_layout(dl);
                }
            }
            // Register proto declarations on the casting graph
            if let Some(ref mut graph) = b.ctx.casting_graph {
                for item in items.iter() {
                    if let briev_compiler::ast::TopLevel::ProtocolDef(pd) = item {
                        graph.register_protocol_def(pd);
                    }
                }
                // 2026-08-03 (P1.5): prove cross-type inverse pairs
                // (b.CastFrom(base)(a.CastTo(base)(x)) == x) so the delta
                // collapse in find_path can make them zero-cost.
                graph.register_inverse_pairs_from(items);
            }
            output = b.generate(items, None);
            // 2026-08-01: surface the backend's warnings (redundant-keep hints,
            // GPU-info, target-triple notes) — they were test-only.
            for w in b.warnings() {
                eprintln!("{}", w);
            }
            ".ll"
        }
        BackendKind::Spirv => {
            // 2026-07-15: SPIR-V backend compiles kernels to binary
            // 2026-08-23 (§2.2): frontend-driven kernel selection via the shared
            // accel analysis. "main" = wildcard entry (all eligible kernels);
            // a specific txn name validates presence once §9.7 dispatch
            // metadata lands.
            // 2026-08-26 (§2.4): the normalized universe feeds scalar type
            // resolution in the kernel emitter.
            // 2026-09-02: ONE .spv PER KERNEL (the abv-gpu-by-default
            // doctrine; runner::build_kernels is the single source of
            // blobs). The old path emitted every eligible kernel named
            // "main" into ONE module — a SPIR-V spec violation (entry
            // points must be unique per (name, execution mode)); multi-
            // kernel .abv files never produced a valid artifact. A single-
            // kernel program's file is byte-identical to the old artifact
            // (same emit_kernel inputs). Entries stay "main": the device
            // drivers hardcode pName "main" — the runner path and the file
            // artifacts must never disagree.
            let reuse_map = if briev_compiler::config_tuning::ir_lowering()
                .gpu_schedule_buffer_reuse
            {
                analysis.gpu_schedule.reuse_map()
            } else {
                std::collections::HashMap::new()
            };
            let mut kernels = briev_compiler::backend::spirv::runner::build_kernels(
                items,
                universe,
                opts.int_bits,
                &analysis,
                Some(&reuse_map),
            )?;
            // 2026-09-17 (CyberLlama plan M2.0): dual-image kernels — merge
            // the PTX tier's blobs into the SPIR-V kernels by node name so
            // ONE runner serves both device lanes (the runtime's per-driver
            // blob selection: cuda consumes desc.ptx, vulkan desc.spirv).
            // Best-effort: shapes without a PTX lowering keep their Vulkan
            // image only (per-kernel CPU fallback on the CUDA lane).
            match briev_compiler::backend::ptx::build_ptx_kernels(
                items,
                universe,
                opts.int_bits,
                &analysis.accel,
                &analysis.gpu_schedule,
            ) {
                Ok(ptx_kernels) => {
                    for k in &mut kernels {
                        if let Some(p) = ptx_kernels.iter().find(|p| p.name == k.name) {
                            k.ptx = p.spirv.clone();
                            // 2026-09-18 (P1 lane-coverage fix): propagate
                            // the lane-reduction dispatch flag from the PTX
                            // emitter to the merged runner kernel.
                            if p.block_per_workitem {
                                k.block_per_workitem = true;
                            }
                            // 2026-09-19 (M1 warp-sliced reductions, plan
                            // general-machinery): the desc's block_threads
                            // is what the CUDA lane launches with, and the
                            // CUDA image is the PTX blob — take the PTX
                            // emitter's geometry (warp-sliced kernels: 128
                            // threads = 4 warp slices; the Vulkan lane
                            // parses LocalSize and ignores this field).
                            k.block_threads = p.block_threads;
                        }
                    }
                }
                Err(e) => {
                    println!("note: PTX images unavailable for this program ({e}); CUDA lane skipped");
                }
            }
            let out = determine_out_path(&opts.file_path, opts.out_dir.as_deref())?;
            let out_path = out.replace(".ll", ".spv");
            if kernels.len() == 1 {
                std::fs::write(&out_path, &kernels[0].spirv)
                    .map_err(|e| format!("cannot write '{}': {}", out_path, e))?;
                println!("wrote {}", out_path);
            } else {
                let stem = std::path::Path::new(&out_path)
                    .with_extension("")
                    .to_string_lossy()
                    .to_string();
                for k in &kernels {
                    let p = format!("{}_{}.spv", stem, k.name);
                    std::fs::write(&p, &k.spirv)
                        .map_err(|e| format!("cannot write '{}': {}", p, e))?;
                    println!("wrote {}", p);
                }
            }
            // 2026-08-31 (plan abv-gpu-by-default item 4): .abv is PURE GPU
            // code — emit the standalone runner (a self-contained C program
            // embedding one .spv per kernel + the reactive scheduler). The
            // user compiles it with any C compiler; `brievc run x.abv` does
            // it automatically.
            // `kernels` was built above (the .spv artifacts' single source);
            // `brievc run x.abv` (Track A) drives the linked GPU runtime
            // in-process — no runner .c file, no cc round trip.
            if opts.run {
                let prog = briev_compiler::backend::spirv::runner::prepare_run(
                    items,
                    universe,
                    opts.int_bits,
                    &kernels,
                    Some(&analysis.gpu_schedule),
                )?;
                let counters = briev_compiler::gpu_rt::run_program(&prog)?;
                for (k, c) in kernels.iter().zip(counters.iter()) {
                    println!("[run] node '{}' finished: counter = {}", k.name, c);
                }
                output = String::new();
                return Ok((output, ".spv"));
            }
            let runner =
                briev_compiler::backend::spirv::runner::emit_runner(items, universe, opts.int_bits, &kernels, Some(&analysis.gpu_schedule))?;
            let runner_path = out_path.replace(".spv", "_runner.c");
            std::fs::write(&runner_path, &runner)
                .map_err(|e| format!("cannot write '{}': {}", runner_path, e))?;
            // The runner #includes the runtime (single TU) — copy the
            // runtime AND its device drivers beside it.
            let rt_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("lib/runtime");
            let rt_dir_out = std::path::Path::new(&runner_path)
                .parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(std::path::PathBuf::new);
            // 2026-09-21 (Family K): the header + the Rust-built
            // orchestration archive + the driver archive. Runner cc line:
            //   cc ... runner.c -I. -L. -lbriev_accel_rt -lbriev_gpu_rt -ldl -lpthread -lm
            let briv_out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/compiler-in-briv");
            let rt_files = [
                ("briev_accel_rt.h", rt_dir.as_path()),
                ("libbriev_accel_rt.a", briv_out.as_path()),
                ("libbriev_gpu_rt.a", briv_out.as_path()),
            ];
            for (rt_file, src_dir) in rt_files {
                let dest = rt_dir_out.join(rt_file);
                std::fs::copy(src_dir.join(rt_file), &dest).map_err(|e| {
                    format!("cannot copy runtime '{}' to '{}': {}", rt_file, dest.display(), e)
                })?;
            }
            println!("wrote {}", runner_path);
            output = String::new();
            ".spv"
        }
        BackendKind::Ptx => {
            // 2026-09-08 (plan 2026-09-08-ptx-tier-execution S2a): the PTX
            // tier emits PTX TEXT blobs for the same frontend-selected
            // kernels the SPIR-V backend lowers (same accel analysis +
            // normalizer). The blob rides the identical `RunnerKernel` shape
            // and `emit_runner`/`prepare_run` dispatch — the CUDA driver
            // JITs the PTX via cuModuleLoadData. S2a surface: GEMM-shaped
            // kernels only (see build_ptx_kernels' error).
            let kernels = briev_compiler::backend::ptx::build_ptx_kernels(
                items,
                universe,
                opts.int_bits,
                &analysis.accel,
                &analysis.gpu_schedule,
            )?;
            let out = determine_out_path(&opts.file_path, opts.out_dir.as_deref())?;
            let out_path = out.replace(".ll", ".ptx");
            if kernels.len() == 1 {
                std::fs::write(&out_path, &kernels[0].spirv)
                    .map_err(|e| format!("cannot write '{}': {}", out_path, e))?;
                println!("wrote {}", out_path);
            } else {
                let stem = std::path::Path::new(&out_path)
                    .with_extension("")
                    .to_string_lossy()
                    .to_string();
                for k in &kernels {
                    let p = format!("{}_{}.ptx", stem, k.name);
                    std::fs::write(&p, &k.spirv)
                        .map_err(|e| format!("cannot write '{}': {}", p, e))?;
                    println!("wrote {}", p);
                }
            }
            let runner =
                briev_compiler::backend::spirv::runner::emit_runner(items, universe, opts.int_bits, &kernels, Some(&analysis.gpu_schedule))?;
            let runner_path = out_path.replace(".ptx", "_runner.c");
            std::fs::write(&runner_path, &runner)
                .map_err(|e| format!("cannot write '{}': {}", runner_path, e))?;
            let rt_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("lib/runtime");
            let rt_dir_out = std::path::Path::new(&runner_path)
                .parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(std::path::PathBuf::new);
            // 2026-09-21 (Family K): the header + the Rust-built
            // orchestration archive + the driver archive. Runner cc line:
            //   cc ... runner.c -I. -L. -lbriev_accel_rt -lbriev_gpu_rt -ldl -lpthread -lm
            let briv_out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/compiler-in-briv");
            let rt_files = [
                ("briev_accel_rt.h", rt_dir.as_path()),
                ("libbriev_accel_rt.a", briv_out.as_path()),
                ("libbriev_gpu_rt.a", briv_out.as_path()),
            ];
            for (rt_file, src_dir) in rt_files {
                let dest = rt_dir_out.join(rt_file);
                std::fs::copy(src_dir.join(rt_file), &dest).map_err(|e| {
                    format!("cannot copy runtime '{}' to '{}': {}", rt_file, dest.display(), e)
                })?;
            }
            println!("wrote {}", runner_path);
            output = String::new();
            ".ptx"
        }
        BackendKind::Bad => {
            // 2026-09-21 (bad-dialect plan): .bad never reaches here — the
            // `brievc bad <file.bad>` entry short-circuits before the .bv
            // pipeline (pure assembly, own parser/backend). This arm keeps
            // the dispatch exhaustive; reaching it is a routing bug.
            return Err(
                "bad: route .bad programs through `brievc bad <file.bad>` - they do not \
                 enter the .bv pipeline"
                    .to_string(),
            );
        }
        BackendKind::Vm => {
            // 2026-07-25: VM backend emits .lair bytecode
            let mut b = briev_compiler::backend::vm::VmBackend::new();
            let lair_data = b.generate(items, universe);
            // 2026-08-23 (Plan 0.2): constructs outside the VM surface that
            // slipped past validation became traps — surface them as the
            // compile errors they are instead of shipping silent wrongness.
            if !b.errors.is_empty() {
                return Err(b.errors.join("\n"));
            }
            let out = determine_out_path(&opts.file_path, opts.out_dir.as_deref())?;
            let out_path = out.replace(".ll", ".lair");
            std::fs::write(&out_path, &lair_data)
                .map_err(|e| format!("cannot write '{}': {}", out_path, e))?;
            println!("wrote {}", out_path);
            output = String::new();
            ".lair"
        }
    };

    Ok((output, ext))
}

/// Write a BEAST snapshot at the given pipeline stage and position.
fn emit_beast_snapshot(
    file_path: &str,
    stage: BeastStage,
    position: BeastPosition,
    items: &[briev_compiler::ast::TopLevel],
    universe: &TypeUniverse,
    opts: &BuildOptions,
) -> Result<(), String> {
    // Check if this (stage, position) pair is requested
    let is_requested = opts.emit_beast_stages.iter().any(|f| {
        f.stage == stage && (f.position.is_none() || f.position == Some(position))
    });
    if !is_requested {
        return Ok(());
    }
    let (stage_name, is_ast) = match stage {
        BeastStage::Parse => ("parse", true),
        BeastStage::Resolve => ("resolve", true),
        BeastStage::TypeCheck => ("types", true),
        BeastStage::Normalize => ("normal", true),
        BeastStage::Verify => ("verify", true),
        BeastStage::Alloc => ("alloc", true),
        BeastStage::Provenance => ("prov", true),
        BeastStage::Codegen => ("codegen", false),
        BeastStage::Optimize => ("opt", false),
    };
    let ext = if is_ast { "beast" } else { "ir" };
    let data = briev_compiler::beast::to_beast(items, universe);
    let base = file_path.strip_suffix(".bv").unwrap_or(file_path);
    let priority = position.priority();
    let path = format!("{}.{}.{:03}.{}", base, stage_name, priority, ext);
    std::fs::write(&path, &data)
        .map_err(|e| format!("cannot write '{}': {}", path, e))?;
    eprintln!("wrote {} snapshot: {}", ext, path);
    Ok(())
}

/// Compile a source file up to the $(Typed) stage.
/// Returns items and universe at the Typed stage, ready for beastpack serialization.
/// Used by `brievc bounty` — additive new function, no existing paths modified.

/// Determine the output `.ll` file path from the input path and optional output directory.
fn determine_out_path(file_path: &str, out_dir: Option<&str>) -> Result<String, String> {
    let p = Path::new(file_path);
    let base = p.file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("cannot determine output path from '{}'", file_path))?;

    let parent = match out_dir {
        Some(dir) => dir.trim_end_matches('/').to_string(),
        None => p.parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string()),
    };

    Ok(format!("{}/{}.ll", parent, base))
}

/// 2026-09-21: Compile `bad fn` bodies through the bad backend.
/// Each BadFn's body is a standalone .bad program compiled for the
/// target triple; the resulting .o files are linked into the binary.
fn compile_bad_fn_objects(
    items: &[briev_compiler::ast::TopLevel],
    target_triple: &str,
) -> Result<Vec<PathBuf>, String> {
    use briev_compiler::ast::top::TopLevel;
    let mut objects = Vec::new();
    let family = target_triple.split('-').next().unwrap_or(target_triple);
    let cache_dir = get_ffi_cache_dir();
    // Register parameter bindings per target family.
    let (_, regs) = briev_compiler::backend::bad::registries();
    let abi_args = regs.abi_args(family);
    let abi_args_fp = regs.abi_args_fp(family);

    for item in items {
        let bf = match item {
            TopLevel::BadFn(bf) => bf,
            _ => continue,
        };
        // Build param_env: each .bv param name → Bound::Token(register).
        let mut param_env = std::collections::HashMap::new();
        let mut r_idx = 0usize;
        let mut f_idx = 0usize;
        for (_pname, pty) in &bf.params {
            let type_name = pty.to_string();
            let is_float = type_name.starts_with("Float") || type_name.starts_with("F32")
                || type_name.starts_with("F64") || type_name == "Double";
            let reg = if is_float {
                let r = abi_args_fp.get(f_idx).cloned().unwrap_or_else(|| {
                    format!("f{f_idx}")
                });
                f_idx += 1;
                r
            } else {
                let r = abi_args.get(r_idx).cloned().unwrap_or_else(|| {
                    format!("r{r_idx}")
                });
                r_idx += 1;
                r
            };
            param_env.insert(
                _pname.clone(),
                briev_compiler::backend::bad::lower::Bound::Token(reg),
            );
        }
        // Parse + lower the .bad body.
        let asm = briev_compiler::backend::bad::generate_bad_fn(
            &bf.body,
            target_triple,
            param_env,
        )
        .map_err(|e| format!("bad fn `{}`: {}", bf.name, e))?;
        // Assemble to .o.
        let o_path = cache_dir.join(format!("{}_{}.o", bf.name, family));
        briev_compiler::backend::bad::assemble(&asm, family, &o_path)
            .map_err(|e| format!("bad fn `{}` assemble: {}", bf.name, e))?;
        objects.push(o_path);
    }
    Ok(objects)
}

/// 2026-07-16: P4 — Collect extra object files from ForeignBinding FromSpec paths.
/// Each frgn declaration with a .c/.so/.a/etc. path triggers compilation or direct
/// inclusion. The resolver is used to resolve compiler-relative <name> paths.
fn collect_extra_objects(items: &[briev_compiler::ast::TopLevel], resolver: &briev_compiler::import_resolver::ImportResolver, skip_briev_rt: bool) -> Result<Vec<PathBuf>, String> {
    let cache_dir = get_ffi_cache_dir();
    let mut objects = Vec::new();
    for item in items {
        let fb = match item {
            briev_compiler::ast::TopLevel::ForeignBinding(fb) => fb,
            _ => continue,
        };
        let ext = fb.from.extension();
        // 2026-08-04 (Phase 4, .ebv heap reframe): for .ebv freestanding
        // targets, skip briev_rt.c — the .ebv stdlib provides the symbols
        // (int_to_str, etc.) as Briev defns over the static bump arena.
        if skip_briev_rt && ext.as_deref() == Some("c") {
            let from_str = fb.from.as_str();
            if from_str.contains("briev_rt") || from_str.contains("lib/runtime") {
                continue;
            }
        }
        // 2026-07-26: Check registry directory first for <name> lookups,
        // then fall back to stdlib path, then use the name as a direct path.
        let resolved_path = || -> PathBuf {
            let from_str = fb.from.as_str();
            // Check registry for CompilerRegistry entries (<name>)
            if let briev_compiler::ast::top::FromSpec::CompilerRegistry(_) = &fb.from {
                if let Some(reg_path) = briev_compiler::registry::find_registry_entry(&from_str) {
                    return reg_path;
                }
            }
            resolver.resolve_stdlib_relative_path(&from_str)
                .unwrap_or_else(|| PathBuf::from(from_str))
        };
        match ext.as_deref() {
            Some("c") | Some("cpp") | Some("cc") | Some("cxx") | Some("m") => {
                let src = resolved_path();
                let obj = compile_source_to_object(&src, &cache_dir)?;
                objects.push(obj);
            }
            Some("so") | Some("dylib") | Some("a") | Some("o") => {
                objects.push(resolved_path());
            }
            _ => {}
        }
    }
    Ok(objects)
}

/// 2026-07-16: P4 — Get or create the FFI object cache directory.
fn get_ffi_cache_dir() -> PathBuf {
    let base = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("briev-compiler")
        .join("ffi");
    std::fs::create_dir_all(&base).ok();
    base
}

/// 2026-07-16: P4 — Compile a C/C++ source to a .o object file.
/// Content-hash cached at ~/.cache/briev-compiler/ffi/<hash>.o.
fn compile_source_to_object(source_path: &Path, cache_dir: &Path) -> Result<PathBuf, String> {
    let content = std::fs::read(source_path)
        .map_err(|e| format!("cannot read '{}': {}", source_path.display(), e))?;
    // 2026-07-26: Include compiler flags in the cache key so flag changes
    // (e.g. -flto) produce fresh cache entries instead of reusing stale ones.
    let mut hasher = blake3::Hasher::new();
    hasher.update(&content);
    hasher.update(b":flto:fPIC");
    let hash = hasher.finalize();
    let cache_path = cache_dir.join(format!("{}.o", hash.to_hex()));
    if cache_path.exists() {
        return Ok(cache_path);
    }
    let ext = source_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let lang_flag = match ext {
        "c" | "m" => "c",
        "cpp" | "cc" | "cxx" => "c++",
        _ => return Err(format!("unknown source extension '{}' for '{}'", ext, source_path.display())),
    };
    let status = Command::new("clang")
        .args([
            "-O3", "-flto", "-march=native", "-ffast-math", "-fPIC",
            "-x", lang_flag,
            "-c",
            source_path.to_str().unwrap(),
            "-o", cache_path.to_str().unwrap(),
        ])
        .status()
        .map_err(|e| format!("failed to invoke clang (is it installed?): {}", e))?;
    if !status.success() {
        return Err(format!("clang failed to compile '{}'", source_path.display()));
    }
    Ok(cache_path)
}

/// Compile a `.ll` file to a binary using clang.
///
/// 2026-07-26: Added `protocol_libs` parameter — library names from
/// `from #System` frgns are passed as `-l<lib>` flags to clang.
fn compile_ll_to_binary(ll_path: &str, binary_path: &str, extra_objects: &[PathBuf], protocol_libs: &[String], shared: bool) -> Result<(), String> {
    let ll_text = std::fs::read_to_string(ll_path)
        .map_err(|e| format!("cannot read '{}': {}", ll_path, e))?;
    // Extract target triple from the IR (first line: target triple = "..."`)
    let triple = ll_text.lines()
        .find(|l| l.starts_with("target triple"))
        .and_then(|l| {
            let start = l.find('"')?.checked_add(1)?;
            let end = l.rfind('"')?;
            Some(l[start..end].to_string())
        })
        .unwrap_or_else(|| "x86_64-unknown-linux-gnu".to_string());
    let mut cmd = Command::new("clang");
    // 2026-09-13 (rv64 capability kernel): cross-compilation support.
    // For non-native triples, pass --target=<triple> so clang selects the
    // correct assembler/linker and sysroot. For freestanding triples (non-linux),
    // pass -ffreestanding (implies -nobuiltininc, freestanding libc semantics).
    if !triple.contains("linux") || triple.contains("unknown") {
        cmd.arg(format!("--target={}", triple));
    }
    // 2026-07-26: briev_rt.c is no longer hardcoded here — frgn declarations in
    // stdlib (e.g., `frgn __print_int from "lib/runtime/briev_rt.c"`) are compiled
    // by collect_extra_objects and passed via extra_objects. This removes the
    // duplicate symbol error that occurred when briev_rt.c was compiled twice.
    if shared {
        cmd.args(["-O3", "-flto", "-shared", "-fPIC", ll_path]);
    } else {
        cmd.arg("-O3");
        // 2026-09-13: skip -march=native for cross-compilation targets
        // (e.g. riscv64-unknown-none) where it's invalid.
        let is_native = triple.starts_with("x86_64") || triple.contains("linux");
        if is_native {
            cmd.arg("-march=native");
            cmd.arg("-flto");
        }
        cmd.args(["-ffast-math", ll_path]);
    }
    // 2026-09-10 (Family F): a program whose IR carries the backend-owned
    // `_start` (module asm) and references no runtime objects is FREESTANDING
    // — link -nostdlib (no crt1, no libc) and skip the runtime objects. The
    // owned _start captures argc/argv/environ itself and exits via syscall.
    let freestanding = if shared {
        false
    } else {
        // The owned _start marker is the contract: the IR references no
        // briev_rt.c/libc symbols (the backend gate proved it), so the
        // runtime objects — including briev_rt.o, which the env.bv frgns
        // pull unconditionally — are droppable.
        ll_text.contains("define void @_start() naked") && protocol_libs.is_empty()
    };
    if freestanding {
        cmd.args(["-nostdlib", "-no-pie", "-ffreestanding"]);
        // 2026-09-13: for non-linux targets, use lld (GNU ld may not support
        // the target arch — e.g. riscv64 emulation is missing from binutils ld)
        // and the medany code model — QEMU virt RAM sits at 0x80000000, which
        // overflows medlow's signed-32-bit %hi/%lo addressing (the Linux
        // kernel / OpenSBI need medany for the same reason).
        if !triple.contains("linux") {
            cmd.arg("-fuse-ld=lld");
            if triple.starts_with("riscv64") {
                cmd.arg("-mcmodel=medany");
            }
        }
        // 2026-09-13 (rv64 capability kernel): linker script passthrough.
        // Read the linker script path from the IR (the backend emits a module
        // asm comment `; linker: <path>` when configured). If present, pass
        // -T <path> to the linker.
        if let Some(ld_path) = ll_text.lines()
            .find(|l| l.contains("; linker: "))
            .and_then(|l| {
                let start = l.find("; linker: ")?.checked_add(10)?;
                Some(l[start..].trim().to_string())
            })
        {
            cmd.arg(format!("-T{}", ld_path));
        }
        // 2026-09-13: for riscv64 bare-metal, link compiler-rt helpers
        // (unsigned division/modulo intrinsics that LLVM emits).
        if triple.starts_with("riscv64") {
            // Resolve relative to the workspace root (Cargo.toml dir).
            let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
                .unwrap_or_else(|_| ".".to_string());
            let crt_path = std::path::PathBuf::from(&workspace_root)
                .join("lib/runtime/compiler_rt_rv64.c");
            if crt_path.exists() {
                cmd.arg(crt_path);
            }
        }
        // 2026-09-14 (rv64-finish plan Phase 5): ARM bare-metal — same
        // compiler-rt need, AEABI ABI names (no hardware divider on
        // Cortex-M3; LLVM emits __aeabi_ldivmod/__aeabi_memclr8). The
        // division entries are assembly (.S): LLVM calls them with the
        // AEABI register convention, which C cannot express.
        if triple.starts_with("thumb") || triple.starts_with("arm") {
            let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
                .unwrap_or_else(|_| ".".to_string());
            for shim in ["lib/runtime/compiler_rt_arm.c", "lib/runtime/compiler_rt_arm.S"] {
                let crt_path = std::path::PathBuf::from(&workspace_root).join(shim);
                if crt_path.exists() {
                    cmd.arg(crt_path);
                }
            }
        }
    } else {
        for obj in extra_objects {
            cmd.arg(obj.as_os_str());
        }
    }
    // 2026-07-26: Link protocol-based libraries (from #System).
    // The clang driver adds these as -l<name> flags to the linker.
    for lib in protocol_libs {
        cmd.arg(format!("-l{}", lib));
    }
    cmd.arg("-o").arg(binary_path);
    if !freestanding {
        cmd.args(["-lm", "-ldl"]);
    }
    let status = cmd.status()
        .map_err(|e| format!(
            "failed to invoke clang: {} (is clang installed? use --llvm to emit IR only)",
            e
        ))?;

    if !status.success() {
        return Err(format!(
            "clang failed to compile '{}' to binary '{}'",
            ll_path, binary_path,
        ));
    }

    println!("wrote {}", binary_path);
    Ok(())
}

/// Compile LLVM IR to a linkable static library (`ar rcs lib<name>.a`),
/// plus a PIC `.so` for c_abi hosts. 2026-08-03: the `--library` on-ramp —
/// exported defns become C-callable symbols, `__briev_init_state()` returns
/// a state handle.
///
/// The .a is gcc-linkable: it packages the bridge .o (real ELF from llc)
/// plus a NON-LTO briev_rt.o. frgn-derived objects are LTO bitcode (clang
/// -flto) and live in the .so; plain C hosts link the .a. Bridges with
/// custom C frgns use the .so / clang.
fn compile_ll_to_library(ll_path: &str, base: &str, _extra_objects: &[PathBuf]) -> Result<(), String> {
    // Step 1: optimize the IR, then codegen. 2026-08-03: `llc -O3` alone did
    // NOT SROA the txn allocas in this LLVM (18.1.3) — the loop kept stack
    // slots (2.2× slower than native). Running the IR pipeline via
    // `opt -passes='default<O3>'` first produces the tight SSA loop, then
    // llc codegens it.
    let opt_path = format!("{}.opt.ll", base);
    let mut opt = Command::new("opt");
    opt.args(["-S", "-passes=default<O3>", "-o", &opt_path, ll_path]);
    let status = opt.status()
        .map_err(|e| format!("failed to invoke opt: {}", e))?;
    if !status.success() {
        let _ = std::fs::remove_file(&opt_path);
        return Err(format!("opt failed for '{}'", ll_path));
    }
    let o_path = format!("{}.o", base);
    let mut llc = Command::new("llc");
    llc.args(["-O2", "-filetype=obj", "-relocation-model=pic", "-o", &o_path, &opt_path]);
    let status = llc.status()
        .map_err(|e| format!("failed to invoke llc: {}", e))?;
    let _ = std::fs::remove_file(&opt_path);
    if !status.success() {
        return Err(format!("llc failed for '{}'", ll_path));
    }

    // Step 2: compile briev_rt.c WITHOUT -flto → a real object plain C hosts
    // can link (frgn-derived objects are LTO bitcode and cannot be read by
    // gcc). The .ll references the runtime transitively even when the bridge
    // declares no explicit frgn from briev_rt.c.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rt_c = manifest.join("lib/runtime/briev_rt.c");
    let rt_o = format!("{}.briev_rt.o", base);
    let mut cc_rt = Command::new("cc");
    cc_rt.args(["-c", "-fPIC", "-o", &rt_o]);
    cc_rt.arg(&rt_c);
    let status = cc_rt.status()
        .map_err(|e| format!("failed to invoke cc: {}", e))?;
    if !status.success() {
        return Err(format!("cc failed for '{}'", rt_c.display()));
    }

    // Step 3: ar rcs lib<name>.a <base>.o briev_rt.o
    let base_path = std::path::Path::new(base);
    let a_name = match base_path.file_name().and_then(|s| s.to_str()) {
        Some(stem) => format!("lib{}.a", stem),
        None => format!("lib{}.a", base),
    };
    let a_path = base_path.parent()
        .map(|p| p.join(&a_name))
        .unwrap_or_else(|| std::path::PathBuf::from(&a_name));
    let mut ar = Command::new("ar");
    ar.arg("rcs").arg(&a_path);
    ar.arg(&o_path);
    ar.arg(&rt_o);
    let status = ar.status()
        .map_err(|e| format!("failed to invoke ar: {}", e))?;
    if !status.success() {
        return Err(format!("ar failed for '{}'", a_path.display()));
    }
    println!("wrote {}", a_path.display());

    // Step 4: PIC .so for c_abi hosts (python/node ctypes/ffi-napi).
    // Links the .ll (LTO) + frgn-derived runtime objects via clang — NOT
    // the llc .o (that would duplicate every symbol).
    let so_path = format!("{}.so", base);
    let mut clang = Command::new("clang");
    clang.args(["-O3", "-flto", "-shared", "-fPIC", ll_path]);
    clang.arg(&rt_o);
    clang.args(["-o", &so_path, "-lm"]);
    let status = clang.status()
        .map_err(|e| format!("failed to invoke clang: {}", e))?;
    if !status.success() {
        return Err(format!("clang failed to link '{}'", so_path));
    }
    println!("wrote {}", so_path);
    Ok(())
}

/// Compile LLVM IR (.ll) to WASM binary (.wasm) using llc.
/// 2026-07-26: Phase 5 — Called for BackendKind::Webstack after codegen.
/// The .ll file must have been emitted with wasm32 target triple.
/// Uses `llc -march=wasm32 -filetype=obj` to produce a .o, then
/// `wasm-ld` to link into .wasm. This avoids needing a wasm32 clang.
fn compile_wasm(ll_path: &str, wasm_path: &str, exports: &[String]) -> Result<(), String> {
    // Step 1: compile .ll to .wasm object file
    let obj_path = format!("{}.o", wasm_path);
    let mut assemble = Command::new("llc");
    assemble.args(["-march=wasm32", "-filetype=obj", ll_path, "-o", &obj_path]);
    let status = assemble.status()
        .map_err(|e| format!(
            "failed to invoke llc: {} (install llvm-tools or use --emit-ir-only)",
            e
        ))?;
    if !status.success() {
        return Err(format!("llc failed to compile '{}' to WASM object", ll_path));
    }
    // Step 2: link .o to .wasm — export the reactive entry points the JS shim
    // calls (state_layout + every txn/definition). wasm-ld exports nothing by
    // default; without these the generated module is a dead object.
    let mut link = Command::new("wasm-ld");
    link.args(["--no-entry", "--allow-undefined", "-o", wasm_path, &obj_path]);
    for name in exports {
        link.arg(format!("--export={}", name));
    }
    let status = link.status()
        .map_err(|e| format!(
            "failed to invoke wasm-ld: {} (install wasm-ld or use --emit-ir-only)",
            e
        ))?;
    if !status.success() {
        let _ = std::fs::remove_file(&obj_path);
        return Err(format!("wasm-ld failed to link '{}'", wasm_path));
    }
    // Clean up intermediate object
    let _ = std::fs::remove_file(&obj_path);
    println!("wrote {}", wasm_path);
    Ok(())
}

/// The effective view HTML: the `<view>` tag / `--html` value when present;
/// else the `render Root` container fragment (2026-08-12, 2b3 — component
/// fragments are mounted via tags, so the Root container is the view, not the
/// concatenation of every fragment); else the legacy concatenation of render
/// attachments.

/// Lex + parse + resolve imports + typecheck, returning items and universe.

/// Load TargetConfig, respecting --config-dir when set in opts.
/// 2026-07-16: P1 — Runtime config directory overrides compile-time baked.


/// Lex the source into tokens with source spans.
/// 2026-07-16: Fixed — use actual spans from logos instead of 0..0.

/// 2026-08-06 (Phase 3): Lex `source`, routing `.f`-profile sources through
/// the token-aware layout frontend first. `.f` sources delimit blocks with
/// indentation; the layout pass synthesizes braces/semicolons so the parser
/// produces the SAME AST as canonical brace syntax. The canonical path is
/// untouched (build-safe additive routing).

/// Parse tokens into an AST.

/// 2026-07-20: Validate type parameter bounds (K: String, V: Float).
/// Checks that types declaring bounded type params have at least one
/// operator referencing the bound hashword in their params.

/// Type-check the program against a TypeUniverse.

#[cfg(test)]
mod tests {

    /// 2026-08-27 (Slice A): an extern source that does not exist is a hard
    /// error naming both symbol and path.
    #[test]
    fn test_extern_missing_source_is_hard_error() {
        use briev_compiler::ast::{TopLevel};
        use briev_compiler::ast::top::*;
        let cell = TopLevel::Cell(CellDef {
            name: "Ghost".into(),
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            fields: vec![],
            transactions: vec![],
            definitions: vec![],
            internal_triggers: vec![],
            is_persistent: false,
            metadata: Default::default(),
            span: None,
            doc: None,
            ports_in: vec![],
            ports_out: vec![],
            extern_source: Some("/tmp/opencode/definitely-missing-uart.v".into()),
        });
        let err = copy_extern_companions(&[cell], "/tmp/opencode/probe-none.bv")
            .expect_err("missing source must fail");
        assert!(err.contains("'Ghost'"), "{err}");
        assert!(err.contains("does not exist"), "{err}");
    }

    use super::*;
    use briev_compiler::pipeline::PreprocessedSource;

    /// 2026-08-31 (plan abv-gpu-by-default B3): `.abv` assumes GPU — the
    /// module accel policy defaults to try_all with no annotations; an
    /// explicit `!> accel:` wins; non-.abv sources are untouched.
    #[test]
    fn abv_default_injects_try_all_only_when_accel_absent() {
        use briev_compiler::ast::{PropertyValue, TopLevel};
        let accel_of = |items: &[TopLevel]| {
            items
                .iter()
                .find_map(|i| match i {
                    TopLevel::ModuleMetadata(m) => m.get("accel").cloned(),
                    _ => None,
                })
        };

        // No metadata at all → appended node with accel = try_all.
        let mut items: Vec<TopLevel> = vec![];
        apply_abv_accel_default(&mut items, "k.abv");
        assert!(matches!(
            accel_of(&items),
            Some(PropertyValue::Identifier(v)) if v == "try_all"
        ));

        // Metadata without an accel key → default lands in it.
        let mut meta = std::collections::HashMap::new();
        meta.insert("other".into(), PropertyValue::Identifier("x".into()));
        let mut items = vec![TopLevel::ModuleMetadata(meta)];
        apply_abv_accel_default(&mut items, "k.abv");
        assert!(matches!(
            accel_of(&items),
            Some(PropertyValue::Identifier(v)) if v == "try_all"
        ));

        // Explicit policy wins — no injection.
        let mut meta = std::collections::HashMap::new();
        meta.insert("accel".into(), PropertyValue::Identifier("force".into()));
        let mut items = vec![TopLevel::ModuleMetadata(meta)];
        apply_abv_accel_default(&mut items, "k.abv");
        assert!(matches!(
            accel_of(&items),
            Some(PropertyValue::Identifier(v)) if v == "force"
        ));

        // Non-.abv sources are untouched (no metadata added).
        let mut items: Vec<TopLevel> = vec![];
        apply_abv_accel_default(&mut items, "k.bv");
        assert!(accel_of(&items).is_none());
        assert!(items.is_empty());
    }

    /// 2026-08-18 (check/build divergence): `brievc check` on a program that
    /// imports `std/collections.bv` and calls the HashMap's generic scans
    /// (`m.keys()` → `ks.Count#()`) previously over-reported type errors
    /// ("expected List<K> for arrow assignment, found K") while `build` was
    /// clean. Two causes, both fixed: the import resolver did not walk member
    /// OUTPUT types (so `import { HashMap }` dropped `List`, referenced only in
    /// `keys()`'s return) and the typechecker's name-based `List` special-case
    /// masked the gap; and the check path was a stale lean pipeline that
    /// skipped the build path's plugin/comptime stages. check_source now runs
    /// the unified pipeline and must be clean.
    #[test]
    fn check_on_imported_generic_scans_is_clean() {
        let src = r#"
import { HashMap } from "std/collections.bv";
let m: HashMap<Int, Int> = 4;
let done: Bool = false;
node go [done == false][done == true] {
    when done == false {
        m.insert((1, 10));
        let ks: List<Int> = m.keys();
        println!(ks.Count#());
        done = true;
    };
    term;
};
"#;
        // The import resolves relative to the file's directory — write the
        // program into the workspace (tests/tier1/) so `std/collections.bv`
        // resolves exactly as a real user file would.
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let dir = std::path::Path::new(&manifest).join("tests/tier1");
        std::fs::create_dir_all(&dir).expect("create tests/tier1");
        let path = dir.join("check_divergence_tmp.bv");
        std::fs::write(&path, src).expect("write fixture");
        let result = check_source(path.to_str().unwrap(), src);
        let _ = std::fs::remove_file(&path);
        result.expect("briev check must be clean on imported generic scans");
    }

    #[test]
    fn test_preprocess_source_for_path_rbv_extracts_briev_and_view() {
        let source = "let x: Int = 0;\n<view><div>ok</div></view>\n<style>.a{color:red;}</style>\n";
        let parsed = preprocess_source_for_path("/tmp/sample.rbv", source)
            .expect("rbv parse should succeed");

        assert!(parsed.briev_source.contains("let x: Int = 0;"));
        assert_eq!(parsed.view_html.as_deref(), Some("<div>ok</div>"));
        assert_eq!(parsed.style_css.as_deref(), Some(".a{color:red;}"));
    }

    #[test]
    fn test_preprocess_source_for_path_non_rbv_passthrough() {
        let source = "let x: Int = 0;\n";
        let parsed = preprocess_source_for_path("/tmp/sample.bv", source)
            .expect("bv passthrough should succeed");

        assert_eq!(parsed.briev_source, source);
        assert!(parsed.view_html.is_none());
        assert!(parsed.style_css.is_none());
    }

    #[test]
    fn test_preprocess_source_for_path_rbv_no_markup_passthrough() {
        let source = "let x: Int = 0;\n";
        let parsed = preprocess_source_for_path("/tmp/sample.rbv", source)
            .expect("logic-only rbv should pass through");

        assert_eq!(parsed.briev_source, source);
        assert!(parsed.view_html.is_none());
        assert!(parsed.style_css.is_none());
    }

    /// 2026-08-11 (view wiring): a minimal webstack BuildOptions for the
    /// compile_view unit tests.
    fn webstack_opts(file_path: &str) -> BuildOptions {
        BuildOptions {
            run: false,
            config_dir: None,
            file_path: file_path.to_string(),
            emit_ir_only: false,
            out_dir: None,
            optimize_budget: 256,
            emit_beast_stages: vec![],
            backend: BackendKind::Webstack,
            no_stdlib: false,
            stdlib_path: None,
            disable_plugins: vec![],
            enable_plugins: vec![],
            trg_unresolved_action: TrgUnresolvedAction::Warn,
            explain_causality: false,
            extra_objects: vec![],
            shared: false,
            library_mode: false,
            keep_all_defns: false,
            int_bits: 32,
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
            sysquery_overrides: std::collections::HashMap::new(),
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
            triple_override: None,
            linker_script_override: None,
        }
    }

    fn preprocessed_with_view(view_html: &str) -> PreprocessedSource {
        PreprocessedSource {
            briev_source: "".to_string(),
            style_css: None,
            view_html: Some(view_html.to_string()),
        }
    }

    #[test]
    fn test_compile_view_injects_ids_and_extracts_bindings() {
        let opts = webstack_opts("/tmp/app.rbv");
        let items = vec![
            briev_compiler::ast::TopLevel::Statement(Box::new(
                briev_compiler::ast::Statement::Let {
                    name: "count".to_string(),
                    names: vec![],
                    ty: Some(briev_compiler::ast::Type::int()),
                    expr: Some(briev_compiler::ast::Expr::Decimal(0)),
                    modifiers: vec![],
                },
            )),
        ];
        let pre = preprocessed_with_view(
            r#"<div><span b-text="count">0</span><button b-trigger:click="bump">+</button></div>"#,
        );
        let cv = compile_view("/tmp/app.rbv", &items, &opts, &pre, &ViewMountSpecs { pools: std::collections::HashMap::new(), instances: std::collections::HashMap::new() }).expect("view compiles");
        assert!(!cv.bindings.is_empty(), "b-text/b-trigger bindings extracted");
        let html = cv.modified_html.expect("modified html present");
        assert!(
            html.contains("id=\"rbv-"),
            "element IDs injected for the dom-shim: {html}"
        );
        let has_text = cv.bindings.iter().any(|b| {
            matches!(
                &b.directive,
                briev_compiler::view_compiler::Directive::Text { signal } if signal == "count"
            )
        });
        assert!(has_text, "b-text binding for count present");
    }

    #[test]
    fn test_compile_view_rejects_b_if() {
        let opts = webstack_opts("/tmp/app.rbv");
        let pre = preprocessed_with_view(r#"<div b-if="x">bad</div>"#);
        let err = compile_view("/tmp/app.rbv", &[], &opts, &pre, &ViewMountSpecs { pools: std::collections::HashMap::new(), instances: std::collections::HashMap::new() }).unwrap_err();
        assert!(
            err.contains("`b-if` is invalid"),
            "b-if rejected per SPEC 21.4: {err}"
        );
    }

    #[test]
    fn test_compile_view_strict_rejects_undefined_signal() {
        let opts = webstack_opts("/tmp/ui.s.rbv");
        let pre = preprocessed_with_view(r#"<span b-text="nope">x</span>"#);
        let err = compile_view("/tmp/ui.s.rbv", &[], &opts, &pre, &ViewMountSpecs { pools: std::collections::HashMap::new(), instances: std::collections::HashMap::new() }).unwrap_err();
        assert!(
            err.contains("SRBV001") && err.contains("'nope'"),
            "strict profile rejects undefined signal: {err}"
        );
    }

    #[test]
    fn test_compile_view_non_strict_undefined_signal_passes() {
        // 2026-08-11: plain .rbv builds surface ViewCompiler diagnostics as
        // warnings — SRBV reference errors are a `.s` strict-profile feature.
        let opts = webstack_opts("/tmp/app.rbv");
        let pre = preprocessed_with_view(r#"<span b-text="nope">x</span>"#);
        let cv = compile_view("/tmp/app.rbv", &[], &opts, &pre, &ViewMountSpecs { pools: std::collections::HashMap::new(), instances: std::collections::HashMap::new() }).expect("non-strict view compiles");
        assert!(
            cv.bindings.iter().any(|b| {
                matches!(
                    &b.directive,
                    briev_compiler::view_compiler::Directive::Text { signal } if signal == "nope"
                )
            }),
            "binding still extracted (dead in the shim until the field exists)"
        );
    }

    #[test]
    fn test_compile_view_falls_back_to_render_block_html() {
        // A .bv with `render Name { ... }` and no <view> block derives its
        // view from the render attachment.
        let opts = webstack_opts("/tmp/app.bv");
        let items = vec![briev_compiler::ast::TopLevel::RenderBlock(
            briev_compiler::ast::RenderBlock {
                struct_name: "Root".to_string(),
                view_html: r#"<span b-text="count">0</span>"#.to_string(),
                span: None,
            },
        )];
        let pre = PreprocessedSource {
            briev_source: "".to_string(),
            style_css: None,
            view_html: None,
        };
        let cv = compile_view("/tmp/app.bv", &items, &opts, &pre, &ViewMountSpecs { pools: std::collections::HashMap::new(), instances: std::collections::HashMap::new() }).expect("render block compiles");
        let html = cv.modified_html.expect("html from render block");
        assert!(html.contains("b-text") || html.contains("rbv-"));
        assert!(!cv.bindings.is_empty());
    }

    #[test]
    fn test_view_root_signals_derefs_projection() {
        use briev_compiler::view_compiler::{Binding, Directive};
        let bindings = vec![
            Binding {
                element_id: "a".to_string(),
                directive: Directive::Text {
                    signal: "items.^Size".to_string(),
                },
            },
            Binding {
                element_id: "b".to_string(),
                directive: Directive::Text {
                    signal: "count".to_string(),
                },
            },
            Binding {
                element_id: "c".to_string(),
                directive: Directive::Trigger {
                    event: "click".to_string(),
                    txn: "bump".to_string(),
                    params: vec![],
                },
            },
        ];
        let signals = view_root_signals(&bindings);
        assert!(signals.contains("items"), "projection derefs to root field");
        assert!(signals.contains("count"));
        assert!(!signals.contains("bump"), "triggers reference txns, not fields");
    }

    #[test]
    fn test_resolve_bind_routes_unique_writer() {
        // 2026-08-11 (Phase 2a2): a field written by exactly one transaction
        // resolves to that transaction with the param marshalling category.
        use briev_compiler::analysis::transition_graph::ReactorNode;
        use briev_compiler::glue::web_generator::{BindRoute, ParamKind};
        use std::collections::HashSet;

        let node = |name: &str, fields: &[&str]| ReactorNode {
            name: name.to_string(),
            is_reactive: false,
            precondition: briev_compiler::ast::Expr::Bool(true),
            body: vec![],
            bounded_pre: None,
            increments: None,
            is_pure_body: true,
            write_set: fields.iter().map(|s| s.to_string()).collect(),
            is_effectively_pure: false,
            lexicographic_vars: vec![],
        };
        let graph = briev_compiler::analysis::transition_graph::ReactorTransitionGraph {
            nodes: vec![node("set_name", &["name"])],
            has_triggers: false,
            live_fields: HashSet::new(),
            has_unguarded_ffi: HashSet::new(),
        };
        let items = vec![briev_compiler::ast::TopLevel::Transaction(
            briev_compiler::ast::Transaction {
                name: "set_name".to_string(),
                is_reactive: false,
                is_async: false,
                type_params: vec![],
                parameters: vec![(
                    "n".to_string(),
                    briev_compiler::ast::Type::Custom("String".to_string()),
                )],
                output_type: None,
                outputs: Vec::new(),
                contract: briev_compiler::ast::Contract::new(
                    briev_compiler::ast::Expr::Bool(true),
                    briev_compiler::ast::Expr::Bool(true),
                ),
                body: vec![],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            },
        )];
        let mut universe = TypeUniverse::new();
        let routes = resolve_bind_routes(&Some(graph), &items, &mut universe);
        let route = routes
            .get("name")
            .expect("name resolves")
            .as_ref()
            .expect("no error");
        assert_eq!(route.txn, "set_name");
        assert_eq!(route.param_kind, ParamKind::String);
    }

    #[test]
    fn test_resolve_bind_routes_ambiguous_and_missing() {
        // A field written by two transactions is ambiguous (SPEC 21.4 needs a
        // single proven write contract); a field no transaction writes has no
        // route at all.
        use briev_compiler::analysis::transition_graph::ReactorNode;
        use std::collections::HashSet;

        let node = |name: &str| ReactorNode {
            name: name.to_string(),
            is_reactive: false,
            precondition: briev_compiler::ast::Expr::Bool(true),
            body: vec![],
            bounded_pre: None,
            increments: None,
            is_pure_body: true,
            write_set: HashSet::from(["count".to_string()]),
            is_effectively_pure: false,
            lexicographic_vars: vec![],
        };
        let graph = briev_compiler::analysis::transition_graph::ReactorTransitionGraph {
            nodes: vec![node("w1"), node("w2")],
            has_triggers: false,
            live_fields: HashSet::new(),
            has_unguarded_ffi: HashSet::new(),
        };
        let mut universe = TypeUniverse::new();
        let routes = resolve_bind_routes(&Some(graph), &[], &mut universe);
        let err = routes
            .get("count")
            .expect("count has a resolution")
            .as_ref()
            .expect_err("ambiguous writers must error");
        assert!(err.contains("ambiguous"), "got: {err}");
    }

    /// Helper: create a temporary file with given content, run a function on its path.
    fn with_temp_file<F>(content: &str, f: F)
    where F: FnOnce(&Path)
    {
        let dir = std::env::temp_dir().join("briev_compile_test");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join(format!("test_{}.c", std::process::id()));
        std::fs::write(&path, content).ok();
        f(&path);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_compile_source_to_object_cached() {
        let content = "int foo() { return 42; }";
        with_temp_file(content, |path| {
            let cache_dir = get_ffi_cache_dir();
            // First compilation
            let result1 = compile_source_to_object(path, &cache_dir);
            assert!(result1.is_ok(), "first compile failed: {:?}", result1);
            let obj1 = result1.unwrap();
            assert!(obj1.exists(), "object file not created");
            // Same source → same hash → returns cached path (identical)
            let result2 = compile_source_to_object(path, &cache_dir);
            assert!(result2.is_ok(), "second compile failed: {:?}", result2);
            let obj2 = result2.unwrap();
            assert_eq!(obj1, obj2, "cached path should match");
        });
    }

    #[test]
    fn test_get_ffi_cache_dir_creates_dir() {
        let dir = get_ffi_cache_dir();
        assert!(dir.exists(), "cache directory should be created");
    }

    #[test]
    fn test_compile_source_to_object_bad_ext() {
        let path = Path::new("/tmp/test_bad_ext.xyz");
        std::fs::write(path, "hello").ok();
        let result = compile_source_to_object(path, &get_ffi_cache_dir());
        assert!(result.is_err(), "expected compile error for unknown extension");
        let _ = std::fs::remove_file(path);
    }
}
