//===----------------------------------------------------------------------===//
//
// Briev — the Briev programming language compiler.
//
// Copyright (c) 2026 Randy Smits-Schreuder Goedheijt <randozart@gmail.com>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// SPDX-License-Identifier: Apache-2.0
//
//===----------------------------------------------------------------------===//
/// Panel-pipeline codegen: the universal staged-pipeline skeleton shared by
/// every CUTLASS/cuBLAS kernel and our tensor GEMM / fused-attention kernels.
///
/// Captured from the decompiled cuBLAS HGEMM kernels (2026-09-16): every
/// pipelined tensor-core kernel on sm_86 follows the same skeleton:
///
///   1. Prologue: N_stages × fill (cp.async / LDGSTS + LDGDEPBAR)
///   2. Barrier: BAR.SYNC.DEFER_BLOCKING
///   3. Main loop: ldmatrix → mma → fill next stage → LDGDEPBAR → stage wrap
///   4. Epilogue: final barrier → mma → store
///
/// The fill patterns, ldmatrix configs, and mma shapes vary, but the
/// pipeline structure is universal. This module provides the skeleton as
/// parameterized PTX emitters; callers supply the panel-specific fills and
/// the mma body.
///
/// Goal: one mechanism serves every kernel shape — the compiler-engineering
/// generalization of the 35.5 TF staged-pipeline win.
///
/// Ref: `docs/plans/2026-09-16-panel-pipeline-generalization.md`
use std::fmt::Write;

/// Pipeline stage configuration. Power-of-2 stages use `& (n-1)` wrap;
/// stages=3 uses conditional subtract (the 2026-09-14 parity probe).
#[derive(Debug, Clone, Copy)]
pub struct StageConfig {
    pub stages: usize,
}

impl StageConfig {
    pub fn new(stages: usize) -> Self {
        assert!(
            stages.is_power_of_two() || stages == 3,
            "stages must be power-of-2 or 3, got {stages}"
        );
        Self { stages }
    }

    pub fn is_power_of_two(&self) -> bool {
        self.stages.is_power_of_two()
    }
}

/// Shared-memory layout for a multi-panel pipeline kernel.
/// Each panel type (A, B, ...) gets its own slab; stage buffers are
/// contiguous within each slab.
#[derive(Debug, Clone, Copy)]
pub struct SmemLayout {
    /// Bytes per stage for the A panel(s).
    pub a_per_stage: usize,
    /// Bytes per stage for the B panel(s).
    pub b_per_stage: usize,
    /// Total smem bytes (= stages × (a_per_stage + b_per_stage) + pad).
    pub total_bytes: usize,
}

impl SmemLayout {
    /// Compute the layout for a GEMM-style kernel with mw×nw warps.
    /// A slab: mw × mhr × 512 bytes per stage (the mw_opt convention).
    /// B slab: nw × gr × 256 bytes per stage.
    pub fn gemm(mw: usize, nw: usize, mhr: usize, gr: usize, stages: usize, pad: usize) -> Self {
        let a_per_stage = mw * mhr * 512;
        let b_per_stage = nw * gr * 256;
        let total_bytes = stages * (a_per_stage + b_per_stage) + pad;
        Self { a_per_stage, b_per_stage, total_bytes }
    }

    /// Layout for a fused-attention-style kernel where each panel is
    /// a row-strip of Q/Kt/V of `panel_bytes` per stage.
    pub fn fused(panel_bytes: usize, n_panels: usize, stages: usize) -> Self {
        let total_bytes = stages * n_panels * panel_bytes;
        Self { a_per_stage: panel_bytes, b_per_stage: 0, total_bytes }
    }
}

// ---------------------------------------------------------------------------
// Stage ring primitives
// ---------------------------------------------------------------------------

/// Emit `%r9 = %r9 % stages` (the compute-stage wrap at the KLOOP head).
/// Power-of-2 wraps with `& (stages-1)`; stages=3 uses `rem.u32`.
pub fn emit_stage_modulo(out: &mut String, stages: usize) {
    if stages.is_power_of_two() {
        write!(out, "    and.b32 %r9, %r9, {};\n", stages - 1).unwrap();
    } else {
        write!(out, "    rem.u32 %r9, %r9, {};\n", stages).unwrap();
    }
}

/// Emit `%r17 = (%r9 + stages-1) % stages` (the fill-stage wrap, the ring
/// predecessor of the compute stage). For power-of-2: `& (stages-1)`.
/// For stages=3: add stages-1, then conditional subtract.
pub fn emit_fill_stage(out: &mut String, stages: usize) {
    write!(out, "    add.u32 %r17, %r9, {};\n", stages - 1).unwrap();
    if stages.is_power_of_two() {
        write!(out, "    and.b32 %r17, %r17, {};\n", stages - 1).unwrap();
    } else {
        write!(out, "    setp.ge.u32 %p1, %r17, {};\n", stages as u32).unwrap();
        write!(out, "    @%p1 sub.u32 %r17, %r17, {};\n", stages as u32).unwrap();
    }
}

/// Emit the stage-counter XOR for a 2-stage ring (the CUTLASS/cuBLAS
/// convention: `UR ^= 1`). For >2 stages, use `emit_stage_modulo` instead.
pub fn emit_stage_xor2(out: &mut String, ur: &str) {
    write!(out, "    lop3.lut {}, {}, 0x1, {}, 0xc0, !PT;\n", ur, ur, "URZ").unwrap();
}

/// Emit LDGDEPBAR (the cp.async commit fence). This orders all preceding
/// cp.async fills before subsequent shared-memory reads. Lighter than
/// BAR.SYNC because it only orders the fill group, not the whole block.
pub fn emit_ldgdepbar(out: &mut String) {
    out.push_str("    ldgdepbar;\n");
}

/// Emit BAR.SYNC.DEFER_BLOCKING 0x0 (the full-block synchronization).
pub fn emit_bar_sync(out: &mut String) {
    out.push_str("    bar.sync.defer.blocking 0x0;\n");
}

/// Emit BAR.SYNC 0x0 (non-deferred, for epilogue or strict ordering).
pub fn emit_bar_sync_strict(out: &mut String) {
    out.push_str("    bar.sync 0x0;\n");
}

// ---------------------------------------------------------------------------
// Prologue: initial fill + barrier
// ---------------------------------------------------------------------------

/// Emit the pipeline prologue: fill the first N_stages into smem, with a
/// LDGDEPBAR after each fill group and a BAR.SYNC after the final fill.
///
/// `emit_fill` is called once per stage; it should emit the cp.async / LDGSTS
/// fills for that stage's panel data. The fill function receives the stage
/// index (0..stages-1).
pub fn emit_prologue<F>(out: &mut String, sc: StageConfig, mut emit_fill: F)
where
    F: FnMut(&mut String, usize),
{
    for stage in 0..sc.stages {
        emit_fill(out, stage);
        emit_ldgdepbar(out);
    }
    emit_bar_sync(out);
}

// ---------------------------------------------------------------------------
// Main loop body
// ---------------------------------------------------------------------------

/// Emit the main K-loop body skeleton. The caller supplies a single
/// `body` closure that emits fill-next, barrier, ldmatrix, mma, and
/// stage-wrap per iteration. The skeleton handles the loop label and
/// the k-step counter / branch.
pub fn emit_main_loop(
    out: &mut String,
    sc: StageConfig,
    k_step: i64,
    k_bound: i64,
    body: &mut dyn FnMut(&mut String, usize),
) {
    let loop_head = "KLOOP";

    write!(out, "{loop_head}:\n").unwrap();

    // The body closure emits: fill → ldgdepbar → bar.sync → load → compute → stage wrap.
    body(out, 0);

    // K-step increment and branch.
    write!(out, "    add.u32 %r2, %r2, {};\n", k_step).unwrap();
    write!(out, "    setp.ge.u32 %p1, %r2, {};\n", k_bound).unwrap();
    write!(out, "    @%p1 bra {loop_head};\n").unwrap();
}

// ---------------------------------------------------------------------------
// Epilogue: final mma + store
// ---------------------------------------------------------------------------

/// Emit the epilogue: wait for the last fill, run the final mma, store.
pub fn emit_epilogue<FLoad, FComp>(
    out: &mut String,
    mut emit_load_fragments: FLoad,
    mut emit_compute: FComp,
) where
    FLoad: FnMut(&mut String),
    FComp: FnMut(&mut String),
{
    emit_bar_sync(out);
    emit_load_fragments(out);
    emit_compute(out);
}

// ---------------------------------------------------------------------------
// Fill patterns (reusable cp.async / LDGSTS emitters)
// ---------------------------------------------------------------------------

/// Configuration for a single fill operation (one panel type, one stage).
#[derive(Debug, Clone, Copy)]
pub struct FillConfig {
    /// Fill width in bytes: 4 (cp.async.ca), 8 (.ca), or 16 (.cg).
    pub width: usize,
    /// Cache modifier: "cg" (L1-bypass, for 16-byte), "ca" (L1-cached, for 4/8-byte).
    pub cache_modifier: &'static str,
    /// Number of fill copies per thread.
    pub copies_per_thread: usize,
    /// Stride in bytes between consecutive copies (lanes × width).
    pub copy_stride: usize,
}

impl FillConfig {
    /// Derive the fill config from the per-thread work and thread count.
    /// The rung ladder: per_X % 4 == 0 → 16B .cg, else per_X % 2 == 0 → 8B .ca, else 4B .ca.
    pub fn derive(per_thread: usize, thread_count: usize) -> Self {
        let per = per_thread * thread_count;
        if per % 4 == 0 {
            Self { width: 16, cache_modifier: "cg", copies_per_thread: per / 4, copy_stride: thread_count * 16 }
        } else if per % 2 == 0 {
            Self { width: 8, cache_modifier: "ca", copies_per_thread: per / 2, copy_stride: thread_count * 8 }
        } else {
            Self { width: 4, cache_modifier: "ca", copies_per_thread: per, copy_stride: thread_count * 4 }
        }
    }
}

/// Emit a single cp.async fill (one 4/8/16-byte copy from global to smem).
/// `src_reg` is the global address register (e.g., `%rd5`).
/// `dst_reg` is the smem address register (e.g., `%r16`).
pub fn emit_cp_async(out: &mut String, dst_reg: &str, src_reg: &str, fc: &FillConfig) {
    write!(
        out,
        "    cp.async.{}.shared.global [{dst_reg}], [{src_reg}], {};\n",
        fc.cache_modifier, fc.width
    )
    .unwrap();
}

/// Emit a synchronous load+store pair (the warp-spec path's fallback).
pub fn emit_sync_fill(out: &mut String, dst_reg: &str, src_reg: &str, fc: &FillConfig) {
    match fc.width {
        16 => {
            write!(
                out,
                "    ld.global.nc.v4.u32 {{%r29, %r30, %r31, %r32}}, [{src_reg}];\n"
            )
            .unwrap();
            write!(
                out,
                "    st.shared.v4.u32 [{dst_reg}], {{%r29, %r30, %r31, %r32}};\n"
            )
            .unwrap();
        }
        8 => {
            write!(out, "    ld.global.nc.v2.u32 {{%r29, %r30}}, [{src_reg}];\n").unwrap();
            write!(out, "    st.shared.v2.u32 [{dst_reg}], {{%r29, %r30}};\n").unwrap();
        }
        _ => {
            write!(out, "    ld.global.u32 %r29, [{src_reg}];\n").unwrap();
            write!(out, "    st.shared.u32 [{dst_reg}], %r29;\n").unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// ldmatrix configuration
// ---------------------------------------------------------------------------

/// Configuration for ldmatrix fragment loads from smem.
#[derive(Debug, Clone, Copy)]
pub struct LdmatrixConfig {
    /// Number of B fragment groups (each group = 2 registers via x2.trans).
    pub b_groups: usize,
    /// Number of A fragment groups (each group = 1 register via x4 or x2).
    pub a_groups: usize,
    /// Whether B uses the transposed variant (MT88.4).
    pub b_transposed: bool,
}

impl LdmatrixConfig {
    pub fn gemm(mhr: usize, gr: usize) -> Self {
        Self { b_groups: gr, a_groups: mhr, b_transposed: true }
    }
}

/// Emit a set of ldmatrix.x2.trans for B fragments.
/// `base_reg`: smem address register for the stage base (e.g., `%rd5c`).
/// `lane_reg`: per-lane offset register (e.g., `%r25`).
/// `xor_reg`: swizzle register (e.g., `%r26`).
/// `set_off`: starting register index in the B register set.
pub fn emit_ldmatrix_b_trans(
    out: &mut String,
    gr: usize,
    set_off: usize,
    base_reg: &str,
    lane_reg: &str,
    xor_reg: &str,
) {
    for g in 0..gr {
        // Swizzle: lane ^ group_index, shifted left by 4 (16-byte granularity).
        write!(out, "    xor.b32 %r14, {xor_reg}, {g};\n").unwrap();
        out.push_str("    shl.b32 %r14, %r14, 4;\n");
        write!(out, "    add.u32 %r17, {lane_reg}, %r14;\n").unwrap();
        out.push_str("    mul.wide.u32 %rd5, %r17, 1;\n");
        write!(out, "    add.u64 %rd5, {base_reg}, %rd5;\n").unwrap();
        write!(
            out,
            "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{%b{}, %b{}}}, [%rd5];\n",
            set_off + 2 * g,
            set_off + 2 * g + 1
        )
        .unwrap();
    }
}

/// Emit ldmatrix.x4 for A fragments (4 matrices loaded at once).
/// `base_reg`: smem address register.
/// `set_off`: starting A register index.
pub fn emit_ldmatrix_a_x4(out: &mut String, set_off: usize, base_reg: &str) {
    write!(
        out,
        "    ldmatrix.sync.aligned.m8n8.x4.shared.b16 {{%a{}, %a{}, %a{}, %a{}}}, [{base_reg}];\n",
        set_off, set_off + 1, set_off + 2, set_off + 3
    )
    .unwrap();
}

/// Emit ldmatrix.x2 for A fragments (2 matrices, non-transposed).
pub fn emit_ldmatrix_a_x2(out: &mut String, set_off: usize, base_reg: &str) {
    write!(
        out,
        "    ldmatrix.sync.aligned.m8n8.x2.shared.b16 {{%a{}, %a{}}}, [{base_reg}];\n",
        set_off, set_off + 1
    )
    .unwrap();
}

/// Emit ldmatrix.x2.trans for B fragments (2 matrices, transposed).
pub fn emit_ldmatrix_b_x2_trans(out: &mut String, set_off: usize, base_reg: &str) {
    write!(
        out,
        "    ldmatrix.sync.aligned.m8n8.x2.trans.shared.b16 {{%b{}, %b{}}}, [{base_reg}];\n",
        set_off, set_off + 1
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// B-slab base computation (the stage-dependent smem address)
// ---------------------------------------------------------------------------

/// Emit the B-slab base address: `base = smem_base + stage * bsmem_buf + warp * gr * 256`.
/// `stage_reg`: register holding the stage index.
/// `bsmem_buf`: bytes per B-slab (nw × gr × 256).
/// `gr`: number of B column groups.
/// Result in `%rd5c`.
pub fn emit_b_slab_base(out: &mut String, stage_reg: &str, bsmem_buf: usize, gr: usize) {
    out.push_str("    mov.u64 %rd5c, %rd9;\n");
    write!(out, "    mul.lo.u32 %r12, {stage_reg}, {bsmem_buf};\n").unwrap();
    out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
    out.push_str("    add.u64 %rd5c, %rd5c, %rd5b;\n");
    out.push_str("    mov.u32 %r12, %r11;\n");
    write!(out, "    mul.lo.u32 %r12, %r12, {};\n", gr * 256).unwrap();
    out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
    out.push_str("    add.u64 %rd5c, %rd5c, %rd5b;\n");
}

/// Emit the A-slab base address: `base = smem_base + stage * asmem_buf`.
/// `stage_reg`: register holding the stage index.
/// `asmem_buf`: bytes per A-slab (mw × mhr × 512).
/// Result in `%rd4c` (or caller-chosen register).
pub fn emit_a_slab_base(out: &mut String, stage_reg: &str, asmem_buf: usize) {
    out.push_str("    mov.u64 %rd4c, %rd9;\n");
    write!(out, "    mul.lo.u32 %r12, {stage_reg}, {asmem_buf};\n").unwrap();
    out.push_str("    mul.wide.u32 %rd5b, %r12, 1;\n");
    out.push_str("    add.u64 %rd4c, %rd4c, %rd5b;\n");
}

// ---------------------------------------------------------------------------
// f16 utilities (the epilogue scale path)
// ---------------------------------------------------------------------------

/// Pack an f16 value into both halves of a u32 (the f16x2 pair for mma scale).
pub fn f16x2_pair(k: f64) -> Option<u32> {
    let bits = f32_to_f16(k as f32);
    Some((bits as u32) | ((bits as u32) << 16))
}

/// IEEE-754 round-to-nearest-even f32 → f16.
pub fn f32_to_f16(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 127;
    let frac = b & 0x7fffff;
    if exp > 15 {
        return sign | 0x7c00 | if frac != 0 { 0x200 } else { 0 };
    }
    if exp < -14 {
        return sign;
    }
    let mut h = sign | (((exp + 15) as u16) << 10);
    if exp == -14 && frac != 0 {
        let m = frac | 0x800000;
        let s = 14 - exp;
        h |= (m >> s) as u16;
    } else {
        h |= (frac >> 13) as u16;
    }
    h
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_modulo_pow2() {
        let mut out = String::new();
        emit_stage_modulo(&mut out, 4);
        assert!(out.contains("and.b32 %r9, %r9, 3"));
    }

    #[test]
    fn stage_modulo_non_pow2() {
        let mut out = String::new();
        emit_stage_modulo(&mut out, 3);
        assert!(out.contains("rem.u32 %r9, %r9, 3"));
    }

    #[test]
    fn fill_stage_pow2() {
        let mut out = String::new();
        emit_fill_stage(&mut out, 4);
        assert!(out.contains("add.u32 %r17, %r9, 3"));
        assert!(out.contains("and.b32 %r17, %r17, 3"));
    }

    #[test]
    fn fill_stage_3() {
        let mut out = String::new();
        emit_fill_stage(&mut out, 3);
        assert!(out.contains("add.u32 %r17, %r9, 2"));
        assert!(out.contains("setp.ge.u32 %p1, %r17, 3"));
        assert!(out.contains("@%p1 sub.u32 %r17, %r17, 3"));
    }

    #[test]
    fn ldgdepbar() {
        let mut out = String::new();
        emit_ldgdepbar(&mut out);
        assert!(out.contains("ldgdepbar"));
    }

    #[test]
    fn bar_sync() {
        let mut out = String::new();
        emit_bar_sync(&mut out);
        assert!(out.contains("bar.sync.defer.blocking 0x0"));
    }

    #[test]
    fn fill_config_derive_16b() {
        let fc = FillConfig::derive(2, 256); // per=512, 512%4==0
        assert_eq!(fc.width, 16);
        assert_eq!(fc.cache_modifier, "cg");
        assert_eq!(fc.copies_per_thread, 128);
    }

    #[test]
    fn fill_config_derive_8b() {
        let fc = FillConfig::derive(1, 256); // per=256, 256%4==0 → still 16B
        let fc2 = FillConfig::derive(1, 128); // per=128, 128%4==0 → 16B
        assert_eq!(fc2.width, 16);
    }

    #[test]
    fn f32_to_f16_basic() {
        assert_eq!(f32_to_f16(0.0), 0);
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(0.5), 0x3800);
    }

    #[test]
    fn f16x2_pair_some() {
        let p = f16x2_pair(0.5).unwrap();
        assert_eq!(p, 0x3800_3800);
    }

    #[test]
    fn smem_layout_gemm() {
        let layout = SmemLayout::gemm(4, 2, 2, 4, 3, 0);
        // a_per_stage = 4*2*512 = 4096
        // b_per_stage = 2*4*256 = 2048
        // total = 3*(4096+2048) = 18432
        assert_eq!(layout.a_per_stage, 4096);
        assert_eq!(layout.b_per_stage, 2048);
        assert_eq!(layout.total_bytes, 18432);
    }

    #[test]
    fn prologue_emits_fills_and_barrier() {
        let mut out = String::new();
        let sc = StageConfig::new(2);
        emit_prologue(&mut out, sc, |_out, _stage| {
            // dummy fill
        });
        // 2 LDGDEPBARs + 1 BAR.SYNC
        assert_eq!(out.lines().filter(|l| l.contains("ldgdepbar")).count(), 2);
        assert_eq!(out.lines().filter(|l| l.contains("bar.sync")).count(), 1);
    }

    #[test]
    fn xor2_stage() {
        let mut out = String::new();
        emit_stage_xor2(&mut out, "UR4");
        assert!(out.contains("lop3.lut UR4, UR4, 0x1"));
    }
}
