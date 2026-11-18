# Invalid IR is now a first-class diagnostic (M1–M4, 2026-11-18)

Implements `docs/plans/2026-11-18-compiler-diagnostics-ir-verification.md`.

## What was wrong (measured)

`brievc build --llvm examples/syntax/meta/defn-composite.bv` returned **0** while
writing a module LLVM rejects. A normal build did verify (`compile_ll_to_binary` →
`verify_emitted_ir`), but its message was a raw `opt` dump naming `%t36` and the
`.ll` path — no "this is our defect", no source construct, nothing an author could
act on. Two codegen bugs in the previous session hid behind exactly that.

## What it says now

```
$ brievc build examples/syntax/meta/defn-composite.bv --llvm …
error: the compiler produced invalid IR for '/tmp/v1/defn-composite.ll'
  why:  LLVM rejected the module: '%t36' defined with type 'i64' but expected 'float'
             816 |   %cmn12 = add nuw nsw i64 %cmc16, 1
        >    817 |   %pbf14 = fadd fast float 0.0, %t36
             818 |   br label %.cm_header, !llvm.loop !37
        %t36 is defined at line 811: %t36 = load i64, ptr %t20, align 8
        %t20 is defined at line 789: %t20 = alloca i64, align 8
        the instruction is in: define i32 @main(i32 %argc, ptr %argv) …
  note: this is a compiler defect, not a mistake in your program
  fix:  the module is kept at '/tmp/v1/defn-composite.ll' — pass it, with the
        smallest program that reproduces it, to whoever maintains the compiler
```

Exit code 1. `--no-verify-ir` opts out (measured: exit 0).

Three properties worth keeping:

* **the failing instruction is quoted and marked**, not just a register name;
* **the register is traced to its definition, twice** — `%t36` → the load → the
  alloca that is the real defect (`alloca i64` for a `Float`), which is the whole
  diagnosis;
* **the trace is confined to the enclosing function.** The first implementation
  searched the module and reported a `%t36` from another function 190 lines away —
  a confident wrong pointer, which is worse than none. `function_range` bounds it,
  and `the_trace_never_crosses_a_function_boundary` pins that.

## Corpus sweep — the column becomes actionable

`scripts/ir_verify_corpus.sh` compiles each file twice: once with `--no-verify-ir`
(catches frontend errors) and once with the verifier on. A file that passes the
first and fails the second is `files_invalid_ir` — a compiler defect, not a program
error.

| directory | valid | invalid IR | frontend error |
|---|---|---|---|
| `tests/` | 124 | **2** | 45 |
| `examples/` | 77 | **1** | 15 |

The three findings:

1. `examples/syntax/meta/defn-composite.bv` — the known loop-carried Float (D1).
2. `tests/fixtures/glue_gen/parity_generated.bv` — `use of undefined value '@str_to_c'`
3. `tests/fixtures/glue_gen/parity_handwritten.bv` — same

(2) and (3) are **new**: two fixtures that have been emitting modules LLVM cannot
link, in no test that compiles them to a binary. Recorded here rather than fixed
under a diagnostics session.

`lib/` is not swept yet — 2 compilations per file over the standard library exceeds
a 40-minute window; the script needs parallel execution before that run is useful.

## Message sweep (M4, priorities 1–2)

* the clang/link failure message → replaced by the block above (it shares
  `ir_verify`'s renderer, so `build`, `--llvm` and `check --validate` say the same
  thing);
* the two kernel-surface bail-outs no longer print `Discriminant(11)`:

```
warning: accel 'kb': kernel emission failed — SPIR-V lowering: a compute kernel body
  cannot contain this statement: `when (i < 4) { a[i] = 1; }`
  why:  the SPIR-V kernel lane lowers let / assign / expression / term over integer
        expressions, and this construct has no kernel form
  fix:  compute it on the host lane and pass the result in, or declare the node
        without `accel` to run the whole body on the CPU
```

`Statement` implements `Display`, so the author's own code is quoted (bounded to
110 characters, single line) — `stmt_excerpt`, shared by both sites.

## Verification

* `cargo nextest run --lib ir_verify` — 7 tests (valid module, invalid module,
  defect framing, line parse for both tool prefixes, excerpt bounds, definition
  trace, cross-function containment).
* `cargo nextest run --lib stmt_excerpt` — 1 test.
* `bash scripts/gate.sh quick` — **pass** with the verifier enabled by default, i.e.
  no program in the gate's corpus emits invalid IR.
