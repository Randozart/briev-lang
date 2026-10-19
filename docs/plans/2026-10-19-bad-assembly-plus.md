<!-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception -->
# .bad assembly+ — symbolic conveniences, raw escape, BILLD retirement

**Date:** 2026-10-19
**Status:** ACTIVE — plan locked; implementation starts with the raw-preserving skeleton.
**Worktree:** `../briev-bad-assembly-plus`
**Branch:** `feat/bad-assembly-plus`
**Supersedes:** the BILLD tier (`docs/plans/2026-10-08-billd-intermediate-dialect.md`,
`docs/architecture/bld-dialect.md`, SPEC §20.2). Historical plans stay untouched.

## Goal

Make `.bad` the one low-level dialect: direct hardware control with a lower-level-LLVM
feel. A programmer can write raw assembly exactly as today, or use symbolic values,
expressions, structured control, named engine verbs, and named boot steps — mixing
per line. The compiler is a **convenience expander**, not an optimizer.

The determinant is performance: raw code must stay hand-tunable, byte-for-byte. The
symbolic path must aim for instruction parity on scalar code and stay easy to replace
with raw wherever it does not.

## Locked decisions

| Decision | Choice |
|---|---|
| Dialect | `.bad` only. `.bld` disappears. |
| Default | assembly+ body: instructions, symbolic values, expressions, structured control. |
| Raw escape | physical/target registers and `raw <target>` blocks, emitted verbatim. |
| Register tiers | real target names (`rax`, `x0`, `a0`, `xmm0`), portable (`r0`–`r15`, `f0`–`f15`, `sp`, `pc`), symbolic (anything else). |
| Wrong-target real register | loud error naming the targets that have it; never a silent remap. |
| Conditional | `when` only. **No `else`.** Two-way = second `when` or a `local` label. |
| Control | `when cond: stmt`, `when cond { ... }`, `loop { }`, `while cond { }`, `break`, `continue`. |
| Labels | `local name:` and `global name`; structured control preferred. No dot syntax. Existing `.name:` stays as legacy explicit local. |
| Declarations | `declarations { }` for global/pins/clobbers/ABI; `data { }` for data labels/directives. |
| Inference | silent for predictable ceremony; error only when unsafe, with a paste-ready declaration snippet. |
| Defns | assembly+ bodies, inline by default; `call name(args)` forces a real C-ABI call; `inline name(args)` forces expansion. |
| Entry | `bootstrap Name { ... }` sugar for `global Name` + `Name:` + entry marking. `.bv` `bootstrap bad` stays. |
| Data label as value | resolves to its address in symbolic value position; raw `mov rN, label` still errors and points at `addr`. |
| Engine verbs | `std/bad/engine.bad` aliases + inline defns; delete `config/bad-verbs.dbvl`. |
| Immediate memory | `store`/`storeoff` accept immediates; materialize through a disclosed reserved scratch. |
| ISA metadata | rows carry operand roles and clobbers; allocator stops hardcoding `roles()`. |
| `logic { }` islands | not part of the default. Fallback only: if a symbolic pattern cannot hit parity, structured statements are gated behind an explicit block. |

**Supersedes the 2026-09-21 ruling** that virtual registers are rejected by design.
Physical-only code remains WYSIWYG; symbolic values are the convenience layer.

## Defaults policy (2026-10-19 decision)

Safe defaults only. No catch-all `default_dec!()`, no general `macro!` yet.

| Default | Behavior |
|---|---|
| `.text` | code section unless declared |
| `data { }` | section inferred from directive; explicit `section` overrides |
| `bootstrap Name { }` | global + label + entry |
| Engine prelude | `std/bad/engine.bad` default-on like the friendly aliases; `--raw`/`--no-prelude` opts out; user declarations override |
| Data label in value position | address, not a load |
| `local name:` | scoped/hygienic; no dots |
| Immediate operands | materialized through disclosed scratches |
| Frame-free leaves | no prologue unless a spill/callee-saved use exists |
| `fn` | C-ABI params/return staging; `extern fn` for raw labels |
| Data alignment | inferred from directive; `align` for overrides |

Still explicit: pins, raw-block clobbers, stack setup, exports beyond
`bootstrap`/`export`, signatures for raw labels, layout-sensitive
sections.

Target boilerplate uses **profile imports** (`std/bad/profiles/...`),
not macros: hardware constants, headers/vector tables, per-arch init
defns, raw-call clobbers. Profiles do not set entry or app pins.

Priority after symbolic lowering: `bootstrap` + `data`, `fn` + ABI
staging, profile imports, parity fixes. Parity fixes land with symbolic
lowering, not after.

## Surface

```bad
global _start
_start:
    mov vga, 0xB8000        // symbolic: allocator picks
    mov c, 66               // symbolic
    loop {
        when c > 68: break  // no else
        store c, vga
        vga += 2
        c += 1
    }

// three register tiers, same body:
mov rax, 0xB8000            // real x86_64 register; loud on other targets
mov r5, 0xB8000             // portable register; maps per target
mov vga, 0xB8000            // symbolic; allocator picks

declarations {
    global _start
    vga: r10                // pin
    clobbers raw_int13: r0, r1, r2
}

data {
    banner: .asciz "Briev rv64 boot\n"
}
```

### Boot-checklist shape (acceptance test)

```bad
import "std/bad/engine.bad"

defn grant_pmp() {
    WriteControlReg(944, -1)
    WriteControlReg(928, 15)
}

defn uart_init(base) {
    Store(0x101, base + 0x30)
}

defn uart_puts(base, msg) {
    let p = msg
    loop {
        let ch = Load(p)
        when ch == 0: break
        Store(ch, base)
        p += 1
    }
}

bootstrap Reset_Handler {
    DisableInterrupts()
    grant_pmp()
    uart_init(0x10000000)
    uart_puts(0x10000000, banner)
    Park()
}

data {
    banner: .asciz "Briev rv64 boot\n"
}
```

- Inline defns → no call, no return address, boot-safe.
- `call` when you have a stack and want sharing.
- `bootstrap` exports the entry and marks it for the linker.
- Data label in value position = address.

## Performance contract

| Guarantee | Meaning |
|---|---|
| Raw is verbatim | Physical/target-register lines and `raw` blocks emit exactly as written, source order. |
| Raw-only labels untouched | No virtuals → allocator skips the label; no frame/prologue. |
| Symbolic target | Instruction parity on the scalar corpus after the parity fixes; cycle measurements recorded. |
| No hidden optimizer | Only symbolic lowering + allocation. No scheduling, vectorization, DCE, or reordering. |
| No avoidable moves/jumps/frames | Parity items below are mandatory. |
| Line-level escape | Replace any symbolic line/region with raw without changing the rest. |
| Explicit boundary | Symbolic values crossing raw code require a pin or declared clobber; unsafe = error. |
| Traceability | `--emit-asm` + `--trace-allocation` show homes, spills, frames, inserted moves. |

### Parity work (mandatory)

1. In-place assignment: `x = x op k` and `x += k` lower into `x`'s home, no copy.
2. 2-operand accumulate forms, data-driven `acc` marker (`add d, x` = `d = d + x`).
3. Move coalescing / self-move elimination after allocation.
4. `when cond: break` / `continue` emits one inverted conditional branch, no extra `jmp`.
5. `seq`/`sne` setcc-style boolean values; no branch dance for equality values.
6. Frame-free leaves: no prologue/epilogue unless a spill or callee-saved use exists.
7. Raw `ret` handling: insert the epilogue before it, or reject it in a framed body.
8. Spill diagnostics: "value `x` spilled; pin it in `declarations` or split the region."
9. Symbolic-only peephole on compiler-generated instructions (e.g. redundant move
   removal). Never on raw instructions.

### Parity ledger (fill during implementation)

| Kernel | Raw `.bad` | Symbolic | Instruction delta | Cycle delta | Status |
|---|---|---|---|---|---|
| VGA loop | | | | | |
| UART banner | | | | | |
| memcpy/memset | | | | | |
| bit set/clear/test | | | | | |
| CRC8/checksum | | | | | |
| call chain | | | | | |

Results live in `benchmarks/results/2026-10-19-bad-assembly-plus.md`. Raw corpus must
be instruction-identical before/after the rewrite (modulo generated label names).

## Architecture

```
.bad source
  → parser (line/shape; blocks; symbolic statements)
  → AST (BadBodyItem: Instr | Let/Assign | When | Loop | While | Break | Continue |
           Call | Local | Site | Raw | Logic?)
  → imports expanded
  → names + consts collected
  → defn expansion (first-class AST pass; hygienic locals; branch/site selection)
  → symbolic lowering (expressions/control → virtual BadInstrs; block-local values)
  → allocation (roles/clobbers; raw labels skipped)
  → ISA emission (raw instructions untouched)
```

- Raw lines bypass expression lowering and allocation.
- Defn expansion runs before allocation so inlined `.bad` defns and symbolic bodies
  inside defns are visible to the allocator.
- Virtuals are label-local. Crossing a raw block requires a pin or declared clobber.
- `sp` guard: a body that touches `sp` cannot take a compiler frame; spills are loud.

### Parser/AST

- `BadBodyItem` gains symbolic statement variants and `Raw`/`Local`/`Site` handling.
- Blocks: `when`/`loop`/`while`/`data`/`declarations` use braces. Rule 21: braces are
  grouping/definition; `.bad`'s zero-brace doctrine is updated, not violated.
- `when` has one-way bodies only; `else` is a parse error pointing at a second `when`/label.
- `local name:`/`global name`: plain labels default to error unless declared. `.name:`
  remains a legacy explicit local.
- Data labels: infer section from directive kind; explicit `section` still allowed.
- `declarations` parses `global`, `name: reg`, `clobbers rawblock: rN, ...`.
- Error messages carry what/why/fix and paste-ready declaration snippets.

### Register resolution

- Real target aliases are built from `config/bad-registers.dbvl` (strip `%`, map
  `rax`→r0, `x0`→r0, `a0`→r0, `xmm0`→f0, width variants to `.w8/.w16/.w32`).
- Wrong-target real name → loud error listing targets that have it.
- Portable names map as today.
- Any other identifier in a value position → symbolic virtual.
- Symbols/labels only in `sym` operand positions (`jmp`, `call`, `addr`, data refs).
- Known data label in a symbolic value position → address (emit `addr`).
- Name collisions across virtuals/labels/consts/registers → loud error.

### ISA metadata

`config/bad-isa.dbvl` rows gain:

```text
mov:     "2"; ...; "roles d1,u2";
writecr: "2"; ...; "roles u1,u2"; "clobbers r0";
mul:     "3"; ...; "roles d1,u2,u3"; "clobbers r0,r1";
jz:      "3"; ...; "roles u1,u2"; "sym";
```

- `d`/`u` are operand indexes; `clobbers` are physical register defs.
- Missing metadata on an op used by symbolic code → loud error.
- Allocator forbids clobbered registers for crossing intervals or spills.
- `.bad` W-tier notices consume the same metadata.

### Engine verbs

`std/bad/engine.bad`:

```bad
defn DisableInterrupts
    x86_64 => cli
    riscv64 => mov r9, 8; csrc mstatus, r9
end

defn Park
    x86_64 => cli; halt
    default => halt
end

defn ReadControlReg n!, dst
    readcr dst, n
end

defn WriteControlReg n!, value
    writecr n, value
end
```

- Aliases for portable no-arg ops (`WaitForInterrupt`, `MemoryBarrier`, `InvalidateTlb`,
  `LoadGdt`, `FarJump`).
- `!` marks a constant-required param.
- Delete `config/bad-verbs.dbvl`.
- Delete the internal symbolic test fixture; use `lib/std/bits.bv` or inline expressions.

### Defns and calls

- `defn name(params) { ... }`: assembly+ body; inline expansion by default.
- `call name(args)`: real C-ABI call; requires annotated params/return (or inferred
  Int classes where unambiguous) and a valid `sp`.
- `inline name(args)`: force expansion.
- Inlined defn containing `ret` → loud error.
- Inline params substitute directly; no ABI staging, no copies.
- Recursion depth cap (64) as today.
- `bootstrap Name { ... }`: exactly one per image; emits global entry.

### `.bv` integration

- `.bv` `bad fn` and `bootstrap bad` bodies are parsed by the `.bad` front-end and
  inherit assembly+ automatically.
- Typed handoff via `bad fn` stays; `call`/inline rules apply.

## Phases

1. **Plan + raw golden corpus** — this doc; capture `.s` for existing `.bad` examples
   and tests as the raw-identity baseline.
2. **Raw-preserving parser/AST skeleton** — add statement/block AST and parser without
   changing emission for raw-only code. Golden raw corpus gate.
3. **ISA metadata** — roles/clobbers rows; loader; allocator consumes data; tests for
   `writecr`, `mul`, `div`, `shl`, `syscall`, `store`/`load`.
4. **Defn expansion pass** — AST-level, hygienic locals, branch/site selection, no
   `Defn` at emission; inline/call/inline modifiers; `ret` diagnostics.
5. **Symbolic lowering + allocation** — virtual inference, expressions, `when` (no
   else), loops, assignment; raw labels skipped; boundary pins/clobbers; block-local
   allocation.
6. **Parity fixes** — in-place assignment, accumulate forms, coalescing, branch fusion,
   `seq`/`sne`, frame-free leaves, raw `ret`, spill diagnostics, symbolic peephole.
7. **Engine stdlib + immediate stores** — `std/bad/engine.bad`; `store`/`storeoff`
   immediate forms; delete the old registry.
8. **Delete `.bld` surface** — extension/CLI/backend/source-kind; move engine under
   `src/backend/bad/`; remove target row, compile arms, conformance arm, vocab,
   frgn_dispatch; port `.bld` boots to `.bad` assembly+; remove highlighter `bld`.
9. **Boot checklist acceptance** — `examples/bad/boot_plus.bad` (named defns +
   `bootstrap`) QEMU-verified on rv64/aarch64/x86; no calls/frames on boot path; raw
   prologue byte-identical.
10. **Performance corpus + results** — raw vs symbolic kernels; parity ledger; results
    file; standing regression gate.
11. **Docs + verify** — `bad-dialect.md`, SPEC, README, primitive-coverage, INDEX;
    `cargo test --lib`, QEMU gates, determinism, Praetor, conformance,
    `git grep -i bld` historical only.

## Progress log

| Date | Commit | Slice |
|---|---|---|
| 2026-10-19 | `e2351d50` | Plan locked. |
| 2026-10-19 | `cfa0251f` | Raw `.bad` golden corpus (5 programs × 4 targets); template self-moves elided; raw blocks stay verbatim. |
| 2026-10-19 | `3bf5cb86` | Full-width target register spellings (`rax`, `x0`, `a0`, `xmm0`, `rsp`, `rip`) through the same registry; wrong-target is a loud error. |
| 2026-10-19 | `e4434d63` | Immediate materialization for `|-` rows through `scratch_int`/`scratch_int2`; `store`/`mul`/`mod` immediates now lower instead of erroring. |
| 2026-10-19 | `50b3e024` | Dot-free `local name:` labels scoped to the enclosing global; defn locals hygienic; bare labels stay global. |
| 2026-10-19 | `8b863eec` | `std/bad/engine.bad` named verbs; bare constant params in opcode slots (`readcr`/`writecr`) fixed; registry still exists for `.bld` until retirement. |
| 2026-10-19 | `94cd673d` | Safe-defaults policy: no catch-all macro; profiles planned. |
| 2026-10-19 | `09347abf` | Assembly+ symbolic label bodies (`let`/assignment/`when` no else/`loop`/`while`/`break`/`continue`), BILLD engine reuse, allocator loop-backedge fix, ABI registers available to leaves, in-place assignment, `when: break/continue` fusion, raw golden held. |
| 2026-10-19 | `15a39387` | `bootstrap Name { }` entry sugar; `data { }` blocks with section inference (`.rodata`/`.data`/`.bss`). |
| 2026-10-19 | `794f0476` | Callable `fn` with typed C-ABI params/return, arg staging, cross-calls, void auto-return; labels and fns lower in one synthetic program. |
| 2026-10-19 | `6c259278` | Direct `BldStmt` AST (no synthetic `.bld` text); `BldStmt::Instr`; `addr(label)` expression + label-as-address; shared expression parser; raw goldens unchanged. |
| 2026-10-19 | `b5335e2a` | Definition modes: `defn` inline symbolic, `call defn` callable, `fn` removed. Inline expansion with param binding, block scoping, depth cap 64. |
| 2026-10-19 | `75a3a5bd` | Two-way boundary: `let x = r5` copy in, `r5 = x` copy out, `let p = pin r5` alias, `raw clobbers rN { … }`; clobbers/pins reserved for the recipe. |
| 2026-10-19 | `1d940b71` | `call defn` ABI staging: direct moves when no parallel-move cycle; staging only on collisions. |
| 2026-10-19 | `2f34597f` | `Park` name parity between symbolic and raw engine verbs; `std/bad/profiles/rv64-qemu-virt.bad` constants + UART steps. |
| 2026-10-19 | `1382e595` | Raw `.bad` engine defn calls keep their arity when an intrinsic shares the name (`ReadControlReg n, dst` vs `ReadControlReg(n)`). |
| 2026-10-19 | `73e81d84` | `.bld` dialect retired: routing/CLI/backend-kind/source-kind/target row/examples/gates/highlighter/docs/SPEC removed; symbolic engine stays internal. `config/bad-verbs.dbvl` is the internal verb table. |
| 2026-10-19 | `d762d032` | Pure `.bad` rv64 QEMU boot gate (`boot_plus_rv64.bad`, `qemu-rv64-boot-plus-bad.sh`); `brievc bad --linker-script`. |
| 2026-10-19 | `e8e5f493` | Symbolic `stb`/`sth` (sub-width stores, immediate forms + address materialization), aarch64 QEMU virt profile, pure `.bad` aarch64 boot gate. |

Remaining: profile imports, data-label address expressions in symbolic
code, symbolic `defn` (inline) bodies, ISA roles/clobbers data migration,
`.bld` deletion, boot-checklist QEMU acceptance, performance corpus,
highlighter/docs sweep. Known test note: the conformance sweep can race
its own `.smoke/` artifacts when run concurrently with other tests; it
passes in isolation.

## Resolution plan (2026-10-19, approved)

Blockers from the ergonomic review, with locked spellings:

| Blocker | Resolution |
|---|---|
| One-way raw/symbolic escape | `let x = r5`, `r5 = x`, `let y = pin r5`, `raw clobbers rN { … }`, `declarations { }` |
| No symbolic address-of | `addr(label)` + a known label in a symbolic value position = address; `extern fn`/`extern data` |
| No stack-free named steps | inline `defn name(params) { … }`; `call defn name(params) { … }` = callable; `fn` removed |
| Split engine vocabulary | symbolic calls inline `std/bad/engine.bad` defns; delete `bld-intrinsics.dbvl` |
| `fn` call staging moves | parallel-move scheduler (direct moves when safe; temps only for cycles) |
| Implicit-virtual typos | `--warn-unused-values` |
| Unfinished surface | direct `BldStmt` AST, profiles, `declarations`, `.bld` deletion |

**Definition syntax (locked):**

```bad
defn inc(a: Int) -> Int { return a + 1 }        // INLINE; no return allowed
call defn inc(a: Int) -> Int { … }              // CALLABLE; ABI/frame; return allowed
```

Calls resolve by definition kind; `call name(args)` forces a call. No `fn`.

**Phase order:**

1. Direct `BldStmt` AST: build the symbolic program in memory; add
   `BldStmt::Instr(BadInstr)`; parse expressions via the shared parser;
   add ISA roles/clobbers for `addr`/`readcr`/`writecr`; remove synthetic
   `.bld` text and re-parsing. Output must stay byte-identical.
2. Definition unification: inline symbolic `defn` expansion (params,
   hygiene, depth/size caps); `call defn` callable; remove `fn`.
3. Address-of + `extern fn`/`extern data`.
4. Two-way boundary: `let x = r5`, `r5 = x`, `pin r5`,
   `raw clobbers rN { … }`, `declarations`; allocator clobber wall.
5. Engine verbs: symbolic calls inline `.bad` defns; delete
   `config/bad-verbs.dbvl`; add `!` constant params.
6. Call staging/coalescing, stack profile, profiles, unused-value lint.
7. Boot checklist QEMU gate; delete `.bld`; performance corpus; docs.

Do not delete `.bld` before Phase 5; its intrinsic registry is the
transitional engine-verb source.

## Non-goals

- No instruction scheduling, vectorization, or global optimizer in `.bad`.
- No parity claim for SIMD/tightly scheduled/hand-addressed hot code; raw is the escape.
- No `.bld` compatibility shim.
- No catch-all declarations macro and no general macro system in v1;
  imports/defns/profiles cover the current needs.
- No contracts beyond existing `.bad` positional contracts in v1.
- No automatic `.rodata`/section invention beyond directive-kind inference; unsafe
  cases error with a snippet.

## Logging

- Add bugs/limitations to `BUGS.md` as discovered.
- Record parity and raw-identity results in `benchmarks/results/`.
- Update `docs/architecture/bad-dialect.md` in the same commit as structural changes.

## To undo

Delete this plan and revert the `.bad` parser/AST/lowering/allocator changes; restore
the `.bld` modules from git history; restore `BackendKind::Bld`, `SourceKind::Bld`,
`brievc bld`, the `.bld` target row, `config/bad-verbs.dbvl`, `lib/std/bld/`,
`examples/bld/`, and the highlighter `bld` language. Historical plans remain untouched.
