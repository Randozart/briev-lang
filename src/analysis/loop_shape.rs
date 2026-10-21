// ── Loop Shape Analysis (frontend-driven dispatch, Phase 1) ──────────
//
// 2026-07-31: Compute the STRUCTURAL shape of every foldable bounded-counter
// transaction once, up front, so the LLVM backend's loop-emission dispatch
// (src/backend/llvm/mod.rs:2641-2861) becomes a deterministic switch instead
// of a body of heuristics.
//
// The shape is derived entirely from existing frontend results:
//   - bounded_pre / increments / write_set / purity  → transition_graph
//   - carried fields                                 → loop_carried
//   - vector phi candidates                          → slp_isomorphism
//   - swan song presence                             → swan_song
//
// No threshold (write_density, total_fields<8, total_fields>14) lives here —
// those are derived structurally. See docs/plans/2026-07-31-frontend-driven-dispatch.md §6.

use crate::analysis::loop_carried::FieldClass;
use crate::analysis::swan_song;
use crate::analysis::transition_graph::{BoundedPre, ConvergeDirection, ReactorTransitionGraph};
use crate::ast::{Expr, Statement, TopLevel, Transaction, Type};
use std::collections::{HashMap, HashSet};

/// How the loop counter's bound is known. Backend-agnostic: the backend maps
/// `Field`/`Const`/`Literal` to its own index/const tables at consumption time.
#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    /// bound_var is a state field (has its own %State slot).
    Field(String),
    /// bound_var is a global constant (`const N = ...`).
    Const(String),
    /// bound_var is a runtime-seeded invariant (`init N: Int = ...`).
    /// 2026-08-09 (init kind, Phase 3): provably invariant for the run (set
    /// once before beginprogram), so folding a loop against it is sound — the
    /// backend loads the seeded global as the bound (no Unknown fallback).
    Init(String),
    /// bound is a compile-time literal.
    Literal(i64),
    /// bound_var is neither a state field nor a constant — backend must decide.
    Unknown(String),
}

/// A group of fields that can share a single vector phi node.
///
/// Structural only: same-type and power-of-2 gates that depend on LLVM type
/// knowledge stay in the backend (which converts this to its VectorPhiGroup).
#[derive(Debug, Clone, PartialEq)]
pub struct VectorGroup {
    /// Descriptive name derived from the common field prefix.
    pub name: String,
    /// Number of lanes in the group (power of two).
    pub width: usize,
    /// Field names in index order.
    pub fields: Vec<String>,
}

/// The structural convergence of a loop's exit.
#[derive(Debug, Clone, PartialEq)]
pub enum Convergence {
    /// counter >= bound is the convergence proof.
    CounterGeBound { counter: String, bound: Bound },
    /// The program carries an explicit exit condition (#!exit / term!).
    Explicit(Expr),
    /// Not provable — a conservative reactor loop must be emitted.
    Unprovable,
}

/// The structural shape of a foldable bounded-counter transaction.
#[derive(Debug, Clone)]
pub struct LoopShape {
    /// Transaction / node name.
    pub txn_name: String,
    /// Loop counter field (bounded_pre.var == increments.var).
    pub counter: String,
    /// Loop bound resolution.
    pub bound: Bound,
    /// Counter direction (increasing vs decreasing).
    pub direction: ConvergeDirection,
    /// True when the body writes ONLY the loop counter (write_set == {counter}).
    pub counter_only_writes: bool,
    /// Loop-carried fields in deterministic (sorted) order — the minimal phi set.
    pub carried_fields: Vec<String>,
    /// 2026-10-20 (Local class): written fields whose only reads are inside
    /// this transaction's loop body/guards. They need phis for the loop and
    /// guard, but no post-loop %State commit: once the loop ends, their values
    /// are unobservable. Excludes any field read by a contract, another txn,
    /// a defn, or a top-level expression.
    pub local_fields: Vec<String>,
    /// Isomorphic field groups eligible for vector-phi promotion.
    pub vector_groups: Vec<VectorGroup>,
    /// 2026-10-21 (target-pressure effect shape): loop-carried fields whose
    /// declared/inferred type is floating-point. Presence selects the escaped
    /// effect shape unless every Float is temp-mediated (see below).
    pub carried_float_count: usize,
    /// Loop-carried scalar fields (float + integer/bool/char/pointer slots).
    /// Aggregates are excluded; the selector uses the float subset today.
    pub carried_scalar_count: usize,
    /// Loop-carried aggregate/memory-resident fields. A non-zero count keeps
    /// the stateless effect shape: escaping `%State` blocks SROA of the hot
    /// aggregate (the queue_drain case).
    pub carried_aggregate_count: usize,
    /// 2026-10-21 (target-pressure effect shape): carried Float fields whose
    /// value is assigned from a body-local let (`f = n;` after `let n = ...`).
    /// A fully temp-mediated body enables the stateless SROA/vector shape;
    /// direct field arithmetic takes the escaped effect shape.
    pub temp_mediated_float_count: usize,
    /// True when the body ends with a swan song (`term! -> print`).
    pub has_swan_song: bool,
    /// True when the body is provably pure (or effectively pure).
    pub is_pure: bool,
    /// Structured convergence (not a backend-synthesized Expr).
    pub convergence: Convergence,
}

/// Program-level convergence: whether the whole program is guaranteed to exit.
#[derive(Debug, Clone)]
pub struct ProgramConvergence {
    /// (counter, bound) pairs for every foldable reactive txn, ANDed.
    /// The bound is an Expr so a literal bound (`[ticksA < 3]`) carries its
    /// VALUE (`Decimal(3)`) — a synthetic `__lit__ticksA` name would emit an
    /// undefined `@__lit__ticksA` global in the exit check
    /// (2026-08-08, two-node reactor fix). Empty when the program has an
    /// explicit exit condition or cannot exit.
    pub counter_ge_bounds: Vec<(String, Expr)>,
    /// True when a natural exit was derived (no explicit #!exit present).
    pub has_natural_exit: bool,
}

/// Build a per-txn LoopShape for every reactive bounded-counter transaction
/// whose counter and increment match (`bounded_pre.var == increments.var`).
///
/// Only reactive, foldable txns get a shape — callable txns (defn-style) are
/// emitted through the plain function path, not the loop dispatch.
///
/// 2026-07-31: Phase 3 (§8.1) — `min_width` (vector-phi promotion gate) is
/// config-driven: config/targets.dbvl `vector_min_width` for the target triple.
pub fn build_loop_shapes(
    graph: &ReactorTransitionGraph,
    items: &[TopLevel],
    min_width: usize,
) -> HashMap<String, LoopShape> {
    let names = ItemNames::collect(items);
    let txns = collect_txns(items);

    let mut shapes = HashMap::new();
    for node in &graph.nodes {
        if !node.is_reactive {
            continue;
        }
        let (Some(bp), Some(inc)) = (&node.bounded_pre, &node.increments) else {
            continue;
        };
        if bp.var != inc.var {
            continue;
        }
        let Some(txn) = txns.get(&node.name) else {
            continue;
        };
        shapes.insert(node.name.clone(), build_shape(node, txn, &names, min_width, items));
    }
    shapes
}

/// Compute the program-level natural exit for programs without an explicit
/// `#!exit`. Mirrors the old synthetic-exit construction (mod.rs:2600-2639):
/// when EVERY reactive txn is foldable (bounded_pre + increments), the program
/// exits once all counters reach their bounds.
pub fn program_convergence(
    graph: &ReactorTransitionGraph,
    items: &[TopLevel],
    has_explicit_exit: bool,
) -> ProgramConvergence {
    if has_explicit_exit {
        return ProgramConvergence {
            counter_ge_bounds: Vec::new(),
            has_natural_exit: false,
        };
    }
    let txns = collect_txns(items);
    let has_persistent_txn = txns.values().any(|t| {
        t.is_reactive && !graph.nodes.iter().any(|n| {
            n.name == t.name && n.bounded_pre.is_some() && n.increments.is_some()
        })
    });
    if has_persistent_txn {
        return ProgramConvergence {
            counter_ge_bounds: Vec::new(),
            has_natural_exit: false,
        };
    }
    let mut counter_ge_bounds: Vec<(String, Expr)> = Vec::new();
    for (name, t) in &txns {
        if !t.is_reactive {
            continue;
        }
        let Some(node) = graph.nodes.iter().find(|n| n.name == *name) else {
            continue;
        };
        let (Some(bp), Some(inc)) = (&node.bounded_pre, &node.increments) else {
            continue;
        };
        if bp.var == inc.var {
            // 2026-08-08 (two-node reactor fix): a literal bound
            // (`[ticksA < 3]`) is carried by bp.bound_literal; a field/const
            // bound by bp.bound_var. Emit the VALUE, not a synthetic name —
            // the synthetic `__lit__ticksA` has no backing global and the exit
            // check would reference an undefined `@__lit__ticksA`.
            let bound_expr = match bp.bound_literal {
                Some(n) => Expr::Decimal(n),
                None => Expr::Identifier(bp.bound_var.clone()),
            };
            counter_ge_bounds.push((bp.var.clone(), bound_expr));
        }
    }
    // 2026-09-30 (BUGS.md: async exit-condition order nondeterminism):
    // `txns` comes from `collect_txns` (a HashMap), so this loop pushed
    // (counter, bound) pairs in hash order and the `.reduce(And)` in
    // `emit_toplevel` built the synthetic exit in that random order —
    // the same compiler emitted swapped `gep_exit*` field reads
    // run-to-run (seen in the stage-5 5c IR A/B as
    // `async_counters_idio` instability). The AND is commutative, but
    // emission must be byte-stable (house rule §4): sort by counter
    // name. Undo: remove this sort — the run-to-run .ll drift returns.
    counter_ge_bounds.sort_by(|a, b| a.0.cmp(&b.0));
    let has_natural_exit = !counter_ge_bounds.is_empty();
    ProgramConvergence {
        counter_ge_bounds,
        has_natural_exit,
    }
}

/// Build a single LoopShape from a transition-graph node and its transaction.
fn build_shape(
    node: &crate::analysis::transition_graph::ReactorNode,
    txn: &Transaction,
    names: &ItemNames,
    min_width: usize,
    items: &[TopLevel],
) -> LoopShape {
    let bp = node.bounded_pre.as_ref().unwrap();
    let bound = resolve_bound(bp, names);
    let counter_only_writes = node.write_set.len() == 1 && node.write_set.contains(&bp.var);
    let carried_fields = classify_carried(node.write_set.clone(), txn);
    let local_fields = classify_local_fields(node, txn, items);
    let field_pressure = classify_field_pressure(&carried_fields, items);
    let vector_groups = detect_vector_groups_structural(txn, node.write_set.clone(), &names.state_fields, min_width);
    let temp_mediated_float_count =
        count_temp_mediated_float_writes(txn, &carried_fields, items);
    let has_swan = swan_song::has_swan_song(&txn.body);
    let is_pure = node.is_pure_body || node.is_effectively_pure;
    LoopShape {
        txn_name: node.name.clone(),
        counter: bp.var.clone(),
        bound: bound.clone(),
        direction: bp.direction,
        counter_only_writes,
        carried_fields,
        local_fields,
        vector_groups,
        carried_float_count: field_pressure.float,
        carried_scalar_count: field_pressure.scalar,
        carried_aggregate_count: field_pressure.aggregate,
        temp_mediated_float_count,
        has_swan_song: has_swan,
        is_pure,
        convergence: Convergence::CounterGeBound {
            counter: bp.var.clone(),
            bound,
        },
    }
}

/// Resolve a bound_var to a structured Bound using the item-level knowledge
/// available to the analysis (state fields vs constants), matching the
/// backend's `total_idx` / `total_const_name` resolution exactly.
fn resolve_bound(
    bp: &BoundedPre,
    names: &ItemNames,
) -> Bound {
    if let Some(lit) = bp.bound_literal {
        return Bound::Literal(lit);
    }
    if names.state_fields.contains(&bp.bound_var) {
        return Bound::Field(bp.bound_var.clone());
    }
    if names.consts.contains(&bp.bound_var) {
        return Bound::Const(bp.bound_var.clone());
    }
    if names.inits.contains(&bp.bound_var) {
        return Bound::Init(bp.bound_var.clone());
    }
    Bound::Unknown(bp.bound_var.clone())
}

/// The item-level name sets a loop bound can resolve against: state fields,
/// compile-time constants, and runtime-seeded invariants.
struct ItemNames {
    state_fields: HashSet<String>,
    consts: HashSet<String>,
    inits: HashSet<String>,
}

impl ItemNames {
    fn collect(items: &[TopLevel]) -> ItemNames {
        ItemNames {
            state_fields: collect_state_fields(items),
            consts: collect_const_names(items),
            inits: collect_init_names(items),
        }
    }
}

/// 2026-10-20 (Local class): fields written by the loop whose values are
/// unobservable after it. A field is local only when every reference outside
/// this txn's BODY is absent; its contract references still count as
/// observable, and any other txn/defn/top-level expression that names it
/// keeps it machine state.
fn classify_local_fields(
    node: &crate::analysis::transition_graph::ReactorNode,
    txn: &Transaction,
    items: &[TopLevel],
) -> Vec<String> {
    let mut external: HashSet<String> = HashSet::new();
    // This txn's contract is observable.
    crate::analysis::storage_class::collect_raw_idents(
        &txn.contract.pre_condition,
        &mut external,
    );
    crate::analysis::storage_class::collect_raw_idents(
        &txn.contract.post_condition,
        &mut external,
    );
    for item in items {
        if let TopLevel::Transaction(other) = item {
            if other.name == txn.name {
                // The current transaction's BODY is the loop itself; only its
                // contract is an external observation. Every other top-level
                // surface is walked below.
                continue;
            }
        }
        // One generic walker covers every TopLevel form: other txns/defns,
        // exports, cells, trigger bindings, when-laws, assertions, fuzzed
        // items, type/operator bodies, compile-time items, cfg/sync groups,
        // and initialization expressions. Missing a surface here would
        // misclassify a field as Local and skip its exit store.
        crate::ast::visit::walk_toplevel(item, &mut |e| {
            crate::analysis::storage_class::collect_raw_idents(e, &mut external)
        });
    }
    let mut local: Vec<String> = node
        .write_set
        .iter()
        .filter(|f| !external.contains(*f))
        .cloned()
        .collect();
    local.sort();
    local
}

/// Structural pressure classes for the effect-shape selector. Derived from
/// source-level types only (no LLVM type names, no type-name matching in the
/// backend); an unknown type is conservatively aggregate so a high-float
/// program never escapes state through an opaque object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressureClass {
    Float,
    Scalar,
    Aggregate,
}

#[derive(Debug, Clone, Copy, Default)]
struct FieldPressure {
    float: usize,
    scalar: usize,
    aggregate: usize,
}

/// Count loop-carried fields by pressure class. The type map is built from
/// top-level `let`/`state` declarations; an untyped literal initializer is
/// classified from its literal tag (the parser's inference shape), and
/// anything opaque is aggregate (fail-safe: no escaped effect).
fn classify_field_pressure(fields: &[String], items: &[TopLevel]) -> FieldPressure {
    let (types, exprs) = field_decls(items);
    let mut out = FieldPressure::default();
    for f in fields {
        match pressure_class(types.get(f).and_then(|t| t.as_ref()), exprs.get(f)) {
            PressureClass::Float => out.float += 1,
            PressureClass::Scalar => out.scalar += 1,
            PressureClass::Aggregate => out.aggregate += 1,
        }
    }
    // Scalar pressure includes floats: a float is a scalar slot too.
    out.scalar += out.float;
    out
}

/// Field declaration types + initializers keyed by name. Used by the
/// pressure classifier and the temp-mediated write count.
fn field_decls(items: &[TopLevel]) -> (HashMap<String, Option<Type>>, HashMap<String, Expr>) {
    let mut types: HashMap<String, Option<Type>> = HashMap::new();
    let mut exprs: HashMap<String, Expr> = HashMap::new();
    for item in items {
        match item {
            TopLevel::StateDecl(s) => {
                types.insert(s.name.clone(), Some(s.ty.clone()));
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, ty, expr, .. } = stmt.as_ref() {
                    types.insert(name.clone(), ty.clone());
                    if let Some(e) = expr {
                        exprs.insert(name.clone(), e.clone());
                    }
                }
            }
            _ => {}
        }
    }
    (types, exprs)
}

/// Classify one field type. `None` falls back to the literal initializer's
/// tag; no tag means opaque/aggregate. This deliberately has no target or
/// backend dependency so the shape stays frontend-derived.
fn pressure_class(ty: Option<&Type>, expr: Option<&Expr>) -> PressureClass {
    if let Some(ty) = ty {
        return type_pressure_class(ty);
    }
    match expr {
        Some(Expr::Float(_)) => PressureClass::Float,
        Some(Expr::Decimal(_)) | Some(Expr::Bool(_)) | Some(Expr::Char(_)) => {
            PressureClass::Scalar
        }
        _ => PressureClass::Aggregate,
    }
}

fn type_pressure_class(ty: &Type) -> PressureClass {
    match ty {
        Type::Bits(_) => PressureClass::Scalar,
        Type::Ptr(_) | Type::PtrConst(_) => PressureClass::Scalar,
        Type::Custom(name) => match name.as_str() {
            "Float" | "Float16" | "Float32" | "Float64" | "Half" | "Double" => {
                PressureClass::Float
            }
            "Int" | "UInt" | "Int8" | "Int16" | "Int32" | "Int64" | "Int128"
            | "UInt8" | "UInt16" | "UInt32" | "UInt64" | "UInt128" | "Bool" | "Char"
            | "String" | "Blob" | "Data" => PressureClass::Scalar,
            _ => PressureClass::Aggregate,
        },
        _ => PressureClass::Aggregate,
    }
}

/// Count carried Float fields assigned from a body-local let (`f = n;`).
/// A body that first computes every next value into a let and then copies it
/// into the fields exposes the old-value/vectorizable shape (matrix_pipeline,
/// sweep_dense); direct field arithmetic does not (float_math, telemetry,
/// pid, sweep_sparse).
fn count_temp_mediated_float_writes(
    txn: &Transaction,
    carried: &[String],
    items: &[TopLevel],
) -> usize {
    let lets = body_let_names(&txn.body);
    let carried_float = carried_float_names(carried, items);
    let mut seen: HashSet<&str> = HashSet::new();
    for stmt in &txn.body {
        let Statement::Assign(lhs, Expr::Identifier(rhs)) = stmt else {
            continue;
        };
        let Some(field) = lhs.as_var_name() else { continue };
        if carried_float.contains(field) && lets.contains(rhs) {
            seen.insert(field);
        }
    }
    seen.len()
}

fn body_let_names(body: &[Statement]) -> HashSet<String> {
    body.iter()
        .filter_map(|stmt| match stmt {
            Statement::Let { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn carried_float_names(carried: &[String], items: &[TopLevel]) -> HashSet<String> {
    let (types, exprs) = field_decls(items);
    carried
        .iter()
        .filter(|f| {
            matches!(
                pressure_class(types.get(*f).and_then(|t| t.as_ref()), exprs.get(*f)),
                PressureClass::Float
            )
        })
        .cloned()
        .collect()
}

/// Return the first transaction (sorted for determinism) that needs the
/// escaped effect-memory shape.
///
/// The 2026-10-21 N=1..20 sweep falsified the field-count pressure crossover:
/// every scalar-Float loop in the countdown/periodic-effect shape improved
/// with the escaped context, even at N=1, while Int-only loops were neutral
/// and aggregate-carried loops (queue_drain) preferred stateless print.
/// The causal shape feature is direct field arithmetic vs old-value temp
/// capture: a body that computes every next value into a local let before
/// copying it into the field keeps the stateless SROA/vector shape, while
/// direct arithmetic needs the escaped context. `float_registers -
/// fp_reserved_temps` is used only as a target capability gate: if the target
/// reserves all FP registers (or has a virtual register file), there is no
/// physical spill shape to escape and the stateless path stays.
///
/// Aggregate-carried loops are excluded: escaping `%State` there blocks SROA
/// of the hot aggregate (queue_drain/interval_step). The caller consumes this
/// as a program-level effect-memory decision.
pub fn escaped_effect_cause(
    shapes: &HashMap<String, LoopShape>,
    float_registers: usize,
    fp_reserved_temps: usize,
) -> Option<String> {
    // Virtual-register targets (wasm row is u32::MAX) have no physical FP
    // pressure to escape; keep the stateless ABI.
    if float_registers >= u32::MAX as usize {
        return None;
    }
    if float_registers.saturating_sub(fp_reserved_temps) == 0 {
        return None;
    }
    let mut names: Vec<&String> = shapes.keys().collect();
    names.sort();
    for name in names {
        let shape = &shapes[name];
        if shape.carried_aggregate_count != 0 || shape.carried_float_count == 0 {
            continue;
        }
        // A fully temp-mediated Float body (`f = n;` for every carried field,
        // n computed first) exposes the old-value/vectorizable shape: the
        // stateless SROA path wins (matrix_pipeline, sweep_dense). Direct
        // field arithmetic needs the escaped context (float_math, telemetry,
        // pid, sweep_sparse). This is structural, not a benchmark threshold.
        let fully_temp_mediated = shape.temp_mediated_float_count == shape.carried_float_count;
        if !fully_temp_mediated {
            return Some(name.clone());
        }
    }
    None
}

/// Classify the loop-carried field set (minimal-state, deterministic order).
/// A field is carried when it is written in the loop AND read by the body,
/// a contract expression, or an observable body (guards / swan song).
fn classify_carried(write_set: HashSet<String>, txn: &Transaction) -> Vec<String> {
    let contract_exprs = [&txn.contract.pre_condition, &txn.contract.post_condition];
    let observables = collect_observable_bodies(&txn.body);
    let classes = crate::analysis::loop_carried::classify_fields(
        &write_set,
        &txn.body,
        &contract_exprs,
        &observables,
    );
    let mut carried: Vec<String> = classes
        .into_iter()
        .filter(|(_, c)| *c == FieldClass::LoopCarried)
        .map(|(k, _)| k)
        .collect();
    carried.sort();
    carried
}

/// Collect all guarded bodies (runtime `when` blocks) from a transaction body.
/// Fields read by them must survive as loop-carried because the guard executes
/// inside the loop.
fn collect_observable_bodies(body: &[Statement]) -> Vec<&[Statement]> {
    let mut out: Vec<&[Statement]> = Vec::new();
    for stmt in body {
        match stmt {
            Statement::Guarded(_, stmts) => out.push(stmts.as_slice()),
            _ => {}
        }
    }
    out
}

/// Detect vector-phi groups structurally from the swan-song-stripped body.
/// Mirrors the backend's former `detect_vector_groups` minus the LLVM-type gate
/// (which the backend applies when converting VectorGroup → VectorPhiGroup via
/// LlvmBackend::shape_vector_groups).
fn detect_vector_groups_structural(
    txn: &Transaction,
    write_set: HashSet<String>,
    state_fields: &HashSet<String>,
    min_width: usize,
) -> Vec<VectorGroup> {
    let (stripped, _hoist) = swan_song::hoist_swan_song(&txn.body, state_fields);
    let mut candidates = crate::analysis::slp_isomorphism::analyze_body(&stripped, min_width);
    // Sort by width descending so the LARGEST group is processed first.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.width));
    let mut accepted_fields: HashSet<String> = HashSet::new();
    let mut groups: Vec<VectorGroup> = Vec::new();
    for c in candidates {
        // All fields must be unconditionally written (in write_set).
        if !c.fields.iter().all(|f| write_set.contains(f)) {
            continue;
        }
        // LLVM only supports power-of-2 vector widths.
        if c.width.count_ones() != 1 {
            continue;
        }
        // Skip groups with duplicate field names.
        let mut seen_fields: HashSet<&str> = HashSet::new();
        if c.fields.iter().any(|f| !seen_fields.insert(f)) {
            continue;
        }
        // Skip if ANY field is already in an accepted group (no overlap).
        if c.fields.iter().any(|f| accepted_fields.contains(f)) {
            continue;
        }
        for f in &c.fields {
            accepted_fields.insert(f.clone());
        }
        groups.push(VectorGroup {
            name: c.group_name.clone(),
            width: c.width,
            fields: c.fields.clone(),
        });
    }
    groups
}

/// Collect state field names. The parser emits top-level `let` declarations
/// as `TopLevel::Statement(Let)`; legacy state declarations may appear as
/// `TopLevel::StateDecl`. The LLVM backend's `build_field_index` accepts both
/// (mod.rs:3634, 3715), so the analysis must too.
///
/// 2026-07-31: `pub(crate)` — reused by `backend::build_swan_songs` so the
/// swan-song hoist's let-to-field remap gates on the same field set the
/// backend's `field_index_map` uses. A mismatch would let a top-level `let`
/// state field be treated as a local and break the remap (Phase 1b).
pub(crate) fn collect_state_fields(items: &[TopLevel]) -> HashSet<String> {
    let mut fields = HashSet::new();
    for item in items {
        match item {
            TopLevel::StateDecl(s) => {
                fields.insert(s.name.clone());
            }
            TopLevel::Statement(stmt) => {
                if let Statement::Let { name, .. } = stmt.as_ref() {
                    fields.insert(name.clone());
                }
            }
            _ => {}
        }
    }
    fields
}

/// Collect constant names from `const` declarations (TopLevel::Constant).
fn collect_const_names(items: &[TopLevel]) -> HashSet<String> {
    let mut consts = HashSet::new();
    for item in items {
        if let TopLevel::Constant(c) = item {
            consts.insert(c.name.clone());
        }
    }
    consts
}

/// Collect runtime-seeded invariant names from `init` declarations
/// (TopLevel::Init). 2026-08-09 (init kind, Phase 3): an init is provably
/// invariant for the run, so a loop folded against it is sound.
fn collect_init_names(items: &[TopLevel]) -> HashSet<String> {
    let mut inits = HashSet::new();
    for item in items {
        if let TopLevel::Init(i) = item {
            inits.insert(i.name.clone());
        }
    }
    inits
}

/// Collect all transactions by name.
fn collect_txns(items: &[TopLevel]) -> HashMap<String, &Transaction> {
    // 2026-08-09 (Bug 2): use the effective transactions (direct +
    // sync<group>/export-wrapped) so a sync-group node gets a loop shape and
    // can fold — otherwise it had no shape and the fold machinery skipped it.
    let mut txns = HashMap::new();
    for t in crate::analysis::effective_txns(items) {
        txns.insert(t.name.clone(), t);
    }
    txns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_program(src: &str) -> Vec<TopLevel> {
        let tokens = crate::lexer::tokenize(src).unwrap();
        let mut p = crate::parser::Parser::new(tokens, src);
        p.parse_program().unwrap()
    }

    fn graph_and_items(src: &str) -> (ReactorTransitionGraph, Vec<TopLevel>) {
        let items = parse_program(src);
        let graph = ReactorTransitionGraph::build(&items, &None, &vec![]);
        (graph, items)
    }

    #[test]
    fn test_float_math_local_fields() {
        let src = include_str!("../../benchmarks/float_math.bv");
        let (graph, items) = graph_and_items(src);
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.get("tick").expect("tick shape");
        println!("float_math local_fields={:?}", shape.local_fields);
        assert!(!shape.has_swan_song, "term; is not a swan song");
        for f in [
            "x0", "x1", "x2", "p00", "p01", "p02", "p10", "p11", "p12", "p20",
            "p21", "p22",
        ] {
            assert!(
                shape.local_fields.contains(&f.to_string()),
                "{} should be loop-local, got {:?}", f, shape.local_fields
            );
        }
        assert!(!shape.local_fields.contains(&"count".to_string()));
        assert!(!shape.local_fields.contains(&"total".to_string()));
        assert_eq!(shape.carried_float_count, 12);
        assert_eq!(shape.carried_scalar_count, 13);
        assert_eq!(shape.carried_aggregate_count, 0);
    }

    #[test]
    fn test_effect_shape_selector_float_presence() {
        let src = include_str!("../../benchmarks/float_math.bv");
        let (graph, items) = graph_and_items(src);
        let shapes = build_loop_shapes(&graph, &items, 4);
        let mut map = HashMap::new();
        let shape = shapes.get("tick").expect("tick shape").clone();
        map.insert("tick".to_string(), shape.clone());
        assert_eq!(
            escaped_effect_cause(&map, 16, 4),
            Some("tick".to_string()),
            "any carried Float selects the escaped effect shape"
        );
        let mut one = shape;
        one.carried_float_count = 1;
        map.insert("tick".to_string(), one);
        assert_eq!(
            escaped_effect_cause(&map, 16, 4),
            Some("tick".to_string()),
            "the crossover is Float presence, not a field-count threshold"
        );
    }

    #[test]
    fn test_effect_shape_selector_temp_mediated_stays_stateless() {
        let src = include_str!("../../benchmarks/matrix_pipeline.bv");
        let (graph, items) = graph_and_items(src);
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.get("tick").expect("tick shape").clone();
        assert_eq!(shape.carried_float_count, 16);
        assert_eq!(shape.temp_mediated_float_count, 16);
        let mut map = HashMap::new();
        map.insert("tick".to_string(), shape);
        assert_eq!(escaped_effect_cause(&map, 16, 4), None,
            "a fully temp-mediated body keeps the stateless SROA/vector shape");
    }

    #[test]
    fn test_local_fields_export_keeps_state() {
        let src = r#"
let f: Float = 0.0;
let count: Int = 0;
let total: Int = 10;
export defn read_f() -> Float [true][true] { term f; };
node tick [count < total][count == total] {
    f = f + 1.0;
    count = count + 1;
    term;
};
"#;
        let (graph, items) = graph_and_items(src);
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.get("tick").expect("tick shape");
        assert!(!shape.local_fields.contains(&"f".to_string()),
            "a field referenced by an exported defn is not loop-local");
        assert!(!shape.local_fields.contains(&"count".to_string()));
    }

    #[test]
    fn test_effect_shape_mixed_program_shape_facts() {
        let src = include_str!("../../tests/tier1/mixed_effect_shape.bv");
        let (graph, items) = graph_and_items(src);
        let shapes = build_loop_shapes(&graph, &items, 4);
        let direct = shapes.get("direct").expect("direct shape");
        let temp = shapes.get("temp").expect("temp shape");
        assert_eq!(direct.temp_mediated_float_count, 0,
            "direct arithmetic is not temp-mediated");
        assert_eq!(temp.temp_mediated_float_count, temp.carried_float_count,
            "temp loop captures every next value in a let");
        assert_eq!(
            escaped_effect_cause(&shapes, 16, 4),
            Some("direct".to_string()),
            "the mixed program selects escaped because the direct loop needs it"
        );
    }

    #[test]
    fn test_effect_shape_selector_no_float_and_capability_gates() {
        let src = include_str!("../../benchmarks/float_math.bv");
        let (graph, items) = graph_and_items(src);
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.get("tick").expect("tick shape").clone();
        let mut no_float = shape.clone();
        no_float.carried_float_count = 0;
        no_float.carried_scalar_count = 1;
        let mut map = HashMap::new();
        map.insert("tick".to_string(), no_float);
        assert_eq!(escaped_effect_cause(&map, 16, 4), None,
            "Int-only loops keep stateless print");
        let mut agg = shape.clone();
        agg.carried_aggregate_count = 1;
        map.insert("tick".to_string(), agg);
        assert_eq!(escaped_effect_cause(&map, 16, 4), None,
            "aggregate-carried loops keep the stateless effect shape");
        map.insert("tick".to_string(), shape);
        assert_eq!(escaped_effect_cause(&map, 4, 4), None,
            "a target that reserves all FP registers has no escaped shape");
        assert_eq!(escaped_effect_cause(&map, u32::MAX as usize, 4), None,
            "virtual-register targets have no physical spill shape");
    }

    #[test]
    fn test_counter_only_writes_shape() {
        let (graph, items) = graph_and_items(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             node work [count < bound][count == bound] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let shapes = build_loop_shapes(&graph, &items, 4);
        assert_eq!(shapes.len(), 1, "one foldable txn expected");
        let shape = shapes.values().next().unwrap();
        assert_eq!(shape.counter, "count");
        assert_eq!(shape.bound, Bound::Field("bound".to_string()));
        assert!(shape.counter_only_writes, "only the counter is written");
        assert_eq!(shape.direction, ConvergeDirection::Increasing);
        assert!(!shape.has_swan_song);
    }

    #[test]
    fn test_counter_only_writes_false_with_second_field() {
        let (graph, items) = graph_and_items(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             let acc: Int = 0;\n\
             node work [count < bound][count == bound] {\n\
               acc = acc + count;\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.values().next().unwrap();
        assert!(!shape.counter_only_writes, "acc write breaks counter-only");
        // acc and count are both loop-carried (count is read by the contract).
        assert_eq!(shape.carried_fields, vec!["acc".to_string(), "count".to_string()]);
    }

    #[test]
    fn test_literal_bound() {
        let (graph, items) = graph_and_items(
            "let count: Int = 0;\n\
             node work [count < 50][count == 50] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.values().next().unwrap();
        assert_eq!(shape.bound, Bound::Literal(50));
    }

    #[test]
    fn test_vector_groups_detected() {
        // Two isomorphic groups (bx0..bx3, by0..by3) with identical structure.
        let (graph, items) = graph_and_items(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             let bx0: Int = 0;\n\
             let bx1: Int = 1;\n\
             let bx2: Int = 2;\n\
             let bx3: Int = 3;\n\
             let by0: Int = 0;\n\
             let by1: Int = 1;\n\
             let by2: Int = 2;\n\
             let by3: Int = 3;\n\
             node work [count < bound][count == bound] {\n\
               bx0 = bx0 + 1; by0 = by0 + 1;\n\
               bx1 = bx1 + 1; by1 = by1 + 1;\n\
               bx2 = bx2 + 1; by2 = by2 + 1;\n\
               bx3 = bx3 + 1; by3 = by3 + 1;\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.values().next().unwrap();
        // At least one isomorphic 4+ lane group must be detected.
        assert!(!shape.vector_groups.is_empty(), "expected vector groups");
        for g in &shape.vector_groups {
            assert_eq!(g.width.count_ones(), 1, "width must be a power of 2");
            assert!(g.width >= 4, "analyze_body filters width >= 4");
            assert_eq!(g.fields.len(), g.width, "one lane per field");
        }
    }

    #[test]
    fn test_swan_song_flag() {
        let (graph, items) = graph_and_items(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             let result: Int = 42;\n\
             node work [count < bound][count == bound] {\n\
               count = count + 1;\n\
               when count == bound { endprogram PrintLn!(result); };\n\
               term;\n\
             };\n",
        );
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.values().next().unwrap();
        assert!(shape.has_swan_song);
    }

    #[test]
    fn test_program_convergence_natural_exit() {
        let (graph, items) = graph_and_items(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             node work [count < bound][count == bound] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let conv = program_convergence(&graph, &items, false);
        assert!(conv.has_natural_exit);
        assert_eq!(
            conv.counter_ge_bounds,
            vec![("count".to_string(), Expr::Identifier("bound".to_string()))]
        );
    }

    #[test]
    fn test_program_convergence_literal_bound_uses_value() {
        // 2026-08-08 (two-node reactor fix): a literal countdown bound must
        // converge via its VALUE (`count >= 3`), never a synthetic
        // `__lit__count` name that would reference an undefined global.
        let (graph, items) = graph_and_items(
            "let count: Int = 0;\n\
             node work [count < 3][count == 3] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let conv = program_convergence(&graph, &items, false);
        assert!(conv.has_natural_exit);
        assert_eq!(
            conv.counter_ge_bounds,
            vec![("count".to_string(), Expr::Decimal(3))]
        );
    }

    #[test]
    fn test_program_convergence_explicit_exit_preserved() {
        let (graph, items) = graph_and_items(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             node work [count < bound][count == bound] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let conv = program_convergence(&graph, &items, true);
        assert!(!conv.has_natural_exit, "explicit exit suppresses synthesis");
        assert!(conv.counter_ge_bounds.is_empty());
    }

    #[test]
    fn test_program_convergence_unprovable_persistent_txn() {
        // A reactive txn without a bounded counter blocks natural exit.
        let (graph, items) = graph_and_items(
            "let count: Int = 0;\n\
             let done: Bool = false;\n\
             node ping [done == false][done == true] {\n\
               done = true;\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let conv = program_convergence(&graph, &items, false);
        assert!(!conv.has_natural_exit);
        assert!(conv.counter_ge_bounds.is_empty());
    }

    #[test]
    fn test_unknown_bound_when_var_is_neither_field_nor_const() {
        let (graph, items) = graph_and_items(
            "let count: Int = 0;\n\
             node work [count < local_bound][count == local_bound] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.values().next().unwrap();
        assert_eq!(shape.bound, Bound::Unknown("local_bound".to_string()));
    }

    /// 2026-08-09 (init kind, Phase 3): a loop bound declared by an `init`
    /// resolves to Bound::Init — provably invariant, so the backend folds the
    /// loop against the seeded value (no Unknown fallback).
    #[test]
    fn test_init_bound_resolves_to_bound_init() {
        let (graph, items) = graph_and_items(
            "init N: Int = 64;\n\
             let count: Int = 0;\n\
             node work [count < N][count == N] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let shapes = build_loop_shapes(&graph, &items, 4);
        let shape = shapes.values().next().unwrap();
        assert_eq!(shape.bound, Bound::Init("N".to_string()));
    }

    #[test]
    fn test_collect_state_fields_accepts_let_and_statedecl() {
        // Phase 1b: the parser emits top-level `let` declarations as
        // TopLevel::Statement(Let); legacy code may use TopLevel::StateDecl.
        // build_swan_songs and the LoopShape bound resolution must agree on
        // the field set, otherwise the swan-song let-to-field remap silently
        // treats a top-level let state field as a local.
        let items = parse_program(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             node work [count < bound][count == bound] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        let fields = collect_state_fields(&items);
        assert!(fields.contains("bound"), "top-level let must be collected");
        assert!(fields.contains("count"), "top-level let must be collected");
        assert!(!fields.contains("work"), "non-let statements excluded");
    }

    #[test]
    fn test_collect_state_fields_includes_legacy_statedecl() {
        // Legacy state declarations (TopLevel::StateDecl) must still be
        // collected — the backend's build_field_index accepts both forms
        // (mod.rs build_field_index), so the shared collector must too.
        // The parser no longer emits `state` — construct the legacy form
        // directly, as the fuzzer/backends do.
        let mut items = parse_program(
            "let bound: Int = 100;\n\
             let count: Int = 0;\n\
             node work [count < bound][count == bound] {\n\
               count = count + 1;\n\
               term;\n\
             };\n",
        );
        items.push(TopLevel::StateDecl(crate::ast::StateDecl {
            name: "legacy".to_string(),
            ty: crate::ast::Type::int(),
            span: None,
        }));
        let fields = collect_state_fields(&items);
        assert!(fields.contains("legacy"), "StateDecl must be collected");
        assert!(fields.contains("count"), "top-level let must be collected");
        assert!(fields.contains("bound"), "top-level let must be collected");
    }
}
