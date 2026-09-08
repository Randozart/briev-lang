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
//! Not yet: tensor ops (mma.sync/ldmatrix/cp.async — S3), f16 operands,
//! non-GEMM kernels. Each arrives as a first-class emitter arm with tests.

use crate::ast::{Expr, TopLevel};
use crate::backend::spirv::gemm::GemmPlan;
use crate::backend::spirv::runner::RunnerKernel;
use crate::type_universe::TypeUniverse;

pub mod tensor;

/// PTX entry point name — the CUDA driver's `create_kernel` resolves "main"
/// (`cuModuleGetFunction`). Must never drift from `briev_dev_cuda.c`.
const ENTRY: &str = "main";

/// Naive GEMM PTX: work item `i` computes `y[i] = sum_k a[m*K+k] * b[k*N+n]`
/// with `m = i/N`, `n = i%N`. Flat 1D grid (block 64, grid ceil(items/64))
/// — the runner's `dispatch_geometry_stmt` flat form. `a_off`/`b_off`/`y_off`
/// are the DEVICE projection offsets (from `ssbo_layout`); `elem_bytes` is
/// the array element size (4 = f32 — the only S2a operand).
fn naive_gemm_ptx(m: i64, n: i64, k: i64, elem_bytes: u32,
                  a_off: u64, b_off: u64, y_off: u64) -> String {
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
    // y[i] = acc
    out.push_str(&format!("    mul.wide.u32 %rd5, %r3, {};\n", elem_bytes));
    out.push_str("    add.u64 %rd6, %rd4, %rd5;\n");
    out.push_str("    st.global.f32 [%rd6], %f1;\n");
    out.push_str("    ret;\n}\n");
    out
}

/// Build the PTX kernel set for an `.abv` — one kernel per eligible accel
/// entry. GEMM-shaped entries lower to the naive PTX kernel; anything else
/// is a hard error (the S2a surface gate — GEMM family ONLY until S5).
pub fn build_ptx_kernels(
    program: &[TopLevel],
    universe: &TypeUniverse,
    int_bits: u64,
    entries: &std::collections::HashMap<String, crate::analysis::accel::AccelEntry>,
) -> Result<Vec<RunnerKernel>, String> {
    let mut names: Vec<&String> = entries
        .iter()
        .filter(|(_, e)| e.shape.eligible)
        .map(|(n, _)| n)
        .collect();
    names.sort();

    // The ONE layout rule — the same `ssbo_layout` the runner uses, so the
    // hardcoded PTX offsets and the runner's field table agree by
    // construction (no image plans in the GEMM tier).
    let layout = crate::backend::spirv::runner::ssbo_layout(
        program, universe, int_bits, &std::collections::HashMap::new(),
    )?;

    let mut out = Vec::new();
    for name in names {
        let e = &entries[name];
        let plan = GemmPlan::match_stmts(&e.shape, program).ok_or_else(|| {
            format!(
                "ptx: node '{}' is not a GEMM-shaped kernel — the PTX tier (S2a) \
                 lowers GEMM only until the S5 anchor race is won\n  why: the \
                 surface gate keeps the emitter honest (no silent generic \
                 lowering)\n  fix: use --backend spirv for this program, or \
                 retry once the PTX tier grows the general kernel emitter",
                name
            )
        })?;

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

        let (ptx, cooperative) = if tensor {
            (
                tensor::tensor_gemm_ptx(plan.m, plan.n, plan.k, a_off, b_off, y_off, y_elem),
                true, // 32-lane cooperative dispatch: nx=32, ny=(M/32)*(N/16)
            )
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
            (naive_gemm_ptx(plan.m, plan.n, plan.k, a_elem, a_off, b_off, y_off), false)
        };

        out.push(RunnerKernel {
            name: name.clone(),
            spirv: ptx.into_bytes(),
            image_plans: Vec::new(),
            index_var: e.shape.index_var.clone(),
            count_expr: e.shape.count_expr.clone().unwrap_or(Expr::Decimal(0)),
            work_cols: None,
            cooperative,
            tiled: false,
            tensor: false,
            tensor_tile_rows: 1,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_gemm_ptx_has_flat_grid_and_guards() {
        let ptx = naive_gemm_ptx(64, 64, 16, 4, 0, 65536, 131072);
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
        let ptx = naive_gemm_ptx(8, 16, 4, 4, 0, 1024, 2048);
        assert!(ptx.contains("setp.ge.u32 %p1, %r3, 128;"), "8*16 items: {ptx}");
        assert!(ptx.contains("div.u32 %r4, %r3, 16;"), "N: {ptx}");
        assert!(ptx.contains("setp.ge.u32 %p2, %r6, 4;"), "K: {ptx}");
    }
}