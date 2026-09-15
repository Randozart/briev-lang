//! Accel kernel emission — SPIR-V kernel modules built by REUSING the LLVM
//! expression/statement emitter (emit_stmt/emit_expr + the casting graph)
//! against a kernel-scoped `%State` struct.
//!
//! The kernel state is the MINIMAL projection of the host state restricted to
//! the frontend-proven buffer/scalar contract (`AnalysisResults.accel`
//! `KernelShape`): array buffers (read/write) plus read-only scalars, in a
//! deterministic order. The work-item index is bound to the SPIR-V
//! global-id intrinsic; array reads/writes become GEP + load/store on the
//! buffer fields via the exact same emitter paths the host codegen uses
//! (`emit_array_state_store`, the Index read arm, state-scalar loads). This
//! keeps ONE expression pipeline — no hand-rolled kernel emitter that can
//! drift from CPU codegen (see docs/plans/2026-08-06-accel-gpu-offload.md §6.1).
//!
//! Each kernel is emitted as a self-contained module (`target triple =
//! "spirv64-unknown-unknown"` with its own `%State`), compiled via
//! `llc --mtriple=spirv64-unknown-unknown`, and the resulting blob is embedded
//! in the host module. The host runtime (briev_accel_rt) marshals buffers into
//! the kernel `%State` layout, launches, and unpacks.

use crate::analysis::accel::{AccelDecision, AccelEntry, KernelShape};
use crate::ast::{Expr, Type};
use std::collections::HashMap;

/// A compiled, embeddable SPIR-V kernel blob.
pub(crate) struct AccelKernelBlob {
    pub txn_name: String,
    pub bytes: Vec<u8>,
}

impl super::LlvmBackend {
    /// Emit a SPIR-V kernel for every `Gpu`/`Probe` accel body, using the
    /// STANDALONE SPIR-V backend (src/backend/spirv) — Vulkan-native output
    /// (GLCompute, GLSL450 memory model, LocalSize 64). 2026-08-31 (plan
    /// abv-gpu-by-default, item 4): the old path reused the LLVM emitter +
    /// `llc --mtriple=spirv64`, whose output is OpenCL-flavored SPIR-V
    /// (`OpCapability Kernel`) that Vulkan devices reject at pipeline
    /// creation. The candidate set AND order come from the pre-registered
    /// `accel_kernel_idx`; a kernel whose emission fails keeps its slot with
    /// an EMPTY blob (runtime CPU fallback per kernel).
    pub(super) fn collect_accel_kernels(
        &mut self,
        _entries: &HashMap<String, AccelEntry>,
        items: &[crate::ast::TopLevel],
    ) -> Vec<AccelKernelBlob> {
        let mut blobs = Vec::new();
        let mut names: Vec<String> = self.accel_kernel_idx.keys().cloned().collect();
        names.sort();
        for name in &names {
            let shape = self.accel_entries[name].shape.clone();
            let Some(universe) = self.ctx.type_universe.clone() else {
                self.warnings.push(format!(
                    "warning: accel '{}': no TypeUniverse — staying CPU",
                    name
                ));
                blobs.push(AccelKernelBlob { txn_name: name.clone(), bytes: Vec::new() });
                continue;
            };
            let int_bits = self.ctx.int_bits;
            let mut sb = crate::backend::spirv::SpirvBuilder::new()
                .with_universe(&universe, int_bits);
            // Entry is "main" (each offload module is self-contained; the
            // device drivers look up "main" — kernel.rs's own header note).
            // 2026-09-02: no image plans here — the .bv offload lane keeps
            // texel arrays as plain SSBO fields (image realization is the
            // .abv lane; a .bv accel kernel with a spec-Format array simply
            // stays buffer-backed until the lane catches up).
            let emitted = crate::backend::spirv::kernel::emit_kernel(
                &mut sb, "main", &shape, items, false, &[],
            )
            .and_then(|_| sb.build());
            match emitted {
                Ok(bytes) => blobs.push(AccelKernelBlob { txn_name: name.clone(), bytes }),
                Err(e) => {
                    self.warnings.push(format!(
                        "warning: accel '{}': kernel emission failed — {} (staying CPU)",
                        name, e
                    ));
                    blobs.push(AccelKernelBlob { txn_name: name.clone(), bytes: Vec::new() });
                }
            }
        }
        blobs
    }

    /// Emit one self-contained SPIR-V kernel module. Reuses the host field
    /// maps (compact kernel projection) so every array/scalar access goes
    /// through the standard emitter paths against the kernel `%State`.
    /// Requires the backend's program-wide field maps to be populated
    /// (i.e. after `generate()` built the host field index). Exposed for
    /// integration testing — the emitted IR is the deterministic contract;
    /// SPIR-V blob compilation (`llc`) is machine-dependent.
    pub(crate) fn emit_kernel_module(
        &mut self,
        txn_name: &str,
        shape: &KernelShape,
    ) -> Result<String, String> {
        // Kernel state fields: array buffers (read ∪ write) then scalars,
        // each sorted for determinism.
        let mut arrays: Vec<String> = shape
            .read_buffers
            .iter()
            .chain(shape.write_buffers.iter())
            .cloned()
            .collect();
        arrays.sort();
        arrays.dedup();
        let mut scalars: Vec<String> = shape.scalar_ins.clone();
        scalars.sort();
        scalars.dedup();

        // Build the compact kernel field maps from the program-wide ones.
        let mut kernel_index: HashMap<String, usize> = HashMap::new();
        let mut kernel_types: Vec<String> = Vec::new();
        let mut kernel_briev: Vec<Type> = Vec::new();
        let mut struct_fields: Vec<String> = Vec::new();
        let mut const_globals: Vec<(String, Type, Expr)> = Vec::new();
        for name in arrays.iter().chain(scalars.iter()) {
            if let Some(&fidx) = self.ctx.field_index_map.get(name) {
                let agg = self.ctx.field_types[fidx].clone();
                let briev = self.ctx.field_briev_types[fidx].clone();
                kernel_index.insert(name.clone(), kernel_types.len());
                kernel_types.push(agg.clone());
                kernel_briev.push(briev);
                struct_fields.push(agg);
            } else if let Some((ty, val)) = self.ctx.constants.get(name).cloned() {
                // Read-only global constant referenced by the kernel — it gets
                // a module-local global (the host module's @name is absent here).
                if !matches!(val, Expr::Decimal(_) | Expr::Float(_)) {
                    return Err(format!(
                        "constant '{}' has a non-literal value — kernels need literal consts",
                        name
                    ));
                }
                const_globals.push((name.clone(), ty, val));
            } else {
                return Err(format!("kernel references unknown field '{}'", name));
            }
        }
        if struct_fields.is_empty() {
            return Err("kernel has no state buffers".to_string());
        }

        // Save/restore the program-wide field maps around kernel emission.
        let saved_index = std::mem::replace(&mut self.ctx.field_index_map, kernel_index);
        let saved_types = std::mem::replace(&mut self.ctx.field_types, kernel_types);
        let saved_briev = std::mem::replace(&mut self.ctx.field_briev_types, kernel_briev);

        // Isolated function state — kernels emit after all host emission, so
        // resetting the accumulator/label caches cannot disturb the host module.
        self.fun.reset();

        let mut out = String::new();
        out.push_str(&format!(
            "; Accel kernel: {}\n",
            txn_name
        ));
        out.push_str("target datalayout = \"e-i64:64-v16:16-v24:32-v32:32-v48:64-v96:128-v192:256-v256:256-v512:512-v1024:1024\"\n");
        out.push_str("target triple = \"spirv64-unknown-unknown\"\n\n");
        out.push_str("declare i64 @_Z13get_global_idj(i32) #0\n\n");
        for (cname, cty, cval) in &const_globals {
            let cty_llvm = self.llvm_type(cty);
            let lit = match cval {
                Expr::Decimal(n) => n.to_string(),
                Expr::Float(f) => {
                    // 2026-08-31 (plan abv-gpu-by-default): LLVM 22's SPIR-V
                    // backend rejects DECIMAL float literals ("integer
                    // constant must have integer type"). LangRef hex form
                    // encodes the DOUBLE bits, and for a float-typed
                    // constant the value must round-trip f32 exactly — so
                    // widen the f32 first.
                    let widened = (*f as f32) as f64;
                    format!("0x{:016X}", widened.to_bits())
                }
                _ => unreachable!(),
            };
            out.push_str(&format!(
                "@{} = private constant {} {}\n",
                cname, cty_llvm, lit
            ));
        }
        out.push_str(&format!("%State = type {{ {} }}\n", struct_fields.join(", ")));
        // Entry point is `main`: each kernel module is self-contained (one
        // function), and both device drivers (Vulkan pipeline entry + OpenCL
        // clCreateKernel) expect "main".
        out.push_str(&format!(
            "\ndefine spir_kernel void @main(ptr %state, i64 %n) {{\n",
        ));
        out.push_str("entry:\n");
        out.push_str("  %gtid = call i64 @_Z13get_global_idj(i32 0)\n");
        out.push_str("  %cmp = icmp ult i64 %gtid, %n\n");
        out.push_str("  br i1 %cmp, label %body, label %exit\n");
        out.push_str("body:\n");

        // Bind the work-item index (the virtual `[i < N]` variable) to the
        // global-id register so the standard emitter resolves `i` everywhere.
        self.fun
            .let_bindings
            .insert(shape.index_var.clone(), "%gtid".to_string());
        self.fun
            .let_binding_types
            .insert(shape.index_var.clone(), Type::int());
        self.fun
            .let_original_types
            .insert(shape.index_var.clone(), Type::int());

        for stmt in &shape.kernel_stmts {
            super::emit_stmt::emit_statement(self, &mut out, stmt, "  ");
        }

        self.fun.let_bindings.remove(&shape.index_var);
        self.fun.let_binding_types.remove(&shape.index_var);
        self.fun.let_original_types.remove(&shape.index_var);

        out.push_str("  br label %exit\n");
        out.push_str("exit:\n  ret void\n}\n");

        self.ctx.field_index_map = saved_index;
        self.ctx.field_types = saved_types;
        self.ctx.field_briev_types = saved_briev;
        Ok(out)
    }
}

/// Size in bytes of an LLVM aggregate-type string from the host `%State`
/// layout (`field_types`), e.g. `[16 x float]` → 64, `i64` → 8, `float` → 4.
/// Unknown shapes fall back to 8 (the %State scalar default).
fn llvm_agg_size(s: &str) -> u64 {
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(x) = rest.find(" x ") {
            let n: u64 = rest[..x].trim().parse().unwrap_or(0);
            let elem = rest[x + 3..].trim_end_matches(']').trim();
            return n.saturating_mul(llvm_agg_size(elem));
        }
        return 8;
    }
    match s {
        "i8" => 1,
        "i16" => 2,
        "i32" => 4,
        "i64" => 8,
        "float" => 4,
        "double" => 8,
        "ptr" => 8,
        _ => 8,
    }
}

/// Byte offset of host `%State` field `fidx` (sum of preceding field sizes).
fn host_field_offset(field_types: &[String], fidx: usize) -> u64 {
    field_types[..fidx].iter().map(|t| llvm_agg_size(t)).sum()
}

/// One kernel's descriptor fields in kernel `%State` order (arrays sorted,
/// then scalars sorted), as IR text + the field names.
struct KernelFieldTable {
    fields: Vec<String>,
    names: Vec<String>,
}

/// Field IR entry for one kernel field. `proj_off` is the DEVICE projection
/// offset from the shared layout rule (FnLowerer::projection_offsets —
/// vec4-eligible arrays 16B-aligned); the host `%State` offset stays packed.
fn field_entry_ir(
    host_off: u64,
    proj_off: u64,
    ty: &str,
    name: &str,
    txn: &str,
    write_set: &std::collections::HashSet<&str>,
) -> String {
    let (kind, elem_bytes, count, w) = if ty.starts_with('[') {
        let n: u64 = ty
            .strip_prefix('[')
            .and_then(|r| r.split(" x ").next())
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
        let write = if write_set.contains(name) { 1u32 } else { 0u32 };
        (1u32, llvm_agg_size(ty) / n.max(1), n, write)
    } else {
        (2u32, llvm_agg_size(ty), 1u64, 0u32)
    };
    // 2026-09-02: field order MUST match the C BrievField layout
    // (name, kind, host_offset, elem_bytes, count, is_write, proj_offset) —
    // the old order slotted proj_offset at index 3, which misaligns the
    // runtime's reads and fails opt/clang verification (element type
    // mismatch at is_write).
    format!(
        "%briev.field {{ ptr @str.briev.{}.{}, i32 {}, i64 {}, i64 {}, i64 {}, i32 {}, i64 {} }}",
        txn, name, kind, host_off, elem_bytes, count, w, proj_off
    )
}

/// Build one kernel's field table. 2026-08-31 (plan abv-gpu-by-default):
/// the SPIR-V kernel's SSBO members are ALL state fields sorted by NAME
/// (spirv lower::setup_state_buffer), so the descriptor must list exactly
/// that order — the runtime packs the projection in this order and the
/// kernel indexes members positionally.
fn kernel_field_table(
    backend: &super::LlvmBackend,
    txn: &str,
    items: &[crate::ast::TopLevel],
) -> KernelFieldTable {
    let entry = &backend.accel_entries[txn];
    let shape = &entry.shape;
    // 2026-08-31: the SSBO members are EXACTLY collect_state_fields(items)
    // name-sorted — field_index_map additionally carries internal fields
    // (trg epfd, cycle_count) that must NOT appear in the projection, or the
    // member count/offsets diverge from the kernel's SSBO layout.
    let names: Vec<String> =
        crate::backend::spirv::lower::collect_state_fields(items)
            .into_iter()
            .map(|f| f.name)
            .collect();
    let write_set: std::collections::HashSet<&str> =
        shape.write_buffers.iter().map(|s| s.as_str()).collect();

    let field_types = &backend.ctx.field_types;
    // Device projection offsets from the ONE shared rule (mirrors the
    // kernel's setup_state_buffer walk; 2026-09-01 vec4-projection-layout).
    let proj_offsets = {
        let mut sb = crate::backend::spirv::SpirvBuilder::new()
            .with_universe(backend.ctx.type_universe.as_ref().expect("type universe"), backend.ctx.int_bits);
        let mut sfields = crate::backend::spirv::lower::collect_state_fields(items);
        sfields.sort_by(|a, b| a.name.cmp(&b.name));
        crate::backend::spirv::lower::FnLowerer::projection_offsets(&mut sb, &sfields, None)
            .unwrap_or_default()
    };
    let mut fields = Vec::new();
    for (pos, name) in names.iter().enumerate() {
        let Some(&fidx) = backend.ctx.field_index_map.get(name) else {
            return KernelFieldTable { fields: Vec::new(), names: Vec::new() };
        };
        fields.push(field_entry_ir(
            host_field_offset(field_types, fidx),
            proj_offsets.get(pos).copied().unwrap_or(0) as u64,
            &field_types[fidx],
            name,
            txn,
            &write_set,
        ));
    }
    KernelFieldTable { fields, names }
}

/// Emit one kernel's descriptor: name/field string constants, the fields
/// table, and the descriptor entry. Returns the `%briev.kernel` entry text.
fn emit_one_kernel_desc(
    backend: &super::LlvmBackend,
    out: &mut String,
    k: &AccelKernelBlob,
    items: &[crate::ast::TopLevel],
) -> String {
    let txn = &k.txn_name;
    let table = kernel_field_table(backend, txn, items);
    for name in &table.names {
        let bytes = format!("{}\0", name);
        out.push_str(&format!(
            "@str.briev.{}.{} = private constant [{} x i8] c\"{}\\00\"\n",
            txn,
            name,
            bytes.len(),
            name
        ));
    }
    out.push_str(&format!(
        "@briev_kernel_{}_fields = private constant [{} x %briev.field] [{}]\n",
        txn,
        table.fields.len(),
        table.fields.join(", ")
    ));
    out.push_str(&format!(
        "@str.briev.{} = private constant [{} x i8] c\"{}\\00\"\n",
        txn,
        txn.len() + 1,
        txn
    ));
    // Per-txn auto-tuning verdict (Probe decisions): 0 = CPU, 1 = GPU.
    // The emitted run_probe sets it once at startup; the dispatch wrapper gates
    // on it. Per-txn so independent accel bodies can commit independently.
    out.push_str(&format!(
        "@briev_accel_verdict_{} = private global i32 0\n",
        txn
    ));
    format!(
        // 2026-08-31 (plan abv-gpu-by-default): the blob reference is a full
        // PTR, matching BrievKernelDesc's `const uint8_t* spirv`. The old
        // `i32 ptrtoint` both broke PIE linking (R_X86_64_32 against the
        // blob once it was actually retained) and misaligned the struct
        // against the C descriptor (ptr,ptr,i32,i32,ptr).
        "%briev.kernel {{ ptr @str.briev.{}, ptr @briev_kernel_{}, i32 {}, i32 {}, ptr @briev_kernel_{}_fields }}",
        txn,
        txn,
        k.bytes.len(),
        table.fields.len(),
        txn
    )
}

/// Emit the accel descriptor tables + ABI declares the host program links
/// against (`briev_accel_rt.c`). Each kernel's fields are listed in KERNEL
/// `%State` order (arrays sorted, then scalars sorted) with their HOST
/// offsets, so the runtime's generic pack/unpack matches the kernel's GEPs.
/// Returns the IR text and the txn-name → descriptor-index map the host
/// dispatch wrappers use.
pub(crate) fn emit_accel_descriptors(
    backend: &super::LlvmBackend,
    kernels: &[AccelKernelBlob],
    items: &[crate::ast::TopLevel],
) -> (String, HashMap<String, u32>) {
    let mut out = String::new();
    let mut idx_of: HashMap<String, u32> = HashMap::new();
    if kernels.is_empty() {
        return (out, idx_of);
    }
    out.push_str("\n; === Accel kernel descriptors ===\n");
    // Mirrors the C BrievField exactly — the 7th member is proj_offset
    // (2026-09-01 vec4-projection-layout).
    out.push_str("%briev.field = type { ptr, i32, i64, i64, i64, i32, i64 }\n");
    // 2026-08-31: { name, spirv ptr, size, n_fields, fields } — mirrors the C
    // BrievKernelDesc exactly (the old i32 blob slot misaligned the struct).
    out.push_str("%briev.kernel = type { ptr, ptr, i32, i32, ptr }\n");
    out.push_str("@briev_accel_ready = private global i32 0\n");

    let mut desc_entries: Vec<String> = Vec::new();
    for (i, k) in kernels.iter().enumerate() {
        idx_of.insert(k.txn_name.clone(), i as u32);
        desc_entries.push(emit_one_kernel_desc(backend, &mut out, k, items));
    }
    out.push_str(&format!(
        "@briev_accel_descs = private constant [{} x %briev.kernel] [{}]\n",
        desc_entries.len(),
        desc_entries.join(", ")
    ));
    out.push_str("declare i32 @briev_accel_init(ptr, i32)\n");
    out.push_str("declare i32 @briev_accel_launch(i32, ptr, i64)\n");
    // 2026-09-01 (Track B): resident launches for kernel-pinned .bv programs
    // + CPU-fallback coherence.
    out.push_str("declare i32 @briev_accel_launch_resident(i32, ptr, i64)\n");
    out.push_str("declare i32 @briev_accel_download_written(i32, ptr)\n");
    out.push_str("declare void @briev_accel_invalidate_resident()\n");
    out.push_str("declare i32 @briev_accel_available()\n");
    out.push_str("declare i32 @briev_accel_probe(ptr, ptr, ptr, i64, i64, double, double, ptr)\n");
    (out, idx_of)
}
/// Compile kernel LLVM IR to SPIR-V binary via `llc`.
///
/// Runs `llc --mtriple=spirv64-unknown-unknown -filetype=obj` on the kernel
/// IR. Shelling out is required because LLVM's SPIR-V backend
/// (spirv64-unknown-unknown) is a separate target not enabled in the default
/// LLVM build used by inkwell. Uses unique temp filenames (process + atomic
/// counter) — a fixed path corrupted parallel test runs (TOCTOU, 2026-06-29).
pub(crate) fn compile_to_spirv(ir: &str) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    let tmp_dir = std::env::temp_dir();
    static KERNEL_COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = KERNEL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let unique = format!("briev_kernel_{}_{}", std::process::id(), seq);
    let ir_path = tmp_dir.join(format!("{}.ll", unique));
    let spv_path = tmp_dir.join(format!("{}.spv", unique));

    let mut file = std::fs::File::create(&ir_path)
        .map_err(|e| format!("failed to create temp IR file: {}", e))?;
    file.write_all(ir.as_bytes())
        .map_err(|e| format!("failed to write temp IR: {}", e))?;

    let output = Command::new("llc")
        .arg("--mtriple=spirv64-unknown-unknown")
        // 2026-08-31 (plan abv-gpu-by-default B1): the SPIR-V backend's
        // default `-filetype=asm` emits SPIR-V *assembly text* on LLVM 22
        // (older LLVM emitted binary there) — the embedded blob must be a
        // binary, so the filetype is explicit. Undo: never remove this.
        .arg("-filetype=obj")
        .arg(&ir_path)
        .arg("-o")
        .arg(&spv_path)
        .output()
        .map_err(|e| format!("failed to run llc: {}. Is llc installed?", e))?;

    if !output.status.success() {
        // 2026-08-31 (plan abv-gpu-by-default): keep the failing IR beside the
        // error — kernel-IR bugs are diagnosed from the exact file llc saw.
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "llc failed: {} (kernel IR kept at {})",
            stderr,
            ir_path.display()
        ));
    }

    let binary = std::fs::read(&spv_path)
        .map_err(|e| format!("failed to read SPIR-V output: {}", e))?;
    let _ = std::fs::remove_file(&ir_path);
    let _ = std::fs::remove_file(&spv_path);
    Ok(binary)
}

/// Emit an embedded SPIR-V blob as an LLVM global constant in the host module.
pub(crate) fn embed_spirv_blob(spirv_binary: &[u8], kernel_name: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("\n; Embedded SPIR-V kernel: {}\n", kernel_name));
    out.push_str(&format!(
        "@briev_kernel_{} = private constant [{} x i8] c\"",
        kernel_name,
        spirv_binary.len()
    ));
    // 2026-08-31 (plan abv-gpu-by-default B2): one single-line constant.
    // The former 32-byte `""` + newline wrap produced juxtaposed string
    // segments, which clang rejects ("constant expression type mismatch")
    // for every blob > 32 bytes — i.e. every real kernel. LLVM has no line
    // length limit; hex-escaped bytes are ASCII-safe either way.
    for byte in spirv_binary.iter() {
        out.push_str(&format!("\\{:02X}", byte));
    }
    out.push_str("\", align 4\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_blob_wraps_bytes() {
        let blob = vec![0x03u8, 0x02, 0x23, 0x07];
        let out = embed_spirv_blob(&blob, "force");
        assert!(out.contains("@briev_kernel_force = private constant [4 x i8] c\""));
        assert!(out.contains("\\03\\02\\23\\07"));
    }

    /// 2026-08-31 (B2): blobs > 32 bytes must stay ONE `c"…"` token — the
    /// former `""`+newline wrap made every real kernel unparseable by clang.
    /// Round-trips the hex escapes back to the source bytes.
    #[test]
    fn embed_blob_over_32_bytes_is_single_token() {
        let blob: Vec<u8> = (0..100u8).cycle().take(257).collect();
        let out = embed_spirv_blob(&blob, "big");
        assert!(out.contains("[257 x i8]"), "declared length must match: {out}");
        assert!(!out.contains("\"\""), "no juxtaposed string segments allowed");
        let hex_start = out.find("c\"").expect("constant string") + 2;
        let hex_end = out.find("\", align").expect("constant end");
        let hex = &out[hex_start..hex_end];
        let decoded: Vec<u8> = (0..blob.len())
            .map(|i| u8::from_str_radix(&hex[i * 3 + 1..i * 3 + 3], 16).expect("hex byte"))
            .collect();
        assert_eq!(decoded, blob, "escaped bytes must round-trip exactly");
    }

    /// 2026-08-31 (B1): llc must emit a SPIR-V BINARY (magic 0x07230203
    /// little-endian), not the SPIR-V assembly text that LLVM 22's default
    /// `-filetype=asm` produces. Probe-gated on llc presence.
    #[test]
    fn compile_to_spirv_emits_binary_magic() {
        if std::process::Command::new("llc")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("llc not found — skipping");
            return;
        }
        let ir = "target triple = \"spirv64-unknown-unknown\"\ndefine i32 @main() { ret i32 0 }\n";
        let bin = compile_to_spirv(ir).expect("minimal kernel must compile");
        assert!(bin.len() >= 4, "SPIR-V binary has a header");
        assert_eq!(&bin[..4], &[0x03, 0x02, 0x23, 0x07], "SPIR-V magic bytes");
    }

    #[test]
    fn compile_to_spirv_rejects_garbage_ir() {
        let err = compile_to_spirv("this is not llvm ir").unwrap_err();
        assert!(!err.is_empty());
    }
}
