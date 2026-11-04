//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! Exercise the exported native print lane past a stdout-buffer flush.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
use std::{fs, path::Path, process::Command};

#[test]
fn integer_output_retains_no_per_call_allocations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root)
        .arg("build").arg(root.join("tests/fixtures/load/print_integer_gate.bv"))
        .arg("--llvm").arg("--stdlib-path").arg(root.join("lib"))
        .arg("--config-dir").arg(root.join("config")).arg("--out").arg(dir)
        .output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let ir = dir.join("print_integer_gate.ll");
    fs::write(&ir, fs::read_to_string(&ir).unwrap().replace("@main(", "@fixture_main(")).unwrap();
    let harness = dir.join("probe.c");
    fs::write(&harness, r#"
        #include <stdint.h>
        #include <stddef.h>
        void *__real_malloc(size_t);
        void __real_free(void *);
        static volatile int64_t live;
        void *__wrap_malloc(size_t size) {
            void *allocated = __real_malloc(size);
            if (allocated) ++live;
            return allocated;
        }
        void __wrap_free(void *ptr) { if (ptr) --live; __real_free(ptr); }
        extern int64_t print_int(int64_t);
        extern int64_t stdout_flush(void);
        int main(void) {
            if (print_int(0)) return 1;
            int64_t baseline = live;
            for (int index = 0; index < 2048; ++index) {
                if (print_int(INT64_MIN) || print_int(INT64_MAX)) return 2;
            }
            stdout_flush();
            return live != baseline;
        }
    "#).unwrap();
    let binary = dir.join("probe");
    let linked = Command::new("clang").args(["-O3", "-ffunction-sections", "-fdata-sections"])
        .arg(ir).arg(harness).arg(root.join("lib/runtime/briev_rt.c"))
        .args(["-Wl,--gc-sections", "-Wl,--wrap=malloc", "-Wl,--wrap=free", "-ldl", "-lm", "-o"])
        .arg(&binary).output().unwrap();
    assert!(linked.status.success(), "{}", String::from_utf8_lossy(&linked.stderr));
    let result = Command::new("timeout").arg("20").arg(binary).output().unwrap();
    let expected = format!("0{}", format!("{}{}", i64::MIN, i64::MAX).repeat(2048));
    assert_eq!(result.stdout, expected.as_bytes());
    assert!(result.status.success(), "integer writes left live allocations: {:?}", result.status);
}
