//! Tiled GEMM synthesis (plan 2026-09-01-m2-gemm M2.1).
//!
//! The .abv author writes the NAIVE matmul — one output element per item,
//! a plain foreach dot product. When the accel analysis's shape matches the
//! canonical flattened-2D form, this module synthesizes what a hand-tuner
//! would: a workgroup per TILE_M×TILE_N block of the output, A/B k-panels
//! staged through WORKGROUP SHARED MEMORY, register-blocked 4×4 FMAs per
//! invocation. The metadata that makes this derivable rather than
//! handwritten: static shapes (M/N/K consts), the proven reduction shape,
//! and the single-SSBO layout (no aliasing to disprove).
//!
//! Grid contract (this driver flattens Y — see the handoff trap list): the
//! grid is X-only; `gl_WorkGroupID.x` linearizes (tile_m, tile_n) as
//! `tile_m * tiles_x + tile_n`. LocalSize is 16×16×1 = 256 invocations,
//! each computing a 4×4 register tile → a 64×64 output tile per workgroup.
//!
//! Everything falls back: a body that does not match the canonical form
//! (or shapes not divisible by the tile) lowers to the flat naive kernel,
//! which is correct (M2.0) and merely slow. Tiling is a strategy the
//! compiler picks — never a keyword the user writes.

use super::kernel::{begin_structured_loop, end_structured_loop, CoopLoopSig};
use super::lower::fold_consts;
use crate::ast::{Expr, Statement, TopLevel, Type};
use rspirv::dr::{Instruction, Operand};
use rspirv::spirv::{self, StorageClass, Word};
use std::collections::HashMap;

/// Tile configuration (v1): 64×64 output tile, 16×16 invocations, 4×4
/// register tile per invocation, 64-deep k-panels (16 KB shared per panel,
/// 32 KB total — fits every Vulkan-class GPU's minimum workgroup shared mem).
pub(crate) const TILE: u64 = 64;
pub(crate) const THREADS: u64 = 16;
pub(crate) const REG: u64 = 4;

/// The recognized naive-GEMM shape, with every literal the tiled synthesis
/// needs. Field names stay opaque (no Briev-type knowledge).
#[derive(Clone)]
pub(crate) struct GemmPlan {
    pub m: i64,
    pub n: i64,
    pub k: i64,
    /// State field names (a: row-major M×K, b: row-major K×N, y: M×N).
    pub a_field: String,
    pub b_field: String,
    pub y_field: String,
}

/// Resolve a literal expression: Decimal, a const identifier, or a pure
/// arithmetic combination of those (`M * N` bounds, `0..K` ends). The
/// module-level consts are the .abv metadata that makes the tiled shape
/// statically checkable at all.
fn lit(e: &Expr, consts: &HashMap<String, i64>) -> Option<i64> {
    match e {
        Expr::Decimal(d) => Some(*d),
        Expr::Identifier(name) => consts.get(name).copied(),
        Expr::BinaryOp(kind, l, r) => {
            let (a, b) = (lit(l, consts)?, lit(r, consts)?);
            match kind {
                crate::ast::BinaryOpKind::Add => Some(a.checked_add(b)?),
                crate::ast::BinaryOpKind::Sub => Some(a.checked_sub(b)?),
                crate::ast::BinaryOpKind::Mul => Some(a.checked_mul(b)?),
                crate::ast::BinaryOpKind::Div if b != 0 => Some(a.checked_div(b)?),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `mul(ident, literal)` — either operand order.
fn linear_term_of(e: &Expr, name: &str) -> Option<i64> {
    match e {
        Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) => {
            if let Expr::Identifier(n) = l.as_ref() {
                if n == name {
                    if let Some(v) = lit(r, &HashMap::new()) {
                        return Some(v);
                    }
                }
            }
            if let Expr::Identifier(n) = r.as_ref() {
                if n == name {
                    if let Some(v) = lit(l, &HashMap::new()) {
                        return Some(v);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// `a_idx` must be `m*K + k` or `k + m*K` (row-major, k coefficient 1).
/// Returns K. Callers fold the expr with the const map first so `K` (an
/// identifier const in the .abv) arrives as a Decimal.
fn match_a_index(e: &Expr, m: &str, k: &str) -> Option<i64> {
    match e {
        Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, r) => {
            if let Some(v) = linear_term_of(l, m) {
                if let Expr::Identifier(k1) = r.as_ref() {
                    if k1 == k {
                        return Some(v);
                    }
                }
            }
            if let Some(v) = linear_term_of(r, m) {
                if let Expr::Identifier(k1) = l.as_ref() {
                    if k1 == k {
                        return Some(v);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// `b_idx` must be `k*N + n` or `n + k*N`. Returns N.
fn match_b_index(e: &Expr, k: &str, n: &str) -> Option<i64> {
    match e {
        Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, r) => {
            if let Some(v) = linear_term_of(l, k) {
                if let Expr::Identifier(n1) = r.as_ref() {
                    if n1 == n {
                        return Some(v);
                    }
                }
            }
            if let Some(v) = linear_term_of(r, k) {
                if let Expr::Identifier(n1) = l.as_ref() {
                    if n1 == n {
                        return Some(v);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

impl GemmPlan {
    /// Tier gate: the tensor path needs enough workgroups to be worth it
    /// AND to be reliable on this driver. Measured 2026-09-02 (sentinel
    /// probe, RTX 3060, driver with the known Y-dispatch defect): dispatches
    /// of 2-7 workgroups nondeterministically LOSE workgroups' stores (rows
    /// hold the host sentinel; the missing band varies with launch count);
    /// 1 workgroup and >= 8 are reliable. Small GEMMs fall to the general
    /// naive tier, which is faster there anyway (64^3: 0.019ms naive vs
    /// 0.022ms tensor). Kernel and runner both call this — one tier
    /// decision. Undo: delete the workgroup-count check.
    pub(crate) fn tensor_tier_eligible(&self) -> bool {
        let r = Self::coopmat_tile_rows(self.m);
        let workgroups = (self.m / (16 * r as i64)) * (self.n / 64);
        workgroups >= 8
    }

    /// The effective coopmat tile-rows for a plan: the configured R capped
    /// at 8 and clamped down the power-of-two ladder until 16R divides M —
    /// every strip row must stay inside the output (the plan gate
    /// guarantees 64 | M, so the ladder always bottoms out at a legal R).
    /// KERNEL AND RUNNER BOTH CALL THIS — they can never disagree on the
    /// dispatch geometry. Cap 8: R=16 emitted correct-looking SPIR-V
    /// (spirv-val clean) but miscomputed on the RTX 3060 while ALSO being
    /// slower (min 24.4ms vs R=8's 16.4) — 256 accumulator regs/lane is
    /// past the driver's comfortable fragment allocation (suspected
    /// spill/reload of coopmat fragments); VERDICT: rejected, do not use
    /// R=16. 2026-09-02 (plan 2026-09-02-tensor-tier-run, B-reuse rung).
    /// 2026-09-02 (plan 2026-09-02-cuda-race B3): the FP16-accumulate tier
    /// — accumulator fragments coopmat<f16> (Ampere double-pumped mma, 2×
    /// the FP32-acc ceiling). SEPARATE numerics tier (user-approved): gate
    /// rel ≤ 1e-2 vs the f32-acc tier, which remains the default and the
    /// correctness reference. Off = the default.
    pub(crate) fn coopmat_f16acc() -> bool {
        crate::config_tuning::ir_lowering().spirv_coopmat_f16acc
    }

    /// 2026-09-02 (plan 2026-09-02-cuda-race B2): subgroups per tensor
    /// workgroup (1 = the historical one-warp form).
    pub(crate) fn coopmat_subgroups() -> u32 {
        crate::config_tuning::ir_lowering().spirv_coopmat_subgroups
    }

    /// 2026-09-04 (perf-blocks plan): smem double-buffer staging for the
    /// tensor tier. 0 = direct SSBO coopmat loads (the pre-smem form).
    /// A/B lever for the staging experiment — the dbvl declared this knob
    /// since the smem commit but nothing read it (dead-knob fix).
    pub(crate) fn coopmat_smem() -> bool {
        crate::config_tuning::ir_lowering().spirv_coopmat_smem
    }

    /// 2026-09-04 (beyond-coopmat Stage 1, D3): the smem fill pairs the
    /// per-element half loads/stores into u32 loads/stores — adjacent
    /// flats (even/odd col) read adjacent DRAM halves (one aligned u32)
    /// and land in adjacent smem slots (one aligned u32). Halves the
    /// fill's instruction count; the fill is ~8× the mma instruction
    /// volume per panel.
    pub(crate) fn coopmat_fill_pairs() -> bool {
        crate::config_tuning::ir_lowering().spirv_coopmat_fill_pairs
    }

    /// D3b (beyond-coopmat Stage 1): the paired fill iterates f16×4
    /// quads — one v4f16 load/store per 4 halves (half the pairs-mode
    /// instruction count again). Requires the pairs-view machinery
    /// (vNf16 member retyping); dead unless fill_pairs is on.
    pub(crate) fn coopmat_fill_quad() -> bool {
        crate::config_tuning::ir_lowering().spirv_coopmat_fill_quad
    }

    /// D3b quad view actually active: knob on, pairs mode on (same
    /// vNf16 member-view machinery), and the address math divides
    /// exactly — A row stride K, B row stride N, and the M ladder all
    /// ÷4. Single definition: kernel.rs (member widths) and gemm.rs
    /// (fill + fragment loads) must agree.
    pub(crate) fn coopmat_fill_quad_active(plan: &GemmPlan) -> bool {
        Self::coopmat_smem()
            && Self::coopmat_fill_pairs()
            && Self::coopmat_fill_quad()
            && plan.m % 4 == 0
            && plan.k % 4 == 0
            && plan.n % 4 == 0
    }

    /// D2 (beyond-coopmat Stage 1): the loop refill's DRAM loads
    /// prefetch into registers during the mma phase — the portable
    /// emulation of cp.async (SPIR-V has no async global→workgroup
    /// copy). Requires the wide fill (the split fill halves); the
    /// scalar legacy fill keeps the fused post-barrier refill.
    pub(crate) fn coopmat_fill_prefetch() -> bool {
        crate::config_tuning::ir_lowering().spirv_coopmat_fill_prefetch
    }

    /// D5 (beyond-coopmat Stage 1): rotate each workgroup's K-panel
    /// start (wgid % 8 buckets × groups/8) so co-resident workgroups'
    /// fill/mma barrier phases desynchronize — the anti-phase-locking
    /// lever. Needs groups % 8 == 0 so the buckets divide evenly.
    pub(crate) fn coopmat_stagger() -> bool {
        crate::config_tuning::ir_lowering().spirv_coopmat_stagger
    }

    /// D1 (beyond-coopmat Stage 1): panels per double-buffer stage — 2
    /// halves the barrier count per panel. Falls back to 1 when (K/16)
    /// is odd: the tail pair would double-count a clamped duplicate panel.
    pub(crate) fn coopmat_panels_per_stage(plan_k: i64) -> u32 {
        let knob = crate::config_tuning::ir_lowering()
            .spirv_coopmat_panels_per_stage
            .max(1);
        if knob >= 2 && (plan_k / 16) % 2 == 0 {
            2
        } else {
            1
        }
    }

    /// 2026-09-07 (single-buffer rung): smem stages per buffer. 1 = the
    /// single-buffer form (half the smem → 2× resident WGs/SM; the fill is
    /// barrier-serialized with the mma either way, so the spare stage buys
    /// no overlap at any stage count). Clamped to 1..2.
    pub(crate) fn coopmat_stages() -> u32 {
        crate::config_tuning::ir_lowering()
            .spirv_coopmat_stages
            .max(1)
            .min(2)
    }

    pub(crate) fn coopmat_tile_rows(plan_m: i64) -> u32 {
        let mut r = crate::config_tuning::ir_lowering()
            .spirv_coopmat_tile_rows
            .max(1)
            .min(8);
        while r > 1 && plan_m % (16 * r as i64) != 0 {
            r /= 2;
        }
        r
    }

    /// Recognize the canonical naive-GEMM body in the analyzed shape.
    /// Anything that does not match exactly returns None — the flat naive
    /// kernel (correct since M2.0) remains the fallback.
    pub(crate) fn match_stmts(
        shape: &crate::analysis::accel::KernelShape,
        items: &[TopLevel],
    ) -> Option<GemmPlan> {
        let consts = module_const_map(items);
        let iv = shape.index_var.clone();

        let bound_v = lit(&fold_consts(shape.count_expr.as_ref()?, &consts), &consts)?;
        let lets = collect_let_table(&shape.kernel_stmts, &consts);
        let (m_name, n_name, n) = match_decomposition(&lets, &consts, &iv)?;
        let m = bound_v.checked_div(n)?;

        let (k_item, k, fbody) = match_foreach(&shape.kernel_stmts, &consts)?;
        let decomp = Decomp {
            m_name: &m_name,
            n_name: &n_name,
            k_item: &k_item,
            k,
            n,
        };
        let (acc, a_field, b_field) = match_reduction(&fbody, &decomp, &consts)?;
        let y_field = match_y_store(&shape.kernel_stmts, &iv, &acc)?;

        if m <= 0 || n <= 0 || k <= 0 {
            return None;
        }
        if m % TILE as i64 != 0 || n % TILE as i64 != 0 || k % TILE as i64 != 0 {
            return None;
        }
        Some(GemmPlan {
            m,
            n,
            k,
            a_field,
            b_field,
            y_field,
        })
    }
}

/// Module-level `const` literals — the .abv metadata that makes the tiled
/// shape statically checkable at all.
fn module_const_map(items: &[TopLevel]) -> HashMap<String, i64> {
    let mut consts = HashMap::new();
    for item in items {
        if let TopLevel::Constant(c) = item {
            if let Expr::Decimal(d) = &c.expr {
                consts.insert(c.name.clone(), *d);
            }
        }
    }
    consts
}

/// The node's let table (name → const-folded initializer).
fn collect_let_table(stmts: &[Statement], consts: &HashMap<String, i64>) -> HashMap<String, Expr> {
    let mut lets = HashMap::new();
    for stmt in stmts {
        if let Statement::Let { name, expr: Some(e), .. } = stmt {
            lets.insert(name.clone(), fold_consts(e, consts));
        }
    }
    lets
}

/// The flattened-2D decomposition: m = i / DN, n = i % DN (same divisor).
fn match_decomposition(
    lets: &HashMap<String, Expr>,
    consts: &HashMap<String, i64>,
    iv: &str,
) -> Option<(String, String, i64)> {
    let mut m_name: Option<String> = None;
    let mut n_name: Option<String> = None;
    let mut div: Option<i64> = None;
    for (name, e) in lets {
        if let Expr::BinaryOp(kind, l, r) = e {
            if !matches!(l.as_ref(), Expr::Identifier(x) if x == iv) {
                continue;
            }
            let d = lit(r, consts)?;
            match kind {
                crate::ast::BinaryOpKind::Div => {
                    m_name = Some(name.clone());
                    div = Some(d);
                }
                crate::ast::BinaryOpKind::Mod => {
                    n_name = Some(name.clone());
                }
                _ => {}
            }
        }
    }
    let n = div?;
    Some((m_name?, n_name?, n))
}

/// The reduction foreach: item name, literal trip count K, body clone.
fn match_foreach(
    stmts: &[Statement],
    consts: &HashMap<String, i64>,
) -> Option<(String, i64, Vec<Statement>)> {
    for stmt in stmts {
        if let Statement::Foreach { item, list, body } = stmt {
            if let Expr::Range { end, .. } = list.as_ref() {
                let end = fold_consts(end, consts);
                if let Some(k) = lit(&end, consts) {
                    return Some((item.clone(), k, body.clone()));
                }
            }
        }
    }
    None
}

/// The decomposition names + literal strides the reduction match needs.
struct Decomp<'a> {
    m_name: &'a str,
    n_name: &'a str,
    k_item: &'a str,
    k: i64,
    n: i64,
}

/// The reduction statement: acc = acc + A[m*K + k] * B[k*N + n].
fn match_reduction(
    fbody: &[Statement],
    d: &Decomp,
    consts: &HashMap<String, i64>,
) -> Option<(String, String, String)> {
    if fbody.len() != 1 {
        return None;
    }
    let Statement::Assign(lhs, rhs) = &fbody[0] else {
        return None;
    };
    let Expr::Identifier(acc) = lhs else {
        return None;
    };
    let Expr::BinaryOp(crate::ast::BinaryOpKind::Add, a, b) = rhs else {
        return None;
    };
    if !matches!(a.as_ref(), Expr::Identifier(x) if x == acc) {
        return None;
    }
    let Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) = b.as_ref() else {
        return None;
    };
    let a_idx_f = fold_consts(match_index_of(l.as_ref())?, consts);
    let b_idx_f = fold_consts(match_index_of(r.as_ref())?, consts);
    let a_field = match_field_of(l.as_ref())?;
    let b_field = match_field_of(r.as_ref())?;
    let a_row_stride = match_a_index(&a_idx_f, d.m_name, d.k_item)?;
    let b_col_stride = match_b_index(&b_idx_f, d.k_item, d.n_name)?;
    if a_row_stride != d.k || b_col_stride != d.n {
        return None;
    }
    Some((acc.clone(), a_field, b_field))
}

/// The store: y[i] = acc (LHS index is the BARE counter).
fn match_y_store(stmts: &[Statement], iv: &str, acc: &str) -> Option<String> {
    for stmt in stmts {
        let Statement::Assign(lhs, Expr::Identifier(v)) = stmt else {
            continue;
        };
        if v != acc {
            continue;
        }
        let Expr::Index(of, idx) = lhs else {
            continue;
        };
        if !matches!(idx.as_ref(), Expr::Identifier(x) if x == iv) {
            continue;
        }
        if let Expr::Identifier(f) = of.as_ref() {
            return Some(f.clone());
        }
    }
    None
}

fn match_index_of(e: &Expr) -> Option<&Expr> {
    match e {
        Expr::Index(_, idx) => Some(idx),
        _ => None,
    }
}

fn match_field_of(e: &Expr) -> Option<String> {
    match e {
        Expr::Index(of, _) => match of.as_ref() {
            Expr::Identifier(f) => Some(f.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Ids the caller (emit_kernel) wires up before calling `emit_tiled`.
pub(crate) struct TiledCtx {
    pub ssbo: Word,
    pub wgid: Word,
    pub lid: Word,
    /// Workgroup-shared float[TILE*TILE] arrays, declared in the entry block.
    pub shared_a: Word,
    pub shared_b: Word,
    /// The float element type id.
    pub f32_ty: Word,
    /// SSBO member positions (state fields are name-sorted).
    pub a_member: u32,
    pub b_member: u32,
    pub y_member: u32,
    pub exit_bb: Word,
}

/// Scalar access into a vec4-typed SSBO member: `[member][idx>>2][idx&3]`.
fn ssbo_scalar_ptr(
    builder: &mut super::SpirvBuilder,
    ctx: &TiledCtx,
    member: u32,
    idx: Word,
    int_ty: Word,
) -> Word {
    let f32_ty = ctx.f32_ty;
    let ptr = builder.ptr_class(StorageClass::StorageBuffer, f32_ty);
    let member_c = builder.u32_const(member);
    let two = builder.builder.constant_bit64(int_ty, 2);
    let three = builder.builder.constant_bit64(int_ty, 3);
    let q = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::ShiftRightArithmetic,
        Some(int_ty),
        Some(q),
        vec![Operand::IdRef(idx), Operand::IdRef(two)],
    ));
    let r = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::BitwiseAnd,
        Some(int_ty),
        Some(r),
        vec![Operand::IdRef(idx), Operand::IdRef(three)],
    ));
    let p = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::AccessChain,
        Some(ptr),
        Some(p),
        vec![
            Operand::IdRef(ctx.ssbo),
            Operand::IdRef(member_c),
            Operand::IdRef(q),
            Operand::IdRef(r),
        ],
    ));
    p
}

/// i64 arithmetic over the builder.
fn i64_binop(builder: &mut super::SpirvBuilder, op: spirv::Op, int_ty: Word, a: Word, b: Word) -> Word {
    let id = builder.gen_id();
    builder.emit(Instruction::new(
        op,
        Some(int_ty),
        Some(id),
        vec![Operand::IdRef(a), Operand::IdRef(b)],
    ));
    id
}

fn i64_const(builder: &mut super::SpirvBuilder, int_ty: Word, v: u64) -> Word {
    builder.builder.constant_bit64(int_ty, v)
}

/// 2026-09-01: shared-memory addressing uses u32 — real shaders index
/// workgroup arrays with 32-bit ids, and the i64 variant is both slower
/// and (observed on device) unreliable through this driver.
pub(crate) fn u32_binop(builder: &mut super::SpirvBuilder, op: spirv::Op, a: Word, b: Word) -> Word {
    let ty = builder.u32_type();
    let id = builder.gen_id();
    builder.emit(Instruction::new(
        op,
        Some(ty),
        Some(id),
        vec![Operand::IdRef(a), Operand::IdRef(b)],
    ));
    id
}

pub(crate) fn u32_const(builder: &mut super::SpirvBuilder, v: u32) -> Word {
    builder.u32_const(v)
}

/// Emit ShiftRightLogical for power-of-two division: val / 2^shift → val >> shift.
pub(crate) fn u32_shr(builder: &mut super::SpirvBuilder, val: Word, shift: u32) -> Word {
    let c = u32_const(builder, shift);
    let ty = builder.u32_type();
    let id = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::ShiftRightLogical,
        Some(ty),
        Some(id),
        vec![Operand::IdRef(val), Operand::IdRef(c)],
    ));
    id
}

/// Emit BitwiseAnd for power-of-two modulo: val % 2^n → val & (2^n - 1).
/// The mask is a Word (a u32_const result id), NOT a literal value: rspirv's
/// `type Word = u32` alias makes an id accepted where a value is expected,
/// and the id number then silently becomes the AND mask (the 2026-09-11
/// coopmat fill regression — every fill b_flat_within ANDed with a gen_id).
pub(crate) fn u32_and(builder: &mut super::SpirvBuilder, val: Word, mask: Word) -> Word {
    let ty = builder.u32_type();
    let id = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::BitwiseAnd,
        Some(ty),
        Some(id),
        vec![Operand::IdRef(val), Operand::IdRef(mask)],
    ));
    id
}

fn widen_u2i(builder: &mut super::SpirvBuilder, v: Word, int_ty: Word) -> Word {
    let ulong = builder.builder.type_int(64, 0);
    let wide = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::UConvert,
        Some(ulong),
        Some(wide),
        vec![Operand::IdRef(v)],
    ));
    let signed = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::Bitcast,
        Some(int_ty),
        Some(signed),
        vec![Operand::IdRef(wide)],
    ));
    signed
}

/// Component `c` of a uvec3 Input builtin, as raw u32.
fn builtin_comp_u(builder: &mut super::SpirvBuilder, builtin: Word, c: u32) -> Word {
    let u32_ty = builder.u32_type();
    let ptr = builder.ptr_class(StorageClass::Input, u32_ty);
    let ci = builder.u32_const(c);
    let p = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::AccessChain,
        Some(ptr),
        Some(p),
        vec![Operand::IdRef(builtin), Operand::IdRef(ci)],
    ));
    let v = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::Load,
        Some(u32_ty),
        Some(v),
        vec![Operand::IdRef(p)],
    ));
    v
}

/// Component `c` of a uvec3 Input builtin, widened u32 → i64.
fn builtin_comp(
    builder: &mut super::SpirvBuilder,
    builtin: Word,
    c: u32,
    int_ty: Word,
) -> Word {
    let u32_ty = builder.u32_type();
    let ptr = builder.ptr_class(StorageClass::Input, u32_ty);
    let ci = builder.u32_const(c);
    let p = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::AccessChain,
        Some(ptr),
        Some(p),
        vec![Operand::IdRef(builtin), Operand::IdRef(ci)],
    ));
    let v = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::Load,
        Some(u32_ty),
        Some(v),
        vec![Operand::IdRef(p)],
    ));
    let ulong = builder.builder.type_int(64, 0);
    let wide = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::UConvert,
        Some(ulong),
        Some(wide),
        vec![Operand::IdRef(v)],
    ));
    let signed = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::Bitcast,
        Some(int_ty),
        Some(signed),
        vec![Operand::IdRef(wide)],
    ));
    signed
}

fn shared_ptr(builder: &mut super::SpirvBuilder, shared: Word, idx: Word, f32_ty: Word) -> Word {
    let ptr = builder.ptr_class(StorageClass::Workgroup, f32_ty);
    let id = builder.gen_id();
    builder.emit(Instruction::new(
        spirv::Op::AccessChain,
        Some(ptr),
        Some(id),
        vec![Operand::IdRef(shared), Operand::IdRef(idx)],
    ));
    id
}

/// Emit the tiled body. The caller has declared the shared arrays in the
/// entry block and positions the builder at the end of it; this function
/// emits the whole kernel body and branches to `ctx.exit_bb`.
pub(crate) fn emit_tiled(
    builder: &mut super::SpirvBuilder,
    plan: &GemmPlan,
    ctx: &TiledCtx,
) -> Result<(), String> {
    let int_ty = builder.lower_type(&crate::ast::Type::int())?;
    let bool_ty = builder.lower_type(&crate::ast::Type::Bits(1))?;
    let f32_ty = ctx.f32_ty;

    let tiles_x = (plan.n / (TILE as i64)) as u64;

    let wgid_x = builtin_comp(builder, ctx.wgid, 0, int_ty);
    let lx = builtin_comp(builder, ctx.lid, 0, int_ty);
    let ly = builtin_comp(builder, ctx.lid, 1, int_ty);

    // tile_n = wgid_x % tiles_x, tile_m = wgid_x / tiles_x.
    let tiles_x_c = i64_const(builder, int_ty, tiles_x);
    let tile_n = i64_binop(builder, spirv::Op::SRem, int_ty, wgid_x, tiles_x_c);
    let tile_m = i64_binop(builder, spirv::Op::SDiv, int_ty, wgid_x, tiles_x_c);

    // tid = ly*16 + lx in U32 (shared addressing); the SSBO-side index math
    // stays i64 (widened from the u32 pieces).
    let lx_u = builtin_comp_u(builder, ctx.lid, 0);
    let ly_u = builtin_comp_u(builder, ctx.lid, 1);
    let tid = {
        let ts = u32_const(builder, THREADS as u32);
        let t = u32_binop(builder, spirv::Op::IMul, ly_u, ts);
        u32_binop(builder, spirv::Op::IAdd, t, lx_u)
    };
    let row0 = {
        let ts = i64_const(builder, int_ty, TILE);
        let a = i64_binop(builder, spirv::Op::IMul, int_ty, tile_m, ts);
        let rg = i64_const(builder, int_ty, REG);
        let ly1 = builtin_comp(builder, ctx.lid, 1, int_ty);
        let b = i64_binop(builder, spirv::Op::IMul, int_ty, ly1, rg);
        i64_binop(builder, spirv::Op::IAdd, int_ty, a, b)
    };
    let col0 = {
        let ts = i64_const(builder, int_ty, TILE);
        let a = i64_binop(builder, spirv::Op::IMul, int_ty, tile_n, ts);
        let rg = i64_const(builder, int_ty, REG);
        let b = i64_binop(builder, spirv::Op::IMul, int_ty, lx, rg);
        i64_binop(builder, spirv::Op::IAdd, int_ty, a, b)
    };

    // 16 accumulators as loop-carried phis (zero on entry, the kk-chain
    // result on the back edge).
    let zero_f = builder.float_const(32, 0.0);
    let acc_backedges: Vec<Word> = (0..REG * REG).map(|_| builder.gen_id()).collect();
    let acc_phis: Vec<(Word, Word, Word)> = acc_backedges
        .iter()
        .map(|&be| (f32_ty, zero_f, be))
        .collect();

    let sig = CoopLoopSig {
        int_ty,
        bool_ty,
        groups: (plan.k / (TILE as i64)) as i64,
    };
    let (bbs, acc_phi_ids, cond_next, _cond0, kt_phi, kt_backedge) =
        begin_structured_loop(builder, &sig, &acc_phis)?;
    // The live accumulator SSA values start as the header phi ids; after
    // the loop the PHI ids are the final values (the backedge ids are
    // body-block definitions — they do not dominate the merge block).
    let mut acc_live: Vec<Word> = acc_phi_ids.clone();

    emit_panel_loads(builder, plan, ctx, tid, kt_phi, tile_m, tile_n, int_ty, f32_ty)?;

    // Panel-visibility barrier: SequentiallyConsistent | UniformMemory (0x108),
    // scope Workgroup for both execution and memory.
    let scope = builder.u32_const(2);
    let sem = builder.u32_const(0x108);
    builder.emit(Instruction::new(
        spirv::Op::ControlBarrier,
        None,
        None,
        vec![
            Operand::IdRef(scope),
            Operand::IdRef(scope),
            Operand::IdRef(sem),
        ],
    ));
    // ── Register-tiled inner product over the panel: kk = 0..64 unrolled ──
    // a_r[u] = As[(ly*4+u)*64 + kk]; b_r[v] = Bs[kk*64 + lx*4 + v];
    // acc[u][v] = Fma(a_r[u], b_r[v], acc[u][v]).
    // a_r[u] reads shared row (ly*REG + u) — the invocation's 4-row strip.
    let a_row_base: Vec<Word> = (0..REG)
        .map(|u| {
            let rg = u32_const(builder, REG as u32);
            let ly4 = u32_binop(builder, spirv::Op::IMul, ly_u, rg);
            let uu = u32_const(builder, u as u32);
            let r = u32_binop(builder, spirv::Op::IAdd, ly4, uu);
            let ts = u32_const(builder, TILE as u32);
            u32_binop(builder, spirv::Op::IMul, r, ts)
        })
        .collect();
    // Shared B rows are TILE-local (64 wide): the FMA read column is
    // lx*4 + v, NOT the global col0 — the tile offset would walk into the
    // next shared row (the +1-column fingerprint on 4096^3).
    let b_col_base: Vec<Word> = (0..REG)
        .map(|v| {
            let rg = u32_const(builder, REG as u32);
            let lx4 = u32_binop(builder, spirv::Op::IMul, lx_u, rg);
            let cv = u32_const(builder, v as u32);
            u32_binop(builder, spirv::Op::IAdd, lx4, cv)
        })
        .collect();

    for kk in 0..TILE {
        let kk_u = u32_const(builder, kk as u32);
        let mut a_r: Vec<Word> = Vec::with_capacity(REG as usize);
        for u in 0..REG {
            let base = a_row_base[u as usize];
            let idx = u32_binop(builder, spirv::Op::IAdd, base, kk_u);
            let p = shared_ptr(builder, ctx.shared_a, idx, f32_ty);
            a_r.push(builder.load(f32_ty, p));
        }
        let mut b_r: Vec<Word> = Vec::with_capacity(REG as usize);
        let ts2 = u32_const(builder, TILE as u32);
        let b_row = u32_binop(builder, spirv::Op::IMul, kk_u, ts2);
        for v in 0..REG {
            let bc = b_col_base[v as usize];
            let idx = u32_binop(builder, spirv::Op::IAdd, b_row, bc);
            let p = shared_ptr(builder, ctx.shared_b, idx, f32_ty);
            b_r.push(builder.load(f32_ty, p));
        }
        let mut next: Vec<Word> = Vec::with_capacity((REG * REG) as usize);
        for u in 0..REG {
            for v in 0..REG {
                let i = (u * REG + v) as usize;
                let out = if kk == TILE - 1 {
                    acc_backedges[i]
                } else {
                    builder.gen_id()
                };
                builder.glsl_fma_with_id(out, f32_ty, a_r[u as usize], b_r[v as usize], acc_live[i]);
                next.push(out);
            }
        }
        acc_live = next;
    }

    // Second barrier: every invocation must finish READING the panel before
    // any invocation overwrites it with the next k-panel (WAR hazard).
    builder.emit(Instruction::new(
        spirv::Op::ControlBarrier,
        None,
        None,
        vec![
            Operand::IdRef(scope),
            Operand::IdRef(scope),
            Operand::IdRef(sem),
        ],
    ));

    end_structured_loop(builder, &sig, &bbs, kt_phi, kt_backedge, cond_next)?;

    // ── Store the 4×4 register tile: y[(row0+u)*N + col0+v] ──
    for u in 0..REG {
        for v in 0..REG {
            let i = (u * REG + v) as usize;
            let ru = i64_const(builder, int_ty, u);
            let row = i64_binop(builder, spirv::Op::IAdd, int_ty, row0, ru);
            let cv = i64_const(builder, int_ty, v);
            let col = i64_binop(builder, spirv::Op::IAdd, int_ty, col0, cv);
            let nc2 = i64_const(builder, int_ty, plan.n as u64);
            let idx = i64_binop(builder, spirv::Op::IMul, int_ty, row, nc2);
            let idx = i64_binop(builder, spirv::Op::IAdd, int_ty, idx, col);
            let ptr = ssbo_scalar_ptr(builder, ctx, ctx.y_member, idx, int_ty);
            builder.store(ptr, acc_phi_ids[i]);
        }
    }

    builder.builder.branch(ctx.exit_bb);
    Ok(())
}

/// The cooperative k-panel load phase: 256 invocations × 16 elements each
/// stage the A (row strip) and B (column strip) panels into shared memory.
/// Panel addresses are TILE-relative: A[tile_m*64 + ar][kt*64 + ac] →
/// As[flat], B[kt*64 + br][tile_n*64 + bc] → Bs[flat].
#[allow(clippy::too_many_arguments)]
fn emit_panel_loads(
    builder: &mut super::SpirvBuilder,
    plan: &GemmPlan,
    ctx: &TiledCtx,
    tid: Word,
    kt_phi: Word,
    tile_m: Word,
    tile_n: Word,
    int_ty: Word,
    f32_ty: Word,
) -> Result<(), String> {
    let elems_per_thread = TILE * TILE / (THREADS * THREADS);
    for u in 0..elems_per_thread {
        let t256 = u32_const(builder, (THREADS * THREADS) as u32);
        let uu = u32_const(builder, u as u32);
        let off = u32_binop(builder, spirv::Op::IMul, t256, uu);
        let flat = u32_binop(builder, spirv::Op::IAdd, tid, off);
        let s6 = u32_const(builder, 6);
        let ar_u = u32_binop(builder, spirv::Op::ShiftRightLogical, flat, s6);
        let m63 = u32_const(builder, 63);
        let ac_u = u32_binop(builder, spirv::Op::BitwiseAnd, flat, m63);
        let ar = widen_u2i(builder, ar_u, int_ty);
        let ac = widen_u2i(builder, ac_u, int_ty);

        // A panel: A[tile_m*64 + ar][kt*64 + ac] → As[flat]. The PANEL is
        // tile-relative — row0 is the invoking thread's strip and would
        // load shifted rows (the 64^3 identity-probe fingerprint).
        let tm64 = i64_const(builder, int_ty, TILE);
        let a_tile_row = i64_binop(builder, spirv::Op::IMul, int_ty, tile_m, tm64);
        let a_row = i64_binop(builder, spirv::Op::IAdd, int_ty, a_tile_row, ar);
        let kt64 = i64_const(builder, int_ty, TILE);
        let a_off = i64_binop(builder, spirv::Op::IMul, int_ty, kt_phi, kt64);
        let a_col = i64_binop(builder, spirv::Op::IAdd, int_ty, a_off, ac);
        let kc = i64_const(builder, int_ty, plan.k as u64);
        let a_idx = i64_binop(builder, spirv::Op::IMul, int_ty, a_row, kc);
        let a_idx = i64_binop(builder, spirv::Op::IAdd, int_ty, a_idx, a_col);
        let a_ptr = ssbo_scalar_ptr(builder, ctx, ctx.a_member, a_idx, int_ty);
        let a_val = builder.load(f32_ty, a_ptr);
        let as_p = shared_ptr(builder, ctx.shared_a, flat, f32_ty);
        builder.store(as_p, a_val);

        // B panel: B[kt*64 + br][tile_n*64 + bc] → Bs[flat].
        let s6b = u32_const(builder, 6);
        let br_u = u32_binop(builder, spirv::Op::ShiftRightLogical, flat, s6b);
        let m63b = u32_const(builder, 63);
        let bc_u = u32_binop(builder, spirv::Op::BitwiseAnd, flat, m63b);
        let br = widen_u2i(builder, br_u, int_ty);
        let bc = widen_u2i(builder, bc_u, int_ty);
        let tn64 = i64_const(builder, int_ty, TILE);
        let b_tile_col = i64_binop(builder, spirv::Op::IMul, int_ty, tile_n, tn64);
        let b_col = i64_binop(builder, spirv::Op::IAdd, int_ty, b_tile_col, bc);
        let kt64b = i64_const(builder, int_ty, TILE);
        let b_off = i64_binop(builder, spirv::Op::IMul, int_ty, kt_phi, kt64b);
        let b_row = i64_binop(builder, spirv::Op::IAdd, int_ty, b_off, br);
        let nc = i64_const(builder, int_ty, plan.n as u64);
        let b_idx = i64_binop(builder, spirv::Op::IMul, int_ty, b_row, nc);
        let b_idx = i64_binop(builder, spirv::Op::IAdd, int_ty, b_idx, b_col);
        let b_ptr = ssbo_scalar_ptr(builder, ctx, ctx.b_member, b_idx, int_ty);
        let b_val = builder.load(f32_ty, b_ptr);
        let bs_p = shared_ptr(builder, ctx.shared_b, flat, f32_ty);
        builder.store(bs_p, b_val);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::accel::{KernelShape, ReductionInfo};
    use crate::ast::{BinaryOpKind, Dimension, Type};

    /// Hand-built canonical GEMM shape — no typechecker universe needed
    /// (the matcher consumes the raw AST + a const map).
    fn gemm_shape(m: i64, n: i64, k: i64) -> (KernelShape, Vec<TopLevel>) {
        let idx = |name: &str| Expr::Identifier(name.to_string());
        let num = |v: i64| Expr::Decimal(v);
        let bin = |kind, l: Expr, r: Expr| Expr::BinaryOp(kind, Box::new(l), Box::new(r));
        let index = |of: Expr, i: Expr| Expr::Index(Box::new(of), Box::new(i));

        let items = vec![
            TopLevel::Constant(crate::ast::Constant {
                name: "M".into(),
                expr: num(m),
                ty: Type::Custom("Int".into()),
                section: None,
            }),
            TopLevel::Constant(crate::ast::Constant {
                name: "N".into(),
                expr: num(n),
                ty: Type::Custom("Int".into()),
                section: None,
            }),
            TopLevel::Constant(crate::ast::Constant {
                name: "K".into(),
                expr: num(k),
                ty: Type::Custom("Int".into()),
                section: None,
            }),
        ];

        let body = vec![
            // acc = acc + a[m*K + k] * b[k*N + n];
            Statement::Assign(
                idx("acc"),
                bin(BinaryOpKind::Add, idx("acc"),
                    bin(BinaryOpKind::Mul,
                        index(idx("a"), bin(BinaryOpKind::Add, bin(BinaryOpKind::Mul, idx("m"), num(k)), idx("k"))),
                        index(idx("b"), bin(BinaryOpKind::Add, bin(BinaryOpKind::Mul, idx("k"), num(n)), idx("n"))))),
            ),
        ];

        let kernel_stmts = vec![
            // let acc: Float = 0;
            Statement::Let {
                name: "acc".into(),
                names: vec![],
                ty: Some(Type::Custom("Float".into())),
                expr: Some(num(0)),
                modifiers: vec![],
            },
            // let m: Int = i / N;
            Statement::Let {
                name: "m".into(),
                names: vec![],
                ty: Some(Type::Custom("Int".into())),
                expr: Some(bin(BinaryOpKind::Div, idx("i"), num(n))),
                modifiers: vec![],
            },
            // let n: Int = i % N;
            Statement::Let {
                name: "n".into(),
                names: vec![],
                ty: Some(Type::Custom("Int".into())),
                expr: Some(bin(BinaryOpKind::Mod, idx("i"), num(n))),
                modifiers: vec![],
            },
            Statement::Foreach {
                item: "k".into(),
                list: Box::new(Expr::Range {
                    start: Box::new(num(0)),
                    end: Box::new(idx("K")),
                    inclusive: false,
                }),
                body,
            },
            // y[i] = acc;
            Statement::Assign(index(idx("y"), idx("i")), idx("acc")),
        ];

        let shape = KernelShape {
            index_var: "i".into(),
            count_expr: Some(bin(BinaryOpKind::Mul, idx("M"), idx("N"))),
            kernel_stmts,
            host_stmts: vec![],
            read_buffers: vec![],
            write_buffers: vec![],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: Some(ReductionInfo::dot(idx("K"))),
        };
        (shape, items)
    }

    #[test]
    fn gemm_plan_matches_canonical_body() {
        let (shape, items) = gemm_shape(64, 64, 64);
        let plan = GemmPlan::match_stmts(&shape, &items)
            .expect("canonical body must match");
        assert_eq!((plan.m, plan.n, plan.k), (64, 64, 64));
        assert_eq!(plan.a_field, "a");
        assert_eq!(plan.b_field, "b");
        assert_eq!(plan.y_field, "y");
    }

    #[test]
    fn gemm_plan_requires_tile_divisibility() {
        // 60 is not divisible by the 64-tile → flat fallback.
        let (shape, items) = gemm_shape(60, 64, 64);
        assert!(GemmPlan::match_stmts(&shape, &items).is_none(),
            "non-tile-divisible M must fall back to the flat kernel");
        let (shape, items) = gemm_shape(64, 64, 60);
        assert!(GemmPlan::match_stmts(&shape, &items).is_none(),
            "non-tile-divisible K must fall back to the flat kernel");
    }

    /// Walk every expression (including foreach bodies — the reduction
    /// lives inside one) and retarget the decimal k-stride term to 128.
    fn rewrite_stmt_b_stride(stmt: &mut Statement) {
        match stmt {
            Statement::Assign(_, rhs) => rewrite_b_stride(rhs),
            Statement::Foreach { body, list, .. } => {
                rewrite_b_stride(list);
                for st in body {
                    rewrite_stmt_b_stride(st);
                }
            }
            Statement::Let { expr: Some(e), .. } => rewrite_b_stride(e),
            _ => {}
        }
    }

    fn rewrite_b_stride(e: &mut Expr) {
        match e {
            Expr::BinaryOp(kind, l, r) => {
                rewrite_b_stride(l);
                rewrite_b_stride(r);
                retarget_stride(kind, l, r);
            }
            // the index lives INSIDE an Index — descend
            Expr::Index(of, idx) => {
                rewrite_b_stride(of);
                rewrite_b_stride(idx);
            }
            Expr::UnaryOp(_, e) => rewrite_b_stride(e),
            // everything else carries no stride term
            _ => {}
        }
    }

    /// Replace the decimal stride of the `k * stride` term with 128.
    fn retarget_stride(kind: &BinaryOpKind, l: &mut Box<Expr>, r: &mut Box<Expr>) {
        if *kind != BinaryOpKind::Mul {
            return;
        }
        let lhs_k = matches!(l.as_ref(), Expr::Identifier(n) if n == "k");
        let rhs_k = matches!(r.as_ref(), Expr::Identifier(n) if n == "k");
        if lhs_k {
            if let Expr::Decimal(_) = r.as_ref() {
                *r = Box::new(Expr::Decimal(128));
            }
        } else if rhs_k {
            if let Expr::Decimal(_) = l.as_ref() {
                *l = Box::new(Expr::Decimal(128));
            }
        }
    }

    #[test]
    fn gemm_plan_rejects_stride_mismatch() {
        // b's column stride N2 != N: the tile math would read the wrong
        // columns — the matcher must reject (flat kernel is correct). The
        // .abv's `k * N` arrives const-folded (`k * 64`) — retarget that
        // decimal to 128; a's `m * K` term uses Identifier(m), untouched.
        let (mut shape, items) = gemm_shape(64, 64, 64);
        for stmt in &mut shape.kernel_stmts {
            rewrite_stmt_b_stride(stmt);
        }
        let r = GemmPlan::match_stmts(&shape, &items);
        assert!(r.is_none(),
            "b stride != N must not match the tiled plan");
    }

    #[test]
    fn gemm_plan_ignores_extra_statements() {
        // A host-side statement between the lets and the store keeps the
        // matcher working (statement ORDER never mattered — content does).
        let (mut shape, items) = gemm_shape(64, 64, 64);
        shape.kernel_stmts.insert(
            0,
            Statement::Assign(
                Expr::Identifier("i".into()),
                Expr::Decimal(0),
            ),
        );
        assert!(GemmPlan::match_stmts(&shape, &items).is_some());
    }

    // ── Stage 0 instrument (plan 2026-09-04-beyond-coopmat) ──────────────
    // The coopmat mma-CEILING microkernel: a register-resident
    // OpCooperativeMatrixMulAddKHR chain — no smem, no DRAM in the loop.
    // Its measured throughput IS the vendor SPIR-V lowering's tensor
    // ceiling; the f16acc-vs-f32acc pair answers whether the lowering
    // double-pumps (ceiling ≈ 2× f32 rate) or not (ceiling ≈ f32 rate).
    // That number gates the beyond-coopmat campaign (doctrine
    // abv-gpu-doctrine.md §2).
    //
    // Opt-in: BRIEV_EMIT_MMA_CEILING=1 cargo test emit_mma_ceiling -- --nocapture
    // writes target/spirv/mma_ceiling_{f16acc,f32acc}.spv. Undo: delete
    // this test.

    /// Depth of the round-robin chain: CHAINS independent accumulator
    /// fragments (ILP — a single dependency chain would measure mma
    /// LATENCY), DEPTH total mma ops per loop iteration.
    const CEILING_CHAINS: usize = 4;
    const CEILING_B_FRAGS: usize = 4;
    const CEILING_DEPTH: usize = 16;
    /// Workgroups the host dispatches; each stores one 16×16 f32 tile.
    const CEILING_WGS: usize = 4096;

    #[test]
    fn emit_mma_ceiling_kernels() {
        if std::env::var("BRIEV_EMIT_MMA_CEILING").is_err() {
            return;
        }
        for f16acc in [true, false] {
            let binary = build_mma_ceiling(f16acc).expect("ceiling build");
            let tag = if f16acc { "f16acc" } else { "f32acc" };
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/spirv");
            std::fs::create_dir_all(&dir).unwrap();
            let spv = dir.join(format!("mma_ceiling_{}.spv", tag));
            std::fs::write(&spv, &binary).unwrap();
            // Same structural bar as every emitted kernel (mod.rs §2.5):
            // spirv-val must accept the binary.
            let val = std::process::Command::new("spirv-val")
                .arg(&spv)
                .output()
                .expect("spirv-val on PATH");
            assert!(
                val.status.success(),
                "spirv-val rejected mma_ceiling_{}:\n{}",
                tag,
                String::from_utf8_lossy(&val.stderr)
            );
            println!(
                "mma_ceiling_{}: {} bytes, spirv-val OK → {}",
                tag,
                binary.len(),
                spv.display()
            );
        }
    }

    fn build_mma_ceiling(f16acc: bool) -> Result<Vec<u8>, String> {
        use crate::backend::spirv::SpirvBuilder;
        use rspirv::spirv::{Capability, ExecutionModel, FunctionControl};

        let mut builder = SpirvBuilder::new();
        // Module preamble — mirrors emit_coopmat_smem exactly. new() sets
        // GLSL450; the coopmat class requires the VulkanKHR memory model
        // to pair with the VulkanMemoryModel capability.
        builder.builder.capability(Capability::CooperativeMatrixKHR);
        builder.builder.capability(Capability::VulkanMemoryModel);
        builder.builder.capability(Capability::StorageBuffer16BitAccess);
        builder.builder.capability(Capability::Float16);
        builder.builder.extension("SPV_KHR_cooperative_matrix");
        builder.builder.extension("SPV_KHR_vulkan_memory_model");
        builder.builder.extension("SPV_KHR_16bit_storage");
        builder.builder.module_mut().memory_model = Some(rspirv::dr::Instruction::new(
            spirv::Op::MemoryModel, None, None,
            vec![
                Operand::LiteralBit32(spirv::AddressingModel::Logical as u32),
                Operand::LiteralBit32(spirv::MemoryModel::Vulkan as u32),
            ],
        ));

        // ── Hand-built state SSBO (v2, fold-proof) ──────────────────────
        // The first draft used CONSTANT A/B fragments (splat ones) and the
        // driver folded the whole chain: measured "202 TF/s" = 4× hardware
        // peak, value exact — a per-iteration constant-increment recurrence
        // is legally promotable. Block: A must be RUNTIME data. The A
        // fragment is loaded ONCE per workgroup from a host-seeded f16
        // array (0.1-pattern: binary-inexact → every accumulate rounds →
        // hoisting C = A·B and strength-reducing the chain would change
        // results, i.e. illegal without fast-math). B stays a constant
        // ones-splat: per-element A·B = rowsum(A), uniform across the tile.
        //
        // Layout (mirrors the bench's field table exactly):
        //   i @ 0   i64           (runtime loop bound in, final count out)
        //   a @ 8   half[16M]     (A tile rows @0,4096,...; B rows @16,4112,...)
        //   y @ 536 f32[256*WGS+1](one 16×16 tile per workgroup, +1 vec4 pad)
        let u32_ty = builder.u32_type();
        // SIGNED i64 — the loop uses OpSLessThan (signed-only); production
        // kernels get this via lower_type(Type::int()).
        let int_ty = builder.builder.type_int(64, 1);
        let f16_ty = builder.builder.type_float(16);
        let f32_ty = builder.builder.type_float(32);
        let v3_u32 = builder.builder.type_vector(u32_ty, 3);
        let half_arr_len = builder.u32_const(16 * 1048576);
        let half_arr = builder.builder.type_array(f16_ty, half_arr_len);
        // y as a FIXED HALF array — the production store shape (f16
        // fragment → half member; the f32acc era FConverted before the
        // store, "f32 compute, f16 storage"). Count stays a multiple of
        // 4: the runtime's staging→device CopyBuffer requires a 4-byte
        // multiple total size (a +1 half pad made proj_bytes ≡ 2 mod 4 —
        // undefined copy → device wedge, fence timeout).
        // The store SCATTERS 16 rows at stride-4096 intervals: the last
        // workgroup's row 15 lands at (WGS-1+15)*4096 + 15. Every earlier
        // "mystery wedge" was this span overflowing the member (device
        // fault, fence timeout) — or a proj_bytes not ≡ 0 mod 4 (the
        // runtime's staging→device CopyBuffer needs 4-byte multiples).
        let y_len = builder.u32_const((CEILING_WGS as u32 + 15) * 4096 + 16);
        let y_arr = builder.builder.type_array(f16_ty, y_len);
        let ssbo_struct = builder.builder.type_struct(vec![half_arr, int_ty, y_arr]);
        builder.emit_global(Instruction::new(
            spirv::Op::Decorate, None, None,
            vec![
                Operand::IdRef(ssbo_struct),
                Operand::Decoration(spirv::Decoration::Block),
            ],
        ));
        for (member, offset) in [(0u32, 0u32), (1, 33_554_432), (2, 33_554_440)] {
            builder.emit_global(Instruction::new(
                spirv::Op::MemberDecorate, None, None,
                vec![
                    Operand::IdRef(ssbo_struct),
                    Operand::LiteralBit32(member),
                    Operand::Decoration(spirv::Decoration::Offset),
                    Operand::LiteralBit32(offset),
                ],
            ));
        }
        builder.emit_global(Instruction::new(
            spirv::Op::Decorate, None, None,
            vec![
                Operand::IdRef(half_arr),
                Operand::Decoration(spirv::Decoration::ArrayStride),
                Operand::LiteralBit32(2),
            ],
        ));
        builder.emit_global(Instruction::new(
            spirv::Op::Decorate, None, None,
            vec![
                Operand::IdRef(y_arr),
                Operand::Decoration(spirv::Decoration::ArrayStride),
                // half = 2 bytes. (This said 4 from the f32-y era: the
                // validator accepts a mismatched stride, but the driver
                // scatters the coopmat store rows at 4-byte spacing —
                // silent wrong-address writes, y stayed zero.)
                Operand::LiteralBit32(2),
            ],
        ));
        let ssbo_ptr = builder.ptr_class(StorageClass::StorageBuffer, ssbo_struct);
        let ssbo = builder.gen_id();
        builder.emit_global(Instruction::new(
            spirv::Op::Variable,
            Some(ssbo_ptr),
            Some(ssbo),
            vec![Operand::StorageClass(StorageClass::StorageBuffer)],
        ));
        // The runtime binds the SSBO at set 0 / binding 0 — without these
        // decorations the dispatch reads an unbound descriptor (device
        // fault, fence timeout). Mirrors setup_state_buffer.
        builder.emit_global(Instruction::new(
            spirv::Op::Decorate, None, None,
            vec![
                Operand::IdRef(ssbo),
                Operand::Decoration(spirv::Decoration::DescriptorSet),
                Operand::LiteralBit32(0),
            ],
        ));
        builder.emit_global(Instruction::new(
            spirv::Op::Decorate, None, None,
            vec![
                Operand::IdRef(ssbo),
                Operand::Decoration(spirv::Decoration::Binding),
                Operand::LiteralBit32(0),
            ],
        ));

        // gl_WorkGroupID input builtin (mirrors lower.rs builtin_input).
        // gid/lid are declared too — the production module shape carries
        // all three builtins; keep the module context identical.
        let v3_input_ptr = builder.ptr_class(StorageClass::Input, v3_u32);
        let mut builtin_var = |builtin: spirv::BuiltIn| -> Word {
            let var = builder.gen_id();
            builder.emit_global(Instruction::new(
                spirv::Op::Variable,
                Some(v3_input_ptr),
                Some(var),
                vec![Operand::StorageClass(StorageClass::Input)],
            ));
            builder.emit_global(Instruction::new(
                spirv::Op::Decorate, None, None,
                vec![
                    Operand::IdRef(var),
                    Operand::Decoration(spirv::Decoration::BuiltIn),
                    Operand::BuiltIn(builtin),
                ],
            ));
            var
        };
        let gid = builtin_var(spirv::BuiltIn::GlobalInvocationId);
        let lid = builtin_var(spirv::BuiltIn::LocalInvocationId);
        let wgid = builtin_var(spirv::BuiltIn::WorkgroupId);

        // Fragment types. A/B operands are f16 in BOTH variants (the mma
        // input class is fixed); only the accumulator width changes.
        let scope_sub = u32_const(&mut builder, 3);
        let dim16 = u32_const(&mut builder, 16);
        let use_a = u32_const(&mut builder, 0);
        let use_b = u32_const(&mut builder, 1);
        let use_c = u32_const(&mut builder, 2);
        let cm_a =
            builder.builder.type_cooperative_matrix_khr(f16_ty, scope_sub, dim16, dim16, use_a);
        let cm_b =
            builder.builder.type_cooperative_matrix_khr(f16_ty, scope_sub, dim16, dim16, use_b);
        let acc_elem = if f16acc { f16_ty } else { f32_ty };
        let cm_acc = builder
            .builder
            .type_cooperative_matrix_khr(acc_elem, scope_sub, dim16, dim16, use_c);
        // Accumulator init: a RUNTIME-loaded zero fragment. A CONSTANT
        // coopmat composite as the phi init poisons the whole chain on
        // this driver (constant fragments materialize as zeros AND the
        // compiler constant-folds the phi → the mma chain is DCE'd —
        // measured: y=0 at loop-overhead speed). The init region is
        // a[32*4096 .. +255], host-seeded zeros.
        let zero_acc = builder.float_const(if f16acc { 16 } else { 32 }, 0.0);
        let _ = zero_acc;

        let void_ty = builder.lower_type(&crate::ast::Type::void())?;
        let func_ty = builder.gen_id();
        builder.builder.type_function_id(Some(func_ty), void_ty, []);
        let func_id = builder.gen_id();
        let entry_id = builder.gen_id();
        builder.begin_function(void_ty, func_id, FunctionControl::empty(), func_ty);
        builder.begin_block(Some(entry_id));
        if std::env::var("BRIEV_MMA_CEILING_EMPTY").is_ok() {
            // Bisect knob: EMPTY body — scaffold only (entry, interface,
            // execution mode). If this wedges, the problem is not the body.
            builder.ret();
            builder.end_function();
            let interface: Vec<Word> = [gid, lid, wgid, ssbo].to_vec();
            builder.set_entry_point(func_id, "main", ExecutionModel::GLCompute, &interface);
            builder.add_execution_mode(func_id, spirv::ExecutionMode::LocalSize, 32, 1, 1);
            return builder.build();
        }

        // wg = WorkGroupID.x
        let wgid_v = builder.gen_id();
        let wg = builder.gen_id();
        if true {
            builder.emit(Instruction::new(
                spirv::Op::Load,
                Some(v3_u32),
                Some(wgid_v),
                vec![Operand::IdRef(wgid)],
            ));
            builder.emit(Instruction::new(
                spirv::Op::CompositeExtract,
                Some(u32_ty),
                Some(wg),
                vec![Operand::IdRef(wgid_v), Operand::LiteralBit32(0)],
            ));
        }

        // A fragment: ONE cooperative load from ssbo.a[0] (runtime values,
        // host-seeded — the fold blocker). AccessChain member/index
        // operands are IDS, never literals. NOLOAD bisect knob: constant
        // ones fragment instead (v1 form — ran but folded).
        let c0 = u32_const(&mut builder, 0);
        let c1 = u32_const(&mut builder, 1);
        let c2 = u32_const(&mut builder, 2);
        let layout_row = u32_const(&mut builder, 0);
        let stride16 = u32_const(&mut builder, 16);
        // A fragment: loaded from ssbo.a[0] INSIDE the loop body (a
        // fragment loaded BEFORE the loop and used across the back-edge
        // wedges this driver — bisected: bound=1 completes, bound=100
        // hangs; production loads fragments inside the loop too). NOLOAD
        // knob: constant ones fragment (fold-prone, diagnostics only).
        let a_loads_in_loop = std::env::var("BRIEV_MMA_CEILING_NOLOAD").is_err();

        // bound = ssbo.i (member 0). The loop bound is RUNTIME: a const
        // bound would let the driver fold the whole chain away
        // (observability doctrine).
        let i_ptr_ty = builder.ptr_class(StorageClass::StorageBuffer, int_ty);
        let i_ptr = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(i_ptr_ty),
            Some(i_ptr),
            vec![Operand::IdRef(ssbo), Operand::IdRef(c1)],
        ));
        let bound = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Load,
            Some(int_ty),
            Some(bound),
            vec![Operand::IdRef(i_ptr)],
        ));

        // acc init = LOADED zero fragment (runtime value — see the comment
        // above; constant coopmat composites are poison on this driver).
        let init_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
        let init_ptr = builder.gen_id();
        let c131072 = u32_const(&mut builder, 32 * 4096);
        builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(init_elem_ptr),
            Some(init_ptr),
            vec![
                Operand::IdRef(ssbo),
                Operand::IdRef(c0),
                Operand::IdRef(c131072),
            ],
        ));
        let acc_init = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::CooperativeMatrixLoadKHR,
            Some(cm_acc),
            Some(acc_init),
            vec![
                Operand::IdRef(init_ptr),
                Operand::IdRef(layout_row),
                Operand::IdRef(stride16),
            ],
        ));

        // ── The loop (skipped entirely under BRIEV_MMA_CEILING_NOLOOP —
        // bisect: isolates loop machinery vs struct/load/store) ──────────
        let no_loop = std::env::var("BRIEV_MMA_CEILING_NOLOOP").is_ok();
        // The value stored to y (acc chain head or init) and to i (final
        // counter or the bound itself).
        let acc_final: Word;
        let final_it: Word;
        if no_loop {
            // Discriminates load-broken vs store-broken: NOLOAD stores a
            // CONSTANT ones fragment (store probe); otherwise stores the
            // LOADED fragment (load probe). The load happens HERE (entry
            // block) — this probe has no loop.
            let a_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
            let a_ptr = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::AccessChain,
                Some(a_elem_ptr),
                Some(a_ptr),
                vec![
                    Operand::IdRef(ssbo),
                    Operand::IdRef(c0),
                    Operand::IdRef(c0),
                ],
            ));
            let load_stride = u32_const(&mut builder, 4096);
            let fa = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixLoadKHR,
                Some(cm_a),
                Some(fa),
                vec![
                    Operand::IdRef(a_ptr),
                    Operand::IdRef(layout_row),
                    Operand::IdRef(load_stride),
                ],
            ));
            let one_f16 = builder.float_const(16, 1.0);
            let cm_c16 = cm_c16_probe(&mut builder, f16_ty, scope_sub, dim16, use_c);
            acc_final = if std::env::var("BRIEV_MMA_CEILING_NOLOAD").is_ok() {
                builder.builder.constant_composite(cm_c16, vec![one_f16])
            } else {
                // single mma probe: A·B + acc_init — no phi, no loop
                let b_ptr = builder.gen_id();
                let c16 = u32_const(&mut builder, 16);
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain,
                    Some(a_elem_ptr),
                    Some(b_ptr),
                    vec![
                        Operand::IdRef(ssbo),
                        Operand::IdRef(c0),
                        Operand::IdRef(c16),
                    ],
                ));
                let fb = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::CooperativeMatrixLoadKHR,
                    Some(cm_b),
                    Some(fb),
                    vec![
                        Operand::IdRef(b_ptr),
                        Operand::IdRef(layout_row),
                        Operand::IdRef(load_stride),
                    ],
                ));
                let m = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::CooperativeMatrixMulAddKHR,
                    Some(cm_acc),
                    Some(m),
                    vec![
                        Operand::IdRef(fa),
                        Operand::IdRef(fb),
                        Operand::IdRef(acc_init),
                    ],
                ));
                m
            };
            final_it = bound;
        } else if std::env::var("BRIEV_MMA_CEILING_SCALAR").is_ok() {
            // Bisect knob: SCALAR only — no y store at all (the y Access-
            // Chain + fragment store is the wedge suspect; scalar SSBO
            // access is production-identical).
            acc_final = acc_init;
            final_it = bound;
        } else {
            // Hand-built structured loop with a RUNTIME bound (the shared
            // begin_structured_loop takes a const `groups`; the ceiling
            // needs the bound in state). Mirrors kernel.rs begin/end.
            let zero64 = builder.builder.constant_bit64(int_ty, 0);
            let one64 = builder.builder.constant_bit64(int_ty, 1);
            let header_bb = builder.gen_id();
            let body_bb = builder.gen_id();
            let continue_bb = builder.gen_id();
            let merge_bb = builder.gen_id();
            let preheader_bb = builder.gen_id();
            let cond0 = builder.gen_id();
            let cond_next = builder.gen_id();
            let loop_be = builder.gen_id();
            // CHAINS×B_FRAGS independent accumulator back-edges.
            let acc_be: Vec<Word> =
                (0..CEILING_CHAINS * CEILING_B_FRAGS).map(|_| builder.gen_id()).collect();

            builder.builder.branch(preheader_bb);
            builder.begin_block(Some(preheader_bb));
            let bool_ty = bool_ty_for(&mut builder);
            builder.emit(Instruction::new(
                spirv::Op::SLessThan,
                Some(bool_ty),
                Some(cond0),
                vec![Operand::IdRef(zero64), Operand::IdRef(bound)],
            ));
            builder.builder.branch(header_bb);
            builder.begin_block(Some(header_bb));
            let it = builder
                .builder
                .phi(int_ty, None, [(zero64, preheader_bb), (loop_be, continue_bb)])
                .map_err(|e| format!("it phi: {:?}", e))?;
            let cond_hdr = builder
                .builder
                .phi(bool_ty, None, [(cond0, preheader_bb), (cond_next, continue_bb)])
                .map_err(|e| format!("cond phi: {:?}", e))?;
            let acc_phis: Vec<Word> = (0..CEILING_CHAINS * CEILING_B_FRAGS)
                .map(|j| {
                    // Back-edge ids are pre-reserved (forward reference,
                    // the same pattern as kernel.rs's cond phi →
                    // cond_next); the body defines them into acc_be[j].
                    builder
                        .builder
                        .phi(cm_acc, None, [(acc_init, preheader_bb), (acc_be[j], continue_bb)])
                        .expect("acc phi")
                })
                .collect();
            builder.builder.loop_merge(
                merge_bb,
                continue_bb,
                rspirv::spirv::LoopControl::NONE,
                [] as [Operand; 0],
            );
            builder.builder.branch_conditional(
                cond_hdr, body_bb, merge_bb, [] as [u32; 0],
            );
            builder.begin_block(Some(body_bb));

            let (chain_a, chain_b) = if a_loads_in_loop {
                let a_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
                // A row base = kt*16: the fragment VALUES change every
                // iteration (runtime-dependent increments — the strongest
                // fold blocker; loop-invariant A·B is legally hoistable).
                let ktu = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::UConvert,
                    Some(u32_ty),
                    Some(ktu),
                    vec![Operand::IdRef(it)],
                ));
                let c16k = u32_const(&mut builder, 16);
                let a_row = u32_binop(&mut builder, spirv::Op::IMul, ktu, c16k);
                // ONE A FRAGMENT PER CHAIN (rows kt*16 + j): the 8 chains
                // then consume DISTINCT A·B products — the driver's
                // de-fusion (fma chains sharing A·B → mul+adds) has no
                // redundant work to eliminate.
                let load_stride = u32_const(&mut builder, 4096);
                let mut cas = Vec::new();
                for j in 0..CEILING_CHAINS {
                    let _ = j;
                    let jc = u32_const(&mut builder, j as u32);
                    let row_j = u32_binop(&mut builder, spirv::Op::IAdd, a_row, jc);
                    let a_ptr = builder.gen_id();
                    builder.emit(Instruction::new(
                        spirv::Op::AccessChain,
                        Some(a_elem_ptr),
                        Some(a_ptr),
                        vec![
                            Operand::IdRef(ssbo),
                            Operand::IdRef(c0),
                            Operand::IdRef(row_j),
                        ],
                    ));
                    let fa = builder.gen_id();
                    builder.emit(Instruction::new(
                        spirv::Op::CooperativeMatrixLoadKHR,
                        Some(cm_a),
                        Some(fa),
                        vec![
                            Operand::IdRef(a_ptr),
                            Operand::IdRef(layout_row),
                            Operand::IdRef(load_stride),
                        ],
                    ));
                    cas.push(fa);
                }
                // B: ONE FRAGMENT PER CHAIN (rows kt*16+8+j) — no operand
                // pair repeats within an iteration or across them, so the
                // driver can neither de-fuse nor reassociate the chain.
                let mut cbs = Vec::new();
                for j in 0..CEILING_B_FRAGS {
                    let jc = u32_const(&mut builder, (8 + j) as u32);
                    let brow_j = u32_binop(&mut builder, spirv::Op::IAdd, a_row, jc);
                    let b_ptr = builder.gen_id();
                    builder.emit(Instruction::new(
                        spirv::Op::AccessChain,
                        Some(a_elem_ptr),
                        Some(b_ptr),
                        vec![
                            Operand::IdRef(ssbo),
                            Operand::IdRef(c0),
                            Operand::IdRef(brow_j),
                        ],
                    ));
                    let fb = builder.gen_id();
                    builder.emit(Instruction::new(
                        spirv::Op::CooperativeMatrixLoadKHR,
                        Some(cm_b),
                        Some(fb),
                        vec![
                            Operand::IdRef(b_ptr),
                            Operand::IdRef(layout_row),
                            Operand::IdRef(load_stride),
                        ],
                    ));
                    cbs.push(fb);
                }
                (cas, cbs)
            } else {
                // NOLOAD diagnostics: constant splats (fold-prone).
                let one_f16 = builder.float_const(16, 1.0);
                let mut cas = Vec::new();
                let mut cbs = Vec::new();
                for _ in 0..CEILING_CHAINS {
                    cas.push(builder.builder.constant_composite(cm_a, vec![one_f16]));
                    cbs.push(builder.builder.constant_composite(cm_b, vec![one_f16]));
                }
                (cas, cbs)
            };

            // Round-robin mma chain (acc[j] = A·B + acc[j], j = d %
            // CHAINS). SSA: each mma consumes the PREVIOUS mma result of
            // its chain — only the LAST mma of chain j defines the
            // pre-reserved acc_be[j] the header phi consumes. (First
            // draft reused acc_be[j] as the result id for every mma in
            // the chain: 8 duplicate definitions, exactly what spirv-val's
            // "defined more than once" caught.) NOMMA knob swaps the mmas
            // for CopyObjects (isolates the chain from the machinery).
            let no_mma = std::env::var("BRIEV_MMA_CEILING_NOMMA").is_ok();
            let mut cur: Vec<Word> = acc_phis.clone();
            // The production GEMM's shape: CHAINS×B_FRAGS mma, every
            // (A_r, B_j) pair distinct — no de-fuse/reassociation gain for
            // the driver, and the load:mma ratio (0.5) matches the tensor
            // pipeline.
            for r in 0..CEILING_CHAINS {
                for j in 0..CEILING_B_FRAGS {
                    let idx = r * CEILING_B_FRAGS + j;
                    let res = acc_be[idx];
                    if no_mma {
                        let src = if idx == 0 { chain_a[0] } else { cur[idx] };
                        builder.emit(Instruction::new(
                            spirv::Op::CopyObject,
                            Some(cm_acc),
                            Some(res),
                            vec![Operand::IdRef(src)],
                        ));
                    } else {
                        builder.emit(Instruction::new(
                            spirv::Op::CooperativeMatrixMulAddKHR,
                            Some(cm_acc),
                            Some(res),
                            vec![
                                Operand::IdRef(chain_a[r]),
                                Operand::IdRef(chain_b[j]),
                                Operand::IdRef(cur[idx]),
                            ],
                        ));
                        // NoContraction: forbid fusing this mul+add pair.
                        builder.emit_global(Instruction::new(
                            spirv::Op::Decorate, None, None,
                            vec![
                                Operand::IdRef(res),
                                Operand::Decoration(spirv::Decoration::NoContraction),
                            ],
                        ));
                    }
                    cur[idx] = res;
                }
            }
            builder.builder.branch(continue_bb);
            builder.begin_block(Some(continue_bb));
            builder.emit(Instruction::new(
                spirv::Op::IAdd,
                Some(int_ty),
                Some(loop_be),
                vec![Operand::IdRef(it), Operand::IdRef(one64)],
            ));
            builder.emit(Instruction::new(
                spirv::Op::SLessThan,
                Some(bool_ty),
                Some(cond_next),
                vec![Operand::IdRef(loop_be), Operand::IdRef(bound)],
            ));
            builder.builder.branch(header_bb);
            builder.begin_block(Some(merge_bb));
            acc_final = acc_phis[0];
            final_it = it;
        }

        // Swan song: store acc[0] (FConvert to f32 when f16acc) to
        // y[wg*256], stride 16 — structurally live (an FFI-consumed
        // buffer), so the chain cannot be eliminated.
        let layout_row = u32_const(&mut builder, 0);
        let store_stride = u32_const(&mut builder, 4096);
        let c4096 = u32_const(&mut builder, 4096);
        let y_off = u32_binop(&mut builder, spirv::Op::IMul, wg, c4096);
        let y_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
        let y_ptr = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(y_elem_ptr),
            Some(y_ptr),
            vec![
                Operand::IdRef(ssbo),
                Operand::IdRef(c2),
                Operand::IdRef(y_off),
            ],
        ));
        // ALWAYS store an f16 fragment (the production shape): f16acc
        // stores the accumulator directly; f32acc FConverts first.
        let cm_c16 = builder
            .builder
            .type_cooperative_matrix_khr(f16_ty, scope_sub, dim16, dim16, use_c);
        let frag_out = if f16acc {
            acc_final
        } else {
            let fo = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::FConvert,
                Some(cm_c16),
                Some(fo),
                vec![Operand::IdRef(acc_final)],
            ));
            fo
        };
        if std::env::var("BRIEV_MMA_CEILING_SCALAR").is_err()
            && std::env::var("BRIEV_MMA_CEILING_NOYSTORE").is_err()
        {
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixStoreKHR,
                None,
                None,
                vec![
                    Operand::IdRef(y_ptr),
                    Operand::IdRef(frag_out),
                    Operand::IdRef(layout_row),
                    Operand::IdRef(store_stride),
                ],
            ));
        }
        // Host-visible completion: ssbo.i = it (the runner-style scalar
        // observable).
        let i_store_ptr = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(i_ptr_ty),
            Some(i_store_ptr),
            vec![Operand::IdRef(ssbo), Operand::IdRef(c1)],
        ));
        builder.emit(Instruction::new(
            spirv::Op::Store,
            None,
            None,
            vec![Operand::IdRef(i_store_ptr), Operand::IdRef(final_it)],
        ));

        builder.ret();
        builder.end_function();

        let interface: Vec<Word> = [gid, lid, wgid, ssbo].to_vec();
        builder.set_entry_point(func_id, "main", ExecutionModel::GLCompute, &interface);
        builder.add_execution_mode(func_id, spirv::ExecutionMode::LocalSize, 32, 1, 1);
        builder.build()
    }
}

/// The loop condition compares i64s — Bits(1) through the type lowering.
fn bool_ty_for(builder: &mut super::SpirvBuilder) -> Word {
    builder.lower_type(&crate::ast::Type::Bits(1)).expect("bool type")
}

/// Tensor-operand detection: all three GEMM fields are 16-bit float arrays
/// (the .abv author's `Float16[...]` declarations). Checked through the
/// casting graph — never by type-name matching (rule 19).
pub(crate) fn fields_are_f16(
    builder: &mut super::SpirvBuilder,
    a_elem: &Type,
    b_elem: &Type,
    y_elem: &Type,
) -> bool {
    fn is_f16(builder: &mut super::SpirvBuilder, ty: &Type) -> bool {
        matches!(
            builder.shape_of(ty),
            Ok(crate::casting::graph::SpirvShape::Float { bits: 16 })
        )
    }
    is_f16(builder, a_elem) && is_f16(builder, b_elem) && is_f16(builder, y_elem)
}

/// Cooperative-matrix GEMM (M2.2, plan 2026-09-01-m2-tensor-cores): one
/// 16×64 output tile per WORKGROUP (LocalSize 32 — a single warp owns four
/// C fragments). A loads ONCE per k-panel and is reused across four B
/// fragments (4× the mma per A load — the v1 16×16 tile was load-bound at
/// 2.4 TFLOP/s, measured 2026-09-01). A/B load as f16 fragments straight
/// from the SSBO (the pointer's pointee type may mismatch the component
/// type; the stride is in pointee units), the mma accumulates in f32
/// fragments, the store converts back through OpFConvert. Grid:
/// (M/16)*(N/64) X-flattened.
///
/// SPIR-V contract (mapped via the hand-written smoke kernel, spirv-val
/// clean): Capability CooperativeMatrixKHR + VulkanMemoryModel, extension
/// SPV_KHR_cooperative_matrix, Vulkan memory model, load/store take
/// CONSTANT-INSTRUCTION layout + stride operands, accumulator in Function
/// storage.
/// 2026-09-02 (plan 2026-09-02-cuda-race B2): the tensor kernel's device
/// handles — the SSBO, the two grid builtins, the SubgroupId input (S>1
/// decode), and the three SSBO member indices. One struct keeps the
/// emit_coopmat signature flat.
pub struct CoopMatIo {
    pub ssbo: Word,
    pub wgid: Word,
    /// SubgroupId input variable (u32). Declared by the caller ONLY when
    /// subgroups > 1; S=1 passes Word::MAX and the decode uses the
    /// historical single-subgroup form (no input, no interface entry).
    pub sub_id: Word,
    pub a_member: u32,
    pub b_member: u32,
    pub y_member: u32,
    /// Shared-memory staging arrays (Workgroup, f16).  When present,
    /// emit_coopmat uses the 2-stage double-buffer pipeline.
    /// shared_a = 2 × R × 256 f16, shared_b = 2 × 4 × 256 f16.
    pub shared_a: Option<Word>,
    pub shared_b: Option<Word>,
    /// Subgroup-local lane ID (u32, gl_LocalInvocationID.x).
    pub lane_id: Word,
}


/// Parameters for the smem fill (avoids closure borrow issues).
struct SmemFillParams {
    smem_a: Word,
    smem_b: Word,
    f16_wg_ptr: Word,
    f16_ssbo_ptr: Word,
    a_stage_elems_c: Word,
    a_member_c: Word,
    b_member_c: Word,
    nk: Word,
    n_stride: Word,
    nk_half: Word,
    n_half: Word,
    band_m16: Word,
    tn64: Word,
    s16c: Word,
    f16_ty: Word,
    bool_ty: Word,
    u32_ty: Word,
    lane: Word,
    ssbo: Word,
    elems_per_lane: u32,
    /// D3 (beyond-coopmat Stage 1): paired fill — one v2f16 load/store
    /// per even/odd half pair (the member is retyped array-of-v2f16,
    /// byte-identical; see lower.rs pair_eligible).
    v2_ssbo_ptr: Word,
    v2_wg_ptr: Word,
    v2_f16_ty: Word,
    /// D3b quad view: v4f16 pointers + the ÷4 address constants.
    v4_ssbo_ptr: Word,
    v4_wg_ptr: Word,
    v4_f16_ty: Word,
    k4: Word,
    n4: Word,
    pairs: bool,
    /// D3b quad mode: elems_per_lane counts f16×4 quads; the fill loads
    /// one v4f16 per unit (half the pairs-mode instruction count).
    quad: bool,
    /// D4 (beyond-coopmat Stage 1, occupancy): S subgroups per workgroup.
    /// sub_id = this subgroup's index (0..S-1), derived from LocalInvocationId.
    /// tiles_x_c = the grid decode constant (N / (64*S)) for per-subgroup
    /// tn64 computation. b_stage_elems_c = one stage's B elements per
    /// subgroup (= pps × 4 × 256). subgroups = S (compile-time, for flat stride).
    sub_id: Word,
    tiles_x_c: Word,
    b_stage_elems_c: Word,
    /// Bitwise AND mask for b_stage_elems (b_stage_elems - 1, valid when
    /// b_stage_elems is power-of-two — always true: pps × 4 × 256).
    b_stage_elems_mask: Word,
    /// Bitwise AND mask for b_stage_pairs (b_stage_elems/2 - 1).
    b_stage_pairs_mask: Word,
    subgroups: u32,
    /// Compile-time tile element counts for the split A/B fill (S>1).
    a_stage_elems: u32,
    b_stage_elems: u32,
}

/// Emit DRAM→smem fill for one panel: (R+4)×256 f16 halves.
/// With `pairs` set, the fill works on u32 pairs: the flat index steps
/// in even pairs (adjacent cols = adjacent DRAM halves = adjacent smem
/// slots), so each iteration is ONE aligned u32 load + ONE aligned u32
/// store — half the instruction count of the scalar form, no bitcasts.
fn emit_smem_fill(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    a_off: Word,
    b_off: Word,
    panel_kt: Word,
) {
    if p.pairs {
        emit_smem_fill_pairs(builder, p, a_off, b_off, panel_kt);
        return;
    }
    // Pre-compute all constants outside the loop to avoid double-borrow.
    let c256 = u32_const(builder, 256);
    let c255 = u32_const(builder, 255);
    let c16 = u32_const(builder, 16);
    let c15 = u32_const(builder, 15);
    // D4: flat stride = 32 × subgroups (all S×32 threads cooperate on the fill)
    let c_wg = u32_const(builder, 32 * p.subgroups);

    if p.subgroups > 1 {
        // D4 multi-subgroup fill: A is cooperative (all S×32 threads
        // fill the shared A panel), B is per-subgroup (each subgroup
        // fills its own B slice with its own tn64 — the cooperative
        // fill would use the wrong tn64 for other subgroups' B data).
        // Two sequential loops, no extra barriers.

        // Phase 1: fill A (cooperative, all threads)
        let a_elems_per_lane = p.a_stage_elems / (32 * p.subgroups);
        for u in 0..a_elems_per_lane as usize {
            let flat = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c_wg);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            let tile_idx = u32_shr(builder, flat, 8);
            let elem256 = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
                    vec![Operand::IdRef(flat), Operand::IdRef(c255)],
                ));
                id
            };
            let row_in = u32_shr(builder, elem256, 4);
            let col_in = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
                    vec![Operand::IdRef(elem256), Operand::IdRef(c15)],
                ));
                id
            };
            let a_src = {
                let t16 = u32_binop(builder, spirv::Op::IMul, tile_idx, c16);
                let row = u32_binop(builder, spirv::Op::IAdd, p.band_m16, t16);
                let row2 = u32_binop(builder, spirv::Op::IAdd, row, row_in);
                let kt16 = u32_binop(builder, spirv::Op::IMul, panel_kt, p.s16c);
                let kcol = u32_binop(builder, spirv::Op::IAdd, kt16, col_in);
                let rk = u32_binop(builder, spirv::Op::IMul, row2, p.nk);
                u32_binop(builder, spirv::Op::IAdd, rk, kcol)
            };
            let a_dram = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(p.f16_ssbo_ptr), Some(id),
                    vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.a_member_c), Operand::IdRef(a_src)],
                ));
                id
            };
            let val = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::Load, Some(p.f16_ty), Some(id),
                    vec![Operand::IdRef(a_dram)],
                ));
                id
            };
            let smem_idx = u32_binop(builder, spirv::Op::IAdd, a_off, flat);
            let a_ptr = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(p.f16_wg_ptr), Some(id),
                    vec![Operand::IdRef(p.smem_a), Operand::IdRef(smem_idx)],
                ));
                id
            };
            builder.emit(Instruction::new(
                spirv::Op::Store, None, None,
                vec![Operand::IdRef(a_ptr), Operand::IdRef(val)],
            ));
        }

        // Phase 2: fill B (per-subgroup, each fills its own B slice)
        // flat_b = local_lane + u * 32, total = b_stage_elems / 32
        let b_elems_per_lane = p.b_stage_elems / 32;  // per subgroup
        let c32 = u32_const(builder, 32);
        for u in 0..b_elems_per_lane as usize {
            let flat_b = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c32);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            let tile_idx = u32_shr(builder, flat_b, 8);
            let elem256 = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
                    vec![Operand::IdRef(flat_b), Operand::IdRef(c255)],
                ));
                id
            };
            let row_in = u32_shr(builder, elem256, 4);
            let col_in = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
                    vec![Operand::IdRef(elem256), Operand::IdRef(c15)],
                ));
                id
            };
            // B DRAM source: uses THIS subgroup's tn64 (correct)
            let b_src = {
                let kt16 = u32_binop(builder, spirv::Op::IMul, panel_kt, c16);
                let brow = u32_binop(builder, spirv::Op::IAdd, kt16, row_in);
                let j16 = u32_binop(builder, spirv::Op::IMul, tile_idx, c16);
                let bcol = u32_binop(builder, spirv::Op::IAdd, p.tn64, j16);
                let bcol2 = u32_binop(builder, spirv::Op::IAdd, bcol, col_in);
                let rn = u32_binop(builder, spirv::Op::IMul, brow, p.n_stride);
                u32_binop(builder, spirv::Op::IAdd, rn, bcol2)
            };
            let b_dram = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(p.f16_ssbo_ptr), Some(id),
                    vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.b_member_c), Operand::IdRef(b_src)],
                ));
                id
            };
            let val = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::Load, Some(p.f16_ty), Some(id),
                    vec![Operand::IdRef(b_dram)],
                ));
                id
            };
            // B smem dest: b_off + flat_b (b_off already includes sub_b_offset)
            let smem_idx = u32_binop(builder, spirv::Op::IAdd, b_off, flat_b);
            let b_ptr = {
                let id = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(p.f16_wg_ptr), Some(id),
                    vec![Operand::IdRef(p.smem_b), Operand::IdRef(smem_idx)],
                ));
                id
            };
            builder.emit(Instruction::new(
                spirv::Op::Store, None, None,
                vec![Operand::IdRef(b_ptr), Operand::IdRef(val)],
            ));
        }
        return;
    }

    // S=1: original single-loop fill (A and B in one flat loop)
    for u in 0..p.elems_per_lane as usize {
        let flat = {
            let u_c = u32_const(builder, u as u32);
            let mul = u32_binop(builder, spirv::Op::IMul, u_c, c_wg);
            u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
        };
        let in_a = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::ULessThan, Some(p.bool_ty), Some(id),
                vec![Operand::IdRef(flat), Operand::IdRef(p.a_stage_elems_c)],
            ));
            id
        };
        let tile_idx = u32_shr(builder, flat, 8);
        let elem256 = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
                vec![Operand::IdRef(flat), Operand::IdRef(c255)],
            ));
            id
        };
        let row_in = u32_shr(builder, elem256, 4);
        let col_in = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
                vec![Operand::IdRef(elem256), Operand::IdRef(c15)],
            ));
            id
        };
        // b_flat = flat - a_stage_elems (offset within the ENTIRE B region).
        let b_flat = u32_binop(builder, spirv::Op::ISub, flat, p.a_stage_elems_c);
        // D4: b_flat_within = b_flat % b_stage_elems (index within THIS
        // subgroup's B slice — the DRAM source uses this for the tile
        // column; the smem dest uses the full b_flat for the contiguous layout).
        let b_flat_within = u32_and(builder, b_flat, p.b_stage_elems_mask);
        // B tile index: b_flat_within / 256 gives j in 0..3.
        let b_tile_idx = u32_shr(builder, b_flat_within, 8);
        // A source: (band_m16 + r*16 + row) * K + panel_kt*16 + col
        let a_src = {
            let t16 = u32_binop(builder, spirv::Op::IMul, tile_idx, c16);
            let row = u32_binop(builder, spirv::Op::IAdd, p.band_m16, t16);
            let row2 = u32_binop(builder, spirv::Op::IAdd, row, row_in);
            let kt16 = u32_binop(builder, spirv::Op::IMul, panel_kt, p.s16c);
            let kcol = u32_binop(builder, spirv::Op::IAdd, kt16, col_in);
            let rk = u32_binop(builder, spirv::Op::IMul, row2, p.nk);
            u32_binop(builder, spirv::Op::IAdd, rk, kcol)
        };
        // B source: (panel_kt*16 + row) * N + tn64 + j*16 + col
        let b_src = {
            let kt16 = u32_binop(builder, spirv::Op::IMul, panel_kt, c16);
            let brow = u32_binop(builder, spirv::Op::IAdd, kt16, row_in);
            let j16 = u32_binop(builder, spirv::Op::IMul, b_tile_idx, c16);
            let bcol = u32_binop(builder, spirv::Op::IAdd, p.tn64, j16);
            let bcol2 = u32_binop(builder, spirv::Op::IAdd, bcol, col_in);
            let rn = u32_binop(builder, spirv::Op::IMul, brow, p.n_stride);
            u32_binop(builder, spirv::Op::IAdd, rn, bcol2)
        };
        // SSBO struct member index must be constant — emit separate
        // AccessChains for A and B, then Select the element pointer.
        let a_dram = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::AccessChain, Some(p.f16_ssbo_ptr), Some(id),
                vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.a_member_c), Operand::IdRef(a_src)],
            ));
            id
        };
        let b_dram = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::AccessChain, Some(p.f16_ssbo_ptr), Some(id),
                vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.b_member_c), Operand::IdRef(b_src)],
            ));
            id
        };
        let dram_ptr = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::Select, Some(p.f16_ssbo_ptr), Some(id),
                vec![Operand::IdRef(in_a), Operand::IdRef(a_dram), Operand::IdRef(b_dram)],
            ));
            id
        };
        let val = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::Load, Some(p.f16_ty), Some(id),
                vec![Operand::IdRef(dram_ptr)],
            ));
            id
        };
        let idx = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::Select, Some(p.u32_ty), Some(id),
                vec![Operand::IdRef(in_a), Operand::IdRef(flat), Operand::IdRef(b_flat_within)],
            ));
            id
        };
        let base = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::Select, Some(p.u32_ty), Some(id),
                vec![Operand::IdRef(in_a), Operand::IdRef(a_off), Operand::IdRef(b_off)],
            ));
            id
        };
        let smem_idx = u32_binop(builder, spirv::Op::IAdd, base, idx);
        let a_ptr = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::AccessChain, Some(p.f16_wg_ptr), Some(id),
                vec![Operand::IdRef(p.smem_a), Operand::IdRef(smem_idx)],
            ));
            id
        };
        let b_ptr = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::AccessChain, Some(p.f16_wg_ptr), Some(id),
                vec![Operand::IdRef(p.smem_b), Operand::IdRef(smem_idx)],
            ));
            id
        };
        let dest = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::Select, Some(p.f16_wg_ptr), Some(id),
                vec![Operand::IdRef(in_a), Operand::IdRef(a_ptr), Operand::IdRef(b_ptr)],
            ));
            id
        };
        builder.emit(Instruction::new(
            spirv::Op::Store, None, None,
            vec![Operand::IdRef(dest), Operand::IdRef(val)],
        ));
    }
}

/// D3 paired fill: one v2f16 load + one v2f16 store per even/odd half
/// pair. The SSBO members (and the smem arrays) are retyped
/// array-of-vNf16 (byte-identical; lower.rs view_width), so the wide
/// index IS the element index — no bitcasts, no /2 byte math. Pair
/// pflat covers halves 2p, 2p+1: same tile/row, cols (2c, 2c+1) — the
/// DRAM pair index = a_src/2 (a_src even: K, kt*16, tn64, j*16 all even
/// and col_pair = (col&14)/2).
///
/// D3b quad mode (p.quad): p.elems_per_lane counts QUAD units (f16×4);
/// each iteration loads ONE v4f16. A quad covers halves 4q..4q+3 — same
/// tile/row (flat4%16 ∈ {0,4,8,12} never crosses a 16-half row), never
/// straddles the A/B region boundary (a_pairs = pps·R·128 is even), and
/// the DRAM quad index divides exactly (K, N, tn64+j16, col4 all ÷4).
fn emit_smem_fill_pairs(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    a_off: Word,
    b_off: Word,
    panel_kt: Word,
) {
    if p.subgroups > 1 {
        // D4 multi-subgroup pair/quad fill: A cooperative, B per-subgroup.
        let c_wg = u32_const(builder, 32 * p.subgroups);
        let c32 = u32_const(builder, 32);
        let view_w = if p.quad { 4u32 } else { 2 };

        // Phase 1: fill A (cooperative, all S×32 threads)
        // a_pairs = A_elems / view_w (pair or quad units)
        let a_pairs = p.a_stage_elems / view_w;
        let a_pairs_per_lane = a_pairs / (32 * p.subgroups);
        for u in 0..a_pairs_per_lane as usize {
            let uflat = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c_wg);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            if p.quad {
                emit_smem_fill_quad_unit(builder, p, uflat, a_off, b_off, panel_kt);
            } else {
                emit_smem_fill_pair_unit(builder, p, uflat, a_off, b_off, panel_kt);
            }
        }

        // Phase 2: fill B (per-subgroup, each fills its own B slice)
        // b_pairs_per_sub = B_elems / view_w (pair or quad units per subgroup)
        let b_pairs_per_sub = p.b_stage_elems / view_w;
        let b_pairs_per_lane = b_pairs_per_sub / 32;
        // D4: offset pflat past a_elems so in_a = false in pair/quad unit.
        // a_pairs = A_elems / view_w = the A phase total pair/quad units.
        let a_pairs = p.a_stage_elems / view_w;
        let a_pairs_c = u32_const(builder, a_pairs);
        for u in 0..b_pairs_per_lane as usize {
            let raw = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c32);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            let uflat = u32_binop(builder, spirv::Op::IAdd, raw, a_pairs_c);
            if p.quad {
                emit_smem_fill_quad_unit(builder, p, uflat, a_off, b_off, panel_kt);
            } else {
                emit_smem_fill_pair_unit(builder, p, uflat, a_off, b_off, panel_kt);
            }
        }
        return;
    }

    // S=1: original single-loop fill
    let c32 = u32_const(builder, 32);
    for u in 0..p.elems_per_lane as usize {
        // unit flat = lane + u*32 (PAIR or QUAD units; halves = w× that).
        let uflat = {
            let u_c = u32_const(builder, u as u32);
            let mul = u32_binop(builder, spirv::Op::IMul, u_c, c32);
            u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
        };
        if p.quad {
            emit_smem_fill_quad_unit(builder, p, uflat, a_off, b_off, panel_kt);
        } else {
            emit_smem_fill_pair_unit(builder, p, uflat, a_off, b_off, panel_kt);
        }
    }
}

/// D2 split, pair width — DRAM half: given the PAIR index `pflat`,
/// compute the source address and load the v2f16. Returns the value;
/// the smem destination is recomputed in the store half (D2 prefetch:
/// the load issues during the mma phase, the store after the barrier).
fn emit_fill_pair_dram(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    pflat: Word,
    panel_kt: Word,
) -> Word {
    let c256 = u32_const(builder, 256);
    let c255 = u32_const(builder, 255);
    let c14 = u32_const(builder, 14);
    let c16 = u32_const(builder, 16);
    let c2 = u32_const(builder, 2);
    let c8 = u32_const(builder, 8);
    let flat2 = u32_binop(builder, spirv::Op::IAdd, pflat, pflat);
    let in_a = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::ULessThan, Some(p.bool_ty), Some(id),
            vec![Operand::IdRef(flat2), Operand::IdRef(p.a_stage_elems_c)],
        ));
        id
    };
    let tile_idx = u32_shr(builder, flat2, 8);
    let elem256 = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
            vec![Operand::IdRef(flat2), Operand::IdRef(c255)],
        ));
        id
    };
    let row_in = u32_shr(builder, elem256, 4);
    // Pair col = (elem256 & 14)/2 — the v2f16 element column.
    let col_pair = {
        let c14v = {
            let id = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
                vec![Operand::IdRef(elem256), Operand::IdRef(c14)],
            ));
            id
        };
        u32_shr(builder, c14v, 1)
    };
    let b_flat = u32_binop(builder, spirv::Op::ISub, flat2, p.a_stage_elems_c);
    // D4: b_flat_within = b_flat % b_stage_elems (pair units) — index
    // within THIS subgroup's B slice for the DRAM source tile column.
    let b_flat_within = u32_and(builder, b_flat, p.b_stage_pairs_mask);
    let b_tile_idx = u32_shr(builder, b_flat_within, 8);
    // A pair source: row2*(K/2) + kt*8 + col_pair (all even/2 exact).
    let a_src = {
        let t16 = u32_binop(builder, spirv::Op::IMul, tile_idx, c16);
        let row = u32_binop(builder, spirv::Op::IAdd, p.band_m16, t16);
        let row2 = u32_binop(builder, spirv::Op::IAdd, row, row_in);
        let rk_half = u32_binop(builder, spirv::Op::IMul, row2, p.nk_half);
        let kt8 = u32_binop(builder, spirv::Op::IMul, panel_kt, c8);
        let kcol = u32_binop(builder, spirv::Op::IAdd, kt8, col_pair);
        u32_binop(builder, spirv::Op::IAdd, rk_half, kcol)
    };
    // B pair source: (kt*16+row)*(N/2) + (tn64 + j*16)/2 + col_pair
    let b_src = {
        let kt16 = u32_binop(builder, spirv::Op::IMul, panel_kt, c16);
        let brow = u32_binop(builder, spirv::Op::IAdd, kt16, row_in);
        let rn_half = u32_binop(builder, spirv::Op::IMul, brow, p.n_half);
        let j16 = u32_binop(builder, spirv::Op::IMul, b_tile_idx, c16);
        let bcol = u32_binop(builder, spirv::Op::IAdd, p.tn64, j16);
        let bcol_half = u32_shr(builder, bcol, 1);
        let col = u32_binop(builder, spirv::Op::IAdd, bcol_half, col_pair);
        u32_binop(builder, spirv::Op::IAdd, rn_half, col)
    };
    let a_dram = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v2_ssbo_ptr), Some(id),
            vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.a_member_c), Operand::IdRef(a_src)],
        ));
        id
    };
    let b_dram = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v2_ssbo_ptr), Some(id),
            vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.b_member_c), Operand::IdRef(b_src)],
        ));
        id
    };
    let dram_ptr = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.v2_ssbo_ptr), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(a_dram), Operand::IdRef(b_dram)],
        ));
        id
    };
    let val = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Load, Some(p.v2_f16_ty), Some(id),
            vec![Operand::IdRef(dram_ptr)],
        ));
        id
    };
    val
}

/// D2 split, pair width — smem half: given the PAIR index and the
/// loaded value, compute the staged smem destination and store. The
/// address math repeats the DRAM half's flat/in_a derivation (~6 ALU
/// ops) so no SSA value but `val` crosses the barrier.
fn emit_fill_pair_smem(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    pflat: Word,
    val: Word,
    a_off: Word,
    b_off: Word,
) {
    let c2 = u32_const(builder, 2);
    let flat2 = u32_binop(builder, spirv::Op::IAdd, pflat, pflat);
    let in_a = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::ULessThan, Some(p.bool_ty), Some(id),
            vec![Operand::IdRef(flat2), Operand::IdRef(p.a_stage_elems_c)],
        ));
        id
    };
    let b_flat = u32_binop(builder, spirv::Op::ISub, flat2, p.a_stage_elems_c);
    // D4: b_flat_within = b_flat % b_stage_elems — index within THIS
    // subgroup's B slice for the smem dest.
    let b_flat_within = u32_and(builder, b_flat, p.b_stage_elems_mask);
    // Smem v2f16 destination: (base + flat2)/2 — exact (all offsets
    // even). The pair index IS the v2f16 element index.
    let idx = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.u32_ty), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(flat2), Operand::IdRef(b_flat_within)],
        ));
        id
    };
    let base = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.u32_ty), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(a_off), Operand::IdRef(b_off)],
        ));
        id
    };
    let smem_idx_pair = u32_binop(builder, spirv::Op::IAdd, base, idx);
    let smem_pair_idx = u32_shr(builder, smem_idx_pair, 1);
    let a_ptr = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v2_wg_ptr), Some(id),
            vec![Operand::IdRef(p.smem_a), Operand::IdRef(smem_pair_idx)],
        ));
        id
    };
    let b_ptr = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v2_wg_ptr), Some(id),
            vec![Operand::IdRef(p.smem_b), Operand::IdRef(smem_pair_idx)],
        ));
        id
    };
    let dest = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.v2_wg_ptr), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(a_ptr), Operand::IdRef(b_ptr)],
        ));
        id
    };
    builder.emit(Instruction::new(
        spirv::Op::Store, None, None,
        vec![Operand::IdRef(dest), Operand::IdRef(val)],
    ));
}

/// Fused pair unit = the D2 halves back to back (the prologue fills —
/// nothing to overlap outside the loop).
fn emit_smem_fill_pair_unit(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    pflat: Word,
    a_off: Word,
    b_off: Word,
    panel_kt: Word,
) {
    let val = emit_fill_pair_dram(builder, p, pflat, panel_kt);
    emit_fill_pair_smem(builder, p, pflat, val, a_off, b_off);
}

/// D2 split, quad width — DRAM half: given the QUAD index `qflat`,
/// compute the source address and load the v4f16.
fn emit_fill_quad_dram(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    qflat: Word,
    panel_kt: Word,
) -> Word {
    let c256 = u32_const(builder, 256);
    let c255 = u32_const(builder, 255);
    let c15 = u32_const(builder, 15);
    let c16 = u32_const(builder, 16);
    let c4 = u32_const(builder, 4);
    // quad flat → half flat = 4× the quad index.
    let flat4 = {
        let m1 = u32_binop(builder, spirv::Op::IAdd, qflat, qflat);
        u32_binop(builder, spirv::Op::IAdd, m1, m1)
    };
    let in_a = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::ULessThan, Some(p.bool_ty), Some(id),
            vec![Operand::IdRef(flat4), Operand::IdRef(p.a_stage_elems_c)],
        ));
        id
    };
    let tile_idx = u32_shr(builder, flat4, 8);
    let elem256 = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
            vec![Operand::IdRef(flat4), Operand::IdRef(c255)],
        ));
        id
    };
    let row_in = u32_shr(builder, elem256, 4);
    // Quad col = elem256 & 15 ∈ {0,4,8,12}; the v4 column = /4.
    let col4 = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::BitwiseAnd, Some(p.u32_ty), Some(id),
            vec![Operand::IdRef(elem256), Operand::IdRef(c15)],
        ));
        id
    };
    let colq = u32_shr(builder, col4, 2);
    let b_flat4 = u32_binop(builder, spirv::Op::ISub, flat4, p.a_stage_elems_c);
    // D4: b_flat4_within = b_flat4 % b_stage_elems — index within THIS
    // subgroup's B slice for the DRAM source tile column.
    let b_flat4_within = u32_and(builder, b_flat4, p.b_stage_elems_mask);
    let b_tile_idx = u32_shr(builder, b_flat4_within, 8);
    // A quad source: row2*(K/4) + kt*4 + col4/4 (K ÷4; all exact).
    let a_src = {
        let t16 = u32_binop(builder, spirv::Op::IMul, tile_idx, c16);
        let row = u32_binop(builder, spirv::Op::IAdd, p.band_m16, t16);
        let row2 = u32_binop(builder, spirv::Op::IAdd, row, row_in);
        let rk_q = u32_binop(builder, spirv::Op::IMul, row2, p.k4);
        let kt4 = u32_binop(builder, spirv::Op::IMul, panel_kt, c4);
        let kcol = u32_binop(builder, spirv::Op::IAdd, kt4, colq);
        u32_binop(builder, spirv::Op::IAdd, rk_q, kcol)
    };
    // B quad source: (kt*16+row)*(N/4) + (tn64 + j*16)/4 + col4/4.
    let b_src = {
        let kt16 = u32_binop(builder, spirv::Op::IMul, panel_kt, c16);
        let brow = u32_binop(builder, spirv::Op::IAdd, kt16, row_in);
        let rn_q = u32_binop(builder, spirv::Op::IMul, brow, p.n4);
        let j16 = u32_binop(builder, spirv::Op::IMul, b_tile_idx, c16);
        let bcol = u32_binop(builder, spirv::Op::IAdd, p.tn64, j16);
        let bcol_q = u32_shr(builder, bcol, 2);
        let col = u32_binop(builder, spirv::Op::IAdd, bcol_q, colq);
        u32_binop(builder, spirv::Op::IAdd, rn_q, col)
    };
    let a_dram = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v4_ssbo_ptr), Some(id),
            vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.a_member_c), Operand::IdRef(a_src)],
        ));
        id
    };
    let b_dram = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v4_ssbo_ptr), Some(id),
            vec![Operand::IdRef(p.ssbo), Operand::IdRef(p.b_member_c), Operand::IdRef(b_src)],
        ));
        id
    };
    let dram_ptr = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.v4_ssbo_ptr), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(a_dram), Operand::IdRef(b_dram)],
        ));
        id
    };
    let val = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Load, Some(p.v4_f16_ty), Some(id),
            vec![Operand::IdRef(dram_ptr)],
        ));
        id
    };
    val
}

/// D2 split, quad width — smem half: given the QUAD index and the
/// loaded value, compute the staged smem destination and store.
fn emit_fill_quad_smem(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    qflat: Word,
    val: Word,
    a_off: Word,
    b_off: Word,
) {
    let c4 = u32_const(builder, 4);
    let flat4 = {
        let m1 = u32_binop(builder, spirv::Op::IAdd, qflat, qflat);
        u32_binop(builder, spirv::Op::IAdd, m1, m1)
    };
    let in_a = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::ULessThan, Some(p.bool_ty), Some(id),
            vec![Operand::IdRef(flat4), Operand::IdRef(p.a_stage_elems_c)],
        ));
        id
    };
    let b_flat4 = u32_binop(builder, spirv::Op::ISub, flat4, p.a_stage_elems_c);
    // D4: b_flat4_within = b_flat4 % b_stage_elems — index within THIS
    // subgroup's B slice for the smem dest.
    let b_flat4_within = u32_and(builder, b_flat4, p.b_stage_elems_mask);
    // Smem v4f16 destination: (base + flat4)/4 — exact (all offsets ÷4).
    let idx = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.u32_ty), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(flat4), Operand::IdRef(b_flat4_within)],
        ));
        id
    };
    let base = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.u32_ty), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(a_off), Operand::IdRef(b_off)],
        ));
        id
    };
    let smem_idx = u32_binop(builder, spirv::Op::IAdd, base, idx);
    let smem_quad_idx = u32_shr(builder, smem_idx, 2);
    let a_ptr = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v4_wg_ptr), Some(id),
            vec![Operand::IdRef(p.smem_a), Operand::IdRef(smem_quad_idx)],
        ));
        id
    };
    let b_ptr = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain, Some(p.v4_wg_ptr), Some(id),
            vec![Operand::IdRef(p.smem_b), Operand::IdRef(smem_quad_idx)],
        ));
        id
    };
    let dest = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::Select, Some(p.v4_wg_ptr), Some(id),
            vec![Operand::IdRef(in_a), Operand::IdRef(a_ptr), Operand::IdRef(b_ptr)],
        ));
        id
    };
    builder.emit(Instruction::new(
        spirv::Op::Store, None, None,
        vec![Operand::IdRef(dest), Operand::IdRef(val)],
    ));
}

/// Fused quad unit = the D2 halves back to back (prologue fills).
fn emit_smem_fill_quad_unit(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    qflat: Word,
    a_off: Word,
    b_off: Word,
    panel_kt: Word,
) {
    let val = emit_fill_quad_dram(builder, p, qflat, panel_kt);
    emit_fill_quad_smem(builder, p, qflat, val, a_off, b_off);
}

/// D2 load phase: the whole fill's DRAM half — one value per unit,
/// issued as straight-line loads (the caller positions these during the
/// mma phase; the values sit in registers until the first barrier).
/// D2 load phase: the whole fill's DRAM half — every unit's value is
/// loaded into registers (the smem stores land after the barrier in the
/// store phase). S=1: one cooperative loop over the A+B flat space.
/// S>1 (2026-09-07 fix): the fused fill walks TWO phases — A cooperative
/// (all S×32 threads, c_wg stride) then B per-subgroup (each subgroup's
/// own 32 lanes, c32 stride, offset past the A region). The single
/// cooperative loop collapsed the per-subgroup B regions (every subgroup
/// modulo-mapped the whole B space onto its own slice → misplaced
/// stores, rel ~3e-2). Mirror the two-phase walk here so the values
/// land in the order the store phase expects.
fn emit_fill_load_phase(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    panel_kt: Word,
) -> Vec<Word> {
    let view_w = if p.quad { 4u32 } else { 2 };
    if p.subgroups > 1 {
        let c_wg = u32_const(builder, 32 * p.subgroups);
        let c32 = u32_const(builder, 32);
        let a_pairs = p.a_stage_elems / view_w;
        let a_pairs_c = u32_const(builder, a_pairs);
        let a_per_lane = a_pairs / (32 * p.subgroups);
        let b_per_lane = (p.b_stage_elems / view_w) / 32;
        let mut vals = Vec::with_capacity((a_per_lane + b_per_lane) as usize);
        // Phase 1: A (cooperative).
        for u in 0..a_per_lane as usize {
            let uflat = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c_wg);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            let v = if p.quad {
                emit_fill_quad_dram(builder, p, uflat, panel_kt)
            } else {
                emit_fill_pair_dram(builder, p, uflat, panel_kt)
            };
            vals.push(v);
        }
        // Phase 2: B (per-subgroup, offset past the A region).
        for u in 0..b_per_lane as usize {
            let raw = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c32);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            let uflat = u32_binop(builder, spirv::Op::IAdd, raw, a_pairs_c);
            let v = if p.quad {
                emit_fill_quad_dram(builder, p, uflat, panel_kt)
            } else {
                emit_fill_pair_dram(builder, p, uflat, panel_kt)
            };
            vals.push(v);
        }
        vals
    } else {
        // D4: flat stride = 32 × subgroups (subgroups=1 → 32)
        let c_wg = u32_const(builder, 32 * p.subgroups);
        (0..p.elems_per_lane as usize)
            .map(|u| {
                let uflat = {
                    let u_c = u32_const(builder, u as u32);
                    let mul = u32_binop(builder, spirv::Op::IMul, u_c, c_wg);
                    u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
                };
                if p.quad {
                    emit_fill_quad_dram(builder, p, uflat, panel_kt)
                } else {
                    emit_fill_pair_dram(builder, p, uflat, panel_kt)
                }
            })
            .collect()
    }
}

/// D2 store phase: the whole fill's smem half — the loaded values land
/// in the staged smem slots after the barrier. Mirrors the load phase's
/// flat walk (S=1 single loop; S>1 A-cooperative then B-per-subgroup).
fn emit_fill_store_phase(
    builder: &mut super::SpirvBuilder,
    p: &SmemFillParams,
    vals: &[Word],
    a_off: Word,
    b_off: Word,
) {
    let view_w = if p.quad { 4u32 } else { 2 };
    if p.subgroups > 1 {
        let c_wg = u32_const(builder, 32 * p.subgroups);
        let c32 = u32_const(builder, 32);
        let a_pairs = p.a_stage_elems / view_w;
        let a_pairs_c = u32_const(builder, a_pairs);
        let a_per_lane = a_pairs / (32 * p.subgroups);
        let b_per_lane = (p.b_stage_elems / view_w) / 32;
        let mut i = 0usize;
        // Phase 1: A (cooperative).
        for u in 0..a_per_lane as usize {
            let uflat = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c_wg);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            if p.quad {
                emit_fill_quad_smem(builder, p, uflat, vals[i], a_off, b_off);
            } else {
                emit_fill_pair_smem(builder, p, uflat, vals[i], a_off, b_off);
            }
            i += 1;
        }
        // Phase 2: B (per-subgroup).
        for u in 0..b_per_lane as usize {
            let raw = {
                let u_c = u32_const(builder, u as u32);
                let mul = u32_binop(builder, spirv::Op::IMul, u_c, c32);
                u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
            };
            let uflat = u32_binop(builder, spirv::Op::IAdd, raw, a_pairs_c);
            if p.quad {
                emit_fill_quad_smem(builder, p, uflat, vals[i], a_off, b_off);
            } else {
                emit_fill_pair_smem(builder, p, uflat, vals[i], a_off, b_off);
            }
            i += 1;
        }
        return;
    }
    // D4: flat stride = 32 × subgroups (subgroups=1 → 32)
    let c_wg = u32_const(builder, 32 * p.subgroups);
    for (u, val) in vals.iter().enumerate() {
        let uflat = {
            let u_c = u32_const(builder, u as u32);
            let mul = u32_binop(builder, spirv::Op::IMul, u_c, c_wg);
            u32_binop(builder, spirv::Op::IAdd, p.lane, mul)
        };
        if p.quad {
            emit_fill_quad_smem(builder, p, uflat, *val, a_off, b_off);
        } else {
            emit_fill_pair_smem(builder, p, uflat, *val, a_off, b_off);
        }
    }
}

/// Emit a Workgroup barrier (SequentiallyConsistent | UniformMemory | WorkgroupMemory = 0x508).
fn emit_wg_barrier(builder: &mut super::SpirvBuilder) {
    let scope = builder.u32_const(2);
    let sem = builder.u32_const(0x508);
    builder.emit(Instruction::new(
        spirv::Op::ControlBarrier, None, None,
        vec![Operand::IdRef(scope), Operand::IdRef(scope), Operand::IdRef(sem)],
    ));
}

/// 2-stage double-buffered smem pipeline for the coopmat tier.
fn emit_coopmat_smem(
    builder: &mut super::SpirvBuilder,
    plan: &GemmPlan,
    io: &CoopMatIo,
    tile_rows: u32,
    exit_bb: Word,
) -> Result<(), String> {
    use rspirv::spirv::Capability;
    let (ssbo, wgid, a_member, b_member, y_member) =
        (io.ssbo, io.wgid, io.a_member, io.b_member, io.y_member);

    // Module preamble.
    builder.builder.capability(Capability::CooperativeMatrixKHR);
    builder.builder.capability(Capability::VulkanMemoryModel);
    builder.builder.capability(Capability::StorageBuffer16BitAccess);
    builder.builder.capability(Capability::Float16);
    builder.builder.capability(Capability::VariablePointers);
    builder.builder.extension("SPV_KHR_variable_pointers");
    builder.builder.extension("SPV_KHR_cooperative_matrix");
    builder.builder.extension("SPV_KHR_vulkan_memory_model");
    builder.builder.extension("SPV_KHR_16bit_storage");
    builder.builder.module_mut().memory_model = Some(rspirv::dr::Instruction::new(
        spirv::Op::MemoryModel, None, None,
        vec![
            Operand::LiteralBit32(spirv::AddressingModel::Logical as u32),
            Operand::LiteralBit32(spirv::MemoryModel::Vulkan as u32),
        ],
    ));

    let int_ty = builder.lower_type(&crate::ast::Type::int())?;
    let bool_ty = builder.lower_type(&crate::ast::Type::Bits(1))?;
    let f32_ty = builder.builder.type_float(32);
    let f16_ty = builder.builder.type_float(16);
    let u32_ty = builder.u32_type();

    let scope_sub = u32_const(builder, 3);
    let use_a = u32_const(builder, 0);
    let use_b = u32_const(builder, 1);
    let use_c = u32_const(builder, 2);
    let dim16 = u32_const(builder, 16);
    let cm_a16 = builder.builder.type_cooperative_matrix_khr(f16_ty, scope_sub, dim16, dim16, use_a);
    let cm_b16 = builder.builder.type_cooperative_matrix_khr(f16_ty, scope_sub, dim16, dim16, use_b);
    let f16acc = GemmPlan::coopmat_f16acc();
    let cm_c16 = builder.builder.type_cooperative_matrix_khr(f16_ty, scope_sub, dim16, dim16, use_c);
    let cm_c32 = builder.builder.type_cooperative_matrix_khr(f32_ty, scope_sub, dim16, dim16, use_c);
    let acc_cm = if f16acc { cm_c16 } else { cm_c32 };

    let layout_row = builder.u32_const(0);

    // Grid decode.
    let subgroups = GemmPlan::coopmat_subgroups();
    let tiles_x = (plan.n / (64 * subgroups as i64)) as u32;
    let wgid_x = builtin_comp_u(builder, wgid, 0);
    let tiles_x_c = u32_const(builder, tiles_x);
    let tile_my = u32_binop(builder, spirv::Op::UDiv, wgid_x, tiles_x_c);
    let tile_n = if subgroups > 1 {
        let tn_base = u32_binop(builder, spirv::Op::UMod, wgid_x, tiles_x_c);
        let s_c = u32_const(builder, subgroups);
        let tn_mul = u32_binop(builder, spirv::Op::IMul, tn_base, s_c);
        u32_binop(builder, spirv::Op::IAdd, tn_mul, io.sub_id)
    } else {
        u32_binop(builder, spirv::Op::UMod, wgid_x, tiles_x_c)
    };
    let s16 = u32_const(builder, 16);
    let s64 = u32_const(builder, 64);
    let r_rows_c = u32_const(builder, tile_rows);
    let band_m = u32_binop(builder, spirv::Op::IMul, tile_my, r_rows_c);
    let band_m16 = u32_binop(builder, spirv::Op::IMul, band_m, s16);
    let tn64 = u32_binop(builder, spirv::Op::IMul, tile_n, s64);
    let nk = u32_const(builder, plan.k as u32);
    let a_row_bases: Vec<Word> = (0..tile_rows)
        .map(|r| {
            let rc = u32_const(builder, (r * 16) as u32);
            let row = u32_binop(builder, spirv::Op::IAdd, band_m16, rc);
            u32_binop(builder, spirv::Op::IMul, row, nk)
        })
        .collect();

    // Accumulators.
    let zero_f = builder.float_const(if f16acc { 16 } else { 32 }, 0.0);
    let acc_zero = builder.builder.constant_composite(acc_cm, vec![zero_f]);
    let acc_count = tile_rows * 4;
    let acc_backedges: Vec<Word> = (0..acc_count).map(|_| builder.gen_id()).collect();
    let acc_inits: Vec<Word> = (0..acc_count).map(|_| acc_zero).collect();
    let acc_phis: Vec<(Word, Word, Word)> = acc_backedges
        .iter().zip(acc_inits.iter())
        .map(|(&be, &init)| (acc_cm, init, be))
        .collect();

    // ── Smem prologue (before the loop) ──
    let smem_a = io.shared_a.unwrap();
    let smem_b = io.shared_b.unwrap();
    let f16_wg_ptr = builder.ptr_class(StorageClass::Workgroup, f16_ty);
    let f16_ssbo_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
    let lane = io.lane_id;
    let pps = GemmPlan::coopmat_panels_per_stage(plan.k);
    let a_stage_elems = pps * tile_rows * 256; // pps: u32, tile_rows: u32
    let a_stage_elems_c = u32_const(builder, a_stage_elems);
    let subgroups = GemmPlan::coopmat_subgroups();
    let b_stage_elems = pps * 4 * 256; // one subgroup's B per stage
    // D4: total fill elements = A + S × B (each subgroup fills its own B slice)
    let total_elems = a_stage_elems + subgroups * b_stage_elems;
    let pairs = GemmPlan::coopmat_fill_pairs();
    // Pairs mode: the SSBO members AND the smem arrays are retyped
    // array-of-vNf16 (byte-identical; lower.rs view_width) — the fill
    // iterates wide units (half the instruction count per widening).
    // D3b quad fill: pps·(R+S·4)·256 halves = pps·(R+S·4)·64 quad units —
    // always lane-aligned (÷32 exact), and a_stage_elems = pps·R·256 is
    // ÷4 exact so no quad straddles the A/B region boundary.
    let quad = GemmPlan::coopmat_fill_quad_active(plan);
    // D4: elems_per_lane divides by S (the workgroup has S×32 threads)
    let wg_threads = 32 * subgroups;
    let elems_per_lane = if quad {
        total_elems / 4 / wg_threads
    } else if pairs {
        total_elems / 2 / wg_threads
    } else {
        total_elems / wg_threads
    };
    let a_member_c = u32_const(builder, a_member);
    let b_member_c = u32_const(builder, b_member);
    let n_stride = u32_const(builder, plan.n as u32);
    let s16c = u32_const(builder, 16);
    let (v2_ssbo_ptr, v2_wg_ptr, v2_f16_ty, nk_half, n_half) = if pairs {
        let v2 = builder.builder.type_vector(f16_ty, 2);
        (
            builder.ptr_class(StorageClass::StorageBuffer, v2),
            builder.ptr_class(StorageClass::Workgroup, v2),
            v2,
            u32_const(builder, (plan.k / 2) as u32),
            u32_const(builder, (plan.n / 2) as u32),
        )
    } else {
        let z = u32_const(builder, 0);
        (z, z, f16_ty, z, z)
    };
    let (v4_ssbo_ptr, v4_wg_ptr, v4_f16_ty, k4, n4) = if quad {
        let v4 = builder.builder.type_vector(f16_ty, 4);
        (
            builder.ptr_class(StorageClass::StorageBuffer, v4),
            builder.ptr_class(StorageClass::Workgroup, v4),
            v4,
            u32_const(builder, (plan.k / 4) as u32),
            u32_const(builder, (plan.n / 4) as u32),
        )
    } else {
        let z = u32_const(builder, 0);
        (z, z, f16_ty, z, z)
    };

    let prefetch = pairs && GemmPlan::coopmat_fill_prefetch();
    let b_stage_elems_c = u32_const(builder, b_stage_elems);
    let b_stage_elems_mask = u32_const(builder, b_stage_elems - 1);
    let b_stage_pairs_mask = u32_const(builder, b_stage_elems / 2 - 1);
    let fill_params = SmemFillParams {
        smem_a, smem_b, f16_wg_ptr, f16_ssbo_ptr,
        a_stage_elems_c, a_member_c, b_member_c,
        nk, n_stride, nk_half, n_half, band_m16, tn64, s16c,
        f16_ty, bool_ty, u32_ty, lane, ssbo, elems_per_lane,
        v2_ssbo_ptr, v2_wg_ptr, v2_f16_ty,
        v4_ssbo_ptr, v4_wg_ptr, v4_f16_ty, k4, n4,
        pairs, quad,
        sub_id: io.sub_id,
        tiles_x_c: tiles_x_c.clone(),
        b_stage_elems_c,
        b_stage_elems_mask,
        b_stage_pairs_mask,
        subgroups,
        a_stage_elems,
        b_stage_elems,
    };

    // D1: pps panels per stage. The prologue fills stage 0 with the
    // panels 0..pps-1 (sub-offsets tile_rows*256 / 1024 apart), then
    // stage 1 with the panels pps..2pps-1; every panel clamps at the
    // last panel index. 2026-09-05 FIX: the panel index is in PANEL
    // units (stage·pps+pi) matching the refill — the old ×16 passed
    // panel 16 where the loop expected panel 1 (one K-panel doubled,
    // one missing ≈ 0.4% error, hidden inside the 4.476e-03 "OK").
    let groups = (plan.k / 16 / pps as i64) as u32;
    let groups_c = u32_const(builder, groups);
    let last_panel_c = u32_const(builder, groups - 1);
    // D5 stagger: this workgroup's K-panel rotation start — wgid % 8
    // buckets × groups/8 panels apart, so the ~8 co-resident workgroups
    // on an SM walk the K range at distinct offsets (their fill-DRAM
    // bursts and mma phases no longer align). Panels complete via the
    // UMod wrap in the refill; the prologue start+1 never exceeds the
    // last panel for groups ≥ 16 (the tensor-tier minimum).
    let stagger = GemmPlan::coopmat_stagger() && groups % 8 == 0 && groups >= 16;
    let start_panel: Word = if stagger {
        let per = u32_const(builder, groups / 8);
        let seven = u32_const(builder, 7);
        let bucket = u32_and(builder, wgid_x, seven);
        u32_binop(builder, spirv::Op::IMul, bucket, per)
    } else {
        u32_const(builder, 0)
    };
    // D4: per-subgroup B smem base — each subgroup fills its own B slice.
    // Layout: [sub0_stage0, sub0_stage1, sub1_stage0, sub1_stage1, ...]
    let b_stage_elems = pps * 4 * 256;
    let stages = GemmPlan::coopmat_stages();
    let sub_b_base_elems = stages * b_stage_elems; // all stages per subgroup
    // sub_b_offset = sub_id × sub_b_base_elems (runtime — different per subgroup)
    let sub_b_offset = if subgroups > 1 {
        let sub_b_elems_c = u32_const(builder, sub_b_base_elems);
        u32_binop(builder, spirv::Op::IMul, io.sub_id, sub_b_elems_c)
    } else {
        u32_const(builder, 0)
    };
    for stage in 0..stages {
        for pi in 0..pps {
            let a_off = u32_const(
                builder,
                (stage * a_stage_elems + pi * tile_rows * 256) as u32,
            );
            let b_stage_off = u32_const(builder, (stage * pps * 4 * 256 + pi * 4 * 256) as u32);
            // D4: b_off = sub_b_offset + stage/panel offset (per-subgroup B slice)
            let b_off = u32_binop(builder, spirv::Op::IAdd, sub_b_offset, b_stage_off);
            let kt_panel = {
                let raw = if stagger {
                    let off = u32_const(builder, stage * pps + pi);
                    u32_binop(builder, spirv::Op::IAdd, start_panel, off)
                } else {
                    u32_const(builder, stage * pps + pi)
                };
                let over = {
                    let id = builder.gen_id();
                    builder.emit(Instruction::new(
                        spirv::Op::UGreaterThan, Some(bool_ty), Some(id),
                        vec![Operand::IdRef(raw), Operand::IdRef(last_panel_c)],
                    ));
                    id
                };
                {
                    let id = builder.gen_id();
                    builder.emit(Instruction::new(
                        spirv::Op::Select, Some(u32_ty), Some(id),
                        vec![Operand::IdRef(over), Operand::IdRef(last_panel_c), Operand::IdRef(raw)],
                    ));
                    id
                }
            };
            emit_smem_fill(builder, &fill_params, a_off, b_off, kt_panel);
        }
        emit_wg_barrier(builder);
    }

    // ── Loop ──
    let sig = CoopLoopSig {
        int_ty,
        bool_ty,
        groups: (plan.k / 16 / pps as i64),
    };
    let (bbs, acc_ids, cond_next, _cond0, kt_phi, kt_backedge) =
        begin_structured_loop(builder, &sig, &acc_phis)?;
    let acc_phis_live: Vec<Word> = acc_ids.clone();

    let kt_u = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::UConvert, Some(u32_ty), Some(id),
            vec![Operand::IdRef(kt_phi)],
        ));
        id
    };

    // Stage index: stages=2 → kt parity, stages=1 → 0 (single buffer).
    let s = {
        let mask = u32_const(builder, stages - 1);
        u32_binop(builder, spirv::Op::BitwiseAnd, kt_u, mask)
    };

    // Load A fragments from smem: pps panels × tile_rows strips. Pairs
    // mode: the smem arrays are array-of-v2f16 — the AccessChain walks
    // [pair_idx, 0] to a half pointer (the pair's first half); the
    // stride-16 fragment walk is unchanged (the production form).
    let c_a_se = u32_const(builder, a_stage_elems);
    let stage_off_a = u32_binop(builder, spirv::Op::IMul, s, c_a_se);
    // 2026-09-07 (anti-WAR refill): the refill writes the OTHER stage
    // (1-s) so it never races the in-flight CM loads of stage s — the
    // workgroup barrier between the mma and the refill becomes
    // unnecessary (double-buffer only). The refill fills stage 1-s with
    // panel kt+1 (read at the next iteration); the CM loads keep reading
    // stage s (panel kt). Single-buffer (stages=1) has no spare stage —
    // refill offset 0, barrier retained.
    let refill_stage = if stages > 1 {
        let one = u32_const(builder, 1);
        u32_binop(builder, spirv::Op::BitwiseXor, s, one)
    } else {
        u32_const(builder, 0)
    };
    let stage_off_a_refill = u32_binop(builder, spirv::Op::IMul, refill_stage, c_a_se);
    let a_panel_stride = (tile_rows * 256) as usize;
    let mut frag_as: Vec<Word> = Vec::new();
    for pi in 0..pps as usize {
        let pi_c = u32_const(builder, (pi * a_panel_stride) as u32);
        let panel_off = u32_binop(builder, spirv::Op::IAdd, stage_off_a, pi_c);
        for r in 0..a_row_bases.len() {
            let r_c = u32_const(builder, (r * 256) as u32);
            let base = u32_binop(builder, spirv::Op::IAdd, panel_off, r_c);
            let ptr = builder.gen_id();
            if pairs {
                // vNf16 smem view: walk [unit_idx, 0] to a half pointer
                // (unit = pair or quad; the D3b quad smem is v4f16).
                let c0 = u32_const(builder, 0);
                let vw = if quad { 4 } else { 2 };
                let vw_c = u32_const(builder, vw);
                let unit_base = u32_binop(builder, spirv::Op::UDiv, base, vw_c);
                let half_ptr = builder.ptr_class(StorageClass::Workgroup, f16_ty);
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(half_ptr), Some(ptr),
                    vec![Operand::IdRef(smem_a), Operand::IdRef(unit_base), Operand::IdRef(c0)],
                ));
            } else {
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(f16_wg_ptr), Some(ptr),
                    vec![Operand::IdRef(smem_a), Operand::IdRef(base)],
                ));
            }
            let frag = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixLoadKHR, Some(cm_a16), Some(frag),
                vec![Operand::IdRef(ptr), Operand::IdRef(layout_row), Operand::IdRef(s16c)],
            ));
            frag_as.push(frag);
        }
    }

    // Load B fragments from smem: pps panels × 4 tiles.
    let b_stage_size = pps * 4 * 256;
    let c_b_ss = u32_const(builder, b_stage_size);
    // D4: each subgroup reads from its own B slice in smem_b.
    // Layout: [sub0_stage0, sub0_stage1, sub1_stage0, sub1_stage1, ...]
    // sub_b_base = sub_id × stages × b_stage_size (skip other subgroups' stages)
    // stage_off_b = sub_b_base + s × b_stage_size (stage within this subgroup's slice)
    let sub_b_base = if subgroups > 1 {
        let stages_b = u32_const(builder, stages * b_stage_size);
        u32_binop(builder, spirv::Op::IMul, io.sub_id, stages_b)
    } else {
        u32_const(builder, 0)
    };
    let stage_off_b_raw = u32_binop(builder, spirv::Op::IMul, s, c_b_ss);
    let stage_off_b = u32_binop(builder, spirv::Op::IAdd, sub_b_base, stage_off_b_raw);
    // Anti-WAR refill B offset: write the OTHER stage (1-s) — see the A
    // refill comment above.
    let stage_off_b_refill_raw = u32_binop(builder, spirv::Op::IMul, refill_stage, c_b_ss);
    let stage_off_b_refill = u32_binop(builder, spirv::Op::IAdd, sub_b_base, stage_off_b_refill_raw);
    let mut frag_bs: Vec<Word> = Vec::new();
    for pi in 0..pps as usize {
        let pi_c = u32_const(builder, (pi * 4 * 256) as u32);
        let panel_off = u32_binop(builder, spirv::Op::IAdd, stage_off_b, pi_c);
        for j in 0..4usize {
            let j_c = u32_const(builder, (j * 256) as u32);
            let base = u32_binop(builder, spirv::Op::IAdd, panel_off, j_c);
            let ptr = builder.gen_id();
            if pairs {
                // vNf16 smem view: walk [unit_idx, 0] to a half pointer
                // (unit = pair or quad; the D3b quad smem is v4f16).
                let c0 = u32_const(builder, 0);
                let vw = if quad { 4 } else { 2 };
                let vw_c = u32_const(builder, vw);
                let unit_base = u32_binop(builder, spirv::Op::UDiv, base, vw_c);
                let half_ptr = builder.ptr_class(StorageClass::Workgroup, f16_ty);
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(half_ptr), Some(ptr),
                    vec![Operand::IdRef(smem_b), Operand::IdRef(unit_base), Operand::IdRef(c0)],
                ));
            } else {
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(f16_wg_ptr), Some(ptr),
                    vec![Operand::IdRef(smem_b), Operand::IdRef(base)],
                ));
            }
            let frag = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixLoadKHR, Some(cm_b16), Some(frag),
                vec![Operand::IdRef(ptr), Operand::IdRef(layout_row), Operand::IdRef(s16c)],
            ));
            frag_bs.push(frag);
        }
    }

    // Smem refill for stage s: the panels kt+pps..kt+2pps-1, clamped at
    // last (the clamped duplicates are never loaded — the pair loop's
    // bound is groups/pps). The panel metadata computes ONCE here; the
    // D2 prefetch (below) and the post-barrier stores both consume it.
    let refill_meta: Vec<(Word, Word, Word)> = (0..pps)
        .map(|pi| {
            // The panel index is RUNTIME: (kt_pair + 1)*pps + pi. The refill
            // targets the buffer that the load at iteration kt+1 reads
            // (the anti-WAR refill writes the OTHER stage 1-s, so it fills
            // the panels of the very next iteration — the double-buffer
            // fills the same-parity stage kt+2, the single-buffer fills
            // the one stage. The +1, never +stages: writing the other
            // stage means the next read is one iteration away.)
            // 2026-09-05: the index wraps with UMod groups — the old
            // clamp compared panel units against a ×16 constant (never
            // fired for pps=1), so the tail iteration filled a panel
            // index past K (a benign-but-dirty OOB read into a
            // never-read stage).
            let raw = {
                let one = u32_const(builder, 1);
                let pps_c = u32_const(builder, pps);
                let pi_c = u32_const(builder, pi as u32);
                let pair1 = u32_binop(builder, spirv::Op::IAdd, kt_u, one);
                let base = u32_binop(builder, spirv::Op::IMul, pair1, pps_c);
                let raw = u32_binop(builder, spirv::Op::IAdd, base, pi_c);
                if stagger {
                    u32_binop(builder, spirv::Op::IAdd, raw, start_panel)
                } else {
                    raw
                }
            };
            let kt_fill = u32_binop(builder, spirv::Op::UMod, raw, groups_c);
            let a_off_c = u32_const(builder, (pi as usize * a_panel_stride) as u32);
            let a_off = u32_binop(builder, spirv::Op::IAdd, stage_off_a_refill, a_off_c);
            let b_off_c = u32_const(builder, (pi as usize * 4 * 256) as u32);
            let b_off = u32_binop(builder, spirv::Op::IAdd, stage_off_b_refill, b_off_c);
            (kt_fill, a_off, b_off)
        })
        .collect();

    // D2 register-prefetch: the refill's DRAM loads issue into
    // registers BEFORE the mma chain — the DRAM system is idle during
    // the mma (it reads smem only), so the strided global reads hide
    // under tensor work. The values sit in registers across the first
    // barrier; the smem stores land after it.
    let refill_vals: Vec<Vec<Word>> = if pairs && prefetch {
        refill_meta
            .iter()
            .map(|&(kt_fill, _, _)| emit_fill_load_phase(builder, &fill_params, kt_fill))
            .collect()
    } else {
        Vec::new()
    };

    // MMA: every panel's (r,j) products accumulate into the SAME 16
    // accs — the k-dimension sums across the panels of the step. The
    // panels SERIALIZE per accumulator: phi → mma(p0) → mma(p1) →
    // back-edge (only the last panel's mma defines the back-edge id;
    // two definitions of the same id = the spirv-val duplicate trap).
    let mut cur: Vec<Word> = acc_phis_live.clone();
    for pi in 0..pps as usize {
        for r in 0..tile_rows as usize {
            for j in 0..4usize {
                let idx = r * 4 + j;
                let res = if pi == pps as usize - 1 {
                    acc_backedges[idx]
                } else {
                    builder.gen_id()
                };
                let frag_a = frag_as[pi * tile_rows as usize + r];
                let frag_b = frag_bs[pi * 4 + j];
                builder.emit(Instruction::new(
                    spirv::Op::CooperativeMatrixMulAddKHR, Some(acc_cm),
                    Some(res),
                    vec![
                        Operand::IdRef(frag_a),
                        Operand::IdRef(frag_b),
                        Operand::IdRef(cur[idx]),
                    ],
                ));
                cur[idx] = res;
            }
        }
    }

    // Smem refill landing: with the D2 prefetch the values are already
    // in registers — only the smem stores remain. Without it the fused
    // fill (DRAM loads + stores) runs here.
    // 2026-09-07 (anti-WAR refill): the refill writes the OTHER stage
    // (1-s) — it cannot race the in-flight CM loads of stage s — so the
    // workgroup barrier between the mma and the refill is dropped for the
    // FUSED double-buffer fill. The D2 prefetch KEEPS the barrier:
    // measured 4.55ms with vs 4.80ms without — the barrier lets the mma
    // issue cleanly before the store phase contends for issue slots (the
    // DRAM latency is already hidden by the loop-top loads, so the
    // barrier costs nothing there). The single-buffer (stages=1) fills
    // the ONE stage the loads just read: the barrier stays (WAR).
    {
        if prefetch || stages == 1 {
            emit_wg_barrier(builder);
        }

        if prefetch {
            for (vals, &(_, a_off, b_off)) in refill_vals.iter().zip(refill_meta.iter()) {
                emit_fill_store_phase(builder, &fill_params, vals, a_off, b_off);
            }
        } else {
            for &(kt_fill, a_off, b_off) in &refill_meta {
                emit_smem_fill(builder, &fill_params, a_off, b_off, kt_fill);
            }
        }
        emit_wg_barrier(builder);
    }

    end_structured_loop(builder, &sig, &bbs, kt_phi, kt_backedge, cond_next)?;

    // Store.
    let y_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
    let y_member_c = u32_const(builder, y_member);
    for (r, row_base) in a_row_bases.iter().enumerate() {
        let rc = u32_const(builder, (r * 16) as u32);
        let row = u32_binop(builder, spirv::Op::IAdd, band_m16, rc);
        let c_row = u32_binop(builder, spirv::Op::IMul, row, n_stride);
        for j in 0..4 {
            let phi = acc_phis_live[r * 4 + j];
            let frag_out = if f16acc { phi } else {
                let fo = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::FConvert, Some(cm_c16), Some(fo),
                    vec![Operand::IdRef(phi)],
                ));
                fo
            };
            let jm = u32_const(builder, 16);
            let ji = u32_const(builder, j as u32);
            let jcol = u32_binop(builder, spirv::Op::IMul, jm, ji);
            let col_j = u32_binop(builder, spirv::Op::IAdd, tn64, jcol);
            let c_off = u32_binop(builder, spirv::Op::IAdd, c_row, col_j);
            let y_ptr = builder.gen_id();
            if pairs {
                // The y member carries the v2f16 view: walk [pair_idx, 0]
                // to a half pointer (c_off even: N and the tile cols even).
                let c2 = u32_const(builder, 2);
                let c0 = u32_const(builder, 0);
                let v2_half_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
                let pair_idx = u32_shr(builder, c_off, 1);
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(v2_half_ptr), Some(y_ptr),
                    vec![Operand::IdRef(ssbo), Operand::IdRef(y_member_c),
                         Operand::IdRef(pair_idx), Operand::IdRef(c0)],
                ));
            } else {
                builder.emit(Instruction::new(
                    spirv::Op::AccessChain, Some(y_elem_ptr), Some(y_ptr),
                    vec![Operand::IdRef(ssbo), Operand::IdRef(y_member_c), Operand::IdRef(c_off)],
                ));
            }
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixStoreKHR, None, None,
                vec![Operand::IdRef(y_ptr), Operand::IdRef(frag_out),
                     Operand::IdRef(layout_row), Operand::IdRef(n_stride)],
            ));
        }
    }

    builder.builder.branch(exit_bb);
    Ok(())
}

pub(crate) fn emit_coopmat(
    builder: &mut super::SpirvBuilder,
    plan: &GemmPlan,
    io: &CoopMatIo,
    exit_bb: Word,
) -> Result<(), String> {
    if io.shared_a.is_some() && io.shared_b.is_some() {
        return emit_coopmat_smem(builder, plan, io,
            GemmPlan::coopmat_tile_rows(plan.m), exit_bb);
    }
    use rspirv::spirv::Capability;
    let (ssbo, wgid, a_member, b_member, y_member) =
        (io.ssbo, io.wgid, io.a_member, io.b_member, io.y_member);

    // Module preamble: capabilities, extensions, Vulkan memory model.
    // (Shader is already declared by SpirvBuilder::new.)
    builder.builder.capability(Capability::CooperativeMatrixKHR);
    builder.builder.capability(Capability::VulkanMemoryModel);
    builder.builder.capability(Capability::StorageBuffer16BitAccess);
    // The mma is ARITHMETIC over f16 fragments — shaderFloat16 surface.
    // (Storage alone would only need 16bit_storage.)
    builder.builder.capability(Capability::Float16);
    builder.builder.extension("SPV_KHR_cooperative_matrix");
    builder.builder.extension("SPV_KHR_vulkan_memory_model");
    builder.builder.extension("SPV_KHR_16bit_storage");
    builder.builder.module_mut().memory_model = Some(rspirv::dr::Instruction::new(
        spirv::Op::MemoryModel,
        None,
        None,
        vec![
            Operand::LiteralBit32(spirv::AddressingModel::Logical as u32),
            Operand::LiteralBit32(spirv::MemoryModel::Vulkan as u32),
        ],
    ));

    let int_ty = builder.lower_type(&crate::ast::Type::int())?;
    let bool_ty = builder.lower_type(&crate::ast::Type::Bits(1))?;
    let f32_ty = builder.builder.type_float(32);
    let f16_ty = builder.builder.type_float(16);
    let u32_ty = builder.u32_type();

    // Cooperative matrix fragment types: A(f16) B(f16) C(f32), subgroup
    // scope, 16×16 — a supported shape on coopmat-capable NVIDIA GPUs
    // (vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR, M2.2 plan).
    // Rows/Columns/Use/Scope must be CONSTANT-INSTRUCTION ids.
    let scope_sub = u32_const(builder, 3); // ScopeSubgroup
    let use_a = u32_const(builder, 0);
    let use_b = u32_const(builder, 1);
    let use_c = u32_const(builder, 2);
    let dim16 = u32_const(builder, 16);
    let cm_a16 = builder.builder.type_cooperative_matrix_khr(
        f16_ty, scope_sub, dim16, dim16, use_a);
    let cm_b16 = builder.builder.type_cooperative_matrix_khr(
        f16_ty, scope_sub, dim16, dim16, use_b);
    let cm_c32 = builder.builder.type_cooperative_matrix_khr(
        f32_ty, scope_sub, dim16, dim16, use_c);
    let cm_c16 = builder.builder.type_cooperative_matrix_khr(
        f16_ty, scope_sub, dim16, dim16, use_c);
    // 2026-09-02 (B3): the f16-acc tier accumulates IN f16 — the mma runs
    // double-pumped on Ampere and the store needs no OpFConvert (the field
    // is f16 already). The f32-acc path is byte-identical to before.
    let f16acc = GemmPlan::coopmat_f16acc();
    let sub_check = GemmPlan::coopmat_subgroups();
    eprintln!("DBG knobs: f16acc={} subgroups={} pps={} pairs={}", f16acc, sub_check,
        GemmPlan::coopmat_panels_per_stage(plan.k), GemmPlan::coopmat_fill_pairs());
    let acc_cm = if f16acc { cm_c16 } else { cm_c32 };

    // Memory layout constant: RowMajorKHR = 0.
    let layout_row = builder.u32_const(0);

    // Grid decode: R 16-row strips × 64 cols of output per workgroup
    // (R = coopmat_tile_rows — the B-reuse rung). X-flattened
    // workgroups: wgx = tile_my * tiles_x + tile_n — the ROW band is the
    // MAJOR (dividend), the COL tile the MINOR (modulo). 2026-09-02: the
    // decode had them swapped (tile_n = UDiv, tile_m = UMod), so
    // tile_m wrapped every tiles_x workgroups and tile_n ran to
    // workgroups/tiles_x — only tiles_x/… of the tile-rows were ever
    // computed (256 tile-rows → 64 = 25% of the output: THE "~25% y-fill
    // fault" of the M2.2 session, misread then as a driver/NVVM issue),
    // and tile_n ≥ N/64 re-wrote earlier rows with B strips read past
    // the output width (garbage over correct tiles). Undo: swap the two
    // ops back.
    let tile_rows = GemmPlan::coopmat_tile_rows(plan.m);
    // B2 (cuda-race): S subgroups per workgroup — the X-flatten divisor is
    // tiles_x' = N/(64*S) and each SUBGROUP owns its own tile_n slice
    // (adjacent N tiles share the A panel rows: L1/L2 reuse).
    let subgroups = GemmPlan::coopmat_subgroups();
    let tiles_x = (plan.n / (64 * subgroups as i64)) as u32;
    let wgx = builtin_comp_u(builder, wgid, 0);
    let tiles_x_c = u32_const(builder, tiles_x);
    let tile_my = u32_binop(builder, spirv::Op::UDiv, wgx, tiles_x_c);
    let tile_n = if subgroups > 1 {
        let tn_base = u32_binop(builder, spirv::Op::UMod, wgx, tiles_x_c);
        let s_c = u32_const(builder, subgroups);
        let tn_mul = u32_binop(builder, spirv::Op::IMul, tn_base, s_c);
        u32_binop(builder, spirv::Op::IAdd, tn_mul, io.sub_id)
    } else {
        u32_binop(builder, spirv::Op::UMod, wgx, tiles_x_c)
    };
    let s16 = u32_const(builder, 16);
    let s64 = u32_const(builder, 64);
    // The workgroup's FIRST 16-row strip: (tile_my * R) * 16.
    let r_rows_c = u32_const(builder, tile_rows);
    let band_m = u32_binop(builder, spirv::Op::IMul, tile_my, r_rows_c);
    let band_m16 = u32_binop(builder, spirv::Op::IMul, band_m, s16);
    let tn64 = u32_binop(builder, spirv::Op::IMul, tile_n, s64);
    let nk = u32_const(builder, plan.k as u32);
    // Per-strip A row base: (band_m16 + r*16) * K, r = 0..R.
    let a_row_bases: Vec<Word> = (0..tile_rows)
        .map(|r| {
            let rc = u32_const(builder, (r * 16) as u32);
            let row = u32_binop(builder, spirv::Op::IAdd, band_m16, rc);
            u32_binop(builder, spirv::Op::IMul, row, nk)
        })
        .collect();

    // f32 accumulator fragments, zero-initialized (OpConstantComposite with
    // one scalar constituent fills the whole matrix). The accumulators are
    // LOOP PHIs — the mma defines the pre-reserved back-edge id (coopmat
    // types allocate in Function/Private storage only; the phi carries the
    // value across iterations without a Function variable). R strips ×
    // 4 column fragments each.
    let zero_f = builder.float_const(if f16acc { 16 } else { 32 }, 0.0);
    let acc_zero = builder.builder.constant_composite(acc_cm, vec![zero_f]);

    let acc_count = tile_rows * 4;
    let acc_backedges: Vec<Word> = (0..acc_count).map(|_| builder.gen_id()).collect();
    let acc_inits: Vec<Word> = (0..acc_count).map(|_| acc_zero).collect();
    let acc_phis: Vec<(Word, Word, Word)> = acc_backedges
        .iter()
        .zip(acc_inits.iter())
        .map(|(&be, &init)| (acc_cm, init, be))
        .collect();
    let sig = CoopLoopSig {
        int_ty,
        bool_ty,
        groups: (plan.k / 16) as i64,
    };
    let (bbs, acc_ids, cond_next, _cond0, kt_phi, kt_backedge) =
        begin_structured_loop(builder, &sig, &acc_phis)?;
    let acc_phis_live: Vec<Word> = acc_ids.clone();

    // The induction variable is i64 (the shared loop machinery); the panel
    // offset math is u32 — truncate once at the loop head.
    let kt_u = {
        let id = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::UConvert,
            Some(u32_ty),
            Some(id),
            vec![Operand::IdRef(kt_phi)],
        ));
        id
    };

    // A fragments: A[band_m16 + r*16 + (0..15)][kt*16 + (0..15)] — pointer
    // to the first element (half storage), stride K (in half elements),
    // row-major. R strips per workgroup.
    let a_member_c = u32_const(builder, a_member);
    let b_member_c = u32_const(builder, b_member);
    let y_member_c = u32_const(builder, y_member);
    let k_stride = u32_const(builder, plan.k as u32);
    let n_stride = u32_const(builder, plan.n as u32);
    // The panel's k offset is kt*16 — kt alone overlaps panels (the
    // wrong-slice fingerprint on 4096^3).
    let s16c = u32_const(builder, 16);
    let kt16 = u32_binop(builder, spirv::Op::IMul, kt_u, s16c);
    let a_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
    // R A fragments — one per strip. Each is reused across the 4 B
    // fragments of its strip.
    let frag_as: Vec<Word> = a_row_bases
        .iter()
        .map(|row_base| {
            let a_off = u32_binop(builder, spirv::Op::IAdd, *row_base, kt16);
            let a_ptr = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::AccessChain,
                Some(a_elem_ptr),
                Some(a_ptr),
                vec![
                    Operand::IdRef(ssbo),
                    Operand::IdRef(a_member_c),
                    Operand::IdRef(a_off),
                ],
            ));
            let frag_a = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixLoadKHR,
                Some(cm_a16),
                Some(frag_a),
                vec![
                    Operand::IdRef(a_ptr),
                    Operand::IdRef(layout_row),
                    Operand::IdRef(k_stride),
                ],
            ));
            frag_a
        })
        .collect();

    // B fragments j=0..3: columns tn*64 + j*16 .. +16, stride N. Loaded
    // ONCE per workgroup per panel — the whole point of R > 1: each B
    // fragment feeds R mma chains (B traffic ÷ R).
    let b_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
    let b_row = u32_binop(builder, spirv::Op::IMul, kt16, n_stride);
    let mut frag_bs: Vec<Word> = Vec::with_capacity(4);
    for j in 0..4 {
        let jmul = u32_const(builder, 16);
        let jidx = u32_const(builder, j as u32);
        let jcol = u32_binop(builder, spirv::Op::IMul, jmul, jidx);
        let b_col = u32_binop(builder, spirv::Op::IAdd, tn64, jcol);
        let b_off = u32_binop(builder, spirv::Op::IAdd, b_row, b_col);
        let b_ptr = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(b_elem_ptr),
            Some(b_ptr),
            vec![
                Operand::IdRef(ssbo),
                Operand::IdRef(b_member_c),
                Operand::IdRef(b_off),
            ],
        ));
        let frag_b = builder.gen_id();
        builder.emit(Instruction::new(
            spirv::Op::CooperativeMatrixLoadKHR,
            Some(cm_b16),
            Some(frag_b),
            vec![
                Operand::IdRef(b_ptr),
                Operand::IdRef(layout_row),
                Operand::IdRef(n_stride),
            ],
        ));
        frag_bs.push(frag_b);
    }

    // mma: acc[r][j] = A_r × B_j + acc[r][j], INTO the phi's back-edge id.
    for (r, frag_a) in frag_as.iter().enumerate() {
        for (j, frag_b) in frag_bs.iter().enumerate() {
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixMulAddKHR,
                Some(acc_cm),
                Some(acc_backedges[r * 4 + j]),
                vec![
                    Operand::IdRef(*frag_a),
                    Operand::IdRef(*frag_b),
                    Operand::IdRef(acc_phis_live[r * 4 + j]),
                ],
            ));
        }
    }

    end_structured_loop(builder, &sig, &bbs, kt_phi, kt_backedge, cond_next)?;

    // Store: convert the f32 fragments to the field's f16 component
    // (OpFConvert is defined on cooperative matrices) and write with
    // stride N at each strip's tile corner.
    let y_elem_ptr = builder.ptr_class(StorageClass::StorageBuffer, f16_ty);
    for (r, row_base) in a_row_bases.iter().enumerate() {
        // Strip corner row in ELEMENTS: (band_m16 + r*16) * N — reuse the
        // row base's row (÷K) via a fresh multiply by N.
        let rc = u32_const(builder, (r * 16) as u32);
        let row = u32_binop(builder, spirv::Op::IAdd, band_m16, rc);
        let c_row = u32_binop(builder, spirv::Op::IMul, row, n_stride);
        for j in 0..4 {
            let phi = acc_phis_live[r * 4 + j];
            let frag_out = if f16acc {
                // The accumulator IS f16 — store it directly.
                phi
            } else {
                let fo = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::FConvert,
                    Some(cm_c16),
                    Some(fo),
                    vec![Operand::IdRef(phi)],
                ));
                fo
            };
            let jm = u32_const(builder, 16);
            let ji = u32_const(builder, j as u32);
            let jcol = u32_binop(builder, spirv::Op::IMul, jm, ji);
            let col_j = u32_binop(builder, spirv::Op::IAdd, tn64, jcol);
            let c_off = u32_binop(builder, spirv::Op::IAdd, c_row, col_j);
            let y_ptr = builder.gen_id();
            builder.emit(Instruction::new(
                spirv::Op::AccessChain,
                Some(y_elem_ptr),
                Some(y_ptr),
                vec![
                    Operand::IdRef(ssbo),
                    Operand::IdRef(y_member_c),
                    Operand::IdRef(c_off),
                ],
            ));
            builder.emit(Instruction::new(
                spirv::Op::CooperativeMatrixStoreKHR,
                None,
                None,
                vec![
                    Operand::IdRef(y_ptr),
                    Operand::IdRef(frag_out),
                    Operand::IdRef(layout_row),
                    Operand::IdRef(n_stride),
                ],
            ));
        }
    }

    builder.builder.branch(exit_bb);
    Ok(())
}

/// Probe helper: the accumulator-class coopmat type (use=C) over half.
fn cm_c16_probe(
    builder: &mut super::SpirvBuilder,
    f16_ty: Word,
    scope_sub: Word,
    dim16: Word,
    use_c: Word,
) -> Word {
    builder.builder.type_cooperative_matrix_khr(f16_ty, scope_sub, dim16, dim16, use_c)
}
