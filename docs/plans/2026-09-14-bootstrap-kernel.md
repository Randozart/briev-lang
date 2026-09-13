# Bootstrap + Machine-Entry Syntax — Kernel Phase 4 Plan (2026-09-14)

**Status**: APPROVED (design closed 2026-09-13/14 in session; user sign-off on
handoff contracts, wiring rules, fallback). Worktree `briev-rv64-capability`,
branch `feat/rv64-capability-kernel`.

## 0. Goal

Complete the rv64 capability arc: a micro-kernel demo — two user-mode tasks,
`ecall` syscall boundary, timer-preemptive scheduling — in pure Briev, with
zero asm in user code and zero machine-specific keywords beyond one. The
design was shaped by three demands: no systems DSL (grounded in a C/C++/Rust/Zig
survey), 100%-unambiguous keyword semantics, and asm confined to its two honest
layers (compiler-owned ISA scaffold; typed register-access shim).

One sentence: **entries are declared, machines wire them, compilers own ABI,
users own policy.**

## 1. Design of record

### 1.1 `bootstrap node` — the authored program entry

```briev
bootstrap node reset [armed == false] {
    wire_trap_vector();
    interrupts_enable();
    timer_arm(TICK_INTERVAL);
    armed = true;
};
```

- **Syntax**: `bootstrap node <name> [<handoff-postcondition>] { body };`
  One bracket group only (postcondition). A `[pre][post]` form is a
  typechecker rejection (nothing fires it but the machine).
- **Body**: ordinary typed Briev. Machine facts via `Asm#` expressions or
  shim calls — never raw register text.
- **Emission**: the compiler-owned ISA scaffold wraps the body —
  `sp ← _stack_top`, `.bss` zero, body, hand off to the reactor. Emitted as a
  naked function placed in `.text.start` (the entry symbol; QEMU virt's
  `-bios none` enters at the start of RAM, not `e_entry`).
- **Handoff contract**: the postcondition is over program state at reactor
  handoff, proven from the body's typed stores + the state initializer
  (constant evaluation — the solver's existing proof). The scaffold's machine
  behavior is compiler-owned ABI, the same trust class as any prologue.
- **Fallback**: the canned `_start` emission remains when no bootstrap is
  declared. Extraction of the canned sequence to board/stdlib is Phase 5+
  follow-up.
- First-declared bootstrap is the reset entry. Always a liveness root; never
  inlined or cloned; excluded from reactor dispatch and the concurrency gate.

### 1.2 `node @ vector` — machine-serviced events

```briev
node trap_service @ timer_irq [ticks >= 0] {
    match trap_cause() {
        7 => schedule(),
        8 => syscall_dispatch(),
        _ => {}
    };
};
```

- **Syntax**: `node <name> @ <vector-name-or-number> [<pre>][<post>] { body };`
  The `@` reuses Briev's hardware-association delimiter (MMIO addresses,
  `trg` pins); namespace (addresses vs interrupts) comes from the board files.
- **Mechanism inference**: the active target profile's `isr_mechanism` field
  names the mechanism row (the fallback path already exists —
  typechecker/mod.rs:5310). The explicit `isr<riscv_machine>` override is
  retired; multi-mechanism targets are tier-3 (documented deferral).
- **Convention scaffold**: the mechanism row's convention supplies the entry
  scaffold. New `full_context` field: save x1–x31 + sp → kernel stack → body →
  restore → `mret`. The existing interrupt-attr convention (clobbered-save)
  remains for simple handlers.
- **Vector identity**: board `interrupts.dbvl` names (`timer_irq`) or literal
  numbers; `resolve_isr_vector` already resolves board names.
- **Semantics**: machine-fired (excluded from reactor dispatch and the
  concurrency gate), always a liveness root, ordinary state contracts
  (solver-checked; delegation obligations live on called txns). The `isr`
  keyword is fully dissolved — this syntax replaces it.

### 1.3 The trap frame — the context switch as typed memory

`@__briev_trap_frame` — compiler-owned global (convention family of
`@__briev_state`), documented layout: x1–x31, sp, pc. The scheduler's context
switch is typed `VolatileLoad#/Store#` into the frame + `mepc`; the scaffold's
restore path resumes whoever the frame names. Frame layout is pinned ABI —
documented in `docs/architecture/machine-entry.md`.

### 1.4 The register shim — all the asm that remains

`lib/kernel/rv64-machine.bv`: ~6 typed one-liners (`mcause()`,
`set_mepc()`, `advance_mepc_past_ecall()`, `wire_trap_vector()`,
`interrupts_enable()`, `timer_arm()`, task-side `ecall()` wrappers). The
`riscv::register` pattern from the survey. One disclosed necessity:
`wire_trap_vector` references the handler symbol inside its template
(`la $0, trap_service`) — symbol-in-template is a pinned convention
(`@txn_<name>` family).

### 1.5 What retires

| Piece | Successor |
|---|---|
| `naked` (never added) | not needed — bootstrap + convention scaffold subsume it |
| `isr` keyword (+ `<mechanism>`) | `node @ vector` + profile inference |
| canned `_start` as the only reset | `bootstrap node` (canned = fallback) |
| `beginprogram` as mechanism | optional sugar — handoff state kicks the first node |
| asm in kernel logic | shim library + compiler scaffolds |

## 2. Phases

- **A. `bootstrap node` keyword** — lexer token, parser (prefix form, like
  `sync<…> node`), AST (transaction modifier annotation), typechecker
  (single-bracket form; handoff postcondition proven from body stores;
  rejections with what/why/fix), emitter (scaffold + naked `.text.start`
  entry when declared; canned `_start` otherwise; body inlined at main's
  head after init stores, before the dispatch loop). Gate: emission-shape
  tests (scaffold present, body before loop, entry symbol), rejection tests,
  full suite green.
- **B. Profile + board data** — `[target.riscv64-unknown-none]` row with
  `isr_mechanism = "riscv_machine"`; `lib/boards/qemu-virt-rv64/
  interrupts.dbvl` (`timer_irq = 7`, `software_irq = 3`). Gate: mechanism-
  less declaration compiles; the "no [target] entry" warning is gone for
  riscv64.
- **C. `full_context` convention + service scaffold + trap frame** — registry
  field (data), emitter arm (additive), frame global + documented layout.
  Gate: emitted stub saves x1–x31+sp, calls body, restores, `mret`; existing
  isr tests green; frame-bound contract still enforced.
- **D. `@ vector` node syntax + `isr` migration** — parser (node header
  vector wiring), typechecker (mechanism inference via profile, machine-fired
  classification, gate exclusion), emitter routing to the service path,
  liveness roots. Migrate existing isr tests/examples/SPEC §13.2. Gate:
  migrated suite green; hello_rv64 + timer_rv64 still boot in QEMU.
- **E. Kernel `.bv` + demo + gate** — `lib/kernel/rv64-machine.bv` shim;
  `examples/kernel_rv64.b.bv` (reset, trap_service, schedule, syscall table,
  two U-mode tasks); `tests/bare/qemu-rv64-kernel.sh` golden gate
  (interleaved `ABAB…` via ecall-write, timer-preempted). Gate: script pass.
- **F. Docs sweep** — execution-model doc (bootstrap/beginprogram layering),
  AGENTS.md reference row, spec highlighter, QUICK-REFERENCE, this plan's
  addendum with results.

Per-commit: `cargo test --lib` green, no new warnings, Praetor on changed
directories, continuous commits.

## 3. Rules compliance

- **Rule 2**: no strategy keyword needed for capability — `bootstrap`/`@`
  express intent; scaffolds are automatic.
- **Rule 3**: all special treatment disclosed (keyword, `@`, shim, ABI
  scaffolds documented).
- **Rule 6 (additive)**: new emitter arms; canned `_start` path unchanged as
  fallback; existing isr path migrates in Phase D with behavioral tests
  preserved.
- **Rule 7/9**: each phase ends at its gate; every emission arm tested.
- **Rule 14**: kernel policy (dispatch, schedule, syscalls, tasks) lives in
  `.bv`; compiler keeps only ISA ABI (scaffolds, frame) — the `_start`
  precedent.
- **Rule 21**: `@` = hardware association; namespaces (addresses vs
  interrupts) are board-file scoped — one-line spec note.
- **Rule 13**: SPEC §11.5/§13.2 + delimiter note updated with the syntax
  change (this session, before implementation); arch doc
  `docs/architecture/machine-entry.md` is the ABI/convention record.
- Not a performance plan — no benchmark table (Rule 12 scope); QEMU gates
  stand in.

## 4. Risks / undo

| Risk | Mitigation |
|---|---|
| Service scaffold vs caller-saved regs (U-mode entry) | full save-all; U-entry = mepc/mstatus set in typed body; frame layout pinned + tested |
| Frame ABI drift | layout constants in one emitter function + arch doc; scheduler reads via accessors |
| Migration churn (isr → @vector) | behavioral tests preserved; mechanical syntax move; SPEC §13.2 same commit |
| Handoff contract vs init-store ordering | bootstrap body inlines AFTER main's init stores — its stores win; ordering tested |
| Bootstrap body needing state before init | body runs after init stores by construction; rejection test covers state-typed use |

Undo per phase: A (revert keyword + emitter arm), B (data-only), C (revert
registry field + arm), D (revert parser arm; isr syntax restored), E
(example-only). No cross-phase coupling beyond the documented order.

## 5. Provenance

- Authored 2026-09-14, agent session (opencode), from the design
  conversation of 2026-09-13/14 — user-directed: no systems DSL (C/C++/Rust/Zig
  survey), 100%-unambiguous semantics, "why do we even need asm", "is isr<>
  even required — can the compiler infer this".
- Companion docs: `docs/architecture/machine-entry.md` (ABI/convention
  record), `docs/architecture/briev-execution-model.md` (reactor model),
  `2026-09-11-rv64-capability-kernel.md` (Phases 0–3, Addendum D),
  `2026-09-13-defn-liveness-emission.md` (emission gating this builds on).

---

## Addendum A: The refined paradigm (2026-09-14, same session — pre-execution)

Four refinements, closed in design review before Phase A landed. Each traces
to a user challenge; each shrinks or corrects the base plan.

### A.1 Universal `@` wiring — the inline trigger

`@` on a node header is ONE pattern with TWO dispatch classes, resolved by
the board-file namespace of what follows:

| Wiring | Class | Dispatch | Wake source |
|---|---|---|---|
| `@ timer_irq` / `@ 7` (interrupts namespace) | machine-vectored | machine preempts; convention scaffold; never reactor-dispatched | the interrupt line |
| `@ thermal_alert` / `@ 0x0200BFF8` / `@ *ptr` (addresses namespace / pointer) | reactor-pass | stays in reactor dispatch, gated by the wiring (existing trigger machinery) | the reactor's pass |

Dynamic `@ *ptr` is always reactor-pass (mtvec needs a static handler).
Ambiguous names → compile error naming both namespaces. Wired nodes may omit
`[pre]` — the wiring IS the eligibility. `trg` coexists (named reusable
binding); inline-`@` is the point-of-use form.

### A.2 Latency contracts — "polling" reframed

Address wires cannot fire-on-write without an interrupt line; the honest
mechanisms are the reactor's pass (fires within one pass) and, at
equilibrium, declared-bound quantum parks. `within <bound>` (the existing
watchdog deadline syntax) on a wired node declares the latency contract; the
compiler picks spin or park-with-quantum to meet it. Vectored nodes take no
`within` — their latency is the hardware's.

### A.3 Wake-dependency analysis + frontier-driven equilibrium

New frontend analysis (`AnalysisResults.wake_sets`): per wake source, the set
of preconditions that reference fields that source can affect — from typed
writer sets (the `build_write_masks` precedent) × precondition reader sets.
**Conservative by construction**: a field written anywhere live is
may-written for any wake reaching that writer; MMIO-pinned/external fields
are permanently in the external frontier. A missed writer = silent deadlock,
so over-approximation is mandatory; under-approximation fails loudly at
compile time (emitter asserts the park policy against the computed frontier).

Frontier-driven equilibrium (replaces the Phase 3 unconditional `wfi` park):

| Program frontier | Equilibrium behavior | Re-check on wake |
|---|---|---|
| state-sequenced only | direct fallthrough — fold machinery chains provably-next nodes, no re-check | none needed |
| vectored only | `wfi` park | dependent set of the trap's writes only |
| external (address wires) | spin (default — fires within one pass) or quantum park when `within` declared | external frontier's set + state-sequenced fallthrough |

**BUG (logged BUGS.md 2026-09-14)**: the Phase 3 equilibrium parks in `wfi`
unconditionally — a program with an address-wired node would sleep through
eligibility (the value changes, no interrupt asserts, nothing wakes the
CPU). Phase C/D replaces it with the frontier-driven park above.

### A.4 Corrections of record

- **Handoff postcondition = state AFTER the body** (at reactor handoff). The
  base plan's sketch `bootstrap node reset [armed == false]` with body
  `armed = true` was wrong under its own semantics; the correct form
  documents the post-body state (`[armed == true]`). The convergence
  checker's existing reachability proof applies unchanged.
- **`beginprogram` remains first-class sugar** — genuinely useful on native
  targets where the environment provides initial state (user correction);
  authored-bootstrap programs typically omit it.
- **Phase D reshaped** to the unified wiring (A.1): the parser accepts
  `@ wiring` on any node header; classification by namespace; the vectored
  class routes to the mechanism scaffold, the polled class to the existing
  trigger tables.

### A.5 Phase A status

Keyword landed (lexer/vocab/parser/gate/typechecker/emitter threading).
Constraint: authored machine entries currently require the direct-SSA
dispatch — other dispatch paths reject loudly with the fix (guard helper
`reject_bootstrap_off_ssa`). Handoff postconditions ride the existing
reactive convergence proof (no new checker machinery); bootstrap nodes take
`ContractKind::Optional` (no handoff obligation declared = allowed; written
`[true]` still rejected as tautological).
