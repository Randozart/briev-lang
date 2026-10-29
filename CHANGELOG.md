# Changelog

## v0.1.0-preview — 2026-10-28

### Highlights

- Tier 1 native Linux (`.bv`, `x86_64-unknown-linux-gnu`) and browser
  (`.rbv`, wasm32 webstack) are the supported release surfaces.
- Added the browser/server I11 integration path: deterministic peer gate,
  real browser/server gate, and a reproducible Python runner with artifact
  hashes.
- Added deterministic target reporting (`brievc targets`) and the Tier 1 /
  Tier 2 / experimental support contract.
- Added IR verification before external tools and a full seven-corpus IR
  determinism gate.
- Fixed state-collection lifetime, top-level unpacked-instance assignment,
  callable-txn tail-state leakage, defn web-flush leakage, keyed DOM churn,
  and reactor range `foreach`.
- Recovered `ws_frame` from a 3.85x locked-baseline regression to baseline
  parity; post-fix it is 0.30x C.

### Known limitations

- Cross-language C/C++/Python/Node bridges, C cancellation, needs-state
  reference, and CLI entry cases remain quarantined.
- GPU/electronics/silicon/SPIR-V surfaces remain experimental.
- `recommender_server_properties.rs` is not present in the external app
  checkout; its gate records an explicit skip.

## 2026-04-30

### Language

- **Enums**: Added `enum` declarations with Unit, Tuple, and Struct variants. Supports type parameters (e.g. `enum Result<T, E> { Ok(T), Err(E) }`).
- **Pattern matching**: New `[value Variant(field1, field2)]` guard syntax for destructuring enum variants and binding fields to variables. Works with identifiers and keyword variants (`Ok`, `Err`).
- **JSON serialization**: Built-in `to_json(value) -> String` and `from_json(json_str) -> Result<Object, String>` functions. `from_json` returns a `Result` enum that must be pattern-matched.
- **`b-style` directive**: Reactive style bindings in views (`b-style="property: signal"`).

### FFI Simplification (2026-04-30)

- **New FFI syntax**: Simplified foreign function declarations without TOML file references:
  - `frgn name(param: Type) -> Result<T, E>;` - standard FFI with compiler-picked address
  - `frgn! name(param: Type);` - fire-and-forget (void return)
  - `frgn name @ address (param: Type) -> Result<T, E>;` - explicit memory address
  - `frgn! name @ address (param: Type);` - fire-and-forget with address

- **New file-level attribute**: `#![ffi.<lang>, bind("./profile.toml"), map("from","to")]`
  - `ffi.<lang>` - language target (e.g., `ffi.c`, `ffi.kernel`, `ffi.js`)
  - `bind()` - optional profile TOML path
  - `import()` - optional script/library import path
  - `map()` - inline type mapping overrides

- **Parser changes** (`src/parser.rs`):
  - Added `process_ffi_attributes()` to extract FFI state from file attributes
  - Added `parse_type_name_token()` to handle type keywords and `Err` token
  - Updated `parse_frgn_binding()` to support new syntax and `frgn!`/`frgn` tokens
  - Added handling for `from` keyword as parameter name

- **AST changes** (`src/ast.rs`):
  - Added `FfiState` struct to hold language profile, bind path, import path, global maps
  - Extended `Program` struct with optional `ffi: Option<FfiState>` field

- **Typechecker changes** (`src/typechecker.rs`):
  - Skip binding loading when `toml_path` is empty (new profile-based FFI)

- **Cobol backend fixes** (`src/backend/cobol.rs`):
  - Fixed outdated AST variant names (`IntLit` → `Integer`, etc.)
  - Fixed field names in pattern matching (`target` → `lhs`, etc.)

- **Issues encountered and resolved**:
  - Borrow checker error in `parse_type_name_token()`: Fixed by dereferencing `String` before mutating `self`
  - `UInt` parsed as `Custom("UInt")`: Added `"UInt" => Ok(Type::UInt)` to `string_to_type()`
  - `"from"` treated as keyword in parameters: Added explicit `Token::From` handling in FFI parameter parsing
  - Missing `ffi` field in all `Program` constructors: Added `ffi: None` to 9 locations across codebase
  - Cobol backend using deprecated AST variants: Updated to current `Statement`/`Expr` variants

- **C Backend FFI Integration (2026-04-30 12:00-12:25)**:
  - Added `ffi_bindings` and `ffi_state` fields to `CBackend` struct (`src/backend/c.rs`)
  - Added `collect_ffi_bindings()` to collect FFI declarations from program
  - Added `generate_ffi_declarations()` to emit `extern` declarations in C output
  - Added `generate_ffi_call()` to handle FFI function calls in transactions
  - Updated `statement_to_c()` to detect and generate FFI calls
  - FFI calls now generate proper C function calls with type mappings

- **Stdlib Conversion (2026-04-30 12:24)**:
  - Converted `lib/std/io.bv` to new FFI syntax as example
  - Other stdlib files can be converted similarly

- **Phase 4: Script Import System (2026-04-30 12:35-12:50)**:
  - Added `src/ffi/script.rs` with `ScriptResolver` for loading JS/C/WASM scripts
  - Added `load_js()` to parse JavaScript function signatures
  - Added `load_c_header()` to parse C header function signatures
  - Added `load_wasm()` stub for WebAssembly (placeholder)
  - No LUT - functions resolved by exact name match
  - Exported `ScriptFunction`, `ScriptLanguage`, `ScriptResolver` from `ffi` module

- **Phase 5: Error Handling (2026-04-30 12:50-13:10)**:
  - Added `src/ffi/error.rs` with `ErrorConventions` for TOML convention parsing
  - Added `ErrVariant` enum for built-in error types (IoError, MappingError, BoundsError, VoidReturn, Generic)
  - Added `generate_bounds_check()` for error range checking in C code
  - Added `generate_null_check()` for pointer validation
  - Updated C backend to include error handling in FFI calls:
    - FFI calls with return values generate bounds checks
    - Assignments from FFI calls include error handling
    - Error labels generated for error flow control

### Compiler

- **Kernel Target Fixes (2026-04-29 19:25)**:
  - Fixed `return0` bug in C backend — now generates `return 0;` with proper space (`src/backend/c.rs:184,206`)
  - Fixed duplicate `briev_init` function — renamed wrapper to `init_wrapper()` to avoid kernel naming conflict
  - Fixed NULL state pointer in kernel mode — now uses `static State state_instance;` for static allocation
  - Fixed `find_entry_point()` to prioritize transaction named "init" over generic `[true]` preconditions
  - Fixed Makefile generation — removed circular `-objs` dependency that caused build failures
  - Added `MODULE_DESCRIPTION` to kernel module output
  - **Build test**: Successfully compiled `vitriol.bv` to `vitriol.ko` (175K) in `linux-pipe-module/`

- **Inline Assembly**: New `asm` syntax for low-level code generation. Syntax: `asm "instruction" { "clobber1", "clobber2" };`
- **New Backends**: Added `rust` and `c` CLI commands for native code generation.
  - `rust` command: Generates native Rust with `asm!` (requires nightly) or commented template
  - `c` command: Generates C with `__asm__ __volatile__`
- **CLI Changes**: `check` alias changed from `c` to `ck` to allow `c` for C backend
- **Lexer**: Added `enum`, `Ok`, `Err`, `match`, `Asm` tokens (`src/lexer.rs`).
- **AST**: Added `EnumDefinition`, `EnumVariant` (Unit/Tuple/Struct), `Type::Enum`, `Expr::PatternMatch`, `TopLevel::Enum`, `Statement::InlineAsm`. Changed `SvgComponent(String)` to `SvgComponent { name, content }` (`src/ast.rs`).
- **Parser**: Added `parse_enum()` for enum declarations. Extended guard parsing to detect pattern match expressions. Added `parse_asm_block()` for inline assembly (`src/parser.rs`).
- **Typechecker**: Added `Type::Enum` compatibility checks, stdlib signature registration for `to_json`/`from_json`, foreign sig collection, and `Expr::PatternMatch` inference (`src/typechecker.rs`).
- **Interpreter**: Added `Value::Enum` for runtime enum values. Pattern matching evaluates variant and binds fields. `to_json` serializes instances/lists/enums. `from_json` returns `Result::Ok` or `Result::Err`. InlineAsm statements are logged (no execution in interpreter) (`src/interpreter.rs`).
- **Import resolver**: SVG imports now extract component name from `as` alias or derive from filename. File-based imports (`.css`, `.svg`) preserve slash paths (`src/import_resolver.rs`).
- **Wasm codegen**: Added JS FFI glue for `__json_decode`, `__json_get_string`, `__json_encode`, `__http_get`, `__http_post`. Added `attr` and `style` directive rendering in patch engine (`src/wasm_gen.rs`).
- **New Backends**: Added `src/backend/rust.rs` and `src/backend/c.rs` for native code generation. Added `src/backend/mod.rs` module exports.
- **CLI**: Added `run_rust()` and `run_c()` functions in `src/main.rs`. Added `rust` and `c`/`cc` command handlers. Changed `check` alias from `c` to `ck`.
- **View compiler**: Added `b-style` directive parsing and `Style` binding variant (`src/view_compiler.rs`).
- **Annotator/Proof engine/Symbolic/Reactor**: Updated all passes to handle `Type::Enum`, `Expr::PatternMatch`, and `TopLevel::Enum` (`src/annotator.rs`, `src/proof_engine.rs`, `src/symbolic.rs`, `src/reactor.rs`).

### Stdlib

- **HTTP module**: New `lib/std/http.bv` with `http_get` and `http_post` wrappers over `__http_get`/`__http_post` FFI.

### Documentation

- **Language reference**: Added sections for Enum declarations, Enums with Data, Pattern Matching syntax, and JSON Serialization (`spec/LANGUAGE-REFERENCE.md`).

---

## 2026-04-27

### C Backend - Bare Metal ARM Support

- **Added `bare_metal` flag** to distinguish hosted vs bare-metal targets:
  - `.bv` files → `malloc` allocation, includes `stdlib.h` (Desktop/Embedded Linux)
  - `.ebv` files → static allocation, no heap (ARM bare-metal)

- **Linkage configuration support**:
  - Added `LinkageConfig` loading from `linkage.toml` alongside source file
  - Added `collect_hw_registers()` to find `@ link` hardware register names
  - Added `generate_linkage_defines()` to emit MMIO `#define` macros

- **Static allocation for bare-metal**:
  - Changed from `static State *state = NULL` + malloc to `static State state_instance; static State *state = &state_instance;`
  - Removed `stdlib.h` and `stdio.h` (unavailable in bare-metal)
  - Added `stddef.h` for NULL definition

- **ASM clobber syntax fix**:
  - Clobbers must be in the third section of GCC asm statement (output, input, clobber)
  - Previous incorrect format put clobbers in the input section

- **Hardware register handling**:
  - Identifiers matching `@ link` names generate `HW_REGISTER` macro instead of `state->hw_register`
  - MMIO base addresses resolved from `linkage.toml`

- **Files changed**:
  - `src/backend/c.rs`: Added linkage support, `bare_metal` flag, static allocation
  - `src/main.rs`: Added `run_c()` with linkage config loading

- **Tested with**:
  ```bash
  ./briev-compiler c kernel.ebv --out /tmp/test
  aarch64-linux-gnu-gcc -nostdlib -static -march=armv8-a -ffreestanding -O2 -c /tmp/test/kernel.c -o kernel.o
  ```

---

## Earlier Changes

### COBOL Transpiler

- **New transpilation target**: IBM Enterprise COBOL for z/OS
- **Pre/post condition guards**: RETURN-CODE 4000 on failure
- **Boolean Level 88 condition names**: `88 WS-VAR-TRUE VALUE 'Y'`
- **RECURSIVE always emitted** in PROGRAM-ID for recursion support
- **FFI via LINKAGE SECTION** for existing COBOL program integration
- **Files**: `src/backend/cobol.rs` (685 LOC), `src/main.rs` (`run_cobol()`)
