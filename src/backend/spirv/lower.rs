//! SPIR-V statement/expression lowering — the real kernel body emitter.
//!
//! 2026-08-23 (plan 2026-08-23-spirv-kernel-emission §2.1): replaces the
//! placeholder body block (bare Op.Return). Lowers the bounded subset a
//! compute kernel needs: integer arithmetic/logic/comparisons, locals,
//! GetGlobalId#/GetLocalId# builtins, and program-state access through ONE
//! StorageBuffer binding (Block-decorated struct, deterministic member
//! order = sorted by field name).
//!
//! Rule-19 note: element types come from Briev `Type` values via the
//! TypeCache, never from name matches on user types.
//!
//! To undo: revert kernel.rs to placeholder body + delete this file.

use std::collections::{HashMap, HashSet};

use rspirv::dr::{Instruction, Operand};
use rspirv::spirv::{self, StorageClass, Word};

use crate::ast::{Expr, Statement, Type, UnaryOpKind};
use crate::casting::graph::SpirvShape;
use crate::backend::spirv::builder::SpirvBuilder;

/// One program-state field referenced by the kernel body. Collected before
/// emission so the SSBO layout is stable regardless of use order.
#[derive(Debug, Clone)]
pub struct StateField {
    pub name: String,
    pub ty: Type,
}

/// O3 (plan 2026-08-31-o3-float4-loads.md): a state array retyped as an
/// array of 4-wide vectors. `array` is the member type id; `vector` the
/// element vector type id; `elem` the scalar element type.
#[derive(Clone)]
pub struct Vec4Field {
    pub array: Word,
    pub vector: Word,
    pub elem: Type,
    /// Stage-2 scope: only float groups fuse into Fma.
    pub elem_float: bool,
}

pub struct FnLowerer<'a> {
    pub builder: &'a mut SpirvBuilder,
    /// Local variables: name → (Function-storage pointer id, type).
    pub vars: HashMap<String, (Word, Type)>,
    /// State fields exposed through the SSBO: sorted name → (type, member idx).
    pub state_fields: Vec<StateField>,
    /// D3: the fields carrying the f16 vector view (the smem GEMM's
    /// a/b/y, set by kernel.rs when the tensor tier + the knobs agree).
    /// Width 2 = the D3 pair view (v2f16, ArrayStride 4); width 4 = the
    /// D3b quad view (v4f16, ArrayStride 8) — byte-identical retyping.
    pair_view_fields: Vec<(String, u32)>,
    /// SSBO variable id (StorageBuffer storage class); set by setup_state_buffer.
    pub ssbo_var: Option<Word>,
    /// BuiltIn GlobalInvocationId input variable (pre-threaded or lazy).
    pub global_id_var: Option<Word>,
    /// BuiltIn LocalInvocationId input variable (pre-threaded or lazy).
    pub local_id_var: Option<Word>,
    /// 2026-09-01 (M2.1): gl_WorkGroupID — the tiled GEMM's tile coordinates.
    pub workgroup_id_var: Option<Word>,
    /// 2026-08-31 (plan abv-gpu-by-default): module consts materialized as
    /// SPIR-V constants — name → (constant id, type). Kernel bodies read
    /// `const dt: Float = 0.001;` etc. directly; non-literal initializers
    /// error at materialization.
    pub consts: HashMap<String, (Word, Type)>,
    /// 2026-08-31 (O1): literal const VALUES for unrolling decisions.
    pub const_int_values: HashMap<String, i64>,
    /// 2026-08-31 (O1): loop variables bound to CONSTANTS by unrolling -
    /// reads return the constant id directly (no OpLoad; a load from a
    /// constant id is not a logical pointer).
    pub const_vars: HashMap<String, (Word, Type)>,
    /// 2026-09-01 (warp-mlp-ilp P3): SSA VALUE bindings — a name resolved to
    /// a definition id (loop induction phis), read without a storage load.
    /// Checked first: the live loop-carried value beats every other binding.
    pub value_vars: HashMap<String, (Word, Type)>,
    /// Set when the body executed a term/endprogram — callers stop
    /// branching afterwards (a block can only have one terminator).
    pub terminated: bool,
    /// O3 (plan 2026-08-31-o3-float4-loads.md): fields whose SSBO member is
    /// declared as an array of 4-wide vectors (byte-identical layout). Every
    /// scalar access goes through AccessChain(idx >> 2, idx & 3); aligned
    /// groups in the unrolled prefix emit wide loads.
    pub vec4_fields: HashMap<String, Vec4Field>,
    /// Cooperative vec4: maps (field_name, loop_var_substituted_index) to
    /// the 4 component SSA value ids. Populated before the body is emitted;
    /// emit_expr intercepts Index lookups for these fields.
    pub vec4_coop_components: HashMap<String, [Word; 4]>,
    /// Cooperative vec4: synthetic variable names for pre-loaded vec4
    /// components. Maps "__vec4_{field}_{j}" → (SSA id, elem type).
    /// Populated before body emission; emit_expr resolves these directly.
    pub vec4_component_vars: HashMap<String, (Word, Type)>,
    /// 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): image
    /// storage plans for this kernel — array name → plan. Planned arrays
    /// are EXCLUDED from state_fields (they are not SSBO members); their
    /// writes lower to OpImageWrite against a UniformConstant image.
    pub image_plans: HashMap<String, crate::analysis::image_storage::ImageStoragePlan>,
    /// 2026-09-14 (Phase 3 — buffer reuse): fields aliased to another
    /// field's slot. Maps aliased_field_name → target_field_name. The
    /// aliased field is skipped in the SSBO struct; AccessChain remaps
    /// to the target's member index. Both kernel and runner agree.
    pub alias_map: HashMap<String, String>,
    /// Array name → the emitted image variable id (UniformConstant).
    pub image_vars: HashMap<String, Word>,
    /// Array name → the OpTypeImage id (the OpLoad result type — the
    /// variable's result_type is the POINTER, not the image).
    pub image_types: HashMap<String, Word>,
}

impl<'a> FnLowerer<'a> {
    pub fn new(builder: &'a mut SpirvBuilder, state_fields: Vec<StateField>) -> Self {
        FnLowerer {
            builder,
            vars: HashMap::new(),
            state_fields,
            pair_view_fields: Vec::new(),
            ssbo_var: None,
            global_id_var: None,
            local_id_var: None,
            workgroup_id_var: None,
            consts: HashMap::new(),
            const_int_values: HashMap::new(),
            const_vars: HashMap::new(),
            value_vars: HashMap::new(),
            terminated: false,
            vec4_fields: HashMap::new(),
            vec4_coop_components: HashMap::new(),
            vec4_component_vars: HashMap::new(),
            image_plans: HashMap::new(),
            alias_map: HashMap::new(),
            image_vars: HashMap::new(),
            image_types: HashMap::new(),
        }
    }

    /// Register image storage plans and REMOVE the planned arrays from the
    /// SSBO field list (they are not buffer members). Call before any
    /// setup/buffer work.
    /// D3: the fields carrying the f16 vector view (name, width), set by
    /// kernel.rs when the tensor tier + the knobs agree.
    pub fn set_pair_view_fields(&mut self, fields: Vec<(String, u32)>) {
        self.pair_view_fields = fields;
    }

    pub fn set_image_plans(
        &mut self,
        plans: &[crate::analysis::image_storage::ImageStoragePlan],
    ) {
        for p in plans {
            self.image_plans.insert(p.array.clone(), p.clone());
        }
        self.state_fields
            .retain(|f| !self.image_plans.contains_key(&f.name));
    }

    /// 2026-08-31 (plan abv-gpu-by-default): materialize module consts as
    /// SPIR-V constants for direct (no-load) reads in kernel bodies. Only
    /// literal initializers (int/float/bool) are supported — anything else
    /// errors naming the const, since kernels cannot evaluate host-side
    /// expressions at module scope.
    pub fn materialize_consts(&mut self, items: &[crate::ast::TopLevel]) -> Result<(), String> {
        for item in items {
            let crate::ast::TopLevel::Constant(c) = item else { continue };
            let (id, ty) = match &c.expr {
                Expr::Decimal(n) => {
                    let id = self.builder.i64_const(*n as u64);
                    // O1 unrolling reads the VALUE, not just the constant id.
                    self.const_int_values.insert(c.name.clone(), *n);
                    (id, Type::int())
                }
                Expr::Float(v) => {
                    let bits = self.builder.float_bits_of(&c.ty)?;
                    let id = self.builder.float_const(bits, *v);
                    (id, c.ty.clone())
                }
                Expr::Bool(b) => {
                    let bool_ty = self.builder.lower_type(&Type::Bits(1))?;
                    let op = if *b { spirv::Op::ConstantTrue } else { spirv::Op::ConstantFalse };
                    let id = self.builder.gen_id();
                    self.builder.emit_global(Instruction::new(
                        op,
                        Some(bool_ty),
                        Some(id),
                        vec![],
                    ));
                    (id, Type::Bits(1))
                }
                other => {
                    return self.err(format!(
                        "const '{}' has a non-literal initializer ({:?}) — kernels \
                         read literal consts only; inline the expression",
                        c.name, std::mem::discriminant(other)
                    ));
                }
            };
            self.consts.insert(c.name.clone(), (id, ty));
        }
        Ok(())
    }

    /// Force-create both invocation-id builtin variables as module globals.
    /// SPIR-V entry-point interfaces are complete from the start; warming
    /// avoids arm-order dependence.
    pub fn warm_builtins(&mut self) -> Result<(), String> {
        self.global_invocation_id()?;
        self.local_invocation_id()?;
        self.work_group_id()?;
        Ok(())
    }

    pub fn work_group_id(&mut self) -> Result<Word, String> {
        if let Some(v) = self.workgroup_id_var {
            return Ok(v);
        }
        let v = self.builtin_input(spirv::BuiltIn::WorkgroupId)?;
        self.workgroup_id_var = Some(v);
        Ok(v)
    }

    fn err<T>(&self, what: impl Into<String>) -> Result<T, String> {
        Err(format!("SPIR-V lowering: {}", what.into()))
    }

    // ── Statements ──────────────────────────────────────────────────────

    pub fn emit_stmt(&mut self, stmt: &Statement) -> Result<(), String> {
        match stmt {
            Statement::Let { name, ty, expr: Some(e), .. } => {
                // 2026-08-23: the VARIABLE itself was pre-declared in the
                // entry block (SPIR-V requires every function-scope
                // OpVariable in the FIRST block); here we only store.
                // 2026-08-31 (VITRIOL GEMM comparison M1): a FLOAT let with a
                // DECIMAL initializer (`let acc: Float = 0;`) needs the zero
                // materialized as the float-typed constant - storing an i64
                // zero into a float variable is an OpStore type mismatch.
                // 2026-09-02: the CONSTANT WIDTH follows the RECORDED
                // (Function-storage) type, not the AST annotation — an f16
                // local widens to f32, so its constants are f32 too. Undo:
                // read `ty` from the AST for the bits decision.
                let Some((var, recorded_ty)) = self.vars.get(name.as_str()).cloned() else {
                    return self.err(format!("local '{}' was not pre-declared", name));
                };
                let declared_float = self.builder.is_float_type(&recorded_ty).unwrap_or(false);
                let (val, _ty) = match (declared_float, e) {
                    (true, Expr::Decimal(n)) => {
                        let bits = self.builder.float_bits_of(&recorded_ty)?;
                        let c = self.builder.float_const(bits, *n as f64);
                        (c, recorded_ty)
                    }
                    _ => self.emit_expr(e)?,
                };
                self.builder.store(var, val);
                Ok(())
            }
            Statement::Let { expr: None, .. } => {
                self.err("uninitialized let is not supported in kernels")
            }
            Statement::Assign(lhs, rhs) => {
                let (val, vty) = self.emit_expr(rhs)?;
                // 2026-09-02: image-planned arrays store via OpImageWrite —
                // no SSBO pointer exists for them.
                if let Expr::Index(base, idx) = lhs {
                    if let Expr::Identifier(name) = base.as_ref() {
                        if self.image_plans.contains_key(name) {
                            return self.emit_image_write(name, idx, val, &vty);
                        }
                    }
                }
                let (ptr, pty) = self.lhs_addr(lhs)?;
                // 2026-09-02: the STORE COERCES to the destination shape —
                // an f32-widened local storing into an f16 SSBO member
                // converts (OpFConvert); same-shape stores pass through.
                // The old raw OpStore was a type mismatch (spirv-val)
                // whenever the widths differed.
                let val = self.coerce_value(val, &vty, &pty)?;
                self.builder.emit(Instruction::new(
                    spirv::Op::Store,
                    None,
                    None,
                    vec![Operand::IdRef(ptr), Operand::IdRef(val)],
                ));
                Ok(())
            }
            Statement::Expression(e) => {
                self.emit_expr(e)?;
                Ok(())
            }
            Statement::Term(v) | Statement::EndProgram(v) => {
                if let Some(e) = v {
                    self.emit_expr(e)?;
                }
                // Typed ret — raw Instruction emission leaves the block
                // 'open' in rspirv's state and the next begin_block panics.
                self.builder.ret();
                self.terminated = true;
                Ok(())
            }
            // 2026-08-31 (VITRIOL GEMM comparison M1): a bounded foreach
            // lowers to a STRUCTURED loop (OpLoopMerge) over a Function-scope
            // loop variable. The loop var was pre-declared in the entry block
            // by collect_locals; the body obeys the same scalar/array rules.
            Statement::Foreach { item, list, body } => {
                let Expr::Range { start, end, inclusive } = list.as_ref() else {
                    return self.err(
                        "foreach over a non-range collection - kernel loops iterate `start..end` ranges only",
                    );
                };
                let Some((var, _)) = self.vars.get(item) else {
                    return self.err(format!("foreach item '{}' was not pre-declared", item));
                };
                let var = *var;
                let int_ty = self.type_id(&Type::int())?;
                let bool_ty = self.type_id(&Type::Bits(1))?;
                let cmp_op = if *inclusive { spirv::Op::SLessThanEqual } else { spirv::Op::SLessThan };

                // O1 (plan VITRIOL GEMM comparison): constant-trip-count
                // unrolling. `0..K` with K a literal const emits all trip
                // iterations inlined (loop-var reads bound to per-iteration
                // constants) - no loop instructions at all.
                let start_n = match start.as_ref() {
                    Expr::Decimal(n) if *n >= 0 => Some(*n as u64),
                    _ => None,
                };
                let end_n = match end.as_ref() {
                    Expr::Decimal(n) if *n >= 0 => Some(*n as u64),
                    Expr::Identifier(f) => {
                        self.const_int_values.get(f).copied().map(|v| v as u64)
                    }
                    _ => None,
                };

                if let (Some(s0), Some(e0)) = (start_n, end_n) {
                    let factor = crate::config_tuning::ir_lowering().spirv_unroll;
                    let body_cost = body.len().max(1) as u32;
                    // Code-size budget: at most `budget` inlined body copies
                    // total; a large trip keeps a structured loop for the
                    // rest (K=4096 fully inlined produced megabyte modules
                    // and second-long compiles).
                    let budget: u32 = 64;
                    let factor = if body_cost > 0 { factor.clamp(1, budget / body_cost).max(1) } else { factor };
                    let trip = e0.saturating_sub(s0);
                    let unrolled = trip.min(factor as u64);
                    let mut k = s0;

                    // Unrolled prefix: `unrolled` inlined copies. O3: when
                    // the next four iterations form an aligned group with a
                    // vec4-typed field, ONE wide load covers k..k+3.
                    let group_match =
                        match_vec4_fma(body, &item, &self.vec4_fields, &self.const_int_values);
                    while k < s0 + unrolled {
                        if k % 4 == 0
                            && s0 + unrolled - k >= 4
                            && group_match.is_some()
                        {
                            let g = group_match.as_ref().unwrap();
                            self.emit_vec4_fma_group(g, &item, Some(k), k, var)?;
                            k += 4;
                            continue;
                        }
                        let ck = self.builder.builder.constant_bit64(int_ty, k);
                        self.const_vars.insert(item.clone(), (ck, Type::int()));
                        for s in body {
                            if self.terminated {
                                break;
                            }
                            self.emit_stmt(s)?;
                        }
                        k += 1;
                    }

                    // O3 VECTOR REMAINDER LOOP (plan o3-float4-loads.md):
                    // the unrolled prefix is tiny relative to K — the real
                    // win is a step-4 loop whose body emits one wide load
                    // per vec4 field. Runs when the body matched the
                    // mul-add group pattern; a scalar tail loop (below)
                    // covers the last <4 iterations.
                    if let Some(g) = &group_match {
                        let group_end = k + ((e0 - k) / 4) * 4;
                        if group_end > k {
                            let step_ty = self.type_id(&Type::int())?;
                            let header = self.builder.gen_id();
                            let body_bb = self.builder.gen_id();
                            let continue_bb = self.builder.gen_id();
                            let merge = self.builder.gen_id();
                            let preheader_bb = self.builder.gen_id();
                            let cond0 = self.builder.gen_id();
                            let cond_next = self.builder.gen_id();

                            let start_c =
                                self.builder.builder.constant_bit64(int_ty, k);
                            let end_c =
                                self.builder.builder.constant_bit64(int_ty, group_end);
                            self.builder.store(var, start_c);
                            let emit_cond = |lower: &mut Self,
                                             cond: u32|
                             -> Result<(), String> {
                                let v = lower.builder.gen_id();
                                lower.builder.emit(Instruction::new(
                                    spirv::Op::Load,
                                    Some(int_ty),
                                    Some(v),
                                    vec![Operand::IdRef(var)],
                                ));
                                lower.builder.emit(Instruction::new(
                                    cmp_op,
                                    Some(bool_ty),
                                    Some(cond),
                                    vec![Operand::IdRef(v), Operand::IdRef(end_c)],
                                ));
                                Ok(())
                            };
                            self.builder.builder.branch(preheader_bb);
                            self.builder.begin_block(Some(preheader_bb));
                            emit_cond(self, cond0)?;
                            self.builder.builder.branch(header);
                            self.builder.begin_block(Some(header));
                            let cond_hdr = self
                                .builder
                                .builder
                                .phi(
                                    bool_ty,
                                    None,
                                    [(cond0, preheader_bb), (cond_next, continue_bb)],
                                )
                                .map_err(|e| format!("loop phi: {:?}", e))?;
                            self.builder.builder.loop_merge(
                                merge,
                                continue_bb,
                                rspirv::spirv::LoopControl::NONE,
                                [] as [rspirv::dr::Operand; 0],
                            );
                            self.builder
                                .builder
                                .branch_conditional(cond_hdr, body_bb, merge, [] as [u32; 0]);
                            self.builder.begin_block(Some(body_bb));
                            self.vars.insert(item.clone(), (var, Type::int()));
                            let prev_terminated = self.terminated;
                            self.terminated = false;
                            self.emit_vec4_fma_group(g, &item, None, k, var)?;
                            self.builder.builder.branch(continue_bb);
                            self.builder.begin_block(Some(continue_bb));
                            let cur = self.builder.gen_id();
                            self.builder.emit(Instruction::new(
                                spirv::Op::Load,
                                Some(step_ty),
                                Some(cur),
                                vec![Operand::IdRef(var)],
                            ));
                            let four = self.builder.builder.constant_bit64(step_ty, 4);
                            let next = self.builder.gen_id();
                            self.builder.emit(Instruction::new(
                                spirv::Op::IAdd,
                                Some(step_ty),
                                Some(next),
                                vec![Operand::IdRef(cur), Operand::IdRef(four)],
                            ));
                            self.builder.emit(Instruction::new(
                                spirv::Op::Store,
                                None,
                                None,
                                vec![Operand::IdRef(var), Operand::IdRef(next)],
                            ));
                            emit_cond(self, cond_next)?;
                            self.builder.builder.branch(header);
                            self.builder.begin_block(Some(merge));
                            self.terminated = prev_terminated;
                            k = group_end;
                        }
                    }

                    // Structured remainder loop: k in [k, e0).
                    let step_ty = self.type_id(&Type::int())?;
                    if k < e0 {
                        let header = self.builder.gen_id();
                        let body_bb = self.builder.gen_id();
                        let continue_bb = self.builder.gen_id();
                        let merge = self.builder.gen_id();
                        let preheader_bb = self.builder.gen_id();
                        let cond0 = self.builder.gen_id();
                        let cond_next = self.builder.gen_id();

                        let start_c = self.builder.builder.constant_bit64(int_ty, k);
                        let end_c = self.builder.builder.constant_bit64(int_ty, e0);
                        self.builder.store(var, start_c);
                        let emit_cond = |lower: &mut Self, cond: u32| -> Result<(), String> {
                            let v = lower.builder.gen_id();
                            lower.builder.emit(Instruction::new(
                                spirv::Op::Load,
                                Some(int_ty),
                                Some(v),
                                vec![Operand::IdRef(var)],
                            ));
                            lower.builder.emit(Instruction::new(
                                cmp_op,
                                Some(bool_ty),
                                Some(cond),
                                vec![Operand::IdRef(v), Operand::IdRef(end_c)],
                            ));
                            Ok(())
                        };
                        self.builder.builder.branch(preheader_bb);
                        self.builder.begin_block(Some(preheader_bb));
                        emit_cond(self, cond0)?;
                        self.builder.builder.branch(header);
                        self.builder.begin_block(Some(header));
                        let cond_hdr = self
                            .builder
                            .builder
                            .phi(
                                bool_ty,
                                None,
                                [(cond0, preheader_bb), (cond_next, continue_bb)],
                            )
                            .map_err(|e| format!("loop phi: {:?}", e))?;
                        self.builder
                            .builder
                            .loop_merge(merge, continue_bb, rspirv::spirv::LoopControl::NONE, [] as [rspirv::dr::Operand; 0]);
                        self.builder
                            .builder
                            .branch_conditional(cond_hdr, body_bb, merge, [] as [u32; 0]);
                        self.builder.begin_block(Some(body_bb));
                        self.const_vars.remove(item);
                        self.vars.insert(item.clone(), (var, Type::int()));
                        let prev_terminated = self.terminated;
                        self.terminated = false;
                        for s in body {
                            if self.terminated {
                                break;
                            }
                            self.emit_stmt(s)?;
                        }
                        self.builder.builder.branch(continue_bb);
                        self.builder.begin_block(Some(continue_bb));
                        let cur = self.builder.gen_id();
                        self.builder.emit(Instruction::new(
                            spirv::Op::Load,
                            Some(step_ty),
                            Some(cur),
                            vec![Operand::IdRef(var)],
                        ));
                        let one = self.builder.builder.constant_bit64(step_ty, 1);
                        let next = self.builder.gen_id();
                        self.builder.emit(Instruction::new(
                            spirv::Op::IAdd,
                            Some(step_ty),
                            Some(next),
                            vec![Operand::IdRef(cur), Operand::IdRef(one)],
                        ));
                        self.builder.emit(Instruction::new(
                            spirv::Op::Store,
                            None,
                            None,
                            vec![Operand::IdRef(var), Operand::IdRef(next)],
                        ));
                        emit_cond(self, cond_next)?;
                        self.builder.builder.branch(header);
                        self.builder.begin_block(Some(merge));
                        self.terminated = prev_terminated;
                    }
                    return Ok(());
                }

                // General path: runtime bounds - structured loop.
                let (start_v, _sty) = self.emit_expr(start)?;
                self.builder.store(var, start_v);

                let header = self.builder.gen_id();
                let body_bb = self.builder.gen_id();
                let continue_bb = self.builder.gen_id();
                let merge = self.builder.gen_id();
                let preheader_bb = self.builder.gen_id();
                let cond0 = self.builder.gen_id();
                let cond_next = self.builder.gen_id();

                let emit_cond = |lower: &mut Self, cond: u32| -> Result<(), String> {
                    let v = lower.builder.gen_id();
                    lower.builder.emit(Instruction::new(
                        spirv::Op::Load,
                        Some(int_ty),
                        Some(v),
                        vec![Operand::IdRef(var)],
                    ));
                    let (end_v, _ety) = lower.emit_expr(end)?;
                    lower.builder.emit(Instruction::new(
                        cmp_op,
                        Some(bool_ty),
                        Some(cond),
                        vec![Operand::IdRef(v), Operand::IdRef(end_v)],
                    ));
                    Ok(())
                };

                self.builder.builder.branch(preheader_bb);
                self.builder.begin_block(Some(preheader_bb));
                emit_cond(self, cond0)?;
                self.builder.builder.branch(header);
                self.builder.begin_block(Some(header));
                // OpPhi opens the header; OpLoopMerge must immediately
                // precede the OpBranchConditional after it.
                let cond_hdr = self
                    .builder
                    .builder
                    .phi(
                        bool_ty,
                        None,
                        [(cond0, preheader_bb), (cond_next, continue_bb)],
                    )
                    .map_err(|e| format!("loop phi: {:?}", e))?;
                self.builder
                    .builder
                    .loop_merge(merge, continue_bb, rspirv::spirv::LoopControl::NONE, [] as [rspirv::dr::Operand; 0]);
                self.builder
                    .builder
                    .branch_conditional(cond_hdr, body_bb, merge, [] as [u32; 0]);
                self.builder.begin_block(Some(body_bb));
                self.vars.insert(item.clone(), (var, Type::int()));
                let prev_terminated = self.terminated;
                self.terminated = false;
                for s in body {
                    if self.terminated {
                        break;
                    }
                    self.emit_stmt(s)?;
                }
                self.builder.builder.branch(continue_bb);
                self.builder.begin_block(Some(continue_bb));
                let step_ty = self.type_id(&Type::int())?;
                let cur = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    spirv::Op::Load,
                    Some(step_ty),
                    Some(cur),
                    vec![Operand::IdRef(var)],
                ));
                let one = self.builder.builder.constant_bit64(step_ty, 1);
                let next = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    spirv::Op::IAdd,
                    Some(step_ty),
                    Some(next),
                    vec![Operand::IdRef(cur), Operand::IdRef(one)],
                ));
                self.builder.emit(Instruction::new(
                    spirv::Op::Store,
                    None,
                    None,
                    vec![Operand::IdRef(var), Operand::IdRef(next)],
                ));
                emit_cond(self, cond_next)?;
                self.builder.builder.branch(header);
                self.builder.begin_block(Some(merge));
                self.terminated = prev_terminated;
                Ok(())
            }
            other => self.err(format!(
                "unsupported statement in kernel body ({:?}) — compute kernels \
                 support let/assign/expression/term over integer expressions",
                std::mem::discriminant(other)
            )),
        }
    }

    /// Resolve an assignment target to a storage POINTER id.
    /// Identifiers → Function var; `field[idx]` → SSBO AccessChain.
    /// Resolve an assignment target to `(pointer, pointee_type)`. 2026-09-02:
    /// the pointee type now travels with the pointer — the Assign store
    /// coerces the value to it (an f32-widened local into an f16 SSBO
    /// member).
    fn lhs_addr(&mut self, lhs: &Expr) -> Result<(Word, Type), String> {
        match lhs {
            Expr::Identifier(name) => {
                let Some((var, ty)) = self.vars.get(name) else {
                    return self.err(format!("assignment to unknown '{}'", name));
                };
                Ok((*var, ty.clone()))
            }
            Expr::Index(obj, idx) => {
                let Some(field_name) = field_name_of(obj) else {
                    return self.err("only direct state-field indexing is supported");
                };
                let (idx_val, _) = self.emit_expr(idx)?;
                let (elem_ptr, ety) = self.state_field_elem_ptr(field_name, idx_val)?;
                Ok((elem_ptr, ety))
            }
            other => self.err(format!(
                "unsupported assignment target ({:?})",
                std::mem::discriminant(other)
            )),
        }
    }

    /// Convert a value to the destination shape (2026-09-02). Identity when
    /// the type ids match; otherwise the scalar cast opcode from
    /// `cast_opcode` (float width changes → OpFConvert). Undo: delete and
    /// restore raw OpStore in Assign.
    fn coerce_value(&mut self, id: Word, src: &Type, dst: &Type) -> Result<Word, String> {
        if self.type_id(src)? == self.type_id(dst)? {
            return Ok(id);
        }
        let src_shape = self.builder.shape_of(src)?;
        let dst_shape = self.builder.shape_of(dst)?;
        match Self::cast_opcode(&src_shape, &dst_shape)? {
            None => Ok(id),
            Some(op) => {
                let dst_ty_id = self.type_id(dst)?;
                let res = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    op,
                    Some(dst_ty_id),
                    Some(res),
                    vec![Operand::IdRef(id)],
                ));
                Ok(res)
            }
        }
    }

    // ── Expressions ─────────────────────────────────────────────────────

    pub fn emit_expr(&mut self, e: &Expr) -> Result<(Word, Type), String> {
        match e {
            Expr::Decimal(n) => {
                let c = self.builder.i64_const(*n as u64);
                Ok((c, Type::int()))
            }
            // 2026-09-02: Bool literals as kernel values (match-arm bodies,
            // flag bindings). Same globals-section routing as the numeric
            // constants.
            Expr::Bool(b) => {
                let bool_ty = self.builder.lower_type(&Type::Bits(1))?;
                let c = if *b {
                    self.builder.builder.constant_true(bool_ty)
                } else {
                    self.builder.builder.constant_false(bool_ty)
                };
                Ok((c, Type::Bits(1)))
            }
            // 2026-08-31 (plan abv-gpu-by-default): float literals lower to
            // bit-pattern constants of the default float type (f32 unless the
            // module's Float metadata widens it — resolved via the casting
            // graph, never a name match).
            Expr::Float(v) => {
                let ty = Type::float();
                let bits = self.builder.float_bits_of(&ty)?;
                let c = self.builder.float_const(bits, *v);
                Ok((c, ty))
            }
            Expr::Identifier(name) => {
                // 2026-09-01 (warp-mlp P3): SSA value bindings (induction
                // phis) resolve without a storage round-trip.
                if let Some((id, ty)) = self.value_vars.get(name) {
                    return Ok((*id, ty.clone()));
                }
                // 2026-09-01 (cooperative vec4): pre-loaded vec4 components
                // are synthetic variables — resolved directly without a load.
                if let Some((id, ty)) = self.vec4_component_vars.get(name) {
                    return Ok((*id, ty.clone()));
                }
                // 2026-08-31 (O1 unrolling): unrolled loop variables are
                // CONSTANTS — checked before the variable shadow, so an
                // unrolled body reads the per-iteration constant instead of
                // the stale (never-stored) loop variable.
                if let Some((id, ty)) = self.const_vars.get(name) {
                    return Ok((*id, ty.clone()));
                }
                if let Some((var, ty)) = self.vars.get(name) {
                    let (var, ty) = (*var, ty.clone());
                    let result_ty = self.type_id(&ty)?;
                    let loaded = self.builder.gen_id();
                    self.builder.emit(Instruction::new(
                        spirv::Op::Load,
                        Some(result_ty),
                        Some(loaded),
                        vec![Operand::IdRef(var)],
                    ));
                    return Ok((loaded, ty));
                }
                // 2026-08-31 (plan abv-gpu-by-default): module consts are
                // SPIR-V constants — the id is used directly, no load.
                if let Some((id, ty)) = self.consts.get(name) {
                    return Ok((*id, ty.clone()));
                }
                // 2026-08-31 (plan abv-gpu-by-default): state SCALARS resolve
                // through the SSBO (e.g. the `[i < nb]` bound read inside the
                // kernel's bounds guard) — AccessChain + Load.
                if self.state_fields.iter().any(|f| &f.name == name) {
                    let (ptr, fty) = self.state_field_scalar_ptr(name)?;
                    return self.load_widened(ptr, fty);
                }
                self.err(format!("unknown identifier '{}' in kernel", name))
            }
            // 2026-08-23 (§2.1 read path): state-field element LOADS —
            // out[i] = a[i] + b[i] needs AccessChain + Load on the SSBO.
            Expr::Index(obj, idx) => {
                let Some(fname) = FnLowerer::field_name_of(obj) else {
                    return self.err("only direct state-field indexing reads");
                };
                if !self.state_fields.iter().any(|f| f.name == fname) {
                    return self.err(format!("unknown state field '{}' (declare it as indexed state)", fname));
                }
                let (idx_val, _) = self.emit_expr(idx)?;
                let (ptr, ety) = self.state_field_elem_ptr(fname, idx_val)?;
                self.load_widened(ptr, ety)
            }
            Expr::BinaryOp(kind, l, r) => self.emit_binop(kind, l, r),
            // 2026-08-31 (plan abv-gpu-by-default): `as` casts between scalar
            // numeric kinds. Opcode + signedness derive from the operands'
            // protocol shapes (rule 19); same-shape casts are a passthrough.
            Expr::Cast(e, target) => {
                let (val, src_ty) = self.emit_expr(e)?;
                if std::env::var("BRIEV_SPIRV_DEBUG").is_ok() {
                    eprintln!("[spirv-debug] cast src_ty={src_ty:?} target={target:?}");
                }
                let src_shape = self.builder.shape_of(&src_ty)?;
                let dst_shape = self.builder.shape_of(target)?;
                let op = Self::cast_opcode(&src_shape, &dst_shape)?;;
                match op {
                    None => Ok((val, target.clone())),
                    Some(op) => {
                        let dst_ty_id = self.type_id(target)?;
                        let res = self.builder.gen_id();
                        self.builder.emit(Instruction::new(
                            op,
                            Some(dst_ty_id),
                            Some(res),
                            vec![Operand::IdRef(val)],
                        ));
                        Ok((res, target.clone()))
                    }
                }
            }
            // 2026-08-31 (plan abv-gpu-by-default): unary ops over the same
            // scalar surface — opcode follows the operand's shape (floats
            // negate via 0 − x; SPIR-V has no OpFNegate in the core set
            // without GLSL.std.450 extended instructions).
            Expr::UnaryOp(kind, e) => {
                let (val, ty) = self.emit_expr(e)?;
                let shape = self.builder.shape_of(&ty)?;
                let ty_id = self.type_id(&ty)?;
                match (kind, &shape) {
                    (UnaryOpKind::Neg, SpirvShape::Float { .. }) => {
                        let zero = self.builder.float_const(
                            match &shape { SpirvShape::Float { bits } => *bits, _ => 32 },
                            0.0,
                        );
                        let res = self.builder.gen_id();
                        self.builder.emit(Instruction::new(
                            spirv::Op::FSub,
                            Some(ty_id),
                            Some(res),
                            vec![Operand::IdRef(zero), Operand::IdRef(val)],
                        ));
                        Ok((res, ty))
                    }
                    (UnaryOpKind::Neg, SpirvShape::Int { signed: true, bits }) => {
                        // Zero constant OF THE OPERAND'S type — ISub operands
                        // must share the negated value's width.
                        let zero = match bits {
                            32 => self.builder.builder.constant_bit32(ty_id, 0),
                            64 => self.builder.builder.constant_bit64(ty_id, 0),
                            other => {
                                return self.err(format!(
                                    "negation of {}-bit int — kernels negate 32/64-bit \
                                     values (cast the operand first)",
                                    other
                                ))
                            }
                        };
                        let res = self.builder.gen_id();
                        self.builder.emit(Instruction::new(
                            spirv::Op::ISub,
                            Some(ty_id),
                            Some(res),
                            vec![Operand::IdRef(zero), Operand::IdRef(val)],
                        ));
                        Ok((res, ty))
                    }
                    (UnaryOpKind::Neg, SpirvShape::Int { signed: false, .. }) => {
                        self.err("negating an unsigned value — use a signed type")
                    }
                    (UnaryOpKind::Not, _) => {
                        let bool_ty = self.builder.lower_type(&Type::Bits(1))?;
                        let res = self.builder.gen_id();
                        self.builder.emit(Instruction::new(
                            spirv::Op::LogicalNot,
                            Some(bool_ty),
                            Some(res),
                            vec![Operand::IdRef(val)],
                        ));
                        Ok((res, Type::Bits(1)))
                    }
                    (UnaryOpKind::BitNot, SpirvShape::Int { .. }) => {
                        let res = self.builder.gen_id();
                        self.builder.emit(Instruction::new(
                            spirv::Op::Not,
                            Some(ty_id),
                            Some(res),
                            vec![Operand::IdRef(val)],
                        ));
                        Ok((res, ty))
                    }
                    (UnaryOpKind::Neg, SpirvShape::Bool) => {
                        self.err("negating a Bool — logic needs true/false, not arithmetic")
                    }
                    (UnaryOpKind::BitNot, _) => {
                        self.err("bitwise-not on a non-integer kernel value")
                    }
                }
            }
            Expr::Call(name, args, _) => self.emit_intrinsic_call(name, args),
            Expr::If(cond, then_e, else_e) => {
                let Some(else_expr) = else_e.as_deref() else {
                    return self.err(
                        "if-expression without an else branch produces no value — \
                         add an else arm or use `when` outside kernels",
                    );
                };
                self.emit_bool_selection(cond, then_e, else_expr)
            }
            Expr::Match(scrutinee, arms) => self.emit_bool_match(scrutinee, arms),
            other => self.err(format!(
                "unsupported expression in kernel ({:?}) — integer scalar \
                 compute is the supported surface",
                std::mem::discriminant(other)
            )),
        }
    }

    fn emit_intrinsic_call(&mut self, name: &str, args: &[Expr]) -> Result<(Word, Type), String> {
        match name {
            // Cooperative row kernels (plan 2026-09-01-cooperative-row-kernels):
            // reduce `v` across the invocation's subgroup with a fixed-shape
            // FAdd tree — bit-exact across runs, no atomics.
            "SubgroupFAdd#" => {
                let (v, vty) = match args.first() {
                    Some(e) => self.emit_expr(e)?,
                    None => return self.err("SubgroupFAdd# needs an operand"),
                };
                if !self.builder.is_float_type(&vty)? {
                    return self.err("SubgroupFAdd# is a float reduction");
                }
                let ty_id = self.type_id(&vty)?;
                let res = self.builder.gen_id();
                // Scope is IdScope — a reference to a uint constant, not a
                // literal (spirv-val rejected the literal form).
                let scope = self.builder.u32_const(spirv::Scope::Subgroup as u32);
                self.builder.emit(Instruction::new(
                    spirv::Op::GroupNonUniformFAdd,
                    Some(ty_id),
                    Some(res),
                    vec![
                        Operand::IdRef(scope),
                        Operand::LiteralBit32(spirv::GroupOperation::Reduce as u32),
                        Operand::IdRef(v),
                    ],
                ));
                Ok((res, vty))
            }
            "Exp#" => {
                let (x, xty) = match args.first() {
                    Some(e) => self.emit_expr(e)?,
                    None => return self.err("Exp# needs an operand"),
                };
                if !self.builder.is_float_type(&xty)? {
                    return self.err("Exp# is a float intrinsic");
                }
                let ty_id = self.type_id(&xty)?;
                let res = self.builder.glsl_exp(ty_id, x);
                Ok((res, xty))
            }
            "Sqrt#" => {
                let (x, xty) = match args.first() {
                    Some(e) => self.emit_expr(e)?,
                    None => return self.err("Sqrt# needs an operand"),
                };
                if !self.builder.is_float_type(&xty)? {
                    return self.err("Sqrt# is a float intrinsic");
                }
                let ty_id = self.type_id(&xty)?;
                let res = self.builder.glsl_sqrt(ty_id, x);
                Ok((res, xty))
            }
            "Fabs#" => {
                let (x, xty) = match args.first() {
                    Some(e) => self.emit_expr(e)?,
                    None => return self.err("Fabs# needs an operand"),
                };
                if !self.builder.is_float_type(&xty)? {
                    return self.err("Fabs# is a float intrinsic");
                }
                let ty_id = self.type_id(&xty)?;
                let res = self.builder.glsl_fabs(ty_id, x);
                Ok((res, xty))
            }
            "GetGlobalId#" | "GetLocalId#" => {
                let dim = match args.first() {
                    Some(Expr::Decimal(d)) if *d >= 0 && *d <= 2 => *d as u32,
                    _ => return self.err("builtins take a constant dimension 0..=2"),
                };
                let var = match name {
                    "GetGlobalId#" => self.global_invocation_id()?,
                    _ => self.local_invocation_id()?,
                };
                // Component pointer: AccessChain(var, dim)
                let u32_ty = self.builder.u32_type();
                let ptr_u32 = self.builder.ptr_class(StorageClass::Input, u32_ty);
                let dim_const = self.builder.u32_const(dim);
                let comp = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    spirv::Op::AccessChain,
                    Some(ptr_u32),
                    Some(comp),
                    vec![Operand::IdRef(var), Operand::IdRef(dim_const)],
                ));
                let raw = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    spirv::Op::Load,
                    Some(u32_ty),
                    Some(raw),
                    vec![Operand::IdRef(comp)],
                ));
                // Widen u32 → i64. 2026-08-31 (plan abv-gpu-by-default):
                // Vulkan's Shader environment only allows OpUConvert to an
                // UNSIGNED result — zero-extend to u64 then OpBitcast to the
                // signed i64 the kernel surface uses.
                let int_res_ty = self.builder.lower_type(&Type::int())?;
                let u64_ty = self.builder.builder.type_int(64, 0);
                let wide_u = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    spirv::Op::UConvert,
                    Some(u64_ty),
                    Some(wide_u),
                    vec![Operand::IdRef(raw)],
                ));
                let wide = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    spirv::Op::Bitcast,
                    Some(int_res_ty),
                    Some(wide),
                    vec![Operand::IdRef(wide_u)],
                ));
                Ok((wide, Type::int()))
            }
            "WorkgroupSize#" => {
                // Constant per LocalSize execution mode (64,1,1 set by kernel.rs).
                let dim = match args.first() {
                    Some(Expr::Decimal(d)) if *d >= 0 && *d <= 2 => *d as u32,
                    _ => return self.err("builtins take a constant dimension 0..=2"),
                };
                let sizes = [64u64, 1, 1];
                let c = self.builder.i64_const(sizes[dim as usize]);
                Ok((c, Type::int()))
            }
            "Load#" | "Store#" => {
                // 2026-08-26 (§2.3): raw numeric addresses cannot exist in a
                // Vulkan kernel (no inttoptr over SSBO memory). The honest
                // kernel form of LLVM's raw-address Load#/Store# is an
                // ADDRESS EXPRESSION rooted in the StorageBuffer:
                //   Load#(field) / Load#(field[i]) / Store#(field[i], v)
                // lowered to AccessChain + OpLoad/OpStore on the buffer's
                // typed members. Element widths come from the declared
                // field type, not the byte-count argument.
                if name == "Load#" {
                    let (ptr, elem_ty) = self.emit_addr(args.first().ok_or_else(|| {
                        "SPIR-V lowering: Load# needs an address expression".to_string()
                    })?)?;
                    self.check_width_arg(args.get(1), &elem_ty, "Load#")?;
                    let tid = self.type_id(&elem_ty)?;
                    let loaded = self.builder.load(tid, ptr);
                    Ok((loaded, elem_ty))
                } else {
                    let Some(addr_arg) = args.first() else {
                        return self.err("Store# needs an address expression");
                    };
                    let Some(val_expr) = args.get(1) else {
                        return self.err("Store# needs a value to store");
                    };
                    let (ptr, elem_ty) = self.emit_addr(addr_arg)?;
                    self.check_width_arg(args.get(2), &elem_ty, "Store#")?;
                    let (val, val_ty) = self.emit_expr(val_expr)?;
                    if val_ty != elem_ty {
                        return self.err(format!(
                            "Store# of {:?} into {:?} storage would silently \
                             truncate/convert — cast at the source",
                            val_ty, elem_ty
                        ));
                    }
                    self.builder.store(ptr, val);
                    Ok((val, val_ty))
                }
            }
            other => self.err(format!("unsupported intrinsic '{}'", other)),
        }
    }

    /// 2026-09-02 (plan 2026-09-02-graphics-ray-and-images): two-way value
    /// selection in a kernel. The `if_expr: true` capability is declared, so
    /// the emitter owes the construct: structured selection (the same
    /// OpSelectionMerge + OpBranchConditional shape the bounds guard emits),
    /// values joined by an OpPhi at the merge — the loop-phi emission
    /// pattern, no storage slots (function OpVariables must stay in the
    /// entry block; a phi needs no state).
    ///
    /// selected_block() gives an index into the function's block list;
    /// branches and phis name the LABEL id — convert here.
    fn selected_block_label(&self) -> Word {
        let fidx = self
            .builder
            .builder
            .selected_function()
            .expect("selection arm must be inside a function");
        let bidx = self
            .builder
            .builder
            .selected_block()
            .expect("selection arm must stay inside a block");
        self.builder.builder.module_ref().functions[fidx].blocks[bidx]
            .label_id()
            .expect("block must carry a label id")
    }

    fn emit_bool_selection(
        &mut self,
        cond: &Expr,
        then_e: &Expr,
        else_e: &Expr,
    ) -> Result<(Word, Type), String> {
        let (c, _cty) = self.emit_expr(cond)?;
        let then_bb = self.builder.gen_id();
        let else_bb = self.builder.gen_id();
        let merge_bb = self.builder.gen_id();
        self.builder
            .builder
            .selection_merge(merge_bb, rspirv::spirv::SelectionControl::NONE);
        self.builder
            .builder
            .branch_conditional(c, then_bb, else_bb, [] as [u32; 0]);
        self.builder.begin_block(Some(then_bb));
        let (tv, tty) = self.emit_expr(then_e)?;
        // The arm may itself contain selections (nested match arms) — the
        // phi's predecessor is the block that ACTUALLY branches to the
        // merge, not the arm's entry block. selected_block() is an INDEX
        // into the function's block list; the label id is what branches use.
        let then_end = self.selected_block_label();
        self.builder.builder.branch(merge_bb);
        self.builder.begin_block(Some(else_bb));
        let (ev, ety) = self.emit_expr(else_e)?;
        if tty != ety {
            return self.err(
                "selection arms have different types — both arms must produce \
                 the same type in a kernel",
            );
        }
        let else_end = self.selected_block_label();
        self.builder.builder.branch(merge_bb);
        self.builder.begin_block(Some(merge_bb));
        let ty_id = self.type_id(&tty)?;
        let joined = self
            .builder
            .builder
            .phi(ty_id, None, [(tv, then_end), (ev, else_end)])
            .map_err(|e| format!("selection phi: {:?}", e))?;
        Ok((joined, tty))
    }

    /// `match cond { true => a, false => b }` (SPEC's exhaustive two-way
    /// form) lowers to the same structured selection. Only Bool-scrutinee
    /// literal/wildcard arms are in the kernel surface — other pattern
    /// kinds name their target.
    fn emit_bool_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[crate::ast::MatchArm],
    ) -> Result<(Word, Type), String> {
        let mut then_e: Option<&Expr> = None;
        let mut else_e: Option<&Expr> = None;
        for arm in arms {
            if arm.guard.is_some() {
                return self.err(
                    "match-arm guards are not kernel-computable — move the \
                     condition into the arm pattern or outside the kernel",
                );
            }
            match &arm.pattern {
                crate::ast::Pattern::Literal(Expr::Bool(true)) => {
                    if then_e.is_some() {
                        return self.err("duplicate `true` match arm");
                    }
                    then_e = Some(&arm.body);
                }
                crate::ast::Pattern::Literal(Expr::Bool(false)) => {
                    if else_e.is_some() {
                        return self.err("duplicate `false` match arm");
                    }
                    else_e = Some(&arm.body);
                }
                crate::ast::Pattern::Wildcard => {
                    if else_e.is_none() {
                        else_e = Some(&arm.body);
                    } else if then_e.is_none() {
                        then_e = Some(&arm.body);
                    }
                }
                _ => {
                    return self.err(
                        "only `true`/`false`/`_` match arms are kernel-computable — \
                         other patterns belong on the host side",
                    );
                }
            }
        }
        let (Some(then_e), Some(else_e)) = (then_e, else_e) else {
            return self.err(
                "kernel match must be exhaustive — provide both a `true` and a \
                 `false` arm (or `_`)",
            );
        };
        self.emit_bool_selection(scrutinee, then_e, else_e)
    }

    /// O2: lower float `mul + addend` (either order) as a single Fma.
    /// Returns Ok(None) when the shape is not a float mul-add; the caller
    /// then lowers normally (the speculative ids here are dead code).
    fn try_emit_float_fma(&mut self, l: &Expr, r: &Expr)
        -> Result<Option<(Word, Type)>, String>
    {
        use crate::ast::BinaryOpKind::Mul;
        let (mul_operands, addend) = match (l, r) {
            (Expr::BinaryOp(Mul, a, b), c) => ((a.as_ref(), b.as_ref()), c),
            (c, Expr::BinaryOp(Mul, a, b)) => ((a.as_ref(), b.as_ref()), c),
            _ => return Ok(None),
        };
        let (aid, aty) = self.emit_expr(mul_operands.0)?;
        let (bid, bty) = self.emit_expr(mul_operands.1)?;
        if !(self.builder.is_float_type(&aty)? && self.builder.is_float_type(&bty)?) {
            return Ok(None);
        }
        let (cid, cty) = self.emit_expr(addend)?;
        if !self.builder.is_float_type(&cty)? {
            // Mixed float/int add — the generic path owns this error.
            return Ok(None);
        }
        let aid = self.coerce(aid, &aty, &bty)?;
        let bid = self.coerce(bid, &bty, &aty)?;
        let cid = self.coerce(cid, &cty, &aty)?;
        let ty_id = self.type_id(&aty)?;
        let res = self.builder.glsl_fma(ty_id, aid, bid, cid);
        Ok(Some((res, aty)))
    }

    fn emit_binop(&mut self, kind: &crate::ast::BinaryOpKind, l: &Expr, r: &Expr)
        -> Result<(Word, Type), String>
    {
        use crate::ast::BinaryOpKind::*;
        // O2 (plan 2026-08-31-gpu-next): fuse float `a*b + c` into one
        // GLSL.std.450 Fma BEFORE the generic lowering splits it into FMul +
        // FAdd. One rounding (better numerics) and no dependence on driver
        // contraction. Non-matching / non-float shapes fall through
        // untouched; the speculative sub-lowering leaves only dead ids
        // behind (side-effect-free, eliminated by the driver).
        if matches!(kind, Add) {
            if let Some(fused) = self.try_emit_float_fma(l, r)? {
                return Ok(fused);
            }
        }
        let (lid, lty) = self.emit_expr(l)?;
        let (rid, rty) = self.emit_expr(r)?;
        // 2026-08-31 (plan abv-gpu-by-default): opcode selection is driven by
        // the OPERANDS' protocol category — float-shaped operands get the F*
        // opcode family and their own result type; everything else keeps the
        // integer family. No type names are matched (rule 19).
        let float_lane = self.builder.is_float_type(&lty)?
            || self.builder.is_float_type(&rty)?;
        if float_lane {
            return self.emit_float_binop(kind, lid, lty.clone(), rid, rty.clone());
        }
        let result_int = self.builder.lower_type(&Type::int())?;
        let op = match kind {
            Add => spirv::Op::IAdd,
            Sub => spirv::Op::ISub,
            Mul => spirv::Op::IMul,
            Div => spirv::Op::SDiv,
            Mod => spirv::Op::SRem,
            BitAnd => spirv::Op::BitwiseAnd,
            BitOr => spirv::Op::BitwiseOr,
            BitXor => spirv::Op::BitwiseXor,
            Shl => spirv::Op::ShiftLeftLogical,
            Shr => spirv::Op::ShiftRightArithmetic,
            Lt => spirv::Op::SLessThan,
            Gt => spirv::Op::SGreaterThan,
            Le => spirv::Op::SLessThanEqual,
            Ge => spirv::Op::SGreaterThanEqual,
            Eq => spirv::Op::IEqual,
            Neq => spirv::Op::INotEqual,
            And | Or => {
                // Logical over bool operands.
                let op = if matches!(kind, And) { spirv::Op::LogicalAnd } else { spirv::Op::LogicalOr };
                let bool_ty = self.builder.lower_type(&Type::Bits(1))?;
                let res = self.builder.gen_id();
                self.builder.emit(Instruction::new(
                    op,
                    Some(bool_ty),
                    Some(res),
                    vec![Operand::IdRef(lid), Operand::IdRef(rid)],
                ));
                return Ok((res, Type::Bits(1)));
            }
            Concat => return self.err("string concat is not a kernel operation"),
        };
        let is_cmp = matches!(kind, Lt | Gt | Le | Ge | Eq | Neq);
        let res_ty = if is_cmp {
            self.builder.lower_type(&Type::Bits(1))?
        } else {
            result_int
        };
        // Both operands must share the lowered type id (Int vs Bits widths).
        let lid = self.coerce(lid, &lty, &rty)?;
        let rid = self.coerce(rid, &rty, &lty)?;
        let res = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            op,
            Some(res_ty),
            Some(res),
            vec![Operand::IdRef(lid), Operand::IdRef(rid)],
        ));
        Ok((
            res,
            if is_cmp { Type::Bits(1) } else { lty },
        ))
    }

    /// 2026-08-31 (plan abv-gpu-by-default): the float opcode family. Mixed
    /// float/int operands error (cast at the source — same policy as Store#
    /// width checks); bitwise ops on floats are not kernel operations.
    fn emit_float_binop(
        &mut self,
        kind: &crate::ast::BinaryOpKind,
        lid: Word,
        lty: Type,
        rid: Word,
        rty: Type,
    ) -> Result<(Word, Type), String> {
        use crate::ast::BinaryOpKind::*;
        if !self.builder.is_float_type(&rty)? {
            return self.err(format!(
                "mixed float/int arithmetic ({:?} on {:?} and {:?}) — cast the \
                 integer operand to the float type at the source",
                kind, lty, rty
            ));
        }
        let op = match kind {
            Add => spirv::Op::FAdd,
            Sub => spirv::Op::FSub,
            Mul => spirv::Op::FMul,
            Div => spirv::Op::FDiv,
            Mod => spirv::Op::FRem,
            Lt => spirv::Op::FOrdLessThan,
            Gt => spirv::Op::FOrdGreaterThan,
            Le => spirv::Op::FOrdLessThanEqual,
            Ge => spirv::Op::FOrdGreaterThanEqual,
            Eq => spirv::Op::FOrdEqual,
            Neq => spirv::Op::FOrdNotEqual,
            BitAnd | BitOr | BitXor | Shl | Shr => {
                return self.err("bitwise ops on floats are not kernel operations")
            }
            And | Or => return self.err("logical ops need Bool operands, not floats"),
            Concat => return self.err("string concat is not a kernel operation"),
        };
        let is_cmp = matches!(kind, Lt | Gt | Le | Ge | Eq | Neq);
        let res_ty = if is_cmp {
            self.builder.lower_type(&Type::Bits(1))?
        } else {
            // coerce enforces both sides share the lowered type id first.
            let lid = self.coerce(lid, &lty, &rty)?;
            let rid = self.coerce(rid, &rty, &lty)?;
            let ty_id = self.type_id(&lty)?;
            let res = self.builder.gen_id();
            self.builder.emit(Instruction::new(
                op,
                Some(ty_id),
                Some(res),
                vec![Operand::IdRef(lid), Operand::IdRef(rid)],
            ));
            return Ok((res, lty));
        };
        let lid = self.coerce(lid, &lty, &rty)?;
        let rid = self.coerce(rid, &rty, &lty)?;
        let res = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            op,
            Some(res_ty),
            Some(res),
            vec![Operand::IdRef(lid), Operand::IdRef(rid)],
        ));
        Ok((res, Type::Bits(1)))
    }

    /// Load a scalar and widen 16-bit floats to f32 (2026-09-02): 16-bit
    /// floats are a STORAGE format — compute happens in f32 (Function
    /// storage forbids 16-bit variables; the constant pool is f32), so the
    /// load OpFConverts at the boundary. Other types load as-is. Undo:
    /// restore the raw `builder.load` at the two SSBO load sites.
    fn load_widened(&mut self, ptr: Word, ty: Type) -> Result<(Word, Type), String> {
        let tid = self.type_id(&ty)?;
        let loaded = self.builder.load(tid, ptr);
        if matches!(
            self.builder.shape_of(&ty)?,
            crate::casting::graph::SpirvShape::Float { bits: 16 }
        ) {
            let dst = Type::float();
            let dst_id = self.type_id(&dst)?;
            let res = self.builder.gen_id();
            self.builder.emit(Instruction::new(
                spirv::Op::FConvert,
                Some(dst_id),
                Some(res),
                vec![Operand::IdRef(loaded)],
            ));
            return Ok((res, dst));
        }
        Ok((loaded, ty))
    }

    /// Zero-width mismatches are a lowering bug; identical types pass through.
    fn coerce(&mut self, id: Word, ty: &Type, other: &Type) -> Result<Word, String> {
        if self.type_id(ty)? == self.type_id(other)? {
            Ok(id)
        } else {
            self.err(format!("operand type mismatch {:?} vs {:?}", ty, other))
        }
    }

/// 2026-08-31 (plan abv-gpu-by-default): scalar cast opcode from the source
/// and destination shapes. `None` = identity (same kind + width — the value
/// passes through). The INT side's signedness picks the S/U conversion
/// family; float width changes pick trunc/ext. Bool mixes have no direct
/// scalar conversion and error naming the fix.
fn cast_opcode(
    src: &crate::casting::graph::SpirvShape,
    dst: &crate::casting::graph::SpirvShape,
) -> Result<Option<spirv::Op>, String> {
    use crate::casting::graph::SpirvShape;
    let same_kind = std::mem::discriminant(src) == std::mem::discriminant(dst);
    Ok(match (src, dst) {
        (SpirvShape::Int { bits: x, .. }, SpirvShape::Int { bits: y, .. }) if same_kind && x == y => None,
        (SpirvShape::Int { signed, .. }, SpirvShape::Int { .. }) if same_kind => {
            Some(if *signed { spirv::Op::SConvert } else { spirv::Op::UConvert })
        }
        (SpirvShape::Float { bits: x }, SpirvShape::Float { bits: y }) if same_kind && x == y => None,
        (SpirvShape::Float { bits: x }, SpirvShape::Float { bits: y }) if same_kind => {
            // SPIR-V has one OpFConvert for both float width directions.
            let _ = (x, y);
            Some(spirv::Op::FConvert)
        }
        (SpirvShape::Bool, SpirvShape::Bool) => None,
        (SpirvShape::Int { signed, .. }, SpirvShape::Float { .. }) => {
            Some(if *signed { spirv::Op::ConvertSToF } else { spirv::Op::ConvertUToF })
        }
        (SpirvShape::Float { .. }, SpirvShape::Int { signed, .. }) => {
            Some(if *signed { spirv::Op::ConvertFToS } else { spirv::Op::ConvertFToU })
        }
        _ => {
            return Err(format!(
                "cast {:?} → {:?} is not a kernel scalar conversion — bool mixes \
                 materialize explicitly (0/1 arithmetic)",
                src, dst
            ))
        }
    })
}

// ── State (SSBO) ────────────────────────────────────────────────────
    /// O3 helpers: member byte size (shared by the offset walk) and the
    /// vec4 eligibility / member-type construction.
    /// Device storage bytes for a field type (element × count for arrays).
    /// Pub: the gpu_schedule analysis computes per-array sizes from this so
    /// its slot-reuse greedy never pairs size-mismatched arrays.
    pub fn field_storage_bytes(builder: &mut SpirvBuilder, ty: &Type) -> Result<u32, String> {
        match ty {
            Type::Vector(inner, dims) => {
                let elems: u32 = dims
                    .iter()
                    .map(|d| match d {
                        crate::ast::Dimension::Anonymous(n) => *n as u32,
                        crate::ast::Dimension::Named(_, n) => *n as u32,
                    })
                    .product::<u32>()
                    .max(1);
                Ok(builder.scalar_storage_bytes(inner)? * elems)
            }
            // 2026-09-14 (Matrix type plan): a shape-bearing Applied type's
            // storage is elem(arg[0]) × rows × cols × depth.
            Type::Applied(_, args) => {
                if let Some((r, c, d)) = builder.matrix_shape(ty) {
                    let elem = builder.scalar_storage_bytes(args.first().unwrap_or(&Type::int()))?;
                    Ok(elem * (r as u32) * (c as u32) * (d as u32))
                } else {
                    builder.scalar_storage_bytes(ty)
                }
            }
            other => builder.scalar_storage_bytes(other),
        }
    }

    /// THE projection layout rule (plan 2026-09-01-vec4-projection-layout):
    /// fields are name-sorted (caller's duty) and packed, EXCEPT that a
    /// field the vec4 gate would accept (Float32 array, count % 4 == 0) is
    /// aligned up to 16B so it can be declared as an array-of-v4float and
    /// wide-loaded. Consumers: `setup_state_buffer` (member decorations),
    /// `runner::ssbo_layout` (runner BrievField literals), and the LLVM
    /// descriptor path — device offsets have exactly this one definition.
    /// The HOST state layouts (runner `state[]`, LLVM %State) stay packed;
    /// the RT copies per field, so host ≠ projection needs no bridging.
    ///
    /// `reuse_map` (Phase 3 — buffer reuse): maps aliased_field → target_field.
    /// If present, the aliased field's offset is set to the target's offset
    /// (the arrays share a device slot). Both kernel and runner call this
    /// with the same map, so they agree on the layout.
    ///
    /// The map is filtered to size-compatible pairs first: an alias whose
    /// target has a different device storage size would corrupt (a scalar
    /// sharing an array's slot). `filtered_reuse_map` is the single filter.
    pub fn projection_offsets(
        builder: &mut SpirvBuilder,
        fields: &[StateField],
        reuse_map: Option<&HashMap<String, String>>,
    ) -> Result<Vec<u32>, String> {
        let reuse = Self::filtered_reuse_map(builder, fields, reuse_map)?;
        let mut offsets = Vec::with_capacity(fields.len());
        let mut offset: u32 = 0;
        for f in fields {
            // 2026-09-14 (gpu_schedule): align EVERY array to 16 bytes — the
            // tensor kernels read A/B in 16-byte `ld.global.nc.v4.u32` chunks
            // (8 f16 OR 4 f32), so an 8-aligned f16 array like q@65576
            // (65576 % 16 == 8) made the fused attention's fill read
            // misaligned. vec4 (f32) arrays aligned before; f16 now too.
            if Self::is_vector_type(&f.ty)? {
                offset = offset.next_multiple_of(16);
            }
            offsets.push(offset);
            offset += Self::field_storage_bytes(builder, &f.ty)?;
        }
        // Phase 3 — buffer reuse: remap aliased fields to their target's
        // offset. The target was computed earlier (name-sorted order), so
        // its offset is final. The aliased field shares the same device slot.
        for (i, f) in fields.iter().enumerate() {
            if let Some(target_name) = reuse.get(&f.name) {
                if let Some(target_idx) = fields
                    .iter()
                    .position(|tf| tf.name == *target_name)
                {
                    offsets[i] = offsets[target_idx];
                }
            }
        }
        Ok(offsets)
    }

    /// Phase 3 — buffer reuse: keep only aliases whose target has the SAME
    /// device storage size as the aliased field. An array and a scalar share
    /// nothing; a 1024-float array cannot reuse an 8-byte scalar slot. Both
    /// `projection_offsets` and `setup_state_buffer` call this so the runner
    /// field table and the kernel SSBO struct never disagree.
    pub fn filtered_reuse_map(
        builder: &mut SpirvBuilder,
        fields: &[StateField],
        reuse_map: Option<&HashMap<String, String>>,
    ) -> Result<HashMap<String, String>, String> {
        let Some(reuse) = reuse_map else {
            return Ok(HashMap::new());
        };
        let size_of = |builder: &mut SpirvBuilder, name: &str| -> Result<Option<u32>, String> {
            match fields.iter().find(|f| f.name == name) {
                Some(f) => Ok(Some(Self::field_storage_bytes(builder, &f.ty)?)),
                None => Ok(None),
            }
        };
        let mut out = HashMap::new();
        for (aliased, target) in reuse {
            let Some(a) = size_of(builder, aliased)? else { continue };
            let Some(t) = size_of(builder, target)? else { continue };
            if a == t {
                out.insert(aliased.clone(), target.clone());
            }
        }
        Ok(out)
    }

    /// Shape half of the vec4 gate — offset-independent (Float32 array,
    /// count % 4 == 0). The layout rule aligns these fields so the offset
    /// half holds by construction.
    /// 2026-09-14 (gpu_schedule): true for any state ARRAY (vector type) —
    /// the projection aligns all arrays to 16 bytes (tensor kernels read
    /// A/B in 16-byte chunks).
    fn is_vector_type(ty: &Type) -> Result<bool, String> {
        Ok(matches!(ty, Type::Vector(..)))
    }

    fn vec4_shape_eligible(builder: &mut SpirvBuilder, ty: &Type) -> Result<bool, String> {
        let Type::Vector(inner, dims) = ty else {
            return Ok(false);
        };
        if builder.scalar_storage_bytes(inner)? != 4 {
            return Ok(false);
        }
        let elems: u32 = dims
            .iter()
            .map(|d| match d {
                crate::ast::Dimension::Anonymous(n) => *n as u32,
                crate::ast::Dimension::Named(_, n) => *n as u32,
            })
            .product::<u32>()
            .max(1);
        Ok(elems % 4 == 0)
    }

    fn vec4_eligible(builder: &mut SpirvBuilder, ty: &Type, offset: u32) -> Result<bool, String> {
        if offset % 16 != 0 {
            return Ok(false);
        }
        Self::vec4_shape_eligible(builder, ty)
    }

    /// OpTypeArray(vec4, N/4) with ArrayStride 16 — byte-identical to the
    /// scalar array it replaces (count % 4 == 0 is the eligibility gate).
    fn vec4_member_type(builder: &mut SpirvBuilder, ty: &Type) -> Result<Word, String> {
        let Type::Vector(inner, dims) = ty else {
            return Err("vec4_member_type on a non-array".into());
        };
        let elems: u32 = dims
            .iter()
            .map(|d| match d {
                crate::ast::Dimension::Anonymous(n) => *n as u32,
                crate::ast::Dimension::Named(_, n) => *n as u32,
            })
            .product::<u32>()
            .max(1);
        let scalar = builder.lower_type(inner)?;
        Ok(builder.vec4_array_type(scalar, elems))
    }

    /// D3 (beyond-coopmat Stage 1): f16 arrays with even counts retype
    /// their SSBO member to array-of-vNf16 (byte-identical: same offsets,
    /// ArrayStride 2·N) so the smem fill loads/stores wide chunks with
    /// one op. Gated on the fill knobs via `pair_view_fields` (width 2 =
    /// pairs, 4 = quads); vec4 eligibility never fires for f16 (32-bit
    /// float-shaped gate) so the two views cannot collide.
    fn view_width(&mut self, name: &str, ty: &Type) -> Result<Option<u32>, String> {
        let Some(width) = self.pair_view_fields.iter().find(|(n, _)| n == name).map(|(_, w)| *w)
        else {
            return Ok(None);
        };
        let Type::Vector(inner, dims) = ty else {
            return Ok(None);
        };
        let elems: u32 = dims
            .iter()
            .map(|d| match d {
                crate::ast::Dimension::Anonymous(n) => *n as u32,
                crate::ast::Dimension::Named(_, n) => *n as u32,
            })
            .product::<u32>()
            .max(1);
        if elems % width != 0 {
            return Err(format!(
                "f16 view field '{}' has {} elements, not divisible by the width {}",
                name, elems, width
            ));
        }
        Ok(matches!(
            self.builder.shape_of(inner),
            Ok(crate::casting::graph::SpirvShape::Float { bits: 16 })
        )
        .then_some(width))
    }

    /// OpTypeArray(vNf16, N/width) with ArrayStride 2·width —
    /// byte-identical to the half array it replaces (the count is
    /// divisible by the width; see view_width).
    fn pair_member_type(
        builder: &mut SpirvBuilder,
        ty: &Type,
        width: u32,
    ) -> Result<Word, String> {
        let Type::Vector(inner, dims) = ty else {
            return Err("pair_member_type on a non-array".into());
        };
        let elems: u32 = dims
            .iter()
            .map(|d| match d {
                crate::ast::Dimension::Anonymous(n) => *n as u32,
                crate::ast::Dimension::Named(_, n) => *n as u32,
            })
            .product::<u32>()
            .max(1);
        let scalar = builder.lower_type(inner)?;
        let view = builder.builder.type_vector(scalar, width);
        let view_count = builder.u32_const(elems / width);
        // rspirv dedups identical OpTypeArrays — a second ArrayStride on
        // the deduped id would be invalid (the vec4 machinery notes the
        // same trap). Decorate only on first creation.
        let existing = builder
            .module_ref()
            .types_global_values
            .iter()
            .find(|i| {
                i.class.opcode == spirv::Op::TypeArray
                    && i.result_type.is_none()
                    && i.operands.first() == Some(&Operand::IdRef(view))
                    && i.operands.get(1) == Some(&Operand::IdRef(view_count))
            })
            .and_then(|i| i.result_id);
        if let Some(arr) = existing {
            return Ok(arr);
        }
        let arr = builder.builder.type_array(view, view_count);
        builder.emit_global(Instruction::new(
            spirv::Op::Decorate, None, None,
            vec![
                Operand::IdRef(arr),
                Operand::Decoration(spirv::Decoration::ArrayStride),
                Operand::LiteralBit32(2 * width),
            ],
        ));
        Ok(arr)
    }

    /// Phase 3 — buffer reuse: record the alias map (aliased field → target
    /// field) on the lowerer and return the indices of aliased fields (they
    /// are skipped from the SSBO struct). No-op when `reuse_map` is None.
    fn apply_reuse_aliases(
        &mut self,
        reuse_map: Option<&HashMap<String, String>>,
    ) -> HashSet<usize> {
        let Some(reuse) = reuse_map else {
            return HashSet::new();
        };
        let mut aliased = HashSet::new();
        for (i, f) in self.state_fields.iter().enumerate() {
            if reuse.contains_key(&f.name) {
                aliased.insert(i);
            }
        }
        for (name, target) in reuse {
            self.alias_map.insert(name.clone(), target.clone());
        }
        aliased
    }

    /// Build the SSBO struct member type ids. Aliased fields are skipped
    /// (their device slot is occupied by the target). Vec4-typed members use
    /// their ARRAY-OF-VEC4 id; pair (v2f16) members the paired smem fill id.
    fn build_member_ids(
        &mut self,
        field_types: &[Type],
        pair_ids: &[(String, Word, Type)],
        aliased: &HashSet<usize>,
    ) -> Result<Vec<Word>, String> {
        let mut members = Vec::with_capacity(field_types.len());
        for (idx, ty) in field_types.iter().enumerate() {
            if aliased.contains(&idx) {
                continue;
            }
            if let Some(p) = pair_ids.iter().find(|(n, _, _)| *n == self.state_fields[idx].name) {
                members.push(p.1);
            } else {
                match self.vec4_fields.get(&self.state_fields[idx].name) {
                    Some(vf) => members.push(vf.array),
                    None => members.push(self.type_id(ty)?),
                }
            }
        }
        Ok(members)
    }

    /// Declare the StorageBuffer struct over collected fields (sorted by
    /// name — determinism rule) and create its variable. Called BEFORE any
    /// body statement lowers.
    pub fn setup_state_buffer(&mut self, reuse_map: Option<&HashMap<String, String>>) -> Result<(), String> {        if self.state_fields.is_empty() {
            return Ok(());
        }
        self.state_fields.sort_by(|a, b| a.name.cmp(&b.name));
        let field_types: Vec<Type> =
            self.state_fields.iter().map(|f| f.ty.clone()).collect();
        // THE layout rule (plan 2026-09-01-vec4-projection-layout): shared
        // with the runner + LLVM descriptor paths; vec4-eligible arrays are
        // 16B-aligned, everything else packed.
        let proj_offsets = Self::projection_offsets(self.builder, &self.state_fields, reuse_map)?;
        // Phase 3 — buffer reuse: identify aliased fields (skipped from the
        // SSBO struct; AccessChain remaps to the target's member index) and
        // record the alias map for body lowering. Filtered to size-compatible
        // pairs — the same filter projection_offsets applied.
        let reuse = Self::filtered_reuse_map(self.builder, &self.state_fields, reuse_map)?;
        let aliased = self.apply_reuse_aliases(Some(&reuse));
        let mut vec4_ids: Vec<(String, Word, Type)> = Vec::new();
        let mut pair_ids: Vec<(String, Word, Type)> = Vec::new();
        let state_fields_snapshot: Vec<(String, Type)> = self
            .state_fields
            .iter()
            .map(|f| (f.name.clone(), f.ty.clone()))
            .collect();
        for (fname, fty) in state_fields_snapshot.iter() {
            let off = proj_offsets[self
                .state_fields
                .iter()
                .position(|f| &f.name == fname)
                .unwrap_or(0)];
            let f = (fname, fty);
            if let Some(width) = self.view_width(f.0, f.1)? {
                let arr_id = Self::pair_member_type(self.builder, f.1, width)?;
                pair_ids.push((f.0.clone(), arr_id, f.1.clone()));
            } else if Self::vec4_eligible(self.builder, f.1, off)? {
                let arr_id = Self::vec4_member_type(self.builder, f.1)?;
                vec4_ids.push((f.0.clone(), arr_id, f.1.clone()));
            }
        }
        for (name, arr_id, ty) in &vec4_ids {
            // The VECTOR id is the first operand of the array type's
            // OpTypeArray; the scalar element type comes from the field.
            let vector = match self.builder.module_ref()
                .types_global_values
                .iter()
                .find(|i| i.result_id == Some(*arr_id))
                .and_then(|i| i.operands.first())
            {
                Some(rspirv::dr::Operand::IdRef(v)) => *v,
                _ => return Err("vec4 member type lost its element".into()),
            };
            let Type::Vector(inner, _) = ty else {
                return Err("vec4 field on a non-array".into());
            };
            let elem_float = self.builder.is_float_type(inner)?;
            self.vec4_fields.insert(
                name.clone(),
                Vec4Field {
                    array: *arr_id,
                    vector,
                    elem: (**inner).clone(),
                    elem_float,
                },
            );
        }
        let members = self.build_member_ids(&field_types, &pair_ids, &aliased)?;
        let member_ids: Vec<Word> = members.clone();
        let struct_ty = self.builder.builder.type_struct(member_ids);
        // Block decoration (required for SSBO interface).
                self.builder
            .decorate_raw(struct_ty, spirv::Decoration::Block, vec![]);
        // Explicit member offsets — Block structs must be fully laid out.
        // Only non-aliased fields get Offset decorations (aliased fields
        // share their target's offset and are not SSBO members).
        let mut member_idx: u32 = 0;
        for (idx, &off) in proj_offsets.iter().enumerate() {
            if aliased.contains(&idx) {
                continue;
            }
            self.builder.builder.member_decorate(
                struct_ty,
                member_idx,
                spirv::Decoration::Offset,
                [rspirv::dr::Operand::LiteralBit32(off)],
            );
            member_idx += 1;
        }
        let struct_ptr = self.builder.ptr_class(StorageClass::StorageBuffer, struct_ty);
        let var = self.builder.gen_id();
        self.builder.emit_global(Instruction::new(
            spirv::Op::Variable,
            Some(struct_ptr),
            Some(var),
            vec![Operand::StorageClass(StorageClass::StorageBuffer)],
        ));
        self.builder.decorate_raw(
            var,
            spirv::Decoration::DescriptorSet,
            vec![rspirv::dr::Operand::LiteralBit32(0)],
        );
        self.builder.decorate_raw(
            var,
            spirv::Decoration::Binding,
            vec![rspirv::dr::Operand::LiteralBit32(0)],
        );
        self.ssbo_var = Some(var);
        Ok(())
    }

    /// 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): declare
    /// the UniformConstant storage-image variables for this kernel's
    /// planned arrays — OpTypeImage (typed builder), pointer, variable,
    /// DescriptorSet 0 / Binding 1+ (images bind AFTER the SSBO's set-0
    /// binding 0). The runtime (step 4) allocates the matching VkImage per
    /// binding slot.
    pub fn declare_images(&mut self) -> Result<(), String> {
        let names: Vec<String> = self.image_plans.keys().cloned().collect();
        let mut names = names;
        names.sort();
        for (slot, name) in names.iter().enumerate() {
            let plan = &self.image_plans[name];
            let fmt = self.spirv_image_format(&plan.format)?;
            let f32_ty = self.builder.lower_type(&Type::float())?;
            let img_ty = self
                .builder
                .builder
                .type_image(f32_ty, spirv::Dim::Dim2D, 0, 0, 0, 2, fmt, None);
            let ptr_ty = self
                .builder
                .builder
                .type_pointer(None, spirv::StorageClass::UniformConstant, img_ty);
            let var = self.builder.gen_id();
            self.builder.emit_global(Instruction::new(
                spirv::Op::Variable,
                Some(ptr_ty),
                Some(var),
                vec![Operand::StorageClass(spirv::StorageClass::UniformConstant)],
            ));
            self.builder.decorate_raw(
                var,
                spirv::Decoration::DescriptorSet,
                vec![rspirv::dr::Operand::LiteralBit32(0)],
            );
            self.builder.decorate_raw(
                var,
                spirv::Decoration::Binding,
                vec![rspirv::dr::Operand::LiteralBit32(1 + slot as u32)],
            );
            self.image_vars.insert(name.clone(), var);
            self.image_types.insert(name.clone(), img_ty);
        }
        Ok(())
    }

    /// The backend's texel-format table: analysis-carried format STRING →
    /// SPIR-V ImageFormat. Unknown format = loud error naming the set
    /// (the parser never validates device formats — this is the one place
    /// that knows them).
    fn spirv_image_format(&self, format: &str) -> Result<spirv::ImageFormat, String> {
        match format {
            "R32Float" => Ok(spirv::ImageFormat::R32f),
            other => Err(format!(
                "unknown texel format '{}' — supported: R32Float (more join as \
                 the runtime gains their VkImage paths)",
                other
            )),
        }
    }

    /// The image-write path for `planned[i] = value`: coords (i % W, i / W)
    /// (the same decomposition the plan's detection extracted), texel =
    /// scalar float (R32Float). The value must lower to f32 — R32 IS a
    /// float child; anything else is a loud error, never a silent convert.
    fn emit_image_write(
        &mut self,
        array: &str,
        idx: &Expr,
        val: Word,
        val_ty: &Type,
    ) -> Result<(), String> {
        let Some(var) = self.image_vars.get(array).copied() else {
            return self.err(format!("image plan for '{}' has no variable", array));
        };
        if !self.builder.is_float_type(val_ty)? {
            return self.err(format!(
                "image store into '{}' requires a float value — texel formats \
                 are float-backed in this slice",
                array
            ));
        }
        let plan = &self.image_plans[array];
        let width = self.builder.i64_const(plan.width as u64);
        let i_reg = self.emit_expr(idx)?.0;
        let int_ty = self.builder.lower_type(&Type::int())?;
        // i64 → u32 coords: OpSConvert truncates to the low 32 bits — the
        // plan guarantees i < width*height ≤ u32::MAX.
        let u32_ty = self.builder.u32_type();
        let conv = |me: &mut Self, v: Word| -> Result<Word, String> {
            let id = me.builder.gen_id();
            me.builder.emit(Instruction::new(
                spirv::Op::SConvert,
                Some(u32_ty),
                Some(id),
                vec![Operand::IdRef(v)],
            ));
            Ok(id)
        };
        let rem = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            spirv::Op::SRem,
            Some(int_ty),
            Some(rem),
            vec![Operand::IdRef(i_reg), Operand::IdRef(width)],
        ));
        let quo = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            spirv::Op::SDiv,
            Some(int_ty),
            Some(quo),
            vec![Operand::IdRef(i_reg), Operand::IdRef(width)],
        ));
        let cx = conv(self, rem)?;
        let cy = conv(self, quo)?;
        let coord_ty = self.builder.builder.type_vector(u32_ty, 2);
        let coords = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            spirv::Op::CompositeConstruct,
            Some(coord_ty),
            Some(coords),
            vec![Operand::IdRef(cx), Operand::IdRef(cy)],
        ));
        let image_ty = *self
            .image_types
            .get(array)
            .ok_or_else(|| "image variable lost its type".to_string())?;
        let image = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            spirv::Op::Load,
            Some(image_ty),
            Some(image),
            vec![Operand::IdRef(var)],
        ));
        self.builder.emit(Instruction::new(
            spirv::Op::ImageWrite,
            None,
            None,
            vec![
                Operand::IdRef(image),
                Operand::IdRef(coords),
                Operand::IdRef(val),
            ],
        ));
        Ok(())
    }

    /// Phase 3 — buffer reuse: the SSBO struct member index of the field at
    /// `pos` in `state_fields`. Aliased fields are skipped from the struct
    /// (their device slot is the target's), so the member index counts only
    /// non-aliased fields before `pos`. Without this, AccessChain used the
    /// raw position and walked past the struct end when aliases collapsed it.
    fn struct_member_index(&self, pos: usize) -> u32 {
        self.state_fields[..pos]
            .iter()
            .filter(|f| !self.alias_map.contains_key(&f.name))
            .count() as u32
    }

    /// AccessChain to `field[idx]` inside the SSBO. Returns (elem ptr, elem ty).
    fn state_field_elem_ptr(&mut self, field: &str, idx: Word) -> Result<(Word, Type), String> {
        let Some(var) = self.ssbo_var else {
            return self.err("kernel touches state but no state fields were collected");
        };
        // Phase 3 — buffer reuse: aliased fields are not SSBO members;
        // remap to the target's member index.
        let effective_name: String = self.alias_map.get(field).cloned().unwrap_or_else(|| field.to_string());
        let Some(pos) = self.state_fields.iter().position(|f| f.name == effective_name) else {
            return self.err(format!("state field '{}' was not declared", effective_name));
        };
        let fty = self.state_fields[pos].ty.clone();
        let elem_ty = match &fty {
            Type::Vector(inner, _) => (**inner).clone(),
            other => other.clone(),
        };
        let elem_id = self.type_id(&elem_ty)?;
        // Chain: ssbo var → member index → element index. The member index
        // counts only NON-aliased fields (aliased members are skipped from
        // the SSBO struct, so positions in state_fields ≠ member indices).
        let member_idx = self.builder.u32_const(self.struct_member_index(pos));
        let ptr_ty = self.builder.ptr_class(StorageClass::StorageBuffer, elem_id);
        let chain = self.builder.gen_id();
        if self.vec4_fields.contains_key(&effective_name) {
            // O3: the member is an array of 4-wide vectors (byte-identical
            // layout). The scalar element lives at [idx >> 2][idx & 3].
            let int_ty = self.type_id(&Type::int())?;
            let two = self.builder.i64_const(2);
            let q = self.builder.gen_id();
            self.builder.emit(Instruction::new(
                spirv::Op::ShiftRightArithmetic,
                Some(int_ty),
                Some(q),
                vec![Operand::IdRef(idx), Operand::IdRef(two)],
            ));
            let three = self.builder.i64_const(3);
            let r = self.builder.gen_id();
            self.builder.emit(Instruction::new(
                spirv::Op::BitwiseAnd,
                Some(int_ty),
                Some(r),
                vec![Operand::IdRef(idx), Operand::IdRef(three)],
            ));
            self.builder.emit(Instruction::new(
                spirv::Op::AccessChain,
                Some(ptr_ty),
                Some(chain),
                vec![
                    Operand::IdRef(var),
                    Operand::IdRef(member_idx),
                    Operand::IdRef(q),
                    Operand::IdRef(r),
                ],
            ));
            return Ok((chain, elem_ty));
        }
        self.builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(ptr_ty),
            Some(chain),
            vec![
                Operand::IdRef(var),
                Operand::IdRef(member_idx),
                Operand::IdRef(idx),
            ],
        ));
        Ok((chain, elem_ty))
    }

    /// 2026-08-26 (§2.3): AccessChain to a SCALAR field inside the SSBO
    /// (member index only — no element subscript). Returns (ptr, field ty).
    fn state_field_scalar_ptr(&mut self, field: &str) -> Result<(Word, Type), String> {
        let Some(var) = self.ssbo_var else {
            return self.err("kernel touches state but no state fields were collected");
        };
        // Phase 3 — buffer reuse: remap aliased fields to target member.
        let effective_name: String = self.alias_map.get(field).cloned().unwrap_or_else(|| field.to_string());
        let Some(pos) = self.state_fields.iter().position(|f| f.name == effective_name) else {
            return self.err(format!("state field '{}' was not declared", effective_name));
        };
        let fty = self.state_fields[pos].ty.clone();
        if matches!(fty, Type::Vector(_, _)) {
            return self.err(format!(
                "field '{}' is indexed state — address it as '{}[i]'",
                field, field
            ));
        }
        let elem_id = self.type_id(&fty)?;
        let member_idx = self.builder.u32_const(self.struct_member_index(pos));
        let ptr_ty = self.builder.ptr_class(StorageClass::StorageBuffer, elem_id);
        let chain = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(ptr_ty),
            Some(chain),
            vec![Operand::IdRef(var), Operand::IdRef(member_idx)],
        ));
        Ok((chain, fty))
    }

    /// 2026-08-26 (§2.3): lower an ADDRESS EXPRESSION to an SSBO pointer.
    ///
    /// Valid kernel address forms mirror LLVM's raw-address intent while
    /// staying inside Vulkan's memory model (every address derives from a
    /// buffer base; no numeric addresses):
    /// - `field`      → scalar member pointer (scalar state fields only)
    /// - `field[i]`   → element pointer into indexed (Vector) state
    ///
    /// Anything else is a capability error naming the valid forms.
    fn emit_addr(&mut self, e: &Expr) -> Result<(Word, Type), String> {
        match e {
            Expr::Identifier(name) => self.state_field_scalar_ptr(name),
            Expr::Index(obj, idx) => {
                let Some(fname) = FnLowerer::field_name_of(obj) else {
                    return self.err("only direct state-field indexing forms addresses");
                };
                if !self.state_fields.iter().any(|f| f.name == fname) {
                    return self.err(format!(
                        "unknown state field '{}' (declare it as indexed state)",
                        fname
                    ));
                }
                if !matches!(self.state_fields.iter().find(|f| f.name == fname).unwrap().ty,
                             Type::Vector(_, _))
                {
                    return self.err(format!(
                        "field '{}' is scalar — no '{}'[i]' addressing; use the \
                         field directly",
                        fname, fname
                    ));
                }
                let (idx_val, _) = self.emit_expr(idx)?;
                self.state_field_elem_ptr(fname, idx_val)
            }
            other => self.err(format!(
                "not an address expression ({:?}) — kernel Load#/Store# take \
                 'field' or 'field[i]' rooted in program state",
                std::mem::discriminant(other)
            )),
        }
    }

    /// Byte-count argument check: LLVM Load#/Store# accept a byte width;
    /// over a TYPED buffer the width comes from the declaration. A matching
    /// count passes (source compatibility); anything else names the fix.
    fn check_width_arg(&mut self, arg: Option<&Expr>, elem_ty: &Type, who: &str) -> Result<(), String> {
        let Some(Expr::Decimal(n)) = arg else {
            return Ok(()); // omitted — natural width
        };
        let want = self.builder.scalar_storage_bytes(elem_ty)? as i64;
        if *n != want {
            return self.err(format!(
                "{} byte-width {} does not match the addressed element \
                 ({} bytes by its declared type) — drop the width argument or \
                 fix the field type",
                who, n, want
            ));
        }
        Ok(())
    }

    // ── Small helpers ───────────────────────────────────────────────────

    fn type_id(&mut self, ty: &Type) -> Result<Word, String> {
        self.builder.lower_type(ty)
    }

    fn ptr_to(&mut self, ty: &Type) -> Result<Word, String> {
        let t = self.type_id(ty)?;
        Ok(self.builder.ptr_class(StorageClass::Function, t))
    }

    fn global_invocation_id(&mut self) -> Result<Word, String> {
        if let Some(v) = self.global_id_var {
            return Ok(v);
        }
        let v = self.builtin_input(spirv::BuiltIn::GlobalInvocationId)?;
        self.global_id_var = Some(v);
        Ok(v)
    }

    fn local_invocation_id(&mut self) -> Result<Word, String> {
        if let Some(v) = self.local_id_var {
            return Ok(v);
        }
        let v = self.builtin_input(spirv::BuiltIn::LocalInvocationId)?;
        self.local_id_var = Some(v);
        Ok(v)
    }

    fn builtin_input(&mut self, builtin: spirv::BuiltIn) -> Result<Word, String> {
        // Type: #3 x u32 (vec3<uint>) in Input storage.
        let u32_ty = self.builder.u32_type();
        let vec3 = self.builder.builder.type_vector(u32_ty, 3);
        let ptr = self.builder.ptr_class(StorageClass::Input, vec3);
        let var = self.builder.gen_id();
        self.builder.emit_global(Instruction::new(
            spirv::Op::Variable,
            Some(ptr),
            Some(var),
            vec![Operand::StorageClass(StorageClass::Input)],
        ));
        self.builder.emit_global(Instruction::new(
            spirv::Op::Decorate,
            None,
            None,
            vec![
                Operand::IdRef(var),
                Operand::Decoration(spirv::Decoration::BuiltIn),
                Operand::BuiltIn(builtin),
            ],
        ));
        Ok(var)
    }

    /// Field name of `Ident` or `Field(_, name)` expressions (lhs forms).
    pub fn field_name_of(e: &Expr) -> Option<&str> {
        match e {
            Expr::Identifier(n) => Some(n.as_str()),
            Expr::Field(_, n) => Some(n.as_str()),
            _ => None,
        }
    }
}

/// Free-function twin used by kernel.rs (avoids importing the impl path).
pub fn field_name_of(e: &Expr) -> Option<&str> {
    FnLowerer::field_name_of(e)
}

// Re-export for kernel.rs collection pass.
pub use __collect::collect_state_fields;

mod __collect {
    use super::*;

    /// Walk program top-levels collecting EVERY declared state field used by
    /// the kernel surface (2026-08-26 §2.3: scalars join indexed arrays so
    /// `Load#(scalar)` / `Store#(scalar, v)` have real storage). Sorted +
    /// deduped by setup_state_buffer.
    pub fn collect_state_fields(items: &[crate::ast::TopLevel]) -> Vec<StateField> {
        // 2026-08-31 (plan abv-gpu-by-default): module consts for resolving
        // NAMED array dimensions (`Float[MAXB]` — the AST stores the name
        // with a 0 count; a 0-length OpTypeArray is invalid SPIR-V).
        let consts: HashMap<String, usize> = items
            .iter()
            .filter_map(|i| match i {
                crate::ast::TopLevel::Constant(c) => match &c.expr {
                    Expr::Decimal(n) if *n >= 0 => Some((c.name.clone(), *n as usize)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let resolve_dims = |dims: &[crate::ast::Dimension]| -> Vec<crate::ast::Dimension> {
            dims.iter()
                .map(|d| match d {
                    crate::ast::Dimension::Named(name, n) if *n == 0 => {
                        crate::ast::Dimension::Named(name.clone(), *consts.get(name).unwrap_or(&0))
                    }
                    other => other.clone(),
                })
                .collect()
        };
        let mut fields: Vec<StateField> = Vec::new();
        for item in items {
            match item {
                crate::ast::TopLevel::StateDecl(d) => {
                    let ty = match &d.ty {
                        Type::Vector(inner, dims) => {
                            Type::Vector(inner.clone(), resolve_dims(dims))
                        }
                        other => other.clone(),
                    };
                    fields.push(StateField { name: d.name.clone(), ty });
                }
                // 2026-08-31 (plan abv-gpu-by-default): the real parser emits
                // top-level `let` as Statement(Let) — the same dual form the
                // accel analysis (ProgramInfo::build) and the typechecker
                // consume. Without this arm every pipeline-compiled .abv
                // failed with "no state fields were collected" while
                // hand-built StateDecl fixtures passed. Untyped lets have no
                // kernel storage type and are skipped (referencing one errors
                // naming the field).
                crate::ast::TopLevel::Statement(stmt) => {
                    if let crate::ast::Statement::Let { name, ty: Some(ty), .. } = stmt.as_ref() {
                        let ty = match ty {
                            Type::Vector(inner, dims) => {
                                Type::Vector(inner.clone(), resolve_dims(dims))
                            }
                            other => other.clone(),
                        };
                        fields.push(StateField { name: name.clone(), ty });
                    }
                }
                _ => {}
            }
        }
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        fields.dedup_by(|a, b| a.name == b.name);
        fields
    }
}

/// 2026-08-23: pre-scan a kernel body for typed `let` bindings so their
/// OpVariables can live in the ENTRY block (SPIR-V layout rule). Nested
/// statement lists (guarded bodies) are included.
pub fn collect_locals(body: &[Statement], out: &mut Vec<(String, Type)>) {
    for stmt in body {
        match stmt {
            Statement::Let { name, ty: Some(ty), .. } => {
                out.push((name.clone(), ty.clone()));
            }
            Statement::Guarded(_, inner) | Statement::Block(inner) => {
                collect_locals(inner, out);
            }
            // 2026-08-31 (VITRIOL GEMM comparison M1): the foreach loop
            // variable is a Function-scope long - declared in the entry block
            // like every other local (SPIR-V layout rule); body lets are
            // collected by the recursive call.
            Statement::Foreach { item, body: inner, .. } => {
                out.push((item.clone(), Type::int()));
                collect_locals(inner, out);
            }
            _ => {}
        }
    }
}

// ── O3 stage 2 (plan 2026-08-31-o3-float4-loads.md): aligned group loads ──

/// A matched float mul-add group: `acc = acc + f1[base + k] * f2[...]`
/// (either mul order), where `f1` is vec4-typed and its index is affine in
/// the loop var with coefficient exactly 1.
struct Vec4Group {
    acc: String,
    vec_field: String,
    vec_side: Expr,
    scalar_side: Expr,
    elem: Type,
}

fn match_vec4_fma(
    body: &[Statement],
    item: &str,
    vec4_fields: &HashMap<String, Vec4Field>,
    consts: &HashMap<String, i64>,
) -> Option<Vec4Group> {
    use crate::ast::BinaryOpKind::{Add, Mul};
    if body.len() != 1 {
        return None;
    }
    let Statement::Assign(lhs, rhs) = &body[0] else {
        return None;
    };
    let Expr::Identifier(acc) = lhs else {
        return None;
    };
    let rhs_ref: &Expr = rhs;
    let Expr::BinaryOp(Add, a, b) = rhs_ref else {
        return None;
    };
    let mul = match (a.as_ref(), b.as_ref()) {
        (m @ Expr::BinaryOp(Mul, _, _), _) => m,
        (_, m @ Expr::BinaryOp(Mul, _, _)) => m,
        _ => return None,
    };
    let Expr::BinaryOp(Mul, left, right) = mul else {
        return None;
    };
    let (vec_expr, scalar_expr) = match (left.as_ref(), right.as_ref()) {
        (Expr::Index(of, _), _)
            if vec4_fields.contains_key(field_name_of_index(of).unwrap_or("")) =>
        {
            (left.as_ref(), right.as_ref())
        }
        (_, Expr::Index(of, _))
            if vec4_fields.contains_key(field_name_of_index(of).unwrap_or("")) =>
        {
            (right.as_ref(), left.as_ref())
        }
        _ => return None,
    };
    let Expr::Index(vof, vidx) = vec_expr else {
        return None;
    };
    let vfname = field_name_of_index(vof)?;
    let vf = vec4_fields.get(vfname)?;
    // Affine in the loop var with coefficient exactly 1.
    if expr_var_count(vidx, item) != 1 {
        return None;
    }
    if !vf.elem_float {
        return None; // stage-2 scope: float FMA groups only
    }
    // The alignment proof must hold STATICALLY (base literals divisible by
    // 4 after const folding) — otherwise the vector loop would bail at emit
    // time with no fallback left. Call nodes (GetGlobalId# etc.) fail here,
    // which is exactly the desired conservative behavior.
    let zeroed = subst_var(vidx, item, 0);
    let folded = fold_consts(&zeroed, consts);
    div4(&folded)?;
    Some(Vec4Group {
        acc: acc.clone(),
        vec_field: vfname.to_string(),
        vec_side: vidx.as_ref().clone(),
        scalar_side: scalar_expr.clone(),
        elem: vf.elem.clone(),
    })
}

impl<'a> FnLowerer<'a> {
    /// Emit ONE 4-iteration group of a matched mul-add. `at` is the group
    /// start (constant, % 4 == 0) for the unrolled prefix; `var_storage`
    /// carries the loop phi for the vector-loop form (base = div4(base
    /// literals) + k). The scalar side rebinds the loop var via const_vars.
    fn emit_vec4_fma_group(
        &mut self,
        g: &Vec4Group,
        item: &str,
        at: Option<u64>,
        loop_start: u64,
        var_storage: Word,
    ) -> Result<(), String> {
        let Some(vf) = self.vec4_fields.get(&g.vec_field).cloned() else {
            return Err("vec4 group lost its field".into());
        };
        let Some(ssbo) = self.ssbo_var else {
            return Err("vec4 group without an SSBO".into());
        };
        let member_pos = self
            .state_fields
            .iter()
            .position(|f| f.name == g.vec_field)
            .ok_or_else(|| format!("vec4 field '{}' lost", g.vec_field))?;
        // Group base index: the vec side with the loop var substituted by
        // the group start / k, divided by 4; for the runtime form, the var
        // coefficient is 1 so the phi value IS the group index offset.
        let base_expr = match at {
            Some(k0) => {
                let substituted = subst_var(&g.vec_side, item, k0 as i64);
                let folded = fold_consts(&substituted, &self.const_int_values);
                div4(&folded).ok_or("vec4 group lost alignment")?
            }
            None => {
                // The loop var counts ABSOLUTE elements (stepping 4, starting
                // at loop_start). The vec4 group index is
                // (zeroed + kv)/4 = div4(zeroed) + kv/4 — the ANCHOR at
                // kv = loop_start is div4(zeroed) + loop_start/4, i.e. the
                // unrolled prefix's offset MUST be included. The old
                // `(kv - loop_start)/4` dropped it: every runtime iteration
                // read the vec4 block from the loop's START, re-summing the
                // prefix and shifting the a/b pairing by the whole prefix
                // (found via the identity-matrix probe on GEMM 64^3 —
                // plan 2026-09-01-m2-gemm M2.0).
                let zeroed = subst_var(&g.vec_side, item, 0);
                let folded = fold_consts(&zeroed, &self.const_int_values);
                let b0 = div4(&folded).ok_or("vec4 group lost alignment")?;
                let rel = Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Shr,
                    Box::new(Expr::Identifier(item.to_string())),
                    Box::new(Expr::Decimal(2)),
                );
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Add,
                    Box::new(b0),
                    Box::new(rel),
                )
            }
        };
        let (base_id, _bty) = self.emit_expr(&base_expr)?;
        let v4_ptr = self
            .builder
            .ptr_class(StorageClass::StorageBuffer, vf.vector);
        let member = self.builder.u32_const(member_pos as u32);
        let group = self.builder.gen_id();
        self.builder.emit(Instruction::new(
            spirv::Op::AccessChain,
            Some(v4_ptr),
            Some(group),
            vec![
                Operand::IdRef(ssbo),
                Operand::IdRef(member),
                Operand::IdRef(base_id),
            ],
        ));
        let v4_val = self.builder.load(vf.vector, group);
        let elem_ty_id = self.type_id(&vf.elem)?;
        let acc_ptr = self
            .vars
            .get(&g.acc)
            .map(|(p, _)| *p)
            .ok_or_else(|| format!("vec4 accumulator '{}' lost", g.acc))?;
        for j in 0u32..4 {
            let comp = self.builder.gen_id();
            self.builder.emit(Instruction::new(
                spirv::Op::CompositeExtract,
                Some(elem_ty_id),
                Some(comp),
                vec![Operand::IdRef(v4_val), Operand::LiteralBit32(j)],
            ));
            let (other_id, _oty) = match at {
                Some(k0) => {
                    let scalar_expr = subst_var(&g.scalar_side, item, (k0 + j as u64) as i64);
                    self.emit_expr(&scalar_expr)?
                }
                // Runtime form: the loop var holds the group's base element
                // index — component j reads base + j.
                None => {
                    let kv_plus_j = Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Add,
                        Box::new(Expr::Identifier(item.to_string())),
                        Box::new(Expr::Decimal(j as i64)),
                    );
                    let scalar_expr =
                        subst_var_expr(&g.scalar_side, item, &kv_plus_j);
                    self.emit_expr(&scalar_expr)?
                }
            };
            let acc_val = self.builder.load(elem_ty_id, acc_ptr);
            let fused = self
                .builder
                .glsl_fma(elem_ty_id, comp, other_id, acc_val);
            self.builder.store(acc_ptr, fused);
        }
        Ok(())
    }
}

/// Number of additive occurrences of `name` in `e`.
fn expr_var_count(e: &Expr, name: &str) -> usize {
    match e {
        Expr::Identifier(n) if n == name => 1,
        Expr::BinaryOp(_, l, r) => expr_var_count(l, name) + expr_var_count(r, name),
        _ => 0,
    }
}

/// Replace Identifier `name` with an arbitrary replacement expression.
pub(crate) fn subst_var_expr(e: &Expr, name: &str, repl: &Expr) -> Expr {
    match e {
        Expr::Identifier(n) if n == name => repl.clone(),
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(subst_var_expr(l, name, repl)),
            Box::new(subst_var_expr(r, name, repl)),
        ),
        Expr::Index(o, i) => Expr::Index(
            Box::new(subst_var_expr(o, name, repl)),
            Box::new(subst_var_expr(i, name, repl)),
        ),
        other => other.clone(),
    }
}

/// Replace Identifier `name` with Decimal(value).
fn subst_var(e: &Expr, name: &str, value: i64) -> Expr {
    match e {
        Expr::Identifier(n) if n == name => Expr::Decimal(value),
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(subst_var(l, name, value)),
            Box::new(subst_var(r, name, value)),
        ),
        // The scalar side is typically `x[k]` — the var hides inside Index.
        Expr::Index(o, i) => Expr::Index(
            Box::new(subst_var(o, name, value)),
            Box::new(subst_var(i, name, value)),
        ),
        other => other.clone(),
    }
}

/// Prove `e` (an index expression with the loop var ALREADY substituted —
/// i.e. the group base) is divisible by 4, returning the expression with
/// every literal divided by 4. Structural rule: Decimals must be 4-divisible;
/// Mul(other, Decimal d) divides d; Add recurses; anything else fails.
fn aligned_div4(
    e: &Expr,
    name: &str,
    at: i64,
    consts: &std::collections::HashMap<String, i64>,
) -> Option<Expr> {
    // `at` is the group start (k0 % 4 == 0) — substituting it before the
    // division keeps every literal 4-divisible.
    let at = subst_var(e, name, at);
    let folded = fold_consts(&at, consts);
    div4(&folded)
}

/// Replace const identifiers with their literal values (the unroll alignment
/// proof needs coefficients as literals; emit-time const resolution happens
/// later).
pub(crate) fn fold_consts(e: &Expr, consts: &std::collections::HashMap<String, i64>) -> Expr {
    match e {
        Expr::Identifier(n) => match consts.get(n) {
            Some(v) => Expr::Decimal(*v),
            None => e.clone(),
        },
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(fold_consts(l, consts)),
            Box::new(fold_consts(r, consts)),
        ),
        other => other.clone(),
    }
}

pub(crate) fn div4(e: &Expr) -> Option<Expr> {
    match e {
        Expr::Decimal(d) => {
            if d % 4 == 0 {
                Some(Expr::Decimal(d / 4))
            } else {
                None
            }
        }
        Expr::BinaryOp(crate::ast::BinaryOpKind::Add, l, r) => Some(Expr::BinaryOp(
            crate::ast::BinaryOpKind::Add,
            Box::new(div4(l)?),
            Box::new(div4(r)?),
        )),
        Expr::BinaryOp(crate::ast::BinaryOpKind::Mul, l, r) => {
            match (l.as_ref(), r.as_ref()) {
                (Expr::Decimal(d), other) | (other, Expr::Decimal(d)) => {
                    if d % 4 != 0 {
                        return None;
                    }
                    Some(Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Mul,
                        Box::new(other.clone()),
                        Box::new(Expr::Decimal(d / 4)),
                    ))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Field name of an `Identifier` (the object of an Index expression).
fn field_name_of_index(e: &Expr) -> Option<&str> {
    match e {
        Expr::Identifier(n) => Some(n),
        _ => None,
    }
}

/// Statement-level `subst_var_expr` (cooperative kernel synthesis).
pub(crate) fn subst_stmt_var(stmt: &Statement, name: &str, repl: &Expr) -> Statement {
    match stmt {
        Statement::Assign(lhs, rhs) => Statement::Assign(
            subst_var_expr(lhs, name, repl),
            subst_var_expr(rhs, name, repl),
        ),
        other => other.clone(),
    }
}

/// Substitute ALL occurrences of `name` with `repl` in an expression (deep).
pub(crate) fn subst_var_deep(e: &Expr, name: &str, repl: &Expr) -> Expr {
    subst_var_expr(e, name, repl)
}

/// Check if an expression references a given identifier.
pub(crate) fn expr_references(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Identifier(n) => n == name,
        Expr::BinaryOp(_, l, r) => expr_references(l, name) || expr_references(r, name),
        Expr::Index(o, i) => expr_references(o, name) || expr_references(i, name),
        Expr::Call(_, args, _) => args.iter().any(|a| expr_references(a, name)),
        _ => false,
    }
}

/// Collect Index expressions in an expression tree that reference a vec4-eligible field.
pub(crate) fn collect_vec4_indices(
    e: &Expr,
    vec4_fields: &HashMap<String, Vec4Field>,
    item: &str,
    consts: &HashMap<String, i64>,
    indices: &mut Vec<(String, Expr)>,
) {
    match e {
        Expr::Index(obj, idx) => {
            if let Some(fname) = field_name_of_index(obj) {
                // 2026-09-01 (M2.0 hole 1, plan m2-gemm): the cooperative
                // vec4 load derives its SSBO base as
                // `subst(idx, item → repl) >> 2` — exact ONLY if the
                // substituted index is 4-divisible. The collector must
                // PROVE it: repl is 4-aligned by construction, so the
                // substituted expr must be ≡ 0 (mod 4). `b[k*N + n]` (n
                // arbitrary) rejects → scalar loads; `a[m*K + k]` (K a
                // multiple of 4) proves. Without this the vec4 path read
                // the WRONG element for GEMM's B operand — silently.
                if vec4_fields.contains_key(fname)
                    && expr_references(idx, item)
                    && expr_provably_mod4_zero(idx, item, consts)
                {
                    indices.push((fname.to_string(), (**idx).clone()));
                }
            }
        }
        Expr::BinaryOp(_, l, r) => {
            collect_vec4_indices(l, vec4_fields, item, consts, indices);
            collect_vec4_indices(r, vec4_fields, item, consts, indices);
        }
        Expr::Call(_, args, _) => {
            for a in args {
                collect_vec4_indices(a, vec4_fields, item, consts, indices);
            }
        }
        _ => {}
    }
}

/// Provability lattice for "this expression is ≡ 0 (mod 4) after the
/// cooperative substitution". `item` maps to the lane/iteration replacement
/// `lane*4 + t*stride` — 4-aligned by construction, hence Provably0. A
/// product is Provably0 when EITHER factor is (4a·b ≡ 0); a sum needs both
/// sides; division/modulo/calls destroy the proof (conservative Unknown).
fn expr_provably_mod4_zero(e: &Expr, item: &str, consts: &HashMap<String, i64>) -> bool {
    match e {
        Expr::Identifier(name) if name == item => true,
        Expr::Decimal(d) => d % 4 == 0,
        Expr::Float(_) => false,
        Expr::Identifier(name) => consts.get(name).map(|v| v % 4 == 0).unwrap_or(false),
        Expr::BinaryOp(kind, l, r) => match kind {
            crate::ast::BinaryOpKind::Mul => {
                expr_provably_mod4_zero(l, item, consts)
                    || expr_provably_mod4_zero(r, item, consts)
            }
            crate::ast::BinaryOpKind::Add | crate::ast::BinaryOpKind::Sub => {
                expr_provably_mod4_zero(l, item, consts)
                    && expr_provably_mod4_zero(r, item, consts)
            }
            _ => false,
        },
        _ => false,
    }
}

/// Replace an Index expression `field[idx]` with a replacement expression
/// deep inside another expression.
pub(crate) fn replace_index_in_expr(e: &Expr, fname: &str, orig_idx: &Expr, repl: &Expr) -> Expr {
    match e {
        Expr::Index(obj, idx) => {
            if let Some(n) = field_name_of_index(obj) {
                if n == fname && idx.as_ref() == orig_idx {
                    return repl.clone();
                }
            }
            Expr::Index(
                Box::new(replace_index_in_expr(obj, fname, orig_idx, repl)),
                Box::new(replace_index_in_expr(idx, fname, orig_idx, repl)),
            )
        }
        Expr::BinaryOp(k, l, r) => Expr::BinaryOp(
            *k,
            Box::new(replace_index_in_expr(l, fname, orig_idx, repl)),
            Box::new(replace_index_in_expr(r, fname, orig_idx, repl)),
        ),
        Expr::Call(n, args, ty) => Expr::Call(
            n.clone(),
            args.iter()
                .map(|a| replace_index_in_expr(a, fname, orig_idx, repl))
                .collect(),
            ty.clone(),
        ),
        other => other.clone(),
    }
}

/// Replace an Index expression in a Statement.
pub(crate) fn replace_index_in_stmt(stmt: &Statement, fname: &str, orig_idx: &Expr, repl: &Expr) -> Statement {
    match stmt {
        Statement::Assign(lhs, rhs) => Statement::Assign(
            replace_index_in_expr(lhs, fname, orig_idx, repl),
            replace_index_in_expr(rhs, fname, orig_idx, repl),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod vec4_gate_tests {
    //! The M2.0 correctness gates as unit tests (plan 2026-09-01-m2-gemm):
    //! the vec4 index-alignment proof and the projection layout rule.
    use super::*;
    use crate::ast::{BinaryOpKind, Dimension};

    fn f32_vec(count: u32) -> Type {
        // The internal form of the canonical `Bit<32>`-backed Float array
        // element (BUGS.md "Bits tripwire": internal spelling `Type::Bits`).
        Type::Vector(Box::new(Type::Bits(32)), vec![Dimension::Anonymous(count as usize)])
    }

    fn f32_scalar() -> Type {
        Type::Bits(32)
    }

    fn idx_expr(op: &str) -> Expr {
        // op = "gemv_a":  i*K + k    (2D row + loop var, K const 4096)
        // op = "gemm_b":  k*N + n    (1D col + loop var — the M2.0 hole)
        // op = "gemv_x":  k          (loop var only)
        // op = "no_k":    i*K        (no loop var — never vec4-collected)
        let i = Expr::Identifier("i".into());
        let k = Expr::Identifier("k".into());
        let n = Expr::Identifier("n".into());
        match op {
            "gemv_a" => Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::BinaryOp(BinaryOpKind::Mul, Box::new(i), Box::new(Expr::Identifier("K".into())))),
                Box::new(k)),
            "gemm_b" => Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::BinaryOp(BinaryOpKind::Mul, Box::new(k.clone()), Box::new(Expr::Identifier("N".into())))),
                Box::new(n)),
            "gemv_x" => k,
            _ => Expr::BinaryOp(BinaryOpKind::Mul, Box::new(i), Box::new(Expr::Identifier("K".into()))),
        }
    }

    fn vec4_map(fields: &[&str]) -> HashMap<String, Vec4Field> {
        fields.iter().map(|f| (f.to_string(), Vec4Field {
            array: 0, vector: 0, elem: f32_scalar(), elem_float: true,
        })).collect()
    }

    #[test]
    fn vec4_collector_accepts_aligned_row_index() {
        // a[i*K + k] with K = 4096 (a multiple of 4): provably 4-aligned.
        let mut got = Vec::new();
        let access = Expr::Index(Box::new(Expr::Identifier("a".into())),
                                 Box::new(idx_expr("gemv_a")));
        collect_vec4_indices(
            &access, &vec4_map(&["a"]), "k",
            &[("K".to_string(), 4096)].into_iter().collect(),
            &mut got,
        );
        assert_eq!(got.len(), 1, "aligned row index must be vec4-collected");
    }

    #[test]
    fn vec4_collector_rejects_gemm_b_column_index() {
        // b[k*N + n]: n is arbitrary — the >>2 base read the WRONG element
        // (the M2.0 hole). The collector must reject it → scalar loads.
        let mut got = Vec::new();
        let access = Expr::Index(Box::new(Expr::Identifier("b".into())),
                                 Box::new(idx_expr("gemm_b")));
        collect_vec4_indices(
            &access, &vec4_map(&["b"]), "k",
            &[("N".to_string(), 4096)].into_iter().collect(),
            &mut got,
        );
        assert!(got.is_empty(), "column-shifted index must stay scalar");
    }

    #[test]
    fn vec4_collector_accepts_bare_loop_var() {
        // x[k]: repl is 4-aligned by construction → provable.
        let mut got = Vec::new();
        let access = Expr::Index(Box::new(Expr::Identifier("x".into())),
                                 Box::new(idx_expr("gemv_x")));
        collect_vec4_indices(
            &access, &vec4_map(&["x"]), "k",
            &HashMap::new(),
            &mut got,
        );
        assert_eq!(got.len(), 1, "bare loop-var index must be vec4-collected");
    }

    #[test]
    fn vec4_collector_ignores_indices_without_loop_var() {
        let mut got = Vec::new();
        let access = Expr::Index(Box::new(Expr::Identifier("a".into())),
                                 Box::new(idx_expr("no_k")));
        collect_vec4_indices(
            &access, &vec4_map(&["a"]), "k",
            &[("K".to_string(), 4096)].into_iter().collect(),
            &mut got,
        );
        assert!(got.is_empty(), "no loop var → nothing to substitute");
    }

    #[test]
    fn projection_offsets_align_vec4_arrays_and_pack_the_rest() {
        // THE layout rule (plan 2026-09-01-vec4-projection-layout): name-
        // sorted fields; a vec4-eligible array (Bit<32>-element, count%4==0)
        // aligns up to 16B; everything else packs. The gemv-like field set:
        // a [Float x 4096], i [Int scalar], x [Float x 16], y [Float x 16].
        let fields = vec![
            StateField { name: "a".into(), ty: f32_vec(4096) },
            StateField { name: "i".into(), ty: Type::Bits(64) },
            StateField { name: "x".into(), ty: f32_vec(16) },
            StateField { name: "y".into(), ty: f32_vec(16) },
        ];
        let mut sb = SpirvBuilder::new();
        let offs = FnLowerer::projection_offsets(&mut sb, &fields, None).expect("layout");
        // a @ 0 (already 16-aligned)
        assert_eq!(offs[0], 0, "a: first field, naturally aligned");
        // i: scalar Bit<64> → 8 bytes, PACKED right after a (host==this rule
        // for scalars; only vec4-eligible arrays move)
        assert_eq!(offs[1], 16384, "i: packed after a");
        // x: vec4-eligible → aligned UP from 16392 to 16400
        assert_eq!(offs[2], 16400, "x: 16B-aligned (16392 -> 16400)");
        // y: x ends 16400+64=16464 (already aligned) → packed
        assert_eq!(offs[3], 16464, "y: already aligned after x");
        // Determinism: same input, same output.
        let offs2 = FnLowerer::projection_offsets(&mut sb, &fields, None).expect("layout");
        assert_eq!(offs, offs2, "the layout rule must be deterministic");
    }

    #[test]
    fn projection_offsets_aliases_field_to_target_offset() {
        // Phase 3 — buffer reuse: "y" is aliased to "x" (same type, dead
        // before y's first use). Both get the same device offset.
        let fields = vec![
            StateField { name: "x".into(), ty: f32_vec(16) },
            StateField { name: "y".into(), ty: f32_vec(16) },
        ];
        let mut reuse = HashMap::new();
        reuse.insert("y".into(), "x".into());
        let mut sb = SpirvBuilder::new();
        let offs = FnLowerer::projection_offsets(&mut sb, &fields, Some(&reuse)).expect("layout");
        // x gets its natural 16B-aligned offset (0 for the first field).
        assert_eq!(offs[0], 0, "x: first field, base offset 0");
        // y is aliased to x → same offset.
        assert_eq!(offs[1], 0, "y: aliased to x, shares offset 0");
        // Non-aliased fields still pack normally.
        let fields3 = vec![
            StateField { name: "a".into(), ty: f32_vec(16) },
            StateField { name: "b".into(), ty: f32_vec(16) },
            StateField { name: "c".into(), ty: f32_vec(16) },
        ];
        let mut reuse3 = HashMap::new();
        reuse3.insert("c".into(), "a".into());
        let offs3 = FnLowerer::projection_offsets(&mut sb, &fields3, Some(&reuse3)).expect("layout");
        assert_eq!(offs3[0], 0, "a: base offset 0");
        assert_eq!(offs3[1], 16 * 4, "b: packed after a (64 bytes)");
        assert_eq!(offs3[2], 0, "c: aliased to a, shares offset 0");
    }

    #[test]
    fn projection_offsets_rejects_size_mismatched_alias() {
        // A scalar (8 bytes) must NOT alias an array's slot (64 bytes) —
        // the filter rejects the pair, keeping the scalar at its own offset.
        let fields = vec![
            StateField { name: "arr".into(), ty: f32_vec(16) },
            StateField { name: "scal".into(), ty: Type::Bits(64) },
        ];
        let mut reuse = HashMap::new();
        reuse.insert("scal".into(), "arr".into());
        let mut sb = SpirvBuilder::new();
        let offs = FnLowerer::projection_offsets(&mut sb, &fields, Some(&reuse)).expect("layout");
        assert_eq!(offs[0], 0, "arr: first field, offset 0");
        assert_eq!(offs[1], 16 * 4, "scal: keeps its own slot (filtered out)");
        // Same-size pair survives.
        let mut reuse2 = HashMap::new();
        reuse2.insert("scal".into(), "arr".into());
        let fields2 = vec![
            StateField { name: "arr".into(), ty: f32_vec(8) },
            StateField { name: "scal".into(), ty: f32_vec(8) },
        ];
        let offs2 = FnLowerer::projection_offsets(&mut sb, &fields2, Some(&reuse2)).expect("layout");
        assert_eq!(offs2[1], 0, "same-size array aliases to arr's offset 0");
    }
}
