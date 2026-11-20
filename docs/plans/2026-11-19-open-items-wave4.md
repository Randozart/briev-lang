# Open-item wave 4 plan (2026-11-19 evening)

**Scope.** Everything visible on remote at plan time: trunk CI red on
`09a38e8f7`, PR #145, issue #137, the G2 promotion queue (8 sites), and the
feature-coverage census queue (122 uncovered sections). Ordered by trunk
health first, then the oldest open issue, then the standing queues.

## Ownership line (read first)

A concurrent session owns `feat/stdlib-checksum` (their branch: 4 repair
commits ahead of my F5 push) and is actively editing `helpers.rs` /
`intrinsics.rs` / new stdlib in the **same worktree**. Wave 4 touches their
broken artifacts (`checksum.bv`, one golden) — before each commit, `git
fetch` + check whether their branch already landed the repair on main; if
their fix appears, skip the item and rebase the wave around it. All wave-4
work lands from a detached temp branch via `git push origin HEAD:main`
(worktree protocol), never by switching this checkout.

## A. Trunk repair — unblock CI on main (P0, small)

`09a38e8f7` is red for two reasons, both mechanical:

1. **`checksum.bv` contract ladder.** The sweep failure is the call
   `mod11_scan_digit(scan.sum, scan.digits, width, b - 48)` inside
   `mod11_scan_byte`, which declares **no contract slots at all**, so the
   callee's pre (`sum >= 0 && pos >= 0 && width >= 2 && 0 <= digit <= 9`)
   is unprovable. Repair (contract-first, per the house rule — fix the
   code, not the contract):
   - give `mod11_scan_byte` a precondition `[width >= 2]` (callers pass
     the literal 9, provable at `bsn_mod11`/`iban` paths);
   - make the digit range a **literal path fact**: replace the
     `when checksum_is_digit(b) == 0 { term scan; }` guard with a direct
     range guard `when b < 48 || b > 57 { term scan; }` — then
     `(b - 48) >= 0 && (b - 48) <= 9` holds by arithmetic on the
     fall-through path (the callee-post ⟹ range leap the prover cannot
     make today disappears);
   - `scan.sum >= 0` is inductive through `mod11_fold`'s post
     (`term >= 0`); if the prover still cannot close it, seed the field
     fact by initializing `sum` via a contracted path (it starts at the
     literal 0) — measure before adding anything heavier.
2. **`coll_request_arena.golden`** — strip the trailing newline (restore
   the F3 byte-pin; the binary prints no NL).

Gate: `cargo test --lib conformance::tests::conformance_sweep…` +
`daily_use_smoke::tier1_corpus` locally, then push; CI on the repair
commit must be green before anything else merges.

**Pre-check:** if `feat/stdlib-checksum`'s repairs landed on main in the
meantime, verify and skip.

## B. PR #145 — review and merge (P1, small)

`fix/divergence-willreturn` (`8c34d3117`): 18 checks pass, 1 SKIPPED (fuzz
matrix), 0 failed — the "failing" status was a stale rollup. The change
itself: a defn on a recursion cycle must not claim LLVM `willreturn`
(a non-returning recursive defn claiming willreturn is a miscompile-risk
for the optimizer). Review: (1) read the diff for scope (additive guard or
a general cycle analysis?); (2) confirm the diagnostics/BUGS trail; (3)
merge once main is green again (A first — merges into a red base are how
this mess started). Update the PR description if the premise wording needs
correction (the #140 lesson: never merge under a false premise).

## C. Issue #137 — Float32 storage width in the interpreter (P2, the real design task)

**Requirement** (from the issue): `(1.5 as Float) as Data` must produce the
binary32 word when the value's declared storage is Float32, through
bindings, calls, returns and arithmetic — recognizing only syntactically
nested `as Float32` is explicitly called out as insufficient.

**Design decision (the wave's main one): introduce `Atom::Float32(f32)`.**

- Today `Atom::Float(f64)` is the only float; the raw-word door at
  `eval.rs:172/199` does `number.to_bits()` (f64) — correct for Float64,
  wrong for Float32. 24 `Atom::Float(` sites; ~30 mechanical arms.
- Width information must come from **static type positions** — the
  interpreter has no type hints today (`eval_expr` is expr-only). Thread
  `expected: Option<&Type>` through the few sites that have static types:
  `let` annotations, fn params, cast targets (`Expr::Cast` already carries
  `ty`), returns. At a Float32-typed position, produce/store
  `Atom::Float32`; arithmetic between Float32 values stays Float32 (f32
  math), Float32 ⊕ Float64 promotes to Float64 (C-like usual conversions,
  matching the backend's `adapt_arg_to_param`/`fpext` behavior).
- The raw-word door then splits:
  `Atom::Float32(f) => Value::Data(f.to_bits() as i64 as u64)` (binary32
  word, `1069547520` for 1.5 — the issue's oracle) vs
  `Atom::Float(f) => f.to_bits()`.
- **Non-negotiables from the issue:** cover Float32 and Float64, both cast
  directions, named/returned operands (the type-hint thread is what makes
  these work — a bare syntactic check would not), negative zero and NaN
  payloads (`-0.0 as Float32 → 0x80000000`, NaN payload preservation where
  the language preserves it — pin whatever the backend does).
- **Test fixtures:** the issue's own repro as a `test` block + a tier1
  fixture with golden (`1069547520`), plus `-0.0`/NaN round trips and a
  Float64 control (native == interpreter parity pinned, per Rule 5).
- Watch the pull-tabs: `as_i64`/display arms (`float_to_str`) must print
  Float32 values without double-rounding artifacts (format f32 directly,
  not via f64 re-parse).

## D. G2 promotion (P3, blocked on migration)

Census now says **8 checkpoints / 0 trailing** (server_loop added one).
Promotion order per the wave-3 decision: migrate the sites to the exit
spelling (`term! ->`), flip the warning to an error, keep the census
script as the guard. Only after A (trunk green) so the migration diff is
the only moving part.

## E. Coverage program continuation (P3, incremental waves)

The census queue (122 uncovered) in priority slices — each slice = census
delta + fixtures with goldens + parity:

- **Wave E1 (user-facing reactive surface):** §9.6 cells, §12.2 spawn/await,
  §11.6 critical sections — these have real runtime semantics and zero
  fixtures.
- **Wave E2 (data/expression surface):** §16.5 slicing advanced forms
  (negative stop/step — the new exclusive-stop helper needs negative-step
  coverage; the interpreter supports it, the native gather does not —
  likely the NEXT parity bug), §17.1/17.2 reflection, §14.4 effects.
- **Wave E3 (compile-time surface):** §18.2 expansion, §18.4 `$txn`, §19.8
  export, §13.1 MMIO pins, §22.x data-briev (some have dedicated tests —
  cross-link via `// spec:` tags rather than new fixtures).

Note the E2 lead already in hand: `slice_indices` (interpreter) supports
negative step/stop; `briev_slice_strided64` clamps `step >= 1` — negative
strides are an **open native gap** to either implement or diagnose with a
helpful error, never silently mis-slice.

## Sequencing

A (trunk green) → B (merge #145) → C (#137, the multi-hour item) → D+E in
parallel slices. A and B are tonight-sized; C is a session-sized design
task; D/E are incremental.

## Success criteria

- CI green on the A commit; #145 merged with a corrected premise.
- #137 closed with the issue's own repro passing and the parity fixtures
  pinned (Float32/Float64, ±0, NaN, named/returned operands).
- G2 error-flip landed with 0 checkpoint sites in the census.
- Census coverage climbing per wave with every new fixture golden-pinned
  and dual-engine agreed.


## Execution record (2026-11-19 night)

All five items landed, pushed, and CI-verified:

- **A** `11288520a` — checksum contract ladder (truthful fold posts
  `sum±900`, `pos` threaded as a parameter, literal digit-range guards)
  + golden NL strip. Sweep green; BSN/Luhn 4/4 via `brievc test`.
- **B** PR #145 merged (`23932463c`); regression pin `f393e0ea8`
  (tier1 fixture + `divergent_cycle_defns_lack_willreturn` IR-attr
  test + BUGS.md account incl. the knooppunt routing_service
  connection from #144). `logging_smoke.golden` NL repaired too.
- **C** `c4a843df5` — #137 closed. `Atom::Float32(f32)`;
  declared-width coercion derived from the universe row at the four
  static positions; width-preserving binop/div/neg; the fuzzer's
  float model rounds per-op to binary32. Fixtures both engines; the
  issue's 1069547520 oracle green on both. Bite-proven. The parser
  collapses `Float32` -> `Float` at the type level, so a syntactic
  fix was structurally impossible — the universe-derived width was
  the only correct channel.
- **D** `39d4155d7` — G2 promotion re-scoped by evidence: bare `term;`
  IS the only checkpoint spelling (term! -> ends the program), every
  corpus site is deliberate, so promotion to error is a keyword-
  taxonomy language decision, not a migration. Landed the 0-trailing
  census gate (tests/g2_census_gate.rs delegating to g2_census.py)
  + two diagnostic-semantics pins in termination.rs. Promotion
  re-opens only with an explicit checkpoint spelling.
- **E** `940c64fbb` — E1 wave: 12.2 spawn/await and 11.6 critical
  sections now golden-pinned dual-engine (census 21 -> 23/143). The
  spawn fixture's first draft was REJECTED by the engines' sharp
  segment rule (`let` before `yield;` invisible later — SPEC-stated);
  the landed form is the parameter-carried shape both agree on.
  9.6 cells recorded as a REAL gap: the grammar parses cells but
  port-firing inside a cell body is not wired (port name not in
  scope) — a knooppunt-ledger finding, not a paper-over.

Post-wave trunk: CI 15/15 on c4a843df5; coverage floor 23/143 with
the census queue (120 sections) as the standing work queue; open
issues down to #4 (macOS) and #2 (C static library) — both
external-host/feature-scope.
