# .bad — Ergonomics + .bv Integration

**2026-09-21.** Follow-up to the application-grade + hardening trains
(same day). Three moves, all decided in design session: universal `;`,
friendly mnemonic aliases default-on via the prelude, and first-class
`bad` definitions in `.bv` replacing `asm<Target>` (AsmFn) entirely.

## Decisions (locked in session)

| Decision | Choice |
|---|---|
| `;` | Universal instruction separator — a line is a sequence of instructions, in every context (label bodies, defn bodies, branch rows, exception rows, `.bv` bad bodies). The old split (worked in branch rows, errored in label bodies) was an accident of the grammar. |
| Friendly aliases | Mnemonic aliases via the extended `alias` mechanism, shipped as `std/bad/friendly.bad`, **loaded by default** (prelude), `brievc bad --raw` opts out. Raw names always work — aliases are additive. `Start` is NOT aliased: `_start` is the one genuinely universal name and stays raw. |
| Positional contracts | `[pre: ...]` / `[post: ...]` keywords REMOVED. One bracket group = postcondition (implied, matching `.bv` function contracts); two groups = pre, then post. `[frame: N]` keeps its keyword (different proof kind). `[true]` = explicit empty slot. |
| `bad` in .bv | `bad name(params) -> Ret [post] [pre] { <bad body> }` — body = real .bad grammar (aliases active, `;` universal). Trailing bracket = implied postcondition as a full **Briev** expression over params and `result`; leading group = precondition. Braces belong to .bv (Rule 21); between them .bad rules hold. |
| Value flow | Params bind PER TARGET through the `abi_args` map (`p` is rdi/x0/a0). Int ↔ r-regs, Float ↔ f-regs (new `abi_args_fp` row), Ptr ↔ r-regs (address as value; .bv borrow checker proves validity caller-side). |
| AsmFn | `asm<Target>` node, parser path, and LLVM inline-asm emission REMOVED — zero user-facing uses exist (verified by grep). `AsmLowering`/`Asm#` intrinsic is a different mechanism and stays. |
| Entry | `_start` raw + `global _start` — universal ELF convention, no alias, no keyword. |

## External critique (2026-09-21, integrated)

An architectural review of the park.bad example raised four points;
triaged against the mission (**contract-verifiable low-level
optimization for Briev**, WYSIWYG hand-scheduled asm — NOT an IR):

1. **ABI/stack-arg leak — ACCEPTED.** `LoadOffset r8, sp, 8` hardcodes
   the x86_64 SysV layout; on aarch64 arg 7 arrives in `x6` (a
   register). Fix: the `Arg dst, n` core op materializes the Nth
   argument per target — register move when `n <= abi_reg_args`,
   `loadoff` at `abi_stack_arg_base + (n - reg_args - 1) * 8` beyond.
   Fully config-driven (`abi_args` + `abi_reg_args` +
   `abi_stack_arg_base`); one general routine, zero target knowledge.
   Raw `target =>` stack offsets remain the low-level fallback.
2. **Zeroing exception is decorative — PARTIALLY ACCEPTED.** The
   assembler stays WYSIWYG (no hidden peephole pass — hand-scheduled
   code means what you write is what runs; an auto-xor pass is a
   surprising transformation in an optimization context). But xor-zeroing
   IS decorative as an exception (core has `Xor`), so the example now
   demonstrates exceptions with a genuinely inexpressible win: ×constant
   via `Lea` (x86) / shifted-add (aarch64) where the core `Multiply`
   costs imul latency. riscv64 rides universal — the teaching point
   stands: exceptions are per-target opportunity, never obligation.
3. **Call/Return on inlined defns — ACCEPTED, three real bugs.**
   `Call ChargeGuest` emits a call to an undefined symbol (defns are not
   labels — invocation is at the INSTRUCTION position: `ChargeGuest r1`);
   a `Return` inside an inlined defn exits the ENCLOSING function (legal
   but mid-function use is a footgun — documented); `.local` labels did
   not parse inside defn bodies, and double invocation would collide
   them. Fix: locals allowed in defn bodies with **hygienic per-call-site
   gensym** (`L<defn>__<local>__<n>`); references rewrite through the
   same scope.
4. **Virtual registers — REJECTED BY DESIGN.** .bad is WYSIWYG
   hand-scheduled asm; a register allocator would make it a compiler and
   betray the mission (YOU schedule registers; the config table maps
   them). r14/r15-absent-on-x86_64 style existence errors are the
   feature.

## Phases

### Phase 1 — universal `;`
Parser: the shared instruction-line entry splits on top-level `;` in
every body context. Contracts/exceptions bind to the LAST instruction
of a split. Test: the same packed sequence in a label body, a defn
body, and a branch row lowers identically.

### Phase 2 — alias mechanism + friendly sheet
1. `alias Move = mov` (mnemonic) and `alias Start = _start`-style label
   aliases: resolution order register → mnemonic → label. Mnemonic
   aliases resolve at lowering (before the ISA table); label aliases
   at emission and reference sites.
2. `std/bad/friendly.bad`: the full sheet — Move, Add, Subtract,
   Multiply, Divide, Load, Store, LoadByte, LoadUnsignedByte, LoadHalf,
   LoadUnsignedHalf, StoreByte, StoreHalf, LoadOffset, StoreOffset,
   Jump, JumpIfZero, JumpIfNotZero, JumpIfLess, JumpIfLessEqual,
   JumpIfGreater, JumpIfGreaterEqual, JumpBelow, JumpBelowEqual,
   JumpAbove, JumpAboveOrEqual, Compare, Call, Return, Push, Pop,
   Push2, Pop2, NoOp, SystemCall, Halt, Addr, Arg, FloatMove,
   FloatLoad, FloatStore, FloatAdd, FloatSubtract, FloatMultiply,
   FloatDivide, FloatNegate, FloatAbsolute, FloatCompare,
   FloatJumpIfZero, FloatJumpIfNotZero, FloatJumpIfLess,
   FloatJumpIfLessEqual, FloatJumpIfGreater, FloatJumpIfGreaterEqual,
   IntToFloat, FloatToInt. (No `Start` — `_start` is universal.)
3. Default load: the lowerer expands the sheet as a virtual import
   unless `--raw`. `--trace-lowering` prints canonical mnemonics.
4. **`Arg dst, n`** — materialize the Nth C-ABI argument (critique fix
   #1): register move within `abi_reg_args`, stack `loadoff` beyond.
5. **Defn hygiene** (critique fix #3): `.local` labels legal in defn
   bodies; every expansion renames them `L<defn>__<local>__<callsite_n>`
   and rewrites branch references through the expansion scope. Defn
   invocation is at the INSTRUCTION position (`ChargeGuest r1`); a
   `Return` inside an inlined defn is the enclosing function's return
   (documented footgun).

### Phase 3 — positional contracts in .bad
Label contract groups lose their keywords: `[a]` = post, `[a] [b]` =
pre then post. `frame:` keeps its keyword. Parser + contract checker +
tests.

### Phase 4 — `bad` in .bv, AsmFn migration
1. .bv parser: `bad name(params: Type, ...) -> Ret [groups] { body }`
   → `TopLevel::BadFn { name, params, ret, briev_contracts, body }`
   (body kept as source text).
2. Pipeline: BadFn lowers through the bad backend per the .bv build's
   target → `.s` → object → linked into the binary; LLVM references
   the symbol via `declare`.
3. Params bind via `abi_args` (+ new `abi_args_fp` for Float);
   the implied post = the trailing Briev contract, checked at call
   sites by the existing contract machinery; body register contracts
   checked by bad rules (synthesized label contract from the Briev
   groups where representable).
4. AsmFn removal: node, parser, emission, analysis references.
5. Integration test: `.bv` program with `bad` fns (Int + Ptr), cc/qemu
   path where available.

### Phase 5 — example + docs
`examples/bad/park.bad` (the design-session showcase, runnable), docs
(bad-dialect.md grammar/alias/positional sections), SPEC §20 rewrite
(bad replaces asm<Target>), highlighter aliases. Full-suite gates.

## Undo

Additive parser arms + one node removal; delete the sheet, the alias
plumbing, the BadFn node and its emission; revert the contract grammar.
