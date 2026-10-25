//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// Authored by Otto Westerlund (@odecode); adopted with attribution, see
// docs/contributors/otto-westerlund.md.
import assert from 'node:assert/strict';
import fs from 'node:fs';
const source = fs.readFileSync(new URL('../src/glue/web_heap.js', import.meta.url), 'utf8');
const { createWebHeap } = await import('data:text/javascript;base64,' +
  Buffer.from(source + '\nexport { createWebHeap };').toString('base64'));
const memory = new WebAssembly.Memory({ initial: 2, maximum: 6 });
new Uint8Array(memory.buffer).fill(73);
const heap = createWebHeap(() => memory);
const first = heap.malloc(17n);
const second = heap.malloc(48);
assert.equal(first, 131072);
assert.equal(second, first + 32);
assert.equal(new Uint8Array(memory.buffer)[65536], 73);
heap.memset(first, 42, 17n);
const replacement = heap.realloc(first, 80000n);
assert.deepEqual([...new Uint8Array(memory.buffer, replacement, 17)], Array(17).fill(42));
heap.free(second);
assert.equal(heap.malloc(80), first, 'adjacent freed blocks coalesce');
const zero = heap.calloc(8n, 8n);
assert.deepEqual([...new Uint8Array(memory.buffer, zero, 64)], Array(64).fill(0));
heap.memcpy(zero, replacement, 17n);
heap.memmove(zero + 1, zero, 16n);
assert.deepEqual([...new Uint8Array(memory.buffer, zero, 17)], Array(17).fill(42));
assert.equal(heap.realloc(zero, 0), 0);
assert.throws(() => heap.free(zero), RangeError);
assert.throws(() => heap.malloc(-1), RangeError);
assert.throws(() => heap.malloc(Number.MAX_SAFE_INTEGER), RangeError);
const text = heap.malloc(11);
new DataView(memory.buffer).setBigInt64(text, 2n, true);
new Uint8Array(memory.buffer).set([0xc3, 0xa9], text + 8);
const copy = heap.briev_bits_to_str(text);
assert.notEqual(copy, text);
assert.equal(new DataView(memory.buffer).getBigInt64(copy, true), 2n);
assert.deepEqual([...new Uint8Array(memory.buffer, copy + 8, 3)], [0xc3, 0xa9, 0]);
assert.equal(heap.briev_bits_to_str(0), 0);
assert.throws(() => heap.memcpy(0, memory.buffer.byteLength - 1, 2), RangeError);
assert.throws(() => heap.memset(memory.buffer.byteLength, 0, 1), RangeError);
assert.throws(() => heap.memmove(0, -2147483649, 1), RangeError);
assert.throws(() => heap.calloc(Number.MAX_SAFE_INTEGER, 2), RangeError);
assert.throws(() => heap.malloc(6 * 65536), RangeError);
console.log('Browser heap: growth, stack isolation, reuse, realloc, zeroing and bounds pass');
