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
//! ```
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
/// accumulator, loads linear in the item), the span is ≥ 512, and
/// span % 4 == 0 (4 warps per 128-thread block, exact slices, no tail).
/// Used by both the emitter (lowering choice) and the runner
/// (block-per-workitem dispatch, block_threads 128). Structural match,
/// same discipline as has_lane_reduction.
pub fn has_warp_slice(
    kernel_stmts: &[Statement],
    consts: &std::collections::HashMap<String, Expr>,
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
        if span < 512 || span % 4 != 0 {
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

pub fn emit_general_ptx(
    shape: &KernelShape,
    count: i64,
    layout: &SsboLayout,
    consts: &std::collections::HashMap<String, Expr>,
    universe: &crate::type_universe::TypeUniverse,
    int_bits: u64,
) -> Result<String, String> {
    let mut g = Gen::new(layout, consts, count, universe, int_bits);
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
            strip_acc: None,
            active_strip: 0,
        }
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

        body.push_str("    ld.param.u64 %rd1, [proj_param];\n");
        if self.block_work_item {
            // Work item = BLOCK: w = ctaid.x.  Move the special register
            // to %r1 (a GPR) so it can be used in setp and address math.
            // The guard kills entire blocks (r1 >= count → ret) — uniform
            // per block, safe for the warp-wide butterfly.
            body.push_str("    mov.u32 %r1, %ctaid.x;\n");
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
                    if Self::is_int_ty(ty) {
                        let reg = self.fresh_r();
                        decl.push_str(&format!("    .reg .u32 {};\n", reg));
                        self.emit_index(e, &reg, decl, body)?;
                        self.regs.insert(name.clone(), reg);
                    } else {
                        let reg = self.fresh_f();
                        decl.push_str(&format!("    .reg .f32 {};\n", reg));
                        self.emit_expr(e, &reg, decl, body)?;
                        self.regs.insert(name.clone(), reg);
                    }
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
                let unroll = crate::config_tuning::ir_lowering().ptx_serial_unroll as usize;
                let ctx = UnrollCtx {
                    item,
                    start: start_v,
                    end: self.range_exclusive_end(list, end_v)?,
                    unroll,
                };
                // 2026-09-19 (M1 warp-sliced reductions, plan
                // general-machinery): a LONG serial reduction splits across
                // the block's 4 warps (P1 block-per-workitem dispatch,
                // smem partial merge). Parallelism beats the unroll's MLP,
                // so the slice takes priority; the unroll stays the
                // fallback for short or non-divisible loops.
                if crate::config_tuning::ir_lowering().ptx_warp_slice
                    && matches!(lane_plan, None)
                    && ctx.end >= ctx.start + 512
                    && (ctx.end - ctx.start) % 4 == 0
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
                            loop_body,
                            item,
                            ctx.start,
                            ctx.end - ctx.start,
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
        body_stmts: &[Statement],
        item: &str,
        start: i64,
        span: i64,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        let acc_name = Self::slice_acc_name(body_stmts)
            .ok_or_else(|| "ptx general: warp slice without a scalar accumulator".to_string())?;
        let quarter = (span / 4) as u32;
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
        decl.push_str("    .shared .align 4 .b8 wpart[16];\n");

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
        // merge: the accumulator becomes the sum of the four warp partials.
        body.push_str(&format!("    mov.u64 {}, wpart;\n", rd_part));
        body.push_str(&format!("    ld.shared.f32 {}, [{}+0];\n", acc_reg, rd_part));
        for k in 1..4 {
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
        g.emit_warp_sliced(&body_stmts, "j", 0, 4096, &mut decl, &mut body)
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
            32,
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
        let jslice = (kv / 32) as u32;
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

        // ── pass A: row max (dot per j, butterfly, per-warp max) ──
        body.push_str(&format!("    mov.u32 {}, 0;\n", r_lo));
        body.push_str(&format!(
            "    mad.lo.u32 {}, {}, {}, {};\n",
            r_lo, r_warp, jslice, r_lo
        ));
        body.push_str(&format!("    add.u32 {}, {}, {};\n", r_hi, r_lo, jslice));
        body.push_str(&format!("    mov.u32 {}, {};\n", r_cnt, r_lo));
        let saved_j = self.regs.insert(parts.j_name.clone(), r_cnt.clone());
        let lab = self.label;
        self.label += 3;
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
            for s in &parts.dot_a_body {
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

        // ── pass B: unnormalized accumulation (dot recompute, p, l, acc strips) ──
        body.push_str(&format!("    mov.u32 {}, 0;\n", r_lo));
        body.push_str(&format!(
            "    mad.lo.u32 {}, {}, {}, {};\n",
            r_lo, r_warp, jslice, r_lo
        ));
        body.push_str(&format!("    add.u32 {}, {}, {};\n", r_hi, r_lo, jslice));
        body.push_str(&format!("    mov.u32 {}, {};\n", r_cnt, r_lo));
        let h2h = format!("L{}_b", lab);
        let h2e = format!("L{}_bx", lab);
        let l_loop = format!("L{}_l", lab);
        let l_done = format!("L{}_ld", lab);
        let a_loop = format!("L{}_s", lab);
        let a_done = format!("L{}_sd", lab);
        body.push_str(&format!("{}:\n", h2h));
        body.push_str(&format!("    setp.ge.u32 {}, {}, {};\n", pred, r_cnt, r_hi));
        body.push_str(&format!("    @{} bra {};\n", pred, h2e));
        body.push_str(&format!("    mov.f32 {}, 0f00000000;\n", sc_lane));
        let saved_d = self.regs.insert(parts.db.clone(), d_regs[0].clone());
        for i in 0..strips {
            self.active_strip = i;
            self.regs.insert(parts.db.clone(), d_regs[i].clone());
            self.regs.insert(parts.sc_b.clone(), sc_lane.clone());
            for s in &parts.dot_b_body {
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
