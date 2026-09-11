# Briev: A Programming Language

### TL;DR

**Briev is currently a WIP, be aware the design has not been entirely locked in yet**

Briev is programming language with the following features:

- Contract driven partial correctness verification (without needing to write essays of formal proof)
- Partially declarative, partially imperative, functional invariant based programming
- Invariant based runtime optimization (with execution speed at parity, or even better than C in several cases)
- Backend independence through state-management based syntax and backend handling of intrinsics
- Backend targeting by extension (`.bv`, `.ebv`, `.rbv`, `.abv`, `.sbv`)
- A lightweight systems language that handles complexity at compile time
- Syntax that is equally applicable to the other backends, not just systems programming
- Inferred command flow through reactive top level `node` declarations
- Memory management by proof through AST graphing
- Syntax which encourages and optimizes for flat programming
- Legible metaprogramming syntax for macros and compiler plugins with intermediate `.beast` AST files for understanding how the compiler transforms the code
- The Metropolitan FFI which adapts compiled Briev code to any and every other programming language by adapting their calling and memory conventions through the Metropipe and GLUE system
- Helpful compiler hints on failed compilation
- Optional Python-like indent-based syntax for those who despise curly braces through the opt-in `.f.*bv` extension argument
- Optional extra strictness through the `.s.*bv` extension argument
- Extensible compile-time literals
- A highlighter shipped with the repo

And for those people for whom this is very important, even if the language itself is already robust enough:

- A cute mascot called _Syn_

| <img src="assets/syn_present.png" alt="Syn showing you the logos" width="250"/><br/> <p align="center">*Syn*, the Cybersphinx<p> <p align="center"><sup><sub>(For those people who like a mascot to come with their language)</sup></sub><p> | <img src="assets/briev-logo.svg" alt="Briev" width="200"/><br/><p align="center">**Briev**<p><img src="assets/e-briev-logo.svg" alt="Embedded Briev" width="200"/><br/><p align="center">**Embedded Briev**<p><img src="assets/a-briev-logo.svg" alt="Accelerated Briev" width="200"/><br/><p align="center">**Accelerated Briev**<p> | <img src="assets/r-briev-logo.svg" alt="Rendered Briev" width="200"/><br/><p align="center">**Rendered Briev**<p><img src="assets/d-briev-logo.svg" alt="Data Briev" width="200"/><br/><p align="center">**Data Briev**<p><img src="assets/s-briev-logo.svg" alt="Silicon Briev" width="200"/><p align="center">**Silicon Briev**<p> |
|---|---|---|

## Quick Start

### 1. Build the Compiler

```bash
# Build in debug mode
cargo build

# Build in release mode
cargo build --release

# Run the full library test suite
cargo test --lib
```

### 2. Create Your First Program

Create `counter.bv`:

```briev
let counter: Int = 0;
let bound: Int = GetEnvInt!("BOUND");

node tick [counter < bound][counter == bound] {
    counter = counter + 1;
    term;
};
```

### 3. Compile and Run

```bash
# Type-check only (fast)
./target/release/brievc check counter.bv

# Compile to a native binary (LLVM → machine code)
./target/release/brievc build counter.bv

# Web frontend (TypeScript + view bindings)
./target/release/brievc build counter.rbv

# Embedded/microcontroller binary
./target/release/brievc build counter.ebv

# SPIR-V GPU kernel
./target/release/brievc build counter.abv

# CIRCT hardware description
./target/release/brievc build counter.sbv

# Emit LLVM IR instead of a binary
./target/release/brievc build --llvm counter.bv
```

## Language Variants

The file extension selects which backend compiles your program, and what syntax rules apply. Each variant is a different *view* of the same contract-proven core — the same reactive transaction model, the same `[pre][post]` contracts — but adapted for a different material or strictness tier:

| Type | File Ext | Description | Compilation Target |
|------|----------|-------------|-------------------|
| <img src="assets/briev-icon.svg" alt="Briev" width="25"/> **Briev** | `.bv` | Pure declarative logic | LLVM → native binary, optional SPIR-V offload |
| <img src="assets/a-briev-icon.svg" alt="Briev Accel" width="25"/> **Accelerated Briev** | `.abv` | GPU compute kernel | SPIR-V (GPU intrinsics, no FFI, restricted types) |
| <img src="assets/r-briev-icon.svg" alt="Rendered Briev" width="25"/> **Rendered Briev** | `.rbv` | Reactive web UI | TypeScript + WASM sidecars + view bindings |
| <img src="assets/e-briev-icon.svg" alt="Embedded Briev" width="25"/> **Embedded Briev** | `.ebv` | Microcontroller bare-metal | LLVM → microcontroller binary (no OS, no GC) |
| <img src="assets/s-briev-icon.svg" alt="Silicon Briev" width="25"/> **Silicon Briev** | `.sbv` | Pure hardware logic graph | CIRCT → Verilog/VHDL (no FFI, no external deps) |
| <img src="assets/d-briev-icon.svg" alt="Data Briev" width="25"/> **Data Briev** | `.dbv` / `.dbvs` / `.dbvl` | Configuration data, schemas, line-based databases | Parsed and validated by Briev itself, consumed by all targets |

### Why Variants Exist

Each variant has a different *contract baseline* and *feature set* appropriate to its target. The `s` prefix (Strict Briev — `.sbv`, `.srbv`, `.sebv`) uses the **same backend** as its base variant but enforces stricter syntax rules:

| Variant | Contract Sugar | Intrinsics | `frgn` | Typical Use |
|---------|---------------|------------|--------|-------------|
| `.bv` (Briev) | `[[post]`, `[pre]]` | All available | C, Rust, Python, Java, JavaScript | General-purpose |
| `.rbv` (Render) | sugar allowed | All available | JavaScript (inlined); C/Rust via WASM | Web frontends |
| `.ebv` (Embed) | sugar allowed | All available | C, Rust (Python/Java warned) | Bare-metal MCU |
| `.sbv` (Silicon) | sugar banned | Hardware subset only | Banned | Hardware synthesis |
| `.dbv` (Data) | No contracts | None | None | Configuration |

The rationale: **contracts are optimization information**. The more complete your contracts, the more the compiler can prove, and the faster your program runs. Sugar syntax (`[[post]`, `[pre]]`) is a convenience for prototyping, but strict variants force you to commit to full specifications. This is what makes Briev different from total languages (Coq, Agda — must prove everything upfront) and mainstream languages (C, Rust — prove nothing by default).

**Planned:** COBOL backend (future target for enterprise integration).

## Key Features

- **Contracts first** — `[pre][post]` contracts are the source of truth; they power partial-correctness verification and unlock optimizations.
- **Reactive by default** — top-level `node` declarations infer execution flow.
- **Zero-nesting logic** — flat, single-assignment style that keeps bodies shallow.
- **Metropolitan FFI** — GLUE (compile-time, TOML-driven cross-language wrappers) and Metropipe (runtime shared-memory IPC).
- **Compile-time verification** — contracts, invariants, and static analysis catch errors before runtime.

> Detail on each lives in `docs/architecture/` and `spec/`.

## Compiler Architecture

Frontend-driven: the LLVM backend consumes analysis computed once in the
frontend (loop shapes, swan songs, density, modulo partitions, inline and
batch decisions) and derives type knowledge from the casting graph. Tunables
live in config. See **[docs/compiler-architecture.md](docs/compiler-architecture.md)**
for the pipeline and `docs/architecture/` for the backend internals.

## Standard Library

300+ native functions across 15+ modules, plus 20 auto-imported OS modules
(fs, net, thread, atomic, mem, process, signal, time, ipc, io, …).
See **[docs/standard-library.md](docs/standard-library.md)** for the inventory
and `lib/std/` for the source.

## Learning Briev

```bash
cd learn-briev
```

1. **[00-welcome.md](learn-briev/00-welcome.md)** - What is Briev?
2. **[01-basics.md](learn-briev/01-basics.md)** - Variables, types, transactions
3. **[02-contracts.md](learn-briev/02-contracts.md)** - Preconditions & postconditions
4. **[03-reactive.md](learn-briev/03-reactive.md)** - Reactive transactions
5. **[11-triggers.md](learn-briev/11-triggers.md)** - Triggers and reactive I/O
6. **[15-custom-types.md](learn-briev/15-custom-types.md)** - Custom types with operator declarations

**Full documentation:**
- [spec/SPEC.md](spec/SPEC.md) — language specification
- [spec/LANGUAGE-TUTORIAL.md](spec/LANGUAGE-TUTORIAL.md) — detailed tutorial
- [spec/QUICK-REFERENCE.md](spec/QUICK-REFERENCE.md) — syntax cheat sheet
- [docs/philosophy.md](docs/philosophy.md) — the design philosophy
- [docs/architecture/](docs/architecture/) — backend, casting, and analysis internals
- [docs/handoff-methodology.md](docs/handoff-methodology.md) — the rigorous methodology (required reading)

## Performance

Briev is at or better than C parity on the runtime benchmarks it targets,
including several that beat C (kalman 0.86×, float_math_nonzero 0.95×,
float_math 0.66×, print_loop 0.62×, queue_drain 0.50× at BOUND=50M —
ratio < 1 means Briev is faster). See
**[docs/2026-07-31-session-report.md](docs/2026-07-31-session-report.md)** for
the current benchmark tables and findings, and `benchmarks/results/` for the
raw runs.

## Self-Hosting & Project Structure

The compiler parses, type-checks, and generates code for itself. See
**[docs/self-hosting.md](docs/self-hosting.md)** and
**[docs/project-structure.md](docs/project-structure.md)**.
