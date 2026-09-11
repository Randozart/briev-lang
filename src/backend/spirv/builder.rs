/// SPIR-V module builder — dr::Builder wrapper with typed emission.
///
/// 2026-07-15: Sets capabilities, memory model, provides id and type helpers.
/// 2026-08-23 (assembly-bug fix, BUGS.md): ALL type/constant/decoration
/// emission goes through rspirv's TYPED builder helpers (type_int,
/// constant_bit64, decorate, …) so its internal dedup tables stay
/// consistent. The old path mixed raw insert_types_global_values with the
/// builder's own state and produced streams rspirv/spirv-val reject
/// (duplicate ids / OperandExceeded). The separate TypeCache id space is
/// gone — one id counter, one source of truth.
/// To undo: restore TypeCache + arena + flush_types (git history).

use rspirv::dr::{Builder, Operand};
use std::collections::HashMap;

use crate::ast::Type;
use crate::casting::graph::{CastingGraph, SpirvShape};
use crate::type_universe::TypeUniverse;
use rspirv::binary::Assemble;
use rspirv::spirv::{self, Word, ExecutionModel};

/// Combines rspirv Builder with Briev-type lowering.
pub struct SpirvBuilder {
    pub builder: Builder,
    /// Dedup map for lowered Briev types (key = canonical debug form).
    type_keys: HashMap<String, Word>,
    /// 2026-08-26 (§2.4): universe + casting graph drive scalar type
    /// resolution — (protocol, metadata), never type-name matches. The
    /// default universe has the primordials seeded; the pipeline injects
    /// the NORMALIZED universe via with_universe() so user typedefs resolve.
    universe: TypeUniverse,
    casting_graph: CastingGraph,
    /// Target integer width when a type carries no bits metadata.
    int_bits: u64,
    /// O2 (plan 2026-08-31-gpu-next): lazily imported GLSL.std.450 set id,
    /// cached so repeated Fma emissions share one OpExtInstImport.
    glsl_set: Option<Word>,
    /// O3 (plan 2026-08-31-o3-float4-loads.md): array-of-vec4 member types,
    /// cached per (scalar elem id, length) — rspirv dedups the OpTypeArray,
    /// so a second decoration would be invalid.
    vec4_arrays: HashMap<(Word, u32), Word>,
}

impl SpirvBuilder {
    /// Create builder, set Shader + Int64 + Float64 + GLSL450.
    pub fn new() -> Self {
        let mut b = Builder::new();
        b.capability(spirv::Capability::Shader);
        b.capability(spirv::Capability::Int64);
        b.capability(spirv::Capability::Float64);
        // Cooperative row kernels (plan 2026-09-01-cooperative-row-kernels):
        // subgroup arithmetic for the dot-product reduction.
        b.capability(spirv::Capability::GroupNonUniform);
        b.capability(spirv::Capability::GroupNonUniformArithmetic);
        b.memory_model(spirv::AddressingModel::Logical, spirv::MemoryModel::GLSL450);
        SpirvBuilder {
            builder: b,
            type_keys: HashMap::new(),
            universe: TypeUniverse::new(),
            casting_graph: CastingGraph::new(),
            int_bits: 64,
            glsl_set: None,
            vec4_arrays: HashMap::new(),
        }
    }

    /// 2026-08-26 (§2.4): pipeline injects the NORMALIZED universe so user
    /// typedefs (bits metadata, protocol bases) participate in resolution.
    pub fn with_universe(mut self, universe: &TypeUniverse, int_bits: u64) -> Self {
        self.universe = universe.clone();
        self.int_bits = int_bits;
        self
    }

    /// 2026-09-01 (M2.2): switch this module to the Vulkan memory model and
    /// declare the cooperative-matrix machinery. The coopmat capability
    /// REQUIRES the Vulkan memory model whenever the Shader capability is
    /// present (SPV_KHR_cooperative_matrix). Must run BEFORE body emission
    /// is validated — the binary sections are ordered by the assembler.
    pub fn enable_cooperative_matrix(&mut self) {
        self.builder
            .capability(spirv::Capability::CooperativeMatrixKHR);
        self.builder
            .capability(spirv::Capability::VulkanMemoryModel);
        self.builder
            .capability(spirv::Capability::StorageBuffer16BitAccess);
        self.builder.extension("SPV_KHR_cooperative_matrix");
        self.builder.extension("SPV_KHR_vulkan_memory_model");
        self.builder.extension("SPV_KHR_16bit_storage");
        // Replace the module's memory model instruction (new() emitted
        // GLSL450): dr::Module holds Option<Instruction>.
        self.builder.module_mut().memory_model = Some(rspirv::dr::Instruction::new(
            spirv::Op::MemoryModel,
            None,
            None,
            vec![
                Operand::LiteralBit32(spirv::AddressingModel::Logical as u32),
                Operand::LiteralBit32(spirv::MemoryModel::Vulkan as u32),
            ],
        ));
    }

    /// Finalize module and assemble to SPIR-V binary.
    pub fn build(mut self) -> Result<Vec<u8>, String> {
        // 2026-08-23 (layout bugfix): strict validators require the globals
        // section ordered types/constants → variables → decorations. Our
        // incremental emission interleaved them (builtin variables between
        // constants), which spirv-val rejects as 'Decorate is in an invalid
        // layout section'. Stable-bucket sort here; deterministic because
        // each bucket keeps emission order.
        // To undo: remove this reordering block.
        {
            let mut types_ct = Vec::new();
            let mut vars = Vec::new();
            let mut decors = Vec::new();
            let tgv = &mut self.builder.module_mut().types_global_values;
            for inst in std::mem::take(tgv) {
                match inst.class.opcode {
                    spirv::Op::Variable => vars.push(inst),
                    spirv::Op::Decorate | spirv::Op::MemberDecorate => decors.push(inst),
                    _ => types_ct.push(inst),
                }
            }
            // SPIR-V §2.4 logical layout: annotations (OpDecorate) come in
            // their OWN section BEFORE types/constants/global-variables.
            tgv.extend(decors);
            tgv.extend(types_ct);
            tgv.extend(vars);
        }
        let module = self.builder.module();
        let words = module.assemble();
        let mut binary = Vec::with_capacity(words.len() * 4);
        for w in &words {
            binary.extend_from_slice(&w.to_le_bytes());
        }
        Ok(binary)
    }

    /// Allocate a fresh result ID.
    pub fn gen_id(&mut self) -> Word {
        self.builder.id()
    }

    /// O3 (plan 2026-08-31-o3-float4-loads.md): OpTypeArray(vec4, len/4)
    /// with ArrayStride 16 — byte-identical to the scalar array. Cached per
    /// (scalar elem id, element count): rspirv dedups identical OpTypeArrays
    /// and a second ArrayStride decoration on the same id is invalid.
    pub fn vec4_array_type(&mut self, scalar: Word, elems: u32) -> Word {
        let v4 = self.builder.type_vector(scalar, 4);
        let len = self.u32_const(elems / 4);
        if let Some(arr) = self.vec4_arrays.get(&(v4, elems)) {
            return *arr;
        }
        let arr = self.builder.type_array(v4, len);
        self.decorate_raw(
            arr,
            spirv::Decoration::ArrayStride,
            vec![rspirv::dr::Operand::LiteralBit32(16)],
        );
        self.vec4_arrays.insert((v4, elems), arr);
        arr
    }

    /// O2 (plan 2026-08-31-gpu-next): emit `GLSL.std.450 Fma(a, b, c)` —
    /// one rounding instead of FMul+FAdd's two, and immune to whether the
    /// driver contracts mul+add chains. The OpExtInstImport is emitted
    /// lazily once and reused (it must precede the function in the module;
    /// rspirv routes imports to the import section regardless of when the
    /// call happens). Requires an open function block — kernel lowering
    /// only ever calls this inside a function body.
    pub fn glsl_fma(&mut self, result_ty: Word, a: Word, b: Word, c: Word) -> Word {
        let id = self.gen_id();
        self.glsl_fma_with_id(id, result_ty, a, b, c);
        id
    }

    /// `glsl_fma` with a caller-chosen result id — loop phis reserve the
    /// back-edge id before the header is emitted, and the body must define
    /// its value INTO that id.
    pub fn glsl_fma_with_id(&mut self, result_id: Word, result_ty: Word, a: Word, b: Word, c: Word) {
        let set = match self.glsl_set {
            Some(s) => s,
            None => {
                let s = self.builder.ext_inst_import("GLSL.std.450");
                self.glsl_set = Some(s);
                s
            }
        };
        self.builder
            .ext_inst(
                result_ty,
                Some(result_id),
                set,
                spirv::GLOp::Fma as u32,
                [
                    Operand::IdRef(a),
                    Operand::IdRef(b),
                    Operand::IdRef(c),
                ],
            )
            .expect("Fma emission inside a function block");
    }

    /// Emit `GLSL.std.450 Exp(x)` — single-argument elementwise exponential.
    /// Same lazy-import mechanism as Fma (glsl_set cached per module).
    pub fn glsl_exp(&mut self, result_ty: Word, x: Word) -> Word {
        let id = self.gen_id();
        let set = match self.glsl_set {
            Some(s) => s,
            None => {
                let s = self.builder.ext_inst_import("GLSL.std.450");
                self.glsl_set = Some(s);
                s
            }
        };
        self.builder
            .ext_inst(
                result_ty,
                Some(id),
                set,
                spirv::GLOp::Exp as u32,
                [Operand::IdRef(x)],
            )
            .expect("Exp emission inside a function block");
        id
    }

    /// Emit `GLSL.std.450 Sqrt(x)` — single-argument elementwise square
    /// root (2026-09-02, plan 2026-09-02-graphics-ray-and-images: ray
    /// shading). Same lazy-import mechanism as Exp.
    pub fn glsl_sqrt(&mut self, result_ty: Word, x: Word) -> Word {
        let id = self.gen_id();
        let set = match self.glsl_set {
            Some(s) => s,
            None => {
                let s = self.builder.ext_inst_import("GLSL.std.450");
                self.glsl_set = Some(s);
                s
            }
        };
        self.builder
            .ext_inst(
                result_ty,
                Some(id),
                set,
                spirv::GLOp::Sqrt as u32,
                [Operand::IdRef(x)],
            )
            .expect("Sqrt emission inside a function block");
        id
    }

    /// Emit `GLSL.std.450 FAbs(x)` — single-argument elementwise absolute
    /// value (2026-09-02: shading clamps, max(a, 0) = (a + |a|) / 2).
    /// Same lazy-import mechanism as Exp/Sqrt.
    pub fn glsl_fabs(&mut self, result_ty: Word, x: Word) -> Word {
        let id = self.gen_id();
        let set = match self.glsl_set {
            Some(s) => s,
            None => {
                let s = self.builder.ext_inst_import("GLSL.std.450");
                self.glsl_set = Some(s);
                s
            }
        };
        self.builder
            .ext_inst(
                result_ty,
                Some(id),
                set,
                spirv::GLOp::FAbs as u32,
                [Operand::IdRef(x)],
            )
            .expect("FAbs emission inside a function block");
        id
    }

    // ── Briev type lowering (typed, internally deduped by rspirv) ───────

    /// Lower a Briev type to a SPIR-V type id.
    pub fn lower_type(&mut self, ty: &Type) -> Result<Word, String> {
        let key = format!("{:?}", ty);
        if let Some(&id) = self.type_keys.get(&key) {
            return Ok(id);
        }
        let id = self.lower_type_fresh(ty)?;
        self.type_keys.insert(key, id);
        Ok(id)
    }

    fn lower_type_fresh(&mut self, ty: &Type) -> Result<Word, String> {
        match ty {
            Type::Void => Ok(self.builder.type_void()),
            Type::Bits(1) => Ok(self.builder.type_bool()),
            Type::Bits(bytes) => {
                // Bits(N) unsigned ints; 8 for i8/u8, else full width.
                Ok(self.builder.type_int(*bytes as u32, 0))
            }
            Type::Ptr(elem) => {
                let elem_id = self.lower_type(elem)?;
                Ok(self.builder.type_pointer(None, spirv::StorageClass::Function, elem_id))
            }
            Type::Vector(inner, dims) => {
                // Fixed-size array (indexed state). Innermost-out so each
                // ArrayStride covers its tail. 2026-08-31 (plan
                // abv-gpu-by-default): the stride is the ELEMENT's real
                // storage width from the casting graph — Float32 arrays were
                // strided as i64, so every element after the first read the
                // wrong slot.
                let inner_id = self.lower_type(inner)?;
                let mut cur = inner_id;
                let elem_bytes = self.scalar_storage_bytes(inner)?;
                let mut dim_sizes: Vec<usize> = dims
                    .iter()
                    .map(|d| match d {
                        crate::ast::Dimension::Anonymous(n) => *n,
                        crate::ast::Dimension::Named(_, n) => *n,
                    })
                    .collect();
                dim_sizes.reverse();
                let mut stride = elem_bytes;
                for n in dim_sizes {
                    let len = self.u32_const(n as u32);
                    let arr = self.builder.type_array(cur, len);
                    self.decorate_raw(
                        arr,
                        spirv::Decoration::ArrayStride,
                        vec![rspirv::dr::Operand::LiteralBit32(stride)],
                    );
                    cur = arr;
                    stride *= n as u32;
                }
                Ok(cur)
            }
            // 2026-08-26 (§2.4): everything below resolves through the
            // casting graph from (protocol, metadata). No type names here —
            // `Float64`, stdlib subtypes, and user typedefs all derive from
            // their Cast.* protocol properties + bits metadata alike.
            Type::Custom(_) | Type::Applied(_, _) => {
                let shape = self
                    .casting_graph
                    .resolve_spirv_shape(&self.universe, ty, self.int_bits)?;
                match shape {
                    SpirvShape::Int { bits, signed } => {
                        Ok(self.builder.type_int(bits, if signed { 1 } else { 0 }))
                    }
                    SpirvShape::Float { bits } => Ok(self.builder.type_float(bits)),
                    SpirvShape::Bool => Ok(self.builder.type_bool()),
                }
            }
            other => Err(format!(
                "SPIR-V: unsupported type {:?} — kernel state is scalar                  Int/UInt/Float/Bool-rooted storage",
                other
            )),
        }
    }

    /// Cached 32-bit unsigned type (builtins use u32 by spec).
    pub fn u32_type(&mut self) -> Word {
        let key = "builtin_u32";
        if let Some(&id) = self.type_keys.get(key) {
            return id;
        }
        let id = self.builder.type_int(32, 0);
        self.type_keys.insert(key.to_string(), id);
        id
    }

    /// Cached i64 constant.
    pub fn i64_const(&mut self, v: u64) -> Word {
        let key = format!("i64c_{}", v);
        if let Some(&id) = self.type_keys.get(&key) {
            return id;
        }
        let int_ty = match self.lower_type(&Type::int()) {
            Ok(t) => t,
            Err(_) => unreachable!("i64 type always lowers"),
        };
        let id = self.builder.constant_bit64(int_ty, v);
        self.type_keys.insert(key, id);
        id
    }

    /// Cached u32 constant.
    pub fn u32_const(&mut self, v: u32) -> Word {
        let key = format!("u32c_{}", v);
        if let Some(&id) = self.type_keys.get(&key) {
            return id;
        }
        let u32_ty = self.u32_type();
        let id = self.builder.constant_bit32(u32_ty, v);
        self.type_keys.insert(key, id);
        id
    }

    /// 2026-08-31 (plan abv-gpu-by-default): cached float constant. OpConstant
    /// carries floats as literal BITS (f32/f64 payload word), so the value is
    /// keyed by its bit pattern — 0.0 and -0.0 stay distinct constants.
    pub fn float_const(&mut self, bits: u32, v: f64) -> Word {
        let key = match bits {
            64 => format!("fc_{}_{}", bits, v.to_bits()),
            _ => format!("fc_{}_{}", bits, (v as f32).to_bits()),
        };
        if let Some(&id) = self.type_keys.get(&key) {
            return id;
        }
        let float_ty = self.builder.type_float(bits);
        let id = match bits {
            64 => self.builder.constant_bit64(float_ty, v.to_bits()),
            32 => self.builder.constant_bit32(float_ty, (v as f32).to_bits()),
            // 2026-09-02 (plan 2026-09-02-cuda-race B3): f16 literals —
            // SPIR-V packs a 16-bit float constant into one 32-bit literal
            // word (the value in the LOW half). Zero is the common case
            // (coopmat accumulator init); general half values go through
            // the f32→f16 RNE rounding the storage format already uses.
            16 => {
                let half_bits = crate::backend::llvm::f32_to_f16_bits(v as f32);
                self.builder.constant_bit32(float_ty, half_bits as u32)
            }
            other => unreachable!("float width {} is rejected at shape resolution", other),
        };
        self.type_keys.insert(key, id);
        id
    }

    /// 2026-08-31 (plan abv-gpu-by-default): is this type FLOAT-shaped? Opcode
    /// selection for arithmetic is driven by the operand's protocol category
    /// (rule 19: never a type-name match) — Custom/Applied/hashword types
    /// resolve through the casting graph; compiler constructs (Bits/Vector)
    /// are integer/aggregate by construction.
    pub fn is_float_type(&mut self, ty: &Type) -> Result<bool, String> {
        match ty {
            Type::Custom(_) | Type::Applied(_, _) => Ok(matches!(
                self.casting_graph
                    .resolve_spirv_shape(&self.universe, ty, self.int_bits)?,
                SpirvShape::Float { .. }
            )),
            _ => Ok(false),
        }
    }

    /// 2026-08-31 (plan abv-gpu-by-default): float BIT WIDTH of a type (32 or
    /// 64) — errors when the type is not float-shaped, so callers only use it
    /// after `is_float_type`.
    pub fn float_bits_of(&mut self, ty: &Type) -> Result<u32, String> {
        match ty {
            Type::Custom(_) | Type::Applied(_, _) => match self
                .casting_graph
                .resolve_spirv_shape(&self.universe, ty, self.int_bits)?
            {
                SpirvShape::Float { bits } => Ok(bits),
                _ => self.float_shape_err(ty),
            },
            _ => self.float_shape_err(ty),
        }
    }

    /// 2026-08-31 (plan abv-gpu-by-default): storage byte size of a scalar
    /// surface type — array strides and SSBO member offsets must match the
    /// element's REAL width (Float32 → 4), not a fixed 8, or every kernel
    /// after the first element reads the wrong slot. Widths come from the
    /// casting-graph shape (rule 19).
    pub fn scalar_storage_bytes(&mut self, ty: &Type) -> Result<u32, String> {
        Ok(match self.shape_of(ty)? {
            SpirvShape::Int { bits, .. } => (bits / 8).max(1),
            SpirvShape::Float { bits } => (bits / 8).max(1),
            SpirvShape::Bool => 4,
        })
    }

    /// 2026-08-31 (plan abv-gpu-by-default): the NUMERIC SHAPE of a type for
    /// cast opcode selection — Int/Float/Bool with width and signedness,
    /// resolved through the casting graph (rule 19). Mirrors lower_type's
    /// per-construct mapping so shape and type id always agree.
    pub fn shape_of(&mut self, ty: &Type) -> Result<SpirvShape, String> {
        match ty {
            Type::Bits(1) => Ok(SpirvShape::Bool),
            Type::Bits(n) => Ok(SpirvShape::Int { bits: *n as u32, signed: false }),
            Type::Custom(_) | Type::Applied(_, _) => {
                self.casting_graph
                    .resolve_spirv_shape(&self.universe, ty, self.int_bits)
            }
            other => Err(format!(
                "SPIR-V: type {:?} has no scalar shape — casts need scalar operands",
                other
            )),
        }
    }

    fn float_shape_err<T>(&self, ty: &Type) -> Result<T, String> {
        Err(format!(
            "SPIR-V: type {:?} is not float-shaped — float constants need a \
             Float-rooted type",
            ty
        ))
    }

    /// 2026-08-23: Decoration with EXPLICIT operand list. The typed
    /// dr::Builder::decorate dropped additional_params on the wire for
    /// BuiltIn/DescriptorSet/Binding (assembled wc=3 instead of 4 ->
    /// 'expected more operands'). Raw emission guarantees the words.
    /// To undo: return to self.builder.decorate(...) calls.
    pub fn decorate_raw(
        &mut self,
        target: Word,
        decoration: spirv::Decoration,
        params: Vec<rspirv::dr::Operand>,
    ) {
        let mut operands = vec![
            rspirv::dr::Operand::IdRef(target),
            rspirv::dr::Operand::Decoration(decoration),
        ];
        operands.extend(params);
        let inst = rspirv::dr::Instruction::new(spirv::Op::Decorate, None, None, operands);
        self.emit_global(inst);
    }

    /// Typed pointer with explicit storage class (deduped per class+pointee).
    pub fn ptr_class(&mut self, class: spirv::StorageClass, pointee: Word) -> Word {
        let key = format!("ptr_{:?}_{}", class, pointee);
        if let Some(&id) = self.type_keys.get(&key) {
            return id;
        }
        let id = self.builder.type_pointer(None, class, pointee);
        self.type_keys.insert(key, id);
        id
    }


    /// BuiltIn decoration (requires the BuiltIn literal operand).
    pub fn decorate_builtin(&mut self, target: Word, builtin: spirv::BuiltIn) {
        self.decorate_raw(
            target,
            spirv::Decoration::BuiltIn,
            vec![rspirv::dr::Operand::BuiltIn(builtin)],
        );
    }

    // ── Module globals / function plumbing ──────────────────────────────

    /// Emit a MODULE-GLOBAL instruction (StorageBuffer/Input OpVariables).
    pub fn emit_global(&mut self, inst: rspirv::dr::Instruction) {
        self.builder
            .insert_types_global_values(rspirv::dr::InsertPoint::End, inst);
    }

    /// Set entry point via builder.
    pub fn set_entry_point(
        &mut self,
        func_id: Word,
        name: &str,
        execution_model: ExecutionModel,
        interface: &[Word],
    ) {
        self.builder.entry_point(
            execution_model,
            func_id,
            name,
            interface.to_vec(),
        );
    }

    /// Add execution mode via builder.
    pub fn add_execution_mode(
        &mut self,
        func_id: Word,
        mode: spirv::ExecutionMode,
        x: u32,
        y: u32,
        z: u32,
    ) {
        self.builder.execution_mode(func_id, mode, &[x, y, z]);
    }

    /// Begin a new function.
    pub fn begin_function(
        &mut self,
        return_type: Word,
        func_id: Word,
        control: spirv::FunctionControl,
        func_type: Word,
    ) {
        self.builder
            .begin_function(return_type, Some(func_id), control, func_type)
            .unwrap();
    }

    /// End current function.
    pub fn end_function(&mut self) {
        self.builder.end_function().unwrap();
    }

    /// Begin a new block with optional label ID.
    pub fn begin_block(&mut self, label_id: Option<Word>) {
        self.builder.begin_block(label_id).unwrap();
    }

    /// Emit an instruction into the current block.
    pub fn emit(&mut self, inst: rspirv::dr::Instruction) {
        self.builder
            .insert_into_block(rspirv::dr::InsertPoint::End, inst);
    }

    /// Store value into pointer (inside current block).
    pub fn store(&mut self, ptr: Word, val: Word) {
        let inst = rspirv::dr::Instruction::new(
            spirv::Op::Store,
            None,
            None,
            vec![rspirv::dr::Operand::IdRef(ptr), rspirv::dr::Operand::IdRef(val)],
        );
        self.emit(inst);
    }

    /// Branch to label. Uses rspirv's TYPED terminator so the builder's
    /// selected-block state closes — raw inserts leave it open and the next
    /// begin_block panics (NestedBlock).
    pub fn branch(&mut self, label: Word) {
        self.builder.branch(label).expect("branch");
    }

    /// Typed return (see branch note).
    pub fn ret(&mut self) {
        self.builder.ret().expect("ret");
    }

    /// Typed unreachable (see branch note).
    pub fn spirv_unreachable(&mut self) {
        self.builder.unreachable().expect("unreachable");
    }

    /// Load from pointer into fresh id of result type.
    pub fn load(&mut self, result_ty: Word, ptr: Word) -> Word {
        self.instr(spirv::Op::Load, Some(result_ty), None, vec![
            rspirv::dr::Operand::IdRef(ptr),
        ])
    }

    /// 2026-08-23: read-only module view for tests/inspection.
    pub fn module_ref(&self) -> &rspirv::dr::Module {
        self.builder.module_ref()
    }

    /// Generic instruction into current block.
    pub fn instr(
        &mut self,
        op: spirv::Op,
        result_ty: Option<Word>,
        result_id: Option<Word>,
        operands: Vec<rspirv::dr::Operand>,
    ) -> Word {
        // 2026-08-23 (bugfix): only allocate/embed a result id when the
        // instruction HAS a result type — a result-less op (OpUnreachable)
        // emitted with Some(id) encodes word-count 2 and every consumer
        // rejects it ('expected no more operands after 1 words').
        let id = match (result_ty, result_id) {
            (_, Some(explicit)) => Some(explicit),
            (Some(_), None) => Some(self.gen_id()),
            (None, None) => None,
        };
        let inst = rspirv::dr::Instruction::new(op, result_ty, id, operands);
        self.emit(inst);
        id.unwrap_or_else(|| self.gen_id())
    }

    /// LoopMerge + conditional branch pair (typed — closes the header block).
    pub fn loop_header_tail(
        &mut self,
        cond: Word,
        merge: Word,
        continue_target: Word,
        body: Word,
    ) {
        self.builder
            .loop_merge(merge, continue_target, spirv::LoopControl::empty(), [])
            .expect("loop_merge");
        self.builder
            .branch_conditional(cond, body, merge, [])
            .expect("branch_conditional");
    }
}
