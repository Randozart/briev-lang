//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//

use super::{FunctionContext, LlvmBackend};
use crate::ast::{BinaryOpKind, Expr, Statement};
use std::process::Command;

fn clang_ir(ir: &str, optimization: &str, run: bool) {
    if Command::new("clang").arg("--version").output().is_err() { return; }
    let directory = tempfile::TempDir::new().expect("IR fixture directory");
    let input = directory.path().join("logical.ll");
    let output = directory.path().join(if run { "logical" } else { "logical.o" });
    std::fs::write(&input, ir).expect("write logical IR");
    let mut command = Command::new("clang");
    command.args([optimization, "-Wno-override-module", "-ffunction-sections", "-fdata-sections"]);
    command.arg(&input).arg("-o").arg(&output);
    if run {
        let runtime = directory.path().join("exit.c");
        std::fs::write(&runtime, "#include <stdint.h>\n#include <stdlib.h>\n_Noreturn void __exit(int64_t code) { exit((int)code); }\n").expect("exit ABI shim");
        command.arg(runtime).arg("-Wl,--gc-sections");
    } else { command.arg("-c"); }
    let compiled = command.output().expect("run clang");
    assert!(compiled.status.success(), "invalid logical IR: {}\n{ir}", String::from_utf8_lossy(&compiled.stderr));
    if !run { return; }
    let executed = Command::new(output).output().expect("execute logical fixture");
    assert!(executed.status.success(), "logical effects/guard failed: {:?}\n{}", executed.status, String::from_utf8_lossy(&executed.stderr));
}

#[test]
fn logical_guards_skip_invalid_reads_and_execute_required_rhs_once() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
    let source = r#"
        defn touch(address: Int) -> Bool {
            Store#(address,Load#(address,8)+1,8);
            term true;
        };
        defn logical_and(flag: Bool, address: Int) -> Bool { term flag && touch(address); };
        defn logical_or(flag: Bool, address: Int) -> Bool { term flag || touch(address); };
        defn local_guard(flag: Bool) -> Int {
            let value: Int = 10;
            let chosen: Bool = flag && { value = 20; true; };
            when chosen { term value; };
            term value;
        };
        defn parameter_guard(flag: Bool, value: Int) -> Int {
            let chosen: Bool = flag && { value = 20; true; };
            when chosen { term value; };
            term value;
        };
        defn float_parameter_guard(flag: Bool, value: Float) -> Float {
            let chosen: Bool = flag && { value = 2.5; true; };
            when chosen { term value; };
            term value;
        };
        defn bool_parameter_guard(flag: Bool, value: Bool) -> Bool {
            let chosen: Bool = flag && { value = false; true; };
            when chosen { term value; };
            term value;
        };
        defn string_parameter_guard(flag: Bool, value: String) -> Int {
            let chosen: Bool = flag && { value = "newer"; true; };
            when chosen { term value .^Length; };
            term value .^Length;
        };
        let tick: Int = 0;
        node run [tick < 1][tick == 1] {
            let flag: Bool = SysCall#(39) < 0;
            when logical_and(flag,0) || !logical_or(!flag,0) { endprogram 41; };
            let address: Int = Malloc#(8) as Int;
            when address == 0 { endprogram 42; };
            Store#(address,0,8);
            when !logical_and(!flag,address) || !logical_or(flag,address) { endprogram 43; };
            when Load#(address,8) != 2 { endprogram 44; };
            let nested: Bool = (flag || !flag) && (flag || touch(address));
            when !nested || Load#(address,8) != 3 { endprogram 45; };
            when local_guard(flag) != 10 || local_guard(!flag) != 20 { endprogram 46; };
            when parameter_guard(flag,10) != 10 || parameter_guard(!flag,10) != 20 { endprogram 47; };
            when float_parameter_guard(flag,1.5) != 1.5 || float_parameter_guard(!flag,1.5) != 2.5 { endprogram 48; };
            when !bool_parameter_guard(flag,true) || bool_parameter_guard(!flag,true) { endprogram 49; };
            when string_parameter_guard(flag,"old") != 3 || string_parameter_guard(!flag,"old") != 5 { endprogram 50; };
            let alias: Int = address+0;
            let skipped: Bool = flag && { Store#(alias,100,8); true; };
            when skipped || Load#(alias,8) != 3 { endprogram 51; };
            Free#(address as Ptr);
            tick = 1; endprogram;
        };
    "#;
    let (items, universe) = crate::library::parse_and_check("scratch/logical_guard.bv", source).expect("typed logical fixture");
    let ir = LlvmBackend::new().with_type_universe(universe).generate(&items, None);
    for optimization in ["-O0", "-O2"] { clang_ir(&ir, optimization, true); }
}

#[test]
fn nested_exit_predicates_merge_before_widening() {
    let (items, universe) = crate::library::parse_and_check("scratch/logical_exit.bv",
        "defn keep(flag: Bool) -> Bool { term flag; };").expect("typed Bool universe");
    let mut backend = LlvmBackend::new().with_type_universe(universe);
    backend.generate(&items, None);
    backend.fun = FunctionContext::new();
    let expression = Expr::BinaryOp(BinaryOpKind::And, Box::new(Expr::Bool(false)),
        Box::new(Expr::BinaryOp(BinaryOpKind::Or, Box::new(Expr::Bool(true)), Box::new(Expr::Bool(false)))));
    let mut ir = "define i64 @exit_probe() {\nentry:\n".to_string();
    let value = backend.emit_exit_expr(&mut ir, &expression, "  ");
    ir.push_str(&format!("  ret i64 {value}\n}}\n"));
    assert_eq!(ir.matches("phi i8").count(), 2);
    assert!(ir.contains("zext i8"), "exit ABI widens the final Bool: {ir}");
    clang_ir(&ir, "-O0", false);
}

#[test]
fn terminating_rhs_has_no_fabricated_phi_incoming() {
    let mut backend = LlvmBackend::new();
    let right = Expr::Block(vec![Statement::Trap]);
    let mut ir = "declare void @llvm.trap()\ndefine i8 @trap_guard() {\nentry:\n".to_string();
    let value = backend.emit_short_circuit(&mut ir, (&Expr::Bool(false), &right), false, "  ");
    ir.push_str(&format!("  ret i8 {}\n}}\n", value.name));
    assert!(!backend.fun.terminated, "the short edge continues");
    assert!(!ir.contains("logical.value."), "a trapping edge has no value predecessor: {ir}");
    clang_ir(&ir, "-O0", false);
}
