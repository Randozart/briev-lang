//! In-process GPU runtime executor (plan gpu-backend-hardening Track A).
//!
//! The Briev runtime (lib/runtime/briev_accel_rt.c) is compiled into brievc
//! by build.rs; this module exposes the minimal FFI surface the `brievc run`
//! phase machine needs — init, launch, download, shutdown — and drives the
//! node loop over a `RunProgram` built by the SPIR-V runner.
//!
//! UNSAFE boundary: all FFI lives here; callers use the safe `run_program`.

pub use crate::backend::spirv::runner::{RunDispatch, RunProgram};

/// BRIEV_FIELD_ARRAY (lib/runtime/briev_accel_rt.c).
const FIELD_ARRAY: u32 = 1;
/// BRIEV_FIELD_SCALAR.
const FIELD_SCALAR: u32 = 2;

/// The C `BrievField` layout (abi: name, kind, host_offset, elem_bytes,
/// count, is_write, proj_offset — the 2026-09-01 declared-proj-offset ABI).
#[repr(C)]
pub(super) struct FfiField {
    name: *const std::os::raw::c_char,
    kind: u32,
    host_offset: u64,
    elem_bytes: u64,
    count: u64,
    is_write: u32,
    proj_offset: u64,
}

/// The C `BrievKernelDesc` layout — MUST match briev_accel_rt.c exactly
/// (tail fields: images, block_threads, shared_bytes, program_bytes,
/// seed_fields, n_seed_fields). The 2026-09-15 fix: previously only the
/// first five fields were declared, so the runtime read garbage for every
/// tail field.
#[repr(C)]
pub(super) struct FfiKernelDesc {
    txn_name: *const std::os::raw::c_char,
    spirv: *const u8,
    spirv_size: u32,
    n_fields: u32,
    fields: *const FfiField,
    n_images: u32,
    images: *const std::os::raw::c_void,
    block_threads: u32,
    shared_bytes: u32,
    program_bytes: u64,
    seed_fields: *const FfiField,
    n_seed_fields: u32,
}

unsafe extern "C" {
    fn briev_accel_init(descs: *const FfiKernelDesc, n: u32) -> i32;
    fn briev_accel_available() -> i32;
    fn briev_accel_device_name() -> *const std::os::raw::c_char;
    fn briev_accel_launch_resident_2d(
        idx: u32,
        state: *mut u8,
        nx: u64,
        ny: u64,
    ) -> i32;
    fn briev_accel_launch_resident_batch(
        idx: u32,
        state: *mut u8,
        nx: u64,
        ny: u64,
        times: u32,
    ) -> i32;
    fn briev_accel_download(idx: u32, state: *mut u8) -> i32;
    fn briev_accel_shutdown();
}

/// Drive the run program: init the RT, fire the node phase machine
/// (declaration order, one fast-forwarded dispatch per node per pass),
/// download the outputs, shut down. Returns the per-node final counters.
pub fn run_program(prog: &RunProgram) -> Result<Vec<i64>, String> {
    unsafe { ffi::run_program_impl(prog) }
}

/// The guard cap matches the generated runner's (2e9 passes).
const RUN_GUARD: i64 = 2_000_000_000;

mod ffi {
    use super::{FIELD_ARRAY, FIELD_SCALAR};
    use crate::backend::spirv::runner::{RunDispatch, RunProgram};
    use super::RUN_GUARD;
    use super::{FfiField, FfiKernelDesc};
    use super::{
        briev_accel_available, briev_accel_device_name, briev_accel_download,
        briev_accel_init, briev_accel_launch_resident_2d,
        briev_accel_launch_resident_batch, briev_accel_shutdown,
    };

    /// Build an `FfiField` table over the fields whose name passes `keep`.
    /// Used for the global table (all), the seed table (inputs only), and
    /// each kernel's touched table (Phase 3) — one definition of the ABI row.
    fn ffi_table<'a>(
        fields: &'a [crate::backend::spirv::runner::RunnerField],
        c_names: &'a [std::ffi::CString],
        keep: impl Fn(&crate::backend::spirv::runner::RunnerField) -> bool,
    ) -> Vec<FfiField> {
        fields
            .iter()
            .filter(|f| keep(f))
            .zip(c_names.iter())
            .map(|(f, name)| FfiField {
                name: name.as_ptr(),
                kind: if f.is_array { FIELD_ARRAY } else { FIELD_SCALAR },
                host_offset: f.offset,
                elem_bytes: f.elem_bytes as u64,
                count: f.count,
                is_write: if f.is_array { 1 } else { 0 },
                proj_offset: f.proj_offset,
            })
            .collect()
    }

    pub(super) unsafe fn run_program_impl(prog: &RunProgram) -> Result<Vec<i64>, String> {
        // Field table: name-sorted, host offsets packed, proj offsets declared.
        let c_names: Vec<std::ffi::CString> = prog
            .fields
            .iter()
            .map(|f| {
                std::ffi::CString::new(f.name.clone())
                    .map_err(|_| format!("field '{}': name has NUL byte", f.name))
            })
            .collect::<Result<_, _>>()?;
        let ffi_fields = ffi_table(&prog.fields, &c_names, |_| true);

        // The seed table — input arrays only (Phase 3): the one-time
        // resident seed uploads these; kernel-written arrays are
        // device-produced. Reuse targets are write-first and disjoint.
        let seed_names: std::collections::HashSet<&String> =
            prog.seed_fields.iter().collect();
        let seed_ffi =
            ffi_table(&prog.fields, &c_names, |f| seed_names.contains(&f.name));

        // Kernel descs: one per node, each with its OWN touched-field table
        // (Phase 3 — no kernel packs a field it does not touch).
        let mut k_names: Vec<std::ffi::CString> = Vec::new();
        let mut ffi_descs: Vec<FfiKernelDesc> = Vec::new();
        let mut k_tables: Vec<Vec<FfiField>> = Vec::new();
        for k in &prog.kernels {
            k_names.push(std::ffi::CString::new(k.name.clone())
                .map_err(|e| format!("node '{}': name has NUL byte", k.name))?);
            let table = ffi_table(&prog.fields, &c_names, |f| {
                k.touched_fields.iter().any(|n| n == &f.name)
            });
            k_tables.push(table);
            let last = ffi_descs.len();
            ffi_descs.push(FfiKernelDesc {
                txn_name: k_names[last].as_ptr(),
                spirv: k.spirv.as_ptr(),
                spirv_size: k.spirv.len() as u32,
                n_fields: k_tables[last].len() as u32,
                fields: k_tables[last].as_ptr(),
                n_images: 0,
                images: std::ptr::null(),
                block_threads: k.block_threads,
                shared_bytes: k.shared_bytes,
                program_bytes: prog.program_bytes,
                seed_fields: seed_ffi.as_ptr(),
                n_seed_fields: seed_ffi.len() as u32,
            });
        }

        if briev_accel_init(ffi_descs.as_ptr(), ffi_descs.len() as u32) == 0 {
            return Err("no GPU device available (briev_accel_init failed)".into());
        }
        let name = briev_accel_device_name();
        let name = if name.is_null() {
            "?".to_string()
        } else {
            std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned()
        };
        println!("[run] device: {}", name);

        // State: calloc'd (zeroed) — counters start at 0, arrays at 0.
        let state_bytes = prog.state_bytes as usize + 64;
        let mut state = vec![0u8; state_bytes];

        // Phase machine: declaration order, one fast-forwarded dispatch per
        // node per pass; loop until a full pass fires nothing. The FIRST
        // pass is a warm-up: the initial launch takes the full-copy fallback
        // (which lazily allocates the staging buffer) and never seeds the
        // resident state, so the download needs a second resident launch —
        // the generated runners have the same 2-launch floor.
        let mut counters: Vec<i64> = vec![0; prog.kernels.len()];
        let mut guard: i64 = 0;
        for pass in 0..2 {
            for (i, k) in prog.kernels.iter().enumerate() {
                if pass == 1 && counters[i] >= k.count {
                    continue;
                }
                let idx = i as u32;
                let state_ptr = state.as_mut_ptr();
                match &k.dispatch {
                    RunDispatch::Items(nx) => {
                        if *nx > 0
                            && briev_accel_launch_resident_2d(idx, state_ptr, *nx, 1) == 0
                        {
                            return Err(format!("node '{}': dispatch failed", k.name));
                        }
                    }
                    RunDispatch::Coop { rows } => {
                        if *rows > 0
                            && briev_accel_launch_resident_2d(idx, state_ptr, 32, *rows) == 0
                        {
                            return Err(format!("node '{}': dispatch failed", k.name));
                        }
                    }
                    RunDispatch::Cols { cols, rows } => {
                        if *rows > 0
                            && briev_accel_launch_resident_2d(idx, state_ptr, *cols, *rows) == 0
                        {
                            return Err(format!("node '{}': dispatch failed", k.name));
                        }
                    }
                }
            }
        }
        // Fast-forward all counters (the runner's post-dispatch assignment).
        for (i, k) in prog.kernels.iter().enumerate() {
            counters[i] = k.count;
        }

        if briev_accel_download(0, state.as_mut_ptr()) == 0 {
            return Err("download failed (kernel outputs unavailable)".into());
        }
        briev_accel_shutdown();

        // Observable outputs: first + last element of each written array
        // field, decoded by element width (2 = f16 bits, 4 = f32, 8 = i64).
        for f in prog.fields.iter().filter(|f| f.is_array) {
            let base = f.offset as usize;
            let eb = f.elem_bytes as usize;
            let last_idx = f.count.saturating_sub(1) as usize;
            let last_base = base + last_idx * eb;
            if last_base + eb > state.len() {
                continue;
            }
            let decode = |off: usize| -> String {
                match eb {
                    4 => {
                        let mut b = [0u8; 4];
                        b.copy_from_slice(&state[off..off + 4]);
                        format!("f32 {:.4}", f32::from_bits(u32::from_le_bytes(b)))
                    }
                    8 => {
                        let mut b = [0u8; 8];
                        b.copy_from_slice(&state[off..off + 8]);
                        format!("i64 {}", i64::from_le_bytes(b))
                    }
                    2 => {
                        let mut b = [0u8; 2];
                        b.copy_from_slice(&state[off..off + 2]);
                        format!("f16 0x{:04x}", u16::from_le_bytes(b))
                    }
                    w => format!("{}B {:02x?}", w, &state[off..off + w.min(4)]),
                }
            };
            println!(
                "[run]   {}[0] = {} | {}[{}] = {}",
                f.name,
                decode(base),
                f.name,
                last_idx,
                decode(last_base)
            );
        }

        // Report observable state: the counters (host mirrors) — outputs live
        // in `state` for the caller to inspect (e.g. --verify diffing).
        Ok(counters)
    }
}

