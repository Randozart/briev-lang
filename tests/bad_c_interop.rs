//! .bad ↔ C interop: an 8-arg `.export` callee reads its stack-passed
//! arguments (args 7+) via `loadoff` at the `abi_stack_arg_base` offset;
//! a C caller drives it. Hardware-verified per target (host cc, qemu
//! cross-gcc) with honest skips when toolchains are absent.
//!
//! SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

use std::process::Command;

const SUM8: &str = r#"
section .text
export sum8

sum8:
    // args 1-6 arrive in the abi_args order (x86_64: r5,r4,r2,r1,r6,r7
    // = rdi,rsi,rdx,rcx,r8,r9); args 7-8 ride the stack (base 8).
    add r0, r5, r4
    add r0, r0, r2
    add r0, r0, r1
    add r0, r0, r6
    add r0, r0, r7
    loadoff r8, sp, 8
    loadoff r9, sp, 16
    add r0, r0, r8
    add r0, r0, r9
    ret

section .data
scratch: .zero 8
"#;

const CALLER: &str = r#"
#include <stdio.h>
extern long sum8(long, long, long, long, long, long, long, long);
int main(void) {
    printf("%ld\n", sum8(1, 2, 3, 4, 5, 6, 7, 8));
    return 0;
}
"#;

#[test]
fn eight_arg_export_sums_via_stack_for_c_caller() {
    let exe = env!("CARGO_BIN_EXE_brievc");
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("bad_c_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // cc (and the assembler) stage their own temporaries in $TMPDIR —
    // point it under target/ so the test survives a full /tmp.
    let tmpdir = dir.join("tmp");
    std::fs::create_dir_all(&tmpdir).unwrap();
    let bad = dir.join("sum8.bad");
    let c = dir.join("caller.c");
    std::fs::write(&bad, SUM8).unwrap();
    std::fs::write(&c, CALLER).unwrap();

    // Stack-arg offsets (8/16 = abi_stack_arg_base 8 + stride 8) pin the
    // x86_64 SysV layout; the host cc path is the verified one here.
    if !Command::new(exe)
        .args(["bad", bad.to_str().unwrap(), "--target", "x86_64"])
        .current_dir(&dir)
        .env("TMPDIR", &tmpdir)
        .status()
        .expect("brievc")
        .success()
    {
        panic!("bad compile failed");
    }
    let cc = Command::new("cc")
        .arg(c.to_str().unwrap())
        .arg(dir.join("sum8.o"))
        .arg("-o")
        .arg(dir.join("interop"))
        .env("TMPDIR", &tmpdir)
        .status()
        .expect("cc");
    assert!(cc.success(), "cc link failed");
    let out = Command::new(dir.join("interop"))
        .output()
        .expect("run interop");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(stdout.trim(), "36", "1+..+8 via six registers + two stack args");
}
