# Briev Documentation Index

> **PARTIALLY STALE (2026-09-08).** Reference section numbers predate the
> 2026-08-05 spec conformance migration. The authoritative language-surface
> index is `docs/reference/MASTER-SYNTAX-REFERENCE.md`.

## Quick Navigation

### Getting Started
- [README](README.md) - What Briev is and how to install
- [QUICK-REFERENCE](QUICK-REFERENCE.md) - Syntax at a glance, common patterns
- [LANGUAGE-TUTORIAL](LANGUAGE-TUTORIAL.md) - Step-by-step guide (legacy reference)

### Language Specification
- [SPEC](SPEC.md) - **Master specification** - All language features in one document
  - [Core Language](SPEC.md#1-introduction-and-philosophy)
  - [FFI System](SPEC.md#4-foreign-function-interface-ffi)
  - [Type System](SPEC.md#5-type-system)
  - [Standard Library](SPEC.md#6-standard-library)
  - [Implementation Status](SPEC.md#7-implementation-status)

### Language Variants
- [RENDERED-BRIEV](RENDERED-BRIEV-GUIDE.md) - Web UI components (`rstruct`, directives)
- [EMBEDDED BRIEV](EMBEDDED_BRIEV_2.1_SPEC.md) - Float types, vectors, bit ranges

### Reference
- [QUICK-REFERENCE](QUICK-REFERENCE.md) - Cheat sheet
- [FFI GUIDE](FFI-GUIDE.md) - FFI guide (legacy, superseded by SPEC)
- [WASM SETUP](lib/ffi/wasm/README.md) - WASM backend guide

### Examples
- [examples/](examples/) - Working code examples
  - [bank_transfer_system.bv](examples/bank_transfer_system.bv) - Multi-account state
  - [reactive_counter.bv](examples/reactive_counter.bv) - Reactive transactions
  - [test_ffi.bv](examples/test_ffi.bv) - FFI patterns
  - [stdlib_usage.bv](examples/stdlib_usage.bv) - Standard library usage

### Build & Test
- [CLI GUIDE](CLI-GUIDE.md) - Command line interface
- [CONTRIBUTING](#contributing) - Development setup

---

## Feature Cross-Reference

| Feature | Spec Section | Examples |
|---------|-------------|----------|
| Transactions | SPEC.md §3 | reactive_counter.bv |
| FFI (`frgn`, `syscall`) | SPEC.md §4 | test_ffi.bv |
| Fire-and-forget FFI | SPEC.md §4.1 | (see FFI guide) |
| Resource System | SPEC.md §4.3 | (see FFI guide) |
| Float Types | EMBEDDED_BRIEV §2.4 | (see embedded spec) |
| Bit Packing | SPEC.md §4.2 | (advanced examples) |
| Async Transactions | SPEC.md §3 | (see examples) |
| Pattern Matching | SPEC.md §3 | (see language tutorial) |
| Projection Operator (`:>`) | SPEC.md §3.15 | learn-briev/13-projections.md |
| Ptr\<T\> Types | SPEC.md §3.16 | learn-briev/05-data-types.md §7 |
| Bit Manipulation Intrinsics | SPEC.md §3.15 | std/bits.bv, learn-briev/13-projections.md |
| Safe Pointer Ops | SPEC.md §6.9 | std/ptr.bv |
| DFA Regex (`:> Match`) | SPEC.md §3.17 | learn-briev/13-projections.md |
| Roofline / Bottlenecks | SPEC.md §3.18 | lib/targets/bottlenecks.dbvs |
| Collection Mutation (`<-`) | SPEC.md §3.14 | 05-data-types.md |

---

## Contributing

This documentation is generated from the compiler's understanding of the language. To update the spec, modify `SPEC.md` directly. Key files:

- `spec/SPEC.md` - Master specification (this file)
- `src/` - Compiler source code (truth reference for implementation)
- `spec/old_docs/` - Archived legacy specifications
- `examples/` - Working code examples

To test examples:
```bash
cd examples/bank_transfer_system
briev run
```

---

*Generated from compiler state v0.10.0 (2026-04-20)*