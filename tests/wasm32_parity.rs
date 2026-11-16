//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
//! wasm32 / native cross-backend parity gate.
//!
//! Compiles each fixture in `tests/fixtures/wasm_parity/` twice — once for the
//! host and once with `--backend webstack` — runs both, and requires the same
//! observable outcome. The observable is the process exit status, i.e. the
//! `briev_syscall(231, code)` sink, which the amended Observability-as-Liveness
//! doctrine classes as an FFI observable.
//!
//! # Scope, stated honestly
//!
//! * Only fixtures that end via `endprogram` are comparable, and only when they
//!   do not print: the webstack print path validates string bitcasts and needs
//!   the browser/DOM host (flush contract + `stdout_buf`), which a minimal Node
//!   host does not provide. Stdout parity is a separate piece of work.
//! * Fixtures are classified by shape (a `txn_*` export must exist); anything
//!   else is reported as skipped with the reason, never silently dropped.
//!
//! # Host contract the harness must satisfy
//!
//! Three details were each a wrong diagnosis before they were written down:
//!
//! 1. `briev_syscall` is declared with **seven i64 parameters** and returns
//!    i64. A JS import for an i64 result must return a BigInt — returning `0`
//!    throws "Cannot convert 0 to a BigInt" *inside* the wasm frame.
//! 2. `malloc` must hand out addresses above the module's initial memory
//!    (`memory.buffer.byteLength`) and grow as needed. A bump allocator that
//!    starts low overwrites the module's globals — including `@__web_state` —
//!    and the reactor then traps in its tick guard.
//! 3. A `RuntimeError: unreachable` **after** a recorded `briev_syscall(231,
//!    …)` is success: the emitter marks the call noreturn. An `unreachable`
//!    with no recorded syscall is a real failure and must be reported.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The Node harness. Kept in one place so the host contract above has a single
/// implementation; `run_wasm` writes it next to the build outputs.
const NODE_HARNESS: &str = r#"
import { readFileSync } from 'node:fs';
const toNum = (v) => (typeof v === 'bigint' ? Number(v) : v);
const buf = readFileSync(process.argv[2]);

let memory;
let frontier = 0;
function malloc(n) {
  const size = Math.max(16, Math.ceil(Number(n) / 16) * 16);
  if (!frontier) frontier = memory.buffer.byteLength;
  const end = frontier + size;
  const pages = Math.ceil((end - memory.buffer.byteLength) / 65536);
  if (pages > 0) memory.grow(pages);
  const pointer = frontier;
  frontier = end;
  return pointer;
}

const observed = { exited: false, code: 0, flushes: 0 };
const env = {
  malloc,
  free: () => {},
  __web_flush_state: () => { observed.flushes += 1; },
  briev_bits_to_str: () => 0,
  briev_syscall: (number, arg1) => {
    if (toNum(number) === 231) {
      observed.code = toNum(arg1) & 0xff;
      observed.exited = true;
    }
    return 0n;
  },
};

// Instantiation is where a missing/renamed host import shows up. Without this
// guard Node exits 1 and the Rust side reports "wasm exit 1", which reads like
// the program returned 1 instead of "the harness could not load the module" —
// a wrong diagnosis that costs a debugging session every time.
let instance;
try {
  ({ instance } = await WebAssembly.instantiate(buf, { env }));
} catch (error) {
  console.error('harness: instantiate failed: ' + String(error && error.message));
  process.exit(121);
}
memory = instance.exports.memory;
const exports = instance.exports;
if (typeof exports.__web_boot === 'function') exports.__web_boot();

const txn = Object.keys(exports).find((name) => name.startsWith('txn_'));
if (!txn) { console.error('no txn_* export'); process.exit(125); }
if (typeof exports.__briev_state_ptr !== 'function') {
  console.error('no __briev_state_ptr'); process.exit(124);
}
const state = toNum(exports.__briev_state_ptr());

for (let turn = 0; turn < 200000 && !observed.exited; turn += 1) {
  try {
    exports[txn](state);
  } catch (error) {
    if (/unreachable/.test(String(error.message))) {
      // The noreturn marker after exit_prog traps too; only a trap with no
      // recorded exit is a failure.
      if (!observed.exited) {
        console.error('trap before endprogram:', String(error.stack).split('\n')[1] || '');
        process.exit(120);
      }
      break;
    }
    throw error;
  }
}
process.exit(observed.exited ? observed.code : 127);
"#;

/// Resolve the compiler binary under test.
fn brievc() -> PathBuf {
    if let Ok(explicit) = std::env::var("BRIEVC") {
        return PathBuf::from(explicit);
    }
    for candidate in ["target/debug/brievc", "target/release/brievc"] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return path;
        }
    }
    panic!("no brievc binary found; build it first or set BRIEVC");
}

/// Whether the wasm toolchain and Node are present.
fn wasm_available() -> bool {
    let node = Command::new("node").arg("--version").output().is_ok();
    let llc = Command::new("llc").arg("--version").output().is_ok();
    let linker = Command::new("wasm-ld").arg("--version").output().is_ok();
    node && llc && linker
}

/// Compile one fixture and return the produced binary path.
fn build(fixture: &Path, out_dir: &Path, backend: Option<&str>) -> PathBuf {
    let mut command = Command::new(brievc());
    command.arg("build").arg(fixture).arg("--out").arg(out_dir);
    if let Some(backend) = backend {
        command.arg("--backend").arg(backend);
    }
    let output = command.output().expect("failed to spawn brievc");
    assert!(
        output.status.success(),
        "build failed for {} (backend {:?}):\n{}{}",
        fixture.display(),
        backend,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stem = fixture.file_stem().expect("fixture name");
    let extension = if backend.is_some() { "wasm" } else { "" };
    out_dir.join(format!(
        "{}{}{}",
        stem.to_string_lossy(),
        if extension.is_empty() { "" } else { "." },
        extension
    ))
}

/// Run a native binary and return its exit status code.
fn run_native(binary: &Path) -> i32 {
    let status = Command::new(binary)
        .output()
        .expect("failed to run native binary")
        .status;
    status.code().unwrap_or(-1)
}

/// Run a wasm module through the Node harness and return its exit code.
///
/// The fixture is silent by contract, so *any* stderr output means the harness
/// itself failed rather than the program producing a bad status. Reporting that
/// distinctly matters: the opaque "wasm exit 1" from an uncaught JS exception
/// looks exactly like a genuine cross-backend divergence.
fn run_wasm(harness: &Path, wasm: &Path) -> i32 {
    let output = Command::new("node")
        .arg(harness)
        .arg(wasm)
        .output()
        .expect("failed to spawn node");
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if matches!(code, 120 | 121 | 124 | 125) || !stderr.trim().is_empty() {
        panic!(
            "wasm harness could not run {}: exit {code}\n{}",
            wasm.display(),
            stderr
        );
    }
    code
}

#[test]
fn wasm32_and_native_agree_on_the_exit_status() {
    if !wasm_available() {
        eprintln!("wasm32 parity: skipped (node, llc or wasm-ld unavailable)");
        return;
    }
    let fixtures_dir = Path::new("tests/fixtures/wasm_parity");
    let entries = std::fs::read_dir(fixtures_dir).expect("fixture directory missing");
    let mut fixtures: Vec<PathBuf> = entries
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "bv"))
        .collect();
    fixtures.sort();
    assert!(
        !fixtures.is_empty(),
        "no parity fixtures under {}",
        fixtures_dir.display()
    );

    let scratch = std::env::temp_dir().join(format!("briev-wasm-parity-{}", std::process::id()));
    let native_dir = scratch.join("native");
    let wasm_dir = scratch.join("wasm");
    std::fs::create_dir_all(&native_dir).expect("create native dir");
    std::fs::create_dir_all(&wasm_dir).expect("create wasm dir");
    let harness = scratch.join("harness.mjs");
    std::fs::write(&harness, NODE_HARNESS).expect("write harness");

    let mut checked = 0;
    let mut divergences = Vec::new();
    for fixture in &fixtures {
        let name = fixture
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .to_string();
        let native = build(fixture, &native_dir, None);
        let wasm = build(fixture, &wasm_dir, Some("webstack"));
        let expected = run_native(&native);
        let actual = run_wasm(&harness, &wasm);
        checked += 1;
        if expected != actual {
            divergences.push(format!(
                "{name}: native exit {expected} != wasm exit {actual}"
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(
        divergences.is_empty(),
        "wasm32/native divergence in {} of {checked} fixture(s):\n{}",
        divergences.len(),
        divergences.join("\n")
    );
}
