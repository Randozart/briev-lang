//! Raw `.bad` golden corpus — pins the exact assembly emitted for
//! physical-register code. The assembly+ work must not change these
//! outputs (modulo generated label names).
//!
//! Regenerate with `UPDATE_GOLDEN=1 cargo test --test bad_raw_golden`.
//!
//! SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

use std::path::PathBuf;

const TARGETS: &[(&str, &str)] = &[
    ("x86_64-unknown-linux-gnu", "x86_64"),
    ("aarch64-unknown-none", "aarch64"),
    ("riscv64-unknown-none", "riscv64"),
    ("thumbv7m-none-eabi", "thumbv7m"),
];

const RAW_BASIC: &str = r#"
section .text
global _start
_start:
    mov r0, 0xB8000
    mov r1, 66
.loop:
    ldub r2, r0
    jz r2, r1, .done
    store r2, r0
    add r0, r0, 1
    jmp .loop
.done:
    halt
"#;

const RAW_DEFN: &str = r#"
section .text
global _start

defn add_pair dst, a, b
    add dst, a, b
    add dst, dst, 1

_start:
    mov r0, 2
    mov r1, 3
    add_pair r2, r0, r1
    ret
"#;

const RAW_SITE: &str = r#"
section .text
global _start
_start:
    site
        default => add r0, r1, 2
        x86_64 => lea r0, [r1 + 2]
    end site
    ret
"#;

const RAW_RAWBLOCK: &str = r#"
section .text
global _start
_start:
    mov r0, 1
raw x86_64
    addq $1, %rax
end raw
after:
    ret
"#;

const RAW_CONTRACT: &str = r#"
section .text
global _start
_start: [r10 preserved]
    push r10
    mov r10, 1
    add r10, r10, 2
    pop r10
    ret
"#;

const CORPUS: &[(&str, &str)] = &[
    ("raw_basic", RAW_BASIC),
    ("raw_defn", RAW_DEFN),
    ("raw_site", RAW_SITE),
    ("raw_rawblock", RAW_RAWBLOCK),
    ("raw_contract", RAW_CONTRACT),
];

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bad_raw_golden")
}

#[test]
fn raw_assembly_golden() {
    let update = std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1");
    let dir = golden_dir();
    if update {
        std::fs::create_dir_all(&dir).unwrap();
    }
    for (name, source) in CORPUS {
        for (triple, family) in TARGETS {
            let asm = briev_compiler::backend::bad::generate(source, triple)
                .unwrap_or_else(|e| panic!("{name}/{family}: {e}"));
            let path = dir.join(format!("{name}.{family}.s"));
            if update {
                std::fs::write(&path, &asm).unwrap();
            } else {
                let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
                    panic!(
                        "missing golden {}; run UPDATE_GOLDEN=1 cargo test --test bad_raw_golden",
                        path.display()
                    )
                });
                assert_eq!(asm, expected, "raw golden drifted: {}", path.display());
            }
        }
    }
}
