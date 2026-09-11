# CTD and ALU — Type Metadata Architecture (Superseded)

**Date:** 2026-07-17  
**Superseded:** 2026-07-20 by hashword protocol architecture  
**See:** `docs/architecture/casting-protocol.md` for the current design

CTD and ALU metadata properties are replaced by hashword op signatures.
Types no longer declare `ctd <~ "Float"` or `alu <~ "Float"`. Instead,
they declare `op Add(Float, Float)` — the backend knows from the `Float`
hashword what to emit.

The old `ctd_to_llvm()` + `derive_llvm_type()` fallback chain is replaced
by structure-driven `llvm_type` inference (from fields + bytes).

This document is retained for historical reference during the transition.

## Purpose

Two orthogonal metadata dimensions replace the old `primitive` field:

- **CTD** (Common Type Definition) — What the type *is* semantically.
  An exhaustive, closed set of PascalCase identifiers that every backend must understand.
- **ALU** (Arithmetic Logic Unit) — What hardware computes with values of this type.
  PascalCase for known ALUs, lowercase-quoted for backend/plugin-specific hardware.

## The Exhaustive CTD Set

| CTD | Meaning | LLVM type | JS type | SPIR-V ALU |
|-----|---------|-----------|---------|------------|
| `Int` | Signed integer (size by bytes) | `i8/i16/i32/i64` | `"number"` | `Int` |
| `UInt` | Unsigned integer | `i8/i16/i32/i64` | `"number"` | `Int` |
| `Float` | 32-bit float | `"float"` | `"number"` | `Float` |
| `Double` | 64-bit float | `"double"` | `"number"` | `Float` |
| `Bool` | Boolean | `"i8"` | `"boolean"` | `Bool` |
| `Char` | Unicode codepoint | `"i32"` | `"number"` | `Int` |
| `String` | Heap-allocated string | `"ptr"` | `"string"` | `Int` |
| `Data` | Heap-allocated bytes | `"ptr"` | `"Uint8Array"` | `Int` |
| `Ptr` | Opaque pointer | `"ptr"` | `"number"` | `Ptr` |
| `Void` | No value | `"void"` | `"null"` | `Int` |

This list is EXHAUSTIVE. No new CTDs can be added without updating every backend normalizer.

## Naming Convention

| Syntax | Meaning | Who reads it |
|--------|---------|-------------|
| `ctd = String` (PascalCase Identifier) | Built-in frontend-known type | All backends |
| `alu = Float` (PascalCase Identifier) | Built-in frontend-known ALU | All backends |
| `alu = "my_dsp"` (lowercase quoted String) | Opaque, backend/plugin-only | Specific backend or plugin |

## ALU × CTD Validation Rules

Enforced by the LLVM normalizer. Quoted ALUs bypass validation.

| CTD | Compatible ALUs | Incompatible ALUs |
|-----|----------------|-------------------|
| `Int`, `UInt`, `Char` | `Int` | `Float`, `Bool` |
| `Float`, `Double` | `Float` | `Int`, `Bool` |
| `Bool` | `Bool` | `Int`, `Float` |
| `String`, `Data` | `Int` | `Float`, `Bool` |
| `Ptr` | `Int` | `Float`, `Bool` |
| `Void` | `Int` | `Float`, `Bool` |

## Data Flow

```
Primordial (seed_primordial_types)
  │  Sets ctd + alu for every built-in type
  ▼
Normalizer (per-backend)
  │  Reads CTD → computes llvm_type via ctd_to_llvm()
  │  Validates ALU × CTD compatibility
  │  Stores llvm_type as a ResolvedType property
  ▼
Backend (LLVM, Webstack, SPIR-V, CIRCT)
  │  Reads llvm_type from property (or ctd directly for webstack)
  │  Pure consumer — never recomputes
  ▼
Generated IR
```

## Adding a New Built-in CTD

1. Add it to `PRIMORDIALS` in `src/type_universe/mod.rs`
2. Add default ALU in `default_alu()`
3. Add CTD→LLVM mapping in `ctd_to_llvm()` in `src/backend/llvm/normalizer.rs`
4. Add ALU×CTD validation entries in `validate_alu_ctd()` (same file)
5. Add JS type mapping in `src/backend/webstack_normalizer.rs`
6. Add SPIR-V ALU mapping in `src/backend/spirv/normalizer.rs`
7. Update this document and the exhaustive CTD table above
