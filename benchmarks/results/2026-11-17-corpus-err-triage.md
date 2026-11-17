# 2026-11-17 — verify-bodies corpus triage (queue item 1, step 0)

Follows `docs/plans/2026-11-17-device-free-work-queue.md` item 1. The followups
plan (F3) called this "a fog, not a task": 34 ERR files with group letters that
**do not exist anywhere in the tree** (grepped `docs/` and `BUGS.md`). This is the
triage that turns the fog into a list.

## 1. The runner was lying — two defects, both fixed

### 1.1 Every file shared one `--out` directory (cross-attribution)

`scripts/verify_bodies_corpus.sh` builds `-P $(nproc)` files in parallel and
passed **the same** `--out "$workdir/out"` to every one. Parallel builds
therefore overwrote each other's `<name>.ll`, and `opt` verified whichever file
won the race. Symptom: `examples/wildcard_lift.bv` came back carrying a dozen
verifier errors belonging to `bootloader.ll`, `io.ll`, `process.ll` — while the
files that actually owned those errors looked clean.

Fixed: each file gets its own `--out "$workdir/build-$key"`. (Some of
`wildcard_lift`'s errors are genuine: it *imports* the other example modules, so
their IR really is verified under its build. The tell was `bootloader.ll`, which
it does not import.)

### 1.2 `examples/bad/` is the bare-metal DIALECT, not "invalid source"

The dir name misleads. `docs/architecture/bad-dialect.md` documents `.bad` as the
embedded/`.b` dialect: `bootloader.bv` boots riscv64, `typed_boot.bv` is its
typed end-to-end proof, `boot_mps2.bv` is QEMU-verified. They have no host build
path — the same category as the three already excluded. `bootloader.bv` and
`typed_boot.bv` were charged to the floor as verifier errors for a program that
is not host-buildable. Now excluded with that reason.

## 2. The floor line

Invocation (release binary, fresh build — the script's default is the debug
binary, and both agree, see §5):

```
BRIEVC=<release>/brievc bash scripts/verify_bodies_corpus.sh --full
```

| counter | before the runner fix | after |
|---|---|---|
| Files | 245 | **243** |
| PROVEN | 59 | **59** |
| UNPROVEN | 208 | **207** |
| NO_POSTCONDITION | 0 | **0** |
| SKIPPED | 34 701 | **34 560** |
| VIOLATED | 0 | **0** |
| ERR | 28 | **25** |

**These are the numbers this tree actually produces.** They do not match the
"242 files / 210 PROVEN / 18 UNPROVEN / 34 ERR" recorded in session notes, and I
could not reproduce that figure: the gap is not binary flavour (§5), not the
runner fixes (those moved ERR by 3 and PROVEN by 0), and not the flags. Either
the historical number came from a different tally (the script's own header
records a past accident where the report was appended three times, tripling
per-goal counts) or the prover's coverage changed substantially and silently.
**Treat 59/207 as the new floor and investigate the 18→207 gap separately before
anyone claims "the corpus improved"** — the floor line is the gate, and a floor
nobody can reproduce cannot gate anything.

## 3. The ERR list, classified

All 25 ERR files live under `examples/`. Not one is in `lib/std/`, `tests/`, or
`trophies/` — the production corpus is clean. Five classes, by observed text:

| class | files | first error |
|---|---|---|
| **B** pointer-typed const with an integer initialiser | pointer-trickery, ptr-arithmetic, defn-composite, volatile-io | `integer/byte constant must have integer/byte type` |
| **C** ptr/i64 register width mismatch at an adaption site | ptr-swap, sig_as_type (+ B's files once B is fixed) | `'%t37' defined with type 'i64' but expected 'ptr'` / `'%t5' defined with type 'ptr' but expected 'i64'` |
| **D** undefined global | import-forms (`@val`), sync-group (`@i`) | `use of undefined value '@val'` |
| **E** link failure | ptr-runtime, wildcard_lift | `clang failed to compile … to binary` |
| **F** type errors in the example source | mmap-demo, networking, process-spawn | `error: type errors:` |

`wildcard_lift` additionally surfaces B/C/D from the modules it imports — that
attribution is genuine (see §1.1) and should not be double-counted when slicing.

## 4. Class B — FIXED in this commit (4 files)

`const UART_DR: Ptr<Bits<8>> = 0x40001000;` emitted

```llvm
@UART_DR = constant ptr 1073811456
```

which LLVM rejects: a pointer constant needs the `inttoptr` form. Fixed in the
global-constant emitter (`src/backend/llvm/mod.rs`): a numeric initialiser for a
`ptr`-typed const now emits

```llvm
@UART_DR = constant ptr inttoptr (i64 1073811456 to ptr)
```

Verified: `pointer-trickery.bv` no longer fails on the constant; it now advances
to class C (its next error), which is what a sliced fix looks like.

## 5. Binary flavour is not the variable

Same corpus, same commit, `tests/tier1` only:

| binary | PROVEN | UNPROVEN | ERR |
|---|---|---|---|
| release | 8 | 68 | 0 |
| debug | 8 | 68 | 0 |

So the prover's verdicts are stable across optimisation level; the historical
discrepancy in §2 is not a stale-binary artefact.

## 6. Next slice — class C (the real codegen bug)

After class B, the same files fail here:

```llvm
%t24 = load ptr, ptr @UART_IBRD      ; the const is ptr-typed, so the load is ptr
%t26 = inttoptr i64 %t24 to ptr      ; ← an adapter assuming an i64 handle
call void @write_uart(ptr %t26, i64 %t25)
```

A pointer-valued register is fed through an `inttoptr`, which is invalid IR. The
mechanism for this already exists: `fun.reg_value_ll` records the **actual** LLVM
width of a register (added 2026-11-12 for exactly this "stamped Briev type lies
about the register width" problem), and `intrinsics::adapt_to_ptr_width` already
distinguishes three ptr-LLVM-type shapes with it. The failing site is *not* one
of those adapters — it emits `inttoptr i64` unconditionally. Finding it and
routing it through `reg_value_ll` (skip the conversion when the source register
is already `ptr`) is the next slice; it should clear C for all six files.

Do this as one slice, with `examples/pointer-trickery.bv` as the fixture and the
floor line re-measured — not as a batch with D/E/F.
