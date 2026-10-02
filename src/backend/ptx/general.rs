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
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
//===----------------------------------------------------------------------===//
//! General PTX kernel emitter (the S5 anchor, elementwise-lite).
//!
//! The PTX tier was GEMM-shaped only (S2a). The gpu_schedule tier needs
//! non-GEMM kernels (the elementwise row-ops between GEMMs in an
//! attention-decode graph). This emitter lowers an eligible accel node's
//! `kernel_stmts` — the offloadable, pure, affine body — to a flat 1D PTX
//! kernel:
//!
//! ```text
//! gid = ctaid.x * BLOCK + tid.x;   // the index_var binds to gid
//! if (gid >= N) ret;
//! <body with index_var := gid>
//! ```
//!
//! Surface (honest): assignments over `buf[index_var]`, scalars, and
//! literals; binary +-*/ and %; unary -; `let` locals. Everything else is
//! a gen-time error naming the fix (the surface gate keeps the emitter
//! honest — no silent general lowering). GEMM-shaped nodes keep the
//! tensor path; this emitter serves the rest.

use crate::analysis::accel::KernelShape;
use crate::ast::{BinaryOpKind, Expr, Statement};
use crate::backend::spirv::runner::SsboLayout;

// 2026-09-18 (M3 composition debug): BLOCK must match the CUDA driver's
// default block_threads (64). The old value (256) caused gid = cta*256+tid
// to scramble thread→element mapping when the launch used 64 threads/block.
const BLOCK: u32 = 64;

/// Emit the general 1D PTX kernel for one eligible node.
/// Cooperative row-softmax PTX (2026-09-17, M2a increment 3 — the CUDA-lane
/// twin of the SPIR-V `synthesize_softmax_stmts` lowering).
///
/// Grid contract (matches the CUDA driver's launch_dev2d): block (32,1,1),
/// grid (1, rows) — `ctaid.y` is the row, `tid.x` is the lane. Three
/// strided passes (c = lane, lane+32, ... < inner; inner % 32 == 0 so
/// every access is exactly in-bounds), with butterfly shuffle-tree
/// reductions between them (redux.f32 is sm_100+; NOT available on sm_86).
/// exp(x) = ex2(x * log2(e)) — there is no exp instruction in PTX.
/// All ptxas-verified forms (2026-09-17 isolation sweep).
pub fn emit_cooperative_softmax_ptx(
    inner: u64,
    rows: u64,
    row_buf: &str,
    out_buf: &str,
    layout: &SsboLayout,
) -> Result<String, String> {
    if inner == 0 || inner % 32 != 0 {
        return Err(format!(
            "cooperative softmax needs a row length divisible by 32 (got {inner})"
        ));
    }
    if rows == 0 || rows > 65535 {
        return Err(format!(
            "cooperative softmax row count {rows} outside the CUDA gridDim.y range 1..=65535"
        ));
    }
    let off = |name: &str| -> Result<u64, String> {
        layout
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.proj_offset)
            .ok_or_else(|| format!("ptx softmax: buffer '{name}' not in layout"))
    };
    let off_row = off(row_buf)?;
    let off_out = off(out_buf)?;
    let elem = |name: &str| -> Result<u64, String> {
        layout
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.elem_bytes as u64)
            .ok_or_else(|| format!("ptx softmax: buffer '{name}' not in layout"))
    };
    if elem(row_buf)? != 4 || elem(out_buf)? != 4 {
        return Err("cooperative softmax operates on f32 rows (elem_bytes 4)".into());
    }

    // Fixed register allocation — the kernel is small and straight-line
    // per phase; numbered regs keep the emission readable.
    let mut decl = String::new();
    let mut body = String::new();
    decl.push_str("    .reg .b64 %rd1, %rd2;\n");
    decl.push_str("    .reg .u32 %r1, %r2, %r3, %r4, %r5;\n");
    decl.push_str("    .reg .b32 %r6, %r7;\n");
    decl.push_str("    .reg .pred %p1;\n");
    decl.push_str("    .reg .f32 %f1, %f2, %f3, %f4, %f5;\n");

    // row = ctaid.y; bounds; lane = tid.x; base = row * inner
    body.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    body.push_str("    mov.u32 %r1, %ctaid.y;\n");
    body.push_str(&format!("    setp.ge.u32 %p1, %r1, {rows};\n"));
    body.push_str("    @%p1 ret;\n");
    body.push_str("    mov.u32 %r2, %tid.x;\n");
    body.push_str(&format!("    mul.lo.u32 %r3, %r1, {inner};\n"));
    // TEMP debug: out[base] = row + 1 (probe row execution)
    body.push_str("    add.u32 %r5, %r3, %r2;\n");
    body.push_str("    mul.wide.u32 %rd2, %r5, 4;\n");
    body.push_str("    add.u64 %rd2, %rd2, %rd1;\n");
    body.push_str(&format!("    add.u64 %rd2, %rd2, {off_out};\n"));
    body.push_str("    cvt.rn.f32.s32 %f1, %r1;\n");
    body.push_str("    add.f32 %f1, %f1, 0f3F800000;\n");
    body.push_str("    st.global.f32 [%rd2], %f1;\n");

    // addr(row_buf, base + c) → %rd2 — the shared load sequence.
    let load_row = |body: &mut String| {
        body.push_str("    add.u32 %r5, %r3, %r4;\n");
        body.push_str("    mul.wide.u32 %rd2, %r5, 4;\n");
        body.push_str("    add.u64 %rd2, %rd2, %rd1;\n");
        body.push_str(&format!("    add.u64 %rd2, %rd2, {off_row};\n"));
        body.push_str("    ld.global.f32 %f1, [%rd2];\n");
    };

    // Phase MAX — strided fold into %f2, init -inf.
    body.push_str("    mov.f32 %f2, 0fFF800000;\n");
    body.push_str("    mov.u32 %r4, %r2;\n");
    body.push_str("Lmax0:\n");
    body.push_str(&format!("    setp.ge.u32 %p1, %r4, {inner};\n"));
    body.push_str("    @%p1 bra Lmax1;\n");
    load_row(&mut body);
    body.push_str("    max.f32 %f2, %f2, %f1;\n");
    body.push_str("    add.u32 %r4, %r4, 32;\n");
    body.push_str("    bra Lmax0;\n");
    body.push_str("Lmax1:\n");

    // Butterfly reduction (5 rounds) — op-selectable.
    let mut butterfly = |body: &mut String, val: &str, op: &str| {
        for offset in [16u32, 8, 4, 2, 1] {
            body.push_str(&format!("    mov.b32 %r6, {};\n", val));
            body.push_str(&format!(
                "    shfl.sync.bfly.b32 %r7, %r6, {offset}, 0x1f, 0xffffffff;\n"
            ));
            body.push_str("    mov.b32 %f5, %r7;\n");
            body.push_str(&format!("    {} {}, {}, %f5;\n", op, val, val));
        }
    };
    butterfly(&mut body, "%f2", "max.f32");

    // Phase SUM — s = Σ exp(v - m).
    body.push_str("    mov.f32 %f3, 0f00000000;\n");
    body.push_str("    mov.u32 %r4, %r2;\n");
    body.push_str("Lsum0:\n");
    body.push_str(&format!("    setp.ge.u32 %p1, %r4, {inner};\n"));
    body.push_str("    @%p1 bra Lsum1;\n");
    load_row(&mut body);
    body.push_str("    sub.f32 %f4, %f1, %f2;\n");
    body.push_str("    mul.f32 %f4, %f4, 0f3FB8AA3B;\n");
    body.push_str("    ex2.approx.f32 %f1, %f4;\n");
    body.push_str("    add.f32 %f3, %f3, %f1;\n");
    body.push_str("    add.u32 %r4, %r4, 32;\n");
    body.push_str("    bra Lsum0;\n");
    body.push_str("Lsum1:\n");
    butterfly(&mut body, "%f3", "add.f32");

    // Phase NORM — out[base + c] = exp(v - m) / s.
    body.push_str("    mov.u32 %r4, %r2;\n");
    body.push_str("Lnorm0:\n");
    body.push_str(&format!("    setp.ge.u32 %p1, %r4, {inner};\n"));
    body.push_str("    @%p1 bra Lnorm1;\n");
    load_row(&mut body);
    body.push_str("    sub.f32 %f4, %f1, %f2;\n");
    body.push_str("    mul.f32 %f4, %f4, 0f3FB8AA3B;\n");
    body.push_str("    ex2.approx.f32 %f1, %f4;\n");
    body.push_str("    div.rn.f32 %f1, %f1, %f3;\n");
    body.push_str("    add.u32 %r5, %r3, %r4;\n");
    body.push_str("    mul.wide.u32 %rd2, %r5, 4;\n");
    body.push_str("    add.u64 %rd2, %rd2, %rd1;\n");
    body.push_str(&format!("    add.u64 %rd2, %rd2, {off_out};\n"));
    body.push_str("    st.global.f32 [%rd2], %f1;\n");
    body.push_str("    add.u32 %r4, %r4, 32;\n");
    body.push_str("    bra Lnorm0;\n");
    body.push_str("Lnorm1:\n");
    body.push_str("    ret;\n");

    Ok(format!(
        ".version 8.0\n.target sm_86\n.address_size 64\n.visible .entry main (.param .b64 proj_param)\n{{\n{}\n{}\n}}\n",
        decl, body
    ))
}

/// Cooperative row dot-product PTX (2026-09-17, M2a) — `y[i] = Σ_k a[i*K+k]
/// * x[k]`, the CUDA-lane twin of the SPIR-V cooperative dot lowering.
/// Same grid contract as emit_cooperative_softmax_ptx: block (32,1,1),
/// grid (1, rows), ctaid.y = row, tid.x = lane; one strided pass, one
/// butterfly add-tree, one store per row.
pub fn emit_cooperative_dot_ptx(
    inner: u64,
    rows: u64,
    row_buf: &str,
    col_buf: &str,
    out_buf: &str,
    layout: &SsboLayout,
) -> Result<String, String> {
    if inner == 0 || inner % 32 != 0 {
        return Err(format!(
            "cooperative dot needs a reduction length divisible by 32 (got {inner})"
        ));
    }
    if rows == 0 || rows > 65535 {
        return Err(format!(
            "cooperative dot row count {rows} outside the CUDA gridDim.y range 1..=65535"
        ));
    }
    let off = |name: &str| -> Result<u64, String> {
        layout
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.proj_offset)
            .ok_or_else(|| format!("ptx dot: buffer '{name}' not in layout"))
    };
    let (off_row, off_col, off_out) = (off(row_buf)?, off(col_buf)?, off(out_buf)?);
    for name in [row_buf, col_buf, out_buf] {
        if elem_bytes_of(layout, name)? != 4 {
            return Err(format!(
                "cooperative dot operates on f32 buffers ('{name}' is not)"
            ));
        }
    }


    let mut decl = String::new();
    let mut body = String::new();
    decl.push_str("    .reg .b64 %rd1, %rd2;\n");
    decl.push_str("    .reg .u32 %r1, %r2, %r3, %r4, %r5;\n");
    decl.push_str("    .reg .b32 %r6, %r7;\n");
    decl.push_str("    .reg .pred %p1;\n");
    decl.push_str("    .reg .f32 %f1, %f2, %f3, %f4, %f5;\n");

    body.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    body.push_str("    mov.u32 %r1, %ctaid.y;\n");
    body.push_str(&format!("    setp.ge.u32 %p1, %r1, {rows};\n"));
    body.push_str("    @%p1 ret;\n");
    body.push_str("    mov.u32 %r2, %tid.x;\n");
    body.push_str(&format!("    mul.lo.u32 %r3, %r1, {inner};\n"));

    // acc = 0; strided mul-add over the row.
    body.push_str("    mov.f32 %f2, 0f00000000;\n");
    body.push_str("    mov.u32 %r4, %r2;\n");
    body.push_str("Ldot0:\n");
    body.push_str(&format!("    setp.ge.u32 %p1, %r4, {inner};\n"));
    body.push_str("    @%p1 bra Ldot1;\n");
    // a[base + c]
    body.push_str("    add.u32 %r5, %r3, %r4;\n");
    body.push_str("    mul.wide.u32 %rd2, %r5, 4;\n");
    body.push_str("    add.u64 %rd2, %rd2, %rd1;\n");
    body.push_str(&format!("    add.u64 %rd2, %rd2, {off_row};\n"));
    body.push_str("    ld.global.f32 %f1, [%rd2];\n");
    // x[c]
    body.push_str("    mul.wide.u32 %rd2, %r4, 4;\n");
    body.push_str("    add.u64 %rd2, %rd2, %rd1;\n");
    body.push_str(&format!("    add.u64 %rd2, %rd2, {off_col};\n"));
    body.push_str("    ld.global.f32 %f3, [%rd2];\n");
    body.push_str("    fma.rn.f32 %f2, %f1, %f3, %f2;\n");
    body.push_str("    add.u32 %r4, %r4, 32;\n");
    body.push_str("    bra Ldot0;\n");
    body.push_str("Ldot1:\n");
    // Butterfly add-tree → every lane holds the row total.
    for offset in [16u32, 8, 4, 2, 1] {
        body.push_str("    mov.b32 %r6, %f2;\n");
        body.push_str(&format!(
            "    shfl.sync.bfly.b32 %r7, %r6, {offset}, 0x1f, 0xffffffff;\n"
        ));
        body.push_str("    mov.b32 %f5, %r7;\n");
        body.push_str("    add.f32 %f2, %f2, %f5;\n");
    }
    // y[row] = acc
    body.push_str("    mul.wide.u32 %rd2, %r1, 4;\n");
    body.push_str("    add.u64 %rd2, %rd2, %rd1;\n");
    body.push_str(&format!("    add.u64 %rd2, %rd2, {off_out};\n"));
    body.push_str("    st.global.f32 [%rd2], %f2;\n");
    body.push_str("    ret;\n");

    Ok(format!(
        ".version 8.0\n.target sm_86\n.address_size 64\n.visible .entry main (.param .b64 proj_param)\n{{\n{}\n{}\n}}\n",
        decl, body
    ))
}

fn elem_bytes_of(layout: &SsboLayout, name: &str) -> Result<u64, String> {
    layout
        .fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.elem_bytes as u64)
        .ok_or_else(|| format!("ptx: field '{name}' not in layout"))
}

/// 2026-09-19 (M1 warp-sliced reductions, plan general-machinery):
/// detect a top-level serial foreach-reduce long enough to split across
/// the block's warps: the body matches the serial-unroll shape (single
/// accumulator, loads linear in the item), the span meets the
/// `ptx_warp_slice_min_span` threshold, and it divides the
/// `ptx_warp_slice_warps` count exactly (exact slices, no tail).
/// Used by both the emitter (lowering choice) and the runner
/// (block-per-workitem dispatch, block_threads warps*32). Structural
/// match, same discipline as has_lane_reduction. Thresholds come from
/// config since 2026-09-30 (stage-5 5c) — one predicate
/// (`warp_slice_span_ok`) shared with the emission gate.
pub fn has_warp_slice(
    kernel_stmts: &[Statement],
    consts: &std::collections::HashMap<String, Expr>,
) -> bool {
    let cfg = crate::config_tuning::ir_lowering();
    has_warp_slice_with(
        kernel_stmts,
        consts,
        cfg.ptx_warp_slice_min_span,
        cfg.ptx_warp_slice_warps,
    )
}

/// The detector with explicit thresholds — testable without mutating the
/// process-wide config (the CLI installs overrides via a OnceLock that
/// must not be poisoned by unit tests). `has_warp_slice` is the config
/// reading wrapper.
pub fn has_warp_slice_with(
    kernel_stmts: &[Statement],
    consts: &std::collections::HashMap<String, Expr>,
    min_span: u32,
    warps: u32,
) -> bool {
    for stmt in kernel_stmts {
        let Statement::Foreach {
            item,
            list,
            body,
            ..
        } = stmt
        else {
            continue;
        };
        let Expr::Range { start, end, .. } = list.as_ref() else {
            continue;
        };
        let fold = |e: &Expr| -> Option<i64> {
            match e {
                Expr::Decimal(n) => Some(*n),
                Expr::Identifier(name) => match consts.get(name) {
                    Some(Expr::Decimal(w)) => Some(*w),
                    _ => None,
                },
                _ => None,
            }
        };
        let (Some(s), Some(e)) = (fold(start), fold(end)) else {
            continue;
        };
        let span = e - s;
        if !warp_slice_span_ok(span, min_span, warps) {
            continue;
        }
        let ctx = UnrollCtx {
            item,
            start: s,
            end: e,
            unroll: 4,
        };
        if SerialUnrollPlan::match_body(body, &ctx, consts).is_some() {
            return true;
        }
    }
    false
}

/// 2026-09-30 (stage-5 5c, plan 2026-09-30-stage5-re-rank-and-5c): THE
/// warp-slice eligibility predicate — one source for the detector
/// (`has_warp_slice_with`) and the emission gate (the foreach arm),
/// which previously duplicated `span >= 512 && span % 4 == 0`. `min_span`
/// and `warps` come from config/ir-lowering.dbvl; defaults reproduce the
/// original constants byte-identically. `warps == 0` is guarded even
/// though the config loader clamps to 1..=8 (defensive against a future
/// caller passing raw values — a zero divisor would panic).
fn warp_slice_span_ok(span: i64, min_span: u32, warps: u32) -> bool {
    span >= min_span as i64 && warps > 0 && span % warps as i64 == 0
}

/// The warp-sliced dispatch block size: warps*32 threads (warp = 32 is a
/// hardware fact and stays in the backend). Single source for the
/// emitter's `block_threads` arm and the mod.rs dispatch desc — they
/// must lockstep or the runtime launches the wrong geometry (bug-14
/// class).
pub(crate) fn warp_slice_block_threads(warps: u32) -> u32 {
    warps * 32
}

/// 2026-09-30 (stage-5 5c): the warp-slice emission inputs, bundled
/// (plan + decl + body — 4 parameters; the pre-5c signature was already
/// over the house limit at 7). `warps` derives from config
/// (`ptx_warp_slice_warps`); `start`/`span` come from the matched range.
struct WarpSliceEmit<'a> {
    body_stmts: &'a [Statement],
    item: &'a str,
    start: i64,
    span: i64,
    warps: u32,
}

/// 2026-09-18 (P1 lane-coverage fix): detect whether `kernel_stmts`
/// contains a foreach whose body has a lane-mappable inner reduction.
/// Used by both the PTX emitter (guard shape) and the runner (dispatch
/// geometry).  Structural match: the kernel's top-level statements must
/// include a Foreach whose body calls `LaneReductionPlan::match_body`.
pub fn has_lane_reduction(
    kernel_stmts: &[Statement],
    consts: &std::collections::HashMap<String, Expr>,
) -> bool {
    for stmt in kernel_stmts {
        if let Statement::Foreach { body, .. } = stmt {
            if LaneReductionPlan::match_body(body, consts).is_some() {
                return true;
            }
        }
    }
    false
}

/// 2026-09-30: emit options (keeps [`emit_general_ptx`] within the
/// parameter budget).
#[derive(Default)]
pub struct GeneralEmitOpts {
    pub int_bits: u64,
    /// Deferred-region CTA split factor (1 = none) — the general
    /// reduction-split pass.
    pub deferred_split: u64,
    /// 2026-10-01 (D14 remainder): the declared `unroll<N>` node
    /// modifier — the D2 override of the derived serial-unroll factor
    /// (`ptx_serial_unroll`). Resolved in the FRONTEND
    /// (`declared_modifier`) and read here; `None` = the derived
    /// default, byte-identical behavior.
    pub unroll_override: Option<u64>,
}

pub fn emit_general_ptx(
    shape: &KernelShape,
    count: i64,
    layout: &SsboLayout,
    consts: &std::collections::HashMap<String, Expr>,
    universe: &crate::type_universe::TypeUniverse,
    opts: GeneralEmitOpts,
) -> Result<String, String> {
    let mut g = Gen::new(layout, consts, count, universe, opts.int_bits);
    g.deferred_split = opts.deferred_split.max(1);
    g.skip_pass = crate::config_tuning::ir_lowering().ptx_deferred_skip_pass;
    g.unroll_override = opts.unroll_override;
    g.emit(shape)
}

/// 2026-09-18 (P1, plan fused-f16-decode-node): a lane-mapped inner
/// reduction found in a foreach body — [Let acc = 0, Foreach d in range {
/// acc = acc + <mul> }, ..rest reading acc..]. The matcher is structural
/// (no type names): the inner loop must be a SINGLE self-referencing Add
/// assign over a constant range divisible by the warp width (uniform
/// convergence for the in-loop butterfly), and the accumulator must be
/// consumed by the statements after it.
struct LaneReductionPlan {
    /// accumulator name (`p` in the probe)
    acc: String,
    /// inner loop item (`d`)
    inner_item: String,
    inner_start: i64,
    inner_end: i64,
    /// the inner loop's single assign (lowered with d bound to the lane
    /// strided register)
    inner_body: Vec<Statement>,
}

impl LaneReductionPlan {
    /// Find the lane-mappable inner foreach: returns (position in `body`,
    /// plan). The position lets the emitter lower preceding statements
    /// normally before switching to the lane-mapped form.
    fn match_body(
        body: &[Statement],
        consts: &std::collections::HashMap<String, Expr>,
    ) -> Option<(usize, LaneReductionPlan)> {
        // Range bounds resolve through the module consts (foreach d in
        // 0..D carries the IDENTIFIER D at this stage).
        let const_int = |e: &Expr| -> Option<i64> {
            match e {
                Expr::Decimal(v) => Some(*v),
                Expr::Identifier(n) => consts.get(n).and_then(|v| {
                    match v {
                        Expr::Decimal(w) => Some(*w),
                        _ => None,
                    }
                }),
                _ => None,
            }
        };
        for (pos, stmt) in body.iter().enumerate() {
            let Statement::Foreach {
                item,
                list,
                body: inner,
            } = stmt
            else {
                continue;
            };
            if inner.len() != 1 {
                continue;
            }
            // Self-referencing Add assign: acc = acc + <something>.
            let Statement::Assign(Expr::Identifier(acc), rhs) = &inner[0] else {
                continue;
            };
            let Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, r) = rhs else {
                continue;
            };
            let recurses = matches!(l.as_ref(), Expr::Identifier(n) if n == acc)
                || matches!(r.as_ref(), Expr::Identifier(n) if n == acc);
            if !recurses {
                continue;
            }
            // Constant range, warp-divisible (uniform lane iterations).
            let Expr::Range {
                start,
                end,
                inclusive,
            } = list.as_ref()
            else {
                continue;
            };
            let (Some(s), Some(e)) = (const_int(start), const_int(end)) else {
                continue;
            };
            let span = if *inclusive { e - s + 1 } else { e - s };
            if span <= 0 || span % 32 != 0 {
                continue;
            }
            // The accumulator must be consumed AFTER the loop (otherwise
            // the mapping is unobservable and the serial path is fine).
            let consumed = body[pos + 1..]
                .iter()
                .any(|st| stmt_mentions_ident(st, acc));
            if !consumed {
                continue;
            }
            return Some((
                pos,
                LaneReductionPlan {
                    acc: acc.clone(),
                    inner_item: item.clone(),
                    inner_start: s,
                    inner_end: e,
                    inner_body: inner.clone(),
                },
            ));
        }
        None
    }
}

/// 2026-09-19 (serial-loop unroll, plan flash-decode-gate): a SERIAL
/// reduction foreach — the work-item loop was already decomposed, so the
/// reduction loop runs whole inside every thread and the *coalescing*
/// comes from neighbouring work items (consecutive gids reading
/// consecutive addresses). Lane-mapping such a loop would wreck that
/// coalescing (M1 lesson), so the fix is issue-rate, not mapping: unroll
/// the loop by N with per-site running byte pointers and N independent
/// load registers per site (no false WAR deps), preserving the strict
/// left-to-right accumulation order (bit-exact vs the serial form).
/// Measured on the pv kernel (bitnet geometry, RTX 3060): 212 -> 153 µs
/// at N=4, 146 µs at N=8 — N defaults to 4 (`ptx_serial_unroll`).
struct SerialUnrollPlan {
    /// the loop variable (also the non-site substitution key)
    item: String,
    /// unroll factor the plan was matched for
    unroll: usize,
    /// distinct load sites whose address is linear in the loop item with
    /// nonzero coefficient: (the exact Index expr — the `pipelined` key —
    /// coefficient of the item in the address)
    sites: Vec<(Expr, i64)>,
    start: i64,
    /// exclusive end (inclusive ranges normalized)
    end: i64,
}

/// A resolved preload site: the `pipelined` key, the running base address
/// register, the per-item byte stride, and the element width.
struct UnrollSite {
    key: String,
    base: String,
    stride_bytes: i64,
    elem: u64,
}

/// Coefficient of `item` in the linear address form `a*item + c`:
/// `Some(a)`, or `None` when the item appears non-linearly (nested index,
/// divide, product of two item terms, unknown scalar factor). Everything
/// else is treated as a loop-invariant contribution.
fn linear_coeff(
    e: &Expr,
    item: &str,
    consts: &std::collections::HashMap<String, Expr>,
) -> Option<i64> {
    match e {
        Expr::Decimal(_) => Some(0),
        Expr::Identifier(n) if n == item => Some(1),
        Expr::Identifier(_) => Some(0),
        // A nested index inside an address (k[ii[j]]) is neither linearly
        // addressable nor lowerable by emit_index — reject the body.
        Expr::Index(..) => None,
        Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, r) => {
            Some(linear_coeff(l, item, consts)? + linear_coeff(r, item, consts)?)
        }
        Expr::BinaryOp(crate::ast::BinaryOpKind::Sub, l, r) => {
            Some(linear_coeff(l, item, consts)? - linear_coeff(r, item, consts)?)
        }
        Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) => {
            let la = linear_coeff(l, item, consts)?;
            let ra = linear_coeff(r, item, consts)?;
            let cv = |x: &Expr| -> Option<i64> {
                match x {
                    Expr::Decimal(n) => Some(*n),
                    Expr::Identifier(n) => match consts.get(n) {
                        Some(Expr::Decimal(w)) => Some(*w),
                        _ => None,
                    },
                    _ => None,
                }
            };
            if la != 0 && ra != 0 {
                return None; // quadratic in the item
            }
            if la != 0 {
                Some(la * cv(r)?)
            } else if ra != 0 {
                Some(ra * cv(l)?)
            } else {
                Some(0)
            }
        }
        _ => None,
    }
}

/// Clone of `e` with every `Identifier(item)` replaced by `repl` — builds
/// the loop-invariant base address expression (item bound to the range
/// start) for a running pointer.
fn subst_item(e: &Expr, item: &str, repl: &Expr) -> Expr {
    match e {
        Expr::Identifier(n) if n == item => repl.clone(),
        Expr::BinaryOp(kind, l, r) => Expr::BinaryOp(
            *kind,
            Box::new(subst_item(l, item, repl)),
            Box::new(subst_item(r, item, repl)),
        ),
        Expr::UnaryOp(kind, x) => Expr::UnaryOp(*kind, Box::new(subst_item(x, item, repl))),
        Expr::Index(buf, idx) => Expr::Index(buf.clone(), Box::new(subst_item(idx, item, repl))),
        Expr::Cast(x, t) => Expr::Cast(Box::new(subst_item(x, item, repl)), t.clone()),
        other => other.clone(),
    }
}

impl SerialUnrollPlan {
    /// Match a serial foreach body: every statement an Assign/Let, every
    /// load address linear in the item, at least one nonzero-coefficient
    /// site, constant range divisible by the unroll factor. Anything else
    /// keeps the serial fallback.
    fn match_body(
        body: &[Statement],
        ctx: &UnrollCtx,
        consts: &std::collections::HashMap<String, Expr>,
    ) -> Option<SerialUnrollPlan> {
        unroll_span(ctx.start, ctx.end, ctx.unroll)?;
        let mut sites: Vec<(Expr, i64)> = Vec::new();
        let mut site_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut found = false;
        for stmt in body {
            found |= collect_stmt_sites(stmt, ctx, consts, &mut sites, &mut site_keys)?;
        }
        if !found {
            return None;
        }
        Some(SerialUnrollPlan {
            item: ctx.item.to_string(),
            unroll: ctx.unroll,
            sites,
            start: ctx.start,
            end: ctx.end,
        })
    }
}

/// The match inputs of one serial foreach: the loop variable, its constant
/// range (end exclusive), and the requested unroll factor.
struct UnrollCtx<'a> {
    item: &'a str,
    start: i64,
    end: i64,
    unroll: usize,
}

/// The unroll factor must be ≥ 2 and tile the trip count exactly (no tail
/// in v1 — a tail would need a peeled remainder loop; not worth it while
/// the whole point is a dense issue stream).
fn unroll_span(start: i64, end: i64, unroll: usize) -> Option<i64> {
    if unroll < 2 {
        return None;
    }
    let n = unroll as i64;
    let span = end - start;
    if span < 2 * n || span % n != 0 {
        return None;
    }
    Some(n)
}

/// Collect one statement's preload sites into `sites` (deduped by the
/// expr-debug key). Returns whether a nonzero-coefficient site was found.
/// `None` = the statement shape is not serial-unrollable (bails the whole
/// body back to the plain serial loop).
fn collect_stmt_sites(
    stmt: &Statement,
    ctx: &UnrollCtx,
    consts: &std::collections::HashMap<String, Expr>,
    sites: &mut Vec<(Expr, i64)>,
    site_keys: &mut std::collections::HashSet<String>,
) -> Option<bool> {
    let exprs: Vec<&Expr> = match stmt {
        Statement::Assign(lhs, rhs) => {
            // A store inside the loop keeps its address via the per-slot
            // item register (regs[item]); only the rhs loads are preloaded.
            if matches!(lhs, Expr::Index(_, idx) if linear_coeff(idx, ctx.item, consts).is_none())
            {
                return None;
            }
            vec![rhs]
        }
        Statement::Let { expr: Some(e), .. } => vec![e],
        _ => return None,
    };
    let mut found = false;
    for e in exprs {
        found |= collect_expr_sites(e, ctx, consts, sites, site_keys)?;
    }
    Some(found)
}

/// The per-expression half of `collect_stmt_sites`.
fn collect_expr_sites(
    e: &Expr,
    ctx: &UnrollCtx,
    consts: &std::collections::HashMap<String, Expr>,
    sites: &mut Vec<(Expr, i64)>,
    site_keys: &mut std::collections::HashSet<String>,
) -> Option<bool> {
    let mut found = false;
    for load in collect_operand_loads(e) {
        let Expr::Index(_, idx) = &load else {
            continue;
        };
        let a = linear_coeff(idx, ctx.item, consts)?;
        if a == 0 {
            continue;
        }
        found = true;
        let key = format!("{:?}", load);
        if site_keys.insert(key) {
            sites.push((load, a));
        }
    }
    Some(found)
}
/// 2026-09-18 (M3 pipelining): the load operands of a reduction addend —
/// every direct `field[idx]` node, in walk order, NOT recursing into a
/// collected node (nested loads like `k[ii[d]]` stay inline; only the
/// outer load pipelines). Each becomes a double-buffered register pair.
/// 2026-09-19 (serial-loop unroll): the same walk collects the unroll's
/// preload sites (with the same non-recursion guarantee — a nested load's
/// index never yields a site, so `linear_coeff` rejects those bodies).
fn collect_operand_loads(e: &Expr) -> Vec<Expr> {
    let mut out = Vec::new();
    fn walk(e: &Expr, out: &mut Vec<Expr>) {
        match e {
            Expr::Index(_, _) => out.push(e.clone()),
            Expr::BinaryOp(_, l, r) => {
                walk(l, out);
                walk(r, out);
            }
            Expr::UnaryOp(_, x) => walk(x, out),
            Expr::Cast(x, _) => walk(x, out),
            Expr::Call(_, args, _) => {
                for a in args {
                    walk(a, out);
                }
            }
            _ => {}
        }
    }
    walk(e, &mut out);
    out
}

/// 2026-10-01 (plan `2026-10-01-atomic-element-rmw.md`): the PTX
/// atomic-op selector for the At family. `add.u64` for arithmetic
/// (Sub rides it negated), `exch.b64` for the exchange (the type
/// suffix differs — exch takes the untyped .b64 form).
#[derive(Clone, Copy)]
enum AtomicAtOp {
    Add,
    Exch,
}

/// Does the statement mention this identifier anywhere (shallow but sound
/// for the consumption check: assignments and lets cover the surface).
fn stmt_mentions_ident(stmt: &Statement, name: &str) -> bool {
    fn expr_mentions(e: &Expr, name: &str) -> bool {
        match e {
            Expr::Identifier(n) => n == name,
            Expr::BinaryOp(_, l, r) => expr_mentions(l, name) || expr_mentions(r, name),
            Expr::UnaryOp(_, x) => expr_mentions(x, name),
            Expr::Index(o, i) => expr_mentions(o, name) || expr_mentions(i, name),
            Expr::Call(_, args, _) => args.iter().any(|a| expr_mentions(a, name)),
            Expr::Cast(x, _) => expr_mentions(x, name),
            _ => false,
        }
    }
    match stmt {
        Statement::Assign(lhs, rhs) => {
            expr_mentions(lhs, name) || expr_mentions(rhs, name)
        }
        Statement::Let {
            name: n, expr, ..
        } => {
            n == name || expr.as_ref().is_some_and(|e| expr_mentions(e, name))
        }
        _ => false,
    }
}

struct Gen<'a> {
    layout: &'a SsboLayout,
    consts: &'a std::collections::HashMap<String, Expr>,
    count: i64,
    out: String,
    freg: u32,
    /// Fresh u32-temp counter (work-id staging, `fresh_named_u32`).
    n_u32: u32,
    /// 2026-10-01 (D14 remainder): the declared `unroll<N>` override
    /// (None = the derived `ptx_serial_unroll` default).
    unroll_override: Option<u64>,
    rreg: u32,
    rdreg: u32,
    preg: u32,
    label: u32,
    /// The register holding gid (the index_var's binding).
    gid: &'static str,
    /// index_var name -> register (locals too).
    regs: std::collections::HashMap<String, String>,
    /// 2026-09-18 (M3 pipelining, plan coalesced-kv-memory-path): operand
    /// loads pre-issued by the lane-reduction's software pipeline — key is
    /// the operand's Debug form, value the register holding its value.
    /// The Index arm checks this BEFORE emitting a load: the body consumes
    /// registers, the pipeline schedule owns the loads.
    pipelined: std::collections::HashMap<String, String>,
    /// 2026-09-18 (P0 f16 swap, plan fused-f16-decode-node): cast targets
    /// resolve through the casting graph (rule 19 — no type-name matching).
    universe: &'a crate::type_universe::TypeUniverse,
    int_bits: u64,
    casting_graph: crate::casting::graph::CastingGraph,
    /// 2026-09-19 (M1-finish): while a deferred region's strips are
    /// emitted, element references to this buffer resolve to per-lane
    /// registers (buf, d item, one register per strip).
    strip_acc: Option<(String, String, Vec<String>)>,
    /// The strip currently being emitted (indexes strip_acc's registers).
    active_strip: usize,
    /// 2026-09-18 (P1 lane-coverage fix): when true, the kernel's work
    /// item IS the block (w = ctaid.x).  The entry guard must be
    /// whole-block (`ctaid >= count → ret`) so all 64 threads enter the
    /// body and the lane-strided butterfly has full 32-lane coverage.
    block_work_item: bool,
    /// 2026-09-25 (bug 14): threads per block for the block-per-workitem
    /// dispatch — the stride for thread-distributed loops. Must mirror the
    /// dispatch decision in ptx/mod.rs exactly (deferred region 1024,
    /// warp-sliced 128, lane-reduction 64); the two sites are the one
    /// contract.
    block_threads: u32,
    /// 2026-09-30 (general reduction-split): the deferred-region CTA split
    /// factor `S` (1 = none). When `> 1`, `emit` decodes
    /// `(h = ctaid/S, slice = ctaid%S)` and `emit_deferred_region` reduces
    /// the slice's `j` sub-span and writes per-slice partials.
    deferred_split: u64,
    /// The register holding `slice = ctaid % S` while a split deferred
    /// region is emitted (`None` when no split).
    split_slice_reg: Option<String>,
    /// 2026-10-01 (5a lever 2 probe, the nofill precedent): skip a
    /// deferred pass for the wall-split measurement — WRONG numerics by
    /// design, timing evidence only.
    skip_pass: u32,
}

impl<'a> Gen<'a> {
    fn new(
        layout: &'a SsboLayout,
        consts: &'a std::collections::HashMap<String, Expr>,
        count: i64,
        universe: &'a crate::type_universe::TypeUniverse,
        int_bits: u64,
    ) -> Self {
        Self {
            layout,
            consts,
            count,
            out: String::new(),
            freg: 0,
            n_u32: 0,
            unroll_override: None,
            rreg: 3,
            rdreg: 2,
            preg: 2,
            label: 0,
            gid: "%r1",
            regs: std::collections::HashMap::new(),
            pipelined: std::collections::HashMap::new(),
            universe,
            int_bits,
            casting_graph: crate::casting::graph::CastingGraph::new(),
            block_work_item: false,
            block_threads: 64,
            strip_acc: None,
            active_strip: 0,
            deferred_split: 1,
            split_slice_reg: None,
            skip_pass: 0,
        }
    }

    /// A fresh named u32 virtual register (`%u<N>`) — for staging
    /// special-register reads through typed temps.
    fn fresh_named_u32(&mut self) -> String {
        self.n_u32 += 1;
        format!("%u{}", self.n_u32)
    }

    fn fresh_f(&mut self) -> String {
        let n = self.freg;
        self.freg += 1;
        format!("%f{}", n)
    }

    fn fresh_r(&mut self) -> String {
        let n = self.rreg;
        self.rreg += 1;
        format!("%r{}", n)
    }

    fn fresh_rd(&mut self) -> String {
        let n = self.rdreg;
        self.rdreg += 1;
        format!("%rd{}", n)
    }

    fn fresh_p(&mut self) -> String {
        let n = self.preg;
        self.preg += 1;
        format!("%p{}", n)
    }

    /// 2026-09-17 (M2b): integer-class locals — Int/UInt families get u32
    /// registers and integer ops. Names not starting with Int/UInt are float.
    fn is_int_ty(ty: &Option<crate::ast::Type>) -> bool {
        match ty {
            Some(crate::ast::Type::Custom(n)) => n.starts_with("Int") || n.starts_with("UInt"),
            _ => false,
        }
    }

    /// A loop bound: a literal or a module const (the same literal-const
    /// contract the SPIR-V cooperative path enforces).
    fn const_int(&self, e: &Expr) -> Result<i64, String> {
        match e {
            Expr::Decimal(n) => Ok(*n),
            Expr::Identifier(name) => match self.consts.get(name) {
                Some(Expr::Decimal(n)) => Ok(*n),
                _ => Err(format!(
                    "ptx general: loop bound '{}' must be a literal or module const",
                    name
                )),
            },
            _ => Err("ptx general: loop bound must be a literal or module const".into()),
        }
    }

    fn field_off(&self, name: &str) -> Option<u64> {
        self.layout
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.proj_offset)
    }

    fn field_of(&self, e: &Expr) -> Result<String, String> {
        match e {
            Expr::Identifier(s) => Ok(s.clone()),
            other => Err(format!(
                "ptx general: expected a buffer identifier, got {:?}",
                other
            )),
        }
    }

    fn emit(&mut self, shape: &KernelShape) -> Result<String, String> {
        let mut decl = String::new();
        let mut body = String::new();

        decl.push_str("    .reg .b64  %rd1;\n");
        decl.push_str("    .reg .u32  %r1, %r2;\n");
        decl.push_str("    .reg .pred %p1;\n");
        decl.push_str("    .reg .b32  %t0;\n");

        // 2026-09-18 (P1 lane-coverage fix): detect lane-reduction BEFORE
        // the guard so we can emit a block-level guard (whole-block exit)
        // instead of the flat gid guard.  The lane-mapped reduction needs
        // all32 lanes of each warp alive for full d-coverage; the flat
        // guard kills threads beyond `count`, leaving <32 live lanes.
        self.block_work_item = has_lane_reduction(&shape.kernel_stmts, self.consts)
            || has_warp_slice(&shape.kernel_stmts, self.consts);
        // 2026-09-19 (M1-finish): the deferred-softmax region is a
        // block-per-workitem dispatch at 1024 threads (32 warp slices).
        let region = if crate::config_tuning::ir_lowering().ptx_deferred_region {
            detect_deferred_region(&shape.kernel_stmts, &shape.index_var)
        } else {
            None
        };
        if region.is_some() {
            self.block_work_item = true;
        }
        // 2026-09-25 (bug 14): the stride for thread-distributed loops must
        // equal the dispatch's block_threads exactly (ptx/mod.rs: deferred
        // region 1024, warp-sliced 128, lane-reduction 64) — a mismatched
        // stride silently skips or double-covers iterations.
        self.block_threads = if region.is_some() {
            1024
        } else if has_warp_slice(&shape.kernel_stmts, self.consts) {
            warp_slice_block_threads(crate::config_tuning::ir_lowering().ptx_warp_slice_warps)
        } else {
            64
        };

        body.push_str("    ld.param.u64 %rd1, [proj_param];\n");
        if self.block_work_item {
            // Work item = BLOCK: w = ctaid.x.  Move the special register
            // to %r1 (a GPR) so it can be used in setp and address math.
            // The guard kills entire blocks (r1 >= count → ret) — uniform
            // per block, safe for the warp-wide butterfly.
            body.push_str("    mov.u32 %r1, %ctaid.x;\n");
            // 2026-09-30 (general reduction-split): with a split, the grid
            // is count*S — decode slice = ctaid % S and h = ctaid / S into
            // %r1, so the existing guard (%r1 >= count) still kills only
            // out-of-range blocks.
            self.decode_split_ctaid(&mut decl, &mut body);
            body.push_str(&format!(
                "    setp.ge.u32 %p1, %r1, {};\n",
                self.count
            ));
            body.push_str("    @%p1 ret;\n");
            // Bind index_var to %r1 (holds ctaid = work item).
            self.regs
                .insert(shape.index_var.clone(), self.gid.to_string());
        } else {
            // Flat gid: gid = ctaid.x * BLOCK + tid.x
            body.push_str("    mov.u32 %r1, %ctaid.x;\n");
            body.push_str(&format!(
                "    mul.lo.u32 %r1, %r1, {};\n",
                BLOCK
            ));
            body.push_str("    mov.u32 %r2, %tid.x;\n");
            body.push_str("    add.u32 %r1, %r1, %r2;\n");
            body.push_str(&format!(
                "    setp.ge.u32 %p1, %r1, {};\n",
                self.count
            ));
            body.push_str("    @%p1 ret;\n");
            self.regs
                .insert(shape.index_var.clone(), self.gid.to_string());
        }

        if let Some((start, count, parts)) = region {
            // The deferred-softmax region lowers as one unit; statements
            // outside it (none in practice) walk normally.
            self.emit_deferred_region(&parts, &mut decl, &mut body)?;
            for (i, stmt) in shape.kernel_stmts.iter().enumerate() {
                if i >= start && i < start + count {
                    continue;
                }
                self.emit_stmt(stmt, &mut decl, &mut body)?;
            }
        } else {
            for stmt in &shape.kernel_stmts {
                self.emit_stmt(stmt, &mut decl, &mut body)?;
            }
        }

        Ok(format!(
            ".version 8.0\n.target sm_86\n.address_size 64\n.visible .entry main (.param .b64 proj_param)\n{{\n{}\n{}\n    ret;\n}}\n",
            decl, body
        ))
    }

    fn emit_stmt(
        &mut self,
        stmt: &Statement,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        match stmt {            Statement::Assign(lhs, rhs) => self.emit_assign(lhs, rhs, decl, body),
            Statement::Let { name, expr, ty, .. } => {
                // A pure local: lower the initializer into a register of the
                // DECLARED class — Int locals are u32 with integer ops (the
                // GQA decompositions h = t/NKV need integer semantics; f32
                // division silently truncates only for power-of-2 divisors
                // and corrupts the bit pattern for everything else).
                if let Some(e) = expr {
                    let (d, b) = self.emit_local(name, e, ty);
                    decl.push_str(&d);
                    body.push_str(&b);
                }
                Ok(())
            }
            Statement::Foreach { item, list, body: loop_body } => {
                // 2026-09-17 (M2a): bounded loops in the general PTX kernel —
                // `foreach c in start..end` lowers to a counter register +
                // label/branch pair; the item binds like a local so body
                // index expressions resolve through `regs`. Registers are
                // function-scoped in PTX, so loop-carried locals (the
                // softmax's running max/sum) persist across iterations.
                let Expr::Range { start, end, .. } = list.as_ref() else {
                    return Err(
                        "ptx general: foreach over a non-range collection — kernel loops iterate `start..end` ranges only"
                            .into(),
                    );
                };
                let start_v = self.const_int(start)?;
                let end_v = self.const_int(end)?;
                if end_v <= start_v {
                    return Ok(()); // empty range — no code
                }
                // 2026-09-18 (P1, plan fused-f16-decode-node): a LANE-MAPPED
                // inner reduction. Shape: outer body = [.., Let acc = 0,
                // Foreach d in range { acc = acc + a[..d..] * b[..d..] },
                // ..rest reading acc..]. The serial fallback computes the
                // full dot REDUNDANTLY on every lane (probe: 71.5 ms at
                // bitnet NKV=4096 — 20 blocks x 64 lanes x full-D serial);
                // this path strides the inner loop across lanes (base =
                // lane-in-warp, step = warp) and combines per outer
                // iteration with the butterfly reduce. Sequential semantics
                // preserved: the butterfly hands EVERY lane the warp total,
                // so `rest` reads exactly what the serial form produced.
                // Gated on (end-start) % 32 == 0 for uniform convergence
                // (all lanes iterate the same count) — anything else keeps
                // the serial fallback. tid&31 because BLOCK=64 spans two
                // warps and a raw tid base would double-count d across
                // them; both warps then compute the same full total.
                let lane_plan = LaneReductionPlan::match_body(loop_body, self.consts);
                // 2026-09-19 (serial-loop unroll, plan flash-decode-gate):
                // the serial fallback is the workhorse shape for
                // work-item-decomposed reduction kernels (pv, qk) — its
                // coalescing comes from neighbouring gids, so lane-mapping
                // is wrong here; the win is issue rate. Try the unroll
                // before falling back to the plain 1-wide loop.
                // 2026-10-01 (D14 remainder): the declared `unroll<N>`
                // overrides the derived factor (D2) — resolved from the
                // node modifiers in the frontend, read here.
                let unroll = self
                    .unroll_override
                    .unwrap_or(crate::config_tuning::ir_lowering().ptx_serial_unroll as u64)
                    as usize;
                let ctx = UnrollCtx {
                    item,
                    start: start_v,
                    end: self.range_exclusive_end(list, end_v)?,
                    unroll,
                };
                // 2026-09-25 (bug 14): a block-per-workitem kernel runs its
                // body on ALL block threads. A loop whose stores hit
                // item-affine addresses was lowered block-redundantly —
                // every thread read-modify-wrote the SAME addresses with no
                // synchronization, so updates were lost
                // scheduling-dependently (attention_decode_2pass, CUDA lane:
                // a_err up to 13.5 vs a 1e-6 reference, streaky PASS/FAIL
                // runs of the identical binary). Such loops are
                // thread-distributed instead: thread t takes items
                // {start+t, start+t+block, ...}, each address written
                // exactly once — the interpreter's sequential semantics,
                // parallelized. Pure-register bodies (carried scalars like
                // the softmax's m/l) stay block-redundant: every thread
                // computes the identical total, which is benign. See
                // body_is_distributable for the exact gate.
                if self.block_work_item
                    && Self::body_is_distributable(loop_body, item, self.consts)
                {
                    return self.emit_thread_distributed(
                        loop_body,
                        item,
                        ctx.start,
                        ctx.end,
                        decl,
                        body,
                    );
                }
                // 2026-09-19 (M1 warp-sliced reductions, plan
                // general-machinery): a LONG serial reduction splits across
                // the block's warps (P1 block-per-workitem dispatch, smem
                // partial merge). Parallelism beats the unroll's MLP, so
                // the slice takes priority; the unroll stays the
                // fallback for short or non-divisible loops. Eligibility
                // is the shared predicate (2026-09-30 stage-5 5c) — the
                // detector and this gate used to duplicate the literals.
                let ws = crate::config_tuning::ir_lowering();
                if ws.ptx_warp_slice
                    && matches!(lane_plan, None)
                    && warp_slice_span_ok(
                        ctx.end - ctx.start,
                        ws.ptx_warp_slice_min_span,
                        ws.ptx_warp_slice_warps,
                    )
                {
                    let slice_ctx = UnrollCtx {
                        item,
                        start: ctx.start,
                        end: ctx.end,
                        unroll: 4,
                    };
                    if SerialUnrollPlan::match_body(loop_body, &slice_ctx, self.consts)
                        .is_some()
                    {
                        return self.emit_warp_sliced(
                            WarpSliceEmit {
                                body_stmts: loop_body,
                                item,
                                start: ctx.start,
                                span: ctx.end - ctx.start,
                                warps: ws.ptx_warp_slice_warps,
                            },
                            decl,
                            body,
                        );
                    }
                }
                let serial_plan = if matches!(lane_plan, None) {
                    SerialUnrollPlan::match_body(loop_body, &ctx, self.consts)
                } else {
                    None
                };
                if let Some(plan) = &serial_plan {
                    return self.emit_serial_unrolled(plan, loop_body, decl, body);
                }
                let cnt = self.fresh_r();
                decl.push_str(&format!("    .reg .u32 {};\n", cnt));
                let pred = self.fresh_p();
                decl.push_str(&format!("    .reg .pred {};\n", pred));
                let lab = self.label;
                self.label += 1;
                let head = format!("L{}_head", lab);
                let tail = format!("L{}_end", lab);
                body.push_str(&format!("    mov.u32 {}, {};\n", cnt, start_v));
                body.push_str(&format!("{}:\n", head));
                body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, cnt, end_v));
                body.push_str(&format!("    @{} bra {};\n", pred, tail));
                self.regs.insert(item.clone(), cnt.clone());
                match &lane_plan {
                    Some((split, r)) => {
                        // Statements before the inner foreach lower
                        // normally (the `let acc = 0` zero-init); the lane
                        // reduction replaces the inner foreach; statements
                        // after it consume the reduced accumulator.
                        for s in &loop_body[..*split] {
                            self.emit_stmt(s, decl, body)?;
                        }
                        self.emit_lane_reduction(r, decl, body)?;
                        for s in &loop_body[split + 1..] {
                            self.emit_stmt(s, decl, body)?;
                        }
                    }
                    None => {
                        for s in loop_body {
                            self.emit_stmt(s, decl, body)?;
                        }
                    }
                }
                body.push_str(&format!("    add.u32 {}, {}, 1;\n", cnt, cnt));
                body.push_str(&format!("    bra {};\n", head));
                body.push_str(&format!("{}:\n", tail));
                Ok(())
            }
            other => Err(format!(
                "ptx general: statement {:?} outside the elementwise surface\n  \
                 why: the S5-lite emitter handles assignments, lets, and the \
                 foreach/term host split\n  fix: split the body so only pure \
                 elementwise statements remain, or use --backend spirv",
                std::mem::discriminant(other)
            )),
        }
    }

    /// Emit the lane-mapped inner reduction: acc starts at its Let value,
    /// lanes stride the inner range (base = tid&31, step = warp), then the
    /// butterfly reduce hands every lane the total. `rest` rebinds acc to
    /// the reduced register and lowers normally.
    fn emit_lane_reduction(
        &mut self,
        r: &LaneReductionPlan,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        // zero init (the Let statement's initializer is Decimal(0) by the
        // matcher's contract — emit it as a plain mov).
        let acc = self.fresh_f();
        decl.push_str(&format!("    .reg .f32 {};\n", acc));
        body.push_str(&format!("    mov.f32 {}, 0.0e0;\n", acc));

        // lane-strided inner loop over d: base = tid.x & 31, step 32.
        let dcur = self.fresh_r();
        let dnext = self.fresh_r();
        let pred = self.fresh_p();
        decl.push_str(&format!("    .reg .u32 {};\n", dcur));
        decl.push_str(&format!("    .reg .u32 {};\n", dnext));
        decl.push_str(&format!("    .reg .pred {};\n", pred));
        let lab = self.label;
        self.label += 1;
        let head = format!("L{}_head", lab);
        let tail = format!("L{}_end", lab);

        // ── M3 software pipelining (plan coalesced-kv-memory-path) ────
        // The serial-fallback dot is LOAD-LATENCY bound: each iteration
        // issues two global loads and stalls on the first (~500 cycles
        // with 40 warps on the machine — nothing hides them). This
        // schedule double-buffers: iteration i+1's operands are loaded
        // BEFORE iteration i's FMA consumes the previous pair, so one
        // load's latency is amortized per iteration instead of paid
        // serially. The doctrine obligation is that the DEFAULT does
        // this — no source change, no keyword.
        //
        // Shape (count = iterations, guaranteed >= 1 by the matcher):
        //   load(cur, base)
        //   head: if base+32*(i+1) >= end -> tail
        //         load(next, dnext)        // always in-bounds here
        //         body(cur)                // consumes pipelined regs
        //         dcur = dnext; dnext += 32; cur <- next (mov per operand)
        //         bra head
        //   tail: body(cur)                // peeled final iteration
        //   butterfly(acc)
        let addend = match &r.inner_body[0] {
            Statement::Assign(_, rhs) => rhs.clone(),
            _ => unreachable!("matcher guarantees one Assign"),
        };
        let ops = collect_operand_loads(&addend);

        body.push_str("    mov.u32 %r2, %tid.x;\n");
        body.push_str("    and.b32 %r2, %r2, 31;\n");
        body.push_str(&format!("    mov.u32 {}, %r2;\n", dcur));
        body.push_str(&format!("    add.u32 {}, {}, 32;\n", dnext, dcur));

        self.regs.insert(r.acc.clone(), acc.clone());
        eprintln!("[lane-dbg] ops={} first_is_assign={}", ops.len(), matches!(&r.inner_body[0], Statement::Assign(_, _)));

        if ops.is_empty() {
            // No pipelineable loads — the plain loop (previous behavior).
            body.push_str(&format!("{}:\n", head));
            body.push_str(&format!(
                "    setp.ge.u32 {}, {}, {};\n",
                pred, dcur, r.inner_end
            ));
            body.push_str(&format!("    @{} bra {};\n", pred, tail));
            self.regs.insert(r.inner_item.clone(), dcur.clone());
            if let Statement::Assign(lhs, rhs) = &r.inner_body[0] {
                self.emit_assign(lhs, rhs, decl, body)?;
            }
            body.push_str(&format!("    add.u32 {}, {}, 32;\n", dcur, dcur));
            body.push_str(&format!("    bra {};\n", head));
            body.push_str(&format!("{}:\n", tail));
        } else {
            // Per-operand double buffers.
            let mut cur = Vec::with_capacity(ops.len());
            let mut nxt = Vec::with_capacity(ops.len());
            for _ in &ops {
                let c = self.fresh_f();
                let n = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {}, {};\n", c, n));
                cur.push(c);
                nxt.push(n);
            }

            // Prologue: iteration 0's operands (index regs bound to dcur).
            self.regs.insert(r.inner_item.clone(), dcur.clone());
            for (op, creg) in ops.iter().zip(&cur) {
                self.emit_expr(op, creg, decl, body)?;
            }

            body.push_str(&format!("{}:\n", head));
            body.push_str(&format!(
                "    setp.ge.u32 {}, {}, {};\n",
                pred, dnext, r.inner_end
            ));
            body.push_str(&format!("    @{} bra {};\n", pred, tail));

            // Next iteration's loads (index regs bound to dnext).
            self.regs.insert(r.inner_item.clone(), dnext.clone());
            for (op, nreg) in ops.iter().zip(&nxt) {
                self.emit_expr(op, nreg, decl, body)?;
            }

            // Body: consumes the CURRENT registers via the pipelined map.
            self.regs.insert(r.inner_item.clone(), dcur.clone());
            for (op, creg) in ops.iter().zip(&cur) {
                self.pipelined
                    .insert(format!("{:?}", op), creg.clone());
            }
            if let Statement::Assign(lhs, rhs) = &r.inner_body[0] {
                self.emit_assign(lhs, rhs, decl, body)?;
            }
            self.pipelined.clear();

            // Advance + swap.
            body.push_str(&format!("    mov.u32 {}, {};\n", dcur, dnext));
            body.push_str(&format!("    add.u32 {}, {}, 32;\n", dnext, dnext));
            for (creg, nreg) in cur.iter().zip(&nxt) {
                body.push_str(&format!("    mov.f32 {}, {};\n", creg, nreg));
            }
            body.push_str(&format!("    bra {};\n", head));

            // Peeled final iteration (no next load exists).
            body.push_str(&format!("{}:\n", tail));
            self.regs.insert(r.inner_item.clone(), dcur.clone());
            for (op, creg) in ops.iter().zip(&cur) {
                self.pipelined
                    .insert(format!("{:?}", op), creg.clone());
            }
            if let Statement::Assign(lhs, rhs) = &r.inner_body[0] {
                self.emit_assign(lhs, rhs, decl, body)?;
            }
            self.pipelined.clear();
        }

        // Butterfly reduce: every lane ends with the warp total.
        let total = self.fresh_f();
        decl.push_str(&format!("    .reg .f32 {};\n", total));
        let reduced = self.emit_butterfly_add(acc, total, decl, body)?;
        self.regs.insert(r.acc.clone(), reduced);
        Ok(())
    }

    /// 2026-09-19 (serial-loop unroll, plan flash-decode-gate): emit the
    /// serial foreach unrolled by `unroll` with running byte pointers per
    /// load site and `unroll` independent load registers per site issued
    /// back-to-back before the consuming math (MLP = sites × unroll).
    /// Accumulation order is exactly the serial one: per slot, statements
    /// lower in source order against that slot's preloaded registers.
    /// The loop item in non-site positions (store addresses, scalar math)
    /// reads a per-slot register (cnt + k) so correctness never depends on
    /// the site analysis.
    /// 2026-09-25 (bug 14): a loop body is thread-distributable when every
    /// statement stores to an address linear in the loop item with a
    /// NONZERO coefficient (distinct items → distinct addresses, so a
    /// tid-strided distribution writes each address exactly once), and no
    /// statement assigns a carried scalar (a register updated across
    /// iterations would turn into a per-thread partial under
    /// distribution). `Let` locals are pure per-thread registers — their
    /// redundant initialization is identical across threads and benign.
    /// Anything else keeps the existing lowering (block-redundant for
    /// register bodies; the lane-reduction path owns the reduction shape).
    fn body_is_distributable(
        body: &[Statement],
        item: &str,
        consts: &std::collections::HashMap<String, Expr>,
    ) -> bool {
        let mut any_store = false;
        for s in body {
            match s {
                Statement::Assign(Expr::Index(_, idx), _) => {
                    match linear_coeff(idx, item, consts) {
                        Some(c) if c != 0 => any_store = true,
                        _ => return false,
                    }
                }
                // A carried scalar (`l = l + …`) must NOT be distributed —
                // each thread would hold a partial instead of the total.
                Statement::Assign(Expr::Identifier(_), _) => return false,
                Statement::Let { .. } => continue,
                _ => return false,
            }
        }
        any_store
    }

    /// Emit a foreach as a thread-distributed loop: thread t handles items
    /// {start + t, start + t + block_threads, …}. Every stored address is
    /// written exactly once across the block (body_is_distributable proved
    /// the addresses item-affine-nonzero and the body carried-free), so the
    /// result equals the interpreter's sequential execution.
    fn emit_thread_distributed(
        &mut self,
        loop_body: &[Statement],
        item: &str,
        start: i64,
        end: i64,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        let cnt = self.fresh_r();
        decl.push_str(&format!("    .reg .u32 {};\n", cnt));
        let pred = self.fresh_p();
        decl.push_str(&format!("    .reg .pred {};\n", pred));
        let tid = self.fresh_r();
        decl.push_str(&format!("    .reg .u32 {};\n", tid));
        let lab = self.label;
        self.label += 1;
        let head = format!("L{}_head", lab);
        let tail = format!("L{}_end", lab);
        body.push_str(&format!("    mov.u32 {}, %tid.x;\n", tid));
        body.push_str(&format!("    add.u32 {}, {}, {};\n", cnt, tid, start));
        body.push_str(&format!("{}:\n", head));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, cnt, end));
        body.push_str(&format!("    @{} bra {};\n", pred, tail));
        self.regs.insert(item.to_string(), cnt.clone());
        for s in loop_body {
            self.emit_stmt(s, decl, body)?;
        }
        body.push_str(&format!(
            "    add.u32 {}, {}, {};\n",
            cnt, cnt, self.block_threads
        ));
        body.push_str(&format!("    bra {};\n", head));
        body.push_str(&format!("{}:\n", tail));
        Ok(())
    }

    fn emit_serial_unrolled(
        &mut self,
        plan: &SerialUnrollPlan,
        body_stmts: &[Statement],
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        let n = plan.unroll as i64;
        let sites = self.emit_unroll_site_bases(plan, decl, body)?;

        let cnt = self.fresh_r();
        decl.push_str(&format!("    .reg .u32 {};\n", cnt));
        let pred = self.fresh_p();
        decl.push_str(&format!("    .reg .pred {};\n", pred));
        let lab = self.label;
        self.label += 1;
        let head = format!("L{}_head", lab);
        let tail = format!("L{}_end", lab);
        // Per-slot item register for non-site uses (stores, scalar math).
        let item_reg = self.fresh_r();
        decl.push_str(&format!("    .reg .u32 {};\n", item_reg));

        body.push_str(&format!("    mov.u32 {}, {};\n", cnt, plan.start));
        body.push_str(&format!("{}:\n", head));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, cnt, plan.end));
        body.push_str(&format!("    @{} bra {};\n", pred, tail));
        // One shot per slot: preloads first (all sites issue back-to-back),
        // then the statements consume them in source order.
        let saved_pipelined = std::mem::take(&mut self.pipelined);
        let saved_item = self.regs.insert(plan.item.clone(), item_reg.clone());
        for k in 0..n {
            body.push_str(&format!("    add.u32 {}, {}, {};\n", item_reg, cnt, k));
            self.emit_unroll_slot_loads(&sites, k, decl, body)?;
            self.emit_body_slice(body_stmts, decl, body)?;
        }
        // Bump the running pointers by the whole unroll span.
        self.emit_unroll_pointer_bumps(&sites, n, body);
        self.pipelined = saved_pipelined;
        match saved_item {
            Some(v) => {
                self.regs.insert(plan.item.clone(), v);
            }
            None => {
                self.regs.remove(&plan.item);
            }
        }
        body.push_str(&format!("    add.u32 {}, {}, {};\n", cnt, cnt, n));
        body.push_str(&format!("    bra {};\n", head));
        body.push_str(&format!("{}:\n", tail));
        Ok(())
    }

    fn emit_body_slice(
        &mut self,
        stmts: &[Statement],
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        for s in stmts {
            self.emit_stmt(s, decl, body)?;
        }
        Ok(())
    }

/// The accumulator of a warp-slice body: the LAST `x = x + …` assign in
/// the loop (the serial reduction's carried scalar). None = not a
/// scalar-reduce body (the caller keeps the serial/unroll fallback).
fn slice_acc_name(body_stmts: &[Statement]) -> Option<String> {
    let mut name: Option<String> = None;
    for s in body_stmts {
        let Statement::Assign(Expr::Identifier(lhs), rhs) = s else {
            continue;
        };
        let Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, _) = rhs else {
            continue;
        };
        let Expr::Identifier(acc) = l.as_ref() else {
            continue;
        };
        if acc == lhs {
            name = Some(lhs.clone());
        }
    }
    name
}

/// 2026-09-19 (M1 warp-sliced reductions, plan general-machinery):
    /// lower a serial foreach-reduce as FOUR warp-private slices over the
    /// block's single work item (P1 block-per-workitem dispatch, 128
    /// threads): warp w accumulates j ∈ [start + w·span/4,
    /// start + (w+1)·span/4), the four partials merge through shared
    /// memory, and every thread leaves with the full total. The loop is
    /// warp-uniform (lanes execute the same trips redundantly — the P1
    /// contract), so the bar.sync is race-free. fp reassociation (slice
    /// sums before the total) is accepted under the a_err gate — same
    /// contract as the butterflies and the unroll pass.
    fn emit_warp_sliced(
        &mut self,
        plan: WarpSliceEmit<'_>,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        let WarpSliceEmit {
            body_stmts,
            item,
            start,
            span,
            warps,
        } = plan;
        let acc_name = Self::slice_acc_name(body_stmts)
            .ok_or_else(|| "ptx general: warp slice without a scalar accumulator".to_string())?;
        // 2026-09-30 (stage-5 5c): every warp-count constant below
        // derives from `warps` (config `ptx_warp_slice_warps`, default 4
        // = the original hardcoded shape). The constants that stay
        // literal are hardware facts: warp = 32 lanes (`shr 5`), f32
        // slot = 4 bytes (smem addressing), `bar.sync`.
        let quarter = (span / warps as i64) as u32;
        let r_warp = self.fresh_r();
        let r_lo = self.fresh_r();
        let r_hi = self.fresh_r();
        let cnt = self.fresh_r();
        let pred = self.fresh_p();
        let rd_part = self.fresh_rd();
        let f_t = self.fresh_f();
        for r in [&r_warp, &r_lo, &r_hi, &cnt] {
            decl.push_str(&format!("    .reg .u32 {};\n", r));
        }
        decl.push_str(&format!("    .reg .pred {};\n", pred));
        decl.push_str(&format!("    .reg .f32 {};\n", f_t));
        decl.push_str(&format!("    .reg .b64 {};\n", rd_part));
        // One f32 partial slot per warp (warps * 4 bytes).
        decl.push_str(&format!(
            "    .shared .align 4 .b8 wpart[{}];\n",
            warps * 4
        ));

        let acc_reg = self.regs.get(&acc_name).cloned().ok_or_else(|| {
            format!("ptx general: warp slice accumulator '{acc_name}' is not bound")
        })?;

        // warp id + slice bounds: lo = start + warp*quarter, hi = lo + quarter
        body.push_str(&format!("    mov.u32 {}, %tid.x;\n", r_warp));
        body.push_str(&format!("    shr.u32 {}, {}, 5;\n", r_warp, r_warp));
        body.push_str(&format!("    mov.u32 {}, {};\n", r_lo, start));
        body.push_str(&format!(
            "    mad.lo.u32 {}, {}, {}, {};\n",
            r_lo, r_warp, quarter, r_lo
        ));
        body.push_str(&format!("    add.u32 {}, {}, {};\n", r_hi, r_lo, quarter));
        body.push_str(&format!("    mov.u32 {}, {};\n", cnt, r_lo));
        let lab = self.label;
        self.label += 1;
        let head = format!("L{}_head", lab);
        let tail = format!("L{}_end", lab);
        body.push_str(&format!("{}:\n", head));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, cnt, r_hi));
        body.push_str(&format!("    @{} bra {};\n", pred, tail));
        let saved_item = self.regs.insert(item.to_string(), cnt.clone());
        self.emit_body_slice(body_stmts, decl, body)?;
        match saved_item {
            Some(v) => {
                self.regs.insert(item.to_string(), v);
            }
            None => {
                self.regs.remove(item);
            }
        }
        body.push_str(&format!("    add.u32 {}, {}, 1;\n", cnt, cnt));
        body.push_str(&format!("    bra {};\n", head));
        body.push_str(&format!("{}:\n", tail));
        // partial -> shared memory (all 32 lanes store the same value to
        // the same slot: benign same-value race). mad.wide folds the
        // row offset: address = wpart + warp*4.
        body.push_str(&format!("    mov.u64 {}, wpart;\n", rd_part));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 4, {};\n",
            rd_part, r_warp, rd_part
        ));
        body.push_str(&format!("    st.shared.f32 [{}], {};\n", rd_part, acc_reg));
        body.push_str("    bar.sync 0;\n");
        // merge: the accumulator becomes the sum of the warp partials
        // (warps of them; default 4 = the original shape).
        body.push_str(&format!("    mov.u64 {}, wpart;\n", rd_part));
        body.push_str(&format!("    ld.shared.f32 {}, [{}+0];\n", acc_reg, rd_part));
        for k in 1..warps {
            body.push_str(&format!(
                "    ld.shared.f32 {}, [{}+{}];\n",
                f_t, rd_part, k * 4
            ));
            body.push_str(&format!(
                "    add.f32 {}, {}, {};\n",
                acc_reg, acc_reg, f_t
            ));
        }
        Ok(())
    }

    /// The exclusive end of a foreach range (inclusive ranges normalize to
    /// end + 1) — the same contract the lane matcher's span arithmetic
    /// uses, factored so both arms share it.
    fn range_exclusive_end(&self, list: &Expr, fallback: i64) -> Result<i64, String> {
        let Expr::Range {
            end,
            inclusive,
            ..
        } = list
        else {
            return Ok(fallback);
        };
        if *inclusive {
            Ok(self.const_int(end)? + 1)
        } else {
            Ok(self.const_int(end)?)
        }
    }

    fn emit_unroll_pointer_bumps(&mut self, sites: &[UnrollSite], n: i64, body: &mut String) {
        for site in sites {
            body.push_str(&format!(
                "    add.u64 {}, {}, {};\n",
                site.base,
                site.base,
                n * site.stride_bytes
            ));
        }
    }

    /// Prologue of the unrolled loop: one loop-invariant base address
    /// register per preload site (item bound to the range start). The site
    /// bounds check rejects shapes whose slot offsets leave the PTX
    /// immediate range.
    fn emit_unroll_site_bases(
        &mut self,
        plan: &SerialUnrollPlan,
        decl: &mut String,
        body: &mut String,
    ) -> Result<Vec<UnrollSite>, String> {
        let n = plan.unroll as i64;
        let mut sites = Vec::new();
        let start_expr = Expr::Decimal(plan.start);
        for (load, a) in &plan.sites {
            let Expr::Index(buf, idx) = load else {
                continue;
            };
            let buf_name = self.field_of(buf)?;
            let off = self.field_off(&buf_name).ok_or_else(|| {
                format!("ptx general: unroll buffer '{}' not in layout", buf_name)
            })?;
            let elem = self.elem_bytes(&buf_name)?;
            if n * a.abs() * elem as i64 > (1 << 30) {
                return Err(format!(
                    "ptx general: unroll site '{}' needs slot offset {}x{}x{} beyond the PTX immediate range",
                    buf_name, n, a, elem
                ));
            }
            let idx_base = subst_item(idx, &plan.item, &start_expr);
            let base = self.array_addr(buf_name, off, elem, &idx_base, decl, body)?;
            sites.push(UnrollSite {
                key: format!("{:?}", load),
                base,
                stride_bytes: a * elem as i64,
                elem,
            });
        }
        Ok(sites)
    }

    /// Issue slot `k`'s preload for every site (back-to-back, independent
    /// destination registers — no false WAR dependencies) and register the
    /// values under the site keys for the consuming statements.
    fn emit_unroll_slot_loads(
        &mut self,
        sites: &[UnrollSite],
        k: i64,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        for site in sites {
            let val = self.fresh_f();
            decl.push_str(&format!("    .reg .f32 {};\n", val));
            let slot_bytes = site.stride_bytes * k;
            if site.elem == 2 {
                body.push_str(&format!(
                    "    ld.global.u16 %t0, [{}+{}];\n",
                    site.base, slot_bytes
                ));
                body.push_str(&format!("    cvt.f32.f16 {}, %t0;\n", val));
            } else {
                body.push_str(&format!(
                    "    ld.global.f32 {}, [{}+{}];\n",
                    val, site.base, slot_bytes
                ));
            }
            self.pipelined.insert(site.key.clone(), val);
        }
        Ok(())
    }

    /// Butterfly sum over the warp: 5 shfl.bfly rounds, f32 punned through
    /// b32. Returns the register holding the full total (every lane).
    fn emit_butterfly_add(
        &mut self,
        v: String,
        out: String,
        decl: &mut String,
        body: &mut String,
    ) -> Result<String, String> {
        // 2026-09-20 (M1-finish NaN fix): use a dedicated scratch register
        // instead of the pre-declared %r2, which may hold live values from
        // the general emit pipeline.
        let mut fa = v;
        let scratch = self.fresh_r();
        decl.push_str(&format!("    .reg .u32 {};\n", scratch));
        for offset in [16u32, 8, 4, 2, 1] {
            let fb = self.fresh_f();
            let tb = self.fresh_r();
            decl.push_str(&format!("    .reg .f32 {};\n", fb));
            decl.push_str(&format!("    .reg .u32 {};\n", tb));
            body.push_str(&format!("    mov.b32 {}, {};\n", scratch, fa));
            body.push_str(&format!(
                "    shfl.sync.bfly.b32 {}, {}, {}, 0x1f, 0xffffffff;\n",
                tb, scratch, offset
            ));
            body.push_str(&format!("    mov.b32 {}, {};\n", fb, tb));
            body.push_str(&format!("    add.f32 {}, {}, {};\n", fa, fa, fb));
        }
        body.push_str(&format!("    mov.f32 {}, {};\n", out, fa));
        Ok(out)
    }

    fn emit_assign(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        // lhs = Index(buf, index) → array store; lhs = Identifier(scalar) → scalar store.
        // 2026-09-19 (M1-finish): a deferred region's element-wise RMW on
        // the strip buffer resolves to the strip's register (the rhs's own
        // acc reference reads the same register — the add happens in
        // registers; the smem merge owns the cross-warp traffic).
        if let Expr::Index(buf, idx) = lhs {
            let strip = self.strip_acc.clone();
            if let Some((buf_name_s, d_item, regs)) = &strip {
                if *buf_name_s == self.field_of(buf)? && format!("{:?}", idx).contains(&format!("\"{}\"", d_item)) {
                    let acc_reg = regs[self.active_strip].clone();
                    let tmp = self.fresh_f();
                    decl.push_str(&format!("    .reg .f32 {};\n", tmp));
                    self.emit_expr(rhs, &tmp, decl, body)?;
                    body.push_str(&format!("    mov.f32 {}, {};\n", acc_reg, tmp));
                    return Ok(());
                }
            }
        }
        if let Expr::Index(buf, idx) = lhs {
            let buf_name = self.field_of(buf)?;
            let off = self.field_off(&buf_name).ok_or_else(|| {
                format!("ptx general: write buffer '{}' not in layout", buf_name)
            })?;
            let elem = self.elem_bytes(&buf_name)?;
            let val = self.fresh_f();
            decl.push_str(&format!("    .reg .f32 {};\n", val));
            self.emit_expr(rhs, &val, decl, body)?;
            let addr = self.array_addr(buf_name, off, elem, idx, decl, body)?;
            if elem == 2 {
                // f16 store: convert the f32 value and store 16 bits.
                body.push_str(&format!("    cvt.rn.f16.f32 %t0, {};\n", val));
                body.push_str(&format!("    st.global.u16 [{}], %t0;\n", addr));
            } else {
                body.push_str(&format!("    st.global.f32 [{}], {};\n", addr, val));
            }
            return Ok(());
        }
        if let Expr::Identifier(name) = lhs {
            // 2026-09-17 (M2a): a LOCAL assignment writes the bound register
            // (PTX registers are function-scoped — the value persists across
            // loop iterations). State-scalar writes keep the global store.
            // The index_var is host-owned (the runner fast-forwards it) and
            // never appears here.
            if let Some(reg) = self.regs.get(name).cloned() {
                if reg.starts_with("%r") {
                    self.emit_index(rhs, &reg, decl, body)?;
                } else {
                    self.emit_expr(rhs, &reg, decl, body)?;
                }
                return Ok(());
            }
            let off = self.field_off(name).ok_or_else(|| {
                format!("ptx general: scalar '{}' not in layout", name)
            })?;
            let val = self.fresh_f();
            decl.push_str(&format!("    .reg .f32 {};\n", val));
            self.emit_expr(rhs, &val, decl, body)?;
            body.push_str(&format!("    st.global.f32 [%rd1+{}], {};\n", off, val));
            return Ok(());
        }
        Err(format!(
            "ptx general: assignment target {:?} outside the elementwise surface",
            lhs
        ))
    }

    /// Compute the byte address of `buf[index]` into a fresh %rd register.
    fn array_addr(
        &mut self,
        buf: String,
        off: u64,
        elem: u64,
        idx: &Expr,
        decl: &mut String,
        body: &mut String,
    ) -> Result<String, String> {
        // The index: usually the index_var (gid) — the common elementwise case.
        let i_reg = self.fresh_r();
        decl.push_str(&format!("    .reg .u32 {};\n", i_reg));
        self.emit_index(idx, &i_reg, decl, body)?;
        let addr = self.fresh_rd();
        decl.push_str(&format!("    .reg .b64 {};\n", addr));
        body.push_str(&format!("    mul.wide.u32 {}, {}, {};\n", addr, i_reg, elem));
        body.push_str(&format!("    add.u64 {}, %rd1, {};\n", addr, addr));
        if off > 0 {
            body.push_str(&format!("    add.u64 {}, {}, {};\n", addr, addr, off));
        }
        Ok(addr)
    }

    fn emit_index(
        &mut self,
        idx: &Expr,
        out: &str,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        // u32 index arithmetic: identifiers (gid, loop vars, consts) and
        // Add/Sub/Mul trees — `i * NKV + c` from the row kernels.
        match idx {
            Expr::Identifier(name) => {
                if let Some(reg) = self.regs.get(name).cloned() {
                    body.push_str(&format!("    mov.u32 {}, {};\n", out, reg));
                    return Ok(());
                }
                match self.consts.get(name) {
                    Some(Expr::Decimal(n)) => {
                        body.push_str(&format!("    mov.u32 {}, {};\n", out, n));
                        Ok(())
                    }
                    _ => Err(format!("ptx general: index var '{}' not bound", name)),
                }
            }
            Expr::Decimal(n) => {
                body.push_str(&format!("    mov.u32 {}, {};\n", out, n));
                Ok(())
            }
            Expr::BinaryOp(kind, l, r) => {
                let lr = self.fresh_r();
                let rr = self.fresh_r();
                decl.push_str(&format!("    .reg .u32 {};\n", lr));
                decl.push_str(&format!("    .reg .u32 {};\n", rr));
                self.emit_index(l, &lr, decl, body)?;
                self.emit_index(r, &rr, decl, body)?;
                let op = match kind {
                    crate::ast::BinaryOpKind::Add => "add.u32",
                    crate::ast::BinaryOpKind::Sub => "sub.u32",
                    crate::ast::BinaryOpKind::Mul => "mul.lo.u32",
                    // 2026-09-17 (M2b): index-space division — the GQA head
                    // decompositions (h = t / NKV). Unsigned: indices are
                    // non-negative by construction.
                    crate::ast::BinaryOpKind::Div => "div.u32",
                    other => {
                        return Err(format!(
                            "ptx general: index arithmetic {:?} outside the supported surface (Add/Sub/Mul/Div)",
                            other
                        ))
                    }
                };
                body.push_str(&format!("    {} {}, {}, {};\n", op, out, lr, rr));
                Ok(())
            }
            other => Err(format!(
                "ptx general: index expression {:?} outside the elementwise surface",
                other
            )),
        }
    }

    /// Lower `expr` into the f32 register `out`.
    fn emit_expr(
        &mut self,
        expr: &Expr,
        out: &str,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        match expr {
            Expr::Decimal(n) => {
                // ptxas rejects integer literals in .f32 position — 0 -> 0.0
                body.push_str(&format!("    mov.f32 {}, {}.0;\n", out, n));
            }
            Expr::Float(f) => {
                body.push_str(&format!("    mov.f32 {}, {:e};\n", out, f));
            }
            Expr::Identifier(name) => {
                self.emit_ident_read(name, out, decl, body)?;
            }
            Expr::Index(buf, idx) => {
                // M3 pipelining: a pre-issued operand load consumes its
                // register instead of re-loading (the schedule owns the
                // memory traffic; see emit_lane_reduction).
                let key = format!("{:?}", expr);
                if let Some(reg) = self.pipelined.get(&key) {
                    body.push_str(&format!("    mov.f32 {}, {};
", out, reg));
                    return Ok(());
                }
                // 2026-09-19 (M1-finish): inside a deferred region's strips,
                // element references to the accumulator buffer resolve to
                // per-lane registers (the vector state lives in registers;
                // the smem merge owns the cross-warp traffic).
                let strip = self.strip_acc.clone();
                if let Some((buf_name, d_item, regs)) = &strip {
                    if *buf_name == self.field_of(buf)? && format!("{:?}", idx).contains(&format!("\"{}\"", d_item)) {
                        let r = regs[self.active_strip].clone();
                        body.push_str(&format!("    mov.f32 {}, {};\n", out, r));
                        return Ok(());
                    }
                }
                let buf_name = self.field_of(buf)?;
                let off = self.field_off(&buf_name).ok_or_else(|| {
                    format!("ptx general: read buffer '{}' not in layout", buf_name)
                })?;
                let elem = self.elem_bytes(&buf_name)?;
                let addr = self.array_addr(buf_name, off, elem, idx, decl, body)?;
                if elem == 2 {
                    // f16 load: 16-bit load + convert to f32 for the math.
                    body.push_str(&format!("    ld.global.u16 %t0, [{}];\n", addr));
                    body.push_str(&format!("    cvt.f32.f16 {}, %t0;\n", out));
                } else {
                    body.push_str(&format!("    ld.global.f32 {}, [{}];\n", out, addr));
                }
            }
            Expr::BinaryOp(kind, l, r) => {
                let lreg = self.fresh_f();
                let rreg = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {};\n", lreg));
                decl.push_str(&format!("    .reg .f32 {};\n", rreg));
                self.emit_expr(l, &lreg, decl, body)?;
                self.emit_expr(r, &rreg, decl, body)?;
                let op = match kind {
                    BinaryOpKind::Add => "add.f32",
                    BinaryOpKind::Sub => "sub.f32",
                    BinaryOpKind::Mul => "mul.f32",
                    BinaryOpKind::Div => "div.rn.f32",
                    BinaryOpKind::Mod => "fmod.f32",
                    _ => {
                        return Err(format!(
                            "ptx general: binary op {:?} outside the elementwise surface",
                            kind
                        ))
                    }
                };
                body.push_str(&format!("    {} {}, {}, {};\n", op, out, lreg, rreg));
            }
            Expr::UnaryOp(kind, x) => {
                let xreg = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {};\n", xreg));
                self.emit_expr(x, &xreg, decl, body)?;
                if matches!(kind, crate::ast::UnaryOpKind::Neg) {
                    body.push_str(&format!("    mul.f32 {}, {}, -1.0e0;\n", out, xreg));
                } else {
                    body.push_str(&format!("    mov.f32 {}, {};\n", out, xreg));
                }
            }
            Expr::Call(name, args, _) => {
                self.emit_intrinsic_call(name, args, out, decl, body)?;
            }
            Expr::Cast(x, target) => {
                // 2026-09-18 (P0 f16 swap, plan 2026-09-18-fused-f16-decode-node):
                // a widening cast in value position lowers to its operand —
                // this surface computes in f32 registers and the Index arm
                // converts f16 storage at the load (ld.global.u16 +
                // cvt.f32.f16), so the f32 target is already satisfied. The
                // target resolves through the casting graph (rule 19 — no
                // type-name matching); anything else (narrowing to f16,
                // f64) stays outside the surface and errors honestly.
                let shape = self
                    .casting_graph
                    .resolve_spirv_shape(self.universe, target, self.int_bits)?;
                if matches!(
                    shape,
                    crate::casting::graph::SpirvShape::Float { bits: 32 }
                ) {
                    self.emit_expr(x, out, decl, body)?;
                } else {
                    return Err(format!(
                        "ptx general: cast to {:?} outside the elementwise surface",
                        shape
                    ));
                }
            }
            other => {
                return Err(format!(
                    "ptx general: expression {:?} outside the elementwise surface",
                    other
                ))
            }
        }
        Ok(())
    }

    /// Identifier read in value position: a let-bound local reads its
    /// register (f32 locals only — the index var is u32 and lives in index
    /// positions, never value positions); otherwise a baked const or a
    /// state scalar field.
    fn emit_ident_read(
        &mut self,
        name: &str,
        out: &str,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        if let Some(reg) = self.regs.get(name).cloned() {
            if reg.starts_with("%f") {
                body.push_str(&format!("    mov.f32 {}, {};\n", out, reg));
                return Ok(());
            }
        }
        if let Some(Expr::Decimal(n)) = self.consts.get(name) {
            // ptxas rejects integer literals in .f32 position (as does the
            // Decimal arm above) — bake with a decimal fraction.
            body.push_str(&format!("    mov.f32 {}, {}.0;\n", out, n));
        } else if let Some(Expr::Float(f)) = self.consts.get(name) {
            body.push_str(&format!("    mov.f32 {}, {:e};\n", out, f));
        } else {
            let off = self.field_off(name).ok_or_else(|| {
                format!("ptx general: scalar '{}' not in layout or consts", name)
            })?;
            body.push_str(&format!("    ld.global.f32 {}, [%rd1+{}];\n", out, off));
        }
        Ok(())
    }

    /// 2026-10-01 (plan `2026-10-01-atomic-element-rmw.md`): the
    /// compare-exchange — `atom.cas.b64 old, [addr], cmp, val` (TWO
    /// value operands; the compare rides the instruction, no setp).
    /// Same element addressing as the RMW family; swap iff equal,
    /// ALWAYS returns old.
    fn emit_atomic_cas_at(
        &mut self, args: &[Expr], out: &str,
    ) -> Result<(String, String), String> {
        if args.len() < 4 {
            return Err("AtomicCasAt# takes (buf, i, cmp, v)".into());
        }
        let mut d = String::new();
        let mut b = String::new();
        let buf_name = self.field_of(&args[0])?;
        let off = self
            .field_off(&buf_name)
            .ok_or_else(|| format!("ptx general: atomic target '{buf_name}' not in layout"))?;
        let elem = self.elem_bytes(&buf_name)?;
        if elem != 8 {
            return Err(format!(
                "ptx general: AtomicCasAt# is Int-arrays-only - '{buf_name}' has \
                 {elem}-byte elements"
            ));
        }
        let addr = self.array_addr(buf_name.clone(), off, elem, &args[1], &mut d, &mut b)?;
        let cmp_f = self.fresh_f();
        let val_f = self.fresh_f();
        d.push_str(&format!("    .reg .f32 {cmp_f};\n"));
        d.push_str(&format!("    .reg .f32 {val_f};\n"));
        self.emit_expr(&args[2], &cmp_f, &mut d, &mut b)?;
        self.emit_expr(&args[3], &val_f, &mut d, &mut b)?;
        let cmp = self.fresh_rd();
        let val = self.fresh_rd();
        let old = self.fresh_rd();
        d.push_str(&format!("    .reg .b64 {cmp};\n"));
        d.push_str(&format!("    .reg .b64 {val};\n"));
        d.push_str(&format!("    .reg .b64 {old};\n"));
        b.push_str(&format!("    cvt.rni.s64.f32 {cmp}, {cmp_f};\n"));
        b.push_str(&format!("    cvt.rni.s64.f32 {val}, {val_f};\n"));
        b.push_str(&format!(
            "    atom.acq_rel.gpu.global.cas.b64 {old}, [{addr}], {cmp}, {val};\n"
        ));
        b.push_str(&format!("    cvt.rn.f32.s64 {out}, {old};\n"));
        Ok((d, b))
    }

    /// 2026-10-01 (L1 primitive-coverage audit, gap #4): work-id names
    /// A pure local: lowered into a register of the DECLARED class —
    /// Int locals are u32 with integer ops (the GQA decompositions
    /// h = t/NKV need integer semantics); f32 division silently truncates
    /// only for power-of-2 divisors and corrupts the bit pattern for
    /// everything else. 2026-10-01 (plan `2026-10-01-atomic-element-rmw.md`
    /// A3): intrinsic calls yield f32-flattened Ints — routed through the
    /// expression emitter and converted into the u32 local; only pure
    /// integer arithmetic keeps the emit_index path.
    fn emit_local(
        &mut self, name: &str, e: &Expr, ty: &Option<crate::ast::Type>,
    ) -> (String, String) {
        let mut d = String::new();
        let mut b = String::new();
        if Self::is_int_ty(ty) {
            let reg = self.fresh_r();
            d.push_str(&format!("    .reg .u32 {};\n", reg));
            if matches!(e, Expr::Call(n, _, _) if n.ends_with('#')) {
                let f = self.fresh_f();
                d.push_str(&format!("    .reg .f32 {};\n", f));
                let _ = self.emit_expr(e, &f, &mut d, &mut b);
                b.push_str(&format!("    cvt.rzi.s32.f32 {}, {};\n", reg, f));
            } else {
                let _ = self.emit_index(e, &reg, &mut d, &mut b);
            }
            self.regs.insert(name.to_string(), reg);
        } else {
            let reg = self.fresh_f();
            d.push_str(&format!("    .reg .f32 {};\n", reg));
            let _ = self.emit_expr(e, &reg, &mut d, &mut b);
            self.regs.insert(name.to_string(), reg);
        }
        (d, b)
    }

    /// 2026-10-01 (plan `2026-10-01-atomic-element-rmw.md` A3): — the element
    /// address comes from the SAME math as a `buf[i]` access
    /// (`array_addr`: `mul.wide` + `add.u64` off the `%rd1` state base),
    /// then one true i64 atomic on global memory:
    /// `atom.acq_rel.gpu.global.add.u64` (seq_cst equivalent; sm_60+).
    /// Values flatten through f32 (the lane's Int convention — exact for
    /// kernel-realistic magnitudes, `cvt.rni` for the round-trip); the
    /// RMW itself never touches an f32 register.
    fn emit_atomic_at(
        &mut self, args: &[Expr], out: &str, op: AtomicAtOp, negate: bool,
    ) -> Result<(String, String), String> {
        if args.len() < 3 {
            return Err("the At-family atomics take (buf, i, v)".into());
        }
        let mut d = String::new();
        let mut b = String::new();
        let buf_name = self.field_of(&args[0])?;
        let off = self
            .field_off(&buf_name)
            .ok_or_else(|| format!("ptx general: atomic target '{buf_name}' not in layout"))?;
        let elem = self.elem_bytes(&buf_name)?;
        if elem != 8 {
            return Err(format!(
                "ptx general: AtomicAddAt# is Int-arrays-only - '{buf_name}' has \
                 {elem}-byte elements (GPU Float arrays are f32 storage; Int is i64)"
            ));
        }
        let addr = self.array_addr(buf_name.clone(), off, elem, &args[1], &mut d, &mut b)?;
        let v = self.fresh_f();
        d.push_str(&format!("    .reg .f32 {};\n", v));
        self.emit_expr(&args[2], &v, &mut d, &mut b)?;
        let v64 = self.fresh_rd();
        let old = self.fresh_rd();
        d.push_str(&format!("    .reg .b64 {v64};\n"));
        d.push_str(&format!("    .reg .b64 {old};\n"));
        b.push_str(&format!("    cvt.rni.s64.f32 {}, {};\n", v64, v));
        if negate {
            b.push_str(&format!("    neg.s64 {}, {v64};\n", v64));
        }
        let (ptx_op, ty) = match op {
            AtomicAtOp::Add => ("add", "u64"),
            AtomicAtOp::Exch => ("exch", "b64"),
        };
        b.push_str(&format!(
            "    atom.acq_rel.gpu.global.{ptx_op}.{ty} {}, [{}], {};\n",
            old, addr, v64
        ));
        b.push_str(&format!("    cvt.rn.f32.s64 {}, {};\n", out, old));
        Ok((d, b))
    }

    /// over the structural ids. Contract mirrors the SPIR-V lane
    /// (spirv/lower.rs: a CONSTANT dim 0..=2, integer value); this lane
    /// flattens Ints to f32 (emit_expr's own convention — Decimal
    /// literals already emit as mov.f32), so the id lands via
    /// `cvt.rn.f32.u32` — identical for every f32 use. dim 0 is the flat
    /// gid the prologue computed (ctaid.x*BLOCK+tid.x); dims 1/2 see
    /// ctaid 0 (1D launches), so the global id is the raw tid.
    fn emit_work_id(
        &mut self, name: &str, args: &[Expr], out: &str,
    ) -> Result<(String, String), String> {
        let Some(Expr::Decimal(dim)) = args.first() else {
            return Err(format!(
                "{name} takes a constant dimension 0..=2 - Fix: \
                 write the dimension as a literal, e.g. {name}(0)"
            ));
        };
        if *dim < 0 || *dim > 2 {
            return Err(format!("{name} dimension must be 0..=2, got {dim}"));
        }
        let src: &str = match (name, dim) {
            ("GetGlobalId#", 0) => self.gid,
            ("GetGlobalId#", 1) => "%tid.y",
            ("GetGlobalId#", 2) => "%tid.z",
            ("GetLocalId#", 0) => "%tid.x",
            ("GetLocalId#", 1) => "%tid.y",
            _ => "%tid.z",
        };
        let mut d = String::new();
        let mut b = String::new();
        if src.starts_with('%') && src[1..].starts_with("tid") {
            // A special register: stage through a declared u32 temp.
            let tmp = self.fresh_named_u32();
            d.push_str(&format!("    .reg .u32 {};\n", tmp));
            b.push_str(&format!("    mov.u32 {}, {};\n", tmp, src));
        }
        b.push_str(&format!("    cvt.rn.f32.u32 {}, {};\n", out, src));
        Ok((d, b))
    }

    fn emit_intrinsic_call(
        &mut self,
        name: &str,
        args: &[Expr],
        out: &str,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        match name {
            "Exp#" => {
                if args.len() != 1 {
                    return Err("Exp# takes exactly 1 argument".into());
                }
                let xreg = self.fresh_f();
                let treg = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {}, {};\n", xreg, treg));
                self.emit_expr(&args[0], &xreg, decl, body)?;
                // 2026-09-17 ptxas correction: there is NO exp.approx.f32 in
                // PTX — exp(x) = ex2(x * log2(e)). log2(e) = 0f3FB8AA3B.
                body.push_str(&format!("    mul.f32 {}, {}, 0f3FB8AA3B;\n", treg, xreg));
                body.push_str(&format!("    ex2.approx.f32 {}, {};\n", out, treg));
                Ok(())
            }
            "Max#" | "Min#" => {
                if args.len() != 2 {
                    return Err(format!("{} takes (a, b)", name));
                }
                let areg = self.fresh_f();
                let breg = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {};\n", areg));
                decl.push_str(&format!("    .reg .f32 {};\n", breg));
                self.emit_expr(&args[0], &areg, decl, body)?;
                self.emit_expr(&args[1], &breg, decl, body)?;
                let op = if name == "Max#" { "max" } else { "min" };
                body.push_str(&format!("    {}.f32 {}, {}, {};\n", op, out, areg, breg));
                Ok(())
            }
            // 2026-10-01 (plan 2026-10-01-atomic-element-rmw.md A3): the
            // element-addressed atomic — see `emit_atomic_add_at`.
            "AtomicAddAt#" => {
                let (d, b) = self.emit_atomic_at(args, out, AtomicAtOp::Add, false)?;
                decl.push_str(&d);
                body.push_str(&b);
                Ok(())
            }
            "AtomicSubAt#" => {
                // PTX has no atomic sub — add of the negated value is
                // the identical wrapping-i64 RMW (the same route LLVM's
                // atomicrmw sub takes through negation on some targets).
                let (d, b) = self.emit_atomic_at(args, out, AtomicAtOp::Add, true)?;
                decl.push_str(&d);
                body.push_str(&b);
                Ok(())
            }
            "AtomicXchgAt#" => {
                let (d, b) = self.emit_atomic_at(args, out, AtomicAtOp::Exch, false)?;
                decl.push_str(&d);
                body.push_str(&b);
                Ok(())
            }
            "AtomicCasAt#" => {
                // atom.cas takes TWO value operands (cmp, new) — its own
                // emitter; the same element addressing.
                let (d, b) = self.emit_atomic_cas_at(args, out)?;
                decl.push_str(&d);
                body.push_str(&b);
                Ok(())
            }
            // 2026-10-01 (L1 primitive-coverage audit, gap #4): work-id
            // names over the structural ids — see `emit_work_id`.
            "GetGlobalId#" | "GetLocalId#" => {
                let (d, b) = self.emit_work_id(name, args, out)?;
                decl.push_str(&d);
                body.push_str(&b);
                Ok(())
            }
            // 2026-10-01 (L1 primitive-coverage audit, primitive-coverage.md
            // section 3): parity with the SPIR-V lane (spirv/lower.rs lowers
            // both via GLSL.std.450) — the registry and the accel purity
            // gate already admit these; only the PTX arm was missing.
            "Sqrt#" => {
                if args.len() != 1 {
                    return Err("Sqrt# takes exactly 1 argument".into());
                }
                let xreg = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {};\n", xreg));
                self.emit_expr(&args[0], &xreg, decl, body)?;
                body.push_str(&format!("    sqrt.rn.f32 {}, {};\n", out, xreg));
                Ok(())
            }
            "Fabs#" => {
                if args.len() != 1 {
                    return Err("Fabs# takes exactly 1 argument".into());
                }
                let xreg = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {};\n", xreg));
                self.emit_expr(&args[0], &xreg, decl, body)?;
                body.push_str(&format!("    abs.f32 {}, {};\n", out, xreg));
                Ok(())
            }
            "Fma#" => {
                if args.len() != 3 {
                    return Err("Fma# takes (a, b, c)".into());
                }
                let areg = self.fresh_f();
                let breg = self.fresh_f();
                let creg = self.fresh_f();
                decl.push_str(&format!("    .reg .f32 {};\n", areg));
                decl.push_str(&format!("    .reg .f32 {};\n", breg));
                decl.push_str(&format!("    .reg .f32 {};\n", creg));
                self.emit_expr(&args[0], &areg, decl, body)?;
                self.emit_expr(&args[1], &breg, decl, body)?;
                self.emit_expr(&args[2], &creg, decl, body)?;
                body.push_str(&format!("    fma.rn.f32 {}, {}, {}, {};\n", out, areg, breg, creg));
                Ok(())
            }
            "ShuffleDown#" | "ShuffleXor#" | "SubgroupBallot#" | "SubgroupBroadcast#" => {
                self.emit_lane_intrinsic(name, args, out, decl, body)
            }
            "SubgroupFAdd#" | "SubgroupFMax#" | "SubgroupFMin#" => {
                self.emit_warp_reduce(name, args, out, decl, body)
            }
            _ => Err(format!(
                "ptx general: intrinsic call '{}' not supported in elementwise kernel",
                name
            )),
        }
    }

    /// Lane-coordination family: shuffles (Down/Xor — value + constant lane
    /// selector), broadcast (value + constant lane index), ballot (predicate
    /// → warp bitmask). Split from the dispatcher (2026-09-17 warp-pr plan).
    fn emit_lane_intrinsic(
        &mut self,
        name: &str,
        args: &[Expr],
        out: &str,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        if name == "SubgroupBallot#" {
            if args.len() != 1 {
                return Err("SubgroupBallot# takes (pred)".into());
            }
            let preg = self.fresh_r();
            decl.push_str(&format!("    .reg .pred {};\n", preg));
            let xreg = self.fresh_f();
            decl.push_str(&format!("    .reg .f32 {};\n", xreg));
            self.emit_expr(&args[0], &xreg, decl, body)?;
            body.push_str(&format!("    setp.ne.f32 {}, {}, 0.0;\n", preg, xreg));
            // vote.sync.ballot: verified against sm_86 ptxas (2026-09-17)
            body.push_str(&format!("    vote.sync.ballot.b32 {}, {}, 0xffffffff;\n", out, preg));
            return Ok(());
        }
        // Shuffle family: (f32 value, compile-time-constant lane selector).
        // 2026-09-17 ptxas-verified forms on sm_86 (CUDA 13.4):
        //   shfl.sync.{down|bfly|idx}.b32 d, a, b, clamp, membermask;
        // — .sync BEFORE the mode, FIVE operands (clamp required), b32-only
        // registers (f32 values punning through mov.b32), xor = bfly.
        if args.len() != 2 {
            return Err(format!("{} takes (value, lane_selector)", name));
        }
        let vreg = self.fresh_f();
        let sreg = self.fresh_r();
        let ra = self.fresh_r();
        let rb = self.fresh_r();
        let fout = self.fresh_f();
        decl.push_str(&format!("    .reg .f32 {}, {};\n", vreg, fout));
        decl.push_str(&format!("    .reg .u32 {}, {}, {};\n", sreg, ra, rb));
        self.emit_expr(&args[0], &vreg, decl, body)?;
        if let Expr::Decimal(n) = &args[1] {
            body.push_str(&format!("    mov.u32 {}, {};\n", sreg, n));
        } else {
            return Err(format!("{} lane selector must be a compile-time constant", name));
        }
        let mode = match name {
            "ShuffleDown#" => "down",
            "ShuffleXor#" => "bfly",
            _ => "idx",
        };
        body.push_str(&format!("    mov.b32 {}, {};\n", ra, vreg));
        body.push_str(&format!(
            "    shfl.sync.{}.b32 {}, {}, {}, 0x1f, 0xffffffff;\n",
            mode, rb, ra, sreg
        ));
        body.push_str(&format!("    mov.b32 {}, {};\n", fout, rb));
        body.push_str(&format!("    mov.f32 {}, {};\n", out, fout));
        Ok(())
    }

    /// Warp-wide float reduction. 2026-09-17 correction: `redux.sync.*.f32`
    /// is NOT supported on sm_86 (integer redux only until sm_100) — the
    /// phase-1 plan's redux finding was wrong for float. The sm_86 form is
    /// the butterfly shuffle tree: 5 rounds (16/8/4/2/1), f32 punning
    /// through b32, combine in registers. Butterfly gives EVERY lane the
    /// warp total (the SPIR-V Reduce semantics the intrinsics promise).
    fn emit_warp_reduce(
        &mut self,
        name: &str,
        args: &[Expr],
        out: &str,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        if args.len() != 1 {
            return Err(format!("{} takes (v)", name));
        }
        let vreg = self.fresh_f();
        decl.push_str(&format!("    .reg .f32 {};\n", vreg));
        self.emit_expr(&args[0], &vreg, decl, body)?;
        let op = match name {
            "SubgroupFMax#" => "max.f32",
            "SubgroupFMin#" => "min.f32",
            _ => "add.f32",
        };
        // f32 <-> b32 scratch, reused across rounds.
        let fa = self.fresh_f();
        let ra = self.fresh_r();
        let rb = self.fresh_r();
        decl.push_str(&format!("    .reg .f32 {};\n", fa));
        decl.push_str(&format!("    .reg .u32 {}, {};\n", ra, rb));
        body.push_str(&format!("    mov.f32 {}, {};\n", fa, vreg));
        for offset in [16u32, 8, 4, 2, 1] {
            body.push_str(&format!("    mov.b32 {}, {};\n", ra, fa));
            body.push_str(&format!(
                "    shfl.sync.bfly.b32 {}, {}, {}, 0x1f, 0xffffffff;\n",
                rb, ra, offset
            ));
            // fa = fa op rb  — combine on the f32 side
            let fb = self.fresh_f();
            decl.push_str(&format!("    .reg .f32 {};\n", fb));
            body.push_str(&format!("    mov.b32 {}, {};\n", fb, rb));
            body.push_str(&format!("    {} {}, {}, {};\n", op, fa, fa, fb));
        }
        body.push_str(&format!("    mov.f32 {}, {};\n", out, fa));
        Ok(())
    }

    fn elem_bytes(&self, name: &str) -> Result<u64, String> {
        self.layout
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.elem_bytes as u64)
            .ok_or_else(|| format!("ptx general: field '{}' not in layout", name))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinaryOpKind::*;

    fn id(n: &str) -> Expr {
        Expr::Identifier(n.into())
    }
    fn num(n: i64) -> Expr {
        Expr::Decimal(n)
    }
    fn idx(buf: &str, e: Expr) -> Expr {
        Expr::Index(Box::new(id(buf)), Box::new(e))
    }
    fn bin(kind: BinaryOpKind, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(kind, Box::new(l), Box::new(r))
    }
    fn consts() -> std::collections::HashMap<String, Expr> {
        let mut m = std::collections::HashMap::new();
        m.insert("D".into(), num(128));
        m.insert("NKV".into(), num(4096));
        m.insert("G".into(), num(4));
        m
    }

    /// pv-like reduction addend: acc = acc + o1[h*NKV + j] * v[kh*NKV*D + j*D + di]
    fn pv_addend() -> Expr {
        bin(
            Add,
            id("acc"),
            bin(
                Mul,
                idx("o1", bin(Add, bin(Mul, id("h"), id("NKV")), id("j"))),
                idx(
                    "v",
                    bin(
                        Add,
                        bin(
                            Add,
                            bin(Mul, id("kh"), bin(Mul, id("NKV"), id("D"))),
                            bin(Mul, id("j"), id("D")),
                        ),
                        id("di"),
                    ),
                ),
            ),
        )
    }

    #[test]
    fn linear_coeff_extracts_item_stride() {
        let c = consts();
        // h*NKV + j -> coefficient 1
        assert_eq!(
            linear_coeff(&bin(Add, bin(Mul, id("h"), id("NKV")), id("j")), "j", &c),
            Some(1)
        );
        // j*D -> coefficient D (const ident factor)
        assert_eq!(
            linear_coeff(&bin(Mul, id("j"), id("D")), "j", &c),
            Some(128)
        );
        // kh*NKV*D + j*D + di -> coefficient D
        assert_eq!(
            linear_coeff(
                &bin(
                    Add,
                    bin(Add, bin(Mul, id("kh"), bin(Mul, id("NKV"), id("D"))), bin(Mul, id("j"), id("D"))),
                    id("di"),
                ),
                "j",
                &c
            ),
            Some(128)
        );
        // quadratic: j*j -> None
        assert_eq!(linear_coeff(&bin(Mul, id("j"), id("j")), "j", &c), None);
        // non-linear: nested index k[ii[j]] -> None
        assert_eq!(
            linear_coeff(&idx("k", idx("ii", id("j"))), "j", &c),
            None
        );
    }

    #[test]
    fn subst_item_bakes_the_range_start() {
        let e = bin(Add, bin(Mul, id("h"), id("NKV")), id("j"));
        let out = subst_item(&e, "j", &num(0));
        assert_eq!(
            format!("{:?}", out),
            format!("{:?}", bin(Add, bin(Mul, id("h"), id("NKV")), num(0)))
        );
    }

    #[test]
    fn serial_unroll_matches_the_pv_body() {
        let c = consts();
        let body = vec![Statement::Assign(id("acc"), pv_addend())];
        let plan = SerialUnrollPlan::match_body(&body, &ctx("j", 0, 4096, 4), &c)
            .expect("pv body must match");
        assert_eq!(plan.sites.len(), 2, "o1 and v sites");
        let strides: Vec<i64> = plan.sites.iter().map(|(_, a)| *a).collect();
        assert!(strides.contains(&1), "o1 stride: {:?}", strides);
        assert!(strides.contains(&128), "v stride: {:?}", strides);
        // Non-divisible trip count keeps the serial form.
        assert!(SerialUnrollPlan::match_body(&body, &ctx("j", 0, 4095, 4), &c).is_none());
        // Unroll factor below 2 is a no-op request.
        assert!(SerialUnrollPlan::match_body(&body, &ctx("j", 0, 4096, 1), &c).is_none());
        // A foreach inside the body is not serial-unrollable.
        let nested = vec![Statement::Foreach {
            item: "k".into(),
            list: Box::new(Expr::Range {
                start: Box::new(num(0)),
                end: Box::new(id("D")),
                inclusive: false,
            }),
            body: vec![Statement::Assign(id("acc"), pv_addend())],
        }];
        assert!(SerialUnrollPlan::match_body(&nested, &ctx("j", 0, 4096, 4), &c).is_none());
    }

    fn ctx(item: &str, start: i64, end: i64, unroll: usize) -> UnrollCtx<'_> {
        UnrollCtx {
            item,
            start,
            end,
            unroll,
        }
    }

    fn f64_field(name: &str, proj: u64, elems: u64) -> crate::backend::spirv::runner::RunnerField {
        crate::backend::spirv::runner::RunnerField {
            name: name.into(),
            offset: proj,
            proj_offset: proj,
            elem_bytes: 4,
            count: elems,
            is_array: true,
            type_is_float: true,
        }
    }

    fn pv_layout() -> crate::backend::spirv::runner::SsboLayout {
        crate::backend::spirv::runner::SsboLayout {
            fields: vec![
                f64_field("out", 0, 2560),
                f64_field("o1", 1 << 16, 81920),
                f64_field("v", 1 << 20, 2_621_440),
            ],
            images: vec![],
            state_bytes: 1 << 24,
            program_bytes: 1 << 24,
        }
    }

    fn pv_shape() -> crate::analysis::accel::KernelShape {
        // let h = t / NKV; let kh = h / G; let di = t - h * D;
        // acc = 0; foreach j in 0..NKV { acc = acc + o1[..j..] * v[..j*D+di..]; }
        // out[t] = acc;
        let lets = [
            Statement::Let {
                name: "h".into(),
                names: vec![],
                ty: Some(crate::ast::Type::Custom("Int".into())),
                expr: Some(bin(Div, id("t"), id("NKV"))),
                modifiers: vec![],
            },
            Statement::Let {
                name: "kh".into(),
                names: vec![],
                ty: Some(crate::ast::Type::Custom("Int".into())),
                expr: Some(bin(Div, id("h"), id("G"))),
                modifiers: vec![],
            },
            Statement::Let {
                name: "di".into(),
                names: vec![],
                ty: Some(crate::ast::Type::Custom("Int".into())),
                expr: Some(bin(
                    Sub,
                    id("t"),
                    bin(Mul, id("h"), id("D")),
                )),
                modifiers: vec![],
            },
            Statement::Let {
                name: "acc".into(),
                names: vec![],
                ty: None,
                expr: Some(crate::ast::Expr::Float(0.0)),
                modifiers: vec![],
            },
        ];
        let mut kernel_stmts: Vec<Statement> = lets.into_iter().collect();
        kernel_stmts.push(Statement::Foreach {
            item: "j".into(),
            list: Box::new(Expr::Range {
                start: Box::new(num(0)),
                end: Box::new(id("NKV")),
                inclusive: false,
            }),
            body: vec![Statement::Assign(id("acc"), pv_addend())],
        });
        kernel_stmts.push(Statement::Assign(idx("out", id("t")), id("acc")));
        crate::analysis::accel::KernelShape {
            index_var: "t".into(),
            count_expr: Some(num(2560)),
            kernel_stmts,
            host_stmts: vec![],
            read_buffers: vec!["o1".into(), "v".into()],
            write_buffers: vec!["out".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: None,
        }
    }

    #[test]
    fn warp_slice_emission_has_block_guard_merge_and_bar() {
        // Direct emitter test (config-independent — the knob ships off
        // until the slice composes with lane-mapping; see the plan's M1
        // status note).
        let shape = pv_shape();
        let shape = pv_shape();
        let layout = pv_layout();
        let consts = consts();
        let universe = crate::type_universe::TypeUniverse::new();
        let mut g = Gen::new(&layout, &consts, 2560, &universe, 32);
        g.block_work_item = true;
        g.regs.insert("h".into(), "%r1".into());
        g.regs.insert("d".into(), "%r6".into());
        g.regs.insert("kh".into(), "%r11".into());
        // The accumulator binding the sliced loop merges into.
        g.regs.insert("acc".into(), "%f0".into());
        let mut decl = String::from("    .reg .b64  %rd1;\n");
        let mut body = String::from("    mov.u32 %r1, %ctaid.x;\n");
        // The pv j loop body (the slice's statements).
        let body_stmts = vec![Statement::Assign(
            id("acc"),
            bin(
                Add,
                id("acc"),
                bin(
                    Mul,
                    idx("o1", bin(Add, bin(Mul, id("h"), id("NKV")), id("j"))),
                    idx(
                        "v",
                        bin(
                            Add,
                            bin(
                                Add,
                                bin(Mul, id("kh"), bin(Mul, id("NKV"), id("D"))),
                                bin(Mul, id("j"), id("D")),
                            ),
                            id("d"),
                        ),
                    ),
                ),
            ),
        )];
        g.emit_warp_sliced(
            WarpSliceEmit {
                body_stmts: &body_stmts,
                item: "j",
                start: 0,
                span: 4096,
                warps: 4,
            },
            &mut decl,
            &mut body,
        )
        .expect("emits");
        let ptx = format!("{decl}{body}");
        // Warp id, exact quarter slices (4096/4), smem partials, barrier.
        assert!(ptx.contains("shr.u32"), "warp id: {ptx}");
        assert!(ptx.contains(", 1024, "), "slice quarter: {ptx}");
        assert!(ptx.contains(".shared .align 4 .b8 wpart[16];"), "smem: {ptx}");
        assert!(ptx.contains("st.shared.f32"), "partial store: {ptx}");
        assert!(ptx.contains("bar.sync 0;"), "merge barrier: {ptx}");
        assert_eq!(ptx.matches("ld.shared.f32").count(), 4, "4 partial reads");
        // has_warp_slice agrees at the shape level (mod.rs dispatch reads it).
        assert!(has_warp_slice(&shape.kernel_stmts, &consts));
    }

    #[test]
    fn warp_slice_emission_derives_geometry_from_warp_count() {
        // 2026-09-30 (stage-5 5c): warps=2 must re-derive EVERY count —
        // smem slots, quarter, merge reads — while the hardware facts
        // (32-lane warp id, 4-byte f32 slot, barrier) stay literal.
        let layout = pv_layout();
        let consts = consts();
        let universe = crate::type_universe::TypeUniverse::new();
        let mut g = Gen::new(&layout, &consts, 2560, &universe, 32);
        g.block_work_item = true;
        g.regs.insert("acc".into(), "%f0".into());
        let mut decl = String::from("    .reg .b64  %rd1;\n");
        let mut body = String::from("    mov.u32 %r1, %ctaid.x;\n");
        let body_stmts = vec![Statement::Assign(
            id("acc"),
            bin(Add, id("acc"), bin(Mul, idx("o1", id("j")), idx("v", id("j")))),
        )];
        g.emit_warp_sliced(
            WarpSliceEmit {
                body_stmts: &body_stmts,
                item: "j",
                start: 0,
                span: 4096,
                warps: 2,
            },
            &mut decl,
            &mut body,
        )
        .expect("emits");
        let ptx = format!("{decl}{body}");
        assert!(ptx.contains(".shared .align 4 .b8 wpart[8];"), "smem = warps*4: {ptx}");
        assert!(ptx.contains(", 2048, "), "quarter = span/2: {ptx}");
        assert_eq!(
            ptx.matches("ld.shared.f32").count(),
            2,
            "1 + (warps - 1) partial reads: {ptx}"
        );
        assert!(ptx.contains("bar.sync 0;"), "merge barrier: {ptx}");
        assert!(ptx.contains("shr.u32"), "warp id stays a hardware fact: {ptx}");
    }

    #[test]
    fn warp_slice_span_predicate_edges() {
        // The shared predicate reproduces the original literals at the
        // default config pair (512 / 4)…
        assert!(!warp_slice_span_ok(511, 512, 4), "below min_span");
        assert!(warp_slice_span_ok(512, 512, 4), "exact min_span");
        assert!(!warp_slice_span_ok(513, 512, 4), "not divisible by warps");
        // …and each tunable moves the decision independently.
        assert!(warp_slice_span_ok(256, 64, 4), "min_span 64 admits 256");
        assert!(!warp_slice_span_ok(256, 512, 4), "same span, default min");
        assert!(warp_slice_span_ok(4096, 512, 8), "warps 8 divides 4096");
        assert!(!warp_slice_span_ok(514, 512, 8), "warps 8 rejects 514");
        // Defensive: warps=0 never divides (the config loader clamps to
        // 1..=8; this guards non-config callers from a modulo panic).
        assert!(!warp_slice_span_ok(512, 512, 0), "zero warps reject");
        // Dispatch geometry derives from the warp count.
        assert_eq!(warp_slice_block_threads(4), 128, "default geometry");
        assert_eq!(warp_slice_block_threads(2), 64);
        assert_eq!(warp_slice_block_threads(8), 256);
    }

    #[test]
    fn has_warp_slice_with_applies_configured_thresholds() {
        // 2026-09-30 (stage-5 5c): explicit-threshold detector — the
        // unit-testable path (the config wrapper reads a process-wide
        // OnceLock the CLI owns; unit tests must not poison it).
        // Default shape (span 4096): non-divisor warp counts reject —
        // detector and emission gate share this predicate, so a config
        // the emitter's exact-slice arithmetic could not serve is never
        // half-selected.
        let shape = pv_shape();
        let full_span_consts = consts();
        assert!(has_warp_slice_with(
            &shape.kernel_stmts,
            &full_span_consts,
            512,
            4
        ));
        assert!(has_warp_slice_with(
            &shape.kernel_stmts,
            &full_span_consts,
            512,
            8
        ));
        assert!(!has_warp_slice_with(
            &shape.kernel_stmts,
            &full_span_consts,
            512,
            3
        ));

        // Span 256 (NKV pinned): rejected at default min_span, admitted
        // when the configured threshold drops.
        let mut shape = pv_shape();
        let mut short_span_consts = full_span_consts;
        short_span_consts.insert("NKV".into(), num(256));
        if let Some(Statement::Foreach { list, .. }) = shape
            .kernel_stmts
            .iter_mut()
            .find(|s| matches!(s, Statement::Foreach { .. }))
        {
            *list = Box::new(Expr::Range {
                start: Box::new(num(0)),
                end: Box::new(num(256)),
                inclusive: false,
            });
        }
        assert!(!has_warp_slice_with(&shape.kernel_stmts, &short_span_consts, 512, 4));
        assert!(has_warp_slice_with(&shape.kernel_stmts, &short_span_consts, 64, 4));
        assert!(!has_warp_slice_with(&shape.kernel_stmts, &short_span_consts, 64, 3), "256 % 3 != 0");
    }

    #[test]
    /// 2026-10-01 (D14 remainder): the declared `unroll<N>` overrides
    /// the derived factor (D2) — 2 slots instead of the config default's
    /// 4: half the back-to-back loads, the counter steps by 2.
    #[test]
    fn declared_unroll_overrides_the_derived_factor() {
        let mut consts = consts();
        consts.insert("NKV".into(), num(256));
        let mut shape = pv_shape();
        if let Some(Statement::Foreach { list, .. }) = shape
            .kernel_stmts
            .iter_mut()
            .find(|s| matches!(s, Statement::Foreach { .. }))
        {
            *list = Box::new(Expr::Range {
                start: Box::new(num(0)),
                end: Box::new(num(256)),
                inclusive: false,
            });
        }
        let ptx = emit_general_ptx(
            &shape,
            2560,
            &pv_layout(),
            &consts,
            &crate::type_universe::TypeUniverse::new(),
            GeneralEmitOpts {
                int_bits: 32,
                deferred_split: 1,
                unroll_override: Some(2),
                ..Default::default()
            },
        )
        .expect("emits");
        // 2 slots x 2 sites = 4 loads (the default 4x gives 8).
        let loads: Vec<&str> = ptx.lines().filter(|l| l.contains("ld.global.f32")).collect();
        assert_eq!(loads.len(), 4, "2x unroll x 2 sites: {ptx}");
        assert!(
            ptx.lines().any(|l| l.trim().starts_with("add.u32") && l.trim_end().ends_with(", 2;")),
            "counter steps by 2: {ptx}"
        );
    }

    fn serial_unroll_fires_when_the_slice_does_not_apply() {
        // NKV=256: span 256 < 512 -> below the slice threshold, the unroll
        // keeps the loop (MLP within one warp).
        let mut shape = pv_shape();
        let mut consts = consts();
        consts.insert("NKV".into(), num(256));
        // Rebuild the j loop end so the span follows the consts.
        if let Some(Statement::Foreach { list, .. }) = shape
            .kernel_stmts
            .iter_mut()
            .find(|s| matches!(s, Statement::Foreach { .. }))
        {
            *list = Box::new(Expr::Range {
                start: Box::new(num(0)),
                end: Box::new(num(256)),
                inclusive: false,
            });
        }
        let ptx = emit_general_ptx(
            &shape,
            2560,
            &pv_layout(),
            &consts,
            &crate::type_universe::TypeUniverse::new(),
            GeneralEmitOpts { int_bits: 32, deferred_split: 1, ..Default::default() },
        )
        .expect("emits");
        assert!(!has_warp_slice(&shape.kernel_stmts, &consts));
        // 4 slots x 2 sites = 8 back-to-back loads per iteration.
        let loads: Vec<&str> = ptx.lines().filter(|l| l.contains("ld.global.f32")).collect();
        assert_eq!(loads.len(), 8, "4x unroll x 2 sites: {ptx}");
        // Running pointers advance by the whole unroll span per iteration:
        // o1 by 4*1*4 = 16 bytes, v by 4*128*4 = 2048 bytes.
        assert!(ptx.contains(", 16;\n"), "o1 pointer bump: {ptx}");
        assert!(ptx.contains(", 2048;\n"), "v pointer bump: {ptx}");
        // The loop counter advances by the unroll factor, not by 1.
        assert!(
            ptx.lines().any(|l| l.trim().starts_with("add.u32") && l.trim_end().ends_with(", 4;")),
            "counter steps by 4: {ptx}"
        );
    }

    #[test]
    fn deferred_combine_emits_online_softmax_merge() {
        use crate::backend::spirv::runner::{RunnerField, SsboLayout};
        let f = |name: &str, proj: u64| RunnerField {
            name: name.into(),
            offset: proj,
            proj_offset: proj,
            elem_bytes: 4,
            count: 65536,
            is_array: true,
            type_is_float: true,
        };
        let layout = SsboLayout {
            fields: vec![f("o1", 0), f("a_out", 262144)],
            images: vec![],
            state_bytes: 524288,
            program_bytes: 524288,
        };
        let ptx = emit_deferred_combine_ptx(
            &layout,
            "o1",
            "a_out",
            CombineSpec { count: 32, dim: 128, split: 4 },
        )
        .expect("combine emits");
        assert!(ptx.contains("ex2.approx.f32"), "exp merge: {ptx}");
        assert!(ptx.contains("max.f32"), "running max: {ptx}");
        assert!(ptx.contains("div.rn.f32"), "normalize: {ptx}");
        assert!(ptx.contains("mul.lo.u32 %r3, %r3, 4;"), "split stride: {ptx}");
        assert!(ptx.contains("setp.ge.u32 %p1, %r1, 32;"), "count guard: {ptx}");
        assert!(ptx.contains("setp.ge.u32 %p2, %r2, 128;"), "dim guard: {ptx}");
        // 2026-10-01 (split device-validation): the merge's running-max init
        // must be the -inf immediate the JIT accepts (a one-nibble-short
        // literal was a parse error on-device; shape strings cannot see it).
        assert!(
            ptx.contains(&f32_imm(f32::NEG_INFINITY)),
            "-inf immediate: {ptx}"
        );
        assert_ptx_well_formed(&ptx);
    }

    /// 2026-10-01 (plan `2026-10-01-atomic-element-rmw.md` A3): the
    /// element-addressed atomic lowers to the element-address math +
    /// ONE true i64 `atom.acq_rel.gpu.global.add.u64`. The Int field
    /// (8-byte elements) is addressed like any buffer; the ptxas smoke
    /// (when present) proves the instruction syntax.
    #[test]
    fn atomic_add_at_lowers_to_true_i64_atomic() {
        use crate::backend::spirv::runner::{RunnerField, SsboLayout};
        let int_field = RunnerField {
            name: "total".into(),
            offset: 4096,
            proj_offset: 4096,
            elem_bytes: 8,
            count: 8,
            is_array: true,
            type_is_float: false,
        };
        let f_field = RunnerField {
            name: "a".into(),
            offset: 0,
            proj_offset: 0,
            elem_bytes: 4,
            count: 1024,
            is_array: true,
            type_is_float: true,
        };
        let layout = SsboLayout {
            fields: vec![f_field, int_field.clone()],
            images: vec![],
            state_bytes: 8192,
            program_bytes: 8192,
        };
        let shape = crate::analysis::accel::KernelShape {
            index_var: "i".into(),
            count_expr: Some(Expr::Decimal(1024)),
            kernel_stmts: vec![
                Statement::Let {
                    name: "old".into(),
                    names: vec![],
                    ty: None,
                    expr: Some(Expr::Call(
                        "AtomicAddAt#".into(),
                        vec![
                            Expr::Identifier("total".into()),
                            Expr::Decimal(0),
                            Expr::Decimal(1),
                        ],
                        None,
                    )),
                    modifiers: vec![],
                },
                Statement::Assign(idx("res", id("i")), id("old")),
                Statement::Assign(id("i"), bin(Add, id("i"), num(1))),
            ],
            host_stmts: vec![],
            read_buffers: vec![],
            write_buffers: vec!["res".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: None,
        };
        let mut layout2 = layout;
        layout2.fields.insert(1, RunnerField {
            name: "res".into(),
            offset: 4096,
            proj_offset: 4096,
            elem_bytes: 4,
            count: 1024,
            is_array: true,
            type_is_float: true,
        });
        let consts = std::collections::HashMap::new();
        let universe = crate::type_universe::TypeUniverse::new();
        let ptx = emit_general_ptx(
            &shape,
            1024,
            &layout2,
            &consts,
            &universe,
            GeneralEmitOpts { int_bits: 32, deferred_split: 0, ..Default::default() },
        )
        .unwrap_or_else(|e| panic!("atomic emits: {e}"));
        assert!(
            ptx.contains("atom.acq_rel.gpu.global.add.u64"),
            "the true i64 atomic:\n{ptx}"
        );
        assert!(ptx.contains("cvt.rni.s64.f32"), "value flatten:\n{ptx}");
        assert!(ptx.contains("cvt.rn.f32.s64"), "old flatten:\n{ptx}");
        assert!(ptx.contains("mul.wide.u32"), "element addressing:\n{ptx}");
        assert_ptx_well_formed(&ptx);
        // ptxas smoke: the instruction syntax must assemble.
        if std::process::Command::new("ptxas").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            let blob = crate::backend::ptx::compile_cubin(&ptx, 0);
            assert!(blob.is_some(), "ptxas must accept the atomic PTX");
        }
        // 2026-10-01: the family expansion — Sub lowers to neg + atom.add
        // (PTX has no atomic sub; the wrapping-i64 RMW is identical).
        let sub_shape = crate::analysis::accel::KernelShape {
            kernel_stmts: vec![
                Statement::Let {
                    name: "old".into(),
                    names: vec![],
                    ty: None,
                    expr: Some(Expr::Call(
                        "AtomicSubAt#".into(),
                        vec![
                            Expr::Identifier("total".into()),
                            Expr::Decimal(0),
                            Expr::Decimal(1),
                        ],
                        None,
                    )),
                    modifiers: vec![],
                },
                Statement::Assign(idx("res", id("i")), id("old")),
                Statement::Assign(id("i"), bin(Add, id("i"), num(1))),
            ],
            ..shape.clone()
        };
        let ptx_sub = emit_general_ptx(
            &sub_shape,
            1024,
            &layout2,
            &consts,
            &universe,
            GeneralEmitOpts { int_bits: 32, deferred_split: 0, ..Default::default() },
        )
        .unwrap_or_else(|e| panic!("sub emits: {e}"));
        assert!(ptx_sub.contains("neg.s64"), "the negation:\n{ptx_sub}");
        assert!(
            ptx_sub.contains("atom.acq_rel.gpu.global.add.u64"),
            "the same atomic:\n{ptx_sub}"
        );
        if std::process::Command::new("ptxas").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            assert!(
                crate::backend::ptx::compile_cubin(&ptx_sub, 0).is_some(),
                "ptxas must accept the sub PTX"
            );
        }
        // 2026-10-01: Xchg — exch.b64 (the exchange type suffix).
        let xchg_shape = crate::analysis::accel::KernelShape {
            kernel_stmts: vec![
                Statement::Let {
                    name: "old".into(),
                    names: vec![],
                    ty: None,
                    expr: Some(Expr::Call(
                        "AtomicXchgAt#".into(),
                        vec![
                            Expr::Identifier("total".into()),
                            Expr::Decimal(0),
                            Expr::Decimal(1),
                        ],
                        None,
                    )),
                    modifiers: vec![],
                },
                Statement::Assign(idx("res", id("i")), id("old")),
                Statement::Assign(id("i"), bin(Add, id("i"), num(1))),
            ],
            ..shape.clone()
        };
        let ptx_xchg = emit_general_ptx(
            &xchg_shape,
            1024,
            &layout2,
            &consts,
            &universe,
            GeneralEmitOpts { int_bits: 32, deferred_split: 0, ..Default::default() },
        )
        .unwrap_or_else(|e| panic!("xchg emits: {e}"));
        assert!(
            ptx_xchg.contains("atom.acq_rel.gpu.global.exch.b64"),
            "the exchange:\n{ptx_xchg}"
        );
        if std::process::Command::new("ptxas").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            assert!(
                crate::backend::ptx::compile_cubin(&ptx_xchg, 0).is_some(),
                "ptxas must accept the xchg PTX"
            );
        }
        // 2026-10-01: Cas — atom.cas with TWO value operands.
        let cas_shape = crate::analysis::accel::KernelShape {
            kernel_stmts: vec![
                Statement::Let {
                    name: "old".into(),
                    names: vec![],
                    ty: None,
                    expr: Some(Expr::Call(
                        "AtomicCasAt#".into(),
                        vec![
                            Expr::Identifier("total".into()),
                            Expr::Decimal(0),
                            Expr::Decimal(5),
                            Expr::Decimal(50),
                        ],
                        None,
                    )),
                    modifiers: vec![],
                },
                Statement::Assign(idx("res", id("i")), id("old")),
                Statement::Assign(id("i"), bin(Add, id("i"), num(1))),
            ],
            ..shape.clone()
        };
        let ptx_cas = emit_general_ptx(
            &cas_shape,
            1024,
            &layout2,
            &consts,
            &universe,
            GeneralEmitOpts { int_bits: 32, deferred_split: 0, ..Default::default() },
        )
        .unwrap_or_else(|e| panic!("cas emits: {e}"));
        assert!(
            ptx_cas.contains("atom.acq_rel.gpu.global.cas.b64"),
            "the compare-exchange:\n{ptx_cas}"
        );
        if std::process::Command::new("ptxas").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            assert!(
                crate::backend::ptx::compile_cubin(&ptx_cas, 0).is_some(),
                "ptxas must accept the cas PTX"
            );
        }
    }

    /// 2026-10-01 (L1 primitive-coverage audit, gap #4): work-id names
    /// lower over the structural ids — dim 0 is the prologue's flat gid,
    /// dims 1/2 stage the tid special registers through a declared u32
    /// temp. The f32 flatten matches the lane's Int convention.
    #[test]
    fn work_ids_lower_over_structural_ids() {
        use crate::backend::spirv::runner::{RunnerField, SsboLayout};
        let field = |name: &str, off: u64| RunnerField {
            name: name.into(),
            offset: off,
            proj_offset: off,
            elem_bytes: 4,
            count: 1024,
            is_array: true,
            type_is_float: true,
        };
        let layout = SsboLayout {
            fields: vec![field("a", 0), field("res", 4096)],
            images: vec![],
            state_bytes: 8192,
            program_bytes: 8192,
        };
        let shape_for = |call: Expr| crate::analysis::accel::KernelShape {
            index_var: "i".into(),
            count_expr: Some(Expr::Decimal(1024)),
            kernel_stmts: vec![
                Statement::Assign(
                    idx("res", id("i")),
                    bin(Add, idx("a", id("i")), call),
                ),
                Statement::Assign(id("i"), bin(Add, id("i"), num(1))),
            ],
            host_stmts: vec![],
            read_buffers: vec!["a".into()],
            write_buffers: vec!["res".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: None,
        };
        let consts = std::collections::HashMap::new();
        let universe = crate::type_universe::TypeUniverse::new();
        for (call, needle) in [
            (
                Expr::Call("GetGlobalId#".into(), vec![Expr::Decimal(0)], None),
                "cvt.rn.f32.u32 ",
            ),
            (
                Expr::Call("GetLocalId#".into(), vec![Expr::Decimal(1)], None),
                "mov.u32 %u1, %tid.y;",
            ),
        ] {
            let ptx = emit_general_ptx(
                &shape_for(call),
                1024,
                &layout,
                &consts,
                &universe,
                GeneralEmitOpts { int_bits: 32, deferred_split: 0, ..Default::default() },
            )
            .unwrap_or_else(|e| panic!("work-id emits: {e}"));
            assert!(ptx.contains(needle), "expected `{needle}`:\n{ptx}");
            if needle.starts_with("cvt") {
                assert!(
                    ptx.contains("cvt.rn.f32.u32") && ptx.contains(", %r1;"),
                    "dim 0 reads the flat gid (%r1):\n{ptx}"
                );
            }
            assert_ptx_well_formed(&ptx);
        }
        // Non-constant dim = loud (the SPIR-V lane's contract).
        let bad = emit_general_ptx(
            &shape_for(Expr::Call(
                "GetGlobalId#".into(),
                vec![idx("a", id("i"))],
                None,
            )),
            1024,
            &layout,
            &consts,
            &universe,
            GeneralEmitOpts { int_bits: 32, deferred_split: 0, ..Default::default() },
        );
        assert!(bad.is_err(), "non-constant dim must error");
    }

    /// 2026-10-01 (L1 primitive-coverage audit, primitive-coverage.md
    /// §3): `Sqrt#`/`Fabs#` reach the PTX lane with the same admission
    /// the SPIR-V lane already had (registry + accel purity both admit
    /// them; only the PTX arm was missing) — a parity fill, locked by
    /// the instruction text and the well-formedness guard.
    #[test]
    fn sqrt_and_fabs_lower_to_ptx_instructions() {
        use crate::backend::spirv::runner::{RunnerField, SsboLayout};
        let field = |name: &str, off: u64| RunnerField {
            name: name.into(),
            offset: off,
            proj_offset: off,
            elem_bytes: 4,
            count: 1024,
            is_array: true,
            type_is_float: true,
        };
        let layout = SsboLayout {
            fields: vec![field("a", 0), field("out", 4096)],
            images: vec![],
            state_bytes: 8192,
            program_bytes: 8192,
        };
        let body_for = |intrinsic: &str| {
            vec![
                Statement::Assign(
                    idx("out", id("i")),
                    Expr::Call(
                        intrinsic.to_string(),
                        vec![idx("a", id("i"))],
                        None,
                    ),
                ),
                Statement::Assign(id("i"), bin(Add, id("i"), num(1))),
            ]
        };
        for (intrinsic, instr) in [("Sqrt#", "sqrt.rn.f32"), ("Fabs#", "abs.f32")] {
            let shape = crate::analysis::accel::KernelShape {
                index_var: "i".into(),
                count_expr: Some(Expr::Decimal(1024)),
                kernel_stmts: body_for(intrinsic),
                host_stmts: vec![],
                read_buffers: vec!["a".into()],
                write_buffers: vec!["out".into()],
                scalar_ins: vec![],
                eligible: true,
                reasons: vec![],
                work_cols: None,
                reduction: None,
                deferred_normalize: None,
            };
            let consts = std::collections::HashMap::new();
            let universe = crate::type_universe::TypeUniverse::new();
            let ptx = emit_general_ptx(
                &shape,
                1024,
                &layout,
                &consts,
                &universe,
                GeneralEmitOpts { int_bits: 32, deferred_split: 0, ..Default::default() },
            )
            .unwrap_or_else(|e| panic!("{intrinsic} emits: {e}"));
            assert!(
                ptx.contains(&format!("    {instr} ")),
                "{intrinsic} must lower to `{instr}`:\n{ptx}"
            );
            assert_ptx_well_formed(&ptx);
        }
    }
}

#[cfg(test)]
mod probe_tmp {
    use super::*;
    #[test]
    fn dump_flash2p_stmts() {
        let src = std::fs::read_to_string("examples/gpu/flash2p_fixture_tmp.abv")
            .expect("fixture");
        let tokens = crate::lexer::tokenize(&src).expect("lex");
        let mut parser = crate::parser::Parser::new(tokens, &src);
        let items = parser.parse_program().expect("parse");
        let universe = crate::type_universe::TypeUniverse::new();
        let info = crate::analysis::accel::ProgramInfo::build(&items);
        for item in &items {
            if let crate::ast::TopLevel::Transaction(t) = item {
                let shape = crate::analysis::accel::prove_kernel_pub(
                    &t.name, &t.body, &t.contract, &info, &universe,
                );
                println!("=== {} eligible={} deferred={} ===", t.name, shape.eligible, detect_deferred_region(&shape.kernel_stmts, &shape.index_var).is_some());
                for r in &shape.reasons { println!("reason: {}", r); }
                for (i, s) in shape.kernel_stmts.iter().enumerate() {
                    println!("[{}] {:#?}", i, s);
                }
            }
        }
    }
}

// ── 2026-09-19 (M1-finish, plan general-machinery): the deferred-softmax
// region — a head-mapped node whose body is [max pass, accumulate pass,
// deferred normalize] over a KV dimension. Lowering = the gate kernel's
// composition of GENERIC mechanisms: warps slice the KV dimension, lanes
// own element strips (coalesced), merges are declared-by-shape operators
// (max for the max pass, + for the accumulation and exp-sum). No
// softmax-specific algebra lives here: every index expression, scale and
// exp/max call is emitted verbatim from the source with d bound to strips
// and sc bound to the butterfly total. ─────────────────────────────────

/// One recognized deferred-softmax region.
struct DeferredRegion {
    /// Index of the first statement (the leading lets) in kernel_stmts.
    start: usize,
    /// Number of statements the region replaces.
    count: usize,
    /// The max-pass j loop index (the region's statements reference the
    /// loop item names for binding).
    body: Vec<Statement>,
}

/// Strip state: a vector buffer whose element references resolve to
/// per-lane registers while a region is being emitted.
struct StripAcc {
    buf: String,
    d_item: String,
    regs: Vec<String>,
}

/// Match the deferred-softmax region: [leading lets (kh/m/l)],
/// max pass (dot + Max# accumulate), exp-sum + vector accumulation pass,
/// deferred normalize. Structural and conservative; anything else → None
/// and the general path keeps the node.
fn detect_deferred_region(
    stmts: &[Statement],
    index_var: &str,
) -> Option<(usize, usize, DeferredRegionParts)> {
    // Statement shapes we track.
    let mut kh: Option<String> = None;
    let mut kh_stmt: Option<Statement> = None;
    let mut m_name: Option<String> = None;
    let mut m_init: Option<Expr> = None;
    let mut l_name: Option<String> = None;
    let mut l_init: Option<Expr> = None;
    let mut loop_a: Option<&Statement> = None;
    let mut loop_b: Option<&Statement> = None;
    let mut loop_c: Option<&Statement> = None;
    let mut region_start = usize::MAX;
    let mut region_end = 0usize;
    for (i, stmt) in stmts.iter().enumerate() {
        match stmt {
            Statement::Let { name, expr: Some(e), .. } => {
                // leading scalar inits: kh (head/G), m (neg float), l (zero)
                if let Expr::BinaryOp(crate::ast::BinaryOpKind::Div, a, _) = e {
                    if matches!(a.as_ref(), Expr::Identifier(n) if n == index_var) {
                        kh = Some(name.clone());
                        kh_stmt = Some(stmt.clone());
                        region_start = region_start.min(i);
                        region_end = i + 1;
                        continue;
                    }
                }
                if let Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, x) = e {
                    if matches!(x.as_ref(), Expr::Float(_)) && m_name.is_none() {
                        m_name = Some(name.clone());
                        m_init = Some((**x).clone());
                        region_start = region_start.min(i);
                        region_end = i + 1;
                        continue;
                    }
                }
                // 2026-09-21 (comptime fold): the fold may splice the init
                // as a direct negative literal — same value, same meaning
                // (m_init always carries the positive magnitude; the
                // emitter negates it).
                if let Expr::Float(f) = e {
                    if *f < 0.0 && m_name.is_none() {
                        m_name = Some(name.clone());
                        m_init = Some(Expr::Float(-f));
                        region_start = region_start.min(i);
                        region_end = i + 1;
                        continue;
                    }
                }
                if matches!(e, Expr::Decimal(0) | Expr::Float(0.0)) && l_name.is_none() && m_name.is_some() {
                    l_name = Some(name.clone());
                    l_init = Some(e.clone());
                    region_start = region_start.min(i);
                    region_end = i + 1;
                    continue;
                }
            }
            Statement::Foreach { .. } => {
                // loop A: before l's init; loop B: after; loop C: last (d-ranged)
                if l_name.is_none() && loop_a.is_none() {
                    loop_a = Some(stmt);
                    region_end = i + 1;
                } else if l_name.is_some() && loop_b.is_none() {
                    loop_b = Some(stmt);
                    region_end = i + 1;
                } else {
                    loop_c = Some(stmt);
                    region_end = i + 1;
                }
            }
            _ => {}
        }
    }
    let (Some(la), Some(lb), Some(lc)) = (loop_a, loop_b, loop_c) else {
        return None;
    }
    ;
    let _ = kh;
    let Statement::Foreach { item: ja, list: la_range, body: la_body } = la else { return None };
    let j_name = ja.clone();
    let Expr::Range { end: la_end, .. } = la_range.as_ref() else { return None };
    let Statement::Foreach { item: jb, list: lb_range, body: lb_body } = lb else { return None };
    let Expr::Range { end: lb_end, .. } = lb_range.as_ref() else { return None };
    if format!("{:?}", la_end) != format!("{:?}", lb_end) || jb != &j_name {
        return None;
    }
    let Statement::Foreach { item: dc, list: lc_range, body: lc_body } = lc else { return None };
    let Expr::Range { end: lc_end, .. } = lc_range.as_ref() else { return None };

    let m_name = m_name?;
    let l_name = l_name.clone()?;
    let l_init = l_init?;

    // Loop A: [Let sc = 0, Foreach d { sc = sc + a*b }, m = Max#(m, expr(sc))]
    let la_stmts = la_body;
    if la_stmts.len() != 3 {
        return None;
    }
    let (sc_a, dot_a) = match &la_stmts[0] {
        Statement::Let { name, expr: Some(Expr::Decimal(0)), .. } => (name.clone(), &la_stmts[1]),
        _ => return None,
    };
    let Statement::Foreach { item: da, list: da_range, body: da_body } = dot_a else { return None };
    let Expr::Range { end: da_end, .. } = da_range.as_ref() else { return None };
    if da_body.len() != 1 {
        return None;
    }
    if !dot_shape(&da_body[0], &sc_a, &da) {
        return None;
    }
    let (q_buf, k_buf) = dot_buffers(&da_body[0], da);
    let Statement::Assign(max_lhs, max_rhs) = &la_stmts[2] else { return None };
    let Expr::Identifier(m_target) = max_lhs else { return None };
    if m_target != &m_name {
        return None;
    }
    let Expr::Call(max_fn, max_args, _) = max_rhs else { return None };
    if max_fn != "Max#" || max_args.len() != 2 {
        return None;
    }
    let score_a = if matches!(&max_args[0], Expr::Identifier(n) if n == &m_name) {
        &max_args[1]
    } else if matches!(&max_args[1], Expr::Identifier(n) if n == &m_name) {
        &max_args[0]
    } else {
        return None;
    };

    // Loop B: [Let sc2 = 0, Foreach d {dot}, Let p = Exp#(...sc2...),
    //          l = l + p, Foreach d { acc[..d..] = acc[..d..] + p * v }]
    let lb_stmts = lb_body;
    if lb_stmts.len() != 5 {
        return None;
    }
    let (sc_b, dot_b) = match &lb_stmts[0] {
        Statement::Let { name, expr: Some(Expr::Decimal(0)), .. } => (name.clone(), &lb_stmts[1]),
        _ => return None,
    };
    let Statement::Foreach { item: db, list: db_range, body: db_body } = dot_b else { return None };
    let Expr::Range { end: db_end, .. } = db_range.as_ref() else { return None };
    if format!("{:?}", db_end) != format!("{:?}", da_end) || db_body.len() != 1 {
        return None;
    }
    if !dot_shape(&db_body[0], &sc_b, &db) {
        return None;
    }
    let Statement::Let { name: p_name, expr: Some(p_expr), .. } = &lb_stmts[2] else { return None };
    if !format!("{:?}", p_expr).contains(&format!("\"{}\"", sc_b)) {
        return None;
    }
    let Statement::Assign(l_lhs, l_rhs) = &lb_stmts[3] else { return None };
    let Expr::Identifier(l_t) = l_lhs else { return None };
    if l_t != &l_name {
        return None;
    }
    if !format!("{:?}", l_rhs).contains(&format!("\"{}\"", p_name)) {
        return None;
    }
    let Statement::Foreach { item: dc2, list: dc2_range, body: dc2_body } = &lb_stmts[4] else { return None };
    if format!("{:?}", dc2_range) != format!("{:?}", lc_range) || dc2_body.is_empty() {
        return None;
    }
    // every acc statement: element-wise RMW on the same buffer, d-affine
    let mut acc_buf = String::new();
    for s in dc2_body {
        let Statement::Assign(lhs_a, rhs) = s else { return None };
        let Expr::Index(b1, i1) = lhs_a else { return None };
        let Expr::Identifier(bname) = b1.as_ref() else { return None };
        if !format!("{:?}", i1).contains(&format!("\"{}\"", dc2)) {
            return None;
        }
        if acc_buf.is_empty() {
            acc_buf = bname.clone();
        } else if &acc_buf != bname {
            return None;
        }
        if !format!("{:?}", rhs).contains(&format!("\"{}\"", acc_buf)) {
            return None;
        }
        if !format!("{:?}", rhs).contains(&format!("\"{}\"", p_name)) {
            return None;
        }
    }

    // Loop C: [Foreach d { out[..d..] = acc[..d..] / l }]
    let lc_stmts = lc_body;
    if lc_stmts.len() != 1 {
        return None;
    }
    let Statement::Assign(lhs_c, out_val) = &lc_stmts[0] else { return None };
    let Expr::Index(cbuf, idx_c) = lhs_c else { return None };
    let Expr::Identifier(out_buf) = cbuf.as_ref() else { return None };
    let out_buf = out_buf.clone();
    let Expr::BinaryOp(crate::ast::BinaryOpKind::Div, num, den) = out_val else { return None };
    let Expr::Index(nbuf, _) = num.as_ref() else { return None };
    let Expr::Identifier(num_buf) = nbuf.as_ref() else { return None };
    if num_buf != &acc_buf {
        return None;
    }
    let Expr::Identifier(den_name) = den.as_ref() else { return None };
    if den_name != &l_name {
        return None;
    }

    Some((
        region_start,
        region_end,
        DeferredRegionParts {
            kh_stmt: kh_stmt.clone(),
            m_init: m_init?,
            l_init: l_init.clone(),
            q_buf: q_buf.clone(),
            k_buf: k_buf.clone(),
            v_buf: acc_v_buf(lb_body, &acc_buf),
            acc_buf,
            out_buf: out_buf.clone(),
            score_a: score_a.clone(),
            p_expr: p_expr.clone(),
            l_rhs: l_rhs.clone(),
            dot_a_body: da_body.clone(),
            dot_b_body: db_body.clone(),
            acc_stmts: dc2_body.clone(),
            norm_stmt: lc_stmts[0].clone(),
            max_stmt: la_stmts[2].clone(),
            p_name: p_name.clone(),
            sc_a,
            sc_b,
            da: da.clone(),
            db: db.clone(),
            dc2: dc2.clone(),
            dc: dc.clone(),
            la_end: (**la_end).clone(),
            lc_end: (**lc_end).clone(),
            m_name,
            l_name,
            j_name,
        },
    ))
}

use crate::ast::UnaryOpKind;

/// Written-array names in a statement list (Assign targets that are
/// Index expressions — the bases the loop mutates; their reads must
/// never hoist).
fn collect_written_arrays(stmts: &[Statement], out: &mut std::collections::HashSet<String>) {
    for s in stmts {
        match s {
            Statement::Assign(lhs, _) => {
                if let Expr::Index(base, _) = lhs {
                    if let Expr::Identifier(n) = base.as_ref() {
                        out.insert(n.clone());
                    }
                }
            }
            Statement::Let { expr: Some(e), .. } | Statement::Expression(e) | Statement::Term(Some(e)) | Statement::Gate(e) => {
                collect_written_expr_arrays(e, out);
            }
            Statement::Guarded(_, body) | Statement::Block(body) => {
                collect_written_arrays(body, out);
            }
            Statement::Foreach { body, .. } => collect_written_arrays(body, out),
            _ => {}
        }
    }
}

fn collect_written_expr_arrays(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match e {
        Expr::BinaryOp(_, a, b) | Expr::Index(a, b) => {
            collect_written_expr_arrays(a, out);
            collect_written_expr_arrays(b, out);
        }
        Expr::UnaryOp(_, a) => collect_written_expr_arrays(a, out),
        Expr::Call(_, args, _) => {
            for a in args {
                collect_written_expr_arrays(a, out);
            }
        }
        _ => {}
    }
}

/// Does the expression reference `name` at all (the j-binder test)?
fn expr_references(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Identifier(n) => n == name,
        Expr::BinaryOp(_, a, b) | Expr::Index(a, b) => {
            expr_references(a, name) || expr_references(b, name)
        }
        Expr::UnaryOp(_, a) => expr_references(a, name),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_references(a, name)),
        _ => false,
    }
}

fn gather_j_invariant_reads(stmts: &[Statement], j: &str, written: &std::collections::HashSet<String>, reads: &mut Vec<Expr>) {
    fn walk(e: &Expr, j: &str, written: &std::collections::HashSet<String>, reads: &mut Vec<Expr>) {
        if let Expr::Index(base, idx) = e {
            if let Expr::Identifier(bn) = base.as_ref() {
                if !written.contains(bn) && !expr_references(idx, j) {
                    reads.push(e.clone());
                }
            }
        }
        match e {
            Expr::BinaryOp(_, a, b) | Expr::Index(a, b) => {
                walk(a, j, written, reads);
                walk(b, j, written, reads);
            }
            Expr::UnaryOp(_, a) => walk(a, j, written, reads),
            Expr::Call(_, args, _) => {
                for a in args {
                    walk(a, j, written, reads);
                }
            }
            _ => {}
        }
    }
    fn ws(stmts: &[Statement], j: &str, written: &std::collections::HashSet<String>, reads: &mut Vec<Expr>) {
        for s in stmts {
            match s {
                Statement::Assign(_, rhs) => walk(rhs, j, written, reads),
                Statement::Let { expr: Some(e), .. } | Statement::Expression(e) | Statement::Gate(e) => walk(e, j, written, reads),
                Statement::Term(Some(e)) => walk(e, j, written, reads),
                Statement::Guarded(_, body) | Statement::Block(body) => ws(body, j, written, reads),
                Statement::Foreach { list, body, .. } => {
                    walk(list, j, written, reads);
                    ws(body, j, written, reads);
                }
                _ => {}
            }
        }
    }
    ws(stmts, j, written, reads);
}

fn rewrite_j_expr(e: &mut Expr, pairs: &[(String, Expr)]) -> bool {
    for (name, key) in pairs {
        if *e == *key {
            *e = Expr::Identifier(name.clone());
            return true;
        }
    }
    match e {
        Expr::BinaryOp(_, a, b) => rewrite_j_expr(a, pairs) | rewrite_j_expr(b, pairs),
        Expr::Index(a, b) => rewrite_j_expr(a, pairs) | rewrite_j_expr(b, pairs),
        Expr::UnaryOp(_, a) => rewrite_j_expr(a, pairs),
        Expr::Call(_, args, _) => {
            let mut any = false;
            for a in args.iter_mut() {
                any |= rewrite_j_expr(a, pairs);
            }
            any
        }
        _ => false,
    }
}

fn rewrite_j_stmts(stmts: &mut [Statement], pairs: &[(String, Expr)]) {
    for s in stmts {
        match s {
            Statement::Assign(lhs, rhs) => {
                rewrite_j_expr(lhs, pairs);
                rewrite_j_expr(rhs, pairs);
            }
            Statement::Let { expr: Some(e), .. } | Statement::Expression(e) | Statement::Gate(e) | Statement::Term(Some(e)) => {
                rewrite_j_expr(e, pairs);
            }
            Statement::Guarded(_, body) | Statement::Block(body) => rewrite_j_stmts(body, pairs),
            Statement::Foreach { list, body, .. } => {
                rewrite_j_expr(list, pairs);
                rewrite_j_stmts(body, pairs);
            }
            _ => {}
        }
    }
}

/// 2026-10-01 (5a lever 2, E2/E3-sized): collect the j-INVARIANT array
/// reads from the two pass bodies — `Index` reads whose index never
/// references the j binder and whose base is never written in the loop —
/// deduped structurally, capped at 32. Returns the (name, expr) pairs
/// AND rewrites both bodies to read the hoisted names (the caller emits
/// one immutable `let` per pair per strip; immutable lets resolve to
/// REGISTERS — last_val_temps — so the loop body reads registers, not
/// global memory).
fn collect_j_invariant_reads(
    dot_a: &[Statement],
    dot_b: &[Statement],
    parts: &DeferredRegionParts,
) -> Vec<(usize, Expr)> {
    let mut written = std::collections::HashSet::new();
    collect_written_arrays(dot_a, &mut written);
    collect_written_arrays(dot_b, &mut written);
    let mut all: Vec<Expr> = Vec::new();
    gather_j_invariant_reads(dot_a, &parts.j_name, &written, &mut all);
    gather_j_invariant_reads(dot_b, &parts.j_name, &written, &mut all);
    let mut uniq: Vec<Expr> = Vec::new();
    for e in all {
        if !uniq.contains(&e) {
            uniq.push(e.clone());
        }
        if uniq.len() >= 32 {
            break;
        }
    }
    uniq.into_iter().enumerate().collect()
}

struct DeferredRegionParts {
    j_name: String,
    kh_stmt: Option<Statement>,
    m_init: Expr,
    l_init: Expr,
    q_buf: String,
    k_buf: String,
    v_buf: String,
    acc_buf: String,
    out_buf: String,
    score_a: Expr,
    p_expr: Expr,
    l_rhs: Expr,
    dot_a_body: Vec<Statement>,
    dot_b_body: Vec<Statement>,
    acc_stmts: Vec<Statement>,
    norm_stmt: Statement,
    max_stmt: Statement,
    p_name: String,
    sc_a: String,
    sc_b: String,
    da: String,
    db: String,
    dc2: String,
    dc: String,
    la_end: Expr,
    lc_end: Expr,
    m_name: String,
    l_name: String,
}

/// 2026-09-30 (general reduction-split): per-CTA context for
/// [`Gen::emit_deferred_partial_store`].
struct PartialStore<'a> {
    parts: &'a DeferredRegionParts,
    a_regs: &'a [String],
    r_lane: &'a str,
    m_reg: &'a str,
    l_tot: &'a str,
}

/// The dot statement shape: `sc = sc + a * b` (either mul order), where at
/// least one side is an index expression mentioning the loop item.
fn dot_shape(stmt: &Statement, sc: &str, d_item: &str) -> bool {
    let Statement::Assign(Expr::Identifier(lhs), rhs) = stmt else { return false };
    if lhs != sc {
        return false;
    }
    let Expr::BinaryOp(crate::ast::BinaryOpKind::Add, a, b) = rhs else { return false };
    let is_self = |e: &Expr| matches!(e, Expr::Identifier(n) if n == sc);
    let mul = if is_self(a) { b.as_ref() } else if is_self(b) { a.as_ref() } else { return false };
    let Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) = mul else { return false };
    let mentions_d = |e: &Expr| {
        let mut found = false;
        fn walk(e: &Expr, name: &str, found: &mut bool) {
            match e {
                Expr::Identifier(n) => { if n == name { *found = true; } }
                Expr::BinaryOp(_, l, r) => { walk(l, name, found); walk(r, name, found); }
                Expr::Index(_, i) => walk(i, name, found),
                Expr::Cast(x, _) => walk(x, name, found),
                _ => {}
            }
        }
        walk(l, d_item, &mut found);
        walk(r, d_item, &mut found);
        found
    };
    mentions_d(l) && mentions_d(r)
}

/// The two d-affine load buffers of a dot statement (Cast-transparent).
fn dot_buffers(stmt: &Statement, _d_item: &str) -> (String, String) {
    let mut bufs = Vec::new();
    if let Statement::Assign(_, rhs) = stmt {
        fn walk(e: &Expr, bufs: &mut Vec<String>) {
            match e {
                Expr::Index(b, _) => { if let Expr::Identifier(bn) = b.as_ref() { bufs.push(bn.clone()); } }
                Expr::Cast(x, _) => walk(x, bufs),
                Expr::BinaryOp(_, l, r) => { walk(l, bufs); walk(r, bufs); }
                _ => {}
            }
        }
        walk(rhs, &mut bufs);
    }
    let a = bufs.first().cloned().unwrap_or_default();
    let b = bufs.get(1).cloned().unwrap_or_default();
    (a, b)
}

/// The V buffer of the accumulation pass: the load inside the acc statements
/// that is not the accumulator itself.
fn acc_v_buf(lb_body: &[Statement], acc_buf: &str) -> String {
    let Some(Statement::Foreach { body, .. }) = lb_body.iter().nth(4) else { return String::new() };
    let mut found = String::new();
    if let Some(Statement::Assign(_, rhs)) = body.first() {
        fn walk(e: &Expr, acc: &str, found: &mut String) {
            match e {
                Expr::Index(b, _) => { if let Expr::Identifier(bn) = b.as_ref() { if bn != acc { *found = bn.clone(); } } }
                Expr::Cast(x, _) => walk(x, acc, found),
                Expr::BinaryOp(_, l, r) => { walk(l, acc, found); walk(r, acc, found); }
                _ => {}
            }
        }
        walk(rhs, acc_buf, &mut found);
    }
    found
}

fn r_tmp_name() -> &'static str {
    "%r2"
}

fn f32_imm(v: f32) -> String {
    format!("0f{:08X}", v.to_bits())
}

/// 2026-10-01 (split device-validation): every `.reg` declaration in the
/// PTX, as `%name` tokens.
#[cfg(test)]
fn collect_declared_regs(ptx: &str) -> std::collections::HashSet<String> {
    ptx.lines()
        .filter(|line| line.trim_start().starts_with(".reg"))
        .flat_map(|line| line.split(','))
        .flat_map(|part| part.split_whitespace())
        .filter_map(|tok| tok.strip_prefix('%'))
        .map(|name| name.trim_matches(|c: char| c == ';' || c == ','))
        .filter(|name| !name.is_empty())
        .map(|name| format!("%{name}"))
        .collect()
}

/// 2026-10-01: from `lb[start..]`, the end of the alphabetic run and the
/// end of the full `%alpha<digits>` token (`%r3` → (alpha_end, tok_end)).
/// Iterator positions, not nested scans — a byte-pair walk is enough.
#[cfg(test)]
fn reg_token_parts(lb: &[u8], start: usize) -> (usize, usize) {
    let alpha_end = lb[start..]
        .iter()
        .position(|b| !b.is_ascii_alphabetic())
        .map_or(lb.len(), |p| start + p);
    let tok_end = lb[alpha_end..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .map_or(lb.len(), |p| alpha_end + p);
    (alpha_end, tok_end)
}

/// 2026-10-01: the numeric register operand at a `%` position — `%rN`/
/// `%rdN`/`%fN`/`%pN` with at least one digit. Declarations on `.reg`
/// lines are filtered by the caller.
#[cfg(test)]
fn numeric_reg_at(line: &str, pct: usize) -> Option<&str> {
    let (alpha_end, tok_end) = reg_token_parts(line.as_bytes(), pct + 1);
    let alpha = &line[pct + 1..alpha_end];
    if tok_end > alpha_end && matches!(alpha, "r" | "rd" | "f" | "p") {
        return Some(&line[pct..tok_end]);
    }
    None
}

/// 2026-10-01: register uses that no `.reg` line declares — the CUDA JIT
/// rejects the module for a single one.
#[cfg(test)]
fn undeclared_regs(ptx: &str, declared: &std::collections::HashSet<String>) -> Vec<String> {
    ptx.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with(".reg"))
        .flat_map(|(li, line)| line.match_indices('%').map(move |(pct, _)| (li, line, pct)))
        .filter_map(|(li, line, pct)| numeric_reg_at(line, pct).map(|reg| (li, reg)))
        .filter(|(_, reg)| !declared.contains(*reg))
        .map(|(li, reg)| format!("line {}: {reg}", li + 1))
        .collect()
}

/// 2026-10-01: end of the hexdigit run starting at `start`.
#[cfg(test)]
fn hex_run_end(lb: &[u8], start: usize) -> usize {
    lb[start..]
        .iter()
        .position(|b| !b.is_ascii_hexdigit())
        .map_or(lb.len(), |p| start + p)
}

/// 2026-10-01: byte positions where a `0f`/`0F` float immediate starts
/// (token boundary: start of line or space/comma/equals/bracket/tab
/// before the `0`).
#[cfg(test)]
fn imm0f_starts(lb: &[u8]) -> Vec<usize> {
    lb.windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] == b'0' && (w[1] == b'f' || w[1] == b'F'))
        .filter(|(i, _)| *i == 0 || matches!(lb[i - 1], b' ' | b',' | b'=' | b'[' | b'\t'))
        .map(|(i, _)| i)
        .collect()
}

/// 2026-10-01: `0f`/`0F` float immediates whose hex run is not exactly 8
/// digits (a seven-digit run parses as garbage — ptxas fatals near it).
/// The run stops at the token end by construction, so only the length is
/// in question.
#[cfg(test)]
fn malformed_float_immediates(ptx: &str) -> Vec<String> {
    ptx.lines()
        .enumerate()
        .flat_map(|(li, line)| imm0f_starts(line.as_bytes()).into_iter().map(move |i| (li, line, i)))
        .filter_map(|(li, line, i)| {
            let end = hex_run_end(line.as_bytes(), i + 2);
            if end <= i + 2 || end - (i + 2) == 8 {
                return None;
            }
            Some(format!("line {}: '0f{}'", li + 1, &line[i + 2..end]))
        })
        .collect()
}

/// 2026-10-01 (split device-validation): static PTX well-formedness —
/// every numeric register operand (`%rN`/`%rdN`/`%fN`/`%pN`) is declared,
/// and every `0f` immediate carries exactly 8 hex digits. These are the
/// two defect classes `ptxas`/the CUDA JIT reject (the split's undeclared
/// `%r3` slice register; a one-nibble-short `-inf` literal in the combine
/// kernel) — both shipped because shape-string tests cannot see them.
/// Toolkit-free so it runs anywhere; the on-device gate (softmax_gate.sh,
/// both lanes) remains the semantic authority.
#[cfg(test)]
pub(crate) fn assert_ptx_well_formed(ptx: &str) {
    let declared = collect_declared_regs(ptx);
    let undeclared = undeclared_regs(ptx, &declared);
    let bad_imm = malformed_float_immediates(ptx);
    assert!(
        undeclared.is_empty(),
        "PTX uses undeclared registers: {:?}",
        &undeclared[..undeclared.len().min(8)]
    );
    assert!(
        bad_imm.is_empty(),
        "PTX has malformed float immediates: {:?}",
        &bad_imm[..bad_imm.len().min(8)]
    );
}


fn fold_f32(
    e: &Expr,
    consts: &std::collections::HashMap<String, Expr>,
) -> Option<f32> {
    match e {
        Expr::Float(f) => Some(*f as f32),
        Expr::Decimal(n) => Some(*n as f32),
        Expr::UnaryOp(crate::ast::UnaryOpKind::Neg, x) => Some(-fold_f32(x, consts)?),
        Expr::Identifier(n) => match consts.get(n) {
            Some(v) => fold_f32(v, consts),
            None => None,
        },
        _ => None,
    }
}

impl Gen<'_> {
    /// 2026-09-19 (M1-finish, plan general-machinery): lower the deferred-
    /// softmax region as the gate-proven composition — 32 warps slice the
    /// KV dimension (exact slices, KV % 32 == 0), lanes own 4-element
    /// strips of the head dim (coalesced loads, per-lane partials),
    /// merges are the bodies' own generic operators (max for the max
    /// pass, + for the accumulation and the exp-sum), and the dot is
    /// butterfly-reduced per KV position. Every index expression, scale
    /// and call is emitted verbatim from the source with d bound to strip
    /// registers and sc bound to the butterfly total.
    /// 2026-09-30 (general reduction-split): decode `slice = ctaid % S` and
    /// `h = ctaid / S` into `%r1` for a split block-per-work-item kernel;
    /// no-op when the split is off.
    fn decode_split_ctaid(&mut self, decl: &mut String, body: &mut String) {
        if self.deferred_split <= 1 {
            return;
        }
        let slice_reg = self.fresh_r();
        // 2026-10-01 (split device-validation): every fresh register must
        // land in the declaration block — the slice register was allocated
        // here but never declared, and the CUDA JIT rejected the whole
        // partial kernel for the undeclared `%r3` (the device gate found
        // it; shape-string tests could not).
        decl.push_str(&format!("    .reg .u32 {};\n", slice_reg));
        body.push_str(&format!(
            "    rem.u32 {}, %r1, {};\n",
            slice_reg, self.deferred_split
        ));
        body.push_str(&format!("    div.u32 %r1, %r1, {};\n", self.deferred_split));
        self.split_slice_reg = Some(slice_reg);
    }

    /// 2026-09-30 (general reduction-split): write the per-slice partial
    /// `(m, l, acc[dim])` for one work item into the accumulator buffer.
    /// Layout: `partial[h*S + slice] = {m, l, acc[0..dim]}` (`dim+2` floats),
    /// read by [`emit_deferred_combine_ptx`]. `m`/`l` are CTA-uniform (lane 0
    /// stores them); the `acc` strips are per-lane.
    fn emit_deferred_partial_store(
        &mut self,
        s: &PartialStore<'_>,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        let split = self.deferred_split;
        let dim = self.const_int(&s.parts.lc_end)?;
        let acc_off = self.field_off(&s.parts.acc_buf).ok_or_else(|| {
            format!(
                "ptx general: split partial buffer '{}' not in layout",
                s.parts.acc_buf
            )
        })?;
        let slice = self
            .split_slice_reg
            .clone()
            .ok_or("ptx general: split slice register missing")?;
        let h = self.gid.to_string();
        let rt = self.fresh_r();
        let rda = self.fresh_rd();
        let rdb = self.fresh_rd();
        let p1 = self.fresh_p();
        decl.push_str(&format!("    .reg .u32 {};\n", rt));
        decl.push_str(&format!("    .reg .b64 {}, {};\n", rda, rdb));
        decl.push_str(&format!("    .reg .pred {};\n", p1));
        let stride = 2 + dim;
        body.push_str(&format!("    mul.lo.u32 {}, {}, {};\n", rt, h, split));
        body.push_str(&format!("    add.u32 {}, {}, {};\n", rt, rt, slice));
        body.push_str(&format!("    mul.lo.u32 {}, {}, {};\n", rt, rt, stride));
        body.push_str(&format!("    mul.wide.u32 {}, {}, 4;\n", rda, rt));
        body.push_str(&format!("    mov.u64 {}, {};\n", rdb, acc_off));
        body.push_str(&format!("    add.u64 {}, %rd1, {};\n", rdb, rdb));
        body.push_str(&format!("    add.u64 {}, {}, {};\n", rdb, rdb, rda));
        // m, l — CTA-uniform, lane 0 writes.
        body.push_str(&format!("    setp.eq.u32 {}, {}, 0;\n", p1, s.r_lane));
        body.push_str(&format!(
            "    @{} st.global.f32 [{}+0], {};\n",
            p1, rdb, s.m_reg
        ));
        body.push_str(&format!(
            "    @{} st.global.f32 [{}+4], {};\n",
            p1, rdb, s.l_tot
        ));
        // acc strips — per-lane d.
        body.push_str(&format!("    mul.wide.u32 {}, {}, 4;\n", rda, s.r_lane));
        body.push_str(&format!("    add.u64 {}, {}, {};\n", rda, rdb, rda));
        body.push_str(&format!("    add.u64 {}, {}, 8;\n", rda, rda));
        for (i, a) in s.a_regs.iter().enumerate() {
            body.push_str(&format!("    st.global.f32 [{}+{}], {};\n", rda, 128 * i, a));
        }
        Ok(())
    }

    fn emit_deferred_region(
        &mut self,
        parts: &DeferredRegionParts,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        let kv = self.const_int(&parts.la_end)?;
        let dim = self.const_int(&parts.lc_end)?;
        if kv % 32 != 0 || dim % 32 != 0 || kv < 32 || dim < 32 {
            return Err(format!(
                "ptx general: deferred region needs KV and D divisible by 32 (got KV {kv}, D {dim})"
            ));
        }
        let strips = (dim / 32) as usize;
        // 2026-09-30 (general reduction-split): with S > 1 each CTA reduces a
        // KV/S sub-span (further split across the 32 warps) and writes
        // per-slice partials; the combine kernel merges them.
        let split = self.deferred_split;
        if split > 1 && (kv % split as i64 != 0 || (kv / split as i64) % 32 != 0) {
            return Err(format!(
                "ptx general: deferred split {split} needs KV {kv} divisible by 32*S"
            ));
        }
        let per_slice = if split > 1 { kv / split as i64 } else { kv };
        let jslice = (per_slice / 32) as u32;
        let warps = 32u32;

        // leading let: kh through the normal path (h is bound to ctaid).
        if let Some(kh_stmt) = &parts.kh_stmt {
            self.emit_stmt(kh_stmt, decl, body)?;
        }

        // warp/lane/slice registers.
        let r_tid = self.fresh_r();
        let r_warp = self.fresh_r();
        let r_lane = self.fresh_r();
        let r_cnt = self.fresh_r();
        let r_lo = self.fresh_r();
        let r_hi = self.fresh_r();
        let r_w = self.fresh_r();
        // 2026-09-30 (split): the slice's base `j` offset = slice * per_slice.
        let sbase = if split > 1 {
            let sb = self.fresh_r();
            let slice = self
                .split_slice_reg
                .clone()
                .ok_or("ptx general: split slice register missing")?;
            decl.push_str(&format!("    .reg .u32 {};\n", sb));
            body.push_str(&format!(
                "    mul.lo.u32 {}, {}, {};\n",
                sb, slice, per_slice
            ));
            Some(sb)
        } else {
            None
        };
        let pred = self.fresh_p();
        let f_t = self.fresh_f();
        let sc_lane = self.fresh_f();
        let sc_w = self.fresh_f();
        let p_reg = self.fresh_f();
        let m_reg = self.fresh_f();
        let l_reg = self.fresh_f();
        let l_w = self.fresh_f();
        let l_tot = self.fresh_f();
        for r in [&r_tid, &r_warp, &r_lane, &r_cnt, &r_lo, &r_hi, &r_w] {
            decl.push_str(&format!("    .reg .u32 {};\n", r));
        }
        decl.push_str(&format!("    .reg .pred {};\n", pred));
        for f in [&f_t, &sc_lane, &sc_w, &p_reg, &m_reg, &l_reg, &l_w, &l_tot] {
            decl.push_str(&format!("    .reg .f32 {};\n", f));
        }
        // shared: per-warp max (32), per-warp l (32), per-warp acc rows.
        decl.push_str("    .shared .align 4 .b8 redm[128];\n");
        decl.push_str("    .shared .align 4 .b8 redl[128];\n");
        decl.push_str("    .shared .align 4 .b8 smacc[16384];\n");

        // lane/warp ids + d strip regs (loop-invariant).
        body.push_str(&format!("    mov.u32 {}, %tid.x;\n", r_tid));
        body.push_str(&format!("    and.b32 {}, {}, 31;\n", r_lane, r_tid));
        body.push_str(&format!("    shr.u32 {}, {}, 5;\n", r_warp, r_tid));
        let mut d_regs = Vec::new();
        for i in 0..strips {
            let d = self.fresh_r();
            decl.push_str(&format!("    .reg .u32 {};\n", d));
            if i == 0 {
                body.push_str(&format!("    mov.u32 {}, {};\n", d, r_lane));
            } else {
                body.push_str(&format!("    add.u32 {}, {}, {};\n", d, d_regs[0], 32 * i));
            }
            d_regs.push(d);
        }
        // state init: m from the author's constant, everything else zero.
        // 2026-09-20 (NaN fix): the detector extracts Float(1e30) from
        // Neg(Float(1e30)), stripping the negation.  Negate here.
        let m_init = fold_f32(&parts.m_init, self.consts)
            .ok_or_else(|| "ptx general: deferred region max init is not a constant".to_string())?;
        body.push_str(&format!("    mov.f32 {}, {};\n", m_reg, f32_imm(-m_init)));
        body.push_str(&format!("    mov.f32 {}, 0f00000000;\n", l_reg));
        let mut a_regs = Vec::new();
        for _ in 0..strips {
            let a = self.fresh_f();
            decl.push_str(&format!("    .reg .f32 {};\n", a));
            body.push_str(&format!("    mov.f32 {}, 0f00000000;\n", a));
            a_regs.push(a);
        }

        // 2026-10-01 (5a lever 2, E2/E3-sized): hoist j-INVARIANT array
        // reads out of the two j passes. The dot bodies re-load q
        // (h·D+d — independent of the j counter) EVERY iteration; ptxas
        // cannot hoist those loads (the state pointer's aliasing), and
        // at 12 loads/j they are a third of the loop's memory traffic.
        // Immutable lets resolve to REGISTERS (last_val_temps — the
        // alloca path only takes mutated bindings), so a hoisted
        // `let __dqh<N>: Float = q[..];` read per iteration is a
        // register read. The hoistable set: Index reads whose index does
        // not reference the j binder, whose base is never WRITTEN in
        // either pass body, deduped structurally, capped at 32.
        // The hoist is PER-STRIP: the d binder resolves differently in
        // each strip, so strip i's reads hoist to names __dqh{i}_{j}.
        // ONE shared name would collapse the strips (the last emitted
        // let wins in last_val_temps — every strip would read the last
        // strip's register: the CUDA-lane FAIL this fixed).
        let hoisted = collect_j_invariant_reads(&parts.dot_a_body, &parts.dot_b_body, parts);
        let strip_name = |strip: usize, idx: usize| format!("__dqh{}_{}", strip, idx);
        if !hoisted.is_empty() {
            let saved_j0 = self.regs.insert(parts.j_name.clone(), r_cnt.clone());
            for i in 0..strips {
                self.regs.insert(parts.da.clone(), d_regs[i].clone());
                for (jdx, (_, expr)) in hoisted.iter().enumerate() {
                    let stmt = Statement::Let {
                        name: strip_name(i, jdx),
                        names: vec![],
                        ty: Some(crate::ast::Type::float()),
                        expr: Some(expr.clone()),
                        modifiers: vec![],
                    };
                    self.emit_stmt(&stmt, decl, body)?;
                }
            }
            match saved_j0 {
                Some(v) => { self.regs.insert(parts.j_name.clone(), v); }
                None => { self.regs.remove(&parts.j_name); }
            }
        }

        let strip_pairs: Vec<(String, Expr)> = (0..strips)
            .flat_map(|i| {
                hoisted.iter().enumerate().map(move |(jdx, (_, e))| (strip_name(i, jdx), e.clone()))
            })
            .collect();

        // ── pass A: row max (dot per j, butterfly, per-warp max) ──
        // 2026-10-01 (5a probe): the skip_pass diagnostic — the pass
        // emits nothing; m/l stay at their inits (WRONG numerics, the
        // timing shape is the question).
        let lab = self.label;
        let saved_j = self.regs.insert(parts.j_name.clone(), r_cnt.clone());
        self.label += 3;
        if self.skip_pass != 1 {
        body.push_str(&format!("    mov.u32 {}, 0;\n", r_lo));
        body.push_str(&format!(
            "    mad.lo.u32 {}, {}, {}, {};\n",
            r_lo, r_warp, jslice, r_lo
        ));
        if let Some(sb) = &sbase {
            body.push_str(&format!("    add.u32 {}, {}, {};\n", r_lo, r_lo, sb));
        }
        body.push_str(&format!("    add.u32 {}, {}, {};\n", r_hi, r_lo, jslice));
        body.push_str(&format!("    mov.u32 {}, {};\n", r_cnt, r_lo));

        let h1h = format!("L{}_a", lab);
        let h1e = format!("L{}_ax", lab);
        let m_loop = format!("L{}_m", lab);
        let m_done = format!("L{}_md", lab);
        body.push_str(&format!("{}:\n", h1h));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, r_cnt, r_hi));
        body.push_str(&format!("    @{} bra {};\n", pred, h1e));
        body.push_str(&format!("    mov.f32 {}, 0f00000000;\n", sc_lane));
        // 2026-09-20 (M1-finish NaN fix): emit the dot body directly,
        // NOT the Foreach — emit_stmt on a Foreach overwrites the d binding
        // with its own loop counter, producing a full-D serial loop instead
        // of the inlined strip computation.
        let saved_d = self.regs.insert(parts.da.clone(), d_regs[0].clone());
        for i in 0..strips {
            self.active_strip = i;
            self.regs.insert(parts.da.clone(), d_regs[i].clone());
            self.regs.insert(parts.sc_a.clone(), sc_lane.clone());
            let mut body_i = parts.dot_a_body.clone();
            let pairs_i: Vec<(String, Expr)> = hoisted.iter().enumerate()
                .map(|(jdx, (_, e))| (strip_name(i, jdx), e.clone()))
                .collect();
            rewrite_j_stmts(&mut body_i, &pairs_i);
            for s in &body_i {
                self.emit_stmt(s, decl, body)?;
            }
        }
        match saved_d {
            Some(v) => { self.regs.insert(parts.da.clone(), v); }
            None => { self.regs.remove(&parts.da); }
        }
        self.emit_butterfly_add(sc_lane.clone(), sc_w.clone(), decl, body)?;
        let saved_sc = self.regs.insert(parts.sc_a.clone(), sc_w.clone());
        let saved_m0 = self.regs.insert(parts.m_name.clone(), m_reg.clone());
        self.emit_stmt(&parts.max_stmt, decl, body)?;
        match saved_m0 {
            Some(v) => { self.regs.insert(parts.m_name.clone(), v); }
            None => { self.regs.remove(&parts.m_name); }
        }
        match saved_sc {
            Some(v) => { self.regs.insert(parts.sc_a.clone(), v); }
            None => { self.regs.remove(&parts.sc_a); }
        }
        body.push_str(&format!("    add.u32 {}, {}, 1;\n", r_cnt, r_cnt));
        body.push_str(&format!("    bra {};\n", h1h));
        body.push_str(&format!("{}:\n", h1e));
        // per-warp max → smem → M (32-slot max loop; base reset per iter).
        let rd_m = self.fresh_rd();
        decl.push_str(&format!("    .reg .b64 {};\n", rd_m));
        body.push_str(&format!("    mov.u64 {}, redm;\n", rd_m));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 4, {};\n",
            rd_m, r_warp, rd_m
        ));
        body.push_str(&format!("    st.shared.f32 [{}], {};\n", rd_m, m_reg));
        body.push_str("    bar.sync 0;\n");
        body.push_str(&format!("    mov.f32 {}, {};\n", m_reg, f32_imm(-1e30)));
        body.push_str(&format!("    mov.u32 {}, 0;\n", r_w));
        body.push_str(&format!("{}:\n", m_loop));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, r_w, warps));
        body.push_str(&format!("    @{} bra {};\n", pred, m_done));
        body.push_str(&format!("    mov.u64 {}, redm;\n", rd_m));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 4, {};\n",
            rd_m, r_w, rd_m
        ));
        body.push_str(&format!("    ld.shared.f32 {}, [{}];\n", f_t, rd_m));
        body.push_str(&format!("    max.f32 {}, {}, {};\n", m_reg, m_reg, f_t));
        body.push_str(&format!("    add.u32 {}, {}, 1;\n", r_w, r_w));
        body.push_str(&format!("    bra {};\n", m_loop));
        body.push_str(&format!("{}:\n", m_done));
        } // skip_pass != 1

        // ── pass B: unnormalized accumulation (dot recompute, p, l, acc strips) ──
        let h2h = format!("L{}_b", lab);
        let h2e = format!("L{}_bx", lab);
        let l_loop = format!("L{}_l", lab);
        let l_done = format!("L{}_ld", lab);
        let a_loop = format!("L{}_s", lab);
        let a_done = format!("L{}_sd", lab);
        if self.skip_pass != 2 {
        body.push_str(&format!("    mov.u32 {}, 0;\n", r_lo));
        body.push_str(&format!(
            "    mad.lo.u32 {}, {}, {}, {};\n",
            r_lo, r_warp, jslice, r_lo
        ));
        if let Some(sb) = &sbase {
            body.push_str(&format!("    add.u32 {}, {}, {};\n", r_lo, r_lo, sb));
        }
        body.push_str(&format!("    add.u32 {}, {}, {};\n", r_hi, r_lo, jslice));
        body.push_str(&format!("    mov.u32 {}, {};\n", r_cnt, r_lo));
        body.push_str(&format!("{}:\n", h2h));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, r_cnt, r_hi));
        body.push_str(&format!("    @{} bra {};\n", pred, h2e));
        body.push_str(&format!("    mov.f32 {}, 0f00000000;\n", sc_lane));
        let saved_d = self.regs.insert(parts.db.clone(), d_regs[0].clone());
        for i in 0..strips {
            self.active_strip = i;
            self.regs.insert(parts.db.clone(), d_regs[i].clone());
            self.regs.insert(parts.sc_b.clone(), sc_lane.clone());
            let mut body_b = parts.dot_b_body.clone();
            let pairs_b: Vec<(String, Expr)> = hoisted.iter().enumerate()
                .map(|(jdx, (_, e))| (strip_name(i, jdx), e.clone()))
                .collect();
            rewrite_j_stmts(&mut body_b, &pairs_b);
            for s in &body_b {
                self.emit_stmt(s, decl, body)?;
            }
        }
        match saved_d {
            Some(v) => { self.regs.insert(parts.db.clone(), v); }
            None => { self.regs.remove(&parts.db); }
        }
        self.emit_butterfly_add(sc_lane.clone(), sc_w.clone(), decl, body)?;
        let saved_sc = self.regs.insert(parts.sc_b.clone(), sc_w.clone());
        let saved_m = self.regs.insert(parts.m_name.clone(), m_reg.clone());
        self.emit_expr(&parts.p_expr, &p_reg, decl, body)?;
        let saved_l = self.regs.insert(parts.l_name.clone(), l_reg.clone());
        let saved_p = self.regs.insert(parts.p_name.clone(), p_reg.clone());
        self.emit_stmt(
            &Statement::Assign(
                Expr::Identifier(parts.l_name.clone()),
                parts.l_rhs.clone(),
            ),
            decl,
            body,
        )?;
        // acc strips: registers; the smem merge owns cross-warp traffic.
        self.strip_acc = Some((parts.acc_buf.clone(), parts.dc2.clone(), a_regs.clone()));
        for i in 0..strips {
            self.active_strip = i;
            self.regs.insert(parts.dc2.clone(), d_regs[i].clone());
            for s in &parts.acc_stmts {
                self.emit_stmt(s, decl, body)?;
            }
        }
        self.strip_acc = None;
        match saved_p {
            Some(v) => { self.regs.insert(parts.p_name.clone(), v); }
            None => { self.regs.remove(&parts.p_name); }
        }
        match saved_l {
            Some(v) => { self.regs.insert(parts.l_name.clone(), v); }
            None => { self.regs.remove(&parts.l_name); }
        }
        match saved_m {
            Some(v) => { self.regs.insert(parts.m_name.clone(), v); }
            None => { self.regs.remove(&parts.m_name); }
        }
        match saved_sc {
            Some(v) => { self.regs.insert(parts.sc_b.clone(), v); }
            None => { self.regs.remove(&parts.sc_b); }
        }
        body.push_str(&format!("    add.u32 {}, {}, 1;\n", r_cnt, r_cnt));
        body.push_str(&format!("    bra {};\n", h2h));
        body.push_str(&format!("{}:\n", h2e));
        } // skip_pass != 2

        // ── merges: l (warp-direct smem sum), acc strips (smem sum) ──
        // 2026-09-20 (M1-finish NaN fix): l is warp-uniform (all 32 lanes
        // compute the same p and accumulate the same l).  A butterfly would
        // sum 32 identical copies → 32× overcount per warp → 1024× in
        // l_tot.  Store l_reg directly; the merge loop sums 32 warp values.
        let rd_l = self.fresh_rd();
        decl.push_str(&format!("    .reg .b64 {};\n", rd_l));
        body.push_str(&format!("    mov.u64 {}, redl;\n", rd_l));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 4, {};\n",
            rd_l, r_warp, rd_l
        ));
        body.push_str(&format!("    st.shared.f32 [{}], {};\n", rd_l, l_reg));
        body.push_str("    bar.sync 0;\n");
        body.push_str(&format!("    mov.f32 {}, 0f00000000;\n", l_tot));
        body.push_str(&format!("    mov.u32 {}, 0;\n", r_w));
        body.push_str(&format!("{}:\n", l_loop));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, r_w, warps));
        body.push_str(&format!("    @{} bra {};\n", pred, l_done));
        body.push_str(&format!("    mov.u64 {}, redl;\n", rd_l));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 4, {};\n",
            rd_l, r_w, rd_l
        ));
        body.push_str(&format!("    ld.shared.f32 {}, [{}];\n", f_t, rd_l));
        body.push_str(&format!("    add.f32 {}, {}, {};\n", l_tot, l_tot, f_t));
        body.push_str(&format!("    add.u32 {}, {}, 1;\n", r_w, r_w));
        body.push_str(&format!("    bra {};\n", l_loop));
        body.push_str(&format!("{}:\n", l_done));

        // acc strips → this warp's smem row.
        let rd_a = self.fresh_rd();
        decl.push_str(&format!("    .reg .b64 {};\n", rd_a));
        body.push_str(&format!("    mov.u64 {}, smacc;\n", rd_a));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 512, {};\n",
            rd_a, r_warp, rd_a
        ));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 4, {};\n",
            rd_a, r_lane, rd_a
        ));
        for i in 0..strips {
            body.push_str(&format!(
                "    st.shared.f32 [{}+{}], {};\n",
                rd_a,
                i * 128,
                a_regs[i]
            ));
        }
        body.push_str("    bar.sync 0;\n");
        // a_i = Σ over the 32 warp rows (row pointer reset per iteration).
        for i in 0..strips {
            body.push_str(&format!("    mov.f32 {}, 0f00000000;\n", a_regs[i]));
        }
        let rd_row = self.fresh_rd();
        decl.push_str(&format!("    .reg .b64 {};\n", rd_row));
        body.push_str(&format!("    mov.u32 {}, 0;\n", r_w));
        body.push_str(&format!("{}:\n", a_loop));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, r_w, warps));
        body.push_str(&format!("    @{} bra {};\n", pred, a_done));
        body.push_str(&format!("    mov.u64 {}, smacc;\n", rd_row));
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 512, {};\n",
            rd_row, r_w, rd_row
        ));
        // 2026-09-20 (M1-finish NaN fix): include the lane offset so each
        // thread reads its OWN partial from smem, not lane 0's.
        body.push_str(&format!(
            "    mad.wide.u32 {}, {}, 4, {};\n",
            rd_row, r_lane, rd_row
        ));
        for i in 0..strips {
            body.push_str(&format!(
                "    ld.shared.f32 {}, [{}+{}];\n",
                f_t,
                rd_row,
                i * 128
            ));
            body.push_str(&format!(
                "    add.f32 {}, {}, {};\n",
                a_regs[i], a_regs[i], f_t
            ));
        }
        body.push_str(&format!("    add.u32 {}, {}, 1;\n", r_w, r_w));
        body.push_str(&format!("    bra {};\n", a_loop));
        body.push_str(&format!("{}:\n", a_done));

        if split > 1 {
            // Split: write per-slice partials; the combine kernel merges.
            let spec = PartialStore {
                parts,
                a_regs: &a_regs,
                r_lane: &r_lane,
                m_reg: &m_reg,
                l_tot: &l_tot,
            };
            self.emit_deferred_partial_store(&spec, decl, body)?;
        } else {
            // ── deferred normalize: strips own d; l broadcast; global stores ──
            let saved_l2 = self.regs.insert(parts.l_name.clone(), l_tot.clone());
            self.strip_acc = Some((parts.acc_buf.clone(), parts.dc.clone(), a_regs.clone()));
            for i in 0..strips {
                self.active_strip = i;
                self.regs.insert(parts.dc.clone(), d_regs[i].clone());
                self.emit_stmt(&parts.norm_stmt, decl, body)?;
            }
            self.strip_acc = None;
            match saved_l2 {
                Some(v) => { self.regs.insert(parts.l_name.clone(), v); }
                None => { self.regs.remove(&parts.l_name); }
            }
        }
        let _ = warps;
        match saved_j {
            Some(v) => { self.regs.insert(parts.j_name.clone(), v); }
            None => { self.regs.remove(&parts.j_name); }
        }
        Ok(())
    }
}

/// 2026-09-19 (M1-finish): shape-level predicate for the runner dispatch —
/// the deferred-softmax region is a 1024-thread block-per-workitem kernel
/// (32 warp slices of the KV dimension).
pub fn has_deferred_region(stmts: &[Statement], index_var: &str) -> bool {
    detect_deferred_region(stmts, index_var).is_some()
}

/// 2026-09-30 (general reduction-split): `(kv, dim, acc_buf, out_buf)` of a
/// deferred region, or `None` when no region matches — the dispatch uses it
/// to size the split factor (`gpu_strategy::reduction_split_factor`) and to
/// build the combine kernel.
pub fn deferred_region_info(
    stmts: &[Statement],
    index_var: &str,
) -> Option<(Expr, Expr, String, String)> {
    detect_deferred_region(stmts, index_var)
        .map(|(_, _, p)| (p.la_end, p.lc_end, p.acc_buf, p.out_buf))
}

/// 2026-09-30 (general reduction-split): the combine kernel's parameters.
pub struct CombineSpec {
    pub count: i64,
    pub dim: i64,
    pub split: u64,
}

/// 2026-09-30 (general reduction-split): the combine kernel for a split
/// deferred region. Grid = `count` blocks of `dim` threads; each block
/// merges its work item's `split` partials (written by
/// `emit_deferred_partial_store`) via the online-softmax algebra:
///   `m* = max_s m_s ; acc = Σ_s acc_s·exp(m_s−m*) ; l = Σ_s l_s·exp(m_s−m*)`
///   `out[d] = acc / l`.
pub fn emit_deferred_combine_ptx(
    layout: &SsboLayout,
    acc_buf: &str,
    out_buf: &str,
    spec: CombineSpec,
) -> Result<String, String> {
    let CombineSpec { count, dim, split } = spec;
    let off = |name: &str| -> Result<u64, String> {
        layout
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.proj_offset)
            .ok_or_else(|| format!("ptx combine: buffer '{name}' not in layout"))
    };
    let acc_off = off(acc_buf)?;
    let out_off = off(out_buf)?;
    let stride = 2 + dim;
    let log2e = "0f3FB8AA3B"; // 1.4426950408889634
    // 2026-10-01 (split device-validation): derived via `f32_imm` — the
    // hand-written "0ff800000" was one hex nibble short of `-inf`
    // (`0fFF800000`), a PTX parse error the CUDA lane died on. Immediates
    // are never hand-typed; `assert_ptx_well_formed` pins the digit count.
    let neg_inf = f32_imm(f32::NEG_INFINITY);
    let mut out = String::new();
    out.push_str(".version 8.0\n.target sm_86\n.address_size 64\n");
    out.push_str(".visible .entry main (.param .b64 proj_param)\n{\n");
    out.push_str("    .reg .b64 %rd1, %rd2, %rd3, %rd4, %rd5, %rd6;\n");
    out.push_str("    .reg .u32 %r1, %r2, %r3, %r4, %r5;\n");
    out.push_str("    .reg .f32 %f1, %f2, %f3, %f4, %f5, %f6;\n");
    out.push_str("    .reg .pred %p1, %p2;\n");
    out.push_str("    ld.param.u64 %rd1, [proj_param];\n");
    out.push_str("    mov.u32 %r1, %ctaid.x;\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r1, {count};\n"));
    out.push_str("    @%p1 ret;\n");
    out.push_str("    mov.u32 %r2, %tid.x;\n");
    out.push_str(&format!("    setp.ge.u32 %p2, %r2, {dim};\n"));
    out.push_str("    @%p2 ret;\n");
    // partial base: %rd3 = proj + acc_off + (w·S)·stride·4
    out.push_str("    mov.u32 %r3, %r1;\n");
    out.push_str(&format!("    mul.lo.u32 %r3, %r3, {split};\n"));
    out.push_str(&format!("    mul.lo.u32 %r3, %r3, {stride};\n"));
    out.push_str("    mul.wide.u32 %rd2, %r3, 4;\n");
    out.push_str(&format!("    mov.u64 %rd3, {acc_off};\n"));
    out.push_str("    add.u64 %rd3, %rd1, %rd3;\n");
    out.push_str("    add.u64 %rd3, %rd3, %rd2;\n");
    // pass 1: m* = max_s m_s
    out.push_str(&format!("    mov.f32 %f1, {neg_inf};\n"));
    out.push_str("    mov.u32 %r4, 0;\n");
    out.push_str("CM:\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r4, {split};\n"));
    out.push_str("    @%p1 bra CMD;\n");
    out.push_str("    mov.u32 %r5, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r5, %r5, {stride};\n"));
    out.push_str("    mul.wide.u32 %rd4, %r5, 4;\n");
    out.push_str("    add.u64 %rd5, %rd3, %rd4;\n");
    out.push_str("    ld.global.f32 %f2, [%rd5+0];\n");
    out.push_str("    max.f32 %f1, %f1, %f2;\n");
    out.push_str("    add.u32 %r4, %r4, 1;\n");
    out.push_str("    bra CM;\n");
    out.push_str("CMD:\n");
    // pass 2: acc, l
    out.push_str("    mov.f32 %f3, 0f00000000;\n");
    out.push_str("    mov.f32 %f4, 0f00000000;\n");
    out.push_str("    mov.u32 %r4, 0;\n");
    out.push_str("CS:\n");
    out.push_str(&format!("    setp.ge.u32 %p1, %r4, {split};\n"));
    out.push_str("    @%p1 bra CSD;\n");
    out.push_str("    mov.u32 %r5, %r4;\n");
    out.push_str(&format!("    mul.lo.u32 %r5, %r5, {stride};\n"));
    out.push_str("    mul.wide.u32 %rd4, %r5, 4;\n");
    out.push_str("    add.u64 %rd5, %rd3, %rd4;\n");
    out.push_str("    ld.global.f32 %f2, [%rd5+0];\n"); // m_s
    out.push_str("    sub.f32 %f5, %f2, %f1;\n");
    out.push_str(&format!("    mul.f32 %f5, %f5, {log2e};\n"));
    out.push_str("    ex2.approx.f32 %f5, %f5;\n"); // exp(m_s - m*)
    out.push_str("    ld.global.f32 %f6, [%rd5+4];\n"); // l_s
    out.push_str("    fma.rn.f32 %f4, %f6, %f5, %f4;\n");
    out.push_str("    mul.wide.u32 %rd6, %r2, 4;\n");
    out.push_str("    add.u64 %rd6, %rd5, %rd6;\n");
    out.push_str("    ld.global.f32 %f6, [%rd6+8];\n"); // acc_s[d]
    out.push_str("    fma.rn.f32 %f3, %f6, %f5, %f3;\n");
    out.push_str("    add.u32 %r4, %r4, 1;\n");
    out.push_str("    bra CS;\n");
    out.push_str("CSD:\n");
    out.push_str("    div.rn.f32 %f6, %f3, %f4;\n");
    // out[w*dim + d] = acc/l
    out.push_str("    mov.u32 %r5, %r1;\n");
    out.push_str(&format!("    mul.lo.u32 %r5, %r5, {dim};\n"));
    out.push_str("    add.u32 %r5, %r5, %r2;\n");
    out.push_str("    mul.wide.u32 %rd6, %r5, 4;\n");
    out.push_str(&format!("    mov.u64 %rd5, {out_off};\n"));
    out.push_str("    add.u64 %rd5, %rd1, %rd5;\n");
    out.push_str("    add.u64 %rd5, %rd5, %rd6;\n");
    out.push_str("    st.global.f32 [%rd5], %f6;\n");
    out.push_str("    ret;\n}\n");
    Ok(out)
}
