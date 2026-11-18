# Better compiler messages: make invalid IR a first-class diagnostic (2026-11-18)

Prompted by the loop-carried-Float session: the compiler emitted a module LLVM could
not parse, `brievc build --llvm` returned **0**, and the whole burden of discovery
fell on a linker message naming a register the author never wrote.

Four findings, then a plan ordered by value-per-hour.

---

## Findings

### F1. On a clang failure the compiler says nothing useful

`src/compile.rs:3356`:

```rust
if !status.success() {
    return Err(format!("clang failed to compile '{}'", source_path.display()));
}
```

What the author gets:

```
error: clang failed to compile 'examples/syntax/meta/defn-composite.bv'
```

Clang's actual diagnostic (`'%t36' defined with type 'i64' but expected 'float'`)
was written to inherited stdio — it appears *somewhere* in the scrollback,
unattributed, and no message says the module is the compiler's fault rather than the
program's. House style (`docs/architecture/agent-reference.md`, "Helpful
diagnostics") requires what/why/fix; this has one of the three.

### F2. `--llvm` never validates, so invalid IR ships with exit 0

`brievc build x.bv --llvm` writes `<x>.ll` and returns success. No verifier runs.
Two of this session's bugs hid exactly here:

* the f16 store (`store half %t35` with `%t35 : float`) — the program "built";
* the loop-carried Float (`fadd fast float 0.0, %t36` with an i64 operand) — same.

A compile step that reports success on a module LLVM rejects is the most expensive
kind of message defect: **it is silent**. Everything downstream (benchmarks, CI,
corpus gates) then measures or scores a program that cannot exist.

### F3. There is no way to attribute a register to a construct

* The emitted IR carries **no debug metadata**: `!dbg` / `DIFile` / `DILocation`
  count is **0** in a real module.
* AST statements and expressions carry **no spans** — only the diagnostic-recovery
  nodes in `briev-verify/src/ast/bad.rs` do.

So the author is told about `%t36`, and nothing maps that to `acc_ = acc_ + x` in
`$defn accumulate`. The emitter, however, *holds* the `&Statement` at every point —
so attribution is available cheaply (see M2) without adding spans.

### F4. The verifier already exists — in a test, not in production

`src/backend/llvm/match_termination_tests.rs:22` has exactly the right helper:

```rust
Command::new("clang").args(["-Wno-override-module", "-x", "ir", "-c"]) …
assert!(result.status.success(), "LLVM rejected match IR: {}\n{ir}");
```

It is used by that module only. Nothing in `src/compile.rs` calls it.

### Why this matters beyond one bug

`BUGS.md` groups A–E are **17 corpus IR regressions** — all "invalid IR that only
clang sees". They are tracked as an `ERR` column in the corpus tally, i.e. a number
nobody can act on individually. A production verifier plus a corpus sweep turns them
into 17 named diagnostics with the offending construct.

---

## Plan

### M1. Verify every emitted module, by default (highest value, lowest cost)

Instead of the current post-hoc link, run the verifier on the module *before* it is
used:

* mechanism: `clang -Wno-override-module -x ir -fsyntax-only <tmp.ll>` (faster than
  `-c`; the existing test uses `-c`, both work). Cache the result per module hash so
  repeated builds of an unchanged file are free.
* run it in **every** mode, including `--llvm` (that is the hole), and before the
  link step so the message is about the compiler's output, not clang's invocation.
* on failure: keep the `.ll` on disk (it is the reproducer), print the diagnostic
  below, and **exit non-zero**.
* `--no-verify-ir` for emergencies; the default is on. Skip silently only when the
  chosen target has no clang (cross builds), with a `--verbose` note — never silently
  on the host target.

The message (what / why / fix, and it must not blame the author):

```
error: the compiler produced invalid IR for 'examples/syntax/meta/defn-composite.bv'
  why:  LLVM rejected the module: '%t36' defined with type 'i64' but expected 'float'
        register %t36 is used at defn-composite.ll:817
          %pbf14 = fadd fast float 0.0, %t36
  note: this is a compiler defect, not a mistake in your program
  fix:  the module is kept at build/defn-composite.ll — attach it (and the smallest
        program that reproduces it) to a bug report
```

### M2. Breadcrumbs in the IR, so the message can name the construct

The emitter has the `&Statement` at every emission point. Emit a comment when the
"current construct" changes:

```llvm
  ; briev: accumulate! loop body — acc_ = acc_ + x   (defn-composite.bv)
  %t27 = fadd fast float %t31, %t30
```

Then, on a verifier failure, take clang's line number, walk back to the nearest
`; briev:` marker, and quote it. Result: "the invalid register was produced while
emitting `acc_ = acc_ + x`". This needs **no spans and no LLVM metadata** — the
breadcrumb is a comment, which the verifier ignores, so there is no risk to codegen.

Cost: one line per construct; keeps `.ll` diffs noisy in review, so gate the
breadcrumbs behind `--ir-breadcrumbs` (on for `--llvm` and on for verifier failures,
off for the determinism corpus).

### M3. Corpus sweep: verify every emitted `.ll`

`scripts/verify_bodies_corpus.sh` currently classifies by the compiler's own output.
Add a pass that runs the verifier on each emitted module and reports
`invalid_ir` separately from `unproven`:

```
files_invalid_ir  17   (BUGS.md groups A-E — each now has a construct breadcrumb)
```

That converts an unactionable column into a burn-down list, and it is the gate that
would have caught both of this session's codegen bugs at the commit that introduced
them.

### M4. Message sweep for what/why/fix (opportunistic, per touched file)

Audit the `error:` / `warning:` strings a *user* can reach and add the missing parts.
Use the accel-lane messages added on 2026-11-17 as the template:

```
warning: accel node 'rowsum' will run on the CPU lane, not the device
  why:  the kernel shape is valid, but this mode could not prove a device win for it
  fix:  restructure the body to the kernel surface, or pass `--accel=force` …
```

Priority within M4 (things an author actually hits):

1. `clang failed to compile '<f>.bv'` → M1's message (this one is F1);
2. codegen bail-outs that currently return bare prose (`SPIR-V lowering:
   unsupported statement …`, `host body statement … outside the runner v1 surface`)
   → what the construct is, why the lane cannot carry it, and the two fixes
   (restructure / choose the other lane);
3. every "internal" message that leaks register or pass names without saying "this is
   a compiler defect".

---

## Order, cost, gates

| # | item | time | gate |
|---|---|---|---|
| 1 | M1 verifier on by default in every mode | 0.5 d | known-bad module fails `build`; `--llvm` exits 1; good corpus unaffected |
| 2 | M2 breadcrumbs + attribution of a failing line | 1 d | tonight's two bugs each report the construct; determinism check still byte-identical with breadcrumbs off |
| 3 | M3 corpus `invalid_ir` column | 0.5 d | 17 named, each with a breadcrumb |
| 4 | M4 sweep (start with the three priorities) | 1–2 d | messages carry what/why/fix; snapshot tests for the shaped ones |

## Trade-offs and risks

* **Cost per build.** The verifier is one clang invocation (~50 ms) plus, on failure,
  the file already written. Cache by module hash so incremental builds pay nothing.
* **Don't turn a compiler bug into a user error.** The severity and wording matter
  more than the plumbing: M1's message must say the *compiler* is at fault and hand
  the author a reproducer path, or we will teach people to work around our defects.
* **Breadcrumbs vs `.ll` determinism.** `ll_determinism_check.sh` byte-compares two
  compilations; comments are deterministic, but they inflate diffs. Keep them behind a
  flag and assert the flag's default in one test.
* **Verifier ≠ correctness.** It proves the module parses and types check; it cannot
  see the truncating `sitofp`/`fptosi` round-trip in D1. That still needs the
  differential/arithmetic gate — a reason to keep M1 scoped as "well-formedness".

## Why this is worth doing now

Three of the last four bugs in this campaign were found by *reading emitted IR*, not
by a diagnostic: the f16 store, the 16× `LocalSize` over-dispatch, and the
loop-carried Float. Each cost hours of instrument-building. M1 alone makes the class
"the compiler cannot even produce a module" impossible to ship silently, and M2 makes
the message point at the construct instead of a register.
