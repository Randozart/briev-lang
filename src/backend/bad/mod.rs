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
pub mod comptime;
pub mod lower;
pub mod notices;
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
    generate_with(source, target_triple, false, None, true)
}

/// `--trace-lowering`: stderr note per instruction — which exception
/// fired / which form picked / where registers landed. `friendly`
/// selects the default alias sheet; `--raw` passes false.
pub fn generate_with(
    source: &str, target_triple: &str, trace: bool, base_dir: Option<&std::path::Path>,
    friendly: bool,
) -> Result<String, String> {
    let (asm, _) = generate_with_notices(source, target_triple, trace, base_dir, friendly)?;
    Ok(asm)
}

/// `generate_with` plus the W-tier notices collected during lowering
/// (the predicted probable errors, acknowledged and unacknowledged).
/// Callers that want the notices — the CLI, the tests — use this.
pub fn generate_with_notices(
    source: &str, target_triple: &str, trace: bool, base_dir: Option<&std::path::Path>,
    friendly: bool,
) -> Result<(String, Vec<notices::Notice>), String> {
    let program: BadProgram =
        parse_bad(source).map_err(|e| format!("bad: line {}: {}", e.line, e.message))?;
    let (isa, regs) = registries();
    let family = target_triple.split('-').next().unwrap_or(target_triple);
    let mut lowerer = lower::Lowerer::new(&isa, &regs, family)
        .with_trace(trace)
        .with_friendly(friendly)
        .with_base_dir(base_dir.map(|p| p.to_path_buf()));
    let asm = lowerer.run(&program)?;
    let notices = lowerer.notices().to_vec();
    Ok((asm, notices))
}

/// 2026-09-21: Compile a `bad fn` body from `.bv` — wrap the body
/// in an entry label, append `ret`, parse as a .bad program, and lower
/// with pre-bound parameter registers.
pub fn generate_bad_fn(
    body: &str,
    target_triple: &str,
    param_env: std::collections::HashMap<String, lower::Bound>,
) -> Result<String, String> {
    // Wrap body in an entry label and ensure it ends with `ret`.
    let trimmed = body.trim();
    let mut wrapped = String::from("_entry:\n");
    wrapped.push_str(trimmed);
    // Auto-append `ret` if the body doesn't already end with one.
    let last_line = trimmed.lines().last().unwrap_or("").trim();
    if last_line != "ret" && !last_line.ends_with("ret") {
        wrapped.push_str("\nret");
    }
    wrapped.push('\n');
    let program: BadProgram =
        parse_bad(&wrapped).map_err(|e| format!("bad fn body: line {}: {}", e.line, e.message))?;
    let (isa, regs) = registries();
    let family = target_triple.split('-').next().unwrap_or(target_triple);
    lower::Lowerer::new(&isa, &regs, family)
        .with_param_env(param_env)
        .run(&program)
}

/// Whether the cross toolchain for `family` is installed — the cross_as
/// bin, or (thumb/arm) clang's integrated assembler fallback.
pub fn toolchain_available(family: &str) -> bool {
    let (isa, regs) = registries();
    let _ = isa;
    let via_bin = regs
        .cross_as(family)
        .and_then(|as_bin| {
            std::process::Command::new(as_bin)
                .arg("--version")
                .output()
                .ok()
                .map(|o| o.status.success())
        })
        .unwrap_or(false);
    if via_bin {
        return true;
    }
    if family.starts_with("thumb") || family.starts_with("arm") {
        return clang_available();
    }
    false
}

/// Assemble emitted text to an object file via the platform assembler.
/// The toolchain comes from the `cross_as` row per family. For thumb/arm
/// bare-metal targets, clang's integrated assembler is the fallback when
/// the prefixed binutils (`arm-none-eabi-as`) is not installed — the
/// dialect degrades to a documented clang path, never a silent pass.
pub fn assemble(text: &str, family: &str, out_path: &std::path::Path) -> Result<(), String> {
    let (_, regs) = registries();
    let as_bin = regs.cross_as(family).ok_or_else(|| {
        format!(
            "no cross_as row for target `{family}` - add one to bad-registers.dbvl"
        )
    })?;
    let flags: Vec<&str> = match family {
        "x86_64" => vec!["--64"],
        _ => vec![],
    };
    let s_path = out_path.with_extension("s");
    std::fs::write(&s_path, text)
        .map_err(|e| format!("cannot write '{}': {}", s_path.display(), e))?;
    // Preferred toolchain: the cross_as bin. If it is not installed, a
    // thumb/arm family falls back to clang's integrated assembler.
    let preferred = std::process::Command::new(as_bin).arg("--version").output().ok();
    let bin: &str = if preferred.as_ref().map(|o| o.status.success()).unwrap_or(false) {
        as_bin
    } else if (family.starts_with("thumb") || family.starts_with("arm")) && clang_available() {
        return clang_assemble(family, &s_path, out_path);
    } else {
        return Err(format!(
            "cannot run `{as_bin}` for `{family}` - install the cross binutils, or \
             (thumb/arm) clang's integrated assembler"
        ));
    };
    let status = std::process::Command::new(bin)
        .args(&flags)
        .arg(&s_path)
        .arg("-o")
        .arg(out_path)
        .status()
        .map_err(|e| format!("cannot run `{bin}`: {e} - is binutils installed?"))?;
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

fn clang_available() -> bool {
    std::process::Command::new("clang")
        .arg("--version")
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Assemble bare-metal thumb/arm with clang's integrated assembler.
fn clang_assemble(
    family: &str, s_path: &std::path::Path, out_path: &std::path::Path,
) -> Result<(), String> {
    let triple = format!("{family}-none-eabi");
    let status = std::process::Command::new("clang")
        .arg(format!("--target={triple}"))
        .arg("-mcpu=cortex-m3")
        .arg("-c")
        .arg(s_path)
        .arg("-o")
        .arg(out_path)
        .status()
        .map_err(|e| format!("cannot run clang for `{family}`: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "clang rejected the emitted code for `{family}` - inspect {} for the \
             exact instruction",
            s_path.display()
        ))
    }
}

#[cfg(test)]
fn test_dir(tag: &str) -> std::path::PathBuf {
    // Under target/ — gitignored, cargo-cleanable, and immune to /tmp
    // pressure from other sessions.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/bad-test")
        .join(format!("{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
        // SysV write(fd=rdi, buf=rsi, len=rdx): portable syscall routes
        // the operands per target.
        let src = "section .text\nglobal _start\n_start:\n    addr r4, msg\n    mov r5, 1\n    \
                   mov r2, 16\n    syscall write, r5, r4, r2\n    syscall exit, r5, r4, r5\n\n\
                   section .data\nmsg: .asciz \"ok\\n\"\n";
        let asm = lower_ok(src, "x86_64-unknown-linux-gnu");
        assert!(asm.contains("leaq msg(%rip), %rsi"), "{}", asm);
        assert!(asm.contains("syscall"), "{}", asm);
        let joined = asm.replace('\n', "; ");
        assert!(joined.contains("movq $1, %rax; syscall"), "number loads last: {}", asm);
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
        let src = "section .text\nglobal _start\n_start: [r10 preserved]\n    \
                   mov r10, 5\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("proven: callee-saved"), "{}", x86);
    }

    #[test]
    fn preserved_caller_saved_needs_push_pop_pairing() {
        let ok = "_start: [r0 preserved]\n    push r0\n    mov r0, 1\n    pop r0\n    \
                  ret\n";
        lower_ok(ok, "x86_64");
        let bad = "_start: [r0 preserved]\n    mov r0, 1\n    ret\n";
        let err = generate(bad, "x86_64").unwrap_err();
        assert!(err.contains("unproven") && err.contains("caller-saved"), "{}", err);
        assert!(err.contains("push r0"), "error must state the fix: {}", err);
    }

    #[test]
    fn valid_contract_checks_register_existence() {
        lower_ok("_start: [r0 valid]\n    ret\n", "x86_64");
        let err = generate("_start: [r14 valid]\n    ret\n", "x86_64").unwrap_err();
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
        let out = test_dir("assemble").join("t.o");
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
        assert!(riscv.contains("li t0, 10") && riscv.contains("blt a0, t0, l"), "{}", riscv);
    }

    #[test]
    fn logic_and_shift_ops() {
        let src = "s:\n    and r0, r1, 15\n    xor r2, r2, r2\n    shl r3, r3, 4\n    \
                   shl r4, r4, r5\n    not r6, r7\n    neg r8, r9\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("andq $15, %rax"), "{}", x86);
        assert!(x86.contains("shlq $4, %rbx"), "{}", x86);
        let joined = x86.replace('\n', "; ");
        assert!(joined.contains("movq %rdi, %rcx; shlq %cl"), "variable shift uses %cl: {}", x86);
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
        let joined = x86.replace('\n', "; ");
        assert!(joined.contains("cqto; idivq %rdx; movq %rdx, %rax"), "{}", x86);
        assert!(joined.contains("cmpq $7, %r9; setl %al; movzbq %al, %r8"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        let aj = arm.replace('\n', "; ");
        assert!(aj.contains("sdiv x9, x1, x2; msub x0, x9, x2, x1"), "{}", arm);
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
        assert!(x86.contains("pushq %rax") && x86.contains("pushq %rcx"), "{}", x86);
        assert!(x86.contains("popq %rcx") && x86.contains("popq %rax"), "{}", x86);
    }

    #[test]
    fn fp_ops_and_register_classes() {
        let src = "f:\n    fadd f0, f1, f2\n    fneg f3, f4\n    fmul f5, f0, f1\n    \
                   itof f6, r0\n    ftoi r1, f6\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        let joined = x86.replace('\n', "; ");
        assert!(joined.contains("movsd %xmm1, %xmm0; addsd %xmm2, %xmm0"), "{}", x86);
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
        let joined = x86.replace('\n', "; ");
        assert!(joined.contains("ucomisd %xmm1, %xmm0; jb fl"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        let joined = arm.replace('\n', "; ");
        assert!(joined.contains("fcmp d0, d1; b.mi fl"), "{}", arm);
        let riscv = lower_ok(src, "riscv64");
        let rj = riscv.replace('\n', "; ");
        assert!(rj.contains("flt.d t0, fa0, fa1; bne t0, zero, fl"), "{}", riscv);
        assert!(rj.contains("fle.d t0, fa3, fa2; bne t0, zero, fl"), "{}", riscv);
    }

    #[test]
    fn callee_saved_fp_detected_on_aarch64() {
        // f8 is d8 on aarch64 = callee-saved; f0 is caller-saved.
        let ok = "g: [f8 preserved]\n    fmov f8, f0\n    ret\n";
        lower_ok(ok, "aarch64");
        let bad = "g: [f0 preserved]\n    fmov f0, f1\n    ret\n";
        let err = generate(bad, "aarch64").unwrap_err();
        assert!(err.contains("caller-saved"), "{}", err);
    }
}

#[cfg(test)]
mod phase_b_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn const_directive_feeds_expressions() {
        let src = ".const MAX_ROWS 64\n.const LIMIT MAX_ROWS * 4 - 1\n\
                   _start:\n    mov r0, LIMIT\n    mov r1, MAX_ROWS >> 2\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("movq $255, %rax"), "{}", x86);
        assert!(x86.contains("movq $16, %rcx"), "{}", x86);
    }

    #[test]
    fn const_cycle_is_a_loud_error() {
        let src = ".const A B + 1\n.const B A + 1\n_start:\n    mov r0, A\n    ret\n";
        let err = generate(src, "x86_64").unwrap_err();
        assert!(err.contains("const cycle"), "{}", err);
    }

    #[test]
    fn struct_layout_computes_field_offsets() {
        let src = ".const RIDE_SIZE 12\n.struct Ride\n.field excitement, 8\n.field \
                   nausea, RIDE_SIZE - 8\n.field level, 1\n.end\n\
                   _start:\n    loadoff r0, r1, Ride.nausea\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        // excitement: 0 (size 8) → nausea at 8 (size 4) → level at 12.
        assert!(x86.contains("movq 8(%rcx), %rax"), "{}", x86);
    }

    #[test]
    fn struct_size_const_lands() {
        let src = ".struct P\n.field x, 4\n.field y, 4\n.end\n.const HALF P.size / 2\n\
                   _start:\n    mov r0, HALF\n    ret\n";
        let asm = lower_ok(src, "x86_64");
        assert!(asm.contains("movq $4, %rax"), "{}", asm);
    }

    #[test]
    fn expr_operands_evaluate_at_use() {
        let src = ".const BASE 4096\n_start:\n    mov r0, BASE + 16\n    loadoff r1, r2, \
                   BASE / 2\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("movq $4112, %rax"), "{}", x86);
        assert!(x86.contains("movq 2048(%rdx), %rcx"), "{}", x86);
    }

    #[test]
    fn register_param_inside_expr_is_a_type_error() {
        let src = "defn f x\n    mov r0, x + 1\n    ret\n\n_start:\n    f r3\n    ret\n";
        let err = generate(src, "x86_64").unwrap_err();
        assert!(err.contains("REGISTER"), "{}", err);
    }

    #[test]
    fn local_labels_scope_and_resolve() {
        let src = "foo:\n.loop:\n    jnz r0, 1, .loop\n    ret\n\nbar:\n.loop:\n    jnz \
                   r1, 1, .loop\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("Lfoo__loop:"), "{}", x86);
        assert!(x86.contains("jne Lfoo__loop"), "{}", x86);
        assert!(x86.contains("Lbar__loop:"), "{}", x86);
        assert!(x86.contains("jne Lbar__loop"), "{}", x86);
        assert!(!x86.contains(".loop"), "{}", x86);
    }

    #[test]
    fn word_relocation_passes_through() {
        let src = "section .text\nglobal _start\n_start:\n    ret\n\nsection .data\n\
                   table: .word _start\n     .word 42\n";
        let asm = lower_ok(src, "x86_64");
        assert!(asm.contains("table: .word _start"), "{}", asm);
        assert!(asm.contains(".word 42"), "{}", asm);
    }
}

#[cfg(test)]
mod phase_c_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn abi_args_map_per_target() {
        let (_, regs) = registries();
        let x86 = regs.abi_args("x86_64");
        assert_eq!(x86, vec!["r5", "r4", "r2", "r1", "r6", "r7"]);
        assert_eq!(regs.abi_args("aarch64")[0], "r0");
        assert_eq!(regs.abi_args("riscv64")[0], "r0");
        assert_eq!(regs.push_width("x86_64"), 8);
        assert_eq!(regs.push_width("aarch64"), 16);
    }

    #[test]
    fn export_validates_label_existence_and_emits_global() {
        let ok = "section .text\nglobal _start\nexport add_one\n\nadd_one: [r0 \
                  valid]\n    add r0, r0, 1\n    ret\n\n_start:\n    ret\n";
        let asm = lower_ok(ok, "x86_64");
        assert!(asm.contains(".global add_one"), "{}", asm);
        let err = generate("export nope\n\n_start:\n    ret\n", "x86_64").unwrap_err();
        assert!(err.contains("names no label"), "{}", err);
    }

    #[test]
    fn frame_contract_tracks_push_pop_discipline() {
        let ok = "fn: [frame: 32]\n    push2 r0, r1\n    pop2 r1, r0\n    ret\n";
        lower_ok(ok, "x86_64");
        // Unbalanced: net displacement at the end.
        let bad = "fn: [frame: 32]\n    push2 r0, r1\n    ret\n";
        let err = generate(bad, "x86_64").unwrap_err();
        assert!(err.contains("net sp displacement"), "{}", err);
        // High-water bound.
        let big = "fn: [frame: 8]\n    push2 r0, r1\n    pop2 r1, r0\n    ret\n";
        let err = generate(big, "x86_64").unwrap_err();
        assert!(err.contains("stacks up to 16 bytes"), "{}", err);
    }

    #[test]
    fn frame_contract_demands_call_alignment() {
        let ok = "fn: [frame: 16]\n    push2 r0, r1\n    call other\n    pop2 r1, r0\n    \
                  ret\n\nother:\n    ret\n";
        lower_ok(ok, "x86_64");
        let bad = "fn: [frame: 16]\n    push r0\n    call other\n    pop r0\n    ret\n\n\
                   other:\n    ret\n";
        let err = generate(bad, "x86_64").unwrap_err();
        assert!(err.contains("16-aligned"), "{}", err);
    }
}

#[cfg(test)]

fn branch_target_of(line: &str) -> Option<String> {
    let head = line.split_whitespace().next()?;
    let branches = ["jmp", "je", "jne", "b", "b.eq", "b.ne", "j", "beq", "bne"];
    if branches.iter().any(|m| head.starts_with(m)) {
        line.split_whitespace().last().map(String::from)
    } else {
        None
    }
}

/// Label -> referenced-labels adjacency, parsed from emitted asm. The
/// cross-target equivalence proof: lowering is semantics-preserving, so
/// every target's control-flow graph must match.
#[cfg(test)]
fn branch_graph(asm: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut current = String::new();
    for line in asm.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_suffix(':') {
            if !line.starts_with('L') || line.contains("__") {
                current = name.to_string();
                out.push((current.clone(), Vec::new()));
            }
            continue;
        }
        if !current.is_empty() {
            if let Some(target) = branch_target_of(line) {
                out.last_mut().unwrap().1.push(target);
            }
        }
    }
    out
}

mod phase_d_tests {
    use super::*;
    const STDLIB: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/std/bad/string.bad"));

    fn assert_all_syms(asm: &str, triple: &str, syms: &[&str]) {
        for sym in syms {
            assert!(asm.contains(sym), "{triple} missing {sym}: {asm}");
        }
    }

    #[test]
    fn stdlib_lowers_to_all_three_targets() {
        for triple in ["x86_64", "aarch64", "riscv64"] {
            let asm = generate(STDLIB, triple)
                .unwrap_or_else(|e| panic!("stdlib failed on {triple}: {e}"));
            assert_all_syms(&asm, triple, &["memcpy:", "memset:", "strlen:", "strcmp:"]);
        }
    }

    #[test]
    fn thumb_lowers_to_cortex_m_and_assembles_via_clang() {
        // 2026-09-22 (bootstrap-bad plan): thumb/arm rows in the ISA +
        // register configs. If clang is installed, the emitted thumb-2
        // text must actually assemble — the dialect's hardware-verification
        // doctrine, degraded to a documented skip when clang is absent.
        let src = "section .text\nglobal _start\n_start:\n    mov r0, 1\n    \
                   add r1, r0, r0\n    jz r1, r0, .done\n    mov r2, 99\n    \
                   .done:\n    halt\n";
        let asm = generate(src, "thumbv7m-none-eabi").unwrap();
        assert!(asm.contains("movw r0, #1"), "{asm}");
        assert!(asm.contains("adds r1, r0, r0"), "{asm}");
        assert!(asm.contains("beq"), "{asm}");
        assert!(asm.contains("wfi"), "{asm}");
        if toolchain_available("thumbv7m") {
            let out = test_dir("thumb").join("t.o");
            assemble(&asm, "thumbv7m", &out).expect("thumb assemble failed");
            std::fs::remove_file(&out).ok();
        }
    }

    #[test]
    fn thumb_missing_fpu_and_syscall_are_loud_errors() {
        // Cortex-M3 has no FPU and no OS — the FP class and syscall have
        // NO thumb row. A loud capability error, never a silent pass.
        let err = generate("t:\n    fmov f0, 1.5\n    ret\n", "thumbv7m").unwrap_err();
        assert!(err.contains("no `thumbv7m` lowering"), "{err}");
        let err = generate("t:\n    syscall write, r0, r1, r2\n", "thumbv7m").unwrap_err();
        assert!(err.contains("no `thumbv7m` lowering"), "{err}");
    }

    #[test]
    fn stdlib_assembles_on_host() {
        let asm = generate(STDLIB, "x86_64").unwrap();
        let out = test_dir("stdlib").join("t.o");
        assemble(&asm, "x86_64", &out).expect("stdlib assemble failed");
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn import_expands_at_the_import_line() {
        let dir = test_dir("import");
        let lib = dir.join("lib.bad");
        std::fs::write(&lib, "defn double_it x\n    add r0, x, x\n    ret\n").unwrap();
        let main_src = "import \"lib.bad\"\n\n_start:\n    double_it r5\n    ret\n";
        let asm = generate_with(main_src, "x86_64", false, Some(&dir), true).unwrap();
        std::fs::remove_file(&lib).ok();
        std::fs::remove_dir(&dir).ok();
        // `add r0, x, x` with x = r5 rides the x86 lea imm-form row.
        assert!(asm.contains("leaq (%rdi, %rdi), %rax"), "{}", asm);
        // The defn is inlined, NOT emitted as a label.
        assert!(!asm.contains("double_it:"), "{}", asm);
    }

    #[test]
    fn import_cycles_terminate_idempotently() {
        // A cycle is not an error: re-import is a no-op (diamond-safe),
        // depth is capped at 16 for pathological graphs.
        let dir = test_dir("cycle");
        std::fs::write(dir.join("a.bad"), "import \"b.bad\"\n.const V 1\n").unwrap();
        std::fs::write(dir.join("b.bad"), "import \"a.bad\"\n").unwrap();
        let src = "import \"a.bad\"\n\n_start:\n    mov r0, V\n    ret\n";
        let asm = generate_with(src, "x86_64", false, Some(&dir), true).unwrap();
        std::fs::remove_file(dir.join("a.bad")).ok();
        std::fs::remove_file(dir.join("b.bad")).ok();
        std::fs::remove_dir(&dir).ok();
        assert!(asm.contains("movq $1, %rax"), "{}", asm);
    }

    #[test]
    fn duplicate_labels_across_imports_are_loud() {
        let dir = test_dir("dup");
        std::fs::write(dir.join("l.bad"), "dup:\n    ret\n").unwrap();
        let src = "import \"l.bad\"\n\ndup:\n    ret\n";
        let err = generate_with(src, "x86_64", false, Some(&dir), true).unwrap_err();
        std::fs::remove_file(dir.join("l.bad")).ok();
        std::fs::remove_dir(&dir).ok();
        assert!(err.contains("declared twice"), "{}", err);
    }

    #[test]
    fn cross_target_branch_graph_is_identical() {
        let src = "_start:\n    jz r0, 1, .exit\n    jmp .mid\n.mid:\n    jlt r1, 2, .exit\n    \
                   ret\n.exit:\n    ret\n";
        let g1 = branch_graph(&generate(src, "x86_64").unwrap());
        let g2 = branch_graph(&generate(src, "aarch64").unwrap());
        let g3 = branch_graph(&generate(src, "riscv64").unwrap());
        assert_eq!(g1, g2, "x86 vs aarch64 branch graph");
        assert_eq!(g2, g3, "aarch64 vs riscv branch graph");
    }
}

#[test]
fn sym_op_with_immediate_last_operand_is_rejected() {
    let err = generate(
        "section .text\nglobal _start\n_start:\n    addr r5, 1\n    ret\n",
        "x86_64",
    )
    .unwrap_err();
    assert!(err.contains("label or symbol") && err.contains("mov` for values"), "{}", err);
}

#[cfg(test)]
mod hardening_tests {
    use super::*;

    const HELLO: &str = "section .text\nglobal _start\n_start:\n    mov r5, 1\n    addr \
                         r4, msg\n    mov r2, 6\n    syscall write, r5, r4, r2\n    mov \
                         r5, 0\n    mov r4, 0\n    mov r3, 0\n    syscall exit, r5, r4, \
                         r3\n\nsection .data\nmsg: .asciz \"cross\\n\"\n";

    /// Run a linked binary under the family's emulator; returns stdout.
    fn run_under_qemu(bin: &std::path::Path, family: &str) -> Result<String, String> {
        let qemu = format!("qemu-{family}");
        let sysroot = format!("/usr/{family}-linux-gnu");
        let mut cmd = std::process::Command::new(&qemu);
        if std::path::Path::new(&sysroot).is_dir() {
            cmd.arg("-L").arg(&sysroot);
        }
        let out = cmd
            .arg(bin)
            .output()
            .map_err(|e| format!("qemu: {e}"))?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    #[test]
    fn hello_cross_verifies_on_installed_targets() {
        for family in ["x86_64", "aarch64", "riscv64"] {
            if !toolchain_available(family) {
                eprintln!("skip: {family} cross toolchain not installed");
                continue;
            }
            let asm = generate(HELLO, &format!("{family}-linux-gnu"))
                .unwrap_or_else(|e| panic!("{family} generate: {e}"));
            let dir = test_dir(&format!("cross_{family}"));
            let o = dir.join("hello.o");
            assemble(&asm, family, &o).unwrap_or_else(|e| panic!("{family} assemble: {e}"));
            let (_, regs) = registries();
            let ld = regs.cross_ld(family).unwrap();
            let bin = dir.join("hello");
            let st = std::process::Command::new(ld).arg(&o).arg("-o").arg(&bin).status()
                .expect("ld");
            assert!(st.success(), "{family} link failed");
            let stdout = run_under_qemu(&bin, family).unwrap_or_else(|e| panic!("{family}: {e}"));
            assert_eq!(stdout, "cross\n", "{family} output");
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}

#[cfg(test)]
mod phase3_hardening_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn fmov_imm_pools_on_x86_and_riscv() {
        let src = "f:\n    fmov f0, 1.5\n    fmov f1, 1.5\n    fmov f2, 2.25\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        // Dedup by value: one pool entry for the two 1.5s.
        assert!(x86.contains("movsd .Lfloat_0(%rip), %xmm0"), "{}", x86);
        assert!(x86.contains("movsd .Lfloat_0(%rip), %xmm1"), "{}", x86);
        assert!(x86.contains("movsd .Lfloat_1(%rip), %xmm2"), "{}", x86);
        assert!(x86.contains(".Lfloat_0:\n.double 1.5"), "{}", x86);
        assert!(x86.contains(".Lfloat_1:\n.double 2.25"), "{}", x86);
        let riscv = lower_ok(src, "riscv64");
        let joined = riscv.replace('\n', "; ");
        assert!(joined.contains("la t0, .Lfloat_0; fld fa0, 0(t0)"), "{}", riscv);
    }

    #[test]
    fn fmov_imm_pools_on_aarch64_via_adrp() {
        let asm = lower_ok("f:\n    fmov f0, 1.5\n    ret\n", "aarch64");
        let joined = asm.replace('\n', "; ");
        assert!(joined.contains("adrp x9, .Lfloat_0"), "{}", asm);
        assert!(joined.contains("ldr d0, [x9]"), "{}", asm);
        assert!(asm.contains(".double 1.5"), "{}", asm);
    }

    #[test]
    fn float_literals_cross_assemble_and_run_where_toolchains_exist() {
        let src = "section .text\nglobal _start\n_start:\n    fmov f0, 1.5\n    fmov f1, \
                   2.25\n    fadd f2, f0, f1\n    addr r4, out\n    fstore f2, r4\n    addr \
                   r9, out\n    mov r5, 1\n    mov r2, 8\n    syscall write, r5, r9, r2\n    \
                   syscall exit, r5, r4, r4\n\nsection .data\nbuf: .zero 8\nout: .double \
                   0\n";
        for family in ["x86_64", "aarch64", "riscv64"] {
            fp_cross_verify(src, family);
        }
    }

    /// Generate → assemble → link → run one program on one target under
    /// qemu; asserts the stored f64 bit pattern of 1.5 + 2.25.
    fn fp_cross_verify(src: &str, family: &str) {
        if !toolchain_available(family) {
            eprintln!("skip: {family} cross toolchain not installed");
            return;
        }
        let asm = generate(src, &format!("{family}-linux-gnu"))
            .unwrap_or_else(|e| panic!("{family}: {e}"));
        let dir = test_dir(&format!("fp_{family}"));
        let o = dir.join("fp.o");
        assemble(&asm, family, &o).unwrap_or_else(|e| panic!("{family} assemble: {e}"));
        let (_, regs) = registries();
        let bin = dir.join("fp");
        let mut ld_cmd = std::process::Command::new(regs.cross_ld(family).unwrap());
        for flag in regs.cross_ld_flags(family) {
            ld_cmd.arg(flag);
        }
        let st = ld_cmd.arg(&o).arg("-o").arg(&bin).status().expect("ld");
        assert!(st.success(), "{family} link failed");
        let stdout = std::process::Command::new(format!("qemu-{family}"))
            .arg(&bin)
            .output()
            .map(|o| o.stdout)
            .unwrap_or_else(|e| panic!("{family} qemu: {e}"));
        assert_eq!(stdout, 3.75f64.to_bits().to_le_bytes(), "{family} fp math");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod phase4_hardening_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn frame_tracks_direct_sp_arithmetic() {
        let ok = "fn: [frame: 48]\n    sub sp, sp, 32\n    add sp, sp, 32\n    ret\n";
        lower_ok(ok, "x86_64");
        let unbalanced = "fn: [frame: 48]\n    sub sp, sp, 32\n    ret\n";
        let err = generate(unbalanced, "x86_64").unwrap_err();
        assert!(err.contains("net sp displacement -32"), "{}", err);
        let over = "fn: [frame: 16]\n    sub sp, sp, 32\n    add sp, sp, 32\n    ret\n";
        let err = generate(over, "x86_64").unwrap_err();
        assert!(err.contains("stacks up to 32 bytes"), "{}", err);
    }

    #[test]
    fn stack_arg_base_row_per_target() {
        let (_, regs) = registries();
        assert_eq!(regs.abi_stack_arg_base("x86_64"), 8);
        assert_eq!(regs.abi_stack_arg_base("aarch64"), 0);
        assert_eq!(regs.abi_stack_arg_base("riscv64"), 0);
    }

    #[test]
    fn syscall_number_lookup_per_target() {
        let (_, regs) = registries();
        assert_eq!(regs.syscall_number("x86_64", "write"), Some(1));
        assert_eq!(regs.syscall_number("aarch64", "write"), Some(64));
        assert_eq!(regs.syscall_number("riscv64", "write"), Some(64));
        assert_eq!(regs.syscall_number("x86_64", "nope"), None);
    }

    #[test]
    fn unknown_syscall_name_is_loud_with_known_list() {
        let err = generate(
            "_start:\n    mov r5, 1\n    syscall frobnicate, r5, r5, r5\n    ret\n",
            "x86_64",
        )
        .unwrap_err();
        assert!(err.contains("frobnicate") && err.contains("known:"), "{}", err);
    }
}

#[cfg(test)]
mod arg_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn arg_routes_register_window_per_target() {
        // Arg 1-6 on x86_64 = r5,r4,r2,r1,r6,r7; arg 1-8 on arm/riscv.
        let src = "f:\n    arg r9, 1\n    arg r8, 6\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        let joined = x86.replace('\n', "; ");
        assert!(joined.contains("movq %rdi, %r11") && joined.contains("movq %r9, %r10"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        let joined = arm.replace('\n', "; ");
        assert!(joined.contains("mov x9, x0") && joined.contains("mov x8, x5"), "{}", arm);
    }

    #[test]
    fn arg_routes_stack_window_beyond_reg_args() {
        // Arg 7 on x86_64 = stack (base 8); arg 7 on aarch64 = still a
        // register (8 reg args); arg 9 on aarch64 = stack (base 0).
        let src = "f:\n    arg r9, 7\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("movq 8(%rsp), %r11"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("mov x9, x6"), "{}", arm);
        let src9 = "f:\n    arg r9, 9\n    ret\n";
        let arm = lower_ok(src9, "aarch64");
        assert!(arm.contains("ldr x9, [sp, #0]"), "{}", arm);
    }

    #[test]
    fn arg_index_must_be_constant() {
        let err = generate("_start:\n    arg r0, r1\n    ret\n", "x86_64").unwrap_err();
        assert!(err.contains("must be a constant"), "{}", err);
    }
}

#[cfg(test)]
mod hygiene_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn local_labels_inside_defns_are_hygienic_across_calls() {
        // ChargeGuest-style defn invoked twice — the same .full label
        // must materialize twice without collision.
        let src = "defn twice x\n.loop:\n    sub x, x, 1\n    jnz x, 1, .loop\n\n\
                   _start:\n    twice r3\n    twice r4\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("Ltwice__1__loop:"), "{}", x86);
        assert!(x86.contains("Ltwice__2__loop:"), "{}", x86);
        assert_eq!(x86.matches("Ltwice__1__loop").count(), 2, "label + branch ref");
        assert_eq!(x86.matches("Ltwice__2__loop").count(), 2, "{}", x86);
    }

    #[test]
    fn local_labels_are_illegal_in_branch_defns() {
        let err = generate(
            "defn f x\n    default => nop\n.name:\n",
            "x86_64",
        )
        .unwrap_err();
        assert!(err.contains("branch defn"), "{}", err);
    }
}

#[cfg(test)]
mod alias_tests {
    use super::tests::lower_ok;
    use super::*;

    #[test]
    fn friendly_sheet_loads_by_default() {
        let asm = lower_ok("t:\n    Move r0, 1\n    Return\n", "x86_64");
        assert!(asm.contains("movq $1, %rax"), "{}", asm);
        assert!(asm.contains("ret"), "{}", asm);
    }

    #[test]
    fn raw_mode_skips_the_sheet() {
        let (_, regs) = registries();
        let _ = regs;
        let err = generate_with("t:\n    Move r0, 1\n", "x86_64", false, None, false)
            .unwrap_err();
        assert!(err.contains("Move") && err.contains("not a core op"), "{}", err);
        // Raw names work in both modes.
        lower_ok("t:\n    mov r0, 1\n", "x86_64");
        lower_ok("t:\n    mov r0, 1\n", "x86_64");
    }

    #[test]
    fn user_alias_overrides_the_sheet() {
        let src = "alias Jump = jz

_start:
    Jump r0, r1, .done
.done:
    jmp .done
";
        let asm = lower_ok(src, "x86_64");
        // Jump = jz: compare-and-branch (3 operands) wins over jmp;
        // line breaks in the output join with "; " in the assertion.
        let joined = asm.replace('\n', "; ");
        assert!(joined.contains("cmpq %rcx, %rax; je L_start__done"), "{}", asm);
    }

    #[test]
    fn label_aliases_resolve_at_emission_and_reference() {
        let src = "alias entry = _start\n\nentry:\n    Jump entry\n";
        let asm = lower_ok(src, "x86_64");
        assert!(asm.contains("_start:"), "{}", asm);
        assert!(asm.contains("jmp _start"), "{}", asm);
        assert!(!asm.contains("entry:"), "{}", asm);
    }

    #[test]
    fn branch_exception_bodies_use_friendly_names() {
        let src = "defn f x\n    default => Move x, 0\n    x86_64 => Xor x, x, x\n\n\
                   _start:\n    f r3\n    ret\n";
        let x86 = lower_ok(src, "x86_64");
        assert!(x86.contains("xor %rbx, %rbx, %rbx"), "{}", x86);
        let arm = lower_ok(src, "aarch64");
        assert!(arm.contains("mov x3, #0"), "{}", arm);
    }
}

// ── W-tier notices (2026-09-22, acknowledge tier) ────────────────────
mod notices_tests {
    use super::*;
    use crate::backend::bad::notices::Notice;

    fn notices(src: &str, triple: &str) -> Vec<Notice> {
        generate_with_notices(src, triple, false, None, true).unwrap().1
    }

    fn codes(src: &str, triple: &str) -> Vec<String> {
        notices(src, triple).iter().map(|n| n.code.to_string()).collect()
    }

    #[test]
    fn w1_fires_on_caller_saved_live_across_call() {
        let c = codes("t:\n    mov r5, 1\n    call f\n    ret\n", "x86_64");
        assert!(c.contains(&"W1".to_string()), "{c:?}");
    }

    #[test]
    fn w1_silent_for_callee_saved() {
        let c = codes("t:\n    mov r10, 1\n    call f\n    ret\n", "x86_64");
        assert!(!c.contains(&"W1".to_string()), "{c:?}");
    }

    #[test]
    fn w3_fires_on_ret_with_sp_delta() {
        let c = codes("t:\n    push r0\n    ret\n", "x86_64");
        assert!(c.contains(&"W3".to_string()), "{c:?}");
    }

    #[test]
    fn w5_fires_on_defn_inlined_ret() {
        let c = codes("defn f x\n    ret\n\nt:\n    call f\n", "x86_64");
        assert!(c.contains(&"W5".to_string()), "{c:?}");
    }

    #[test]
    fn ack_suppresses_and_records() {
        let n = notices("t:\n    mov r5, 1\n    ^ W1 call f\n    ret\n", "x86_64");
        let w1 = n.iter().find(|x| x.code == "W1").expect("W1 recorded (never silent)");
        assert!(w1.acknowledged, "{w1:?}");
        let n = notices("t:\n    mov r5, 1\n    ^ call f\n    ret\n", "x86_64");
        let w1 = n.iter().find(|x| x.code == "W1").expect("bare ^ acks");
        assert!(w1.acknowledged, "{w1:?}");
    }

    #[test]
    fn ack_on_wrong_line_does_not_suppress() {
        // `^` (Instr scope) on the mov line does NOT cover the call's W1.
        let n = notices("t:\n    ^ mov r5, 1\n    call f\n    ret\n", "x86_64");
        let w1 = n.iter().find(|x| x.code == "W1").expect("W1 still fires");
        assert!(!w1.acknowledged, "{w1:?}");
    }

    #[test]
    fn line_ack_propagates_across_segments() {
        let n = notices("t:\n    ^^ mov r5, 1; call f\n    ret\n", "x86_64");
        let w1 = n.iter().find(|x| x.code == "W1").expect("W1 recorded");
        assert!(w1.acknowledged, "{w1:?}");
    }

    #[test]
    fn stale_named_ack_is_a_loud_error() {
        let err = generate("t:\n    ^ W9 mov r5, 1\n    ret\n", "x86_64").unwrap_err();
        assert!(err.contains("stale"), "{err}");
    }

    #[test]
    fn stale_ack_not_for_unfired_bare_ack() {
        // Bare `^` (no names) never goes stale — it acknowledges whatever fires.
        let n = notices("t:\n    ^ mov r5, 1\n    ret\n", "x86_64");
        assert!(n.is_empty(), "{n:?}");
    }

    #[test]
    fn w2_fires_on_branch_path_imbalance() {
        let c = codes("defn f x
    default => push r0
    x86_64 => pop r0

t:\n    call f
", "x86_64");
        assert!(c.contains(&"W2".to_string()), "{c:?}");
    }
}
