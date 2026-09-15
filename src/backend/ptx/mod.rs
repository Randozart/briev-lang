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
    for name in names {
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
            let (mw, nw) = select_mw_nw(plan.m, plan.n, thread_cap, warp_mh);
            let mw_ok = plan.m % ((16 * warp_mh * mw) as i64) == 0
                && plan.n % ((8 * gr * nw) as i64) == 0;
            if mw_ok && (mw > 1 || nw > 1) {
                // 2026-09-15 (repro): wire the epilogue variant when a
                // fused scale applies (the f16 packed-f16x2 mul path).
                let ptx = match epilogue_scale {
                    Some(s) => tensor::tensor_gemm_ptx_smem_mw_epilogue(
                        plan.m, plan.n, plan.k, a_off, b_off, y_off, y_elem, mw, nw,
                        f16_acc, stages, warp_mh, s,
                    ),
                    None => tensor::tensor_gemm_ptx_smem_mw(
                        plan.m, plan.n, plan.k, a_off, b_off, y_off, y_elem, mw, nw,
                        f16_acc, stages, warp_mh,
                    ),
                };
                (
                    ptx,
                    true,
                    e.shape.count_expr.clone().unwrap_or(Expr::Decimal(0)),
                    (mw * nw * 32) as u32,
                    ((mw * warp_mh * 512 + nw * gr * 256) * stages) as u32,
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
}