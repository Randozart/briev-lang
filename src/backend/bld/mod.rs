// ── .bld backend — BILLD (Briev Intermediate Low-Level Dialect) ───────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-10-08 (BILLD plan, docs/plans/2026-10-08-billd-intermediate-dialect.md
// M3): entry point. A .bld program lowers to a `BadProgram` and emits
// through the .bad backend unchanged — same registries, same targets,
// same `.s`. Virtual value registers (`vN`, `fvN` for float) are
// INTENTIONAL in the M3 output: golden tests assert on the BadProgram
// shape, and only const/physical recipes reach `.s` end to end; the M4
// register allocator resolves the virtuals.
//
// To undo: delete src/backend/bld/ and revert `pub mod bld;` in
// src/backend/mod.rs.

pub mod alloc;
pub mod lower;
pub mod registry;

use crate::ast::bad::BadProgram;

/// Lower a .bld source to a `BadProgram` — the M3 boundary golden tests
/// assert on. `base_dir` resolves relative imports; `root_path` (when
/// known) seeds module dedup so import cycles back to the root file
/// terminate.
pub fn lower_to_bad(
    source: &str,
    family: &str,
    base_dir: Option<&std::path::Path>,
    root_path: Option<&std::path::Path>,
) -> Result<BadProgram, String> {
    let (isa, regs) = super::bad::registries();
    let mut lowerer = lower::BldLowerer::new(
        &isa,
        &regs,
        family,
        base_dir.map(|p| p.to_path_buf()),
        root_path.map(|p| p.to_path_buf()),
    );
    lowerer.load(source)?;
    lowerer.compile()
}

/// Lower `.bld → BadProgram → target .s text` through the .bad backend.
pub fn generate(source: &str, target_triple: &str) -> Result<String, String> {
    generate_with(source, target_triple, None, None)
}

/// `generate` with import resolution roots.
pub fn generate_with(
    source: &str,
    target_triple: &str,
    base_dir: Option<&std::path::Path>,
    root_path: Option<&std::path::Path>,
) -> Result<String, String> {
    let family = target_triple.split('-').next().unwrap_or(target_triple);
    let program = lower_to_bad(source, family, base_dir, root_path)?;
    let (isa, regs) = super::bad::registries();
    // M4 (2026-10-08): virtual value registers resolve to physical
    // registers (or frame slots) before the .bad backend sees them.
    let program = self::alloc::allocate(program, &regs, family)?;
    let mut lw = super::bad::lower::Lowerer::new(&isa, &regs, family)
        .with_base_dir(base_dir.map(|p| p.to_path_buf()));
    lw.run(&program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::bad::{BadBodyItem, BadOperand, BadTopLevel};

    fn lower_ok(src: &str) -> BadProgram {
        lower_to_bad(src, "x86_64", None, None)
            .unwrap_or_else(|e| panic!("lower failed: {e}"))
    }

    fn lower_err(src: &str) -> String {
        lower_to_bad(src, "x86_64", None, None)
            .err()
            .unwrap_or_else(|| panic!("expected an error, got success"))
    }

    fn instrs<'a>(prog: &'a BadProgram, label: &str) -> Vec<&'a BadBodyItem> {
        for item in &prog.items {
            if let BadTopLevel::Label(l) = item {
                if l.name == label {
                    return l.body.iter().collect();
                }
            }
        }
        panic!("no label `{label}` in the program");
    }

    fn mnems(items: &[&BadBodyItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                BadBodyItem::Instr(x) => Some(x.mnemonic.clone()),
                _ => None,
            })
            .collect()
    }

    fn labels(items: &[&BadBodyItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                BadBodyItem::Local(x) => Some(x.name.clone()),
                _ => None,
            })
            .collect()
    }

    fn op_text(o: &BadOperand) -> String {
        match o {
            BadOperand::Int(n) => n.to_string(),
            BadOperand::Float(t) => t.clone(),
            BadOperand::Name(n) => n.clone(),
            BadOperand::Expr(e) => e.clone(),
        }
    }

    fn ops_of(item: &BadBodyItem) -> Vec<String> {
        match item {
            BadBodyItem::Instr(x) => x.operands.iter().map(op_text).collect(),
            _ => Vec::new(),
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bld_test_{}_{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── program shape ─────────────────────────────────────────────────

    #[test]
    fn section_preamble_leads_the_program() {
        let p = lower_ok("bootstrap B() { return; }");
        assert!(matches!(
            &p.items[0],
            BadTopLevel::Directive(d) if d.name == "section" && d.args == ".text"
        ));
    }

    #[test]
    fn bootstrap_exports_global_label() {
        let p = lower_ok("bootstrap Reset() { return; }");
        assert!(matches!(
            &p.items[1],
            BadTopLevel::Directive(d) if d.name == "global" && d.args == "Reset"
        ));
        assert!(matches!(
            &p.items[2],
            BadTopLevel::Label(l) if l.name == "Reset" && !l.local
        ));
    }

    #[test]
    fn two_bootstraps_across_imports_are_loud() {
        let dir = temp_dir("twoboot");
        std::fs::write(dir.join("b.bld"), "bootstrap B() { return; }").unwrap();
        let e = lower_to_bad(
            "import \"b.bld\";\nbootstrap A() { return; }",
            "x86_64",
            Some(&dir),
            None,
        )
        .err()
        .unwrap();
        assert!(e.contains("exactly one entry point"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn consts_emit_as_dot_const_directives() {
        let p = lower_ok("const TOP = 0x90000;\nbootstrap B() { return; }");
        assert!(p.items.iter().any(|i| matches!(
            i,
            BadTopLevel::Directive(d) if d.name == ".const" && d.args == "TOP 589824"
        )));
    }

    // ── consts ────────────────────────────────────────────────────────

    #[test]
    fn const_folds_into_mov() {
        let p = lower_ok("const N = 2 + 3;\ndefn five() -> Int { return N; }");
        let items = instrs(&p, "five");
        assert_eq!(mnems(&items), vec!["mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["r0", "5"]);
    }

    #[test]
    fn const_division_by_zero_is_loud() {
        let e = lower_err("const Z = 1 / 0;\ndefn f() { return; }");
        assert!(e.contains("divides by zero"), "{e}");
    }

    #[test]
    fn const_cycle_is_loud() {
        let e = lower_err("const A = B + 1;\nconst B = A + 1;\ndefn f() { return; }");
        assert!(e.contains("depends on itself"), "{e}");
    }

    #[test]
    fn duplicate_const_is_loud() {
        let e = lower_err("const N = 1;\nconst N = 2;\ndefn f() { return; }");
        assert!(e.contains("more than once"), "{e}");
    }

    #[test]
    fn float_const_div_by_zero_folds_to_inf() {
        let src = "defn f() -> Float { let v = 1.0 / 0.0; return v; }";
        let prog = lower_ok(src);
        let items = instrs(&prog, "f");
        assert_eq!(mnems(&items), vec!["fmov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["f0", "inf"]);
    }

    // ── params and returns ────────────────────────────────────────────

    #[test]
    fn defn_params_bind_abi_registers() {
        let p = lower_ok("defn f(a: Int, b: Int) -> Int { return a; }");
        let items = instrs(&p, "f");
        // x86_64 abi_args[0] = r5
        assert_eq!(ops_of(items[0]), vec!["r0", "r5"]);
    }

    #[test]
    fn float_and_int_params_bind_separate_classes() {
        let p = lower_ok("defn g(x: Float, y: Int) -> Int { return y; }");
        let items = instrs(&p, "g");
        // x = fp0 (f0), y = integer arg #1 (r5)
        assert_eq!(ops_of(items[0]), vec!["r0", "r5"]);
    }

    #[test]
    fn untyped_param_is_loud() {
        let e = lower_err("defn f(a) { return; }");
        assert!(e.contains("no type"), "{e}");
    }

    #[test]
    fn too_many_int_params_is_loud() {
        let e = lower_err(
            "defn f(a: Int, b: Int, c: Int, d: Int, e2: Int, g: Int, h: Int) { return; }",
        );
        assert!(e.contains("only 6"), "{e}");
    }

    #[test]
    fn return_value_in_void_recipe_is_loud() {
        let e = lower_err("defn f() { return 1; }");
        assert!(e.contains("declares no return type"), "{e}");
    }

    #[test]
    fn bare_return_in_typed_recipe_is_loud() {
        let e = lower_err("defn f() -> Int { return; }");
        assert!(e.contains("cannot end it"), "{e}");
    }

    // ── expressions ───────────────────────────────────────────────────

    #[test]
    fn const_valued_let_emits_no_copy() {
        let src = "defn f() -> Int { let v = 5; return v; }";
        let prog = lower_ok(src);
        let items = instrs(&prog, "f");
        assert_eq!(mnems(&items), vec!["mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["r0", "5"]);
    }

    #[test]
    fn unknown_name_is_loud() {
        let e = lower_err("defn f() -> Int { return q; }");
        assert!(e.contains("not bound"), "{e}");
    }

    #[test]
    fn duplicate_let_in_scope_is_loud() {
        let e = lower_err("defn f() { let v = 1; let v = 2; }");
        assert!(e.contains("already bound"), "{e}");
    }

    #[test]
    fn nested_block_shadows_outer_binding() {
        let src = "defn f(a: Int) -> Int { let v = a + 1; { let v = 5; a = v; } return v; }";
        lower_ok(src);
    }

    #[test]
    fn string_literal_is_loud() {
        let e = lower_err("defn f() { let s = \"hi\"; }");
        assert!(e.contains("string literal"), "{e}");
    }

    #[test]
    fn field_assign_is_loud() {
        let e = lower_err("defn f(p: Ptr) { p.x = 1; }");
        assert!(e.contains("bad { }"), "{e}");
    }

    #[test]
    fn int_comparison_value_lowers_to_slt() {
        let src = "defn f(a: Int) -> Int { let b = a > 0; return b; }";
        let prog = lower_ok(src);
        let items = instrs(&prog, "f");
        assert_eq!(mnems(&items), vec!["slt", "mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["v0", "0", "r5"]);
    }

    #[test]
    fn le_uses_slt_plus_xor() {
        let src = "defn f(a: Int) -> Int { let b = a <= 9; return b; }";
        let prog = lower_ok(src);
        let items = instrs(&prog, "f");
        assert_eq!(mnems(&items), vec!["slt", "xor", "mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["v0", "9", "r5"]);
        assert_eq!(ops_of(items[1]), vec!["v0", "v0", "1"]);
    }

    #[test]
    fn eq_value_uses_dance() {
        let src = "defn f(a: Int) -> Int { let b = a == 7; return b; }";
        let prog = lower_ok(src);
        let items = instrs(&prog, "f");
        assert_eq!(mnems(&items), vec!["mov", "jnz", "jmp", "mov", "mov", "ret"]);
        assert_eq!(ops_of(items[1]), vec!["r5", "7", ".bf_0"]);
        assert_eq!(labels(&items), vec!["bf_0", "be_1"]);
    }

    #[test]
    fn float_comparison_materializes_constants() {
        let src = "defn f(x: Float) -> Int { let b = x == 1.5; return b; }";
        let prog = lower_ok(src);
        let items = instrs(&prog, "f");
        assert_eq!(mnems(&items)[2], "fjnz");
        assert_eq!(ops_of(items[2]), vec!["f0", "fv1", ".bf_0"]);
    }

    #[test]
    fn int_value_promotes_to_float_for_arithmetic() {
        let src = "defn f(n: Int, x: Float) -> Float { return x + n; }";
        let prog = lower_ok(src);
        let items = instrs(&prog, "f");
        assert_eq!(mnems(&items), vec!["itof", "fadd", "fmov", "ret"]);
    }

    #[test]
    fn x86_mul_takes_the_immediate_directly() {
        let p = lower_ok("defn f(a: Int) -> Int { return a * 3; }");
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["mul", "mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["v0", "r5", "3"]);
    }

    #[test]
    fn illegal_immediates_materialize_before_the_op() {
        // mul on aarch64 has no immediate form
        let p = lower_to_bad("defn f(a: Int) -> Int { return a * 3; }", "aarch64", None, None)
            .unwrap();
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["mov", "mul", "mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["v1", "3"]);
        assert_eq!(ops_of(items[1]), vec!["v0", "r0", "v1"]);
    }

    #[test]
    fn missing_target_row_is_a_capability_error() {
        // slt has no thumb row
        let e = lower_to_bad(
            "defn f(a: Int) -> Int { let b = a < 5; return b; }",
            "thumb",
            None,
            None,
        )
        .err()
        .unwrap();
        assert!(e.contains("thumb"), "{e}");
    }

    // ── control flow ──────────────────────────────────────────────────

    #[test]
    fn when_lowers_branch_and_local_labels() {
        let src = "defn f(a: Int) { when a < 10 { a = 20; } }";
        let p = lower_ok(src);
        let items = instrs(&p, "f");
        // jump past the body when the condition is false
        assert_eq!(mnems(&items), vec!["jge", "mov"]);
        assert_eq!(ops_of(items[0]), vec!["r5", "10", ".whe_0"]);
        assert_eq!(labels(&items), vec!["whe_0"]);
    }

    #[test]
    fn when_else_emits_both_labels() {
        let src = "defn f(a: Int) { when a == 0 { a = 1; } else { a = 2; } }";
        let p = lower_ok(src);
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["jnz", "mov", "jmp", "mov"]);
        assert_eq!(labels(&items), vec!["whe_0", "whend_1"]);
    }

    #[test]
    fn while_emits_head_test_and_backedge() {
        let src = "defn f(a: Int) { while a > 0 { a = a - 1; } }";
        let p = lower_ok(src);
        let items = instrs(&p, "f");
        assert_eq!(labels(&items), vec!["wlp_0", "wlpend_1"]);
        let m = mnems(&items);
        assert_eq!(m[0], "jle");
        assert_eq!(m.last().map(String::as_str), Some("jmp"));
    }

    #[test]
    fn loop_break_and_continue_jump_loop_labels() {
        let src = "defn spin(a: Int) { loop { when a == 0 { break; } continue; } }";
        let p = lower_ok(src);
        let items = instrs(&p, "spin");
        assert_eq!(labels(&items), vec!["lpe_0", "whe_2", "lpend_1"]);
        let m = mnems(&items);
        assert!(m.contains(&"jmp".to_string()));
    }

    #[test]
    fn break_outside_loop_is_loud() {
        let e = lower_err("defn f() { break; }");
        assert!(e.contains("outside"), "{e}");
    }

    #[test]
    fn continue_outside_loop_is_loud() {
        let e = lower_err("defn f() { continue; }");
        assert!(e.contains("outside"), "{e}");
    }

    #[test]
    fn and_condition_tests_each_conjunct() {
        let src = "defn f(a: Int, b: Int) { when a > 0 && b > 0 { a = 1; } }";
        let p = lower_ok(src);
        let items = instrs(&p, "f");
        // false polarity: each false conjunct jumps straight to the end
        assert_eq!(mnems(&items), vec!["jle", "jle", "mov"]);
        assert_eq!(labels(&items), vec!["whe_0"]);
    }

    #[test]
    fn or_condition_uses_a_skip_label() {
        let src = "defn f(a: Int, b: Int) { when a > 0 || b > 0 { a = 1; } }";
        let p = lower_ok(src);
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["jgt", "jle", "mov"]);
        assert_eq!(labels(&items), vec!["or_2", "whe_0"]);
    }

    // ── calls ─────────────────────────────────────────────────────────

    #[test]
    fn call_stages_args_into_abi_registers() {
        let src = "defn g(a: Int, b: Int) -> Int { return a; }\ndefn f() -> Int { return g(1, 2); }";
        let p = lower_ok(src);
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["mov", "mov", "mov", "mov", "call", "mov", "mov", "ret"]);
        assert_eq!(ops_of(items[2]), vec!["r5", "v0"]);
        assert_eq!(ops_of(items[3]), vec!["r4", "v1"]);
        assert_eq!(ops_of(items[4]), vec!["g"]);
    }

    #[test]
    fn call_arity_is_checked() {
        let e = lower_err(
            "defn g(a: Int) -> Int { return a; }\ndefn f() -> Int { return g(1, 2); }",
        );
        assert!(e.contains("takes 1 argument"), "{e}");
    }

    #[test]
    fn unknown_callee_is_loud() {
        let e = lower_err("defn f() { mystery(1); }");
        assert!(e.contains("no `defn`"), "{e}");
    }

    #[test]
    fn too_many_call_args_is_loud() {
        let src = "defn f() { ext(1, 2, 3, 4, 5, 6, 7); }\n\
                   defn ext(a: Int, b: Int, c: Int, d: Int, e2: Int, g: Int, h: Int) { return; }";
        let e = lower_err(src);
        assert!(e.contains("only 6"), "{e}");
    }

    #[test]
    fn module_path_call_is_loud_with_fix() {
        let e = lower_err("defn f() { other.thing(); }");
        assert!(e.contains("bare name"), "{e}");
    }

    // ── bad blocks ────────────────────────────────────────────────────

    #[test]
    fn bad_block_instructions_splice_into_the_body() {
        let src = "bootstrap B() { bad { mov r0, 42\nmov r1, r0 } }";
        let p = lower_ok(src);
        let items = instrs(&p, "B");
        assert_eq!(mnems(&items), vec!["mov", "mov"]);
        assert_eq!(ops_of(items[0]), vec!["r0", "42"]);
        assert_eq!(ops_of(items[1]), vec!["r1", "r0"]);
    }

    #[test]
    fn ownership_block_at_recipe_start_frames_the_label() {
        let src = "bootstrap B() { bad { section .text }\nreturn; }";
        let p = lower_ok(src);
        let sections: Vec<usize> = p
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, BadTopLevel::Directive(d) if d.name == "section"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(sections.len(), 2, "preamble + block section");
        let idx_label = p
            .items
            .iter()
            .position(|i| matches!(i, BadTopLevel::Label(l) if l.name == "B"))
            .unwrap();
        assert!(sections[1] < idx_label, "ownership precedes the label");
    }

    #[test]
    fn ownership_block_at_recipe_end_trails_the_label() {
        let src = "bootstrap B() { return;\nbad { msg: .asciz \"hi\" } }";
        let p = lower_ok(src);
        let idx_label = p
            .items
            .iter()
            .position(|i| matches!(i, BadTopLevel::Label(l) if l.name == "B"))
            .unwrap();
        assert!(matches!(
            p.items.get(idx_label + 1),
            Some(BadTopLevel::Data(d)) if d.name == "msg"
        ));
    }

    #[test]
    fn mid_recipe_ownership_is_loud() {
        let e = lower_err("bootstrap B() { return;\nbad { section .data }\nreturn; }");
        assert!(e.contains("first or last statement"), "{e}");
    }

    #[test]
    fn bad_block_parse_error_names_the_block() {
        let e = lower_err("bootstrap B() { bad { raw x86_64 } }");
        assert!(e.contains("does not parse"), "{e}");
    }

    // ── imports ───────────────────────────────────────────────────────

    #[test]
    fn bad_import_passes_through_and_harvests_signature() {
        let dir = temp_dir("badimport");
        std::fs::write(dir.join("ext.bad"), "start_here:\nret\n").unwrap();
        let src = "import \"ext.bad\";\ndefn f() { start_here(); }";
        let p = lower_to_bad(src, "x86_64", Some(&dir), None).unwrap();
        assert!(p
            .items
            .iter()
            .any(|i| matches!(i, BadTopLevel::Directive(d) if d.name == "import")));
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items).first().map(String::as_str), Some("call"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bld_import_diamond_merges_once() {
        let dir = temp_dir("diamond");
        std::fs::write(dir.join("leaf.bld"), "defn leaf() -> Int { return 1; }").unwrap();
        std::fs::write(
            dir.join("a.bld"),
            "import \"leaf.bld\";\ndefn a_fn() -> Int { return leaf(); }",
        )
        .unwrap();
        let src = "import \"leaf.bld\";\nimport \"a.bld\";\ndefn f() -> Int { return leaf(); }";
        let p = lower_to_bad(src, "x86_64", Some(&dir), None).unwrap();
        let count = p
            .items
            .iter()
            .filter(|i| matches!(i, BadTopLevel::Label(l) if l.name == "leaf"))
            .count();
        assert_eq!(count, 1, "diamond import merges once");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bld_import_cycle_with_root_path_terminates() {
        let dir = temp_dir("cycle");
        let root = dir.join("cyc_a.bld");
        std::fs::write(
            &root,
            "import \"cyc_b.bld\";\ndefn ca() -> Int { return 1; }",
        )
        .unwrap();
        std::fs::write(
            dir.join("cyc_b.bld"),
            "import \"cyc_a.bld\";\ndefn cb() -> Int { return 2; }",
        )
        .unwrap();
        let src = "import \"cyc_b.bld\";\ndefn ca() -> Int { return 1; }";
        let p = lower_to_bad(src, "x86_64", Some(&dir), Some(&root)).unwrap();
        let count = p
            .items
            .iter()
            .filter(|i| matches!(i, BadTopLevel::Label(l) if l.name == "ca"))
            .count();
        assert_eq!(count, 1, "cycle back to the root merges once");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_import_is_loud() {
        let e = lower_err("import \"nope.bld\";\ndefn f() { return; }");
        assert!(e.contains("no such file"), "{e}");
    }

    #[test]
    fn import_of_unknown_extension_is_loud() {
        let dir = temp_dir("junk");
        std::fs::write(dir.join("junk.txt"), "nothing").unwrap();
        let e = lower_to_bad("import \"junk.txt\";\ndefn f() { return; }", "x86_64", Some(&dir), None)
            .err()
            .unwrap();
        assert!(e.contains("neither a `.bld`"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── end to end (const/physical recipes reach .s) ──────────────────

    #[test]
    fn end_to_end_physical_recipe_emits_assembly() {
        let src = "bootstrap Reset() { bad { mov r0, 42 } }";
        let s = generate(src, "x86_64-unknown-linux-gnu").unwrap();
        assert!(s.contains(".global Reset"), "{s}");
        assert!(s.contains("Reset:"), "{s}");
        assert!(s.contains("movq $42, %rax"), "{s}");
    }

    #[test]
    fn end_to_end_const_return_defn() {
        let src = "defn five() -> Int { return 5; }";
        let s = generate(src, "x86_64-unknown-linux-gnu").unwrap();
        assert!(s.contains("five:"), "{s}");
        assert!(s.contains("movq $5, %rax"), "{s}");
    }

    #[test]
    fn end_to_end_float_param_passthrough() {
        let src = "defn id(x: Float) -> Float { return x; }";
        let s = generate(src, "x86_64-unknown-linux-gnu").unwrap();
        assert!(s.contains("movsd %xmm0, %xmm0"), "{s}");
    }

    // ── M4: register allocation ───────────────────────────────────────

    fn alloc_ok(src: &str) -> BadProgram {
        let family = "x86_64";
        let prog = lower_to_bad(src, family, None, None)
            .unwrap_or_else(|e| panic!("lower failed: {e}"));
        let (_, regs) = crate::backend::bad::registries();
        alloc::allocate(prog, &regs, family)
            .unwrap_or_else(|e| panic!("alloc failed: {e}"))
    }

    #[test]
    fn temps_get_real_registers() {
        let p = alloc_ok("defn f(a: Int) -> Int { return a + 1; }");
        let items = instrs(&p, "f");
        // leaf recipe: param stays in r5; the temp lands in the first
        // allocatable register (x86_64: r3 after scratch reservation)
        assert_eq!(mnems(&items), vec!["add", "mov", "ret"]);
        // first allocatable caller-saved register (r8; r12/r13 went to
        // the scratch reserve)
        assert_eq!(ops_of(items[0]), vec!["r8", "r5", "1"]);
        assert_eq!(ops_of(items[1]), vec!["r0", "r8"]);
    }

    #[test]
    fn leaf_param_stays_in_abi_register() {
        let p = alloc_ok("defn f(a: Int) -> Int { return a; }");
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["r0", "r5"]);
    }

    #[test]
    fn param_is_stashed_when_the_body_calls() {
        let src = "defn g(x: Int) -> Int { return x; }\n\
                   defn f(a: Int) -> Int { g(1); return a; }";
        let p = alloc_ok(src);
        let items = instrs(&p, "f");
        // entry stash copy into a call-crossing (callee-saved) register,
        // staged call, return read back from the stash
        assert_eq!(
            mnems(&items),
            vec!["push", "mov", "mov", "mov", "call", "mov", "mov", "pop", "ret"]
        );
        assert_eq!(ops_of(items[1]), vec!["r3", "r5"]);
        assert_eq!(ops_of(items[6]), vec!["r0", "r3"]);
    }

    #[test]
    fn spill_opens_a_frame() {
        let src = "defn f(a: Int) -> Int { let p = a + 1; let q = p + 1; \
                   let r = q + 1; let s = r + 1; let t = s + 1; let u = t + 1; \
                   return p + q + r + s + t + u; }";
        let p = alloc_ok(src);
        let items = instrs(&p, "f");
        let m = mnems(&items);
        assert!(m.contains(&"loadoff".to_string()), "{m:?}");
        assert!(m.contains(&"storeoff".to_string()), "{m:?}");
        // frame prologue/epilogue bracket the body
        assert_eq!(m.first().map(String::as_str), Some("push"));
        // The chain's intermediate temps legitimately spill too (six
        // values live at once, five registers): the first reload serves
        // the second chain link, hence slot 1 (offset 8), not slot 0.
        let load = items.iter().find(|i| matches!(i, BadBodyItem::Instr(x) if x.mnemonic == "loadoff"));
        assert_eq!(ops_of(load.unwrap()), vec!["r13", "sp", "8"], "scratch reload");
    }

    #[test]
    fn spilled_s_carries_the_frame() {
        let src = "defn f(a: Int) -> Int { let p = a + 1; let q = p + 1; \
                   let r = q + 1; let s = r + 1; let t = s + 1; let u = t + 1; \
                   return p + q + r + s + t + u; }";
        let s = generate(src, "x86_64-unknown-linux-gnu").unwrap();
        // 2 spill slots (16 bytes) + 3 callee saves (24) → 48 total,
        // aligned; the sub is the tail: 48 - 24 = 24.
        assert!(s.contains("pushq %rbx"), "{s}");
        assert!(s.contains("subq $24, %rsp"), "{s}");
        assert!(s.contains("addq $24, %rsp"), "{s}");
        assert!(s.contains("popq %rbx"), "{s}");
        assert!(s.contains("movq 0(%rsp)"), "{s}");
        // the frame restores BEFORE the ret, never after
        let ret = s.find("ret").unwrap();
        let add = s.find("addq $24, %rsp").unwrap();
        assert!(add < ret, "epilogue must precede ret: {s}");
    }

    #[test]
    fn float_crossing_a_call_spills_on_x86() {
        let src = "defn g(x: Float) -> Float { return x; }\n\
                   defn f(y: Float) -> Float { let a2 = y * 2.0; let b2 = g(a2); \
                   return a2 + b2; }";
        let p = alloc_ok(src);
        let items = instrs(&p, "f");
        let m = mnems(&items);
        // x86_64 has no callee-saved float registers: `a2` spills
        assert!(m.contains(&"storeoff".to_string()), "{m:?}");
        assert!(m.contains(&"loadoff".to_string()), "{m:?}");
    }

    #[test]
    fn sp_recipe_refuses_a_frame() {
        let src = "defn f(a: Int) -> Int { bad { mov sp, r8 }\n\
                   let p = a + 1; let q = p + 1; let r = q + 1; let s = r + 1; \
                   let t = s + 1; let u = t + 1; \
                   return p + q + r + s + t + u; }";
        let prog = lower_to_bad(src, "x86_64", None, None).unwrap();
        let (_, regs) = crate::backend::bad::registries();
        let e = alloc::allocate(prog, &regs, "x86_64").err().unwrap();
        assert!(e.contains("cannot open a frame"), "{e}");
    }

    #[test]
    fn defn_named_v1_is_rejected() {
        let e = lower_err("defn v1() { return; }");
        assert!(e.contains("value namespace"), "{e}");
    }

    #[test]
    fn loop_carried_binding_stays_in_a_register() {
        let src = "defn f(a: Int) -> Int { let n = a; \
                   loop { when n == 0 { break; } n = n - 1; } return n; }";
        let p = alloc_ok(src);
        let items = instrs(&p, "f");
        let m = mnems(&items);
        assert!(!m.contains(&"storeoff".to_string()), "{m:?}");
        assert!(!m.contains(&"loadoff".to_string()), "{m:?}");
    }

    // ── M5: engine intrinsics ─────────────────────────────────────────

    fn lower_family(src: &str, family: &str) -> Result<BadProgram, String> {
        let prog = lower_to_bad(src, family, None, None);
        prog
    }

    #[test]
    fn halt_inlines_the_target_sequence() {
        let p = lower_family("defn stop() { Halt(); }", "x86_64").unwrap();
        let items = instrs(&p, "stop");
        assert_eq!(mnems(&items), vec!["cli", "halt"]);
        // aarch64 has no cli row — the aarch64 Halt row is just halt
        let p = lower_family("defn stop() { Halt(); }", "aarch64").unwrap();
        assert_eq!(mnems(&instrs(&p, "stop")), vec!["halt"]);
    }

    #[test]
    fn wait_for_interrupt_on_every_provided_target() {
        for family in ["x86_64", "aarch64", "riscv64"] {
            let p = lower_family("defn idle() { WaitForInterrupt(); }", family).unwrap();
            assert_eq!(mnems(&instrs(&p, "idle")), vec!["wfi"], "{family}");
        }
    }

    #[test]
    fn read_control_reg_substitutes_and_returns() {
        let src = "defn f() -> Int { return ReadControlReg(0); }";
        let p = lower_family(src, "x86_64").unwrap();
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["readcr", "mov", "mov", "ret"]);
        assert_eq!(ops_of(items[0]), vec!["r0", "0"]);
    }

    #[test]
    fn write_control_reg_takes_two_args() {
        let src = "defn f() { WriteControlReg(0, 0x8000); }";
        let p = lower_family(src, "x86_64").unwrap();
        let items = instrs(&p, "f");
        assert_eq!(mnems(&items), vec!["writecr"]);
        assert_eq!(ops_of(items[0]), vec!["0", "32768"]);
    }

    #[test]
    fn cr_number_must_be_a_constant() {
        let src = "defn f(n: Int) -> Int { return ReadControlReg(n); }";
        let e = lower_family(src, "x86_64").err().unwrap();
        assert!(e.contains("compile-time constant"), "{e}");
    }

    #[test]
    fn missing_target_row_names_the_available_targets() {
        let src = "defn f() -> Int { return ReadControlReg(0); }";
        let e = lower_family(src, "aarch64").err().unwrap();
        assert!(e.contains("aarch64"), "{e}");
        assert!(e.contains("x86_64"), "{e}");
    }

    #[test]
    fn far_jump_is_x86_only() {
        let src = "bootstrap B() { FarJump(8, 4096); }";
        let p = lower_family(src, "x86_64").unwrap();
        let items = instrs(&p, "B");
        assert_eq!(mnems(&items), vec!["ljmp"]);
        assert_eq!(ops_of(items[0]), vec!["8", "4096"]);
        let e = lower_family(src, "riscv64").err().unwrap();
        assert!(e.contains("riscv64"), "{e}");
    }

    #[test]
    fn intrinsic_arity_is_checked() {
        let e = lower_family("defn f() { Halt(1); }", "x86_64").err().unwrap();
        assert!(e.contains("takes 0 argument"), "{e}");
    }

    #[test]
    fn defn_shadowing_an_intrinsic_is_loud() {
        let e = lower_family("defn Halt() { return; }", "x86_64").err().unwrap();
        assert!(e.contains("engine intrinsic"), "{e}");
    }

    #[test]
    fn intrinsic_args_read_values_not_registers() {
        // a value argument rides the recipe's own value register into the
        // template slot — no ABI staging
        let src = "defn f(a: Int) { WriteControlReg(3, a + 1); }";
        let p = lower_family(src, "x86_64").unwrap();
        let items = instrs(&p, "f");
        // the recipe calls something, so the param stashes at entry
        assert_eq!(mnems(&items), vec!["mov", "add", "writecr"]);
        assert_eq!(ops_of(items[2]), vec!["3", "v1"]);
    }

    #[test]
    fn end_to_end_cr_read_renders_the_encoded_number() {
        let src = "defn f() -> Int { return ReadControlReg(0); }";
        let s = generate(src, "x86_64-unknown-linux-gnu").unwrap();
        assert!(s.contains("movq %cr0, %rax"), "{s}");
    }

    #[test]
    fn end_to_end_halt_renders() {
        let src = "bootstrap Halt() { Halt(); }";
        let e = lower_err(src);
        // `Halt` as a bootstrap name collides with the verb
        assert!(e.contains("engine intrinsic"), "{e}");
        let src = "bootstrap Reset() { Halt(); }";
        let s = generate(src, "x86_64-unknown-linux-gnu").unwrap();
        assert!(s.contains("cli"), "{s}");
        assert!(s.contains("hlt"), "{s}");
    }

    #[test]
    fn bad_defns_are_not_call_targets() {
        // a .bad sequence defn has no label — calling one from .bld would
        // link-fail; it is inline material for `bad { }` blocks
        let dir = temp_dir("baddefn");
        std::fs::write(dir.join("mach.bad"), "defn seq1 a\nret\n").unwrap();
        let e = lower_to_bad(
            "import \"mach.bad\";\ndefn f() { seq1(1); }",
            "x86_64",
            Some(&dir),
            None,
        )
        .err()
        .unwrap();
        assert!(e.contains("no `defn`"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── M6: stdlib bit wrappers ───────────────────────────────────────

    #[test]
    fn stdlib_bit_wrappers_lower() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src = "import \"lib/std/bld/bits.bld\";\n\
                   defn f(a: Int) -> Bool { return test_bit(a, 3); }";
        let p = lower_to_bad(src, "x86_64", Some(&dir), None).unwrap();
        // the call site
        let items = instrs(&p, "f");
        // the recipe calls, so the param stashes, then staging copies,
        // then the call itself
        let call_pos = mnems(&items)
            .iter()
            .position(|m| m == "call")
            .unwrap_or_else(|| panic!("no call: {:?}", mnems(&items)));
        assert_eq!(ops_of(items[call_pos]), vec!["test_bit"]);
        // the wrapper body: shift, mask, then the equality dance
        let body = instrs(&p, "test_bit");
        let m = mnems(&body);
        assert!(m.contains(&"shr".to_string()), "{m:?}");
        assert!(m.contains(&"and".to_string()), "{m:?}");
    }

    #[test]
    fn stdlib_set_bit_clears_through_not() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src = "import \"lib/std/bld/bits.bld\";\n\
                   defn f(a: Int) -> Int { return clear_bit(a, 5); }";
        let p = lower_to_bad(src, "x86_64", Some(&dir), None).unwrap();
        let body = instrs(&p, "clear_bit");
        let m = mnems(&body);
        assert!(m.contains(&"not".to_string()), "{m:?}");
        assert!(m.contains(&"and".to_string()), "{m:?}");
    }

    #[test]
    fn end_to_end_bit_wrapper_assembles() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src = "import \"lib/std/bld/bits.bld\";\n\
                   defn f(a: Int) -> Int { return set_bit(a, 3); }";
        let s = generate_with(src, "x86_64-unknown-linux-gnu", Some(&dir), None).unwrap();
        assert!(s.contains("set_bit:"), "{s}");
        assert!(s.contains("orq"), "{s}");
    }

    #[test]
    fn loop_counter_binds_one_register_through_the_loop() {
        let src = "defn f(a: Int) -> Int { let n = a; \
                   loop { when n == 0 { break; } n = n - 1; } return n; }";
        let p = alloc_ok(src);
        let items = instrs(&p, "f");
        // every mov/sub touching `n` uses the SAME physical register
        let n_ops: Vec<String> = items
            .iter()
            .filter_map(|i| match i {
                BadBodyItem::Instr(x) if x.mnemonic == "mov" || x.mnemonic == "sub" => {
                    Some(ops_of(i))
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert!(n_ops.iter().filter(|o| o.as_str() == "r8").count() >= 3, "{n_ops:?}");
    }
}
