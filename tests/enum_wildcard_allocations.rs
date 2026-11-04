//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
use std::path::Path;
use std::process::Command;

#[test]
fn discarded_enum_payloads_do_not_allocate() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) { return; }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = tempfile::tempdir().unwrap();
    let generated = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME",root).arg("build")
        .arg(root.join("tests/fixtures/enum_wildcard_probe.bv"))
        .arg("--llvm").arg("--stdlib-path").arg(root.join("lib"))
        .arg("--config-dir").arg(root.join("config"))
        .arg("--out").arg(directory.path()).output().unwrap();
    assert!(generated.status.success(), "{}",String::from_utf8_lossy(&generated.stderr));
    let input = directory.path().join("enum_wildcard_probe.ll");
    let ir = std::fs::read_to_string(&input).unwrap();
    std::fs::write(&input,ir.replace("@main(","@unused_briev_main(")).unwrap();
    let harness = directory.path().join("harness.c");
    std::fs::write(&harness,r#"
        #include <stdint.h>
        #include <stddef.h>
        static volatile uint64_t allocations;
        void *__real_malloc(size_t);
        void *__wrap_malloc(size_t bytes) { ++allocations; return __real_malloc(bytes); }
        extern uint8_t wildcard_probe(uint64_t);
        int main(void) {
            uint64_t present[3] = {0,7,8}, absent[3] = {1,0,0}, several[3] = {2,0,0};
            uint64_t before = allocations;
            if (!wildcard_probe((uintptr_t)present)) return 1;
            if (wildcard_probe((uintptr_t)absent)) return 2;
            if (wildcard_probe((uintptr_t)several)) return 4;
            return allocations != before ? 3 : 0;
        }
    "#).unwrap();
    for optimization in ["-O0","-O3"] {
        let binary = directory.path().join("wildcard");
        let compiled = Command::new("clang").args([optimization,"-ffunction-sections","-fdata-sections"])
            .arg(&input).arg(&harness).arg(root.join("lib/runtime/briev_rt.c"))
            .args(["-Wl,--gc-sections","-Wl,--wrap=malloc","-ldl","-lm","-o"])
            .arg(&binary).output().unwrap();
        assert!(compiled.status.success(),"{}",String::from_utf8_lossy(&compiled.stderr));
        let run = Command::new("timeout").arg("20").arg(binary).output().unwrap();
        assert!(run.status.success(),"{optimization}: discarded payload allocated or changed tag semantics: {:?}",run.status);
    }
}
