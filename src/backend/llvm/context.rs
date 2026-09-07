// ── Backend Context Architecture ───────────────────────────────────────
//
// 2026-06-29: Three-tier context separation to eliminate the fragile
// "save/restore" anti-pattern that previously required manual cloning of
// 7+ fields at every inline txn boundary (see dispatch.rs emit_inline_txn_body).
//
// Why three contexts instead of one:
//   CompilerContext — global, read-only during codegen. Holds AST definitions,
//     target spec, FFI signatures, type info. Never modified once codegen starts.
//   FunctionContext — per-function, mutable. Holds SSA counter, local bindings,
//     phi state, arena slots. Cloned at inline boundaries; restored after.
//   BlockContext — per-basic-block, lightweight. Tracks current label and
//     transient allocations. Rarely needed beyond label tracking.
//
// Why not just make all per-function fields Clone-and-restore:
//   Cloning HashMaps at every inline boundary has measurable cost, but the
//   functional correctness benefit (eliminating state contamination) far
//   outweighs the overhead. If profiling shows this is a bottleneck, switch
//   to an arena-backed HashMap or an indexed slot approach.

use crate::analysis::dependency_graph::DependencyGraph;
use crate::analysis::pgo::PgoProfile;
use crate::analysis::FieldMode;
use crate::ast::{
    CellDef, EnumDefinition, Expr, ForeignSignature, Statement,
    TriggerDeclaration, Type,
};
use crate::backend::llvm::directive::OptimizationRemark;
use crate::backend::llvm::{AllocStrategy, ChimeraInfo};
use crate::target_spec::TargetSpec;
use crate::type_universe::TypeUniverse;
use std::collections::{HashMap, HashSet};

// ── CompilerContext ───────────────────────────────────────────────────
//
// Global compilation context — immutable once code generation begins.
// Holds everything that does not change between functions/transactions.
// Previously part of the LlvmBackend god object; extracted to prevent
// accidental mutation during per-function codegen.
#[derive(Debug, Clone)]
pub struct CompilerContext {
    // Target & Spec
    pub spec: Option<TargetSpec>,
    pub explain: bool,
    pub dump_layout: bool,
    pub library_mode: bool,
    pub is_shared_lib: bool,
    /// 2026-07-23: Skip emitting the default main() entry point.
    /// Used by protocol bridge generators that provide their own main.
    pub no_main: bool,
    /// 2026-07-23: Emit C extension module init metadata (PyInit_*, etc.)
    pub module_init: bool,

    // State layout (built during generate(), then read-only)
    pub field_index_map: HashMap<String, usize>,
    pub field_types: Vec<String>,
    pub field_briev_types: Vec<Type>,
    pub field_initializers: HashMap<String, Option<Expr>>,
    /// 2026-08-11 (2b2 slice 2b): component-instance slot initializers from
    /// Briev-side seeds (`c1.count` → 5). build_field_index merges them into
    /// field_initializers after the StateDecl registration.
    pub component_initializers: HashMap<String, Expr>,
    pub field_modes: HashMap<String, FieldMode>,
    pub cache_slots: HashMap<String, HashMap<String, (usize, usize)>>,
    pub range_bounds: HashMap<String, (i64, i64)>,
    pub field_to_meta_idx: HashMap<String, usize>,
    /// 2026-08-01 (D2): garbage scheduling — txn name → heap-backed fields to
    /// free after that txn's body (the frontend-computed free_after map).
    pub global_free_after: HashMap<String, Vec<String>>,
    /// 2026-08-06 (fix): closures collected during emission, emitted as
    /// top-level functions at the end of the module.
    pub pending_closures: Vec<PendingClosure>,
    /// 2026-08-04 (out-observability plan): names whose calls are liveness
    /// roots (`out defn`/`out node`/`out txn`) or whose reads/writes are live
    /// (`out`/`vol` lets). Frontend-computed into AnalysisResults; copied here
    /// so the backend's guard-outlining / folding gates treat calls to these
    /// like observable-intrinsic calls.
    pub observable_names: std::collections::HashSet<String>,
    /// 2026-08-15 (coll grow-on-full): (txn, coll_obj_type) pairs whose coll
    /// length provably stays below capacity across the txn — the grow guard is
    /// dead and is stripped from the inlined push member body. Frontend-driven
    /// (src/analysis/coll_length.rs).
    pub coll_safe_txns: std::collections::HashSet<(String, String)>,
    /// 2026-08-16 (three-track Phase 2): (txn, coll_name) -> intra-firing peak
    /// for LOCAL colls needing a pre-grow `EnsureCap#(q, peak)` at their
    /// construction (let site). Per coll NAME — two local `Q`s in one txn must
    /// not share a guard strip. Frontend-driven (src/analysis/coll_length.rs).
    pub coll_pregrow: std::collections::HashMap<(String, String), i64>,
    /// 2026-08-26 (async Phase C, plan 2026-08-26-async-phase-c-segmented-
    /// lowering.md): spawned defns' segment bodies + param types, computed
    /// once by the frontend pass and consumed by emit_task_runtime. Only
    /// spawn-targeted defns appear here.
    pub task_segments:
        std::collections::HashMap<String, (Vec<Vec<Statement>>, Vec<(String, Type)>)>,
    /// 2026-08-26 (async Phase D): per-obj port wiring order — base name →
    /// (in-port names, out-port names). Populated alongside struct_types so
    /// spawn can allocate/wire event-slot columns positionally.
    pub obj_port_wiring: std::collections::HashMap<String, (Vec<String>, Vec<String>)>,
    /// 2026-08-26 (async Phase D): port-event lowering is an LLVM-surface
    /// feature; the flag gates the emission arms. Always true on this
    /// backend (capability `obj_ports` mirrors it).
    pub obj_ports_enabled: bool,
    /// 2026-08-16 (multi-node internal fold, Direction 3): reactive txn names
    /// whose whole bounded pass runs inside `@txn_<name>` (a noinline countdown
    /// loop) — the reactor dispatch calls the txn once per pass instead of
    /// inlining the per-firing body.
    pub internal_fold_txns: std::collections::HashSet<String>,
    /// 2026-08-03: Per-export `needs_state` from the export ABI analysis
    /// (src/analysis/export_abi.rs). Pure exports keep a clean C ABI;
    /// exports calling any Briev defn carry `ptr %state` first.
    pub export_needs_state: HashMap<String, bool>,
    /// 2026-07-27: Reverse index from state field position to field name.
    /// Used by load_field_type() to look up !range metadata by field index.
    pub idx_to_field_name: HashMap<usize, String>,
    // 2026-07-04: Metadata ID for the !StateAliasScope used by !noalias
    // on Ptr<T> volatile accesses. Set during IR emission, then read-only
    // by intrinsics.rs volatile_load#/volatile_store# emission.
    pub state_alias_scope_md: usize,
    pub exit_condition: Option<Box<Expr>>,
    pub has_natural_exit: bool,
    /// 2026-08-11 (view wiring): state fields referenced by the web view's
    /// bindings (`b-text`, `b-show`, `b-each`, …). The DOM consumes them, so
    /// they are live by the observability-as-liveness rule and must never be
    /// pruned by dead-field elimination. Frontend-computed (ViewCompiler
    /// root signals) and copied here before generate().
    pub view_bound_fields: std::collections::HashSet<String>,
    /// 2026-08-12 (Iterable protocol, slice 4): the `b-each` iterable fields
    /// that are generic collections — the backend emits a
    /// `__view_items_<field>()` snapshot materializer for each (driving the
    /// collection's op Count/op At into a `[len][word…]` buffer the shim reads).
    pub collection_iterables: std::collections::HashSet<String>,
    /// 2026-08-12 (slice 4): whether `@reactor_tick` was emitted — a folded
    /// program (no live reactive nodes) omits it, so `render_frame` must not
    /// call it (an undefined `@reactor_tick` fails llc).
    pub has_reactor_tick: bool,
    /// 2026-09-01 (Track B): the whole program's accel kernels may use the
    /// resident launch path (all-readers-are-kernels proven per field).
    pub accel_resident_ok: bool,
    /// 2026-09-02: Minimum work-item count for GPU dispatch (--accel-cpu-fallback).
    /// Shapes with fewer work items fall to the CPU loop instead.
    pub accel_cpu_fallback: Option<u64>,

    // MMIO & Schema
    pub mmio_fields: HashMap<String, u64>,
    pub mmio_initializers: HashMap<String, Option<Expr>>,
    pub mmio_prepopulated: bool,
    /// 2026-07-26: Replaced DbrievType with HashSet. The type annotation
    /// was never read in production — only names matter for cross-validation.
    pub schema_alias_names: HashSet<String>,

    // FFI & Declarations
    pub triggers: HashMap<String, TriggerDeclaration>,

    /// 2026-08-27 (cbv-HW plan Slice B): @-addressed trigger VALUE reads —
    /// name -> static address. Reads lower to a volatile load through the
    /// boxed-pointer ABI (inttoptr of the constant). Writes are rejected by
    /// the typechecker (pins are inputs).
    pub trg_addresses: HashMap<String, u64>,
    pub trigger_names: Vec<String>,
    pub frgn_map: HashMap<String, ForeignSignature>,
    pub defn_params: HashMap<String, Vec<Type>>,
    pub defn_return_types: HashMap<String, Vec<Type>>,

    // Constants & Strings
    pub string_constants: Vec<String>,
    /// 2026-08-06 (Phase 7): raw-bytes Data literals (`#b"..."`) — emitted as
    /// `@bstr.N` globals carrying the exact bytes (`\xHH`), distinct from the
    /// lossy UTF-8 string constants.
    pub byte_constants: Vec<Vec<u8>>,
    /// 2026-08-07 (object instance pools): `@bmask.N` globals — raw i8 arrays (0/1) built
    /// from compile-time Boolean mask literals in `data[mask]`.
    pub mask_constants: Vec<Vec<u8>>,
    /// 2026-08-07 (object instance pools): the prefixed member slots of
    /// unpacked obj instances (`st.data`, `st.len`). They are ALWAYS live —
    /// the field-liveness scan does not yet walk member bodies, so without
    /// this an unpacked member referenced only inside a member txn would be
    /// pruned as Never.
    pub instance_slots: std::collections::HashSet<String>,
    /// 2026-08-07 (object instance pools): unpacked obj instance name → (obj
    /// base, its Init expression) — `let b: Box<Int, 5> = 0` → ("Box", 0).
    /// The Init member runs against the instance's prefixed slots during
    /// init_state.
    pub obj_instance_inits: std::collections::HashMap<String, (String, Expr)>,
    pub constants: HashMap<String, (Type, Expr)>,
    /// 2026-09-06 (Phase 8): the section(".name") placement per constant
    /// (the parser carries it on the Constant item; the @constant global
    /// emission reads it).
    pub constant_sections: HashMap<String, String>,
    /// 2026-08-09 (init kind, Phase 2): runtime-seeded invariants, name → type
    /// + seeding form. Emitted as mutable globals (`@name = global ... 0`),
    /// seeded once in the pre-reactor phase (emit_init_state /
    /// emit_inline_init_stores / __briev_init_state), then read via load. Not
    /// folded like `constants` — the value is only known at runtime.
    pub inits: HashMap<String, crate::ast::top::InitDecl>,

    // Type info
    pub struct_types: HashMap<String, Vec<(String, Type)>>,
    /// 2026-08-13 (layout-keywords plan): `pack struct` names. Carried on the
    /// context (frontend-declared), NOT on ResolvedType — the type still owns
    /// only protocol + declared metadata; codegen consults the packed layout
    /// helper when it must materialize the representation.
    pub packed_structs: HashSet<String>,
    /// 2026-08-15 (coll plan): `coll obj`/`coll struct` storage modes — the
    /// compiler picks the most effective representation per shape (SPEC §8.10,
    /// "storage is the compiler's choice"). `seq coll` forces the contiguous
    /// element block.
    pub coll_storage: HashMap<String, crate::backend::llvm::coll_scaffold::CollStorage>,
    /// 2026-08-13 (layout-keywords plan Phase 5): atomic field slots, keyed
/// `"<type>.<field>"` → ordering ("seq", "relaxed", "acquire", "release",
/// "bartered"). Populated at registration from the parser's
/// metadata["atomic_fields"] carrier; field load/store emitters query
/// membership and ordering to emit `load atomic`/`store atomic` (SPEC §8.2).
pub atomic_fields: std::collections::HashMap<String, String>,
    /// 2026-08-13 (layout-keywords plan Phase 6): `union` type names — all
    /// fields overlay at offset 0; the type materializes as a byte array of
    /// the largest aligned field storage (SPEC §8.2).
    pub unions: HashSet<String>,
    /// 2026-08-13 (obj value ABI): `obj` declaration names whose VALUES are
    /// boxed i64 (or i{int_bits}) handles — the pooled-instance representation
    /// used by state slots, struct literals, and field access. Distinct from
    /// `struct_types`, which also holds StaticStruct (C-compatible) names that
    /// are REAL pointers at the FFI boundary. `llvm_type`/params/returns use
    /// the handle width for obj types so a defn returning an obj (new_builder)
    /// and one taking it (append_char) share one representation.
    pub obj_types: std::collections::HashSet<String>,
    /// 2026-07-31 (A8): obj member declarations (txn/defn/node bodies), keyed
    /// by type name. Used by MethodCall codegen to emit the member body with
    /// `self` bound to the receiver instance.
    pub obj_members: HashMap<String, Vec<crate::ast::TopLevel>>,
    /// 2026-07-31 (A8): obj declared type-parameter names, keyed by type name
    /// (`obj Stack<T, N>` → ["T", "N"]). Used to substitute a receiver's
    /// concrete type args into the generic slots/members (monomorphization).
    pub obj_type_params: HashMap<String, Vec<String>>,
    pub enum_types: HashMap<String, EnumDefinition>,
    pub cell_defs: HashMap<String, CellDef>,
    pub cell_state_types: HashMap<String, (HashMap<String, usize>, Vec<String>)>,
    pub cell_wires: Vec<(String, String, String, String)>,
    pub cell_trigger_bindings: Vec<(String, String, String)>,
    pub variant_disc: HashMap<String, (String, u64, usize)>,
    /// 2026-08-23 (Phase 5d): variant constructor registry from TypeDef
    /// enums — variant name → (enum name, tag index). Built during the
    /// definition walk; read by construction calls + match dispatch.
    pub variant_ctor: HashMap<String, (String, usize)>,
    /// 2026-08-26 (Track B): declared enum names — values are boxed
    /// {tag,payload} images behind an i64 handle; llvm_type resolves them
    /// to the handle width, never to a struct/pointer ABI.
    pub enum_handle_types: std::collections::HashSet<String>,

    // Optimization
    pub optimize_budget: u64,
    /// 2026-07-22: Initial arena buffer size in bytes. Default 65536 (64KB).
    /// Larger values reduce realloc calls; smaller values waste less memory.
    pub arena_initial_size: u64,
    /// 2026-07-18: Max stack allocation size. Allocs above this → heap.
    pub stack_threshold: u64,
    pub optimize_report: bool,
    pub optimize_size: Option<u64>,
    pub pgo_profile: Option<PgoProfile>,
    pub dead_info_disabled: bool,
    pub emit_remarks: bool,
    pub has_cycles: bool,
    /// 2026-07-27: Byte size of the %State struct. Computed after field index
    /// population in generate(), used as dereferenceable(N) on ptr %state params.
    pub state_size_bytes: u64,
    /// 2026-07-28: Transition graph from analysis pipeline — carries bounded_pre
    /// (induction variable, bound, direction) and increments per transaction name.
    /// Used by emit_toplevel.rs for precise !prof branch weights on guard conditions.
    pub transition_graph: Option<crate::analysis::transition_graph::ReactorTransitionGraph>,
    /// 2026-08-07 (object instance pools): the proven maximum live instances
    /// per obj base — the member column sizes (predictably inexhaustible).
    pub spawn_pools: std::collections::HashMap<String, usize>,
    /// 2026-08-07 (object instance pools): the runtime-bound spawn terms per
    /// obj base — the columns of a base here are RUNTIME-SIZED heap buffers
    /// (malloc'd at init to the sum of the terms + 1, SPEC §16.6 dependent
    /// bounds). Keys that also appear in `spawn_pools` still include the
    /// static capacity as a minimum (rows = static_cap + sum(terms)).
    pub dependent_pools: std::collections::HashMap<String, Vec<crate::analysis::spawn_pool::DependentTerm>>,
    /// 2026-08-09 (Phase 5): storage class of non-pooled spawn bases — `box`
    /// (per-instance-heap) or `spill` (growable). Bases here never get a
    /// static `[capacity x T]` column; their spawns allocate per instance.
    pub spawn_storage: std::collections::HashMap<String, crate::ast::SpawnStorage>,
    /// 2026-08-09 (Phase 5): per-instance-heap (box/spill) member layout — base
    /// → member → (byte offset within the instance block, Briev type). The boxed
    /// block is one instance's worth of member storage laid out like the static
    /// instance columns; member access inttoptrs the handle + GEPs the offset.
    pub boxed_offsets: std::collections::HashMap<String, std::collections::HashMap<String, (u64, crate::ast::Type)>>,
    /// 2026-08-07 (object instance pools): state-field indices whose column is
    /// a DEPENDENT heap buffer → the LLVM element type string of one row (the
    /// static column's `load_ty`). Member access GEPs through the buffer
    /// pointer stored in the slot.
    pub heap_columns: std::collections::HashMap<usize, String>,
    /// 2026-07-31: Phase 2 measurement passes (plan §7) — set once in
    /// generate() from AnalysisResults so emission consumers read frontend
    /// analysis instead of re-walking bodies. See §7.5.
    /// `density` per reactive txn (the `#11 → #0` downgrade),
    /// `inline_decisions` per callable txn (auto-inline), and the program-wide
    /// `modulo_partition` (modulo-switch dispatch).
    pub density: HashMap<String, crate::analysis::density::ComputeDensity>,
    pub inline_decisions: HashMap<String, crate::analysis::inline_cost::InlineDecision>,
    pub modulo_partition: Option<crate::analysis::modulo_partition::ModuloPartition>,
    /// 2026-07-28: Per-transaction iteration bounds from RegionAnalyzer.
    /// Maps txn_name → iteration_count. Used with bounded_pre + increments for !prof.
    pub iter_bounds: HashMap<String, u64>,
    /// 2026-07-27: Pre-computed parameter string for `ptr %state` with noundef
    /// and conditional dereferenceable(N). Set by set_state_ptr_param().
    pub state_ptr_param: String,
    /// 2026-07-27: Set of function names proven to need arena initialization.
    /// Populated pre-codegen by analyze_arena_need. When empty for a given
    /// function, arena fields in %State and emit_arena_init/fini are skipped.
    pub needs_arena: HashSet<String>,

    // 2026-07-26: Native integer width for #Int protocol (default 64).
    // Controls i32 vs i64 emission for Int/UInt types.
    // WASM targets set to 32 to avoid BigInt in JavaScript.
    pub int_bits: u64,

    // Target triple config (Phase 6 — WASM support)
    /// LLVM target triple (e.g. "x86_64-unknown-linux-gnu", "wasm32-unknown-wasi").
    /// 2026-07-11: Phase 6 — read by emit_header() for dynamic target configuration.
    pub target_triple: String,
    /// LLVM data layout string. None = use default for target triple.
    /// 2026-07-11: Phase 6.
    pub data_layout: Option<String>,

    // Embedded mode
    pub is_embedded: bool,
    /// 2026-09-06 (ISR plan): the active target profile's ISR mechanism —
    /// the configured default for mechanism-less `isr` declarations.
    pub isr_mechanism: Option<String>,
    /// 2026-09-06 (ISR plan): true when the program declares ISR handlers —
    /// state becomes a GLOBAL (`@__briev_state`) so ISR bodies (which run
    /// on the hardware stack, outside main's frame) can share it with the
    /// reactor. Every `%state = alloca` site aliases the global via a
    /// zero-GEP instead.
    pub state_is_global: bool,
    pub type_universe: Option<TypeUniverse>,

    // Operator definitions (extracted from AST TypeDef bodies)
    /// 2026-07-20: Operator definitions per type, used by <- operator dispatch.
    /// Populated from TopLevel::TypeDef.body.operators in compile.rs before
    /// backend.generate(). Empty HashMap means all ops are backend-intrinsics.
    pub operator_defs: HashMap<String, Vec<crate::ast::top::OperatorDef>>,

    // 2026-07-30: Protocol casting graph — replaces operator_defs-based cast dispatch.
    // Every base protocol has a hardcoded direct lane to every other base protocol.
    // Variant edges from proto declarations add BFS-discoverable alternative paths.
    // See docs/plans/2026-07-30-casting-graph-rewrite.md and src/casting/graph.rs.
    pub casting_graph: Option<crate::casting::graph::CastingGraph>,

    // Dependency graph (built during generate(), then read-only)
    pub dep_graph: DependencyGraph,

    // 2026-07-26: Webstack (WASM-first rendering) enabled.
    // When true, emit __web_flush_state calls at term; and export
    // state_layout() for the JS shim. Only active for .rbv compilation
    // with BackendKind::Webstack.
    pub webstack_enabled: bool,

    /// 2026-08-10: Size of the webstack flush buffer — the largest transaction
    /// write_set. Computed once in generate() from the transition graph (never
    /// re-walked), then read by both the term-site batch emitters and the
    /// @__web_flush_buf declaration. 0 before generate() runs.
    pub web_max_entries: u32,

    // 2026-07-28: Phase H.0 — !> metadata registry for optimization hints.
    // Maps (key, value) metadata pairs to backend-specific LLVM IR attributes.
    // Loaded once from config/meta-vocab.dbv at CompilerContext construction.
    // Each backend has its own registry instance (Webstack/CIRCT load separately).
    pub metadata_registry: crate::backend::metadata::MetadataRegistry,
}

impl CompilerContext {
    /// 2026-07-29: Derive target float register count from LLVM target triple.
    /// Returns `usize::MAX` for virtual-register targets (WASM).
    /// See docs/plans/2026-07-29-phi-register-pressure-capping.md.
    pub fn float_register_count(&self) -> usize {
        // 2026-07-31: Phase 3 (§8.1) — the register budget comes from
        // config/targets.dbvl `[target.<triple-prefix>]`, replacing the
        // hardcoded triple-prefix match. Unknown targets fall back to the
        // x86_64 default and generate() warns (no silent x86 assumptions).
        crate::config_tuning::target_settings_for(&self.target_triple).float_registers
    }

    /// 2026-07-27: Parse the default pointer width (in bits) from a target data
    /// layout string. Looks for `-p:<abi>:<pref>-` (unqualified) or the largest
    /// `-p<num>:<abi>:<pref>-` (qualified address space). Falls back to 64.
    pub fn parse_pointer_width(dl: &str) -> u64 {
        // Match unqualified: -p:32:32- or -p:64:64-
        if let Some(cap) = dl.split('-').find_map(|seg| {
            let seg = seg.trim();
            if seg.starts_with("p:") {
                seg.split(':').nth(1).and_then(|s| s.parse::<u64>().ok())
            } else {
                None
            }
        }) {
            return cap;
        }
        // Fallback: find qualified p<num>:<abi>:<pref>, take largest abi
        let mut max_abi = 64u64;
        for seg in dl.split('-') {
            let seg = seg.trim();
            if let Some(rest) = seg.strip_prefix('p') {
                if let Some(abi) = rest.split(':').nth(1).and_then(|s| s.parse::<u64>().ok()) {
                    if abi > max_abi { max_abi = abi; }
                }
            }
        }
        max_abi
    }

    /// Get the pointer width in bytes for the current target.
    /// Derived from int_bits (set by data layout or CLI override).
    pub fn pointer_bytes(&self) -> u64 {
        self.int_bits / 8
    }

    pub fn new() -> Self {
        CompilerContext {
            spec: None,
            explain: false,
            dump_layout: false,
            library_mode: false,
            is_shared_lib: false,
            no_main: false,
            module_init: false,
            field_index_map: HashMap::new(),
            field_types: Vec::new(),
            field_briev_types: Vec::new(),
            field_initializers: HashMap::new(),
            component_initializers: HashMap::new(),
            field_modes: HashMap::new(),
            cache_slots: HashMap::new(),
            range_bounds: HashMap::new(),
            field_to_meta_idx: HashMap::new(),
            global_free_after: HashMap::new(),
            pending_closures: Vec::new(),
            observable_names: std::collections::HashSet::new(),
            coll_safe_txns: std::collections::HashSet::new(),
            coll_pregrow: std::collections::HashMap::new(),
            task_segments: std::collections::HashMap::new(),
            obj_port_wiring: std::collections::HashMap::new(),
            obj_ports_enabled: true,
            internal_fold_txns: std::collections::HashSet::new(),
            export_needs_state: HashMap::new(),
            idx_to_field_name: HashMap::new(),
            collection_iterables: std::collections::HashSet::new(),
            has_reactor_tick: false,
            accel_resident_ok: false,
            accel_cpu_fallback: None,
            state_alias_scope_md: 0,
            exit_condition: None,
            has_natural_exit: false,
            view_bound_fields: std::collections::HashSet::new(),
            mmio_fields: HashMap::new(),
            mmio_initializers: HashMap::new(),
            mmio_prepopulated: false,
            schema_alias_names: HashSet::new(),
            triggers: HashMap::new(),
            trg_addresses: HashMap::new(),
            trigger_names: Vec::new(),
            frgn_map: HashMap::new(),
            defn_params: HashMap::new(),
            defn_return_types: HashMap::new(),
            string_constants: Vec::new(),
            byte_constants: Vec::new(),
            mask_constants: Vec::new(),
            instance_slots: std::collections::HashSet::new(),
            obj_instance_inits: std::collections::HashMap::new(),
            constants: HashMap::new(),
            constant_sections: HashMap::new(),
            inits: HashMap::new(),
            struct_types: HashMap::new(),
            packed_structs: HashSet::new(),
            coll_storage: HashMap::new(),
            atomic_fields: std::collections::HashMap::new(),
            unions: HashSet::new(),
            obj_types: std::collections::HashSet::new(),
            obj_members: HashMap::new(),
            obj_type_params: HashMap::new(),
            enum_types: HashMap::new(),
            cell_defs: HashMap::new(),
            cell_state_types: HashMap::new(),
            cell_wires: Vec::new(),
            cell_trigger_bindings: Vec::new(),
            variant_disc: HashMap::new(),
            variant_ctor: HashMap::new(),
            enum_handle_types: std::collections::HashSet::new(),
            optimize_budget: 256,
            // 2026-07-31: Phase 3 (§8.2) — arena/stack sizing comes from
            // config/ir-lowering.toml instead of hardcoded literals.
            arena_initial_size: crate::config_tuning::ir_lowering().arena_initial_size,
            stack_threshold: crate::config_tuning::ir_lowering().stack_threshold,
            optimize_report: false,
            optimize_size: None,
            pgo_profile: None,
            dead_info_disabled: false,
            emit_remarks: false,
            has_cycles: false,
            state_size_bytes: 0,
            transition_graph: None,
            spawn_pools: std::collections::HashMap::new(),
            dependent_pools: std::collections::HashMap::new(),
            spawn_storage: std::collections::HashMap::new(),
            boxed_offsets: std::collections::HashMap::new(),
            heap_columns: std::collections::HashMap::new(),
            // 2026-07-31: Phase 2 measurement passes (plan §7) — stored on the
            // context so every emission consumer reads frontend analysis instead
            // of re-walking bodies. Set once in generate() from AnalysisResults.
            density: std::collections::HashMap::new(),
            inline_decisions: std::collections::HashMap::new(),
            modulo_partition: None,
            iter_bounds: HashMap::new(),
            state_ptr_param: "ptr noundef noalias nocapture align 8 %state".to_string(),
            needs_arena: HashSet::new(),
            int_bits: 64,
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            data_layout: Some(
                "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
                    .to_string(),
            ),
            is_embedded: false,
            isr_mechanism: None,
            state_is_global: false,
            type_universe: None,
            operator_defs: HashMap::new(),
            casting_graph: Some(crate::casting::graph::CastingGraph::new()),
            webstack_enabled: false,
            web_max_entries: 0,
            dep_graph: DependencyGraph {
                topo_order: Vec::new(),
                bit_index: HashMap::new(),
                dependencies: HashMap::new(),
                dependents: HashMap::new(),
                is_trg: HashSet::new(),
                all_vars: HashSet::new(),
            },
            // 2026-07-28: Phase H.0 — !> metadata registry for optimization hints.
            // Each backend loads its own MetadataRegistry instance from the embedded
            // config/meta-vocab.dbv file. The registry maps (key, value) metadata pairs
            // to backend-specific IR attributes (e.g., ("readonly", "*") → "readonly").
            metadata_registry: crate::backend::metadata::MetadataRegistry::load(),
        }
    }
}

// ── FunctionContext ────────────────────────────────────────────────────
//
// Per-function/transaction mutable state. Instantiated at the start of
// each function emission and discarded at the end. Previously these fields
// lived on LlvmBackend and required manual save/restore when inlining
// transaction bodies — a constant source of state contamination bugs.
//
// When inlining, clone this struct before entering the inline body and
// restore it after. FunctionContext implements Clone for this purpose.
/// A let-bound closure: its parameter names, body, and the free variables it
/// captures (env slots). Used by the escaping-closure lowering: `let f =
/// lambda` allocates an env block `[fn_ptr, cap1..capN]`; calls go indirect
/// through the stored fn_ptr.
#[derive(Debug, Clone)]
pub struct ClosureDef {
    pub params: Vec<String>,
    pub body: Box<crate::ast::Expr>,
    /// Free variables of the body (idents not bound by params/lets) captured
    /// by value into the env block at creation.
    pub free_vars: Vec<String>,
}

/// A closure awaiting its top-level function emission at the end of the
/// module. The closure value is a heap env block; the function reads captured
/// vars from it and returns the body's value.
#[derive(Debug, Clone)]
pub struct PendingClosure {
    pub symbol: String,
    pub params: Vec<String>,
    pub body: crate::ast::Expr,
    pub free_vars: Vec<String>,
}

/// 2026-08-06 (fix): the free variables a closure captures — every identifier
/// in the body not bound by the closure's params (or a nested lambda/block
/// let). Intrinsic names (`x#`) are excluded. Order is deterministic (first
/// appearance) so the env slot layout is stable.
pub fn collect_free_vars(body: &crate::ast::Expr, params: &[String]) -> Vec<String> {
    let mut bound: std::collections::HashSet<String> = params.iter().cloned().collect();
    let mut free: Vec<String> = Vec::new();
    collect_free_expr(body, &mut bound, &mut free);
    free
}

fn collect_free_expr(
    e: &crate::ast::Expr,
    bound: &mut std::collections::HashSet<String>,
    free: &mut Vec<String>,
) {
    match e {
        crate::ast::Expr::Identifier(n) => {
            if !bound.contains(n) && !n.ends_with('#') && !free.contains(n) {
                free.push(n.clone());
            }
        }
        crate::ast::Expr::Lambda(params, body) => {
            let mut nested = bound.clone();
            for p in params {
                nested.insert(p.clone());
            }
            collect_free_expr(body, &mut nested, free);
        }
        crate::ast::Expr::Block(stmts) => {
            let mut nested = bound.clone();
            collect_free_stmts(stmts, &mut nested, free);
        }
        crate::ast::Expr::BinaryOp(_, l, r) => {
            collect_free_expr(l, bound, free);
            collect_free_expr(r, bound, free);
        }
        crate::ast::Expr::UnaryOp(_, v)
        | crate::ast::Expr::Cast(v, _)
        | crate::ast::Expr::IsType(v, _)
        | crate::ast::Expr::Consume(v)
        | crate::ast::Expr::Deref(v)
        | crate::ast::Expr::AddrOf(v)
        | crate::ast::Expr::Reflect(v, _, _)
        | crate::ast::Expr::Within(v, _) => collect_free_expr(v, bound, free),
        crate::ast::Expr::Call(_, args, _)
        | crate::ast::Expr::List(args)
        | crate::ast::Expr::Tuple(args) => collect_free_exprs(args, bound, free),
        crate::ast::Expr::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                collect_free_expr(v, bound, free);
            }
        }
        crate::ast::Expr::Index(o, i) => {
            collect_free_expr(o, bound, free);
            collect_free_expr(i, bound, free);
        }
        crate::ast::Expr::Slice { array, start, end, stride } => {
            collect_free_expr(array, bound, free);
            for b in [start, end, stride].into_iter().flatten() {
                collect_free_expr(b, bound, free);
            }
        }
        crate::ast::Expr::Field(o, _) => collect_free_expr(o, bound, free),
        crate::ast::Expr::If(c, t, f) => {
            collect_free_expr(c, bound, free);
            collect_free_expr(t, bound, free);
            if let Some(f) = f {
                collect_free_expr(f, bound, free);
            }
        }
        crate::ast::Expr::Match(s, arms) => {
            collect_free_expr(s, bound, free);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_free_expr(g, bound, free);
                }
                collect_free_expr(&arm.body, bound, free);
            }
        }
        crate::ast::Expr::MethodCall(recv, _, args, _) => {
            collect_free_expr(recv, bound, free);
            collect_free_exprs(args, bound, free);
        }
        crate::ast::Expr::PluginIntercept { args, .. } => {
            collect_free_exprs(args, bound, free);
        }
        crate::ast::Expr::Exists(_) => {}
        _ => {}
    }
}

/// Recurse into a list of expressions (list-like shapes).
fn collect_free_exprs(
    es: &[crate::ast::Expr],
    bound: &mut std::collections::HashSet<String>,
    free: &mut Vec<String>,
) {
    for e in es {
        collect_free_expr(e, bound, free);
    }
}

/// Recurse into a block's statements, tracking local `let` bindings so a
/// local let is not captured as free.
fn collect_free_stmts(
    stmts: &[crate::ast::Statement],
    bound: &mut std::collections::HashSet<String>,
    free: &mut Vec<String>,
) {
    for s in stmts {
        match s {
            crate::ast::Statement::Let { name, expr, .. } => {
                if let Some(e) = expr {
                    collect_free_expr(e, bound, free);
                }
                bound.insert(name.clone());
            }
            crate::ast::Statement::Term(Some(v)) | crate::ast::Statement::EndProgram(Some(v)) => {
                collect_free_expr(v, bound, free)
            }
            crate::ast::Statement::Expression(v) | crate::ast::Statement::Gate(v) => {
                collect_free_expr(v, bound, free)
            }
            crate::ast::Statement::Assign(_, v) => collect_free_expr(v, bound, free),
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionContext {    // SSA register counters — NEVER rewound (prevents %t{N} collisions)
    pub txn_counter: usize,
    pub within_counter: usize,
    pub metadata_counter: usize,
    /// Arena bump counter for unique per-allocation register names
    pub arena_counter: usize,

    // Local variable bindings (let x = ...)
    pub let_bindings: HashMap<String, String>,
    pub let_binding_types: HashMap<String, Type>,
    pub let_original_types: HashMap<String, Type>,
    /// 2026-07-18: Tracks which let-bindings point to allocas (vs SSA registers).
    pub let_binding_allocas: HashSet<String>,
    /// 2026-08-04 (compiler-in-Briev): top-level `let name` bindings that are
    /// reassigned somewhere in the body (incl. inside when/if/foreach blocks).
    /// Pre-declared as allocas at the function ENTRY so a reassignment inside a
    /// guard/loop body stores into an entry-block alloca — emitting the demotion
    /// alloca at the assignment site violated LLVM dominance (alloca in
    /// guard.thenN, load in guard.endN → "Instruction does not dominate all
    /// uses"). Value is the let's declared type (None = untyped).
    pub reassigned_lets: HashMap<String, Option<Type>>,
    /// 2026-08-01 (audit): regs produced by boxing a Bool/Char/Data param
    /// (zext from the native width to i64) at defn entry. The Print#
    /// convenience intrinsic dispatches on the declared protocol category
    /// (Bool → __print_bool, Char → __print_char) — a boxed Bool reg is
    /// already i64 0/1, so it must be passed directly; a native i8 Bool
    /// (literal/let/field) needs a zext first.
    pub boxed_scalar_regs: HashSet<String>,
    /// 2026-08-26 (async Phase C): registers holding TASK handles (returned
    /// by a defn spawn, or moved from another task handle binding). `free x;`
    /// consults this set: task handles cancel through @briev_task_cancel,
    /// ordinary locals keep the destroy path.
    pub task_handle_regs: HashSet<String>,
    /// 2026-08-26 (async Phase C): BINDING names whose value is a task
    /// handle — populated at the let/assign sites whose RHS spawns a defn
    /// (or moves another handle). `free x;` cancels iff x is here.
    pub task_handle_names: HashSet<String>,
    /// 2026-08-01 (Phase 3): registers consumed by an `Expr::Consume` since the
    /// last statement boundary. The backing is destroyed (strategy-aware free)
    /// after the consuming op has used the value — drained by
    /// `emit_statement_sequence`.
    pub pending_consumes: Vec<String>,
    /// 2026-07-24: Tracks the original alloca register for struct literal values.
    /// Keyed by variable name. When &x is taken on a struct-typed let binding,
    /// this map provides the stack alloca pointer instead of the ptrtoint result.
    pub struct_literal_allocas: HashMap<String, String>,
    /// 2026-08-13 (reactor fix): struct-literal `alloca` instructions deferred
    /// by emit_struct_literal while `defer_struct_allocas` is set, flushed to
    /// the function entry / loop preheader by flush_pending_struct_allocas. An
    /// alloca left inside a reactor loop body makes clang -O3 peel the loop and
    /// emit a bogus exit assumption — the node fires once.
    pub pending_struct_allocas: Vec<String>,
    /// 2026-08-13: while true, emit_struct_literal/emit_struct_array write
    /// their alloca to `pending_struct_allocas` instead of the output. Set by
    /// the reactor loop builders around the in-loop body emission (and by
    /// emit_definition), so in-loop struct literals hoist to the preheader.
    pub defer_struct_allocas: bool,

    /// 2026-07-31 (A5): active obj-member `self` binding — (struct type name,
    /// self pointer register). While set, a bare identifier naming a slot of
    /// the struct resolves to `getelementptr self + offset` + load (read) or
    /// store (write), so `txn push(val) { data[len] = val; len = len + 1; }`
    /// mutates the receiver instance.
    pub self_binding: Option<(String, String)>,
    /// 2026-08-07 (object instance pools): the unpacked obj instance prefix
    /// (`st`) while emitting a member body. Bare member names (`data`, `len`)
    /// resolve to the `{prefix}.{member}` top-level field slots instead of a
    /// boxed self address. The second element is the pool ROW register —
    /// "0" for a static instance, the handle's register for a spawned one.
    pub self_prefix: Option<(String, String)>,
    /// 2026-08-17 (foreach break): stack of the innermost enclosing foreach's
    /// `end` labels. Pushed when a `foreach` body begins, popped after. A
    /// `break;` emits `br label %<top>` — the nearest enclosing foreach end.
    pub foreach_break_labels: Vec<String>,

    // Register type caches
    pub reg_float_cache: HashMap<String, String>,
    pub reg_type_cache: HashMap<String, Type>,

    // ── ⚠  NON-DETERMINISM WARNING ⚠  ─────────────────────────────────
    //
    // Every HashMap below is iterated during LLVM IR emission (phi header
    // creation, ssa_old cache setup, latch backedge, commit block stores,
    // post-loop loads, etc.).  Rust's HashMap uses SipHash with a random
    // seed per process, so iteration order differs EVERY COMPILATION.
    //
    // THIS IS A BUG if the iteration order determines LLVM IR instruction
    // order.  LLVM's optimizer (SROA, GVN, vectorizer) is phi-order-sensitive
    // — different phi node orderings produce different optimized code,
    // causing up to ~9% benchmark-to-benchmark performance variation.
    //
    // If you add a new for-loop over any of these maps for code generation,
    // you MUST sort the entries by key before iterating:
    //
    //   let mut sorted: Vec<_> = map.iter().map(|(k,v)| (k.clone(),v.clone())).collect();
    //   sorted.sort_by_key(|(k,_)| k.clone());
    //   for (key, val) in &sorted { ... }
    //
    // The same applies if you add a NEW HashMap that will be iterated for
    // code emission.  HashMaps used solely for O(1) lookups are fine.
    // See docs/plans/2026-07-06-ir-determinism-and-benchmark-strategy.md
    // and commit 139c345 for the full fix history.
    //
    // ────────────────────────────────────────────────────────────────────
    pub ssa_old_int_regs: HashMap<String, String>,
    pub ssa_old_float_regs: HashMap<String, String>,
    pub pending_phi_backedge: HashMap<String, String>,
    pub phi_field_regs: HashMap<String, String>,
    pub backedge_field_regs: HashMap<String, String>,
    pub used_phi_loop: bool,
    pub phi_induction_reg: Option<(String, String, String)>,
    pub loop_exit_label: Option<String>,

    // Function-level state flags
    pub terminated: bool,
    /// 2026-08-26 (async Phase D): true while emitting a `__task_*_seg<k>`
    /// body — an unready port read may suspend (returns the BLOCKED
    /// aggregate); anywhere else it traps (top-level reads gate on .^Ready).
    pub is_task_segment: bool,
    /// 2026-07-27: Name of the transaction/function being compiled.
    /// Used as key for CompilerContext.needs_arena lookup.
    pub txn_name: String,
    pub returns_i64: bool,
    pub fn_ret_ty: String,
    pub main_body: bool,
    pub in_callable_txn: bool,
    pub callable_txn_result: Option<String>,
    pub callable_txn_post_label: Option<String>,
    /// 2026-07-26: Target label for [expr]; convergence gates.
    /// Set by callable-txn body entry. Gate emits `br i1 %cond, continue, convergence_target`
    /// when the condition is false, branching back to the convergence loop for retry.
    pub convergence_target: Option<String>,
    /// 2026-08-09 (Phase 10): `defer { ... }` bodies registered by the current
    /// transaction/reactive firing, in registration order. Flushed LIFO before
    /// every exit (term/rollback/fallthrough ret) via flush_defer_cleanup.
    pub defer_bodies: Vec<Vec<crate::ast::Statement>>,
    /// 2026-08-04 (term-termination-diagnostics): label that a value-form
    /// `term <val>` / `term! <val>` in a VOID function branches to, unwinding
    /// the rest of the transaction body (interpreter TermReturn in
    /// src/interpreter/eval.rs). Set by the SSA main loop to the current
    /// txn's `.ssn_<name>` next-txn label so a terminating term skips the
    /// remaining statements of THIS txn without returning from the reactor.
    /// `None` in per-txn void functions (async/standalone/pre/callable) —
    /// there the term emits `ret void`. A bare `term;` / `term!;` is a
    /// convergence checkpoint and never uses this.
    pub void_txn_abort_label: Option<String>,

    // SSA state
    pub ssa_state_reg: Option<String>,
    pub param_slots: HashMap<String, String>,
    pub state_reg_name: String,

    // Arena allocator state (per-function)
    pub arena_slots: Option<(String, String, String)>,
    pub field_prealloc_info: HashMap<String, (String, String)>,

    // Whether the canonical loop bound is a compile-time constant
    // 2026-07-01: Enables post-inc comparison (counting-down loop) for
    // static bounds — LLVM can emit `add + jne` instead of `cmp + add + jl`.
    // For dynamic (runtime-determined) bounds, pre-inc comparison is used.
    pub is_static_bound: bool,

    // Accumulators flushed per-function
    pub pending_metadata: String,
    // 2026-07-03: Full Statement blocks hoisted from body, not just field+intrinsic
    // pairs. Allows hoisting guards whose swan song references let-bindings (e.g.
    // `energy` in nbody) by re-emitting the entire guard body post-loop.
    pub pending_post_hoist: Vec<Vec<Statement>>,
    // 2026-09-07 (swan-song dominance fix): let-local names referenced by the
    // pending post-hoist. In the countable-loop body each such `let` binds
    // through a preheader-flushed alloca (let_binding_allocas) instead of its
    // body-defined SSA register, so the exit-block hoisted print reads a
    // dominating slot holding the last-iteration value. Body-defined SSA
    // registers do not dominate the exit block (zero-trip path) — async-ready-
    // gate repro: `produced` (briev_await result) used by the post-loop print.
    pub swan_song_locals: std::collections::HashSet<String>,
    pub pending_cleanup: Vec<Statement>,
    // 2026-07-03: Native-typed backedge values for per-field phi loops.
    // Populated by emit_memory_field_store when it computes the typed value
    // (after ensure_typed_value).  Used by emit_countable_latch to avoid
    // reloading from %State — the typed register name is substituted directly
    // as the phi backedge operand, eliminating the store→GEP→load roundtrip.
    pub pending_phi_native_backedge: HashMap<String, String>,

    // 2026-07-04: Whether the EmitPerFieldPhi hot loop body must emit stores to %State
    // for the done: block to read. When false (the common case — no post-loop
    // hoisted guards), the field stores are suppressed. The phi registers and
    // pending_phi_native_backedge carry all values forward, and the latch uses
    // the native backedge registers directly. LLVM's optimizer sees a clean
    // phi loop with zero memory traffic — no dead stores for DSE to eliminate,
    // no barriers for the vectorizer.
    //
    // Dual-path architecture:
    //   Path A (false): Zero stores in the hot loop body. The loop body
    //     is a pure register pipeline (phi → compute → latch backedge).
    //     Used when done: does not read %State (no pending_post_hoist).
    //     Enables full vectorization and ILP scheduling.
    //   Path B (true): Stores emitted as before. Required when done:
    //     reads %State via GEP+load (post-loop hoisted guards from
    //     term! -> swan_song). The stores ensure done:'s loads see
    //     the final iteration's field values.
    //
    // Both paths must be preserved when refactoring. Removing Path A
    // regresses all EmitPerFieldPhi benchmarks by N dead stores per iteration
    // (N = field count). Removing Path B breaks term! swan song
    // correctness for benchmarks that print at convergence.
    pub needs_state_stores_in_body: bool,

    // 2026-07-04: Whether the current loop body is parallel-safe.
    // When true, emit_memory_field_store does NOT update ssa_old_*_regs
    // after & assignments — all reads continue to use the phi register
    // (old value).  This makes every computation independent of every
    // other, enabling LLVM's vectorizer to SIMD the entire body.
    //
    // Enabled for ALL bodies.  This restores the EmitInlineSsa struct-SSA behavior:
    // extractvalue from the state phi always gives old values, so all
    // computations are naturally independent.  The per-field phi loop
    // (EmitPerFieldPhi) broke this by updating ssa_old caches after each &
    // (correct per Briev semantics but creates artificial dependency
    // chains).  Parallel-safe mode restores the EmitInlineSsa independence.
    //
    // Exception: the counter field (tracked by counter_field_name) always
    // updates ssa_old_*_regs even in parallel-safe mode.  Guard conditions
    // like [count % 5000000 == 0] read the counter — they must see the
    // new value, not the old phi register.
    pub parallel_safe_body: bool,

    // 2026-07-04: Name of the loop counter field (the induction variable).
    // Set by emit_countable_main when entering the loop body.  This field
    // is exempt from parallel-safe mode — it always updates ssa_old_*_regs
    // so guard conditions like [count % N == 0] see the correct new value.
    pub counter_field_name: Option<String>,

    // 2026-07-04: State fields that guard conditions read.
    // Populated by scanning the body for Guarded statements and collecting
    // Expr::Identifier references.  These fields are exempt from parallel-
    // safe mode — they always update ssa_old_*_regs so guards see the
    // correct new values.  The counter_field_name is also implicitly exempt
    // (tracked separately as it's always the induction variable).
    // Guards containing TermBang (terminating guards) are excluded from
    // this scan — their bodies are hoisted and re-emitted post-loop, so
    // they don't need sequential updates within the loop body.
    pub parallel_safe_exempt_fields: HashSet<String>,

    // 2026-07-04: State fields that the done: block reads via
    // emit_hoisted_post_loop_prints. Populated by scanning hoisted
    // statements for Expr::Identifier references. When non-empty,
    // only these fields get stores emitted in Path B
    // (needs_state_stores_in_body=true). Empty set means "all fields
    // needed" (fallback — emit all stores).
    pub done_needs_fields: HashSet<String>,

    // 2026-07-04: Last-value temporaries (allocas) for phi commit block.
    // When the done: block reads state fields (hoisted prints), these
    // allocas store the phi's final value ONCE at loop exit (in the commit
    // block).  emit_hoisted_post_loop_prints loads from these instead of
    // from %State, eliminating per-iteration stores.  Maps field_name →
    // alloca register name.
    pub last_val_temps: HashMap<String, String>,

    // 2026-08-01 (B): the label of the block the emitter is currently writing
    // into. The countdown's guard body can end in a `when`, whose next_label
    // becomes the live block — the latch edge (br .cdl_) and the latch phis'
    // guard predecessor must use it, not `.cdg_`.
    pub cur_block: Option<String>,
    /// 2026-08-01 (E): inside a `vol let`'s RHS — state-field / Ptr loads
    /// emit `load volatile` so an MMIO address read is never cached or
    /// eliminated. Reset after the let.
    pub volatile_read: bool,
    /// 2026-08-01 (E): locals bound via `vol let x` — stores THROUGH them
    /// (`x[i] = v`, `*x = v`) emit `store volatile` (MMIO register writes).
    pub volatile_locals: std::collections::HashSet<String>,
    /// 2026-08-01 (D3): the `term <expr>` value register of a defn MEMBER body
    /// (`defn get(i: Int) -> T { term inner.data[i]; }`). emit_member_body
    /// returns it so `lst.get(0)` propagates the member's result (previously a
    /// fresh unused register — the caller bound an undefined value).
    pub member_result: Option<(String, crate::ast::Type)>,

    // 2026-07-17: Type of each last_val_temp entry. Parallel map so the
    // Identifier handler in emit_expr can return the correct Type when
    // reading a variable that was written earlier in the same body iteration.
    // Without this, let bindings (e.g. `let f0: Float = b0 * input;`) would
    // return Type::int() because f0 is not in field_index_map.
    pub last_val_types: HashMap<String, Type>,

    /// 2026-08-06 (Phase 8): closures bound by `let` in this function. A call
    /// to a closure name INLINES the body with the params bound to the arg
    /// registers; captured free variables resolve from the enclosing function
    /// scope. Because let-bound SSA registers are immutable, capture is
    /// by-value for let-bound immutables — matching the interpreter's closure
    /// semantics (Slice E). Escaping closures (stored, passed as arguments,
    /// returned) are not yet lowered — that needs fn_ptr + heap env blocks and
    /// is the documented boundary. Undo: remove the let-intercept and the
    /// emit_user_call inline branch; restore the Lambda arm's body emission.
    pub closure_lets: HashMap<String, ClosureDef>,

    // 2026-07-05: Fields participating in a body rotation pattern.
    // Forces body stores for these fields so the latch can reload them
    // via GEP, breaking circular phi chains for SCEV analysis.
    pub rotation_fields: HashSet<String>,

    // 2026-07-29: Active vector phi groups for the current countable body.
    // Populated by LlvmBackend::shape_vector_groups (mod.rs) from the frontend
    // LoopShape groups. emit_countable_main clears it — vector-phi emission is
    // deferred until the infrastructure handles all edge cases (counter.rs:236).
    pub(crate) active_vector_groups: Vec<crate::backend::llvm::vector_phi::VectorPhiGroup>,

    // 2026-07-29: Maps field_name → vector phi register name for extractelement.
    pub(crate) field_to_phi: HashMap<String, String>,

    // 2026-07-29: Maps field_name → lane index within its vector group.
    pub(crate) field_to_lane: HashMap<String, u32>,

    // 2026-07-05: Tracks the current accumulated vector value during body
    // emission for insertelement chaining.  Maps group-lane key
    // (format: "{name}-{lane_idx}") → the most recent lane value register.
    pub vector_phi_current: HashMap<String, String>,

    // Chimera tracking
    pub chimera_map: HashMap<String, ChimeraInfo>,

    // Expression hash-consing dedup cache.
    // 2026-07-01: Maps (op_string, lhs_reg, rhs_reg) → result_reg.
    // Prevents emitting the same instruction twice within a body emission scope.
    // Persists across let-bindings (not cleared between body statements) so that
    // sub-expressions like `dxe23*dxe23` that appear in multiple statements
    // (e.g., energy computation) reuse the emitted register.
    // Only caches "expensive" ops (fp ops, division) — not cheap integer add/sub.
    // Cleared at function entry alongside other caches.
    pub expr_dedup_cache: HashMap<(String, String, String), String>,

    // 2026-07-18: Allocation strategy per register name.
    // Populated by emit_alloc / emit_malloc, consulted by emit_free.
    // Keyed by the underlying SSA register (%t{N}) or alloca name.
    // Propagated through let-bindings in emit_statement.
    pub alloc_strategies: HashMap<String, AllocStrategy>,

    // 2026-07-18: Fat pointer provenance — base, offset, remaining registers.
    // Keyed by the fat pointer register name. Populated by emit_expr when
    // taking address-of (&s, &s[i], &buf[i]) and by Alloc#/Malloc#.
    // Consulted by Length# (reads remaining), Index# (adjusts offset/remaining),
    // and Deref# (bounds check against remaining).
    pub fat_ptrs: HashMap<String, (String, String, String)>,
}

impl FunctionContext {
    /// Generate a unique SSA register name: %tN
    pub fn gen_reg(&mut self) -> String {
        let n = self.txn_counter;
        self.txn_counter += 1;
        format!("%t{}", n)
    }

    pub fn new() -> Self {
        FunctionContext {
            txn_counter: 0,
            within_counter: 0,
            metadata_counter: 100,
            arena_counter: 0,
            let_bindings: HashMap::new(),
            let_binding_types: HashMap::new(),
            let_original_types: HashMap::new(),
            let_binding_allocas: HashSet::new(),
            reassigned_lets: HashMap::new(),
            boxed_scalar_regs: HashSet::new(),
            task_handle_regs: HashSet::new(),
            task_handle_names: HashSet::new(),
            pending_consumes: Vec::new(),
            struct_literal_allocas: HashMap::new(),
            pending_struct_allocas: Vec::new(),
            defer_struct_allocas: false,
            self_binding: None,
            self_prefix: None,
            foreach_break_labels: Vec::new(),
            reg_float_cache: HashMap::new(),
            reg_type_cache: HashMap::new(),
            ssa_old_int_regs: HashMap::new(),
            ssa_old_float_regs: HashMap::new(),
            pending_phi_backedge: HashMap::new(),
            phi_field_regs: HashMap::new(),
            backedge_field_regs: HashMap::new(),
            used_phi_loop: false,
            phi_induction_reg: None,
            loop_exit_label: None,
            terminated: false,
            is_task_segment: false,
            txn_name: String::new(),
            returns_i64: false,
            fn_ret_ty: "void".to_string(),
            main_body: false,
            in_callable_txn: false,
            callable_txn_result: None,
            callable_txn_post_label: None,
            convergence_target: None,
            defer_bodies: Vec::new(),
            void_txn_abort_label: None,
            ssa_state_reg: None,
            param_slots: HashMap::new(),
            state_reg_name: "%state".to_string(),
            arena_slots: None,
            field_prealloc_info: HashMap::new(),
            is_static_bound: false,
            pending_metadata: String::new(),
            pending_post_hoist: Vec::new(),
            swan_song_locals: std::collections::HashSet::new(),
            pending_cleanup: Vec::new(),
            pending_phi_native_backedge: HashMap::new(),
            // 2026-07-21: Default false enables Path A (zero memory traffic).
            // Set to true by dispatch when phi-capped fields need %State stores,
            // or by emit_countable_main when post-loop hoisted prints exist.
            needs_state_stores_in_body: false,
            parallel_safe_body: true,
            counter_field_name: None,
            parallel_safe_exempt_fields: HashSet::new(),
            done_needs_fields: HashSet::new(),
            last_val_temps: HashMap::new(),
            cur_block: None,
            volatile_read: false,
            volatile_locals: std::collections::HashSet::new(),
            member_result: None,
            last_val_types: HashMap::new(),
            closure_lets: HashMap::new(),
            rotation_fields: HashSet::new(),
            active_vector_groups: Vec::new(),
            field_to_phi: HashMap::new(),
            field_to_lane: HashMap::new(),
            vector_phi_current: HashMap::new(),

            chimera_map: HashMap::new(),
            expr_dedup_cache: HashMap::new(),
            alloc_strategies: HashMap::new(),
            fat_ptrs: HashMap::new(),
        }
    }

    /// Generate a unique SSA register name within this function.
    /// This is the SOLE source of register names — never use format!("%t{}", counter)
    /// outside this method. Guarantees no duplicate `%t{N}` definitions.
    pub fn next_reg(&mut self) -> String {
        let r = format!("%t{}", self.txn_counter);
        self.txn_counter += 1;
        r
    }

    /// Generate a unique label name within this function.
    pub fn next_label(&mut self, prefix: &str) -> String {
        let l = format!("{}_{}", prefix, self.txn_counter);
        self.txn_counter += 1;
        l
    }

    /// Generate a unique register with a custom prefix (for non-%t{N} names).
    /// Used by type-conversion helpers and specialized intrinsic emitters.
    pub fn next_reg_with_prefix(&mut self, prefix: &str) -> String {
        let r = format!("%{}{}", prefix, self.txn_counter);
        self.txn_counter += 1;
        r
    }

    /// Clear all local variable bindings (used at function entry).
    pub fn clear_locals(&mut self) {
        self.let_bindings.clear();
        self.let_binding_types.clear();
        self.let_original_types.clear();
        self.boxed_scalar_regs.clear();
        self.task_handle_regs.clear();
        self.task_handle_names.clear();
        self.is_task_segment = false;
        self.pending_consumes.clear();
        self.let_binding_allocas.clear();
        self.struct_literal_allocas.clear();
        self.pending_struct_allocas.clear();
        self.defer_struct_allocas = false;
        self.foreach_break_labels.clear();
        self.reg_float_cache.clear();
        self.reg_type_cache.clear();
        // 2026-08-18 (Phase E, BUGS.md SSA-main destructure): last_val_temps
        // is a PER-ITERATION "just written" cache — it must not survive an
        // emission-pass boundary. The alwaysinline @txn_go pass and the
        // SSA-main replay emit the SAME node body; a stale entry (e.g. a
        // foreach item name) made the replay's `let (k, v) = e` destructure
        // resolve `k` to a register owned by a LATER statement of the FIRST
        // pass (undefined forward reference → wrong hash probes). Clearing
        // here isolates the passes.
        self.last_val_temps.clear();
        self.last_val_types.clear();
    }

    /// Reset function state for a new function (keeps txn_counter if needed).
    pub fn reset(&mut self) {
        self.txn_counter = 0;
        self.within_counter = 0;
        self.clear_locals();
        self.terminated = false;
        self.returns_i64 = false;
        self.fn_ret_ty = "void".to_string();
        self.main_body = false;
        self.in_callable_txn = false;
        self.callable_txn_result = None;
        self.callable_txn_post_label = None;
        self.convergence_target = None;
        self.void_txn_abort_label = None;
        self.loop_exit_label = None;
        self.phi_induction_reg = None;
        self.arena_slots = None;
        self.field_prealloc_info.clear();
        self.pending_metadata.clear();
        self.pending_cleanup.clear();
        self.chimera_map.clear();
    }
}

// ── BlockContext ──────────────────────────────────────────────────────
//
// Per-basic-block lightweight context. Primarily tracks the current block
// label. In the future, this may track transient allocations that should
// be freed at block exit.
#[derive(Debug, Clone)]
pub struct BlockContext {
    pub label: String,
}

impl BlockContext {
    pub fn new(label: &str) -> Self {
        BlockContext {
            label: label.to_string(),
        }
    }

    pub fn entry() -> Self {
        BlockContext {
            label: "entry".to_string(),
        }
    }
}

// ── Function Guard ────────────────────────────────────────────────────
//
// Saves a FunctionContext snapshot for later restoration.
// Unlike the RAII pattern (which creates borrow-checker conflicts because
// it holds &mut FunctionContext while the caller also needs &mut self.fun),
// this guard is explicitly restored via restore().
//
// This is still safer than the original 7-field save/restore pattern because
// it snapshots ALL fields — adding new FunctionContext fields automatically
// protects them without editing the save/restore code.
//
// Usage:
//   let guard = FunctionGuard::new(&self.fun);
//   self.fun.terminated = false;
//   // ... modify self.fun extensively ...
//   guard.restore(&mut self.fun);
pub struct FunctionGuard {
    saved: FunctionContext,
}

impl FunctionGuard {
    pub fn new(fun: &FunctionContext) -> Self {
        FunctionGuard { saved: fun.clone() }
    }

    /// Restore the FunctionContext to the state captured at construction time.
    /// Call this after the inline body has been emitted.
    pub fn restore(self, fun: &mut FunctionContext) {
        *fun = self.saved;
    }

    // 2026-07-01: Restore all state EXCEPT SSA register counters.
    //
    // When inlining multiple txn bodies into the same function (e.g., in
    // emit_reactor's emit_inline_txn_body), restore() rewinds txn_counter
    // and arena_counter to the pre-body snapshot value. The second body then
    // emits identical register names (%dab263, %t7, etc.), causing "multiple
    // definition of local value" errors from opt.
    //
    // This variant preserves the monotonic counter invariants documented on
    // txn_counter ("NEVER rewound — prevents %t{N} collisions") and
    // arena_counter, while still restoring all other state (local bindings,
    // caches, phi state, flags).
    //
    // Trade-off: Register numbers grow monotonically across the full function
    // (~0.1% longer names at scale). No functional impact — LLVM normalizes
    // register names in its own passes.
    pub fn restore_preserve_counters(self, fun: &mut FunctionContext) {
        let txn_ct = fun.txn_counter;
        let arena_ct = fun.arena_counter;
        let within_ct = fun.within_counter;
        let md_ct = fun.metadata_counter;
        *fun = self.saved;
        fun.txn_counter = txn_ct;
        fun.arena_counter = arena_ct;
        fun.within_counter = within_ct;
        fun.metadata_counter = md_ct;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compiler_context_default_triple() {
        let ctx = CompilerContext::new();
        assert_eq!(ctx.target_triple, "x86_64-unknown-linux-gnu");
        assert_eq!(ctx.int_bits, 64);
        assert_eq!(ctx.pointer_bytes(), 8);
    }

    #[test]
    fn test_compiler_context_wasm_data_layout() {
        let wasm_dl =
            "e-m:e-p:32:32-p10:8:8-p20:8:8-i64:64-n32:64-S128-ni:1:10:20".to_string();
        assert_eq!(CompilerContext::parse_pointer_width(&wasm_dl), 32);
        assert_eq!(CompilerContext::parse_pointer_width(
            &"e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"),
            64);
    }
}
