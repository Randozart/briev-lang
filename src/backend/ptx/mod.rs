//! PTX backend — Briev-owned CUDA codegen (plan 2026-09-08-ptx-tier-execution).
//!
//! S2a scope (this module today): GEMM-shaped `.abv` kernels only — the
//! plan's risk gate ("GEMM family ONLY until S5 passes"). Each kernel is a
//! naive per-element GEMM in PTX text; the blob rides the SAME
//! `RunnerKernel` shape the SPIR-V runner consumes (the blob is opaque to
//! the runtime — the CUDA driver JITs it via cuModuleLoadData). The emitter
//! hardcodes the device projection offsets from `ssbo_layout` — the ONE
//! layout rule — so the kernel and the runner's field table can never
//! drift (the SPIR-V backend's invariant, reused here).
//!
//! Not yet: cp.async multi-stage (S3b+), multi-warp CTA, register blocking
//! (S3b+ occupancy rungs), non-GEMM kernels. Each arrives as a first-class
//! emitter arm with tests.

use crate::ast::{Expr, Statement, TopLevel, Type};
use crate::backend::spirv::gemm::GemmPlan;
use crate::backend::spirv::runner::RunnerKernel;
use crate::type_universe::TypeUniverse;

pub mod pipeline;
pub mod tensor;


/// 2026-09-14 (gpu_schedule S5-lite): the general elementwise PTX kernel
/// emitter — non-GEMM eligible nodes (the row-ops between GEMMs in an
/// attention-decode graph). See general.rs.
pub mod general;

/// PTX entry point name — the CUDA driver's `create_kernel` resolves "main"
/// (`cuModuleGetFunction`). Must never drift from `briev_dev_cuda.c`.
const ENTRY: &str = "main";

/// Naive GEMM PTX: work item `i` computes `y[i] = sum_k a[m*K+k] * b[k*N+n]`
/// with `m = i/N`, `n = i%N`. Flat 1D grid (block 64, grid ceil(items/64))
/// — the runner's `dispatch_geometry_stmt` flat form. `a_off`/`b_off`/`y_off`
/// are the DEVICE projection offsets (from `ssbo_layout`); `elem_bytes` is
/// the array element size (4 = f32 — the only S2a operand).
fn naive_gemm_ptx(m: i64, n: i64, k: i64, elem_bytes: u32,
                  a_off: u64, b_off: u64, y_off: u64,
                  epilogue_scale: Option<f64>) -> String {
    let items = m * n;
    let mut out = String::new();
    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
    out.push_str(&format!(".visible .entry {} (.param .b64 proj_param)\n{{\n", ENTRY));
    out.push_str("    .reg .u64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7;\n");
    out.push_str("    .reg .f32  %f1, %f2, %f3, %f4;\n");
    out.push_str("    .reg .pred %p1, %p2;\n");
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    out.push_str("    mov.u32 %r1, %ctaid.x;\n");
    out.push_str("    mov.u32 %r2, %tid.x;\n");
    out.push_str("    mul.lo.u32 %r3, %r1, 64;\n");
    out.push_str("    add.u32 %r3, %r3, %r2;\n");           // i
    out.push_str(&format!("    setp.ge.u32 %p1, %r3, {};\n", items));
    out.push_str("    @%p1 ret;\n");
    out.push_str(&format!("    div.u32 %r4, %r3, {};\n", n)); // m
    out.push_str(&format!("    rem.u32 %r5, %r3, {};\n", n)); // n
    out.push_str("    mov.f32 %f1, 0f00000000;\n");            // acc = 0
    out.push_str("    mov.u32 %r6, 0;\n");                     // k
    out.push_str(&format!("    add.u64 %rd2, %rd1, {};\n", a_off));
    out.push_str(&format!("    add.u64 %rd3, %rd1, {};\n", b_off));
    out.push_str(&format!("    add.u64 %rd4, %rd1, {};\n", y_off));
    out.push_str("LOOP:\n");
    out.push_str(&format!("    setp.ge.u32 %p2, %r6, {};\n", k));
    out.push_str("    @%p2 bra EXIT;\n");
    // a[m*K + k]
    out.push_str(&format!("    mul.lo.u32 %r7, %r4, {};\n", k));
    out.push_str("    add.u32 %r7, %r7, %r6;\n");
    out.push_str(&format!("    mul.wide.u32 %rd5, %r7, {};\n", elem_bytes));
    out.push_str("    add.u64 %rd6, %rd2, %rd5;\n");
    out.push_str("    ld.global.f32 %f2, [%rd6];\n");
    // b[k*N + n]
    out.push_str(&format!("    mul.lo.u32 %r7, %r6, {};\n", n));
    out.push_str("    add.u32 %r7, %r7, %r5;\n");
    out.push_str(&format!("    mul.wide.u32 %rd5, %r7, {};\n", elem_bytes));
    out.push_str("    add.u64 %rd6, %rd3, %rd5;\n");
    out.push_str("    ld.global.f32 %f3, [%rd6];\n");
    out.push_str("    mul.f32 %f4, %f2, %f3;\n");
    out.push_str("    add.f32 %f1, %f1, %f4;\n");
    out.push_str("    add.u32 %r6, %r6, 1;\n");
    out.push_str("    bra.uni LOOP;\n");
    out.push_str("EXIT:\n");
    // y[i] = acc  (2026-09-14 gpu_schedule Phase 4a: an epilogue-fused
    // consumer scale multiplies the accumulator before the store — the
    // consumer kernel is dropped and writes directly to its output field).
    if let Some(s) = epilogue_scale {
        out.push_str(&format!("    mul.f32 %f1, %f1, {:e};\n", s));
    }
    out.push_str(&format!("    mul.wide.u32 %rd5, %r3, {};\n", elem_bytes));
    out.push_str("    add.u64 %rd6, %rd4, %rd5;\n");
    out.push_str("    st.global.f32 [%rd6], %f1;\n");
    out.push_str("    ret;\n}\n");
    out
}

/// Fused-attention PTX (Phase 4b v1 — the correctness reference; the mma
/// rung follows). ONE kernel computing `o = scale(q · kt) · v` with the
/// scaled S tile staged in SHARED memory — the intermediate never touches
/// HBM (the emergent chain-fusion claim). Block = 512 threads covering
/// `block_rows = 512/kn` rows: phase 1 computes S'[tid] = (Q·Kt)·scale into
/// smem; `bar.sync`; phase 2 computes O = S'·V reading the row from smem.
///
/// General shapes (not attention-specific): `m`/`k1` = the producer GEMM's
/// M/K; `kn` = the S width (producer output cols = the consumer's inner
/// dim); `on` = the consumer's output cols. `a_off`/`b_off`/`v_off`/`o_off`
/// are the DEVICE projection offsets (from `ssbo_layout`).
fn fused_attention_ptx(
    m: i64,
    k1: i64,
    kn: i64,
    on: i64,
    a_off: u64,
    b_off: u64,
    v_off: u64,
    o_off: u64,
    scale: f64,
) -> String {
    const BLOCK: i64 = 512;
    let block_rows = (BLOCK / kn).max(1);
    let _ = block_rows;
    let mut out = String::new();
    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
    out.push_str(&format!(".visible .entry {} (.param .b64 proj_param)\n{{\n", ENTRY));
    // u64: %rd1 = proj base; %rd2 = Q row base; %rd3 = Kt col base;
    //      %rd4 = V col base; %rd5 = O addr; %rd6 = address scratch.
    // u32: %r1 = ctaid; %r2 = tid; %r3 = local_m; %r4 = col; %r5 = global_m;
    //      %r6 = kk; %r7 = elem scratch; %r8 = elem scratch; %r9 = byte idx.
    // f32: %f1 = acc; %f2/%f3 = operands; %f4 = product.
    out.push_str("    .reg .u64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9;\n");
    out.push_str("    .reg .f32  %f1, %f2, %f3, %f4;\n");
    out.push_str("    .reg .b16  %b1, %b2;\n");
    out.push_str("    .reg .pred %p1, %p2, %p3;\n");
    out.push_str("    .shared .b16 s[512];\n");
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    out.push_str("    mov.u32 %r1, %ctaid.x;\n");
    out.push_str("    mov.u32 %r2, %tid.x;\n");
    // local_m = tid / kn ; col = tid % kn.
    out.push_str(&format!("    div.u32 %r3, %r2, {};\n", kn));
    out.push_str(&format!("    rem.u32 %r4, %r2, {};\n", kn));
    // global_m = ctaid.x * block_rows + local_m.
    out.push_str(&format!("    mul.lo.u32 %r5, %r1, {};\n", block_rows));
    out.push_str("    add.u32 %r5, %r5, %r3;\n");
    // Guards: p1 = global_m >= m ; p2 = col >= on.
    out.push_str(&format!("    setp.ge.u32 %p1, %r5, {};\n", m));
    out.push_str(&format!("    setp.ge.u32 %p2, %r4, {};\n", on));
    // ---- Phase 1: S'[tid] = (Q·Kt)·scale ----
    out.push_str("    mov.f32 %f1, 0f00000000;\n");
    out.push_str("    mov.u32 %r6, 0;\n");
    // Q row base = proj + a_off + global_m*(k1*2).
    out.push_str("    mov.u64 %rd2, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd2, %rd2, {};\n", a_off));
    out.push_str(&format!("    mul.wide.u32 %rd6, %r5, {};\n", k1 * 2));
    out.push_str("    add.u64 %rd2, %rd2, %rd6;\n");
    // Kt col base = proj + b_off + col*2.
    out.push_str("    mov.u64 %rd3, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", b_off));
    out.push_str("    mul.wide.u32 %rd6, %r4, 2;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd6;\n");
    out.push_str("L1:\n");
    out.push_str(&format!("    setp.ge.u32 %p3, %r6, {};\n", k1));
    out.push_str("    @%p3 bra L1X;\n");
    // Q[global_m*k1 + kk] — addr = rd2 + kk*2.
    out.push_str("    mul.wide.u32 %rd6, %r6, 2;\n");
    out.push_str("    add.u64 %rd6, %rd2, %rd6;\n");
    out.push_str("    ld.global.b16 %b1, [%rd6];\n");
    out.push_str("    cvt.f32.f16 %f2, %b1;\n");
    // Kt[kk*kn + col] — addr = rd3 + kk*kn*2.
    out.push_str(&format!("    mul.wide.u32 %rd6, %r6, {};\n", kn * 2));
    out.push_str("    add.u64 %rd6, %rd3, %rd6;\n");
    out.push_str("    ld.global.b16 %b2, [%rd6];\n");
    out.push_str("    cvt.f32.f16 %f3, %b2;\n");
    out.push_str("    mul.f32 %f4, %f2, %f3;\n");
    out.push_str("    add.f32 %f1, %f1, %f4;\n");
    out.push_str("    add.u32 %r6, %r6, 1;\n");
    out.push_str("    bra.uni L1;\n");
    out.push_str("L1X:\n");
    out.push_str(&format!("    mul.f32 %f1, %f1, {:e};\n", scale));
    out.push_str("    cvt.rn.f16.f32 %b1, %f1;\n");
    // Store S'[tid] at smem byte 2*tid (predicated on global_m < m);
    // bar.sync is UNCONDITIONAL (all threads must reach it).
    out.push_str("    @%p1 bra L1S;\n");
    out.push_str("    mul.lo.u32 %r9, %r2, 2;\n");
    out.push_str("    st.shared.b16 [%r9], %b1;\n");
    out.push_str("L1S:\n");
    out.push_str("    bar.sync 0;\n");
    // ---- Phase 2: O[global_m*on + col] = S'[local_m*kn + kk]·V[kk*on + col] ----
    out.push_str("    @%p1 ret;\n");
    out.push_str("    @%p2 ret;\n");
    out.push_str("    mov.f32 %f1, 0f00000000;\n");
    out.push_str("    mov.u32 %r6, 0;\n");
    // S' row base (smem element idx) = local_m*kn.
    out.push_str(&format!("    mul.lo.u32 %r7, %r3, {};\n", kn));
    // V col base = proj + v_off + col*2.
    out.push_str("    mov.u64 %rd4, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd4, %rd4, {};\n", v_off));
    out.push_str("    mul.wide.u32 %rd6, %r4, 2;\n");
    out.push_str("    add.u64 %rd4, %rd4, %rd6;\n");
    out.push_str("L2L:\n");
    out.push_str(&format!("    setp.ge.u32 %p3, %r6, {};\n", kn));
    out.push_str("    @%p3 bra L2X;\n");
    // S'[local_m*kn + kk] — smem byte 2*(r7 + kk).
    out.push_str("    add.u32 %r8, %r7, %r6;\n");
    out.push_str("    mul.lo.u32 %r9, %r8, 2;\n");
    out.push_str("    ld.shared.b16 %b2, [%r9];\n");
    out.push_str("    cvt.f32.f16 %f2, %b2;\n");
    // V[kk*on + col] — addr = rd4 + kk*on*2.
    out.push_str(&format!("    mul.wide.u32 %rd6, %r6, {};\n", on * 2));
    out.push_str("    add.u64 %rd6, %rd4, %rd6;\n");
    out.push_str("    ld.global.b16 %b2, [%rd6];\n");
    out.push_str("    cvt.f32.f16 %f3, %b2;\n");
    out.push_str("    mul.f32 %f4, %f2, %f3;\n");
    out.push_str("    add.f32 %f1, %f1, %f4;\n");
    out.push_str("    add.u32 %r6, %r6, 1;\n");
    out.push_str("    bra.uni L2L;\n");
    out.push_str("L2X:\n");
    out.push_str("    cvt.rn.f16.f32 %b1, %f1;\n");
    // O[global_m*on + col] — addr = proj + o_off + (global_m*on + col)*2.
    out.push_str(&format!("    mul.lo.u32 %r8, %r5, {};\n", on));
    out.push_str("    add.u32 %r8, %r8, %r4;\n");
    out.push_str("    mul.wide.u32 %rd6, %r8, 2;\n");
    out.push_str("    mov.u64 %rd5, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd5, %rd5, {};\n", o_off));
    out.push_str("    add.u64 %rd5, %rd5, %rd6;\n");
    out.push_str("    st.global.b16 [%rd5], %b1;\n");
    out.push_str("    ret;\n}\n");
    out
}

/// Fused-attention MMA PTX (Phase 4b mma rung). ONE kernel computing
/// `o = scale(q · kt) · v` on the m16n8k16 tensor cores, with the scaled S
/// tile staged in SHARED memory (never HBM). Block = one 32-lane warp per
/// 16-row m-tile (grid = M/16). Phase 1 computes S' = Q·Kt·scale into smem
/// via mma (direct global fragment loads); `bar.sync`; phase 2 computes
/// o = S'·V via mma (A fragments read from smem).
///
/// Fragment layouts (m16n8k16): A(16×16) a0..a3 = rows {t/4, t/4+8} × k
/// {2t', 2t'+1} in k-halves {0, 8} (t' = t%4); B(16×8) b0,b1 = k
/// {2t', 2t'+1} / {2t'+8, 2t'+9} × col t/4; D(16×8) c0..c3 = rows
/// {t/4, t/4+8} × cols {2t', 2t'+1}.
fn fused_attention_mma_ptx(
    m: i64,
    k1: i64,
    kn: i64,
    on: i64,
    a_off: u64,
    b_off: u64,
    v_off: u64,
    o_off: u64,
    scale: f64,
    nwarps: usize,
) -> String {
    debug_assert!(m % 16 == 0 && k1 % 16 == 0 && kn % 16 == 0 && on % 8 == 0);
    debug_assert!(kn % 8 == 0 && (kn as usize / 8) % nwarps == 0 && (on as usize / 8) % nwarps == 0);
    let smem_bytes = 16 * kn * 2;
    let phase1_per = (kn as usize / 8) / nwarps;
    let phase2_per = (on as usize / 8) / nwarps;
    let mut out = String::new();
    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
    out.push_str(&format!(".visible .entry {} (.param .b64 proj_param)\n{{\n", ENTRY));
    out.push_str("    .reg .u64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6, %rd7;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9, %r10, %r11, %r12, %r13;\n");
    out.push_str("    .reg .b32  %a0, %a1, %a2, %a3, %b0, %b1, %t0, %t1;\n");
    out.push_str("    .reg .f32  %c0, %c1, %c2, %c3;\n");
    out.push_str("    .reg .pred %p1;\n");
    out.push_str(&format!("    .shared .align 16 .b8 s[{}];\n", smem_bytes));
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    out.push_str("    mov.u32 %r1, %tid.x;\n");
    out.push_str("    mov.u32 %r2, %ctaid.y;\n"); // m_tile
    out.push_str("    and.b32 %r1, %r1, 31;   // lane (fragment math is per-warp)\n");
    out.push_str("    shr.u32 %r3, %r1, 2;      // lane/4 (row)\n");
    out.push_str("    and.b32 %r4, %r1, 3;      // lane%4 (col)\n");
    out.push_str("    shl.b32 %r5, %r4, 1;      // 2t'\n");
    out.push_str("    mov.u32 %r13, %tid.x;\n");
    out.push_str("    shr.u32 %r9, %r13, 5;   // warp\n");
    out.push_str("    mov.u64 %rd5, s;\n");

    // ---- Phase 1: S'[16][kn] = Q·Kt·scale, warp w owns n-subs
    // [w*phase1_per, (w+1)*phase1_per). Runtime n_sub loop.
    out.push_str(&format!("    mul.lo.u32 %r10, %r9, {};\n", phase1_per)); // n_sub = warp*per
    out.push_str(&format!("    add.u32 %r11, %r10, {};\n", phase1_per));   // n_sub_end
    out.push_str("P1L:\n");
    out.push_str("    setp.ge.u32 %p1, %r10, %r11;\n");
    out.push_str("    @%p1 bra P1X;\n");
    out.push_str("    mov.f32 %c0, 0f00000000; mov.f32 %c1, 0f00000000; mov.f32 %c2, 0f00000000; mov.f32 %c3, 0f00000000;\n");
    for kstep in 0..(k1 / 16) {
        // A fragment base: Q[m_tile*16 + row][kstep*16 + k].
        out.push_str("    mov.u32 %r6, %r2;\n");
        out.push_str(&format!("    mul.lo.u32 %r6, %r6, {};\n", 16 * k1 * 2));
        out.push_str(&format!("    mul.lo.u32 %r7, %r3, {};\n", k1 * 2));
        out.push_str("    add.u32 %r6, %r6, %r7;\n");
        out.push_str("    mul.lo.u32 %r7, %r5, 2;\n");
        out.push_str(&format!("    add.u32 %r7, %r7, {};\n", kstep * 16 * 2));
        out.push_str("    add.u32 %r6, %r6, %r7;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, %rd1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", a_off));
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.global.b32 %a0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    ld.global.b32 %a1, [%rd3];\n");
        out.push_str(&format!("    sub.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    add.u64 %rd3, %rd3, 16;\n");
        out.push_str("    ld.global.b32 %a2, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    ld.global.b32 %a3, [%rd3];\n");
        // B fragments: Kt[(kstep*16 + k)][n_sub*8 + t/4].
        out.push_str("    mov.u32 %r6, %r3;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 2;\n");
        out.push_str("    mov.u32 %r12, %r10;\n");
        out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", 8 * 2)); // n_sub*16
        out.push_str("    add.u32 %r6, %r6, %r12;\n");
        out.push_str(&format!("    mul.lo.u32 %r7, %r5, {};\n", kn * 2));
        out.push_str("    add.u32 %r6, %r6, %r7;\n");
        out.push_str(&format!("    add.u32 %r7, %r6, {};\n", kstep * 16 * kn * 2));
        out.push_str("    mul.wide.u32 %rd2, %r7, 1;\n");
        out.push_str("    mov.u64 %rd3, %rd1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", b_off));
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.global.b16 %t0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", kn * 2));
        out.push_str("    ld.global.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 7 * kn * 2));
        out.push_str("    ld.global.b16 %t0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", kn * 2));
        out.push_str("    ld.global.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c0,%c1,%c2,%c3}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c0,%c1,%c2,%c3}};\n"
        ));
    }
    // Scale + store S' to smem. col = n_sub*8 + 2t'.
    out.push_str(&format!(
        "    mul.f32 %c0, %c0, {:e}; mul.f32 %c1, %c1, {:e}; mul.f32 %c2, %c2, {:e}; mul.f32 %c3, %c3, {:e};\n",
        scale, scale, scale, scale
    ));
    out.push_str("    cvt.rn.f16.f32 %t0, %c0; cvt.rn.f16.f32 %t1, %c1;\n");
    out.push_str("    mov.b32 %a0, %t0; shl.b32 %t1, %t1, 16; or.b32 %a0, %a0, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c2; cvt.rn.f16.f32 %t1, %c3;\n");
    out.push_str("    mov.b32 %a1, %t0; shl.b32 %t1, %t1, 16; or.b32 %a1, %a1, %t1;\n");
    // smem byte = (row*kn + n_sub*8 + 2t')*2.
    out.push_str(&format!("    mul.lo.u32 %r6, %r3, {};\n", kn * 2));
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str("    mul.lo.u32 %r12, %r12, 16;\n");
    out.push_str("    add.u32 %r6, %r6, %r12;\n");
    out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
    out.push_str("    add.u32 %r6, %r6, %r8;\n");
    out.push_str("    st.shared.b32 [%r6], %a0;\n");
    out.push_str(&format!("    add.u32 %r6, %r6, {};\n", 8 * kn * 2));
    out.push_str("    st.shared.b32 [%r6], %a1;\n");
    out.push_str("    add.u32 %r10, %r10, 1;\n");
    out.push_str("    bra.uni P1L;\n");
    out.push_str("P1X:\n");
    out.push_str("    bar.sync 0;\n");

    // ---- Phase 2: o[16][on] = S'·V, warp w owns n-subs [w*phase2_per, ...).
    out.push_str(&format!("    mul.lo.u32 %r10, %r9, {};\n", phase2_per));
    out.push_str(&format!("    add.u32 %r11, %r10, {};\n", phase2_per));
    out.push_str("P2L:\n");
    out.push_str("    setp.ge.u32 %p1, %r10, %r11;\n");
    out.push_str("    @%p1 bra P2X;\n");
    out.push_str("    mov.f32 %c0, 0f00000000; mov.f32 %c1, 0f00000000; mov.f32 %c2, 0f00000000; mov.f32 %c3, 0f00000000;\n");
    for kstep in 0..(kn / 16) {
        // A from S' smem: S'[row][kstep*16 + k].
        out.push_str(&format!("    mul.lo.u32 %r6, %r3, {};\n", kn * 2));
        out.push_str(&format!("    add.u32 %r6, %r6, {};\n", kstep * 16 * 2));
        out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, %rd5;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.shared.b32 %a0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    ld.shared.b32 %a1, [%rd3];\n");
        out.push_str(&format!("    sub.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    add.u64 %rd3, %rd3, 16;\n");
        out.push_str("    ld.shared.b32 %a2, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    ld.shared.b32 %a3, [%rd3];\n");
        // B from V: V[(kstep*16 + k)][n_sub*8 + t/4].
        out.push_str("    mov.u32 %r6, %r3;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 2;\n");
        out.push_str("    mov.u32 %r12, %r10;\n");
        out.push_str("    mul.lo.u32 %r12, %r12, 16;\n");
        out.push_str("    add.u32 %r6, %r6, %r12;\n");
        out.push_str(&format!("    mul.lo.u32 %r7, %r5, {};\n", on * 2));
        out.push_str("    add.u32 %r6, %r6, %r7;\n");
        out.push_str(&format!("    add.u32 %r7, %r6, {};\n", kstep * 16 * on * 2));
        out.push_str("    mul.wide.u32 %rd2, %r7, 1;\n");
        out.push_str("    mov.u64 %rd3, %rd1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", v_off));
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.global.b16 %t0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", on * 2));
        out.push_str("    ld.global.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 7 * on * 2));
        out.push_str("    ld.global.b16 %t0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", on * 2));
        out.push_str("    ld.global.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c0,%c1,%c2,%c3}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c0,%c1,%c2,%c3}};\n"
        ));
    }
    // Store o to global: O[(m_tile*16 + row)][n_sub*8 + col].
    out.push_str("    cvt.rn.f16.f32 %t0, %c0; cvt.rn.f16.f32 %t1, %c1;\n");
    out.push_str("    mov.b32 %a0, %t0; shl.b32 %t1, %t1, 16; or.b32 %a0, %a0, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c2; cvt.rn.f16.f32 %t1, %c3;\n");
    out.push_str("    mov.b32 %a1, %t0; shl.b32 %t1, %t1, 16; or.b32 %a1, %a1, %t1;\n");
    out.push_str(&format!("    mul.lo.u32 %r6, %r2, {};\n", 16 * on * 2));
    out.push_str(&format!("    mul.lo.u32 %r7, %r3, {};\n", on * 2));
    out.push_str("    add.u32 %r6, %r6, %r7;\n");
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str("    mul.lo.u32 %r12, %r12, 16;\n");
    out.push_str("    add.u32 %r6, %r6, %r12;\n");
    out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
    out.push_str("    add.u32 %r6, %r6, %r8;\n");
    out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
    out.push_str("    mov.u64 %rd3, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", o_off));
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    st.global.b32 [%rd3], %a0;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * on * 2));
    out.push_str("    st.global.b32 [%rd3], %a1;\n");
    out.push_str("    add.u32 %r10, %r10, 1;\n");
    out.push_str("    bra.uni P2L;\n");
    out.push_str("P2X:\n");
    out.push_str("    ret;\n}\n");
    out
}

/// Fused-attention MMA PTX, SMEM-STAGED (Phase 4b — the cuBLAS lesson).
/// Same math as `fused_attention_mma_ptx` (m16n8k16, S' on-chip) but the
/// operands are staged through shared memory with COALESCED 16-byte fills
/// and the fragments are read from smem — not scattered direct global
/// loads (the ~10× cuBLAS gap was the load path; cuBLAS = LDG.128 fills →
/// STS.128 → ldmatrix → HMMA). Single-buffered first (correctness); the
/// multi-stage pipeline is the next rung.
///
/// smem: qsmem[16×k1×2] (the Q tile, filled once), bsmem[16×16×2] (the Kt/V
/// panel, reused), ssmem[16×kn×2] (the scaled S' tile, phase 2's A).
fn fused_attention_mma_staged_ptx(
    m: i64,
    k1: i64,
    kn: i64,
    on: i64,
    a_off: u64,
    b_off: u64,
    v_off: u64,
    o_off: u64,
    scale: f64,
    nwarps: usize,
) -> String {
    debug_assert!(m % 16 == 0 && k1 % 16 == 0 && kn % 16 == 0 && on % 8 == 0);
    debug_assert!(kn % 16 == 0 && on % 16 == 0);
    let phase1_per = (kn as usize / 16) / nwarps;
    let phase2_per = (on as usize / 16) / nwarps;
    let mut out = String::new();
    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
    out.push_str(&format!(".visible .entry {} (.param .b64 proj_param)\n{{\n", ENTRY));
    out.push_str("    .reg .u64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6, %rd7, %rd8, %rd9;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9, %r10, %r11, %r12, %r13, %r14;\n");
    out.push_str("    .reg .b32  %a0, %a1, %a2, %a3, %b0, %b1, %t0, %t1;\n");
    out.push_str("    .reg .b32  %f0, %f1, %f2, %f3;\n");
    out.push_str("    .reg .f32  %c0, %c1, %c2, %c3, %c4, %c5, %c6, %c7;\n");
    out.push_str("    .reg .pred %p1;\n");
    out.push_str(&format!("    .shared .align 16 .b8 qsmem[{}];\n", 16 * k1 * 2));
    out.push_str(&format!("    .shared .align 16 .b8 bsmem[{}];\n", nwarps * 512));
    out.push_str(&format!("    .shared .align 16 .b8 ssmem[{}];\n", 16 * kn * 2));
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    out.push_str("    mov.u32 %r1, %tid.x;\n");
    out.push_str("    and.b32 %r1, %r1, 31;      // lane\n");
    out.push_str("    mov.u32 %r2, %ctaid.y;     // m_tile\n");
    out.push_str("    shr.u32 %r3, %r1, 2;       // lane/4 (row)\n");
    out.push_str("    and.b32 %r4, %r1, 3;       // lane%4\n");
    out.push_str("    shl.b32 %r5, %r4, 1;       // 2t'\n");
    out.push_str("    mov.u32 %r13, %tid.x;\n");
    out.push_str("    shr.u32 %r9, %r13, 5;      // warp\n");

    // ---- Fill qsmem: Q[m_tile*16 .. +16][0..k1], coalesced 16-byte.
    // Thread t copies row t/2, col-half t%2: 8 f16 (16 bytes) each; the
    // tile is 16 rows × k1 cols → (16·k1·2)/32 = k1 bytes per lane.
    // Thread t copies bytes [t*128, (t+1)*128) of the contiguous Q tile
    // (16 rows × k1 cols, row-major): base = t*128, 8× 16-byte chunks.
    out.push_str("    mov.u32 %r10, %r1;\n");
    out.push_str(&format!("    mul.lo.u32 %r10, %r10, {};\n", (16 * k1 * 2) / 32)); // t*128
    out.push_str("    mov.u32 %r11, %r2;\n");
    out.push_str(&format!("    mul.lo.u32 %r11, %r11, {};\n", 16 * k1 * 2)); // m_tile*4096
    out.push_str("    add.u32 %r11, %r10, %r11;\n"); // GLOBAL offset
    out.push_str("    mul.wide.u32 %rd2, %r11, 1;\n");
    out.push_str("    mov.u64 %rd3, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", a_off));
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    mov.u64 %rd5, qsmem;\n");
    out.push_str("    mul.wide.u32 %rd6, %r10, 1;\n"); // SMEM offset (t*128 only)
    out.push_str("    add.u64 %rd5, %rd5, %rd6;\n");
    out.push_str(&format!("    mov.u32 %r14, {};\n", ((16 * k1 * 2) / 32) / 16));
    out.push_str("QFILL:\n");
    out.push_str("    setp.eq.u32 %p1, %r14, 0;\n");
    out.push_str("    @%p1 bra QFILLX;\n");
    out.push_str("    ld.global.v4.b32 {%f0,%f1,%f2,%f3}, [%rd3];\n");
    out.push_str("    st.shared.v4.b32 [%rd5], {%f0,%f1,%f2,%f3};\n");
    out.push_str("    add.u64 %rd3, %rd3, 16; add.u64 %rd5, %rd5, 16;\n");
    out.push_str("    sub.u32 %r14, %r14, 1;\n");
    out.push_str("    bra.uni QFILL;\n");
    out.push_str("QFILLX:\n");
    out.push_str("    bar.sync 0;\n");

    // ---- Phase 1: S'[16][kn] = Q·Kt·scale. Warp w owns n-panels
    // [w*phase1_per, (w+1)*phase1_per). n-panel outer, k-panel inner.
    out.push_str(&format!("    mul.lo.u32 %r10, %r9, {};\n", phase1_per));
    out.push_str(&format!("    add.u32 %r11, %r10, {};\n", phase1_per));
    out.push_str("P1L:\n");
    out.push_str("    setp.ge.u32 %p1, %r10, %r11;\n");
    out.push_str("    @%p1 bra P1X;\n");
    out.push_str("    mov.f32 %c0, 0f00000000; mov.f32 %c1, 0f00000000; mov.f32 %c2, 0f00000000; mov.f32 %c3, 0f00000000;\n");
    out.push_str("    mov.f32 %c4, 0f00000000; mov.f32 %c5, 0f00000000; mov.f32 %c6, 0f00000000; mov.f32 %c7, 0f00000000;\n");
    for kpanel in 0..(k1 / 16) {
        // Fill bsmem: Kt[(kpanel*16 + k)][(n_panel*16 + n)], 16×16 tile.
        // Thread t copies row t/2, 8 f16 (16 B) starting at col (t%2)*8.
        out.push_str("    mov.u32 %r6, %r1;\n");
        out.push_str("    shr.u32 %r6, %r6, 1;   // t/2 (k row)\n");
        out.push_str("    and.b32 %r7, %r1, 1;   // t%2\n");
        // global Kt addr.
        out.push_str("    mov.u32 %r12, %r10;\n");
        out.push_str("    mul.lo.u32 %r12, %r12, 16; // n_panel*16\n");
        out.push_str(&format!("    add.u32 %r12, %r12, {};\n", kpanel * 16));
        out.push_str("    add.u32 %r12, %r12, %r6;\n"); // + t/2 (k row)
        out.push_str("    mul.lo.u32 %r12, %r12, 0; // placeholder\n");
        // simpler: byte = b_off + (kpanel*16 + t/2)*kn*2 + (n_panel*16 + (t%2)*8)*2
        out.push_str("    mov.u32 %r12, %r6;\n");
        out.push_str(&format!("    add.u32 %r12, %r12, {};\n", kpanel * 16));
        out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", kn * 2));
        out.push_str("    mov.u32 %r8, %r10;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 32; // n_panel*16*2\n");
        out.push_str("    add.u32 %r12, %r12, %r8;\n");
        out.push_str("    mul.lo.u32 %r8, %r7, 16; // (t%2)*8*2\n");
        out.push_str("    add.u32 %r12, %r12, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r12, 1;\n");
        out.push_str("    mov.u64 %rd3, %rd1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", b_off));
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        // smem target.
        out.push_str("    mov.u64 %rd5, bsmem;\n");
        out.push_str(&format!("    mul.wide.u32 %rd7, %r9, {};\n", 512));
        out.push_str("    add.u64 %rd5, %rd5, %rd7;\n");
        out.push_str("    mov.u32 %r12, %r6;\n");
        out.push_str("    mul.lo.u32 %r12, %r12, 32; // (t/2)*16*2\n");
        out.push_str("    mul.lo.u32 %r8, %r7, 16;\n");
        out.push_str("    add.u32 %r12, %r12, %r8;\n");
        out.push_str("    mul.wide.u32 %rd6, %r12, 1;\n");
        out.push_str("    add.u64 %rd6, %rd5, %rd6;\n");
        out.push_str("    ld.global.v4.b32 {%f0,%f1,%f2,%f3}, [%rd3];\n");
        out.push_str("    st.shared.v4.b32 [%rd6], {%f0,%f1,%f2,%f3};\n");
        out.push_str("    bar.sync 0;\n");
        // A fragments from qsmem, B from bsmem; 2 mma side-by-side.
        // A: a0=(t/4, 2t') a1=(t/4+8, 2t') a2=(t/4, 2t'+8) a3=(t/4+8, 2t'+8)
        //   from qsmem at ((t/4+ro)*k1 + kpanel*16 + 2t'+ko)*2.
        out.push_str("    mov.u32 %r6, %r3;\n");
        out.push_str(&format!("    mul.lo.u32 %r6, %r6, {};\n", k1 * 2));
        out.push_str(&format!("    add.u32 %r6, %r6, {};\n", kpanel * 16 * 2));
        out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, qsmem;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.shared.b32 %a0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    ld.shared.b32 %a1, [%rd3];\n");
        out.push_str(&format!("    sub.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    add.u64 %rd3, %rd3, 16;\n");
        out.push_str("    ld.shared.b32 %a2, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    ld.shared.b32 %a3, [%rd3];\n");
        // B: b0=(2t', t/4) b1=(2t'+8, t/4) from bsmem at (k*16 + n)*2.
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str(&format!("    mul.lo.u32 %r6, %r6, {};\n", 32));
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, bsmem;\n");
        out.push_str(&format!("    mul.wide.u32 %rd7, %r9, {};\n", 512));
        out.push_str("    add.u64 %rd3, %rd3, %rd7;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n"); // next k row (16 n × 2)
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * 16 * 2));
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c0,%c1,%c2,%c3}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c0,%c1,%c2,%c3}};\n"
        ));
        // Second mma: B cols +8 (n = t/4 + 8).
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 32;\n");
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str(&format!("    add.u32 %r8, %r8, {};\n", 8));
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, bsmem;\n");
        out.push_str(&format!("    mul.wide.u32 %rd7, %r9, {};\n", 512));
        out.push_str("    add.u64 %rd3, %rd3, %rd7;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str("    add.u64 %rd3, %rd3, 256;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c4,%c5,%c6,%c7}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c4,%c5,%c6,%c7}};\n"
        ));
        out.push_str("    bar.sync 0;\n");
    }
    // Scale + store S' to ssmem. n_panel*16 + 2t' cols.
    out.push_str(&format!(
        "    mul.f32 %c0, %c0, {:e}; mul.f32 %c1, %c1, {:e}; mul.f32 %c2, %c2, {:e}; mul.f32 %c3, %c3, {:e};\n",
        scale, scale, scale, scale
    ));
    out.push_str(&format!(
        "    mul.f32 %c4, %c4, {:e}; mul.f32 %c5, %c5, {:e}; mul.f32 %c6, %c6, {:e}; mul.f32 %c7, %c7, {:e};\n",
        scale, scale, scale, scale
    ));
    out.push_str("    cvt.rn.f16.f32 %t0, %c0; cvt.rn.f16.f32 %t1, %c1;\n");
    out.push_str("    mov.b32 %a0, %t0; shl.b32 %t1, %t1, 16; or.b32 %a0, %a0, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c2; cvt.rn.f16.f32 %t1, %c3;\n");
    out.push_str("    mov.b32 %a1, %t0; shl.b32 %t1, %t1, 16; or.b32 %a1, %a1, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c4; cvt.rn.f16.f32 %t1, %c5;\n");
    out.push_str("    mov.b32 %a2, %t0; shl.b32 %t1, %t1, 16; or.b32 %a2, %a2, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c6; cvt.rn.f16.f32 %t1, %c7;\n");
    out.push_str("    mov.b32 %a3, %t0; shl.b32 %t1, %t1, 16; or.b32 %a3, %a3, %t1;\n");
    // ssmem byte = (row*kn + n_panel*16 + 2t')*2.
    out.push_str(&format!("    mul.lo.u32 %r6, %r3, {};\n", kn * 2));
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str("    mul.lo.u32 %r12, %r12, 32;\n");
    out.push_str("    add.u32 %r6, %r6, %r12;\n");
    out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
    out.push_str("    add.u32 %r6, %r6, %r8;\n");
    out.push_str("    mov.u64 %rd3, ssmem;\n");
    out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    st.shared.b32 [%rd3], %a0;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
    out.push_str("    st.shared.b32 [%rd3], %a1;\n");
    out.push_str("    sub.u64 %rd3, %rd3, 0;\n");
    // a2/a3 at col +8 (the second mma's cols) → +16 bytes.
    out.push_str("    sub.u64 %rd3, %rd3, 0;\n");
    out.push_str("    mov.u64 %rd3, ssmem;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    add.u64 %rd3, %rd3, 16;\n");
    out.push_str("    st.shared.b32 [%rd3], %a2;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
    out.push_str("    st.shared.b32 [%rd3], %a3;\n");
    out.push_str("    add.u32 %r10, %r10, 1;\n");
    out.push_str("    bra.uni P1L;\n");
    out.push_str("P1X:\n");
    out.push_str("    bar.sync 0;\n");

    // ---- Phase 2: o[16][on] = S'·V. A from ssmem, B (V) staged.
    out.push_str(&format!("    mul.lo.u32 %r10, %r9, {};\n", phase2_per));
    out.push_str(&format!("    add.u32 %r11, %r10, {};\n", phase2_per));
    out.push_str("P2L:\n");
    out.push_str("    setp.ge.u32 %p1, %r10, %r11;\n");
    out.push_str("    @%p1 bra P2X;\n");
    out.push_str("    mov.f32 %c0, 0f00000000; mov.f32 %c1, 0f00000000; mov.f32 %c2, 0f00000000; mov.f32 %c3, 0f00000000;\n");
    out.push_str("    mov.f32 %c4, 0f00000000; mov.f32 %c5, 0f00000000; mov.f32 %c6, 0f00000000; mov.f32 %c7, 0f00000000;\n");
    for kpanel in 0..(kn / 16) {
        // Fill bsmem: V[(kpanel*16 + k)][(n_panel*16 + n)].
        out.push_str("    mov.u32 %r6, %r1;\n");
        out.push_str("    shr.u32 %r6, %r6, 1;\n");
        out.push_str("    and.b32 %r7, %r1, 1;\n");
        out.push_str("    mov.u32 %r12, %r6;\n");
        out.push_str(&format!("    add.u32 %r12, %r12, {};\n", kpanel * 16));
        out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", on * 2));
        out.push_str("    mov.u32 %r8, %r10;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 32;\n");
        out.push_str("    add.u32 %r12, %r12, %r8;\n");
        out.push_str("    mul.lo.u32 %r8, %r7, 16;\n");
        out.push_str("    add.u32 %r12, %r12, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r12, 1;\n");
        out.push_str("    mov.u64 %rd3, %rd1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", v_off));
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    mov.u64 %rd5, bsmem;\n");
        out.push_str(&format!("    mul.wide.u32 %rd7, %r9, {};\n", 512));
        out.push_str("    add.u64 %rd5, %rd5, %rd7;\n");
        out.push_str("    mov.u32 %r12, %r6;\n");
        out.push_str("    mul.lo.u32 %r12, %r12, 32;\n");
        out.push_str("    mul.lo.u32 %r8, %r7, 16;\n");
        out.push_str("    add.u32 %r12, %r12, %r8;\n");
        out.push_str("    mul.wide.u32 %rd6, %r12, 1;\n");
        out.push_str("    add.u64 %rd6, %rd5, %rd6;\n");
        out.push_str("    ld.global.v4.b32 {%f0,%f1,%f2,%f3}, [%rd3];\n");
        out.push_str("    st.shared.v4.b32 [%rd6], {%f0,%f1,%f2,%f3};\n");
        out.push_str("    bar.sync 0;\n");
        // A from ssmem: S'[(t/4+ro)][kpanel*16 + 2t'+ko].
        out.push_str(&format!("    mul.lo.u32 %r6, %r3, {};\n", kn * 2));
        out.push_str(&format!("    add.u32 %r6, %r6, {};\n", kpanel * 16 * 2));
        out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, ssmem;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.shared.b32 %a0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    ld.shared.b32 %a1, [%rd3];\n");
        out.push_str(&format!("    sub.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    add.u64 %rd3, %rd3, 16;\n");
        out.push_str("    ld.shared.b32 %a2, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    ld.shared.b32 %a3, [%rd3];\n");
        // B from bsmem (first mma): b0=(2t', t/4) b1=(2t'+8, t/4).
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 32;\n");
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, bsmem;\n");
        out.push_str(&format!("    mul.wide.u32 %rd7, %r9, {};\n", 512));
        out.push_str("    add.u64 %rd3, %rd3, %rd7;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str("    add.u64 %rd3, %rd3, 256;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c0,%c1,%c2,%c3}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c0,%c1,%c2,%c3}};\n"
        ));
        // Second mma: B cols +8.
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 32;\n");
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str(&format!("    add.u32 %r8, %r8, {};\n", 8));
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, bsmem;\n");
        out.push_str(&format!("    mul.wide.u32 %rd7, %r9, {};\n", 512));
        out.push_str("    add.u64 %rd3, %rd3, %rd7;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str("    add.u64 %rd3, %rd3, 256;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c4,%c5,%c6,%c7}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c4,%c5,%c6,%c7}};\n"
        ));
        out.push_str("    bar.sync 0;\n");
    }
    // Store o to global.
    out.push_str("    cvt.rn.f16.f32 %t0, %c0; cvt.rn.f16.f32 %t1, %c1;\n");
    out.push_str("    mov.b32 %a0, %t0; shl.b32 %t1, %t1, 16; or.b32 %a0, %a0, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c2; cvt.rn.f16.f32 %t1, %c3;\n");
    out.push_str("    mov.b32 %a1, %t0; shl.b32 %t1, %t1, 16; or.b32 %a1, %a1, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c4; cvt.rn.f16.f32 %t1, %c5;\n");
    out.push_str("    mov.b32 %a2, %t0; shl.b32 %t1, %t1, 16; or.b32 %a2, %a2, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c6; cvt.rn.f16.f32 %t1, %c7;\n");
    out.push_str("    mov.b32 %a3, %t0; shl.b32 %t1, %t1, 16; or.b32 %a3, %a3, %t1;\n");
    out.push_str(&format!("    mul.lo.u32 %r6, %r2, {};\n", 16 * on * 2));
    out.push_str(&format!("    mul.lo.u32 %r7, %r3, {};\n", on * 2));
    out.push_str("    add.u32 %r6, %r6, %r7;\n");
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str("    mul.lo.u32 %r12, %r12, 32;\n");
    out.push_str("    add.u32 %r6, %r6, %r12;\n");
    out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
    out.push_str("    add.u32 %r6, %r6, %r8;\n");
    out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
    out.push_str("    mov.u64 %rd3, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", o_off));
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    st.global.b32 [%rd3], %a0;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * on * 2));
    out.push_str("    st.global.b32 [%rd3], %a1;\n");
    out.push_str("    add.u64 %rd3, %rd3, 0;\n");
    out.push_str("    mov.u64 %rd3, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", o_off));
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    add.u64 %rd3, %rd3, 16;\n");
    out.push_str("    st.global.b32 [%rd3], %a2;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * on * 2));
    out.push_str("    st.global.b32 [%rd3], %a3;\n");
    out.push_str("    add.u32 %r10, %r10, 1;\n");
    out.push_str("    bra.uni P2L;\n");
    out.push_str("P2X:\n");
    out.push_str("    ret;\n}\n");
    out
}

/// Fused-attention MMA PTX, Kt/V-STAGED (2026-09-16).
/// Same math as `fused_attention_mma_ptx` (m16n8k16, S' on-chip, 8 warps)
/// but Kt/V panels are staged through smem with coalesced 16-byte fills,
/// while Q stays as direct global loads (no smem staging — saves 16KB smem
/// vs `fused_attention_mma_staged_ptx` which crushes occupancy).
///
/// smem: bsmem[nwarps × 512] (Kt/V panel per warp, reused), ssmem[16×kn×2]
/// (the scaled S' tile, phase 1's output, phase 2's A).
fn fused_attention_mma_kv_staged_ptx(
    m: i64,
    k1: i64,
    kn: i64,
    on: i64,
    a_off: u64,
    b_off: u64,
    v_off: u64,
    o_off: u64,
    scale: f64,
    nwarps: usize,
) -> String {
    debug_assert!(m % 16 == 0 && k1 % 16 == 0 && kn % 16 == 0 && on % 8 == 0);
    debug_assert!(kn % 16 == 0 && on % 16 == 0);
    debug_assert!((kn as usize / 16) % nwarps == 0 && (on as usize / 16) % nwarps == 0);
    let phase1_per = (kn as usize / 16) / nwarps;
    let phase2_per = (on as usize / 16) / nwarps;
    let smem_bytes = nwarps * 512 + 16 * kn as usize * 2;
    let mut out = String::new();
    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n\n");
    out.push_str(&format!(".visible .entry {} (.param .b64 proj_param)\n{{\n", ENTRY));
    out.push_str("    .reg .u64  %rd1, %rd2, %rd3, %rd4, %rd5, %rd6, %rd7, %rd8, %rd9;\n");
    out.push_str("    .reg .u32  %r1, %r2, %r3, %r4, %r5, %r6, %r7, %r8, %r9, %r10, %r11, %r12, %r13, %r14;\n");
    out.push_str("    .reg .b32  %a0, %a1, %a2, %a3, %b0, %b1, %t0, %t1;\n");
    out.push_str("    .reg .b32  %f0, %f1, %f2, %f3;\n");
    out.push_str("    .reg .f32  %c0, %c1, %c2, %c3, %c4, %c5, %c6, %c7;\n");
    out.push_str("    .reg .pred %p1;\n");
    out.push_str(&format!("    .shared .align 16 .b8 smem[{}];\n", smem_bytes));
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    out.push_str("    mov.u32 %r1, %tid.x;\n");
    out.push_str("    and.b32 %r1, %r1, 31;      // lane\n");
    out.push_str("    mov.u32 %r2, %ctaid.y;     // m_tile\n");
    out.push_str("    shr.u32 %r3, %r1, 2;       // lane/4 (row)\n");
    out.push_str("    and.b32 %r4, %r1, 3;       // lane%4\n");
    out.push_str("    shl.b32 %r5, %r4, 1;       // 2t'\n");
    out.push_str("    mov.u32 %r13, %tid.x;\n");
    out.push_str("    shr.u32 %r9, %r13, 5;      // warp\n");
    // bsmem base: smem + warp * 512
    out.push_str("    mov.u64 %rd5, smem;\n");
    out.push_str(&format!("    mul.wide.u32 %rd6, %r9, {};\n", 512));
    out.push_str("    add.u64 %rd5, %rd5, %rd6;\n");
    // ssmem base: smem + nwarps * 512
    out.push_str("    mov.u64 %rd8, smem;\n");
    out.push_str(&format!("    add.u64 %rd8, %rd8, {};\n", nwarps * 512));

    // ---- Phase 1: S'[16][kn] = Q·Kt·scale.
    // Q: direct global loads. Kt: coalesced fill into bsmem, then ld.shared.
    out.push_str(&format!("    mul.lo.u32 %r10, %r9, {};\n", phase1_per));
    out.push_str(&format!("    add.u32 %r11, %r10, {};\n", phase1_per));
    out.push_str("P1L:\n");
    out.push_str("    setp.ge.u32 %p1, %r10, %r11;\n");
    out.push_str("    @%p1 bra P1X;\n");
    out.push_str("    mov.f32 %c0, 0f00000000; mov.f32 %c1, 0f00000000; mov.f32 %c2, 0f00000000; mov.f32 %c3, 0f00000000;\n");
    out.push_str("    mov.f32 %c4, 0f00000000; mov.f32 %c5, 0f00000000; mov.f32 %c6, 0f00000000; mov.f32 %c7, 0f00000000;\n");
    for kstep in 0..(k1 / 16) {
        // Fill bsmem with Kt panel: Kt[(kstep*16 + krow)][n_panel*16 .. +16].
        // 32 threads fill the 16×16 tile: thread t fills row t/2, col-half
        // (t%2)*8 → 8 f16 (16 B). Row-major smem: row k at k*32, col n at n*2.
        // NB: r12/r14 temps only — r9 (warp) must survive to the B reads.
        out.push_str("    mov.u32 %r6, %r1;\n"); // lane
        out.push_str("    shr.u32 %r6, %r6, 1;\n"); // lane/2 (krow)
        out.push_str("    and.b32 %r7, %r1, 1;\n"); // lane%2 (half)
        // global Kt addr: b_off + (kstep*16 + krow)*kn*2 + n_panel*32 + half*16
        out.push_str(&format!("    add.u64 %rd3, %rd1, {};\n", b_off));
        out.push_str(&format!("    mov.u32 %r12, {};\n", kstep * 16));
        out.push_str("    add.u32 %r12, %r12, %r6;\n");
        out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", kn * 2));
        out.push_str("    mov.u32 %r14, %r10;\n");
        out.push_str("    mul.lo.u32 %r14, %r14, 32;\n");
        out.push_str("    add.u32 %r12, %r12, %r14;\n");
        out.push_str("    mul.lo.u32 %r14, %r7, 16;\n");
        out.push_str("    add.u32 %r12, %r12, %r14;\n");
        out.push_str("    mul.wide.u32 %rd2, %r12, 1;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        // smem dst: bsmem + krow*32 + half*16
        out.push_str("    mov.u64 %rd6, %rd5;\n");
        out.push_str("    mul.lo.u32 %r12, %r6, 32;\n");
        out.push_str("    mul.lo.u32 %r14, %r7, 16;\n");
        out.push_str("    add.u32 %r12, %r12, %r14;\n");
        out.push_str("    mul.wide.u32 %rd4, %r12, 1;\n");
        out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
        out.push_str("    ld.global.v4.b32 {%f0,%f1,%f2,%f3}, [%rd3];\n");
        out.push_str("    st.shared.v4.b32 [%rd6], {%f0,%f1,%f2,%f3};\n");
        out.push_str("    bar.sync 0;\n");

        // A fragments from Q (direct global loads — same as fused_attention_mma_ptx).
        out.push_str("    mov.u32 %r6, %r2;\n");
        out.push_str(&format!("    mul.lo.u32 %r6, %r6, {};\n", 16 * k1 * 2));
        out.push_str(&format!("    mul.lo.u32 %r7, %r3, {};\n", k1 * 2));
        out.push_str("    add.u32 %r6, %r6, %r7;\n");
        out.push_str("    mul.lo.u32 %r7, %r5, 2;\n");
        out.push_str(&format!("    add.u32 %r7, %r7, {};\n", kstep * 16 * 2));
        out.push_str("    add.u32 %r6, %r6, %r7;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    mov.u64 %rd3, %rd1;\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", a_off));
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    ld.global.b32 %a0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    ld.global.b32 %a1, [%rd3];\n");
        out.push_str(&format!("    sub.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    add.u64 %rd3, %rd3, 16;\n");
        out.push_str("    ld.global.b32 %a2, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * k1 * 2));
        out.push_str("    ld.global.b32 %a3, [%rd3];\n");

        // B fragments from bsmem (Kt staged): byte = k*32 + n*2 (row-major
        // 16-col tile). b0 = (k=2t', n=t/4) & (k=2t'+1, n=t/4) packed;
        // b1 = k+8 (byte +8*32). Second mma reads n+8 (byte +16).
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 32;\n");
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    add.u64 %rd3, %rd5, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n"); // next k row
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str("    add.u64 %rd3, %rd3, 256;\n"); // k+8 rows
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c0,%c1,%c2,%c3}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c0,%c1,%c2,%c3}};\n"
        ));
        // Second mma: B cols +8 (n = t/4 + 8) → +16 bytes.
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 32;\n");
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str("    add.u32 %r8, %r8, 8;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    add.u64 %rd3, %rd5, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str("    add.u64 %rd3, %rd3, 256;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c4,%c5,%c6,%c7}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c4,%c5,%c6,%c7}};\n"
        ));
        out.push_str("    bar.sync 0;\n");
    }
    // Scale + store S' to ssmem.
    out.push_str(&format!(
        "    mul.f32 %c0, %c0, {:e}; mul.f32 %c1, %c1, {:e}; mul.f32 %c2, %c2, {:e}; mul.f32 %c3, %c3, {:e};\n",
        scale, scale, scale, scale
    ));
    out.push_str(&format!(
        "    mul.f32 %c4, %c4, {:e}; mul.f32 %c5, %c5, {:e}; mul.f32 %c6, %c6, {:e}; mul.f32 %c7, %c7, {:e};\n",
        scale, scale, scale, scale
    ));
    out.push_str("    cvt.rn.f16.f32 %t0, %c0; cvt.rn.f16.f32 %t1, %c1;\n");
    out.push_str("    mov.b32 %a0, %t0; shl.b32 %t1, %t1, 16; or.b32 %a0, %a0, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c2; cvt.rn.f16.f32 %t1, %c3;\n");
    out.push_str("    mov.b32 %a1, %t0; shl.b32 %t1, %t1, 16; or.b32 %a1, %a1, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c4; cvt.rn.f16.f32 %t1, %c5;\n");
    out.push_str("    mov.b32 %a2, %t0; shl.b32 %t1, %t1, 16; or.b32 %a2, %a2, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c6; cvt.rn.f16.f32 %t1, %c7;\n");
    out.push_str("    mov.b32 %a3, %t0; shl.b32 %t1, %t1, 16; or.b32 %a3, %a3, %t1;\n");
    // ssmem byte = (row*kn + n_panel*16 + 2t')*2.
    out.push_str(&format!("    mul.lo.u32 %r6, %r3, {};\n", kn * 2));
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str("    mul.lo.u32 %r12, %r12, 32;\n");
    out.push_str("    add.u32 %r6, %r6, %r12;\n");
    out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
    out.push_str("    add.u32 %r6, %r6, %r8;\n");
    out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
    out.push_str("    add.u64 %rd3, %rd8, %rd2;\n");
    out.push_str("    st.shared.b32 [%rd3], %a0;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
    out.push_str("    st.shared.b32 [%rd3], %a1;\n");
    out.push_str("    add.u64 %rd3, %rd3, 0;\n");
    out.push_str("    mov.u64 %rd3, %rd8;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    add.u64 %rd3, %rd3, 16;\n");
    out.push_str("    st.shared.b32 [%rd3], %a2;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
    out.push_str("    st.shared.b32 [%rd3], %a3;\n");
    out.push_str("    add.u32 %r10, %r10, 1;\n");
    out.push_str("    bra.uni P1L;\n");
    out.push_str("P1X:\n");
    out.push_str("    bar.sync 0;\n");

    // ---- Phase 2: o[16][on] = S'·V. A from ssmem, B (V) staged.
    out.push_str(&format!("    mul.lo.u32 %r10, %r9, {};\n", phase2_per));
    out.push_str(&format!("    add.u32 %r11, %r10, {};\n", phase2_per));
    out.push_str("P2L:\n");
    out.push_str("    setp.ge.u32 %p1, %r10, %r11;\n");
    out.push_str("    @%p1 bra P2X;\n");
    out.push_str("    mov.f32 %c0, 0f00000000; mov.f32 %c1, 0f00000000; mov.f32 %c2, 0f00000000; mov.f32 %c3, 0f00000000;\n");
    out.push_str("    mov.f32 %c4, 0f00000000; mov.f32 %c5, 0f00000000; mov.f32 %c6, 0f00000000; mov.f32 %c7, 0f00000000;\n");
    for kstep in 0..(kn / 16) {
        // Fill bsmem with V panel: V[(kstep*16 + krow)][n_panel*16 .. +16].
        // 32 threads fill the 16×16 tile (same as Kt): row = lane/2, half = lane%2.
        out.push_str("    mov.u32 %r6, %r1;\n"); // lane
        out.push_str("    shr.u32 %r6, %r6, 1;\n"); // lane/2 (krow)
        out.push_str("    and.b32 %r7, %r1, 1;\n"); // lane%2 (half)
        out.push_str(&format!("    add.u64 %rd3, %rd1, {};\n", v_off));
        out.push_str(&format!("    mov.u32 %r12, {};\n", kstep * 16));
        out.push_str("    add.u32 %r12, %r12, %r6;\n");
        out.push_str(&format!("    mul.lo.u32 %r12, %r12, {};\n", on * 2));
        out.push_str("    mov.u32 %r14, %r10;\n");
        out.push_str("    mul.lo.u32 %r14, %r14, 32;\n");
        out.push_str("    add.u32 %r12, %r12, %r14;\n");
        out.push_str("    mul.lo.u32 %r14, %r7, 16;\n");
        out.push_str("    add.u32 %r12, %r12, %r14;\n");
        out.push_str("    mul.wide.u32 %rd2, %r12, 1;\n");
        out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
        out.push_str("    mov.u64 %rd6, %rd5;\n");
        out.push_str("    mul.lo.u32 %r12, %r6, 32;\n");
        out.push_str("    mul.lo.u32 %r14, %r7, 16;\n");
        out.push_str("    add.u32 %r12, %r12, %r14;\n");
        out.push_str("    mul.wide.u32 %rd4, %r12, 1;\n");
        out.push_str("    add.u64 %rd6, %rd6, %rd4;\n");
        out.push_str("    ld.global.v4.b32 {%f0,%f1,%f2,%f3}, [%rd3];\n");
        out.push_str("    st.shared.v4.b32 [%rd6], {%f0,%f1,%f2,%f3};\n");
        out.push_str("    bar.sync 0;\n");

        // A from ssmem: S'[(row)][kstep*16 + 2t'].
        out.push_str(&format!("    mul.lo.u32 %r6, %r3, {};\n", kn * 2));
        out.push_str(&format!("    add.u32 %r6, %r6, {};\n", kstep * 16 * 2));
        out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    add.u64 %rd3, %rd8, %rd2;\n");
        out.push_str("    ld.shared.b32 %a0, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    ld.shared.b32 %a1, [%rd3];\n");
        out.push_str(&format!("    sub.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    add.u64 %rd3, %rd3, 16;\n");
        out.push_str("    ld.shared.b32 %a2, [%rd3];\n");
        out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * kn * 2));
        out.push_str("    ld.shared.b32 %a3, [%rd3];\n");

        // B from bsmem (V staged): byte = k*32 + n*2. b0 = (2t', t/4),
        // b1 = k+8. Second mma reads n+8 (+16 bytes).
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 32;\n");
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    add.u64 %rd3, %rd5, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str("    add.u64 %rd3, %rd3, 256;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c0,%c1,%c2,%c3}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c0,%c1,%c2,%c3}};\n"
        ));
        // Second mma: B cols +8.
        out.push_str("    mov.u32 %r6, %r5;\n");
        out.push_str("    mul.lo.u32 %r6, %r6, 32;\n");
        out.push_str("    mov.u32 %r8, %r3;\n");
        out.push_str("    add.u32 %r8, %r8, 8;\n");
        out.push_str("    mul.lo.u32 %r8, %r8, 2;\n");
        out.push_str("    add.u32 %r6, %r6, %r8;\n");
        out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
        out.push_str("    add.u64 %rd3, %rd5, %rd2;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b0, %t0; shl.b32 %t1, %t1, 16; or.b32 %b0, %b0, %t1;\n");
        out.push_str("    sub.u64 %rd3, %rd3, 32;\n");
        out.push_str("    add.u64 %rd3, %rd3, 256;\n");
        out.push_str("    ld.shared.b16 %t0, [%rd3];\n");
        out.push_str("    add.u64 %rd3, %rd3, 32;\n");
        out.push_str("    ld.shared.b16 %t1, [%rd3];\n");
        out.push_str("    mov.b32 %b1, %t0; shl.b32 %t1, %t1, 16; or.b32 %b1, %b1, %t1;\n");
        out.push_str(&format!(
            "    mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 {{%c4,%c5,%c6,%c7}}, {{%a0,%a1,%a2,%a3}}, {{%b0,%b1}}, {{%c4,%c5,%c6,%c7}};\n"
        ));
        out.push_str("    bar.sync 0;\n");
    }
    // Store o to global.
    out.push_str("    cvt.rn.f16.f32 %t0, %c0; cvt.rn.f16.f32 %t1, %c1;\n");
    out.push_str("    mov.b32 %a0, %t0; shl.b32 %t1, %t1, 16; or.b32 %a0, %a0, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c2; cvt.rn.f16.f32 %t1, %c3;\n");
    out.push_str("    mov.b32 %a1, %t0; shl.b32 %t1, %t1, 16; or.b32 %a1, %a1, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c4; cvt.rn.f16.f32 %t1, %c5;\n");
    out.push_str("    mov.b32 %a2, %t0; shl.b32 %t1, %t1, 16; or.b32 %a2, %a2, %t1;\n");
    out.push_str("    cvt.rn.f16.f32 %t0, %c6; cvt.rn.f16.f32 %t1, %c7;\n");
    out.push_str("    mov.b32 %a3, %t0; shl.b32 %t1, %t1, 16; or.b32 %a3, %a3, %t1;\n");
    out.push_str(&format!("    mul.lo.u32 %r6, %r2, {};\n", 16 * on * 2));
    out.push_str(&format!("    mul.lo.u32 %r7, %r3, {};\n", on * 2));
    out.push_str("    add.u32 %r6, %r6, %r7;\n");
    out.push_str("    mov.u32 %r12, %r10;\n");
    out.push_str("    mul.lo.u32 %r12, %r12, 32;\n");
    out.push_str("    add.u32 %r6, %r6, %r12;\n");
    out.push_str("    mul.lo.u32 %r8, %r5, 2;\n");
    out.push_str("    add.u32 %r6, %r6, %r8;\n");
    out.push_str("    mul.wide.u32 %rd2, %r6, 1;\n");
    out.push_str("    mov.u64 %rd3, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", o_off));
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    st.global.b32 [%rd3], %a0;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * on * 2));
    out.push_str("    st.global.b32 [%rd3], %a1;\n");
    out.push_str("    mov.u64 %rd3, %rd1;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", o_off));
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    out.push_str("    add.u64 %rd3, %rd3, 16;\n");
    out.push_str("    st.global.b32 [%rd3], %a2;\n");
    out.push_str(&format!("    add.u64 %rd3, %rd3, {};\n", 8 * on * 2));
    out.push_str("    st.global.b32 [%rd3], %a3;\n");
    out.push_str("    add.u32 %r10, %r10, 1;\n");
    out.push_str("    bra.uni P2L;\n");
    out.push_str("P2X:\n");
    out.push_str("    ret;\n}\n");
    out
}

/// Build the ONE fused attention kernel for a detected chain (Phase 4b v1).
/// Derives the GEMM operands from the producer/consumer shapes (never from
/// pattern names): the producer GEMM is `a·b → mid_in` (M×K → M×kn), the
/// consumer is `mid_out·v → o` (M×kn → M×on). v1 scope: f16 operands and a
/// square middle (on == kn). Returns None when not applicable (the chain
/// falls back to the 2-kernel epilogue fusion).
///
/// 2026-09-15 (mma rung): the kernel is `fused_attention_mma_ptx` (m16n8k16
/// tensor cores, direct fragment loads); the naive-on-chip kernel remains as
/// the correctness reference and the fallback when the shape does not tile.
fn build_fused_attention_kernel(
    program: &[TopLevel],
    universe: &TypeUniverse,
    layout: &crate::backend::spirv::runner::SsboLayout,
    entries: &std::collections::HashMap<String, crate::analysis::accel::AccelEntry>,
    cf: &crate::analysis::gpu_schedule::ChainFusion,
    consts: &std::collections::HashMap<String, Expr>,
) -> Result<Option<RunnerKernel>, String> {
    let producer = entries.get(&cf.producer).ok_or_else(|| {
        format!("ptx fused: producer '{}' not in accel entries", cf.producer)
    })?;
    let consumer = entries.get(&cf.consumer).ok_or_else(|| {
        format!("ptx fused: consumer '{}' not in accel entries", cf.consumer)
    })?;
    let Some(pplan) = GemmPlan::match_stmts(&producer.shape, program) else {
        return Ok(None);
    };
    let Some(cplan) = GemmPlan::match_stmts(&consumer.shape, program) else {
        return Ok(None);
    };
    // 2026-09-16 (shape strategy selector Stage 0a): the ONE-kernel fused
    // emitter is a 10× REGRESSION vs the 2-kernel tensor-tier composition
    // (0.752 vs 0.073 ms @512², both correct) — the 16-row m-tile gives zero
    // Kt/V reuse across m-tiles. Default OFF; the composition is correct.
    // Re-enable when the cost model gates fusion on "beats the composition".
    if !crate::config_tuning::ir_lowering().ptx_fused_attention {
        return Ok(None);
    }
    // The consumer's A operand must be the middle's output (the chain).
    if cplan.a_field != cf.mid_out || pplan.y_field != cf.mid_in {
        return Ok(None);
    }
    // Shapes: producer M×K → M×kn; consumer M×kn → M×on. v1: square middle
    // (on == kn) and f16 operands throughout.
    let (m, k1, kn) = (pplan.m, pplan.k, pplan.n);
    let on = cplan.n;
    if on != kn {
        return Ok(None);
    }
    let elem_of = |field: &str| -> u32 {
        layout
            .fields
            .iter()
            .find(|f| f.name == field)
            .map(|f| f.elem_bytes)
            .unwrap_or(4)
    };
    let f16 = elem_of(&pplan.a_field) == 2 && elem_of(&pplan.b_field) == 2
        && elem_of(&cplan.b_field) == 2;
    if !f16 {
        return Ok(None);
    }
    let find_off = |field: &str| -> Result<u64, String> {
        layout
            .fields
            .iter()
            .find(|f| f.name == field)
            .map(|f| f.proj_offset)
            .ok_or_else(|| format!("ptx fused: field '{}' not in layout", field))
    };
    let a_off = find_off(&pplan.a_field)?;
    let b_off = find_off(&pplan.b_field)?;
    let v_off = find_off(&cplan.b_field)?;
    let o_off = find_off(&cplan.y_field)?;
    // 2026-09-15 (mma rung): the m16n8k16 tensor-core kernel when the shapes
    // tile (m%16, k1%16, kn%16, on%8); otherwise the naive-on-chip reference.
    // 8 warps per block (each warp owns an n-slice of the shared S' tile) —
    // 32 single-warp blocks underfilled the SM count and lost 3.4× to the
    // composition.
    let tiles = m % 16 == 0 && k1 % 16 == 0 && kn % 16 == 0 && on % 8 == 0;
    let nwarps: usize = 8;
    let staged_ok = tiles && kn % 16 == 0 && on % 16 == 0
        && (kn as usize / 16) % nwarps == 0 && (on as usize / 16) % nwarps == 0;
    let mma_ok = tiles && (kn as usize / 8) % nwarps == 0 && (on as usize / 8) % nwarps == 0;
    // 2026-09-15: the smem-STAGED emitter (coalesced 16-byte fills + smem
    // fragment reads) is the cuBLAS lesson, but its full-width Q staging
    // crushes occupancy (1 block/SM vs 3) and per-step barriers serialize —
    // measured 3.9x SLOWER than the direct-load kernel. It stays behind
    // `ptx_fused_staged` (experimental); the DIRECT-load mma is the default;
    // the naive is the last resort.
    //
    // 2026-09-16: `fused_attention_mma_kv_staged_ptx` keeps Q as direct
    // global loads (saves 16KB smem) and stages only Kt/V panels (512B each)
    // through smem with coalesced 16-byte fills. This is the new
    // `ptx_fused_staged` path — same coalesced-load benefit without the
    // occupancy cost.
    let staged_on = crate::config_tuning::ir_lowering().ptx_fused_staged;
    let kv_staged_smem = (nwarps * 512 + 16 * kn as usize * 2) as u32;
    let (ptx, fused_mma, block_threads, shared_bytes, fused_div) = if staged_on && staged_ok {
        (
            fused_attention_mma_kv_staged_ptx(m, k1, kn, on, a_off, b_off, v_off, o_off, cf.scale, nwarps),
            true,
            (32 * nwarps) as u32,
            kv_staged_smem,
            (16 * on) as u32,
        )
    } else if mma_ok {
        (
            fused_attention_mma_ptx(m, k1, kn, on, a_off, b_off, v_off, o_off, cf.scale, nwarps),
            true,
            (32 * nwarps) as u32,
            (16 * kn * 2) as u32,
            (16 * on) as u32,
        )
    } else {
        (
            fused_attention_ptx(m, k1, kn, on, a_off, b_off, v_off, o_off, cf.scale),
            false,
            512,
            1024,
            0,
        )
    };
    let blob = if crate::config_tuning::ir_lowering().ptx_emit_cubin {
        compile_cubin(&ptx, 64).unwrap_or_else(|| ptx.into_bytes())
    } else {
        ptx.into_bytes()
    };
    let mut touched = vec![
        pplan.a_field.clone(),
        pplan.b_field.clone(),
        cplan.b_field.clone(),
        cplan.y_field.clone(),
    ];
    if !producer.shape.index_var.is_empty() {
        touched.push(producer.shape.index_var.clone());
    }
    touched.sort();
    touched.dedup();
    Ok(Some(RunnerKernel {
        name: format!("{}__{}_{}", cf.producer, cf.middle, cf.consumer),
        spirv: blob,
        image_plans: Vec::new(),
        // The producer's counter drives the runner's pass loop (the fused
        // kernel covers all three nodes' work in one launch). The count is
        // the WORK count (M·N) — the dispatch divides by fused_div to get
        // the block count.
        index_var: producer.shape.index_var.clone(),
        count_expr: Expr::Decimal(m * on),
        work_cols: None,
        cooperative: false,
        tiled: false,
        tensor: false,
        tensor_tile_rows: 1,
        ptx_tensor: false,
        fused_mma,
        fused_mma_blocks_div: fused_div,
        block_threads,
        shared_bytes,
        touched_fields: touched,
    }))
}

/// Select multi-warp CTA dimensions (mw, nw) for the mw kernel.
/// CTA tile = (mw*32) × (nw*64), block_threads = mw*nw*32 (max 256).
/// Register budget: the mw kernel compiles to 128 regs natural, 0 spills
/// (2026-09-10 ptxas 13.3, after the per-mh/per-g compute scheduling trim).
/// 128 × 256 = 32,768, so TWO 8-warp CTAs co-reside per SM — measured
/// 2026-09-10 @4096³: (2,4)@256T 18.4 vs (4,4)@512T 16.2 TFLOP/s. A single
/// 512-thread CTA (128 × 512 = the full register file, 1 CTA/SM) has half
/// the cp.async streams and loses. History: the original 512 cap assumed
/// 108 regs; -maxrregcount=108 cubins fault IMA at runtime (ptxas 12.8 AND
/// 13.3) — never ship capped-register cubins, verify with ptxas -v.
/// Also: nw must divide 8, mw must divide 16 (for per_a/per_b integer
/// division in the kernel). Prefers wider nw (B reuse) then scales mw
/// (A reuse).
/// The PTX tensor tier's warp shape (mhr = 16-row blocks per warp).
/// Single source of truth: the dispatch and every dump-test artifact read
/// this — an A/B that edits one and benches the other measures nothing
/// (BUGS.md 2026-09-12). f16acc uses the 64x32 A-sharing warp (mhr=4):
/// per_a=4 fires the 16B .cg A rung, B loads drop 16x -> 4x per kstep
/// (+2.9 TF at 4096^3, 4/4 rounds). f32 keeps the 32x64 warp its serial
/// schedule was tuned at (mh4 A/B pending).
pub(crate) fn ptx_warp_mh(f16_acc: bool) -> usize {
    if f16_acc {
        4
    } else {
        2
    }
}

/// E4c (2026-09-13) growth-order policy: at mhr>=4 the A-sharing warp
/// (64x32, B loaded 4x per kstep) measures nw-heavy CTAs decisively above
/// mw-heavy at equal thread count — (2,4)@256T beats (4,2)@256T 35.3 vs
/// 31.0 TFLOP/s (4096^3 f16acc, interleaved A/B x3) and beats the old
/// (4,4)@512T 34.5 by +2.3% (also +12.5% at 2048^3, +6% at 8192^3,
/// same-window). Warps stacked along N replicate A-fragment reads; mhr=2
/// keeps the mw-first order so the f32 (4,2)@256T landing is
/// byte-identical. Undo: return [(2,2),(2,1),(1,2)] unconditionally.
fn walk_order(mhr: usize) -> [(usize, usize); 3] {
    if mhr >= 4 {
        [(2, 2), (1, 2), (2, 1)]
    } else {
        [(2, 2), (2, 1), (1, 2)]
    }
}

fn select_mw_nw(m: i64, n: i64, thread_cap: usize, mhr: usize) -> (usize, usize) {
    // mhr scales the warp's 16-row block count (warp_mh): the CTA tile is
    // (16*mhr*mw) rows x (8*gr*nw) cols with gr = 16/mhr, so the aspect of
    // the divisibility guards follows the warp shape. mhr=2 keeps the
    // historical 32-row/64-col guards byte-for-byte.
    let gr = 16 / mhr;
    let mut mw: usize = 1;
    let mut nw: usize = 1;
    loop {
        let mut grew = false;
        // 2026-09-11: double both axes first, then the aspect-preferred
        // axis (see walk_order). The old (1,2)-first walk ran nw to its
        // cap and never reached the balanced (4,4) — shipping (2,8)@512T
        // at 16.4 TFLOP/s where (4,4) measures 21.0 (4096^3 f16acc,
        // three interleaved reps).
        for (dm, dn) in walk_order(mhr) {
            let (tm, tn) = (mw * dm, nw * dn);
            if tm <= 16
                && tn <= 8
                && m % ((tm * 16 * mhr) as i64) == 0
                && n % ((tn * 8 * gr) as i64) == 0
                && tm * tn * 32 <= thread_cap
            {
                mw = tm;
                nw = tn;
                grew = true;
                break;
            }
        }
        if !grew {
            break;
        }
    }
    (mw, nw)
}

/// 2026-09-16 (shape strategy selector Stage 1): map the cost model's
/// strategy back onto the tensor-GEMM (mw, nw, stages) so the dispatch is
/// analysis-driven, not hardcoded. The model picks the CTA tile (tile_m,
/// tile_n) and stages; the emitter wants (mw, nw) warp-grid + warp_mh.
/// Returns None when the strategy doesn't map to the tensor tier (falls
/// back to the legacy select_mw_nw).
fn strategy_to_mwnw(
    s: &crate::analysis::gpu_strategy::Strategy,
    warp_mh: usize,
    m: i64,
    n: i64,
    thread_cap: usize,
) -> Option<(usize, usize, usize)> {
    let gr = 16 / warp_mh;
    let tile_m = (16 * warp_mh) as u64;
    let tile_n = (8 * gr) as u64;
    if s.tile_m % tile_m != 0 || s.tile_n % tile_n != 0 {
        return None;
    }
    let mw = (s.tile_m / tile_m) as usize;
    let nw = (s.tile_n / tile_n) as usize;
    if mw == 0 || nw == 0 || mw * nw * 32 > thread_cap {
        return None;
    }
    if m % ((mw * 16 * warp_mh) as i64) != 0 || n % ((nw * 8 * gr) as i64) != 0 {
        return None;
    }
    Some((mw, nw, s.stages as usize))
}

/// Build the PTX kernel set for an `.abv` — one kernel per eligible accel
/// entry. GEMM-shaped entries lower to the naive PTX kernel; anything else
/// is a hard error (the S2a surface gate — GEMM family ONLY until S5).

/// 2026-09-11 (cubin shipping): compile PTX text to cubin bytes through
/// offline ptxas — the driver JIT is bypassed entirely (it ignores
/// CU_JIT_MAX_REGISTERS: 166 regs vs the requested 128 → 1 CTA/SM, −27%;
/// rejects the `.maxnreg` directive text; and its internal compiler state
/// wedges after fault storms). The PTX carries its own `.maxnreg`
/// contract; ptxas honors it, so no extra flags. Returns None when ptxas
/// is unavailable or fails — the caller ships PTX text and the runtime
/// JITs (historical path).
pub(crate) fn compile_cubin(ptx: &str, maxnreg: u32) -> Option<Vec<u8>> {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("briev-ptx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let in_path = dir.join("kernel.ptx");
    let out_path = dir.join("kernel.cubin");
    std::fs::write(&in_path, ptx).ok()?;

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("TRITON_PTXAS") {
        candidates.push(std::path::PathBuf::from(p));
    }
    candidates.push(std::path::PathBuf::from("ptxas"));
    candidates.push(std::path::PathBuf::from("/opt/cuda/bin/ptxas"));
    if let Ok(home) = std::env::var("HOME") {
        // Any installed python's triton backend (the benchmark toolchain's
        // ptxas — 12.8 validated alongside 13.3).
        if let Ok(entries) = std::fs::read_dir(format!("{home}/.local/lib")) {
            for e in entries.flatten() {
                let c = e.path().join(
                    "site-packages/triton/backends/nvidia/bin/ptxas",
                );
                if c.exists() {
                    candidates.push(c);
                }
            }
        }
    }

    let mut cubin = None;
    for ptxas in &candidates {
        let nreg = format!("{maxnreg}");
        let out = Command::new(ptxas)
            .args([
                "-arch",
                "sm_86",
                "-maxrregcount",
                &nreg,
                &in_path.to_string_lossy(),
                "-o",
                &out_path.to_string_lossy(),
            ])
            .output();
        let Ok(out) = out else { continue };
        if !out.status.success() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&out_path) {
            // A cubin is an ELF image; anything else is not a blob the
            // runtime can load.
            if bytes.len() > 4 && bytes[0..4] == [0x7f, b'E', b'L', b'F'] {
                cubin = Some(bytes);
                break;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    cubin
}

/// Module consts (literals only) for the general-kernel emitter and count
/// folding — the same rule the runner's emitter uses.
fn module_expr_consts(program: &[TopLevel]) -> std::collections::HashMap<String, Expr> {
    let mut m = std::collections::HashMap::new();
    for item in program {
        if let TopLevel::Constant(c) = item {
            if matches!(c.expr, Expr::Decimal(_) | Expr::Float(_)) {
                m.insert(c.name.clone(), c.expr.clone());
            }
        }
    }
    m
}

/// 2026-09-14 (Matrix type plan): field name → type, from the program's
/// `let` declarations — used to validate GEMM shapes against Matrix types.
fn field_type_map(program: &[TopLevel]) -> std::collections::HashMap<String, Type> {
    program
        .iter()
        .filter_map(|tl| {
            if let TopLevel::Statement(stmt) = tl {
                if let Statement::Let {
                    name,
                    ty: Some(ty),
                    ..
                } = stmt.as_ref()
                {
                    return Some((name.clone(), ty.clone()));
                }
            }
            None
        })
        .collect()
}

/// Fold an accel node's work-item count expression to a constant.
fn fold_count(
    shape: &crate::analysis::accel::KernelShape,
    consts: &std::collections::HashMap<String, Expr>,
) -> Result<i64, String> {
    let e = shape.count_expr.clone().unwrap_or(Expr::Decimal(0));
    match e {
        Expr::Decimal(n) => Ok(n),
        Expr::Identifier(s) => match consts.get(&s) {
            Some(Expr::Decimal(n)) => Ok(*n),
            _ => Err(format!("ptx general: count '{}' is not a constant", s)),
        },
        Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) => {
            let lv = fold_side(&l, consts)?;
            let rv = fold_side(&r, consts)?;
            Ok(lv * rv)
        }
        other => Err(format!(
            "ptx general: count expression {:?} not foldable",
            other
        )),
    }
}

fn fold_side(e: &Expr, consts: &std::collections::HashMap<String, Expr>) -> Result<i64, String> {
    match e {
        Expr::Decimal(n) => Ok(*n),
        Expr::Identifier(s) => match consts.get(s) {
            Some(Expr::Decimal(n)) => Ok(*n),
            _ => Err(format!("ptx general: count operand '{}' not a constant", s)),
        },
        other => Err(format!("ptx general: count operand {:?} not foldable", other)),
    }
}

pub fn build_ptx_kernels(
    program: &[TopLevel],
    universe: &TypeUniverse,
    int_bits: u64,
    entries: &std::collections::HashMap<String, crate::analysis::accel::AccelEntry>,
    schedule: &crate::analysis::gpu_schedule::GpuSchedule,
) -> Result<Vec<RunnerKernel>, String> {
    let mut names: Vec<&String> = entries
        .iter()
        .filter(|(_, e)| e.shape.eligible)
        .map(|(n, _)| n)
        .collect();
    names.sort();

    // The ONE layout rule — the same `ssbo_layout` the runner uses, so the
    // hardcoded PTX offsets and the runner's field table agree by
    // construction (no image plans in the GEMM tier). 2026-09-15: pass the
    // schedule's reuse map (Phase 3 buffer aliasing) so the PTX kernel
    // offsets match the runner's ALIASED projection — without it the kernel
    // writes s2@131120 while the runner reads s2@98352 (its aliased slot).
    let reuse = crate::backend::spirv::runner::gated_reuse_map(Some(schedule));
    let layout = crate::backend::spirv::runner::ssbo_layout(
        program, universe, int_bits, &std::collections::HashMap::new(), reuse.as_ref(),
    )?;

    let mut out = Vec::new();
    // 2026-09-15 (Phase 4b — emergent chain fusion): when the schedule
    // detected a GEMM → elementwise → GEMM chain with dead intermediates,
    // emit ONE fused kernel (the middle's output stays on-chip). v1 scope:
    // f16 operands and a square middle (on == kn). The producer/middle/
    // consumer nodes are then skipped below.
    let mut chain_skip: std::collections::HashSet<String> = Default::default();
    let mut chain_name: Option<String> = None;
    if let Some(cf) = &schedule.chain_fusion {
        let fused_kernel = build_fused_attention_kernel(
            program,
            universe,
            &layout,
            entries,
            cf,
            &module_expr_consts(program),
        )?;
        if let Some(k) = fused_kernel {
            out.push(k);
            chain_skip.insert(cf.producer.clone());
            chain_skip.insert(cf.middle.clone());
            chain_skip.insert(cf.consumer.clone());
            chain_name = Some(format!("{}__{}_{}", cf.producer, cf.middle, cf.consumer));
        }
    }
    for name in names {
        if chain_skip.contains(name) {
            continue;
        }
        let e = &entries[name];
        // 2026-09-14 (gpu_schedule Phase 4a): a fused consumer's work is done
        // by its producer's epilogue — no kernel is emitted for it. The
        // schedule already gates f16 fusions on the f16-acc tier; f32 always
        // fuses. (2026-09-15: the f16 packed-acc epilogue is verified
        // correct; the f32-acc f16 producer has no packed-acc epilogue and
        // would silently drop the scale, so the schedule excludes it.)
        let fused = schedule.fusions.iter().find(|f| f.consumer == *name);
        if let Some(f) = fused {
            let a_elem = layout
                .fields
                .iter()
                .find(|fl| fl.name == f.in_field)
                .map(|fl| fl.elem_bytes)
                .unwrap_or(4);
            if crate::analysis::gpu_schedule::fusion_applies(
                a_elem as u64,
                crate::config_tuning::ir_lowering().ptx_tensor_f16acc,
            ) {
                continue;
            }
        }
        let plan = GemmPlan::match_stmts(&e.shape, program);
        // 2026-09-14 (gpu_schedule S5-lite): a non-GEMM eligible node is an
        // elementwise kernel (the row-ops between GEMMs in an attention
        // decode) — emit the general 1D PTX kernel.
        if plan.is_none() {
            let consts = module_expr_consts(program);
            let count = fold_count(&e.shape, &consts)?;
            let ptx = general::emit_general_ptx(&e.shape, count, &layout, &consts)?;
            let blob = if crate::config_tuning::ir_lowering().ptx_emit_cubin {
                compile_cubin(&ptx, 64).unwrap_or_else(|| ptx.into_bytes())
            } else {
                ptx.into_bytes()
            };
            out.push(RunnerKernel {
                name: name.clone(),
                spirv: blob,
                image_plans: Vec::new(),
                index_var: e.shape.index_var.clone(),
                count_expr: e.shape.count_expr.clone().unwrap_or(Expr::Decimal(0)),
                work_cols: None,
                cooperative: false,
                tiled: false,
                tensor: false,
                tensor_tile_rows: 1,
                ptx_tensor: false,
        fused_mma: false,
        fused_mma_blocks_div: 0,
                block_threads: 256,
                shared_bytes: 0,
                touched_fields: crate::backend::spirv::runner::kernel_touched_fields(&e.shape),
            });
            continue;
        }
        let plan = plan.unwrap();
        // 2026-09-14 (Matrix type plan): when the a/b/y fields carry
        // Matrix<T,R,C> types, validate that the type-shape M/N/K matches
        // the body-derived shape. The type is the contract (Rule 1).
        let ftypes = field_type_map(program);
        if let Some(a_ty) = ftypes.get(&plan.a_field) {
            if let Some((m, k, _)) = universe.matrix_shape(a_ty) {
                if plan.m != m as i64 || plan.k != k as i64 {
                    return Err(format!(
                        "ptx: node '{}': Matrix shape mismatch on '{}': type is {}×{} but body implies M={}, K={}",
                        name, plan.a_field, m, k, plan.m, plan.k
                    ));
                }
            }
        }
        if let Some(b_ty) = ftypes.get(&plan.b_field) {
            if let Some((b_rows, n, _)) = universe.matrix_shape(b_ty) {
                if plan.k != b_rows as i64 || plan.n != n as i64 {
                    return Err(format!(
                        "ptx: node '{}': Matrix shape mismatch on '{}': type is {}×{} but body implies K={}, N={}",
                        name, plan.b_field, b_rows, n, plan.k, plan.n
                    ));
                }
            }
        }
        if let Some(y_ty) = ftypes.get(&plan.y_field) {
            if let Some((m, n, _)) = universe.matrix_shape(y_ty) {
                if plan.m != m as i64 || plan.n != n as i64 {
                    return Err(format!(
                        "ptx: node '{}': Matrix shape mismatch on '{}': type is {}×{} but body implies M={}, N={}",
                        name, plan.y_field, m, n, plan.m, plan.n
                    ));
                }
            }
        }
        let find_off = |field: &str| -> Result<u64, String> {
            layout
                .fields
                .iter()
                .find(|f| f.name == field)
                .map(|f| f.proj_offset)
                .ok_or_else(|| format!("ptx: node '{}': field '{}' not in layout", name, field))
        };
        let a_off = find_off(&plan.a_field)?;
        let b_off = find_off(&plan.b_field)?;
        let y_off = find_off(&plan.y_field)?;
        // 2026-09-14 (gpu_schedule Phase 4a): epilogue fusion — if this GEMM
        // is the producer of a pure-scale consumer, write the consumer's
        // output field with the scale applied, and drop the consumer node.
        // 2026-09-15: the f16 tensor epilogue (packed f16x2 mul) is verified
        // correct on the f16-acc tier; f32 naive always fuses. The schedule
        // gates f16 fusions on f16_acc, so a fusion here is always
        // applicable.
        let fusion = schedule.fusions.iter().find(|f| f.producer == *name);
        let a_elem = layout
            .fields
            .iter()
            .find(|fl| fl.name == plan.a_field)
            .map(|fl| fl.elem_bytes)
            .unwrap_or(4);
        let (y_off, epilogue_scale) = match fusion {
            Some(f) => (find_off(&f.out_field)?, Some(f.scale)),
            _ => (y_off, None),
        };

        // f16 a/b → the tensor tier (S3b mma kernel); f32 → the naive tier.
        // The tensor tier needs M%32, N%16, K%16 (warp-tile geometry).
        let a_elem = layout
            .fields
            .iter()
            .find(|f| f.name == plan.a_field)
            .map(|f| f.elem_bytes)
            .unwrap_or(4);
        let y_elem = layout
            .fields
            .iter()
            .find(|f| f.name == plan.y_field)
            .map(|f| f.elem_bytes)
            .unwrap_or(4);
        let tensor = a_elem == 2
            && plan.m % 32 == 0
            && plan.n % 16 == 0
            && plan.k % 16 == 0;

        let f16_acc = crate::config_tuning::ir_lowering().ptx_tensor_f16acc;
        // f16-acc halves the accumulator registers (64 f32 -> 32 f16x2),
        // funding 64-reg/256T kernels: 4 CTAs/SM at 16KB smem (E4c, 2026-09-13).
        // f32-acc keeps 4 stages (deep pipeline, 1-2 CTAs by config).
        // 2026-09-14 (parity probe P1): stages=3 at the f16acc (2,4) tile
        // (24KB, still 4 CTAs/SM) measured +1.0-1.8% at every large square
        // shape — the auto default. `ptx_tensor_stages` overrides (2|3).
        let stages = match crate::config_tuning::ir_lowering().ptx_tensor_stages {
            0 => {
                if f16_acc {
                    3usize
                } else {
                    4usize
                }
            }
            v => v as usize,
        };
        // On-device sweep (2026-09-10, 4096^3): the f32 kernel's best is
        // (4,2)@256T (16.6) — 2 CTAs/SM beat the 1-CTA wide tile.
        // 2026-09-13 (E4c): the f16acc cap drops to 256 — the 8-warp CTA
        // with 4 co-resident CTAs/SM (16KB smem, 64 regs) beats the
        // (4,4)@512T x2 point on every large square shape (nw-first walker
        // lands (2,4)@256T; see select_mw_nw for the A/B numbers).
        let thread_cap = 256;
        let (ptx, ptx_tensor, count_expr, block_threads, shared_bytes) = if tensor {
            let warp_mh = ptx_warp_mh(f16_acc);
            let gr = 16 / warp_mh;
            // Select mw/nw for multi-warp CTA. The mw kernel needs
            // M%(16*mhr*mw)==0 and N%(8*gr*nw)==0; fall back to single-warp
            // smem kernel when the shape doesn't tile cleanly.
            // 2026-09-16 (shape strategy selector Stage 1): prefer the cost
            // model's strategy (tile + stages chosen from shape evidence,
            // calibrated against cuBLAS); fall back to the legacy walker
            // when the strategy doesn't map (or the model has no candidate).
            let strategy = crate::analysis::gpu_strategy::select(
                plan.m as u64,
                plan.n as u64,
                plan.k as u64,
                &crate::analysis::gpu_strategy::GpuHardware::SM86,
            );
            // 2026-09-16 (L4): shallow-K at M*N >= 1024² has a
            // non-deterministic fill/compute race in the mw kernel
            // (BUGS.md 2026-09-16). Route to the race-free single-warp
            // S3b path (mw=1,nw=1 forces the fallback below).
            let mw_kernel_ok = !crate::analysis::gpu_strategy::shallow_k_race(
                plan.m as u64,
                plan.n as u64,
                plan.k as u64,
            );
            let (mw, nw, eff_stages) = if mw_kernel_ok {
                match strategy
                    .and_then(|s| strategy_to_mwnw(&s, warp_mh, plan.m, plan.n, thread_cap))
                {
                    Some((mw, nw, st)) => (mw, nw, st),
                    None => {
                        let (mw, nw) = select_mw_nw(plan.m, plan.n, thread_cap, warp_mh);
                        (mw, nw, stages)
                    }
                }
            } else {
                (1, 1, stages)
            };
            let mw_ok = plan.m % ((16 * warp_mh * mw) as i64) == 0
                && plan.n % ((8 * gr * nw) as i64) == 0;
            if mw_ok && (mw > 1 || nw > 1) {
                // 2026-09-15 (repro): wire the epilogue variant when a
                // fused scale applies (the f16 packed-f16x2 mul path).
                let ptx = match epilogue_scale {
                    Some(s) => tensor::tensor_gemm_ptx_smem_mw_epilogue(
                        plan.m, plan.n, plan.k, a_off, b_off, y_off, y_elem, mw, nw,
                        f16_acc, eff_stages, warp_mh, s,
                    ),
                    None => tensor::tensor_gemm_ptx_smem_mw(
                        plan.m, plan.n, plan.k, a_off, b_off, y_off, y_elem, mw, nw,
                        f16_acc, eff_stages, warp_mh,
                    ),
                };
                (
                    ptx,
                    true,
                    e.shape.count_expr.clone().unwrap_or(Expr::Decimal(0)),
                    (mw * nw * 32) as u32,
                    ((mw * warp_mh * 512 + nw * gr * 256) * eff_stages) as u32,
                )
            } else {
                // Single-warp smem kernel (32×16 tile, 1 warp).
                (
                    tensor::tensor_gemm_ptx_smem(plan.m, plan.n, plan.k, a_off, b_off, y_off, y_elem),
                    true,
                    e.shape.count_expr.clone().unwrap_or(Expr::Decimal(0)),
                    64,
                    0,
                )
            }
        } else {
            if a_elem != 4 {
                return Err(format!(
                    "ptx: node '{}': element size {} bytes — the PTX tier supports \
                     f32 (naive) or f16 with M%32=N%16=K%16=0 (tensor). f16 shapes \
                     that do not tile are not supported yet.\n  why: the tensor \
                     warp-tile geometry fixes M%32, N%16, K%16\n  fix: pad the \
                     shape to multiples of 32/16/16, or use --backend spirv",
                    name, a_elem
                ));
            }
            (
                naive_gemm_ptx(plan.m, plan.n, plan.k, a_elem, a_off, b_off, y_off, epilogue_scale),
                false,
                e.shape.count_expr.clone().unwrap_or(Expr::Decimal(0)),
                64,
                0,
            )
        };

        // 2026-09-11 (cubin shipping): prefer offline-ptxas cubin bytes;
        // the driver JIT ignores the register cap (166 vs 128 → 1 CTA/SM)
        // and wedges after fault storms. Fallback = PTX text (JIT path).
        let blob: Vec<u8> = if crate::config_tuning::ir_lowering().ptx_emit_cubin {
            match compile_cubin(&ptx, if f16_acc { 64 } else { 128 }) {
                Some(bytes) => bytes,
                None => ptx.into_bytes(),
            }
        } else {
            ptx.into_bytes()
        };
        out.push(RunnerKernel {
            name: name.clone(),
            spirv: blob,
            image_plans: Vec::new(),
            index_var: e.shape.index_var.clone(),
            count_expr,
            work_cols: None,
            cooperative: false,
            tiled: false,
            tensor: false,
            tensor_tile_rows: 1,
            ptx_tensor,
            fused_mma: false,
        fused_mma_blocks_div: 0,
            block_threads,
            shared_bytes,
            touched_fields: crate::backend::spirv::runner::kernel_touched_fields(&e.shape),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_mw_nw_respects_warp_aspect() {
        // The divisibility guards follow the warp shape: mhr=4 grows on
        // M%(64*mw)/N%(32*nw), mhr=2 on M%(32*mw)/N%(64*nw).
        assert_eq!(select_mw_nw(4096, 4096, 512, 4), (4, 4), "f16acc 4096^3");
        assert_eq!(select_mw_nw(4096, 4096, 512, 2), (4, 4), "mhr=2 4096^3");
        assert_eq!(select_mw_nw(4096, 4096, 256, 2), (4, 2), "f32 256-cap");
        // E4c (2026-09-13): mhr>=4 walks nw-first — (2,4)@256T beats both
        // (4,4)@512T (+2.3% at 4096^3) and (4,2)@256T (+14%) on-device.
        assert_eq!(select_mw_nw(4096, 4096, 256, 4), (2, 4), "f16acc E4c 4096^3");
        assert_eq!(select_mw_nw(2048, 2048, 256, 4), (2, 4), "f16acc E4c 2048^3");
        assert_eq!(select_mw_nw(8192, 8192, 256, 4), (2, 4), "f16acc E4c 8192^3");
        // 96 rows cannot tile a 64-row warp block (mhr=4): every growth
        // candidate fails the M%(16*mhr*mw) guard, so the selector returns
        // (1,1) and the dispatch falls back to the single-warp kernel.
        // mhr=2 tiles 96 rows (96%32==0), so nw grows to its cap.
        assert_eq!(select_mw_nw(96, 4096, 512, 4), (1, 1));
        assert_eq!(select_mw_nw(96, 4096, 512, 2), (1, 8));
    }

    #[test]
    fn naive_gemm_ptx_has_flat_grid_and_guards() {
        let ptx = naive_gemm_ptx(64, 64, 16, 4, 0, 65536, 131072, None);
        assert!(ptx.contains(".entry main"), "entry: {ptx}");
        assert!(ptx.contains("setp.ge.u32 %p1, %r3, 4096;"), "items guard: {ptx}");
        assert!(ptx.contains("div.u32 %r4, %r3, 64;"), "m = i/N: {ptx}");
        assert!(ptx.contains("rem.u32 %r5, %r3, 64;"), "n = i%N: {ptx}");
        assert!(ptx.contains("add.u64 %rd2, %rd1, 0;"), "a base: {ptx}");
        assert!(ptx.contains("add.u64 %rd3, %rd1, 65536;"), "b base: {ptx}");
        assert!(ptx.contains("add.u64 %rd4, %rd1, 131072;"), "y base: {ptx}");
        assert!(ptx.contains("ld.global.f32"), "f32 loads: {ptx}");
        assert!(ptx.contains("st.global.f32"), "f32 store: {ptx}");
    }

    #[test]
    fn naive_gemm_ptx_small_shape() {
        let ptx = naive_gemm_ptx(8, 16, 4, 4, 0, 1024, 2048, None);
        assert!(ptx.contains("setp.ge.u32 %p1, %r3, 128;"), "8*16 items: {ptx}");
        assert!(ptx.contains("div.u32 %r4, %r3, 16;"), "N: {ptx}");
        assert!(ptx.contains("setp.ge.u32 %p2, %r6, 4;"), "K: {ptx}");
    }

    #[test]
    fn fused_attention_ptx_stages_s_in_smem() {
        // 128×128 attention: the ONE fused kernel must stage the scaled S
        // tile in shared memory (the on-chip intermediate) and never write
        // the intermediate to global — only the output o is stored.
        let ptx = fused_attention_ptx(128, 128, 128, 128, 0, 1024, 2048, 4096, 0.5);
        assert!(ptx.contains(".shared .b16 s[512];"), "S smem tile: {ptx}");
        assert!(ptx.contains("st.shared.b16"), "S' written to smem: {ptx}");
        assert!(ptx.contains("ld.shared.b16"), "phase-2 reads S' from smem: {ptx}");
        assert!(ptx.contains("bar.sync 0"), "barrier between phases: {ptx}");
        // The only global store is the output o.
        let global_stores: Vec<&str> = ptx.lines().filter(|l| l.contains("st.global.b16")).collect();
        assert_eq!(global_stores.len(), 1, "only o stored to global: {:?}", global_stores);
        assert!(ptx.contains("5e-1"), "the middle scale folds into phase 1");
    }

    #[test]
    fn fused_attention_mma_uses_tensor_cores_and_smem_s() {
        // 128² attention: the ONE mma kernel must use the m16n8k16 tensor
        // cores, stage the scaled S tile in smem, barrier between phases,
        // and read S' back for the second GEMM.
        let ptx = fused_attention_mma_ptx(128, 128, 128, 128, 0, 1024, 2048, 4096, 0.5, 8);
        assert!(ptx.contains("mma.sync.aligned.m16n8k16"), "mma tensor cores: {ptx}");
        assert!(ptx.contains(".shared .align 16 .b8 s[4096];"), "16×128×2 smem S tile: {ptx}");
        assert!(ptx.contains("st.shared.b32"), "S' written to smem");
        assert!(ptx.contains("ld.shared.b32"), "phase-2 reads S' from smem");
        assert!(ptx.contains("bar.sync 0"), "barrier between phases");
        // Scale folds into phase 1 (f32 on the accumulator).
        assert!(ptx.contains("5e-1"), "the middle scale folds into phase 1");
        assert!(ptx.contains("%ctaid.y"), "m_tile decodes from ctaid.y (the 2D block grid)");
    }
}