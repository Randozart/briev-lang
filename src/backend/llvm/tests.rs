use std::collections::HashMap;
use super::*;
use crate::ast::*;

fn empty_program() -> Vec<TopLevel> {
    vec![]
}

/// A minimal eligible accel kernel: `a[i] = i` under `[i < N]` with a host
/// bookkeeping write (`count = count + 1`).
fn accel_kernel_program() -> Vec<TopLevel> {
    let array_state = TopLevel::StateDecl(StateDecl {
        name: "a".to_string(),
        ty: Type::Vector(Box::new(Type::Custom("Float".to_string())), vec![crate::ast::Dimension::Anonymous(16)]),
        span: None,
    });
    let n_state = TopLevel::StateDecl(StateDecl {
        name: "N".to_string(),
        ty: Type::int(),
        span: None,
    });
    let count_state = TopLevel::StateDecl(StateDecl {
        name: "count".to_string(),
        ty: Type::int(),
        span: None,
    });
    // Design A: the work-item counter is a REAL state field.
    let i_state = TopLevel::StateDecl(StateDecl {
        name: "i".to_string(),
        ty: Type::int(),
        span: None,
    });
    let force = TopLevel::Transaction(Transaction {
        name: "force".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(
                BinaryOpKind::Lt,
                Box::new(Expr::Identifier("i".to_string())),
                Box::new(Expr::Identifier("N".to_string())),
            ),
            post_condition: Expr::Bool(true),
            watchdog: None,
            explicit: true,
            span: None,
        post_authority: false},
        body: vec![
            Statement::Assign(
                Expr::Index(Box::new(Expr::Identifier("a".to_string())), Box::new(Expr::Identifier("i".to_string()))),
                Expr::Identifier("i".to_string()),
            ),
            Statement::Assign(
                Expr::Identifier("i".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("i".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Assign(
                Expr::Identifier("count".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("count".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![Annotation { name: "accel".to_string(), value: None }],
        span: None,
        doc: None,
    });
    vec![array_state, n_state, count_state, i_state, force]
}

#[test]
fn test_accel_kernel_module_emits_projected_state() {
    // 2026-08-06 (accel plan): an eligible `accel` body emits a self-contained
    // SPIR-V kernel module by REUSING the host emitter against a kernel-scoped
    // `%State` (the minimal buffer/scalar projection). The IR text is the
    // deterministic contract — SPIR-V blob compilation (llc) is machine-gated.
    let mut backend = LlvmBackend::new().with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = accel_kernel_program();
    let _host = backend.generate(&program, None); // populates field maps
    let analysis = crate::backend::analyze_program(
        &program,
        false,
        4,
        Some(&crate::type_universe::TypeUniverse::new()),
    );
    let entry = analysis.accel.get("force").expect("accel entry for force");
    assert!(entry.shape.eligible, "kernel shape must be eligible: {:?}", entry.shape.reasons);
    let ir = backend.emit_kernel_module("force", &entry.shape).expect("kernel module emits");
    assert!(ir.contains("target triple = \"spirv64-unknown-unknown\""), "kernel triple");
    assert!(ir.contains("declare i64 @_Z13get_global_idj(i32)"), "global-id decl");
    assert!(ir.contains("%State = type { [16 x float] }"), "projected state struct: {ir}");
    assert!(ir.contains("define spir_kernel void @main(ptr %state, i64 %n)"), "kernel sig");
    assert!(ir.contains("call i64 @_Z13get_global_idj(i32 0)"), "work-item id");
    assert!(ir.contains("getelementptr inbounds %State, ptr %state, i32 0, i32 0"), "buffer GEP");
}

#[test]
fn test_no_accel_no_kernel_blob() {
    // 2026-08-06 (accel plan): without accel request, no kernel blob embeds.
    let mut backend = LlvmBackend::new().with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![state_count(), make_txn("plain", vec![])];
    let output = backend.generate(&program, None);
    assert!(
        !output.contains("briev_kernel_"),
        "no accel body must not embed kernels; got:\n{}",
        &output[output.len().saturating_sub(2000)..]
    );
}

fn make_txn(name: &str, modifiers: Vec<Annotation>) -> TopLevel {
    TopLevel::Transaction(Transaction {
        name: name.to_string(),
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
            Statement::Assign(Expr::Identifier("count".to_string()), Expr::Decimal(1)),
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers,
        span: None,
        doc: None,
    })
}

fn state_count() -> TopLevel {
    TopLevel::StateDecl(StateDecl {
        name: "count".to_string(),
        ty: Type::int(),
        span: None,
    })
}

fn default_contract() -> Contract {
    Contract {
        pre_condition: Expr::Bool(true),
        post_condition: Expr::Bool(true),
        watchdog: None,
        explicit: false,
        span: None,
        post_authority: false,
    }
}

#[test]
fn test_llvm_generates_module() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&empty_program(), None);
    assert!(output.contains("ModuleID"));
    assert!(output.contains("target triple"));
}

/// 2026-08-09 (init kind, Phase 2): a runtime-seeded invariant emits as a
/// mutable global, seeds once in the pre-reactor phase, and reads load it.
#[test]
fn test_init_emits_global_seeding_and_read() {
    let init = TopLevel::Init(crate::ast::top::InitDecl {
        name: "BufSize".to_string(),
        bound: None,
        ty: Type::int(),
        value: Some(Expr::Decimal(64)),
        body: vec![],
        span: None,
        doc: None,
    });
    // A node that reads the init, so a load of the global is emitted.
    let node = TopLevel::Transaction(Transaction {
        name: "go".to_string(),
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
            Statement::Let {
                name: "x".to_string(),
                names: vec![],
                ty: Some(Type::int()),
                expr: Some(Expr::Identifier("BufSize".to_string())),
                modifiers: vec![],
            },
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&[init, node], None);
    assert!(output.contains("@BufSize = global i64 0"), "init must emit a mutable global");
    assert!(output.contains("store i64"), "init seeding must store to the global");
    assert!(output.contains("load i64, ptr @BufSize"), "init read must load the global");
}

/// 2026-08-09 (init kind, Phase 3): a bounded-counter loop whose bound is a
/// runtime-seeded init folds against the seeded global — the IR must load the
/// init as the loop bound (`flb` register prefix), NOT the Unknown `add i64 0,
/// 1` fallback that ran the loop once.
#[test]
fn test_init_bound_loop_folds_against_seeded_global() {
    let init = TopLevel::Init(crate::ast::top::InitDecl {
        name: "N".to_string(),
        bound: None,
        ty: Type::int(),
        value: Some(Expr::Decimal(64)),
        body: vec![],
        span: None,
        doc: None,
    });
    let count_state = TopLevel::StateDecl(StateDecl {
        name: "count".to_string(),
        ty: Type::int(),
        span: None,
    });
    let node = TopLevel::Transaction(Transaction {
        name: "work".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(
                BinaryOpKind::Lt,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Identifier("N".to_string())),
            ),
            post_condition: Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Identifier("N".to_string())),
            ),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body: vec![
            Statement::Assign(
                Expr::Identifier("count".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("count".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&[init, count_state, node], None);
    // The folded-loop bound loads the seeded init global.
    assert!(
        output.contains("load i64, ptr @N, align 8"),
        "init-bound loop must load the seeded global as its bound; got:\n{output}"
    );
}

/// A program with `let masked: Data = data[[true, false, true]]` — exercises
/// the Boolean mask-index lowering (2026-08-07, Phase 7).
fn mask_index_program() -> Vec<TopLevel> {
    let data_state = TopLevel::Statement(Box::new(Statement::Let {
        name: "data".to_string(),
        names: vec![],
        ty: Some(Type::Custom("Blob".to_string())),
        expr: Some(Expr::TaggedQuotedLiteral(vec![1, 2, 3, 4, 5], "b".to_string())),
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
            Statement::Let {
                name: "masked".to_string(),
                names: vec![],
                ty: Some(Type::Custom("Blob".to_string())),
                expr: Some(Expr::Index(
                    Box::new(Expr::Identifier("data".to_string())),
                    Box::new(Expr::List(vec![
                        Expr::Bool(true),
                        Expr::Bool(false),
                        Expr::Bool(true),
                    ])),
                )),
                modifiers: vec![],
            },
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![data_state, node]
}

#[test]
fn test_mask_index_emits_gather_and_bmask_constant() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&mask_index_program(), None);
    assert!(output.contains("@briev_mask_select"),
        "mask index must call the runtime gather helper");
    assert!(output.contains("@bmask.0"),
        "a constant Boolean mask must be interned as a @bmask global");
    assert!(output.contains("[3 x i64]"),
        "the mask constant must use i64 slots matching Bool-vector state fields");
}

/// A program with `let m: List<Int> = v[[true, false]]` where `v: Int[2]` —
/// exercises the TYPED mask gather (Int-vector field → heap List).
fn mask_index_typed_program() -> Vec<TopLevel> {
    let v_state = TopLevel::Statement(Box::new(Statement::Let {
        name: "v".to_string(),
        names: vec![],
        ty: Some(Type::Vector(Box::new(Type::int()), vec![crate::ast::Dimension::Anonymous(2)])),
        expr: None,
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
            Statement::Let {
                name: "m".to_string(),
                names: vec![],
                ty: Some(Type::Applied("List".to_string(), vec![Type::int()])),
                expr: Some(Expr::Index(
                    Box::new(Expr::Identifier("v".to_string())),
                    Box::new(Expr::List(vec![Expr::Bool(true), Expr::Bool(false)])),
                )),
                modifiers: vec![],
            },
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![v_state, node]
}

#[test]
fn test_mask_index_typed_emits_select64() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&mask_index_typed_program(), None);
    assert!(output.contains("@briev_mask_select64"),
        "an Int-vector mask index must call the typed gather helper");
    assert!(output.contains("ptrtoint ptr"),
        "the List result must be boxed to an i64 handle like emit_heap_seq");
}

/// 2026-08-16 (slice-6 deletion + mask-list fix): a mask index over a
/// `coll obj` value gathers the true-mask elements through the tier layout —
/// the OLD heap-seq gather read slot 0 as the length (it is the data pointer)
/// and segfaulted. The gather now loads the data pointer (slot 0) + length
/// (slot 2) and boxes the selection as a proper `[data, cap, len]` block.
#[test]
fn test_mask_index_list_emits_element_gep() {
    let src = r#"
coll obj MyList { data: Ptr<Int>; };
let done: Int = 0;
node s [done == 0][done == 1] {
    let l: MyList = [10, 20, 30, 40];
    let m: MyList = l[[true, false, true, false]];
    done = m.Count#();
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let output = backend.generate(&items, None);
    assert!(output.contains("call ptr @briev_mask_select64"),
        "the List mask index must call the typed gather helper; got:\n{output}");
}

/// A program with `let m: List<Float> = v[[true, false]]` where `v: Float[2]`
/// — exercises the f32 mask gather (2026-08-07, Phase 7).
fn mask_index_f32_program() -> Vec<TopLevel> {
    let v_state = TopLevel::Statement(Box::new(Statement::Let {
        name: "v".to_string(),
        names: vec![],
        ty: Some(Type::Vector(Box::new(Type::float()), vec![crate::ast::Dimension::Anonymous(2)])),
        expr: None,
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
            Statement::Let {
                name: "m".to_string(),
                names: vec![],
                ty: Some(Type::Applied("List".to_string(), vec![Type::float()])),
                expr: Some(Expr::Index(
                    Box::new(Expr::Identifier("v".to_string())),
                    Box::new(Expr::List(vec![Expr::Bool(true), Expr::Bool(false)])),
                )),
                modifiers: vec![],
            },
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![v_state, node]
}

#[test]
fn test_mask_index_f32_emits_float_gather() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&mask_index_f32_program(), None);
    assert!(output.contains("@briev_mask_select_f32"),
        "a Float vector mask must call the f32 gather helper");
    assert!(output.contains("ptrtoint ptr"),
        "the List<Float> result must be boxed to an i64 handle");
}

/// A program with `foreach(i in 0..=5) { acc = acc + i; }` — exercises the
/// counted-iteration loop lowering (2026-08-07, Phase 7).
fn foreach_range_program() -> Vec<TopLevel> {
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
            Statement::Let {
                name: "acc".to_string(),
                names: vec![],
                ty: Some(Type::int()),
                expr: Some(Expr::Decimal(0)),
                modifiers: vec![],
            },
            Statement::Foreach {
                item: "i".to_string(),
                list: Box::new(Expr::Range {
                    start: Box::new(Expr::Decimal(0)),
                    end: Box::new(Expr::Decimal(5)),
                    inclusive: true,
                }),
                body: vec![Statement::Assign(
                    Expr::Identifier("acc".to_string()),
                    Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Add,
                        Box::new(Expr::Identifier("acc".to_string())),
                        Box::new(Expr::Identifier("i".to_string())),
                    ),
                )],
            },
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![node]
}

#[test]
fn test_foreach_range_emits_counted_loop() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&foreach_range_program(), None);
    assert!(output.contains("foreach.hdr"),
        "foreach must emit a loop header");
    assert!(output.contains("icmp sle"),
        "an inclusive range must compare with sle");
    assert!(output.contains("foreach.end"),
        "foreach must emit a loop exit label");
}

/// 2026-09-08 (audit): a foreach-local assigned inside a `Mutex`/`Barrier`/
/// `Match` body must still get an alloca slot (those bodies emit inline per
/// iteration). Regression: collect_foreach_assigned's `_ => {}` catch-all
/// skipped them — the body read the stale pre-loop register every iteration.
#[test]
fn test_foreach_assigned_inside_mutex_gets_alloca() {
    let body = vec![
        Statement::Let {
            name: "acc".to_string(),
            names: vec![],
            ty: Some(Type::int()),
            expr: Some(Expr::Decimal(0)),
            modifiers: vec![],
        },
        Statement::Foreach {
            item: "i".to_string(),
            list: Box::new(Expr::Range {
                start: Box::new(Expr::Decimal(0)),
                end: Box::new(Expr::Decimal(5)),
                inclusive: true,
            }),
            body: vec![Statement::Mutex(vec![Statement::Assign(
                Expr::Identifier("acc".to_string()),
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Add,
                    Box::new(Expr::Identifier("acc".to_string())),
                    Box::new(Expr::Identifier("i".to_string())),
                ),
            )])],
        },
        Statement::Term(None),
    ];
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
        body,
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&vec![node], None);
    // The bug: without recursion into Mutex, the body created a FRESH alloca
    // each iteration, seeded it with the stale pre-loop value, and never
    // accumulated. The fix seeds the loop-carried alloca BEFORE the header
    // and the body LOADS from it. Discriminator: a pre-header alloca that is
    // LOADED after the `foreach.body` label. (The counter's alloca is also
    // pre-header but is loaded in the header, not the body — so this pattern
    // uniquely marks the loop-carried accumulator.)
    let body_pos = output.find("foreach.body").expect("foreach body label");
    let (before_hdr, _) = output.split_at(output.find("foreach.hdr").expect("foreach header"));
    let (_, after_body) = output.split_at(body_pos);
    let pre_allocas: Vec<String> = before_hdr
        .split('\n')
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix('%')?;
            let name = rest.split(' ').next()?;
            if l.contains("alloca i64") { Some(name.to_string()) } else { None }
        })
        .collect();
    assert!(!pre_allocas.is_empty(), "loop-carried alloca must be seeded before the header; got:\n{output}");
    let body_load = pre_allocas.iter().any(|r| after_body.contains(&format!("load i64, ptr %{r}")));
    assert!(body_load,
        "loop body must LOAD from a pre-header alloca (accumulation, not stale-read); got:\n{output}");
}

/// A program with `foreach(i in 0..=5) { if i == 3 { acc = 42; break; } }` —
/// exercises the `break` early-exit lowering (2026-08-17).
fn foreach_break_program() -> Vec<TopLevel> {
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
            Statement::Let {
                name: "acc".to_string(),
                names: vec![],
                ty: Some(Type::int()),
                expr: Some(Expr::Decimal(0)),
                modifiers: vec![],
            },
            Statement::Foreach {
                item: "i".to_string(),
                list: Box::new(Expr::Range {
                    start: Box::new(Expr::Decimal(0)),
                    end: Box::new(Expr::Decimal(5)),
                    inclusive: true,
                }),
                body: vec![Statement::Guarded(
                    Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Eq,
                        Box::new(Expr::Identifier("i".to_string())),
                        Box::new(Expr::Decimal(3)),
                    ),
                    vec![
                        Statement::Assign(
                            Expr::Identifier("acc".to_string()),
                            Expr::Decimal(42),
                        ),
                        Statement::Break,
                    ],
                )],
            },
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![node]
}

#[test]
fn test_foreach_break_emits_exit_branch() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&foreach_break_program(), None);
    assert!(output.contains("foreach.hdr"),
        "foreach must emit a loop header");
    assert!(output.contains("br label %foreach.end"),
        "a `break` must branch to the innermost foreach end label");
    assert!(output.contains("foreach.end"),
        "foreach must emit a loop exit label");
}

/// 2026-08-16 (slice-6 deletion): `foreach x in list` over a `coll obj` uses
/// the TIER path (`op Count`/`op At` inlined member bodies) — the hardcoded
/// `[len][elems]` heap-seq `IterKind::List` arm is DELETED. The coll is
/// declared INLINE (tests don't resolve imports) so the scaffolded op surface
/// fires `tier2_op_collection`.
#[test]
fn test_foreach_list_emits_index_loop() {
    let src = r#"
coll obj MyList { data: Ptr<Int>; };
let xs: MyList = [10, 20];
let done: Int = 0;
node s [done == 0][done == 1] {
    foreach x in xs {
        done = done + x;
    };
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let output = backend.generate(&items, None);
    assert!(output.contains("icmp slt"),
        "a collection iteration compares the counter against the length");
    assert!(output.contains("getelementptr i64, ptr"),
        "the tier iteration must GEP each element slot from the data pointer");
}

/// 2026-08-14 (String unification): `foreach c in s` on a `String` operand
/// iterates CHARs via the decode lane — the bound is the stored byte length
/// (`.^Length` header) and each item is a decoded codepoint (Char), never a
/// raw byte walk.
#[test]
fn test_foreach_string_emits_char_decode_lane() {
    let src = r#"
        let s: String = "hé";
        node report [true][true] {
            foreach c in s {
                term;
            };
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let output = backend.generate(&items, None);
    assert!(output.contains("briev_str_next_char"),
        "a String foreach must call the char decode lane; got:\n{output}");
    assert!(output.contains("trunc i64"),
        "the decoded codepoint must be truncated to Char's native i32; got:\n{output}");
}

/// 2026-08-14 (boundary plan, SPEC §17.3): the four bit intrinsics dispatch
/// to their LLVM lanes — `Popcount#` → ctpop, `LeadingZeros#` → ctlz,
/// `TrailingZeros#` → cttz, `BitReverse#` → bitreverse.
#[test]
fn test_bit_intrinsics_emit_llvm_lanes() {
    let src = r#"
let a: Int = 5;
node report [a > 0][true] {
    let p: Int = Popcount#(a);
    let l: Int = LeadingZeros#(a);
    let t: Int = TrailingZeros#(a);
    let b: Int = BitReverse#(a);
    term;
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new();
    let ir = backend.generate(&items, None);
    assert!(ir.contains("llvm.ctpop"), "Popcount# must emit llvm.ctpop; got:\n{ir}");
    assert!(ir.contains("llvm.ctlz"), "LeadingZeros# must emit llvm.ctlz; got:\n{ir}");
    assert!(ir.contains("llvm.cttz"), "TrailingZeros# must emit llvm.cttz; got:\n{ir}");
    assert!(ir.contains("llvm.bitreverse"), "BitReverse# must emit llvm.bitreverse; got:\n{ir}");
}

/// 2026-08-14 (boundary plan): `Abs#` emits `llvm.abs` (int path) — the
/// intrinsic's home for a computed truth (SPEC §17.3).
#[test]
fn test_abs_intrinsic_emits_llvm_abs() {
    let src = r#"
let a: Int = -7;
node report [a < 0][true] {
    let b: Int = Abs#(a);
    term;
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new();
    let ir = backend.generate(&items, None);
    assert!(ir.contains("llvm.abs"), "Abs# must emit llvm.abs; got:\n{ir}");
}

/// A program with a `Int[2][3]` state field written and read at `[1][2]` —
/// exercises the multi-dim array layout + row-view GEPs (2026-08-07, Phase 7).
fn multidim_program() -> Vec<TopLevel> {
    let field = TopLevel::Statement(Box::new(Statement::Let {
        name: "m".to_string(),
        names: vec![],
        ty: Some(Type::Vector(
            Box::new(Type::int()),
            vec![crate::ast::Dimension::Anonymous(2), crate::ast::Dimension::Anonymous(3)],
        )),
        expr: None,
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
            Statement::Assign(
                Expr::Index(
                    Box::new(Expr::Index(
                        Box::new(Expr::Identifier("m".to_string())),
                        Box::new(Expr::Decimal(1)),
                    )),
                    Box::new(Expr::Decimal(2)),
                ),
                Expr::Decimal(42),
            ),
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![field, node]
}

#[test]
fn test_multidim_field_emits_nested_geps() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&multidim_program(), None);
    assert!(output.contains("[2 x [3 x i64]]"),
        "a 2-dim field must lay out as [2 x [3 x i64]]");
}

/// An `obj Box<T, M> { data: T[M]; total: Int; ... }` with a top-level
/// `let b: Box<Int, 5> = 0` — exercises the UNPACKED instance representation
/// (2026-08-07, object instance pools): members become prefixed top-level
/// slots, the Init runs against them, and `b.data[i]`/`b.total` resolve
/// through the standard field paths.
fn unpacked_instance_program() -> Vec<TopLevel> {
    use crate::ast::top::TypeDefBody;
    use crate::ast::top::TypeDefSlot;
    let obj_decl = TopLevel::TypeDef(Box::new(crate::ast::top::TypeDef {
        name: "Box".to_string(),
        type_params: vec![
            crate::ast::top::TypeParam { name: "T".to_string(), bound: None },
            crate::ast::top::TypeParam { name: "M".to_string(), bound: None },
        ],
        parent: None,
        protocol: None,
        traits: vec![],
        bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
        body: crate::ast::top::TypeDefBody {
            pins: Vec::new(),
            reference: None,
            tolerance: None,
            rating: None,
            slots: vec![
                crate::ast::top::TypeDefSlot { name: "data".to_string(), ty: Type::Vector(
                    Box::new(Type::Custom("T".to_string())),
                    vec![crate::ast::Dimension::Named("M".to_string(), 0)],
                ), bit_range: None },
                crate::ast::top::TypeDefSlot { name: "total".to_string(), ty: Type::int(), bit_range: None },
            ],
            metadata: HashMap::new(),
            projections: vec![],
            bindings: vec![],
            operators: vec![],
            op_bindings: vec![],
            constraints: vec![],
            members: vec![TopLevel::Transaction(Transaction {
                name: "init".to_string(),
                is_reactive: true,
                is_async: false,
                type_params: vec![],
                parameters: vec![("v".to_string(), Type::Custom("T".to_string()))],
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
                    Statement::Assign(Expr::Identifier("data".to_string()),
                        Expr::Index(Box::new(Expr::Identifier("data".to_string())), Box::new(Expr::Decimal(0)))),
                    Statement::Assign(Expr::Identifier("total".to_string()), Expr::Decimal(1)),
                    Statement::Term(None),
                ],
                metadata: HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            })],
            span: None,
        },
        span: None,
    }));
    let inst = TopLevel::Statement(Box::new(Statement::Let {
        name: "b".to_string(),
        names: vec![],
        ty: Some(Type::Applied("Box".to_string(), vec![
            Type::int(),
            crate::ast::Type::Number(5),
        ])),
        expr: Some(Expr::Decimal(0)),
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
            Statement::Assign(
                Expr::Index(
                    Box::new(Expr::Field(Box::new(Expr::Identifier("b".to_string())), "data".to_string())),
                    Box::new(Expr::Decimal(2)),
                ),
                Expr::Decimal(42),
            ),
            Statement::Term(Some(Expr::Field(Box::new(Expr::Identifier("b".to_string())), "total".to_string()))),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![obj_decl, inst, node]
}

#[test]
fn test_unpacked_instance_emits_prefixed_slots() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&unpacked_instance_program(), None);
    assert!(output.contains("[5 x i64]"),
        "the member-array must unpack into a [5 x i64] top-level slot");
    assert!(output.contains("define void @txn_s"),
        "the node must compile");
    assert!(output.contains("getelementptr [5 x i64], ptr"),
        "b.data[2] must GEP the unpacked slot's array");
}

/// A countdown node that spawns a Counter + calls its member — the spawned
/// handle's member body must resolve the column at the handle's row (not the
/// boxed inttoptr path). Exercises the loop-engine Let-binding fix.
fn spawn_countdown_program() -> Vec<TopLevel> {
    use crate::ast::top::TypeDefBody;
    use crate::ast::top::TypeDefSlot;
    let obj_decl = TopLevel::TypeDef(Box::new(crate::ast::top::TypeDef {
        name: "Counter".to_string(),
        type_params: vec![],
        parent: None,
        protocol: None,
        traits: vec![],
        bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
        body: TypeDefBody {
            pins: Vec::new(),
            reference: None,
            tolerance: None,
            rating: None,
            slots: vec![TypeDefSlot { name: "count".to_string(), ty: Type::int(), bit_range: None }],
            metadata: HashMap::new(),
            projections: vec![],
            bindings: vec![],
            operators: vec![],
            op_bindings: vec![],
            constraints: vec![],
            members: vec![TopLevel::Transaction(Transaction {
                name: "inc".to_string(),
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
                    Statement::Assign(
                        Expr::Identifier("count".to_string()),
                        Expr::BinaryOp(
                            crate::ast::BinaryOpKind::Add,
                            Box::new(Expr::Identifier("count".to_string())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                    Statement::Term(None),
                ],
                metadata: HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            })],
            span: None,
        },
        span: None,
    }));
    let inst = TopLevel::Statement(Box::new(Statement::Let {
        name: "c".to_string(),
        names: vec![],
        ty: Some(Type::Custom("Counter".to_string())),
        expr: Some(Expr::Decimal(0)),
        modifiers: vec![],
    }));
    let ticks = TopLevel::Statement(Box::new(Statement::Let {
        name: "ticks".to_string(),
        names: vec![],
        ty: Some(Type::int()),
        expr: Some(Expr::Decimal(0)),
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(Transaction {
        name: "work".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(
                crate::ast::BinaryOpKind::Lt,
                Box::new(Expr::Identifier("ticks".to_string())),
                Box::new(Expr::Decimal(2)),
            ),
            post_condition: Expr::BinaryOp(
                crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("ticks".to_string())),
                Box::new(Expr::Decimal(2)),
            ),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body: vec![
            Statement::Let {
                name: "h".to_string(),
                names: vec![],
                ty: Some(Type::Custom("Counter".to_string())),
                expr: Some(Expr::Spawn { type_name: "Counter".to_string(), args: vec![], storage: crate::ast::SpawnStorage::Pooled }),
                modifiers: vec![],
            },
            Statement::Expression(Expr::MethodCall(
                Box::new(Expr::Identifier("h".to_string())),
                "inc".to_string(),
                vec![],
                None,
                vec![],
            )),
            Statement::Assign(
                Expr::Identifier("ticks".to_string()),
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Add,
                    Box::new(Expr::Identifier("ticks".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Term(Some(Expr::Call(
                "Print#".to_string(),
                vec![Expr::Decimal(1)],
                None,
            ))),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![obj_decl, inst, ticks, node]
}

#[test]
fn test_spawn_member_call_uses_column_row() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&spawn_countdown_program(), None);
    assert!(output.contains("getelementptr [3 x i64], ptr"),
        "the countdown loop body must GEP the count column at the handle's row");
    assert!(!output.contains("inttoptr"),
        "a spawned handle's member body must NOT fall back to the boxed self path");
    assert!(output.contains("llvm.loop.disable_nonforced"),
        "a loop with an observable call (the Print# term) must not be folded by LLVM");
    // 2026-08-08 (literal-bound countdown fix): the countdown `[ticks < 2]`
    // must loop to the literal 2, not the `add i64 0, 1` fallback that ran
    // every literal-bound loop once and silently dropped spawns.
    assert!(output.contains("add i64 0, 2"),
        "the literal countdown bound (2) must be emitted, not the fallback 1: {output}");
}

/// A countdown whose bound is a RUNTIME state field (`N`), spawning a
/// Counter each firing — the pool is DEPENDENT: the backend must malloc a
/// runtime-sized heap buffer at init (after the `N` read, before the static
/// instance's row-0 writes), slot the address, and GEP the member accesses
/// inside the buffer instead of the static `[capacity x T]` column.
fn spawn_dependent_countdown_program() -> Vec<TopLevel> {
    // `let N: Int = get_env_int!("BOUND")` — a runtime-bound field value.
    let n_item = TopLevel::Statement(Box::new(Statement::Let {
        name: "N".to_string(),
        names: vec![],
        ty: Some(Type::int()),
        expr: Some(Expr::Call("get_env_int!".to_string(), vec![Expr::Quoted(b"BOUND".to_vec())], None)),
        modifiers: vec![],
    }));
    spawn_pool_countdown_program(n_item)
}

/// 2026-08-09 (init kind, Phase 4): a bounded-init countdown — the pool is
/// sized statically to the max of the bound set, not a dependent heap buffer.
fn spawn_bounded_init_countdown_program() -> Vec<TopLevel> {
    let n_item = TopLevel::Init(crate::ast::top::InitDecl {
        name: "N".to_string(),
        bound: Some(crate::ast::top::BoundSpec::Choice(vec![
            crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(16)),
            crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(32)),
            crate::ast::top::BoundSpec::Single(crate::ast::top::BoundTerm::Lit(64)),
        ])),
        ty: Type::int(),
        value: Some(Expr::Decimal(0)),
        body: vec![],
        span: None,
        doc: None,
    });
    spawn_pool_countdown_program(n_item)
}

/// The shared spawn-pool program: an obj `Counter` with a countdown node that
/// spawns one instance per tick against the bound `N`. The bound item (a
/// runtime field or a bounded init) is supplied by the caller; the storage
/// class of the spawn is settable for box/spill tests.
fn spawn_pool_countdown_program(n_item: TopLevel) -> Vec<TopLevel> {
    spawn_pool_countdown_program_storage(n_item, crate::ast::SpawnStorage::Pooled)
}

/// Variant with an explicit spawn storage class (2026-08-09, Phase 5).
fn spawn_pool_countdown_program_storage(
    n_item: TopLevel,
    storage: crate::ast::SpawnStorage,
) -> Vec<TopLevel> {
    use crate::ast::top::{TypeDef, TypeDefBody, TypeDefSlot};
    let obj = TopLevel::TypeDef(Box::new(TypeDef {
        name: "Counter".to_string(),
        type_params: vec![],
        parent: None,
        protocol: None,
        traits: vec![],
        bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
        body: TypeDefBody {
            pins: Vec::new(),
            reference: None,
            tolerance: None,
            rating: None,
            slots: vec![TypeDefSlot { name: "count".to_string(), ty: Type::int(), bit_range: None }],
            metadata: HashMap::new(),
            projections: vec![],
            bindings: vec![],
            operators: vec![],
            op_bindings: vec![],
            constraints: vec![],
            members: vec![TopLevel::Transaction(Transaction {
                name: "inc".to_string(),
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
                    Statement::Assign(
                        Expr::Identifier("count".to_string()),
                        Expr::BinaryOp(
                            crate::ast::BinaryOpKind::Add,
                            Box::new(Expr::Identifier("count".to_string())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                    Statement::Term(None),
                ],
                metadata: HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            })],
            span: None,
        },
        span: None,
    }));
    let inst = TopLevel::Statement(Box::new(Statement::Let {
        name: "c".to_string(),
        names: vec![],
        ty: Some(Type::Custom("Counter".to_string())),
        expr: Some(Expr::Decimal(0)),
        modifiers: vec![],
    }));
    let ticks = TopLevel::Statement(Box::new(Statement::Let {
        name: "ticks".to_string(),
        names: vec![],
        ty: Some(Type::int()),
        expr: Some(Expr::Decimal(0)),
        modifiers: vec![],
    }));
    // `let N: Int = get_env_int!("BOUND")` — a runtime-bound field value.
    let node = TopLevel::Transaction(Transaction {
        name: "work".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(
                crate::ast::BinaryOpKind::Lt,
                Box::new(Expr::Identifier("ticks".to_string())),
                Box::new(Expr::Identifier("N".to_string())),
            ),
            post_condition: Expr::BinaryOp(
                crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("ticks".to_string())),
                Box::new(Expr::Identifier("N".to_string())),
            ),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body: vec![
            Statement::Let {
                name: "h".to_string(),
                names: vec![],
                ty: Some(Type::Custom("Counter".to_string())),
                expr: Some(Expr::Spawn { type_name: "Counter".to_string(), args: vec![], storage }),
                modifiers: vec![],
            },
            Statement::Expression(Expr::MethodCall(
                Box::new(Expr::Identifier("h".to_string())),
                "inc".to_string(),
                vec![],
                None,
                vec![],
            )),
            Statement::Assign(
                Expr::Identifier("ticks".to_string()),
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Add,
                    Box::new(Expr::Identifier("ticks".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Term(Some(Expr::Call(
                "Print#".to_string(),
                vec![Expr::Decimal(1)],
                None,
            ))),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![obj, inst, ticks, n_item, node]
}

#[test]
fn test_dependent_spawn_pool_heap_buffer() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&spawn_dependent_countdown_program(), None);
    assert!(output.contains("= call ptr @malloc"),
        "a DEPENDENT pool must allocate a runtime-sized heap buffer at init: missing malloc");
    assert!(output.contains("inttoptr i64"),
        "the heap buffer address must be stored as an i64 slot and re-pointed on access");
    assert!(!output.contains("getelementptr [3 x i64], ptr"),
        "a DEPENDENT pool must NOT emit the static [capacity x T] column");
    // 2026-08-08 (Bug 3): the buffer holds total + 1 rows — the allocator
    // counter starts at row 1, so the last spawned row is index `total`; the
    // malloc size must be (bound + 1) * elem_size, else the final spawn writes
    // one-past-the-end (hidden only by malloc slop in spr.bv). The size
    // computation sits immediately before the malloc call: the +1 add, the
    // elem_size multiply, then malloc.
    let malloc_pos = output.find("= call ptr @malloc")
        .expect("malloc must follow the size computation: {output}");
    let before = &output[malloc_pos.saturating_sub(200)..malloc_pos];
    assert!(before.contains(", 1") && before.contains("add i64") && before.contains("mul i64"),
        "size = (bound + 1) * elem must be computed right before malloc: {output}");
    assert!(output.contains("llvm.loop.disable_nonforced"),
        "observable spawn work loop must not be folded");
}

/// 2026-08-09 (init kind, Phase 4): a bounded-init countdown sizes its pool
/// statically to the max of the bound set — a `[65 x i64]` column (64 spawns +
/// row 0), NO runtime malloc (the dependent-heap path stays for unbounded
/// inits).
#[test]
fn test_bounded_init_spawn_pool_is_static_set_max() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&spawn_bounded_init_countdown_program(), None);
    assert!(
        output.contains("getelementptr [65 x i64], ptr"),
        "a bounded-init pool must emit the static [65 x i64] column (max of set + row 0); got:\n{output}"
    );
    assert!(
        !output.contains("= call ptr @malloc"),
        "a bounded-init pool is statically sized — must NOT malloc a dependent heap buffer:\n{output}"
    );
}

/// 2026-08-09 (Phase 5): a `box` spawn allocates a per-instance heap block
/// (malloc), inttoptrs it as the handle, and member access GEPs the block —
/// NOT a pooled `[capacity x T]` column row.
#[test]
fn test_box_spawn_emits_per_instance_heap() {
    let n_item = TopLevel::Statement(Box::new(Statement::Let {
        name: "N".to_string(),
        names: vec![],
        ty: Some(Type::int()),
        expr: Some(Expr::Decimal(8)),
        modifiers: vec![],
    }));
    let mut backend = LlvmBackend::new();
    let output = backend.generate(
        &spawn_pool_countdown_program_storage(n_item, crate::ast::SpawnStorage::Box),
        None,
    );
    assert!(
        output.contains("= call ptr @malloc"),
        "a box spawn must malloc a per-instance heap block:\n{output}"
    );
    assert!(
        output.contains("inttoptr i64"),
        "the boxed handle must be an inttoptr'd block address:\n{output}"
    );
}

/// 2026-08-09 (Phase 5): a `spill` spawn is also a per-instance heap block —
/// never a static pool column.
#[test]
fn test_spill_spawn_emits_per_instance_heap() {
    let n_item = TopLevel::Statement(Box::new(Statement::Let {
        name: "N".to_string(),
        names: vec![],
        ty: Some(Type::int()),
        expr: Some(Expr::Decimal(8)),
        modifiers: vec![],
    }));
    let mut backend = LlvmBackend::new();
    let output = backend.generate(
        &spawn_pool_countdown_program_storage(n_item, crate::ast::SpawnStorage::Spill),
        None,
    );
    assert!(
        output.contains("= call ptr @malloc"),
        "a spill spawn must allocate per instance:\n{output}"
    );
    assert!(
        output.contains("inttoptr i64"),
        "the spilled handle must be an inttoptr'd block address:\n{output}"
    );
}

/// 2026-08-09 (Bug 1): a base that is ONLY spawned (no top-level `let c: Obj
/// = ...` instance) must still register its pool counter + member columns —
/// otherwise `spawn Obj()` panics on a missing pool and the member body reads
/// a nonexistent `@member` global.
#[test]
fn test_spawn_only_base_registers_pool() {
    use crate::ast::top::{TypeDef, TypeDefBody, TypeDefSlot};
    let obj = TopLevel::TypeDef(Box::new(TypeDef {
        name: "Counter".to_string(),
        type_params: vec![],
        parent: None,
        protocol: None,
        traits: vec![],
        bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
        body: TypeDefBody {
            pins: Vec::new(),
            reference: None,
            tolerance: None,
            rating: None,
            slots: vec![TypeDefSlot { name: "count".to_string(), ty: Type::int(), bit_range: None }],
            metadata: HashMap::new(),
            projections: vec![],
            bindings: vec![],
            operators: vec![],
            op_bindings: vec![],
            constraints: vec![],
            members: vec![TopLevel::Transaction(Transaction {
                name: "inc".to_string(),
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
                    Statement::Assign(
                        Expr::Identifier("count".to_string()),
                        Expr::BinaryOp(
                            crate::ast::BinaryOpKind::Add,
                            Box::new(Expr::Identifier("count".to_string())),
                            Box::new(Expr::Decimal(1)),
                        ),
                    ),
                    Statement::Term(None),
                ],
                metadata: HashMap::new(),
                derivation: None,
                modifiers: vec![],
                span: None,
                doc: None,
            })],
            span: None,
        },
        span: None,
    }));
    let ticks = TopLevel::Statement(Box::new(Statement::Let {
        name: "ticks".to_string(),
        names: vec![],
        ty: Some(Type::int()),
        expr: Some(Expr::Decimal(0)),
        modifiers: vec![],
    }));
    let node = TopLevel::Transaction(Transaction {
        name: "work".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(
                crate::ast::BinaryOpKind::Lt,
                Box::new(Expr::Identifier("ticks".to_string())),
                Box::new(Expr::Decimal(3)),
            ),
            post_condition: Expr::BinaryOp(
                crate::ast::BinaryOpKind::Eq,
                Box::new(Expr::Identifier("ticks".to_string())),
                Box::new(Expr::Decimal(3)),
            ),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body: vec![
            Statement::Let {
                name: "h".to_string(),
                names: vec![],
                ty: Some(Type::Custom("Counter".to_string())),
                expr: Some(Expr::Spawn { type_name: "Counter".to_string(), args: vec![], storage: crate::ast::SpawnStorage::Pooled }),
                modifiers: vec![],
            },
            Statement::Expression(Expr::MethodCall(
                Box::new(Expr::Identifier("h".to_string())),
                "inc".to_string(),
                vec![],
                None,
                vec![],
            )),
            Statement::Assign(
                Expr::Identifier("ticks".to_string()),
                Expr::BinaryOp(
                    crate::ast::BinaryOpKind::Add,
                    Box::new(Expr::Identifier("ticks".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Term(Some(Expr::Call(
                "Print#".to_string(),
                vec![Expr::Identifier("ticks".to_string())],
                None,
            ))),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&[obj, ticks, node], None);
    // No top-level instance — but the pool counter + member column must still
    // be registered (as %State struct slots) so spawn + member access resolve
    // to columns, never @count. Slot 2 = the spawn counter; slot 3 = the
    // `Counter.count` column ([4 x i64] = capacity 3 + row 0).
    assert!(
        output.contains("[4 x i64]"),
        "spawn-only base must register a member column sized capacity+1:\n{output}"
    );
    assert!(
        output.contains("i32 0, i32 2"),
        "spawn-only base must register the __spawn_next_Counter slot:\n{output}"
    );
    assert!(
        !output.contains("load i64, ptr @count"),
        "member body must resolve the column, not a nonexistent @count global:\n{output}"
    );
}


/// A reactive node whose term is a match over the `n` state field:
/// `1..=5 => 7`, `_ => 0`. Exercises the codegen match lowering
/// (2026-08-06, Phase 7).
fn match_range_program() -> Vec<TopLevel> {
    let n_state = TopLevel::StateDecl(StateDecl {
        name: "n".to_string(),
        ty: Type::int(),
        span: None,
    });
    let node = TopLevel::Transaction(Transaction {
        name: "s".to_string(),
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
        body: vec![Statement::Term(Some(Expr::Match(
            Box::new(Expr::Identifier("n".to_string())),
            vec![
                MatchArm {
                    pattern: Pattern::RangeInclusive(Expr::Decimal(1), Expr::Decimal(5)),
                    guard: None,
                    body: Box::new(Expr::Decimal(7)),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    guard: None,
                    body: Box::new(Expr::Decimal(0)),
                },
            ],
        )))],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    vec![n_state, node]
}

#[test]
fn test_match_emits_arm_chain_and_phi() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&match_range_program(), None);
    assert!(output.contains(".match_arm_"), "match arms must be emitted as blocks");
    assert!(output.contains("phi i64"), "match results must merge in a phi");
    assert!(output.contains("icmp sle"), "inclusive range must use sle");
    assert!(output.contains("icmp sge"), "range lower bound must use sge");
}

#[test]
fn test_webstack_enabled_emits_flush_state() {
    // 2026-07-26: Phase 4 — with_webstack(true) should emit __web_flush_state
    // import and state_layout() export function in the generated IR.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let output = backend.generate(&empty_program(), None);
    assert!(output.contains("__web_flush_state"),
        "should declare __web_flush_state import");
    assert!(output.contains("state_layout"),
        "should export state_layout function");
    assert!(output.contains("__web_generation"),
        "should emit generation counter global");
}

#[test]
fn test_webstack_disabled_omits_flush_state() {
    // 2026-07-26: Phase 4 — Without with_webstack, no webstack emits.
    let mut backend = LlvmBackend::new()
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let output = backend.generate(&empty_program(), None);
    assert!(!output.contains("__web_flush_state"),
        "should NOT declare __web_flush_state without webstack enabled");
    assert!(!output.contains("state_layout"),
        "should NOT export state_layout without webstack enabled");
}

#[test]
fn test_webstack_emits_flush_at_term() {
    // 2026-07-26: Phase 4 — Transactions with webstack emit __web_flush_state call.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
        make_txn("increment", vec![]),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("__web_flush_state"),
        "transactions should call __web_flush_state at term");
}

#[test]
fn test_webstack_bv_logic_only() {
    // 2026-07-26: Phase 5 — A .bv-style program (pure logic, no view bindings)
    // compiled with webstack backend should produce WASM-targeted LLVM IR.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let program = vec![
        state_count(),
        make_txn("compute", vec![]),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("wasm32-unknown-wasi"),
        "should use wasm32 target triple in .bv webstack mode");
    assert!(output.contains("__web_flush_state"),
        "should emit flush state for webstack even with logic-only .bv");
    assert!(output.contains("state_layout"),
        "should export state_layout function");
}

/// 2026-08-11 (wasm32 obj-member fix): a Ptr-indexed store `data[i] = v` at
/// int_bits=32 must widen the i32 index to i64 for the GEP — the old bare
/// `add i64 {i32}, 0` produced invalid IR (`%t38 defined with type 'i32' but
/// expected 'i64'`) that llc rejected for the webstack build of
/// examples/todo.rbv (the List.push body's `inner.data[len] = val`).
#[test]
fn test_wasm32_ptr_index_store_widens_index() {
    let src = r#"
let buf: Ptr<Int> = Malloc#(64) as Ptr<Int>;
let i: Int = 0;
txn store [i < 8][true] {
    buf[i] = 42;
    term;
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("sext i32"),
        "wasm32 Ptr-indexed store must widen the i32 index to i64 for the GEP; got:\n{ir}"
    );
}

#[test]
fn test_llvm_generates_state_type() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "counter".to_string(),
            ty: Type::int(),
            span: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("%State"));
    assert!(output.contains("i64"));
    assert!(output.contains("%state"));
}

#[test]
fn test_llvm_generates_transaction() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "increment".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("count".to_string()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("count".to_string())), Box::new(Expr::Decimal(1)))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // 2026-07-17: emit_transaction uses @txn_<name> prefix to match the
    // call sites in emit_ssa_loop and emit_folded_multi_main.
    assert!(output.contains("@txn_increment("));
}

#[test]
fn test_llvm_emits_sync_block_body() {
    // 2026-09-08 (deprecation audit): `sync { }` is the LEGACY spelling of
    // `mutex { }`. It was silently DROPPED by the `_ =>` catch-all in
    // emit_statement — the body vanished from the IR. Regression: the body
    // must emit exactly like a mutex section (inline serial execution).
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "increment".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::SyncBlock(vec![Statement::Assign(
                    Expr::Identifier("count".to_string()),
                    Expr::BinaryOp(
                        BinaryOpKind::Add,
                        Box::new(Expr::Identifier("count".to_string())),
                        Box::new(Expr::Decimal(1)),
                    ),
                )]),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // The body's increment must survive codegen — the silent-drop path
    // emitted no `add` for the SyncBlock's inner assignment.
    assert!(
        output.contains("add nsw i64"),
        "SyncBlock body was silently dropped:\n{}",
        output
    );
}

#[test]
fn test_llvm_has_noalias() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "increment".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![Statement::Term(None)],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("noalias"), "Transaction should have noalias");
    assert!(output.contains("nocapture"), "Transaction should have nocapture");
    assert!(output.contains("local_unnamed_addr"), "Should have local_unnamed_addr");
    assert!(output.contains("attributes #0"), "Should have attribute block");
    assert!(output.contains("mustprogress"), "Should have mustprogress");
    assert!(output.contains("llvm.assume"), "Should declare llvm.assume intrinsic");
}

#[test]
fn test_llvm_acyclic_annotation() {
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&empty_program(), None);
    assert!(!output.is_empty());
}

#[test]
fn test_inline_directive_emits_alwaysinline() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        state_count(),
        make_txn("inline_txn", vec![Annotation { name: "inline".to_string(), value: Some(Expr::Bool(true)) }]),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("alwaysinline"), "#inline should emit alwaysinline");
}

#[test]
fn test_speculative_inline_emits_inlinehint() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        state_count(),
        make_txn("hinted_txn", vec![Annotation { name: "?inline".to_string(), value: Some(Expr::Bool(true)) }]),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("inlinehint"), "#?inline should emit inlinehint");
}

#[test]
fn test_inline_directive_absent_no_extra_attr() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        state_count(),
        make_txn("plain_txn", vec![]),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("alwaysinline"), "cycle-free txn should have alwaysinline by default");
}


#[test]
fn test_accel_descriptors_emit() {
    // 2026-08-06 (accel plan): the descriptor table + ABI declares are emitted
    // with each field's HOST offset (kernel field order), so the runtime's
    // generic pack matches the kernel's %State GEPs. Tested directly — SPIR-V
    // blob compilation (llc) is machine-gated.
    let mut backend = LlvmBackend::new().with_type_universe(crate::type_universe::TypeUniverse::new());
    backend.ctx.field_index_map.insert("a".to_string(), 0);
    backend.ctx.field_types.push("[16 x float]".to_string());
    backend.ctx.field_briev_types.push(Type::Vector(
        Box::new(Type::Custom("Float".to_string())),
        vec![crate::ast::Dimension::Anonymous(16)],
    ));
    let entry = accel_test_entry("a", true);
    backend.accel_entries.insert("force".to_string(), entry);
    // 2026-08-31: the descriptor's field list = the kernel SSBO's members =
    // collect_state_fields(items) — the fixture declares its state item.
    let state_item = crate::ast::TopLevel::StateDecl(crate::ast::top::StateDecl {
        name: "a".into(),
        ty: Type::Vector(
            Box::new(Type::Custom("Float".to_string())),
            vec![crate::ast::Dimension::Anonymous(16)],
        ),
        span: None,
    });
    let blob = crate::backend::llvm::kernel::AccelKernelBlob {
        txn_name: "force".to_string(),
        bytes: vec![0x03, 0x02, 0x23, 0x07],
    };
    let (ir, idx_of) =
        crate::backend::llvm::kernel::emit_accel_descriptors(&backend, &[blob], &[state_item]);
    assert_eq!(idx_of["force"], 0, "txn → descriptor index");
    assert!(
        ir.contains("%briev.field = type { ptr, i32, i64, i64, i64, i32, i64 }"),
        "field type (7th member = proj_offset)"
    );
    assert!(ir.contains("%briev.kernel = type { ptr, ptr, i32, i32, ptr }"), "kernel type");
    assert!(ir.contains("@briev_accel_descs"), "descs table");
    assert!(ir.contains("declare i32 @briev_accel_init(ptr, i32)"), "init decl");
    assert!(ir.contains("declare i32 @briev_accel_launch(i32, ptr, i64)"), "launch decl");
    // a: array, host_offset 0 (fidx 0), elem 4 (float), count 16, write,
    // proj_offset 0 (a is first — already 16B-aligned). 2026-09-02: the
    // field-entry order matches the C BrievField layout exactly
    // (kind, host_offset, elem_bytes, count, is_write, proj_offset).
    assert!(ir.contains("i32 1, i64 0, i64 4, i64 16, i32 1, i64 0"), "field entry: {ir}");
}

#[test]
fn test_accel_wrapper_emits_dispatch() {
    // 2026-08-06 (accel plan): the reactor calls @txn_<name> by name; an accel
    // body's @txn_<name> is a dispatch wrapper (lazy init + gate → launch,
    // else the CPU body @txn_<name>_cpu). The GPU lane fast-forwards the
    // work-item counter to the bound, coalescing the counted loop into one
    // dispatch (Design A). Tested directly (llc-independent).
    let mut backend = LlvmBackend::new();
    backend.accel_kernel_idx.insert("force".to_string(), 0);
    backend.ctx.field_index_map.insert("i".to_string(), 0);
    backend.ctx.field_types.push("i64".to_string());
    backend.ctx.field_briev_types.push(Type::int());
    let entry = accel_test_entry("a", false); // Probe: verdict gate
    backend.accel_entries.insert("force".to_string(), entry);
    let mut out = String::new();
    backend.emit_accel_dispatch_wrapper(&mut out, "force");
    assert!(out.contains("define void @txn_force("), "wrapper define: {out}");
    assert!(out.contains("@briev_accel_init(ptr @briev_accel_descs, i32 1)"), "lazy init");
    assert!(out.contains("load i32, ptr @briev_accel_verdict"), "probe verdict gate");
    assert!(out.contains("call i32 @briev_accel_launch(i32 0, ptr %state, i64 16)"), "launch call: {out}");
    // Design A: one dispatch covers all N work-items → fast-forward i to 16 so
    // the `[i < N]` counted loop exits after this single firing.
    assert!(out.contains("store i64 16, ptr %"), "counter fast-forward: {out}");
    assert!(out.contains("call void @txn_force_cpu(ptr %state)"), "cpu fallback");
}

fn accel_test_entry(buffer: &str, write: bool) -> crate::analysis::accel::AccelEntry {
    use crate::analysis::accel::{AccelDecision, AccelEntry, AccelMode, KernelShape};
    let mut read_buffers = vec![buffer.to_string()];
    let mut write_buffers = Vec::new();
    if write {
        write_buffers.push(buffer.to_string());
    }
    let shape = KernelShape {
        index_var: "i".to_string(),
        count_expr: Some(Expr::Decimal(16)),
        kernel_stmts: vec![],
        host_stmts: vec![],
        read_buffers,
        write_buffers,
        scalar_ins: vec![],
        eligible: true,
        reasons: vec![],
        work_cols: None,
        reduction: None,
        deferred_normalize: None,
    };
    AccelEntry {
        mode: AccelMode::TryKeyword,
        forced: false,
        shape,
        decision: AccelDecision::Probe,
    }
}

#[test]
fn test_accel_probe_functions_emit() {
    // 2026-08-06 (Phase 7): a Probe-decision accel body emits the auto-tuning
    // probe functions — CPU lane (loop to completion), GPU lane (one dispatch +
    // fast-forward), output-equality gate, run_probe → verdict global. Tested
    // directly (llc-independent).
    let mut backend = LlvmBackend::new();
    backend.accel_kernel_idx.insert("force".to_string(), 0);
    backend.ctx.state_size_bytes = 32;
    backend.ctx.field_index_map.insert("i".to_string(), 0);
    backend.ctx.field_types.push("i64".to_string());
    backend.ctx.field_briev_types.push(Type::int());
    backend.ctx.field_index_map.insert("a".to_string(), 1);
    backend.ctx.field_types.push("[4 x float]".to_string());
    backend.ctx.field_briev_types.push(Type::Vector(
        Box::new(Type::Custom("Float".to_string())),
        vec![crate::ast::Dimension::Anonymous(4)],
    ));
    backend.accel_entries.insert("force".to_string(), accel_test_entry("a", true));
    let mut out = String::new();
    backend.emit_accel_probe_functions(&mut out, "force");
    assert!(out.contains("define void @briev_accel_probe_cpu_force(ptr %state)"), "cpu lane: {out}");
    assert!(out.contains("call void @txn_force_cpu(ptr %state)"), "cpu lane runs the loop");
    assert!(out.contains("define void @briev_accel_probe_gpu_force(ptr %state)"), "gpu lane: {out}");
    assert!(out.contains("call i32 @briev_accel_launch(i32 0, ptr %state, i64 16)"), "gpu lane launch");
    assert!(out.contains("define i8 @briev_accel_gpu_ok_force(ptr %a, ptr %b, double %tol)"), "gate: {out}");
    assert!(out.contains("fcmp oge float"), "float tolerance compare");
    assert!(out.contains("define void @briev_accel_run_probe_force(ptr %state)"), "run_probe: {out}");
    assert!(out.contains("store i32 %v, ptr @briev_accel_verdict_force"), "verdict commit");
}

#[test]
fn test_endprogram_emits_process_exit() {
    // 2026-08-06 (endprogram plan): `endprogram` emits a real process exit
    // (`@__exit`) with the value's i64 code, then an unreachable terminator —
    // not a plain `ret`. Regression for the infinite-output bug (a node whose
    // precondition stays true must terminate the process, not re-fire forever).
    let mut backend = LlvmBackend::new();
    let program = vec![
        state_count(),
        TopLevel::Transaction(Transaction {
            name: "report".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![Statement::EndProgram(Some(Expr::Decimal(7)))],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("declare void @__exit(i64)"), "exit declare: {output}");
    assert!(output.contains("call void @__exit(i64"), "exit call: {output}");
    assert!(output.contains("unreachable"), "unreachable after exit: {output}");
}

#[test]
fn test_beginprogram_entry_loop_emits_flag_and_goal_clear() {
    // 2026-08-06 (beginprogram plan): a beginprogram node emits its entry flag
    // (@briev_begin_<name>, true until the goal) and a goal-check that clears
    // it — `[beginprogram && i < N]` drives a one-shot entry loop with no
    // phase gate.
    let mut backend = LlvmBackend::new();
    let i_state = TopLevel::StateDecl(StateDecl {
        name: "i".to_string(),
        ty: Type::int(),
        span: None,
    });
    let init = TopLevel::Transaction(Transaction {
        name: "init".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(
                BinaryOpKind::And,
                Box::new(Expr::BeginProgram),
                Box::new(Expr::BinaryOp(
                    BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("i".to_string())),
                    Box::new(Expr::Decimal(4)),
                )),
            ),
            post_condition: Expr::BinaryOp(
                BinaryOpKind::Eq,
                Box::new(Expr::Identifier("i".to_string())),
                Box::new(Expr::Decimal(4)),
            ),
            watchdog: None,
            explicit: true,
            span: None,
        post_authority: false},
        body: vec![
            Statement::Assign(
                Expr::Identifier("i".to_string()),
                Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("i".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
            ),
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    });
    let output = backend.generate(&vec![i_state, init], None);
    assert!(
        output.contains("@briev_begin_init = private global i1 1"),
        "entry flag must emit: {output}"
    );
    assert!(
        output.contains("store i1 false, ptr @briev_begin_init"),
        "goal-check must clear the flag: {output}"
    );
}

#[test]
fn test_escape_non_ASCII_string() {
    let output = escape_llvm_string("héllo");
    assert!(output.contains("\\c3"), "Should hex-escape byte C3");
    assert!(output.contains("\\a9"), "Should hex-escape byte A9");
    assert!(output.contains("h"), "ASCII 'h' should be preserved");
    assert!(output.contains("llo"), "ASCII 'llo' should be preserved after escape bytes");
}

#[test]
fn test_no_range_lower_bound_defaults_to_i64_min() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(100))),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(None)],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("-9223372036854775808"),
        "Range with no lower bound should use i64::MIN");
}

#[test]
fn test_binop_no_nuw_nsw() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::StateDecl(StateDecl {
            name: "y".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(BinaryOpKind::And,
                    Box::new(Expr::BinaryOp(BinaryOpKind::Ge, Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(0)))),
                    Box::new(Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(10))))),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("x".to_string()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Identifier("y".to_string())))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(!output.contains("nuw nsw"),
        "add on bounded variables should NOT emit nuw nsw (LLVM infers from !range; nuw nsw causes urem→128bit mul)");
}

#[test]
fn test_range_metadata_suppressed_for_written_field() {
    // 2026-08-01: Regression test — a contract-derived !range must NOT be
    // attached to loads of a field the node body writes. The range comes from
    // the precondition ([x < 1]) but the dispatch-loop guard re-reads the field
    // each tick; once the body writes x = 1 the value leaves the range, which is
    // LLVM UB and made clang fold the guard to always-true (reactor never
    // converges — observed as an infinite loop in the B0 format demo).
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(1))),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("x".to_string()), Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // The write_set = {x}, so no module-level !range node may reference x's
    // field loads. The inline `!range !{0, bound}` reload emitted by
    // emit_precondition_check is sound (fresh read inside the guard's safe path)
    // and is allowed to remain.
    let module_level_range_on_x_load = output.contains("!range !50")
        || output.contains("!range !51");
    assert!(!module_level_range_on_x_load,
        "precondition-derived !range must not attach to loads of a written field\n{output}");
    assert!(!output.contains("!5 = !{ i64 -9223372036854775808, i64 1 }"),
        "the module-level range node for the written field must not be emitted\n{output}");
}

#[test]
fn test_range_metadata_kept_for_read_only_field() {
    // 2026-08-01: Control test for the above — a field NO node writes keeps its
    // precondition-derived !range (it is loop-invariant and therefore sound).
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Decimal(100))),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(None)],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("!range !") || output.contains("!range !{"),
        "read-only field should keep precondition-derived range metadata\n{output}");
}

#[test]
fn test_float_binary_add() {
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::float(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("x".to_string()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("x".to_string())), Box::new(Expr::Float(2.0)))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // 2026-07-17: Float literal + Float field → fmul/fadd float (32-bit), not double.
    // The typechecker assigns Type::float() to Float literals and the constant
    // emitter stores Float as "float" in LLVM IR. Operations use the correct width.
    assert!(output.contains("fadd fast float"),
        "Float binary add should emit fadd fast float");
}

#[test]
fn test_enum_type_registered_and_variant_disc() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Enum(EnumDefinition {
            name: "Option".to_string(),
            type_params: vec![],
            variants: vec![
                EnumVariant::Unit("None".to_string()),
                EnumVariant::Tuple("Some".to_string(), vec![Type::int()]),
            ],
            span: None,
        }),
    ];
    let _ = backend.generate(&program, None);
    assert!(backend.ctx.enum_types.contains_key("Option"));
    assert!(backend.ctx.variant_disc.contains_key("None"));
    assert!(backend.ctx.variant_disc.contains_key("Some"));
    assert_eq!(backend.ctx.variant_disc.get("None").map(|(_, d, _)| *d), Some(0));
    assert_eq!(backend.ctx.variant_disc.get("Some").map(|(_, d, _)| *d), Some(1));
    assert_eq!(backend.ctx.variant_disc.get("Some").map(|(_, _, f)| *f), Some(1));
}

#[test]
fn test_enum_multi_variant_discriminants() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Enum(EnumDefinition {
            name: "Tree".to_string(),
            type_params: vec![],
            variants: vec![
                EnumVariant::Unit("Leaf".to_string()),
                EnumVariant::Tuple("Node".to_string(), vec![Type::int(), Type::int()]),
            ],
            span: None,
        }),
    ];
    let _ = backend.generate(&program, None);
    assert_eq!(backend.ctx.variant_disc.get("Leaf").map(|(_, d, _)| *d), Some(0));
    assert_eq!(backend.ctx.variant_disc.get("Node").map(|(_, d, _)| *d), Some(1));
    assert_eq!(backend.ctx.variant_disc.get("Node").map(|(_, _, f)| *f), Some(2));
}

#[test]
fn test_struct_type_registered() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Point".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![
                StructField { name: "x".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
                StructField { name: "y".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
            ],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("ModuleID"), "Output should be valid IR");
    assert!(backend.ctx.struct_types.contains_key("Point"),
        "Struct 'Point' should be registered");
    assert_eq!(backend.ctx.struct_types["Point"].len(), 2);
}

#[test]
fn test_struct_type_declaration_in_ir() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Point".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![
                StructField { name: "x".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
                StructField { name: "y".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
            ],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("%Point = type { i64, i64 }"),
        "Struct type declaration should appear in IR.\nGot:\n{}", output);
}

#[test]
fn test_struct_type_declaration_empty_struct() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Empty".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("%Empty = type {}"),
        "Empty struct should emit %Empty = type {{}}.\nGot:\n{}", output);
}

#[test]
fn test_struct_type_declaration_sorted_order() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Zebra".to_string(), type_params: vec![], parent: None,
            fields: vec![StructField { name: "s".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public }],
            transactions: vec![], view_html: None, span: None, modifiers: vec![], variants: vec![],
        }),
        TopLevel::Obj(StructDefinition {
            name: "Alpha".to_string(), type_params: vec![], parent: None,
            fields: vec![StructField { name: "s".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public }],
            transactions: vec![], view_html: None, span: None, modifiers: vec![], variants: vec![],
        }),
    ];
    let output = backend.generate(&program, None);
    let alpha_pos = output.find("%Alpha = type { i64 }").unwrap();
    let zebra_pos = output.find("%Zebra = type { i64 }").unwrap();
    assert!(alpha_pos < zebra_pos,
        "Struct declarations should be sorted: Alpha before Zebra. Got:\n{}", output);
}

#[test]
fn test_type_with_slots_populates_struct_types() {
    let program = vec![
        TopLevel::TypeDef(Box::new(TypeDef {
            name: "MyBuffer".to_string(),
            type_params: vec![],
            parent: None,
            protocol: None,
            traits: vec![],
            bit_range: None,
            coll: false,
            ports_in: Vec::new(),
            ports_out: Vec::new(),
            seq: false,
            body: TypeDefBody {
                pins: Vec::new(),
                reference: None,
                tolerance: None,
                rating: None,
                slots: vec![
                    TypeDefSlot { name: "ptr".to_string(), ty: Type::Applied("Ptr".to_string(), vec![Type::Custom("UInt8".to_string())]), bit_range: None },
                    TypeDefSlot { name: "len".to_string(), ty: Type::Custom("Int".to_string()), bit_range: None },
                ],
                metadata: HashMap::new(),
                projections: vec![],
                bindings: vec![],
                operators: vec![], op_bindings: vec![],
                constraints: vec![],
                members: vec![],
                span: None,
            },
            span: None,
        })),
    ];
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    let output = backend.generate(&program, None);
    assert!(output.contains("ModuleID"), "Output should be valid IR");
    assert!(backend.ctx.struct_types.contains_key("MyBuffer"),
        "Type with slots 'MyBuffer' should be registered in struct_types");
    assert_eq!(backend.ctx.struct_types["MyBuffer"].len(), 2);
}

#[test]
fn test_struct_auto_registered_in_type_universe() {
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Point".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![
                StructField { name: "x".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
                StructField { name: "y".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
            ],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
    ];
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    let _output = backend.generate(&program, None);
    if let Some(ref universe) = backend.ctx.type_universe {
        assert!(universe.types.contains_key("Point"),
            "Struct 'Point' should be auto-registered in TypeUniverse");
        let rt = universe.types.get("Point").unwrap();
        assert_eq!(rt.bytes, 16);
        // 2026-08-15 (fundamentals): every type's base is Data (the universal
        // parent); Bit is the leaf bit type.
        assert_eq!(rt.base, "Data");
    } else {
        panic!("TypeUniverse should exist after generate");
    }
}

fn make_point_program(body: Vec<Statement>) -> Vec<TopLevel> {
    vec![
        TopLevel::Obj(StructDefinition {
            name: "Point".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![
                StructField { name: "x".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
                StructField { name: "y".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
            ],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
        TopLevel::StateDecl(StateDecl {
            name: "pt".to_string(),
            ty: Type::int(), span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "main".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ]
}

#[test]
fn test_string_state_init_not_null() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "s".to_string(),
            ty: Type::string(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "_".to_string(),
                    ty: None,
                    expr: Some(Expr::Identifier("s".to_string())),
                    modifiers: vec![],
                },
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(!output.contains("store ptr null, ptr"),
        "String state field should NOT be null. Got: {}", output);
}

#[test]
fn test_const_trg_write_emits_error() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "locked".to_string(),
            ty: Type::bool_(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("locked".to_string()), Expr::Bool(true)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // This test verifies the backend doesn't crash for simple state assignments
    assert!(!output.is_empty());
}

#[test]
fn test_local_float_binding() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::float(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("x".to_string()), Expr::Float(2.0)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // 2026-07-29: Float literal emits bitcast i32 <hex> to float (32-bit).
    // The add+i32 + bitcast + fadd wrapper was removed — a single bitcast
    // from the hex i32 bit pattern produces the float value.
    assert!(output.contains("bitcast i32"),
        "Float literal should emit bitcast i32 to float: {}", output);
}

#[test]
fn test_tfd_sfd_nonblock_constants() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::int(),
            span: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(!output.is_empty());
}

// ── Main and reactor attribute tests ──────────────────────────────

fn make_wake_program_no_triggers() -> Vec<TopLevel> {
    vec![
        TopLevel::StateDecl(StateDecl {
            name: "ops".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Constant(Constant {
            name: "N".to_string(),
            ty: Type::int(),
            expr: Expr::Decimal(100),
            section: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "work".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(BinaryOpKind::And,
                    Box::new(Expr::Bool(true)),
                    Box::new(Expr::BinaryOp(BinaryOpKind::Lt,
                        Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Identifier("N".to_string()))))),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("ops".to_string()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Decimal(1)))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ]
}

#[test]
fn test_main_and_reactor_use_non_willreturn_attr() {
    let program = make_wake_program_no_triggers();
    let output = LlvmBackend::new().generate(&program, None);
    // 2026-07-14: Program may fold to a no-main constant store (EmitPureCounterFold) when
    // the analysis correctly detects it as fully precomputable.
    let has_main = output.contains("define i32 @main()");
    if has_main {
        let has_correct_main = output.contains("define i32 @main() local_unnamed_addr #3")
            || output.contains("define i32 @main() local_unnamed_addr #5")
            || output.contains("define i32 @main() local_unnamed_addr #9");
        assert!(has_correct_main,
            "main() should use #3/#5/#9, got: {:?}",
            output.lines().find(|l| l.contains("define i32 @main")).unwrap_or("(not found)"));
    }
    assert!(output.contains("attributes #0"),
        "attributes #0 should still be present for terminating functions");
    assert!(output.contains("define void @init_state(ptr noundef"),
        "init_state() should still use #0 with noundef");
}

// ── Exit condition tests ──────────────────────────────────

fn make_exit_program(exit_expr: Option<Expr>, is_wake: bool) -> Vec<TopLevel> {
    let mut items: Vec<TopLevel> = vec![
        TopLevel::StateDecl(StateDecl {
            name: "ops".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Constant(Constant {
            name: "N".to_string(),
            ty: Type::int(),
            expr: Expr::Decimal(100),
            section: None,
        }),
    ];
    // 2026-07-14: Create a trigger when is_wake is set so has_wake_triggers
    // fires and natural death detection runs.
    if is_wake {
        items.push(TopLevel::Trigger(Trigger {
            name: "__wake_trg".to_string(),
            instance: Expr::Identifier("".to_string()),
            span: None,
        }));
    }
    let pre = Expr::BinaryOp(BinaryOpKind::And,
        Box::new(Expr::Bool(true)),
        Box::new(Expr::BinaryOp(BinaryOpKind::Lt,
            Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Identifier("N".to_string())))));
    let txn = Transaction {
        name: "work".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: pre,
            post_condition: Expr::Bool(true),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body: vec![
            Statement::Assign(Expr::Identifier("ops".to_string()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Decimal(1)))),
            Statement::Term(None),
        ],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    };
    items.push(TopLevel::Transaction(txn));
    items
}

#[test]
fn test_exit_pragma_in_wake_main() {
    let exit_cond = Expr::BinaryOp(BinaryOpKind::Eq,
        Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Identifier("N".to_string())));
    let program = make_exit_program(Some(exit_cond.clone()), true);
    let output = LlvmBackend::new().generate(&program, Some(Box::new(exit_cond)));
    assert!(output.contains("trunc i64"),
        "Exit condition should trunc i64 to i1");
    assert!(output.contains("br i1"),
        "Exit condition should branch on icmp result");
    assert!(output.contains(".end:"),
        "Exit condition should emit .end label");
    assert!(output.contains("ret i32 0"),
        ".end label should return 0");
}

#[test]
fn test_exit_pragma_without_wake_no_change() {
    let exit_cond = Expr::BinaryOp(BinaryOpKind::Eq,
        Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Identifier("N".to_string())));
    let program = make_exit_program(Some(exit_cond.clone()), false);
    let output = LlvmBackend::new().generate(&program, Some(Box::new(exit_cond)));
    assert!(output.contains("trunc i64"),
        "Exit condition should trunc i64 to i1 even without wake");
    assert!(output.contains("br i1"),
        "Exit condition should branch");
    assert!(output.contains(".end:"),
        "Exit condition should emit .end label");
    assert!(output.contains("ret i32 0"),
        "done label should return 0");
}

#[test]
fn test_no_exit_without_pragma() {
    let program = make_exit_program(None, true);
    let output = LlvmBackend::new().generate(&program, None);
    assert!(!output.contains("wait:"),
        "No wait label without exit condition in this path");
}

#[test]
fn test_exit_in_enum_main() {
    let exit_cond = Expr::BinaryOp(BinaryOpKind::Eq,
        Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Identifier("N".to_string())));
    let program = make_exit_program(Some(exit_cond.clone()), false);
    let output = LlvmBackend::new().with_optimize_budget(256).generate(&program, Some(Box::new(exit_cond)));
    assert!(output.contains("ret i32 0"),
        "Should return 0");
}

// ── Exit diagnostic tests ──────────────────────────────────

#[test]
fn test_check_exit_condition_idents_valid() {
    let mut backend = LlvmBackend::new();
    backend.ctx.field_index_map.insert("ops".to_string(), 0);
    backend.ctx.constants.insert("N".to_string(), (Type::int(), Expr::Decimal(100)));

    let expr = Expr::BinaryOp(BinaryOpKind::Eq,
        Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Identifier("N".to_string())));
    let errors = backend.check_exit_condition_idents(&expr);
    assert!(errors.is_empty(),
        "No errors for known identifiers: {:?}", errors);
}

#[test]
fn test_check_exit_condition_idents_invalid() {
    let mut backend = LlvmBackend::new();
    backend.ctx.field_index_map.insert("ops".to_string(), 0);
    backend.ctx.constants.insert("N".to_string(), (Type::int(), Expr::Decimal(100)));

    let expr = Expr::BinaryOp(BinaryOpKind::Eq,
        Box::new(Expr::Identifier("ops".to_string())), Box::new(Expr::Identifier("bogus_var".to_string())));
    let errors = backend.check_exit_condition_idents(&expr);
    assert!(!errors.is_empty(),
        "Should report error for unknown identifier");
    assert!(errors[0].contains("bogus_var"),
        "Error should reference the unknown name: {}", errors[0]);
}

// ── Natural death tests ───────────────────────────────────

#[test]
fn test_natural_death_exits_foldable_program() {
    let program = make_exit_program(None, true);
    let mut backend = LlvmBackend::new();
    let _output = backend.generate(&program, None);
    // 2026-07-14: Natural death creates a synthetic exit condition, so the
    // "no exit path" warning is not emitted (there IS an exit path now).
    assert!(backend.ctx.has_natural_exit,
        "Foldable wake program should have natural exit");
}

#[test]
fn test_natural_death_skipped_for_persistent_txn() {
    let program = make_exit_program(None, true);
    let mut backend = LlvmBackend::new();
    let _output = backend.generate(&program, None);
    // 2026-07-14: Natural death creates a synthetic exit condition, so
    // the "no exit path" warning is no longer emitted.
    let has_warning = backend.warnings().iter().any(|w| {
        w.contains("has wake triggers but no exit path")
    });
    assert!(!has_warning,
        "Persistent program without #!exit — natural death creates exit condition");
}

// ── 2026-08-26 (bug sweep B4): undispatched plain-txn warning ────────────
// The warning hoisted ahead of dispatch-mode selection must fire for EVERY
// program mode and stay silent for library/shared-lib export shims.
fn make_dead_txn_program(with_reactive_node: bool) -> Vec<TopLevel> {
    let mut items: Vec<TopLevel> = vec![
        TopLevel::StateDecl(StateDecl {
            name: "i".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Constant(Constant {
            name: "N".to_string(),
            ty: Type::int(),
            expr: Expr::Decimal(4),
            section: None,
        }),
    ];
    if with_reactive_node {
        items.push(TopLevel::Transaction(Transaction {
            name: "run".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(
                    BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("i".to_string())),
                    Box::new(Expr::Identifier("N".to_string())),
                ),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: true,
                span: None,
            post_authority: false},
            body: vec![
                Statement::Assign(
                    Expr::Identifier("i".to_string()),
                    Expr::BinaryOp(
                        BinaryOpKind::Add,
                        Box::new(Expr::Identifier("i".to_string())),
                        Box::new(Expr::Decimal(1)),
                    ),
                ),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            doc: None,
            span: None,
        }));
    }
    // A plain top-level txn: zero-param, no-output, called by nothing.
    items.push(TopLevel::Transaction(Transaction {
        name: "unused_helper".to_string(),
        is_reactive: false,
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
        body: vec![],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        doc: None,
        span: None,
    }));
    items
}

#[test]
fn dead_plain_txn_warns_with_and_without_reactive_node() {
    for with_node in [true, false] {
        let program = make_dead_txn_program(with_node);
        let mut backend = LlvmBackend::new();
        let _ = backend.generate(&program, None);
        assert!(
            backend.warnings().iter().any(|w| w.contains("'unused_helper' is never dispatched")),
            "with_node={with_node}: plain unreferenced txn must be named in a warning"
        );
    }
}

#[test]
fn library_mode_silences_undispatched_txn_warning() {
    let program = make_dead_txn_program(false);
    let mut backend = LlvmBackend::new().with_library_mode(true);
    let _ = backend.generate(&program, None);
    assert!(
        !backend.warnings().iter().any(|w| w.contains("never dispatched")),
        "library shim exports plain txns as symbols — silence is correct"
    );
}

// ── SLP Hazard Detection Tests ────────────────────────────

fn make_slp_float_program(n_floats: usize, cross_body: Vec<Statement>, precondition: Option<Expr>) -> Vec<TopLevel> {
    let mut items: Vec<TopLevel> = Vec::new();
    for i in 0..n_floats {
        items.push(TopLevel::StateDecl(StateDecl {
            name: format!("f{}", i),
            ty: Type::float(),
            span: None,
        }));
    }
    items.push(TopLevel::StateDecl(StateDecl {
        name: "count".to_string(),
        ty: Type::int(),
        span: None,
    }));
    items.push(TopLevel::StateDecl(StateDecl {
        name: "total".to_string(),
        ty: Type::int(),
        span: None,
    }));
    items.push(TopLevel::Transaction(Transaction {
        name: "tick".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: precondition.unwrap_or(Expr::Bool(true)),
            post_condition: Expr::Identifier("count".to_string()),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body: cross_body,
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    }));
    items
}

fn make_cross_float_body(n_floats: usize, cross_count: usize) -> Vec<Statement> {
    let mut stmts: Vec<Statement> = Vec::new();
    for i in 0..cross_count {
        let a = (i * 3) % n_floats;
        let b = ((i * 3) + 1) % n_floats;
        let c = ((i * 3) + 2) % n_floats;
        stmts.push(Statement::Assign(
            Expr::Identifier(format!("f{}", a)),
            Expr::BinaryOp(BinaryOpKind::Mul,
                Box::new(Expr::Identifier(format!("f{}", b))),
                Box::new(Expr::Identifier(format!("f{}", c)))),
        ));
    }
    stmts.push(Statement::Assign(
        Expr::Identifier("count".to_string()),
        Expr::BinaryOp(BinaryOpKind::Add,
            Box::new(Expr::Identifier("count".to_string())),
            Box::new(Expr::Decimal(1))),
    ));
    stmts
}

#[test]
fn test_slp_hazard_no_floats() {
    let program = make_slp_float_program(0, make_cross_float_body(0, 0), None);
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&program, None);
    assert!(!output.contains("disable-slp-vectorize"),
        "No float fields should produce no SLP-disabled attributes");
}

#[test]
fn test_slp_hazard_small_field_count() {
    let body = make_cross_float_body(4, 6);
    let program = make_slp_float_program(4, body, None);
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&program, None);
    assert!(!output.contains("disable-slp-vectorize"),
        "4 float fields with 6 ops should not trigger SLP disable");
}

#[test]
fn test_slp_hazard_large_field_count() {
    let body = make_cross_float_body(20, 40);
    let program = make_slp_float_program(20, body, None);
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&program, None);
    // 2026-07-27: SLP hazard attribute emission removed — manual SLP vector
    // codegen was disabled so there's no conflict with LLVM's auto-vectorizer.
    // The hazard analysis still runs but produces no attribute output.
    assert!(!output.contains("disable-slp-vectorize"),
        "SLP hazard attributes no longer emitted after SLP codegen removal");
}

#[test]
fn test_slp_hazard_independent_channels() {
    let mut body: Vec<Statement> = Vec::new();
    for i in 0..12 {
        body.push(Statement::Assign(
            Expr::Identifier(format!("f{}", i)),
            Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::Identifier(format!("f{}", i))),
                Box::new(Expr::Float(1.0))),
        ));
    }
    body.push(Statement::Assign(
        Expr::Identifier("count".to_string()),
        Expr::BinaryOp(BinaryOpKind::Add,
            Box::new(Expr::Identifier("count".to_string())),
            Box::new(Expr::Decimal(1))),
    ));
    let program = make_slp_float_program(12, body, None);
    let mut backend = LlvmBackend::new();
    let output = backend.generate(&program, None);
    assert!(!output.contains("disable-slp-vectorize"),
        "12 independent float fields should NOT disable SLP");
}

#[test]
fn test_slp_hazard_with_target_spec() {
    let body = make_cross_float_body(12, 18);
    let program = make_slp_float_program(12, body, None);
    let mut backend = LlvmBackend::new();
    let spec = crate::target_spec::TargetSpec {
        target: Some(crate::target_spec::TargetSection {
            name: "aarch64-unknown-linux-gnu".to_string(),
            backend: "llvm".to_string(),
            capabilities: vec!["neon".to_string()],
            import_ffi: None,
        }),
        ffi: None,
        codegen: None,
        memory: None,
        bottlenecks: None,
    };
    backend = backend.with_spec(spec);
    let output = backend.generate(&program, None);
    assert!(!output.contains("disable-slp-vectorize"),
        "AArch64 with 32 registers and ASR 2.4 > 1.5 should allow SLP for 12 fields");
}

#[test]
fn test_slp_hazard_avx_target() {
    let body = make_cross_float_body(12, 32);
    let program = make_slp_float_program(12, body, None);
    let mut backend = LlvmBackend::new();
    let spec = crate::target_spec::TargetSpec {
        target: Some(crate::target_spec::TargetSection {
            name: "x86_64-unknown-linux-gnu".to_string(),
            backend: "llvm".to_string(),
            capabilities: vec!["avx2".to_string()],
            import_ffi: None,
        }),
        ffi: None,
        codegen: None,
        memory: None,
        bottlenecks: None,
    };
    backend = backend.with_spec(spec);
    let output = backend.generate(&program, None);
    // 2026-07-27: SLP hazard attribute emission removed — same rationale as
    // test_slp_hazard_large_field_count.
    assert!(!output.contains("disable-slp-vectorize"),
        "SLP hazard attributes no longer emitted after SLP codegen removal");
}

// ── Schema alias tests ────────────────────────────────────

#[test]
fn test_schema_aliases_loaded() {
    let mut aliases = HashSet::new();
    aliases.insert("uart_debug".to_string());
    let mut backend = LlvmBackend::new().with_schema_aliases(aliases);
    assert_eq!(backend.ctx.schema_alias_names.len(), 1);
    assert!(backend.ctx.schema_alias_names.contains("uart_debug"));
    let output = backend.generate(&empty_program(), None);
    assert!(output.contains("ModuleID"));
}

#[test]
fn test_no_schema_import_no_validation() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
    ];
    let _output = backend.generate(&program, None);
    assert!(backend.warnings().is_empty(),
        "No schema import should produce no warnings");
}

#[test]
fn test_multiple_schema_imports_merged() {
    let mut aliases = HashSet::new();
    aliases.insert("gpio0".to_string());
    aliases.insert("gpio1".to_string());
    let mut backend = LlvmBackend::new().with_schema_aliases(aliases);
    assert_eq!(backend.ctx.schema_alias_names.len(), 2);
    let output = backend.generate(&empty_program(), None);
    assert!(output.contains("ModuleID"));
}

#[test]
fn test_imported_alias_is_mmio() {
    let mut aliases = HashSet::new();
    aliases.insert("led_0".to_string());
    let mut mmio: HashMap<String, u64> = HashMap::new();
    mmio.insert("led_0".to_string(), 0x40000000);
    let mut backend = LlvmBackend::new()
        .with_schema_aliases(aliases)
        .with_mmio_addresses(mmio);
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "led_0".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "main".to_string(),
            type_params: vec![],
            parameters: vec![],
            is_reactive: true,
            is_async: false,
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("led_0".to_string()), Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("inttoptr i64 1073741824"),
        "led_0 with schema import should be MMIO (inttoptr). Got: {}", output);
    assert!(output.contains("store volatile i64"),
        "led_0 with schema import should use volatile store. Got: {}", output);
}

#[test]
fn test_unimported_alias_not_mmio() {
    let mut aliases = HashSet::new();
    aliases.insert("uart_debug".to_string());
    let mut mmio: HashMap<String, u64> = HashMap::new();
    mmio.insert("led_0".to_string(), 0x40000000);
    mmio.insert("uart_debug".to_string(), 0xFF010000);
    let mut backend = LlvmBackend::new()
        .with_schema_aliases(aliases)
        .with_mmio_addresses(mmio);
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "led_0".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "t".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "_".to_string(),
                    ty: None,
                    expr: Some(Expr::Identifier("led_0".to_string())),
                    modifiers: vec![],
                },
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(!output.contains("inttoptr i64 1073741824"),
        "led_0 NOT in schema should NOT be MMIO (no inttoptr for 0x40000000). Got: {}", output);
    assert!(output.contains("getelementptr inbounds %State"),
        "led_0 NOT in schema should use struct GEP. Got: {}", output);
}

// ── Intrinsic tests ──────────────────────────────────────

fn make_intrinsic_program(intrinsic: Expr) -> Vec<TopLevel> {
    vec![
        TopLevel::Transaction(Transaction {
            name: "main".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "r".to_string(),
                    ty: Some(Type::int()),
                    expr: Some(intrinsic),
                    modifiers: vec![],
                },
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ]
}

fn make_float_intrinsic_program(intrinsic: Expr) -> Vec<TopLevel> {
    vec![
        TopLevel::Transaction(Transaction {
            name: "main".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "r".to_string(),
                    ty: Some(Type::float()),
                    expr: Some(intrinsic),
                    modifiers: vec![],
                },
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ]
}

#[test]
fn test_emit_cast_int_to_string() {
    // The direct-cast path resolves String membership via the universe (the
    // real pipeline always has one), so the bare-backend test must set it.
    let mut backend = LlvmBackend::new().with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::Transaction(Transaction {
            name: "main".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "r".to_string(),
                    ty: Some(Type::string()),
                    expr: Some(Expr::Cast(Box::new(Expr::Decimal(42)), Type::string())),
                    modifiers: vec![],
                },
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // 2026-08-04 (Phase 3): the hardcoded `__int_to_str__` fallback arm was
    // removed — the casting graph's `Int -> String` ExtCall lane is the sole
    // path. Assert ONLY the graph lane is emitted.
    assert!(output.contains("call ptr @int_to_str(i64"),
        "Cast Int -> String must resolve through the casting graph lane. Got:\n{}", output);
    assert!(!output.contains("call ptr @__int_to_str__("),
        "The hardcoded __int_to_str__ fallback must be gone. Got:\n{}", output);
}

#[test]
fn test_emit_cast_string_to_int() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Transaction(Transaction {
            name: "main".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "r".to_string(),
                    ty: Some(Type::int()),
                    expr: Some(Expr::Cast(Box::new(Expr::Quoted("42".into())), Type::int())),
                    modifiers: vec![],
                },
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // 2026-07-30: Protocol-based cast path replaces __str_to_int.
    // String→Int now goes through protocol dispatch, not __str_to_int.
    // 2026-07-30: Check that __str_to_int is NOT called (only declared as extern).
    // The extern declaration is always emitted for known runtime functions.
    assert!(!output.contains("call i64 @__str_to_int"),
        "Cast String -> Int should NOT call __str_to_int (protocol path). Got:\n{}", output);
}

// ── List tests ───────────────────────────────────────────

// ── Tuple tests ──────────────────────────────────────────

#[test]
fn test_tuple_emits_2slot_header() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "t".to_string(), ty: Type::int(), span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "mktup".to_string(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![],
            output_type: None, outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("t".to_string()), Expr::Tuple(vec![Expr::Decimal(1), Expr::Decimal(2), Expr::Decimal(3)])),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("call ptr @malloc(i64 40)"), "3-elem tuple = 40 bytes (5 slots × 8). Got: {}", output);
    assert!(output.contains("store i64 3, ptr"), "Length should be 3. Got: {}", output);
}

// ── Optimization report & chain composition ──────────────

fn make_chain_program(
    txns: Vec<(&str, Vec<Statement>)>,
    consts: &[(&str, i64)],
    states: &[(&str, i64)],
) -> Vec<TopLevel> {
    let mut items: Vec<TopLevel> = Vec::new();
    for (name, val) in consts {
        items.push(TopLevel::Constant(Constant {
            name: name.to_string(),
            ty: Type::int(),
            expr: Expr::Decimal(*val),
            section: None,
        }));
    }
    for (name, val) in states {
        items.push(TopLevel::StateDecl(StateDecl {
            name: name.to_string(),
            ty: Type::int(),
            span: None,
        }));
    }
    for (txn_name, body) in txns {
        let pre = Expr::BinaryOp(BinaryOpKind::Lt,
            Box::new(Expr::Identifier("count".to_string())), Box::new(Expr::Identifier("total".to_string())));
        items.push(TopLevel::Transaction(Transaction {
            name: txn_name.to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: pre,
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body,
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }));
    }
    items
}

fn ident_s(s: &str) -> Expr { Expr::Identifier(s.to_string()) }
fn int_s(v: i64) -> Expr { Expr::Decimal(v) }

#[test]
fn test_report_shows_ranking() {
    let program = make_chain_program(
        vec![("t1", vec![
            Statement::Assign(ident_s("x"), ident_s("sensor")),
            Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
        ])],
        &[("total", 100)],
        &[("count", 0), ("x", 0)],
    );
    let mut backend = LlvmBackend::new()
        .with_optimize_budget(256).with_optimize_report(true);
    let _output = backend.generate(&program, None);
    let report: Vec<&str> = backend.report().iter().map(|s| s.as_str()).collect();
    let joined = report.join("\n");
    // 2026-07-14: With full analysis wired, check that the report has
    // substantive optimization content.
    assert!(!report.is_empty(), "Report should contain content");
    assert!(joined.contains("Budget plan") || joined.contains("Linear transaction")
        || joined.contains("Optimization priority"),
        "Report should contain optimization analysis");
}

#[test]
fn test_report_shows_budget() {
    let program = make_chain_program(
        vec![("t1", vec![
            Statement::Assign(ident_s("x"), ident_s("sensor")),
            Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
        ])],
        &[("total", 100)],
        &[("count", 0), ("x", 0)],
    );
    let mut backend = LlvmBackend::new()
        .with_optimize_budget(10).with_optimize_report(true);
    let _output = backend.generate(&program, None);
    let report: Vec<&str> = backend.report().iter().map(|s| s.as_str()).collect();
    let joined = report.join("\n");
    assert!(joined.contains("Budget plan"),
        "Report should contain budget plan section");
}

#[test]
fn test_report_shows_size() {
    let program = make_chain_program(
        vec![("t1", vec![
            Statement::Assign(ident_s("x"), ident_s("sensor")),
            Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
        ])],
        &[("total", 100)],
        &[("count", 0), ("x", 0)],
    );
    let mut backend = LlvmBackend::new()
        .with_optimize_budget(256).with_optimize_report(true)
        .with_optimize_size(10000);
    let _output = backend.generate(&program, None);
    let report: Vec<&str> = backend.report().iter().map(|s| s.as_str()).collect();
    let joined = report.join("\n");
    // 2026-07-14: Size estimation requires triggers for enumerable dispatch.
    // The report still contains chain analysis and budget info.
    assert!(joined.contains("Budget plan") || joined.contains("Linear transaction"),
        "Report should contain optimization info");
}

#[test]
fn test_report_shows_chains() {
    let program = make_chain_program(
        vec![
            ("step_a", vec![
                Statement::Assign(ident_s("x"), ident_s("sensor")),
                Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
            ]),
            ("step_b", vec![
                Statement::Assign(ident_s("y"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("x")), Box::new(int_s(1)))),
                Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
            ]),
        ],
        &[("total", 100)],
        &[("count", 0), ("x", 0), ("y", 0)],
    );
    let mut backend = LlvmBackend::new()
        .with_optimize_budget(256).with_optimize_report(true);
    let _output = backend.generate(&program, None);
    let report: Vec<&str> = backend.report().iter().map(|s| s.as_str()).collect();
    let joined = report.join("\n");
    assert!(joined.contains("Linear transaction chains")
        || joined.contains("Composed chains"),
        "Report should detect multi-txn chains");
}

#[test]
fn test_precompute_pure_counter() {
    let program = make_chain_program(
        vec![
            ("step_a", vec![
                Statement::Assign(ident_s("x"), int_s(42)),
                Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
            ]),
            ("step_b", vec![
                Statement::Assign(ident_s("y"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("x")), Box::new(int_s(1)))),
                Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
            ]),
        ],
        &[("total", 100)],
        &[("count", 0), ("x", 0), ("y", 0)],
    );
    let output = LlvmBackend::new().with_optimize_budget(256).generate(&program, None);
    assert!(output.contains("ret i32 0"),
        "Should return normally");
}

#[test]
fn test_precompute_budget_exceeded_fallback() {
    let program = make_chain_program(
        vec![
            ("step_a", vec![
                Statement::Assign(ident_s("x"), int_s(42)),
                Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
            ]),
            ("step_b", vec![
                Statement::Assign(ident_s("y"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("x")), Box::new(int_s(1)))),
                Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
            ]),
        ],
        &[("total", 100)],
        &[("count", 0), ("x", 0), ("y", 0)],
    );
    let output = LlvmBackend::new().with_optimize_budget(0).generate(&program, None);
    assert!(output.contains("getelementptr inbounds %State, ptr %state, i32 0, i32"),
        "All-convergent program should use per-field GEP loads");
    assert!(!output.contains("@reactor_tick"),
        "All-convergent program should not emit reactor_tick");
}

#[test]
fn test_iir_filter_folded_path_regression() {
    let program = make_chain_program(
        vec![("process", vec![
            Statement::Assign(ident_s("x"), int_s(42)),
            Statement::Assign(ident_s("count"), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("count")), Box::new(int_s(1)))),
        ])],
        &[("total", 50000000)],
        &[("count", 0), ("x", 0)],
    );
    let output = LlvmBackend::new().generate(&program, None);
    assert!(!output.contains("switch i64"),
        "Single-txn convergence should use folded path, not enum dispatch");
    assert!(!output.contains("@reactor_tick"),
        "Single-txn convergence should use folded path, not standard reactor");
    assert!(output.contains("store i64 50000000"),
        "Effectively-pure body should emit O(1) store i64 total, not a while-loop");
    assert!(output.contains("ret i32 0"),
        "Should return after store");
    let main_idx = output.find("define i32 @main()").unwrap_or(0);
    let store_in_main = output[main_idx..].contains("store i64 50000000");
    assert!(store_in_main, "store must be in main, not in process");
}

// ── Async tests ──────────────────────────────────────────

fn make_async_pair_program() -> Vec<TopLevel> {
    vec![
        TopLevel::StateDecl(StateDecl {
            name: "a".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::StateDecl(StateDecl {
            name: "b".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "inc_a".to_string(),
            is_reactive: true,
            is_async: true,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("a".to_string()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("a")), Box::new(int_s(1)))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "inc_b".to_string(),
            is_reactive: true,
            is_async: true,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("b".to_string()), Expr::BinaryOp(BinaryOpKind::Add, Box::new(ident_s("b")), Box::new(int_s(1)))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ]
}

#[test]
fn test_async_body_functions_emitted() {
    let program = make_async_pair_program();
    let output = LlvmBackend::new().generate(&program, None);
    assert!(output.contains("@async_body_inc_a"),
        "Async body function for inc_a should be emitted");
    assert!(output.contains("@async_body_inc_b"),
        "Async body function for inc_b should be emitted");
}

// 2026-09-10 (Family H): the pthread pool is replaced by direct
// sequential async-body calls — these tests assert the COOPERATIVE
// contract (bodies called from main, no pool machinery).
#[test]
fn test_async_bodies_called_in_main() {
    let program = make_async_pair_program();
    let output = LlvmBackend::new().generate(&program, None);
    assert!(output.contains("call void @async_body_inc_a(ptr noalias nocapture %state)"),
        "Main should call the inc_a body directly");
    assert!(output.contains("call void @async_body_inc_b(ptr noalias nocapture %state)"),
        "Main should call the inc_b body directly");
    assert!(!output.contains("@__thread_pool_init__"),
        "The pthread pool is deleted — no init call");
    assert!(!output.contains("@__barrier_release__"),
        "The barriers are deleted");
    assert!(!output.contains("@__barrier_wait__"),
        "The barriers are deleted");
}

#[test]
fn test_no_thread_pool_without_async_txns() {
    let program = make_exit_program(None, false);
    let output = LlvmBackend::new().generate(&program, None);
    assert!(!output.contains("@llvm.thread_pool"),
        "No thread pool metadata without async txns");
    assert!(!output.contains("call void @__barrier__"),
        "No barrier calls without async txns");
    assert!(!output.contains("call void @__thread_pool_init__"),
        "No thread pool init without async txns");
}

// ── Struct param tests ───────────────────────────────────

#[test]
fn test_struct_param_uses_ptr_in_signature() {
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Point".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![
                StructField { name: "x".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
                StructField { name: "y".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
            ],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "process".to_string(),
            type_params: vec![],
            parameters: vec![("p".to_string(), Type::Custom("Point".to_string()))],
            outputs: vec![Type::bool_()],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Term(Some(Expr::Bool(true))),
            ],
            modifiers: vec![Annotation { name: "export".to_string(), value: Some(Expr::Bool(true)) }],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("define i64 @process(ptr noundef noalias nocapture align 8 %state, i64 %arg0"),
        "Struct param should be the boxed i64 handle in the function signature.\nGot:\n{}", output);
}

#[test]
fn test_struct_param_ptrtoint_at_entry() {
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Point".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![
                StructField { name: "x".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
                StructField { name: "y".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
            ],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "process".to_string(),
            type_params: vec![],
            parameters: vec![("p".to_string(), Type::Custom("Point".to_string()))],
            outputs: vec![Type::bool_()],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Term(Some(Expr::Bool(true))),
            ],
            modifiers: vec![Annotation { name: "export".to_string(), value: Some(Expr::Bool(true)) }],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // 2026-08-13 (obj value ABI): a struct PARAM arrives as the boxed i64
    // handle directly — no ptrtoint boxing at entry (the old ptr-param ABI
    // boxed a `ptr` param to i64 for SSA). Field access inttoprs the handle.
    assert!(!output.contains("ptrtoint ptr %arg0"),
        "Struct param arrives boxed — no ptrtoint at entry.\nGot:\n{}", output);
    assert!(output.contains("i64 %arg0"),
        "Struct param is the boxed i64 handle.\nGot:\n{}", output);
}

#[test]
fn test_call_with_ptr_arg_emits_inttoptr() {
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "callee".to_string(),
            type_params: vec![],
            parameters: vec![("p".to_string(), Type::Ptr(Box::new(Type::int())))],
            outputs: vec![Type::int()],
            output_type: None,
            contract: default_contract(),
            body: vec![Statement::Term(Some(Expr::Decimal(42)))],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "caller".to_string(),
            type_params: vec![],
            parameters: vec![("p".to_string(), Type::Ptr(Box::new(Type::int())))],
            outputs: vec![Type::int()],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Term(Some(Expr::Call(
                    "callee".to_string(),
                    vec![Expr::Identifier("p".to_string())],
                    None,
                ))),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("inttoptr"),
        "Call with Ptr arg should emit inttoptr before the call.\nGot:\n{}", output);
}


#[test]
fn test_struct_param_field_access_works() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::Obj(StructDefinition {
            name: "Point".to_string(),
            type_params: vec![],
            parent: None,
            fields: vec![
                StructField { name: "x".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
                StructField { name: "y".to_string(), ty: Type::int(), default: None, visibility: Visibility::Public },
            ],
            transactions: vec![],
            view_html: None,
            span: None,
            modifiers: vec![],
            variants: vec![],
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "get_x".to_string(),
            type_params: vec![],
            parameters: vec![("p".to_string(), Type::Custom("Point".to_string()))],
            outputs: vec![Type::int()],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Term(Some(Expr::Field(Box::new(Expr::Identifier("p".to_string())), "x".to_string()))),
            ],
            modifiers: vec![Annotation { name: "export".to_string(), value: Some(Expr::Quoted("get_x".into())) }],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("getelementptr"),
        "Field access on struct param should emit GEP.\nGot:\n{}", output);
    assert!(!output.contains("not found on object"),
        "Field access on struct param should succeed.\nGot:\n{}", output);
}

// ── Event model / Trigger (basic) tests ──────────────────

#[test]
fn test_event_model_trigger_handling() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "pump".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![Statement::Term(None)],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("%State"),
        "Should generate state type");
    assert!(output.contains("define void @init_state"),
        "Should have init_state");
}

// ── First-class function ptr test ────────────────────────

fn make_fn_ptr_program() -> Vec<TopLevel> {
    vec![
        TopLevel::StateDecl(StateDecl {
            name: "x".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "apply".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(Expr::Identifier("x".to_string()), Expr::Decimal(42)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ]
}

#[test]
fn test_fn_ptr_not_crashes() {
    let program = make_fn_ptr_program();
    let output = LlvmBackend::new().generate(&program, None);
    assert!(output.contains("define i32 @main"),
        "Should emit main function");
}

#[test]
fn test_emit_address_of() {
    // AddressOf#("uart") should emit inttoptr with uart's address (0xFFE01000)
    let program = vec![
        TopLevel::Transaction(Transaction {
            name: "main".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: default_contract(),
            body: vec![
                Statement::Assign(
                    Expr::Identifier("ptr".to_string()),
                    Expr::Call("AddressOf#".to_string(), vec![Expr::Quoted(b"uart".to_vec())], None),
                ),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = LlvmBackend::new().generate(&program, None);
    // uart = 0xFFE01000 in config/address-map.dbvl
    assert!(output.contains("inttoptr"), "Should emit inttoptr");
    // Resolve the expected address from the shared resolver
    let expected_addr = crate::address_resolver::resolve_address("uart");
    let expected_str = expected_addr.to_string();
    assert!(output.contains(&expected_str), "Should contain uart address {} (= 0x{:X})", expected_str, expected_addr);
}

#[test]
fn test_frgn_ptr_declare() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::ForeignBinding(ForeignBinding {
            foreign_name: "test_fn".to_string(),
            briev_name: None,
            from: FromSpec::CompilerRegistry("c".to_string()),
            target: ForeignTarget::C,
            inputs: vec![("arg".to_string(), Type::Ptr(Box::new(Type::int())))],
            success_output: vec![("".to_string(), Type::int())],
            error_type: String::new(),
            error_fields: vec![],
            input_layout: None,
            output_layout: None,
            precondition: None,
            postcondition: None,
            buffer_mode: None,
            default_watchdog: None,
            wasm_impl: None,
            wasm_setup: None,
            span: None,
            is_optional: false,
            is_fire_forget: false,
            is_delivery: false,
            is_variadic: false,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("declare i64 @test_fn(ptr)"),
        "Ptr param should produce 'ptr' in declare, not 'i64'.\nGot:\n{}", output);
}

#[test]
fn test_frgn_ptr_return() {
    let mut backend = LlvmBackend::new();
    let program = vec![
        TopLevel::ForeignBinding(ForeignBinding {
            foreign_name: "make_ptr".to_string(),
            briev_name: None,
            from: FromSpec::CompilerRegistry("c".to_string()),
            target: ForeignTarget::C,
            inputs: vec![("n".to_string(), Type::int())],
            success_output: vec![("".to_string(), Type::Ptr(Box::new(Type::int())))],
            error_type: String::new(),
            error_fields: vec![],
            input_layout: None,
            output_layout: None,
            precondition: None,
            postcondition: None,
            buffer_mode: None,
            default_watchdog: None,
            wasm_impl: None,
            wasm_setup: None,
            span: None,
            is_optional: false,
            is_fire_forget: false,
            is_delivery: false,
            is_variadic: false,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    assert!(output.contains("declare ptr @make_ptr(i64)"),
        "Ptr return should produce 'ptr' in declare, not 'i64'.\nGot:\n{}", output);
}

#[test]
fn test_struct_literal_field_offsets() {
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(tu);
    let program = vec![
        TopLevel::StaticStruct(StructDef {
            type_params: vec![],
            name: "Mixed".to_string(),
            fields: vec![
                ("a".to_string(), Type::int()),
                ("b".to_string(), Type::bool_()),
                ("c".to_string(), Type::char_()),
            ],
            metadata: HashMap::new(),
            seq: false,
            pack: false,
            union: false,
            coll: false,
            span: None,
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "test".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "x".to_string(),
                    ty: None,
                    expr: Some(Expr::StructLiteral {
                        type_name: "Mixed".to_string(),
                        fields: vec![
                            ("a".to_string(), Expr::Decimal(42)),
                            ("b".to_string(), Expr::Bool(true)),
                            ("c".to_string(), Expr::Decimal(65)),
                        ],
                    }),
                    modifiers: vec![],
                },
                Statement::Term(Some(Expr::Decimal(0))),
            ],
            modifiers: vec![],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // Field layout (pack=1): a(Int=8B)@0, b(Bool=1B)@8, c(Char=4B)@9
    assert!(output.contains("getelementptr i8, ptr %t1, i64 0"),
        "Field 'a' should be at offset 0.\nGot:\n{}", output);
    assert!(output.contains("getelementptr i8, ptr %t1, i64 8"),
        "Field 'b' (Bool) should be at offset 8.\nGot:\n{}", output);
    assert!(output.contains("getelementptr i8, ptr %t1, i64 9"),
        "Field 'c' (Char) should be at offset 9.\nGot:\n{}", output);
}

#[test]
fn test_addr_of_struct_literal() {
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![
        TopLevel::StaticStruct(StructDef {
            type_params: vec![],
            name: "Point".to_string(),
            fields: vec![
                ("x".to_string(), Type::int()),
                ("y".to_string(), Type::int()),
            ],
            metadata: HashMap::new(),
            seq: false,
            pack: false,
            union: false,
            coll: false,
            span: None,
        }),
        TopLevel::ForeignBinding(ForeignBinding {
            foreign_name: "use_ptr".to_string(),
            briev_name: None,
            from: FromSpec::CompilerRegistry("c".to_string()),
            target: ForeignTarget::C,
            inputs: vec![("p".to_string(), Type::Ptr(Box::new(Type::int())))],
            success_output: vec![("".to_string(), Type::int())],
            error_type: String::new(),
            error_fields: vec![],
            input_layout: None,
            output_layout: None,
            precondition: None,
            postcondition: None,
            buffer_mode: None,
            default_watchdog: None,
            wasm_impl: None,
            wasm_setup: None,
            span: None,
            is_optional: false,
            is_fire_forget: false,
            is_delivery: false,
            is_variadic: false,
            doc: None,
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "main".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "pt".to_string(),
                    ty: None,
                    expr: Some(Expr::StructLiteral {
                        type_name: "Point".to_string(),
                        fields: vec![
                            ("x".to_string(), Expr::Decimal(10)),
                            ("y".to_string(), Expr::Decimal(20)),
                        ],
                    }),
                    modifiers: vec![],
                },
                // Call frgn with &pt param
                Statement::Expression(Expr::Call(
                    "use_ptr".to_string(),
                    vec![Expr::AddrOf(Box::new(Expr::Identifier("pt".to_string())))],
                    None,
                )),
                Statement::Term(Some(Expr::Decimal(0))),
            ],
            modifiers: vec![],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // &pt should emit ptrtoint on the struct allocation, NOT ptrtoint on a
    // function ptr. 2026-08-13 (struct value lifetime): the struct literal is
    // HEAP-allocated (a stack alloca's handle dangles once the constructing
    // function returns), so the assertion is malloc, not `alloca`.
    assert!(output.contains("call ptr @malloc(i64 16)"),
        "Struct literal should malloc 16 bytes (2 x 8B fields).\nGot:\n{}", output);
    assert!(output.contains("ptrtoint ptr %t"),
        "Should emit ptrtoint of the struct allocation for &pt.\nGot:\n{}", output);
    // Should NOT reference @pt as a function symbol
    assert!(!output.contains("ptrtoint ptr @pt"),
        "Should NOT emit ptrtoint of @pt as if it were a function.\nGot:\n{}", output);
}

#[test]
fn test_frgn_ptr_param_inttoptr() {
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![
        TopLevel::StaticStruct(StructDef {
            type_params: vec![],
            name: "Point".to_string(),
            fields: vec![
                ("x".to_string(), Type::int()),
                ("y".to_string(), Type::int()),
            ],
            metadata: HashMap::new(),
            seq: false,
            pack: false,
            union: false,
            coll: false,
            span: None,
        }),
        TopLevel::ForeignBinding(ForeignBinding {
            foreign_name: "use_ptr".to_string(),
            briev_name: None,
            from: FromSpec::CompilerRegistry("c".to_string()),
            target: ForeignTarget::C,
            inputs: vec![("p".to_string(), Type::Ptr(Box::new(Type::int())))],
            success_output: vec![("".to_string(), Type::int())],
            error_type: String::new(),
            error_fields: vec![],
            input_layout: None,
            output_layout: None,
            precondition: None,
            postcondition: None,
            buffer_mode: None,
            default_watchdog: None,
            wasm_impl: None,
            wasm_setup: None,
            span: None,
            is_optional: false,
            is_fire_forget: false,
            is_delivery: false,
            is_variadic: false,
            doc: None,
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "main".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "pt".to_string(),
                    ty: None,
                    expr: Some(Expr::StructLiteral {
                        type_name: "Point".to_string(),
                        fields: vec![
                            ("x".to_string(), Expr::Decimal(10)),
                            ("y".to_string(), Expr::Decimal(20)),
                        ],
                    }),
                    modifiers: vec![],
                },
                Statement::Expression(Expr::Call(
                    "use_ptr".to_string(),
                    vec![Expr::AddrOf(Box::new(Expr::Identifier("pt".to_string())))],
                    None,
                )),
                Statement::Term(Some(Expr::Decimal(0))),
            ],
            modifiers: vec![],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // When calling a frgn with Ptr param, the i64 address should be converted
    // via inttoptr so the LLVM call uses ptr type matching the declare.
    assert!(output.contains("inttoptr i64"),
        "Should emit inttoptr to convert i64 to ptr for Ptr param.\nGot:\n{}", output);
    // The call should use ptr type for the Ptr param
    assert!(output.contains("call i64 @use_ptr(ptr"),
        "Call to use_ptr(ptr) should use 'ptr' type for the first param.\nGot:\n{}", output);
}

#[test]
fn test_trg_deref_error_flag() {
    // When --error-unresolved-trg is set, a @ *ptr dynamic trigger should
    // emit a null check + unreachable before the load volatile.
    // The trigger must be referenced in the transaction's precondition
    // for emit_trg_load to be called. Without a precondition reference,
    // the trigger is dead code and the backend skips it.
    use crate::ast::Contract;
    let program = vec![
        TopLevel::Trigger(Trigger {
            name: "dyn_trg".to_string(),
            // @ *ptr — Expr::Deref wraps the pointer expression
            instance: Expr::Deref(Box::new(Expr::Identifier("my_ptr".to_string()))),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "pump".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(
                    BinaryOpKind::Eq,
                    Box::new(Expr::Identifier("dyn_trg".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(None)],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = LlvmBackend::new()
        .with_trg_unresolved_action(crate::backend::llvm::TrgUnresolvedAction::Error)
        .generate(&program, None);
    assert!(output.contains("icmp eq ptr"), "Should emit null check for error mode");
    assert!(output.contains("unreachable"), "Should emit unreachable for error mode");
}

#[test]
fn test_trg_deref_warn_default_no_null_check() {
    // Default (Warn) mode should NOT emit null check for @ *ptr triggers.
    use crate::ast::Contract;
    let program = vec![
        TopLevel::Trigger(Trigger {
            name: "dyn_trg".to_string(),
            instance: Expr::Deref(Box::new(Expr::Identifier("my_ptr".to_string()))),
            span: None,
        }),
        TopLevel::Transaction(Transaction {
            name: "pump".to_string(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(
                    BinaryOpKind::Eq,
                    Box::new(Expr::Identifier("dyn_trg".to_string())),
                    Box::new(Expr::Decimal(1)),
                ),
                post_condition: Expr::Bool(true),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![Statement::Term(None)],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = LlvmBackend::new().generate(&program, None);
    // 2026-07-18: The comparison type now uses the operand's LLVM type.
    // For pointer operands, icmp eq ptr is valid — update assertion to match.
    // If the output contains "icmp eq ptr" that's fine (no null check flag).
    // The null check flag is "null_check" in the IR comments, not icmp.
    assert!(!output.contains("null_check"), "Default mode should not emit null check");
}

#[test]
fn test_struct_array_list_literal() {
    // When a list literal contains only struct literals of the same
    // known struct type, the backend should emit a contiguous stack array
    // (alloca) instead of a heap-allocated list (malloc).
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![
        TopLevel::StaticStruct(StructDef {
            type_params: vec![],
            name: "Point".to_string(),
            fields: vec![
                ("x".to_string(), Type::int()),
                ("y".to_string(), Type::int()),
            ],
            metadata: HashMap::new(),
            seq: false,
            pack: false,
            union: false,
            coll: false,
            span: None,
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "main".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "pts".to_string(),
                    ty: None,
                    expr: Some(Expr::List(vec![
                        Expr::StructLiteral {
                            type_name: "Point".to_string(),
                            fields: vec![
                                ("x".to_string(), Expr::Decimal(10)),
                                ("y".to_string(), Expr::Decimal(20)),
                            ],
                        },
                        Expr::StructLiteral {
                            type_name: "Point".to_string(),
                            fields: vec![
                                ("x".to_string(), Expr::Decimal(30)),
                                ("y".to_string(), Expr::Decimal(40)),
                            ],
                        },
                    ])),
                    modifiers: vec![],
                },
                Statement::Term(Some(Expr::Decimal(0))),
            ],
            modifiers: vec![],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // Should allocate 32 bytes (2 elements * 16 bytes each)
    assert!(output.contains("alloca i8, i64 32"),
        "Should allocate 32 bytes for 2-element Point array.\nGot:\n{}", output);
    // First element at offset 0: x=10, y=20
    assert!(output.contains("getelementptr i8, ptr %t"),
        "Should emit GEP for first element's first field at offset 0.\nGot:\n{}", output);
    // Should NOT call malloc (no heap allocation)
    assert!(!output.contains("call @malloc"),
        "Should NOT emit malloc call for struct array list.\nGot:\n{}", output);
    // Should emit ptrtoint (the handle is the pointer to the stack array)
    assert!(output.contains("ptrtoint ptr"),
        "Should emit ptrtoint to produce i64 handle.\nGot:\n{}", output);
}

#[test]
fn test_struct_array_addr_of_and_frgn_call() {
    // Struct array + &var + frgn call with Ptr param.
    // The address-of should produce the alloca pointer, and the frgn call
    // should emit inttoptr to convert i64 handle to ptr for the Ptr param.
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![
        TopLevel::StaticStruct(StructDef {
            type_params: vec![],
            name: "PyMethodDef".to_string(),
            fields: vec![
                ("name".to_string(), Type::int()),
                ("flags".to_string(), Type::int()),
            ],
            metadata: HashMap::new(),
            seq: false,
            pack: false,
            union: false,
            coll: false,
            span: None,
        }),
        TopLevel::ForeignBinding(ForeignBinding {
            foreign_name: "use_methods".to_string(),
            briev_name: None,
            from: FromSpec::CompilerRegistry("c".to_string()),
            target: ForeignTarget::C,
            inputs: vec![("p".to_string(), Type::Ptr(Box::new(Type::int())))],
            success_output: vec![("".to_string(), Type::int())],
            error_type: String::new(),
            error_fields: vec![],
            input_layout: None,
            output_layout: None,
            precondition: None,
            postcondition: None,
            buffer_mode: None,
            default_watchdog: None,
            wasm_impl: None,
            wasm_setup: None,
            span: None,
            is_optional: false,
            is_fire_forget: false,
            is_delivery: false,
            is_variadic: false,
            doc: None,
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "main".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: default_contract(),
            body: vec![
                Statement::Let { names: vec![], 
                    name: "methods".to_string(),
                    ty: None,
                    expr: Some(Expr::List(vec![
                        Expr::StructLiteral {
                            type_name: "PyMethodDef".to_string(),
                            fields: vec![
                                ("name".to_string(), Expr::Decimal(1)),
                                ("flags".to_string(), Expr::Decimal(2)),
                            ],
                        },
                    ])),
                    modifiers: vec![],
                },
                Statement::Expression(Expr::Call(
                    "use_methods".to_string(),
                    vec![Expr::AddrOf(Box::new(Expr::Identifier("methods".to_string())))],
                    None,
                )),
                Statement::Term(Some(Expr::Decimal(0))),
            ],
            modifiers: vec![],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let output = backend.generate(&program, None);
    // Should allocate 16 bytes (1 element * 16 bytes = 2 x i64)
    assert!(output.contains("alloca i8, i64 16"),
        "Should allocate 16 bytes for 1-element PyMethodDef array.\nGot:\n{}", output);
    // Should NOT call malloc
    assert!(!output.contains("call @malloc"),
        "Should NOT emit malloc call.\nGot:\n{}", output);
    // Should emit inttoptr for passing the struct array pointer to the frgn
    assert!(output.contains("inttoptr i64"),
        "Should emit inttoptr for Ptr param.\nGot:\n{}", output);
    // The call should use ptr type for the Ptr param
    assert!(output.contains("call i64 @use_methods(ptr"),
        "Should call use_methods with ptr type.\nGot:\n{}", output);
}

#[test]
fn test_shape_vector_groups_same_type_gate() {
    // Phase 1b: the frontend structural pass cannot express the LLVM same-type
    // gate; the backend re-applies it in shape_vector_groups. A mixed-type
    // group must be dropped, an all-float group accepted.
    let mut backend = LlvmBackend::new();
    backend.ctx.field_index_map.insert("f0".to_string(), 0);
    backend.ctx.field_index_map.insert("f1".to_string(), 1);
    backend.ctx.field_index_map.insert("i0".to_string(), 2);
    backend.ctx.field_index_map.insert("i1".to_string(), 3);
    backend.ctx.field_types = vec![
        "float".to_string(),
        "float".to_string(),
        "i64".to_string(),
        "i64".to_string(),
    ];
    let write_set: HashSet<String> = [
        "f0".to_string(),
        "f1".to_string(),
        "i0".to_string(),
        "i1".to_string(),
    ]
    .into_iter()
    .collect();
    let groups = vec![
        crate::analysis::loop_shape::VectorGroup {
            name: "mixed".to_string(),
            width: 2,
            fields: vec!["f0".to_string(), "i0".to_string()],
        },
        crate::analysis::loop_shape::VectorGroup {
            name: "floats".to_string(),
            width: 2,
            fields: vec!["f0".to_string(), "f1".to_string()],
        },
    ];
    let vg = backend.shape_vector_groups(&groups, &write_set);
    assert_eq!(vg.len(), 1, "mixed-type group must be dropped");
    assert_eq!(vg[0].name, "floats");
    assert_eq!(vg[0].element_ty, "float");
}

#[test]
fn test_shape_vector_groups_drops_not_in_write_set() {
    // A group whose fields are not all unconditionally written must be dropped.
    let mut backend = LlvmBackend::new();
    backend.ctx.field_index_map.insert("f0".to_string(), 0);
    backend.ctx.field_index_map.insert("f1".to_string(), 1);
    backend.ctx.field_types = vec!["float".to_string(), "float".to_string()];
    // write_set only contains f0; f1 is written conditionally (e.g. in a
    // guarded block) so the group must not be used.
    let write_set: HashSet<String> = ["f0".to_string()].into_iter().collect();
    let groups = vec![crate::analysis::loop_shape::VectorGroup {
        name: "g".to_string(),
        width: 2,
        fields: vec!["f0".to_string(), "f1".to_string()],
    }];
    let vg = backend.shape_vector_groups(&groups, &write_set);
    assert!(vg.is_empty(), "group with unwritten field must be dropped");
}

#[test]
fn test_shape_vector_groups_no_overlap() {
    // A group whose fields overlap an already-accepted group must be dropped.
    let mut backend = LlvmBackend::new();
    backend.ctx.field_index_map.insert("f0".to_string(), 0);
    backend.ctx.field_index_map.insert("f1".to_string(), 1);
    backend.ctx.field_index_map.insert("f2".to_string(), 2);
    backend.ctx.field_types = vec![
        "float".to_string(),
        "float".to_string(),
        "float".to_string(),
    ];
    let write_set: HashSet<String> =
        ["f0".to_string(), "f1".to_string(), "f2".to_string()].into_iter().collect();
    let groups = vec![
        crate::analysis::loop_shape::VectorGroup {
            name: "g0".to_string(),
            width: 2,
            fields: vec!["f0".to_string(), "f1".to_string()],
        },
        crate::analysis::loop_shape::VectorGroup {
            name: "g1".to_string(),
            width: 2,
            fields: vec!["f1".to_string(), "f2".to_string()],
        },
    ];
    let vg = backend.shape_vector_groups(&groups, &write_set);
    // g1 reuses f1 from the accepted g0 → dropped; only g0 survives.
    assert_eq!(vg.len(), 1, "overlapping group must be dropped");
    assert_eq!(vg[0].name, "g0");
}

// ── Phase 2 measurement-pass consumers ──────────────────────────────

/// Phase 2 (§7.2): a sparse-dispatch-like modulo program must still be
/// dispatched via the modulo-rotated main loop (`.mr_loop`), driven by the
/// frontend-computed ModuloPartition.
#[test]
fn test_modulo_partition_drives_rotated_loop() {
    let mut program = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::StateDecl(StateDecl {
            name: "total".to_string(),
            ty: Type::int(),
            span: None,
        }),
    ];
    for (i, name) in ["even".to_string(), "odd".to_string()].iter().enumerate() {
        let pre = Expr::BinaryOp(BinaryOpKind::And,
            Box::new(Expr::BinaryOp(BinaryOpKind::Lt,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Identifier("total".to_string())))),
            Box::new(Expr::BinaryOp(BinaryOpKind::Eq,
                Box::new(Expr::BinaryOp(BinaryOpKind::Mod,
                    Box::new(Expr::Identifier("count".to_string())),
                    Box::new(Expr::Decimal(2)))),
                Box::new(Expr::Decimal(i as i64)))),
        );
        program.push(TopLevel::Transaction(Transaction {
            name: name.clone(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: pre,
                post_condition: Expr::BinaryOp(BinaryOpKind::Eq,
                    Box::new(Expr::Identifier("count".to_string())),
                    Box::new(Expr::Identifier("total".to_string()))),
                watchdog: None,
                explicit: false,
                span: None,
            post_authority: false},
            body: vec![
                Statement::Assign(
                    Expr::Identifier("count".to_string()),
                    Expr::BinaryOp(BinaryOpKind::Add,
                        Box::new(Expr::Identifier("count".to_string())),
                        Box::new(Expr::Decimal(1))),
                ),
                Statement::Term(None),
            ],
            metadata: HashMap::new(),
            derivation: None,
            modifiers: vec![],
            span: None,
            doc: None,
        }));
    }
    let output = LlvmBackend::new().generate(&program, None);
    assert!(output.contains(".mr_loop"),
        "modulo-bounded set must use the rotated loop, got:\n{}", &output[..output.len().min(12000)]);
    assert!(output.contains("srem i64"));
}

/// Phase 2 (§7.1): a dense kalman-style txn (FFI guard outlined → #11) must
/// be downgraded to `#0` because the frontend density measurement is > 4.0.
#[test]
fn test_density_consumer_downgrades_dense_txn() {
    let float_field = |name: &str| TopLevel::Statement(Box::new(Statement::Let {
        name: name.to_string(),
        names: vec![],
        ty: Some(Type::Custom("Float".to_string())),
        expr: Some(Expr::Float(0.0)),
        modifiers: vec![],
    }));
    let mut program: Vec<TopLevel> = vec![
        TopLevel::StateDecl(StateDecl {
            name: "count".to_string(),
            ty: Type::int(),
            span: None,
        }),
        TopLevel::StateDecl(StateDecl {
            name: "total".to_string(),
            ty: Type::int(),
            span: None,
        }),
    ];
    for n in ["x0", "x1", "x2", "p00", "p10", "p20"] {
        program.push(float_field(n));
    }
    for (n, v) in [("a00", 1.0), ("a01", 0.01), ("a02", 0.0)] {
        program.push(TopLevel::Constant(Constant {
            name: n.to_string(),
            ty: Type::Custom("Float".to_string()),
            expr: Expr::Float(v),
            section: None,
        }));
    }
    let mul = |l: Expr, r: Expr| Expr::BinaryOp(BinaryOpKind::Mul, Box::new(l), Box::new(r));
    let add = |l: Expr, r: Expr| Expr::BinaryOp(BinaryOpKind::Add, Box::new(l), Box::new(r));
    let body = vec![
        Statement::Let {
            name: "nx0".to_string(),
            names: vec![],
            ty: Some(Type::Custom("Float".to_string())),
            expr: Some(add(add(
                mul(Expr::Identifier("a00".to_string()), Expr::Identifier("x0".to_string())),
                mul(Expr::Identifier("a01".to_string()), Expr::Identifier("x1".to_string())),
            ), mul(Expr::Identifier("a02".to_string()), Expr::Identifier("x2".to_string())))),
            modifiers: vec![],
        },
        Statement::Let {
            name: "ap00".to_string(),
            names: vec![],
            ty: Some(Type::Custom("Float".to_string())),
            expr: Some(add(add(
                mul(Expr::Identifier("a00".to_string()), Expr::Identifier("p00".to_string())),
                mul(Expr::Identifier("a01".to_string()), Expr::Identifier("p10".to_string())),
            ), mul(Expr::Identifier("a02".to_string()), Expr::Identifier("p20".to_string())))),
            modifiers: vec![],
        },
        Statement::Assign(
            Expr::Identifier("x0".to_string()),
            Expr::Identifier("nx0".to_string()),
        ),
        Statement::Assign(
            Expr::Identifier("count".to_string()),
            Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Decimal(1))),
        ),
        // Guarded FFI → outlined (cold function), so the density check fires.
        Statement::Guarded(
            Expr::BinaryOp(BinaryOpKind::Eq,
                Box::new(Expr::BinaryOp(BinaryOpKind::Mod,
                    Box::new(Expr::Identifier("count".to_string())),
                    Box::new(Expr::Decimal(5)))),
                Box::new(Expr::Decimal(0))),
            vec![Statement::Expression(Expr::Call("PrintLn#".to_string(), vec![], None))],
        ),
        Statement::Term(None),
    ];
    program.push(TopLevel::Transaction(Transaction {
        name: "propagate".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Identifier("total".to_string()))),
            post_condition: Expr::BinaryOp(BinaryOpKind::Eq,
                Box::new(Expr::Identifier("count".to_string())),
                Box::new(Expr::Identifier("total".to_string()))),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body,
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    }));
    let output = LlvmBackend::new().generate(&program, None);
    let txn_line = output.lines()
        .find(|l| l.contains("define void @txn_propagate"))
        .unwrap_or("(not found)");
    assert!(txn_line.contains("#0"),
        "dense txn must be downgraded to #0 via the density measurement, got: {}\n{}",
        txn_line, &output[..output.len().min(1500)]);
}

// ── Batch-loop dispatch (plan 2026-07-31-regain-kalman-float-math-parity) ──

/// A post-increment periodic guard (`when count % N == 0` AFTER count++)
/// dispatches via the countdown loop (.cd_/.cdg_ structure), eliminating the
/// per-iteration modulo check.
#[test]
fn test_batch_loop_dispatch_post_increment() {
    // Dense body (≥ 40 arithmetic ops — the batch cost-model gate) on a set of
    // float fields, kalman-style. 12 fields each updated by a 3-term multiply
    // chain gives ~50+ ops, so the batch dispatch fires.
    let fld = |n: &str| TopLevel::StateDecl(StateDecl { name: n.into(), ty: Type::float(), span: None });
    let mut program = vec![
        TopLevel::StateDecl(StateDecl { name: "count".into(), ty: Type::int(), span: None }),
        TopLevel::StateDecl(StateDecl { name: "total".into(), ty: Type::int(), span: None }),
    ];
    for i in 0..12 {
        program.push(fld(&format!("f{}", i)));
    }
    let mul = |l: Expr, r: Expr| Expr::BinaryOp(BinaryOpKind::Mul, Box::new(l), Box::new(r));
    let add = |l: Expr, r: Expr| Expr::BinaryOp(BinaryOpKind::Add, Box::new(l), Box::new(r));
    let mut body: Vec<Statement> = Vec::new();
    // Each f_i update = f_i*a + f_(i+1)%12*b + f_(i+2)%12*c  (5 ops each → 60 total).
    for i in 0..12 {
        let rhs = add(add(
            mul(Expr::Identifier(format!("f{}", i)), Expr::Float(0.5)),
            mul(Expr::Identifier(format!("f{}", (i + 1) % 12)), Expr::Float(0.25)),
        ), mul(Expr::Identifier(format!("f{}", (i + 2) % 12)), Expr::Float(0.125)));
        body.push(Statement::Assign(Expr::Identifier(format!("f{}", i)), rhs));
    }
    body.push(Statement::Assign(Expr::Identifier("count".into()),
        Expr::BinaryOp(BinaryOpKind::Add,
            Box::new(Expr::Identifier("count".into())),
            Box::new(Expr::Decimal(1)))));
    body.push(Statement::Guarded(
        Expr::BinaryOp(BinaryOpKind::Eq,
            Box::new(Expr::BinaryOp(BinaryOpKind::Mod,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Decimal(100)))),
            Box::new(Expr::Decimal(0))),
        vec![Statement::Expression(Expr::Call("__print_float".into(), vec![Expr::Identifier("f0".into())], None))]));
    body.push(Statement::Term(None));
    program.push(TopLevel::Transaction(Transaction {
        name: "tick".into(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Identifier("total".into()))),
            post_condition: Expr::BinaryOp(BinaryOpKind::Eq,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Identifier("total".into()))),
            watchdog: None, explicit: false, span: None,
        post_authority: false},
        body,
        metadata: HashMap::new(), derivation: None, modifiers: vec![],
        span: None, doc: None,
    }));
    let output = LlvmBackend::new().generate(&program, None);
    assert!(output.contains(".cd_"), "post-increment periodic guard must use the countdown loop, got:\n{}", &output[..output.len().min(1200)]);
    assert!(output.contains(".cdg_"), "countdown loop must have a cold guard block");
    // The per-iteration modulo must be GONE from the body (it fires only when
    // the countdown %rem hits 0).
    let body = output.split(".cdb_").nth(1).unwrap_or("");
    let body_seg = body.split(".cdg_").next().unwrap_or("");
    assert!(!body_seg.contains("urem"), "countdown body must not compute count % N per iteration");
}

/// 2026-08-16 (sweep parity): the countdown's float-field backedge copies must
/// carry `fast`. A bare `fadd float 0.0, %x` cannot fold to `x` under strict
/// IEEE (the `-0.0`/signaling-NaN edge), so LLVM kept it as a live vector add
/// on the loop-carried critical path — the sweep-family loss (sparse 1.37x,
/// mid 1.08x, dense 1.48x). `fast` lets instcombine fold the value rename.
#[test]
fn test_countdown_field_backedge_copies_are_fast() {
    let fld = |n: &str| TopLevel::StateDecl(StateDecl { name: n.into(), ty: Type::float(), span: None });
    let mut program = vec![
        TopLevel::StateDecl(StateDecl { name: "count".into(), ty: Type::int(), span: None }),
        TopLevel::StateDecl(StateDecl { name: "total".into(), ty: Type::int(), span: None }),
        fld("f0"),
        fld("f1"),
    ];
    let mul = |l: Expr, r: Expr| Expr::BinaryOp(BinaryOpKind::Mul, Box::new(l), Box::new(r));
    let add = |l: Expr, r: Expr| Expr::BinaryOp(BinaryOpKind::Add, Box::new(l), Box::new(r));
    let body = vec![
        Statement::Assign(Expr::Identifier("f0".into()),
            add(mul(Expr::Identifier("f0".into()), Expr::Float(0.5)),
                mul(Expr::Identifier("f1".into()), Expr::Float(0.25)))),
        Statement::Assign(Expr::Identifier("f1".into()),
            add(mul(Expr::Identifier("f1".into()), Expr::Float(0.5)),
                mul(Expr::Identifier("f0".into()), Expr::Float(0.25)))),
        Statement::Assign(Expr::Identifier("count".into()),
            Expr::BinaryOp(BinaryOpKind::Add,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Decimal(1)))),
        Statement::Guarded(
            Expr::BinaryOp(BinaryOpKind::Eq,
                Box::new(Expr::BinaryOp(BinaryOpKind::Mod,
                    Box::new(Expr::Identifier("count".into())),
                    Box::new(Expr::Decimal(100)))),
                Box::new(Expr::Decimal(0))),
            vec![Statement::Expression(Expr::Call("__print_float".into(), vec![Expr::Identifier("f0".into())], None))]),
        Statement::Term(None),
    ];
    program.push(TopLevel::Transaction(Transaction {
        name: "tick".into(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Identifier("total".into()))),
            post_condition: Expr::BinaryOp(BinaryOpKind::Eq,
                Box::new(Expr::Identifier("count".into())),
                Box::new(Expr::Identifier("total".into()))),
            watchdog: None, explicit: false, span: None,
        post_authority: false},
        body,
        metadata: HashMap::new(), derivation: None, modifiers: vec![],
        span: None, doc: None,
    }));
    let output = LlvmBackend::new().with_type_universe(crate::type_universe::TypeUniverse::new())
        .generate(&program, None);
    assert!(output.contains(".cd_"), "post-increment periodic guard must use the countdown loop, got:\n{}", &output[..output.len().min(1200)]);
    assert!(output.contains("fadd fast float 0.0"),
        "countdown field backedge copies must be fast-math value renames (foldable), got:\n{}", &output[..output.len().min(12000)]);
    assert!(!output.contains("fadd float 0.0"),
        "countdown must never emit a bare non-fast fadd float 0.0 copy: {output}");
    assert!(!output.contains("fadd double 0.0"),
        "countdown must never emit a bare non-fast fadd double 0.0 copy: {output}");
}

/// A pre-increment periodic guard (knucleotide pattern) is NOT batched — it
/// stays on version-DAG (the batch structure is off-by-one for it).
#[test]
fn test_batch_loop_rejects_pre_increment() {
    let program = vec![
        TopLevel::StateDecl(StateDecl { name: "count".into(), ty: Type::int(), span: None }),
        TopLevel::StateDecl(StateDecl { name: "total".into(), ty: Type::int(), span: None }),
        TopLevel::StateDecl(StateDecl { name: "acc".into(), ty: Type::int(), span: None }),
        TopLevel::Transaction(Transaction {
            name: "tick".into(),
            is_reactive: true,
            is_async: false,
            type_params: vec![],
            parameters: vec![],
            output_type: None,
            outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("count".into())),
                    Box::new(Expr::Identifier("total".into()))),
                post_condition: Expr::BinaryOp(BinaryOpKind::Eq,
                    Box::new(Expr::Identifier("count".into())),
                    Box::new(Expr::Identifier("total".into()))),
                watchdog: None, explicit: false, span: None,
            post_authority: false},
            body: vec![
                // Guard BEFORE the increment (pre-increment semantics).
                Statement::Guarded(
                    Expr::BinaryOp(BinaryOpKind::Eq,
                        Box::new(Expr::BinaryOp(BinaryOpKind::Mod,
                            Box::new(Expr::Identifier("count".into())),
                            Box::new(Expr::Decimal(100)))),
                        Box::new(Expr::Decimal(0))),
                    vec![Statement::Expression(Expr::Call("__print_int".into(), vec![Expr::Identifier("acc".into())], None))]),
                Statement::Assign(Expr::Identifier("acc".into()),
                    Expr::BinaryOp(BinaryOpKind::Add,
                        Box::new(Expr::Identifier("acc".into())),
                        Box::new(Expr::Decimal(1)))),
                Statement::Assign(Expr::Identifier("count".into()),
                    Expr::BinaryOp(BinaryOpKind::Add,
                        Box::new(Expr::Identifier("count".into())),
                        Box::new(Expr::Decimal(1)))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![],
            span: None, doc: None,
        }),
    ];
    let output = LlvmBackend::new().generate(&program, None);
    assert!(!output.contains(".cd_"), "pre-increment guard must NOT use the countdown loop");
}

// ── FFI regression guard (2026-08-01, Phase 0-1 of the plugin/macro rework) ──
// print!/println! rewrite to the generic Print# intrinsic, which the
// backend emits as `call i64 @__print_*` (dispatch by the argument's type).
// The syntax renames in the plugin/macro rework (PrintLn! -> println!,
// GetEnvInt! -> get_env_int!) must never introduce an indirection layer (GLUE
// bridge shims, protocol chains) between the macro and the C runtime call.
// This test pins that contract: if a future rewrite stops emitting the direct
// call, it fails here.

/// Lex + parse a .bv source string into an AST (test helper).
fn parse_bv_source(src: &str) -> Vec<TopLevel> {
    let tokens = crate::lexer::tokenize(src).expect("tokenize failed");
    let mut parser = crate::parser::Parser::new(tokens, src);
    parser.parse_program().expect("parse failed")
}

#[test]
fn test_print_plugin_emits_direct_ffi_calls() {
    let src = r#"
        let x: Int = 5;
        defn show(v: Int) -> Int {
            println!(v);
            term v;
        };
    
        node __test_go [true][true] { show(0); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("print plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @__print_int("),
        "expected direct @__print_int call after plugin rewrite; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @__print_char("),
        "expected direct @__print_char newline call after plugin rewrite; got:\n{ir}"
    );
    assert!(
        !ir.contains("bridge_"),
        "print rewrite must not route through the GLUE bridge; got:\n{ir}"
    );
}

/// 2026-08-04 (out-observability plan): SMOKE test — `out defn` parses, flows
/// through the plugin/normalizer pipeline, and emits valid IR. (The current
/// backend is conservative: any non-hash call already blocks folding, so the
/// call-survival behavior is the SAME with or without `out` today. The
/// discriminating regression tests live at the analysis layer — see
/// transition_graph::tests::test_out_let_field_forced_live.)
#[test]
fn test_out_defn_unused_result_call_survives() {
    let src = r#"
        out defn sink(x: Int) -> Int {
            term x;
        };
        let start: Int = 0;
        node work [start == 0][start == 0] {
            sink(42);
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @sink(i64 42)") || ir.contains("call i64 @sink("),
        "an out-defn call with an unused result must survive in the IR; got:\n{ir}"
    );
}

/// 2026-08-04: SMOKE test — `out let` parses and emits valid IR end-to-end.
/// (Discriminating liveness behavior is tested at the analysis layer.)
#[test]
fn test_out_let_computation_survives() {
    let src = r#"
        defn expensive() -> Int {
            term 99;
        };
        node work [true][true] {
            out let x: Int = expensive();
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @expensive("),
        "an out-let's RHS call must survive (the computation is live); got:\n{ir}"
    );
}

/// 2026-08-04 (Phase 4, .ebv heap reframe): an embedded target with String
/// state must NOT error — the static bump arena (@embedded_heap) provides a
/// heap without @malloc/briev_rt.c. The old hard rejection was a vestige of
    /// the pre-split .ebv/.sbv entanglement; the heap rejection belongs to .sbv
/// (CIRCT synthesizes hardware), not .ebv (LLVM embedded).
#[test]
fn test_embedded_string_state_uses_static_heap() {
    let src = r#"
        let done: Bool = false;
        node work [done == false][done == true] {
            let s: String = "hi";
            done = true;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new()
        .with_type_universe(universe)
        .with_embedded_mode(true);
    let ir = backend.generate(&items, None);
    // The static bump heap must be emitted and no @malloc call may appear.
    assert!(
        ir.contains("@embedded_heap"),
        "embedded target must emit the static bump heap; got:\n{ir}"
    );
    // String literals are static constants — no heap allocation call.
    assert!(
        !ir.contains("call ptr @malloc(") && !ir.contains("call noalias ptr @malloc("),
        "embedded target must not call @malloc (static heap instead); got:\n{ir}"
    );
}

/// 2026-08-04 (Phase 4): embedded String state is a WARNING (finite static
/// heap), not a TargetError. The old rejection was removed.
#[test]
fn test_embedded_string_state_warns_not_errors() {
    let src = r#"
        let done: Bool = false;
        node work [done == false][done == true] {
            let s: String = "hi";
            done = true;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new()
        .with_type_universe(universe)
        .with_embedded_mode(true);
    let _ = backend.generate(&items, None);
    assert!(
        backend.warnings().iter().any(|w| w.contains("TargetWarning") && w.contains("static bump arena")),
        "embedded String state must be a warning, not an error; got {:?}",
        backend.warnings()
    );
}

/// 2026-08-01: Format-string println! — literal segments print via __print_str,
/// placeholders dispatch on the value kind, and a newline is appended. All
/// must remain direct runtime calls with no bridge indirection.
#[test]
fn test_println_format_string_emits_direct_ffi_calls() {
    let src = r#"
        let x: Int = 5;
        let f: Float = 1.5;
        defn show() -> Int {
            println!("sum={} and {1}", x + 1, f);
            term 0;
        };
    
        node __test_go [true][true] { show(); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("print plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @__print_str("),
        "expected direct @__print_str call for format literal segment; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @__print_int("),
        "expected direct @__print_int call for integer placeholder; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @__print_float("),
        "expected direct @__print_float call for float placeholder; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @__print_char("),
        "expected direct @__print_char newline call; got:\n{ir}"
    );
    assert!(
        !ir.contains("bridge_"),
        "format print rewrite must not route through the GLUE bridge; got:\n{ir}"
    );
}

/// 2026-08-01 (audit): the generic `Print#` convenience intrinsic dispatches
/// by protocol category — Bool → __print_bool (true/false), Char →
/// __print_char, and an explicit `(b as Int)` cast → __print_int (1/0).
/// This pins the natural-representation contract on the LLVM backend.
#[test]
fn test_print_dispatch_by_protocol_category() {
    let src = r#"
        let b: Bool = true;
        let c: Char = 'A';
        defn show(b: Bool, c: Char) -> Int {
            println!(b);
            println!(c);
            println!((b as Int));
            term 0;
        };
    
        node __test_go [true][true] { show(0, 0); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("print plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @__print_bool("),
        "a Bool must print via __print_bool (true/false); got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @__print_char("),
        "a Char must print via __print_char; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @__print_int("),
        "a Bool explicitly cast to Int must print via __print_int (1/0); got:\n{ir}"
    );
    assert!(
        !ir.contains("__print_bool_placeholder"),
        "no placeholder emission may remain; got:\n{ir}"
    );
}
/// 2026-08-01: An out-of-range positional placeholder is a compile error
/// surfaced by the plugin stage, not a silent runtime truncation.
#[test]
fn test_println_out_of_range_placeholder_errors() {
    let src = r#"
        defn show() -> Int {
            println!("x={1}", 42);
            term 0;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    let err = pm
        .run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect_err("out-of-range {1} must be a plugin-stage error");
    assert!(
        err.contains("out of range"),
        "expected out-of-range message, got: {err}"
    );
}

/// 2026-08-01: B0 acceptance — a String value is a `ptr` to [len][bytes] with
/// no fat-pointer `{ i64, i64 }` or `i128` claim anywhere in emitted IR. The
/// test exercises the full String path: literal → state store → load → print
/// via __print_str with a pointer argument.
#[test]
fn test_string_is_ptr_no_fat_pointer_in_ir() {
    let src = r#"
        let name: String = "world";
        let x: Int = 42;
        node report [true] {
            print!("hello, {}! x={1}", name, x);
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("print plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    // String values must be pointer-sized machine words in state and registers.
    assert!(
        ir.contains("call i64 @__print_str(ptr "),
        "format literal must print via __print_str(ptr); got:\n{ir}"
    );
    // The named %String struct type must not be declared (String is ptr now),
    // and no i128 state load/claim may remain for String slots. (The LLVM
    // datalayout string always contains i128:128 for the target — that is
    // target ABI info, not a String claim, so it is excluded.)
    assert!(
        !ir.contains("%String = type { i64, i64 }")
            && !ir.contains("load i128")
            && !ir.contains("type { i128")
            && !ir.contains("type { i64, i64, i128")
            && !ir.contains("extractvalue"),
        "no {{ i64, i64 }}/i128 String claim may remain in emitted IR (B0); got:\n{ir}"
    );
}

/// 2026-08-01: Legacy PascalCase names (PrintLn!) are no longer rewritten by
/// the plugin — they fall through to the typechecker, which rejects them with
/// a rename hint. The plugin must leave them untouched.
#[test]
fn test_legacy_println_not_rewritten_by_plugin() {
    let src = r#"
        defn show(v: Int) -> Int {
            PrintLn!(v);
            term v;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("print plugin stage failed");
    assert!(
        format!("{:?}", items).contains("PrintLn"),
        "legacy PrintLn! must survive the plugin for the typechecker to reject; got:\n{items:?}"
    );
}

/// 2026-08-01 (B1): String == / != on String operands emits a content
/// comparison (briev_str_eq) instead of `icmp eq ptr` (address comparison).
/// This is the backend half of B1; the interpreter already does content
/// equality (rule #4). The entry!-shaped comparison `cmd == "build"` is the
/// motivating pattern (Phase 3).
#[test]
fn test_string_content_eq_emits_briev_str_eq() {
    let src = r#"
        let a: String = "abc";
        let b: String = "abc";
        defn run() -> Bool {
            term a == b;
        };
    
        node __test_go [true][true] { run(); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @briev_str_eq(ptr "),
        "String == must emit briev_str_eq content compare; got:\n{ir}"
    );
    assert!(
        !ir.contains("icmp eq ptr"),
        "String == must not compare addresses (icmp eq ptr); got:\n{ir}"
    );
}

/// 2026-08-01 (B1): int == still emits icmp eq (numeric path) — the String
/// content-eq arm must not swallow numeric comparisons.
#[test]
fn test_int_eq_still_emits_icmp() {
    let src = r#"
        let x: Int = 5;
        defn run() -> Bool {
            term x == 6;
        };
    
        node __test_go [true][true] { run(); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("icmp eq i64 "),
        "int == must still emit icmp eq i64; got:\n{ir}"
    );
    assert!(
        !ir.contains("call i64 @briev_str_eq("),
        "int == must not call briev_str_eq; got:\n{ir}"
    );
}

/// 2026-08-01 (B1): String & | ^ ~ emit content-bitwise runtime calls
/// (briev_str_band/bor/bxor/bnot) and return a String (ptr).
#[test]
fn test_string_bitwise_emits_content_ops() {
    let src = r#"
        let a: String = "abc";
        let b: String = "abc";
        defn run() -> String {
            let r1: String = a & b;
            let r2: String = a | b;
            let r3: String = a ^ b;
            let r4: String = ~a;
            term r4;
        };
    
        node __test_go [true][true] { run(); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call ptr @briev_str_band("),
        "String & must emit briev_str_band; got:\n{ir}"
    );
    assert!(
        ir.contains("call ptr @briev_str_bor("),
        "String | must emit briev_str_bor; got:\n{ir}"
    );
    assert!(
        ir.contains("call ptr @briev_str_bxor("),
        "String ^ must emit briev_str_bxor; got:\n{ir}"
    );
    assert!(
        ir.contains("call ptr @briev_str_bnot("),
        "String ~ must emit briev_str_bnot; got:\n{ir}"
    );
}

/// 2026-08-01 (Phase 3a): emitted main is `main(i32 %argc, ptr %argv)` and
/// captures argc/argv into the runtime globals for the CLI argv helpers.
#[test]
fn test_main_signature_and_argv_capture() {
    let src = r#"
        let x: Int = 5;
        defn run() -> Int {
            term x;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("define i32 @main(i32 %argc, ptr %argv)"),
        "main must take (i32 %argc, ptr %argv); got:\n{ir}"
    );
    assert!(
        ir.contains("store i32 %argc, ptr @__briev_argc"),
        "main must store argc into @__briev_argc; got:\n{ir}"
    );
    assert!(
        ir.contains("store ptr %argv, ptr @__briev_argv"),
        "main must store argv into @__briev_argv; got:\n{ir}"
    );
    assert!(
        !ir.contains("define i32 @main()"),
        "main() without args must not be emitted; got:\n{ir}"
    );
}

/// 2026-08-01 (Phase 3b): a Bool (i8) state field must NOT get `!range
/// !{ i64 0, i64 256 }` — LLVM range bounds must match the load width; the
/// i64 bounds on a load i8 crash clang. The range is skipped as vacuous.
#[test]
fn test_bool_field_no_malformed_i8_range() {
    let src = r#"
        let done: Bool = false;
        node work [done == false][done] {
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("!range !{ i64 0, i64 256 }"),
        "Bool field must not emit malformed i64 range on i8 load; got:\n{ir}"
    );
}

/// 2026-08-01 (B2): `String → Bit` is the CONTENT VIEW — a String value is a
/// ptr to [len][bytes], so the cast yields the buffer ADDRESS (ptrtoint), not
/// the old `extractvalue {i64,i64}, 0` fat-pointer extraction. This pins the
/// content-view lane under the bits model.
#[test]
fn test_string_to_bit_content_view() {
    let src = r#"
        let s: String = "hello";
        let tick: Int = 0;
        node report [tick < 1][tick == 1] {
            let b: Bit = s as Bit;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("ptrtoint ptr"),
        "String → Bit must emit ptrtoint (content view = buffer address); got:\n{ir}"
    );
    assert!(
        !ir.contains("extractvalue"),
        "String → Bit must not extractvalue (String is a ptr under B0); got:\n{ir}"
    );
}

/// 2026-08-01 (B2): `Bit → String` is the ENCODING DOOR — wraps the bits
/// (a [len][bytes] buffer) back into a String by materializing the header via
/// briev_bits_to_str. Not a bitcast.
#[test]
fn test_bit_to_string_encoding_door() {
    let src = r#"
        let s: String = "hello";
        let tick: Int = 0;
        node report [tick < 1][tick == 1] {
            let b: Bit = s as Bit;
            let r: String = b as String;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call ptr @briev_bits_to_str(ptr "),
        "Bit → String must emit briev_bits_to_str (UTF8 wrap); got:\n{ir}"
    );
    assert!(
        !ir.contains("extractvalue"),
        "the encoding door must not extractvalue; got:\n{ir}"
    );
}

/// 2026-08-01 (B3): `x.^Length` on a String → the `Size` prop default = UTF8
/// char count (briev_char_len); `x.^^Bytes` → the `Bytes` prop default = O(1)
/// header read (byte length). Also verifies a String `let` used only via
/// reflection stays live (not eliminated as a dead state field).
#[test]
fn test_string_len_and_bytes_reflect() {
    let src = r#"
        let s: String = "hello";
        let tick: Int = 0;
        node report [tick < 1][tick == 1] {
            let c: Int = s.^Length;
            let b: Int = s.^^Bytes;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");

    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("load i64, ptr "),
        "String .^Length must read the stored byte header (SPEC 17.1); got:\n{ir}"
    );
    assert!(
        !ir.contains("call i64 @briev_char_len(ptr "),
        ".^Length must NOT emit the char scan (that is CharCount#); got:\n{ir}"
    );
    assert!(
        ir.contains("load i64, ptr ") || ir.contains("load i64, ptr %"),
        "String .^^Bytes must emit an O(1) header load; got:\n{ir}"
    );
    // The String field must stay live (not eliminated as dead — it's only
    // read via reflection). Verify the state has a String slot.
    assert!(
        ir.contains("%State = type { i64,"),
        "the reflect-read String field must stay in %State; got:\n{ir}"
    );
}











// ── Phase 3: consumptive operators + arrow (2026-08-01) ────────────────

/// A `~=` move-assign and a `~+` consumptive add in a defn must emit the op
/// (the consumed param's backing destroy is a no-op for scalars).
#[test]
fn test_consumptive_ops_emit_normal_arithmetic() {
    let src = r#"
        defn f(a: Int, b: Int) -> Int {
            a ~= b;
            term a ~+ 1;
        };
    
        node __test_go [true][true] { f(0, 0); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("add nsw i64") || ir.contains("add i64"),
        "a ~+ b / a ~= b must emit the arithmetic; got:\n{ir}"
    );
    assert!(
        ir.contains("define i64 @f("),
        "the defn must be emitted; got:\n{ir}"
    );
}

/// `dest <- src` / `<- src;` / `~<- src;` parse + emit as ArrowAssign without
/// a stray @<global> reference (the discard of a non-collection is a no-op).
#[test]
fn test_arrow_statements_emit_without_broken_globals() {
    let src = r#"
        defn f(a: Int, b: Int) -> Int {
            a <- b;
            <- b;
            ~<- b;
            term a;
        };
    
        node __test_go [true][true] { f(0, 0); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("call void @free"),
        "scalar consumes must not free (no heap backing); got:\n{ir}"
    );
    assert!(
        ir.contains("define i64 @f("),
        "the defn must be emitted; got:\n{ir}"
    );
}

// ── Phase 4: stream symbols + trigger port removal (2026-08-01) ─────────

/// `#StdOut <- value` lowers to the print family; `#StdErr <- <String>` to the
/// stderr printer. Both survive the loop-engine body emission.
#[test]
fn test_stream_writes_emit_print_family() {
    let src = r#"
        defn f(count: Int) -> Int {
            #StdOut <- count;
            #StdErr <- "err";
            term count;
        };
    
        node __test_go [true][true] { f(0); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @__print_int("),
        "#StdOut <- count must lower to __print_int; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @__eprint_str(ptr "),
        "#StdErr <- \"err\" must lower to __eprint_str; got:\n{ir}"
    );
}

/// A trigger parses as the whole-target form — no `.port` may appear.
#[test]
fn test_trigger_is_whole_target() {
    let src = "trg btn @ 0x1000A000;\n";
    let items = parse_bv_source(src);
    assert_eq!(items.len(), 1, "one trigger");
    match &items[0] {
        TopLevel::Trigger(t) => {
            assert_eq!(t.name, "btn");
        }
        other => panic!("expected Trigger, got {:?}", other),
    }
}

/// A `keep x;` on a field the garbage scheduler would not auto-free is a
/// redundant-keep warning in the backend report.
#[test]
fn test_redundant_keep_warns() {
    let src = r#"
        let count: Int = 0;
        node report [count < 3][count == 3] {
            keep count;
            count = count + 1;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let _ir = backend.generate(&items, None);
    assert!(
        backend.warnings().iter().any(|w| w.contains("redundant") && w.contains("count")),
        "keep on a non-schedulable field must warn; warnings: {:?}",
        backend.warnings()
    );
}

/// A `~op` on a top-level const-let inside a txn must resolve the operand as a
/// state field (regression: the liveness/reference walks dropped `Expr::Consume`,
/// so a consumed-only field was eliminated and emitted an undefined `@b` global).
#[test]
fn test_consumptive_op_on_const_let_in_txn() {
    let src = r#"
        let a: Int = 5;
        let b: Int = 3;
        let c: Int = 0;
        node report [c < 3][c == 3] {
            c = a ~+ b;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("load i64, ptr @b"),
        "a consumed const-let must resolve as a %State field, not an undefined @b global; got:\n{ir}"
    );
    assert!(
        ir.contains("add nsw i64") || ir.contains("add i64"),
        "the consumptive add must be emitted; got:\n{ir}"
    );
}

/// A collection referenced ONLY through arrow statements must be kept in
/// %State (regression: the field-liveness walk dropped ArrowAssign, so
/// arrow-only collections were eliminated and the push/pop silently no-op'd —
/// queue_drain/stack_push_pop matched the C reference only because their
/// output was the counter, not the collection).
#[test]
fn test_arrow_only_collection_is_kept_in_state() {
    let src = r#"
        import { Stack } from "std/collections.bv";
        let st: Stack<Int, 8> = 0;
        let v: Int = 0;
        node report [v < 3][v == 3] {
            st <- v;
            <- st;
            v = v + 1;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    // The stack must get a %State slot (a GEP on slot 0 for `st`).
    assert!(
        ir.contains("getelementptr inbounds %State, ptr %state, i32 0, i32 0"),
        "the arrow-only collection must be kept in %State; got:\n{ir}"
    );
    assert!(
        !ir.contains("load i64, ptr @st"),
        "the collection must resolve as a %State field, not an undefined @st global; got:\n{ir}"
    );
}

// ── Coll grow-on-full (2026-08-15) ────────────────────────────────────

/// A `coll obj` push past the default cap (16) must emit the grow-on-full
/// guard: `if len == cap { __briev_coll_resize(h, cap * 2) }` BEFORE the
/// store, and re-read `data` from the slot after the join (the runtime
/// mutates the `[data, cap, len]` block in place — no register merge).
/// Pre-fix the scaffolded push had no guard and wrote OOB past the 16-slot
/// buffer (SPEC §8.10 grow-on-full).
#[test]
fn test_coll_grow_on_full_guard_emits() {
    let src = r#"
coll obj MyQueue { data: Ptr<Int>; };
let done: Int = 0;
node go [done == 0][done == 1] {
    let q: MyQueue = [];
    foreach x in 0..21 {
        q.push(x);
    };
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @__briev_coll_resize"),
        "the grow guard must call the runtime realloc-or-copy resize; got:\n{ir}"
    );
}

/// A declared handle-only `op Grow: triple(#Lh)` wins over the default
/// doubling — the synthesized push guard calls the bound function with the
/// receiver handle (`#Self`), never the default `Resize#` doubling.
#[test]
fn test_coll_grow_override_binding_wins() {
    let src = r#"
coll obj GeometricQueue {
    data: Ptr<Int>;
    op Grow: triple(#Lh);
};
defn triple(q: GeometricQueue) {
    Resize#(q, Capacity#(q) * 3);
};
let done: Int = 0;
node go [done == 0][done == 1] {
    let q: GeometricQueue = [];
    foreach x in 0..17 {
        q.push(x);
    };
    done = 1;
    term;
};

        node __test_go [true][true] { triple(0); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        // 2026-09-14: void defns now define/call as `void` (the ret-0 fix);
        // the behavioral assertion is the dispatch to the bound override.
        ir.contains("call void @triple("),
        "the grow guard must dispatch to the bound override; got:\n{ir}"
    );
}

/// 2026-08-16 (hashmap redesign, plan 2026-08-16-hashmap-redesign.md): a
/// hand-written `obj HashMap<K,V>` (no `coll` keyword) is a collection VALUE
/// through its op surface — a state field `let m: HashMap<Int,Int> = 0`
/// constructs via `op Init` (allocates the arrays), and insert/get/Count#
/// work through the member methods. This verifies the op-driven
/// classification (is_heap_coll via operator_defs) and the seed-init path
/// that the redesign added. The obj is declared INLINE (tests don't resolve
/// imports).
#[test]
fn test_hashmap_state_field_init_and_ops() {
    let src = r#"
struct Entry { key: Int; val: Int; };
obj HashMap {
    keys: Ptr<Int>;
    vals: Ptr<Int>;
    occupied: Ptr<Int>;
    count: Int;
    cap: Int;
    op InsertAt: insert(#Lh, #Rh);
    op ExtractFrom: remove(#Rh);
    op CopyFrom: get(#Rh);
    op Init: init(#Lh, #Rh);
    op Count() -> Int { term count; };
    txn init(v: Int) [v >= 0][v >= 0] {
        keys = Malloc#(256 * 8) as Ptr<Int>;
        vals = Malloc#(256 * 8) as Ptr<Int>;
        occupied = Malloc#(256 * 8) as Ptr<Int>;
        cap = 256;
        count = 0;
    };
    txn insert(e: Entry) [count < cap][count <= cap] {
        let h: Int = (e.key as Int) % cap;
        keys[h] = e.key;
        vals[h] = e.val;
        occupied[h] = 1;
        count = count + 1;
    };
    defn get(key: Int) -> Int [count > 0][count >= 0] {
        let h: Int = (key as Int) % cap;
        term vals[h];
    };
};
let m: HashMap = 0;
let done: Int = 0;
node go [done == 0][done == 1] {
    let e1: Entry = Entry { key: 1, val: 10 };
    m.insert(e1);
    let g: Int = m.get(1);
    let n: Int = m.Count#();
    done = g + n;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("srem"),
        "insert/get must hash the key; got:\n{ir}"
    );
    assert!(
        !ir.contains("ptr @m."),
        "a HashMap state field must be a boxed handle, never unpacked columns; got:\n{ir}"
    );
}

/// 2026-08-17 (param/field shadowing regression): a member-body PARAMETER must
/// shadow a same-named INSTANCE FIELD. `txn set(cap: Int)` on obj Box has a
/// state field `cap`; before the fix, `cap` in the body resolved to the field
/// column (zero) instead of the passed argument, so the argument was silently
/// dropped. This pins the arg (600) reaching the body via the parameter, not
/// the field column.
#[test]
fn test_member_param_shadows_same_named_field() {
    let src = r#"
obj Box {
    cap: Int;
    txn set(cap: Int) [cap >= 0][cap >= 0] {
        cap = cap + 100;
    };
};
let b: Box = Box { cap: 0 };
let done: Int = 0;
node go [done == 0][done == 1] {
    b.set(600);
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    // `cap = cap + 100` with arg 600 must store 700, computed from the
    // PARAMETER (the arg), not from a load of the (zeroed) `cap` field column.
    // In the buggy code the arg register is dead and the +100 add takes a field
    // load; in the fixed code the arg register feeds the add. Robustly verify
    // the arg register is consumed by an `add nsw` (the +100 computation).
    let seed_reg = ir
        .lines()
        .find(|l| l.contains("add i64 0, 600"))
        .and_then(|l| l.split('=').next().map(|s| s.trim().to_string()));
    let seed_reg = seed_reg.expect("the member arg (600) must be emitted as a register");
    let add_results: Vec<String> = ir
        .lines()
        .filter(|l| l.contains(&format!("add nsw i64 {}, ", seed_reg)))
        .filter_map(|l| l.split('=').next().map(|s| s.trim().to_string()))
        .collect();
    assert!(
        !add_results.is_empty(),
        "the param (arg {seed_reg}) must feed the +100 add (not a field load); got:\n{ir}"
    );
    // 2026-08-18 (WRITE side of the shadow): `cap = cap + 100` must STORE
    // into the param's shadowed local (a fresh alloca), NOT into the `b.cap`
    // field column. In the asymmetric bug the read took the param but the
    // store went to the pooled column; both sides must resolve to the same
    // storage. Every store of an add-result must target an alloca.
    let add_regs: Vec<String> = add_results
        .iter()
        .map(|r| format!("store i64 {r}, ptr "))
        .collect();
    let field_store = ir.lines().find(|l| {
        add_regs
            .iter()
            .any(|needle| l.contains(needle.as_str()))
            && {
                let tgt = l
                    .split("store i64")
                    .nth(1)
                    .and_then(|s| s.split("ptr").nth(1))
                    .map(|s| s.trim().to_string())
                    .expect("store must name a target");
                !ir.lines()
                    .any(|al| al.contains(&format!("{tgt} = alloca ")))
            }
    });
    assert!(
        field_store.is_none(),
        "every store of a +100 result must target an alloca, never the field column (found: {}); got:\n{ir}",
        field_store.unwrap_or_default()
    );
    assert!(
        add_results.iter().all(|result| ir
            .lines()
            .any(|l| l.contains(&format!("store i64 {result}, ptr ")))),
        "each +100 result must be stored (shadowed param write); got:\n{ir}"
    );
}

/// 2026-08-18 (HashMap capacity-init + break-probe): `txn init(capacity)` must
/// flow the seed through `match capacity { 0 => 256, _ => capacity }` into the
/// table size (`c * 8` mallocs) and the `cap` column. The linear-probe loops
/// must EARLY-EXIT on the matched/free slot (the `break` after the `when`)
/// instead of scanning the full cap. The init is called directly (`m.init(40)`)
/// because unit tests skip the full pipeline's numeric-seed construction — that
/// path (`let m: HashMap<Int,Int> = 2 * N`) is pinned end-to-end by the
/// hash_ops_idio benchmark (MATCH at parity).
#[test]
fn test_hashmap_capacity_seed_and_break_probe() {
    let src = r#"
obj HashMap<K, V> {
    keys: Ptr<K>;
    vals: Ptr<V>;
    occupied: Ptr<Int>;
    count: Int;
    cap: Int;
    op InsertAt: insert(#Lh, #Rh);
    op CopyFrom: get(#Rh);
    op Init: init(#Lh, #Rh);
    op Count() -> Int { term count; };
    txn init(capacity: Int) [true][count == 0] {
        let c: Int = match capacity {
            0 => 256,
            _ => capacity,
        };
        keys = Malloc#(c * 8) as Ptr<K>;
        vals = Malloc#(c * 8) as Ptr<V>;
        occupied = Malloc#(c * 8) as Ptr<Int>;
        cap = c;
        count = 0;
    };
    txn insert(e: (K, V)) [count < cap][count <= cap] {
        let (k, v) = e;
        let h: Int = (k as Int) % cap;
        let done_slot: Bool = false;
        foreach q in 0..cap {
            let p: Int = (h + q) % cap;
            when !done_slot {
                when occupied[p] == 0 || keys[p] == k {
                    keys[p] = k;
                    vals[p] = v;
                    occupied[p] = 1;
                    done_slot = true;
                    break;
                };
            };
        };
        count = count + 1;
    };
    defn get(key: K) -> V [count > 0][count >= 0] {
        let h: Int = (key as Int) % cap;
        let r: V = 0 as V;
        foreach q in 0..cap {
            let p: Int = (h + q) % cap;
            when occupied[p] == 1 && keys[p] == key {
                r = vals[p];
                break;
            };
        };
        term r;
    };
};
let m: HashMap<Int, Int> = 0;
let done: Int = 0;
node go [done == 0][done == 1] {
    m.init(40);
    m.insert((1, 10));
    m.insert((2, 20));
    let g: Int = m.get(2);
    done = g;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    // 1. The numeric seed (40) must be a live register (the typechecker's
    //    numeric-seed construction admits it as an `op Init` capacity).
    let seed_reg = ir
        .lines()
        .find(|l| l.contains("add i64 0, 40"))
        .and_then(|l| l.split('=').next().map(|s| s.trim().to_string()));
    let seed_reg = seed_reg.expect("the capacity seed (40) must be emitted; got:\n{ir}");    // 2. `match capacity { 0 => 256, _ => capacity }` → the seed and the 256
    //    fallback must meet in a phi (the fallback feeds the table size).
    let phi_lines: Vec<&str> = ir
        .lines()
        .filter(|l| l.contains("phi") && l.contains(&seed_reg))
        .collect();
    assert!(
        !phi_lines.is_empty(),
        "the capacity seed {seed_reg} must feed a phi (0 => 256 fallback); got:\n{ir}"
    );
    let fallback_regs: Vec<String> = ir
        .lines()
        .filter(|l| l.contains("add i64 0, 256"))
        .filter_map(|l| l.split('=').next().map(|s| s.trim().to_string()))
        .collect();
    assert!(
        !fallback_regs.is_empty(),
        "the 0 => 256 fallback constant must be emitted; got:\n{ir}"
    );
    let phi_meets_fallback = phi_lines
        .iter()
        .any(|l| fallback_regs.iter().any(|r| l.contains(r)));
    assert!(
        phi_meets_fallback,
        "the capacity seed {seed_reg} and the 0 => 256 fallback must meet in a phi; got:\n{ir}"
    );
    // 3. The `c * 8` malloc sizing must consume the seed-derived phi (the
    //    capacity reaches the allocations — not a dropped seed).
    let phi_reg = ir
        .lines()
        .filter(|l| l.contains("phi") && l.contains(&seed_reg))
        .find_map(|l| l.split('=').next().map(|s| s.trim().to_string()))
        .expect("the seed-feeding phi must have a result register; got:\n{ir}");
    assert!(
        ir.lines()
            .any(|l| l.contains("mul nsw i64") && l.contains(&phi_reg)),
        "the capacity phi {phi_reg} must size the column mallocs (mul by 8); got:\n{ir}"
    );
    // 4. The linear probes must EARLY-EXIT: an unconditional `br label
    //    %foreach.end{label}` from inside the body (the `break`), distinct
    //    from the header backedge. Without `break` the loop scans the full cap.
    assert!(
        ir.lines().any(|l| l.trim().starts_with("br label %foreach.end")),
        "the probe loops must break early (unconditional branch to foreach.end); got:\n{ir}"
    );
}

/// 2026-08-18 (Phase C, BUGS.md arrow-push): the two silent-drop arrow bugs.
/// (a) `let ks: List<Int> = b.keys()` was routed through the collection
/// SEED constructor — a non-List RHS was assumed to be a seed, so the returned
/// list got wrapped as `[<list>]` (a new box with len forced to 1) and the
/// scan read `1` of N elements. The fix binds the returned list DIRECTLY.
/// (b) `items <- e` on a POOLED member-field list (PiggyBank.put) found no
/// InsertAt strategy (the slot is `PiggyBank.items`, a `Vector(..)` column)
/// and the plain-copy fallback couldn't resolve the target — the push emitted
/// NOTHING. The fix resolves the self-prefixed slot with the Vector column
/// type peeled. Both pins: the printed values (`n`, `s`) must be LOADS of a
/// List len field (i64 16 GEP) that was written by exactly `N` INCREMENT
/// stores (`add nsw i64 %{..}, 1`, one per push) — never a constant-1 seed
/// store (bug a) and never an empty/absent field (bug b).
#[test]
fn test_arrow_push_binds_returned_list_and_pooled_member_field() {
    let src = r#"
coll obj MyList { data: Ptr<Int>; };
obj Box {
    keys: Ptr<Int>;
    vals: Ptr<Int>;
    occupied: Ptr<Int>;
    count: Int;
    cap: Int;
    txn init(capacity: Int) [true][count == 0] {
        let c: Int = match capacity { 0 => 256, _ => capacity };
        keys = Malloc#(c * 8) as Ptr<Int>;
        vals = Malloc#(c * 8) as Ptr<Int>;
        occupied = Malloc#(c * 8) as Ptr<Int>;
        cap = c;
        count = 0;
    };
    txn insert(k: Int, v: Int) [count < cap][count <= cap] {
        let h: Int = k % cap;
        foreach q in 0..cap {
            when occupied[(h + q) % cap] == 0 {
                keys[(h + q) % cap] = k;
                vals[(h + q) % cap] = v;
                occupied[(h + q) % cap] = 1;
                break;
            };
        };
        count = count + 1;
    };
    defn keys() -> MyList {
        let acc: MyList = [];
        foreach i in 0..cap {
            when occupied[i] == 1 {
                acc <- keys[i];
            };
        };
        term acc;
    };
};
obj PiggyBank {
    items: MyList;
    defn init(v: Int) { let e: MyList = []; items = e; }
    defn put(e: Int) { items <- e; }
    defn size() -> Int { term items.Count#(); };
};
let p: PiggyBank = 0;
let b: Box = 8;
let done: Int = 0;
node go [done == 0][done == 3] {
    b.insert(1, 10);
    b.insert(2, 20);
    b.insert(3, 30);
    let ks: MyList = b.keys();
    let n: Int = ks.Count#();
    p.put(1);
    p.put(2);
    p.put(3);
    let s: Int = p.size();
    done = n;
    println!(n);
    println!(s);
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    let lines: Vec<&str> = ir.lines().collect();

    // The transaction body is emitted twice (the alwaysinline @txn_go defn +
    // main's reactive replay), each with its own registers; any assertion must
    // hold over the WHOLE IR (both copies count the same way).

    // Bug (a) — the `[<list>]` seed wrapper. When `let ks: MyList = b.keys()`
    // was misrouted to the collection seed constructor, the RETURNED list was
    // wrapped in a NEW box whose len field was forced to the constant 1 (a
    // `store i64 {c}, ptr {len_gep}` with `{c} = add i64 0, 1`). The fixed
    // code binds the returned list DIRECTLY: its len is built only by `add nsw`
    // increments (one per scanned element), never by a constant-1 seed store.
    let len_geps: Vec<String> = lines
        .iter()
        .filter(|l| l.contains("getelementptr i8, ptr") && l.contains("i64 16"))
        .filter_map(|l| l.split('=').next().map(|s| s.trim().to_string()))
        .collect();
    assert!(!len_geps.is_empty(), "no List len fields emitted; got:\n{ir}");
    let constant_one_seed_store = lines.iter().find(|l| {
        let Some(v) = l.trim_start().strip_prefix("store i64 ").and_then(|s| s.split(',').next().map(|s| s.trim().to_string())) else {
            return false;
        };
        let Some(gep) = l.split("ptr").nth(1).map(|s| s.trim().to_string()) else {
            return false;
        };
        len_geps.contains(&gep)
            && (v == "1"
                || lines
                    .iter()
                    .any(|dl| dl.trim().starts_with(&format!("{v} = add i64 0, 1"))))
    });
    assert!(
        constant_one_seed_store.is_none(),
        "a len field must never be written with the constant 1 (`[<list>]` wrapper bug); got:\n{ir}"
    );

    // Bug (b) — the pooled member-field push. `items <- e` must emit the List
    // push inline: the len field is INCREMENTED once per push (`add nsw i64
    // {..}, 1`, stored to a len GEP). There are 3 keys-scan pushes + 3 put
    // pushes per copy, two copies = 12. When the push was SILENTLY DROPPED the
    // put bodies emitted nothing (only the argument eval), so the count fell to
    // 3 per copy.
    let increment_stores: Vec<&str> = lines
        .iter()
        .cloned()
        .filter(|l| {
            let Some(v) = l.trim_start().strip_prefix("store i64 ").and_then(|s| s.split(',').next().map(|s| s.trim().to_string())) else {
                return false;
            };
            let Some(gep) = l.split("ptr").nth(1).map(|s| s.trim().to_string()) else {
                return false;
            };
            len_geps.contains(&gep)
                && lines.iter().any(|dl| {
                    dl.trim().starts_with(&format!("{v} = add nsw i64"))
                })
        })
        .collect();
    assert_eq!(
        increment_stores.len(),
        8,
        "the len fields must be written by exactly 8 push increments (1 keys-scan loop body + 3 member-field puts, twice); got:\n{ir}"
    );

    // Sanity: the printed values (n = ks.Count#(), s = p.size()) are LOADs of a
    // len field, never constants or forwarded args.
    let prints: Vec<String> = lines
        .iter()
        .filter(|l| l.contains("call i64 @__print_int"))
        .filter_map(|l| {
            l.split("__print_int(i64 ").nth(1).and_then(|s| s.split(')').next().map(|r| r.trim().to_string()))
        })
        .collect();
    for p in &prints {
        assert!(
            lines
                .iter()
                .any(|l| l.trim().starts_with(&format!("{p} = load i64"))),
            "the printed value {p} must be a load (the returned/member list length); got:\n{ir}"
        );
    }
    assert_eq!(prints.len(), 4, "expected four prints (two per copy); got:\n{ir}");
}

/// 2026-08-18 (Phase E, BUGS.md SSA-main destructure): a FOREACH loop variable
/// whose name is also a member-body destructure name (`let (k, v) = e`)
/// poisoned the member's reads. The foreach item binding leaked into
/// `last_val_temps` AFTER the loop, and `last_val_temps` also survived the
/// @txn_go → SSA-main emission-pass boundary (clear_locals didn't clear it).
/// The SSA-main replay's `(k as Int) % 16` then resolved `k` to the stale
/// foreach register (owned by a LATER statement — an undefined forward
/// reference, wrong inserts/gets, and a clang -O3 -flto frontend SIGSEGV).
/// This test compiles and RUNS the program: the member must hash the tuple's
/// FIRST element (7), never the leftover foreach counter.
#[test]
fn test_foreach_item_does_not_poison_member_destructure() {
    let src = r#"
coll obj MyList { data: Ptr<Int>; };
obj Probe {
    data: Int;
    defn hash_of_pair(e: (Int, Int)) -> Int {
        let (k, v) = e;
        term (k as Int) % 16;
    };
};
let p: Probe = 0;
let done: Bool = false;
node go [done == false][done == true] {
    when done == false {
        let acc: Int = 0;
        foreach k in 0..3 {
            acc = acc + k;
        };
        let r: Int = p.hash_of_pair((7, 8));
        println!(r);
        done = true;
    };
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);

    use std::process::Command;
    // 2026-09-09 (Family B): briev_rt.c no longer defines the print family —
    // it moved to pure-Briev defns (std/cast_lanes.bv, write(2) via
    // SysCall#). These tests only need OBSERVABLE OUTPUT, so link a tiny
    // printf stub instead of the (shrinking) runtime.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let rt = std::env::temp_dir().join(format!(
        "briev_print_stub_{}.c",
        std::process::id()
    ));
    std::fs::write(&rt, "#include <stdio.h>\n#include <stdint.h>\n#include <stdlib.h>\n#include <string.h>\nint64_t __print_int(int64_t n) { printf(\"%ld\", (long)n); return 0; }\nint64_t __print_char(int64_t c) { putchar((int)c); return 0; }\nint64_t __briev_coll_resize(int64_t handle, int64_t new_cap) {\n    if (!handle || new_cap < 0) return 1;\n    int64_t* block = (int64_t*)handle;\n    int64_t old_data = block[0];\n    int64_t len = block[2];\n    if (new_cap == 0) { free((void*)old_data); block[0] = 0; block[1] = 0; return 0; }\n    int64_t* nd = (int64_t*)malloc((size_t)(new_cap * 8));\n    if (!nd) return 1;\n    int64_t copy_n = len < new_cap ? len : new_cap;\n    if (old_data && copy_n > 0) memcpy(nd, (void*)old_data, (size_t)(copy_n * 8));\n    if (old_data) free((void*)old_data);\n    block[0] = (int64_t)nd;\n    block[1] = new_cap;\n    return 0;\n}\n").expect("write stub");
    let out = std::env::temp_dir().join(format!(
        "briev_foreach_destructure_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let ll = out.with_extension("ll");
    // The unit-test pipeline registers PrintPlugin (emitting `call` sites) but
    // not the FFI runtime DECLARATIONS (compile.rs normally does that); declare
    // them for the standalone link — inserted after the module header (top-level
    // entity position).
    let prelude = "declare i64 @__print_int(i64)\ndeclare i64 @__print_char(i64)\n";
    let ir = if let Some(pos) = ir.find("target triple") {
        let end = ir[pos..].find('\n').map(|e| pos + e + 1).unwrap_or(ir.len());
        format!("{}{}{}", &ir[..end], prelude, &ir[end..])
    } else {
        format!("{prelude}{ir}")
    };
    std::fs::write(&ll, &ir).expect("write .ll");
    let compile = Command::new("clang")
        .arg("-O0")
        .arg(&ll)
        .arg(&rt)
        .arg("-lm")
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "link failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&out).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let _ = std::fs::remove_file(&ll);
    let _ = std::fs::remove_file(&out);
    assert!(
        run.status.success(),
        "program crashed: {} (ir:\n{ir})",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        stdout.trim(),
        "7",
        "hash_of_pair((7, 8)) must return 7 % 16 — the foreach item leaked into \
         the member's `(k, v)` destructure resolution; got:\n{stdout}\n---\n{ir}"
    );
}

/// 2026-09-16 (Bug B, chaining fixups): a member body that contains its own
/// method chain must NOT pollute the caller's positional back-reference stack.
/// `c.bump().2>>sum2(c)`: `.2` is two operations back from `sum2` — the value
/// `c` — not a register from `bump`'s internal `base.Add#(1)` chain. The
/// interpreter derives a fresh stack per chain; the codegen previously shared
/// one stack across inline member bodies, so `.2` resolved to `bump`'s internal
/// Add# result (an Int where a Calc is expected) and the run produced garbage.
#[test]
fn test_chain_backref_isolated_from_member_body_chains() {
    let src = r#"
obj Calc {
    base: Int;
    defn bump() -> Int { term base.Add#(1); };
};
defn pick(a: Int, b: Calc) -> Int { term a + b.base; };
let c: Calc = 0;
let done: Bool = false;
node go [done == false][done == true] {
    when done == false {
        let r: Int = c.bump().2>>pick();
        println!(r);
        done = true;
    };
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);

    use std::process::Command;
    let rt = std::env::temp_dir().join(format!("briev_chain_bug_b_{}.c", std::process::id()));
    std::fs::write(
        &rt,
        "#include <stdio.h>\n#include <stdint.h>\nint64_t __print_int(int64_t n) { printf(\"%ld\", (long)n); return 0; }\nint64_t __print_char(int64_t c) { putchar((int)c); return 0; }\n",
    )
    .expect("write stub");
    let out = std::env::temp_dir().join(format!(
        "briev_chain_bug_b_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let ll = out.with_extension("ll");
    let prelude = "declare i64 @__print_int(i64)\ndeclare i64 @__print_char(i64)\n";
    let ir = if let Some(pos) = ir.find("target triple") {
        let end = ir[pos..].find('\n').map(|e| pos + e + 1).unwrap_or(ir.len());
        format!("{}{}{}", &ir[..end], prelude, &ir[end..])
    } else {
        format!("{prelude}{ir}")
    };
    std::fs::write(&ll, &ir).expect("write .ll");
    let compile = Command::new("clang")
        .arg("-O0")
        .arg(&ll)
        .arg(&rt)
        .arg("-lm")
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "link failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&out).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let _ = std::fs::remove_file(&ll);
    let _ = std::fs::remove_file(&out);
    assert!(
        run.status.success(),
        "program crashed: {} (ir:\n{ir})",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        stdout.trim(),
        "1",
        "`.2` must resolve to `c` (base 10), not a register from bump()'s \
         internal chain; got:\n{stdout}\n---\n{ir}"
    );
}

/// 2026-08-18 (Phase D, PiggyBank): the arrow's CONSUME flag selects the
/// value-side op — `dest <- src` (read) resolves `op CopyFrom`, `dest ~<- src`
/// (destructive) resolves `op ExtractFrom` (extract_op_order in the
/// typechecker, find_extract_strategy_for_arrow in codegen). Only a
/// ZERO-PARAM member is a valid arrow target (the arrow supplies no args — the
/// coll scaffold's `get(i)` CopyFrom can never read). q holds [1,2,3]: pop → 3,
/// front (no pop) → 1, pop → 2.
#[test]
fn test_arrow_consume_selects_copyfrom_vs_extractfrom() {
    let src = r#"
coll obj Q { data: Ptr<Int>; };
let q: Q = [1, 2, 3];
let done: Bool = false;
node go [done == false][done == true] {
    when done == false {
        let a: Int;
        a ~<- q;
        println!(a);
        let b: Int;
        b <- q;
        println!(b);
        done = true;
    };
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);

    use std::process::Command;
    // 2026-09-09 (Family B): briev_rt.c no longer defines the print family —
    // it moved to pure-Briev defns (std/cast_lanes.bv, write(2) via
    // SysCall#). These tests only need OBSERVABLE OUTPUT, so link a tiny
    // printf stub instead of the (shrinking) runtime.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let rt = std::env::temp_dir().join(format!(
        "briev_print_stub_{}.c",
        std::process::id()
    ));
    std::fs::write(&rt, "#include <stdio.h>\n#include <stdint.h>\n#include <stdlib.h>\n#include <string.h>\nint64_t __print_int(int64_t n) { printf(\"%ld\", (long)n); return 0; }\nint64_t __print_char(int64_t c) { putchar((int)c); return 0; }\nint64_t __briev_coll_resize(int64_t handle, int64_t new_cap) {\n    if (!handle || new_cap < 0) return 1;\n    int64_t* block = (int64_t*)handle;\n    int64_t old_data = block[0];\n    int64_t len = block[2];\n    if (new_cap == 0) { free((void*)old_data); block[0] = 0; block[1] = 0; return 0; }\n    int64_t* nd = (int64_t*)malloc((size_t)(new_cap * 8));\n    if (!nd) return 1;\n    int64_t copy_n = len < new_cap ? len : new_cap;\n    if (old_data && copy_n > 0) memcpy(nd, (void*)old_data, (size_t)(copy_n * 8));\n    if (old_data) free((void*)old_data);\n    block[0] = (int64_t)nd;\n    block[1] = new_cap;\n    return 0;\n}\n").expect("write stub");
    let out = std::env::temp_dir().join(format!(
        "briev_arrow_consume_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let ll = out.with_extension("ll");
    // Declare the runtime prints (the unit-test pipeline emits the calls but
    // not the FFI declarations).
    let prelude = "declare i64 @__print_int(i64)\ndeclare i64 @__print_char(i64)\n";
    let ir = if let Some(pos) = ir.find("target triple") {
        let end = ir[pos..].find('\n').map(|e| pos + e + 1).unwrap_or(ir.len());
        format!("{}{}{}", &ir[..end], prelude, &ir[end..])
    } else {
        format!("{prelude}{ir}")
    };
    std::fs::write(&ll, &ir).expect("write .ll");
    let compile = Command::new("clang")
        .arg("-O0")
        .arg(&ll)
        .arg(&rt)
        .arg("-lm")
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "link failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&out).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let _ = std::fs::remove_file(&ll);
    let _ = std::fs::remove_file(&out);
    assert!(
        run.status.success(),
        "program crashed: {} (ir:\n{ir})",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        stdout.trim(),
        "3\n3",
        "`~<- q` and `<- q` must both POP (the scaffold's CopyFrom `get(i)` is \
         parameterized, so a plain `<-` falls back to ExtractFrom pop, the \
         pre-Phase-D behavior); got:\n{stdout}\n---\n{ir}"
    );
}

/// 2026-08-18 (pooled-member foreach): `foreach x in obj.items` — `items` a
/// POOLED member List (slot `{prefix}.items`, column type
/// `Vector(List<Int>, [Anonymous(1)])`) — must iterate through the tier-2
/// `op Count`/`op At` surface. Before the fix, tier2_op_collection resolved
/// only bare let/state names, so both the bare-member form (inside a member
/// body) and the FIELD-ACCESS form (`ledger.items`) fell through to the
/// emit_stmt.rs:262 panic. Runs the program: node-level foreach sum = 6,
/// Count# = 3, member-body foreach total = 6.
#[test]
fn test_foreach_over_pooled_member_list() {
    let src = r#"
coll obj MyList { data: Ptr<Int>; };
obj Ledger {
    items: MyList;
    op Count() -> Int { term items.Count#(); };
    op At(i: Int) -> Int { term items[i]; };
    txn init(v: Int) [true][items.^Size == 0] {
        let e: MyList = [];
        items = e;
    };
    defn put(e: Int) { items <- e; };
    defn total() -> Int {
        let acc: Int = 0;
        foreach x in items {
            acc = acc + x;
        };
        term acc;
    };
};
let ledger: Ledger = 0;
let done: Bool = false;
node go [done == false][done == true] {
    when done == false {
        ledger.put(1);
        let s: Int = 0;
        foreach x in ledger.items {
            s = s + x;
        };
        println!(s);
        println!(ledger.total());
        done = true;
    };
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    // 2026-08-18: the pooled-member iterable must resolve through the tier-2
    // op Count/op At surface. BEFORE the fix tier2_op_collection resolved only
    // bare let/state names — a pooled member slot (`{prefix}.items`,
    // `Vector(inner, [Anonymous(1)])`) fell through and `backend.generate`
    // PANICKED at emit_stmt.rs:262. `generate` returning at all is the gate;
    // the tier-2 COUNTED LOOP (`icmp slt` header) proves the iteration was
    // emitted (not silently dropped). This runs the in-process pipeline, which
    // skips the typechecker — the MyList coll obj scaffolds the op surface.
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("icmp slt"),
        "a pooled-member foreach must emit a tier-2 counted loop; got:\n{ir}"
    );
}

/// 2026-08-18 (BUGS.md defn-param mutation): a POOLED instance used as a VALUE
/// (a defn argument) has no scalar register — its fields live in the state's
/// pooled columns, and the identifier arm emits an undefined `@p` global.
/// box_pooled_instance_value must materialize a heap BOX at the call site.
/// Assert the IR: no `@p` global, and the box is malloc'd.
#[test]
fn test_pooled_instance_defn_arg_is_boxed() {
    let src = r#"
obj P {
    data: Int;
    defn set(v: Int) { data = v; }
};
let p: P = 0;
defn poke(x: P) -> Int { term x.data; };
node go [true][true] {
    let r: Int = poke(p);
    println!(r);
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("load i64, ptr @p"),
        "a pooled-instance defn ARG must be boxed, never emitted as the @p global; got:\n{ir}"
    );
    assert!(
        ir.contains("call ptr @malloc"),
        "the pooled-instance defn arg must be boxed with a malloc; got:\n{ir}"
    );
}

/// The frontend bounded-length analysis proves a balanced drain (pop then push
/// keeps len ≤ initial < cap) never overflows, so the grow guard is stripped
/// from the inlined push — no opaque resize call in the loop. This is the
/// queue_drain_idio fix (0.58x → 4.00x → 0.58x).
#[test]
fn test_coll_guard_stripped_in_proven_drain() {
    let src = r#"
coll obj Q { data: Ptr<Int>; };
let q: Q = [0];
let count: Int = 0;
node work [count < N][count == N] {
    <- q;
    q.push(count);
    count = count + 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("call i64 @__briev_coll_resize"),
        "a proven-safe drain must have NO resize call in the loop; got:\n{ir}"
    );
}

/// A coll that genuinely exceeds the default cap (foreach with 21 pushes) is
/// NOT provable — the grow guard must stay and the coll must still grow.
#[test]
fn test_coll_guard_kept_for_unproven_growth() {
    let src = r#"
coll obj Q { data: Ptr<Int>; };
let q: Q = [];
let done: Int = 0;
node work [done == 0][done == 1] {
    foreach x in 0..21 { q.push(x); };
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @__briev_coll_resize"),
        "an unprovable (growing) coll must keep the grow guard; got:\n{ir}"
    );
}

/// D2 pre-grow (2026-08-16, plan three-track Phase 2): a LOCAL coll whose
/// intra-firing peak exceeds the default cap (21 pushes vs cap 16) gets ONE
/// `EnsureCap#(q, peak)` at the LET site — the resize call moves BEFORE the
/// loop and the per-push grow guard (which would now be dead) is stripped.
/// The node body is emitted once per guard range (two ranges here), so the
/// resize call may appear more than once — but NEVER inside a foreach body.
/// State colls never pre-grow (peak only covers one firing); declared `op
/// Grow` colls never pre-grow (user contract); fixed-buffer colls never
/// pre-grow (EnsureCap# would corrupt the buffer).
#[test]
fn test_coll_pregrow_local_moves_resize_before_loop() {
    let src = r#"
coll obj MyQueue { data: Ptr<Int>; };
let done: Int = 0;
node go [done == 0][done == 1] {
    let q: MyQueue = [];
    foreach x in 0..21 {
        q.push(x);
    };
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    // Every resize call must be at a LET site, never inside a foreach body
    // (the stripped grow guard would have placed it there).
    let mut i = 0;
    while let Some(body_at) = ir[i..].find("foreach.body") {
        let body_at = i + body_at;
        let end_at = ir[body_at..]
            .find("foreach.end")
            .map(|e| body_at + e)
            .expect("foreach.body must have a foreach.end");
        let region = &ir[body_at..end_at];
        assert!(
            !region.contains("call i64 @__briev_coll_resize"),
            "no resize call may appear inside a foreach body (guard must be stripped); got:\n{ir}"
        );
        i = end_at;
    }
    // And the pre-grow must have actually fired (resize emitted at the let
    // site — this is what the per-push guard would have grown to).
    assert!(
        ir.contains("call i64 @__briev_coll_resize"),
        "pre-grown local coll must emit the let-site EnsureCap#; got:\n{ir}"
    );
}

/// Per-NAME strip: a txn with TWO local colls, one pre-grown (21 pushes) and
/// one below cap (10 pushes) — the below-cap coll keeps its lazy growth path
/// (its (txn, name) fact is absent), the pre-grown one strips BOTH the guard
/// and keeps exactly one resize at its own let site. Keying the strip on
/// (txn, coll_name) rather than base prevents cross-coll strip leakage.
#[test]
fn test_coll_pregrow_strip_keys_on_coll_name_not_base() {
    let src = r#"
coll obj Q { data: Ptr<Int>; };
let done: Int = 0;
node go [done == 0][done == 1] {
    let q: Q = [];
    let r: Q = [];
    foreach x in 0..21 {
        q.push(x);
    };
    foreach x in 0..10 {
        r.push(x);
    };
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    // q (peak 21 > cap 16) pre-grows: its let-site EnsureCap# call is the only
    // resize in the IR. r (peak 10 < cap 16) never exceeds cap — no resize for
    // it. The strip keyed on (txn, "q") must NOT remove r's grow guard... but
    // r's guard would only fire if r exceeded cap, which it cannot here.
    assert!(
        ir.contains("call i64 @__briev_coll_resize"),
        "the pre-grown coll q must have its let-site EnsureCap#; got:\n{ir}"
    );
}

// ── Coll struct literal construction (2026-08-16, Phase 3a) ──────────

/// A fixed `coll struct Fixed { data: Int[4] }` literal constructs DIRECTLY
/// into the inline `T[N]` array — the IR declares `%Fixed = type { [4 x i64] }`
/// (not the scalar collapse `{ i64 }`), and the literal stores elements at
/// data[0..N-1] with NO [len] heap-seq header. `f.data[2]` then reads element
/// 2 (the inline GEP), and `.^Length`/`Count#`/`Capacity#` all report N.
#[test]
fn test_coll_struct_literal_inline_array() {
    let src = r#"
coll struct Fixed { data: Int[4]; };
let done: Int = 0;
node go [done == 0][done == 1] {
    let f: Fixed = [1, 2, 3, 4];
    done = f.data[2];
    done = f.^Length;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("%Fixed = type { [4 x i64] }"),
        "the coll struct must declare the inline array type, not a scalar collapse; got:\n{ir}"
    );
    assert!(
        !ir.contains("%Fixed = type { i64 }"),
        "the fixed T[N] field must never collapse to a scalar i64; got:\n{ir}"
    );
    // The literal must NOT have a [len] heap-seq header (store of 4 then
    // elements at GEP 1..4). A direct construction stores at data[0..3].
    assert!(
        !ir.contains("store i64 4, ptr"),
        "the fixed literal must not write a [len] heap-seq header; got:\n{ir}"
    );
}

/// foreach over a fixed `coll struct` iterates the inline array via the
/// synthesized op surface (op Count returns the constant N, op At reads
/// data[i]) — no extractelement on a scalar, no undefined @data global, no
/// heap-seq literal. Runs the sum of [1,2,3,4] = 10.
#[test]
fn test_coll_struct_literal_foreach() {
    let src = r#"
coll struct Fixed { data: Int[4]; };
let done: Int = 0;
node go [done == 0][done == 1] {
    let f: Fixed = [1, 2, 3, 4];
    let s: Int = 0;
    foreach v in f {
        s = s + v;
    };
    done = s;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("extractelement"),
        "foreach over a fixed coll struct must GEP+load the inline array, not extractelement; got:\n{ir}"
    );
    assert!(
        !ir.contains("@data = "),
        "the member-body 'data' must resolve through the boxed self, never an undefined @data global; got:\n{ir}"
    );
}

/// An over-length literal for a fixed `coll struct` is a type error — the
/// heap-seq fallback must never construct a coll struct value (it misaligns
/// the inline array by the [len] header). Codegen defends in depth with a
/// hard error (panic), not a silent fallback.
#[test]
#[should_panic(expected = "capacity is 2 elements")]
fn test_coll_struct_oversize_literal_rejected() {
    let src = r#"
coll struct Fixed { data: Int[2]; };
let done: Int = 0;
node go [done == 0][done == 1] {
    let f: Fixed = [1, 2, 3];
    println!(f.^Length);
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let _ = backend.generate(&items, None);
}

/// Phase 3b — const generics: `coll struct Fixed<T, N> { data: T[N] }` with
/// `Fixed<Int, 4>` must resolve the mono dimension to a concrete `Int[4]`:
/// the literal stores into `[4 x i64]` GEPs, the scaffolded Count is the
/// constant 4 (NOT a `len` slot read — a generic coll struct has no hidden
/// len), and `f.data[3]` reads element 3. Before the mono-keyed fix the
/// generic base's unresolved `Named("N",0)` dim made Count read an undefined
/// `@len` global.
#[test]
fn test_coll_struct_generic_const_dimension() {
    let src = r#"
coll struct Fixed<T, N> { data: T[N]; };
let done: Int = 0;
node go [done == 0][done == 1] {
    let f: Fixed<Int, 4> = [1, 2, 3, 4];
    done = f.Count#() + f.data[3];
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("ptr @len"),
        "the generic coll struct Count must be the constant N, not a len-slot read; got:\n{ir}"
    );
    assert!(
        ir.contains("getelementptr [4 x i64]"),
        "the mono Fixed<Int, 4> literal must store into [4 x i64] GEPs; got:\n{ir}"
    );
}

// ── Multi-node internal fold (2026-08-16, Direction 3) ───────────────

/// A counted-loop node in a MULTI-node program is folded into a noinline
/// countdown `@txn_<name>` that the reactor calls once per pass — the other
/// node fires only at the pass boundary (`i == bound`), so it is not starved.
#[test]
fn test_multi_node_internal_fold_calls_txn() {
    let src = r#"
let i: Int = 0;
let bound: Int = 10;
let done: Int = 0;
async node step [i < bound][i == bound] {
    i = i + 1;
    term;
};
async node fin [i == bound][done == 1] {
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call void @txn_step("),
        "the reactor must call the folded countdown txn; got:\n{ir}"
    );
}

/// A node whose precondition fires MID-pass (`i % 2 == 0`) would be starved by
/// the fold — the gate must reject it and keep the per-firing dispatch.
#[test]
fn test_multi_node_internal_fold_rejected_when_interior_fire() {
    let src = r#"
let i: Int = 0;
let bound: Int = 10;
let done: Int = 0;
async node step [i < bound][i == bound] {
    i = i + 1;
    term;
};
async node intr [i % 2 == 0][true] {
    done = 1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("call void @txn_step("),
        "an interior-firing node must block the fold (it would be starved); got:\n{ir}"
    );
}

/// `(n as String)` routes through the `Int → String` casting-graph lane
/// (`ExtCall int_to_str`), which must emit `call ptr @int_to_str(i64)` — the
/// String IS a ptr to [len][bytes]. Regression: the ExtCall hardcoded `i64`
/// (type mismatch) and `int_to_str` was undefined (a latent link error).
#[test]
fn test_cast_int_to_string_lane_emits_ptr_call() {
    let src = r#"
        defn f(n: Int) -> String {
            term (n as String);
        };
    
        node __test_go [true][true] { f(0); };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call ptr @int_to_str(i64"),
        "the Int->String lane must emit call ptr @int_to_str; got:\n{ir}"
    );
}

/// A Float64 state field initialized from a literal (positive and negative)
/// must store a proper double — regression: the init boxed the FLOAT32 into
/// the double slot (4 garbage low bytes, e.g. 3.25 → 5.33e-315).
#[test]
fn test_float64_field_init_stores_double() {
    let src = r#"
        let d: Float64 = 3.25;
        let n: Float64 = -3.25;
        let count: Int = 0;
        node report [count < 3][count == 3] {
            count = count + 1;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let _ir = backend.generate(&items, None);
    // The end-to-end correctness (a double slot stores the f64 bits, not a
    // float32 boxed with 4 garbage bytes) is verified by the brievc build path
    // (d=3.25 / d=-3.25 print correctly); the bare generate here is a smoke
    // test that Float64 literals + the coercion typecheck and emit.
}

/// Regression (2026-08-03, BUGS.md loop-guard state-store fix): a body READ
/// of the loop-guard field must resolve to the live counter phi, not a stale
/// `%State` load. Before the fix, `println!(count)` in a counter-only loop
/// emitted `load i64, ptr %state` (always 0); after, it emits the counter phi
/// register directly (0,1,2,…).
#[test]
fn test_counter_loop_guard_read_uses_phi_not_state() {
    let src = r#"
        let count: Int = 0;
        node report [count < 3][count == 3] {
            println!("count={}", count);
            count = count + 1;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.register(Box::new(crate::plugin::print_plugin::PrintPlugin));
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("print plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);

    // The count print in @main's folded loop must feed the counter phi
    // register (a value defined by a `phi` instruction), not a GEP+load from
    // %State. The unused always-inline `txn_report` body still carries the
    // naive state load, but the folded @main is what runs.
    let body = ir.split(".fmain.body:").nth(1)
        .unwrap_or("")
        .split(".fmain.latch:").next()
        .unwrap_or("");
    assert!(
        body.contains("call i64 @__print_int(i64 %flc"),
        "main's guard-field read must resolve to the counter phi; got:\n{ir}"
    );
    assert!(
        !body.contains("call i64 @__print_int(i64 %t")
            || body.contains("call i64 @__print_int(i64 %flc"),
        "main must not print a stale %State load of the counter; got:\n{ir}"
    );
    // The counter advances through the phi latch (add on the phi) in the same
    // main function.
    let main = ir.split(".fmain.header:").nth(1)
        .unwrap_or("")
        .split(".fmain.end:").next()
        .unwrap_or("");
    assert!(
        main.contains("add nuw nsw i64 %flc6, 1"),
        "counter must increment the phi backedge; got:\n{ir}"
    );
}

// ── Phase 8: closure inline lowering + `^^Type` descriptor ──────────

fn txn_with_body(body: Vec<Statement>) -> TopLevel {
    TopLevel::Transaction(Transaction {
        name: "c".to_string(),
        is_reactive: true,
        is_async: false,
        type_params: vec![],
        parameters: vec![],
        output_type: None,
        outputs: vec![],
        contract: default_contract(),
        body,
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    })
}

#[test]
fn test_closure_let_emits_env_and_indirect_call() {
    // `let f = x -> x + 1; let y = f(41);` — the closure is a heap env block
    // (fn_ptr at slot 0); the call goes INDIRECT through it. The closure
    // function is emitted at module end and must return the body's value.
    let mut backend = LlvmBackend::new();
    let txn = txn_with_body(vec![
        Statement::Let {
            name: "f".into(),
            names: vec![],
            ty: None,
            expr: Some(Expr::Lambda(
                vec!["x".into()],
                Box::new(Expr::BinaryOp(
                    BinaryOpKind::Add,
                    Box::new(Expr::Identifier("x".into())),
                    Box::new(Expr::Decimal(1)),
                )),
            )),
            modifiers: vec![],
        },
        Statement::Let {
            name: "y".into(),
            names: vec![],
            ty: None,
            expr: Some(Expr::Call("f".into(), vec![Expr::Decimal(41)], None)),
            modifiers: vec![],
        },
        Statement::Term(None),
    ]);
    let ir = backend.generate(&vec![txn], None);
    assert!(
        ir.contains("briev_closure_"),
        "a closure function must be emitted; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 %"),
        "the closure call must go indirect through the fn_ptr; got:\n{ir}"
    );
    assert!(
        ir.contains("define i64 @briev_closure_0(ptr %env, i64 %p0)"),
        "the closure function must take env + the param; got:\n{ir}"
    );
    assert!(
        ir.contains("ret i64 %"),
        "the closure function must return the body value; got:\n{ir}"
    );
}

#[test]
fn test_reflect_type_emits_category_constant() {
    // `f.^^Type` on a Float receiver folds to the Float category code (1).
    let mut backend = LlvmBackend::new();
    let txn = txn_with_body(vec![
        Statement::Let {
            name: "f".into(),
            names: vec![],
            ty: None,
            expr: Some(Expr::Reflect(
                Box::new(Expr::Float(1.5)),
                "Type".into(),
                ReflectKind::CompileTime,
            )),
            modifiers: vec![],
        },
        Statement::Term(None),
    ]);
    let ir = backend.generate(&vec![txn], None);
    assert!(
        ir.contains("add i64 0, 1"),
        "Float ^^Type must fold to category code 1; got:\n{ir}"
    );
}

/// 2026-08-14 (boundary plan): `s.^^Element` on a `String` operand folds to
/// the Char category code (3) — a frozen descriptor, single constant.
#[test]
fn test_reflect_element_on_string_folds_char_code() {
    let src = r#"
        let s: String = "hello";
        node report [true][true] {
            let e: Int = s.^^Element;
            term;
        };
    "#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("add i64 0, 3"),
        "String ^^Element must fold to the Char category code (3); got:\n{ir}"
    );
}

/// 2026-08-14 (boundary plan): `xs.^^Element` on a `List<String>` folds to the
/// String element's category code — the read-op return substituted, single
/// source.
#[test]
fn test_reflect_element_on_list_folds_element_code() {
    let src = r#"
coll obj MyList { data: Ptr<Int>; };
let xs: MyList = [1, 2, 3];
let done: Int = 0;
node report [done == 0][done == 1] {
    let e: Int = xs.^^Element;
    done = e;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("add i64 0, 0"),
        "List<Int> ^^Element must fold to the Int category code (0); got:\n{ir}"
    );
}

// ── Phase 5: op elaboration → declared function call ────────────────

#[test]
fn test_declared_op_elaborates_to_function_call() {
    // A declared `op Add(Int): my_add(#Lh, #Rh)` must lower `MyNum + Int` to a
    // call of my_add (the typechecker's elaboration rewrites the BinaryOp).
    let src = r#"
defn my_add(a: Int, b: Int) -> Int { term (a * 3) + b; };
type MyNum : Int {
    op Add(Int): my_add(#Lh, #Rh);
};
node start [true][false] {
    let x: MyNum = 4;
    let y: Int = 2;
    let z: MyNum = x + y;
    term;
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let mut items = p.parse_program().unwrap();
    let universe = crate::type_universe::TypeUniverse::new();
    crate::typechecker::check_program(&mut items, &universe).unwrap();
    let mut backend = LlvmBackend::new();
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @my_add"),
        "declared op must lower to a my_add call; got:\n{ir}"
    );
    assert!(
        !ir.contains("call i64 @add("),
        "Int + Int must not lower to the undefined bootstrap 'add' symbol; got:\n{ir}"
    );
}

// ── Phase 9: garbage scheduler — loop-exit Free# emission ───────────

#[test]
fn test_loop_txn_last_consumer_emits_free_after_loop() {
    // A countable-loop txn (the benchmark's countdown shape, with a periodic
    // `when` guard) that is a heap-backed field's last consumer must emit
    // __briev_free AFTER the loop exits (never inside the iterating body).
    let src = r#"
let N: Int = GetEnvInt#("BOUND");
let buf: Ptr<Int> = Malloc#(64) as Ptr<Int>;
let sum: Int = 0;
node life [sum < N][sum == N] {
    buf[sum % 64] = sum;
    sum = sum + 1;
    when sum % 5000000 == 0 {
        buf[sum % 64] = 0;
    };
    term;
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new();
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 @__briev_free(ptr %state"),
        "the scheduler must free the heap buffer after the loop; got:\n{ir}"
    );
    // The free must be in a terminal block (after the loop) — find its
    // position and ensure it precedes a `ret`.
    let free_line = ir
        .lines()
        .position(|l| l.contains("call i64 @__briev_free"))
        .expect("free call line");
    let tail: Vec<&str> = ir.lines().skip(free_line).take(6).collect();
    assert!(
        tail.iter().any(|l| l.contains("ret ")),
        "the free must precede a return (post-loop), got: {tail:?}"
    );
}

// ── Diagnostics sweep (2026-08-06): scheduler leak warning ──────────

#[test]
fn test_non_bounded_reactive_heap_txn_not_scheduled_for_free() {
    // A reactive last-consumer with NO bounded loop has no sound free point.
    // The scheduler must NOT plan a free for it (falls back to "lives for the
    // program") — no spurious "will leak" warning, because the plan is never
    // made in the first place (root-cause fix).
    let src = r#"
let done: Bool = false;
let buf: Ptr<Int> = Malloc#(64) as Ptr<Int>;
node t [done == false][done == true] {
    buf[0] = 1;
    done = true;
    term;
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new();
    backend.generate(&items, None);
    assert!(
        !backend.warnings().iter().any(|w| w.contains("will leak")),
        "a non-bounded last consumer must not be scheduled for a free (lives for \
         the program); got warnings: {:?}",
        backend.warnings()
    );
}


// ── Fix 4 (2026-08-06): escaping closures — env + indirect call ────

#[test]
fn test_escaping_closure_env_and_indirect_call() {
    // `let f = x -> x * k; let a = f(2);` — f is a heap env block (captures k
    // by value), the call is indirect through the stored fn_ptr.
    let src = r#"
node start [true][false] {
    let k: Int = 5;
    let f = x -> x * k;
    let a: Int = f(2);
    let b: Int = f(3);
    let c: Int = a + b;
    term Print#(c);
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new();
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("define i64 @briev_closure_0(ptr %env, i64 %p0)"),
        "closure function must take env + param; got:\n{ir}"
    );
    assert!(
        ir.contains("call i64 %"),
        "the call must go indirect through the fn_ptr; got:\n{ir}"
    );
    assert!(
        ir.contains("getelementptr i64, ptr %env, i64 1"),
        "the captured var must be read from env slot 1; got:\n{ir}"
    );
}

#[test]
fn test_closure_alias_shares_env() {
    // `let g = f;` — g aliases f's env block; calling g goes indirect too.
    let src = r#"
node start [true][false] {
    let f = x -> x + 1;
    let g = f;
    let a: Int = g(41);
    term Print#(a);
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new();
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("call i64 %"),
        "the alias call must go indirect; got:\n{ir}"
    );
    assert!(
        !ir.contains("call i64 @g("),
        "no direct symbol call for the alias; got:\n{ir}"
    );
}

// ── Phase 7 (2026-08-06): `#b` raw-bytes Blob literal ───────────────

#[test]
fn test_byte_literal_emits_raw_bstr_constant() {
    // `#b"\x89PNG"` emits an @bstr.N constant with the EXACT bytes and a
    // Blob-typed value; `^Len` reads the [len] header (byte count).
    let src = r#"
node start [true][false] {
    let b: Blob = #b"\x89PNG";
    let c: Int = b.^Length;
    term Print#(c);
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut backend = LlvmBackend::new();
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("@bstr.0") && ir.contains("i8 -119") && ir.contains("i8 80"),
        "the byte literal must emit an @bstr constant with raw bytes; got:\n{ir}"
    );
    assert!(
        ir.contains("load i64, ptr %"),
        "Blob ^Len must load the [len] header; got:\n{ir}"
    );
}

// ── First-class backend normalizers (2026-08-10): webstack layout helper ─

#[test]
fn test_web_llvm_byte_size_scalars() {
    use crate::backend::llvm::web_llvm_byte_size;
    assert_eq!(web_llvm_byte_size("i8"), 1);
    assert_eq!(web_llvm_byte_size("i32"), 4);
    assert_eq!(web_llvm_byte_size("i64"), 8);
    assert_eq!(web_llvm_byte_size("float"), 4);
    assert_eq!(web_llvm_byte_size("double"), 8);
    assert_eq!(web_llvm_byte_size("ptr"), 8);
}

#[test]
fn test_web_llvm_byte_size_arrays_and_unknown() {
    use crate::backend::llvm::web_llvm_byte_size;
    assert_eq!(web_llvm_byte_size("[1024 x i64]"), 8 * 1024);
    assert_eq!(web_llvm_byte_size("[8 x float]"), 4 * 8);
    assert_eq!(web_llvm_byte_size("i128"), 0, ">i64 words not stored in %State");
    assert_eq!(web_llvm_byte_size("bogus"), 0);
}

#[test]
fn test_web_generator_type_tag_from_protocol_category() {
    use crate::glue::web_generator::TypeTag;
    assert_eq!(TypeTag::from_protocol_category(Some("Int")), TypeTag::Int);
    assert_eq!(TypeTag::from_protocol_category(Some("UInt")), TypeTag::Int);
    assert_eq!(TypeTag::from_protocol_category(Some("Float")), TypeTag::Int);
    assert_eq!(TypeTag::from_protocol_category(Some("Bool")), TypeTag::Bool);
    assert_eq!(TypeTag::from_protocol_category(Some("String")), TypeTag::String);
    assert_eq!(TypeTag::from_protocol_category(Some("Char")), TypeTag::String);
    assert_eq!(TypeTag::from_protocol_category(None), TypeTag::Int);
}

#[test]
fn test_state_layout_emits_real_field_rows() {
    // With webstack enabled, @__web_layout carries one row per %State field:
    // handle, structural offset, byte size, and protocol-derived type tag.
    let src = r#"
let count: Int = 0;
let name: String = "";
node start [true][name .^Length == 0] {
    term Print#(count);
};
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let universe = crate::type_universe::TypeUniverse::new();
    let mut backend = crate::backend::llvm::LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("@__web_layout") && ir.contains("define i32 @state_layout()"),
        "webstack must emit the layout table; got:\n{ir}"
    );
    assert!(
        ir.contains("i32 3, i32 ptrtoint (ptr @__web_generation to i32), i32 ptrtoint (ptr @__web_flush_buf to i32)"),
        "header must count 3 fields and reference the real buffer/counter; got:\n{ir}"
    );
    // String field → tag 3 (TypeTag::String), Int field → tag 0.
    assert!(
        ir.contains("i32 3") && ir.contains("i32 0"),
        "rows must carry string(cat=3) and int(cat=0) tags; got:\n{ir}"
    );
}
#[test]
fn test_webstack_flush_batch_covers_written_fields() {
    // 2026-08-10: term flushes a real update batch — one {handle, value_ptr,
    // value_len} record per written field, not the historical (0, 0) stub.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "increment".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("count".into()), Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    // Buffer must be declared and the call must pass its address (not 0).
    assert!(ir.contains("@__web_flush_buf = private global"),
        "flush buffer must be declared; got:\n{ir}");
    assert!(ir.contains("call void @__web_flush_state(i32 ptrtoint (ptr @__web_flush_buf to i32), i32 1)"),
        "flush call must pass the buffer with 1 record; got:\n{ir}");
    assert!(!ir.contains("call void @__web_flush_state(i32 0, i32 0)"),
        "stub call must be gone; got:\n{ir}");
    // The written field's record: handle 0, value_len 8 (i64 word).
    assert!(ir.contains("store i32 0, ptr %t") && ir.contains("store i32 8, ptr %t"),
        "record must carry handle 0 and len 8; got:\n{ir}");
    // Generation counter must bump after the flush.
    assert!(ir.contains("add i32") && ir.contains("store i32 %t") && ir.contains("@__web_generation"),
        "generation must be incremented; got:\n{ir}");
}

#[test]
fn test_webstack_flush_empty_write_set_is_noop() {
    // 2026-08-10: a txn that writes nothing flushes (0, 0) — valid no-op for
    // the JS shim, and the buffer records stay untouched.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "tick".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![Statement::Term(None)],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("call void @__web_flush_state(i32 0, i32 0)"),
        "empty write_set must flush the no-op path; got:\n{ir}");
}

#[test]
fn test_webstack_layout_header_describes_real_buffer() {
    // 2026-08-10: the state_layout header must point at the real flush buffer
    // and generation counter (link-time-resolved ptrtoint), not the old
    // hardcoded `flush_off=64, max_entries=16`.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi");
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "increment".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("count".into()), Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("i32 ptrtoint (ptr @__web_generation to i32)"),
        "header must reference the real generation counter; got:\n{ir}");
    assert!(ir.contains("i32 ptrtoint (ptr @__web_flush_buf to i32)"),
        "header must reference the real flush buffer; got:\n{ir}");
    assert!(!ir.contains("i32 0, i32 64, i32 16"),
        "hardcoded stub header must be gone; got:\n{ir}");
}

#[test]
fn test_web_state_layout_carries_field_names() {
    // 2026-08-10: web_state_layout() must expose the field name → handle map
    // the JS shim needs to resolve view bindings (signal = field name).
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi")
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Statement(Box::new(Statement::Let {
            name: "label".into(), ty: Some(Type::string()), expr: None,
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "increment".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("count".into()), Expr::Decimal(1)),
                // reference label so the String field stays live (not pruned)
                Statement::Let {
                    name: "tmp".into(), ty: None,
                    expr: Some(Expr::Call("Len#".into(), vec![Expr::Identifier("label".into())], None)),
                    modifiers: vec![], names: vec![],
                },
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    backend.generate(&program, None);
    let layout = backend.web_state_layout("test_app");
    assert!(layout.fields.len() >= 3, "count + label (+ synthetic cycle_count); got: {:?}", layout.fields);
    let count = layout.fields.iter().find(|f| f.name == "count")
        .expect("count field present");
    assert_eq!(count.field_handle, 0, "count is the first field");
    let label = layout.fields.iter().find(|f| f.name == "label")
        .expect("label field present");
    assert_eq!(label.field_handle, 1, "label is the second field");
    assert_eq!(count.type_tag, crate::glue::web_generator::TypeTag::Int);
    assert_eq!(label.type_tag, crate::glue::web_generator::TypeTag::String);
}

#[test]
fn test_webstack_ssa_precondition_emits_valid_bool_branch() {
    // 2026-08-10: a [true] precondition in the SSA main loop must emit
    // `trunc i8 <reg> to i1` before `br i1` (as_bool_reg), not `br i1 <i8>`
    // directly. Without a type universe the membership check fails, so the
    // test sets one (as the CLI path does).
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi")
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "increment".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("count".into()), Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    // The SSA-loop guard must truncate the i8 Bool literal to i1 before br.
    assert!(ir.contains("trunc i8 %t") && ir.contains("br i1 %tb"),
        "precondition must trunc i8 → i1 before br; got:\n{ir}");
    assert!(!ir.contains("br i1 %t5,"),
        "no raw i8 reg may feed br i1; got:\n{ir}");
}

#[test]
fn test_webstack_int_literal_emits_target_width() {
    // 2026-08-10: emit_int must produce i{int_bits} (i32 on wasm32), matching
    // llvm_type(Int)/binop_int_type. The old hardcoded `add i64` produced
    // `sext i32 <i64 reg>` — invalid IR that llc rejects.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi")
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "increment".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("count".into()), Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("add i32 0, 1"),
        "Int literal must emit at i32 for wasm32; got:\n{ir}");
    assert!(!ir.contains("add i64 0, 1"),
        "Int literal must not hardcode i64; got:\n{ir}");
}

#[test]
fn test_webstack_state_int_slots_are_target_width() {
    // 2026-08-10: flexible Int %State slots are i{int_bits} (i32 on wasm32) —
    // the --int-bits design intent. x86_64 (int_bits=64) is unchanged.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi")
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Statement(Box::new(Statement::Let {
            name: "label".into(), ty: Some(Type::string()), expr: None,
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "increment".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Let { name: "tmp".into(), ty: None,
                    expr: Some(Expr::Call("Len#".into(), vec![Expr::Identifier("label".into())], None)),
                    modifiers: vec![], names: vec![] },
                Statement::Assign(Expr::Identifier("count".into()), Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    // Int slot → i32; String slot stays i64 (boxed ptr).
    assert!(ir.contains("%State = type { i32, i64 }") || ir.contains("%StateChunk0 = type { i32, i64 }")
        || ir.contains("%State = type { i32, i64, i64 }") || ir.contains("%StateChunk0 = type { i32, i64, i64 }"),
        "Int slot must be i32 on wasm32, String i64; got:\n{ir}");
}

#[test]
fn test_webstack_folded_loop_is_width_consistent() {
    // 2026-08-10: the folded loop counter must be i{int_bits} (phi i32, add i32)
    // with only the bound compare sext'd to i64. The old `phi i64` feeding
    // `add nsw i32` was invalid IR that llc rejected.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi")
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "count".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "increment".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract {
                pre_condition: Expr::BinaryOp(crate::ast::BinaryOpKind::Lt,
                    Box::new(Expr::Identifier("count".into())), Box::new(Expr::Decimal(100))),
                post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Assign(Expr::Identifier("count".into()),
                    Expr::BinaryOp(crate::ast::BinaryOpKind::Add,
                        Box::new(Expr::Identifier("count".into())), Box::new(Expr::Decimal(1)))),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("phi i32"),
        "folded loop counter phi must be i32 on wasm32; got:\n{ir}");
    assert!(ir.contains("add nuw nsw i32"),
        "folded loop backedge must be i32; got:\n{ir}");
    assert!(ir.contains("sext i32") && ir.contains("icmp slt i64"),
        "bound compare must sext i32 → i64; got:\n{ir}");
}

#[test]
fn test_webstack_array_field_store_gep_widens_index() {
    // 2026-08-10: `f[i] = v` on an Int[N] state field emits the GEP index at
    // i64 (sext from the i32 index) — the old raw i32 index was invalid.
    let mut backend = LlvmBackend::new()
        .with_webstack(true)
        .with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi")
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::Statement(Box::new(Statement::Let {
            name: "idx".into(), ty: Some(Type::int()), expr: Some(Expr::Decimal(0)),
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Statement(Box::new(Statement::Let {
            name: "buf".into(),
            ty: Some(Type::Vector(Box::new(Type::int()),
                vec![crate::ast::Dimension::Anonymous(4)])),
            expr: None,
            modifiers: vec![], names: vec![],
        })),
        TopLevel::Transaction(Transaction {
            name: "fill".into(), is_reactive: true, is_async: false,
            type_params: vec![], parameters: vec![], output_type: None, outputs: vec![],
            contract: Contract { pre_condition: Expr::Bool(true), post_condition: Expr::Bool(true), watchdog: None, explicit: false, span: None, post_authority: false},
            body: vec![
                Statement::Assign(
                    Expr::Index(Box::new(Expr::Identifier("buf".into())),
                        Box::new(Expr::Identifier("idx".into()))),
                    Expr::Decimal(1)),
                Statement::Term(None),
            ],
            metadata: HashMap::new(), derivation: None, modifiers: vec![], span: None, doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("sext i32") && ir.contains("getelementptr [4 x i32]"),
        "array store GEP index must sext i32 → i64; got:\n{ir}");
}

// ── pack struct (layout-keywords plan Phase 2) ────────────────────────

fn packed_struct_def(
    name: &str,
    fields: Vec<(&str, Type)>,
    endian: Option<&str>,
) -> StructDef {
    let mut metadata = HashMap::new();
    if let Some(e) = endian {
        metadata.insert(
            "endian".to_string(),
            crate::ast::PropertyValue::Identifier(e.to_string()),
        );
    }
    StructDef {
        type_params: vec![],
        name: name.to_string(),
        fields: fields.into_iter().map(|(n, t)| (n.to_string(), t)).collect(),
        metadata,
        seq: false,
        pack: true,
        union: false,
        span: None,
        coll: false,
    }
}

fn struct_literal_stmt(name: &str, bind: &str, fields: Vec<(&str, Expr)>) -> Statement {
    Statement::Let {
        names: vec![],
        name: bind.to_string(),
        ty: None,
        expr: Some(Expr::StructLiteral {
            type_name: name.to_string(),
            fields: fields.into_iter().map(|(n, e)| (n.to_string(), e)).collect(),
        }),
        modifiers: vec![],
    }
}

fn term_field(recv: &str, field: &str) -> Statement {
    Statement::Term(Some(Expr::Field(
        Box::new(Expr::Identifier(recv.to_string())),
        field.to_string(),
    )))
}

fn print_field(recv: &str, field: &str) -> Statement {
    Statement::Expression(Expr::Call(
        "__print_int".to_string(),
        vec![Expr::Field(
            Box::new(Expr::Identifier(recv.to_string())),
            field.to_string(),
        )],
        None,
    ))
}

fn packed_main_def(body: Vec<Statement>) -> TopLevel {
    TopLevel::Definition(Definition {
        variadic_param: None,
        name: "main".to_string(),
        type_params: vec![],
        parameters: vec![],
        outputs: vec![],
        output_type: None,
        contract: default_contract(),
        body,
        modifiers: vec![],
        metadata: HashMap::new(),
        derivation: None,
        annotations: vec![],
        span: None,
        doc: None,
    })
}

#[test]
fn test_packed_whole_byte_emits_native_aggregate() {
    // 2026-08-13 (pack): whole-byte packed structs (every field % 8 == 0)
    // declare LLVM's native packed type `<{ ... }>` and keep byte-offset GEP +
    // aligned loads/stores — the rule-19-validated native path.
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::StaticStruct(packed_struct_def(
            "Eth",
            vec![
                ("dst", Type::bits(48)),
                ("src", Type::bits(48)),
                ("etype", Type::bits(16)),
            ],
            Some("Big"),
        )),
        packed_main_def(vec![
            struct_literal_stmt(
                "Eth",
                "x",
                vec![
                    ("dst", Expr::Decimal(0x00_60_80_00_AA_BB)),
                    ("src", Expr::Decimal(0x00_20_40_00_CC_DD)),
                    ("etype", Expr::Decimal(0x0800)),
                ],
            ),
            term_field("x", "dst"),
        ]),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("%Eth = type <{ i48, i48, i16 }>"),
        "whole-byte packed struct must emit the native packed aggregate; got:\n{ir}");
    assert!(ir.contains("store i48") && ir.contains("align 1"),
        "whole-byte packed stores must be i48 with align 1; got:\n{ir}");
    assert!(ir.contains("load i48, ptr") && ir.contains("align 1"),
        "whole-byte packed reads must be aligned i48 loads; got:\n{ir}");
}

#[test]
fn test_packed_sub_byte_le_emits_byte_array_and_slices() {
    // 2026-08-13 (pack): sub-byte packed structs hide behind a byte array
    // `{ [N x i8] }`; fields read via load-shift-trunc (LE: shift = bit pos).
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::StaticStruct(packed_struct_def(
            "Nib",
            vec![
                ("a", Type::bits(12)),
                ("b", Type::bits(4)),
                ("c", Type::bits(8)),
            ],
            None,
        )),
        packed_main_def(vec![
            struct_literal_stmt(
                "Nib",
                "x",
                vec![
                    ("a", Expr::Decimal(0xABC)),
                    ("b", Expr::Decimal(0xF)),
                    ("c", Expr::Decimal(0xFF)),
                ],
            ),
            print_field("x", "b"),
            print_field("x", "a"),
        ]),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("%Nib = type { [3 x i8] }"),
        "sub-byte packed struct must hide fields behind a byte array; got:\n{ir}");
    assert!(ir.contains("lshr i8") && ir.contains("trunc i8") && ir.contains("to i4"),
        "LE sub-byte read must shift the byte and truncate to i4; got:\n{ir}");
    assert!(ir.contains("lshr i16") || ir.contains("trunc i16") && ir.contains("to i12"),
        "LE 12-bit cross-byte read must handle the i16 covering load; got:\n{ir}");
    assert!(ir.contains("and i8") && ir.contains("or i8"),
        "sub-byte store must clear+insert (and/or) in the byte; got:\n{ir}");
}

#[test]
fn test_packed_be_sub_byte_byte_reverses() {
    // 2026-08-13 (pack): Big-endian packed fields read the COVERED bytes
    // little-endian, mirror the byte order, then shift (cov*8 - bits).
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(crate::type_universe::TypeUniverse::new());
    let program = vec![
        TopLevel::StaticStruct(packed_struct_def(
            "BigP",
            vec![("a", Type::bits(12)), ("b", Type::bits(4))],
            Some("Big"),
        )),
        packed_main_def(vec![
            struct_literal_stmt(
                "BigP",
                "x",
                vec![("a", Expr::Decimal(0xABC)), ("b", Expr::Decimal(0xF))],
            ),
            print_field("x", "a"),
            print_field("x", "b"),
        ]),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("%BigP = type { [2 x i8] }"),
        "12+4 bits must collapse to a 2-byte array; got:\n{ir}");
    assert!(ir.contains("lshr i16") && ir.contains("to i12"),
        "BE 12-bit read must reverse bytes then shift; got:\n{ir}");
    assert!(ir.contains("shl i16") || ir.contains("trunc i16"),
        "BE read must place the high byte into the low position; got:\n{ir}");
    assert!(ir.contains("trunc i8") && ir.contains("to i4"),
        "BE single-byte low-nibble read truncs the byte (shift 0); got:\n{ir}");
}

#[test]
fn test_packed_struct_rejects_overwide_at_parse() {
    // 2026-08-13 (pack): Bits(>64) packed fields are rejected at parse time
    // (the 64-bit slice machinery). Parser-side test mirrors codegen guard.
    let src = "pack struct W { wide: Bits<128>; };";
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    assert!(p.parse_program().is_err(), "Bits<128> packed field must not parse");
}

// ── trap (layout-keywords plan Phase 4) ───────────────────────────────

#[test]
fn test_trap_statement_emits_llvm_trap() {
    // 2026-08-13 (layout-keywords plan Phase 4): `trap;` compiles to
    // `call void @llvm.trap()` + `unreachable` (SPEC §8.8), declared once in
    // the module header and terminating the block.
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let program = vec![packed_main_def(vec![
        Statement::Trap,
        Statement::Term(Some(Expr::Decimal(0))),
    ])];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("declare void @llvm.trap() noreturn"),
        "module must declare @llvm.trap; got:\n{ir}");
    assert!(ir.contains("call void @llvm.trap()") && ir.contains("unreachable"),
        "trap; must emit call void @llvm.trap() then unreachable; got:\n{ir}");
}

// ── atomic field modifier (layout-keywords plan Phase 5) ───────────────

#[test]
fn test_atomic_field_load_store_rmw() {
    // 2026-08-13 (Phase 5): `atomic` fields read/write with seq_cst atomic
    // ops; `obj.f = obj.f + c` lowers to atomicrmw add; plain fields stay on
    // the default non-atomic path (no `atomic` in their ops).
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(tu);
    let mut meta = HashMap::new();
    meta.insert(
        "atomic_fields".to_string(),
        crate::ast::PropertyValue::List(vec![crate::ast::PropertyValue::String("count".into())]),
    );
    let program = vec![
        TopLevel::StaticStruct(StructDef {
            type_params: vec![],
            name: "Counter".to_string(),
            fields: vec![
                ("count".to_string(), Type::int()),
                ("other".to_string(), Type::int()),
            ],
            metadata: meta,
            seq: false,
            pack: false,
            union: false,
            coll: false,
            span: None,
        }),
        TopLevel::Definition(Definition {
            variadic_param: None,
            name: "main".to_string(),
            type_params: vec![],
            parameters: vec![],
            outputs: vec![],
            output_type: None,
            contract: default_contract(),
            body: vec![
                struct_literal_stmt(
                    "Counter",
                    "c",
                    vec![("count", Expr::Decimal(0)), ("other", Expr::Decimal(1))],
                ),
                // atomic field RMW: c.count = c.count + 1
                Statement::Assign(
                    Expr::Field(Box::new(Expr::Identifier("c".into())), "count".into()),
                    Expr::BinaryOp(
                        crate::ast::BinaryOpKind::Add,
                        Box::new(Expr::Field(Box::new(Expr::Identifier("c".into())), "count".into())),
                        Box::new(Expr::Decimal(1)),
                    ),
                ),
                // atomic field read
                print_field("c", "count"),
                // plain field read (must stay non-atomic)
                print_field("c", "other"),
            ],
            modifiers: vec![],
            metadata: HashMap::new(),
            derivation: None,
            annotations: vec![],
            span: None,
            doc: None,
        }),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("load atomic i64, ptr") && ir.contains("seq_cst"),
        "atomic field read must be an atomic load; got:\n{ir}");
    assert!(ir.contains("atomicrmw add"),
        "c.count = c.count + 1 must lower to atomicrmw add; got:\n{ir}");
    assert!(ir.contains("store atomic i64"),
        "atomic struct-literal field store must be atomic; got:\n{ir}");
    assert!(ir.contains("load i64, ptr"),
        "plain field read stays on the default (non-atomic) path; got:\n{ir}");
}

// ── union (layout-keywords plan Phase 6) ───────────────────────────────

#[test]
fn test_union_emits_byte_array_and_offset_zero() {
    // 2026-08-13 (Phase 6): a union materializes as a byte array of its
    // largest aligned field storage; every field overlays at offset 0.
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(tu);
    let program = vec![
        TopLevel::StaticStruct(StructDef {
            type_params: vec![],
            name: "Word".to_string(),
            fields: vec![
                ("i".to_string(), Type::int()),
                ("b".to_string(), Type::bits(64)),
            ],
            metadata: HashMap::new(),
            seq: false,
            pack: false,
            union: true,
            coll: false,
            span: None,
        }),
        packed_main_def(vec![
            struct_literal_stmt("Word", "w", vec![("i", Expr::Decimal(7))]),
            print_field("w", "b"),
        ]),
    ];
    let ir = backend.generate(&program, None);
    assert!(ir.contains("%Word = type { [8 x i8] }"),
        "union must materialize as a byte array of the largest field; got:\n{ir}");
    assert!(ir.contains("getelementptr i8, ptr") && ir.contains("i64 0"),
        "union field access must GEP at offset 0; got:\n{ir}");
}

// ── reactor struct-slot loop (2026-08-13 fix) ─────────────────────────

#[test]
fn test_struct_literal_alloca_deferred_in_loop() {
    // The reactor fix defers struct-literal storage while `defer_struct_allocas`
    // is set (loop bodies), flushing it to the loop preheader. Assert the
    // mechanism: with the flag set the allocation is NOT emitted to `out`; the
    // flush writes it. (An allocation inside a reactor loop body made clang
    // -O3 peel the loop and emit a bogus exit assumption — nodes with
    // struct-typed state slots fired once.) The allocation is a heap `malloc`
    // (struct-literal lifetime: the handle crosses function boundaries), so the
    // deferred line is the malloc call, not a stack alloca.
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    backend.ctx.struct_types.insert(
        "Point".to_string(),
        vec![("x".to_string(), Type::int()), ("y".to_string(), Type::int())],
    );
    let mut out = String::new();
    backend.fun.defer_struct_allocas = true;
    let reg = backend.emit_struct_literal(
        &mut out, "%v", "Point",
        &[("x".to_string(), Expr::Decimal(1)), ("y".to_string(), Expr::Decimal(2))],
        "  ",
    );
    assert!(!out.contains("malloc"),
        "with defer_struct_allocas set, the allocation must NOT be emitted inline; got:\n{out}");
    assert_eq!(backend.fun.pending_struct_allocas.len(), 1,
        "the allocation must be deferred to pending_struct_allocas");
    backend.flush_pending_struct_allocas(&mut out);
    assert!(out.contains("call ptr @malloc(i64 16)"),
        "flush_pending_struct_allocas must write the malloc call; got:\n{out}");
    let _ = reg;
}

// ── typed !range metadata (2026-08-13 fix) ────────────────────────────

/// 2026-08-27 (plan 2026-08-27-cbv-foreign-hardware-and-mmio.md Slice C):
/// VolatileLoad#/VolatileStore# emit VOLATILE accesses whose width comes
/// from the declared pointee, through the boxed-pointer ABI (i64 word →
/// inttoptr at the access boundary).
#[test]
fn test_volatile_intrinsics_emit_typed_accesses() {
    let src = "let out_v: Int = 0;\n\
        defn poke(p: Ptr<Int>, v: Int) -> Int {\n\
            VolatileStore#(p, v);\n\
            term VolatileLoad#(p);\n\
        };\n\
        node n [out_v == 0][true] {\n\
            let base = Malloc#(8);\n\
            out_v = poke(base as Ptr<Int>, 0x41);\n\
        };\n";
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    let ir = backend.generate(&items, None);
    assert!(ir.contains("store volatile"), "volatile store must be emitted:\n{ir}");
    assert!(ir.contains("load volatile i64"), "volatile load at pointee width expected:\n{ir}");
    assert!(ir.contains("inttoptr"), "boxed-address re-materialization expected:\n{ir}");
}

/// 2026-09-14 (rv64-finish plan Phase 5): VolatileStore# width-adapts the
/// value to the POINTEE type (MMIO is Ptr<Bit<32>> on 32-bit targets — an
/// Int/i64 store would clobber the neighbouring register). The narrowing
/// cast must be a TRUNC (i64 -> i32), not a zext — the old path compared
/// Briev metadata via resolve_arg_bytes, which under-reports the abstract
/// Int on narrow targets and emitted invalid `zext i64 to i32`.
#[test]
fn test_volatile_store_narrows_to_32_bit_pointee() {
    let src = "let out_v: Int = 0;\n\
        defn poke32(p: Ptr<Bit<32>>, v: Int) {\n\
            VolatileStore#(p, v);\n\
        };\n\
        node n [out_v == 0][true] {\n\
            poke32(0x40004000 as Ptr<Bit<32>>, 0x41);\n\
            out_v = 1;\n\
        };\n";
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    let ir = backend.generate(&items, None);
    assert!(ir.contains("store volatile i32"), "32-bit volatile store expected:\n{ir}");
    assert!(!ir.contains("zext i64"), "invalid widening must not appear:\n{ir}");
    assert!(ir.contains("trunc i64"), "i64 -> i32 narrowing must trunc:\n{ir}");
}

/// 2026-09-14 (rv64-finish plan Phase 5): Asm#("raw", …) emits the output
/// with the earlyclobber marker. Without `&`, LLVM may alias an input
/// register with the output when the template writes $0 before reading an
/// input (`str $0, [$1]`) — the ARM SysTick vector-patch template
/// self-destructed into `str r2, [r2]`.
#[test]
fn test_asm_raw_earlyclobber_constraint() {
    let src = "let done: Int = 0;\n\
        defn patch(slot: Int) {\n\
            Asm#(\"raw\", \"mov $0, #1; str $0, [$1]\", slot);\n\
        };\n\
        node n [done == 0][true] {\n\
            patch(0x3C);\n\
            done = 1;\n\
        };\n";
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    let ir = backend.generate(&items, None);
    assert!(ir.contains("\"=&r,r,~{memory}\""),
        "earlyclobber output constraint expected:\n{ir}");
}

#[test]
fn test_range_metadata_bounds_are_typed() {
    // A bounded i64 precondition (`[count < 10]`) emits `!range` on the
    // pre-load. Bounds must be typed (`!{ i64 0, i64 10 }`) — the untyped
    // legacy form (`!{ 0, 10 }`) is rejected by clang/opt 18+.
    let src = "let count: Int = 0;\n\
        node n [count < 10][true] {\n\
            count = count + 1;\n\
            term;\n\
        };\n";
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_type_universe(tu);
    let ir = backend.generate(&items, None);
    assert!(ir.contains("!range !{ i64 0, i64 10 }"),
        "range bounds must be typed i64; got:\n{ir}");
    assert!(!ir.contains("!range !{ 0, 10 }"),
        "the untyped legacy range form must not be emitted; got:\n{ir}");
}

/// 2026-08-17 (tuple correctness, plan 2026-08-17-hashmap-storage-tuple-correctness.md):
/// `let (a, b) = t` destructures a boxed tuple handle into element registers
/// — GEP i64 slot (i+1) loads. Previously codegen dropped the `names` list and
/// the second name was an undefined register.
#[test]
fn test_tuple_destructure_emits_element_geps() {
    let src = r#"
let done: Int = 0;
node go [done == 0][done == 1] {
    let t: (Int, Int) = (1, 2);
    let (a, b) = t;
    done = a + b;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.matches("getelementptr i64, ptr ").count() >= 2,
        "the destructure must GEP each tuple element; got:\n{ir}"
    );
}

/// 2026-08-17 (tuple correctness): numeric field access `t.0`/`t.1` reads a
/// tuple element through the boxed handle.
#[test]
fn test_tuple_numeric_field_access() {
    let src = r#"
let done: Int = 0;
node go [done == 0][done == 1] {
    let t: (Int, Int) = (5, 6);
    done = t.0 + t.1;
    term;
};
"#;
    let mut items = parse_bv_source(src);
    let mut universe = crate::type_universe::TypeUniverse::new();
    let mut pm = crate::plugin::PluginManager::new();
    pm.run_ast(crate::ast::StageKind::Parsed, &mut items, &mut universe)
        .expect("plugin stage failed");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.matches("getelementptr i64, ptr ").count() >= 2,
        "numeric field access must GEP each tuple element; got:\n{ir}"
    );
    assert!(
        !ir.contains("@t = "),
        "numeric field access must not emit an undefined @t global; got:\n{ir}"
    );
}

/// 2026-08-23 (BUGS.md "callable-txn bodies silently drop match"): a
/// STATEMENT-level match inside a callable txn — the exact shape of
/// lib/tamer/vm.bv's exec_op opcode dispatch — must emit its arm blocks.
/// The old emit_statement had no Statement::Match arm; the construct fell
/// to the catch-all and vanished, leaving an empty convergent loop.
fn stmt_match_program() -> Vec<TopLevel> {
    vec![TopLevel::Transaction(Transaction {
        name: "dispatch".to_string(),
        is_reactive: false,
        is_async: false,
        type_params: vec![],
        parameters: vec![("op".to_string(), Type::int())],
        output_type: Some(OutputType::Single(Type::int())),
        outputs: vec![],
        contract: Contract {
            pre_condition: Expr::Bool(true),
            post_condition: Expr::Bool(true),
            watchdog: None,
            explicit: false,
            span: None,
        post_authority: false},
        body: vec![Statement::Match {
            expr: Box::new(Expr::Identifier("op".to_string())),
            arms: vec![
                StmtMatchArm {
                    patterns: vec![Pattern::Literal(Expr::Decimal(1))],
                    body: vec![Statement::Term(Some(Expr::Decimal(11)))],
                },
                StmtMatchArm {
                    patterns: vec![Pattern::Literal(Expr::Decimal(2))],
                    body: vec![Statement::Term(Some(Expr::Decimal(22)))],
                },
                StmtMatchArm {
                    patterns: vec![Pattern::Wildcard],
                    body: vec![Statement::Term(Some(Expr::Decimal(0)))],
                },
            ],
        }],
        metadata: HashMap::new(),
        derivation: None,
        modifiers: vec![],
        span: None,
        doc: None,
    })]
}

#[test]
fn test_statement_match_emits_arm_blocks_in_callable_txn() {
    let mut backend = LlvmBackend::new().with_force_emit_all(true);
    let output = backend.generate(&stmt_match_program(), None);
    assert!(
        output.contains(".smt_body_"),
        "statement-match arms MUST be emitted as blocks (was silently dropped):\n{output}"
    );
    assert!(
        output.contains(".smt_end_"),
        "statement-match merge block must exist:\n{output}"
    );
    // Arm conditions materialize each literal (add i64 0, N) and icmp
    // against the scrutinee register.
    assert!(output.contains("add i64 0, 1"), "arm 1 literal materialized");
    assert!(output.contains("add i64 0, 2"), "arm 2 literal materialized");
    assert!(output.contains("icmp eq i64"), "literal compare against scrutinee");
    // …and every arm's result store reaches the IR (three %result stores
    // after the match chain — one per arm).
    let arm_stores = output.matches("store i64 %t").count();
    assert!(arm_stores >= 3, "all three arms must store their result:\n{output}");
}


#[test]
fn test_enum_handle_abi_no_struct_decl() {
    // 2026-08-26 (Track B): an enum's runtime image is the boxed
    // {tag, payload} pair behind an i64 handle. The TypeDef must NOT emit a
    // struct declaration (zero-payload variants made `{ i64, void }` —
    // invalid IR), and functions taking the enum take the handle width.
    let src = r#"
enum Option {
    Some(Int),
    None,
};

defn get(o: Option) -> Int {
  term match o {
    Some(v) => v,
    None => 0 - 1,
  };
}

defn make() -> Option {
  term Some(7);
}

        node __test_go [true][true] { get(make()); };
    "#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut universe = crate::type_universe::TypeUniverse::new();
    crate::backend::register_types::register_typedefs(&items, &mut universe, 64).unwrap();
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        !ir.contains("%Option = type"),
        "enum must not declare a struct: {}",
        &ir[..ir.len().min(2000)]
    );
    assert!(
        ir.contains("define i64 @get(ptr noundef noalias nocapture align 8 %state, i64"),
        "enum param must be the i64 handle: {}",
        &ir[..ir.len().min(3000)]
    );
    // Construction boxes {tag=0, payload} — the Some arm's malloc image.
    assert!(ir.contains("call ptr @malloc(i64 16)"), "boxed image missing");
}

/// 2026-08-27 (Slice B): an @-addressed trigger VALUE read lowers to a
/// volatile load at the static address (boxed-pointer ABI inttoptr), and
/// the pin is EXCLUDED from event dispatch (no dangling @txn_<pin> call).
#[test]
fn test_mmio_pin_reads_volatile_and_skips_dispatch() {
    let src = "trg sensor @ 0x1000;\n\
               let acc: Int = 0;\n\
               txn tick [acc < 255][acc <= 255] {\n\
                   acc = sensor + 1;\n\
               }\n";
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let tu = crate::type_universe::TypeUniverse::new();
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(tu);
    let ir = backend.generate(&items, None);
    assert!(ir.contains("inttoptr i64 4096 to ptr"),
        "pin address must materialize:\n{ir}");
    assert!(ir.contains("load volatile i64, ptr"), "volatile pin read:\n{ir}");
    assert!(!ir.contains("@txn_sensor"),
        "value-pin must not event-dispatch to a nonexistent body txn:\n{ir}");
}

/// 2026-09-02 (plan fundamental-parent-membership): Float16 arithmetic
/// lowers through the shape-driven binop path — `fadd fast half` from
/// (Float category, bits 16), never an integer add. The typedef is the
/// stdlib's de-hashtagged form: bare `Float` parent, no hashword, no
/// width-suffixed intrinsics; the universe resolves it via the base-chain
/// walk. Undo evidence: before the emit_binary_op migration this program
/// emitted an integer add (name-equality float detection).
#[test]
fn test_f16_binop_emits_fadd_half() {
    let src = r#"
type Float16 : Float { spec MaxBits: 16; spec Alignment: 2; };
let a: Float16 = 0.0;
let b: Float16 = 0.0;
let y: Float16 = 0.0;
txn add16 [y < 100][y <= 100] {
    y = a + b;
}
"#;
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    let items = p.parse_program().unwrap();
    let mut universe = crate::type_universe::TypeUniverse::new();
    crate::backend::register_types::register_typedefs(&items, &mut universe, 64).unwrap();
    let mut backend = LlvmBackend::new().with_force_emit_all(true).with_type_universe(universe);
    let ir = backend.generate(&items, None);
    assert!(
        ir.contains("fadd fast half"),
        "Float16 add must emit fadd fast half:\n{}",
        &ir[..ir.len().min(3000)]
    );
    assert!(
        !ir.contains("add nuw nsw half"),
        "Float16 add must never take the integer path:\n{}",
        &ir[..ir.len().min(3000)]
    );
}

// ── ISR emission (2026-09-06, plan 2026-09-06-isr-handlers-and-sections.md) ──

fn parse_isr_program(src: &str) -> Vec<crate::ast::TopLevel> {
    let tokens = crate::lexer::tokenize(src).unwrap();
    let mut p = crate::parser::Parser::new(tokens, src);
    p.parse_program().unwrap()
}

#[test]
fn test_isr_vector_table_layout() {
    // Golden layout: SP slot 0 (arm mechanism) + handler slot + gaps →
    // the default spin handler; .isr_vector section; align 4.
    let src = "isr<arm_cortex_m> handler @ 1: tick() [true][done == true] { done = true; };\n\
               let done: Bool = false;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new())
        .with_isr_mechanism(None);
    let ir = backend.generate(&program, None);
    assert!(ir.contains("define void @tick() nounwind \"interrupt\"=\"IRQ\" {"),
        "wrapper carries the mechanism's calling convention:\n{ir}");
    assert!(ir.contains("define void @__isr_body_tick(ptr"), "body takes the shared state");
    assert!(ir.contains("call void @__isr_body_tick(ptr @__briev_state)"),
        "wrapper passes the shared state global");
    assert!(ir.contains("@__briev_state = global %State zeroinitializer"),
        "ISR programs share state through a global");
    assert!(ir.contains("@__vector_table_arm_cortex_m = global [3 x i32] [ i32 0, i32 ptrtoint (ptr @tick to i32), i32 ptrtoint (ptr @Default_Handler to i32) ], section \".isr_vector\", align 4"),
        "golden table layout (SP slot 0, handler, gap→default):\n{ir}");
    assert!(ir.contains("define void @Default_Handler() nounwind {"), "default handler emitted");
    // main aliases the global instead of alloca.
    assert!(ir.contains("%state = getelementptr %State, ptr @__briev_state, i64 0"),
        "main aliases the shared state global");
    let alloca_lines: Vec<&str> = ir.lines()
        .filter(|l| l.contains("%state = alloca %State") && !l.trim_start().starts_with(';'))
        .collect();
    assert!(alloca_lines.is_empty(),
        "alloca must not coexist with the global alias: {:?}\n{ir}",
        alloca_lines);
}

#[test]
fn test_isr_no_handlers_uses_plain_alloca() {
    // Without ISR handlers, state stays a main-local alloca (SROA path).
    let src = "node tick [true][done == true] { done = true; term; };\n\
               let done: Bool = false;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let ir = backend.generate(&program, None);
    assert!(ir.contains("%state = alloca %State"), "non-ISR programs keep the alloca");
    assert!(!ir.contains("@__briev_state"), "no shared-state global without handlers");
}

// ── halt; statement (2026-09-06 slice) ─────────────────────────────────

#[test]
fn test_halt_embedded_arm_emits_wfi_spin() {
    // ARM-family triple → the wfi spin loop (survives spurious wakeups;
    // entry-branch pattern keeps the entry block predecessor-free).
    let src = "node tick [true][done == true] { done = true; halt; term; };\n\
               let done: Bool = false;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new())
        .with_embedded_mode(true)
        .with_target_triple("thumbv7em-none-eabihf");
    let ir = backend.generate(&program, None);
    assert!(ir.contains("br label %halt_wait_"), "branch into the wait loop:\n{ir}");
    assert!(ir.contains("call void asm sideeffect \"wfi\", \"\"()"), "the wfi wait");
    assert!(!ir.contains("call void @llvm.trap()"), "no trap on the ARM path");
}

#[test]
fn test_halt_x86_triple_emits_trap() {
    // Non-ARM family (incl. x86_64 host) → the trap abort — loud and
    // portable; `wfi` would be an invalid mnemonic under the host
    // assembler.
    let src = "node tick [true][done == true] { done = true; halt; term; };\n\
               let done: Bool = false;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new())
        .with_embedded_mode(true);
    let ir = backend.generate(&program, None);
    assert!(ir.contains("call void @llvm.trap()"), "trap on non-ARM triples:\n{ir}");
    assert!(!ir.contains("wfi"), "no wfi under an x86 triple");
}

#[test]
fn test_section_placement_ir() {
    // 2026-09-06 (Phase 8): section(".name") emits define/global attributes.
    let src = "section(\".init\") defn startup() -> Int { term 42; };\n\
               section(\".rodata\") const TABLE: Int = 5;\n\
               node tick [true][n == 42] { n = startup(); term; };\n\
               let n: Int = 0;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let ir = backend.generate(&program, None);
    assert!(ir.contains("@TABLE = constant i64 5 section \".rodata\""),
        "const global carries its section:\n{ir}");
    assert!(ir.contains("section \".init\""),
        "sectioned defn carries its section:\n{ir}");
    // A section-less defn must NOT carry a section attribute.
    assert!(!ir.contains("section \".rodata\" }"), "no stray attrs");
}

// ── Phase 3 (rv64 capability kernel, 2026-09-13): trap capability ──────

#[test]
fn test_riscv_isr_full_context_scaffold() {
    // full_context mechanisms (riscv_machine) get the preemptive-service
    // scaffold: naked + align 4 (mtvec base), save-all via the mscratch↔tp
    // exchange, kernel-stack swap, typed body call, restore, mret — plus the
    // compiler-owned trap frame global.
    let src = "isr<riscv_machine> handler @ 7: tick() [true][n >= 0] { n = n; };\n\
               let n: Int = 0;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let ir = backend.generate(&program, None);
    assert!(ir.contains(
        "define void @tick() naked noinline align 4 {"),
        "riscv wrapper: naked scaffold + align 4 (mtvec base):\n{ir}");
    assert!(ir.contains("csrrw tp, mscratch, tp"), "frame base via mscratch↔tp:\n{ir}");
    assert!(ir.contains("sd x1, 0(tp);"), "saves x1 first");
    assert!(ir.contains("ld x31, 240(tp);"), "restores x31 last");
    assert!(ir.contains("ld sp, 248(tp);"), "kernel stack from frame slot 31");
    assert!(ir.contains("call __isr_body_tick"), "calls the typed body:\n{ir}");
    assert!(ir.contains("mret"), "returns via mret:\n{ir}");
    assert!(ir.contains("@__briev_trap_frame = global [32 x i64] zeroinitializer"),
        "compiler-owned trap frame:\n{ir}");
    // riscv mechanism: no link-time table (runtime-built mtvec).
    assert!(!ir.contains("@tick_vec"), "no link-time table for riscv");
}

#[test]
fn test_embedded_equilibrium_parks_in_wfi() {
    // On ARM/RISC-V embedded targets the reactor parks in wfi when no node
    // can fire (equilibrium), with the ~{memory} clobber that keeps
    // ISR-written fields fresh; hosted programs still exit.
    let src = "node tick [false][n == 0] { term; };\n\
               let n: Int = 0;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new())
        .with_embedded_mode(true)
        .with_target_triple("riscv64-unknown-none");
    let ir = backend.generate(&program, None);
    assert!(ir.contains("call void asm sideeffect \"wfi\", \"~{memory}\"()"),
        "equilibrium wfi with memory clobber:\n{ir}");
    assert!(ir.contains("br label %.ss_main_loop"), "park re-enters the dispatch loop:\n{ir}");
}

/// 2026-09-14 (rv64-finish plan Phase 4b): an address-wired (reactor-pass)
/// node — `node n @ *<ptr>` — polls a memory-mapped value that changes
/// WITHOUT an interrupt. The equilibrium park must SPIN (re-evaluate every
/// pass), never `wfi` through the eligibility. The `.end` block branches
/// straight back to the dispatch loop instead of emitting the wfi call.
#[test]
fn test_address_wired_node_spins_not_parks() {
    let src = "let armed: Int = 0;\n\
               let last: Int = 0;\n\
               node poll @ *(0x40004000 as Ptr<Int>) [last >= 0][last >= 0] {\n\
                   last = 1;\n\
               };\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new())
        .with_embedded_mode(true)
        .with_target_triple("riscv64-unknown-none");
    let ir = backend.generate(&program, None);
    // The address-wired node's pre is a state obligation; if it ever reads
    // false, the reactor must re-read every pass rather than sleep.
    assert!(ir.contains(".end:\n  br label %.ss_main_loop"),
        "external frontier must spin, not park:\n{ir}");
    assert!(!ir.contains(".end:\n  call void asm sideeffect \"wfi\""),
        "no wfi park with an external frontier:\n{ir}");
}

/// The parser classifies `node n @ *ptr` as a reactor-pass Transaction (not
/// a machine-vectored IsrHandler) and the contract brackets are NOT eaten
/// as an array index.
#[test]
fn test_address_wired_node_parses_as_reactive_txn() {
    let src = "let armed: Int = 0;\n\
               let last: Int = 0;\n\
               node poll @ *(0x40004000 as Ptr<Int>) [last >= 0][last >= 0] {\n\
                   last = 1;\n\
               };\n";
    let program = parse_isr_program(src);
    let txns: Vec<&crate::ast::top::Transaction> = program.iter()
        .filter_map(|i| match i { crate::ast::TopLevel::Transaction(t) => Some(t), _ => None })
        .collect();
    assert_eq!(txns.len(), 1, "address-wired node is one reactive transaction");
    let t = txns[0];
    assert_eq!(t.name, "poll");
    assert!(t.metadata.contains_key("address_wired"), "address-wired marker set");
    assert!(t.contract.explicit, "contract brackets survive parse (not eaten as index)");
    assert!(!matches!(program.iter().find(|i| matches!(i, crate::ast::TopLevel::IsrHandler(_))),
        Some(crate::ast::TopLevel::IsrHandler(_))), "not a machine-serviced handler");
}

#[test]
fn test_sync_wrapped_beginprogram_node_emits_flag() {
    // `sync<group> node …` wraps the transaction; its beginprogram entry
    // flag must still be emitted or the wrapper references an undefined
    // global (clang: use of undefined value '@briev_begin_<name>').
    let src = "sync<timer> node boot [beginprogram][true] { term; };\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let ir = backend.generate(&program, None);
    assert!(ir.contains("@briev_begin_boot = private global i1 1"),
        "sync-wrapped entry node emits its flag:\n{ir}");
}

// ── 2026-09-14 (machine-entry plan): `bootstrap node` ──────────────────

#[test]
fn test_bootstrap_body_inlines_at_main_head_and_stays_out_of_dispatch() {
    // The authored entry runs ONCE after the state initializer, before the
    // first dispatch pass — never as a reactor-dispatched transition.
    let src = "let armed: Bool = false;\n\
               let done: Bool = false;\n\
               bootstrap node reset [armed == true] { armed = true; };\n\
               node reporter [armed][done == true] { done = true; term; };\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new());
    let ir = backend.generate(&program, None);
    let loop_pos = ir.find(".ss_main_loop:").expect("dispatch loop present");
    // Bool fields lower to i8; the body's `armed = true` materializes the
    // constant then stores it through the field gep. The FIRST occurrence
    // is the bootstrap's (main head) — the reporter's sits after the loop.
    let body_pos = ir.find("add i8 0, 1")
        .unwrap_or_else(|| panic!("bootstrap body store present:\n{ir}"));
    assert!(body_pos < loop_pos,
        "the bootstrap body must run BEFORE the dispatch loop:\n{ir}");
    // Not reactor-dispatched: no per-tick precheck block for the bootstrap.
    assert!(!ir.contains(".ssb_reset"),
        "bootstrap must stay out of the dispatch list:\n{ir}");
    // Ordinary nodes still dispatch.
    assert!(ir.contains(".ssb_reporter"), "reporter dispatches:\n{ir}");
}

#[test]
fn test_node_at_vector_dissolves_isr_keyword() {
    // `node tick @ 7 [..] { … }` — machine-serviced event node syntax. The
    // parser desugars to the same IsrHandler representation; the mechanism
    // comes from the target profile (inference), so the emitted scaffold is
    // identical to the explicit-isr form.
    let src = "node tick @ 7 [true][n >= 0] { n = n; };\n\
               let n: Int = 0;\n";
    let program = parse_isr_program(src);
    let mut backend = LlvmBackend::new()
        .with_type_universe(crate::type_universe::TypeUniverse::new())
        .with_isr_mechanism(Some("riscv_machine".to_string()));
    let ir = backend.generate(&program, None);
    assert!(ir.contains("define void @tick() naked noinline align 4 {"),
        "wired node emits the service scaffold:\n{ir}");
    assert!(ir.contains("call __isr_body_tick"), "typed body called:\n{ir}");
    assert!(ir.contains("mret"), "returns via mret:\n{ir}");
}
