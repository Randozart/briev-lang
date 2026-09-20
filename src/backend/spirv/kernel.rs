/// SPIR-V kernel emission — frontend-driven work-item kernels.
///
/// 2026-08-23 (plan §2.2): kernel selection and body content come from the
/// FRONTEND's accel analysis (`AnalysisResults.accel`, built by
/// src/analysis/accel.rs) — the `[idx < N]` string-sniffing is gone. The
/// analyzed shape provides:
///   - `index_var`:    the state counter that becomes the work-item id
///   - `kernel_stmts`: statements PROVEN safe to offload (pure, affine)
///   - read/write buffers → the StorageBuffer surface
///
/// Structure: a GLCompute invocation IS one work item, so there is no
/// induction loop — `index_var` binds to get_global_id(0) and the host
/// sets dispatch dimensions from N.
use rspirv::dr::{Instruction, Operand};
use rspirv::spirv::{self, Word, ExecutionModel, StorageClass, FunctionControl};
use crate::ast::{Expr, Type};
use crate::backend::spirv::builder::SpirvBuilder;
use crate::backend::spirv::lower::{collect_locals, collect_state_fields, FnLowerer};
use crate::analysis::accel::KernelShape;
use crate::backend::spirv::gemm;
use crate::ast::Statement;

/// Local workgroup size — matches the WorkgroupSize# intrinsic constants.
const LOCAL_SIZE_X: u32 = 256;

/// Per-kernel device surface (Phase 3): the arrays bound as storage images
/// and the slot-alias map (aliased_field → target_field). Bundled so the
/// kernel emitter's signature stays small — surface concerns travel together.
#[derive(Default)]
pub struct KernelSurface<'a> {
    pub images: &'a [crate::analysis::image_storage::ImageStoragePlan],
    pub reuse_map: Option<&'a std::collections::HashMap<String, String>>,
}

/// Emit one GPU kernel from an analyzed shape. Returns the function id.
pub fn emit_kernel(
    builder: &mut SpirvBuilder,
    kernel_name: &str,
    shape: &KernelShape,
    items: &[crate::ast::TopLevel],
    cooperative: bool,
    surface: &KernelSurface,
) -> Result<Word, String> {
    let mut cooperative = cooperative;
    let void_id = builder.lower_type(&Type::void())?;
    let func_type_id = builder.gen_id();
    builder.builder.type_function_id(Some(func_type_id), void_id, []);

    // ── Module globals: state SSBO + invocation-id builtins. Ids thread
    // into the body lowerer so nothing is created twice.
    let state_fields = collect_state_fields(items);
    // 2026-09-02 (plan fundamental-parent-membership): an f16 shape anywhere
    // in the state surface requires the Float16 + 16-bit-storage
    // capabilities BEFORE the SSBO types are lowered — the tiled f16 path
    // (coopmat knob OFF) emits OpTypeFloat 16 members and previously
    // failed spirv-val ("requires the Float16 capability"). Shape-driven
    // via the casting graph, never name-matched. Undo: delete the scan
    // (restores the capability gap on tiled-f16 kernels).
    for f in &state_fields {
        let elem = match &f.ty {
            Type::Vector(inner, _) => Some((**inner).clone()),
            other => Some(other.clone()),
        };
        if let Some(elem) = elem {
            if let Ok(crate::casting::graph::SpirvShape::Float { bits: 16 }) = builder.shape_of(&elem) {
                builder.builder.capability(rspirv::spirv::Capability::Float16);
                builder.builder.capability(rspirv::spirv::Capability::StorageBuffer16BitAccess);
                break;
            }
        }
    }
    // The GEMM plan + tensor-tier decision must precede the state-buffer
    // setup: the D3 pair view (array-of-v2f16, byte-identical) applies to
    // the smem GEMM's fields ONLY — the fallback lanes keep their scalar
    // half views. (2026-09-04, plan 2026-09-04-beyond-coopmat Stage 1.)
    let gemm_plan_early = gemm::GemmPlan::match_stmts(shape, items);
    let pair_view_fields: Vec<(String, u32)> = {
        let tensor = gemm_plan_early.as_ref().map_or(false, |plan| {
            crate::config_tuning::ir_lowering().spirv_coopmat
                && plan.tensor_tier_eligible()
                && gemm::GemmPlan::coopmat_smem()
                && gemm::GemmPlan::coopmat_fill_pairs()
                && {
                    let field_elem = |name: &str| -> Option<Type> {
                        state_fields.iter().find(|f| f.name == name).and_then(|f| match &f.ty {
                            Type::Vector(inner, _) => Some((**inner).clone()),
                            other => Some(other.clone()),
                        })
                    };
                    match (field_elem(&plan.a_field), field_elem(&plan.b_field), field_elem(&plan.y_field)) {
                        (Some(ae), Some(be), Some(ye)) => gemm::fields_are_f16(builder, &ae, &be, &ye),
                        _ => false,
                    }
                }
        });
        if tensor {
            let plan = gemm_plan_early.as_ref().unwrap();
            // D3b: the fill's a/b carry the f16×4 quad view when the
            // quad knob + the ÷4 shape guards hold; y stays at the pair
            // width (the fragment-store path is unchanged).
            let quad = gemm::GemmPlan::coopmat_fill_quad_active(plan);
            let ab_width = if quad { 4 } else { 2 };
            vec![
                (plan.a_field.clone(), ab_width),
                (plan.b_field.clone(), ab_width),
                (plan.y_field.clone(), 2),
            ]
        } else {
            Vec::new()
        }
    };
    let (ssbo_var, global_id_var, local_id_var, workgroup_id_var, vec4_fields, state_fields_sorted, image_vars, image_types, alias_map) = {
        let mut warm = FnLowerer::new(builder, state_fields.clone());
        warm.set_image_plans(surface.images);
        warm.set_pair_view_fields(pair_view_fields);
        warm.materialize_consts(items)?;
        warm.warm_builtins()?;
        warm.setup_state_buffer(surface.reuse_map)?;
        warm.declare_images()?;
        (
            warm.ssbo_var,
            warm.global_id_var,
            warm.local_id_var,
            warm.workgroup_id_var,
            warm.vec4_fields,
            warm.state_fields,
            warm.image_vars,
            warm.image_types,
            warm.alias_map,
        )
    };
    // Types referenced by the function must precede it in the module.

    // ── Tiled GEMM plan + module-scope shared arrays (M2.1) ──
    // Workgroup-class variables are MODULE-GLOBAL in SPIR-V (glslang emits
    // them before the function; the validator rejects them inside it —
    // found on device, plan 2026-09-01-m2-gemm).
    // The GEMM shape match is type-agnostic; the FIELD TYPES decide the
    // tier: Float16 operands (16-bit float shape through the casting
    // graph) + the coopmat knob → tensor fragment kernel; Float32
    // vec4-eligible operands → the shared-memory tiled kernel; anything
    // else → the flat naive kernel. (Plan 2026-09-01-m2-tensor-cores.)
    let gemm_plan = gemm_plan_early;
    let field_elem = |name: &str| -> Option<Type> {
        state_fields.iter().find(|f| f.name == name)
            .and_then(|f| match &f.ty {
                Type::Vector(inner, _) => Some((**inner).clone()),
                other => Some(other.clone()),
            })
    };
    let gemm_tensor = gemm_plan.as_ref().map_or(false, |plan| {
        crate::config_tuning::ir_lowering().spirv_coopmat
            && plan.tensor_tier_eligible()
            && {
            let (ae, be, ye) = (
                field_elem(&plan.a_field),
                field_elem(&plan.b_field),
                field_elem(&plan.y_field),
            );
            match (ae, be, ye) {
                (Some(ae), Some(be), Some(ye)) => {
                    super::gemm::fields_are_f16(builder, &ae, &be, &ye)
                }
                _ => false,
            }
        }
    });
    // Tiled needs vec4-eligible fields (wide shared-memory staging).
    let gemm_tiled: Option<(gemm::GemmPlan, super::lower::Vec4Field, super::lower::Vec4Field, super::lower::Vec4Field)> =
        if gemm_tensor { None } else {
        gemm_plan.clone().and_then(|plan| {
            let a_v4 = vec4_fields.get(&plan.a_field)?.clone();
            let b_v4 = vec4_fields.get(&plan.b_field)?.clone();
            let y_v4 = vec4_fields.get(&plan.y_field)?.clone();
            Some((plan, a_v4, b_v4, y_v4))
        })
        };
    let (shared_a, shared_b) = if gemm_tiled.is_some() {
        let f32_ty = builder.lower_type(&gemm_tiled.as_ref().unwrap().1.elem)?;
        let len_c = builder.u32_const((gemm::TILE * gemm::TILE) as u32);
        let arr_ty = builder.builder.type_array(f32_ty, len_c);
        let wg_ptr = builder.ptr_class(StorageClass::Workgroup, arr_ty);
        let sa = builder.gen_id();
        builder.emit_global(Instruction::new(
            spirv::Op::Variable,
            Some(wg_ptr),
            Some(sa),
            vec![Operand::StorageClass(StorageClass::Workgroup)],
        ));
        let sb = builder.gen_id();
        builder.emit_global(Instruction::new(
            spirv::Op::Variable,
            Some(wg_ptr),
            Some(sb),
            vec![Operand::StorageClass(StorageClass::Workgroup)],
        ));
        (Some(sa), Some(sb))
    } else {
        (None, None)
    };

    // ── Tensor-tier smem staging (2026-09-02): Workgroup arrays for the
    // cooperative matrix double-buffer pipeline.  The tiled tier's
    // `OpTypeArray` + `emit_global` + `OpAccessChain` pattern is reused.
    // Sizes: shared_a = 2×R×256 halves, shared_b = 2×4×256 halves.
    // With the default R=4: 4096 + 4096 = 8192 halves = 16 KB — trivial.
    // Gated on `spirv_coopmat_smem` (2026-09-04): 0 = direct SSBO coopmat
    // loads — the knob existed in the dbvl but nothing read it.
    let (coop_shared_a, coop_shared_b) = if gemm_tensor
        && gemm::GemmPlan::coopmat_smem()
    {
        let plan = gemm_plan.as_ref().unwrap();
        let r = gemm::GemmPlan::coopmat_tile_rows(plan.m);
        let f16_ty = builder.builder.type_float(16);
        // Pairs mode (D3): the smem arrays carry the vNf16 view
        // (byte-identical; the fill stores wide units, the coopmat loads
        // walk [unit_idx, 0] to a half pointer). D3b quad mode widens to
        // v4f16 when the quad knob + the ÷4 shape guards hold.
        let quad = gemm::GemmPlan::coopmat_fill_quad_active(plan);
        let view_width: u32 = if quad {
            4
        } else if gemm::GemmPlan::coopmat_fill_pairs() {
            2
        } else {
            0
        };
        let view_ty = if view_width > 0 {
            Some(builder.builder.type_vector(f16_ty, view_width))
        } else {
            None
        };
        // D1: panels per stage doubles the per-stage footprint.
        let pps = gemm::GemmPlan::coopmat_panels_per_stage(plan.k);
        let subgroups = gemm::GemmPlan::coopmat_subgroups();
        // 2026-09-07: `stages` (1 = single-buffer) scales the footprint —
        // single-buffer halves smem → 2× resident WGs/SM at 48KB. The fill
        // is barrier-serialized with the mma regardless, so the double
        // buffer's spare stage buys no overlap — only smem it occupies.
        let stages = gemm::GemmPlan::coopmat_stages();
        let a_elems = (stages * pps * r * 16 * 16) as u32;  // stages × pps panels × R strips × 256
        let b_elems_one = (stages * pps * 4 * 16 * 16) as u32;  // one subgroup's B: stages × pps × 4 × 256
        let b_elems = b_elems_one * subgroups;  // S subgroups each own a B slice
        let (a_len, b_len) = if view_width > 0 {
            (
                builder.u32_const(a_elems / view_width),
                builder.u32_const(b_elems / view_width),
            )
        } else {
            (builder.u32_const(a_elems), builder.u32_const(b_elems))
        };
        let (a_arr_ty, b_arr_ty) = if let Some(vt) = view_ty {
            (
                builder.builder.type_array(vt, a_len),
                builder.builder.type_array(vt, b_len),
            )
        } else {
            (
                builder.builder.type_array(f16_ty, a_len),
                builder.builder.type_array(f16_ty, b_len),
            )
        };
        let a_ptr_ty = builder.ptr_class(StorageClass::Workgroup, a_arr_ty);
        let b_ptr_ty = builder.ptr_class(StorageClass::Workgroup, b_arr_ty);
        let sa = builder.gen_id();
        builder.emit_global(Instruction::new(
            spirv::Op::Variable,
            Some(a_ptr_ty),
            Some(sa),
            vec![Operand::StorageClass(StorageClass::Workgroup)],
        ));
        let sb = builder.gen_id();
        builder.emit_global(Instruction::new(
            spirv::Op::Variable,
            Some(b_ptr_ty),
            Some(sb),
            vec![Operand::StorageClass(StorageClass::Workgroup)],
        ));
        (Some(sa), Some(sb))
    } else {
        (None, None)
    };

    // All direct-builder work happens BEFORE the body lowerer borrows it:
    // ids, function, entry block, and every function-scope OpVariable (they
    // must be the first instructions of the entry block).
    let func_id = builder.gen_id();
    let entry_id = builder.gen_id();
    let int_ptr = {
        let int_ty = builder.lower_type(&Type::int())?;
        builder.ptr_class(StorageClass::Function, int_ty)
    };
    let index_var = builder.gen_id();

    // 2026-09-17 (M2a): the cooperative row softmax lowers to SYNTHESIZED
    // statements — strided (lane + t*32) index expressions over the row,
    // SubgroupFMax#/SubgroupFAdd# between the phases — replacing the source
    // body BEFORE local collection, so the generic declaration/emission
    // machinery handles everything. The synthesis must precede the entry
    // block's OpVariables; by the emission branch it is too late.
    let shape_owned;
    let shape = if cooperative
        && matches!(
            shape.reduction.as_ref().map(|r| &r.kind),
            Some(crate::analysis::accel::ReductionKind::Softmax)
        ) {
        let red = shape.reduction.as_ref().unwrap();
        // Const resolution: the lowerer's const map is built later
        // (materialize_consts) — resolve here from the program's constants.
        let consts = crate::backend::ptx::module_expr_consts(items);
        let inner_len = match &red.inner {
            Expr::Identifier(name) => match consts.get(name) {
                Some(Expr::Decimal(n)) => *n,
                _ => {
                    return Err(format!(
                        "softmax row length '{}' is not a literal const",
                        name
                    ))
                }
            },
            Expr::Decimal(n) => *n,
            other => {
                return Err(format!(
                    "softmax row length {:?} must be a literal const",
                    other
                ))
            }
        };
        if inner_len <= 0 || inner_len % 32 != 0 {
            return Err(format!(
                "cooperative softmax needs a row length divisible by 32 (got {})",
                inner_len
            ));
        }
        shape_owned = synthesize_softmax_stmts(shape, red, inner_len as u64);
        &shape_owned
    } else {
        shape
    };

    let mut collected: Vec<(String, Type)> = Vec::new();
    collect_locals(&shape.kernel_stmts, &mut collected);
    // 2026-09-02: Vulkan forbids 16-bit-typed variables in Function
    // storage outright — 16-bit floats are a STORAGE format, not a compute
    // format. Widen f16 locals to f32: the body computes in f32 (the
    // constant pool and the widened SSBO loads are f32), and Assign
    // coerces back to the member shape at the SSBO boundary. This mirrors
    // the tensor path's fp32-accumulate semantics and makes the tiled-f16
    // kernel spirv-val clean (the OpStore width mismatch). Undo: restore
    // raw `ty` in the tuple below.
    let local_vars: Vec<(String, Word, Type)> = collected
        .into_iter()
        .map(|(name, ty)| {
            let storage_ty = match builder.shape_of(&ty) {
                Ok(crate::casting::graph::SpirvShape::Float { bits: 16 }) => Type::float(),
                _ => ty,
            };
            let elem = builder.lower_type(&storage_ty)?;
            let ptr = builder.ptr_class(StorageClass::Function, elem);
            let var = builder.gen_id();
            Ok((name, var, storage_ty))
        })
        .collect::<Result<Vec<_>, String>>()?;

    builder.begin_function(void_id, func_id, FunctionControl::empty(), func_type_id);
    builder.begin_block(Some(entry_id));
    builder.instr(
        spirv::Op::Variable,
        Some(int_ptr),
        Some(index_var),
        vec![Operand::StorageClass(StorageClass::Function)],
    );
    for (_, var, ty) in &local_vars {
        // 2026-09-01 (M2.2): the tensor kernel uses none of the .abv body's
        // locals — it synthesizes its own accumulator fragments. Skip them
        // on the tensor path. (Since 2026-09-02 the locals are f32-widened,
        // so the non-tensor path emits legal Function-storage variables.)
        if gemm_tensor {
            continue;
        }
        let elem = builder.lower_type(ty)?;
        let ptr = builder.ptr_class(StorageClass::Function, elem);
        builder.instr(
            spirv::Op::Variable,
            Some(ptr),
            Some(*var),
            vec![Operand::StorageClass(StorageClass::Function)],
        );
    }

    if let Some(plan) = &gemm_plan {
        if gemm_tensor {
            // Tensor tier: cooperative-matrix fragments, one 16×16 tile per
            // warp-sized workgroup. No shared memory, no vec4 machinery.
            // Phase 3: member indices count only NON-aliased fields (aliased
            // members are skipped from the SSBO struct).
            let member_of = |name: &str| -> Option<usize> {
                let eff = alias_map.get(name).map(|s| s.as_str()).unwrap_or(name);
                let pos = state_fields_sorted.iter().position(|f| f.name == eff)?;
                Some(
                    state_fields_sorted[..pos]
                        .iter()
                        .filter(|f| !alias_map.contains_key(&f.name))
                        .count(),
                )
            };
            let exit_bb = builder.gen_id();
            // B2 (plan 2026-09-02-cuda-race): S subgroups per workgroup —
            // each owns its tile_n slice via the SubgroupId builtin. S=1
            // declares no input at all (the historical module shape).
            let subgroups = gemm::GemmPlan::coopmat_subgroups();
            // B2: SubgroupId without the builtin — NVVM rejected
            // SubgroupId outright and LocalInvocationIndex is i64 (UDiv
            // wants unsigned). The LINEAR local index is y*LocalSizeX + x,
            // so subgroup = y*4 + x/32 — exact on every 32-lane target.
            let sub_id_var = if subgroups > 1 {
                let lid_ptr = local_id_var.ok_or("gemm without LocalInvocationId")?;
                let u32_ty = builder.u32_type();
                let lid = builder.gen_id();
                let v3_ty = builder.builder.type_vector(u32_ty, 3);
                builder.emit(Instruction::new(
                    spirv::Op::Load,
                    Some(v3_ty),
                    Some(lid),
                    vec![Operand::IdRef(lid_ptr)],
                ));
                let ly = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::CompositeExtract,
                    Some(u32_ty),
                    Some(ly),
                    vec![Operand::IdRef(lid), Operand::LiteralBit32(1)],
                ));
                let lx = builder.gen_id();
                builder.emit(Instruction::new(
                    spirv::Op::CompositeExtract,
                    Some(u32_ty),
                    Some(lx),
                    vec![Operand::IdRef(lid), Operand::LiteralBit32(0)],
                ));
                let c4 = super::gemm::u32_const(builder, 4);
                let y4 = super::gemm::u32_binop(builder, spirv::Op::IMul, ly, c4);
                let x32 = super::gemm::u32_shr(builder, lx, 5);
                Some(super::gemm::u32_binop(builder, spirv::Op::IAdd, y4, x32))
            } else {
                None
            };
            let coopmat_args = gemm::CoopMatIo {
                ssbo: ssbo_var.ok_or("gemm without SSBO")?,
                wgid: workgroup_id_var.ok_or("gemm without WorkGroupId")?,
                sub_id: sub_id_var.unwrap_or(Word::MAX),
                a_member: member_of(&plan.a_field).ok_or("gemm a field not in state")? as u32,
                b_member: member_of(&plan.b_field).ok_or("gemm b field not in state")? as u32,
                y_member: member_of(&plan.y_field).ok_or("gemm y field not in state")? as u32,
                shared_a: coop_shared_a,
                shared_b: coop_shared_b,
                lane_id: {
                    let lid_ptr = local_id_var.ok_or("gemm coopmat without LocalInvocationId")?;
                    let u32_ty = builder.u32_type();
                    let lid = builder.gen_id();
                    let v3_ty = builder.builder.type_vector(u32_ty, 3);
                    builder.emit(Instruction::new(
                        spirv::Op::Load,
                        Some(v3_ty),
                        Some(lid),
                        vec![Operand::IdRef(lid_ptr)],
                    ));
                    let lane = builder.gen_id();
                    builder.emit(Instruction::new(
                        spirv::Op::CompositeExtract,
                        Some(u32_ty),
                        Some(lane),
                        vec![Operand::IdRef(lid), Operand::LiteralBit32(0)],
                    ));
                    lane
                },
            };
            gemm::emit_coopmat(builder, plan, &coopmat_args, exit_bb)?;
            builder.begin_block(Some(exit_bb));
            builder.ret();
            builder.end_function();
            let interface: Vec<Word> = [global_id_var, local_id_var, workgroup_id_var]
                .into_iter()
                .flatten()
                .chain(ssbo_var.into_iter())
                .chain(coop_shared_a.into_iter())
                .chain(coop_shared_b.into_iter())
                .chain(image_vars.values().copied())
                .collect();
            builder.set_entry_point(func_id, kernel_name, ExecutionModel::GLCompute, &interface);
            builder.add_execution_mode(
                func_id,
                spirv::ExecutionMode::LocalSize,
                32 * subgroups,
                1,
                1,
            );
            return Ok(func_id);
        }
    }

    if let (Some((plan, a_v4, _b_v4, _y_v4)), Some(shared_a), Some(shared_b)) =
        (&gemm_tiled, shared_a, shared_b)
    {
        let f32_ty = builder.lower_type(&a_v4.elem)?;
        let member_of = |name: &str| -> Option<usize> {
            let eff = alias_map.get(name).map(|s| s.as_str()).unwrap_or(name);
            let pos = state_fields_sorted.iter().position(|f| f.name == eff)?;
            Some(
                state_fields_sorted[..pos]
                    .iter()
                    .filter(|f| !alias_map.contains_key(&f.name))
                    .count(),
            )
        };
        let ctx = gemm::TiledCtx {
            ssbo: ssbo_var.ok_or("gemm without SSBO")?,
            wgid: workgroup_id_var.ok_or("gemm without WorkGroupId")?,
            lid: local_id_var.ok_or("gemm without LocalInvocationId")?,
            shared_a,
            shared_b,
            f32_ty,
            a_member: member_of(&plan.a_field).ok_or("gemm a field not in state")? as u32,
            b_member: member_of(&plan.b_field).ok_or("gemm b field not in state")? as u32,
            y_member: member_of(&plan.y_field).ok_or("gemm y field not in state")? as u32,
            exit_bb: builder.gen_id(),
        };
        gemm::emit_tiled(builder, &plan, &ctx)?;
        builder.begin_block(Some(ctx.exit_bb));
        builder.ret();
        builder.end_function();
        // SPIR-V 1.4+: the interface lists EVERY global the shader touches —
        // including the Workgroup shared arrays.
        let interface: Vec<Word> = [global_id_var, local_id_var, workgroup_id_var]
            .into_iter()
            .flatten()
            .chain(ssbo_var.into_iter())
            .chain([shared_a, shared_b])
            .chain(image_vars.values().copied())
            .collect();
        builder.set_entry_point(func_id, kernel_name, ExecutionModel::GLCompute, &interface);
        builder.add_execution_mode(
            func_id,
            spirv::ExecutionMode::LocalSize,
            gemm::THREADS as u32,
            gemm::THREADS as u32,
            1,
        );
        return Ok(func_id);
    }

    let mut lower = FnLowerer::new(builder, state_fields);
    lower.set_image_plans(surface.images);
    lower.image_vars = image_vars.clone();
    lower.image_types = image_types;
    lower.ssbo_var = ssbo_var;
    lower.global_id_var = global_id_var;
    lower.local_id_var = local_id_var;
    lower.vec4_fields = vec4_fields;
    lower.alias_map = alias_map;
    lower.materialize_consts(items)?;
    lower
        .vars
        .insert(shape.index_var.clone(), (index_var, Type::int()));
    for (name, var, ty) in &local_vars {
        lower.vars.insert(name.clone(), (*var, ty.clone()));
    }

    // 2026-09-01 (M2.0 hole 2, plan m2-gemm): the cooperative row form
    // requires the item id to BE the row (y[i], a[i*K + k]). When the body
    // DECOMPOSES the counter (m = i / N, n = i % N — a flattened 2D shape),
    // binding row = gid>>5 would compute each output element 32x (once per
    // lane) with a wrong work mapping. Div/mod of the index var anywhere in
    // the kernel statements → flat path. MUST run before the binding.
    if cooperative && kernel_stmts_decompose_counter(shape) {
        cooperative = false;
    }

    if cooperative {
        // Row = gid.y; the lane is GetGlobalId#(0) inside the body.
        bind_work_item_row(&mut lower, index_var)?;
    } else {
        bind_work_item_index(&mut lower, index_var, shape.work_cols);
    }

    // 2026-08-31 (plan abv-gpu-by-default): BOUNDS GUARD. The host dispatches
    // ceil(N / LocalSize) workgroups, so up to LocalSize-1 extra invocations
    // run; each must exit before touching state when its global id exceeds
    // the work-item count (a runtime field or a literal — exactly the bound
    // the eligibility proof extracted from `[i < N]`).
    //
    // 2D (plan 2026-08-31-gpu-next §2b): the flat-launch tail argument only
    // holds when N is a multiple of the workgroup size. With 2D geometry the
    // tail can reach cols-1 items, and even a literal count need not be a
    // multiple — so a 2D shape ALWAYS carries the guard. (Found while
    // wiring this up: a literal count not divisible by 64 had the same hole
    // in pure 1D; the literal%64 check closes that too.)
    let count_is_literal = matches!(shape.count_expr, Some(Expr::Decimal(_)));
    let count_multiple_of_workgroup = match shape.count_expr {
        Some(Expr::Decimal(n)) => n % LOCAL_SIZE_X as i64 == 0,
        _ => false,
    };
    // 2026-09-17 (M2a correction): cooperative row kernels dispatch EXACTLY
    // (ny=rows workgroups × 32 lanes) — every invocation is a valid
    // (row, lane) pair, and the flat count (rows) would wrongly kill
    // lanes ≥ rows of every workgroup. No guard on the cooperative path.
    let needs_guard = if cooperative {
        false
    } else {
        !count_is_literal || shape.work_cols.is_some() || !count_multiple_of_workgroup
    };
    let exit_bb = lower.builder.gen_id();
    if needs_guard {
        let body_bb = lower.builder.gen_id();
        let bound_expr = shape
            .count_expr
            .clone()
            .unwrap_or(Expr::Decimal(0));
        let (bound, _bty) = lower.emit_expr(&bound_expr)?;
        let int_ty = lower.builder.lower_type(&Type::int())?;
        let gid_reg = lower.builder.gen_id();
        lower.builder.emit(Instruction::new(
            spirv::Op::Load,
            Some(int_ty),
            Some(gid_reg),
            vec![Operand::IdRef(index_var)],
        ));
        let bool_ty = lower.builder.lower_type(&Type::Bits(1))?;
        let in_bounds = lower.builder.gen_id();
        lower.builder.emit(Instruction::new(
            spirv::Op::ULessThan,
            Some(bool_ty),
            Some(in_bounds),
            vec![Operand::IdRef(gid_reg), Operand::IdRef(bound)],
        ));
        // Vulkan requires structured selection: OpSelectionMerge before the
        // conditional branch.
        lower
            .builder
            .builder
            .selection_merge(exit_bb, rspirv::spirv::SelectionControl::NONE);
        lower
            .builder
            .builder
            .branch_conditional(in_bounds, body_bb, exit_bb, [] as [u32; 0]);
        lower.builder.begin_block(Some(body_bb));
    }

    // 2026-09-17 (M2a): the softmax body was already synthesized into plain
    // statements before local collection — it lowers through the generic
    // emit_stmt path below. Only the DOT shape takes the fused reduce
    // lowering.
    let dot_cooperative = cooperative
        && matches!(
            shape.reduction.as_ref().map(|r| &r.kind),
            Some(crate::analysis::accel::ReductionKind::Dot)
        );
    if dot_cooperative {
        let red = shape
            .reduction
            .as_ref()
            .ok_or("cooperative kernel without a recognized reduction")?;
        let inner_len = match &red.inner {
            Expr::Identifier(name) => *lower
                .const_int_values
                .get(name)
                .ok_or_else(|| format!("reduction length '{}' is not a literal const", name))?,
            Expr::Decimal(n) => *n,
            other => {
                return Err(format!(
                    "reduction length {:?} must be a literal const for the cooperative path",
                    other
                ))
            }
        };
        if inner_len <= 0 || inner_len % 32 != 0 {
            return Err(format!(
                "cooperative reduction needs a length divisible by 32 (got {})",
                inner_len
            ));
        }
        emit_cooperative_reduce(&mut lower, shape, inner_len as u64)?;
    } else {
        for stmt in &shape.kernel_stmts {
            if lower.terminated {
                break;
            }
            lower.emit_stmt(stmt)?;
        }
    }

    lower.builder.builder.branch(exit_bb);
    lower.builder.begin_block(Some(exit_bb));
    builder.ret();
    builder.end_function();

    // Entry-point interface lists every Input/Output + SSBO variable.
    let interface: Vec<Word> = [global_id_var, local_id_var]
        .into_iter()
        .flatten()
        .chain(ssbo_var.into_iter())
        .chain(image_vars.values().copied())
        .collect();
    builder.set_entry_point(func_id, kernel_name, ExecutionModel::GLCompute, &interface);
    // Cooperative row kernels (plan 2026-09-01-cooperative-row-kernels):
    // ONE 32-lane workgroup per row — the subgroup IS the row's team.
    let local_x = if cooperative { 32 } else { LOCAL_SIZE_X };
    builder.add_execution_mode(
        func_id,
        spirv::ExecutionMode::LocalSize,
        local_x,
        1,
        1,
    );

    Ok(func_id)
}

/// Synthesize the cooperative row-softmax body (2026-09-17, M2a): plain
/// statements — strided (lane + t*32) row reads, `Max#` fold, `Exp#`-sum,
/// normalize, with `SubgroupFMax#`/`SubgroupFAdd#` warp reductions between
/// the phases (SPIR-V GroupNonUniform Reduce — the result is uniform across
/// the subgroup, so no barrier is needed). The generic machinery (local
/// collection/declaration, emit_stmt, the cooperative row binding) handles
/// the rest. Requires inner % 32 == 0 (enforced by the caller) so strided
/// accesses are exactly in-bounds — no bounds guards.
fn synthesize_softmax_stmts(
    shape: &KernelShape,
    red: &crate::analysis::accel::ReductionInfo,
    inner_len: u64,
) -> KernelShape {
    use crate::ast::BinaryOpKind::{Add, BitAnd, Div, Mul, Sub};
    let mut out = shape.clone();
    let gid0 = || {
        Expr::Call("GetGlobalId#".into(), vec![Expr::Decimal(0)], None)
    };
    let lane = Expr::BinaryOp(
        BitAnd,
        Box::new(gid0()),
        Box::new(Expr::Decimal(31)),
    );
    let base = Expr::BinaryOp(
        Mul,
        Box::new(Expr::Identifier(shape.index_var.clone())),
        Box::new(Expr::Decimal(inner_len as i64)),
    );
    // element index = base + lane + t*32
    let elem = |t: Expr| -> Expr {
        Expr::BinaryOp(
            Add,
            Box::new(base.clone()),
            Box::new(Expr::BinaryOp(
                Add,
                Box::new(lane.clone()),
                Box::new(Expr::BinaryOp(Mul, Box::new(t), Box::new(Expr::Decimal(32)))),
            )),
        )
    };
    let row_read = |t: Expr| -> Expr {
        Expr::Index(
            Box::new(Expr::Identifier(red.row_buf.clone())),
            Box::new(elem(t)),
        )
    };
    let out_write = |t: Expr| -> Expr {
        Expr::Index(
            Box::new(Expr::Identifier(red.out_buf.clone())),
            Box::new(elem(t)),
        )
    };
    let fty = Some(crate::ast::Type::float());
    let tiles = (inner_len / 32) as i64;
    let id = |n: &str| Expr::Identifier(n.into());
    let m = "briev_sm_m";
    let s = "briev_sm_s";
    let v = "briev_sm_v";
    let t = "briev_sm_t";
    let range = |a: i64, b: i64| Expr::Range {
        start: Box::new(Expr::Decimal(a)),
        end: Box::new(Expr::Decimal(b)),
        inclusive: false,
    };

    let mut stmts: Vec<Statement> = Vec::new();
    // Phase 1 — row max (the t=0 round duplicates the init; Max# idempotent)
    stmts.push(Statement::Let {
        name: m.into(),
        names: vec![],
        ty: fty.clone(),
        expr: Some(row_read(Expr::Decimal(0))),
        modifiers: vec![],
    });
    stmts.push(Statement::Foreach {
        item: t.into(),
        list: Box::new(range(0, tiles)),
        body: vec![
            Statement::Let {
                name: v.into(),
                names: vec![],
                ty: fty.clone(),
                expr: Some(row_read(id(t))),
                modifiers: vec![],
            },
            Statement::Assign(
                id(m),
                Expr::Call("Max#".into(), vec![id(m), id(v)], None),
            ),
        ],
    });
    stmts.push(Statement::Assign(
        id(m),
        Expr::Call("SubgroupFMax#".into(), vec![id(m)], None),
    ));
    // Phase 2 — exp-sum
    stmts.push(Statement::Let {
        name: s.into(),
        names: vec![],
        ty: fty.clone(),
        expr: Some(Expr::Decimal(0)),
        modifiers: vec![],
    });
    stmts.push(Statement::Foreach {
        item: t.into(),
        list: Box::new(range(0, tiles)),
        body: vec![Statement::Assign(
            id(s),
            Expr::BinaryOp(
                Add,
                Box::new(id(s)),
                Box::new(Expr::Call(
                    "Exp#".into(),
                    vec![Expr::BinaryOp(Sub, Box::new(row_read(id(t))), Box::new(id(m)))],
                    None,
                )),
            ),
        )],
    });
    stmts.push(Statement::Assign(
        id(s),
        Expr::Call("SubgroupFAdd#".into(), vec![id(s)], None),
    ));
    // Phase 3 — normalize
    stmts.push(Statement::Foreach {
        item: t.into(),
        list: Box::new(range(0, tiles)),
        body: vec![Statement::Assign(
            out_write(id(t)),
            Expr::BinaryOp(
                Div,
                Box::new(Expr::Call(
                    "Exp#".into(),
                    vec![Expr::BinaryOp(Sub, Box::new(row_read(id(t))), Box::new(id(m)))],
                    None,
                )),
                Box::new(id(s)),
            ),
        )],
    });

    out.kernel_stmts = stmts;
    out
}

/// BIND the work-item index (2026-08-31, plan abv-gpu-by-default): the doc
/// once claimed "index_var binds to get_global_id(0)" but nothing stored it —
/// the standalone kernels were never executed, so every invocation read an
/// undefined index. Widening u32→i64 mirrors the builtin path.
///
/// 2D (plan 2026-08-31-gpu-next §2b): when the shape carries a dispatch
/// width, reconstruct `i = gid.y * cols + gid.x`. The values are IDENTICAL
/// to a 1D linearization for every covered item, so any launcher that
/// covers the total count stays correct (a flat 1D launch has gid.y == 0
/// and gid.x spanning the count); the 2D shape exists so the launcher can
/// hand the row/col split to the hardware.
fn bind_work_item_index(
    lower: &mut FnLowerer,
    index_var: spirv::Word,
    work_cols: Option<u64>,
) -> Result<(), String> {
    let idx_expr = match work_cols {
        Some(cols) if cols > 1 => Expr::BinaryOp(
            crate::ast::BinaryOpKind::Add,
            Box::new(Expr::BinaryOp(
                crate::ast::BinaryOpKind::Mul,
                Box::new(Expr::Call(
                    "GetGlobalId#".into(),
                    vec![Expr::Decimal(1)],
                    None,
                )),
                Box::new(Expr::Decimal(cols as i64)),
            )),
            Box::new(Expr::Call(
                "GetGlobalId#".into(),
                vec![Expr::Decimal(0)],
                None,
            )),
        ),
        _ => Expr::Call("GetGlobalId#".into(), vec![Expr::Decimal(0)], None),
    };
    let (gid64, _t) = lower.emit_expr(&idx_expr)?;
    lower.builder.store(index_var, gid64);
    Ok(())
}

/// Cooperative row kernels (plan 2026-09-01-cooperative-row-kernels): bind
/// the work-item index to `GetGlobalId#(1)` — the ROW. The lane is
/// `GetGlobalId#(0)`, referenced inside the synthesized body.
fn bind_work_item_row(lower: &mut FnLowerer, index_var: spirv::Word) -> Result<(), String> {
    // The runner FLATTENS the cooperative grid into X (vkCmdDispatch
    // (groups_x*ny, 1, 1) — the 09-01 Y-inert diagnosis still holds on
    // 615.71.09), so the row is gid.x >> 5 and the lane is gid.x & 31.
    // 2026-09-17: the real cooperative regression was the BOUNDS GUARD —
    // it compared the flat count (rows) against the flattened gid, killing
    // every workgroup past the first. The guard is now skipped for
    // cooperative kernels (exact dispatch); this binding was never wrong.
    let (gid64, _t) = lower.emit_expr(&Expr::BinaryOp(
        crate::ast::BinaryOpKind::Shr,
        Box::new(Expr::Call("GetGlobalId#".into(), vec![Expr::Decimal(0)], None)),
        Box::new(Expr::Decimal(5)),
    ))?;
    lower.builder.store(index_var, gid64);
    Ok(())
}

/// Shared context for the cooperative vec4 loop emission — bundles the
/// per-function state so helpers stay under the parameter-count limit.
struct Vec4LoopCtx<'a> {
    item: &'a str,
    repl: &'a Expr,
    fbody: &'a [Statement],
    field_data: &'a [(String, crate::backend::spirv::lower::Vec4Field, usize)],
    all_indices: &'a [(String, Expr)],
    stride: u64,
    inner_len: u64,
}

/// Collect (field, index_expr) pairs the body loads through vec4-eligible
/// fields, deduplicated by (field, index shape). Used by vec4 detection and
/// by the vec4 loop body substitution.
fn collect_dedup_vec4_indices(
    lower: &FnLowerer,
    fbody: &[Statement],
    item: &str,
) -> Vec<(String, Expr)> {
    let mut indices: Vec<(String, Expr)> = Vec::new();
    for stmt in fbody {
        if let Statement::Assign(_, rhs) = stmt {
            crate::backend::spirv::lower::collect_vec4_indices(
                rhs, &lower.vec4_fields, item, &lower.const_int_values, &mut indices,
            );
        }
    }
    indices.sort_by(|a, b| a.0.cmp(&b.0).then(format!("{:?}", a.1).cmp(&format!("{:?}", b.1))));
    indices.dedup_by(|a, b| a.0 == b.0 && format!("{:?}", a.1) == format!("{:?}", b.1));
    indices
}

/// Split the kernel statements at the Foreach: statements BEFORE it (e.g. the
/// `acc = 0` initialization) must be emitted before the cooperative loop,
/// not after it (emitting `acc = 0` in the loop merge wiped the accumulator
/// before the subgroup reduce — 2026-09-01 gemv FAIL root cause).
fn split_at_foreach(stmts: &[Statement]) -> (Vec<&Statement>, Vec<&Statement>) {
    let mut pre = Vec::new();
    let mut post = Vec::new();
    let mut seen_foreach = false;
    for stmt in stmts {
        if matches!(stmt, Statement::Foreach { .. }) {
            seen_foreach = true;
            continue;
        }
        if seen_foreach { post.push(stmt); } else { pre.push(stmt); }
    }
    (pre, post)
}

/// Emit the kernel statements that FOLLOW the cooperative loop: the final
/// store becomes a subgroup reduction. Shared by the vec4 and scalar paths.
/// The counter increment is dropped — the runner fast-forwards the counter;
/// a cooperative row kernel does not advance it.
fn emit_coop_reduce_store(
    lower: &mut FnLowerer,
    stmts: &[&Statement],
    shape: &KernelShape,
) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Statement::Assign(lhs, Expr::Identifier(name))
                if shape.reduction.as_ref().is_some() && lower.vars.contains_key(name) =>
            {
                if *name == shape.index_var {
                    lower.emit_stmt(stmt)?;
                } else {
                    let reduced = Expr::Call(
                        "SubgroupFAdd#".into(),
                        vec![Expr::Identifier(name.clone())],
                        None,
                    );
                    lower.emit_stmt(&Statement::Assign(lhs.clone(), reduced))?;
                }
            }
            Statement::Assign(Expr::Identifier(n), _) if *n == shape.index_var => {}
            other => lower.emit_stmt(other)?,
        }
    }
    Ok(())
}

/// Load one vec4 element of `fname` at the cooperative base index —
/// `subst(field index, loop_var → repl) >> 2` — and return the `v4float`
/// value id. Deriving the base FROM THE FIELD'S OWN index expression makes
/// 2D (`i*K + k`) and 1D (`k`) fields work alike; the row term is not
/// special-cased. The shift is exact: the vec4 gate (count % 4 == 0) makes
/// the row term 4-aligned, and repl = lane*4 + t*stride is 4-aligned by
/// construction. Callers must have bound the loop var in `lower.vars`.
fn emit_vec4_load(
    lower: &mut FnLowerer,
    ctx: &Vec4LoopCtx,
    fname: &str,
    int_ty: Word,
    ssbo: Word,
) -> Result<(Word, crate::backend::spirv::lower::Vec4Field), String> {
    let (vf, member_pos) = {
        let (_, vf, mp) = ctx.field_data.iter().find(|(f, _, _)| f == fname)
            .ok_or_else(|| format!("vec4 field '{}' not loaded", fname))?;
        (vf.clone(), *mp)
    };
    let idx_expr = &ctx.all_indices.iter().find(|(f, _)| f == fname).unwrap().1;
    // subst inserts the replacement without re-processing it, so the
    // loop-var reference inside repl survives.
    let scalar_idx = crate::backend::spirv::lower::subst_var_deep(idx_expr, ctx.item, ctx.repl);
    let (scalar_id, _) = lower.emit_expr(&scalar_idx)?;
    let two = lower.builder.builder.constant_bit64(int_ty, 2);
    let base = lower.builder.gen_id();
    lower.builder.emit(Instruction::new(
        spirv::Op::ShiftRightArithmetic, Some(int_ty), Some(base),
        vec![Operand::IdRef(scalar_id), Operand::IdRef(two)],
    ));

    let v4_ptr = lower.builder.ptr_class(
        rspirv::spirv::StorageClass::StorageBuffer,
        vf.vector,
    );
    let member = lower.builder.u32_const(member_pos as u32);
    let group = lower.builder.gen_id();
    lower.builder.emit(Instruction::new(
        spirv::Op::AccessChain,
        Some(v4_ptr),
        Some(group),
        vec![
            Operand::IdRef(ssbo),
            Operand::IdRef(member),
            Operand::IdRef(base),
        ],
    ));
    let v4_val = lower.builder.load(vf.vector, group);
    Ok((v4_val, vf))
}

/// Per-iteration vec4 loads for the UNROLLED (per-component) body form:
/// one vec4 load per field, components exposed as synthetic
/// `__vec4_<field>_<j>` variables the unrolled statements read.
fn emit_vec4_field_loads(
    lower: &mut FnLowerer,
    ctx: &Vec4LoopCtx,
    int_ty: Word,
    ssbo: Word,
) -> Result<(), String> {
    for (fname, vf, _member_pos) in ctx.field_data {
        let (v4_val, vf) = emit_vec4_load(lower, ctx, fname, int_ty, ssbo)?;
        let elem_ty_id = lower.builder.lower_type(&vf.elem)?;
        for jj in 0u32..4 {
            let comp = lower.builder.gen_id();
            lower.builder.emit(Instruction::new(
                spirv::Op::CompositeExtract,
                Some(elem_ty_id),
                Some(comp),
                vec![Operand::IdRef(v4_val), Operand::LiteralBit32(jj)],
            ));
            let synthetic_name = format!("__vec4_{}_{}", fname, jj);
            lower.vec4_component_vars.insert(synthetic_name, (comp, vf.elem.clone()));
        }
    }
    Ok(())
}

/// Emit the body once per vec4 component: fields read their synthetic
/// component var; the scalar side substitutes the loop var with the FULL
/// cooperative index `repl + j` (= lane*4 + t*stride + j after binding).
fn emit_vec4_unrolled_body(
    lower: &mut FnLowerer,
    ctx: &Vec4LoopCtx,
    j: u32,
) -> Result<(), String> {
    let subst_j = Expr::BinaryOp(
        crate::ast::BinaryOpKind::Add,
        Box::new(ctx.repl.clone()),
        Box::new(crate::ast::Expr::Decimal(j as i64)),
    );
    let mut body_j = ctx.fbody.to_vec();
    for (fname, _, _) in ctx.field_data {
        let idx_expr = &ctx.all_indices.iter().find(|(f, _)| f == fname).unwrap().1;
        let lowered_idx = crate::backend::spirv::lower::subst_var_deep(
            idx_expr, ctx.item, &crate::ast::Expr::Identifier(ctx.item.to_string()),
        );
        let synthetic_var = crate::ast::Expr::Identifier(format!("__vec4_{}_{}", fname, j));
        for stmt in &mut body_j {
            *stmt = crate::backend::spirv::lower::replace_index_in_stmt(
                stmt, fname, &lowered_idx, &synthetic_var,
            );
        }
    }
    for stmt in &body_j {
        if lower.terminated { break; }
        let st = crate::backend::spirv::lower::subst_stmt_var(stmt, ctx.item, &subst_j);
        lower.emit_stmt(&st)?;
    }
    Ok(())
}

/// Basic-block set for the hand-built structured cooperative loop.
pub(crate) struct CoopLoopBBs {
    pub(crate) header_bb: Word,
    pub(crate) continue_bb: Word,
    pub(crate) merge_bb: Word,
}

/// Shared operands of the structured-loop begin/end helpers: the lowered
/// int/bool type ids and the trip count. The induction variable is SSA
/// (a header phi returned by `begin_structured_loop`), not storage.
pub(crate) struct CoopLoopSig {
    pub(crate) int_ty: Word,
    pub(crate) bool_ty: Word,
    pub(crate) groups: i64,
}

/// Begin the hand-built structured loop (preheader → header), leaving the
/// builder positioned at the start of the body block. Mirrors the Foreach
/// emission in lower.rs; splitting begin/end lets the caller interleave the
/// body emission (vec4 loads depend on the loop variable).
pub(crate) fn begin_structured_loop(
    builder: &mut SpirvBuilder,
    sig: &CoopLoopSig,
    // (type, init id, pre-reserved back-edge id) per loop-carried
    // accumulator; the body defines the back-edge value into that id.
    acc_phis: &[(Word, Word, Word)],
) -> Result<(CoopLoopBBs, Vec<Word>, Word, Word, Word, Word), String> {
    let CoopLoopSig { int_ty, bool_ty, groups, .. } = *sig;
    let header_bb = builder.gen_id();
    let body_bb = builder.gen_id();
    let continue_bb = builder.gen_id();
    let merge_bb = builder.gen_id();
    let preheader_bb = builder.gen_id();
    let cond0 = builder.gen_id();
    let cond_next = builder.gen_id();
    // 2026-09-01 (P3): the induction variable is SSA — a header phi, not
    // Function storage. Removes the per-iteration OpLoad/OpStore pair in
    // the continue block AND every body-side load of the loop variable.
    let loop_backedge = builder.gen_id();
    let zero = builder.builder.constant_bit64(int_ty, 0);

    let emit_cond = |builder: &mut SpirvBuilder, cond_id: Word, cur: Word| -> Result<(), String> {
        let end_c = builder.builder.constant_bit64(int_ty, groups as u64);
        builder.emit(Instruction::new(
            spirv::Op::SLessThan, Some(bool_ty), Some(cond_id),
            vec![Operand::IdRef(cur), Operand::IdRef(end_c)],
        ));
        Ok(())
    };

    builder.builder.branch(preheader_bb);
    builder.begin_block(Some(preheader_bb));
    emit_cond(builder, cond0, zero)?;
    builder.builder.branch(header_bb);

    builder.begin_block(Some(header_bb));
    let loop_phi = builder.builder.phi(
        int_ty,
        None,
        [(zero, preheader_bb), (loop_backedge, continue_bb)],
    ).map_err(|e| format!("loop induction phi: {:?}", e))?;
    let cond_hdr = builder.builder.phi(
        bool_ty,
        None,
        [(cond0, preheader_bb), (cond_next, continue_bb)],
    ).map_err(|e| format!("loop phi: {:?}", e))?;
    // Loop-carried accumulators (e.g. the v4float FMA accumulator): one phi
    // each, (init, preheader) + (back-edge value, continue). Back-edge ids
    // are defined later in the body — the same forward reference the cond
    // phi makes to cond_next.
    let acc_ids: Vec<Word> = acc_phis
        .iter()
        .map(|&(ty, init, backedge)| {
            builder.builder.phi(
                ty,
                None,
                [(init, preheader_bb), (backedge, continue_bb)],
            ).expect("loop accumulator phi")
        })
        .collect();
    builder.builder.loop_merge(
        merge_bb,
        continue_bb,
        rspirv::spirv::LoopControl::NONE,
        [] as [rspirv::dr::Operand; 0],
    );
    builder.builder.branch_conditional(cond_hdr, body_bb, merge_bb, [] as [u32; 0]);
    builder.begin_block(Some(body_bb));
    Ok((CoopLoopBBs { header_bb, continue_bb, merge_bb }, acc_ids, cond_next, cond0, loop_phi, loop_backedge))
}

/// Close the structured loop: continue block (increment + re-check), branch
/// back to the header, then position the builder at the merge block.
pub(crate) fn end_structured_loop(
    builder: &mut SpirvBuilder,
    sig: &CoopLoopSig,
    bbs: &CoopLoopBBs,
    loop_phi: Word,
    loop_backedge: Word,
    cond_next: Word,
) -> Result<(), String> {
    let CoopLoopSig { int_ty, groups, .. } = *sig;
    builder.builder.branch(bbs.continue_bb);
    builder.begin_block(Some(bbs.continue_bb));
    // next = cur + 1, defined INTO the induction phi's pre-reserved
    // back-edge id; then the next iteration's condition.
    let one = builder.builder.constant_bit64(int_ty, 1);
    builder.emit(Instruction::new(
        spirv::Op::IAdd, Some(int_ty), Some(loop_backedge),
        vec![Operand::IdRef(loop_phi), Operand::IdRef(one)],
    ));
    let bool_ty = builder.lower_type(&crate::ast::Type::Bits(1))?;
    let end_c = builder.builder.constant_bit64(int_ty, groups as u64);
    builder.emit(Instruction::new(
        spirv::Op::SLessThan,
        Some(bool_ty),
        Some(cond_next),
        vec![Operand::IdRef(loop_backedge), Operand::IdRef(end_c)],
    ));
    builder.builder.branch(bbs.header_bb);
    builder.begin_block(Some(bbs.merge_bb));
    Ok(())
}

/// Resolve (field, Vec4Field, SSBO member position) triples for every vec4
/// index the cooperative body loads through.
fn collect_vec4_field_data(
    lower: &FnLowerer,
    all_indices: &[(String, Expr)],
) -> Result<Vec<(String, crate::backend::spirv::lower::Vec4Field, usize)>, String> {
    let mut field_data = Vec::new();
    for (fname, _) in all_indices {
        let vf = lower.vec4_fields.get(fname)
            .ok_or_else(|| format!("vec4 field '{}' lost", fname))?
            .clone();
        // Phase 3 — buffer reuse: remap aliased fields to their target and
        // compute the correct SSBO struct member index (aliases are skipped
        // from the struct).
        let effective_name = lower.alias_map.get(fname).cloned().unwrap_or_else(|| fname.clone());
        let raw_pos = lower.state_fields.iter().position(|f| f.name == effective_name)
            .ok_or_else(|| format!("vec4 field '{}' not in state", fname))?;
        let member_pos = lower.struct_member_index(raw_pos) as usize;
        field_data.push((fname.clone(), vf, member_pos));
    }
    Ok(field_data)
}

/// Vec4 cooperative path: a hand-built structured loop so the vec4 loads
/// execute INSIDE each iteration (they depend on the loop variable). The
/// body is unrolled 4x — one vec4 load feeding 4 scalar FMAs.
fn emit_cooperative_vec4(
    lower: &mut FnLowerer,
    shape: &KernelShape,
    item: &str,
    inner_len: u64,
) -> Result<(), String> {
    let ssbo = lower.ssbo_var.ok_or("cooperative vec4 without SSBO")?;
    let fbody = match shape.kernel_stmts.iter().find_map(|s| match s {
        Statement::Foreach { body, .. } => Some(body.clone()),
        _ => None,
    }) {
        Some(b) => b,
        None => return Err("cooperative kernel lost its foreach".into()),
    };
    let all_indices = collect_dedup_vec4_indices(lower, &fbody, item);
    let field_data = collect_vec4_field_data(lower, &all_indices)?;
    let (pre_loop, post_loop) = split_at_foreach(&shape.kernel_stmts);
    for stmt in &pre_loop {
        if lower.terminated { break; }
        lower.emit_stmt(stmt)?;
    }

    // MEASURED (plan 2026-09-01-warp-mlp-ilp): multi-vec4 ILP per lane was
    // REFUTED at M1 occupancy — 4096 independent warps already hide latency;
    // stride stays 128 (one vec4 pair per lane-iteration). Do not re-add
    // per-lane ILP without a measurement at a latency-bound shape.
    let stride: u64 = 128;
    // lane's element set per iteration: [lane*4 + t*stride, +4).
    // 4-aligned by construction → vec4 bases exact under >>2.
    let lane: Expr = Expr::BinaryOp(
        crate::ast::BinaryOpKind::BitAnd,
        Box::new(Expr::Call("GetGlobalId#".into(), vec![Expr::Decimal(0)], None)),
        Box::new(Expr::Decimal(31)),
    );
    let repl = Expr::BinaryOp(
        crate::ast::BinaryOpKind::Add,
        Box::new(Expr::BinaryOp(
            crate::ast::BinaryOpKind::Mul,
            Box::new(lane),
            Box::new(Expr::Decimal(4)),
        )),
        Box::new(Expr::BinaryOp(
            crate::ast::BinaryOpKind::Mul,
            Box::new(Expr::Identifier(item.to_string())),
            Box::new(Expr::Decimal(stride as i64)),
        )),
    );

    let ctx = Vec4LoopCtx {
        item,
        repl: &repl,
        fbody: &fbody,
        field_data: &field_data,
        all_indices: &all_indices,
        stride,
        inner_len,
    };
    // Vector-accumulator form first (one Fma per iteration); the unrolled
    // per-component form is the fallback for bodies it cannot match.
    if emit_cooperative_vec4_fma(lower, &ctx, shape, &post_loop)? {
        return Ok(());
    }

    let sig = CoopLoopSig {
        int_ty: lower.builder.lower_type(&crate::ast::Type::int())?,
        bool_ty: lower.builder.lower_type(&crate::ast::Type::Bits(1))?,
        groups: (inner_len / stride) as i64,
    };

    let (bbs, _acc_ids, cond_next, _cond0, loop_phi, loop_backedge) =
        begin_structured_loop(lower.builder, &sig, &[])?;

    lower.const_vars.remove(item);
    lower.value_vars.insert(item.to_string(), (loop_phi, crate::ast::Type::int()));
    let prev_terminated = lower.terminated;
    lower.terminated = false;

    emit_vec4_field_loads(lower, &ctx, sig.int_ty, ssbo)?;
    for j in 0..4u32 {
        emit_vec4_unrolled_body(lower, &ctx, j)?;
    }

    end_structured_loop(lower.builder, &sig, &bbs, loop_phi, loop_backedge, cond_next)?;
    lower.value_vars.remove(item);
    lower.terminated = prev_terminated;

    emit_coop_reduce_store(lower, &post_loop, shape)
}

/// Both-sides-vec4 FMA detection: a single `acc = acc + F1[i1] * F2[i2]`
/// where BOTH indexed fields are vec4-loaded. Returns (acc, lhs, rhs).
fn match_vec4_vector_fma(
    fbody: &[Statement],
    field_data: &[(String, crate::backend::spirv::lower::Vec4Field, usize)],
) -> Option<(String, String, String)> {
    use crate::ast::BinaryOpKind::{Add, Mul};
    if fbody.len() != 1 {
        return None;
    }
    let Statement::Assign(lhs, rhs) = &fbody[0] else {
        return None;
    };
    let Expr::Identifier(acc) = lhs else {
        return None;
    };
    let Expr::BinaryOp(Add, a, b) = rhs else {
        return None;
    };
    // The additive operand must be the accumulator itself (vectorizing
    // replaces `acc + p*q` with Fma(p, q, acc) — any other additive term
    // would be dropped).
    let mul = match (a.as_ref(), b.as_ref()) {
        (m @ Expr::BinaryOp(Mul, _, _), other) => {
            if matches!(other, Expr::Identifier(n) if n == acc) { m } else { return None; }
        }
        (other, m @ Expr::BinaryOp(Mul, _, _)) => {
            if matches!(other, Expr::Identifier(n) if n == acc) { m } else { return None; }
        }
        _ => return None,
    };
    let Expr::BinaryOp(Mul, left, right) = mul else {
        return None;
    };
    let field_of = |e: &Expr| -> Option<String> {
        let Expr::Index(of, _) = e else { return None };
        let Expr::Identifier(n) = of.as_ref() else { return None };
        field_data.iter().find(|(f, _, _)| f == n).map(|(f, _, _)| f.clone())
    };
    let lhs_field = field_of(left)?;
    let rhs_field = field_of(right)?;
    Some((acc.clone(), lhs_field, rhs_field))
}

/// Vector-accumulator form (P3, plan vec4-projection-layout): the loop body
/// is ONE componentwise GLSL Fma on `v4float`s — `acc_v = Fma(F1, F2, acc_v)`
/// — instead of 4 scalar FMAs on extracted components. After the loop the 4
/// lanes of the vector accumulator fold into the scalar accumulator and the
/// ordinary subgroup-reduce store follows. Returns false when the body does
/// not match (caller falls back to the unrolled form).
fn emit_cooperative_vec4_fma(
    lower: &mut FnLowerer,
    ctx: &Vec4LoopCtx,
    shape: &KernelShape,
    post_loop: &[&Statement],
) -> Result<bool, String> {
    let Some((acc, lhs_field, rhs_field)) =
        match_vec4_vector_fma(ctx.fbody, ctx.field_data)
    else {
        return Ok(false);
    };
    let ssbo = lower.ssbo_var.ok_or("cooperative vec4 without SSBO")?;
    let sig = CoopLoopSig {
        int_ty: lower.builder.lower_type(&crate::ast::Type::int())?,
        bool_ty: lower.builder.lower_type(&crate::ast::Type::Bits(1))?,
        groups: (ctx.inner_len / ctx.stride) as i64,
    };
    let v4_ty = ctx.field_data.iter().find(|(f, _, _)| *f == lhs_field)
        .ok_or("lhs vec4 field lost")?.1.vector;

    // Vector accumulator as a loop phi: zero on entry, the fused Fma value
    // on the back edge. No per-iteration accumulator load/store.
    let acc_backedge = lower.builder.gen_id();
    let acc_zero = lower.builder.builder.constant_null(v4_ty);
    let (bbs, acc_ids, cond_next, _cond0, loop_phi, loop_backedge) =
        begin_structured_loop(lower.builder, &sig, &[(v4_ty, acc_zero, acc_backedge)])?;
    let acc_phi = *acc_ids.first().ok_or("accumulator phi lost")?;
    lower.const_vars.remove(ctx.item);
    lower.value_vars.insert(ctx.item.to_string(), (loop_phi, crate::ast::Type::int()));
    let prev_terminated = lower.terminated;
    lower.terminated = false;

    let (lhs_val, _) = emit_vec4_load(lower, ctx, &lhs_field, sig.int_ty, ssbo)?;
    let (rhs_val, _) = emit_vec4_load(lower, ctx, &rhs_field, sig.int_ty, ssbo)?;
    lower.builder.glsl_fma_with_id(acc_backedge, v4_ty, lhs_val, rhs_val, acc_phi);

    end_structured_loop(lower.builder, &sig, &bbs, loop_phi, loop_backedge, cond_next)?;
    lower.value_vars.remove(ctx.item);
    lower.terminated = prev_terminated;

    // Fold the vector accumulator into the scalar accumulator the store
    // reads: acc = (e0 + e1) + (e2 + e3).
    let elem_ty = lower.builder.lower_type(
        &ctx.field_data.iter().find(|(f, _, _)| *f == lhs_field)
            .ok_or("lhs vec4 field lost")?.1.elem)?;
    let acc_final = acc_phi;
    let mut comps = Vec::with_capacity(4);
    for jj in 0u32..4 {
        let comp = lower.builder.gen_id();
        lower.builder.emit(Instruction::new(
            spirv::Op::CompositeExtract, Some(elem_ty), Some(comp),
            vec![Operand::IdRef(acc_final), Operand::LiteralBit32(jj)],
        ));
        comps.push(comp);
    }
    let add = |lower: &mut FnLowerer, a: Word, b: Word| -> Word {
        let id = lower.builder.gen_id();
        lower.builder.emit(Instruction::new(
            spirv::Op::FAdd, Some(elem_ty), Some(id),
            vec![Operand::IdRef(a), Operand::IdRef(b)],
        ));
        id
    };
    let sum01 = add(lower, comps[0], comps[1]);
    let sum23 = add(lower, comps[2], comps[3]);
    let total = add(lower, sum01, sum23);
    let acc_ptr = lower.vars.get(&acc).map(|(p, _)| *p)
        .ok_or_else(|| format!("accumulator '{}' lost", acc))?;
    lower.builder.store(acc_ptr, total);

    emit_coop_reduce_store(lower, post_loop, shape)?;
    Ok(true)
}

/// Scalar cooperative path: substitute the loop var with `lane + t*32` and
/// emit through the ordinary Foreach machinery (which unrolls the tail).
fn emit_cooperative_scalar(
    lower: &mut FnLowerer,
    shape: &KernelShape,
    item: &str,
    fbody: &[Statement],
    inner_len: u64,
    repl: &Expr,
) -> Result<(), String> {
    let new_body: Vec<Statement> = fbody.iter()
        .map(|st| crate::backend::spirv::lower::subst_stmt_var(st, item, repl))
        .collect();
    let groups = (inner_len / 32) as i64;
    let synthesized = Statement::Foreach {
        item: item.to_string(),
        list: Box::new(Expr::Range {
            start: Box::new(crate::ast::Expr::Decimal(0)),
            end: Box::new(crate::ast::Expr::Decimal(groups)),
            inclusive: false,
        }),
        body: new_body,
    };
    let (pre_loop, post_loop) = split_at_foreach(&shape.kernel_stmts);
    for stmt in &pre_loop {
        if lower.terminated { break; }
        lower.emit_stmt(stmt)?;
    }
    lower.emit_stmt(&synthesized)?;
    emit_coop_reduce_store(lower, &post_loop, shape)
}

/// THE cooperative-shape decision — single source of truth consumed by
/// kernel emission AND the runner's dispatch geometry, so the two can never
/// drift (the M2.0 bug: the blob went flat while the runner still dispatched
/// cooperative geometry, 256x redundant work). Shape-level properties only;
/// emission-side refinements (literal inner length, %32) keep their
/// error-and-CPU-fallback path.

/// True when any kernel statement derives a value from the counter via
/// division or modulo (the flattened-2D signature: `m = i / N`, `n = i %N`).
/// 2026-09-18 (M3.5): the predicate moved to the analysis layer
/// (`analysis::accel::is_cooperative_shape`) so frontend analyses (the
/// coalescing report) share one source of truth; the SPIR-V router
/// consumes it from there.
pub(crate) fn is_cooperative_shape(shape: &KernelShape) -> bool {
    crate::analysis::accel::is_cooperative_shape(shape)
}

pub(crate) fn kernel_stmts_decompose_counter(shape: &KernelShape) -> bool {
    crate::analysis::accel::kernel_stmts_decompose_counter(shape)
}
/// Synthesize the cooperative body for a recognized dot-product reduction:
/// the foreach iterates `t in 0..K/stride` with the original loop var mapped
/// to `lane*4 + t*stride` (coalesced stride) when vec4-eligible fields are
/// present, or `lane + t*32` otherwise. The accumulator ends in a subgroup
/// FAdd, and the counter increment is dropped (the runner fast-forwards).
fn emit_cooperative_reduce(
    lower: &mut FnLowerer,
    shape: &KernelShape,
    inner_len: u64,
) -> Result<(), String> {
    let (item, fbody) = match shape.kernel_stmts.iter().find_map(|s| match s {
        Statement::Foreach { item, body, .. } => Some((item.clone(), body.clone())),
        _ => None,
    }) {
        Some(v) => v,
        None => return Err("cooperative kernel lost its foreach".into()),
    };

    // Detect vec4-eligible fields in the body. When present, each lane loads
    // 4 consecutive floats per iteration — one vec4 load instead of 4 scalar
    // loads. The stride becomes 128 (4 elements × 32 lanes) instead of 32.
    let vec4_indices = collect_dedup_vec4_indices(lower, &fbody, &item);
    let use_vec4 = !vec4_indices.is_empty()
        && vec4_indices.iter().all(|(fname, _)| {
            lower.vec4_fields.get(fname).map(|vf| vf.elem_float).unwrap_or(false)
        });

    if use_vec4 {
        return emit_cooperative_vec4(lower, shape, &item, inner_len);
    }

    // Scalar stride (32): lane + t*32. The strided loop REUSES the original
    // loop-var name (it is the one pre-declared local collect_locals saw);
    // the replacement inserts the same name as the group index — subst
    // inserts the replacement without re-processing it, so this is safe.
    let lane: Expr = Expr::BinaryOp(
        crate::ast::BinaryOpKind::BitAnd,
        Box::new(Expr::Call("GetGlobalId#".into(), vec![Expr::Decimal(0)], None)),
        Box::new(Expr::Decimal(31)),
    );
    let repl = Expr::BinaryOp(
        crate::ast::BinaryOpKind::Add,
        Box::new(lane),
        Box::new(Expr::BinaryOp(
            crate::ast::BinaryOpKind::Mul,
            Box::new(Expr::Identifier(item.clone())),
            Box::new(Expr::Decimal(32)),
        )),
    );
    emit_cooperative_scalar(lower, shape, &item, &fbody, inner_len, &repl)
}

#[cfg(test)]
mod decomposition_gate_tests {
    //! M2.0 hole 2 as unit tests: a counter decomposed with div/mod (the
    //! flattened-2D signature) must reject the cooperative row form; a bare
    //! counter (GEMV) must not. (Plan 2026-09-01-m2-gemm.)
    use super::*;
    use crate::analysis::accel::{KernelShape, ReductionInfo};
    use crate::ast::{BinaryOpKind, Type};

    fn shape_with(stmts: Vec<Statement>) -> KernelShape {
        KernelShape {
            index_var: "i".into(),
            count_expr: Some(Expr::Decimal(4096)),
            kernel_stmts: stmts,
            host_stmts: vec![],
            read_buffers: vec![],
            write_buffers: vec![],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: Some(ReductionInfo::dot(Expr::Decimal(64))),
            deferred_normalize: None,
        }
    }

    fn id(n: &str) -> Expr {
        Expr::Identifier(n.into())
    }

    #[test]
    fn decomposed_counter_rejects_cooperative() {
        // let m: Int = i / N; let n: Int = i % N;  → flattened 2D.
        let stmts = vec![
            Statement::Let {
                name: "m".into(),
                names: vec![],
                ty: Some(Type::Custom("Int".into())),
                expr: Some(Expr::BinaryOp(BinaryOpKind::Div, Box::new(id("i")), Box::new(id("N")))),
                modifiers: vec![],
            },
            Statement::Let {
                name: "n".into(),
                names: vec![],
                ty: Some(Type::Custom("Int".into())),
                expr: Some(Expr::BinaryOp(BinaryOpKind::Mod, Box::new(id("i")), Box::new(id("N")))),
                modifiers: vec![],
            },
        ];
        let shape = shape_with(stmts);
        assert!(kernel_stmts_decompose_counter(&shape),
            "div/mod of the counter must reject the cooperative form");
    }

    #[test]
    fn bare_counter_keeps_cooperative() {
        // y[i] = acc; — the GEMV form, no decomposition anywhere.
        let stmts = vec![
            Statement::Assign(Expr::Index(Box::new(id("y")), Box::new(id("i"))), id("acc")),
        ];
        let shape = shape_with(stmts);
        assert!(!kernel_stmts_decompose_counter(&shape),
            "a bare counter is the GEMV cooperative form");
    }

    #[test]
    fn decomposition_inside_foreach_body_counts() {
        // The counter decomposed INSIDE the reduction body must reject too —
        // the binding happens before any body emission.
        let inner = vec![
            Statement::Let {
                name: "row".into(),
                names: vec![],
                ty: Some(Type::Custom("Int".into())),
                expr: Some(Expr::BinaryOp(BinaryOpKind::Div, Box::new(id("i")), Box::new(id("T")))),
                modifiers: vec![],
            },
        ];
        let stmts = vec![
            Statement::Foreach {
                item: "k".into(),
                list: Box::new(Expr::Range {
                    start: Box::new(Expr::Decimal(0)),
                    end: Box::new(Expr::Decimal(64)),
                    inclusive: false,
                }),
                body: inner,
            },
        ];
        let shape = shape_with(stmts);
        assert!(kernel_stmts_decompose_counter(&shape),
            "decomposition inside the foreach body counts");
    }
}
