//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// Authored by Otto Westerlund (@odecode); adopted with attribution.
use super::{LlvmBackend, TypedRegister};
use crate::ast::{Expr, Type};

#[test]
fn stateless_calls_use_declared_integer_width() {
    let mut backend = LlvmBackend::new().with_int_bits(32);
    backend.ctx.defn_params.insert("accept".into(), vec![Type::bits(64)]);
    backend.ctx.defn_return_types.insert("accept".into(), vec![Type::bits(64)]);
    backend.ctx.defn_needs_state.insert("accept".into(), false);
    let mut output = String::new();
    let value = backend.emit_user_call(&mut output, "%result", "accept", &[Expr::Decimal(7)], "  ");
    assert_eq!(value.ty, Type::bits(64));
    assert!(output.contains("call i64 @accept(i64 "), "{output}");
    assert!(!output.contains("@accept(i32 "), "{output}");
}

#[test]
fn stateful_helpers_do_not_publish_partial_transactions() {
    let source = "let count: Int = 0; defn update() -> Int { count = count+1; term count; }; node run [count < 1][count == 1] { update(); term; };";
    let (items, universe) = crate::library::parse_and_check("scratch/helper_flush.bv", source).unwrap();
    let ir = LlvmBackend::new().with_type_universe(universe).with_webstack(true).generate(&items, None);
    let function = ir.split("define i64 @update(").nth(1).expect("live helper");
    let body = function.split("\n}").next().unwrap();
    assert!(body.contains("%state"), "helper must exercise stateful emission");
    assert!(!body.contains("@__web_flush_state"), "{body}");
}

#[test]
fn boolean_match_literal_uses_native_boolean_width() {
    let (items, universe) = crate::library::parse_and_check("scratch/match_bool.bv",
        "defn keep(flag: Bool) -> Bool { term flag; };").unwrap();
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    backend.generate(&items, None);
    let mut ir = String::new();
    let value = TypedRegister { name: "%flag".into(), ty: Type::bool_() };
    backend.emit_pattern_condition(&crate::ast::Pattern::Literal(Expr::Bool(true)),
        &value.name, &value.ty, &mut ir, "  ");
    assert!(!ir.contains("icmp eq i64 %flag"), "{ir}");
    assert!(ir.contains("icmp eq i8 %flag"), "{ir}");
}

#[test]
fn pooled_object_replacement_survives_reactor_firings() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
    let source = r#"
        obj Snapshot {
            ready: Bool; count: Int; weight: Float; title: String;
            defn value() -> Int { term count; };
        };
        defn initial_snapshot() -> Snapshot {
            term Snapshot { ready: true, count: 0, weight: 0.5, title: "alive" };
        };
        defn advance(value: Snapshot) -> Snapshot {
            term Snapshot { ready: true, count: value.count+1, weight: value.weight, title: value.title };
        };
        out let snapshot: Snapshot = initial_snapshot();
        out let iterations: Int = 0;
        out node step [true][false] {
            when !snapshot.ready || snapshot.weight != 0.5 || snapshot.title .^Length != 5 { SysCall#(60,71); };
            snapshot = advance(snapshot);
            iterations += 1;
            when snapshot.count != iterations { SysCall#(60,72); };
            when iterations >= 3 { endprogram 0; };
            term;
        };
    "#;
    let (items, universe) = crate::library::parse_and_check("scratch/pooled_replace.bv", source).unwrap();
    let ir = LlvmBackend::new().with_type_universe(universe).generate(&items, None);
    for optimization in ["-O0", "-O2"] {
        super::logical_tests::clang_ir(&ir, optimization, true);
    }
}

fn normalized_program(source: &str) -> (Vec<crate::ast::TopLevel>, crate::type_universe::TypeUniverse) {
    let (items, mut universe) = crate::library::parse_and_check("scratch/layout_regression.bv", source)
        .expect("checked layout regression");
    crate::backend::register_types::register_typedefs(&items, &mut universe, 64)
        .expect("resolved regression types");
    (items, universe)
}

#[test]
fn nested_object_enum_payload_fits_on_native_and_wasm_targets() {
    let source = r#"
        obj Stamp { epoch: String; account: Int; revision: String; };
        obj Receipt { stamp: Stamp; operation: String; };
        enum Event { Saved(Receipt), Empty };
        defn receipt() -> Receipt {
            term Receipt { stamp: Stamp { epoch: "epoch", account: 7, revision: "9" },
                operation: "operation" };
        };
        defn wrap() -> Event { term Saved(receipt()); };
        let current: Event = wrap();
    "#;
    let (items, universe) = normalized_program(source);
    for bits in [32, 64] {
        let mut backend = LlvmBackend::new().with_type_universe(universe.clone()).with_int_bits(bits);
        if bits == 32 {
            backend = backend.with_target_triple("wasm32-unknown-wasi");
        }
        let ir = backend.generate(&items, None);
        assert!(!ir.is_empty(), "nested payload must reach code generation");
    }
}

#[test]
fn enum_field_constructor_uses_its_declared_layout_and_restores_context() {
    let source = r#"
        obj Stamp { epoch: String; account: Int; revision: String; };
        obj Tag { epoch: String; account: Int; revision: String; job: String; attempt: Int; generation: String; };
        enum Job { NoJob, Owned(Tag) };
        obj Record { job: Job; };
        enum Status { Success(Record), Failure(Stamp) };
    "#;
    let (items, universe) = normalized_program(source);
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    backend.generate(&items, None);
    let enclosing = Type::Custom("Status".into());
    backend.fun.expected_enum_type = Some(enclosing.clone());
    backend.fun.let_bindings.insert("input".into(), "%input".into());
    backend.fun.let_binding_types.insert("input".into(), Type::Custom("Tag".into()));
    let fields = vec![("job".into(), Expr::Call("Owned".into(),
        vec![Expr::Identifier("input".into())], None))];
    let mut ir = String::new();
    backend.emit_struct_literal(&mut ir, "%record", "Record", &fields, "  ");
    assert_eq!(backend.fun.expected_enum_type, Some(enclosing));
    assert!(!ir.is_empty(), "field construction must reach code generation");
}
#[test]
fn extracted_enum_objects_survive_the_enum_result_frame() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
    let source = include_str!("../../../tests/tier1/enum_object_projection_lifetime.bv");
    let (items, universe) = normalized_program(source);
    let ir = LlvmBackend::new().with_type_universe(universe).generate(&items, None);
    for optimization in ["-O0", "-O2"] {
        super::logical_tests::clang_ir(&ir, optimization, true);
    }
}

#[test]
fn callable_transaction_local_slots_are_inside_the_function() {
    let source = r#"
        let count: Int = 0;
        txn choose(value: Int) [value > 0][count == value] {
            let local: Int = 0;
            when value > 0 { local = value; };
            count = local;
            term;
        };
        node drive [count == 0][count > 0] { choose(3); term; };
    "#;
    let (items, universe) = crate::library::parse_and_check("scratch/callable_local.bv", source)
        .expect("checked callable transaction");
    let ir = LlvmBackend::new().with_type_universe(universe).generate(&items, None);
    let mut in_function = false;
    for line in ir.lines() {
        if line.trim_start().starts_with(';') { continue; }
        if line.starts_with("define ") { in_function = true; }
        assert!(in_function || !line.contains(" = alloca "), "module-level allocation: {line}");
        if line == "}" { in_function = false; }
    }
    assert!(ir.contains(" = alloca "), "exercise stack allocation");
    assert_unique_ssa_definitions(&ir);
}

fn assert_unique_ssa_definitions(ir: &str) {
    let mut names = std::collections::HashSet::new();
    for line in ir.lines().map(str::trim) {
        if line.starts_with(';') { continue; }
        if line.starts_with("define ") { names.clear(); }
        let Some((name, _)) = line.split_once(" = ") else { continue; };
        assert!(!name.starts_with('%') || names.insert(name), "duplicate local: {name}");
    }
}

#[test]
fn inline_members_do_not_publish_the_enclosing_browser_transaction() {
    let source = r#"
        obj Counter {
            value: Int;
            defn read() -> Int { term value; };
        };
        let counter: Counter = Counter { value: 1 };
        let chosen: Int = 0;
        txn choose(value: Int) [value > 0][chosen == value] {
            let current: Int = counter.read();
            chosen = value+current-1;
            term;
        };
        node drive [chosen == 0][chosen > 0] { choose(1); term; };
    "#;
    let (items, universe) = normalized_program(source);
    let ir = LlvmBackend::new().with_type_universe(universe).with_webstack(true)
        .with_int_bits(32).with_target_triple("wasm32-unknown-wasi").generate(&items, None);
    let function = ir.split("define void @choose(").nth(1).expect("callable transaction");
    let body = function.split("\n}").next().expect("function body");
    assert_eq!(body.matches("call void @__web_flush_state(").count(), 1,
        "only the outer transaction may publish browser state: {body}");
}

#[test]
fn guarded_tail_backedge_does_not_swallow_later_return() {
    let source = r#"
        defn identity(value: Int) -> Int { term value; };
        defn descend(n: Int) -> Int {
            when n > 0 { term descend(n-1); };
            term identity(7);
        };
        let answer: Int = descend(2);
    "#;
    let (items, universe) = crate::library::parse_and_check("scratch/tail_guard_return.bv", source)
        .expect("checked tail recursion");
    let ir = LlvmBackend::new().with_type_universe(universe).with_int_bits(32)
        .with_target_triple("wasm32-unknown-wasi").generate(&items, None);
    let function = ir.split("define i32 @descend(").nth(1).expect("live recursive function");
    let body = function.split("\n}").next().expect("function body");
    assert!(body.contains(".tail_header_"), "must exercise tail-loop lowering: {body}");
    assert!(body.contains("call i32 @identity("), "must exercise the later return: {body}");
    assert!(!body.contains("ret i32 0"), "later call result was discarded: {body}");
}
