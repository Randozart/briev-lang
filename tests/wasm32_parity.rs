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

// 2026-11-16 (native-sink parity leg): the tick loop tracks the last
// successful-tick state. Two terminal shapes:
//   * Terminating: a briev_syscall(231, code) records the exit; the
//     subsequent noreturn `unreachable` is expected (the emitter marks
//     exit_prog noreturn). Report the recorded code.
//   * Equilibrium: no endprogram. Once the guard is permanently false the
//     reactor traps on the next tick. That post-settle trap is the
//     equilibrium signal — report the last successful-tick state (the fixed
//     point) with exit = -1. A trap on the FIRST tick (no state ever settled)
//     is a real failure (120).
let settled = false;
let sawTrap = false;
for (let turn = 0; turn < 200000 && !observed.exited; turn += 1) {
  try {
    exports[txn](state);
    settled = true;
  } catch (error) {
    if (/unreachable/.test(String(error.message))) {
      sawTrap = true;
      if (observed.exited) { break; }          // noreturn after exit_prog
      if (settled) { break; }                  // equilibrium: settled, then trap
      // No successful tick before the trap — a genuine guard-fail.
      console.error('trap before any settle:', String(error.stack).split('\n')[1] || '');
      process.exit(120);
    }
    throw error;
  }
}
if (!observed.exited && !settled && !sawTrap) {
  // Ran the full tick cap without settling or trapping: the reactor never
  // reached a terminal state. That is not a clean equilibrium — report it.
  console.error('no settle and no trap within the tick cap');
  process.exit(126);
}
// 2026-11-16 (native-sink parity leg): dump the final state slots so the
// gate can compare the host-readable %State against the fixture's
// documented fixed point. state_layout() returns a pointer to:
//   +0: field_count u32, +4: generation_off, +8: flush_off,
//   +12: max_entries, then field_count rows of {handle, off, size, tag}
// (all u32, little-endian). A slot's value lives at
// `__briev_state_ptr() + off`. Terminating fixtures read state at exit;
// equilibrium fixtures read it after the tick cap (the reactor has
// settled, so cap == equilibrium for a finite state space).
const report = { exit: observed.exited ? observed.code : -1, state: [] };
if (typeof exports.state_layout === 'function') {
  const dv = new DataView(memory.buffer);
  const layout = toNum(exports.state_layout());
  const count = dv.getUint32(layout, true);
  for (let i = 0; i < count; i += 1) {
    const row = layout + 16 + i * 16;
    const off = dv.getUint32(row + 4, true);
    const size = dv.getUint32(row + 8, true);
    report.state.push(
      size === 4 ? dv.getInt32(state + off, true) : Number(dv.getBigInt64(state + off, true))
    );
  }
}
console.log(JSON.stringify(report));
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

/// Per-fixture metadata, optional. A `<stem>.json` beside the `.bv`
/// classifies the fixture's shape and states the expected final state slots.
///
/// * `equilibrium: true` — the program settles instead of ending (no
///   `endprogram` reachable). The native side is documented-skipped: a native
///   process with no terminating path idles and its final state is not
///   host-readable, so only the wasm-side native sink (host-read %State)
///   is checked.
/// * `state` — expected final slot values in `state_layout` table order.
///   Checked against the wasm run's dump. For terminating fixtures the exit
///   code already cross-anchors state (it is derived from it), so this adds
///   the host-read observable on top; for equilibrium fixtures it is the
///   only observable.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct FixtureMeta {
    #[serde(default)]
    equilibrium: bool,
    #[serde(default)]
    state: Option<Vec<i64>>,
}

/// Load the optional `<stem>.json` metadata for a fixture (None means
/// exit-code comparison only — the historical gate behaviour).
fn load_meta(fixtures_dir: &Path, stem: &str) -> Option<FixtureMeta> {
    let path = fixtures_dir.join(format!("{}.json", stem));
    if !path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&path).expect("read fixture metadata");
    let parsed: FixtureMeta = serde_json::from_str(&raw).unwrap_or_else(|error| {
        panic!(
            "bad metadata in {} (expected {{\"equilibrium\": bool, \"state\":[i64]}}): {error}",
            path.display()
        )
    });
    Some(parsed)
}

/// The harness's end-of-run report: the recorded `endprogram` exit code (-1
/// when none fired) and the final state slots in `state_layout` order.
#[derive(Debug, Clone, serde::Deserialize)]
struct WasmReport {
    exit: i32,
    state: Vec<i64>,
}

/// Parse the harness's JSON report (last non-empty stdout line) into
/// (exit, state). A malformed report is a harness fault, not a divergence —
/// panic loudly so the gate never silently passes on garbage.
fn parse_wasm_report(stdout: &str) -> WasmReport {
    let line = stdout.lines().rev().find(|l| !l.trim().is_empty());
    serde_json::from_str(line.unwrap_or("{}")).expect("harness emitted no JSON report")
}

/// Run a wasm module through the Node harness; return its recorded exit
/// code and final state slots.
///
/// The fixture is silent by contract, so *any* stderr output means the harness
/// itself failed rather than the program producing a bad status. Reporting that
/// distinctly matters: the opaque "wasm exit 1" from an uncaught JS exception
/// looks exactly like a genuine cross-backend divergence.
fn run_wasm(harness: &Path, wasm: &Path) -> (i32, Vec<i64>) {
    let output = Command::new("node")
        .arg(harness)
        .arg(wasm)
        .output()
        .expect("failed to spawn node");
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if matches!(code, 120 | 121 | 124 | 125 | 126) || !stderr.trim().is_empty() {
        panic!(
            "wasm harness could not run {}: exit {code}\n{}",
            wasm.display(),
            stderr
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report = parse_wasm_report(&stdout);
    // The harness process exits with the recorded code (or 127 when none
    // fired), so disagreement means a garbled report — never trust the
    // process code over the report (or vice versa) silently.
    let expected_process = if report.exit >= 0 { report.exit } else { 127 };
    assert_eq!(
        code, expected_process,
        "harness process exit {code} disagrees with reported exit {}",
        report.exit
    );
    (report.exit, report.state)
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
    let mut state_checked = 0;
    let mut equilibrium = 0;
    let mut notes = Vec::new();
    for fixture in &fixtures {
        let name = fixture
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .to_string();
        let meta = load_meta(fixtures_dir, &name);
        let wasm = build(fixture, &wasm_dir, Some("webstack"));
        let (wasm_exit, wasm_state) = run_wasm(&harness, &wasm);
        checked += 1;
        let equilibrium_meta = meta.as_ref().filter(|m| m.equilibrium);
        if equilibrium_meta.is_some() {
            // Equilibrium shape: no endprogram, so the native side is
            // documented-skipped (a native process with no terminating path
            // idles and its final state is not host-readable). The only
            // observable is the host-read state sink, declared in metadata.
            equilibrium += 1;
            notes.push(format!(
                "{name}: equilibrium shape — native side skipped by design (no terminating path); state sink checked"
            ));
            let Some(meta) = meta.as_ref() else {
                unreachable!("equilibrium branch only taken when meta exists");
            };
            let Some(expected_state) = meta.state.as_ref() else {
                panic!(
                    "{name}: equilibrium fixture must declare its expected final state in {name}.json"
                );
            };
            assert!(
                state_slots_match(expected_state, &wasm_state),
                "{name}: equilibrium state diverges: expected {expected_state:?}, wasm {wasm_state:?}"
            );
            state_checked += 1;
            continue;
        }
        // Terminating shape: the exit code is the cross-backend observable.
        let native = build(fixture, &native_dir, None);
        let native_exit = run_native(&native);
        if native_exit != wasm_exit {
            divergences.push(format!(
                "{name}: native exit {native_exit} != wasm exit {wasm_exit}"
            ));
        }
        if let Some(expected_state) = meta.and_then(|m| m.state) {
            assert!(
                state_slots_match(&expected_state, &wasm_state),
                "{name}: final state diverges: expected {expected_state:?}, wasm {wasm_state:?}"
            );
            state_checked += 1;
        }
    }
    for note in &notes {
        eprintln!("note: {note}");
    }
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(
        divergences.is_empty(),
        "wasm32/native divergence in {} of {} fixture(s) ({} with state sink, {} equilibrium):\n{}",
        divergences.len(),
        checked,
        state_checked,
        equilibrium,
        divergences.join("\n")
    );
}

/// Slot values are declared in `state_layout` table order and compared
/// positionally — the table rows are ordered by field handle, which both
/// the emitter and the harness read in the same order.
fn state_slots_match(expected: &[i64], actual: &[i64]) -> bool {
    expected.len() == actual.len() && expected.iter().zip(actual).all(|(e, a)| e == a)
}
