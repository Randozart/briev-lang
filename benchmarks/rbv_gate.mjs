// rbv_gate.mjs — wasm32 runtime gate for the web surface.
//
// Usage: node benchmarks/rbv_gate.mjs <built.wasm>
// Exits 0 on pass, 1 on any failure. Instantiated with the browser host
// imports stubbed (location/navigate/malloc/__web_flush_state) so the router
// fixture's state + navigation can be observed without a DOM.
//
// 2026-10-07 (plan 2026-10-07-web-surface-completion.md, W1): guards the
// pointer-width / void-frgn / view-liveness fixes. A funcref signature
// mismatch (e.g. `call i32` vs `declare void`) lowers to a trap stub at
// runtime — these checks catch it.
import fs from 'fs';

const wasmPath = process.argv[2];
if (!wasmPath) {
  console.error('usage: node rbv_gate.mjs <built.wasm>');
  process.exit(2);
}
const bytes = fs.readFileSync(wasmPath);
let instance = null;
let bump = 0;
let navCalls = [];
let maxFlushCount = -1;

// Heap grows UPWARD from the end of the initial image (the stack lives in
// the image and grows down) — every allocation lands in fresh pages.
function malloc(n) {
  const need = (Number(n) + 7) & ~7;
  const mem = instance.exports.memory;
  if (bump + need > mem.buffer.byteLength) {
    mem.grow(Math.ceil((bump + need - mem.buffer.byteLength) / 65536) + 1);
  }
  const p = bump;
  bump += need;
  return p;
}
function writeStr(s) {
  const enc = new TextEncoder().encode(s);
  const ptr = malloc(8 + enc.length);
  const dv = new DataView(instance.exports.memory.buffer);
  dv.setBigUint64(ptr, BigInt(enc.length), true);
  new Uint8Array(instance.exports.memory.buffer, ptr + 8, enc.length).set(enc);
  return ptr;
}
function readStr(ptr) {
  if (!ptr) return null;
  const mem = instance.exports.memory;
  const dv = new DataView(mem.buffer);
  const len = Number(dv.getBigUint64(ptr, true));
  return new TextDecoder().decode(new Uint8Array(mem.buffer, ptr + 8, len));
}

const { instance: inst } = await WebAssembly.instantiate(bytes, {
  env: {
    // (updatesPtr, count) — count only; the gate asserts commits happened,
    // the DOM decode itself is exercised by the shim's own tests.
    __web_flush_state: (updatesPtr, count) => { maxFlushCount = Math.max(maxFlushCount, Number(count)); },
    malloc: (n) => malloc(n),
    location: () => writeStr('/'),
    // The browser host import from the router's host module — recorded.
    navigate: (p) => { navCalls.push(readStr(p)); },
  },
});
instance = inst;
bump = instance.exports.memory.buffer.byteLength;
const ex = instance.exports;

let failures = 0;
function check(name, got, want) {
  const ok = got === want;
  if (!ok) failures++;
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}: got=${JSON.stringify(got)} want=${JSON.stringify(want)}`);
}

// Boot: init_state runs current_path() (runtime, host-provided) + route_name.
ex.__web_boot();

// Runtime (host-derived) string comparison — the pointer-width bug class.
check('route_name(current_path()=="/")', readStr(ex.route_name(writeStr('/'))), 'home');
check('route_name("")', readStr(ex.route_name(writeStr(''))), 'home');
check('route_name("/about")', readStr(ex.route_name(writeStr('/about'))), '/about');
check('briev_str_eq equal', ex.briev_str_eq(writeStr('home'), writeStr('home')), 1);
check('briev_str_eq different', ex.briev_str_eq(writeStr('home'), writeStr('Home')), 0);
check('briev_str_eq lengths differ', ex.briev_str_eq(writeStr('ab'), writeStr('abc')), 0);
check('str_len_bytes', ex.str_len_bytes(writeStr('hello')), 5);

// View-surface liveness: the view-trigger-bound handler must be EMITTED
// (export exists — defn-liveness used to drop it) and must RUN end-to-end:
// update state, commit a flush batch, host-navigate.
check('go export exists (view-trigger txn emitted)', typeof ex.go, 'function');
navCalls = [];
maxFlushCount = -1;
ex.go(ex.__briev_state_ptr(), writeStr('/about'));
check('go("/about") fires navigate', JSON.stringify(navCalls), JSON.stringify(['/about']));
check('go commits a state flush batch', maxFlushCount > 0, true);
navCalls = [];
ex.go(ex.__briev_state_ptr(), writeStr('/'));
check('go("/") fires navigate', JSON.stringify(navCalls), JSON.stringify(['/']));

process.exit(failures === 0 ? 0 : 1);
