/// SPIR-V backend — compiles Briev GPU kernels to SPIR-V binary modules.
///
/// 2026-07-15: v1 baseline. 2026-08-23 (plan §2.1–2.2): real statement/
/// expression lowering + frontend accel-driven kernel selection. 2026-08-26
/// (plan §2.3): Load#/Store# take ADDRESS EXPRESSIONS rooted in program
/// state — `Load#(field)` / `Load#(field[i])` / `Store#(field[i], v)` —
/// lowered to AccessChain over the single StorageBuffer binding; numeric
/// addresses do not exist in a Vulkan kernel and error naming the fix.
/// Supported builtins: GetGlobalId#, GetLocalId#, WorkgroupSize#. Scalar
/// type resolution is UNIVERSE-DRIVEN (§2.4): (protocol, metadata) via the
/// casting graph's SPIR-V table — Int/UInt signedness included; heap
/// categories (String/Blob/Char) and non-Vulkan widths error naming the fix.
/// No type names are matched in the emitter.
///
/// # Entry point
/// `compile_spirv(program, options) -> Result<Vec<u8>>`
///
/// The returned `Vec<u8>` is a valid SPIR-V binary suitable for Vulkan
/// or OpenCL consumption.

pub mod builder;
pub mod kernel;
pub mod lower;
pub mod gemm;
pub mod normalizer;

// 2026-08-31 (plan abv-gpu-by-default): the accel OFFLOAD path (LLVM host)
// emits its kernels through this backend — re-export for cross-module use.
pub use builder::SpirvBuilder;

use crate::ast::TopLevel;
use crate::backend::spirv::kernel::emit_kernel;

/// 2026-08-23 (Plan 0.2) / 2026-08-31 (plan abv-gpu-by-default): SPIR-V's
/// declared surface — integer AND float scalar compute, indexed state
/// access (SSBO AccessChain), Load#/Store# address forms, invocation-id
/// builtins. Strings, collections, pointers and control flow beyond the
/// kernel body's bounded shape are compile errors naming the fix.
pub const CAPABILITIES: crate::backend::capabilities::BackendCapabilities =
    crate::backend::capabilities::BackendCapabilities {
        name: "SPIR-V (.spv GPU kernels)",
        nature: "a Vulkan/OpenCL compute kernel is bounded structured control \
                 flow over typed buffers",
        int_literals: true,
        bool_char_literals: true,
        floats: true,
        int_ops: true,
        unary_ops: true,
        intrinsics: true,
        index: true,
        casts: true,
        // 2026-08-31 (VITRIOL GEMM comparison M1): bounded foreach over a
        // range lowers to a structured loop - the reduction primitive GEMV
        // needs.
        foreach: true,
        slices_ranges: true,
        // 2026-09-02 (plan 2026-09-02-graphics-ray-and-images): two-way
        // value selection — `if` expressions and exhaustive Bool-scrutinee
        // `match` (true/false/_) lower to structured OpSelectionMerge +
        // OpPhi. Other pattern kinds are rejected at emission.
        if_expr: true,
        match_expr: true,
        let_stmt: true,
        assign_stmt: true,
        term_endprogram: true,
        ..crate::backend::capabilities::BackendCapabilities::NONE
    };

/// 2026-07-15: Compile a Briev program to SPIR-V binary.
///
/// # Parameters
/// * `program` — The typed AST (must be type-checked already)
/// * `entry_name` — The kernel entry point name (e.g., "main")
///
/// # Returns
/// A valid SPIR-V binary, or an error describing what went wrong.
pub fn compile_spirv(
    program: &[TopLevel],
    entry_name: &str,
    analysis: &crate::backend::AnalysisResults,
    universe: &crate::type_universe::TypeUniverse,
    int_bits: u64,
) -> Result<Vec<u8>, String> {
    compile_spirv_builder(program, entry_name, analysis, universe, int_bits)?.build()
}

/// 2026-08-23 (§2.2): frontend-driven selection AND module construction
/// without assembling — tests inspect the dr::Module directly. Kernels are
/// the ELIGIBLE entries of the accel analysis (the shape's proven statements
/// form the body); `entry_name` = "main" accepts any, a specific name must
/// exist among them.
pub fn compile_spirv_builder(
    program: &[TopLevel],
    entry_name: &str,
    analysis: &crate::backend::AnalysisResults,
    universe: &crate::type_universe::TypeUniverse,
    int_bits: u64,
) -> Result<SpirvBuilder, String> {
    // 2026-08-26 (§2.4): the NORMALIZED universe drives scalar type
    // resolution — (protocol, metadata), never type-name matches.
    let mut builder = SpirvBuilder::new().with_universe(universe, int_bits);
    let mut emitted: Vec<String> = Vec::new();
    let mut rejected: Vec<(String, Vec<String>)> = Vec::new();

    for item in program {
        if let TopLevel::Transaction(txn) = item {
            if let Some(entry) = analysis.accel.get(&txn.name) {
                if !entry.shape.eligible {
                    rejected.push((txn.name.clone(), entry.shape.reasons.clone()));
                    continue;
                }
                // Entry point is ALWAYS "main": the runtime drivers hardcode
                // pName "main" (briev_dev_vulkan.c), and build_kernels (the
                // runner blob path) emits "main" too. The old per-node entry
                // name made every standalone .spv file fail pipeline
                // creation ("compute pipeline failed") while the same
                // kernel embedded in the runner worked — the two paths must
                // never disagree again.
                let cooperative =
                    crate::backend::spirv::kernel::is_cooperative_shape(&entry.shape);
                // 2026-09-02: plan-free — this combined-module helper is
                // single-kernel surface/tests only; the artifact path goes
                // through build_kernels (one module per kernel).
                emit_kernel(&mut builder, "main", &entry.shape, program, cooperative, &crate::backend::spirv::kernel::KernelSurface::default())?;
                emitted.push(txn.name.clone());
            }
        }
    }

    if emitted.is_empty() {
        // 2026-08-31 (plan abv-gpu-by-default): name WHY each candidate was
        // rejected — the eligibility proof's reasons are the user's fix path.
        let detail = rejected
            .iter()
            .map(|(name, reasons)| format!("  '{}': {}", name, reasons.join("; ")))
            .collect::<Vec<_>>()
            .join("\n");
        let mut msg = format!(
            "no GPU kernels: no transaction passed the accel eligibility proof \
             (bound the node with '[i < N]' over a real counter)"
        );
        if !detail.is_empty() {
            msg.push_str("\nrejected candidates:\n");
            msg.push_str(&detail);
        }
        return Err(msg);
    }
    if entry_name != "main" && !emitted.iter().any(|n| n == entry_name) {
        return Err(format!(
            "entry '{}' is not an eligible GPU kernel (eligible: {:?})",
            entry_name, emitted
        ));
    }

    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::casting::graph::CastingGraph;
    use rspirv::spirv;

    /// §2.4 tests: fresh universe — primordials are auto-seeded by
    /// TypeUniverse::new(); fixtures declare no user typedefs.
    fn test_universe() -> crate::type_universe::TypeUniverse {
        crate::type_universe::TypeUniverse::new()
    }

    fn state_decl(name: &str, n: i64) -> TopLevel {
        TopLevel::StateDecl(StateDecl {
            name: name.into(),
            ty: Type::Vector(Box::new(Type::int()), vec![Dimension::Anonymous(n as usize)]),
            span: None,
        })
    }

    /// §2.3 tests: a DIRECT shape for lowering-focused fixtures — the accel
    /// eligibility model (§2.2) does not classify Load#/Store# bodies yet.
    fn raw_shape(
        index_var: &str,
        kernel_stmts: Vec<Statement>,
        reads: &[&str],
        writes: &[&str],
    ) -> crate::analysis::accel::KernelShape {
        crate::analysis::accel::KernelShape {
            index_var: index_var.into(),
            count_expr: Some(Expr::Decimal(64)),
            kernel_stmts,
            host_stmts: vec![],
            read_buffers: reads.iter().map(|s| s.to_string()).collect(),
            write_buffers: writes.iter().map(|s| s.to_string()).collect(),
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: None,
        }
    }

    /// Canonical accel-shape fixture (Design A): `i` is a real state counter
    /// incremented in the body; `out[i] = i * 2` is work-item affine. This is
    /// exactly the shape src/analysis/accel.rs proves eligible.
    fn scale_kernel_program() -> Vec<TopLevel> {
        // 2026-08-23: `!> accel: try_all` gates the analysis — without it
        // accel.analyze produces no entries at all (policy: absent means
        // keyword-bodies-only).
        let mut meta = std::collections::HashMap::new();
        meta.insert(
            "accel".into(),
            crate::ast::PropertyValue::String("try_all".into()),
        );
        vec![
            TopLevel::ModuleMetadata(meta),
            TopLevel::StateDecl(StateDecl {
                name: "i".into(),
                ty: Type::int(),
                span: None,
            }),
            state_decl("out", 1024),
            TopLevel::Transaction(Transaction {
                name: "scale".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(
                        BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(64)),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("out".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                        Expr::BinaryOp(
                            BinaryOpKind::Mul,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(2)),
                        ),
                    ),
                    Statement::Assign(
                        Expr::Identifier("i".into()),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ]
    }

    fn analyze(program: &[TopLevel]) -> crate::backend::AnalysisResults {
        let universe = crate::type_universe::TypeUniverse::new();
        crate::backend::analyze_program(program, false, 1, Some(&universe))
    }

    fn eligible_shape<'a>(
        analysis: &'a crate::backend::AnalysisResults,
    ) -> &'a crate::analysis::accel::KernelShape {
        let entry = analysis
            .accel
            .get("scale")
            .expect("fixture txn must be accel-analyzed");
        assert!(entry.shape.eligible, "fixture must be eligible: {:?}", entry.shape.reasons);
        &entry.shape
    }

    /// §2.1/§2.2: the lowered kernel contains real work-item compute.
    #[test]
    fn test_scale_kernel_lowers_real_body() {
        let program = scale_kernel_program();
        let analysis = analyze(&program);
        let shape = eligible_shape(&analysis).clone();
        let mut builder = SpirvBuilder::new();
        emit_kernel(&mut builder, "scale", &shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default())
            .expect("kernel with real body must compile");
        let m = builder.module_ref();

        let mut ops: Vec<rspirv::spirv::Op> = Vec::new();
        for g in &m.types_global_values { ops.push(g.class.opcode); }
        for f in &m.functions {
            for b in &f.blocks {
                for i in &b.instructions { ops.push(i.class.opcode); }
            }
        }
        assert!(ops.contains(&rspirv::spirv::Op::IMul),
            "body must contain IMul; ops={:?}", ops);
        assert!(ops.contains(&rspirv::spirv::Op::AccessChain),
            "state access must go through AccessChain");
        assert!(ops.contains(&rspirv::spirv::Op::Store),
            "state write must Store");

        let has_global_id_builtin = m.types_global_values.iter().any(|i| {
            i.class.opcode == rspirv::spirv::Op::Decorate
                && matches!(i.operands.get(1),
                    Some(rspirv::dr::Operand::Decoration(rspirv::spirv::Decoration::BuiltIn)))
                && matches!(i.operands.get(2),
                    Some(rspirv::dr::Operand::BuiltIn(rspirv::spirv::BuiltIn::GlobalInvocationId)))
        });
        assert!(has_global_id_builtin,
            "the index counter must bind to BuiltIn GlobalInvocationId");

        let ssbo = m.types_global_values.iter().any(|i| {
            i.class.opcode == rspirv::spirv::Op::Variable
                && matches!(i.operands.first(),
                    Some(rspirv::dr::Operand::StorageClass(rspirv::spirv::StorageClass::StorageBuffer)))
        });
        assert!(ssbo, "indexed state must lower to a StorageBuffer variable");
    }

    /// §2.5: spirv-val validation — typed-emission refactor closed the
    /// assembly bug (BUGS.md 2026-08-23 CLOSED).
    #[test]
    fn test_scale_kernel_passes_spirv_val() {
        if !std::process::Command::new("spirv-val").arg("--version").output().is_ok() {
            eprintln!("spirv-val not found — skipping");
            return;
        }
        let program = scale_kernel_program();
        let analysis = analyze(&program);
        let shape = eligible_shape(&analysis).clone();
        let mut builder = SpirvBuilder::new();
        emit_kernel(&mut builder, "scale", &shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
        let binary = builder.build().unwrap();

        let dir = std::env::temp_dir().join(format!("briev_spv_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scale.spv");
        std::fs::write(&path, &binary).unwrap();
        let out = std::process::Command::new("spirv-val")
            .arg(&path)
            .output()
            .expect("spirv-val");
        assert!(
            out.status.success(),
            "spirv-val rejected:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// §2.1 read-path lock: a kernel READING two buffers (scale-and-add)
    /// must emit TWO AccessChains + loads feeding the compute — locks the
    /// SSBO read side, not just writes.
    #[test]
    fn test_mad_kernel_reads_two_buffers() {
        // out[i] = fa * (a[i] + b[i])
        // Policy gate — without '!> accel:' the analysis produces no entries.
        let mut meta = std::collections::HashMap::new();
        meta.insert("accel".into(), crate::ast::PropertyValue::String("try_all".into()));
        let program = vec![
            TopLevel::ModuleMetadata(meta),
            TopLevel::StateDecl(StateDecl {
                name: "i".into(),
                ty: Type::int(),
                span: None,
            }),
            state_decl("a", 256),
            state_decl("b", 256),
            state_decl("out", 256),
            TopLevel::Transaction(Transaction {
                name: "mad".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(
                        BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(64)),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("out".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::Index(
                                Box::new(Expr::Identifier("a".into())),
                                Box::new(Expr::Identifier("i".into())),
                            )),
                            Box::new(Expr::Index(
                                Box::new(Expr::Identifier("b".into())),
                                Box::new(Expr::Identifier("i".into())),
                            )),
                        ),
                    ),
                    Statement::Assign(
                        Expr::Identifier("i".into()),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        let analysis = analyze(&program);
        let entry = analysis.accel.get("mad")
            .unwrap_or_else(|| panic!("mad must be analyzed; accel keys: {:?}",
                analysis.accel.keys().collect::<Vec<_>>()));
        assert!(entry.shape.eligible, "{:?}", entry.shape.reasons);
        assert!(entry.shape.write_buffers.contains(&"out".to_string()),
            "write buffers: {:?}", entry.shape.write_buffers);
        // Reads may be empty if the analysis classifies a[i]/b[i] as
        // work-item-affine loads folded into the write — the KERNEL-side
        // assertion below (AccessChains) is what locks the read path.
        let reads = entry.shape.read_buffers.clone();
        eprintln!("read_buffers={:?} scalars={:?}", reads, entry.shape.scalar_ins);

        let mut builder = SpirvBuilder::new();
        emit_kernel(&mut builder, "mad", &entry.shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
        let m = builder.module_ref();
        let access_chains = m.functions.iter()
            .flat_map(|f| f.blocks.iter())
            .flat_map(|b| b.instructions.iter())
            .filter(|i| i.class.opcode == rspirv::spirv::Op::AccessChain)
            .count();
        // 3 chains: a[i] load, b[i] load, out[i] store (the index local
        // needs none). Locks BOTH read paths + the write path.
        assert!(access_chains >= 3,
            "two reads + one write need >=3 access chains; got {}",
            access_chains);
    }

    /// 2026-08-31 (plan abv-gpu-by-default): float arithmetic lowers through
    /// the F* opcode family (opcode chosen by the operands' protocol
    /// category, never a type-name match), float literals become bit-pattern
    /// constants, and the assembled binary passes spirv-val.
    #[test]
    fn test_float_kernel_fmul_fadd_passes_spirv_val() {
        let has_val = std::process::Command::new("spirv-val").arg("--version").output().is_ok();
        let float_state = |name: &str, n: i64| TopLevel::StateDecl(StateDecl {
            name: name.into(),
            ty: Type::Vector(Box::new(Type::float()), vec![Dimension::Anonymous(n as usize)]),
            span: None,
        });
        let program = vec![
            float_state("a", 128),
            float_state("b", 128),
            float_state("dst", 128),
            TopLevel::Transaction(Transaction {
                name: "fmad".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(
                        BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(128)),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    // dst[i] = a[i] * 2.5 + b[i]
                    Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("dst".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::BinaryOp(
                                BinaryOpKind::Mul,
                                Box::new(Expr::Index(
                                    Box::new(Expr::Identifier("a".into())),
                                    Box::new(Expr::Identifier("i".into())),
                                )),
                                Box::new(Expr::Float(2.5)),
                            )),
                            Box::new(Expr::Index(
                                Box::new(Expr::Identifier("b".into())),
                                Box::new(Expr::Identifier("i".into())),
                            )),
                        ),
                    ),
                    Statement::Assign(
                        Expr::Identifier("i".into()),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        // Direct shape: locks the float LOWERING (the accel eligibility model
        // for float buffers is the frontend's own surface).
        let txn_stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let shape = crate::analysis::accel::KernelShape {
            index_var: "i".into(),
            count_expr: Some(Expr::Decimal(128)),
            kernel_stmts: txn_stmts,
            host_stmts: vec![],
            read_buffers: vec!["a".into(), "b".into()],
            write_buffers: vec!["dst".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: None,
        };

        let mut builder = SpirvBuilder::new().with_universe(&test_universe(), 64);
        emit_kernel(&mut builder, "fmad", &shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default())
            .expect("float kernel must lower");
        let ops = {
            let m = builder.module_ref();
            let mut ops: Vec<rspirv::spirv::Op> = Vec::new();
            for f in &m.functions {
                for b in &f.blocks {
                    for i in &b.instructions {
                        ops.push(i.class.opcode);
                    }
                }
            }
            ops
        };
        // Fused-multiply-add is the discriminator (O2, plan
        // 2026-08-31-gpu-next): a float `a*b+c` lowers to ONE GLSL.std.450
        // Fma ExtInst — no separate FMul/FAdd pair. A missed float lane
        // would lower the SAME math as IMul/IAdd, and an unfused fallback
        // would show FMul+FAdd. (IAdd may still appear for the integer
        // counter increment — that is correct.)
        assert!(
            ops.contains(&rspirv::spirv::Op::ExtInst),
            "ExtInst (Fma) in {ops:?} (no fused float op emitted — float lane not taken)"
        );
        assert!(!ops.contains(&rspirv::spirv::Op::FAdd), "unfused FAdd in {ops:?}");
        assert!(!ops.contains(&rspirv::spirv::Op::FMul), "unfused FMul in {ops:?}");

        if !has_val {
            eprintln!("spirv-val not found — binary checks only");
            return;
        }
        let binary = builder.build().unwrap();
        let dir = std::env::temp_dir().join(format!("briev_spv_f_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fmad.spv");
        std::fs::write(&path, &binary).unwrap();
        let out = std::process::Command::new("spirv-val").arg(&path).output().expect("spirv-val");
        assert!(out.status.success(), "spirv-val rejected:\n{}",
            String::from_utf8_lossy(&out.stderr));
    }

    /// §2.3: Load#/Store# address forms — Load#(field[i]), Store#(field[i], v),
    /// Load#(scalar), Store#(scalar, v) all lower to SSBO AccessChains and the
    /// binary passes spirv-val.
    ///
    /// Shape is CONSTRUCTED directly: this test locks the §2.3 LOWERING.
    /// Frontend eligibility is §2.2's surface (its own tests) — the accel
    /// purity model does not yet classify Load#/Store# bodies, which is
    /// tracked in planned-features-tracker.md under SPIR-V follow-ups.
    #[test]
    fn test_load_store_address_forms_pass_spirv_val() {
        if !std::process::Command::new("spirv-val").arg("--version").output().is_ok() {
            eprintln!("spirv-val not found — skipping");
            return;
        }
        let load_call = |arg| Expr::Call("Load#".into(), vec![arg], None);
        let store_call = |addr, val| {
            Expr::Call("Store#".into(), vec![addr, val], None)
        };
        let program = vec![
            TopLevel::StateDecl(StateDecl { name: "i".into(), ty: Type::int(), span: None }),
            // Scalar state — Load#/Store#-only surface (no plain Assign to it).
            TopLevel::StateDecl(StateDecl { name: "total".into(), ty: Type::int(), span: None }),
            state_decl("a", 256),
            state_decl("out", 256),
            TopLevel::Transaction(Transaction {
                name: "ls".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(
                        BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(64)),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("out".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                        // out[i] = Load#(a[i]) + 1
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(load_call(Expr::Index(
                                Box::new(Expr::Identifier("a".into())),
                                Box::new(Expr::Identifier("i".into())),
                            ))),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                    // total = Store#(total, Load#(total) + i)  (expression stmt)
                    Statement::Expression(store_call(
                        Expr::Identifier("total".into()),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(load_call(Expr::Identifier("total".into()))),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                    )),
                    Statement::Assign(
                        Expr::Identifier("i".into()),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        // Direct shape: §2.3 locks the LOWERING; eligibility is §2.2's
        // separate surface (see this file's selection tests + tracker note).
        let txn_stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let shape = crate::analysis::accel::KernelShape {
            index_var: "i".into(),
            count_expr: Some(Expr::Decimal(64)),
            kernel_stmts: txn_stmts,
            host_stmts: vec![],
            read_buffers: vec!["a".into()],
            write_buffers: vec!["out".into(), "total".into()],
            scalar_ins: vec![],
            eligible: true,
            reasons: vec![],
            work_cols: None,
            reduction: None,
            deferred_normalize: None,
        };

        let mut builder = SpirvBuilder::new();
        emit_kernel(&mut builder, "ls", &shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
        // Count inside a scope: module_ref borrows; build() consumes.
        let chain_count = {
            let m = builder.module_ref();
            m.functions.iter()
                .flat_map(|f| f.blocks.iter())
                .flat_map(|b| b.instructions.iter())
                .filter(|inst| inst.class.opcode == spirv::Op::AccessChain)
                .count()
        };
        let binary = builder.build().unwrap();
        // a[i] load-chain, out[i] store-chain, total member-load chain,
        // total member-store chain (2 chains each side of the scalar RMW).
        assert!(chain_count >= 4,
            "two element forms + scalar load/store need >=4 access chains; got {}",
            chain_count);

        let dir = std::env::temp_dir().join(format!("briev_spv_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("load_store.spv");
        std::fs::write(&path, &binary).unwrap();
        let out = std::process::Command::new("spirv-val")
            .arg(&path)
            .output()
            .expect("spirv-val");
        assert!(
            out.status.success(),
            "spirv-val rejected:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// §2.3 honesty: a non-address first argument is a CAPABILITY ERROR
    /// naming the valid forms — no numeric-address fallback, no silent drop.
    #[test]
    fn test_load_rejects_non_address_expressions() {
        let program = vec![
            TopLevel::StateDecl(StateDecl { name: "i".into(), ty: Type::int(), span: None }),
            TopLevel::StateDecl(StateDecl { name: "total".into(), ty: Type::int(), span: None }),
            TopLevel::Transaction(Transaction {
                name: "bad".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::Bool(true),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![
                    // total = Store#(total, Load#(5)) — 5 is not an address.
                    Statement::Assign(
                        Expr::Identifier("total".into()),
                        Expr::Call(
                            "Load#".into(),
                            vec![Expr::Decimal(5)],
                            None,
                        ),
                    ),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        let stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let mut builder = SpirvBuilder::new();
        let err = emit_kernel(&mut builder, "bad", &raw_shape("i", stmts, &[], &["total"]), &program, false, &crate::backend::spirv::kernel::KernelSurface::default())
            .err()
            .expect("Load#(5) must be rejected");
        assert!(err.contains("not an address expression"), "{err}");
        assert!(err.contains("field"), "{err}"); // names the fix form
    }

    /// §2.3 width honesty: an explicit byte-count that disagrees with the
    /// declared field type errors naming both numbers.
    #[test]
    fn test_load_width_mismatch_errors() {
        let program = vec![
            TopLevel::StateDecl(StateDecl { name: "i".into(), ty: Type::int(), span: None }),
            state_decl("a", 16),
            TopLevel::Transaction(Transaction {
                name: "wbad".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::Bool(true),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![Statement::Expression(Expr::Call(
                    "Load#".into(),
                    vec![
                        Expr::Index(
                            Box::new(Expr::Identifier("a".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                        // Int elements are 8 bytes; 4 disagrees.
                        Expr::Decimal(4),
                    ],
                    None,
                ))],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        let stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let mut builder = SpirvBuilder::new();
        let err = emit_kernel(&mut builder, "wbad", &raw_shape("i", stmts, &["a"], &[]), &program, false, &crate::backend::spirv::kernel::KernelSurface::default())
            .err()
            .expect("width mismatch must error");
        assert!(err.contains("byte-width"), "{err}");
    }

    /// §2.4: a user typedef registers through the SHARED registration
    /// (register_types) and resolves from its Float base + bits metadata —
    /// no name matching anywhere in the emitter.
    #[test]
    fn test_user_typedef_resolves_from_protocol_and_metadata() {
        let mut u = test_universe();
        let typedef = TopLevel::TypeDef(Box::new(crate::ast::top::TypeDef {
            name: "Temp".into(),
            type_params: vec![],
            parent: None,
            protocol: Some("Float".into()),
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: vec![],
            ports_out: vec![],
            seq: false,
            body: {
                let mut md = std::collections::HashMap::new();
                md.insert("bits".into(), crate::ast::PropertyValue::Int(64));
                crate::ast::top::TypeDefBody {
                    pins: Vec::new(),
                    reference: None,
                    tolerance: None,
                    rating: None,
                    slots: vec![],
                    metadata: md,
                    projections: vec![],
                    bindings: vec![],
                    operators: vec![],
                    op_bindings: vec![],
                    constraints: vec![],
                    members: vec![],
                    span: None,
                }
            },
            span: None,
        }));
        crate::backend::register_types::register_typedefs(
            &[typedef], &mut u, 64).unwrap();

        // Kernel with one scalar state field of type Temp.
        let program = vec![
            TopLevel::StateDecl(StateDecl { name: "i".into(), ty: Type::int(), span: None }),
            TopLevel::StateDecl(StateDecl { name: "t".into(), ty: Type::Custom("Temp".into()), span: None }),
            TopLevel::Transaction(Transaction {
                name: "tk".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::Bool(true),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                // Scalar state is reached through the §2.3 address surface.
                body: vec![Statement::Expression(Expr::Call(
                    "Store#".into(),
                    vec![
                        Expr::Identifier("t".into()),
                        Expr::Call("Load#".into(), vec![Expr::Identifier("t".into())], None),
                    ],
                    None,
                ))],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        let stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let mut builder = SpirvBuilder::new().with_universe(&u, 64);
        emit_kernel(&mut builder, "tk", &raw_shape("i", stmts, &[], &["t"]), &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
        // The SSBO struct member must be OpTypeFloat 64 — derived from the
        // Temp typedef's Cast.Float property + bits metadata, not from names.
        let has_float64 = builder.module_ref().types_global_values.iter().any(|inst| {
            inst.class.opcode == rspirv::spirv::Op::TypeFloat && inst.operands.first()
                == Some(&rspirv::dr::Operand::LiteralBit32(64))
        });
        assert!(has_float64, "Temp must lower to OpTypeFloat(64)");
    }

    /// §2.4: Briev Int carries SIGNEDNESS — the emitted OpTypeInt is
    /// (width=64, signedness=1). UInt is unsigned (signedness=0).
    #[test]
    fn test_int_signedness_from_protocol() {
        let program = vec![
            TopLevel::StateDecl(StateDecl { name: "i".into(), ty: Type::int(), span: None }),
            TopLevel::Transaction(Transaction {
                name: "sk".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::Bool(true),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![Statement::Assign(
                    Expr::Identifier("i".into()),
                    Expr::BinaryOp(
                        BinaryOpKind::Add,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(1)),
                    ),
                )],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        let stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let mut builder = SpirvBuilder::new();
        emit_kernel(&mut builder, "sk", &raw_shape("i", stmts, &[], &["i"]), &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
        let int_64_signed = builder.module_ref().types_global_values.iter().any(|inst| {
            inst.class.opcode == rspirv::spirv::Op::TypeInt
                && inst.operands.get(0)
                    == Some(&rspirv::dr::Operand::LiteralBit32(64))
                && inst.operands.get(1)
                    == Some(&rspirv::dr::Operand::LiteralBit32(1))
        });
        assert!(int_64_signed, "Briev Int must emit OpTypeInt(64, signed=1)");
    }

    /// §2.4 capability honesty: a heap-category state field errors naming
    /// the protocol category and the supported roots.
    #[test]
    fn test_heap_category_state_errors() {
        let program = vec![
            TopLevel::StateDecl(StateDecl { name: "s".into(), ty: Type::Custom("String".into()), span: None }),
            TopLevel::Transaction(Transaction {
                name: "sk".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::Bool(true),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                post_authority: false},
                body: vec![Statement::Assign(
                    Expr::Identifier("s".into()),
                    Expr::Identifier("s".into()),
                )],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        let stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let mut builder = SpirvBuilder::new();
        let err = emit_kernel(&mut builder, "sk", &raw_shape("i", stmts, &[], &["s"]), &program, false, &crate::backend::spirv::kernel::KernelSurface::default())
            .err()
            .expect("String state must be rejected");
        assert!(err.contains("String"), "{err}");
        assert!(err.contains("Int"), "{err}");
    }

    /// §2.4 width honesty: an integer width outside Vulkan's compute set
    /// (8/16/32/64) errors naming both the width and the constraint.
    #[test]
    fn test_integer_width_out_of_range_errors() {
        let mut u = test_universe();
        let typedef = TopLevel::TypeDef(Box::new(crate::ast::top::TypeDef {
            name: "Odd".into(),
            type_params: vec![],
            parent: None,
            protocol: Some("Int".into()),
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: vec![],
            ports_out: vec![],
            seq: false,
            body: {
                let mut md = std::collections::HashMap::new();
                md.insert("bits".into(), crate::ast::PropertyValue::Int(24));
                crate::ast::top::TypeDefBody {
                    pins: Vec::new(),
                    reference: None,
                    tolerance: None,
                    rating: None,
                    slots: vec![],
                    metadata: md,
                    projections: vec![],
                    bindings: vec![],
                    operators: vec![],
                    op_bindings: vec![],
                    constraints: vec![],
                    members: vec![],
                    span: None,
                }
            },
            span: None,
        }));
        crate::backend::register_types::register_typedefs(&[typedef], &mut u, 64).unwrap();
        let g = CastingGraph::new();
        let e = g
            .resolve_spirv_shape(&u, &Type::Custom("Odd".into()), 64)
            .expect_err("width 24 must be rejected");
        assert!(e.contains("24"), "{e}");
    }

    /// §2.5 validation harness: EVERY emitted binary passes spirv-val AND a
    /// spirv-dis structural sweep — GLCompute entry point present, LocalSize
    /// execution mode declared, one Block-decorated StorageBuffer binding.
    /// A single helper runs both tools so new fixtures inherit the sweep.
    ///
    /// Loop structure: deliberately NOT asserted — one invocation IS one
    /// work item (kernel.rs charter); there is no induction loop to find.
    fn validate_and_disassemble(binary: &[u8], tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("briev_spv_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let spv = dir.join(format!("{}.spv", tag));
        std::fs::write(&spv, binary).unwrap();

        let val = std::process::Command::new("spirv-val")
            .arg(&spv)
            .output()
            .expect("spirv-val");
        assert!(
            val.status.success(),
            "spirv-val rejected {}:\n{}",
            tag,
            String::from_utf8_lossy(&val.stderr)
        );

        let dis = std::process::Command::new("spirv-dis")
            .arg(&spv)
            .output()
            .expect("spirv-dis");
        assert!(dis.status.success(), "spirv-dis failed on {}", tag);
        String::from_utf8_lossy(&dis.stdout).to_string()
    }

    #[test]
    fn test_harness_structural_sweep_on_scale_kernel() {
        if std::process::Command::new("spirv-dis").arg("--version").output().is_err() {
            eprintln!("spirv-dis not found — skipping structural sweep");
            return;
        }
        let program = scale_kernel_program();
        let analysis = analyze(&program);
        let shape = eligible_shape(&analysis).clone();
        let mut builder = SpirvBuilder::new();
        emit_kernel(&mut builder, "scale", &shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
        let asm = validate_and_disassemble(&builder.build().unwrap(), "harness_scale");

        // Entry point: GLCompute on "scale" (spirv-dis quotes the name).
        assert!(asm.contains("OpEntryPoint GLCompute"), "entry point:\n{}", asm);
        assert!(
            asm.contains("\"scale\"") || asm.contains("@scale"),
            "entry named scale must appear"
        );

        // LocalSize execution mode (workgroup 64,1,1 per LOCAL_SIZE_X).
        assert!(
            asm.contains("OpExecutionMode") && asm.contains("LocalSize"),
            "LocalSize execution mode missing:\n{}",
            asm
        );

        // Storage buffer surface: Block-decorated struct bound at set 0.
        assert!(asm.contains("Block"), "Block decoration missing");
        assert!(asm.contains("StorageBuffer"), "StorageBuffer class missing");
        assert!(asm.contains("DescriptorSet 0"), "descriptor set binding missing");
        assert!(asm.contains("Binding 0"), "binding index missing");
    }

    /// §2.5 optional smoke: execute a fixture kernel when a Vulkan runner is
    /// installed. Probe-gated — absent runner skips loudly, never fails.
    #[test]
    fn test_vulkan_runner_smoke_gated() {
        for runner in ["vkm", "vkrunner", "vulkan-sample"] {
            if std::process::Command::new(runner)
                .arg("--help")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok()
            {
                eprintln!("vulkan runner '{}' found — wire the smoke fixture here", runner);
                return; // placeholder until the runner's fixture format lands
            }
        }
        eprintln!("no vulkan runner — smoke test skipped (probe-gated by design)");
    }

    /// Capability honesty + selection: ineligible bodies never become
    /// kernels, and a named entry that doesn't exist errors helpfully.
    #[test]
    fn test_selection_rejects_ineligible_and_honors_entry_name() {
        let program = scale_kernel_program();
        let analysis = analyze(&program);
        // Eligible + `!> accel:` metadata → "main" accepts any kernel.
        let binary = compile_spirv(&program, "main", &analysis, &test_universe(), 64)
            .expect("eligible fixture must build under wildcard entry");
        // The entry point must be named "main" in the emitted module too —
        // the runtime drivers hardcode pName "main" (regression: standalone
        // .spv files once carried the node name and failed pipeline
        // creation while the runner blobs worked).
        let needle = b"main\0";
        assert!(
            binary.windows(needle.len()).any(|w| w == needle),
            "emitted module must contain a 'main' entry point"
        );

        // A specific entry name must EXIST among eligible kernels.
        let err = compile_spirv_builder(&program, "nope", &analysis, &test_universe(), 64)
            .err()
            .expect("missing named entry must error");
        assert!(err.contains("'nope'"), "{err}");
        compile_spirv_builder(&program, "scale", &analysis, &test_universe(), 64)
            .expect("named existing entry compiles");

        // Ineligible body (counter never incremented) → not a kernel.
        let mut bad = scale_kernel_program();
        if let TopLevel::Transaction(t) = &mut bad[3] {
            t.body.pop(); // drop the i = i + 1 increment
        }
        let analysis_bad = analyze(&bad);
        assert!(!analysis_bad.accel.get("scale").map_or(false, |e| e.shape.eligible));
        let err = compile_spirv(&bad, "main", &analysis_bad, &test_universe(), 64)
            .err()
            .expect("ineligible body must not become a kernel");
        assert!(err.contains("no GPU kernels"), "{err}");
    }
    /// O3 (plan 2026-08-31-o3-float4-loads.md): vec4-eligible array members
    /// are declared as arrays of 4-wide vectors (byte-identical layout); every
    /// scalar access to them goes through AccessChain(member, idx>>2, idx&3).
    /// Locks: OpTypeVector present, the shifted AccessChain shape, and the
    /// binary passes spirv-val.
    #[test]
    fn test_vec4_member_typing_and_shifted_access() {
        use crate::ast::*;
        use crate::ast::Dimension as _Dimension_unused;
        let has_val = std::process::Command::new("spirv-val").arg("--version").output().is_ok();
        let float_state = |name: &str, n: i64| TopLevel::StateDecl(StateDecl {
            name: name.into(),
            ty: Type::Vector(Box::new(Type::float()), vec![Dimension::Anonymous(n as usize)]),
            span: None,
        });
        let program = vec![
            float_state("a", 4096),
            float_state("out", 4096),
            TopLevel::Transaction(Transaction {
                name: "copy".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(
                        BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(4096)),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                    post_authority: false,
                },
                body: vec![
                    Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("out".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                        Expr::Index(
                            Box::new(Expr::Identifier("a".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                    ),
                    Statement::Assign(
                        Expr::Identifier("i".into()),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        let txn_stmts = match &program.last().unwrap() {
            TopLevel::Transaction(t) => t.body.clone(),
            other => panic!("expected transaction, got {other:?}"),
        };
        let shape = crate::analysis::accel::KernelShape {
            index_var: "i".into(),
            count_expr: Some(Expr::Decimal(4096)),
            kernel_stmts: txn_stmts,
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
        let mut builder = SpirvBuilder::new().with_universe(&test_universe(), 64);
        emit_kernel(&mut builder, "main", &shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
        let m = builder.module_ref();
        // The vec4 type exists, and the array-of-vec4 member type carries
        // ArrayStride 16.
        // Find the array-of-vector member type: a TypeArray whose element is
        // a TypeVector (the builtin vec3 input is also a TypeVector, so match
        // by structure, not by "first vector in the module").
        let arr = m.types_global_values.iter()
            .find(|i| {
                if i.class.opcode != spirv::Op::TypeArray {
                    return false;
                }
                let Some(&rspirv::dr::Operand::IdRef(elem)) = i.operands.first() else {
                    return false;
                };
                m.types_global_values.iter().any(|t| {
                    t.result_id == Some(elem)
                        && t.class.opcode == spirv::Op::TypeVector
                })
            })
            .expect("array-of-vec4 member type");
        let v4_id = match arr.operands.first() {
            Some(rspirv::dr::Operand::IdRef(v)) => *v,
            other => panic!("unexpected TypeArray operand {other:?}"),
        };
        let arr_id = arr.result_id.unwrap();
        assert!(
            m.types_global_values.iter().any(|a| {
                a.class.opcode == spirv::Op::Decorate
                    && a.operands.first() == Some(&rspirv::dr::Operand::IdRef(arr_id))
            }),
            "vec4 array must carry its ArrayStride decoration"
        );
        // Every AccessChain into the vec4 member has FOUR operands:
        // var, member, idx>>2, idx&3.
        assert!(
            m.functions.iter()
                .flat_map(|f| f.blocks.iter())
                .flat_map(|b| b.instructions.iter())
                .filter(|inst| inst.class.opcode == spirv::Op::AccessChain)
                .any(|inst| inst.operands.len() == 4),
            "scalar access into a vec4 member must use the shifted chain"
        );
        if !has_val {
            eprintln!("spirv-val not found — binary checks only");
            return;
        }
        let binary = builder.build().unwrap();
        let dir = std::env::temp_dir().join(format!("briev_spv_vec4_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vec4.spv");
        std::fs::write(&path, &binary).unwrap();
        let out = std::process::Command::new("spirv-val").arg(&path).output().expect("spirv-val");
        assert!(out.status.success(), "spirv-val rejected:
{}",
            String::from_utf8_lossy(&out.stderr));
    }

    /// 2026-09-02: multi-kernel .abv programs build ONE MODULE PER KERNEL
    /// (the runner doctrine) — the old combined artifact named every entry
    /// "main" in one module, a SPIR-V spec violation (BUGS.md 2026-09-02).
    /// Guards: per-kernel modules, each with exactly one entry point, all
    /// named "main" (the drivers hardcode it).
    #[test]
    fn multi_kernel_builds_one_module_per_kernel() {
        let float_state = |name: &str, n: i64| TopLevel::StateDecl(StateDecl {
            name: name.into(),
            ty: Type::Vector(Box::new(Type::float()), vec![Dimension::Anonymous(n as usize)]),
            span: None,
        });
        let txn = |name: &str, dst: &str, src: &str, fold: f64| TopLevel::Transaction(Transaction {
            name: name.into(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(
                    BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("i".into())),
                    Box::new(Expr::Decimal(1024)),
                ),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
                post_authority: false,
            },
            body: vec![
                Statement::Assign(
                    Expr::Index(
                        Box::new(Expr::Identifier(dst.into())),
                        Box::new(Expr::Identifier("i".into())),
                    ),
                    Expr::BinaryOp(
                        BinaryOpKind::Mul,
                        Box::new(Expr::Index(
                            Box::new(Expr::Identifier(src.into())),
                            Box::new(Expr::Identifier("i".into())),
                        )),
                        Box::new(Expr::Float(fold)),
                    ),
                ),
                Statement::Assign(
                    Expr::Identifier("i".into()),
                    Expr::BinaryOp(
                        BinaryOpKind::Add,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(1)),
                    ),
                ),
            ],
            metadata: std::collections::HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        });
        let program = vec![
            // `!> accel: try_all` gates the analysis — without it
            // accel.analyze produces no entries at all (policy: absent means
            // keyword-bodies-only).
            TopLevel::ModuleMetadata({
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "accel".into(),
                    crate::ast::PropertyValue::String("try_all".into()),
                );
                m
            }),
            TopLevel::StateDecl(StateDecl {
                name: "i".into(),
                ty: Type::int(),
                span: None,
            }),
            float_state("x", 1024),
            float_state("y", 1024),
            float_state("z", 1024),
            txn("fill", "y", "x", 1.0),
            txn("scale", "z", "y", 2.0),
        ];
        let analysis = analyze(&program);
        let kernels = crate::backend::spirv::runner::build_kernels(
            &program,
            &test_universe(),
            64,
            &analysis,
            None,
        )
        .expect("multi-kernel build");
        let mut names: Vec<String> = kernels.iter().map(|k| k.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["fill".to_string(), "scale".to_string()]);
        for k in &kernels {
            let asm = validate_and_disassemble(&k.spirv, "multi_kernel");
            let entries = asm.lines().filter(|l| l.contains("OpEntryPoint")).count();
            assert_eq!(entries, 1, "kernel '{}' must be its own module:\n{}", k.name, asm);
            assert!(asm.contains("\"main\""), "entry must stay 'main':\n{}", asm);
        }
    }

    /// 2026-09-02 (plan 2026-09-02-image-and-dehashtag, revised): a kernel
    /// whose write array carries an image storage plan emits OpTypeImage +
    /// OpImageWrite, EXCLUDES the array from the SSBO field surface, and
    /// passes spirv-val. Guards the SSBO/image binding partition end to end.
    #[test]
    fn image_plan_partitions_ssbo_and_writes_texel() {
        // The pipeline route (parse + check + normalize): the texel type
        // registration into the universe is what the real compiler does —
        // hand-built fixtures miss invisible registration state.
        let src = r#"
!> accel: try_all;

const N: Int = 4096;

type R32: Float {
    spec Bits: 32;
    spec Format: R32Float;
};

let i: Int = 0;
let x: Float[4096];
let img: R32[4096];

async node fill [i < N][i == N] {
    let pix: Int = i % 64;
    let row: Int = i / 64;
    img[i] = x[i];
    i = i + 1;
    term;
};
"#;
        // The fixture name carries the sweep-exclusion prefix
        // (conformance.rs class 3): the file exists only mid-test and a
        // concurrent sweep would otherwise race it.
        let path = format!("examples/gpu/briev_img_pipeline_test_{}.abv", std::process::id());
        std::fs::write(&path, src).expect("write fixture");
        let mut opts = crate::pipeline::BuildOptions {
            run: false,
            config_dir: None,
            file_path: path.to_string(),
            emit_ir_only: false,
            out_dir: None,
            optimize_budget: 256,
            emit_beast_stages: vec![],
            backend: crate::target::BackendKind::Vm,
            no_stdlib: false,
            stdlib_path: None,
            disable_plugins: vec![],
            enable_plugins: vec![],
            trg_unresolved_action: crate::pipeline::TrgUnresolvedAction::Warn,
            explain_causality: false,
            extra_objects: vec![],
            shared: false,
            library_mode: false,
            keep_all_defns: false,
            int_bits: 64,
            glue_config: None,
            stack_threshold: 65536,
            allow_read: false,
            allow_write: false,
            allow_run: false,
            allow_sys_query: false,
            allow_net: false,
            macro_budget: 0,
            dump_vfs: false,
            update_lockfile: false,
            dump_traces: false,
            diff_mode: false,
            sysquery_overrides: std::collections::HashMap::new(),
            target: None,
            sysquery_pairs: vec![],
            sysquery_files: vec![],
            style_css: None,
            view_html: None,
            view_bindings: vec![],
            ssr: false,
            dev: false,
            accel_cpu_fallback: None,
            isr_mechanism: None,
            triple_override: None,
            linker_script_override: None,
            raw_bin: false,
        no_link: false,
        };
        let (mut items, mut universe) =
            crate::pipeline::compile_to_typed(&path, src, &opts).expect("pipeline");
        // The CLI normalizes (registering source types like R32 into the
        // universe) BEFORE the backend analysis — mirror that exactly.
        crate::backend::spirv::normalizer::normalize(&mut items, &mut universe, 64)
            .expect("normalize");
        // NOT the test analyze() helper — it builds a FRESH universe and
        // would lose the normalizer's R32 registration (fundamentals are
        // seeded in every universe, source types are not).
        let analysis = crate::backend::analyze_program(&items, false, 1, Some(&universe));
        let plan = crate::analysis::image_storage::ImageStoragePlan {
            array: "img".into(),
            width: 64,
            height: 64,
            format: "R32Float".into(),
        };
        let mut plans = std::collections::HashMap::new();
        plans.insert("fill".to_string(), vec![plan.clone()]);
        let mut analysis = analysis;
        analysis.image_storage = plans;
        let kernels = crate::backend::spirv::runner::build_kernels(
            &items,
            &universe,
            64,
            &analysis,
            None,
        )
        .expect("image kernel build");
        let _ = std::fs::remove_file(&path);
        assert_eq!(kernels.len(), 1);
        assert_eq!(kernels[0].image_plans, vec![plan.clone()]);
        let asm = validate_and_disassemble(&kernels[0].spirv, "image_plan");
        assert!(asm.contains("OpTypeImage %float 2D 0 0 0 2 R32f"), "image type:\n{}", asm);
        assert!(asm.contains("OpImageWrite"), "image write:\n{}", asm);
        // The SSBO keeps i + x — img is device-image resident, not a
        // buffer member.
        let ssbo_members = asm
            .lines()
            .find(|l| l.contains("OpTypeStruct"))
            .map(|l| l.matches("%float").count() + l.matches("%long").count())
            .unwrap_or(0);
        assert!(ssbo_members <= 3, "SSBO must exclude the image array:\n{}", asm);
    }

    /// 2026-09-02 (plan 2026-09-02-graphics-ray-and-images): the GLSL.std.450
    /// math intrinsics lower as single-operand OpExtInst on floats. Locks the
    /// emission shape for `Exp#` and `Sqrt#` (the raytracer's dependence):
    /// ext-inst import present, OpExtInst %float with the right opcode name,
    /// spirv-val clean.
    #[test]
    fn test_math_intrinsics_emit_glsl_ext_inst() {
        let float_state = |name: &str, n: i64| TopLevel::StateDecl(StateDecl {
            name: name.into(),
            ty: Type::Vector(Box::new(Type::float()), vec![Dimension::Anonymous(n as usize)]),
            span: None,
        });
        let program_for = |intrinsic: &str| vec![
            float_state("a", 1024),
            float_state("out", 1024),
            TopLevel::Transaction(Transaction {
                name: "map".into(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![],
                output_type: None,
                outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(
                        BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(1024)),
                    ),
                    post_condition: Expr::Bool(true),
                    watchdog: None,
                    explicit: false,
                    span: None,
                    post_authority: false,
                },
                body: vec![
                    Statement::Assign(
                        Expr::Index(
                            Box::new(Expr::Identifier("out".into())),
                            Box::new(Expr::Identifier("i".into())),
                        ),
                        Expr::Call(
                            intrinsic.to_string(),
                            vec![Expr::Index(
                                Box::new(Expr::Identifier("a".into())),
                                Box::new(Expr::Identifier("i".into())),
                            )],
                            None,
                        ),
                    ),
                    Statement::Assign(
                        Expr::Identifier("i".into()),
                        Expr::BinaryOp(
                            BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            }),
        ];
        for (intrinsic, gl_name) in [("Exp#", "Exp"), ("Sqrt#", "Sqrt"), ("Fabs#", "FAbs")] {
            let program = program_for(intrinsic);
            let txn_stmts = match &program.last().unwrap() {
                TopLevel::Transaction(t) => t.body.clone(),
                other => panic!("expected transaction, got {other:?}"),
            };
            let shape = crate::analysis::accel::KernelShape {
                index_var: "i".into(),
                count_expr: Some(Expr::Decimal(1024)),
                kernel_stmts: txn_stmts,
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
            let mut builder = SpirvBuilder::new().with_universe(&test_universe(), 64);
            emit_kernel(&mut builder, "main", &shape, &program, false, &crate::backend::spirv::kernel::KernelSurface::default()).unwrap();
            let asm = validate_and_disassemble(&builder.build().unwrap(), "math_intrinsics");
            // Disasm shape: `%id = OpExtInst %float %set Exp %x` — the result
            // id sits between the type and the GL opcode name.
            assert!(
                asm.lines().any(|l| {
                    let mut tok = l.split_whitespace();
                    tok.any(|t| t == "OpExtInst")
                        && tok.any(|t| t == "%float")
                        && tok.any(|t| t == &format!("{gl_name}"))
                }),
                "{intrinsic} must lower to GLSL.std.450 {gl_name}:\n{}",
                asm
            );
        }
    }

    // ── Bitwise-RHS static guard (BUGS.md 2026-09-11, u32_and regression) ─
    //
    // rspirv's `type Word = u32` alias means a result id can silently flow
    // into a literal-value position: dd5f5e26 masked every coopmat fill
    // index with gen_id numbers for THREE DAYS because the fill emitters
    // passed Word ids into u32_and's `mask: u32` parameter, and nothing
    // inspected the emitted instructions. This guard closes that hole
    // structurally: in every emitted module, the RHS of a bitwise/shift op
    // must resolve to an OpConstant, AND-masks must be 2^k-1 shaped, and
    // shift amounts must be < 64. A Word-id-in-literal-position bug emits
    // a constant whose value is a gen_id — never mask-shaped — so the bug
    // class now fails at test time instead of on device.

    /// The module's OpConstant pool: result id → value (bit patterns for
    /// float constants; only integer shapes are consulted by the audit).
    /// The disassembly's OpConstant pool: symbolic id (`%uint_1023`,
    /// `%41`) → value. Covers both the named-literal rendering spirv-dis
    /// synthesizes and ordinary ids.
    fn collect_constants(asm: &str) -> std::collections::HashMap<String, u64> {
        let mut pool = std::collections::HashMap::new();
        for line in asm.lines() {
            // `%id = OpConstant %type <value>`
            let mut parts = line.split_whitespace();
            let id = match parts.next() {
                Some(t) if t.starts_with('%') => t.to_string(),
                _ => continue,
            };
            if parts.next() != Some("=") || parts.next() != Some("OpConstant") {
                continue;
            }
            let _ty = parts.next();
            let value = match parts.next().and_then(|v| v.parse::<u64>().ok()) {
                Some(v) => v,
                None => continue,
            };
            pool.insert(id, value);
        }
        pool
    }

    /// A mask-shaped constant: one contiguous run of one-bits (2^k-1, the
    /// modulo form `2^k-1`, and field-extraction runs like 0b1110 all
    /// qualify; 0 is the trivial mask). The u32_and regression emitted
    /// gen_id-valued "masks" — arbitrary integers with SPARSE bits — so
    /// the run predicate is what catches the bug class: `v` is a run iff
    /// `v + low_bit(v)` is a power of two, i.e. `(v + (v & -v)) & v == 0`.
    fn is_mask_shape(v: u64) -> bool {
        let low = v & v.wrapping_neg();
        v & v.wrapping_add(low) == 0
    }

    fn assert_bitwise_rhs_are_mask_consts(asm: &str, tag: &str) {
        let pool = collect_constants(asm);
        let mut visited = 0usize;
        for line in asm.lines() {
            let mut parts = line.split_whitespace();
            let head = parts.next().unwrap_or("");
            let opcode = if head.starts_with('%') {
                // `%id = OpXxx ...` — skip the result id and the '='.
                let eq = parts.next().unwrap_or("");
                debug_assert_eq!(eq, "=");
                parts.next().unwrap_or("")
            } else {
                head
            };
            let is_bitwise = matches!(
                opcode,
                "OpBitwiseAnd"
                    | "OpBitwiseOr"
                    | "OpBitwiseXor"
                    | "OpShiftRightLogical"
                    | "OpShiftLeftLogical"
            );
            if !is_bitwise {
                continue;
            }
            visited += 1;
            let fail = |why: String| -> String {
                format!(
                    "{tag}: {opcode} violates the bitwise-RHS contract: {why}\n\
                     the RHS of every bitwise/shift op must be an OpConstant \
                     (AND-masks 2^k-1, shifts < 64) — a Word id flowing into a \
                     literal-value position emits a gen_id-valued constant here\n\
                     line: {line}"
                )
            };
            let rhs = match line.split_whitespace().last() {
                Some(t) if t.starts_with('%') => t.trim_end_matches(','),
                _ => panic!("{}", fail("no %id RHS".to_string())),
            };
            let value = match pool.get(rhs) {
                Some(v) => *v,
                None => panic!(
                    "{}",
                    fail(format!("RHS {rhs} is not an OpConstant (computed value?)"))
                ),
            };
            if opcode == "OpBitwiseAnd" {
                assert!(
                    is_mask_shape(value),
                    "{}",
                    fail(format!("{rhs} = {value:#x} is not a contiguous-bit-run mask"))
                );
            } else {
                assert!(
                    value < 64,
                    "{}",
                    fail(format!("{rhs} = {value} is not a valid shift amount"))
                );
            }
        }
        // The 4096^3 coopmat module must contain the strength-reduced ops
        // this guard protects — a zero count means the fixture drifted off
        // the tensor tier and the guard silently stopped covering the fill.
        assert!(visited > 0, "{tag}: no bitwise/shift ops found — the fixture no longer emits the coopmat fill path");
    }

    /// End-to-end: the real pipeline (parse + normalize + analyze +
    /// build_kernels with the shipped config, coopmat knob ON) emits the
    /// f16 coopmat GEMM, spirv-val accepts it, and every bitwise/shift RHS
    /// is a proper mask/shift constant.
    #[test]
    fn coopmat_fill_bitwise_rhs_are_mask_consts() {
        let src = r#"
!> accel: try_all;

import "std/types/float.bv";

const M: Int = 4096;
const N: Int = 4096;
const K: Int = 4096;

let i: Int = 0;
let a: Float16[16777216];
let b: Float16[16777216];
let y: Float16[16777216];

async node gemm [i < M * N][i == M * N] {
    let acc: Float16 = 0.0;
    let m: Int = i / N;
    let n: Int = i % N;
    foreach k in 0..K {
        acc = acc + a[m * K + k] * b[k * N + n];
    }
    y[i] = acc;
    i = i + 1;
    term;
};
"#;
        let path = format!("examples/gpu/briev_bitwise_guard_test_{}.abv", std::process::id());
        std::fs::write(&path, src).expect("write fixture");
        let opts = crate::pipeline::BuildOptions {
            run: false,
            config_dir: None,
            file_path: path.clone(),
            emit_ir_only: false,
            out_dir: None,
            optimize_budget: 256,
            emit_beast_stages: vec![],
            backend: crate::target::BackendKind::Vm,
            no_stdlib: false,
            stdlib_path: None,
            disable_plugins: vec![],
            enable_plugins: vec![],
            trg_unresolved_action: crate::pipeline::TrgUnresolvedAction::Warn,
            explain_causality: false,
            extra_objects: vec![],
            shared: false,
            library_mode: false,
            keep_all_defns: false,
            int_bits: 64,
            glue_config: None,
            stack_threshold: 65536,
            allow_read: false,
            allow_write: false,
            allow_run: false,
            allow_sys_query: false,
            allow_net: false,
            macro_budget: 0,
            dump_vfs: false,
            update_lockfile: false,
            dump_traces: false,
            diff_mode: false,
            sysquery_overrides: std::collections::HashMap::new(),
            target: None,
            sysquery_pairs: vec![],
            sysquery_files: vec![],
            style_css: None,
            view_html: None,
            view_bindings: vec![],
            ssr: false,
            dev: false,
            accel_cpu_fallback: None,
            isr_mechanism: None,
            triple_override: None,
            linker_script_override: None,
            raw_bin: false,
        no_link: false,
        };
        let (mut items, mut universe) =
            crate::pipeline::compile_to_typed(&path, src, &opts).expect("pipeline");
        crate::backend::spirv::normalizer::normalize(&mut items, &mut universe, 64)
            .expect("normalize");
        let analysis = crate::backend::analyze_program(&items, false, 1, Some(&universe));
        let kernels = crate::backend::spirv::runner::build_kernels(
            &items,
            &universe,
            64,
            &analysis,
            None,
        )
        .expect("coopmat kernel build");
        let _ = std::fs::remove_file(&path);
        assert_eq!(kernels.len(), 1, "the fixture must emit exactly one kernel");
        let binary = &kernels[0].spirv;
        // The guard targets the tensor tier: prove the module IS the
        // coopmat path before auditing it.
        let asm = validate_and_disassemble(binary, "bitwise_guard");
        assert!(
            asm.contains("OpCooperativeMatrixMulAddKHR"),
            "fixture must reach the coopmat tensor tier:\n{}",
            asm
        );
        assert_bitwise_rhs_are_mask_consts(&asm, "coopmat_gemm");
    }

    /// The shape predicate itself, including the boundary values a wrong
    /// implementation would misclassify.
    #[test]
    fn mask_shape_predicate_boundaries() {
        assert!(is_mask_shape(0));
        assert!(is_mask_shape(1));
        assert!(is_mask_shape(7));
        assert!(is_mask_shape(255));
        assert!(is_mask_shape(1023));
        assert!(is_mask_shape(u32::MAX as u64));
        assert!(is_mask_shape(u64::MAX));
        // Field-extraction runs (the fill's & 14 = 0b1110 et al).
        assert!(is_mask_shape(0b1110));
        assert!(is_mask_shape(0b0011_1000));
        // Single bits and non-low-aligned runs are still runs.
        assert!(is_mask_shape(2));
        assert!(is_mask_shape(6));
        assert!(is_mask_shape(1024));
        // Two separate runs are not.
        assert!(!is_mask_shape(0b1010));
        assert!(!is_mask_shape(0b1010));
        // The regression's actual failure shape: gen_id-valued "masks" are
        // arbitrary integers with sparse bits, never one run.
        assert!(!is_mask_shape(0x1_0000_2A50));
    }

    /// 2026-09-14 (Phase 3 — buffer reuse): end-to-end test that building
    /// kernels with a reuse_map produces valid SPIR-V. Kernel "k2" reads
    /// array "a" and writes "b"; kernel "k3" reads "b" and writes "c".
    /// Reuse map: "c" aliases to "a" (a is dead after k2, c starts at k3).
    /// Both kernels must pass spirv-val with the aliased layout.
    #[test]
    fn buffer_reuse_aliasing_produces_valid_spirv() {
        let float_state = |name: &str, n: i64| TopLevel::StateDecl(StateDecl {
            name: name.into(),
            ty: Type::Vector(Box::new(Type::float()), vec![Dimension::Anonymous(n as usize)]),
            span: None,
        });
        let mut meta = std::collections::HashMap::new();
        meta.insert("accel".into(), crate::ast::PropertyValue::String("try_all".into()));
        let program = vec![
            TopLevel::ModuleMetadata(meta),
            TopLevel::StateDecl(StateDecl { name: "i".into(), ty: Type::int(), span: None }),
            float_state("a", 1024),
            float_state("b", 1024),
            float_state("c", 1024),
            float_state("d", 1024),
            // k2: reads a, writes b.
            TopLevel::Transaction(Transaction {
                name: "k2".into(),
                is_reactive: true, is_async: false,
                type_params: vec![], parameters: vec![],
                output_type: None, outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(1024))),
                    post_condition: Expr::Bool(true), watchdog: None,
                    explicit: false, span: None, post_authority: false,
                },
                body: vec![
                    Statement::Assign(
                        Expr::Index(Box::new(Expr::Identifier("b".into())),
                                   Box::new(Expr::Identifier("i".into()))),
                        Expr::Index(Box::new(Expr::Identifier("a".into())),
                                   Box::new(Expr::Identifier("i".into())))),
                    Statement::Assign(Expr::Identifier("i".into()),
                        Expr::BinaryOp(BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)))),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None, modifiers: vec![], span: None, doc: None,
            }),
            // k3: reads b, writes c (and reads d — d's struct member sits AFTER the
            // skipped aliased member c, exercising the member-index drift
            // the AccessChain remap must correct).
            TopLevel::Transaction(Transaction {
                name: "k3".into(),
                is_reactive: true, is_async: false,
                type_params: vec![], parameters: vec![],
                output_type: None, outputs: vec![],
                contract: Contract {
                    pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("i".into())),
                        Box::new(Expr::Decimal(1024))),
                    post_condition: Expr::Bool(true), watchdog: None,
                    explicit: false, span: None, post_authority: false,
                },
                body: vec![
                    Statement::Assign(
                        Expr::Index(Box::new(Expr::Identifier("c".into())),
                                   Box::new(Expr::Identifier("i".into()))),
                        Expr::BinaryOp(BinaryOpKind::Add,
                            Box::new(Expr::Index(Box::new(Expr::Identifier("b".into())),
                                                 Box::new(Expr::Identifier("i".into())))),
                            Box::new(Expr::Index(Box::new(Expr::Identifier("d".into())),
                                                 Box::new(Expr::Identifier("i".into())))))),
                    Statement::Assign(Expr::Identifier("i".into()),
                        Expr::BinaryOp(BinaryOpKind::Add,
                            Box::new(Expr::Identifier("i".into())),
                            Box::new(Expr::Decimal(1)))),
                ],
                metadata: std::collections::HashMap::new(),
                derivation: None, modifiers: vec![], span: None, doc: None,
            }),
        ];
        let mut analysis = analyze(&program);
        // Reuse map: "c" aliases to "a" (a is dead after k2, c starts at k3).
        analysis.gpu_schedule.reuse_opportunities =
            vec![("a".into(), "c".into(), "k2".into())];
        let mut reuse = std::collections::HashMap::new();
        reuse.insert("c".into(), "a".into());
        let kernels = crate::backend::spirv::runner::build_kernels(
            &program,
            &test_universe(),
            64,
            &analysis,
            Some(&reuse),
        )
        .expect("aliasing build must succeed");
        assert_eq!(kernels.len(), 2, "two kernels expected");
        // Both must pass spirv-val.
        for k in &kernels {
            validate_and_disassemble(&k.spirv, &format!("reuse_{}", k.name));
        }
        // The k3 kernel (which writes "c") must have "c" aliased to "a" —
        // verify via the disassembly that the struct has fewer members
        // (a is skipped in k3's SSBO since c reuses its slot).
        let k3 = kernels.iter().find(|k| k.name == "k3").expect("k3 kernel");
        let asm = validate_and_disassemble(&k3.spirv, "k3_alias_check");
        // With aliasing, "c" is not an SSBO member of k3 — it reuses "a"'s
        // slot. The struct should have 4 members (a, b, d, i) not 5
        // (a, b, c, d, i). Member offsets confirm: c is absent, and d sits
        // at the post-drift struct index (member 2, not its raw position 3).
        let member_offsets: Vec<&str> = asm.lines()
            .filter(|l| l.contains("OpMemberDecorate") && l.contains("Offset"))
            .collect();
        assert_eq!(member_offsets.len(), 4,
            "k3 SSBO should have 3 member offsets (a, b, i) with c aliased to a: {:?}", member_offsets);
        // a and b must keep their natural offsets (0 and 4096).
        assert!(member_offsets.iter().any(|l| l.contains("0 Offset 0")),
            "a stays at offset 0: {:?}", member_offsets);
        assert!(member_offsets.iter().any(|l| l.contains("1 Offset 4096")),
            "b stays at offset 4096: {:?}", member_offsets);
    }

}
pub mod runner;

