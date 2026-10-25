//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// Authored by Otto Westerlund (@odecode); adopted with attribution, see
// docs/contributors/otto-westerlund.md.
// Build tests/fixtures/collection_snapshot.rbv --split, then pass its WASM path.
import assert from 'node:assert/strict';
import fs from 'node:fs';
const source = fs.readFileSync(new URL('../src/glue/web_heap.js', import.meta.url), 'utf8');
const { createWebHeap } = await import('data:text/javascript;base64,' + Buffer.from(source + '\nexport { createWebHeap };').toString('base64'));
let memory;
const heap = createWebHeap(() => memory);
const { instance } = await WebAssembly.instantiate(fs.readFileSync(process.argv[2]), {
  env: { ...heap, __web_flush_state: () => {} },
});
memory = instance.exports.memory;
const api = instance.exports;
api.__web_boot();
const state = api.__briev_state_ptr();
function snapshot(name) {
  const pointer = api['__view_items_' + name](state);
  const data = new DataView(memory.buffer);
  const count = Number(data.getBigInt64(pointer, true));
  return Array.from({ length: count }, (_, index) => {
    const string = Number(data.getBigUint64(pointer + 8 + index * 8, true));
    const size = Number(data.getBigUint64(string, true));
    return new TextDecoder().decode(new Uint8Array(memory.buffer, string + 8, size));
  });
}
assert.deepEqual(snapshot('items'), ['héllo', '世界']);
assert.deepEqual(snapshot('empty_items'), []);
api.grow_items(state);
assert.deepEqual(snapshot('items'), Array.from({ length: 1100 }, (_, index) => String(index)));
assert.deepEqual(snapshot('empty_items'), []);
api.reset_items(state);
assert.deepEqual(snapshot('items'), []);
console.log('WASM collection state survives boot; Unicode, >1023 items, shared snapshots and reset pass');
