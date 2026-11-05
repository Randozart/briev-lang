//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// Execute the tuple_values Rust integration's wasm32 IR after clang/wasm-ld.
import assert from 'node:assert/strict';
import fs from 'node:fs';
let cursor = 131072;
let memory;
const { instance } = await WebAssembly.instantiate(fs.readFileSync(process.argv[2]), {
  env: { malloc(bytes) {
    const address = cursor;
    cursor = (cursor + Number(bytes) + 7) & ~7;
    if (cursor > memory.buffer.byteLength) memory.grow(Math.ceil((cursor - memory.buffer.byteLength) / 65536));
    return address;
  } },
});
memory = instance.exports.memory;
for (const value of [17, 999, -37]) assert.equal(instance.exports.probe(value), value * 2);
console.log('wasm32_nested_tuple_values_ok');
