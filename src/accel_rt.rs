// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! Briev Accel Runtime — the Rust host orchestrator (Family K, plan
//! `2026-09-21-family-k-accel-host-consolidation.md`).
//!
//! The compiler never names a device. It emits SPIR-V kernel blobs +
//! per-kernel layout descriptors and calls the stable `briev_accel_*` C ABI
//! (defined of record in `lib/runtime/briev_accel_rt.h`). This module is the
//! ORCHESTRATION half of that ABI — device selection, seeding, dirty-range
//! tracking, the lazy prime (with the 2× snapshot/restore from `02418f49`),
//! batch amortization, readback, strided push, and the auto-tune probe —
//! ported 1:1 from the former C state machine so every consumer (LLVM IR
//! declares, generated runners, harness scripts, `brievc run`) keeps
//! linking by symbol, unchanged.
//!
//! The device drivers (`lib/runtime/briev_dev_*.c`) stay C bindings behind
//! the `BrievDeviceDriver` ops table; they are linked as separate
//! translation units and reference this module's exported `g_verbose` /
//! `g_async_launch` ints exactly as they did when the runtime was one C
//! translation unit.
//!
//! THREADING CONTRACT: single-threaded, same as the C runtime it replaces
//! (callers are the generated runner's main loop, the probe, and
//! `brievc run` — all single-threaded). Global state lives in
//! `static mut RT` and is touched only through the accessor below; there is
//! no lock by design, not by omission.

#![allow(static_mut_refs)]

use std::ffi::CStr;
use std::ffi::c_char;
use std::time::Instant;
use std::ptr::{addr_of, addr_of_mut};

// ── repr(C) ABI mirrors — briev_accel_rt.h is the definition of record ──

pub const BRIEV_FIELD_ARRAY: u32 = 1;
pub const BRIEV_FIELD_SCALAR: u32 = 2;
pub const BRIEV_DEV_CAP_ZERO_COPY: u32 = 0x1;
pub const BRIEV_DEV_CAP_SHARED_STATE: u32 = 0x2;

#[repr(C)]
pub struct BrievField {
    pub name: *const c_char,
    pub kind: u32,
    pub host_offset: u64,
    pub elem_bytes: u64,
    pub count: u64,
    pub is_write: u32,
    pub proj_offset: u64,
}

#[repr(C)]
pub struct BrievImageDesc {
    pub name: *const c_char,
    pub host_offset: u64,
    pub width: u32,
    pub height: u32,
    pub format: u32,
}

#[repr(C)]
pub struct BrievKernelDesc {
    pub txn_name: *const c_char,
    pub spirv: *const u8,
    pub spirv_size: u32,
    pub n_fields: u32,
    pub fields: *const BrievField,
    pub n_images: u32,
    pub images: *const BrievImageDesc,
    pub block_threads: u32,
    pub shared_bytes: u32,
    pub program_bytes: u64,
    pub seed_fields: *const BrievField,
    pub n_seed_fields: u32,
    pub ptx: *const u8,
    pub ptx_size: u32,
    pub block_per_workitem: u32,
}

#[repr(C)]
pub struct BrievStridedCopy {
    pub src: *const core::ffi::c_void,
    pub proj_off: usize,
    pub src_pitch: usize,
    pub dst_pitch: usize,
    pub width_bytes: usize,
    pub count: usize,
}

#[repr(C)]
pub struct BrievPushDesc {
    pub src: *const core::ffi::c_void,
    pub proj_off: usize,
    pub count: usize,
    pub elem_bytes: usize,
    pub src_pitch: usize,
    pub dst_pitch: usize,
}

type AvailableFn = unsafe extern "C" fn() -> i32;
type InitFn = unsafe extern "C" fn() -> i32;
type CreateKernelFn =
    unsafe extern "C" fn(*const u8, usize, *mut *mut core::ffi::c_void) -> i32;
type LaunchFn = unsafe extern "C" fn(
    *mut core::ffi::c_void,
    *const core::ffi::c_void,
    usize,
    usize,
    *mut core::ffi::c_void,
) -> i32;
type DestroyFn = unsafe extern "C" fn(*mut core::ffi::c_void);
type ShutdownFn = unsafe extern "C" fn();
type MappedFn = unsafe extern "C" fn(*mut core::ffi::c_void) -> *mut core::ffi::c_void;
type LaunchDevFn = unsafe extern "C" fn(*mut core::ffi::c_void, usize) -> i32;
type LaunchDev2dFn = unsafe extern "C" fn(
    *mut core::ffi::c_void,
    usize,
    usize,
    i32,
    *const usize,
    u32,
) -> i32;
type DownloadDevFn = unsafe extern "C" fn(*mut core::ffi::c_void) -> i32;
type LaunchDev2dBatchFn = unsafe extern "C" fn(
    *mut core::ffi::c_void,
    usize,
    usize,
    u32,
    i32,
    *const usize,
    u32,
) -> i32;
type DeviceNameFn = unsafe extern "C" fn() -> *const c_char;
type SetImagesFn =
    unsafe extern "C" fn(*mut core::ffi::c_void, *const BrievImageDesc, u32) -> i32;
type DownloadImagesFn = unsafe extern "C" fn(
    *mut core::ffi::c_void,
    *const BrievImageDesc,
    u32,
    *mut core::ffi::c_void,
) -> i32;
type SetU32Fn = unsafe extern "C" fn(*mut core::ffi::c_void, u32) -> i32;
type UploadRangesFn =
    unsafe extern "C" fn(*mut core::ffi::c_void, *const usize, u32) -> i32;
type PushStridedFn =
    unsafe extern "C" fn(*mut core::ffi::c_void, *const BrievStridedCopy, u32) -> i32;

#[repr(C)]
pub struct BrievDeviceDriver {
    pub name: *const c_char,
    pub capabilities: u32,
    pub available: Option<AvailableFn>,
    pub init: Option<InitFn>,
    pub create_kernel: Option<CreateKernelFn>,
    pub launch: Option<LaunchFn>,
    pub destroy_kernel: Option<DestroyFn>,
    pub shutdown: Option<ShutdownFn>,
    pub mapped: Option<MappedFn>,
    pub launch_dev: Option<LaunchDevFn>,
    pub launch_dev2d: Option<LaunchDev2dFn>,
    pub download_dev: Option<DownloadDevFn>,
    pub launch_dev2d_batch: Option<LaunchDev2dBatchFn>,
    pub device_name: Option<DeviceNameFn>,
    pub set_images: Option<SetImagesFn>,
    pub download_images: Option<DownloadImagesFn>,
    pub set_block_threads: Option<SetU32Fn>,
    pub set_shared_bytes: Option<SetU32Fn>,
    pub upload_ranges: Option<UploadRangesFn>,
    pub push_strided: Option<PushStridedFn>,
    pub set_block_per_workitem: Option<SetU32Fn>,
}

// Driver tables (defined in the C drivers; referenced as data symbols).
unsafe extern "C" {
    unsafe static briev_dev_cuda: BrievDeviceDriver;
    unsafe static briev_dev_vulkan: BrievDeviceDriver;
    unsafe static briev_dev_opencl: BrievDeviceDriver;
}

// Shared RT state the C drivers read (declared extern in the header).
#[unsafe(no_mangle)]
pub static mut g_verbose: i32 = 0;
#[unsafe(no_mangle)]
pub static mut g_async_launch: i32 = 0;

// ── Runtime state (single-threaded contract — see module doc) ──

struct Rt {
    driver: *const BrievDeviceDriver,
    init_done: bool,
    resident_seeded: Vec<u8>,
    // 2026-09-14 (gpu_schedule Phase 0): program-level residency seed for
    // drivers with BRIEV_DEV_CAP_SHARED_STATE — the shared buffer is seeded
    // once, then arrays persist on-device (scalars only upload dirty).
    program_seeded: bool,
    // 2026-09-14 (Phase 2, sync elimination): kernels on ONE stream execute
    // in submission order, so the per-launch synchronize is pure overhead —
    // when set (BRIEV_ACCEL_ASYNC=1), resident launches submit WITHOUT
    // syncing; sync happens at download / explicit sync.
    async_launch: bool,
    kernels: Vec<*mut core::ffi::c_void>,
    descs: *const BrievKernelDesc,
    n_kernels: u32,
}

static mut RT: Rt = Rt {
    driver: std::ptr::null(),
    init_done: false,
    resident_seeded: Vec::new(),
    program_seeded: false,
    async_launch: false,
    kernels: Vec::new(),
    descs: std::ptr::null(),
    n_kernels: 0,
};

/// The single-threaded state accessor.
fn rt() -> &'static mut Rt {
    unsafe { &mut *addr_of_mut!(RT) }
}

fn driver() -> Option<&'static BrievDeviceDriver> {
    let d = rt().driver;
    if d.is_null() {
        None
    } else {
        Some(unsafe { &*d })
    }
}

/// `.name` as a Rust str lossily (driver names are ASCII).
fn driver_name(d: &BrievDeviceDriver) -> String {
    unsafe { CStr::from_ptr(d.name).to_string_lossy().into_owned() }
}

fn desc_name(k: &BrievKernelDesc) -> String {
    unsafe { CStr::from_ptr(k.txn_name).to_string_lossy().into_owned() }
}

// ── Device selection (config default + BRIEV_ACCEL_DEVICE + fallback) ──

/// BRIEV_ACCEL_DEVICE names a specific driver — take it when available.
fn env_preferred(chain: &[*const BrievDeviceDriver], env: &str) -> *const BrievDeviceDriver {
    for &d in chain {
        let drv = unsafe { &*d };
        if driver_name(drv) != env {
            continue;
        }
        let ok = drv.available.map_or(false, |f| unsafe { (f)() } != 0);
        if ok {
            return d;
        }
    }
    std::ptr::null()
}

fn select_driver() -> *const BrievDeviceDriver {
    // 2026-09-08 (S1): CUDA first — the perf tier (PTX tensor cores). Falls
    // to Vulkan when libcuda is absent.
    let chain = [
        addr_of!(briev_dev_cuda),
        addr_of!(briev_dev_vulkan),
        addr_of!(briev_dev_opencl),
    ];
    if let Ok(env) = std::env::var("BRIEV_ACCEL_DEVICE") {
        if !env.is_empty() {
            let picked = env_preferred(&chain, &env);
            if !picked.is_null() {
                return picked;
            }
        }
        // env names a driver that is unavailable → fall through to the chain.
    }
    for d in chain {
        let drv = unsafe { &*d };
        let ok = drv.available.map_or(false, |f| unsafe { (f)() } != 0);
        if ok {
            return d;
        }
    }
    std::ptr::null()
}

/// Register the kernel set. Call once from the emitted program's init.
/// Returns 1 when a device is active and all kernels compiled, 0 for CPU.
/// 2026-08-31 (abv-gpu-by-default): a failed device init or kernel compile
/// marks the chain DEAD (available()==0 → clean CPU fallback).
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_init(descs: *const BrievKernelDesc, n: u32) -> i32 {
    let verbose = std::env::var("BRIEV_ACCEL_VERBOSE").is_ok();
    let async_env = std::env::var("BRIEV_ACCEL_ASYNC").is_ok();
    unsafe {
        addr_of_mut!(g_verbose).write(verbose as i32);
        addr_of_mut!(g_async_launch).write(async_env as i32);
    }
    let r = rt();
    if !r.init_done {
        r.driver = init_driver(verbose);
        r.init_done = true;
    }
    r.descs = descs;
    r.n_kernels = n;
    let Some(d) = driver() else {
        return 0;
    };
    r.kernels.clear();
    r.kernels.resize(n as usize, std::ptr::null_mut());
    r.resident_seeded.clear();
    r.resident_seeded.resize(n as usize, 0);
    if !create_kernels(d, descs, n, verbose) {
        return 0;
    }
    1
}

/// Compile + configure kernel `i`'s blob on the driver (blob selection,
/// per-kernel overrides, images). Returns false when the chain must die.
fn create_kernels(
    d: &BrievDeviceDriver,
    descs: *const BrievKernelDesc,
    n: u32,
    verbose: bool,
) -> bool {
    let r = rt();
    for i in 0..n as usize {
        let kdesc = unsafe { &*descs.add(i) };
        let Some((blob, blob_size)) = kernel_blob(d, kdesc, verbose) else {
            r.kernels[i] = std::ptr::null_mut();
            continue;
        };
        let Some(handle) = compile_blob(d, kdesc, blob, blob_size, verbose) else {
            r.driver = std::ptr::null();
            return false;
        };
        apply_kernel_overrides(d, kdesc, handle);
        // 2026-09-02: image-resident arrays need the driver's image path.
        // Absent = loud refusal (a silent skip leaves the descriptor
        // unwritten and the kernel reading garbage).
        if kdesc.n_images > 0 && !setup_images(d, kdesc, handle) {
            r.driver = std::ptr::null();
            return false;
        }
        r.kernels[i] = handle;
    }
    true
}

/// 2026-09-17 (M2.0): per-driver blob selection — CUDA consumes PTX TEXT
/// (cuModuleLoadData JITs it), Vulkan/OpenCL consume SPIR-V. `None` = an
/// EMPTY blob: a per-kernel CPU fallback slot (descriptor indices stay
/// stable) — skipped, not fatal.
fn kernel_blob(
    d: &BrievDeviceDriver,
    kdesc: &BrievKernelDesc,
    verbose: bool,
) -> Option<(*const u8, u32)> {
    let (blob, blob_size) = if driver_name(d) == "cuda" {
        (kdesc.ptx, kdesc.ptx_size)
    } else {
        (kdesc.spirv, kdesc.spirv_size)
    };
    if blob_size == 0 {
        if verbose {
            eprintln!(
                "[briev_accel] kernel '{}' has no {} image — CPU lane",
                desc_name(kdesc),
                driver_name(d)
            );
        }
        return None;
    }
    Some((blob, blob_size))
}

/// Compile one blob on the driver; verbose diagnostics name kernel and
/// driver. `None` → the driver rejected it (chain dies).
fn compile_blob(
    d: &BrievDeviceDriver,
    kdesc: &BrievKernelDesc,
    blob: *const u8,
    blob_size: u32,
    verbose: bool,
) -> Option<*mut core::ffi::c_void> {
    let mut handle: *mut core::ffi::c_void = std::ptr::null_mut();
    let created = d
        .create_kernel
        .map_or(false, |f| unsafe { (f)(blob, blob_size as usize, &mut handle) } != 0);
    if !created {
        if verbose {
            eprintln!(
                "[briev_accel] kernel '{}' rejected by driver '{}' — CPU fallback",
                desc_name(kdesc),
                driver_name(d)
            );
        }
        return None;
    }
    if verbose {
        eprintln!(
            "[briev_accel] kernel '{}' compiled on '{}'",
            desc_name(kdesc),
            driver_name(d)
        );
    }
    Some(handle)
}

/// 2026-09-09/2026-09-18: per-kernel block size, shared bytes, and the
/// block-per-workitem dispatch flag (all optional driver hooks).
fn apply_kernel_overrides(d: &BrievDeviceDriver, kdesc: &BrievKernelDesc, handle: *mut core::ffi::c_void) {
    if kdesc.block_threads > 0 {
        if let Some(f) = d.set_block_threads {
            unsafe { (f)(handle, kdesc.block_threads) };
        }
    }
    if kdesc.shared_bytes > 0 {
        if let Some(f) = d.set_shared_bytes {
            unsafe { (f)(handle, kdesc.shared_bytes) };
        }
    }
    if kdesc.block_per_workitem != 0 {
        if let Some(f) = d.set_block_per_workitem {
            unsafe { (f)(handle, kdesc.block_per_workitem) };
        }
    }
}

/// Allocate + bind the kernel's device storage images. false → the chain
/// dies (missing driver support or allocation failure).
fn setup_images(
    d: &BrievDeviceDriver,
    kdesc: &BrievKernelDesc,
    handle: *mut core::ffi::c_void,
) -> bool {
    let Some(f) = d.set_images else {
        eprintln!(
            "[briev_accel] kernel '{}' has {} image array(s) but driver '{}' has no image support",
            desc_name(kdesc),
            kdesc.n_images,
            driver_name(d)
        );
        return false;
    };
    let ok = unsafe { (f)(handle, kdesc.images, kdesc.n_images) } != 0;
    if !ok {
        eprintln!(
            "[briev_accel] image allocation failed for '{}'",
            desc_name(kdesc)
        );
    }
    ok
}

/// One-time device selection + init. NULL → CPU fallback (reason printed
/// when verbose).
fn init_driver(verbose: bool) -> *const BrievDeviceDriver {
    let selected = select_driver();
    if selected.is_null() {
        if verbose {
            eprintln!("[briev_accel] no device driver available — CPU fallback");
        }
        return std::ptr::null();
    }
    let d = unsafe { &*selected };
    let ok = d.init.map_or(true, |f| unsafe { (f)() } != 0);
    if !ok {
        if verbose {
            eprintln!(
                "[briev_accel] driver '{}' init failed — CPU fallback",
                driver_name(d)
            );
        }
        return std::ptr::null();
    }
    selected
}

/// 1 when a device is active (after init), 0 → CPU path.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_available() -> i32 {
    let r = rt();
    (r.init_done && !r.driver.is_null()) as i32
}

/// The active DEVICE name (e.g. "NVIDIA GeForce RTX 3060"), or the driver
/// name, or "cpu". 2026-09-02: the device op when the driver provides it —
/// run diagnostics should name the GPU, not the API.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_device_name() -> *const c_char {
    let r = rt();
    if !r.init_done || r.driver.is_null() {
        return b"cpu\0".as_ptr() as *const c_char;
    }
    let d = unsafe { &*r.driver };
    if let Some(f) = d.device_name {
        let n = unsafe { (f)() };
        if !n.is_null() {
            return n;
        }
    }
    d.name
}

// ── Generic pack/unpack + dispatch ──

/// Total device projection size. 2026-09-15 (Phase 3): the compiler
/// supplies `program_bytes` (the union extent over ALL buffer fields)
/// because a kernel's SSBO struct still spans every member. Falls back to
/// the field-derived end for older descriptors (program_bytes == 0).
fn proj_size(k: &BrievKernelDesc) -> u64 {
    if k.program_bytes != 0 {
        return k.program_bytes;
    }
    let mut end = 0u64;
    for j in 0..k.n_fields as usize {
        let f = unsafe { &*k.fields.add(j) };
        let f_end = f.proj_offset + f.count * f.elem_bytes;
        if f_end > end {
            end = f_end;
        }
    }
    end
}

/// 2026-09-15 (Phase 3): seed the program's INPUT arrays (the compiler's
/// seed table) into the device mirror. Inputs are read-first; write-first
/// reuse targets are disjoint from inputs — the seed never clobbers a live
/// value.
unsafe fn seed_program_fields(k: &BrievKernelDesc, state: *const u8, mapped: *mut u8) {
    for i in 0..k.n_seed_fields as usize {
        let f = unsafe { &*k.seed_fields.add(i) };
        std::ptr::copy_nonoverlapping(
            state.add(f.host_offset as usize),
            mapped.add(f.proj_offset as usize),
            (f.count * f.elem_bytes) as usize,
        );
    }
}

/// The common early-out: device active, idx in range, kernel handle live.
/// (The C original repeated this chain in every entry point.)
/// Push paths additionally require the shared-state program seed (the
/// ranges are projection-relative; no projection, nothing to push into).
fn push_prereqs(r: &Rt, d: &BrievDeviceDriver) -> bool {
    briev_accel_available() != 0
        && !r.kernels.is_empty()
        && !r.kernels[0].is_null()
        && r.program_seeded
        && d.capabilities & BRIEV_DEV_CAP_SHARED_STATE != 0
}

fn kernel_ready(r: &Rt, idx: u32) -> bool {
    briev_accel_available() != 0
        && idx < r.n_kernels
        && !r.kernels.is_empty()
        && !r.kernels[idx as usize].is_null()
}

fn seeded_flag(r: &Rt, shared: bool, idx: u32) -> bool {
    if shared {
        r.program_seeded
    } else {
        idx < 32 && r.resident_seeded.get(idx as usize).copied().unwrap_or(0) != 0
    }
}

/// Pull ONLY the kernel-written fields VRAM→staging→host (Track B): the
/// resident launch's outputs become visible in host state without
/// re-copying read-only inputs.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_download_written(idx: u32, state: *mut core::ffi::c_void) -> i32 {
    let r = rt();
    let Some(d) = driver() else {
        return 0;
    };
    let shared = d.capabilities & BRIEV_DEV_CAP_SHARED_STATE != 0;
    if !kernel_ready(r, idx) || !seeded_flag(r, shared, idx) {
        return 0;
    }
    let kernel = r.kernels[idx as usize];
    let Some(mapped_fn) = d.mapped else {
        return 0;
    };
    let mapped = unsafe { (mapped_fn)(kernel) } as *mut u8;
    if mapped.is_null() {
        return 0;
    }
    if let Some(f) = d.download_dev {
        unsafe { (f)(kernel) };
    }
    let k = unsafe { &*r.descs.add(idx as usize) };
    // 2026-09-02: image-resident arrays pull via the driver's image path —
    // they are not SSBO fields.
    if k.n_images > 0 {
        download_images_or_warn(d, kernel, k, state, "");
    }
    for i in 0..k.n_fields as usize {
        let f = unsafe { &*k.fields.add(i) };
        if f.is_write == 0 {
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                mapped.add(f.proj_offset as usize),
                (state as *mut u8).add(f.host_offset as usize),
                (f.count * f.elem_bytes) as usize,
            );
        }
    }
    1
}

/// CPU-fallback coherence (Track B): after a CPU accel body writes HOST
/// state, the VRAM working set is stale — clear the seed so the next
/// resident launch re-pushes ALL fields from the host.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_invalidate_resident() {
    let r = rt();
    for flag in r.resident_seeded.iter_mut() {
        *flag = 0;
    }
    r.program_seeded = false;
}

#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_launch(
    idx: u32,
    state: *mut core::ffi::c_void,
    work_n: u64,
) -> i32 {
    let r = rt();
    let Some(d) = driver() else {
        return 0;
    };
    if !kernel_ready(r, idx) {
        return 0;
    }
    let k = unsafe { &*r.descs.add(idx as usize) };
    let bytes = proj_size(k) as usize;
    let mut proj = vec![0u8; bytes.max(1)];
    let mut proj_out = vec![0u8; bytes.max(1)];
    // Pack: copy each field from its host offset into projection order.
    for i in 0..k.n_fields as usize {
        let f = unsafe { &*k.fields.add(i) };
        unsafe {
            std::ptr::copy_nonoverlapping(
                (state as *const u8).add(f.host_offset as usize),
                proj.as_mut_ptr().add(f.proj_offset as usize),
                (f.count * f.elem_bytes) as usize,
            );
        }
    }
    let ok = d.launch.map_or(false, |f| unsafe {
        (f)(
            r.kernels[idx as usize],
            proj.as_ptr() as *const core::ffi::c_void,
            bytes,
            work_n as usize,
            proj_out.as_mut_ptr() as *mut core::ffi::c_void,
        )
    } != 0);
    // Unpack: copy written fields back to the host state.
    for i in 0..k.n_fields as usize {
        let f = unsafe { &*k.fields.add(i) };
        if f.is_write == 0 {
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                proj_out.as_ptr().add(f.proj_offset as usize),
                (state as *mut u8).add(f.host_offset as usize),
                (f.count * f.elem_bytes) as usize,
            );
        }
    }
    ok as i32
}

// ── Device residency (2026-08-31, abv-gpu-by-default item 3): iterative
// kernels keep their array state ON the device across launches. Only scalar
// fields cross PCIe each step. ──

#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_launch_resident(
    idx: u32,
    state: *mut core::ffi::c_void,
    work_n: u64,
) -> i32 {
    briev_accel_launch_resident_2d(idx, state, work_n, 1)
}

/// 2D resident launch: `ny == 1` is the flat form. Scalars sync
/// host→device as dirty byte ranges; the first launch seeds the full
/// projection. Everything else stays in VRAM.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_launch_resident_2d(
    idx: u32,
    state: *mut core::ffi::c_void,
    nx: u64,
    ny: u64,
) -> i32 {
    let r = rt();
    let Some(d) = driver() else {
        return 0;
    };
    if !kernel_ready(r, idx) {
        return 0;
    }
    if resident_ops_missing(d) {
        return briev_accel_launch(idx, state, nx * ny); // driver can't
    }
    let kernel = r.kernels[idx as usize];
    let mapped_fn = d.mapped.unwrap();
    let mut mapped = unsafe { (mapped_fn)(kernel) } as *mut u8;
    let k = unsafe { &*r.descs.add(idx as usize) };
    if mapped.is_null() {
        if !prime_lazy_buffer(idx, kernel, state, nx * ny, mapped_fn) {
            return 0;
        }
        mapped = unsafe { (mapped_fn)(kernel) } as *mut u8;
        if mapped.is_null() {
            return 0;
        }
    }
    let (full_sync, dirty, n_dirty) = seed_or_sync_scalars(d, k, idx, state, mapped);
    // No device→host scalar sync: the host owns the scalars (they were
    // just uploaded); arrays stay device-resident until download.
    let ok = unsafe {
        (d.launch_dev2d.unwrap())(
            kernel,
            nx as usize,
            ny as usize,
            full_sync,
            dirty.as_ptr(),
            n_dirty as u32,
        )
    } != 0;
    // 2026-09-18 (multi-kernel composition): after the FIRST resident
    // dispatch (full_sync=1), the output arrays live on-device but NOT in
    // host state. Pull written fields back so subsequent kernels seed
    // correct inputs.
    if ok && full_sync == 1 {
        briev_accel_download_written(idx, state);
    }
    ok as i32
}

/// Resident dispatch needs mapped + launch_dev + launch_dev2d; without
/// them the runtime falls back to the full-copy launch.
fn resident_ops_missing(d: &BrievDeviceDriver) -> bool {
    d.launch_dev2d.is_none() || d.mapped.is_none() || d.launch_dev.is_none()
}

/// The lazy driver needs a buffer created; the only historical hook was a
/// FULL launch (seed + dispatch + download). 2026-09-21 (BUGS.md
/// 2026-09-20): the prime's download replaces host state with run-1
/// OUTPUTS — snapshot the seed spans before it, restore the authored
/// bytes after, so the resident seed uploads what the program author
/// wrote. (The prime's device work is discarded: full_sync re-seeds every
/// seed field, including the scratch.) One-time per program.
fn prime_lazy_buffer(
    idx: u32,
    kernel: *mut core::ffi::c_void,
    state: *mut core::ffi::c_void,
    work: u64,
    mapped_fn: MappedFn,
) -> bool {
    let r = rt();
    let k = unsafe { &*r.descs.add(idx as usize) };
    let mut snap_bytes = 0usize;
    for i in 0..k.n_seed_fields as usize {
        let f = unsafe { &*k.seed_fields.add(i) };
        snap_bytes += (f.count * f.elem_bytes) as usize;
    }
    let mut snap = vec![0u8; snap_bytes.max(1)];
    let mut off = 0usize;
    for i in 0..k.n_seed_fields as usize {
        let f = unsafe { &*k.seed_fields.add(i) };
        let n = (f.count * f.elem_bytes) as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                (state as *const u8).add(f.host_offset as usize),
                snap.as_mut_ptr().add(off),
                n,
            );
        }
        off += n;
    }
    let primed = briev_accel_launch(idx, state, work) != 0;
    off = 0;
    for i in 0..k.n_seed_fields as usize {
        let f = unsafe { &*k.seed_fields.add(i) };
        let n = (f.count * f.elem_bytes) as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                snap.as_ptr().add(off),
                (state as *mut u8).add(f.host_offset as usize),
                n,
            );
        }
        off += n;
    }
    if !primed {
        return false;
    }
    !unsafe { (mapped_fn)(kernel) }.is_null()
}

/// 2026-09-14 (Phase 0) / 2026-09-15 (Phase 3): with a shared device
/// buffer the FULL projection is seeded once (program-level, from the
/// compiler's INPUT-array seed table); every later launch syncs the host's
/// scalar counters as dirty byte ranges (the phase machine runs on the
/// host — scalars are host-authoritative).
fn seed_or_sync_scalars(
    d: &BrievDeviceDriver,
    k: &BrievKernelDesc,
    idx: u32,
    state: *mut core::ffi::c_void,
    mapped: *mut u8,
) -> (i32, [usize; 32], usize) {
    let r = rt();
    let mut dirty = [0usize; 2 * 16];
    let mut n_dirty = 0usize;
    let shared = d.capabilities & BRIEV_DEV_CAP_SHARED_STATE != 0;
    if !seeded_flag(r, shared, idx) {
        unsafe { seed_program_fields(k, state as *const u8, mapped) };
        if shared {
            r.program_seeded = true;
        } else if (idx as usize) < 32 {
            r.resident_seeded[idx as usize] = 1;
        }
        return (1, dirty, 0);
    }
    for i in 0..k.n_fields as usize {
        if n_dirty >= 16 {
            break;
        }
        let f = unsafe { &*k.fields.add(i) };
        if f.kind != BRIEV_FIELD_SCALAR {
            continue;
        }
        let off = f.proj_offset as usize;
        dirty[2 * n_dirty] = off;
        dirty[2 * n_dirty + 1] = f.elem_bytes as usize;
        n_dirty += 1;
        unsafe {
            std::ptr::copy_nonoverlapping(
                (state as *const u8).add(f.host_offset as usize),
                mapped.add(off),
                f.elem_bytes as usize,
            );
        }
    }
    (0, dirty, n_dirty)
}

/// Batched resident launch (smallm-splitk): `times` IDENTICAL dispatches
/// in one submission. CONTRACT: the host's scalar state must be
/// launch-invariant across the batch; scalars sync ONCE before the
/// dispatches. Returns 1 on success.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_launch_resident_batch(
    idx: u32,
    state: *mut core::ffi::c_void,
    nx: u64,
    ny: u64,
    times: u32,
) -> i32 {
    let r = rt();
    let Some(d) = driver() else {
        return 0;
    };
    if !kernel_ready(r, idx) || times == 0 {
        return 0;
    }
    if resident_ops_missing(d) || d.launch_dev2d_batch.is_none() {
        return batch_sequential(idx, state, nx, ny, times);
    }
    let kernel = r.kernels[idx as usize];
    let mapped = unsafe { (d.mapped.unwrap())(kernel) } as *mut u8;
    if mapped.is_null() {
        return batch_sequential(idx, state, nx, ny, times);
    }
    let k = unsafe { &*r.descs.add(idx as usize) };
    // Scalars sync ONCE — the batch contract is launch-invariant scalars.
    let (full_sync, dirty, n_dirty) = seed_or_sync_scalars(d, k, idx, state, mapped);
    unsafe {
        (d.launch_dev2d_batch.unwrap())(
            kernel,
            nx as usize,
            ny as usize,
            times,
            full_sync,
            dirty.as_ptr(),
            n_dirty as u32,
        )
    }
}

/// Driver can't batch: sequential fallback (identical semantics).
fn batch_sequential(
    idx: u32,
    state: *mut core::ffi::c_void,
    nx: u64,
    ny: u64,
    times: u32,
) -> i32 {
    for _ in 0..times {
        if briev_accel_launch_resident_2d(idx, state, nx, ny) == 0 {
            return 0;
        }
    }
    1
}

/// Pull the FULL projection back to the host state (end of a resident run
/// — observables read host state). Returns 0 when residency isn't active
/// for `idx` (the caller's state is then already current).
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_download(idx: u32, state: *mut core::ffi::c_void) -> i32 {
    let r = rt();
    let Some(d) = driver() else {
        return 0;
    };
    let shared = d.capabilities & BRIEV_DEV_CAP_SHARED_STATE != 0;
    if !kernel_ready(r, idx) || !seeded_flag(r, shared, idx) {
        return 0;
    }
    let kernel = r.kernels[idx as usize];
    let Some(mapped_fn) = d.mapped else {
        return 0;
    };
    let mapped = unsafe { (mapped_fn)(kernel) } as *mut u8;
    if mapped.is_null() {
        return 0;
    }
    // 2026-09-01: with the device-local working set, results land in VRAM
    // — pull them into the staging window before the host-side copy.
    if let Some(f) = d.download_dev {
        unsafe { (f)(kernel) };
    }
    let k = unsafe { &*r.descs.add(idx as usize) };
    if k.n_images > 0 {
        download_images_or_warn(d, kernel, k, state, desc_name(k).as_str());
    }
    for i in 0..k.n_fields as usize {
        let f = unsafe { &*k.fields.add(i) };
        unsafe {
            std::ptr::copy_nonoverlapping(
                mapped.add(f.proj_offset as usize),
                (state as *mut u8).add(f.host_offset as usize),
                (f.count * f.elem_bytes) as usize,
            );
        }
    }
    1
}

/// Pull image-resident arrays through the driver's image path; loud on
/// missing support or failure (a silent skip loses the readback).
fn download_images_or_warn(
    d: &BrievDeviceDriver,
    kernel: *mut core::ffi::c_void,
    k: &BrievKernelDesc,
    state: *mut core::ffi::c_void,
    name: &str,
) {
    let Some(f) = d.download_images else {
        eprintln!(
            "[briev_accel] kernel '{}' has {} image array(s) but the driver cannot download images — no readback",
            name, k.n_images
        );
        return;
    };
    if unsafe { (f)(kernel, k.images, k.n_images, state) } == 0 {
        eprintln!("[briev_accel] image download failed for '{}'", name);
    }
}

/// 2026-09-18 (M4 decode-append): push array byte ranges from the host
/// staging `state` into the device working set WITHOUT a dispatch.
/// `ranges` holds TRIPLES: projection offset, host offset, bytes. Contract:
/// the program must already be seeded through the shared-state resident
/// path and previous launches synchronized. Returns 0 when unavailable.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_push_ranges(
    state: *mut core::ffi::c_void,
    ranges: *const usize,
    n: u32,
) -> i32 {
    let r = rt();
    let Some(d) = driver() else {
        return 0;
    };
    if !push_prereqs(r, d) || d.upload_ranges.is_none() || d.mapped.is_none() {
        return 0;
    }
    let mapped = unsafe { (d.mapped.unwrap())(r.kernels[0]) } as *mut u8;
    if mapped.is_null() {
        return 0;
    }
    let mut dev_ranges = vec![0usize; 2 * n as usize];
    for rr in 0..n as usize {
        let proj_off = unsafe { *ranges.add(3 * rr) };
        let host_off = unsafe { *ranges.add(3 * rr + 1) };
        let len = unsafe { *ranges.add(3 * rr + 2) };
        unsafe {
            std::ptr::copy_nonoverlapping(
                (state as *const u8).add(host_off),
                mapped.add(proj_off),
                len,
            );
        }
        dev_ranges[2 * rr] = proj_off;
        dev_ranges[2 * rr + 1] = len;
    }
    unsafe { (d.upload_ranges.unwrap())(r.kernels[0], dev_ranges.as_ptr(), n) }
}

/// 2026-09-18 (M2 strided append): push strided element ranges from the
/// host staging into the device working set WITHOUT a dispatch — the
/// pitched-copy generalization of push_ranges. The host mirror is gathered
/// first (coherence with download/mirror readers), then the driver performs
/// the pitched host→device copies. Treat 0 as fatal (the mirror may be
/// partially gathered; the caller aborts).
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_push_strided(descs: *const BrievPushDesc, n: u32) -> i32 {
    let r = rt();
    let Some(d) = driver() else {
        return 0;
    };
    if !push_prereqs(r, d) || d.push_strided.is_none() || d.mapped.is_none() {
        return 0;
    }
    let mapped = unsafe { (d.mapped.unwrap())(r.kernels[0]) } as *mut u8;
    if mapped.is_null() {
        return 0;
    }
    // Host mirror gather (element-wise; descriptors are few and rows are
    // 512 B—2 KB, CPU cost negligible next to the HtoD) + driver-level
    // copies: both read the descriptor's ABSOLUTE source pointer.
    let mut copies: Vec<BrievStridedCopy> = Vec::with_capacity(n as usize);
    for rr in 0..n as usize {
        let dsc = unsafe { &*descs.add(rr) };
        let dst = unsafe { mapped.add(dsc.proj_off) };
        for i in 0..dsc.count {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (dsc.src as *const u8).add(i * dsc.src_pitch),
                    dst.add(i * dsc.dst_pitch),
                    dsc.elem_bytes,
                );
            }
        }
        copies.push(BrievStridedCopy {
            src: dsc.src,
            proj_off: dsc.proj_off,
            src_pitch: dsc.src_pitch,
            dst_pitch: dsc.dst_pitch,
            width_bytes: dsc.count * dsc.elem_bytes,
            count: dsc.count,
        });
    }
    unsafe { (d.push_strided.unwrap())(r.kernels[0], copies.as_ptr(), n) }
}

/// Free kernel handles + driver shutdown. Called by program exit.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_shutdown() {
    let r = rt();
    if let Some(d) = driver() {
        for kernel in r.kernels.iter() {
            if !kernel.is_null() {
                if let Some(f) = d.destroy_kernel {
                    unsafe { (f)(*kernel) };
                }
            }
        }
        if let Some(f) = d.shutdown {
            unsafe { (f)() };
        }
    }
    r.resident_seeded = Vec::new();
    r.kernels = Vec::new();
    r.driver = std::ptr::null();
    r.init_done = false;
}

// ── Auto-tuning probe (D7). Runs both lanes on separate state copies;
// verifies output equality within tolerance and commits to the faster
// path. Returns 1 = GPU. ──

/// Auto-tuning probe: runs the CPU and GPU lanes on SEPARATE state copies,
/// times each over `probe_k` full-map runs, and commits to the GPU path
/// only when its wall time beats CPU by the margin AND `gpu_ok` confirms
/// the outputs match within `tolerance`. Returns 1 = GPU, 0 = CPU.
// Praetor note: the 8-parameter signature is the PUBLISHED C ABI — every
// consumer (LLVM IR declares, runners) binds it positionally. It cannot be
// split without breaking the linkage contract this module exists to keep.
#[unsafe(no_mangle)]
pub extern "C" fn briev_accel_probe(
    cpu_fn: unsafe extern "C" fn(*mut core::ffi::c_void),
    gpu_fn: unsafe extern "C" fn(*mut core::ffi::c_void),
    ctx: *mut core::ffi::c_void,
    state_size: u64,
    probe_k: i64,
    tolerance: f64,
    margin: f64,
    gpu_ok: Option<unsafe extern "C" fn(*const core::ffi::c_void, *const core::ffi::c_void, f64, *mut core::ffi::c_void) -> i32>,
) -> i32 {
    if briev_accel_available() == 0 || probe_k <= 0 {
        return 0;
    }
    let n = state_size as usize;
    let mut cpu_state = vec![0u8; n.max(1)];
    let mut gpu_state = vec![0u8; n.max(1)];
    unsafe {
        std::ptr::copy_nonoverlapping(ctx as *const u8, cpu_state.as_mut_ptr(), n);
        std::ptr::copy_nonoverlapping(ctx as *const u8, gpu_state.as_mut_ptr(), n);

        // Warm-up: one dummy full-map run each (first device dispatch is slow).
        (cpu_fn)(cpu_state.as_mut_ptr() as *mut core::ffi::c_void);
        (gpu_fn)(gpu_state.as_mut_ptr() as *mut core::ffi::c_void);
    }

    let t0 = Instant::now();
    for _ in 0..probe_k {
        unsafe { (cpu_fn)(cpu_state.as_mut_ptr() as *mut core::ffi::c_void) };
    }
    let cpu_t = t0.elapsed().as_secs_f64();

    let t0 = Instant::now();
    for _ in 0..probe_k {
        unsafe { (gpu_fn)(gpu_state.as_mut_ptr() as *mut core::ffi::c_void) };
    }
    let gpu_t = t0.elapsed().as_secs_f64();

    // Correctness gate: the GPU lane's result must match the CPU lane's
    // within tolerance — the probe doubles as the safety net against GPU
    // codegen bugs.
    if let Some(ok_fn) = gpu_ok {
        let ok = unsafe {
            (ok_fn)(
                cpu_state.as_ptr() as *const core::ffi::c_void,
                gpu_state.as_ptr() as *const core::ffi::c_void,
                tolerance,
                ctx,
            )
        };
        if ok == 0 {
            return 0;
        }
    }
    (gpu_t * (1.0 + margin) < cpu_t) as i32
}

// ── Self-test (the former BRIEV_ACCEL_SELF_TEST C main) — selection pack
// math and the probe gate on synthetic data, NO device required. ──

#[cfg(test)]
mod self_test {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // 2026-09-24 (BUGS.md accel self-test race): both tests swap the SAME
    // process-global `rt()` driver/init_done around their scenario — run in
    // parallel (default test threads), one test's cleanup
    // (`driver = null; init_done = false`) landed between the other's setup
    // and launch → `briev_accel_launch` returned 0 ("fake launch returns
    // ok"). The scenario state is process-global; serialize the tests on one
    // lock. `into_inner` so a failing test's poisoned guard never cascades.
    // To undo: drop the guards (restores the scheduling-dependent flake).
    static SELF_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn self_test_guard() -> std::sync::MutexGuard<'static, ()> {
        SELF_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[repr(C)]
    struct FakeDriver {
        inner: BrievDeviceDriver,
    }

    unsafe extern "C" fn fake_launch(
        _k: *mut core::ffi::c_void,
        proj: *const core::ffi::c_void,
        bytes: usize,
        _n: usize,
        proj_out: *mut core::ffi::c_void,
    ) -> i32 {
        // Identity copy so write fields round-trip.
        unsafe {
            std::ptr::copy_nonoverlapping(
                proj as *const u8,
                proj_out as *mut u8,
                bytes,
            );
        }
        1
    }

    static REJECT_HITS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn probe_cpu_fn(_ctx: *mut core::ffi::c_void) {}

    unsafe extern "C" fn probe_gpu_fn(_ctx: *mut core::ffi::c_void) {
        REJECT_HITS.fetch_add(1, Ordering::Relaxed);
    }

    unsafe extern "C" fn probe_reject_ok(
        _a: *const core::ffi::c_void,
        _b: *const core::ffi::c_void,
        _tol: f64,
        _ctx: *mut core::ffi::c_void,
    ) -> i32 {
        0
    }

    #[test]
    fn probe_gate_rejecting_gpu_ok_forces_cpu() {
        // The probe returns GPU(1) only when gpu_ok confirms output
        // equality; a rejecting gate forces CPU regardless of timing.
        let _guard = self_test_guard();
        let fake = BrievDeviceDriver {
            name: c"fake".as_ptr(),
            capabilities: 0,
            available: None,
            init: None,
            create_kernel: None,
            launch: Some(fake_launch),
            destroy_kernel: None,
            shutdown: None,
            mapped: None,
            launch_dev: None,
            launch_dev2d: None,
            download_dev: None,
            launch_dev2d_batch: None,
            device_name: None,
            set_images: None,
            download_images: None,
            set_block_threads: None,
            set_shared_bytes: None,
            upload_ranges: None,
            push_strided: None,
            set_block_per_workitem: None,
        };
        let r = rt();
        r.driver = &fake as *const BrievDeviceDriver;
        r.init_done = true;
        let mut pctx: u32 = 4;
        let verdict = briev_accel_probe(
            probe_cpu_fn,
            probe_gpu_fn,
            &mut pctx as *mut u32 as *mut core::ffi::c_void,
            4,
            2,
            0.001,
            0.05,
            Some(probe_reject_ok),
        );
        assert_eq!(verdict, 0, "rejecting gate forces CPU");
        r.driver = std::ptr::null();
        r.init_done = false;
    }

    #[test]
    fn pack_math_and_launch_roundtrip() {
        let _guard = self_test_guard();
        // Field projection order + offsets (declared): array a (Float[4],
        // 4B) is vec4-eligible and 16B-aligned at proj 0; scalar s packed
        // at proj 16 (8B). Host struct has count first (8B) so a's
        // host_offset is 8, s's is 24 — host layouts stay packed; only the
        // projection aligns.
        let name_a = c"a".as_ptr();
        let name_s = c"s".as_ptr();
        let fields = [
            BrievField { name: name_a, kind: BRIEV_FIELD_ARRAY, host_offset: 8, elem_bytes: 4, count: 4, is_write: 1, proj_offset: 0 },
            BrievField { name: name_s, kind: BRIEV_FIELD_SCALAR, host_offset: 24, elem_bytes: 8, count: 1, is_write: 0, proj_offset: 16 },
        ];
        let txn = c"force".as_ptr();
        let desc = BrievKernelDesc {
            txn_name: txn,
            spirv: std::ptr::null(),
            spirv_size: 0,
            n_fields: 2,
            fields: fields.as_ptr(),
            n_images: 0,
            images: std::ptr::null(),
            block_threads: 0,
            shared_bytes: 0,
            program_bytes: 0,
            seed_fields: std::ptr::null(),
            n_seed_fields: 0,
            ptx: std::ptr::null(),
            ptx_size: 0,
            block_per_workitem: 0,
        };

        // ── Pack math ──
        assert_eq!(proj_size(&desc), 24, "proj_size = 16 + 8");

        // ── Launch with the fake driver ──
        let fake = BrievDeviceDriver {
            name: c"fake".as_ptr(),
            capabilities: 0,
            available: None,
            init: None,
            create_kernel: None,
            launch: Some(fake_launch),
            destroy_kernel: None,
            shutdown: None,
            mapped: None,
            launch_dev: None,
            launch_dev2d: None,
            download_dev: None,
            launch_dev2d_batch: None,
            device_name: None,
            set_images: None,
            download_images: None,
            set_block_threads: None,
            set_shared_bytes: None,
            upload_ranges: None,
            push_strided: None,
            set_block_per_workitem: None,
        };
        let r = rt();
        r.driver = &fake as *const BrievDeviceDriver;
        r.init_done = true;
        r.descs = &desc;
        r.n_kernels = 1;
        r.kernels = vec![1usize as *mut core::ffi::c_void];

        let mut host = [0u8; 32];
        let a = &mut host[8..24];
        a[0..4].copy_from_slice(&1.0f32.to_le_bytes());
        a[4..8].copy_from_slice(&2.0f32.to_le_bytes());
        a[8..12].copy_from_slice(&3.0f32.to_le_bytes());
        a[12..16].copy_from_slice(&4.0f32.to_le_bytes());
        host[24..32].copy_from_slice(&42i64.to_le_bytes());
        let ok = briev_accel_launch(0, host.as_mut_ptr() as *mut core::ffi::c_void, 4);
        assert_eq!(ok, 1, "fake launch returns ok");
        let a = &host[8..24];
        assert_eq!(
            f32::from_le_bytes(a[0..4].try_into().unwrap()),
            1.0,
            "write field round-trips"
        );
        assert_eq!(
            f32::from_le_bytes(a[12..16].try_into().unwrap()),
            4.0,
            "write field round-trips"
        );
        assert_eq!(
            i64::from_le_bytes(host[24..32].try_into().unwrap()),
            42,
            "scalar preserved"
        );

        // Restore state so other tests start clean.
        r.driver = std::ptr::null();
        r.init_done = false;
        r.kernels = Vec::new();
    }
}
