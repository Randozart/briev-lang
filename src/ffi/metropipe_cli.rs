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

//! Metro CLI - `briev metropipe connect` command implementation
//!
//! Connects to metropipe shared memory channels at `/dev/shm/metro_{service_name}`.
//! Supports interactive REPL, one-shot RPC, and stub generation modes.

use crate::ffi::metropipe::MetropolitanHub;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

// Metro protocol header layout (32 bytes) matching metropipe/clients/c/metropipe.h
/// Status IDLE — channel is ready for a request
const STATUS_IDLE: u32 = 0;
/// Consumer has written a request payload
const STATUS_CONSUMER_REQ: u32 = 1;
/// Provider has acknowledged the request
const STATUS_PROVIDER_ACK: u32 = 2;
/// Provider has written a response payload
const STATUS_PROVIDER_RES: u32 = 3;
/// An error occurred on the provider side
const STATUS_ERR: u32 = 4;

/// Offset of the status field within the header (4 bytes)
const STATUS_OFFSET: isize = 0;
/// Offset of the CAS lock field (4 bytes)
const CAS_LOCK_OFFSET: isize = 1;
/// Offset of the payload size field (4 bytes)
const PAYLOAD_SIZE_OFFSET: isize = 2;
/// Offset of the max capacity field (4 bytes)
const MAX_CAPACITY_OFFSET: isize = 3;
/// Offset of the error code field (4 bytes)
const ERROR_CODE_OFFSET: isize = 4;
/// Offset of payload data from the start of the header (32 bytes = 8 u32s)
const PAYLOAD_U32_OFFSET: isize = 8;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Intent: handle SIGINT by setting the interrupted flag
extern "C" fn handle_sigint(_: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Metro CLI connection to a Metropolitan shared memory channel.
///
/// Manages a file-backed mmap of `/dev/shm/metro_{service_name}` using the
/// 32-byte header protocol from the metrod specification.
pub struct MetroCli {
    /// Name of the service channel
    pub service_name: String,
    /// Path to the shared memory file (`/dev/shm/metro_{service_name}`)
    pub shm_path: String,
    /// Pointer to the 32-byte header (8 × u32) at the start of the mapped region
    pub header: *mut u32,
    /// Size of the last sent/received payload
    pub payload_size: usize,
    /// Maximum payload capacity in bytes
    pub capacity: usize,
}

unsafe impl Send for MetroCli {}
unsafe impl Sync for MetroCli {}

impl MetroCli {
    /// Create a new MetroCli for the given service name.
    ///
    /// Does not open the channel — call `open()` to connect.
    pub fn new(service_name: &str) -> Self {
        MetroCli {
            service_name: service_name.to_string(),
            shm_path: format!("/dev/shm/metro_{}", service_name),
            header: std::ptr::null_mut(),
            payload_size: 0,
            capacity: 0,
        }
    }

    /// Open the shared memory channel at `/dev/shm/metro_{service_name}`.
    ///
    /// Maps the file with read-write shared access and reads the header to
    /// determine the channel capacity. Returns a helpful error if the file
    /// does not exist.
    pub fn open(&mut self) -> Result<(), String> {
        let path = &self.shm_path;

        if !Path::new(path).exists() {
            return Err(format!(
                "Shared memory file not found: {}\n\
                 Create the channel first by running the provider that sets up {}",
                path, path
            ));
        }

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("Cannot open {}: {}", path, e))?;

        let file_size = file
            .metadata()
            .map_err(|e| format!("Cannot stat {}: {}", path, e))?
            .len() as usize;

        if file_size < 32 {
            return Err(format!(
                "{} is too small ({} bytes) — expected at least 32 bytes for header",
                path, file_size
            ));
        }

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;

            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    file_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    0,
                )
            };

            if ptr == libc::MAP_FAILED {
                return Err(format!(
                    "mmap failed for {}: {}",
                    path,
                    io::Error::last_os_error()
                ));
            }

            self.header = ptr as *mut u32;
        }

        // Read max capacity from the header at u32 offset 3 (= byte 12)
        self.capacity = unsafe {
            let cap_ptr = self.header.add(MAX_CAPACITY_OFFSET as usize);
            std::ptr::read_volatile(cap_ptr) as usize
        };

        if self.capacity == 0 {
            // Fallback: capacity = file_size - header_size
            self.capacity = file_size - 32;
        }

        Ok(())
    }

    /// Send a payload to the channel.
    ///
    /// Acquires the CAS lock, writes the payload data and size, sets the status
    /// to `CONSUMER_REQ`, and releases the lock.
    pub fn send(&mut self, data: &[u8]) -> Result<(), String> {
        if self.header.is_null() {
            return Err("Channel not opened".to_string());
        }
        if data.len() > self.capacity {
            return Err(format!(
                "Payload size {} exceeds channel capacity {}",
                data.len(),
                self.capacity
            ));
        }

        // Acquire CAS lock
        let lock_ptr = unsafe { self.header.add(CAS_LOCK_OFFSET as usize) as *mut AtomicU32 };
        loop {
            let prev =
                unsafe { (*lock_ptr).compare_exchange(0, 1, Ordering::SeqCst, Ordering::Relaxed) };
            if prev.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_micros(100));
        }

        // Write payload data at byte offset 32 (u32 offset 8)
        let payload_ptr = unsafe { self.header.add(PAYLOAD_U32_OFFSET as usize) as *mut u8 };
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), payload_ptr, data.len());
        }

        // Write payload size at u32 offset 2 (= byte 8)
        let size_ptr = unsafe { self.header.add(PAYLOAD_SIZE_OFFSET as usize) as *mut u32 };
        unsafe {
            std::ptr::write_volatile(size_ptr, data.len() as u32);
        }
        self.payload_size = data.len();

        // Memory barrier before setting status
        std::sync::atomic::fence(Ordering::SeqCst);

        // Set status to CONSUMER_REQ
        let status_ptr = unsafe { self.header.add(STATUS_OFFSET as usize) as *mut u32 };
        unsafe {
            std::ptr::write_volatile(status_ptr, STATUS_CONSUMER_REQ);
        }

        // Release CAS lock
        unsafe {
            (*lock_ptr).store(0, Ordering::SeqCst);
        }

        Ok(())
    }

    /// Receive a response from the channel with a timeout.
    ///
    /// Polls for `PROVIDER_RES` status, reads the response payload, resets
    /// the status to `IDLE`, and returns the data.
    pub fn receive(&self, timeout_ms: u64) -> Result<Vec<u8>, String> {
        if self.header.is_null() {
            return Err("Channel not opened".to_string());
        }

        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        let status_ptr = unsafe { self.header.add(STATUS_OFFSET as usize) as *const u32 };

        loop {
            if start.elapsed() >= timeout {
                return Err(format!(
                    "Timeout waiting for response from {} ({}ms)",
                    self.shm_path, timeout_ms
                ));
            }

            let status = unsafe { std::ptr::read_volatile(status_ptr) };

            match status {
                STATUS_PROVIDER_RES => {
                    // Memory barrier before reading response
                    std::sync::atomic::fence(Ordering::SeqCst);

                    // Read payload size
                    let size_ptr =
                        unsafe { self.header.add(PAYLOAD_SIZE_OFFSET as usize) as *const u32 };
                    let resp_size = unsafe { std::ptr::read_volatile(size_ptr) } as usize;
                    let resp_size = std::cmp::min(resp_size, self.capacity);

                    // Read payload data
                    let payload_ptr =
                        unsafe { self.header.add(PAYLOAD_U32_OFFSET as usize) as *mut u8 };
                    let mut buf = vec![0u8; resp_size];
                    unsafe {
                        std::ptr::copy_nonoverlapping(payload_ptr, buf.as_mut_ptr(), resp_size);
                    }

                    // Reset status to IDLE
                    let status_mut = unsafe { self.header.add(STATUS_OFFSET as usize) as *mut u32 };
                    unsafe {
                        std::ptr::write_volatile(status_mut, STATUS_IDLE);
                    }

                    return Ok(buf);
                }
                STATUS_ERR => {
                    // Read error code at u32 offset 4 (= byte 16)
                    let err_ptr =
                        unsafe { self.header.add(ERROR_CODE_OFFSET as usize) as *const u32 };
                    let err_code = unsafe { std::ptr::read_volatile(err_ptr) };

                    // Reset status to IDLE
                    let status_mut = unsafe { self.header.add(STATUS_OFFSET as usize) as *mut u32 };
                    unsafe {
                        std::ptr::write_volatile(status_mut, STATUS_IDLE);
                    }

                    return Err(format!("Provider error code: {}", err_code));
                }
                _ => {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    /// Send a request and receive the response.
    ///
    /// Convenience wrapper around `send` + `receive`.
    pub fn request(&mut self, data: &[u8], timeout_ms: u64) -> Result<Vec<u8>, String> {
        self.send(data)?;
        self.receive(timeout_ms)
    }

    /// Close the channel and unmap the shared memory.
    pub fn close(&self) {
        if !self.header.is_null() {
            #[cfg(unix)]
            unsafe {
                let file_size = self.capacity + 32;
                libc::munmap(self.header as *mut _, file_size);
            }
        }
    }

    /// Run an interactive REPL on the channel.
    ///
    /// Reads lines from stdin, sends each as a request, and prints the
    /// response. Press Ctrl+C to exit.
    pub fn interactive_repl(&mut self) {
        if self.header.is_null() {
            eprintln!("Error: Channel not opened. Call open() first.");
            return;
        }

        println!(
            "Connected to {} (capacity: {} bytes)",
            self.shm_path, self.capacity
        );
        println!("Type a payload and press Enter to send. Press Ctrl+C to exit.");

        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGINT, handle_sigint as *const () as usize);
        }

        INTERRUPTED.store(false, Ordering::SeqCst);

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        while !INTERRUPTED.load(Ordering::SeqCst) {
            print!("> ");
            if stdout.flush().is_err() {
                break;
            }

            let mut line = String::new();
            match stdin.lock().read_line(&mut line) {
                Ok(n) if n > 0 => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    match self.request(line.as_bytes(), 5000) {
                        Ok(response) => match std::str::from_utf8(&response) {
                            Ok(text) => println!("Response ({} bytes): {}", response.len(), text),
                            Err(_) => {
                                println!("Response ({} bytes): {:02x?}", response.len(), response)
                            }
                        },
                        Err(e) => {
                            eprintln!("Error: {}", e);
                        }
                    }
                }
                Ok(_) => break,  // EOF
                Err(_) => break, // Interrupted or error
            }
        }

        println!("Channel closed.");
    }

    /// Generate client stub files for the channel.
    ///
    /// Creates C header, Python module, Rust module, and JavaScript module
    /// files in the specified output directory. Uses `MetropolitanHub` for
    /// code generation. Defaults to 4096-byte capacity if not available.
    pub fn generate_stubs(&self, output_dir: &str) {
        let hub = MetropolitanHub::new();
        let channel_id = &self.service_name;
        let cap = if self.capacity > 0 {
            self.capacity
        } else {
            4096
        };

        // Create the channel metadata so the hub can generate stubs
        if let Err(e) = hub.create_channel(channel_id, "c", cap, cap) {
            eprintln!("Warning: Could not register channel metadata: {}", e);
        }

        let out_path = Path::new(output_dir);
        if !out_path.exists() {
            if let Err(e) = fs::create_dir_all(out_path) {
                eprintln!(
                    "Error: Cannot create output directory {}: {}",
                    output_dir, e
                );
                return;
            }
        }

        // Generate C header
        match hub.generate_c_header(channel_id) {
            Ok(header) => {
                let c_path = out_path.join(format!("{}_metro.h", channel_id));
                if let Err(e) = fs::write(&c_path, &header) {
                    eprintln!("Error writing C header: {}", e);
                } else {
                    println!("  Generated: {}", c_path.display());
                }
            }
            Err(e) => {
                eprintln!("Error generating C header: {}", e);
            }
        }

        // Generate Python module
        match hub.generate_python_module(channel_id) {
            Ok(py) => {
                let py_path = out_path.join(format!("{}_metro.py", channel_id));
                if let Err(e) = fs::write(&py_path, &py) {
                    eprintln!("Error writing Python module: {}", e);
                } else {
                    println!("  Generated: {}", py_path.display());
                }
            }
            Err(e) => {
                eprintln!("Error generating Python module: {}", e);
            }
        }

        // Generate Rust module
        match hub.generate_rust_module(channel_id) {
            Ok(rs) => {
                let rs_path = out_path.join(format!("{}_metro.rs", channel_id));
                if let Err(e) = fs::write(&rs_path, &rs) {
                    eprintln!("Error writing Rust module: {}", e);
                } else {
                    println!("  Generated: {}", rs_path.display());
                }
            }
            Err(e) => {
                eprintln!("Error generating Rust module: {}", e);
            }
        }

        // Generate JavaScript stub module
        let js = self.generate_js_stub(cap);
        let js_path = out_path.join(format!("{}_metro.js", channel_id));
        if let Err(e) = fs::write(&js_path, &js) {
            eprintln!("Error writing JavaScript stub: {}", e);
        } else {
            println!("  Generated: {}", js_path.display());
        }
    }

    /// Intent: generate a JavaScript stub module
    fn generate_js_stub(&self, capacity: usize) -> String {
        format!(
            r#"// Metropolitan FFI - JS Stub for channel: {}
// Generated by Briev Compiler
// DO NOT EDIT MANUALLY

const METRO_SERVICE = "{}";
const SHM_PATH = "/dev/shm/metro_" + METRO_SERVICE;

// Status values matching metro.h
const STATUS_IDLE = 0;
const STATUS_CONSUMER_REQ = 1;
const STATUS_PROVIDER_ACK = 2;
const STATUS_PROVIDER_RES = 3;
const STATUS_ERR = 4;

// Header offsets (bytes)
const HEADER_SIZE = 32;
const STATUS_OFFSET = 0;
const CAS_LOCK_OFFSET = 4;
const PAYLOAD_SIZE_OFFSET = 8;
const CAPACITY = {};
const ERROR_CODE_OFFSET = 16;

// Note: JavaScript cannot directly mmap /dev/shm files in Node.js
// without native bindings. Use this module as a reference for
// implementing a Node native addon or use the C/Python stubs.
//
// For Node.js, use:
//   const fs = require('fs');
//   const buf = fs.readFileSync(SHM_PATH); // snapshot only

export const CAPACITY = CAPACITY;
export const SERVICE_NAME = "{}";
export default {{ CAPACITY, SERVICE_NAME }};
"#,
            self.service_name, self.service_name, capacity, self.service_name
        )
    }
}

impl Drop for MetroCli {
    fn drop(&mut self) {
        self.close();
    }
}

/// Run the metropipe connect CLI.
///
/// The caller passes the remaining arguments after `metropipe connect`.
/// Supports interactive REPL (default), one-shot RPC (`--send <payload>`),
/// and stub generation (`--gen-stub <output_dir>` or `--gen-stub`).
pub fn run_metro_cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        eprintln!("Error: Missing service name");
        eprintln!(
            "Usage: briev metropipe connect <service_name> [--send <payload>] [--gen-stub [<dir>]]"
        );
        return Ok(());
    }

    let service_name = &args[0];
    let mut metro = MetroCli::new(service_name);

    // Check for --gen-stub flag
    let gen_stub_idx = args.iter().position(|a| a == "--gen-stub");
    if let Some(idx) = gen_stub_idx {
        let output_dir = args.get(idx + 1).map(|s| s.as_str()).unwrap_or(".");
        metro.capacity = 4096; // Default capacity for stub generation
        println!(
            "Generating stubs for service '{}' in '{}'",
            service_name, output_dir
        );
        metro.generate_stubs(output_dir);
        return Ok(());
    }

    // Check for --send flag (one-shot RPC)
    let send_idx = args.iter().position(|a| a == "--send");
    if let Some(idx) = send_idx {
        let payload = args.get(idx + 1).ok_or("Missing payload after --send")?;
        metro.open()?;
        println!("Sending to {}...", metro.shm_path);
        match metro.request(payload.as_bytes(), 5000) {
            Ok(response) => match std::str::from_utf8(&response) {
                Ok(text) => println!("Response ({} bytes): {}", response.len(), text),
                Err(_) => println!("Response ({} bytes): {:02x?}", response.len(), response),
            },
            Err(e) => {
                eprintln!("RPC failed: {}", e);
            }
        }
        metro.close();
        return Ok(());
    }

    // Default: interactive REPL
    metro.open()?;
    metro.interactive_repl();
    metro.close();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metro_cli_new() {
        let cli = MetroCli::new("WeatherApi");
        assert_eq!(cli.service_name, "WeatherApi");
        assert_eq!(cli.shm_path, "/dev/shm/metro_WeatherApi");
        assert!(cli.header.is_null());
        assert_eq!(cli.capacity, 0);
    }

    #[test]
    fn test_open_missing_file() {
        let mut cli = MetroCli::new("nonexistent_service_xyz");
        let result = cli.open();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("not found") || err.contains("nonexistent"));
    }

    #[test]
    fn test_send_without_open() {
        let mut cli = MetroCli::new("test");
        let result = cli.send(b"hello");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not opened"));
    }

    #[test]
    fn test_receive_without_open() {
        let cli = MetroCli::new("test");
        let result = cli.receive(100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not opened"));
    }

    #[test]
    fn test_generate_stubs_via_hub() {
        let hub = MetropolitanHub::new();
        let result = hub.create_channel("test_stubs", "c", 4096, 4096);
        assert!(result.is_ok());

        let c_header = hub.generate_c_header("test_stubs");
        assert!(c_header.is_ok());
        assert!(c_header.unwrap().contains("REQUEST_BASE"));

        let py_module = hub.generate_python_module("test_stubs");
        assert!(py_module.is_ok());
        assert!(py_module.unwrap().contains("STATUS_READY"));

        let rs_module = hub.generate_rust_module("test_stubs");
        assert!(rs_module.is_ok());
        assert!(rs_module.unwrap().contains("wait_for_request"));
    }

    #[test]
    fn test_generate_stubs_creates_files() {
        let cli = MetroCli::new("test_gen");

        // Use a temporary directory for stub output
        let tmp_dir = std::env::temp_dir().join(format!("metro_stubs_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);

        cli.generate_stubs(tmp_dir.to_str().unwrap());

        assert!(tmp_dir.join("test_gen_metro.h").exists());
        assert!(tmp_dir.join("test_gen_metro.py").exists());
        assert!(tmp_dir.join("test_gen_metro.rs").exists());
        assert!(tmp_dir.join("test_gen_metro.js").exists());

        // Cleanup
        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_generate_js_stub() {
        let cli = MetroCli::new("TestApi");
        let js = cli.generate_js_stub(4096);
        assert!(js.contains("TestApi"));
        assert!(js.contains("CAPACITY"));
        assert!(js.contains("STATUS_IDLE"));
    }

    #[test]
    fn test_drop_null_pointer() {
        // Dropping a MetroCli with null header should not crash
        let cli = MetroCli::new("test_drop");
        drop(cli);
    }

    #[test]
    fn test_run_metro_cli_gen_stub() {
        let tmp_dir = std::env::temp_dir().join(format!("metro_cli_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);

        let args = vec![
            "StubSvc".to_string(),
            "--gen-stub".to_string(),
            tmp_dir.to_str().unwrap().to_string(),
        ];

        let result = run_metro_cli(&args);
        assert!(result.is_ok());

        assert!(tmp_dir.join("StubSvc_metro.h").exists());
        assert!(tmp_dir.join("StubSvc_metro.js").exists());

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
