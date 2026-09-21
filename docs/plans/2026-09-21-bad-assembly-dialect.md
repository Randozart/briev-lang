# .bad — Briev Assembly Dialect

**2026-09-21.** Design session: syntax resolved interactively (see "Decisions").
Goal: a portable assembly dialect that feels native — universal core ISA with
zero ceremony, `target =>` exception/default granularity for architectural
optimization, compile-time-inlined `defn`s, and Briev-style formal contracts.

## Decisions (locked during design)

| Decision | Choice |
|---|---|
| Place in pipeline | **New backend** (`BackendKind::Bad`, `.bad` extension) |
| Integration depth | Pure assembly + Briev-style contracts (no Briev expressions inside bodies) |
| Core ISA | **Built-in** — `mov add sub mul div load store cmp jmp jz jnz call ret push pop nop syscall halt`; universal lowerings in `config/bad-isa.dbvl` (row shape of `asm-lowering.dbvl`) |
| Exception keyword | None — bare `target => instr` match-arm form (rejected `except` as prose, `:` as delimiter-overload) |
| Inline exceptions | `x86_64 => lea r0, [r1 + 1]` on the line after an instruction; replaces it on that target, universal otherwise |
| Optimization granularity | `defn` (sequence shape + branch shape), inlined as-is at call site |
| Multi-instruction | `;` separators within one line |
| Directives | Standard asm: `section`, `global`, `.asciz`, `.word`, `.byte`, `.half`, `.align`, `.zero` |
| Contracts | Label-level `[pre: c] [post: c]` + inline `[expr]` before an instruction |
| Registers | Built-in portable `r0`-`r15`, `sp`, `pc`; `config/bad-registers.dbvl` maps to physical + properties; `alias` is source-level sugar |
| Block structure | **Strictly line-oriented, zero braces** — label/defn own following lines until next top-level construct; no indentation sensitivity |
| Label form | Brace-free `name [contracts]` on its own line |
| Data labels | `msg: .asciz "..."` (colon-suffix, asm-native; distinct construct from exceptions) |
| Error path | Missing universal lowering for target + no exception = loud compile error (capability doctrine, what/why/fix) |
| Target scope | All three ISA tables up front: x86_64, aarch64, riscv64 |

## Grammar (token-shape disambiguation only)

| Line shape | Meaning |
|---|---|
| `mnemonic operands` | instruction — belongs to nearest preceding label/defn |
| `target => instr; instr` | exception — attaches to nearest preceding instruction (or whole defn in branch-defn) |
| `default => ...` | branch-defn default row (must use universal core syntax) |
| `name [contracts]` | label — owns following instructions until next top-level line |
| `defn name params` | defn head — owns following lines until next top-level line |
| `section .x` / `global n` | top-level directives |
| `.dir args` | standard asm data/alignment directive |
| `msg: .asciz "..."` | data label + directive |
| `[expr]` | inline contract for next instruction |
| `alias x = r0` | register alias |

Two defn shapes:
1. **Sequence defn** — universal body lines (+ optional per-instruction exceptions).
2. **Branch defn** — only `default?/target => line` rows; `default` row uses
   universal core syntax; a target row replaces the whole defn on match.

## Register model

- `config/bad-registers.dbvl`: per-target rows `target.x86_64: r0=rax; r1=rcx; ... sp=rsp; pc=rip;`
  plus properties (caller_saved/callee_saved, readability).
- Existence validation: `r20` on x86_64 (16 GPRs) = loud compile error naming
  target and fix. Never silent remap.
- Properties drive **proofs**: `[post: r3 preserved]` checks callee-saved
  status from config; caller-saved preservation demands verified push/pop pairing.
- Special-register knowledge (`zero` etc.) lives in ISA lowering templates,
  never in register logic (Rule 15).
- Width: MVP 64-bit only; subregisters deferred (clean later extension).

## Contract semantics

- `result` keyword + named params usable in contract expressions; register
  terms refer to portable names.
- Checked at label boundaries (caller↔callee), defn expansion points, and
  inline positions. Failures are loud, house-style what/why/fix.

## Phases

### Phase 1 — Foundation
1. `config/bad-isa.dbvl` — universal per-target lowerings (x86_64, aarch64,
   riscv64) for the full core ISA.
2. `config/bad-registers.dbvl` — register maps + properties, same three targets.
3. `src/ast/bad.rs` — `BadProgram { directives, labels, defns, aliases }`,
   `BadLabel { name, contracts, body }`, `BadInstr { mnemonic, operands,
   exceptions }`, `BadDefn` (sequence + branch shapes), contracts.
4. `src/parser/bad.rs` — standalone line-oriented parser (separate dialect,
   separate entry; not woven into the .bv parser).
5. `BackendKind::Bad` + `.bad` row in `config/targets.dbvl` + golden test
   (`parity_targets_dbvl_matches_toml`) updated in the same commit.

### Phase 2 — Lowering
6. `src/backend/bad/` — resolve per target: expand defns inline as-is, apply
   exceptions (target match → replacement; else universal lowering), map
   registers via config, emit target assembly text.
7. Contract checking pass with what/why/fix diagnostics.
8. Assembly path: reuse `AsmAssembler` (`PlatformAssembler`) `.s` → `.o`,
   link via existing linker driver.

### Phase 3 — Tests
9. Parser tests: every construct, every error case.
10. Lowering tests: same program → three targets, snapshot asm output.
11. Contract-failure diagnostics tests.
12. End-to-end: hello-world `.bad` → binary → runs on host (x86_64).

### Documentation (same commit as structural changes)
- `docs/architecture/bad-dialect.md` — the dialect reference.
- `spec/SPEC.md` section + syntax-highlighter token updates.

## Undo

Delete `src/ast/bad.rs`, `src/parser/bad.rs`, `src/backend/bad/`, both config
files, revert `BackendKind`/targets.dbvl rows, remove docs sections. No
existing optimization path is touched (additive-only, Rule 6).
