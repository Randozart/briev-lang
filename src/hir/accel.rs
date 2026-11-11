// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! 2026-11-11 (H5 port): GPU-deferral shape analysis over typed HIR.
//!
//! Mirrors `analysis::accel` (the AST remains the oracle until the three
//! consumers — coalescing, image storage, GPU scheduling — flip). Detection,
//! eligibility proofs, buffer contracts, launch geometry, const extraction,
//! and index-math recognition are all derived from HIR nodes.
//!
//! Result type: the public `analysis::accel::{AccelEntry, KernelShape, …}`
//! structs are reused so the existing consumers can read the HIR analysis
//! unchanged once flipped. Their payload fields are AST-shaped
//! (`kernel_stmts`, `host_stmts`, `count_expr`, reduction/normalize end
//! expressions), so this module reconstructs those bounded fragments once at
//! the RESULT BOUNDARY via `hir::to_ast`. No shape fact is derived from the
//! reconstructed AST; the fragments exist only because the consumers re-emit
//! them. A node `to_ast` cannot represent makes the kernel ineligible with a
//! named reason instead of guessing.

use super::expr::{HirExpr, HirExprKind, HirLiteral};
use super::pattern::HirPatternKind;
use super::program::{HirCallable, HirCallableKind, HirProgram};
use super::stmt::{HirStmt, HirStmtKind};
use crate::analysis::accel::{
    is_flat_scalar, AccelDecision, AccelEntry, AccelMode, DeferredNormalization, FoldIdiomBlock,
    KernelShape, ReductionInfo, ReductionKind,
};
use crate::ast::{BinaryOpKind, Type};
use crate::type_universe::TypeUniverse;
use std::collections::{HashMap, HashSet};

/// State-field facts extracted from the HIR program.
pub(crate) struct HirProgramInfo {
    state_fields: HashSet<String>,
    pub(crate) array_types: HashMap<String, Type>,
    consts: HashMap<String, HirExpr>,
    const_types: HashMap<String, Type>,
}

impl HirProgramInfo {
    pub(crate) fn build(program: &HirProgram) -> HirProgramInfo {
        let mut state_fields = HashSet::new();
        let mut array_types = HashMap::new();
        for global in &program.globals {
            state_fields.insert(global.name.clone());
            if let Some(ty) = &global.ty {
                if matches!(ty, Type::Vector(..) | Type::Applied(..)) {
                    array_types.insert(global.name.clone(), ty.clone());
                }
            }
        }
        let consts = program.const_exprs.iter().cloned().collect();
        let const_types = program.const_types.iter().cloned().collect();
        HirProgramInfo {
            state_fields,
            array_types,
            consts,
            const_types,
        }
    }

    /// Flatness of a referenced array through the shared universe predicate.
    fn array_is_flat(&self, name: &str, universe: &TypeUniverse) -> bool {
        let ty = self
            .array_types
            .get(name)
            .or_else(|| self.const_types.get(name));
        match ty {
            Some(Type::Vector(inner, _)) => is_flat_scalar(universe, inner),
            Some(Type::Applied(_, args)) => {
                let elem = args.first().unwrap();
                is_flat_scalar(universe, elem)
            }
            Some(ty) => is_flat_scalar(universe, ty),
            None => false,
        }
    }
}

/// The work-item bound conjunct: strip `beginprogram` markers, then scan
/// `&&` conjunctions left-to-right for the first `identifier < N`.
fn work_item_bound(pre: &HirExpr) -> Option<(String, HirExpr)> {
    match &strip_beginprogram(pre).kind {
        HirExprKind::Binary {
            op: BinaryOpKind::And,
            left,
            right,
        } => work_item_bound(left).or_else(|| work_item_bound(right)),
        HirExprKind::Binary {
            op: BinaryOpKind::Lt,
            left,
            right,
        } if matches!(&left.kind, HirExprKind::Identifier(_)) => match &left.kind {
            HirExprKind::Identifier(s) => Some((s.clone(), (**right).clone())),
            _ => unreachable!(),
        },
        _ => None,
    }
}

/// Remove `BeginProgram` conjuncts, keeping a singleton marker as `true`.
fn strip_beginprogram(pre: &HirExpr) -> HirExpr {
    match &pre.kind {
        HirExprKind::Literal(HirLiteral::BeginProgram) => {
            let mut out = pre.clone();
            out.kind = HirExprKind::Literal(HirLiteral::Bool(true));
            out
        }
        HirExprKind::Binary {
            op: BinaryOpKind::And,
            left,
            right,
        } => {
            let a = strip_beginprogram(left);
            let b = strip_beginprogram(right);
            let a_true = matches!(a.kind, HirExprKind::Literal(HirLiteral::Bool(true)));
            let b_true = matches!(b.kind, HirExprKind::Literal(HirLiteral::Bool(true)));
            if a_true {
                return b;
            }
            if b_true {
                return a;
            }
            let mut out = pre.clone();
            out.kind = HirExprKind::Binary {
                op: BinaryOpKind::And,
                left: Box::new(a),
                right: Box::new(b),
            };
            out
        }
        _ => pre.clone(),
    }
}

/// True when the body increments `var` by a positive delta.
fn body_increments_counter(body: &[HirStmt], var: &str) -> bool {
    body.iter().any(|stmt| {
        let HirStmtKind::Assign(lhs, rhs) = &stmt.kind else {
            return false;
        };
        matches!(&lhs.kind, HirExprKind::Identifier(n) if n == var) && is_self_increment(rhs, var)
    })
}

fn is_self_increment(rhs: &HirExpr, var: &str) -> bool {
    let HirExprKind::Binary { op, left, right } = &rhs.kind else {
        return false;
    };
    let is_var = |e: &HirExpr| matches!(&e.kind, HirExprKind::Identifier(v) if v == var);
    match op {
        BinaryOpKind::Add => {
            (is_var(left) && const_delta(right).map_or(false, |d| d > 0))
                || (is_var(right) && const_delta(left).map_or(false, |d| d > 0))
        }
        BinaryOpKind::Sub => is_var(left) && const_delta(right).map_or(false, |d| d > 0),
        _ => false,
    }
}

fn const_delta(expr: &HirExpr) -> Option<i64> {
    match &expr.kind {
        HirExprKind::Literal(HirLiteral::Int(n)) => Some(*n),
        HirExprKind::Literal(HirLiteral::Char(c)) => Some(*c as i64),
        _ => None,
    }
}

/// True when `expr` mentions `var` anywhere.
fn expr_contains(expr: &HirExpr, var: &str) -> bool {
    match &expr.kind {
        HirExprKind::Identifier(name) => name == var,
        HirExprKind::Binary { left, right, .. } => {
            expr_contains(left, var) || expr_contains(right, var)
        }
        HirExprKind::Unary { inner, .. } => expr_contains(inner, var),
        HirExprKind::Index(a, i) => expr_contains(a, var) || expr_contains(i, var),
        HirExprKind::Cast { inner, .. } => expr_contains(inner, var),
        HirExprKind::Field { receiver, .. } => expr_contains(receiver, var),
        HirExprKind::Call { args, .. } => args.iter().any(|a| expr_contains(a, var)),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            items.iter().any(|i| expr_contains(i, var))
        }
        _ => false,
    }
}

/// `a*var + b` with constant `a`/`b`; identifiers are linear.
fn is_linear_in(expr: &HirExpr, var: &str) -> bool {
    match &expr.kind {
        HirExprKind::Identifier(_) => true,
        HirExprKind::Literal(
            HirLiteral::Int(_) | HirLiteral::Char(_) | HirLiteral::Bool(_) | HirLiteral::Float(_),
        ) => true,
        HirExprKind::Binary {
            op: BinaryOpKind::Add | BinaryOpKind::Sub,
            left,
            right,
        } => is_linear_in(left, var) && is_linear_in(right, var),
        HirExprKind::Binary {
            op: BinaryOpKind::Mul,
            left,
            right,
        } => {
            (expr_contains(left, var)
                && !expr_contains(right, var)
                && is_linear_in(left, var)
                && is_linear_in(right, var))
                || (!expr_contains(left, var)
                    && expr_contains(right, var)
                    && is_linear_in(left, var)
                    && is_linear_in(right, var))
        }
        _ => false,
    }
}

/// Purity: kernel expressions are pure arithmetic/data. Rejects calls except
/// the explicit offload-safe intrinsic set, reflection, pointers, control
/// flow, and observables.
fn expr_is_pure(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Literal(
            HirLiteral::Bytes(_)
            | HirLiteral::Int(_)
            | HirLiteral::Char(_)
            | HirLiteral::TaggedInt(_, _)
            | HirLiteral::TaggedBytes(_, _)
            | HirLiteral::Bool(_)
            | HirLiteral::Float(_),
        )
        | HirExprKind::Identifier(_) => true,
        HirExprKind::Binary { left, right, .. } => expr_is_pure(left) && expr_is_pure(right),
        HirExprKind::Unary { inner, .. } => expr_is_pure(inner),
        HirExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_is_pure(condition)
                && expr_is_pure(then_branch)
                && else_branch.as_deref().map(expr_is_pure).unwrap_or(true)
        }
        HirExprKind::Match { scrutinee, arms } => {
            expr_is_pure(scrutinee)
                && arms.iter().all(|arm| {
                    arm.guard.is_none()
                        && matches!(
                            &arm.pattern.kind,
                            HirPatternKind::Literal(HirExpr {
                                kind: HirExprKind::Literal(HirLiteral::Bool(_)),
                                ..
                            }) | HirPatternKind::Wildcard
                        )
                        && expr_is_pure(&arm.body)
                })
        }
        HirExprKind::Index(a, i) => expr_is_pure(a) && expr_is_pure(i),
        HirExprKind::Cast { inner, .. } => expr_is_pure(inner),
        HirExprKind::Tuple(items) | HirExprKind::List(items) => items.iter().all(expr_is_pure),
        HirExprKind::Call { callee, args, .. } if pure_intrinsic(callee) => {
            args.iter().all(expr_is_pure)
        }
        _ => false,
    }
}

fn pure_intrinsic(name: &str) -> bool {
    matches!(
        name,
        "SubgroupFAdd#"
            | "SubgroupFMul#"
            | "SubgroupIAdd#"
            | "SubgroupIMul#"
            | "SubgroupSMin#"
            | "SubgroupSMax#"
            | "SubgroupFMax#"
            | "SubgroupFMin#"
            | "ShuffleDown#"
            | "ShuffleXor#"
            | "SubgroupBallot#"
            | "SubgroupBroadcast#"
            | "Fma#"
            | "Max#"
            | "Min#"
            | "GetGlobalId#"
            | "GetLocalId#"
            | "Exp#"
            | "Sqrt#"
            | "Fabs#"
    ) || (name.starts_with("Atomic") && name.ends_with("At#"))
}

struct KernelCtx<'a> {
    index_var: &'a str,
    info: &'a HirProgramInfo,
    reasons: &'a mut Vec<String>,
}

struct PartitionOut {
    kernel: Vec<HirStmt>,
    host: Vec<HirStmt>,
    locals: HashSet<String>,
}

/// The HIR statements the accel partition classifies as kernel-safe, in
/// source order. Sibling GPU-family ports (image storage, coalescing,
/// scheduling) scan HIR directly; routing them through this helper keeps the
/// kernel/host split in ONE place instead of re-deriving it per consumer.
pub(crate) fn kernel_statements(
    program: &HirProgram,
    callable: &HirCallable,
    index_var: &str,
) -> Vec<HirStmt> {
    let info = HirProgramInfo::build(program);
    let mut reasons = Vec::new();
    let mut ctx = KernelCtx {
        index_var,
        info: &info,
        reasons: &mut reasons,
    };
    partition(&callable.body.statements, &mut ctx).kernel
}

fn partition(body: &[HirStmt], ctx: &mut KernelCtx<'_>) -> PartitionOut {
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

fn stmt_is_kernel(stmt: &HirStmt, ctx: &mut KernelCtx<'_>, locals: &mut HashSet<String>) -> bool {
    match &stmt.kind {
        HirStmtKind::Let {
            name,
            expr: Some(e),
            ..
        } => {
            if expr_is_pure(e) {
                locals.insert(name.clone());
                true
            } else {
                false
            }
        }
        HirStmtKind::Assign(lhs, rhs) => assign_is_kernel(lhs, rhs, ctx, locals),
        HirStmtKind::Guarded(cond, body) => {
            expr_is_pure(cond)
                && !body.is_empty()
                && body.iter().all(|s| stmt_is_kernel(s, ctx, locals))
        }
        HirStmtKind::Block(body) => {
            !body.is_empty() && body.iter().all(|s| stmt_is_kernel(s, ctx, locals))
        }
        HirStmtKind::Foreach {
            item, list, body, ..
        } => {
            let HirExprKind::Range { start, end, .. } = &list.kind else {
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

fn assign_is_kernel(
    lhs: &HirExpr,
    rhs: &HirExpr,
    ctx: &mut KernelCtx<'_>,
    locals: &mut HashSet<String>,
) -> bool {
    match &lhs.kind {
        HirExprKind::Index(arr, idx) => index_write_is_kernel(arr, idx, rhs, ctx),
        HirExprKind::Identifier(name) => {
            !ctx.info.state_fields.contains(name) && locals.contains(name) && expr_is_pure(rhs)
        }
        _ => false,
    }
}

fn index_write_is_kernel(
    arr: &HirExpr,
    idx: &HirExpr,
    rhs: &HirExpr,
    ctx: &mut KernelCtx<'_>,
) -> bool {
    if !expr_is_pure(rhs) {
        return false;
    }
    let HirExprKind::Identifier(arr_name) = &arr.kind else {
        return false;
    };
    if !ctx.info.state_fields.contains(arr_name) {
        ctx.reasons
            .push(format!("write to non-state array '{}'", arr_name));
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

/// HIR-native reduction facts; converted to the public AST payload only at
/// the result boundary.
struct HirReduction {
    inner: HirExpr,
    kind: ReductionKind,
    row_buf: String,
    col_buf: String,
    out_buf: String,
    seq: bool,
    foreach_approx: bool,
    foreach_exact: bool,
}

impl HirReduction {
    fn into_ast(self) -> Option<ReductionInfo> {
        Some(ReductionInfo {
            inner: super::to_ast::expr(&self.inner)?,
            kind: self.kind,
            row_buf: self.row_buf,
            col_buf: self.col_buf,
            out_buf: self.out_buf,
            seq: self.seq,
            foreach_approx: self.foreach_approx,
            foreach_exact: self.foreach_exact,
        })
    }
}

struct HirDeferred {
    denominator: String,
    acc_buf: String,
    out_buf: String,
    normalize_var: String,
    normalize_end: HirExpr,
    reduce_end: HirExpr,
}

impl HirDeferred {
    fn into_ast(self) -> Option<DeferredNormalization> {
        Some(DeferredNormalization {
            denominator: self.denominator,
            acc_buf: self.acc_buf,
            out_buf: self.out_buf,
            normalize_var: self.normalize_var,
            normalize_end: super::to_ast::expr(&self.normalize_end)?,
            reduce_end: super::to_ast::expr(&self.reduce_end)?,
        })
    }
}

/// Accumulating proof state; `finish` is the single AST conversion point.
struct HirProof {
    index_var: String,
    count_expr: Option<HirExpr>,
    bound_n: Option<u64>,
    kernel: Vec<HirStmt>,
    host: Vec<HirStmt>,
    read_buffers: Vec<String>,
    write_buffers: Vec<String>,
    scalar_ins: Vec<String>,
    reasons: Vec<String>,
    work_cols: Option<u64>,
    reduction: Option<HirReduction>,
    deferred: Option<HirDeferred>,
    idiom_lane: bool,
    approx_declared: bool,
    exact_declared: bool,
}

impl HirProof {
    fn new(approx_declared: bool, exact_declared: bool) -> HirProof {
        HirProof {
            index_var: String::new(),
            count_expr: None,
            bound_n: None,
            kernel: Vec::new(),
            host: Vec::new(),
            read_buffers: Vec::new(),
            write_buffers: Vec::new(),
            scalar_ins: Vec::new(),
            reasons: Vec::new(),
            work_cols: None,
            reduction: None,
            deferred: None,
            idiom_lane: false,
            approx_declared,
            exact_declared,
        }
    }

    fn finish(self) -> (KernelShape, Option<u64>) {
        let HirProof {
            index_var,
            count_expr,
            bound_n,
            kernel,
            host,
            read_buffers,
            write_buffers,
            scalar_ins,
            mut reasons,
            work_cols,
            reduction,
            deferred,
            idiom_lane,
            approx_declared,
            exact_declared,
        } = self;
        let kernel_stmts = convert_stmts("kernel", &kernel, &mut reasons);
        let host_stmts = convert_stmts("host partition", &host, &mut reasons);
        let count_expr = convert_count(count_expr, &mut reasons);
        let reduction = convert_reduction(reduction, &mut reasons);
        let deferred_normalize = convert_deferred(deferred, &mut reasons);
        let eligible = reasons.is_empty();
        (
            KernelShape {
                index_var,
                count_expr,
                kernel_stmts,
                host_stmts,
                read_buffers,
                write_buffers,
                scalar_ins,
                eligible,
                reasons,
                work_cols,
                reduction,
                deferred_normalize,
                idiom_lane,
                approx_declared,
                exact_declared,
            },
            bound_n,
        )
    }
}

fn convert_stmts(
    label: &str,
    stmts: &[HirStmt],
    reasons: &mut Vec<String>,
) -> Vec<crate::ast::Statement> {
    match super::to_ast::stmt_body(stmts) {
        Some(v) => v,
        None => {
            reasons.push(format!(
                "{label} contains a statement form the AST consumer payload cannot represent"
            ));
            Vec::new()
        }
    }
}

fn convert_count(
    count_expr: Option<HirExpr>,
    reasons: &mut Vec<String>,
) -> Option<crate::ast::Expr> {
    let Some(e) = count_expr else {
        return None;
    };
    match super::to_ast::expr(&e) {
        Some(e) => Some(e),
        None => {
            reasons.push(
                "work-item bound expression is not representable in the AST consumer payload"
                    .to_string(),
            );
            None
        }
    }
}

fn convert_reduction(
    reduction: Option<HirReduction>,
    reasons: &mut Vec<String>,
) -> Option<ReductionInfo> {
    let Some(r) = reduction else {
        return None;
    };
    match r.into_ast() {
        Some(r) => Some(r),
        None => {
            reasons.push(
                "reduction span is not representable in the AST consumer payload".to_string(),
            );
            None
        }
    }
}

fn convert_deferred(
    deferred: Option<HirDeferred>,
    reasons: &mut Vec<String>,
) -> Option<DeferredNormalization> {
    let Some(d) = deferred else {
        return None;
    };
    match d.into_ast() {
        Some(d) => Some(d),
        None => {
            reasons.push(
                "normalize span is not representable in the AST consumer payload".to_string(),
            );
            None
        }
    }
}

/// Shared environment for one kernel proof.
struct ProveCtx<'a> {
    name: &'a str,
    info: &'a HirProgramInfo,
    universe: &'a TypeUniverse,
    program: &'a HirProgram,
}

/// Prove the whole kernel and collect its buffer contracts.
fn prove_kernel(
    ctx: &ProveCtx<'_>,
    body: &[HirStmt],
    contract: Option<&super::program::HirContract>,
    approx_declared: bool,
    exact_declared: bool,
) -> (KernelShape, Option<u64>) {
    let mut proof = HirProof::new(approx_declared, exact_declared);
    if !prove_bound(ctx.name, body, contract, ctx.info, &mut proof) {
        return proof.finish();
    }
    let PartitionOut { kernel, host, .. } = {
        let mut kctx = KernelCtx {
            index_var: &proof.index_var,
            info: ctx.info,
            reasons: &mut proof.reasons,
        };
        partition(body, &mut kctx)
    };
    proof.kernel = kernel;
    proof.host = host;
    if proof.kernel.is_empty() {
        proof.reasons.push(format!(
            "accel '{}' has no offloadable statements (pure, disjoint, work-item-affine)",
            ctx.name
        ));
        return proof.finish();
    }
    collect_and_check_buffers(ctx.name, &mut proof, ctx.info, ctx.universe);
    proof.work_cols = detect_work_cols(&proof.kernel, &proof.index_var);
    detect_reductions(&mut proof);
    proof.deferred = detect_deferred_normalizer(&proof.kernel, &proof.index_var);
    proof.idiom_lane = fold_idiom_lane_hir(&proof, ctx.program);
    proof.finish()
}

/// The bound proof: `[i < N]` over a real, incrementing state counter.
fn prove_bound(
    name: &str,
    body: &[HirStmt],
    contract: Option<&super::program::HirContract>,
    info: &HirProgramInfo,
    proof: &mut HirProof,
) -> bool {
    let Some(contract) = contract else {
        proof.reasons.push(format!(
            "accel '{}' requires a work-item bound precondition '[i < N]' over a real counter 'i'",
            name
        ));
        return false;
    };
    let Some((index_var, count_expr)) = work_item_bound(&contract.pre) else {
        proof.reasons.push(format!(
            "accel '{}' requires a work-item bound precondition '[i < N]' over a real counter 'i'",
            name
        ));
        return false;
    };
    if !info.state_fields.contains(&index_var) {
        proof.reasons.push(format!(
            "accel '{}' bound variable '{}' is not a state counter — declare 'let {}: Int = 0;' and increment it in the body",
            name, index_var, index_var
        ));
        return false;
    }
    if !body_increments_counter(body, &index_var) {
        proof.reasons.push(format!(
            "accel '{}' bound counter '{}' is never incremented ('{} = {} + 1') — the node would not terminate",
            name, index_var, index_var, index_var
        ));
        return false;
    }
    proof.bound_n = constant_n(Some(&count_expr), &info.consts);
    proof.index_var = index_var;
    proof.count_expr = Some(count_expr);
    true
}

fn collect_and_check_buffers(
    name: &str,
    proof: &mut HirProof,
    info: &HirProgramInfo,
    universe: &TypeUniverse,
) {
    let (reads, writes, mut scalars) = collect_buffers(&proof.kernel, info);
    scalars.retain(|s| s != &proof.index_var);
    proof.read_buffers = reads;
    proof.write_buffers = writes;
    proof.scalar_ins = scalars;
    for buf in proof.read_buffers.iter().chain(proof.write_buffers.iter()) {
        if !info.array_is_flat(buf, universe) {
            proof.reasons.push(format!(
                "accel '{}' array '{}' is not a flat scalar type (needs Int/UInt/Float/Bool/Char)",
                name, buf
            ));
        }
    }
}

fn detect_reductions(proof: &mut HirProof) {
    proof.reduction = detect_reduction(&proof.kernel, &proof.index_var);
    if proof.reduction.is_none() {
        proof.reduction = detect_idiom_fold(&proof.kernel);
    }
    if let Some(red) = proof.reduction.as_mut() {
        if matches!(
            red.kind,
            ReductionKind::Dot
                | ReductionKind::Sum
                | ReductionKind::Product
                | ReductionKind::Min
                | ReductionKind::Max
        ) {
            red.out_buf = proof.write_buffers.first().cloned().unwrap_or_default();
        }
    }
    if proof.reduction.is_none() {
        proof.reduction = detect_row_softmax(&proof.kernel, &proof.index_var);
    }
}

/// Analyze every candidate body. Requires a populated `TypeUniverse` for the
/// flat-type proof.
pub fn analyze(
    program: &HirProgram,
    universe: Option<&TypeUniverse>,
    mode_override: Option<AccelMode>,
) -> HashMap<String, AccelEntry> {
    let mut out = HashMap::new();
    let Some(universe) = universe else {
        return out;
    };
    let mode = mode_override.unwrap_or(AccelMode::TryKeyword);
    let info = HirProgramInfo::build(program);
    for callable in candidate_callables(program) {
        let marked = callable.modifiers.iter().any(|m| m == "accel");
        if !(targets_all(mode) || (targets_marked(mode) && marked)) {
            continue;
        }
        let forced = forces_marked(mode) && marked;
        let approx_declared = callable.modifiers.iter().any(|m| m == "approx");
        let exact_declared = callable.modifiers.iter().any(|m| m == "exact");
        let ctx = ProveCtx {
            name: &callable.name,
            info: &info,
            universe,
            program,
        };
        let (shape, bound_n) = prove_kernel(
            &ctx,
            &callable.body.statements,
            callable.contract.as_ref(),
            approx_declared,
            exact_declared,
        );
        let decision = decide(&shape, forced, bound_n);
        emit_explain(&callable.name, &shape, decision);
        out.insert(
            callable.name.clone(),
            AccelEntry {
                mode,
                forced,
                shape,
                decision,
            },
        );
    }
    out
}

/// Top-level transaction candidates only — the AST `analyze` sees bare
/// `TopLevel::Transaction` (not member/exported/sync-wrapped/compile-time).
fn candidate_callables(program: &HirProgram) -> impl Iterator<Item = &HirCallable> {
    program.callables.iter().filter(|c| {
        c.kind == HirCallableKind::Transaction
            && c.owner.is_none()
            && !c.exported
            && c.sync_domains.is_empty()
    })
}

fn targets_marked(mode: AccelMode) -> bool {
    matches!(
        mode,
        AccelMode::TryKeyword | AccelMode::Force | AccelMode::TryAllForce
    )
}

fn targets_all(mode: AccelMode) -> bool {
    matches!(mode, AccelMode::TryAll | AccelMode::TryAllForce)
}

fn forces_marked(mode: AccelMode) -> bool {
    matches!(mode, AccelMode::Force | AccelMode::TryAllForce)
}

fn emit_explain(name: &str, shape: &KernelShape, decision: AccelDecision) {
    if std::env::var_os("BRIEV_ACCEL_EXPLAIN").is_none() {
        return;
    }
    let reasons = if shape.reasons.is_empty() {
        "-".to_string()
    } else {
        shape.reasons.join("; ")
    };
    eprintln!(
        "[accel] {}: eligible={} decision={:?} reasons={}",
        name, shape.eligible, decision, reasons
    );
}

/// Resolve the work-item count to a compile-time constant.
fn constant_n(count_expr: Option<&HirExpr>, consts: &HashMap<String, HirExpr>) -> Option<u64> {
    match count_expr.map(|e| &e.kind) {
        Some(HirExprKind::Literal(HirLiteral::Int(n))) if *n > 0 => Some(*n as u64),
        Some(HirExprKind::Identifier(name)) => match consts.get(name).map(|e| &e.kind) {
            Some(HirExprKind::Literal(HirLiteral::Int(n))) if *n > 0 => Some(*n as u64),
            _ => None,
        },
        _ => None,
    }
}

fn decide(shape: &KernelShape, forced: bool, bound_n: Option<u64>) -> AccelDecision {
    if !shape.eligible {
        return AccelDecision::Cpu;
    }
    if forced {
        return AccelDecision::Gpu;
    }
    match bound_n {
        Some(n) => {
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

/// Recognize the three-phase row softmax over the kernel statements.
fn detect_row_softmax(stmts: &[HirStmt], _index_var: &str) -> Option<HirReduction> {
    let mut max_local: Option<String> = None;
    let mut sum_local: Option<String> = None;
    let mut row_buf: Option<String> = None;
    let mut out_buf: Option<String> = None;
    let mut loops: Vec<HirExpr> = Vec::new();

    for stmt in stmts {
        match &stmt.kind {
            HirStmtKind::Let {
                name,
                expr: Some(e),
                ..
            } => {
                note_init(name, e, &mut max_local, &mut sum_local, &mut row_buf);
            }
            HirStmtKind::Foreach { list, body, .. } => {
                let pass = loop_index(list, &mut loops)?;
                let ok = match pass {
                    1 => match_max_pass(body),
                    2 => match_expsum_pass(body),
                    3 => {
                        out_buf = match_normalize_pass(body);
                        out_buf.is_some()
                    }
                    _ => false,
                };
                if !ok {
                    return None;
                }
            }
            _ => {}
        }
    }

    let _ = (max_local?, sum_local?);
    let row = row_buf?;
    let out = out_buf?;
    if loops.len() != 3 || !same_span(&loops[0], &loops[1]) || !same_span(&loops[0], &loops[2]) {
        return None;
    }
    Some(HirReduction {
        inner: loops[0].clone(),
        kind: ReductionKind::Softmax,
        row_buf: row,
        col_buf: String::new(),
        out_buf: out,
        seq: false,
        foreach_approx: false,
        foreach_exact: false,
    })
}

/// Structural equality without HIR node ids (each retyped expression gets a
/// fresh id). Uses the AST reconstruction, which drops ids and `via`.
fn same_span(a: &HirExpr, b: &HirExpr) -> bool {
    match (super::to_ast::expr(a), super::to_ast::expr(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn note_init(
    name: &str,
    e: &HirExpr,
    max_local: &mut Option<String>,
    sum_local: &mut Option<String>,
    row_buf: &mut Option<String>,
) {
    if max_local.is_none() {
        if let Some(row) = row_init_buffer(e) {
            *max_local = Some(name.to_string());
            *row_buf = Some(row);
        }
    } else if sum_local.is_none() && matches!(&e.kind, HirExprKind::Literal(HirLiteral::Int(_))) {
        *sum_local = Some(name.to_string());
    }
}

fn row_init_buffer(e: &HirExpr) -> Option<String> {
    let HirExprKind::Index(buf, _) = &e.kind else {
        return None;
    };
    match &buf.kind {
        HirExprKind::Identifier(n) => Some(n.clone()),
        _ => None,
    }
}

fn loop_index(list: &HirExpr, loops: &mut Vec<HirExpr>) -> Option<usize> {
    let HirExprKind::Range { end, .. } = &list.kind else {
        return None;
    };
    loops.push((**end).clone());
    Some(loops.len())
}

fn match_max_pass(body: &[HirStmt]) -> bool {
    if body.len() != 2 {
        return false;
    }
    if !matches!(
        &body[0].kind,
        HirStmtKind::Let {
            expr: Some(HirExpr {
                kind: HirExprKind::Index(_, _),
                ..
            }),
            ..
        }
    ) {
        return false;
    }
    let HirStmtKind::Assign(_, rhs) = &body[1].kind else {
        return false;
    };
    matches!(&rhs.kind, HirExprKind::Call { callee, args, .. } if callee == "Max#" && args.len() == 2)
}

fn match_expsum_pass(body: &[HirStmt]) -> bool {
    if body.len() != 1 {
        return false;
    }
    let HirStmtKind::Assign(_, rhs) = &body[0].kind else {
        return false;
    };
    matches!(
        &rhs.kind,
        HirExprKind::Binary {
            op: BinaryOpKind::Add,
            ..
        }
    ) && format!("{rhs:?}").contains("\"Exp#\"")
}

fn match_normalize_pass(body: &[HirStmt]) -> Option<String> {
    if body.len() != 1 {
        return None;
    }
    let HirStmtKind::Assign(lhs, _) = &body[0].kind else {
        return None;
    };
    let HirExprKind::Index(buf, _) = &lhs.kind else {
        return None;
    };
    let HirExprKind::Identifier(out) = &buf.kind else {
        return None;
    };
    if !format!("{:?}", body[0]).contains("\"Exp#\"") {
        return None;
    }
    Some(out.clone())
}

/// Recognize a dot-product reduction directly (mul-add fold).
fn detect_reduction(stmts: &[HirStmt], index_var: &str) -> Option<HirReduction> {
    stmts.iter().find_map(|stmt| dot_foreach(stmt, index_var))
}

fn dot_foreach(stmt: &HirStmt, index_var: &str) -> Option<HirReduction> {
    use BinaryOpKind::{Add, Mul};
    let HirStmtKind::Foreach {
        list,
        body,
        modifiers,
        ..
    } = &stmt.kind
    else {
        return None;
    };
    let HirExprKind::Range { end, .. } = &list.kind else {
        return None;
    };
    if body.len() != 1 {
        return None;
    }
    let HirStmtKind::Assign(lhs, rhs) = &body[0].kind else {
        return None;
    };
    let HirExprKind::Identifier(acc) = &lhs.kind else {
        return None;
    };
    let HirExprKind::Binary {
        op: Add,
        left,
        right,
    } = &rhs.kind
    else {
        return None;
    };
    let is_self = |e: &HirExpr| matches!(&e.kind, HirExprKind::Identifier(n) if n == acc);
    let is_mul = |e: &HirExpr| matches!(&e.kind, HirExprKind::Binary { op: Mul, .. });
    if !((is_self(left) && is_mul(right)) || (is_mul(left) && is_self(right))) {
        return None;
    }
    let mul = if is_mul(left) { left } else { right };
    let (row, col) = dot_buffers(mul, index_var);
    Some(HirReduction {
        inner: (**end).clone(),
        kind: ReductionKind::Dot,
        row_buf: row,
        col_buf: col,
        out_buf: String::new(),
        seq: modifiers.iter().any(|m| m == "seq"),
        foreach_approx: modifiers.iter().any(|m| m == "approx"),
        foreach_exact: modifiers.iter().any(|m| m == "exact"),
    })
}

/// Name the dot's row (index references the work item) and col buffers.
fn dot_buffers(mul: &HirExpr, index_var: &str) -> (String, String) {
    let HirExprKind::Binary { left, right, .. } = &mul.kind else {
        return (String::new(), String::new());
    };
    let mut row = String::new();
    let mut col = String::new();
    for f in [left.as_ref(), right.as_ref()] {
        let HirExprKind::Index(buf, idx) = &f.kind else {
            continue;
        };
        let HirExprKind::Identifier(name) = &buf.kind else {
            continue;
        };
        if dot_index_has_iv(idx, index_var) {
            row = name.clone();
        } else {
            col = name.clone();
        }
    }
    (row, col)
}

fn dot_index_has_iv(e: &HirExpr, name: &str) -> bool {
    match &e.kind {
        HirExprKind::Identifier(n) => n == name,
        HirExprKind::Binary { left, right, .. } => {
            dot_index_has_iv(left, name) || dot_index_has_iv(right, name)
        }
        HirExprKind::Index(_, i) => dot_index_has_iv(i, name),
        _ => false,
    }
}

enum FoldClass {
    NotFold,
    Fold(HirReduction),
    NearMiss(&'static str),
}

fn expr_reads(e: &HirExpr, name: &str) -> bool {
    match &e.kind {
        HirExprKind::Identifier(n) => n == name,
        HirExprKind::Binary { left, right, .. } => {
            expr_reads(left, name) || expr_reads(right, name)
        }
        HirExprKind::Index(b, i) => expr_reads(b, name) || expr_reads(i, name),
        HirExprKind::Call { args, .. } => args.iter().any(|a| expr_reads(a, name)),
        _ => false,
    }
}

fn classify_idiom_fold(stmts: &[HirStmt]) -> FoldClass {
    for stmt in stmts {
        if let Some(class) = fold_foreach(stmt) {
            return class;
        }
    }
    FoldClass::NotFold
}

fn find_accumulator(body: &[HirStmt]) -> Option<(String, &HirExpr)> {
    body.iter().find_map(|st| match &st.kind {
        HirStmtKind::Assign(lhs, rhs) => match &lhs.kind {
            HirExprKind::Identifier(n) if expr_reads(rhs, n) => Some((n.clone(), rhs)),
            _ => None,
        },
        _ => None,
    })
}

fn parse_fold_rhs(rhs: &HirExpr) -> Option<(ReductionKind, &HirExpr, &HirExpr)> {
    use BinaryOpKind::{Add, Mul};
    match &rhs.kind {
        HirExprKind::Binary {
            op: Add,
            left,
            right,
        } => Some((ReductionKind::Sum, left, right)),
        HirExprKind::Binary {
            op: Mul,
            left,
            right,
        } => Some((ReductionKind::Product, left, right)),
        HirExprKind::Call { callee, args, .. } if args.len() == 2 => match callee.as_str() {
            "Max#" => Some((ReductionKind::Max, &args[0], &args[1])),
            "Min#" => Some((ReductionKind::Min, &args[0], &args[1])),
            _ => None,
        },
        _ => None,
    }
}

fn is_acc_ident(e: &HirExpr, acc: &str) -> bool {
    matches!(&e.kind, HirExprKind::Identifier(n) if n == acc)
}

fn fold_operand<'a>(acc: &str, lhs: &'a HirExpr, rhs: &'a HirExpr) -> Option<&'a HirExpr> {
    match (is_acc_ident(lhs, acc), is_acc_ident(rhs, acc)) {
        (true, false) => Some(rhs),
        (false, true) => Some(lhs),
        _ => None,
    }
}

fn fold_foreach(stmt: &HirStmt) -> Option<FoldClass> {
    let HirStmtKind::Foreach {
        list,
        body,
        modifiers,
        ..
    } = &stmt.kind
    else {
        return None;
    };
    let HirExprKind::Range { end, .. } = &list.kind else {
        return None;
    };
    let (acc, rhs) = find_accumulator(body)?;
    if body.len() != 1 {
        return Some(FoldClass::NearMiss(
            "the fold body must be a single statement (initialize the accumulator before the loop)",
        ));
    }
    let Some((kind, lhs_e, rhs_e)) = parse_fold_rhs(rhs) else {
        return Some(FoldClass::NearMiss(
            "only `+`, `*`, `Min#` and `Max#` folds are recognized (this operator is not associative for the lane)",
        ));
    };
    let Some(operand) = fold_operand(&acc, lhs_e, rhs_e) else {
        return None;
    };
    let HirExprKind::Index(buf, idx) = &operand.kind else {
        return Some(FoldClass::NearMiss(
            "the folded operand must be a plain buffer read, e.g. `a[i * K + k]`",
        ));
    };
    let HirExprKind::Identifier(buf_name) = &buf.kind else {
        return None;
    };
    if expr_reads(idx, &acc) {
        return Some(FoldClass::NearMiss(
            "the folded index reads the accumulator — that is a scan, not a fold",
        ));
    }
    Some(FoldClass::Fold(HirReduction {
        inner: (**end).clone(),
        kind,
        row_buf: buf_name.clone(),
        col_buf: String::new(),
        out_buf: String::new(),
        seq: modifiers.iter().any(|m| m == "seq"),
        foreach_approx: modifiers.iter().any(|m| m == "approx"),
        foreach_exact: modifiers.iter().any(|m| m == "exact"),
    }))
}

fn detect_idiom_fold(stmts: &[HirStmt]) -> Option<HirReduction> {
    match classify_idiom_fold(stmts) {
        FoldClass::Fold(r) => Some(r),
        _ => None,
    }
}

/// The full W3/W3.1 lane predicate over HIR facts.
fn fold_idiom_lane_hir(proof: &HirProof, program: &HirProgram) -> bool {
    fold_idiom_outcome_hir(proof, program).is_ok()
}

fn fold_idiom_outcome_hir(proof: &HirProof, program: &HirProgram) -> Result<(), FoldIdiomBlock> {
    let Some(red) = &proof.reduction else {
        return Err(FoldIdiomBlock::NoFold);
    };
    if !matches!(
        red.kind,
        ReductionKind::Sum | ReductionKind::Product | ReductionKind::Min | ReductionKind::Max
    ) {
        return Err(FoldIdiomBlock::NoFold);
    }
    if kernel_stmts_decompose_counter(&proof.kernel, &proof.index_var) {
        return Err(FoldIdiomBlock::CounterDecomposed);
    }
    let consts = folded_const_map(program);
    let Some(k) = lit(&red.inner, &consts) else {
        return Err(FoldIdiomBlock::LengthNotConst);
    };
    if k <= 0 || k % 32 != 0 {
        return Err(FoldIdiomBlock::LengthNotMultiple32);
    }
    if !fold_buffer_is_int(program, &red.row_buf) {
        float_fold_license(proof, red)?;
    }
    Ok(())
}

/// Precedence: `seq` > per-fold `exact` > per-fold `approx` > node `exact`
/// > node `approx` > precise default.
fn float_fold_license(proof: &HirProof, red: &HirReduction) -> Result<(), FoldIdiomBlock> {
    if red.seq {
        return Err(FoldIdiomBlock::SeqPinned);
    }
    if red.foreach_exact {
        return Err(FoldIdiomBlock::ExactPinned);
    }
    let node_licensed = proof.approx_declared && !proof.exact_declared;
    if red.foreach_approx || node_licensed {
        return Ok(());
    }
    Err(if proof.exact_declared {
        FoldIdiomBlock::ExactPinned
    } else {
        FoldIdiomBlock::FloatUnlicensed
    })
}

fn kernel_stmts_decompose_counter(stmts: &[HirStmt], index_var: &str) -> bool {
    fn expr_decomposes(e: &HirExpr, iv: &str) -> bool {
        match &e.kind {
            HirExprKind::Binary { op, left, right } => {
                let here = matches!(op, BinaryOpKind::Div | BinaryOpKind::Mod)
                    && matches!(&left.kind, HirExprKind::Identifier(n) if n == iv);
                here || expr_decomposes(left, iv) || expr_decomposes(right, iv)
            }
            HirExprKind::Call { args, .. } => args.iter().any(|a| expr_decomposes(a, iv)),
            _ => false,
        }
    }
    fn stmt_decomposes(s: &HirStmt, iv: &str) -> bool {
        match &s.kind {
            HirStmtKind::Assign(_, rhs) => expr_decomposes(rhs, iv),
            HirStmtKind::Let { expr: Some(e), .. } => expr_decomposes(e, iv),
            HirStmtKind::Foreach { list, body, .. } => {
                expr_decomposes(list, iv) || body.iter().any(|b| stmt_decomposes(b, iv))
            }
            _ => false,
        }
    }
    stmts.iter().any(|s| stmt_decomposes(s, index_var))
}

/// Fold module constants in declaration order.
fn folded_const_map(program: &HirProgram) -> HashMap<String, i64> {
    let mut map: HashMap<String, i64> = HashMap::new();
    for (name, expr) in &program.const_exprs {
        if let Some(v) = lit(expr, &map) {
            map.insert(name.clone(), v);
        }
    }
    map
}

fn lit(e: &HirExpr, consts: &HashMap<String, i64>) -> Option<i64> {
    match &e.kind {
        HirExprKind::Literal(HirLiteral::Int(d)) => Some(*d),
        HirExprKind::Identifier(name) => consts.get(name).copied(),
        HirExprKind::Binary { op, left, right } => {
            let (a, b) = (lit(left, consts)?, lit(right, consts)?);
            match op {
                BinaryOpKind::Add => Some(a.checked_add(b)?),
                BinaryOpKind::Sub => Some(a.checked_sub(b)?),
                BinaryOpKind::Mul => Some(a.checked_mul(b)?),
                BinaryOpKind::Div if b != 0 => Some(a.checked_div(b)?),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Gate-3 numeric license: the folded buffer's element is the Int fundamental.
fn fold_buffer_is_int(program: &HirProgram, buf: &str) -> bool {
    for global in &program.globals {
        if global.name != buf {
            continue;
        }
        return match &global.ty {
            Some(Type::Vector(inner, _)) => **inner == Type::int(),
            Some(ty) => ty == &Type::int(),
            None => false,
        };
    }
    false
}

/// Recognize the deferrable-normalize structure.
fn detect_deferred_normalizer(stmts: &[HirStmt], _index_var: &str) -> Option<HirDeferred> {
    let mut seen: Vec<String> = Vec::new();
    for stmt in stmts {
        if let HirStmtKind::Let {
            name,
            expr: Some(e),
            ..
        } = &stmt.kind
        {
            let zero = matches!(&e.kind, HirExprKind::Literal(HirLiteral::Int(0)))
                || matches!(&e.kind, HirExprKind::Literal(HirLiteral::Float(f)) if *f == 0.0);
            if zero && !seen.contains(name) {
                seen.push(name.clone());
            }
        }
    }
    seen.into_iter().find_map(|den| match_den(stmts, &den))
}

fn match_den(stmts: &[HirStmt], den: &str) -> Option<HirDeferred> {
    let (w, reduce_end) = single_self_add_loop(stmts, den)?;
    trailing_normalize_loop(stmts, den, w, reduce_end)
}

fn single_self_add_loop(stmts: &[HirStmt], den: &str) -> Option<(usize, HirExpr)> {
    if stmts
        .iter()
        .filter(|s| matches!(&s.kind, HirStmtKind::Let { name, .. } if name == den))
        .count()
        != 1
    {
        return None;
    }
    let writer = self_add_writer(stmts, den)?;
    let top_level_write = top_assigns(stmts)
        .iter()
        .any(|(lhs, _)| assigns_to(lhs, den));
    if top_level_write {
        return None;
    }
    Some(writer)
}

fn self_add_writer(stmts: &[HirStmt], den: &str) -> Option<(usize, HirExpr)> {
    let mut writer: Option<(usize, HirExpr)> = None;
    for (idx, stmt) in stmts.iter().enumerate() {
        let Some((end, hit)) = loop_self_add(stmt, den) else {
            continue;
        };
        if !hit {
            continue;
        }
        if writer.as_ref().is_some_and(|(i, _)| *i != idx) {
            return None;
        }
        writer = Some((idx, end));
    }
    writer
}

/// One foreach's self-add writes to `den`: `(range end, any write)`.
fn loop_self_add(stmt: &HirStmt, den: &str) -> Option<(HirExpr, bool)> {
    let HirStmtKind::Foreach { list, body, .. } = &stmt.kind else {
        return None;
    };
    let HirExprKind::Range { end, .. } = &list.kind else {
        return None;
    };
    let mut hit = false;
    for (lhs, rhs) in top_assigns(body) {
        if !assigns_to(lhs, den) {
            continue;
        }
        if !is_self_add(rhs, den) {
            return None;
        }
        hit = true;
    }
    Some(((**end).clone(), hit))
}

fn trailing_normalize_loop(
    stmts: &[HirStmt],
    den: &str,
    after: usize,
    reduce_end: HirExpr,
) -> Option<HirDeferred> {
    for stmt in stmts.iter().skip(after + 1) {
        let HirStmtKind::Foreach {
            item, list, body, ..
        } = &stmt.kind
        else {
            continue;
        };
        let HirExprKind::Range { end, .. } = &list.kind else {
            continue;
        };
        let Some((out_buf, acc_buf)) = normalize_body_buffers(body, den) else {
            continue;
        };
        return Some(HirDeferred {
            denominator: den.to_string(),
            acc_buf,
            out_buf,
            normalize_var: item.clone(),
            normalize_end: (**end).clone(),
            reduce_end,
        });
    }
    None
}

fn top_assigns(stmts: &[HirStmt]) -> Vec<(&HirExpr, &HirExpr)> {
    stmts
        .iter()
        .filter_map(|s| match &s.kind {
            HirStmtKind::Assign(lhs, rhs) => Some((lhs, rhs)),
            _ => None,
        })
        .collect()
}

fn assigns_to(lhs: &HirExpr, name: &str) -> bool {
    matches!(&lhs.kind, HirExprKind::Identifier(n) if n == name)
}

fn is_self_add(rhs: &HirExpr, name: &str) -> bool {
    let HirExprKind::Binary {
        op: BinaryOpKind::Add,
        left,
        right,
    } = &rhs.kind
    else {
        return false;
    };
    matches!(&left.kind, HirExprKind::Identifier(n) if n == name)
        || matches!(&right.kind, HirExprKind::Identifier(n) if n == name)
}

fn normalize_stmt_buffers(s: &HirStmt, den: &str) -> Option<(String, Option<String>)> {
    let HirStmtKind::Assign(lhs, rhs) = &s.kind else {
        return None;
    };
    if expr_mentions(lhs, den) {
        return None;
    }
    let HirExprKind::Index(o, _) = &lhs.kind else {
        return None;
    };
    let HirExprKind::Identifier(ob) = &o.kind else {
        return None;
    };
    let HirExprKind::Binary {
        op: BinaryOpKind::Div,
        left: num,
        right: divisor,
    } = &rhs.kind
    else {
        return None;
    };
    let HirExprKind::Identifier(dn) = &divisor.kind else {
        return None;
    };
    if dn != den || expr_mentions(num, den) {
        return None;
    }
    let acc = match &num.kind {
        HirExprKind::Index(nb, _) => match &nb.kind {
            HirExprKind::Identifier(nbn) => Some(nbn.clone()),
            _ => None,
        },
        _ => None,
    };
    Some((ob.clone(), acc))
}

fn normalize_body_buffers(body: &[HirStmt], den: &str) -> Option<(String, String)> {
    if body.is_empty() {
        return None;
    }
    let mut out_buf = String::new();
    let mut acc_buf = String::new();
    let mut all_acc_reads = true;
    for s in body {
        let (ob, acc) = normalize_stmt_buffers(s, den)?;
        if !out_buf.is_empty() && out_buf != ob {
            return None;
        }
        if out_buf.is_empty() {
            out_buf = ob;
        }
        match acc {
            Some(a) => {
                if !acc_buf.is_empty() && acc_buf != a {
                    return None;
                }
                acc_buf = a;
            }
            None => all_acc_reads = false,
        }
    }
    if !all_acc_reads {
        acc_buf.clear();
    }
    Some((out_buf, acc_buf))
}

fn expr_mentions(e: &HirExpr, name: &str) -> bool {
    match &e.kind {
        HirExprKind::Identifier(n) => n == name,
        HirExprKind::Index(a, i) => expr_mentions(a, name) || expr_mentions(i, name),
        HirExprKind::Binary { left, right, .. } => {
            expr_mentions(left, name) || expr_mentions(right, name)
        }
        HirExprKind::Unary { inner, .. } => expr_mentions(inner, name),
        HirExprKind::Call { args, .. } => args.iter().any(|a| expr_mentions(a, name)),
        HirExprKind::MethodCall { receiver, args, .. } => {
            expr_mentions(receiver, name) || args.iter().any(|a| expr_mentions(a, name))
        }
        HirExprKind::Cast { inner, .. }
        | HirExprKind::Deref(inner)
        | HirExprKind::AddrOf(inner)
        | HirExprKind::Consume(inner)
        | HirExprKind::Await(inner) => expr_mentions(inner, name),
        HirExprKind::Range { start, end, .. } => {
            expr_mentions(start, name) || expr_mentions(end, name)
        }
        HirExprKind::Slice {
            array,
            start,
            end,
            stride,
        } => {
            expr_mentions(array, name)
                || start.as_ref().is_some_and(|a| expr_mentions(a, name))
                || end.as_ref().is_some_and(|a| expr_mentions(a, name))
                || stride.as_ref().is_some_and(|a| expr_mentions(a, name))
        }
        HirExprKind::Field { receiver, .. } | HirExprKind::Reflect { receiver, .. } => {
            expr_mentions(receiver, name)
        }
        HirExprKind::Tuple(xs) | HirExprKind::List(xs) => xs.iter().any(|x| expr_mentions(x, name)),
        _ => false,
    }
}

/// Derive the 2D dispatch width from shift/mask uses of the work-item id.
fn detect_work_cols(stmts: &[HirStmt], index_var: &str) -> Option<u64> {
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
    e: &HirExpr,
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

fn shift_of_index(e: &HirExpr, index_var: &str) -> Option<u64> {
    let HirExprKind::Binary {
        op: BinaryOpKind::Shr,
        left,
        right,
    } = &e.kind
    else {
        return None;
    };
    if !matches!(&left.kind, HirExprKind::Identifier(v) if v == index_var) {
        return None;
    }
    let HirExprKind::Literal(HirLiteral::Int(k)) = &right.kind else {
        return None;
    };
    if *k < 1 || *k > 31 {
        return None;
    }
    Some(1u64 << *k)
}

fn mask_of_index(e: &HirExpr, index_var: &str) -> Option<u64> {
    let HirExprKind::Binary {
        op: BinaryOpKind::BitAnd,
        left,
        right,
    } = &e.kind
    else {
        return None;
    };
    if !matches!(&left.kind, HirExprKind::Identifier(v) if v == index_var) {
        return None;
    }
    let HirExprKind::Literal(HirLiteral::Int(m)) = &right.kind else {
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
        Some(existing) if *existing != cols => *mask_cols = Some(u64::MAX),
        _ => {}
    }
}

fn scan_stmt_work_cols(
    stmt: &HirStmt,
    index_var: &str,
    shift_cols: &mut Option<u64>,
    mask_cols: &mut Option<u64>,
) {
    match &stmt.kind {
        HirStmtKind::Assign(lhs, rhs) => {
            scan_expr_work_cols(lhs, index_var, shift_cols, mask_cols);
            scan_expr_work_cols(rhs, index_var, shift_cols, mask_cols);
        }
        HirStmtKind::Foreach { list, body, .. } => {
            scan_expr_work_cols(list, index_var, shift_cols, mask_cols);
            for s in body {
                scan_stmt_work_cols(s, index_var, shift_cols, mask_cols);
            }
        }
        _ => {}
    }
}

fn child_exprs(e: &HirExpr) -> Vec<&HirExpr> {
    match &e.kind {
        HirExprKind::Binary { left, right, .. } => vec![left.as_ref(), right.as_ref()],
        HirExprKind::Unary { inner, .. } => vec![inner.as_ref()],
        HirExprKind::Index(o, i) => vec![o.as_ref(), i.as_ref()],
        HirExprKind::Call { args, .. } => args.iter().collect(),
        HirExprKind::MethodCall { receiver, args, .. } => {
            let mut v: Vec<&HirExpr> = vec![receiver.as_ref()];
            v.extend(args.iter());
            v
        }
        _ => Vec::new(),
    }
}

/// Walk kernel statements collecting the buffer contract.
fn collect_buffers(
    stmts: &[HirStmt],
    info: &HirProgramInfo,
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
    stmt: &HirStmt,
    info: &HirProgramInfo,
    reads: &mut HashSet<String>,
    writes: &mut HashSet<String>,
    scalars: &mut HashSet<String>,
) {
    match &stmt.kind {
        HirStmtKind::Let { expr: Some(e), .. } => {
            collect_expr_buffers(e, info, reads, writes, scalars)
        }
        HirStmtKind::Assign(lhs, rhs) => {
            collect_expr_buffers(rhs, info, reads, writes, scalars);
            if let HirExprKind::Index(arr, _) = &lhs.kind {
                if let HirExprKind::Identifier(n) = &arr.kind {
                    writes.insert(n.clone());
                }
            }
        }
        HirStmtKind::Guarded(cond, body) => {
            collect_expr_buffers(cond, info, reads, writes, scalars);
            for s in body {
                collect_stmt_buffers(s, info, reads, writes, scalars);
            }
        }
        HirStmtKind::Block(body) => {
            for s in body {
                collect_stmt_buffers(s, info, reads, writes, scalars);
            }
        }
        HirStmtKind::Foreach { list, body, .. } => {
            collect_expr_buffers(list, info, reads, writes, scalars);
            for s in body {
                collect_stmt_buffers(s, info, reads, writes, scalars);
            }
        }
        _ => {}
    }
}

fn collect_expr_buffers(
    expr: &HirExpr,
    info: &HirProgramInfo,
    reads: &mut HashSet<String>,
    writes: &mut HashSet<String>,
    scalars: &mut HashSet<String>,
) {
    match &expr.kind {
        HirExprKind::Identifier(name) => {
            if info.state_fields.contains(name) || info.consts.contains_key(name) {
                scalars.insert(name.clone());
            }
        }
        HirExprKind::Index(arr, idx) => {
            collect_expr_buffers(arr, info, reads, writes, scalars);
            collect_expr_buffers(idx, info, reads, writes, scalars);
            if let HirExprKind::Identifier(n) = &arr.kind {
                reads.insert(n.clone());
            }
        }
        HirExprKind::Binary { left, right, .. } => {
            collect_expr_buffers(left, info, reads, writes, scalars);
            collect_expr_buffers(right, info, reads, writes, scalars);
        }
        HirExprKind::Call { callee, args, .. } => {
            if callee.starts_with("Atomic") && callee.ends_with("At#") {
                if let Some(HirExpr {
                    kind: HirExprKind::Identifier(n),
                    ..
                }) = args.first()
                {
                    reads.insert(n.clone());
                    writes.insert(n.clone());
                }
                for a in args.iter().skip(1) {
                    collect_expr_buffers(a, info, reads, writes, scalars);
                }
                return;
            }
            for a in args {
                collect_expr_buffers(a, info, reads, writes, scalars);
            }
        }
        HirExprKind::Unary { inner, .. } => {
            collect_expr_buffers(inner, info, reads, writes, scalars)
        }
        HirExprKind::Cast { inner, .. } => {
            collect_expr_buffers(inner, info, reads, writes, scalars)
        }
        HirExprKind::Tuple(items) | HirExprKind::List(items) => {
            for i in items {
                collect_expr_buffers(i, info, reads, writes, scalars);
            }
        }
        _ => {}
    }
}

/// Test-only HIR dump: analyze `program`, then canonicalize by name.
#[cfg(test)]
pub(crate) fn entries_for_test(
    program: &HirProgram,
    universe: &TypeUniverse,
) -> std::collections::BTreeMap<String, String> {
    entries_for_test_mode(program, universe, None)
}

#[cfg(test)]
pub(crate) fn entries_for_test_mode(
    program: &HirProgram,
    universe: &TypeUniverse,
    mode: Option<AccelMode>,
) -> std::collections::BTreeMap<String, String> {
    analyze(program, Some(universe), mode)
        .iter()
        .map(|(name, entry)| (name.clone(), crate::analysis::accel::canonical_entry(entry)))
        .collect()
}

#[cfg(test)]
mod tests {
    //! AST-vs-HIR parity for the accel shape analysis. Every case asserts
    //! exact canonical equality of the full `AccelEntry` dump (BTreeMap keyed
    //! by transaction name), then spot-checks the facts the consumers read.

    use super::*;
    use crate::ast::TopLevel;
    use std::collections::BTreeMap;

    fn parse_lower(src: &str) -> (Vec<TopLevel>, HirProgram) {
        let tokens = crate::lexer::tokenize(src).expect("tokenize");
        let mut parser = crate::parser::Parser::new(tokens, src);
        let items = parser.parse_program().expect("parse");
        let program = crate::hir::lower_program(&items);
        (items, program)
    }

    fn parity(src: &str) -> BTreeMap<String, String> {
        let (items, program) = parse_lower(src);
        let universe = TypeUniverse::new();
        let ast = crate::analysis::accel::entries_for_test(&items, &universe);
        let hir = entries_for_test(&program, &universe);
        assert_eq!(hir, ast, "HIR/AST accel drift:\n{src}");
        ast
    }

    fn analyze_src(src: &str) -> HashMap<String, AccelEntry> {
        let (_, program) = parse_lower(src);
        analyze(&program, Some(&TypeUniverse::new()), None)
    }

    #[test]
    fn scalar_kernel_matches_ast() {
        let src = r#"
const N: Int = 1024;
let x: Float[1024];
let y: Float[1024];
let i: Int = 0;
accel node scale [i < N][i == N] {
    when x[i] > 0.0 { y[i] = x[i] * 2.0; };
    i = i + 1;
    term;
};
"#;
        let dump = parity(src);
        assert_eq!(dump.len(), 1, "one candidate");
        let map = analyze_src(src);
        let e = map.get("scale").expect("scale entry");
        assert!(e.shape.eligible, "reasons: {:?}", e.shape.reasons);
        assert_eq!(e.shape.index_var, "i");
        assert_eq!(e.shape.read_buffers, vec!["x".to_string()]);
        assert_eq!(e.shape.write_buffers, vec!["y".to_string()]);
        assert_eq!(e.shape.work_cols, None);
        assert_eq!(e.shape.kernel_stmts.len(), 1, "the guarded write only");
        assert_eq!(e.shape.host_stmts.len(), 2, "counter advance + term");
    }

    #[test]
    fn matmul_tile_geometry_matches_ast() {
        let src = r#"
const N: Int = 8;
const K: Int = 64;
let a: Float[4096];
let b: Float[4096];
let y: Float[64];
let i: Int = 0;
accel node mm [i < 64][i == 64] {
    let acc: Float = 0.0;
    foreach k in 0..K {
        acc = acc + a[(i >> 3) * K + k] * b[k * N + (i & 7)];
    };
    y[i] = acc;
    i = i + 1;
    term;
};
"#;
        let dump = parity(src);
        assert_eq!(dump.len(), 1);
        let map = analyze_src(src);
        let e = map.get("mm").expect("mm entry");
        assert!(e.shape.eligible, "reasons: {:?}", e.shape.reasons);
        assert_eq!(e.shape.work_cols, Some(8), "i >> 3 names an 8-wide row");
        let red = e.shape.reduction.as_ref().expect("dot reduction");
        assert_eq!(red.kind, ReductionKind::Dot);
        assert_eq!(red.row_buf, "b", "both multiply indices read the counter");
        assert_eq!(red.out_buf, "y");
    }

    #[test]
    fn write_buffers_matches_ast() {
        let src = r#"
const N: Int = 1024;
let x: Float[1024];
let y: Float[1024];
let z: Float[1024];
let i: Int = 0;
accel node dual [i < N][i == N] {
    y[i] = x[i];
    z[i] = y[i] + 1.0;
    i = i + 1;
    term;
};
"#;
        parity(src);
        let map = analyze_src(src);
        let e = map.get("dual").expect("dual entry");
        assert_eq!(e.shape.read_buffers, vec!["x".to_string(), "y".to_string()]);
        assert_eq!(
            e.shape.write_buffers,
            vec!["y".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn dynamic_bound_is_probe_and_matches_ast() {
        let src = r#"
let n: Int = 4096;
let x: Float[4096];
let y: Float[4096];
let i: Int = 0;
accel node dyn [i < n][i == n] {
    y[i] = x[i];
    i = i + 1;
    term;
};
"#;
        parity(src);
        let map = analyze_src(src);
        let e = map.get("dyn").expect("dyn entry");
        assert!(e.shape.eligible, "reasons: {:?}", e.shape.reasons);
        assert_eq!(e.decision, AccelDecision::Probe, "runtime counter ⇒ probe");
    }

    #[test]
    fn reductions_match_ast() {
        let src = r#"
const K: Int = 1024;
const R: Int = 4;
let a: Float[4096];
let x: Float[1024];
let y: Float[4];
let ai: Int[4096];
let yi: Int[4];
let i: Int = 0;
accel node dot [i < R][i == R] {
    let acc: Float = 0.0;
    foreach k in 0..K { acc = acc + a[i * K + k] * x[k]; };
    y[i] = acc;
    i = i + 1;
    term;
};
approx accel node isum [i < R][i == R] {
    let acc: Int = 0;
    foreach k in 0..K { acc = acc + ai[i * K + k]; };
    yi[i] = acc;
    i = i + 1;
    term;
};
"#;
        let dump = parity(src);
        assert_eq!(dump.len(), 2, "two kernels");
        let map = analyze_src(src);
        let dot = map.get("dot").expect("dot entry");
        let red = dot.shape.reduction.as_ref().expect("dot reduction");
        assert_eq!(red.kind, ReductionKind::Dot);
        assert_eq!(red.row_buf, "a");
        assert_eq!(red.col_buf, "x");
        assert_eq!(red.out_buf, "y");
        let sum = map.get("isum").expect("isum entry");
        let fold = sum.shape.reduction.as_ref().expect("fold reduction");
        assert_eq!(fold.kind, ReductionKind::Sum);
        assert_eq!(fold.row_buf, "ai");
        assert!(sum.shape.idiom_lane, "approx + Int fold licenses the lane");
    }

    #[test]
    fn ineligible_shapes_match_ast() {
        let src = r#"
const N: Int = 1024;
let x: Float[1024];
let y: Float[1024];
let ss: String[8];
let i: Int = 0;
accel node no_bound [true][true] {
    y[i] = x[i];
    i = i + 1;
    term;
};
accel node const_slot [i < N][i == N] {
    y[0] = x[i];
    i = i + 1;
    term;
};
accel node not_flat [i < N][i == N] {
    ss[i] = "x";
    i = i + 1;
    term;
};
"#;
        let dump = parity(src);
        assert_eq!(dump.len(), 3);
        let map = analyze_src(src);
        for name in ["no_bound", "const_slot", "not_flat"] {
            assert!(!map[name].shape.eligible, "{name} must be ineligible");
        }
        assert!(map["no_bound"]
            .shape
            .reasons
            .iter()
            .any(|r| r.contains("[i < N]")));
        assert!(map["const_slot"]
            .shape
            .reasons
            .iter()
            .any(|r| r.contains("cross-work-item")));
        assert!(map["not_flat"]
            .shape
            .reasons
            .iter()
            .any(|r| r.contains("not a flat scalar")));
    }

    #[test]
    fn multiple_kernels_match_ast() {
        let src = r#"
const N: Int = 1024;
let x: Float[1024];
let y: Float[1024];
let z: Float[1024];
let i: Int = 0;
accel node first [i < N][i == N] {
    y[i] = x[i];
    i = i + 1;
    term;
};
accel node second [i < N][i == N] {
    z[i] = y[i];
    i = i + 1;
    term;
};
"#;
        let dump = parity(src);
        assert_eq!(dump.len(), 2, "both kernels keyed by name");
        let map = analyze_src(src);
        assert_eq!(map["first"].shape.write_buffers, vec!["y".to_string()]);
        assert_eq!(map["second"].shape.write_buffers, vec!["z".to_string()]);
    }

    #[test]
    fn element_width_and_stride_match_ast() {
        let src = r#"
const N: Int = 512;
let xi: Int[1024];
let yi: Int[1024];
let xf: Float[1024];
let yf: Float[1024];
let i: Int = 0;
accel node strided_int [i < N][i == N] {
    yi[i * 2] = xi[i * 2] + 1;
    i = i + 1;
    term;
};
accel node strided_float [i < N][i == N] {
    yf[i * 4] = xf[i * 4] * 0.5;
    i = i + 1;
    term;
};
"#;
        let dump = parity(src);
        assert_eq!(dump.len(), 2);
        let map = analyze_src(src);
        assert!(map["strided_int"].shape.eligible);
        assert!(map["strided_float"].shape.eligible);
    }
}
