// Copyright 2026 Randy Smits-Schreuder Goedheijt
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Metropolitan FFI - Shared Memory Negotiation System
//!
//! Implements zero-copy FFI through OS-level shared memory regions.
//! Briev negotiates memory layouts with foreign languages and treats
//! them as districts within the Briev metropolis.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Cross-platform shared memory
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Status word values for synchronization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum MetroStatus {
    Ready = 0,
    Processing = 1,
    Complete = 2,
    Error(u32) = 0x8000_0000_0000_0000,
}

impl MetroStatus {
    pub fn from_u64(val: u64) -> Self {
        if val & 0x8000_0000_0000_0000 != 0 {
            MetroStatus::Error(val as u32)
        } else {
            match val {
                0 => MetroStatus::Ready,
                1 => MetroStatus::Processing,
                2 => MetroStatus::Complete,
                _ => MetroStatus::Ready,
            }
        }
    }

    pub fn to_u64(&self) -> u64 {
        match self {
            MetroStatus::Ready => 0,
            MetroStatus::Processing => 1,
            MetroStatus::Complete => 2,
            MetroStatus::Error(code) => 0x8000_0000_0000_0000 | (*code as u64),
        }
    }
}

/// Shared memory region backed by OS-level mmap
pub struct SharedRegion {
    pub id: String,
    pub base_addr: usize,
    pub size: usize,
    pub permissions: String,
    pub foreign_id: String,
    #[cfg(unix)]
    pub mmap_ptr: *mut u8,
    #[cfg(windows)]
    pub handle: std::ptr::NonNull<std::ffi::c_void>,
    pub data: UnsafeCell<Vec<u8>>, // Fallback for testing without real mmap
}

unsafe impl Send for SharedRegion {}
unsafe impl Sync for SharedRegion {}

impl SharedRegion {
    /// Create a new shared region (real mmap on Unix/Windows, Vec fallback for tests)
    pub fn new(
        id: &str,
        size: usize,
        permissions: &str,
        foreign_lang: &str,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        {
            Self::new_unix(id, size, permissions, foreign_lang)
        }
        #[cfg(windows)]
        {
            Self::new_windows(id, size, permissions, foreign_lang)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Self::new_fallback(id, size, permissions, foreign_lang)
        }
    }

    #[cfg(unix)]
    fn new_unix(
        id: &str,
        size: usize,
        permissions: &str,
        foreign_lang: &str,
    ) -> Result<Self, String> {
        use libc::{MAP_ANONYMOUS, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE, mmap, munmap};

        let prot = if permissions.contains('w') {
            PROT_READ | PROT_WRITE
        } else {
            PROT_READ
        };

        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                size,
                prot,
                MAP_ANONYMOUS | MAP_SHARED,
                -1,
                0,
            )
        };

        if ptr == MAP_FAILED {
            return Err(format!("mmap failed for region {}", id));
        }

        Ok(SharedRegion {
            id: id.to_string(),
            base_addr: ptr as usize,
            size,
            permissions: permissions.to_string(),
            foreign_id: format!("{}_{}", foreign_lang, id),
            mmap_ptr: ptr as *mut u8,
            #[cfg(windows)]
            handle: std::ptr::NonNull::dangling(),
            data: UnsafeCell::new(Vec::new()),
        })
    }

    #[cfg(windows)]
    fn new_windows(
        id: &str,
        size: usize,
        permissions: &str,
        foreign_lang: &str,
    ) -> Result<Self, String> {
        use windows::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, PAGE_READWRITE, VirtualAlloc,
            VirtualFree,
        };

        let protect = if permissions.contains('w') {
            PAGE_READWRITE
        } else {
            PAGE_READONLY
        };

        let ptr = unsafe { VirtualAlloc(None, size, MEM_COMMIT | MEM_RESERVE, protect) };

        if ptr.is_null() {
            return Err(format!("VirtualAlloc failed for region {}", id));
        }

        Ok(SharedRegion {
            id: id.to_string(),
            base_addr: ptr as usize,
            size,
            permissions: permissions.to_string(),
            foreign_id: format!("{}_{}", foreign_lang, id),
            #[cfg(unix)]
            mmap_ptr: std::ptr::null_mut(),
            handle: std::ptr::NonNull::new(ptr).unwrap(),
            data: UnsafeCell::new(Vec::new()),
        })
    }

    fn new_fallback(
        id: &str,
        size: usize,
        permissions: &str,
        foreign_lang: &str,
    ) -> Result<Self, String> {
        Ok(SharedRegion {
            id: id.to_string(),
            base_addr: 0,
            size,
            permissions: permissions.to_string(),
            foreign_id: format!("{}_{}", foreign_lang, id),
            #[cfg(unix)]
            mmap_ptr: std::ptr::null_mut(),
            #[cfg(windows)]
            handle: std::ptr::NonNull::dangling(),
            data: UnsafeCell::new(vec![0u8; size]),
        })
    }

    /// Write data to the shared region at offset
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<(), String> {
        if offset + data.len() > self.size {
            return Err(format!(
                "Write exceeds region bounds: {} + {} > {}",
                offset,
                data.len(),
                self.size
            ));
        }

        #[cfg(unix)]
        {
            if !self.mmap_ptr.is_null() {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        self.mmap_ptr.add(offset),
                        data.len(),
                    );
                }
                return Ok(());
            }
        }

        #[cfg(windows)]
        {
            if !self.handle.is_null() {
                let ptr = self.handle.as_ptr() as *mut u8;
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.add(offset), data.len());
                }
                return Ok(());
            }
        }

        // Fallback
        let region_data = unsafe { &mut *self.data.get() };
        region_data[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// Read data from the shared region at offset
    pub fn read(&self, offset: usize, size: usize) -> Result<Vec<u8>, String> {
        if offset + size > self.size {
            return Err(format!(
                "Read exceeds region bounds: {} + {} > {}",
                offset, size, self.size
            ));
        }

        #[cfg(unix)]
        {
            if !self.mmap_ptr.is_null() {
                let mut buf = vec![0u8; size];
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.mmap_ptr.add(offset),
                        buf.as_mut_ptr(),
                        size,
                    );
                }
                return Ok(buf);
            }
        }

        #[cfg(windows)]
        {
            if !self.handle.is_null() {
                let ptr = self.handle.as_ptr() as *mut u8;
                let mut buf = vec![0u8; size];
                unsafe {
                    std::ptr::copy_nonoverlapping(ptr.add(offset), buf.as_mut_ptr(), size);
                }
                return Ok(buf);
            }
        }

        // Fallback
        let region_data = unsafe { &*self.data.get() };
        Ok(region_data[offset..offset + size].to_vec())
    }

    /// Read atomic status word
    pub fn read_status(&self, offset: usize) -> Result<MetroStatus, String> {
        if offset + 8 > self.size {
            return Err("Status offset out of bounds".to_string());
        }

        #[cfg(unix)]
        {
            if !self.mmap_ptr.is_null() {
                let atomic_ptr = unsafe { &*(self.mmap_ptr.add(offset) as *const AtomicU64) };
                return Ok(MetroStatus::from_u64(atomic_ptr.load(Ordering::SeqCst)));
            }
        }

        // Fallback
        let data = self.read(offset, 8)?;
        let val = u64::from_le_bytes(data.try_into().unwrap());
        Ok(MetroStatus::from_u64(val))
    }

    /// Write atomic status word
    pub fn write_status(&self, offset: usize, status: MetroStatus) -> Result<(), String> {
        if offset + 8 > self.size {
            return Err("Status offset out of bounds".to_string());
        }

        #[cfg(unix)]
        {
            if !self.mmap_ptr.is_null() {
                let atomic_ptr = unsafe { &*(self.mmap_ptr.add(offset) as *const AtomicU64) };
                atomic_ptr.store(status.to_u64(), Ordering::SeqCst);
                return Ok(());
            }
        }

        // Fallback
        let val = status.to_u64();
        let bytes = val.to_le_bytes();
        self.write(offset, &bytes)
    }

    /// Atomic compare-and-swap
    pub fn atomic_cas(&self, offset: usize, expected: u64, new_value: u64) -> Result<u64, String> {
        if offset + 8 > self.size {
            return Err("CAS offset out of bounds".to_string());
        }

        #[cfg(unix)]
        {
            if !self.mmap_ptr.is_null() {
                let atomic_ptr = unsafe { &*(self.mmap_ptr.add(offset) as *const AtomicU64) };
                return Ok(atomic_ptr
                    .compare_exchange(expected, new_value, Ordering::SeqCst, Ordering::SeqCst)
                    .unwrap_or_else(|v| v));
            }
        }

        // Fallback (not truly atomic, but works for single-threaded tests)
        let current = self.read_status(offset)?.to_u64();
        if current == expected {
            self.write_status(offset, MetroStatus::from_u64(new_value))?;
        }
        Ok(current)
    }

    /// Memory barrier
    pub fn memory_barrier() {
        std::sync::atomic::fence(Ordering::SeqCst);
    }
}

impl Drop for SharedRegion {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            if !self.mmap_ptr.is_null() {
                unsafe {
                    libc::munmap(self.mmap_ptr as *mut _, self.size);
                }
            }
        }

        #[cfg(windows)]
        {
            use windows::Win32::System::Memory::VirtualFree;
            if !self.handle.is_null() {
                unsafe {
                    VirtualFree(
                        self.handle.as_ptr(),
                        0,
                        windows::Win32::System::Memory::MEM_RELEASE,
                    );
                }
            }
        }
    }
}

/// Metropolitan Channel - Three-region communication channel
pub struct MetropolitanChannel {
    pub id: String,
    pub request_region: Arc<SharedRegion>,
    pub response_region: Arc<SharedRegion>,
    pub sync_region: Arc<SharedRegion>,
    pub input_size: usize,
    pub output_size: usize,
}

/// Layout negotiation result
pub struct NegotiatedLayout {
    pub input_offset: usize,
    pub input_size: usize,
    pub output_offset: usize,
    pub output_size: usize,
    pub status_offset: usize,
}

impl MetropolitanChannel {
    /// Create a new Metropolitan channel with negotiated shared memory
    pub fn create(
        channel_id: &str,
        foreign_lang: &str,
        input_size: usize,
        output_size: usize,
    ) -> Result<Self, String> {
        // Calculate total sync region size: request_status + response_status + barrier
        let sync_size = 64;

        // Request three shared regions
        let request_region = Arc::new(SharedRegion::new(
            &format!("{}_request", channel_id),
            input_size,
            "rw",
            foreign_lang,
        )?);

        let response_region = Arc::new(SharedRegion::new(
            &format!("{}_response", channel_id),
            output_size,
            "rw",
            foreign_lang,
        )?);

        let sync_region = Arc::new(SharedRegion::new(
            &format!("{}_sync", channel_id),
            sync_size,
            "rw",
            foreign_lang,
        )?);

        // Initialize status words to Ready
        request_region.write_status(0, MetroStatus::Ready)?;
        response_region.write_status(0, MetroStatus::Ready)?;
        sync_region.write_status(0, MetroStatus::Ready)?; // Request status at offset 0
        sync_region.write_status(8, MetroStatus::Ready)?; // Response status at offset 8

        Ok(MetropolitanChannel {
            id: channel_id.to_string(),
            request_region,
            response_region,
            sync_region,
            input_size,
            output_size,
        })
    }

    /// Send data through the channel (zero-copy write to shared memory)
    pub fn send(&self, data: &[u8]) -> Result<(), String> {
        if data.len() > self.input_size {
            return Err(format!(
                "Data size {} exceeds input buffer {}",
                data.len(),
                self.input_size
            ));
        }

        // Write data to request region
        self.request_region.write(0, data)?;

        // Signal "request ready" via sync region
        self.sync_region.write_status(0, MetroStatus::Processing)?;

        // Memory barrier to ensure ordering
        SharedRegion::memory_barrier();

        Ok(())
    }

    /// Receive response from the channel with timeout
    pub fn receive(&self, timeout_ms: u64) -> Result<Vec<u8>, String> {
        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        while start.elapsed() < timeout {
            let status = self.sync_region.read_status(8)?; // Response status at offset 8

            match status {
                MetroStatus::Complete => {
                    // Memory barrier before reading response
                    SharedRegion::memory_barrier();

                    // Read response from response region
                    let response = self.response_region.read(0, self.output_size)?;

                    // Reset response status to Ready
                    self.sync_region.write_status(8, MetroStatus::Ready)?;

                    return Ok(response);
                }
                MetroStatus::Error(code) => {
                    return Err(format!("Foreign error code: {}", code));
                }
                _ => {
                    // Still processing, wait a bit
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }

        Err("Timeout waiting for response".to_string())
    }

    /// Get the negotiated memory layout for code generation
    pub fn get_layout(&self) -> NegotiatedLayout {
        NegotiatedLayout {
            input_offset: 0,
            input_size: self.input_size,
            output_offset: 0,
            output_size: self.output_size,
            status_offset: 0, // Relative to sync_region
        }
    }

    /// Get the base addresses for foreign code generation
    pub fn get_addresses(&self) -> HashMap<String, usize> {
        let mut addrs = HashMap::new();
        addrs.insert("request_base".to_string(), self.request_region.base_addr);
        addrs.insert("response_base".to_string(), self.response_region.base_addr);
        addrs.insert("sync_base".to_string(), self.sync_region.base_addr);
        addrs.insert("input_size".to_string(), self.input_size);
        addrs.insert("output_size".to_string(), self.output_size);
        addrs
    }
}

/// Metropolitan Hub - Manages all channels
pub struct MetropolitanHub {
    channels: Mutex<HashMap<String, Arc<MetropolitanChannel>>>,
}

impl MetropolitanHub {
    pub fn new() -> Self {
        MetropolitanHub {
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// Create and register a new channel
    pub fn create_channel(
        &self,
        channel_id: &str,
        foreign_lang: &str,
        input_size: usize,
        output_size: usize,
    ) -> Result<Arc<MetropolitanChannel>, String> {
        let channel = Arc::new(MetropolitanChannel::create(
            channel_id,
            foreign_lang,
            input_size,
            output_size,
        )?);

        self.channels
            .lock()
            .unwrap()
            .insert(channel_id.to_string(), channel.clone());
        Ok(channel)
    }

    /// Get an existing channel
    pub fn get_channel(&self, channel_id: &str) -> Option<Arc<MetropolitanChannel>> {
        self.channels.lock().unwrap().get(channel_id).cloned()
    }

    /// Remove and close a channel
    pub fn close_channel(&self, channel_id: &str) -> Result<(), String> {
        self.channels.lock().unwrap().remove(channel_id);
        Ok(())
    }

    /// Generate C header for foreign side
    pub fn generate_c_header(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;

        let addrs = channel.get_addresses();

        Ok(format!(
            r#"
// Metropolitan FFI - C Header for channel: {channel_id}
// Generated by Briev Compiler
// DO NOT EDIT MANUALLY

#include <stdint.h>
#include <stdatomic.h>

// Shared memory regions (map these addresses)
#define REQUEST_BASE  ((volatile uint8_t*)0x{req:x})
#define RESPONSE_BASE ((volatile uint8_t*)0x{resp:x})
#define SYNC_BASE     ((volatile uint8_t*)0x{sync:x})

#define INPUT_SIZE  {input}
#define OUTPUT_SIZE {output}

// Status word offsets in sync region
#define REQUEST_STATUS  ((atomic_uint64_t*)&SYNC_BASE[0])
#define RESPONSE_STATUS ((atomic_uint64_t*)&SYNC_BASE[8])

// Status values
#define STATUS_READY     0
#define STATUS_PROCESSING 1
#define STATUS_COMPLETE  2
#define STATUS_ERROR     0x8000000000000000ULL

// Wait for Briev to send a request
static inline void wait_for_request() {{
    while (atomic_load_explicit(REQUEST_STATUS, memory_order_seq_cst) != STATUS_PROCESSING) {{
        // Spin wait
    }}
}}

// Signal that response is ready
static inline void signal_complete() {{
    atomic_store_explicit(RESPONSE_STATUS, STATUS_COMPLETE, memory_order_seq_cst);
}}

// Signal error
static inline void signal_error(uint32_t code) {{
    atomic_store_explicit(RESPONSE_STATUS, STATUS_ERROR | code, memory_order_seq_cst);
}}

// Reset request status after processing
static inline void reset_request() {{
    atomic_store_explicit(REQUEST_STATUS, STATUS_READY, memory_order_seq_cst);
}}
"#,
            channel_id = channel_id,
            req = addrs.get("request_base").unwrap_or(&0),
            resp = addrs.get("response_base").unwrap_or(&0),
            sync = addrs.get("sync_base").unwrap_or(&0),
            input = addrs.get("input_size").unwrap_or(&0),
            output = addrs.get("output_size").unwrap_or(&0),
        ))
    }

    /// Generate Rust module for foreign side
    pub fn generate_rust_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;

        let addrs = channel.get_addresses();

        Ok(format!(
            r#"
// Metropolitan FFI - Rust Module for channel: {channel_id}
// Generated by Briev Compiler
// DO NOT EDIT MANUALLY

use std::sync::atomic::{{AtomicU64, Ordering}};
use std::slice;

// Shared memory regions (map these addresses via mmap or similar)
const REQUEST_BASE: usize = 0x{req:x};
const RESPONSE_BASE: usize = 0x{resp:x};
const SYNC_BASE: usize = 0x{sync:x};

const INPUT_SIZE: usize = {input};
const OUTPUT_SIZE: usize = {output};

// Status word values
const STATUS_READY: u64 = 0;
const STATUS_PROCESSING: u64 = 1;
const STATUS_COMPLETE: u64 = 2;
const STATUS_ERROR: u64 = 0x8000_0000_0000_0000;

/// Wait for Briev to send a request
pub fn wait_for_request() {{
    let request_status = unsafe {{ &*(SYNC_BASE as *const AtomicU64) }};
    while request_status.load(Ordering::SeqCst) != STATUS_PROCESSING {{
        std::hint::spin_loop();
    }}
}}

/// Signal that response is ready
pub fn signal_complete() {{
    let response_status = unsafe {{ &*((SYNC_BASE + 8) as *const AtomicU64) }};
    response_status.store(STATUS_COMPLETE, Ordering::SeqCst);
}}

/// Signal error
pub fn signal_error(code: u32) {{
    let response_status = unsafe {{ &*((SYNC_BASE + 8) as *const AtomicU64) }};
    response_status.store(STATUS_ERROR | (code as u64), Ordering::SeqCst);
}}

/// Get input data slice
pub fn get_input<'a>() -> &'a [u8] {{
    unsafe {{ slice::from_raw_parts(REQUEST_BASE as *const u8, INPUT_SIZE) }}
}}

/// Get output data slice (mutable)
pub fn get_output<'a>() -> &'a mut [u8] {{
    unsafe {{ slice::from_raw_parts_mut(RESPONSE_BASE as *mut u8, OUTPUT_SIZE) }}
}}

/// Reset request status after processing
pub fn reset_request() {{
    let request_status = unsafe {{ &*(SYNC_BASE as *const AtomicU64) }};
    request_status.store(STATUS_READY, Ordering::SeqCst);
}}
"#,
            channel_id = channel_id,
            req = addrs.get("request_base").unwrap_or(&0),
            resp = addrs.get("response_base").unwrap_or(&0),
            sync = addrs.get("sync_base").unwrap_or(&0),
            input = addrs.get("input_size").unwrap_or(&0),
            output = addrs.get("output_size").unwrap_or(&0),
        ))
    }

    /// Generate Python module for foreign side
    pub fn generate_python_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;

        let addrs = channel.get_addresses();

        Ok(format!(
            r#"
# Metropolitan FFI - Python Module for channel: {channel_id}
# Generated by Briev Compiler
# DO NOT EDIT MANUALLY

import mmap
import ctypes
import time

# Shared memory region addresses
REQUEST_BASE = 0x{req:x}
RESPONSE_BASE = 0x{resp:x}
SYNC_BASE = 0x{sync:x}

INPUT_SIZE = {input}
OUTPUT_SIZE = {output}

# Status values
STATUS_READY = 0
STATUS_PROCESSING = 1
STATUS_COMPLETE = 2
STATUS_ERROR = 0x8000000000000000

# Map shared memory regions (requires actual mmap setup)
def map_region(address, size):
    # In practice, use mmap with the actual file descriptor
    # This is a placeholder for the concept
    return mmap.mmap(-1, size)

# Status word access
def read_request_status():
    # Read atomic uint64 from SYNC_BASE
    return ctypes.c_uint64.from_address(SYNC_BASE).value

def write_response_status(value):
    ctypes.c_uint64.from_address(SYNC_BASE + 8).value = value

def wait_for_request():
    while read_request_status() != STATUS_PROCESSING:
        time.sleep(0.001)

def signal_complete():
    write_response_status(STATUS_COMPLETE)

def signal_error(code):
    write_response_status(STATUS_ERROR | code)

def get_input():
    # Read from REQUEST_BASE
    pass

def get_output():
    # Write to RESPONSE_BASE
    pass

def reset_request():
    ctypes.c_uint64.from_address(SYNC_BASE).value = STATUS_READY
"#,
            channel_id = channel_id,
            req = addrs.get("request_base").unwrap_or(&0),
            resp = addrs.get("response_base").unwrap_or(&0),
            sync = addrs.get("sync_base").unwrap_or(&0),
            input = addrs.get("input_size").unwrap_or(&0),
            output = addrs.get("output_size").unwrap_or(&0),
        ))
    }

    // ===== metropipe-compatible code generation (32-byte header, single-region protocol) =====

    /// Generate a metropipe C header (32-byte header matching metropipe/clients/c/metropipe.h)
    pub fn generate_metropipe_c_header(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;

        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"
/* metropipe C header for channel: {channel_id} */
/* 32-byte header — metropipe/clients/c/metropipe.h compatible */
/* Auto-generated — DO NOT EDIT */

#include <stdint.h>
#include <stdatomic.h>
#include <stddef.h>

#define METRO_STATUS_IDLE         0
#define METRO_STATUS_CONSUMER_REQ 1
#define METRO_STATUS_PROVIDER_ACK 2
#define METRO_STATUS_PROVIDER_RES 3
#define METRO_STATUS_ERROR        4

#define METRO_HEADER_SIZE     32
#define METRO_OFFSET_STATUS   0
#define METRO_OFFSET_CAS_LOCK 4
#define METRO_OFFSET_SIZE     8
#define METRO_OFFSET_CAPACITY 12
#define METRO_OFFSET_ERROR    16
#define METRO_OFFSET_PAYLOAD  32

#define METRO_CAPACITY {max_size}

typedef struct {{
    volatile uint32_t *header;
    volatile uint8_t  *payload;
    size_t capacity;
    int fd;
}} MetroChannel;

int metro_channel_open(MetroChannel *ch, const char *shm_path);
void metro_channel_close(MetroChannel *ch);
int metro_wait_idle(MetroChannel *ch, int timeout_ms);
int metro_channel_send(MetroChannel *ch, const uint8_t *data, size_t len);
int metro_channel_recv(MetroChannel *ch, uint8_t *out, size_t max_len, int timeout_ms);
int metro_channel_request(MetroChannel *ch, const uint8_t *req, size_t req_len,
                          uint8_t *resp, size_t resp_max, int timeout_ms);

static inline uint32_t metro_read_status(MetroChannel *ch) {{
    return atomic_load_explicit((_Atomic uint32_t*)&ch->header[0], memory_order_seq_cst);
}}
static inline void metro_write_status(MetroChannel *ch, uint32_t value) {{
    atomic_store_explicit((_Atomic uint32_t*)&ch->header[0], value, memory_order_seq_cst);
}}
static inline uint32_t metro_read_size(MetroChannel *ch) {{
    return atomic_load_explicit((_Atomic uint32_t*)&ch->header[2], memory_order_seq_cst);
}}
static inline void metro_write_size(MetroChannel *ch, uint32_t size) {{
    atomic_store_explicit((_Atomic uint32_t*)&ch->header[2], size, memory_order_seq_cst);
}}
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }

    /// Generate a metropipe Python module (32-byte header, single-region)
    pub fn generate_metropipe_python_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;

        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"""
metropipe Python stub for {channel_id}
32-byte header — metropipe/clients/python/metropipe.py compatible

Usage:
    from {channel_id}_stub import MetroChannel
    ch = MetroChannel("/dev/shm/metro_{channel_id}")
    result = ch.request(bytes, timeout_ms=5000)
"""

import mmap, struct, time, os


class MetroError(Exception):
    pass


class MetroTimeoutError(MetroError):
    pass


class MetroChannel:
    STATUS_IDLE = 0
    STATUS_CONSUMER_REQ = 1
    STATUS_PROVIDER_ACK = 2
    STATUS_PROVIDER_RES = 3
    STATUS_ERROR = 4

    HEADER_SIZE = 32
    OFFSET_STATUS = 0
    OFFSET_CAS_LOCK = 4
    OFFSET_PAYLOAD_SIZE = 8
    OFFSET_MAX_CAPACITY = 12
    OFFSET_ERROR_CODE = 16
    OFFSET_PAYLOAD = 32

    CAPACITY = {max_size}

    def __init__(self, shm_path):
        self.shm_path = shm_path
        self._mmap = None
        if os.path.exists(shm_path):
            fd = open(shm_path, "r+b")
            self._mmap = mmap.mmap(fd.fileno(), 0)

    def close(self):
        if self._mmap:
            self._mmap.close()

    def _read_status(self):
        return struct.unpack_from("<I", self._mmap, self.OFFSET_STATUS)[0]

    def _write_status(self, value):
        struct.pack_into("<I", self._mmap, self.OFFSET_STATUS, value)

    def request(self, payload, timeout_ms=5000):
        self._write_status(self.STATUS_CONSUMER_REQ)
        size = len(payload)
        self._mmap[self.OFFSET_PAYLOAD:self.OFFSET_PAYLOAD + size] = payload
        struct.pack_into("<I", self._mmap, self.OFFSET_PAYLOAD_SIZE, size)
        start = time.monotonic()
        while True:
            status = self._read_status()
            if status == self.STATUS_PROVIDER_RES:
                resp_size = struct.unpack_from("<I", self._mmap, self.OFFSET_PAYLOAD_SIZE)[0]
                result = bytes(self._mmap[self.OFFSET_PAYLOAD:self.OFFSET_PAYLOAD + resp_size])
                self._write_status(self.STATUS_IDLE)
                return result
            if status == self.STATUS_ERROR:
                raise MetroError("Provider error")
            if (time.monotonic() - start) * 1000 > timeout_ms:
                raise MetroTimeoutError("Timeout")
            time.sleep(0.001)
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }

    /// Generate a metropipe JavaScript module (32-byte header, SharedArrayBuffer)
    pub fn generate_metropipe_js_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;

        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"
// metropipe JS stub for {channel_id}
// 32-byte header — metropipe/clients/javascript/metropipe.js compatible

const STATUS_IDLE = 0;
const STATUS_CONSUMER_REQ = 1;
const STATUS_PROVIDER_RES = 3;
const STATUS_ERROR = 4;

const OFFSET_STATUS = 0;
const OFFSET_PAYLOAD_SIZE = 8;
const OFFSET_PAYLOAD = 32;
const CAPACITY = {max_size};

class MetroChannel {{
    constructor(shmPath) {{
        this.shmPath = shmPath;
        this.header = null;
        this.payload = null;
    }}

    async request(payload, timeoutMs = 5000) {{
        const fs = require('fs');
        const size = fs.statSync(this.shmPath).size;
        this.buffer = new SharedArrayBuffer(size);
        this.header = new Int32Array(this.buffer, 0, 8);
        this.payload = new Uint8Array(this.buffer, OFFSET_PAYLOAD);
        new Uint8Array(this.buffer).set(fs.readFileSync(this.shmPath));

        const start = Date.now();
        while (Atomics.load(this.header, 0) !== STATUS_IDLE) {{
            if (Date.now() - start > timeoutMs) throw new Error('timeout');
            await new Promise(r => setTimeout(r, 1));
        }}
        this.payload.set(payload);
        this.header[2] = payload.length;
        Atomics.store(this.header, 0, STATUS_CONSUMER_REQ);

        const respStart = Date.now();
        while (true) {{
            const status = Atomics.load(this.header, 0);
            if (status === STATUS_PROVIDER_RES) {{
                const respSize = this.header[2];
                const result = this.payload.slice(0, respSize);
                Atomics.store(this.header, 0, STATUS_IDLE);
                return result;
            }}
            if (status === STATUS_ERROR) throw new Error('provider error');
            if (Date.now() - respStart > timeoutMs) throw new Error('timeout');
            await new Promise(r => setTimeout(r, 1));
        }}
    }}
}}

module.exports = {{ MetroChannel }};
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }

    /// Generate a metropipe Rust module (32-byte header via mmap)
    pub fn generate_metropipe_rust_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;
        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"
// metropipe Rust stub for {channel_id}
// 32-byte header — metropipe protocol compatible

use std::sync::atomic::{{AtomicU32, Ordering}};
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::time::Instant;

const SHM_PATH: &str = "/dev/shm/metro_{channel_id}";
const HEADER_SIZE: usize = 32;
const PAYLOAD_OFFSET: usize = 32;
const CAPACITY: usize = {max_size};

const STATUS_IDLE: u32 = 0;
const STATUS_CONSUMER_REQ: u32 = 1;
const STATUS_PROVIDER_RES: u32 = 3;

pub struct MetroChannel {{
    pub fd: std::fs::File,
    pub ptr: *mut u8,
    pub len: usize,
}}

impl MetroChannel {{
    pub fn open() -> Result<Self, String> {{
        let fd = OpenOptions::new()
            .read(true).write(true)
            .open(SHM_PATH)
            .map_err(|e| format!("Cannot open {{}}: {{}}", SHM_PATH, e))?;
        let len = std::fs::metadata(SHM_PATH)
            .map_err(|e| e.to_string())?.len() as usize;
        let ptr = unsafe {{
            libc::mmap(
                std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED, fd.as_raw_fd(), 0,
            )
        }};
        if ptr == libc::MAP_FAILED {{
            return Err("mmap failed".into());
        }}
        Ok(Self {{ fd, ptr: ptr as *mut u8, len }})
    }}

    pub fn request(&self, payload: &[u8], timeout_ms: u64) -> Result<Vec<u8>, String> {{
        unsafe {{
            let status = &*(self.ptr as *const AtomicU32);
            let size_ptr = &*(self.ptr.add(8) as *const AtomicU32);
            let payload_ptr = self.ptr.add(PAYLOAD_OFFSET);

            let start = Instant::now();
            while status.load(Ordering::SeqCst) != STATUS_IDLE {{
                if start.elapsed().as_millis() as u64 > timeout_ms {{
                    return Err("timeout waiting for IDLE".into());
                }}
            }}
            std::ptr::copy_nonoverlapping(payload.as_ptr(), payload_ptr, payload.len());
            size_ptr.store(payload.len() as u32, Ordering::SeqCst);
            status.store(STATUS_CONSUMER_REQ, Ordering::SeqCst);

            let resp_start = Instant::now();
            loop {{
                let s = status.load(Ordering::SeqCst);
                if s == STATUS_PROVIDER_RES {{
                    let resp_size = size_ptr.load(Ordering::SeqCst) as usize;
                    let mut resp = vec![0u8; resp_size];
                    std::ptr::copy_nonoverlapping(payload_ptr, resp.as_mut_ptr(), resp_size);
                    status.store(STATUS_IDLE, Ordering::SeqCst);
                    return Ok(resp);
                }}
                if resp_start.elapsed().as_millis() as u64 > timeout_ms {{
                    return Err("timeout waiting for response".into());
                }}
            }}
        }}
    }}
}}
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }

    /// Generate a metropipe Go module (32-byte header via syscall.Mmap)
    pub fn generate_metropipe_go_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;
        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"
// metropipe Go stub for {channel_id}
// 32-byte header — metropipe protocol compatible

package metropipe

import (
    "os"
    "syscall"
    "time"
    "encoding/binary"
    "unsafe"
)

const SHMPath = "/dev/shm/metro_{channel_id}"
const HeaderSize = 32
const PayloadOffset = 32
const Capacity = {max_size}

const StatusIdle = 0
const StatusConsumerReq = 1
const StatusProviderRes = 3

type MetroChannel struct {{
    data []byte
}}

func Open() (*MetroChannel, error) {{
    f, err := os.OpenFile(SHMPath, os.O_RDWR, 0)
    if err != nil {{
        return nil, err
    }}
    defer f.Close()
    fi, _ := f.Stat()
    data, err := syscall.Mmap(int(f.Fd()), 0, int(fi.Size()),
        syscall.PROT_READ|syscall.PROT_WRITE, syscall.MAP_SHARED)
    if err != nil {{
        return nil, err
    }}
    return &MetroChannel{{data: data}}, nil
}}

func (ch *MetroChannel) Request(payload []byte, timeoutMs int) ([]byte, error) {{
    start := time.Now()
    for binary.LittleEndian.Uint32(ch.data[0:4]) != StatusIdle {{
        if time.Since(start).Milliseconds() > int64(timeoutMs) {{
            return nil, fmt.Errorf("timeout")
        }}
        time.Sleep(time.Millisecond)
    }}
    copy(ch.data[PayloadOffset:PayloadOffset+len(payload)], payload)
    binary.LittleEndian.PutUint32(ch.data[8:12], uint32(len(payload)))
    binary.LittleEndian.PutUint32(ch.data[0:4], StatusConsumerReq)

    respStart := time.Now()
    for {{
        status := binary.LittleEndian.Uint32(ch.data[0:4])
        if status == StatusProviderRes {{
            respSize := binary.LittleEndian.Uint32(ch.data[8:12])
            resp := make([]byte, respSize)
            copy(resp, ch.data[PayloadOffset:PayloadOffset+int(respSize)])
            binary.LittleEndian.PutUint32(ch.data[0:4], StatusIdle)
            return resp, nil
        }}
        if time.Since(respStart).Milliseconds() > int64(timeoutMs) {{
            return nil, fmt.Errorf("timeout")
        }}
        time.Sleep(time.Millisecond)
    }}
}}
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }

    /// Generate a metropipe Java module (32-byte header via MappedByteBuffer)
    pub fn generate_metropipe_java_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;
        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"
// metropipe Java stub for {channel_id}
// 32-byte header — metropipe protocol compatible

import java.io.RandomAccessFile;
import java.nio.MappedByteBuffer;
import java.nio.channels.FileChannel;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

public class MetroChannel {{
    private static final String SHM_PATH = "/dev/shm/metro_{channel_id}";
    private static final int PAYLOAD_OFFSET = 32;
    private static final int CAPACITY = {max_size};

    private static final int STATUS_IDLE = 0;
    private static final int STATUS_CONSUMER_REQ = 1;
    private static final int STATUS_PROVIDER_RES = 3;

    private MappedByteBuffer buf;

    public MetroChannel() throws Exception {{
        RandomAccessFile f = new RandomAccessFile(SHM_PATH, "rw");
        FileChannel ch = f.getChannel();
        this.buf = ch.map(FileChannel.MapMode.READ_WRITE, 0, 32 + CAPACITY);
        this.buf.order(ByteOrder.LITTLE_ENDIAN);
        ch.close();
    }}

    public byte[] request(byte[] payload, long timeoutMs) throws Exception {{
        long start = System.nanoTime();
        while (buf.getInt(0) != STATUS_IDLE) {{
            if ((System.nanoTime() - start) / 1_000_000 > timeoutMs) throw new Exception("timeout");
            Thread.sleep(1);
        }}
        buf.position(PAYLOAD_OFFSET);
        buf.put(payload);
        buf.putInt(8, payload.length);
        buf.putInt(0, STATUS_CONSUMER_REQ);

        long respStart = System.nanoTime();
        while (true) {{
            int status = buf.getInt(0);
            if (status == STATUS_PROVIDER_RES) {{
                int respSize = buf.getInt(8);
                byte[] resp = new byte[respSize];
                buf.position(PAYLOAD_OFFSET);
                buf.get(resp, 0, respSize);
                buf.putInt(0, STATUS_IDLE);
                return resp;
            }}
            if ((System.nanoTime() - respStart) / 1_000_000 > timeoutMs) throw new Exception("timeout");
            Thread.sleep(1);
        }}
    }}
}}
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }

    /// Generate a metropipe C# module (32-byte header via MemoryMappedFile)
    pub fn generate_metropipe_csharp_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;
        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"
// metropipe C# stub for {channel_id}
// 32-byte header — metropipe protocol compatible

using System;
using System.IO.MemoryMappedFiles;
using System.Runtime.InteropServices;
using System.Threading;

class MetroChannel
{{
    const string ShmPath = "/dev/shm/metro_{channel_id}";
    const int PayloadOffset = 32;
    const int Capacity = {max_size};

    const int StatusIdle = 0;
    const int StatusConsumerReq = 1;
    const int StatusProviderRes = 3;

    private MemoryMappedFile mmf;
    private MemoryMappedViewAccessor accessor;

    public MetroChannel()
    {{
        mmf = MemoryMappedFile.CreateFromFile(ShmPath, FileMode.Open);
        accessor = mmf.CreateViewAccessor(0, 32 + Capacity);
    }}

    public byte[] Request(byte[] payload, int timeoutMs)
    {{
        var start = DateTime.Now;
        while (accessor.ReadInt32(0) != StatusIdle)
        {{
            if ((DateTime.Now - start).TotalMilliseconds > timeoutMs)
                throw new TimeoutException();
            Thread.Sleep(1);
        }}
        accessor.WriteArray(PayloadOffset, payload, 0, payload.Length);
        accessor.Write(8, payload.Length);
        accessor.Write(0, StatusConsumerReq);

        var respStart = DateTime.Now;
        while (true)
        {{
            int status = accessor.ReadInt32(0);
            if (status == StatusProviderRes)
            {{
                int respSize = accessor.ReadInt32(8);
                byte[] resp = new byte[respSize];
                accessor.ReadArray(PayloadOffset, resp, 0, respSize);
                accessor.Write(0, StatusIdle);
                return resp;
            }}
            if ((DateTime.Now - respStart).TotalMilliseconds > timeoutMs)
                throw new TimeoutException();
            Thread.Sleep(1);
        }}
    }}

    public void Close()
    {{
        accessor?.Dispose();
        mmf?.Dispose();
    }}
}}
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }

    /// Generate a metropipe Ruby module (32-byte header via IO/mmap)
    pub fn generate_metropipe_ruby_module(&self, channel_id: &str) -> Result<String, String> {
        let channel = self
            .get_channel(channel_id)
            .ok_or_else(|| format!("Channel {} not found", channel_id))?;
        let addrs = channel.get_addresses();
        let max_size = addrs
            .get("input_size")
            .copied()
            .unwrap_or(4096)
            .max(addrs.get("output_size").copied().unwrap_or(4096));

        Ok(format!(
            r#"
# metropipe Ruby stub for {channel_id}
# 32-byte header — metropipe protocol compatible

require 'io/extra'

SHM_PATH = "/dev/shm/metro_{channel_id}"
PAYLOAD_OFFSET = 32
CAPACITY = {max_size}

STATUS_IDLE = 0
STATUS_CONSUMER_REQ = 1
STATUS_PROVIDER_RES = 3

class MetroChannel
  def initialize
    @fd = IO.sysopen(SHM_PATH, File::RDWR)
    @size = File.size(SHM_PATH)
    @buf = IO.mmap(@fd, @size, IO::PROT_READ | IO::PROT_WRITE, IO::MAP_SHARED)
  end

  def request(payload, timeout_ms = 5000)
    start = Time.now
    while @buf[0, 4].unpack1('L') != STATUS_IDLE
      raise 'timeout' if (Time.now - start) * 1000 > timeout_ms
      sleep 0.001
    end
    @buf[PAYLOAD_OFFSET, payload.bytesize] = payload
    @buf[8, 4] = [payload.bytesize].pack('L')
    @buf[0, 4] = [STATUS_CONSUMER_REQ].pack('L')

    resp_start = Time.now
    loop do
      status = @buf[0, 4].unpack1('L')
      if status == STATUS_PROVIDER_RES
        resp_size = @buf[8, 4].unpack1('L')
        resp = @buf[PAYLOAD_OFFSET, resp_size]
        @buf[0, 4] = [STATUS_IDLE].pack('L')
        return resp
      end
      raise 'timeout' if (Time.now - resp_start) * 1000 > timeout_ms
      sleep 0.001
    end
  end

  def close
    IO.munmap(@buf)
    @fd.close
  end
end
"#,
            channel_id = channel_id,
            max_size = max_size
        ))
    }
}

impl Default for MetropolitanHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_region_create_and_write() {
        let region = SharedRegion::new("test", 1024, "rw", "c").unwrap();
        assert_eq!(region.size, 1024);
        assert_eq!(region.id, "test");

        region.write(0, &[1, 2, 3, 4]).unwrap();
        let data = region.read(0, 4).unwrap();
        assert_eq!(data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_status_word_atomic() {
        let region = SharedRegion::new("test_status", 64, "rw", "c").unwrap();

        region.write_status(0, MetroStatus::Ready).unwrap();
        assert_eq!(region.read_status(0).unwrap(), MetroStatus::Ready);

        region.write_status(0, MetroStatus::Processing).unwrap();
        assert_eq!(region.read_status(0).unwrap(), MetroStatus::Processing);

        region.write_status(0, MetroStatus::Complete).unwrap();
        assert_eq!(region.read_status(0).unwrap(), MetroStatus::Complete);

        region.write_status(0, MetroStatus::Error(42)).unwrap();
        assert_eq!(region.read_status(0).unwrap(), MetroStatus::Error(42));
    }

    #[test]
    fn test_atomic_cas() {
        let region = SharedRegion::new("test_cas", 64, "rw", "c").unwrap();
        region.write_status(0, MetroStatus::Ready).unwrap();

        let result = region.atomic_cas(0, 0, 1).unwrap();
        assert_eq!(result, 0); // Returned old value

        let status = region.read_status(0).unwrap();
        assert_eq!(status, MetroStatus::Processing);
    }

    #[test]
    fn test_metropolitan_channel() {
        let hub = MetropolitanHub::new();
        let channel = hub.create_channel("test_channel", "c", 4096, 4096).unwrap();

        assert_eq!(channel.id, "test_channel");
        assert_eq!(channel.input_size, 4096);
        assert_eq!(channel.output_size, 4096);
    }

    #[test]
    fn test_channel_send_receive() {
        let hub = MetropolitanHub::new();
        let channel = hub
            .create_channel("test_send_recv", "c", 4096, 4096)
            .unwrap();

        // Send data
        let input = vec![1, 2, 3, 4, 5];
        channel.send(&input).unwrap();

        // Verify data was written to request region
        let request_data = channel.request_region.read(0, 5).unwrap();
        assert_eq!(request_data, input);

        // Verify status was set to Processing
        let status = channel.sync_region.read_status(0).unwrap();
        assert_eq!(status, MetroStatus::Processing);

        // Simulate foreign side processing
        channel.response_region.write(0, &[10, 20, 30]).unwrap();
        channel
            .sync_region
            .write_status(8, MetroStatus::Complete)
            .unwrap();

        // Receive response
        let response = channel.receive(1000).unwrap();
        assert_eq!(&response[..3], &[10, 20, 30]);
    }

    #[test]
    fn test_channel_timeout() {
        let hub = MetropolitanHub::new();
        let channel = hub.create_channel("test_timeout", "c", 4096, 4096).unwrap();

        // Send without foreign side responding
        channel.send(&[1, 2, 3]).unwrap();

        // Should timeout
        let result = channel.receive(10); // 10ms timeout
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Timeout"));
    }

    #[test]
    fn test_generate_c_header() {
        let hub = MetropolitanHub::new();
        hub.create_channel("test_c", "c", 1024, 1024).unwrap();

        let header = hub.generate_c_header("test_c").unwrap();
        assert!(header.contains("REQUEST_BASE"));
        assert!(header.contains("RESPONSE_BASE"));
        assert!(header.contains("SYNC_BASE"));
        assert!(header.contains("STATUS_READY"));
    }

    #[test]
    fn test_generate_rust_module() {
        let hub = MetropolitanHub::new();
        hub.create_channel("test_rust", "rust", 2048, 2048).unwrap();

        let module = hub.generate_rust_module("test_rust").unwrap();
        assert!(module.contains("REQUEST_BASE"));
        assert!(module.contains("wait_for_request"));
        assert!(module.contains("signal_complete"));
    }

    #[test]
    fn test_generate_python_module() {
        let hub = MetropolitanHub::new();
        hub.create_channel("test_python", "python", 512, 512)
            .unwrap();

        let module = hub.generate_python_module("test_python").unwrap();
        assert!(module.contains("REQUEST_BASE"));
        assert!(module.contains("wait_for_request"));
        assert!(module.contains("signal_complete"));
    }
}
