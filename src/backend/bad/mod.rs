// ── .bad backend — Briev Assembly Dialect ─────────────────────────────
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
// 2026-09-21 (bad-dialect plan): entry point for the .bad dialect. A .bad
// program is pure assembly — it never enters the .bv pipeline (no lexer,
// typecheck, or reactor); this backend parses, proves contracts, lowers
// through the config registries, and emits target assembly text.
//
// Public surface:
// - `generate(source, target_triple)` → target .s text
// - `compile_to_object(source, triple, out_path)` → assembles via the
//   platform assembler path (same `as` the .bv backend uses)
//
// To undo: delete src/backend/bad/ + the two config files, revert
// `BackendKind::Bad`, the targets.dbvl row, and the parser/AST modules.

pub mod contracts;
pub mod lower;
pub mod registry;

use crate::ast::bad::BadProgram;
use crate::parser::bad::parse_bad;

/// Load both registries once per compilation.
pub fn registries() -> (registry::BadIsa, registry::BadRegisters) {
    (registry::BadIsa::load(), registry::BadRegisters::load())
}

/// Lower a .bad source to target assembly text.
///
/// `family` is the target-triple first component (x86_64, aarch64,
/// riscv64) — the same family extraction asm-lowering.dbvl uses.
pub fn generate(source: &str, target_triple: &str) -> Result<String, String> {
    let program: BadProgram =
        parse_bad(source).map_err(|e| format!("bad: line {}: {}", e.line, e.message))?;
    let (isa, regs) = registries();
    let family = target_triple.split('-').next().unwrap_or(target_triple);
    lower::Lowerer::new(&isa, &regs, family).run(&program)
}

/// Assemble emitted text to an object file via the platform assembler.
/// Returns the object path. Arch flags match PlatformAssembler's table.
pub fn assemble(text: &str, family: &str, out_path: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let s_path = out_path.with_extension("s");
    std::fs::write(&s_path, text)
        .map_err(|e| format!("cannot write '{}': {}", s_path.display(), e))?;
    let (as_bin, flags) = match family {
        "x86_64" => ("as", vec!["--64"]),
        "aarch64" => ("as", vec![]),
        "riscv64" => ("as", vec![]),
        other => {
            return Err(format!(
                "no platform assembler mapping for target `{other}` - extend \
                 backend::bad::assemble or use a supported triple"
            ));
        }
    };
    let status = std::process::Command::new(as_bin)
        .args(&flags)
        .arg(&s_path)
        .arg("-o")
        .arg(out_path)
        .status()
        .map_err(|e| format!("cannot run `{as_bin}`: {e} - is binutils installed?"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "assembler rejected the emitted code for `{family}` - inspect {} for the \
             exact instruction",
            s_path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn lower_ok(src: &str, triple: &str) -> String {
        generate(src, triple).unwrap_or_else(|e| panic!("generate failed: {}", e))
    }

    const THREE_WAY: &str = "section .text\nglobal _start\n_start:\n    mov r0, 42\n    ret\n";

    #[test]
    fn hello_world_end_to_end_x86_64() {
        // SysV write(fd=rdi, buf=rsi, len=rdx) with rax=1.
        let src = "section .text\nglobal _start\n_start:\n    addr r4, msg\n    mov r5, 1\n    \
                   mov r2, 16\n    mov r0, 1\n    syscall\n    mov r0, 60\n    mov r5, 0\n    \
                   syscall\n\nsection .data\nmsg: .asciz \"ok\\n\"\n";
        let asm = lower_ok(src, "x86_64-unknown-linux-gnu");
        assert!(asm.contains("leaq msg(%rip), %rsi"), "{}", asm);
        assert!(asm.contains("movq $1, %rax"));
        assert!(asm.contains(".asciz \"ok\\n\""));
    }

    #[test]
    fn same_source_lowers_to_three_targets() {
        let x86 = lower_ok(THREE_WAY, "x86_64-unknown-linux-gnu");
        let arm = lower_ok(THREE_WAY, "aarch64-linux-gnu");
        let riscv = lower_ok(THREE_WAY, "riscv64-unknown-linux-gnu");
        assert!(x86.contains("movq $42, %rax"), "{}", x86);
        assert!(arm.contains("mov x0, #42"), "{}", arm);
        assert!(riscv.contains("li a0, 42"), "{}", riscv);
    }

    #[test]
    fn inline_exception_swaps_only_on_matching_target() {
        let src = "_start:\n    add r0, r0, 1\n    x86_64 => lea r0, [r1 + 1]\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("leal") || x86.contains("lea"), "{}", x86);
        assert!(!x86.contains("addl") && !x86.contains("addq"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("add x0, x0, #1"), "{}", arm);
        assert!(!arm.contains("lea"), "{}", arm);
    }

    #[test]
    fn sequence_defn_expands_with_param_binding() {
        let src = "defn push2 a, b\n    push a\n    push b\n\n_start:\n    push2 r0, r3\n    \
                   ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("pushq %rax"), "{}", x86);
        assert!(x86.contains("pushq %rbx"), "{}", x86);
    }

    #[test]
    fn branch_defn_picks_target_row_then_default() {
        // Offset access composes through add (core load/store take register
        // addresses; see bad-dialect.md "MVP scope").
        let src = "defn store_pair x, addr\n    default => store x, addr; add r2, addr, 8; \
                   store x, r2\n    x86_64 => movq [addr], x\n\n_start:\n    store_pair r0, \
                   r1\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("movq [%rcx], %rax"), "raw target row expected: {}", x86);
        assert!(!x86.contains("sd "), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("str x0, [x1]"), "{}", arm);
        assert!(arm.contains("add x2, x1, #8"), "{}", arm);
        assert!(arm.contains("str x0, [x2]"), "{}", arm);
    }

    #[test]
    fn riscv_imm_form_picks_li_over_mv() {
        let riscv = lower_ok("_start:\n    mov r0, 7\n    mov r0, r1\n    ret\n", "riscv64");
        assert!(riscv.contains("li a0, 7"), "{}", riscv);
        assert!(riscv.contains("mv a0, a1"), "{}", riscv);
    }

    #[test]
    fn illegal_imm_is_a_loud_capability_error() {
        // aarch64 mul has no immediate form (marked `|-` in bad-isa.dbvl).
        let err = generate("_start:\n    mul r0, r1, 3\n    ret\n", "aarch64").unwrap_err();
        assert!(err.contains("no immediate form"), "{}", err);
        // The same op on x86_64 works (imulq takes immediates).
        lower_ok("_start:\n    mul r0, r1, 3\n    ret\n", "x86_64");
    }

    #[test]
    fn symbol_in_non_sym_op_is_rejected_with_fix() {
        let err = generate("_start:\n    mov r0, msg\n    ret\n\n\
                            section .data\nmsg: .asciz \"x\"\n", "x86_64").unwrap_err();
        assert!(err.contains("addr d, msg"), "{}", err);
    }

    #[test]
    fn unmapped_register_is_a_loud_error() {
        // r14/r15 do not exist on x86_64 (16 GPRs).
        let err = generate("_start:\n    mov r14, r0\n    ret\n", "x86_64").unwrap_err();
        assert!(err.contains("r14"), "{}", err);
        // Same register is fine on aarch64.
        lower_ok("_start:\n    mov r14, r0\n    ret\n", "aarch64");
    }

    #[test]
    fn unknown_mnemonic_names_known_ops() {
        let err = generate("_start:\n    frobnicate r0\n", "x86_64").unwrap_err();
        assert!(err.contains("frobnicate") && err.contains("not a core op"), "{}", err);
    }

    #[test]
    fn arity_mismatch_is_rejected() {
        let err = generate("_start:\n    mov r0\n", "x86_64").unwrap_err();
        assert!(err.contains("takes 2 operand(s), got 1"), "{}", err);
    }

    #[test]
    fn preserved_contract_proven_by_callee_saved_property() {
        let src = "section .text\nglobal _start\n_start: [post: r10 preserved]\n    \
                   mov r10, 5\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("proven: callee-saved"), "{}", x86);
    }

    #[test]
    fn preserved_caller_saved_needs_push_pop_pairing() {
        let ok = "_start: [post: r0 preserved]\n    push r0\n    mov r0, 1\n    pop r0\n    \
                  ret\n";
        lower_ok(ok, "x86_64");
        let bad = "_start: [post: r0 preserved]\n    mov r0, 1\n    ret\n";
        let err = generate(bad, "x86_64").unwrap_err();
        assert!(err.contains("unproven") && err.contains("caller-saved"), "{}", err);
        assert!(err.contains("push r0"), "error must state the fix: {}", err);
    }

    #[test]
    fn valid_contract_checks_register_existence() {
        lower_ok("_start: [pre: r0 valid]\n    ret\n", "x86_64");
        let err = generate("_start: [pre: r14 valid]\n    ret\n", "x86_64").unwrap_err();
        assert!(err.contains("does not exist"), "{}", err);
    }

    #[test]
    fn defn_recursion_is_cycle_guarded() {
        let err = generate("defn a x\n    a x\n\n_start:\n    a r0\n", "x86_64").unwrap_err();
        assert!(err.contains("expansion depth") || err.contains("recursive"), "{}", err);
    }

    #[test]
    fn alias_resolves_through_register_table() {
        let asm = lower_ok("alias out = r0\n_start:\n    mov out, 9\n    ret\n", "x86_64");
        assert!(asm.contains("movq $9, %rax"), "{}", asm);
    }

    #[test]
    fn assemble_produces_object_file() {
        let asm = lower_ok(THREE_WAY, "x86_64");
        let out = std::env::temp_dir().join(format!("bad_test_{}.o", std::process::id()));
        assemble(&asm, "x86_64", &out).expect("assemble failed");
        let bytes = std::fs::read(&out).unwrap();
        std::fs::remove_file(&out).ok();
        assert_eq!(&bytes[..4], b"\x7fELF");
    }
}

#[cfg(test)]
mod appgrade_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn signed_and_unsigned_branch_families() {
        let src = "loop:\n    jlt r0, r1, loop\n    jhs r0, r1, loop\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("jl loop"), "{}", x86);
        assert!(x86.contains("jae loop"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("b.lt loop") && arm.contains("b.hs loop"), "{}", arm);
        let riscv = lower_ok(src, "riscv64");
        assert!(riscv.contains("blt a0, a1, loop"), "{}", riscv);
        assert!(riscv.contains("bgeu a0, a1, loop"), "{}", riscv);
    }

    #[test]
    fn riscv_branch_imm_borrows_t0() {
        let riscv = lower_ok("l:\n    jlt r0, 10, l\n    ret\n", "riscv64");
        assert!(riscv.contains("li t0, 10; blt a0, t0, l"), "{}", riscv);
    }

    #[test]
    fn logic_and_shift_ops() {
        let src = "s:\n    and r0, r1, 15\n    xor r2, r2, r2\n    shl r3, r3, 4\n    \
                   shl r4, r4, r5\n    not r6, r7\n    neg r8, r9\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("andq $15, %rax"), "{}", x86);
        assert!(x86.contains("shlq $4, %rbx"), "{}", x86);
        assert!(x86.contains("movq %rdi, %rcx; shlq %cl"), "variable shift uses %cl: {}", x86);
        assert!(x86.contains("notq %r8"), "{}", x86);
        let riscv = lower_ok(src, "riscv64");
        assert!(riscv.contains("andi a0, a1, 15"), "{}", riscv);
        assert!(riscv.contains("slli a3, a3, 4"), "{}", riscv);
        assert!(riscv.contains("xori a6, a7, -1"), "{}", riscv);
    }

    #[test]
    fn math_ops_mod_mulhi_slt() {
        // mod-with-imm works where the row teaches an imm form (x86, riscv);
        // aarch64 mod is register-only (`|-`) — compose or take the error.
        let src = "m:\n    mod r0, r1, r2\n    mulhi r3, r4, r5\n    slt r6, r7, 7\n    \
                   sltu r8, r9, r10\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("cqto; idivq %rdx; movq %rdx, %rax"), "{}", x86);
        assert!(x86.contains("cmpq $7, %r9; setl %al; movzbq %al, %r8"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("sdiv x9, x1, x2; msub x0, x9, x2, x1"), "{}", arm);
        assert!(arm.contains("smulh x3, x4, x5"), "{}", arm);
        assert!(arm.contains("cset x6, lt"), "{}", arm);
        let riscv = lower_ok(src, "riscv64");
        assert!(riscv.contains("rem a0, a1, a2"), "{}", riscv);
        assert!(riscv.contains("mulh a3, a4, a5"), "{}", riscv);
        assert!(riscv.contains("slti a6, a7, 7"), "{}", riscv);
    }

    #[test]
    fn aarch64_mod_imm_is_a_capability_error() {
        let err = generate("m:\n    mod r0, r1, 10\n    ret\n", "aarch64").unwrap_err();
        assert!(err.contains("no immediate form"), "{}", err);
    }

    #[test]
    fn subwidth_memory_and_width_tokens() {
        let src = "w:\n    ldb r0, r1\n    ldub r2, r1\n    ldh r3, r1\n    stb r4, r1\n    \
                   sth r5, r1\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("movsbq (%rcx), %rax"), "{}", x86);
        assert!(x86.contains("movb %sil, (%rcx)"), "stb uses r4.w8=%sil: {}", x86);
        assert!(x86.contains("movw %di, (%rcx)"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("ldrsb x0, [x1]"), "{}", arm);
        assert!(arm.contains("strb w4, [x1]"), "{}", arm);
        assert!(arm.contains("strh w5, [x1]"), "{}", arm);
        let riscv = lower_ok(src, "riscv64");
        assert!(riscv.contains("lb a0, 0(a1)") && riscv.contains("sb a4, 0(a1)"), "{}", riscv);
    }

    #[test]
    fn loadoff_storeoff_and_bare_disp_ref() {
        let src = "o:\n    loadoff r0, r1, 8\n    storeoff r2, r1, 16\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("movq 8(%rcx), %rax"), "bare $N! strips the $ prefix: {}", x86);
        assert!(x86.contains("movq %rdx, 16(%rcx)"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("ldr x0, [x1, #8]") && arm.contains("str x2, [x1, #16]"), "{}", arm);
        let riscv = lower_ok(src, "riscv64");
        assert!(riscv.contains("ld a0, 8(a1)") && riscv.contains("sd a2, 16(a1)"), "{}", riscv);
    }

    #[test]
    fn push2_pop2_first_param_is_higher_address() {
        let src = "s:\n    push2 r0, r1\n    pop2 r0, r1\n    ret\n";
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("stp x1, x0, [sp, #-16]!"), "{}", arm);
        assert!(arm.contains("ldp x1, x0, [sp], #16"), "{}", arm);
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("pushq %rax; pushq %rcx"), "{}", x86);
        assert!(x86.contains("popq %rcx; popq %rax"), "{}", x86);
    }

    #[test]
    fn fp_ops_and_register_classes() {
        let src = "f:\n    fadd f0, f1, f2\n    fneg f3, f4\n    fmul f5, f0, f1\n    \
                   itof f6, r0\n    ftoi r1, f6\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("movsd %xmm1, %xmm0; addsd %xmm2, %xmm0"), "{}", x86);
        assert!(x86.contains("cvtsi2sdq %rax, %xmm6"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("fadd d0, d1, d2"), "{}", arm);
        assert!(arm.contains("scvtf d6, x0"), "{}", arm);
        let riscv = lower_ok(src, "riscv64");
        assert!(riscv.contains("fadd.d fa0, fa1, fa2"), "{}", riscv);
        assert!(riscv.contains("fcvt.d.l fa6, a0"), "{}", riscv);
    }

    #[test]
    fn fp_branch_family_mirrors_j() {
        let src = "fl:\n    fjlt f0, f1, fl\n    fjge f2, f3, fl\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("ucomisd %xmm1, %xmm0; jb fl"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("fcmp d0, d1; b.mi fl"), "{}", arm);
        let riscv = lower_ok(src, "riscv64");
        assert!(riscv.contains("flt.d t0, fa0, fa1; bne t0, zero, fl"), "{}", riscv);
        assert!(riscv.contains("fle.d t0, fa3, fa2; bne t0, zero, fl"), "{}", riscv);
    }

    #[test]
    fn callee_saved_fp_detected_on_aarch64() {
        // f8 is d8 on aarch64 = callee-saved; f0 is caller-saved.
        let ok = "g: [post: f8 preserved]\n    fmov f8, f0\n    ret\n";
        lower_ok(ok, "aarch64");
        let bad = "g: [post: f0 preserved]\n    fmov f0, f1\n    ret\n";
        let err = generate(bad, "aarch64").unwrap_err();
        assert!(err.contains("caller-saved"), "{}", err);
    }
}
