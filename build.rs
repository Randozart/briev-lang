// ── Compiler-in-Briv: build the needs_state pass library ──────────────
// 2026-08-04 (plan 2026-08-04-compiler-in-briv-dogfood-ffi, P3): produce
// `target/compiler-in-briv/needs_state.so` (the Briv pass compiled by
// briefc) so the crate can dlopen it at runtime. The .so is EMBEDDED via
// cargo:rustc-env (BRIV_COMPILER_IN_BRIV_SO) and loaded on first use by
// src/glue/briv_pass.rs — the same way a host language loads a Briv bridge.
//
// Bootstrap ordering: `briefc` IS this crate's binary, so the FIRST cargo
// build has no briefc yet and skips the pass (the runtime falls back to the
// Rust reference, and the transition test still guards correctness). Every
// build after that finds target/{debug,release}/briefc and rebuilds the .so.
// `cargo:rerun-if-changed` keeps it fresh when the pass source changes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn build_pass(briefc: &Path, bv: &str, out_root: &Path) -> Option<PathBuf> {
    let ok = Command::new(briefc)
        .args(["build", bv, "--library", "--out"])
        .arg(out_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return None;
    }
    // The output .so is `<stem>.so` inside the --out directory.
    let stem = Path::new(bv).file_stem()?.to_string_lossy().to_string();
    let so = out_root.join(format!("{stem}.so"));
    if so.exists() { Some(so) } else { None }
}

fn main() {
    let manifest = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default(),
    );
    let out_root = Path::new(&manifest).join("target").join("compiler-in-briv");
    std::fs::create_dir_all(&out_root).ok(); // fresh worktree bootstrap

    println!("cargo:rerun-if-changed=lib/compiler/needs_state.bv");
    println!("cargo:rerun-if-changed=lib/compiler/soa_reorder.bv");
    println!("cargo:rerun-if-changed=lib/compiler/reader.bv");

    // 2026-09-01 (Track A): the GPU runtime links into brievc for
    // `brievc run` — independent of the briefc bootstrap below.
    build_gpu_rt(&out_root);

    // A prebuilt briefc from a previous build (or BRIEFC_BIN override).
    let briefc = std::env::var("BRIEFC_BIN").ok().map(PathBuf::from).or_else(|| {
        let dbg = Path::new(&manifest).join("target").join("debug").join("briefc");
        let rel = Path::new(&manifest).join("target").join("release").join("briefc");
        if dbg.exists() { Some(dbg) } else if rel.exists() { Some(rel) } else { None }
    });

    let Some(briefc) = briefc else {
        println!("cargo:warning=compiler-in-Briv: no prebuilt briefc found on first build — pass libraries skipped (runtime falls back to the Rust references)");
        println!("cargo:rustc-env=BRIV_COMPILER_IN_BRIV_SO=");
        println!("cargo:rustc-env=BRIV_COMPILER_IN_BRIV_SOA_SO=");
        return;
    };

    // Each pass is an independent .so, dlopen'd by src/glue/briv_pass.rs.
    match build_pass(&briefc, "lib/compiler/needs_state.bv", &out_root) {
        Some(so) => {
            println!("cargo:rustc-env=BRIV_COMPILER_IN_BRIV_SO={}", so.display());
            println!("cargo:warning=compiler-in-Briv: needs_state pass ready at {}", so.display());
        }
        None => {
            println!("cargo:warning=compiler-in-Briv: needs_state pass build failed — runtime falls back to the Rust reference");
            println!("cargo:rustc-env=BRIV_COMPILER_IN_BRIV_SO=");
        }
    }
    match build_pass(&briefc, "lib/compiler/soa_reorder.bv", &out_root) {
        Some(so) => {
            println!("cargo:rustc-env=BRIV_COMPILER_IN_BRIV_SOA_SO={}", so.display());
            println!("cargo:warning=compiler-in-Briv: soa_reorder pass ready at {}", so.display());
        }
        None => {
            println!("cargo:warning=compiler-in-Briv: soa_reorder pass build failed — runtime falls back to the Rust reference");
            println!("cargo:rustc-env=BRIV_COMPILER_IN_BRIV_SOA_SO=");
        }
    }
}

/// ── GPU runtime into brievc (plan gpu-backend-hardening Track A) ──────
/// `brievc run x.abv` drives lib/runtime/briev_accel_rt.c IN-PROCESS: the
/// RT compiles into a static archive linked into the binary, and
/// src/gpu_rt.rs exposes the phase machine over FFI. If no C compiler is
/// available the cfg flag stays off and `brievc run` reports cleanly.
fn build_gpu_rt(out_root: &Path) {
    let rt_dir = Path::new("lib/runtime");
    if !rt_dir.join("briev_accel_rt.h").exists() {
        println!("cargo:warning=gpu-rt: ABI header missing — brievc run unavailable");
        return;
    }
    // The RT dlopens libvulkan/OpenCL at init (LOAD macro) — no -lvulkan
    // link needed; -ldl -lpthread -lm cover dlopen/pthread/libm.
    let cc = match Command::new("cc").arg("--version").output() {
        Ok(o) if o.status.success() => "cc".to_string(),
        _ => match Command::new("clang").arg("--version").output() {
            Ok(o) if o.status.success() => "clang".to_string(),
            _ => {
                println!("cargo:warning=gpu-rt: no cc/clang — brievc run unavailable");
                return;
            }
        },
    };
    // 2026-09-21 (Family K): the orchestration moved to src/accel_rt.rs
    // (exported from the Rust staticlib with the same briev_accel_* C
    // symbols); this archive now carries ONLY the C driver bindings,
    // each its own translation unit (types via briev_accel_rt.h; the
    // g_verbose / g_async_launch ints resolve to the Rust exports).
    let arc = out_root.join("libbriev_gpu_rt.a");
    let mut objs = Vec::new();
    let mut all_ok = true;
    for drv in ["briev_dev_cuda.c", "briev_dev_vulkan.c", "briev_dev_opencl.c"] {
        let src = rt_dir.join(drv);
        let obj = out_root.join(format!(
            "{}.o",
            drv.trim_end_matches(".c")
        ));
        let out = Command::new(&cc)
            .args(["-O2", "-fPIC", "-c"])
            .arg("-I")
            .arg(rt_dir)
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .output()
            .expect("cc spawns");
        let ok = out.status.success();
        if !ok {
            println!("cargo:warning=gpu-rt: {} compile failed — brievc run unavailable", drv);
            eprintln!("gpu-rt cc stderr: {}", String::from_utf8_lossy(&out.stderr));
            eprintln!("gpu-rt cc stdout: {}", String::from_utf8_lossy(&out.stdout));
            all_ok = false;
            break;
        }
        objs.push(obj);
    }
    let ar_ok = all_ok && Command::new("ar")
        .args(["rcs"])
        .arg(&arc)
        .args(&objs)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ar_ok {
        println!("cargo:warning=gpu-rt: ar failed — brievc run unavailable");
        return;
    }
    // The Rust orchestration staticlib (same briev_accel_* C ABI, built
    // from src/accel_rt.rs) — runners and LLVM binaries link this instead
    // of the former C translation unit.
    let manifest = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default(),
    );
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let accel_lib = out_root.join("libbriev_accel_rt.a");
    let accel_ok = Command::new(&rustc)
        .args([
            "--edition=2024",
            "--crate-type=staticlib",
            "--crate-name=briev_accel_rt",
            "-O",
        ])
        .arg(manifest.join("src/accel_rt_standalone.rs"))
        .arg("-o")
        .arg(&accel_lib)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !accel_ok {
        println!("cargo:warning=gpu-rt: accel_rt staticlib build failed — brievc run unavailable");
        return;
    }
    println!("cargo:rustc-link-search=native={}", out_root.display());
    println!("cargo:rustc-link-lib=static=briev_gpu_rt");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rerun-if-changed={}", rt_dir.join("briev_accel_rt.h").display());
    for drv in ["briev_dev_cuda.c", "briev_dev_vulkan.c", "briev_dev_opencl.c"] {
        println!("cargo:rerun-if-changed={}", rt_dir.join(drv).display());
    }
    println!("cargo:rustc-cfg=gpu_rt");
}
// SENTINEL: gpu-rt build script loaded (gpu-backend-hardening Track A)
