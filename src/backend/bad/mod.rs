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

    fn lower_ok(src: &str, triple: &str) -> String {
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
