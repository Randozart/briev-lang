pub mod assembler;
pub mod capabilities;
pub mod register_types;
pub mod circt;
pub mod electronics;
pub mod llvm;
pub mod metadata;
pub mod normalizer;
pub mod vm;
pub mod webstack;
pub mod spirv;
pub mod ptx;

use crate::analysis::call_graph::CallGraph;
use crate::analysis::dependency_graph::DependencyGraph;
use crate::analysis::loop_shape::LoopShape;
use crate::analysis::range::ParameterRanges;
use crate::analysis::dataflow::DataflowError;
use crate::analysis::region::RegionAnalyzer;
use crate::analysis::transition_graph::ReactorTransitionGraph;
use crate::ast::{Annotation, Expr, Statement, TopLevel, Transaction, Definition};
use std::collections::HashMap;

/// Intent: Container for all shared analysis results that backends can consume.
/// Backends check `optimize_mode` to decide whether to use optimized paths
/// (pre-scheduled DAG emission) or fall back to full idiomatic codegen.
pub struct AnalysisResults {
    pub call_graph: CallGraph,
    pub param_ranges: ParameterRanges,
    pub fusable_pairs: Vec<(String, String)>,
    pub dataflow_errors: Vec<DataflowError>,
    pub optimize_mode: bool,
    pub transition_graph: ReactorTransitionGraph,
    pub region_analyzer: RegionAnalyzer,
    pub dependency_graph: DependencyGraph,
    // 2026-07-31: Frontend-driven dispatch (Phase 1). Per-txn structural loop
    // shapes and swan-song hoists computed once, consumed by the LLVM backend
    // instead of re-deriving them with body re-walks. See
    // docs/plans/2026-07-31-frontend-driven-dispatch.md §6.
    pub loop_shapes: HashMap<String, LoopShape>,
    pub swan_songs: HashMap<String, (Vec<Statement>, Vec<Vec<Statement>>)>,
    // 2026-07-31: Phase 2 measurement passes (plan §7). Computed once in the
    // frontend, consumed by the LLVM backend instead of re-walking bodies:
    //   density           — float computation density for the #11 → #0 downgrade
    //   modulo_partition  — `count % K == N` dispatch detection
    //   has_unguarded_ffi — reactive txns with top-level unguarded FFI
    //   inline_decisions  — callable-txn alwaysinline decision (weighted cost)
    pub density: HashMap<String, crate::analysis::density::ComputeDensity>,
    pub modulo_partition: Option<crate::analysis::modulo_partition::ModuloPartition>,
    // 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): image
    // storage plans per txn — the frontend's storage-strategy decision for
    // texel-formatted write buffers. Consumed by the SPIR-V backend
    // (binding partition + OpTypeImage + OpImageWrite) and the runtime
    // (VkImage materialization). Empty = plain SSBOs everywhere.
    pub image_storage: HashMap<String, Vec<crate::analysis::image_storage::ImageStoragePlan>>,
    pub has_unguarded_ffi: std::collections::HashSet<String>,
    pub inline_decisions: HashMap<String, crate::analysis::inline_cost::InlineDecision>,
    // 2026-07-31: Batch-loop decomposition (plan 2026-07-31-regain-kalman-float-
    // math-parity §5, Fix 2) — the io boundary interval derived from the guard
    // precondition, consumed by the backend's emit_countable_batched_main.
    pub batch_shape: Option<crate::analysis::batch_shape::BatchShape>,
    // 2026-08-01 (D2): garbage scheduling — the frontend-computed proof of
    // each heap-backed state field's reactor-ordered last consumer, consumed
    // by the backend to emit a `Free#` exactly after that transaction's body.
    // See docs/plans/2026-08-01-global-lifetime-design.md.
    pub global_lifetime: crate::analysis::global_lifetime::GlobalLifetime,
    // 2026-08-04 (out-observability plan): the names whose CALLS (defn/node/
    // txn marked `out`) or whose READS/WRITES (`out`/`vol`-marked lets) are
    // liveness roots. The backend consumes this set (frontend-driven) so calls
    // to `out` functions survive DCE and block pure-loop folding — the
    // stdlib-side twin of the intrinsic `observable: true` flag. Direct-only:
    // a pure function calling an `out` function is not itself pinned.
    pub observable_names: std::collections::HashSet<String>,
    // 2026-08-15 (coll grow-on-full): (txn, coll_obj_type) pairs whose coll
    // length provably stays below capacity across the txn's firing sequence —
    // the grow guard is dead and the backend strips it from the inlined push.
    // See docs/plans/2026-08-15-coll-loop-guard-elimination.md.
    pub coll_safe_txns: std::collections::HashSet<(String, String)>,
    // 2026-08-16 (three-track Phase 2): (txn, coll_name) -> intra-firing peak
    // for LOCAL colls whose peak exceeds the default cap. The backend emits a
    // single `EnsureCap#(q, peak)` at the coll's construction (let site) and
    // strips the per-push grow guard (dead once cap == peak). Per coll NAME
    // (not base): two local `Q`s in one txn must not share a strip.
    pub coll_pregrow: std::collections::HashMap<(String, String), i64>,
    // 2026-08-06 (accel plan): module-level `!>` metadata (SPEC §8.9) merged
    // from TopLevel::ModuleMetadata nodes, last binding wins per key. Any
    // backend or plugin may consume it; the `accel` key gates the GPU
    // offload analysis (src/analysis/accel.rs).
    pub module_metadata: HashMap<String, crate::ast::PropertyValue>,
    // 2026-08-06 (accel plan): per-txn GPU-deferral analysis (SPEC §9.7) —
    // policy, eligibility proof, kernel shape, and decision. Computed once in
    // the frontend, consumed by the LLVM backend as a deterministic switch.
    pub accel: HashMap<String, crate::analysis::accel::AccelEntry>,
    // 2026-09-11 (Part C, Electronics Briev): contract-inferred netlist —
    // components, union-find nets from precondition pin equalities, dangling
    // diagnostics. Default (non-electronics) for every other backend; the
    // KiCad backend CONSUMES this, never re-derives.
    pub electronics: crate::analysis::electronics::ElectronicsNetlist,
    /// 2026-08-07 (object instance pools): the proven maximum live instances
    /// per obj base — the member column sizes. Predictably inexhaustible:
    /// no runtime exhaustion path exists (the analysis rejects unprovable
    /// spawn counts).
    pub spawn_pools: HashMap<String, usize>,
    /// 2026-08-07 (object instance pools): bases whose spawn count is bounded
    /// by a RUNTIME value — their member columns are runtime-sized heap
    /// buffers (dependent capacity, SPEC §16.6).
    pub dependent_pools: std::collections::HashMap<String, Vec<crate::analysis::spawn_pool::DependentTerm>>,
    /// 2026-08-09 (Phase 5): storage class of non-pooled spawn bases — `box`
    /// (per-instance-heap) or `spill` (growable buffer). The backend skips the
    /// static `[capacity x T]` column for these.
    pub spawn_storage: std::collections::HashMap<String, crate::ast::SpawnStorage>,
    /// 2026-08-26 (async Phase C, plan 2026-08-26-async-phase-c-segmented-
    /// lowering.md): spawned callables' bodies split at cancellation points,
    /// computed once here and consumed by the LLVM backend's segment-function
    /// emission. Keyed by defn name; parameters carry across boundaries,
    /// locals do not (reference-probed rule, SPEC §12.2).
    pub task_segments: HashMap<String, Vec<Vec<Statement>>>,
    /// 2026-08-31 (boundary ownership plan): per-export and per-frgn boundary
    /// ownership — Borrowed / Owned / ZeroCopy / Value / ZeroCost. Computed
    /// once in the frontend from protocol variant + direction (+ calling
    /// convention, defaulted to c_abi here since the glue config is not
    /// available in analyze_program; the GLUE wiring refines lto/wasm). See
    /// docs/plans/2026-08-31-boundary-ownership-inference.md.
    pub boundary_ownership:
        crate::analysis::boundary_ownership::BoundaryOwnershipResult,
}

/// Intent: Run shared program analysis for backend code generation.
/// Returns an AnalysisResults with CallGraph, ParameterRanges, fusable pairs,
/// and dataflow errors. When optimize is true, runs extra analysis passes
/// and applies peephole optimization.
// 2026-07-14: Wire real transition graph and dependency graph analysis.
// RegionAnalyzer is stubbed until Phase 16 reimplements it.
// 2026-07-31: Frontend-driven dispatch — build per-txn loop shapes and
// swan-song hoists so the LLVM backend consumes structured analysis instead
// of re-deriving dispatch decisions from body re-walks.
// 2026-07-31: Phase 3 (§8.1) — `min_width` is the target-config vector-phi
// promotion gate (config/targets.dbvl `vector_min_width`), threaded into
// loop-shape building.
pub fn analyze_program(
    items: &[TopLevel],
    optimize: bool,
    min_width: usize,
    type_universe: Option<&crate::type_universe::TypeUniverse>,
) -> AnalysisResults {
    let transition_graph = crate::analysis::transition_graph::ReactorTransitionGraph::build(
        items, &None, &vec![],
    );
    let dependency_graph = crate::analysis::dependency_graph::DependencyGraph::build(items)
        .unwrap_or_else(|_| crate::analysis::dependency_graph::DependencyGraph {
            topo_order: Vec::new(),
            bit_index: std::collections::HashMap::new(),
            dependencies: std::collections::HashMap::new(),
            dependents: std::collections::HashMap::new(),
            is_trg: std::collections::HashSet::new(),
            all_vars: std::collections::HashSet::new(),
        });
    let loop_shapes = crate::analysis::loop_shape::build_loop_shapes(&transition_graph, items, min_width);
    let swan_songs = build_swan_songs(items);
    // 2026-07-31: Phase 2 measurement passes (plan §7). Computed once here so
    // every backend consumer reads frontend analysis instead of re-walking
    // bodies. See docs/plans/2026-07-31-frontend-driven-dispatch.md §7.
    let density = crate::analysis::density::compute_densities(items);
    let modulo_partition = crate::analysis::modulo_partition::detect_modulo_partition(items);
    let inline_decisions = build_inline_decisions(items);
    let has_unguarded_ffi = transition_graph.has_unguarded_ffi.clone();
    // 2026-07-31: Batch-loop decomposition derived from the swan-song-stripped
    // bodies (what the backend emits). The io boundary is the guard's
    // `count % N == 0` precondition interval.
    let batch_shape = crate::analysis::batch_shape::detect_batch_shape(&swan_songs);
    // 2026-08-01 (D2): garbage scheduling — prove each heap-backed field's
    // reactor-ordered last consumer. Field initializers come from StateDecl +
    // top-level `let f: T = expr` (mirrors build_field_index); the node order
    // is the transition graph's deterministic firing order.
    let mut field_inits: HashMap<String, crate::ast::Expr> = HashMap::new();
    for item in items {
        if let TopLevel::StateDecl(s) = item {
            field_inits.entry(s.name.clone()).or_insert(crate::ast::Expr::Decimal(0));
        } else if let TopLevel::Statement(stmt) = item {
            if let crate::ast::Statement::Let { name, expr, .. } = stmt.as_ref() {
                if let Some(e) = expr {
                    field_inits.entry(name.clone()).or_insert_with(|| e.clone());
                }
            }
        }
    }
    let node_order: Vec<String> = transition_graph.nodes.iter().map(|n| n.name.clone()).collect();
    // 2026-08-06 (fix): only foldable txns (with a bounded_pre) are scheduled
    // for frees — a non-bounded reactive last consumer has no sound free point.
    let foldable: std::collections::HashSet<String> = transition_graph
        .nodes
        .iter()
        .filter(|n| n.bounded_pre.is_some())
        .map(|n| n.name.clone())
        .collect();
    let global_lifetime = crate::analysis::global_lifetime::analyze(items, &field_inits, &node_order, &foldable);
    let observable_names = collect_observable_names(items);
    let coll_safe_txns = crate::analysis::coll_length::analyze(items);
    let coll_pregrow = crate::analysis::coll_length::analyze_pregrow(items);
    let (spawn_pools, dependent_pools, _spawn_errors, spawn_storage) = crate::analysis::spawn_pool::analyze(items);
    // 2026-08-26 (async Phase C): segment every SPAWN-TARGETED defn once.
    // Non-targeted defns keep their ordinary single-body lowering.
    let spawn_targets = crate::analysis::task_segments::collect_spawn_targets(items);
    let task_segments: HashMap<String, Vec<Vec<Statement>>> = items
        .iter()
        .filter_map(|item| {
            let d = match item {
                TopLevel::Definition(d) => d,
                _ => return None,
            };
            if !spawn_targets.contains(&d.name) {
                return None;
            }
            let params: Vec<String> = d.parameters.iter().map(|(n, _)| n.clone()).collect();
            Some((d.name.clone(), crate::analysis::task_segments::split_task_body(&d.body, &params)))
        })
        .collect();
    let module_metadata = collect_module_metadata(items);
    let accel = crate::analysis::accel::analyze(items, &module_metadata, type_universe);
    // 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): image
    // storage strategy — the frontend's storage decision for texel-formatted
    // write buffers. Opt-in until measured (the coopmat precedent; promote
    // on ledger evidence).
    let image_storage = crate::analysis::image_storage::detect(
        items,
        &accel,
        type_universe,
        crate::config_tuning::ir_lowering().spirv_image_storage,
    );
    // 2026-08-31 (boundary ownership plan): classify every export/frgn
    // parameter and return. The calling convention is defaulted to c_abi here
    // (the glue config lives in compile.rs); the GLUE wiring refines
    // lto/wasm_import when it consumes the result.
    let boundary_ownership =
        crate::analysis::boundary_ownership::compute_boundary_ownership(
            items,
            type_universe,
            &|_| None,
        );
    AnalysisResults {
        call_graph: CallGraph::new(),
        param_ranges: ParameterRanges::new(),
        fusable_pairs: Vec::new(),
        dataflow_errors: Vec::new(),
        optimize_mode: optimize,
        transition_graph,
        region_analyzer: RegionAnalyzer::analyze(items),
        dependency_graph,
        loop_shapes,
        swan_songs,
        density,
        modulo_partition,
        has_unguarded_ffi,
        inline_decisions,
        batch_shape,
        global_lifetime,
        observable_names,
        coll_safe_txns,
        coll_pregrow,
        module_metadata,
        accel,
        image_storage,
        spawn_pools,
        dependent_pools,
        spawn_storage,
        task_segments,
        boundary_ownership,
        electronics: crate::analysis::electronics::derive_netlist(items),
    }
}

/// 2026-08-06 (accel plan): merge `TopLevel::ModuleMetadata` nodes into one
/// module map (SPEC §8.9). Last binding wins per key, matching the parser's
/// merge semantics across consecutive top-level `!>` lines.
fn collect_module_metadata(
    items: &[TopLevel],
) -> HashMap<String, crate::ast::PropertyValue> {
    let mut out = HashMap::new();
    for item in items {
        if let TopLevel::ModuleMetadata(map) = item {
            out.extend(map.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
    out
}

/// 2026-08-04 (out-observability plan): collect the liveness-root names —
/// `out defn`/`out node`/`out txn` names, plus `out`/`vol`-marked `let`
/// variables (vol implies out). Direct-only: a pure function that calls an
/// `out` function is not itself pinned (the out function's own body is a root,
/// so anything it calls survives inside it).
fn collect_observable_names(items: &[TopLevel]) -> std::collections::HashSet<String> {
    use crate::ast::Annotation;
    let mut names = std::collections::HashSet::new();
    let has_out = |modifiers: &[Annotation]| modifiers.iter().any(|m| m.name == "out");
    for item in items {
        match item {
            TopLevel::Definition(d) if has_out(&d.modifiers) => {
                names.insert(d.name.clone());
            }
            TopLevel::Transaction(t) if has_out(&t.modifiers) => {
                names.insert(t.name.clone());
            }
            // StateDecl has no modifiers; `out`/`vol` state fields are captured
            // via their top-level `let` sites below.
            TopLevel::Statement(stmt) => {
                collect_let_observable_names(stmt, &mut names);
            }
            _ => {}
        }
    }
    names
}

/// Recursively find `let` statements marked `out` or `vol` and add their
/// variable names to the observable set. Walks blocks and guards.
fn collect_let_observable_names(stmt: &Statement, names: &mut std::collections::HashSet<String>) {
    match stmt {
        Statement::Let { name, modifiers, .. }
            if modifiers.iter().any(|m| m.name == "out" || m.name == "vol") =>
        {
            names.insert(name.clone());
        }
        Statement::Block(body) => {
            for s in body {
                collect_let_observable_names(s, names);
            }
        }
        Statement::Guarded(_, body) => {
            for s in body {
                collect_let_observable_names(s, names);
            }
        }
        _ => {}
    }
}

/// Compute the callable-txn auto-inline decision for every transaction.
///
/// 2026-07-31: Phase 2 (§7.3) — keyed by txn name. The LLVM backend reads
/// `inline_decisions[name]` in emit_callable_txn instead of re-deriving the
/// `params < 8 && body < 20` heuristic. Only callable txns (non-reactive)
/// consult the map, but computing for all transactions is harmless.
fn build_inline_decisions(items: &[TopLevel]) -> HashMap<String, crate::analysis::inline_cost::InlineDecision> {
    let mut out = HashMap::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            out.insert(t.name.clone(), crate::analysis::inline_cost::callable_inline_decision(t));
        }
    }
    out
}

/// Hoist the swan song from every transaction body, keyed by txn name.
///
/// 2026-07-31: The state-field set must match the backend's `field_index_map`
/// keys, which includes BOTH `TopLevel::StateDecl` AND top-level
/// `TopLevel::Statement(Let)` (build_field_index mod.rs:3634/3715). The hoist's
/// let-to-field remap gates on this set — a `let nesc` binding assigned from a
/// state field is only rewritten to that field when the set contains it. Using
/// `loop_shape::collect_state_fields` (the same collector the LoopShape bound
/// resolution uses) keeps the remap, the shape, and the backend field index in
/// agreement. See docs/plans/2026-07-31-frontend-driven-dispatch.md §6.
fn build_swan_songs(
    items: &[TopLevel],
) -> HashMap<String, (Vec<Statement>, Vec<Vec<Statement>>)> {
    let state_fields = crate::analysis::loop_shape::collect_state_fields(items);
    let mut songs = HashMap::new();
    for item in items {
        if let TopLevel::Transaction(t) = item {
            let (stripped, hoisted) = crate::analysis::swan_song::hoist_swan_song(
                &t.body,
                &state_fields,
            );
            songs.insert(t.name.clone(), (stripped, hoisted));
        }
    }
    songs
}

/// Intent: Apply peephole optimization after analysis. Returns a new set of top-level items
/// with redundant assignments, dead expressions, and foldable constants removed.
/// Only called when optimize mode is active.
pub fn run_peephole(items: &[TopLevel], analysis: &AnalysisResults) -> Vec<TopLevel> {
    if !analysis.optimize_mode {
        return items.to_vec();
    }
    items.to_vec()
}

/// Intent: Return the list of hashtags supported by a given backend name.
pub fn supported_hashtags(backend: &str) -> Vec<&'static str> {
    match backend {
        "llvm" => {
            vec!["volatile", "sfence", "lfence", "mfence", "aligned", "packed",
                 "inline", "unroll", "vectorize", "gpu"]
        }
        "webstack" => {
            vec!["volatile", "aligned"]
        }
        "circt" => {
            vec!["clock", "register", "gate", "posedge", "negedge"]
        }
        _ => {
            vec![] // unknown backend — no known support
        }
    }
}

/// Intent: Result of validating a single hashtag against a backend.
#[derive(Debug, Clone, PartialEq)]
pub enum HashtagValidation {
    Supported,
    UnsupportedAdvisory(String),
    UnsupportedMandatory(String),
}

/// Intent: Validate a list of hashtags against a given backend.
/// Returns a list of validation results — callers should emit
/// warnings for `UnsupportedAdvisory` and errors for `UnsupportedMandatory`.
pub fn validate_hashtags(hashtags: &[Annotation], backend: &str) -> Vec<HashtagValidation> {
    let supported = supported_hashtags(backend);
    let mut results = Vec::new();

    for tag in hashtags {
        if is_scoped_elsewhere(tag, backend) {
            continue;
        }
        results.push(validate_single_hashtag(tag, &supported));
    }

    results
}

fn is_scoped_elsewhere(tag: &Annotation, backend: &str) -> bool {
    return false;
}

fn validate_single_hashtag(tag: &Annotation, supported: &[&'static str]) -> HashtagValidation {
    if supported.contains(&tag.name.as_str()) {
        HashtagValidation::Supported
    } else {
        HashtagValidation::UnsupportedAdvisory(tag.name.clone())
    }
}

/// Intent: Collect all hashtags from a list of statements recursively.
fn collect_hashtags_from_body(body: &[Statement]) -> Vec<crate::ast::Annotation> {
    let mut tags = Vec::new();
    for stmt in body {
        match stmt {
            Statement::Let { modifiers, .. } => tags.extend(modifiers.clone()),
            Statement::Guarded(_, stmts) => tags.extend(collect_hashtags_from_body(stmts)),
            _ => {}
        }
    }
    tags
}

/// Intent: Validate all hashtags in a program against the target backend.
/// Returns true if there are NO unsupported mandatory tag errors.
/// Prints warnings/eprintfs for unsupported tags.
pub fn validate_hashtags_in_program(items: &[TopLevel], backend: &str, strict: bool) -> bool {
    let mut all_tags: Vec<crate::ast::Annotation> = Vec::new();

    for item in items {
        match item {
            TopLevel::Transaction(txn) => {
                all_tags.extend(txn.modifiers.clone());
                all_tags.extend(collect_hashtags_from_body(&txn.body));
            }
            TopLevel::Definition(defn) => {
                all_tags.extend(defn.modifiers.clone());
                all_tags.extend(collect_hashtags_from_body(&defn.body));
            }
            _ => {}
        }
    }

    let results = validate_hashtags(&all_tags, backend);
    let mut has_errors = false;

    for result in &results {
        match result {
            HashtagValidation::Supported => {}
            HashtagValidation::UnsupportedAdvisory(name) => {
                eprintln!("warning: Hashtag #{} is not supported by {} backend (advisory, ignored)", name, backend);
            }
            HashtagValidation::UnsupportedMandatory(name) => {
                eprintln!("error: Mandatory hashtag #!{} is not supported by {} backend", name, backend);
                if strict {
                    eprintln!("  Hint: Use a different backend, remove the tag, or add fallbacks with #!A|B|C");
                }
                has_errors = true;
            }
        }
    }

    !has_errors
}

/// Intent: Collect all identifiers referenced by an expression.
pub fn collect_expr_identifiers(expr: &Expr, ids: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::Char(_) => {}
        Expr::BeginProgram => {}
        Expr::Consume(inner) => {
            collect_expr_identifiers(inner, ids);
        }
        Expr::Await(inner) => {
            collect_expr_identifiers(inner, ids);
        }
        Expr::Identifier(n) => {
            ids.insert(n.clone());
        }
        Expr::BinaryOp(_, l, r) => {
            collect_expr_identifiers(l, ids);
            collect_expr_identifiers(r, ids);
        }
        Expr::UnaryOp(_, e)
        | Expr::Cast(e, _)
        | Expr::IsType(e, _)
        | Expr::Field(e, _) => {
            collect_expr_identifiers(e, ids);
        }
        Expr::Call(_, args, _) | Expr::Spawn { args, .. } => {
            for arg in args {
                collect_expr_identifiers(arg, ids);
            }
        }
        Expr::Index(list, idx) => {
            collect_expr_identifiers(list, ids);
            collect_expr_identifiers(idx, ids);
        }
        Expr::List(elems) | Expr::Tuple(elems) => {
            for elem in elems {
                collect_expr_identifiers(elem, ids);
            }
        }
        Expr::If(cond, then, else_) => {
            collect_expr_identifiers(cond, ids);
            collect_expr_identifiers(then, ids);
            if let Some(else_expr) = else_ {
                collect_expr_identifiers(else_expr, ids);
            }
        }
        Expr::Match(expr, arms) => {
            collect_expr_identifiers(expr, ids);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_identifiers(guard, ids);
                }
                collect_expr_identifiers(&arm.body, ids);
            }
        }
        Expr::Block(stmts) => {
            ids.extend(collect_read_identifiers(stmts));
        }
        Expr::Lambda(_, body) => {
            collect_expr_identifiers(body, ids);
        }
        Expr::Within(outer, inner) => {
            collect_expr_identifiers(outer, ids);
            collect_expr_identifiers(inner, ids);
        }
        Expr::DerivationBlock(db) => {
            for ex in &db.examples {
                for input in &ex.inputs {
                    collect_expr_identifiers(input, ids);
                }
                collect_expr_identifiers(&ex.output, ids);
            }
        }
        Expr::Deref(inner) => {
            collect_expr_identifiers(inner, ids);
        }
        Expr::AddrOf(inner) => {
            collect_expr_identifiers(inner, ids);
        }
        Expr::Consume(inner) => {
            collect_expr_identifiers(inner, ids);
        }
        Expr::Field(recv, _) | Expr::Reflect(recv, _, _) => {
            collect_expr_identifiers(recv, ids);
        }
        Expr::MethodCall(recv, _, args, _) => {
            collect_expr_identifiers(recv, ids);
            for a in args { collect_expr_identifiers(a, ids); }
        }
        Expr::FormattingAnnotation(_) | Expr::StructLiteral { .. } => {}
        Expr::Decimal(_) | Expr::TaggedLiteral(_, _) | Expr::Bool(_) | Expr::Float(_) | Expr::Quoted(_) | Expr::TaggedQuotedLiteral(_, _) => {}
        Expr::PluginIntercept { args, .. } => {
            for a in args { collect_expr_identifiers(a, ids); }
        }
        Expr::Exists(name) => { panic!("compile-time existence check '{}' reached codegen", name) },
        Expr::Slice { array, start, end, stride } => {
            collect_expr_identifiers(array, ids);
            if let Some(e) = start.as_deref() { collect_expr_identifiers(e, ids); }
            if let Some(e) = end.as_deref() { collect_expr_identifiers(e, ids); }
            if let Some(e) = stride.as_deref() { collect_expr_identifiers(e, ids); }
        }
        Expr::Range { start, end, inclusive: _ } => {
            collect_expr_identifiers(start, ids);
            collect_expr_identifiers(end, ids);
        }

    }
}

/// Intent: Collect all identifiers assigned in a guarded statement body.
pub fn collect_assigned_identifiers(body: &[Statement]) -> Vec<String> {
    let mut ids = Vec::new();
    for stmt in body {
        if let Statement::Assign(lhs, _) = stmt {
            if let Expr::Identifier(name) = lhs {
                ids.push(name.clone());
            }
        }
    }
    ids
}

/// Intent: Collect all identifiers read by an expression/statement.
pub fn collect_read_identifiers(body: &[Statement]) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    for stmt in body {
        match stmt {
            Statement::Assign(_, expr) => {
                collect_expr_identifiers(expr, &mut ids);
            }
            Statement::Let { expr: Some(e), .. } => {
                collect_expr_identifiers(e, &mut ids);
            }
            Statement::Guarded(cond, stmts) => {
                collect_expr_identifiers(cond, &mut ids);
                ids.extend(collect_read_identifiers(stmts));
            }
            Statement::Expression(e) => {
                collect_expr_identifiers(e, &mut ids);
            }
            // 2026-08-01 (Phase 3c): term expressions read their
            // operands — a `term x;` node genuinely reads x. Without this the
            // concurrency gate's XOR-overlap check would miss read-write
            // dependencies through Term, wrongly requiring classification.
            Statement::Term(Some(e)) | Statement::EndProgram(Some(e)) => {
                collect_expr_identifiers(e, &mut ids);
            }
            _ => {}
        }
    }
    ids
}

/// Intent: Detect pairs of transactions where post(A) implies pre(B),
/// meaning they could be fused into a single atomic transaction.
pub fn detect_fusable_pairs(items: &[TopLevel]) -> Vec<(String, String)> {
    let txns: Vec<&crate::ast::Transaction> = items
        .iter()
        .filter_map(|item| {
            if let TopLevel::Transaction(txn) = item {
                Some(txn)
            } else {
                None
            }
        })
        .collect();

    let mut all_writes: Vec<Vec<String>> = Vec::new();
    let mut all_reads: Vec<std::collections::HashSet<String>> = Vec::new();
    let mut all_post_ids: Vec<std::collections::HashSet<String>> = Vec::new();
    let mut all_pre_ids: Vec<std::collections::HashSet<String>> = Vec::new();

    for txn in &txns {
        all_writes.push(collect_assigned_identifiers(&txn.body));
        all_reads.push(collect_read_identifiers(&txn.body));
        let mut post_ids = std::collections::HashSet::new();
        collect_expr_identifiers(&txn.contract.post_condition, &mut post_ids);
        all_post_ids.push(post_ids);
        let mut pre_ids = std::collections::HashSet::new();
        collect_expr_identifiers(&txn.contract.pre_condition, &mut pre_ids);
        all_pre_ids.push(pre_ids);
    }

    let mut pairs = Vec::new();
    for i in 0..txns.len() {
        for j in 0..txns.len() {
            if i == j { continue; }
            let fusable = all_writes[i].iter().any(|w| all_pre_ids[j].contains(w))
                || all_post_ids[i].iter().any(|id| all_reads[j].contains(id));
            if fusable {
                pairs.push((txns[i].name.clone(), txns[j].name.clone()));
            }
        }
    }
    pairs
}

/// Intent: Shared peephole optimizer that works at the AST level.
pub fn peephole_optimize_program(items: &[TopLevel]) -> Vec<TopLevel> {
    items.to_vec()
}

fn collect_assignments(body: &[Statement], out: &mut Vec<String>) {
    for stmt in body {
        match stmt {
            Statement::Assign(lhs, _) => {
                if let Expr::Identifier(name) = lhs {
                    out.push(name.clone());
                }
            }
            Statement::Guarded(_, stmts) => collect_assignments(stmts, out),
            _ => {}
        }
    }
}

fn reads_variable_general(body: &[Statement], var: &str) -> bool {
    for stmt in body {
        match stmt {
            Statement::Assign(_, expr) => {
                let mut ids = std::collections::HashSet::new();
                collect_expr_identifiers(expr, &mut ids);
                if ids.contains(var) {
                    return true;
                }
            }
            Statement::Guarded(cond, stmts) => {
                let mut ids = std::collections::HashSet::new();
                collect_expr_identifiers(cond, &mut ids);
                if ids.contains(var) || reads_variable_general(stmts, var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn writes_variable_general(body: &[Statement], var: &str) -> bool {
    for stmt in body {
        match stmt {
            Statement::Assign(lhs, _) => {
                if let Expr::Identifier(name) = lhs {
                    if name == var {
                        return true;
                    }
                }
            }
            Statement::Guarded(_, stmts) => {
                if writes_variable_general(stmts, var) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// A bitmask-based dirty flag set for the `trg` reactive system.
/// Each bit corresponds to a variable in the dependency graph's topological order.
/// Supports marking, testing, and clearing individual flags, as well as
/// marking all downstream dependents of a given variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirtyFlags(pub u64);

impl DirtyFlags {
    /// Mark a variable at `index` as dirty.
    pub fn mark(&mut self, index: usize) {
        self.0 |= 1u64 << index;
    }

    /// Check if a variable at `index` is dirty.
    pub fn is_set(&self, index: usize) -> bool {
        (self.0 & (1u64 << index)) != 0
    }

    /// Clear a variable at `index` (mark as clean).
    pub fn clear(&mut self, index: usize) {
        self.0 &= !(1u64 << index);
    }

    /// Mark all variables in `downstream` as dirty.
    pub fn mark_downstream(&mut self, downstream: &[usize]) {
        for &idx in downstream {
            self.mark(idx);
        }
    }

    /// Merge another DirtyFlags into this one (bitwise OR).
    pub fn merge(&mut self, other: &DirtyFlags) {
        self.0 |= other.0;
    }

    /// Check if any flag is set.
    pub fn any(&self) -> bool {
        self.0 != 0
    }

    /// Check if no flag is set.
    pub fn none(&self) -> bool {
        self.0 == 0
    }

    /// Return the raw bitmask.
    pub fn bits(&self) -> u64 {
        self.0
    }
}

/// Intent: Tracks guard dependencies for pre-computation caching.
/// Allows backends to pre-compute guard conditions that depend on state variables.
#[derive(Debug, Clone)]
pub struct GuardTracker {
    pub var_to_guards: std::collections::HashMap<String, std::collections::HashSet<String>>,
    pub guard_to_vars: std::collections::HashMap<String, Vec<String>>,
    pub state_vars: Vec<String>,
}

impl GuardTracker {
    pub fn new() -> Self {
        Self {
            var_to_guards: std::collections::HashMap::new(),
            guard_to_vars: std::collections::HashMap::new(),
            state_vars: Vec::new(),
        }
    }

    pub fn register_guard(&mut self, guard_name: &str, dependencies: Vec<String>) {
        for dep in &dependencies {
            self.var_to_guards
                .entry(dep.clone())
                .or_default()
                .insert(guard_name.to_string());
        }
        self.guard_to_vars
            .insert(guard_name.to_string(), dependencies);
    }

    pub fn guard_dependencies(&self, guard_name: &str) -> Option<&Vec<String>> {
        self.guard_to_vars.get(guard_name)
    }

    pub fn all_state_vars(&self) -> &[String] {
        &self.state_vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Contract, Expr, StateDecl, Type};

    fn swan_song_txn() -> TopLevel {
        // mandelbrot pattern: `escapes = nesc` binds the let to a state field;
        // the hoisted swan song `PrintLn!(nesc)` must be remapped to `escapes`.
        TopLevel::Transaction(Transaction {
            name: "mb".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![
                Statement::Assign(
                    Expr::Identifier("escapes".into()),
                    Expr::Identifier("nesc".into()),
                ),
                Statement::Guarded(
                    Expr::Bool(true),
                    vec![Statement::EndProgram(Some(Expr::Call(
                        "PrintLn!".into(),
                        vec![Expr::Identifier("nesc".into())],
                        None,
                    )))],
                ),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        })
    }

    fn top_level_let(name: &str) -> TopLevel {
        TopLevel::Statement(Box::new(Statement::Let {
            name: name.to_string(),
            names: vec![],
            ty: Some(Type::int()),
            expr: Some(Expr::Decimal(0)),
            modifiers: vec![],
        }))
    }

    #[test]
    fn test_build_swan_songs_remaps_top_level_let_field() {
        // Phase 1b regression: the state-field set backing the swan-song hoist
        // must include top-level `let` declarations (parsed as
        // TopLevel::Statement(Let)), matching the backend field_index_map keys.
        // The old StateDecl-only set dropped `escapes`, so the let-to-field
        // remap silently treated `nesc` as a local and left it un-remapped.
        let items = vec![
            top_level_let("count"),
            top_level_let("bound"),
            top_level_let("escapes"),
            swan_song_txn(),
        ];
        let songs = build_swan_songs(&items);
        let (_, hoist) = songs.get("mb").expect("mb swan song must exist");
        let song = &hoist[0];
        let Statement::Expression(Expr::Call(_, args, _)) = &song[0] else {
            panic!("hoisted swan song should be an Expression call");
        };
        assert!(matches!(&args[0], Expr::Identifier(n) if n == "escapes"),
            "top-level let state field must enable remap: {:?}", args[0]);
    }

    #[test]
    fn test_collect_state_fields_matches_build_field_index() {
        // Both StateDecl (legacy) and top-level let are valid state fields —
        // the backend's build_field_index accepts both (llvm/mod.rs), so the
        // hoist's field set must too. A StateDecl-only set is a latent remap bug.
        let items = vec![
            top_level_let("bound"),
            TopLevel::StateDecl(StateDecl {
                name: "count".to_string(),
                ty: Type::int(),
                span: None,
            }),
        ];
        let fields = crate::analysis::loop_shape::collect_state_fields(&items);
        assert!(fields.contains("count"));
        assert!(fields.contains("bound"));
    }

    #[test]
    fn test_dirty_flags_mark_and_is_set() {
        let mut df = DirtyFlags::default();
        assert!(!df.is_set(0));
        assert!(!df.is_set(5));
        df.mark(0);
        assert!(df.is_set(0));
        assert!(!df.is_set(5));
        df.mark(5);
        assert!(df.is_set(0));
        assert!(df.is_set(5));
    }

    #[test]
    fn test_dirty_flags_clear() {
        let mut df = DirtyFlags::default();
        df.mark(0);
        df.mark(1);
        df.mark(2);
        assert!(df.is_set(1));
        df.clear(1);
        assert!(df.is_set(0));
        assert!(!df.is_set(1));
        assert!(df.is_set(2));
        df.clear(0);
        df.clear(2);
        assert!(df.none());
    }

    #[test]
    fn test_dirty_flags_mark_downstream() {
        let mut df = DirtyFlags::default();
        df.mark_downstream(&[2, 4, 6]);
        assert!(!df.is_set(0));
        assert!(df.is_set(2));
        assert!(!df.is_set(3));
        assert!(df.is_set(4));
        assert!(df.is_set(6));
    }

    #[test]
    fn test_dirty_flags_merge() {
        let mut a = DirtyFlags::default();
        let b = DirtyFlags::default();
        a.mark(0);
        a.mark(2);
        a.merge(&b);
        assert!(a.is_set(0));
        assert!(a.is_set(2));
        assert!(!a.is_set(1));
        let mut b2 = DirtyFlags::default();
        b2.mark(1);
        b2.mark(3);
        a.merge(&b2);
        assert!(a.is_set(0));
        assert!(a.is_set(1));
        assert!(a.is_set(2));
        assert!(a.is_set(3));
    }

    #[test]
    fn test_dirty_flags_any_none() {
        let df = DirtyFlags::default();
        assert!(df.none());
        assert!(!df.any());
        let mut df2 = DirtyFlags::default();
        df2.mark(63);
        assert!(df2.any());
        assert!(!df2.none());
    }

    #[test]
    fn test_dirty_flags_bits() {
        let mut df = DirtyFlags::default();
        assert_eq!(df.bits(), 0);
        df.mark(0);
        assert_eq!(df.bits(), 1);
        df.mark(3);
        assert_eq!(df.bits(), 0b1001);
    }

    #[test]
    fn test_collect_expr_identifiers_identifier() {
        let mut ids = std::collections::HashSet::new();
        collect_expr_identifiers(&Expr::Identifier("x".to_string()), &mut ids);
        assert!(ids.contains("x"));
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn test_collect_expr_identifiers_binary_op() {
        let mut ids = std::collections::HashSet::new();
        let expr = Expr::BinaryOp(
            crate::ast::BinaryOpKind::Add,
            Box::new(Expr::Identifier("a".to_string())),
            Box::new(Expr::Identifier("b".to_string())),
        );
        collect_expr_identifiers(&expr, &mut ids);
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_collect_expr_identifiers_call() {
        let mut ids = std::collections::HashSet::new();
        let expr = Expr::Call(
            "f".to_string(),
            vec![
                Expr::Identifier("x".to_string()),
                Expr::Identifier("y".to_string()),
            ],
            None,
        );
        collect_expr_identifiers(&expr, &mut ids);
        assert!(ids.contains("x"));
        assert!(ids.contains("y"));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_collect_assigned_identifiers_simple() {
        let body = vec![
            Statement::Assign(
                Expr::Identifier("x".to_string()),
                Expr::Decimal(1),
            ),
            Statement::Assign(
                Expr::Identifier("y".to_string()),
                Expr::Decimal(2),
            ),
        ];
        let ids = collect_assigned_identifiers(&body);
        assert!(ids.contains(&"x".to_string()));
        assert!(ids.contains(&"y".to_string()));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_collect_module_metadata_merges_nodes() {
        // 2026-08-06 (accel plan): analyze_program surfaces module-level
        // `!>` metadata; multiple ModuleMetadata nodes merge, last wins.
        use crate::ast::PropertyValue;
        let mut m1 = std::collections::HashMap::new();
        m1.insert("accel".to_string(), PropertyValue::Identifier("try_all".into()));
        let mut m2 = std::collections::HashMap::new();
        m2.insert("accel".to_string(), PropertyValue::Identifier("force".into()));
        m2.insert("target".to_string(), PropertyValue::Identifier("spirv".into()));
        let items = vec![
            TopLevel::ModuleMetadata(m1),
            TopLevel::ModuleMetadata(m2),
        ];
        let merged = collect_module_metadata(&items);
        assert_eq!(merged.len(), 2);
        assert!(matches!(merged.get("accel"),
            Some(PropertyValue::Identifier(s)) if s == "force"),
            "last ModuleMetadata node must win");
        assert!(matches!(merged.get("target"),
            Some(PropertyValue::Identifier(s)) if s == "spirv"));
    }
}