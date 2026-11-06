//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! OpenVINO FFI gate — Workstream C of
//! docs/plans/2026-11-05-intel-xpu-openvino-harvest.md.
//!
//! Compiles a Briev program against `lib/std/ai/openvino.bv`, links
//! `libopenvino_c`, and runs a 2x2 f32 MatMul through the GPU plugin:
//!
//!   a = [[1, 2], [3, 4]]   b = identity   ->   result = a
//!
//! The values are exact in f16 and f32, so the asserted 32-bit float words
//! are precision-independent (OpenVINO's default GPU inference precision is
//! f16 — harvest §3.1a). The test skips gracefully when `libopenvino_c` is
//! not installed, so it is inert on machines without the runtime; on the
//! Panther Lake node it is the real GPU gate.
//!
//! Correctness only. Timing belongs to Workstream B (the collected-timing
//! lane), not this gate.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::path::Path;
use std::process::Command;

const PROJECT_ROOT: &str = env!("CARGO_MANIFEST_DIR");

const PROGRAM: &str = r#"
import {
    openvino_core_create, openvino_core_free,
    openvino_read_model, openvino_model_free,
    openvino_compile_model, openvino_compiled_model_free,
    openvino_infer_request_create, openvino_infer_request_free,
    openvino_infer, openvino_input_tensor, openvino_output_tensor,
    openvino_tensor_free, openvino_tensor_data, openvino_tensor_size,
    openvino_tensor_element_type
} from "std/ai/openvino.bv";

let done: Int = 0;
node main [done == 0][done == 1] {
    let core: Int = openvino_core_create();
    println!(core);

    let model: Int = openvino_read_model(core, "__MODEL__", "__WEIGHTS__");
    println!(model);

    let compiled: Int = openvino_compile_model(core, model, "GPU");
    println!(compiled);

    let req: Int = openvino_infer_request_create(compiled);

    // a = [[1,2],[3,4]] as IEEE-754 f32 bit patterns.
    let in0: Int = openvino_input_tensor(req, 0);
    let p0: Int = openvino_tensor_data(in0);
    Store#(p0,      1065353216, 4);  // 1.0
    Store#(p0 + 4,  1073741824, 4);  // 2.0
    Store#(p0 + 8,  1077936128, 4);  // 3.0
    Store#(p0 + 12, 1082130432, 4);  // 4.0

    // b = identity.
    let in1: Int = openvino_input_tensor(req, 1);
    let p1: Int = openvino_tensor_data(in1);
    Store#(p1,      1065353216, 4);  // 1.0
    Store#(p1 + 4,  0,          4);  // 0.0
    Store#(p1 + 8,  0,          4);  // 0.0
    Store#(p1 + 12, 1065353216, 4);  // 1.0

    let st: Int = openvino_infer(req);
    println!(st);

    let res_t: Int = openvino_output_tensor(req, 0);
    println!(openvino_tensor_element_type(res_t));
    println!(openvino_tensor_size(res_t));
    let po: Int = openvino_tensor_data(res_t);
    println!(Load#(po, 4));
    println!(Load#(po + 4, 4));
    println!(Load#(po + 8, 4));
    println!(Load#(po + 12, 4));

    openvino_tensor_free(res_t);
    openvino_tensor_free(in1);
    openvino_tensor_free(in0);
    openvino_infer_request_free(req);
    openvino_compiled_model_free(compiled);
    openvino_model_free(model);
    openvino_core_free(core);

    done = 1;
    endprogram;
};
"#;

// IEEE-754 f32 words for 1.0, 2.0, 3.0, 4.0.
const EXPECTED: [i64; 4] = [1065353216, 1073741824, 1077936128, 1082130432];
// ov_element_type_e::F32.
const ELEM_F32: i64 = 4;

fn openvino_lib_present() -> bool {
    if let Ok(out) = Command::new("ldconfig").arg("-p").output() {
        if String::from_utf8_lossy(&out.stdout).contains("libopenvino_c.so") {
            return true;
        }
    }
    ["/usr/lib/libopenvino_c.so", "/usr/lib64/libopenvino_c.so",
     "/usr/local/lib/libopenvino_c.so"]
        .iter()
        .any(|p| Path::new(p).exists())
}

#[test]
fn openvino_gpu_matmul_ffi_gate() {
    if !openvino_lib_present() {
        eprintln!("openvino_gate: skipped (libopenvino_c not installed)");
        return;
    }

    let root = Path::new(PROJECT_ROOT);
    let model = root.join("tests/fixtures/load/openvino_mm.xml");
    let weights = root.join("tests/fixtures/load/openvino_mm.bin");
    assert!(model.exists(), "missing fixture {}", model.display());

    let out_dir = root.join("target/openvino_gate");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).expect("create out dir");

    let src = out_dir.join("ov_gate.bv");
    let program = PROGRAM
        .replace("__MODEL__", &model.to_string_lossy())
        .replace("__WEIGHTS__", &weights.to_string_lossy());
    std::fs::write(&src, program).expect("write program");

    let build = Command::new(env!("CARGO_BIN_EXE_brievc"))
        .env("BRIEV_HOME", root)
        .current_dir(root)
        .args(["build", &src.to_string_lossy()])
        .args(["--stdlib-path", &root.join("lib").to_string_lossy()])
        .args(["--config-dir", &root.join("config").to_string_lossy()])
        .args(["--out", &out_dir.to_string_lossy()])
        .output()
        .expect("spawn brievc build");
    assert!(
        build.status.success(),
        "openvino gate build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let exe = out_dir.join("ov_gate");
    let run = Command::new("timeout")
        .arg("180")
        .arg(&exe)
        .current_dir(&out_dir)
        .output()
        .expect("run ov_gate");
    assert!(
        run.status.success(),
        "ov_gate failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    let text = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<i64> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("integer line"))
        .collect();
    assert_eq!(lines.len(), 10, "expected 10 output lines, got: {text}");

    assert!(lines[0] > 0, "ov_core_create failed: {text}");
    assert!(lines[1] > 0, "ov_core_read_model failed: {text}");
    assert!(lines[2] > 0, "ov_core_compile_model(GPU) failed: {text}");
    assert_eq!(lines[3], 0, "ov_infer_request_infer failed: {text}");
    assert_eq!(lines[4], ELEM_F32, "output tensor is not F32: {text}");
    assert_eq!(lines[5], 4, "output tensor is not 4 elements: {text}");
    assert_eq!(
        &lines[6..10],
        &EXPECTED,
        "GPU MatMul output drifted from a = [[1,2],[3,4]] (words {}): {text}",
        lines[6..10]
            .iter()
            .map(|w| format!("{w:#010x}"))
            .collect::<Vec<_>>()
            .join(",")
    );
}
