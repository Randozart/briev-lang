# Per-arch stdlib boot entries (named raw blocks + .bv-top-level .bad import)

**2026-09-22**

One `.bv` file whose `bootstrap bad` body is a portable core calling
per-arch primitives by name — the raw per-arch prologues live in
`std/bad/arch.bad` as **named raw blocks**, imported at the `.bv` top
level. Proves `.bad`'s universal core: the bootstrapper body is written
once, only the entry prologues are per-target, and they are stdlib data
(Rule 14 — the compiler learns nothing per-arch).

## Motivation

The previous phase proved each boot path separately. This phase makes
them SHARE one source, factored per the author's design:

- Per-arch primitives are **named raw blocks** in a stdlib `.bad` file:
  `raw riscv64 uart_init ... end`. A name emits a label on the matching
  family, so the portable core can `call uart_init` / `jmp uart_init` —
  from the bootstrap body OR any `.bad` body.
- The `.bv` file imports the `.bad` file at TOP LEVEL:
  `import "std/bad/arch.bad";` — the resolver recognizes `.bad` (like the
  existing `.css`/`.svg`/`.dbv`/`.dbvl` special-cases), records the
  resolved path, and hands it to the bad backend as a source (never
  parsed as Briev).
- The bootstrap body shrinks to: entry label + `jmp uart_init` + a
  portable core (banner, handoff, halt) using only universal ops.

Interpretation A confirmed: named raw blocks callable from any bad body.
Interpretation B (`.bv` defns calling `.bad` primitives as typed
functions) is documented as a follow-up via the existing `bad fn` ABI
wrapper.

## Changes

### 1. Named raw blocks
- AST `BadRawBlock` gains `name: Option<String>`.
- Parser `try_raw_block`: split `rest` into `target` + optional `name`.
- Lowerer pass 1 `collect_names`: register the name in `label_names`
  ONLY when `family.starts_with(&target)` (prevents cross-family collision
  in the global label namespace).
- Lowerer pass 2: on family match, emit `{name}:` before the raw lines.
- Anonymous raw blocks stay legal.

### 2. `std/bad/arch.bad`
Named per-arch `uart_init` blocks (x86_64 long-mode, riscv64 PMP,
thumbv7m vector table), each ending `jmp/j/b core` to rejoin the portable
core.

### 3. `.bv`-top-level `.bad` import
- `import_resolver.rs`: `.bad` extension → record resolved path, no Briev
  parse.
- `compile_bad_fn_objects` → `generate_bad_fn`: thread `base_dir`
  (opts.file_path's dir) into `Lowerer.with_base_dir` so the bootstrap
  body's `import "std/bad/arch.bad"` resolves.

### 4. `examples/bad/bootloader.bv`
`import "std/bad/arch.bad";` + a tiny `kernel_bv` defn (semicolon fix) +
`bootstrap bad` body = `jmp uart_init` + portable core + `call kernel_bv`
+ `halt`.

### 5. Gates + docs
`tests/bare/qemu-universal-{rv64,arm}.sh`; bad-dialect.md "Per-arch
stdlib entries"; plan records. (x86_64 via multiboot; MBR stays
`boot_mbr.bad` — 16-bit cannot host the 64-bit core.)

## Undo

Revert the AST name field, the parser split, the lowerer name emission +
family-gated registration, the `.bad` import-resolver case, the base_dir
threading, and the example/gates/docs.

## Doc updates

- `docs/architecture/bad-dialect.md`: named raw blocks + per-arch stdlib
  entries section.
- This plan.
- Follow-up: interpretation B (`.bv`→`.bad` typed calls via `bad fn`).