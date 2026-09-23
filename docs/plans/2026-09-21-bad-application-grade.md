# .bad — Application-Grade Dialect (RCT-worthy)

**2026-09-21.** Follow-up to `2026-09-21-bad-assembly-dialect.md` (MVP
shipped same day). Goal: close the gap between "demonstration dialect" and
"a dialect in which a Rollercoaster-Tycoon-scale application is
writable" — Sawyer's game loop, allocator, structs, tables, string math,
FP. Scope locked in session: Tier 1+2+3, both branch sets, `.struct`
directive, FP now, FP branches mirror the integer j-family.

Doctrine: every new op is a config row in `bad-isa.dbvl` /
`bad-registers.dbvl` — zero new Rust match arms for ops. Rust changes are
confined to GENERAL mechanisms (template `$N!` raw-ref escape, width-token
register lookup, comptime expression pass, parser constructs).

## Phase A — Integer-complete ISA (data + two small mechanisms)

1. **Branches** — `jlt jle jgt jge` (signed) + `jlo jls jhi jhs`
   (unsigned). `sym` rows. x86 = cmp+jcc; aarch64 = cmp+b.cond;
   riscv = `blt/bge/bltu/bgeu`.
2. **Logic/shift** — `and or xor shl shr sar` (arity 3), `not neg`
   (arity 2 everywhere; x86 = copy+op so arity stays uniform). x86
   variable shifts hardcode `%cl` (= low byte of r1) — disclosed clobber.
3. **Math** — `mod mulhi mulhiu slt sltu`. `mod`: riscv `rem`, aarch64
   sdiv+msub (scratch `x9` disclosed), x86 `%rdx`. `mulhi(u)`:
   smulh/umulh/rdx.
4. **Sub-width memory** — `ldb ldub ldh lduh stb sth` (sign/zero
   extension on loads). Requires **width register tokens**: new `.w8`,
   `.w16`, `.w32` register rows (x86 `%al/%ax/%eax`-style, aarch64
   `w0`-style, riscv unchanged). Register resolution gains a width
   dimension, defaulting to 64.
5. **Offset memory** — `loadoff storeoff d, base, imm`. x86 displacement
   is BARE (`8(%rcx)`) while imm substitution prefixes `$`; fixed by a
   GENERAL template escape `$N!` = substitute WITHOUT the imm prefix
   (implemented once in `substitute()`, documented in the config header).
6. **FP** — `f0`-`f15` register rows (x86 `xmm0-15`, aarch64 `d0-15`,
   riscv `fa0-7`+`fs0-7`), double precision. Ops: `fmov fadd fsub fmul
   fdiv fneg fabs` + compare-and-branch family mirroring the integer
   j-family: `fjz fjnz fjlt fjle fjgt fjge` + `fcmp itof ftoi`. riscv
   fp-branch borrows `t0` (disclosed). `.double`/`.float` data directives
   pass through already.
7. **Stack pairs** — `push2 pop2`; single `stp/ldp` on aarch64. First
   param = higher address (matches `stp`; documented).

Tests: per-family lowering snapshots ×3 targets, capability errors
(illegal imm forms, unmapped width tokens), `$N!` escape, width tokens,
FP round-trip, push2/pop2 order.

## Phase B — Frontend structure (parser + comptime)

1. `src/backend/bad/comptime.rs` — recursive-descent constant
   expressions: `+ - * / % << >> ()` with precedence, consts, ints.
2. `.const NAME expr` — two-pass (forward refs OK). New
   `BadOperand::Expr(String)`; params inside exprs resolve from the
   expansion env; a register-bound param inside an arithmetic expr = type
   error naming the fix.
3. `.struct Ride / .field name, size[, align] / .end` — sequential
   layout, natural alignment; computes `Ride.name`, `Ride.size` consts.
   Inheritance deferred (noted in dialect doc).
4. Local labels — `.name:` scoped to the enclosing global label; emitted
   as unique `L<parent>__<name>`; cross-scope reference = loud error.
   Disambiguation: `ident:` shape with a leading dot (directives never
   carry a colon).
5. Spans threaded into every lowering/contract diagnostic.
6. Test pinning `.word label` relocation passthrough.

## Phase C — ABI + frames

1. `abi_args` config row — per-target SysV argument-register map
   (x86_64 `r5,r4,r2,r1,r6,r7`; aarch64/riscv64 `r0..r7`).
   `.export name` validates the label's contracts against the boundary.
2. `--with-libc` link mode — dynamic linker + `-lc` so `call malloc`
   resolves.
3. `[frame: N]` contract — static sp-displacement tracking (per-target
   `push_width` row: 8/16/16) proving 16-alignment at calls and a frame
   ≤ N.

## Phase D — Stdlib + tooling

1. `std/bad/` — `memcpy memset strlen strcmp` in pure .bad
   (expressiveness-closure proof).
2. `--trace-lowering` — stderr note per instruction: which exception
   fired / which form picked.
3. Cross-target equivalence test: same program × 3 targets → identical
   branch graph.

## Per-commit gates

`cargo test --lib` green (minus the documented pre-existing ptx fixture
failure), no new warnings in changed files, Praetor clean on
`src/backend/bad` + `src/parser`, docs updated in the same commit as
structural changes (`bad-dialect.md`, SPEC §20.1, highlighter patterns
for `.const`/`.struct`/locals/FP).

## Undo

Each phase is additive rows + isolated modules. Delete the rows/modules,
revert the parser constructs, remove the doc sections.
