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

const BLOCK: u32 = 256;

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

pub fn emit_general_ptx(
    shape: &KernelShape,
    count: i64,
    layout: &SsboLayout,
    consts: &std::collections::HashMap<String, Expr>,
) -> Result<String, String> {
    let mut g = Gen::new(layout, consts, count);
    g.emit(shape)
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
}

impl<'a> Gen<'a> {
    fn new(
        layout: &'a SsboLayout,
        consts: &'a std::collections::HashMap<String, Expr>,
        count: i64,
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

        // gid = ctaid.x * BLOCK + tid.x
        body.push_str("    ld.param.u64 %rd1, [proj_param];\n");
        body.push_str("    mov.u32 %r1, %ctaid.x;\n");
        body.push_str(&format!("    mul.lo.u32 %r1, %r1, {};\n", BLOCK));
        body.push_str("    mov.u32 %r2, %tid.x;\n");
        body.push_str("    add.u32 %r1, %r1, %r2;\n");
        body.push_str(&format!("    setp.ge.u32 %p1, %r1, {};\n", self.count));
        body.push_str("    @%p1 ret;\n");

        self.regs
            .insert(shape.index_var.clone(), self.gid.to_string());

        for stmt in &shape.kernel_stmts {
            self.emit_stmt(stmt, &mut decl, &mut body)?;
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
        match stmt {
            Statement::Assign(lhs, rhs) => self.emit_assign(lhs, rhs, decl, body),
            Statement::Let { name, expr, .. } => {
                // A pure local: lower the initializer into a register.
                if let Some(e) = expr {
                    let reg = self.fresh_f();
                    decl.push_str(&format!("    .reg .f32 {};\n", reg));
                    self.emit_expr(e, &reg, decl, body)?;
                    self.regs.insert(name.clone(), reg);
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
                for s in loop_body {
                    self.emit_stmt(s, decl, body)?;
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

    fn emit_assign(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        decl: &mut String,
        body: &mut String,
    ) -> Result<(), String> {
        // lhs = Index(buf, index) → array store; lhs = Identifier(scalar) → scalar store.
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
                self.emit_expr(rhs, &reg, decl, body)?;
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
                    other => {
                        return Err(format!(
                            "ptx general: index arithmetic {:?} outside the supported surface (Add/Sub/Mul)",
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
            body.push_str(&format!("    mov.f32 {}, {};\n", out, n));
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