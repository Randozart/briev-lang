//! `.abv` standalone runtime generation (2026-08-31, plan
//! abv-gpu-by-default item 4): an Accelerator Briev Volume is PURE GPU code
//! — the compile emits one `.spv` per kernel PLUS a self-contained C runner
//! that drives them.
//!
//! The runner IS the .abv node graph: state = the kernel SSBO projection,
//! each eligible node = a kernel dispatched resident-mode, each scalar-only
//! node = a host-side body (the phase machine), the loop = declared node
//! order until convergence (a full pass fires nothing).
//!
//! v1 surface (everything else is a helpful gen-time error naming the fix):
//! - pre-conditions over scalar state + literals (`i < nb`, `phase == 1`)
//! - `BeginProgram` markers → true (the counter fast-forward terminates the
//!   node: after the pass, the host sets `i = N` so the pre goes false)
//! - scalar-only bodies for non-kernel nodes (assignments + term)
//! - `get_env_int!("K")` initializers, literal initializers
//! - constant-indexed array reads in conditions/prints (observables)
//!
//! Undo: delete this module and its call sites in compile.rs / main.rs.

use crate::analysis::accel::AccelDecision;
use crate::analysis::accel::AccelEntry;
use crate::ast::{BinaryOpKind, Expr, Statement, TopLevel, Transaction, Type, UnaryOpKind};
use crate::backend::spirv::lower::collect_state_fields;
use crate::backend::spirv::SpirvBuilder;
use crate::type_universe::TypeUniverse;
use std::collections::HashMap;

/// One SSBO member: name, HOST byte offset (packed — the runner `state[]`
/// layout and the S_ macros), DEVICE projection byte offset (the shared
/// rule in `FnLowerer::projection_offsets` — vec4-eligible arrays are
/// 16B-aligned), element size, element count, and whether it is an array.
#[derive(Debug, Clone)]
pub struct RunnerField {
    pub name: String,
    pub offset: u64,
    pub proj_offset: u64,
    pub elem_bytes: u32,
    pub count: u64,
    pub is_array: bool,
    pub type_is_float: bool,
}

/// A kernel the runner dispatches: node name, embedded SPIR-V, the index
/// variable to fast-forward, and the work-item count expression.
pub struct RunnerKernel {
    pub name: String,
    pub spirv: Vec<u8>,
    /// 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): this
    /// kernel's image storage plans — the SSBO field table and the runtime
    /// (step 4: VkImage bindings + download) key off these.
    pub image_plans: Vec<crate::analysis::image_storage::ImageStoragePlan>,
    pub index_var: String,
    pub count_expr: Expr,
    /// 2D dispatch width (None = 1D). Mirrors `KernelShape::work_cols` —
    /// the runner must dispatch the SAME geometry the blob was built for.
    pub work_cols: Option<u64>,
    /// Cooperative row kernel (plan 2026-09-01-cooperative-row-kernels):
    /// dispatch nx = 32 lanes x ny = rows.
    pub cooperative: bool,
    /// 2026-09-10 (cp.async stages): dynamic shared-memory bytes the CUDA
    /// runtime must opt in to at launch (cuFuncSetAttribute + the launch's
    /// sharedMemBytes). 0 = none (every non-staged kernel).
    pub shared_bytes: u32,
    /// Tiled GEMM (plan 2026-09-01-m2-gemm M2.1): the blob is a shared-
    /// memory tiled kernel (LocalSize 16x16, 64x64 tile). Dispatch is 1D
    /// flattened: workgroups = (M/64)*(N/64), nx items = workgroups * 16
    /// (the driver divides by the module's local_x = 16).
    pub tiled: bool,
    /// Tensor GEMM (plan 2026-09-01-m2-tensor-cores): cooperative-matrix
    /// kernel (LocalSize 32, 16x16 tile). Dispatch: workgroups = (M/16)*
    /// (N/16) = n/256, nx items = workgroups * 32.
    pub tensor: bool,
    /// 2026-09-02 (B-reuse rung): the EFFECTIVE tile-rows the tensor blob
    /// was built with — `GemmPlan::coopmat_tile_rows(plan.m)` — so the
    /// runner's dispatch formula and the kernel's grid decode can never
    /// disagree (a mismatch = out-of-range tiles = garbage output).
    pub tensor_tile_rows: u32,
    /// PTX tensor warp-tile kernel (plan 2026-09-08-ptx-tier-execution
    /// S3b): one 32×16 C tile per 32-lane block, decoded from ctaid.y.
    /// The count expr is the WORK count (M*N, the counter fast-forward);
    /// the dispatch launches ny = count/(32*16) blocks.
    pub ptx_tensor: bool,
    /// 2026-09-09 (S3b+ perf rungs): CUDA block thread count for this
    /// kernel (default 64 — the driver's fixed block size). The S3b+
    /// multi-warp tensor kernels launch mw*nw*32-thread blocks; the
    /// dispatch geometry and the runtime's launch both key off this.
    pub block_threads: u32,
}

/// The SSBO layout EXACTLY as the kernel sees it (name-sorted, real element
/// widths) — derived with the same builder helpers the emitter uses so the
/// two can never drift.
pub struct SsboLayout {
    /// SSBO-resident fields (image arrays excluded) — the BrievField table.
    pub fields: Vec<RunnerField>,
    /// Image-resident arrays with their HOST offsets (the BrievImageDesc
    /// table's host_offset). proj_offset unused.
    pub images: Vec<RunnerField>,
    /// The FULL host state size — image arrays count (the host %State
    /// still holds the flat arrays; only the device projection excludes
    /// them).
    pub state_bytes: u64,
}

pub fn ssbo_layout(
    items: &[TopLevel],
    universe: &TypeUniverse,
    int_bits: u64,
    image_plans: &std::collections::HashMap<
        String,
        Vec<crate::analysis::image_storage::ImageStoragePlan>,
    >,
) -> Result<SsboLayout, String> {
    let mut sb = SpirvBuilder::new().with_universe(universe, int_bits);
    let mut fields = collect_state_fields(items);
    // 2026-09-02: arrays planned as images in ANY kernel are device-image
    // resident — excluded from the SSBO projection (the blob's SSBO struct
    // excluded them via set_image_plans; the field table must agree).
    // The HOST layout KEEPS them: the host %State still holds the flat
    // arrays, so host offsets of every field after an image array must
    // account for its bytes — the offset walk covers ALL fields.
    let mut plans_by_array: std::collections::HashMap<&str, u64> =
        std::collections::HashMap::new(); // array -> element count (w*h)
    for v in image_plans.values() {
        for p in v {
            plans_by_array.insert(p.array.as_str(), p.width as u64 * p.height as u64);
        }
    }
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    let mut buffer_fields: Vec<crate::backend::spirv::lower::StateField> = Vec::new();
    for f in &fields {
        if !plans_by_array.contains_key(f.name.as_str()) {
            buffer_fields.push(f.clone());
        }
    }
    // Device offsets come from the ONE layout rule the kernel also uses
    // (vec4-eligible arrays 16B-aligned) — they can never drift. Computed
    // over the BUFFER fields only (the blob's SSBO struct).
    let proj_offsets = crate::backend::spirv::lower::FnLowerer::projection_offsets(
        &mut sb, &buffer_fields,
    )?;
    let mut out = Vec::new();
    let mut images = Vec::new();
    let mut offset: u64 = 0;
    let mut proj_idx: usize = 0;
    for f in fields.into_iter() {
        let (elem, count, is_array, is_float) = match &f.ty {
            Type::Vector(inner, dims) => {
                let elems: u64 = dims
                    .iter()
                    .map(|d| match d {
                        crate::ast::Dimension::Anonymous(n) => *n as u64,
                        crate::ast::Dimension::Named(_, n) => *n as u64,
                    })
                    .product::<u64>()
                    .max(1);
                let e = sb.scalar_storage_bytes(inner.as_ref())?;
                let flt = sb.is_float_type(inner.as_ref())?;
                (e, elems, true, flt)
            }
            other => {
                let e = sb.scalar_storage_bytes(other)?;
                let flt = sb.is_float_type(other)?;
                (e, 1, false, flt)
            }
        };
        if let Some(&img_count) = plans_by_array.get(f.name.as_str()) {
            // Image-resident: host offset only; the texel count comes from
            // the plan (w*h — the R32F texel is 4 bytes).
            images.push(RunnerField {
                name: f.name.clone(),
                offset,
                proj_offset: 0,
                elem_bytes: 4,
                count: img_count,
                is_array: true,
                type_is_float: true,
            });
        } else {
            out.push(RunnerField {
                name: f.name.clone(),
                offset,
                proj_offset: proj_offsets[proj_idx] as u64,
                elem_bytes: elem,
                count,
                is_array,
                type_is_float: is_float,
            });
            proj_idx += 1;
        }
        offset += elem as u64 * count;
    }
    Ok(SsboLayout {
        state_bytes: offset,
        fields: out,
        images,
    })
}

fn c_ident(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn field_by_name<'a>(fields: &'a [RunnerField], name: &str) -> Option<&'a RunnerField> {
    fields.iter().find(|f| f.name == name)
}

/// Generate a C expression reading scalars / constant-indexed array
/// elements. Err names the unsupported construct (v1 surface).
fn emit_scalar_read(
    e: &Expr,
    fields: &[RunnerField],
    consts: &std::collections::HashMap<String, Expr>,
    out: &mut String,
) -> Result<(), String> {
    match e {
        Expr::Decimal(n) => {
            out.push_str(&format!("(long long){}", n));
            Ok(())
        }
        Expr::Float(v) => {
            out.push_str(&format!("(double){:e}", v));
            Ok(())
        }
        Expr::Bool(b) => {
            out.push_str(if *b { "1" } else { "0" });
            Ok(())
        }
        Expr::Identifier(name) => match field_by_name(fields, name) {
            Some(f) if !f.is_array => {
                let t = if f.type_is_float { "double" } else { "long long" };
                out.push_str(&format!("(*({}*)(state + {}))", t, f.offset));
                Ok(())
            }
            Some(f) => Err(format!(
                "scalar expression reads array '{}' — conditions and counts \
                 read scalar state only",
                f.name
            )),
            None => match consts.get(name) {
                Some(ce) => emit_scalar_read(ce, fields, consts, out),
                None => Err(format!("unknown state field or const '{}'", name)),
            },
        },
        Expr::BinaryOp(kind, l, r) => {
            let op = match kind {
                BinaryOpKind::Add => " + ",
                BinaryOpKind::Sub => " - ",
                BinaryOpKind::Mul => " * ",
                BinaryOpKind::Div => " / ",
                BinaryOpKind::Mod => " % ",
                BinaryOpKind::Lt => " < ",
                BinaryOpKind::Gt => " > ",
                BinaryOpKind::Le => " <= ",
                BinaryOpKind::Ge => " >= ",
                BinaryOpKind::Eq => " == ",
                BinaryOpKind::Neq => " != ",
                BinaryOpKind::And => " && ",
                BinaryOpKind::Or => " || ",
                BinaryOpKind::BitAnd => " & ",
                BinaryOpKind::BitOr => " | ",
                BinaryOpKind::BitXor => " ^ ",
                BinaryOpKind::Shl => " << ",
                BinaryOpKind::Shr => " >> ",
                BinaryOpKind::Concat => return Err("string concat in a scalar condition".into()),
            };
            out.push('(');
            emit_scalar_read(l, fields, consts, out)?;
            out.push_str(op);
            emit_scalar_read(r, fields, consts, out)?;
            out.push(')');
            Ok(())
        }
        Expr::UnaryOp(kind, e) => {
            let op = match kind {
                UnaryOpKind::Neg => "(-",
                UnaryOpKind::Not => "(!",
                UnaryOpKind::BitNot => "(~",
            };
            out.push_str(op);
            emit_scalar_read(e, fields, consts, out)?;
            out.push(')');
            Ok(())
        }
        Expr::BeginProgram => {
            // The counter fast-forward makes `[i < N]` false after the pass;
            // the entry marker adds nothing to termination here.
            out.push_str("1");
            Ok(())
        }
        Expr::Index(obj, idx) => {
            let Some(fname) = field_name_of_expr(obj) else {
                return Err("indexed read of a non-field expression".into());
            };
            let Some(fd) = field_by_name(fields, fname) else {
                return Err(format!("unknown array '{}'", fname));
            };
            let Expr::Decimal(k) = idx.as_ref() else {
                return Err(format!(
                    "array '{}' read with a non-constant index in a scalar \
                     expression (v1 reads constants only)",
                    fname
                ));
            };
            let t = if fd.type_is_float { "double" } else { "long long" };
            out.push_str(&format!(
                "(*({}*)(state + {} + {} * {}))",
                t, fd.offset, k, fd.elem_bytes
            ));
            Ok(())
        }
        other => Err(format!(
            "unsupported scalar-expression construct ({:?}) — the runner v1 \
             evaluates literals, state fields, arithmetic, comparisons, logic",
            std::mem::discriminant(other)
        )),
    }
}

fn field_name_of_expr(e: &Expr) -> Option<&str> {
    match e {
        Expr::Identifier(n) => Some(n),
        _ => None,
    }
}

fn emit_host_stmt(
    s: &Statement,
    fields: &[RunnerField],
    consts: &std::collections::HashMap<String, Expr>,
    out: &mut String,
    exited: &mut bool,
) -> Result<(), String> {
    match s {
        Statement::Assign(lhs, rhs) => {
            let Some(name) = field_name_of_expr(lhs) else {
                return Err("assignment to a non-field target".into());
            };
            let Some(fd) = field_by_name(fields, name) else {
                return Err(format!("assignment to unknown field '{}'", name));
            };
            if fd.is_array {
                return Err(format!(
                    "array '{}' assignment in a host body — arrays are \
                     kernel-owned in .abv",
                    name
                ));
            }
            let t = if fd.type_is_float { "double" } else { "long long" };
            out.push_str(&format!("      *({}*)(state + {}) = ", t, fd.offset));
            emit_scalar_read(rhs, fields, consts, out)?;
            out.push_str(";\n");
            Ok(())
        }
        // 2026-09-14 (gpu_schedule Phase 0): `term` in a HOST node is the
        // node's own convergence checkpoint — this firing is done, the node
        // will re-check its pre next pass. Only `endprogram` exits the
        // reactor loop (the scheduler must reach LATER nodes; a mid-program
        // `term` jumping to `done` would skip gemm2 in a GEMM chain).
        Statement::Term(_) => Ok(()),
        Statement::EndProgram(_) => {
            *exited = true;
            Ok(())
        }
        other => Err(format!(
            "host body statement ({:?}) outside the runner v1 surface",
            std::mem::discriminant(other)
        )),
    }
}

/// Emit the full self-contained runner C source. `kernels` = one entry per
/// eligible node; each SPIR-V binary is a separate module whose entry point
/// is "main".
pub fn emit_runner(
    program: &[TopLevel],
    universe: &TypeUniverse,
    int_bits: u64,
    kernels: &[RunnerKernel],
    schedule: Option<&crate::analysis::gpu_schedule::GpuSchedule>,
) -> Result<String, String> {
    let layout = ssbo_layout(
        program,
        universe,
        int_bits,
        &kernels
            .iter()
            .map(|k| (k.name.clone(), k.image_plans.clone()))
            .collect(),
    )?;
    let fields = layout.fields;
    // Module consts (literals only) usable in conditions, counts and bodies.
    let mut consts: std::collections::HashMap<String, Expr> = Default::default();
    for item in program {
        if let TopLevel::Constant(c) = item {
            if matches!(c.expr, Expr::Decimal(_) | Expr::Float(_)) {
                consts.insert(c.name.clone(), c.expr.clone());
            }
        }
    }
    let total: u64 = layout.state_bytes;
    let mut out = String::new();

    out.push_str("// Generated by brievc - .abv standalone runner. Do not edit.\n");
    out.push_str("#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n#include <stdint.h>\n\n");
    out.push_str("#include \"briev_accel_rt.c\"\n\n");
    out.push_str(&format!("static unsigned char state[{}];\n", total + 64));
    for f in &fields {
        let t = if f.type_is_float { "double" } else { "long long" };
        out.push_str(&format!(
            "#define S_{} (*({}*)(state + {}))\n",
            c_ident(&f.name),
            t,
            f.offset
        ));
    }
    for (i, k) in kernels.iter().enumerate() {
        out.push_str(&format!("static const uint8_t k{}[] = {{", i));
        for (j, b) in k.spirv.iter().enumerate() {
            if j % 20 == 0 {
                out.push('\n');
            }
            out.push_str(&format!("{},", b));
        }
        out.push_str("\n};\n");
        out.push_str(&format!(
            "static const uint32_t k{}_len = {}u;\n",
            i,
            k.spirv.len()
        ));
    }
    out.push_str("static BrievField fields[] = {\n");
    for f in &fields {
        out.push_str(&format!(
            "    {{ \"{}\", {}, {}, {}, {}, {}, {} }},\n",
            f.name,
            if f.is_array { 1 } else { 2 },
            f.offset,
            f.elem_bytes,
            f.count,
            if f.is_array { 1 } else { 0 },
            f.proj_offset
        ));
    }
    out.push_str("};\n");
    // 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): the image
    // tables — one shared table per kernel set (the kernel's SSBO excluded
    // image arrays; the host %State still holds the flat arrays).
    // The image table's host offsets come from the layout (image arrays
    // sit in the HOST state layout; the projection excludes them); the
    // width/height come from the plans.
    let mut all_images: Vec<(&RunnerField, u32, u32)> = Vec::new(); // (field, w, h)
    for img in &layout.images {
        for k in kernels {
            if let Some(p) = k.image_plans.iter().find(|p| p.array == img.name) {
                all_images.push((img, p.width as u32, p.height as u32));
                break;
            }
        }
    }
    if all_images.is_empty() {
        // Zero-size arrays are invalid C — a dummy row every kernel
        // ignores (its n_images is 0).
        out.push_str("static BrievImageDesc images[1] = { { 0, 0, 0, 0, 0 } };\n");
    } else {
        out.push_str("static BrievImageDesc images[] = {\n");
        for (img, w, h) in &all_images {
            out.push_str(&format!(
                "    {{ \"{}\", {}, {}, {}, {} }},\n",
                img.name, img.offset, w, h, 1u32 /* BRIEV_IMAGE_FORMAT_R32F */
            ));
        }
        out.push_str("};\n");
    }
    out.push_str("static BrievKernelDesc descs[] = {\n");
    for (i, k) in kernels.iter().enumerate() {
        // host_offset patch: the runner emits the table with placeholder
        // offsets, then computes them from the state layout below.
        out.push_str(&format!(
            "    {{ \"{}\", k{}, k{}_len, {}, fields, {}, images, {}, {} }},\n",
            c_ident(&k.name),
            i,
            i,
            fields.len(),
            k.image_plans.len(),
            k.block_threads,
            k.shared_bytes
        ));
    }
    out.push_str(&format!(
        "}};\nstatic const uint32_t N_KERNELS = {};\n",
        kernels.len()
    ));

    // seed_state: literal + get_env_int! initializers.
    out.push_str("static void seed_state(void) {\n");
    for item in program {
        let stmt = match item {
            TopLevel::Statement(s) => s.as_ref(),
            _ => continue,
        };
        let Statement::Let { name, expr: Some(e), .. } = stmt else {
            continue;
        };
        let Some(fd) = field_by_name(&fields, name) else {
            continue;
        };
        if fd.is_array {
            continue;
        }
        let t = if fd.type_is_float { "double" } else { "long long" };
        match e {
            Expr::Decimal(n) => {
                out.push_str(&format!("  *({}*)(state + {}) = {}LL;\n", t, fd.offset, n));
            }
            Expr::Float(v) => {
                out.push_str(&format!("  *({}*)(state + {}) = {:e};\n", t, fd.offset, v));
            }
            Expr::Call(cname, args, _)
                if cname == "get_env_int!" || cname == "get_env_int#" =>
            {
                let key = match args.first() {
                    Some(Expr::Quoted(q)) => String::from_utf8_lossy(q).to_string(),
                    _ => continue,
                };
                out.push_str(&format!(
                    "  {{ const char* e = getenv(\"{}\"); *({}*)(state + {}) = e ? atoll(e) : 0; }}\n",
                    key, t, fd.offset
                ));
            }
            _ => {}
        }
    }
    out.push_str("}\n\n");

    // Scheduler: one pass per iteration, declared node order, exit on
    // convergence. Kernel nodes dispatch resident-mode and fast-forward
    // their counter; host nodes run their scalar bodies.
    out.push_str("int main(void) {\n");
    out.push_str("  seed_state();\n");
    out.push_str(
        "  if (!briev_accel_init(descs, N_KERNELS)) { fprintf(stderr, \"briev: no GPU device available\\n\"); return 1; }\n",
    );
    out.push_str("  long guard = 0;\n");
    out.push_str("  for (;;) {\n");
    out.push_str(
        "    if (++guard > 2000000000L) { fprintf(stderr, \"briev: run cap reached\\n\"); break; }\n",
    );
    out.push_str("    int fired = 0;\n");
    let mut done_label_used = false;
    // 2026-09-14 (gpu_schedule Phase 1): iterate the node DAG's
    // producer-before-consumer topological order when the frontend computed
    // one (fallback: declaration order). The author's `phase` scalars become
    // redundant — the DAG orders the launches and the shared device state
    // carries the array flow.
    let txn_by_name: std::collections::HashMap<&str, &Transaction> = program
        .iter()
        .filter_map(|item| {
            if let TopLevel::Transaction(t) = item {
                Some((t.name.as_str(), t))
            } else {
                None
            }
        })
        .collect();
    let order: Vec<&str> = match schedule {
        Some(s) if !s.order.is_empty() => s.order.iter().map(|n| n.as_str()).collect(),
        _ => program
            .iter()
            .filter_map(|item| match item {
                TopLevel::Transaction(t) => Some(t.name.as_str()),
                _ => None,
            })
            .collect(),
    };
    for name in order {
        let Some(t) = txn_by_name.get(name) else {
            continue;
        };
        let t: &Transaction = t;
        // 2026-09-14 (gpu_schedule Phase 4a): an epilogue-fused consumer's
        // work is done by its producer's kernel — skip its dispatch entirely.
        if let Some(s) = schedule {
            if s.fusions.iter().any(|f| f.consumer == *name) {
                out.push_str(&format!(
                    "    // node '{}' fused into its producer's epilogue (skipped)\n",
                    name
                ));
                continue;
            }
        }
        let name = &t.name;
        let mut pre = String::new();
        emit_scalar_read(&t.contract.pre_condition, &fields, &consts, &mut pre)?;
        let kidx = kernels.iter().position(|k| k.name == *name);
        if let Some(k) = kidx.map(|i| &kernels[i]) {
            // KERNEL node: dispatch + counter fast-forward (the pass covers
            // every work item, so `i = N` makes the pre false next pass).
            emit_kernel_node(&mut out, t, k, kidx.unwrap(), &fields, &consts);
            continue;
        }
        // HOST node: scalar body.
        let mut body = String::new();
        let mut exited = false;
        for s in &t.body {
            emit_host_stmt(s, &fields, &consts, &mut body, &mut exited)?;
            if exited {
                break;
            }
        }
        out.push_str(&format!("    // host node '{}'\n", name));
        out.push_str(&format!("    if ({}) {{\n", pre));
        out.push_str(&format!("      fired = 1;\n{}\n", body));
        if exited {
            out.push_str("      goto done;\n");
            done_label_used = true;
        }
        out.push_str("    }\n");
    }
    out.push_str("    if (!fired) break;\n");
    out.push_str("  }\n");
    if done_label_used {
        out.push_str("done:\n");
    }
    // Observability: dump scalar state.
    for f in &fields {
        if f.is_array {
            continue;
        }
        if f.type_is_float {
            out.push_str(&format!(
                "  printf(\"{} = %f\\n\", S_{});\n",
                f.name,
                c_ident(&f.name)
            ));
        } else {
            out.push_str(&format!(
                "  printf(\"{} = %lld\\n\", S_{});\n",
                f.name,
                c_ident(&f.name)
            ));
        }
    }
    out.push_str("  briev_accel_shutdown();\n  return 0;\n}\n");
    Ok(out)
}

/// Build the per-kernel list for emit_runner: one module per eligible node
/// (entry "main"), plus its index var and work-item count.
pub fn build_kernels(
    program: &[TopLevel],
    universe: &TypeUniverse,
    int_bits: u64,
    entries: &std::collections::HashMap<String, AccelEntry>,
    image_plans: &std::collections::HashMap<
        String,
        Vec<crate::analysis::image_storage::ImageStoragePlan>,
    >,
) -> Result<Vec<RunnerKernel>, String> {
    let mut out = Vec::new();
    // .abv is PURE GPU: every eligible body is a kernel. The Gpu/Probe/Cpu
    // decision (a .bv offload concept — it compares against a CPU lane) does
    // not apply to a standalone volume with no CPU.
    let _ = AccelDecision::Cpu;
    let mut names: Vec<&String> = entries
        .iter()
        .filter(|(_, e)| e.shape.eligible)
        .map(|(n, _)| n)
        .collect();
    names.sort();
    for name in names {
        let e = &entries[name];
        let mut sb = SpirvBuilder::new().with_universe(universe, int_bits);
        let cooperative = crate::backend::spirv::kernel::is_cooperative_shape(&e.shape);
        let plan = crate::backend::spirv::gemm::GemmPlan::match_stmts(&e.shape, program);
        let tiled = plan.is_some();
        let tensor = tiled
            && crate::config_tuning::ir_lowering().spirv_coopmat
            && plan.as_ref().map_or(false, |p| p.tensor_tier_eligible())
            && {
                // f16 operands (same shape_of check as the kernel hook —
                // rule 19: through the casting graph, never a name match).
                let mut sb = crate::backend::spirv::SpirvBuilder::new()
                    .with_universe(universe, int_bits);
                let sfields = crate::backend::spirv::lower::collect_state_fields(program);
                let elem = |name: &str| -> Option<Type> {
                    sfields.iter().find(|f| f.name == name)
                        .and_then(|f| match &f.ty {
                            Type::Vector(inner, _) => Some((**inner).clone()),
                            other => Some(other.clone()),
                        })
                };
                match (elem("a"), elem("b"), elem("y")) {
                    (Some(ae), Some(be), Some(ye)) => crate::backend::spirv::gemm::fields_are_f16(&mut sb, &ae, &be, &ye),
                    _ => false,
                }
            };
        let kplans: Vec<crate::analysis::image_storage::ImageStoragePlan> =
            image_plans.get(name).cloned().unwrap_or_default();
        crate::backend::spirv::kernel::emit_kernel(
            &mut sb, "main", &e.shape, program, cooperative, &kplans,
        )?;
        out.push(RunnerKernel {
            name: name.clone(),
            spirv: sb.build()?,
            image_plans: kplans,
            index_var: e.shape.index_var.clone(),
            count_expr: e.shape.count_expr.clone().unwrap_or(Expr::Decimal(0)),
            work_cols: e.shape.work_cols,
            cooperative,
            tiled,
            tensor,
            // The same clamp the kernel emitter used — one source of truth.
            tensor_tile_rows: if tensor {
                plan.as_ref()
                    .map(|p| crate::backend::spirv::gemm::GemmPlan::coopmat_tile_rows(p.m))
                    .unwrap_or(1)
            } else {
                1
            },
            ptx_tensor: false,
            block_threads: 64,
            shared_bytes: 0,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod runner_tests {
    use super::*;
    use std::collections::HashMap;

    #[allow(dead_code)]
    fn scalar_field(name: &str, offset: u64) -> RunnerField {
        RunnerField {
            name: name.to_string(),
            offset,
            proj_offset: offset,
            elem_bytes: 8,
            count: 1,
            is_array: false,
            type_is_float: false,
        }
    }

    fn lt(lhs: &str, rhs: &str) -> Expr {
        Expr::BinaryOp(
            BinaryOpKind::Lt,
            Box::new(Expr::Identifier(lhs.to_string())),
            Box::new(Expr::Identifier(rhs.to_string())),
        )
    }

    // TEMP: 2026-08-31: regression guard for the multi-const runner
    // fast-forward repro (handoff §5.5: "N2 resolved as NB"). Verified
    // non-reproducing end-to-end; these tests pin the correct behavior.
    // Remove when the runner's const handling gains a real proof pass.
    #[test]
    fn multi_const_bounds_resolve_each_const_distinctly() {
        let fields = vec![scalar_field("i", 0), scalar_field("j", 8)];
        let mut consts: HashMap<String, Expr> = HashMap::new();
        consts.insert("NB".to_string(), Expr::Decimal(4096));
        consts.insert("N2".to_string(), Expr::Decimal(16777216));

        let mut out = String::new();
        emit_scalar_read(&lt("i", "N2"), &fields, &consts, &mut out).unwrap();
        assert!(out.contains("16777216"), "N2 misresolved: {out}");
        assert!(!out.contains("4096"), "N2 resolved as NB: {out}");

        let mut out = String::new();
        emit_scalar_read(&lt("j", "NB"), &fields, &consts, &mut out).unwrap();
        assert!(out.contains("4096"), "NB misresolved: {out}");
        assert!(!out.contains("16777216"), "NB resolved as N2: {out}");
    }

    #[test]
    fn unknown_const_is_a_named_error_not_a_wrong_value() {
        let fields = vec![scalar_field("i", 0)];
        let consts: HashMap<String, Expr> = HashMap::new();
        let mut out = String::new();
        let err = emit_scalar_read(&lt("i", "N2"), &fields, &consts, &mut out)
            .expect_err("unknown const must error");
        assert!(err.contains("N2"), "error must name the const: {err}");
    }
}

/// Emit one KERNEL scheduler node: the pre-condition gate, the geometry
/// dispatch (see `dispatch_geometry_stmt`), and the counter fast-forward
/// (the pass covers every work item, so `i = N` makes the pre false next
/// pass).
fn emit_kernel_node(
    out: &mut String,
    t: &crate::ast::top::Transaction,
    k: &RunnerKernel,
    kidx: usize,
    fields: &[RunnerField],
    consts: &std::collections::HashMap<String, Expr>,
) {
    let name = &t.name;
    let mut pre = String::new();
    emit_scalar_read(&t.contract.pre_condition, fields, consts, &mut pre)
        .expect("kernel pre-condition lowers");
    let mut count_c = String::new();
    emit_scalar_read(&k.count_expr, fields, consts, &mut count_c)
        .expect("kernel count lowers");
    let ci = c_ident(name);
    out.push_str(&format!("    // kernel node '{}'\n", name));
    out.push_str(&format!("    if ({}) {{\n", pre));
    out.push_str(&format!(
        "      fired = 1;\n      long long n_{} = {};\n",
        ci, count_c
    ));
    out.push_str(&dispatch_geometry_stmt(k, kidx, &ci));
    out.push_str(&format!("      S_{} = n_{};\n", c_ident(&k.index_var), ci));
    out.push_str("    }\n");
}

/// The C dispatch statement for one kernel node, by blob geometry
/// (plan 2026-08-31-gpu-next §2b + 2026-09-01-cooperative-row-kernels):
/// cooperative rows (32 lanes × rows), 2D cols×rows, or the flat 1D
/// fallback. Coverage is identical in all three; only the hardware routing
/// of the work-item id differs.
fn dispatch_geometry_stmt(k: &RunnerKernel, kidx: usize, ci: &str) -> String {
    if k.ptx_tensor {
        if k.block_threads > 64 {
            // PTX tensor multi-warp (mw/nw kernel): block_threads-thread
            // blocks. CTA tile = mw*32 rows × nw*64 cols = mw*nw*2048
            // elements. Each thread covers 64 elements. count = M*N.
            // nx = count/64 → gx = count/(64*block_threads) =
            // count/(2048*mw*nw) = correct CTA count. The kernel decodes
            // m_cta and n_cta from ctaid.x.
            return format!(
                "      if (n_{ci} > 0 && !briev_accel_launch_resident_2d({kidx}, state, n_{ci} / 64, 1)) {{ fprintf(stderr, \"briev: dispatch failed\\n\"); return 1; }}\n"
            );
        }
        // PTX tensor warp-tile (S3b): one 32×16 C tile per 32-lane block,
        // decoded from ctaid.y. count = M*N work items (the counter
        // fast-forward), blocks = count/(32*16). nx=32 → the driver's
        // launch_dev2d dispatches exactly ny blocks.
        return format!(
            "      if (n_{ci} > 0 && !briev_accel_launch_resident_2d({kidx}, state, 32, n_{ci} / (32 * 16))) {{ fprintf(stderr, \"briev: dispatch failed\\n\"); return 1; }}\n"
        );
    }
    if k.tensor {
        // Tensor GEMM (R 16-row strips × 64 cols per workgroup, R =
        // k.tensor_tile_rows — the SAME clamp the kernel emitter used):
        // workgroups = (M/(16R)) * (N/64) = n / (16R*64), items =
        // workgroups * 32 (the driver divides nx by local_x = 32).
        // 2026-09-02: this was the v1 16×16-tile formula (n/(16*16)) —
        // 4× over-dispatch; the extra workgroups decoded out-of-range
        // tiles and smeared garbage over correct tiles' outputs — the
        // tensor tier's "zero/garbage y" device symptom. Undo: restore
        // (16 * 16).
        let r = k.tensor_tile_rows;
        return format!(
            "      long long w_{ci} = (n_{ci} / (16 * {r} * 64)) * 32;\n      if (w_{ci} > 0 && !briev_accel_launch_resident({kidx}, state, w_{ci})) {{ fprintf(stderr, \"briev: dispatch failed\\n\"); return 1; }}\n"
        );
    }
    if k.tiled {
        // Tiled GEMM: workgroups = items / (64*64), nx items = workgroups*16
        // (the driver's launch_dev2d divides nx by the module's local_x 16,
        // restoring the workgroup count — see gemm.rs grid contract).
        return format!(
            "      long long g_{ci} = (n_{ci} / (64 * 64)) * 16;\n      if (g_{ci} > 0 && !briev_accel_launch_resident({kidx}, state, g_{ci})) {{ fprintf(stderr, \"briev: dispatch failed\\n\"); return 1; }}\n"
        );
    }
    if k.cooperative {
        // One 32-lane workgroup per row. The driver's 2D launch takes
        // (nx = x work items, ny = workgroup rows) and dispatches
        // ceil(nx/local_x) * ny workgroups — with the kernel's LocalSize 32
        // and nx = 32 that is exactly ny = n one-per-row workgroups.
        // 2026-09-01: was `(n + 31) / 32` rows, which launched 32x too few
        // workgroups under the local_x-divided geometry (128 of 4096 rows).
        return format!(
            "      if (n_{ci} > 0 && !briev_accel_launch_resident_2d({}, state, 32, n_{ci})) {{ fprintf(stderr, \"briev: dispatch failed\\n\"); return 1; }}\n",
            kidx
        );
    }
    if let Some(cols) = k.work_cols {
        return format!(
            "      long long rows_{ci} = (n_{ci} + {cols} - 1) / {cols};\n      if (n_{ci} > 0 && !briev_accel_launch_resident_2d({}, state, {cols}, rows_{ci})) {{ fprintf(stderr, \"briev: dispatch failed\\n\"); return 1; }}\n",
            kidx
        );
    }
    format!(
        "      if (n_{ci} > 0 && !briev_accel_launch_resident({kidx}, state, n_{ci})) {{ fprintf(stderr, \"briev: dispatch failed\\n\"); return 1; }}\n"
    )
}

/// The in-process run program (plan gpu-backend-hardening Track A): what
/// `brievc run` needs to drive the GPU runtime — kernels, the projection
/// field table, and per-node dispatch geometry — with no C generation.
pub struct RunProgram {
    pub fields: Vec<RunnerField>,
    pub state_bytes: u64,
    pub kernels: Vec<RunKernel>,
}

pub struct RunKernel {
    pub name: String,
    pub spirv: Vec<u8>,
    /// The counter's HOST offset in the state (the node's index-var field).
    pub counter_offset: u64,
    /// The node's work-item bound (const-folded count).
    pub count: i64,
    pub dispatch: RunDispatch,
}

pub enum RunDispatch {
    /// 1D items: flat, tiled (n/4096·16), tensor (n/256·32).
    Items(u64),
    /// 2D cooperative row kernels: nx = 32 lanes, ny = rows.
    Coop { rows: u64 },
    /// 2D column kernels: nx = cols items, ny = row groups.
    Cols { cols: u64, rows: u64 },
}

/// Const-fold a count expression to an integer (module consts only).
pub fn eval_count(expr: &Expr, consts: &HashMap<String, i64>) -> Option<i64> {
    match expr {
        Expr::Decimal(d) => i64::try_from(*d).ok(),
        Expr::Identifier(name) => consts.get(name).copied(),
        Expr::BinaryOp(kind, l, r) => {
            let (a, b) = (eval_count(l, consts)?, eval_count(r, consts)?);
            match kind {
                crate::ast::BinaryOpKind::Add => a.checked_add(b),
                crate::ast::BinaryOpKind::Sub => a.checked_sub(b),
                crate::ast::BinaryOpKind::Mul => a.checked_mul(b),
                crate::ast::BinaryOpKind::Div if b != 0 => a.checked_div(b),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Build the run program from the analyzed .abv items. Mirrors
/// dispatch_geometry_stmt + emit_runner's field table EXACTLY — one source
/// of truth shared with the C runner via the same layout/geometry inputs.
pub fn prepare_run(
    items: &[TopLevel],
    universe: &TypeUniverse,
    int_bits: u64,
    kernels: &[RunnerKernel],
) -> Result<RunProgram, String> {
    let layout = ssbo_layout(
        items,
        universe,
        int_bits,
        &kernels
            .iter()
            .map(|k| (k.name.clone(), k.image_plans.clone()))
            .collect(),
    )?;
    let fields = layout.fields;

    // Module consts (name -> literal) for count folding.
    let mut consts: HashMap<String, i64> = HashMap::new();
    for item in items {
        if let TopLevel::Constant(c) = item {
            if let Expr::Decimal(d) = &c.expr {
                consts.insert(c.name.clone(), *d);
            }
        }
    }

    let mut run_kernels = Vec::new();
    for k in kernels {
        let count = eval_count(&k.count_expr, &consts)
            .ok_or_else(|| format!("node '{}': count is not a compile-time integer", k.name))?;
        // The counter is the node's index-var state field.
        let counter_offset = fields
            .iter()
            .find(|f| f.name == k.index_var)
            .map(|f| f.offset)
            .ok_or_else(|| format!("node '{}': counter field '{}' not in state", k.name, k.index_var))?;

        let dispatch = if k.tensor {
            RunDispatch::Items((count as u64 / 256).max(1) * 32)
        } else if k.tiled {
            RunDispatch::Items((count as u64 / 4096).max(1) * 16)
        } else if k.cooperative {
            RunDispatch::Coop { rows: count as u64 }
        } else if let Some(cols) = k.work_cols {
            let rows = (count as u64 + cols - 1) / cols;
            RunDispatch::Cols { cols, rows }
        } else {
            RunDispatch::Items(count as u64)
        };

        run_kernels.push(RunKernel {
            name: k.name.clone(),
            spirv: k.spirv.clone(),
            counter_offset,
            count,
            dispatch,
        });
    }

    Ok(RunProgram {
        fields,
        state_bytes: layout.state_bytes,
        kernels: run_kernels,
    })
}
