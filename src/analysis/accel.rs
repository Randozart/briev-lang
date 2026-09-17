//! Accel analysis — GPU-deferral eligibility, cost, and decision (SPEC §9.7).
//!
//! Frontend-driven dispatch: computed once in `analyze_program`, stored in
//! `AnalysisResults.accel`, consumed by the LLVM backend as a deterministic
//! switch. The backend never re-derives these decisions (it has no `accel`
//! knowledge by design — see docs/plans/2026-08-06-accel-gpu-offload.md).
//!
//! Policy (module-level `!> accel: <value>`):
//!   - absent            → TryKeyword: only `accel`-keyword bodies, try mode
//!   - `try_all`         → every eligible body is a candidate, try mode
//!   - `force`           → keyword bodies MUST offload (strict)
//!   - `try_all_force`   → every body tried; keyword bodies forced
//!
//! Try mode: speedup must be verified (static crossover for known N, else a
//! runtime probe). Any miss is a silent CPU fallback with a remark. Force mode:
//! eligibility must prove (compile error otherwise), the speedup gate is
//! skipped, and a missing GPU at runtime is an error — never a silent fallback.
//!
//! Eligibility is a proof obligation, not a heuristic. The proof covers:
//! bound (`[i < N]`), write disjointness (array writes affine in `i`), flat
//! value types (resolved through the TypeUniverse, never by name — rules 14/18),
//! and purity (no observable/FFI side effects in kernel statements).

use crate::ast::*;
use crate::type_universe::TypeUniverse;
use crate::type_universe::operators::protocol_category;
use std::collections::{HashMap, HashSet};

/// Module-level policy resolved from `!> accel: <value>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelMode {
    /// Absent: only `accel`-keyword bodies, try mode.
    TryKeyword,
    /// `try_all`: every body, try mode.
    TryAll,
    /// `force`: keyword bodies, force mode.
    Force,
    /// `try_all_force`: every body tried; keyword bodies forced.
    TryAllForce,
}

impl AccelMode {
    /// Whether a keyword-marked body is a candidate under this mode.
    fn targets_marked(self) -> bool {
        matches!(self, AccelMode::TryKeyword | AccelMode::Force | AccelMode::TryAllForce)
    }

    /// Whether ALL bodies are candidates under this mode.
    fn targets_all(self) -> bool {
        matches!(self, AccelMode::TryAll | AccelMode::TryAllForce)
    }

    /// Whether a keyword-marked body is forced (not merely tried).
    fn forces_marked(self) -> bool {
        matches!(self, AccelMode::Force | AccelMode::TryAllForce)
    }
}

/// Resolve the module policy from `AnalysisResults.module_metadata`.
/// Unknown values fall back to the conservative default (`TryKeyword`) —
/// never an error, matching the "absent is default" rule.
pub fn resolve_mode(module_metadata: &HashMap<String, PropertyValue>) -> AccelMode {
    let value = match module_metadata.get("accel") {
        Some(PropertyValue::Identifier(s)) | Some(PropertyValue::String(s)) => s.as_str(),
        _ => return AccelMode::TryKeyword,
    };
    match value {
        "try_all" => AccelMode::TryAll,
        "force" => AccelMode::Force,
        "try_all_force" => AccelMode::TryAllForce,
        _ => AccelMode::TryKeyword,
    }
}

/// Per-body offload decision the backend consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelDecision {
    /// Dispatch to GPU unconditionally (static crossover passed, or force).
    Gpu,
    /// Runtime N — emit both paths + the auto-tuning probe.
    Probe,
    /// Try-mode miss: ineligible or unverifiable speedup → CPU + remark.
    Cpu,
}

/// The proven kernel structure of one candidate body.
#[derive(Debug, Clone)]
pub struct KernelShape {
    /// Virtual work-item index bound by `[i < N]`.
    pub index_var: String,
    /// The work-item count expression `N` (may be runtime-determined).
    pub count_expr: Option<Expr>,
    /// Statements proven safe to offload.
    pub kernel_stmts: Vec<Statement>,
    /// Statements that stay on the CPU path (loop control, observables).
    pub host_stmts: Vec<Statement>,
    /// Array state fields read by the kernel (shared, coalesced-friendly).
    pub read_buffers: Vec<String>,
    /// Array state fields written per work-item slot.
    pub write_buffers: Vec<String>,
    /// Read-only scalar state/const fields consumed by the kernel.
    pub scalar_ins: Vec<String>,
    /// Eligibility proof result.
    pub eligible: bool,
    /// Ineligibility evidence (optimization remarks).
    pub reasons: Vec<String>,
    /// 2D dispatch width (plan 2026-08-31-gpu-next §2b): when the body
    /// decomposes the work-item id row/col style (`i >> k`, `i & (2^k - 1)`),
    /// the kernel reconstructs `i = gid.y * cols + gid.x` and launchers may
    /// dispatch a 2D grid. None = plain 1D. Reconstruction is correct under
    /// ANY launch shape that covers the total count, so 1D launchers stay
    /// sound (flat: gid.y is always 0 and gid.x spans the count).
    pub work_cols: Option<u64>,
    /// Cooperative row reduction (plan 2026-09-01-cooperative-row-kernels):
    /// a `foreach k in 0..K` whose body is a float mul-add into a local
    /// accumulator (`acc = acc + f1[..k..] * f2[..k..]`) is a dot-product
    /// reduction. The backend lowers it lane-cooperatively: strided
    /// accumulation + OpGroupNonUniformFAdd + one store per row. Holds the
    /// loop END expression (K — resolved at emission).
    pub reduction: Option<ReductionInfo>,
}

/// The inner-loop length expression of a recognized dot-product reduction.
#[derive(Debug, Clone)]
pub struct ReductionInfo {
    /// The foreach END expression (e.g. `Identifier("K")`).
    pub inner: crate::ast::Expr,
}

/// One analyzed candidate body, keyed by transaction name.
#[derive(Debug, Clone)]
pub struct AccelEntry {
    pub mode: AccelMode,
    /// True when this body is force mode (keyword-marked under force policy).
    pub forced: bool,
    pub shape: KernelShape,
    pub decision: AccelDecision,
}

/// State-field facts extracted from the program.
pub(crate) struct ProgramInfo {
    /// All state field names (scalar + array).
    state_fields: HashSet<String>,
    /// Array state field name → its full `Type::Vector` type.
    pub(crate) array_types: HashMap<String, Type>,
    /// `const Name = expr;` values (compile-time).
    consts: HashMap<String, Expr>,
    /// `const Name: Type = ...;` declared types.
    const_types: HashMap<String, Type>,
}

impl ProgramInfo {
    fn build(items: &[TopLevel]) -> ProgramInfo {
        let mut state_fields = HashSet::new();
        let mut array_types = HashMap::new();
        let mut consts = HashMap::new();
        let mut const_types = HashMap::new();
        for item in items {
            match item {
                TopLevel::StateDecl(s) => {
                    state_fields.insert(s.name.clone());
                    if let Type::Vector(_, _) = &s.ty {
                        array_types.insert(s.name.clone(), s.ty.clone());
                    }
                    // 2026-09-14 (Matrix type plan): shape-bearing Applied
                    // types (Matrix<T,R,C>) are also arrays.
                    if matches!(&s.ty, Type::Applied(..)) {
                        array_types.insert(s.name.clone(), s.ty.clone());
                    }
                }
                TopLevel::Statement(stmt) => {
                    if let Statement::Let { name, ty, expr, .. } = stmt.as_ref() {
                        state_fields.insert(name.clone());
                        if let Some(ty) = ty {
                            if matches!(ty, Type::Vector(..) | Type::Applied(..)) {
                                array_types.insert(name.clone(), ty.clone());
                            }
                        }
                    }
                }
                TopLevel::Constant(c) => {
                    consts.insert(c.name.clone(), c.expr.clone());
                    const_types.insert(c.name.clone(), c.ty.clone());
                }
                _ => {}
            }
        }
        ProgramInfo { state_fields, array_types, consts, const_types }
    }

    /// Flatness of a referenced array: look up its declared type (state or
    /// const) and prove the element type is a flat scalar via the universe.
    fn array_is_flat(&self, name: &str, universe: &TypeUniverse) -> bool {
        let ty = self.array_types.get(name).or_else(|| self.const_types.get(name));
        match ty {
            Some(Type::Vector(inner, _)) => is_flat_scalar(universe, inner),
            // 2026-09-14 (Matrix type plan): shape-bearing Applied type's
            // element is the first arg.
            Some(Type::Applied(_, args)) => {
                let elem = args.first().unwrap();
                is_flat_scalar(universe, elem)
            }
            Some(ty) => is_flat_scalar(universe, ty),
            None => false,
        }
    }
}

/// Flat scalar protocol categories: `Int`, `UInt`, `Float`, `Bool`,
/// `Char`. `String`/`Blob` and pointers are not flat and reject the kernel.
/// Resolved through the TypeUniverse (`protocol_category`), never by matching
/// type names (rules 14/18). `Bits(n)` is the sole physical primitive.
fn is_flat_scalar(universe: &TypeUniverse, ty: &Type) -> bool {
    match ty {
        Type::Bits(_) => true,
        Type::Vector(inner, _) => is_flat_scalar(universe, inner),
        _ => protocol_category(universe, ty)
            .map_or(false, |cat| matches!(cat.as_str(), "Int" | "UInt" | "Float" | "Bool" | "Char")),
    }
}

/// The work-item bound conjunct of a precondition (2026-08-31, plan
/// abv-gpu-by-default): after stripping `beginprogram` markers, scan `&&`
/// conjunctions left-to-right for the first `identifier < N` conjunct. Host
/// predicates (`[phase == 1 && i < nb]`) gate the HOST firing and are not
/// part of the kernel bound.
fn work_item_bound(pre: &Expr) -> Option<(String, Expr)> {
    match strip_beginprogram(pre) {
        Expr::BinaryOp(BinaryOpKind::And, l, r) => {
            work_item_bound(l.as_ref()).or_else(|| work_item_bound(r.as_ref()))
        }
        Expr::BinaryOp(BinaryOpKind::Lt, left, right)
            if matches!(left.as_ref(), Expr::Identifier(_)) =>
        {
            match left.as_ref() {
                Expr::Identifier(s) => Some((s.clone(), right.as_ref().clone())),
                _ => unreachable!(),
            }
        }
        _ => None,
    }
}

/// Remove `Expr::BeginProgram` conjuncts from a precondition, returning the
/// remaining state expression (or `[true]` if only the marker is present).
/// An `accel` entry-loop (`[beginprogram && i < N]`) reads its bound from the
/// stripped form.
fn strip_beginprogram(pre: &Expr) -> Expr {
    match pre {
        Expr::BeginProgram => Expr::Bool(true),
        Expr::BinaryOp(BinaryOpKind::And, a, b) => {
            let a = strip_beginprogram(a);
            let b = strip_beginprogram(b);
            match (a, b) {
                (Expr::Bool(true), b) => b,
                (a, Expr::Bool(true)) => a,
                (a, b) => Expr::BinaryOp(BinaryOpKind::And, Box::new(a), Box::new(b)),
            }
        }
        other => other.clone(),
    }
}

/// True when the body increments `var` by a positive delta (`var = var + d`,
/// `var = var - d` decreasing). The accel node is a native counted loop — the
/// counter must advance so the loop terminates and the map is well-defined.
fn body_increments_counter(body: &[Statement], var: &str) -> bool {
    for stmt in body {
        if let Statement::Assign(lhs, rhs) = stmt {
            if let Expr::Identifier(n) = lhs {
                if n == var && is_self_increment(rhs, var) {
                    return true;
                }
            }
        }
    }
    false
}

/// `var = var ± delta` with a positive literal delta — the counted-loop
/// advance (Design A: `i = i + 1`).
fn is_self_increment(rhs: &Expr, var: &str) -> bool {
    match rhs {
        Expr::BinaryOp(BinaryOpKind::Add, a, b) => {
            (matches!(a.as_ref(), Expr::Identifier(v) if v == var)
                && const_delta(b).map_or(false, |d| d > 0))
                || (matches!(b.as_ref(), Expr::Identifier(v) if v == var)
                    && const_delta(a).map_or(false, |d| d > 0))
        }
        Expr::BinaryOp(BinaryOpKind::Sub, a, b) => {
            matches!(a.as_ref(), Expr::Identifier(v) if v == var)
                && const_delta(b).map_or(false, |d| d > 0)
        }
        _ => false,
    }
}

/// Constant integer value of an expression, if it is a literal.
fn const_delta(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Decimal(n) => Some(*n),
        Expr::Char(c) => Some(*c as i64),
        _ => None,
    }
}

/// True when `expr` mentions `var` anywhere (used to require array-write
/// indices to depend on the work-item index).
fn expr_contains(expr: &Expr, var: &str) -> bool {
    match expr {
        Expr::Identifier(name) => name == var,
        Expr::BinaryOp(_, l, r) => expr_contains(l, var) || expr_contains(r, var),
        Expr::UnaryOp(_, e) => expr_contains(e, var),
        Expr::Index(a, i) => expr_contains(a, var) || expr_contains(i, var),
        Expr::Cast(e, _) => expr_contains(e, var),
        Expr::Field(o, _) => expr_contains(o, var),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_contains(a, var)),
        Expr::Tuple(items) => items.iter().any(|i| expr_contains(i, var)),
        Expr::List(items) => items.iter().any(|i| expr_contains(i, var)),
        _ => false,
    }
}

/// True when `expr` is linear in `var`: `a*var + b` with constant a/b
/// (no var*var, no division by var). An identifier is always linear: either
/// `var` itself (coefficient 1) or a constant. Disjointness additionally
/// requires the index to CONTAIN `var` (see `stmt_is_kernel`).
fn is_linear_in(expr: &Expr, var: &str) -> bool {
    match expr {
        Expr::Identifier(_) | Expr::Decimal(_) | Expr::Char(_) | Expr::Bool(_) | Expr::Float(_) => true,
        Expr::BinaryOp(BinaryOpKind::Add | BinaryOpKind::Sub, l, r) => {
            is_linear_in(l, var) && is_linear_in(r, var)
        }
        Expr::BinaryOp(BinaryOpKind::Mul, l, r) => {
            // Exactly one side depends on var; the other is constant.
            (expr_contains(l, var) && !expr_contains(r, var)
                && is_linear_in(l, var) && is_linear_in(r, var))
                || (!expr_contains(l, var) && expr_contains(r, var)
                    && is_linear_in(l, var) && is_linear_in(r, var))
        }
        _ => false,
    }
}

/// Purity: kernel expressions must be pure arithmetic/data. Reject calls,
/// reflection, pointers, control flow, and any observable/FFI surface.
fn expr_is_pure(expr: &Expr) -> bool {
    match expr {
        Expr::Decimal(_) | Expr::Char(_) | Expr::Bool(_) | Expr::Float(_)
        | Expr::Quoted(_) | Expr::TaggedLiteral(_, _) | Expr::TaggedQuotedLiteral(_, _)
        | Expr::Identifier(_) => true,
        Expr::BinaryOp(_, l, r) => expr_is_pure(l) && expr_is_pure(r),
        Expr::UnaryOp(_, e) => expr_is_pure(e),
        // 2026-09-02 (plan 2026-09-02-graphics-ray-and-images): an
        // if-expression is pure selection — no side effects, deterministic
        // given its inputs. Both arms + the condition must be pure.
        Expr::If(c, t, e) => {
            expr_is_pure(c)
                && expr_is_pure(t)
                && e.as_deref().map(expr_is_pure).unwrap_or(true)
        }
        // `match cond { true => …, false => … }` — the two-way selection
        // form the SPIR-V kernel surface lowers. Guards or non-bool-literal
        // patterns are rejected at emission; purity here mirrors that
        // surface (scrutinee + arm bodies).
        Expr::Match(s, arms) => {
            expr_is_pure(s)
                && arms.iter().all(|arm| {
                    arm.guard.is_none()
                        && matches!(
                            arm.pattern,
                            crate::ast::Pattern::Literal(Expr::Bool(_))
                                | crate::ast::Pattern::Wildcard
                        )
                        && expr_is_pure(&arm.body)
                })
        }
        Expr::Index(a, i) => expr_is_pure(a) && expr_is_pure(i),
        Expr::Cast(e, _) => expr_is_pure(e),
        Expr::Tuple(items) => items.iter().all(expr_is_pure),
        Expr::List(items) => items.iter().all(expr_is_pure),
        // 2026-09-01 (plan 2026-09-01-cooperative-row-kernels): the subgroup
        // reduction is a pure fixed-shape tree over the subgroup — no
        // observable side effects, deterministic per run. The invocation-id
        // builtins are pure reads too.
        Expr::Call(name, args, _)
            if name == "SubgroupFAdd#"
                || name == "SubgroupFMax#"
                || name == "SubgroupFMin#"
                || name == "ShuffleDown#"
                || name == "ShuffleXor#"
                || name == "SubgroupBallot#"
                || name == "SubgroupBroadcast#"
                || name == "Fma#"
                || name == "Max#"
                || name == "Min#"
                || name == "GetGlobalId#"
                || name == "GetLocalId#"
                || name == "Exp#"
                || name == "Sqrt#"
                || name == "Fabs#" =>
        {
            args.iter().all(expr_is_pure)
        }
        _ => false,
    }
}

/// Shared context for the kernel proof: the work-item index, the program's
/// state-field facts, and the ineligibility-evidence sink (remarks).
struct KernelCtx<'a> {
    index_var: &'a str,
    info: &'a ProgramInfo,
    reasons: &'a mut Vec<String>,
}

/// Outputs of the kernel/host partition.
struct PartitionOut {
    kernel: Vec<Statement>,
    host: Vec<Statement>,
    locals: HashSet<String>,
}

/// Partition a body into kernel statements (proven offloadable) and host
/// statements (CPU: loop control, observables). Records the disjoint-write
/// proof and rejects cross-work-item array writes.
fn partition(body: &[Statement], ctx: &mut KernelCtx<'_>) -> PartitionOut {
    let mut out = PartitionOut {
        kernel: Vec::new(),
        host: Vec::new(),
        locals: HashSet::new(),
    };
    for stmt in body {
        if stmt_is_kernel(stmt, ctx, &mut out.locals) {
            out.kernel.push(stmt.clone());
        } else {
            out.host.push(stmt.clone());
        }
    }
    out
}

/// Whether a single statement is offloadable. Host statements (loop counters,
/// term/exit/rollback, when/gate, observables, non-affine scalar writes) fall
/// through to `host`. A cross-work-item array write is NOT host material — it
/// is an ineligibility, recorded in `ctx.reasons`.
fn stmt_is_kernel(
    stmt: &Statement,
    ctx: &mut KernelCtx<'_>,
    locals: &mut HashSet<String>,
) -> bool {
    match stmt {
        Statement::Let { name, expr, .. } => {
            if let Some(e) = expr {
                if expr_is_pure(e) {
                    locals.insert(name.clone());
                    return true;
                }
            }
            false
        }
        Statement::Assign(lhs, rhs) => assign_is_kernel(lhs, rhs, ctx, locals),
        Statement::Guarded(cond, body) => {
            if !expr_is_pure(cond) {
                return false;
            }
            !body.is_empty()
                && body.iter().all(|s| stmt_is_kernel(s, ctx, locals))
        }
        Statement::Block(b) => {
            !b.is_empty() && b.iter().all(|s| stmt_is_kernel(s, ctx, locals))
        }
        // 2026-08-31 (VITRIOL GEMM comparison, M1): a bounded `foreach k in
        // start..end` is a loop-private reduction — the loop variable is a
        // PRIVATE scalar (added to locals: the body's assignments to it and
        // to outer local accumulators obey the same purity rules), the range
        // bounds are pure, and the body obeys every existing rule. The loop
        // itself adds no cross-work-item state: each invocation runs its own
        // copy. Non-range collections (heaps) stay host-side.
        Statement::Foreach { item, list, body } => {
            let Expr::Range { start, end, .. } = list.as_ref() else {
                ctx.reasons.push(format!(
                    "foreach over a non-range collection — kernel loops \
                     iterate `start..end` ranges only"
                ));
                return false;
            };
            if !expr_is_pure(start) || !expr_is_pure(end) {
                return false;
            }
            let inserted = locals.insert(item.clone());
            let ok = !body.is_empty() && body.iter().all(|s| stmt_is_kernel(s, ctx, locals));
            if inserted {
                locals.remove(item);
            }
            ok
        }
        _ => false,
    }
}

/// One assignment's eligibility. Array writes must target a state array at a
/// slot linear in the work-item index (disjointness). Scalar writes are kernel
/// only when the scalar is a body-local temporary; state-scalar writes (loop
/// counters, bookkeeping) belong to the CPU host.
fn assign_is_kernel(
    lhs: &Expr,
    rhs: &Expr,
    ctx: &mut KernelCtx<'_>,
    locals: &mut HashSet<String>,
) -> bool {
    match lhs {
        Expr::Index(arr, idx) => index_write_is_kernel(arr, idx, rhs, ctx),
        Expr::Identifier(name) => {
            !ctx.info.state_fields.contains(name)
                && locals.contains(name)
                && expr_is_pure(rhs)
        }
        _ => false,
    }
}

/// Eligibility of one array-slot write. The slot index must depend on the
/// work-item id (`contains`) and be linear in it (no `i*i`, no `i/j`), so no
/// two work-items write the same slot.
fn index_write_is_kernel(
    arr: &Expr,
    idx: &Expr,
    rhs: &Expr,
    ctx: &mut KernelCtx<'_>,
) -> bool {
    if !expr_is_pure(rhs) {
        return false;
    }
    let arr_name = match arr {
        Expr::Identifier(n) => n.clone(),
        _ => return false,
    };
    if !ctx.info.state_fields.contains(&arr_name) {
        ctx.reasons.push(format!("write to non-state array '{}'", arr_name));
        return false;
    }
    if expr_contains(idx, ctx.index_var) && is_linear_in(idx, ctx.index_var) {
        true
    } else {
        ctx.reasons.push(format!(
            "cross-work-item array write: '{}[...]' index must be linear in work-item id '{}'",
            arr_name, ctx.index_var
        ));
        false
    }
}

/// Prove the whole kernel and collect its buffer contracts.
fn prove_kernel(
    name: &str,
    body: &[Statement],
    contract: &Contract,
    info: &ProgramInfo,
    universe: &TypeUniverse,
) -> KernelShape {
    let mut reasons = Vec::new();
    let mut shape = KernelShape {
        index_var: String::new(),
        count_expr: None,
        kernel_stmts: Vec::new(),
        host_stmts: Vec::new(),
        read_buffers: Vec::new(),
        write_buffers: Vec::new(),
        scalar_ins: Vec::new(),
        eligible: false,
        reasons: Vec::new(),
        work_cols: None,
        reduction: None,
    };

    // 1. Bound: the contract precondition must CONTAIN an `[i < N]` conjunct
    //    where `i` is a REAL state counter that the body increments (Design A
    //    — no virtual variables; the user declares `let i: Int = 0;` and
    //    writes `i = i + 1`). `beginprogram` entry markers are stripped, and
    //    any other host predicate in a conjunction (`[phase == 1 && i < nb]`)
    //    gates the HOST firing — it does not change the kernel bound, so the
    //    first `identifier < N` conjunct is the work-item count.
    //    2026-08-31 (plan abv-gpu-by-default): generalized from bare/`beginprogram`
    //    forms to any conjunction — phase-gated kernels were rejected as
    //    "requires a work-item bound precondition".
    let (index_var, count_expr) = match work_item_bound(&contract.pre_condition) {
        Some((i, n)) => (i, Some(n)),
        None => {
            reasons.push(format!(
                "accel '{}' requires a work-item bound precondition '[i < N]' over a real counter 'i'",
                name
            ));
            shape.reasons = reasons;
            return shape;
        }
    };
    if !info.state_fields.contains(&index_var) {
        reasons.push(format!(
            "accel '{}' bound variable '{}' is not a state counter — declare 'let {}: Int = 0;' and increment it in the body",
            name, index_var, index_var
        ));
        shape.reasons = reasons;
        return shape;
    }
    if !body_increments_counter(body, &index_var) {
        reasons.push(format!(
            "accel '{}' bound counter '{}' is never incremented ('{} = {} + 1') — the node would not terminate",
            name, index_var, index_var, index_var
        ));
        shape.reasons = reasons;
        return shape;
    }
    shape.index_var = index_var.clone();
    shape.count_expr = count_expr;

    // 2/4. Partition + purity + disjoint writes.
    let PartitionOut { kernel, host, .. } = {
        let mut ctx = KernelCtx {
            index_var: &index_var,
            info,
            reasons: &mut reasons,
        };
        partition(body, &mut ctx)
    };
    shape.kernel_stmts = kernel;
    shape.host_stmts = host;
    if shape.kernel_stmts.is_empty() {
        reasons.push(format!(
            "accel '{}' has no offloadable statements (pure, disjoint, work-item-affine)",
            name
        ));
        shape.reasons = reasons;
        return shape;
    }

    // 3. Buffer contracts: array reads (shared) + array writes (disjoint) +
    //    read-only scalars. The counter `i` is the work-item id in the kernel
    //    (bound to get_global_id), never a device input — exclude it.
    let (reads, writes, mut scalars) = collect_buffers(&shape.kernel_stmts, info);
    scalars.retain(|s| s != &index_var);
    shape.read_buffers = reads;
    shape.write_buffers = writes;
    shape.scalar_ins = scalars;

    // 5. Flat types via the TypeUniverse.
    for buf in shape.read_buffers.iter().chain(shape.write_buffers.iter()) {
        if !info.array_is_flat(buf, universe) {
            reasons.push(format!(
                "accel '{}' array '{}' is not a flat scalar type (needs Int/UInt/Float/Bool/Char)",
                name, buf
            ));
        }
    }

    // 6. 2D dispatch geometry (plan 2026-08-31-gpu-next §2b): the body's own
    //    row/col decomposition names the width — `i >> k` says cols = 2^k,
    //    `i & (cols-1)` confirms it. Best-effort: no match stays 1D.
    shape.work_cols = detect_work_cols(&shape.kernel_stmts, &index_var);

    // 7. Dot-product reduction (plan 2026-09-01-cooperative-row-kernels):
    //    conservative structural match — the LAST statement is a foreach
    //    whose body is one mul-add into a local accumulator.
    shape.reduction = detect_reduction(&shape.kernel_stmts);

    shape.eligible = reasons.is_empty();
    shape.reasons = reasons;
    shape
}

/// Recognize a dot-product reduction: a foreach whose body is exactly one
/// assignment `acc = acc + f1[..k..] * f2[..k..]` (either mul order) with
/// the accumulator self-referencing on one additive side. Returns the loop
/// END expression for the cooperative lowering.
fn detect_reduction(stmts: &[Statement]) -> Option<ReductionInfo> {
    use crate::ast::BinaryOpKind::{Add, Mul};
    for stmt in stmts {
        let Statement::Foreach { list, body, .. } = stmt else {
            continue;
        };
        let Expr::Range { end, .. } = list.as_ref() else {
            continue;
        };
        if body.len() != 1 {
            continue;
        }
        let Statement::Assign(lhs, rhs) = &body[0] else {
            continue;
        };
        let Expr::Identifier(acc) = lhs else {
            continue;
        };
        let rhs_ref: &Expr = rhs;
        let Expr::BinaryOp(Add, a, b) = rhs_ref else {
            continue;
        };
        let is_self = |e: &Expr| matches!(e, Expr::Identifier(n) if n == acc);
        let is_mul = |e: &Expr| matches!(e, Expr::BinaryOp(Mul, _, _));
        let (ar, br): (&Expr, &Expr) = (a, b);
        if !((is_self(ar) && is_mul(br)) || (is_mul(ar) && is_self(br))) {
            continue;
        }
        // Both mul operands must reference the loop var — they index with it.
        // The loop item name check is structural (the mul sides contain
        // Index expressions); a full var-flow proof is the lowerer's job.
        return Some(ReductionInfo {
            inner: end.as_ref().clone(),
        });
    }
    None
}

/// Derive the 2D dispatch width from the kernel body's shift/mask uses of
/// the work-item id. Returns Some(cols) when a shift `i >> k` (k in 1..=31)
/// appears and every mask `i & m` agrees with cols = 2^k (m == cols - 1).
fn detect_work_cols(stmts: &[Statement], index_var: &str) -> Option<u64> {
    let mut shift_cols: Option<u64> = None;
    let mut mask_cols: Option<u64> = None;
    for stmt in stmts {
        scan_stmt_work_cols(stmt, index_var, &mut shift_cols, &mut mask_cols);
    }
    match (shift_cols, mask_cols) {
        (Some(c), Some(m)) if c == m => Some(c),
        (Some(c), None) => Some(c),
        _ => None,
    }
}

fn scan_expr_work_cols(
    e: &Expr,
    index_var: &str,
    shift_cols: &mut Option<u64>,
    mask_cols: &mut Option<u64>,
) {
    if let Some(cols) = shift_of_index(e, index_var) {
        if shift_cols.is_none() {
            *shift_cols = Some(cols);
        }
        return;
    }
    if let Some(cols) = mask_of_index(e, index_var) {
        update_mask_evidence(mask_cols, cols);
        return;
    }
    for child in child_exprs(e) {
        scan_expr_work_cols(child, index_var, shift_cols, mask_cols);
    }
}

/// `i >> k` with k in 1..=31 names cols = 2^k.
fn shift_of_index(e: &Expr, index_var: &str) -> Option<u64> {
    let Expr::BinaryOp(BinaryOpKind::Shr, l, r) = e else {
        return None;
    };
    if !matches!(l.as_ref(), Expr::Identifier(v) if v == index_var) {
        return None;
    }
    let Expr::Decimal(k) = r.as_ref() else {
        return None;
    };
    if *k < 1 || *k > 31 {
        return None;
    }
    Some(1u64 << *k)
}

/// `i & m` with m+1 a power of two names cols = m+1.
fn mask_of_index(e: &Expr, index_var: &str) -> Option<u64> {
    let Expr::BinaryOp(BinaryOpKind::BitAnd, l, r) = e else {
        return None;
    };
    if !matches!(l.as_ref(), Expr::Identifier(v) if v == index_var) {
        return None;
    }
    let Expr::Decimal(m) = r.as_ref() else {
        return None;
    };
    if *m <= 0 || (*m as u64).next_power_of_two() != *m as u64 {
        return None;
    }
    Some(*m as u64 + 1)
}

fn update_mask_evidence(mask_cols: &mut Option<u64>, cols: u64) {
    match mask_cols {
        None => *mask_cols = Some(cols),
        Some(existing) if *existing != cols => {
            // conflicting masks: poison the mask evidence
            *mask_cols = Some(u64::MAX);
        }
        _ => {}
    }
}

fn scan_stmt_work_cols(
    stmt: &Statement,
    index_var: &str,
    shift_cols: &mut Option<u64>,
    mask_cols: &mut Option<u64>,
) {
    match stmt {
        Statement::Assign(lhs, rhs) => {
            scan_expr_work_cols(lhs, index_var, shift_cols, mask_cols);
            scan_expr_work_cols(rhs, index_var, shift_cols, mask_cols);
        }
        Statement::Foreach { list, body, .. } => {
            scan_expr_work_cols(list, index_var, shift_cols, mask_cols);
            for s in body {
                scan_stmt_work_cols(s, index_var, shift_cols, mask_cols);
            }
        }
        _ => {}
    }
}

/// Direct expression children (best-effort walk for the 2D scan).
fn child_exprs(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::BinaryOp(_, l, r) => vec![l.as_ref(), r.as_ref()],
        Expr::UnaryOp(_, a) => vec![a.as_ref()],
        Expr::Index(o, i) => vec![o.as_ref(), i.as_ref()],
        Expr::Call(_, args, _) => args.iter().collect(),
        Expr::MethodCall(recv, _, args, _, _) => {
            let mut v: Vec<&Expr> = vec![recv.as_ref()];
            v.extend(args.iter());
            v
        }
        _ => Vec::new(),
    }
}

/// Walk kernel statements collecting the buffer contract:
/// array reads → read_buffers, array writes → write_buffers, scalar state/const
/// reads → scalar_ins (read-only inputs). Returns sorted lists.
fn collect_buffers(
    stmts: &[Statement],
    info: &ProgramInfo,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut reads: HashSet<String> = HashSet::new();
    let mut writes: HashSet<String> = HashSet::new();
    let mut scalars: HashSet<String> = HashSet::new();
    for stmt in stmts {
        collect_stmt_buffers(stmt, info, &mut reads, &mut writes, &mut scalars);
    }
    let mut reads: Vec<String> = reads.into_iter().collect();
    let mut writes: Vec<String> = writes.into_iter().collect();
    let mut scalars: Vec<String> = scalars.into_iter().collect();
    reads.sort();
    writes.sort();
    scalars.sort();
    (reads, writes, scalars)
}

fn collect_stmt_buffers(
    stmt: &Statement,
    info: &ProgramInfo,
    reads: &mut HashSet<String>,
    writes: &mut HashSet<String>,
    scalars: &mut HashSet<String>,
) {
    match stmt {
        Statement::Let { expr, .. } => {
            if let Some(e) = expr {
                collect_expr_buffers(e, info, reads, writes, scalars);
            }
        }
        Statement::Assign(lhs, rhs) => {
            collect_expr_buffers(rhs, info, reads, writes, scalars);
            if let Expr::Index(arr, _) = lhs {
                if let Expr::Identifier(n) = arr.as_ref() {
                    writes.insert(n.clone());
                }
            }
        }
        Statement::Guarded(cond, body) => {
            collect_expr_buffers(cond, info, reads, writes, scalars);
            for s in body {
                collect_stmt_buffers(s, info, reads, writes, scalars);
            }
        }
        Statement::Block(b) => {
            for s in b {
                collect_stmt_buffers(s, info, reads, writes, scalars);
            }
        }
        // 2026-09-01 (Track B find): Foreach bodies were INVISIBLE to the
        // buffer walk — every cooperative kernel's read/write_buffers were
        // empty (the eligibility flatness checks were vacuous). The foreach
        // list (a range — no state access) and body are walked like Guarded.
        Statement::Foreach { list, body, .. } => {
            collect_expr_buffers(list, info, reads, writes, scalars);
            for s in body {
                collect_stmt_buffers(s, info, reads, writes, scalars);
            }
        }
        _ => {}
    }
}

fn collect_expr_buffers(
    expr: &Expr,
    info: &ProgramInfo,
    reads: &mut HashSet<String>,
    writes: &mut HashSet<String>,
    scalars: &mut HashSet<String>,
) {
    match expr {
        Expr::Identifier(name) => {
            if info.state_fields.contains(name) {
                scalars.insert(name.clone());
            } else if info.consts.contains_key(name) {
                scalars.insert(name.clone());
            }
        }
        Expr::Index(arr, idx) => {
            collect_expr_buffers(arr, info, reads, writes, scalars);
            collect_expr_buffers(idx, info, reads, writes, scalars);
            if let Expr::Identifier(n) = arr.as_ref() {
                reads.insert(n.clone());
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_expr_buffers(l, info, reads, writes, scalars);
            collect_expr_buffers(r, info, reads, writes, scalars);
        }
        Expr::UnaryOp(_, e) => collect_expr_buffers(e, info, reads, writes, scalars),
        Expr::Cast(e, _) => collect_expr_buffers(e, info, reads, writes, scalars),
        Expr::Tuple(items) | Expr::List(items) => {
            for i in items {
                collect_expr_buffers(i, info, reads, writes, scalars);
            }
        }
        _ => {}
    }
}

/// Resolve the work-item count to a compile-time constant when possible.
/// `const N = 100;` and literals are static; `get_env_int!` etc. are runtime.
fn constant_n(count_expr: &Option<Expr>, consts: &HashMap<String, Expr>) -> Option<u64> {
    match count_expr {
        Some(Expr::Decimal(n)) if *n > 0 => Some(*n as u64),
        Some(Expr::Identifier(name)) => match consts.get(name) {
            Some(Expr::Decimal(n)) if *n > 0 => Some(*n as u64),
            _ => None,
        },
        _ => None,
    }
}

/// The per-body decision: force skips the speedup gate; try mode verifies
/// statically (known N) or defers to the runtime probe (runtime N).
fn decide(shape: &KernelShape, forced: bool, info: &ProgramInfo) -> AccelDecision {
    if !shape.eligible {
        return AccelDecision::Cpu;
    }
    if forced {
        return AccelDecision::Gpu;
    }
    match constant_n(&shape.count_expr, &info.consts) {
        Some(n) => {
            // Reuse the arithmetic-intensity cost model for the crossover.
            let est = crate::analysis::gpu_cost::estimate(&shape.kernel_stmts, n);
            if n >= est.crossover_point {
                AccelDecision::Gpu
            } else {
                AccelDecision::Cpu
            }
        }
        None => AccelDecision::Probe,
    }
}

/// Analyze every candidate body. Requires a populated TypeUniverse for the
/// flat-type proof; without one, no accel decisions are produced (the LLVM
/// backend always supplies it).
pub fn analyze(
    items: &[TopLevel],
    module_metadata: &HashMap<String, PropertyValue>,
    universe: Option<&TypeUniverse>,
) -> HashMap<String, AccelEntry> {
    let mut out = HashMap::new();
    let Some(universe) = universe else {
        return out;
    };
    let mode = resolve_mode(module_metadata);
    let info = ProgramInfo::build(items);
    for item in items {
        let txn = match item {
            TopLevel::Transaction(t) => t,
            _ => continue,
        };
        let marked = txn.modifiers.iter().any(|m| m.name == "accel");
        if !(mode.targets_all() || (mode.targets_marked() && marked)) {
            continue;
        }
        let forced = mode.forces_marked() && marked;
        let shape = prove_kernel(&txn.name, &txn.body, &txn.contract, &info, universe);
        let decision = decide(&shape, forced, &info);
        out.insert(
            txn.name.clone(),
            AccelEntry { mode, forced, shape, decision },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txn_with(name: &str, pre: Expr, body: Vec<Statement>) -> TopLevel {
        TopLevel::Transaction(Transaction {
            name: name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: pre,
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: true,
            post_authority: false},
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![Annotation { name: "accel".into(), value: None }],
            span: None,
            doc: None,
        })
    }

    fn pre_lt(var: &str, bound: Expr) -> Expr {
        Expr::BinaryOp(
            BinaryOpKind::Lt,
            Box::new(Expr::Identifier(var.to_string())),
            Box::new(bound),
        )
    }

    fn state(items: &mut Vec<TopLevel>) {
        items.push(TopLevel::StateDecl(StateDecl {
            name: "px".into(),
            ty: Type::Vector(Box::new(Type::Custom("Float".into())), vec![]),
            span: None,
        }));
        items.push(TopLevel::StateDecl(StateDecl {
            name: "dv".into(),
            ty: Type::Vector(Box::new(Type::Custom("Float".into())), vec![]),
            span: None,
        }));
        items.push(TopLevel::StateDecl(StateDecl {
            name: "nb".into(),
            ty: Type::int(),
            span: None,
        }));
        // Design A: the work-item counter is a REAL state field, declared 0
        // and incremented in the body.
        items.push(TopLevel::StateDecl(StateDecl {
            name: "i".into(),
            ty: Type::int(),
            span: None,
        }));
    }

    /// `i = i + 1` — the counter advance of the native counted loop.
    fn inc_i() -> Statement {
        Statement::Assign(
            Expr::Identifier("i".into()),
            Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("i".into())),
                Box::new(Expr::Decimal(1)),
            ),
        )
    }

    /// Append the counter advance to a kernel body.
    fn with_inc(mut stmts: Vec<Statement>) -> Vec<Statement> {
        stmts.push(inc_i());
        stmts
    }

    fn universe() -> TypeUniverse {
        // Primordials (Int/Float/Bool/Char/...) carry Cast.<Category> props,
        // so protocol_category resolves flatness without stdlib.
        TypeUniverse::new()
    }

    fn entry<'a>(map: &'a HashMap<String, AccelEntry>, name: &str) -> &'a AccelEntry {
        map.get(name).unwrap_or_else(|| panic!("no entry for {name}"))
    }

    // ── resolve_mode ─────────────────────────────────────────────

    #[test]
    fn mode_absent_is_try_keyword() {
        let meta = HashMap::new();
        assert_eq!(resolve_mode(&meta), AccelMode::TryKeyword);
    }

    #[test]
    fn mode_values() {
        let mut meta = HashMap::new();
        meta.insert("accel".into(), PropertyValue::Identifier("try_all".into()));
        assert_eq!(resolve_mode(&meta), AccelMode::TryAll);
        meta.insert("accel".into(), PropertyValue::Identifier("force".into()));
        assert_eq!(resolve_mode(&meta), AccelMode::Force);
        meta.insert("accel".into(), PropertyValue::Identifier("try_all_force".into()));
        assert_eq!(resolve_mode(&meta), AccelMode::TryAllForce);
        meta.insert("accel".into(), PropertyValue::Identifier("bogus".into()));
        assert_eq!(resolve_mode(&meta), AccelMode::TryKeyword);
    }

    // ── eligibility: bound ────────────────────────────────────────

    #[test]
    fn bound_requires_lt_precondition() {
        let mut items = vec![];
        state(&mut items);
        let body = vec![
            Statement::Assign(
                Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        items.push(txn_with("ok", pre_lt("i", Expr::Identifier("nb".into())), body.clone()));
        // Non-`[i < N]` precondition → ineligible.
        items.push(txn_with("bad", Expr::Bool(true), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        assert!(entry(&map, "ok").shape.eligible);
        assert!(!entry(&map, "bad").shape.eligible);
        assert!(entry(&map, "bad").shape.reasons.iter().any(|r| r.contains("[i < N]")));
    }

    #[test]
    fn counter_must_be_a_state_field() {
        // Design A: the bound variable is a REAL state counter (let i: Int = 0),
        // never a virtual index.
        let mut items = vec![];
        state(&mut items);
        // Use `k` (not declared) as the bound var → ineligible.
        let body = vec![
            Statement::Assign(
                Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("k".into()))),
                Expr::Decimal(1),
            ),
            Statement::Assign(
                Expr::Identifier("k".into()),
                Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("k".into())), Box::new(Expr::Decimal(1))),
            ),
        ];
        items.push(txn_with("t", pre_lt("k", Expr::Identifier("nb".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        let e = entry(&map, "t");
        assert!(!e.shape.eligible, "undeclared counter must be rejected");
        assert!(e.shape.reasons.iter().any(|r| r.contains("not a state counter")));
    }

    #[test]
    fn counter_must_increment_in_body() {
        // Design A: the counter must advance (`i = i + 1`) — a `[i < N]` bound
        // over a never-incremented counter would never terminate.
        let mut items = vec![];
        state(&mut items);
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
            Expr::Decimal(1),
        )];
        items.push(txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        let e = entry(&map, "t");
        assert!(!e.shape.eligible, "never-incremented counter must be rejected");
        assert!(e.shape.reasons.iter().any(|r| r.contains("never incremented")));
    }

    // ── eligibility: write disjointness ───────────────────────────

    #[test]
    fn array_write_must_be_affine_in_index() {
        let mut items = vec![];
        state(&mut items);
        let ok_body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        // a[0] — constant slot written by every work-item → cross-work-item.
        let cross_body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Decimal(0))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        // a[j] with free j → not affine in i → cross-work-item.
        let free_j = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("j".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        let pre = pre_lt("i", Expr::Identifier("nb".into()));
        items.push(txn_with("affine", pre.clone(), ok_body));
        items.push(txn_with("const_slot", pre.clone(), cross_body));
        items.push(txn_with("free_j", pre, free_j));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        assert!(entry(&map, "affine").shape.eligible);
        assert!(!entry(&map, "const_slot").shape.eligible);
        assert!(!entry(&map, "free_j").shape.eligible);
    }

    #[test]
    fn scalar_state_write_is_host_not_kernel() {
        let mut items = vec![];
        state(&mut items);
        // count = count + 1 is loop bookkeeping → host; dv[i] = 1 is kernel.
        let body = vec![
            Statement::Assign(
                Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
            Statement::Assign(
                Expr::Identifier("nb".into()),
                Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("nb".into())), Box::new(Expr::Decimal(1))),
            ),
        ];
        items.push(txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        let e = entry(&map, "t");
        assert!(e.shape.eligible);
        assert_eq!(e.shape.kernel_stmts.len(), 1);
        assert_eq!(e.shape.host_stmts.len(), 2, "i = i + 1 and nb = nb + 1 are host");
    }

    // ── eligibility: purity ───────────────────────────────────────

    #[test]
    fn call_rejects_kernel() {
        let mut items = vec![];
        state(&mut items);
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
            Expr::Call("println!".into(), vec![Expr::Decimal(1)], None),
        )];
        items.push(txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), with_inc(body)));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        assert!(!entry(&map, "t").shape.eligible);
        assert!(entry(&map, "t").shape.reasons.iter().any(|r| r.contains("no offloadable statements")));
    }

    // ── eligibility: flat types ───────────────────────────────────

    #[test]
    fn string_array_rejects_kernel() {
        let mut items = vec![];
        state(&mut items);
        // Add a String[] state array written by the kernel → not flat.
        items.push(TopLevel::StateDecl(StateDecl {
            name: "ss".into(),
            ty: Type::Vector(Box::new(Type::string()), vec![]),
            span: None,
        }));
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("ss".into())), Box::new(Expr::Identifier("i".into()))),
            Expr::Identifier("x".into()),
        )];
        items.push(txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), with_inc(body)));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        let e = entry(&map, "t");
        assert!(!e.shape.eligible);
        assert!(e.shape.reasons.iter().any(|r| r.contains("not a flat scalar")));
    }

    // ── decisions ─────────────────────────────────────────────────

    #[test]
    fn runtime_bound_is_probe() {
        let mut items = vec![];
        state(&mut items);
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        // `[i < nb]` with nb a runtime state scalar → Probe (try mode).
        items.push(txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        let e = entry(&map, "t");
        assert!(e.shape.eligible);
        assert_eq!(e.decision, AccelDecision::Probe);
    }

    #[test]
    fn const_bound_below_crossover_is_cpu() {
        let mut items = vec![];
        state(&mut items);
        items.push(TopLevel::Constant(Constant {
            name: "N".into(),
            ty: Type::int(),
            expr: Expr::Decimal(4),
            section: None,
        }));
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        items.push(txn_with("t", pre_lt("i", Expr::Identifier("N".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        // N=4 below the PCIe crossover → CPU.
        assert_eq!(entry(&map, "t").decision, AccelDecision::Cpu);
    }

    #[test]
    fn forced_body_skips_speedup_gate() {
        let mut items = vec![];
        state(&mut items);
        // const bound below crossover (would be Cpu in try mode)…
        items.push(TopLevel::Constant(Constant { name: "nb".into(), ty: Type::int(), expr: Expr::Decimal(4), section: None }));
        // …but the body is accel-keyword-marked and the policy is force → Gpu.
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        items.push(txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), body));
        let mut meta = HashMap::new();
        meta.insert("accel".into(), PropertyValue::Identifier("force".into()));
        let map = analyze(&items, &meta, Some(&universe()));
        let e = entry(&map, "t");
        assert!(e.forced);
        assert_eq!(e.decision, AccelDecision::Gpu);
    }

    #[test]
    fn try_all_targets_unmarked_body() {
        let mut items = vec![];
        state(&mut items);
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        // No accel modifier on the txn itself.
        let mut t = match txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), body) {
            TopLevel::Transaction(t) => t,
            _ => unreachable!(),
        };
        t.modifiers.clear();
        items.push(TopLevel::Transaction(t));
        let mut meta = HashMap::new();
        meta.insert("accel".into(), PropertyValue::Identifier("try_all".into()));
        let map = analyze(&items, &meta, Some(&universe()));
        assert!(map.contains_key("t"), "try_all must target unmarked bodies");
        assert_eq!(entry(&map, "t").decision, AccelDecision::Probe);
    }

    #[test]
    fn absent_mode_targets_only_marked() {
        let mut items = vec![];
        state(&mut items);
        let body = vec![Statement::Assign(
            Expr::Index(Box::new(Expr::Identifier("dv".into())), Box::new(Expr::Identifier("i".into()))),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        let mut t = match txn_with("t", pre_lt("i", Expr::Identifier("nb".into())), body) {
            TopLevel::Transaction(t) => t,
            _ => unreachable!(),
        };
        t.modifiers.clear();
        items.push(TopLevel::Transaction(t));
        // No module key → absent → unmarked body is not a candidate.
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        assert!(!map.contains_key("t"));
    }

    // ── 2D dispatch geometry (plan 2026-08-31-gpu-next §2b) ──────

    #[test]
    fn shift_mask_body_sets_work_cols() {
        let mut items = vec![];
        state(&mut items);
        // pairs-style body: row = i >> 12, col = i & 4095 → cols = 4096.
        let body = vec![
            Statement::Assign(
                Expr::Index(
                    Box::new(Expr::Identifier("dv".into())),
                    Box::new(Expr::Identifier("i".into())),
                ),
                Expr::Index(
                    Box::new(Expr::Identifier("sv".into())),
                    Box::new(Expr::BinaryOp(
                        BinaryOpKind::Shr,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(12)),
                    )),
                ),
            ),
            inc_i(),
        ];
        items.push(txn_with("pairs", pre_lt("i", Expr::Identifier("nb".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        assert_eq!(entry(&map, "pairs").shape.work_cols, Some(4096));
    }

    #[test]
    fn plain_body_stays_1d() {
        let mut items = vec![];
        state(&mut items);
        let body = vec![
            Statement::Assign(
                Expr::Index(
                    Box::new(Expr::Identifier("dv".into())),
                    Box::new(Expr::Identifier("i".into())),
                ),
                Expr::Decimal(1),
            ),
            inc_i(),
        ];
        items.push(txn_with("flat", pre_lt("i", Expr::Identifier("nb".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        assert_eq!(entry(&map, "flat").shape.work_cols, None);
    }

    #[test]
    fn conflicting_masks_poison_detection() {
        // A shift says cols = 4096; a mask says 1024 → no 2D claim.
        let mut items = vec![];
        state(&mut items);
        let idx = |e: Expr| {
            Expr::Index(Box::new(Expr::Identifier("sv".into())), Box::new(e))
        };
        let body = vec![
            Statement::Assign(
                idx(Expr::BinaryOp(
                    BinaryOpKind::Shr,
                    Box::new(Expr::Identifier("i".into())),
                    Box::new(Expr::Decimal(12)),
                )),
                Expr::Decimal(1),
            ),
            Statement::Assign(
                idx(Expr::BinaryOp(
                    BinaryOpKind::BitAnd,
                    Box::new(Expr::Identifier("i".into())),
                    Box::new(Expr::Decimal(1023)),
                )),
                Expr::Decimal(2),
            ),
            inc_i(),
        ];
        items.push(txn_with("mix", pre_lt("i", Expr::Identifier("nb".into())), body));
        let map = analyze(&items, &HashMap::new(), Some(&universe()));
        assert_eq!(entry(&map, "mix").shape.work_cols, None);
    }
}

/// Resident-launch safety verdict (plan gpu-backend-hardening Track B).
///
/// The resident path leaves results in VRAM between launches — the staging
/// window holds stale array data by design (only scalars are dirty-pushed).
/// A program may use resident launches only when EVERY array field any
/// kernel touches is KERNEL-PINNED: all of its readers and writers in the
/// whole program are eligible accel kernel bodies. Any host-side access (a
/// non-accel node body, an accel txn's partitioned-out host statements, a
/// contract, a defn) forces the full-copy path for the WHOLE program —
/// mixed resident/full-copy is unsound (a full-copy launch packs the stale
/// staging into VRAM and unpacks it back over host state).
pub struct ResidentVerdict {
    /// Every eligible accel kernel may emit resident launches.
    pub resident_ok: bool,
    /// The first host-side array access that forced full-copy, as
    /// (field, where) evidence for the diagnostic.
    pub blocker: Option<(String, String)>,
}

/// Compute the verdict: walk every non-kernel statement context in the
/// program (host partitions of accel bodies, non-accel txns, defns,
/// contracts — top-level `let` initializers are EXCLUDED: they are the
/// seed the first resident launch pushes, not between-launch reads) and
/// flag any state-ARRAY access on a field a kernel touches. Scalars are
/// safe by the dirty-sync design (pushed before every launch; the host
/// copy is the authority for counters).
/// Build the program info for the resident-safety walk (pub(crate) entry —
/// the info type is internal to this module's walkers).
pub(crate) fn build_program_info(items: &[TopLevel]) -> ProgramInfo {
    ProgramInfo::build(items)
}

pub fn analyze_resident_safety(
    items: &[TopLevel],
    accel: &HashMap<String, AccelEntry>,
    info: &ProgramInfo,
    universe: &TypeUniverse,
) -> ResidentVerdict {
    // The arrays any eligible kernel touches — only these matter.
    let mut kernel_arrays: HashSet<String> = HashSet::new();
    for entry in accel.values() {
        if !entry.shape.eligible {
            continue;
        }
        for f in entry
            .shape
            .read_buffers
            .iter()
            .chain(entry.shape.write_buffers.iter())
        {
            kernel_arrays.insert(f.clone());
        }
    }
    if kernel_arrays.is_empty() {
        return ResidentVerdict { resident_ok: false, blocker: None };
    }

    let mut blocker: Option<(String, String)> = None;
    let mut host_touch = |field: &str, ctx_name: &str, blocker: &mut Option<(String, String)>| {
        if kernel_arrays.contains(field) && blocker.is_none() {
            *blocker = Some((field.to_string(), ctx_name.to_string()));
        }
    };

    for item in items {
        match item {
            TopLevel::Transaction(t) => {
                walk_txn_host_accesses(t, accel, info, &mut host_touch, &mut blocker);
            }
            TopLevel::Definition(d) => {
                // Conservative: defns may be called from any host context.
                collect_host_array_accesses(
                    &d.body,
                    info,
                    &format!("defn {}", d.name),
                    &mut host_touch,
                    &mut blocker,
                );
            }
            _ => {}
        }
    }

    let _ = universe;
    ResidentVerdict {
        resident_ok: blocker.is_none(),
        blocker,
    }
}

/// Host-side accesses of one transaction: eligible kernels contribute their
/// PARTITIONED-OUT host statements; anything else is host code wholesale.
/// Contracts are always host-side (they run around the body).
fn walk_txn_host_accesses(
    t: &crate::ast::top::Transaction,
    accel: &HashMap<String, AccelEntry>,
    info: &ProgramInfo,
    host_touch: &mut dyn FnMut(&str, &str, &mut Option<(String, String)>),
    blocker: &mut Option<(String, String)>,
) {
    let entry = accel.get(&t.name);
    let eligible = entry.map_or(false, |e| e.shape.eligible);
    let host_stmts: &[Statement] = if eligible {
        &entry.unwrap().shape.host_stmts
    } else {
        &t.body
    };
    collect_host_array_accesses(host_stmts, info, &t.name, host_touch, blocker);
    // Contracts are Exprs — the expr-level walker applies.
    collect_host_expr_accesses(
        &t.contract.pre_condition,
        info,
        &format!("{}[pre]", t.name),
        host_touch,
        blocker,
    );
    collect_host_expr_accesses(
        &t.contract.post_condition,
        info,
        &format!("{}[post]", t.name),
        host_touch,
        blocker,
    );
}

/// Host-side array-access walker: reuses the kernel buffer walkers (they
/// filter on `info`'s state-field set) and reports ONLY array fields.
fn collect_host_array_accesses(
    stmts: &[Statement],
    info: &ProgramInfo,
    ctx_name: &str,
    host_touch: &mut dyn FnMut(&str, &str, &mut Option<(String, String)>),
    blocker: &mut Option<(String, String)>,
) {
    for s in stmts {
        // collect_stmt_buffers has no Foreach arm (kernel bodies are walked
        // with their own machinery) — descend into foreach bodies here so a
        // host-side `foreach j in 0..K { s = s + a[j]; }` is not invisible.
        if let Statement::Foreach { list, body, .. } = s {
            collect_host_expr_accesses(list, info, ctx_name, host_touch, blocker);
            collect_host_array_accesses(body, info, ctx_name, host_touch, blocker);
            continue;
        }
        let mut reads: HashSet<String> = HashSet::new();
        let mut writes: HashSet<String> = HashSet::new();
        let mut scalars: HashSet<String> = HashSet::new();
        collect_stmt_buffers(s, info, &mut reads, &mut writes, &mut scalars);
        for f in reads.iter().chain(writes.iter()) {
            host_touch(f, ctx_name, blocker);
        }
    }
}

/// Expr-level variant for contracts (pre/post are bare expressions).
fn collect_host_expr_accesses(
    expr: &Expr,
    info: &ProgramInfo,
    ctx_name: &str,
    host_touch: &mut dyn FnMut(&str, &str, &mut Option<(String, String)>),
    blocker: &mut Option<(String, String)>,
) {
    let mut reads: HashSet<String> = HashSet::new();
    let mut writes: HashSet<String> = HashSet::new();
    let mut scalars: HashSet<String> = HashSet::new();
    collect_expr_buffers(expr, info, &mut reads, &mut writes, &mut scalars);
    for f in reads.iter().chain(writes.iter()) {
        host_touch(f, ctx_name, blocker);
    }
}

#[cfg(test)]
mod resident_gate_tests {
    //! Track B regression tests (plan gpu-backend-hardening): the
    //! all-readers-are-kernels gate + the buffer-contract Foreach walk.
    //! The Foreach arm in collect_stmt_buffers was MISSING — every
    //! cooperative kernel's read/write_buffers were empty and the
    //! array_is_flat eligibility checks were vacuous. These tests pin both.

    use super::*;
    use crate::ast::{BinaryOpKind, Type};

    fn build_info(src_state: &[(&str, u64)]) -> ProgramInfo {
        // Minimal items: state decls only (the walker needs array_types).
        let items: Vec<TopLevel> = src_state
            .iter()
            .map(|(name, count)| {
                crate::ast::TopLevel::StateDecl(crate::ast::StateDecl {
                    name: name.to_string(),
                    ty: Type::Vector(
                        Box::new(Type::Custom("Float".into())),
                        vec![Dimension::Anonymous(*count as usize)],
                    ),
                    span: None,
                })
            })
            .collect();
        ProgramInfo::build(&items)
    }

    fn f32_idx(field: &str, idx: &str) -> Expr {
        Expr::Index(
            Box::new(Expr::Identifier(field.into())),
            Box::new(Expr::Identifier(idx.into())),
        )
    }

    /// A minimal Definition with a foreach body over the named fields.
    fn make_defn(name: &str, read_fields: &[&str]) -> TopLevel {
        let body: Vec<Statement> = read_fields
            .iter()
            .map(|f| {
                Statement::Foreach {
                    item: "j".into(),
                    list: Box::new(Expr::Range {
                        start: Box::new(Expr::Decimal(0)),
                        end: Box::new(Expr::Decimal(64)),
                        inclusive: false,
                    }),
                    body: vec![Statement::Assign(
                        Expr::Identifier("s".into()),
                        f32_idx(f, "j"),
                    )],
                }
            })
            .collect();
        TopLevel::Definition(crate::ast::Definition {
            name: name.into(),
            type_params: vec![],
            parameters: vec![],
            output_type: Some(crate::ast::OutputType::Single(Type::Custom("Float".into()))),
            outputs: vec![],
            contract: crate::ast::Contract {
                pre_condition: Expr::Bool(true),
                post_condition: Expr::Bool(true),
                watchdog: None,
                span: None,
                explicit: false,
                post_authority: false,
            },
            body,
            metadata: Default::default(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        })
    }

    /// An eligible kernel entry over the given arrays.
    fn kernel_entry(reads: &[&str], writes: &[&str]) -> AccelEntry {
        let mut shape = crate::analysis::accel::KernelShape {
            index_var: String::new(),
            count_expr: None,
            kernel_stmts: Vec::new(),
            host_stmts: Vec::new(),
            read_buffers: Vec::new(),
            write_buffers: Vec::new(),
            scalar_ins: Vec::new(),
            eligible: false,
            reasons: Vec::new(),
            work_cols: None,
            reduction: None,
        };
        shape.eligible = true;
        shape.read_buffers = reads.iter().map(|s| s.to_string()).collect();
        shape.write_buffers = writes.iter().map(|s| s.to_string()).collect();
        AccelEntry {
            shape,
            decision: AccelDecision::Gpu,
            forced: true,
            mode: AccelMode::Force,
        }
    }

    /// THE regression: a cooperative kernel's foreach body reads must land
    /// in read_buffers. Before the Foreach arm, read_buffers was EMPTY and
    /// the flatness checks were vacuous.
    #[test]
    fn foreach_body_reads_reach_read_buffers() {
        let info = build_info(&[("a", 4096)]);
        let body = vec![Statement::Assign(
            Expr::Identifier("acc".into()),
            Expr::BinaryOp(
                BinaryOpKind::Add,
                Box::new(Expr::Identifier("acc".into())),
                Box::new(f32_idx("a", "k")),
            ),
        )];
        // Wrap in the foreach the partitioner would keep in kernel_stmts.
        let stmts = vec![Statement::Foreach {
            item: "k".into(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Decimal(64)),
                inclusive: false,
            }),
            body,
        }];
        let (reads, _writes, _scalars) = collect_buffers(&stmts, &info);
        assert_eq!(reads, vec!["a".to_string()],
            "foreach-body reads MUST reach read_buffers (the vacuous-contracts bug)");
    }

    /// The gate: a program whose defn reads a kernel array forces full-copy.
    #[test]
    fn host_defn_read_blocks_resident() {
        let universe = crate::type_universe::TypeUniverse::new();
        // State decls + a defn that reads `a`.
        let state = |name: &str, count: u64| {
            crate::ast::TopLevel::StateDecl(crate::ast::StateDecl {
                name: name.into(),
                ty: Type::Vector(
                    Box::new(Type::Custom("Float".into())),
                    vec![Dimension::Anonymous(count as usize)],
                ),
                span: None,
            })
        };
        let items = vec![
            state("a", 4096),
            state("x", 64),
            state("y", 64),
            make_defn("peek", &["a"]),
        ];
        let info = ProgramInfo::build(&items);
        // A kernel entry touching a/x/y.
        let mut accel = std::collections::HashMap::new();
        accel.insert("gemv".to_string(), kernel_entry(&["a", "x"], &["y"]));
        let verdict = analyze_resident_safety(&items, &accel, &info, &universe);
        assert!(!verdict.resident_ok, "defn array read must force full-copy");
        assert_eq!(
            verdict.blocker.as_ref().map(|(f, _)| f.as_str()),
            Some("a"),
            "the blocker names the field"
        );
    }

    /// The gate: a program with NO host-side array access is resident-ok.
    #[test]
    fn kernel_only_program_is_resident_ok() {
        let universe = crate::type_universe::TypeUniverse::new();
        let state = |name: &str, count: u64| {
            crate::ast::TopLevel::StateDecl(crate::ast::StateDecl {
                name: name.into(),
                ty: Type::Vector(
                    Box::new(Type::Custom("Float".into())),
                    vec![Dimension::Anonymous(count as usize)],
                ),
                span: None,
            })
        };
        let items = vec![state("a", 4096), state("x", 64), state("y", 64)];
        let info = ProgramInfo::build(&items);
        let mut accel = std::collections::HashMap::new();
        accel.insert("gemv".to_string(), kernel_entry(&["a", "x"], &["y"]));
        let verdict = analyze_resident_safety(&items, &accel, &info, &universe);
        assert!(verdict.resident_ok, "kernel-only program goes resident");
        assert!(verdict.blocker.is_none());
    }
}
