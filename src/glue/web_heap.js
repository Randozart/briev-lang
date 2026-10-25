//===----------------------------------------------------------------------===//
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//===----------------------------------------------------------------------===//
// 2026-10-21: browser allocations preserve the module's initial stack/globals.
// Authored by Otto Westerlund (@odecode); adopted with attribution, see
// docs/contributors/otto-westerlund.md.
// Coalescing reuses released buffers; all host byte operations validate ranges.
class WebHeap {
  constructor(memory) {
    this.memory = memory;
    this.frontier = 0;
    this.allocations = new Map();
    this.available = [];
  }
  sizeOf(value) {
    const size = Number(value);
    if (!Number.isSafeInteger(size) || size < 0 || size > 0xfffffff0)
      throw new RangeError('Invalid allocation size');
    return Math.max(16, Math.ceil(size / 16) * 16);
  }
  pointer(value) {
    const pointer = Number(value);
    if (!Number.isInteger(pointer) || pointer < -0x80000000 || pointer > 0xffffffff)
      throw new RangeError('Invalid memory pointer');
    return pointer >>> 0;
  }
  range(pointer, length) {
    const start = this.pointer(pointer);
    const size = Number(length);
    if (!Number.isSafeInteger(size) || size < 0 || start + size > this.memory().buffer.byteLength)
      throw new RangeError('Memory range exceeds browser memory');
    return [start, start + size];
  }
  malloc(value) {
    const size = this.sizeOf(value);
    const index = this.available.findIndex(block => block.size >= size);
    if (index >= 0) return this.reuse(index, size);
    const linear = this.memory();
    if (!this.frontier) this.frontier = linear.buffer.byteLength;
    const end = this.frontier + size;
    if (end > 0x100000000) throw new RangeError('Browser memory exhausted');
    const pages = Math.ceil((end - linear.buffer.byteLength) / 65536);
    if (pages > 0) linear.grow(pages);
    const pointer = this.frontier;
    this.frontier = end;
    this.allocations.set(pointer, size);
    return pointer;
  }
  reuse(index, size) {
    const block = this.available[index];
    const pointer = block.pointer;
    block.pointer += size;
    block.size -= size;
    if (!block.size) this.available.splice(index, 1);
    this.allocations.set(pointer, size);
    return pointer;
  }
  free(value) {
    const pointer = this.pointer(value);
    if (!pointer) return;
    const size = this.allocations.get(pointer);
    if (size === undefined) throw new RangeError('Invalid allocation pointer');
    this.allocations.delete(pointer);
    this.available.push({ pointer, size });
    this.available.sort((a, b) => a.pointer - b.pointer);
    this.coalesce();
  }
  coalesce() {
    for (let i = this.available.length - 2; i >= 0; i--) {
      const current = this.available[i];
      const next = this.available[i + 1];
      if (current.pointer + current.size !== next.pointer) continue;
      current.size += next.size;
      this.available.splice(i + 1, 1);
    }
  }
  realloc(value, length) {
    const pointer = this.pointer(value);
    if (!pointer) return this.malloc(length);
    const oldSize = this.allocations.get(pointer);
    if (oldSize === undefined) throw new RangeError('Invalid allocation pointer');
    if (Number(length) === 0) { this.free(pointer); return 0; }
    if (this.sizeOf(length) <= oldSize) return pointer;
    const replacement = this.malloc(length);
    this.copy(replacement, pointer, oldSize);
    this.free(pointer);
    return replacement;
  }
  copy(destination, source, length) {
    const [start, end] = this.range(source, length);
    const [target] = this.range(destination, length);
    new Uint8Array(this.memory().buffer).copyWithin(target, start, end);
    return target;
  }
  fill(pointer, value, length) {
    const [start, end] = this.range(pointer, length);
    new Uint8Array(this.memory().buffer).fill(Number(value) & 255, start, end);
    return start;
  }
  string(value) {
    const pointer = this.pointer(value);
    if (!pointer) return 0;
    this.range(pointer, 8);
    const length = Number(new DataView(this.memory().buffer).getBigInt64(pointer, true));
    if (length < 0 || length > 1024 * 1024 * 1024) return 0;
    this.range(pointer, 8 + length);
    const replacement = this.malloc(length + 9);
    this.copy(replacement, pointer, 8 + length);
    this.fill(replacement + 8 + length, 0, 1);
    return replacement;
  }
  imports() {
    return {
      malloc: this.malloc.bind(this), free: this.free.bind(this), realloc: this.realloc.bind(this),
      briev_bits_to_str: this.string.bind(this),
      calloc: (count, width) => {
        const size = Number(count) * Number(width);
        const pointer = this.malloc(size);
        this.fill(pointer, 0, size);
        return pointer;
      },
      memcpy: this.copy.bind(this), memmove: this.copy.bind(this), memset: this.fill.bind(this),
    };
  }
}
function createWebHeap(memory) { return new WebHeap(memory).imports(); }
