# Wave-7 plan — the evented-net issues, and `checkfree` learning to ask the prover (2026-11-20)

**Trigger.** PR #147 (`feat/evented-net`, another live session) surfaced three
issues — #148, #149, #150 — that are not about the network stack at all. Two
are trunk defects (#148 codegen, #149 tooling truth), one needs reproducing
before it is believed (#150). At the same time, the `checkfree` landing left a
recorded follow-up that turned out to be nearly free: the prover **already**
does the interval reasoning, `checkfree` simply never asked.

Owner direction: "Following your rec."

## Ordering rationale

W7-1 → W7-2 → W7-3 → W7-4 → merge #147.

- **W7-1 first** because it is a general codegen defect that blocks any match
  with a diverging arm, and the evented-net branch had to route around it.
- **W7-2 second** because it is small, and it retires a promise the `checkfree`
  landing made ("consumes the proof engine, inherits its upgrades") that the
  implementation did not actually keep.
- **W7-3 before W7-4** deliberately: #150 was *filed against a `policy: deny`
  that no CLI path enforces*. Until the gate is installed everywhere, a CLI
  probe cannot reproduce it. Fixing the tooling gap is what makes the
  investigation possible.
- **#147 last**, on the owner's word, so the branch can drop any workaround and
  land on a trunk whose defects are closed.

## W7-1 · #148 — a diverging arm leaves a dangling phi

**Diagnosis (reproduced on `552f5e144` with the issue's exact snippet).** The
issue blames the phi's predecessor list; that part is correct — a diverging arm
ends `unreachable`, so it contributes no edge. The defect is **stale state**:

- `emit_match` (`src/backend/llvm/emit_expr.rs:10483`) resets
  `self.fun.terminated = false` at the top of **every arm** (`:10603`), then
  never restores the value it had on entry.
- When the **last** arm diverges (`endprogram`, `term`), the phi-merge path
  (`:10765`) returns with `terminated` still `true`.
- The enclosing body loop (`emit_toplevel.rs:5563`) reads that flag and
  `break`s, so the merge block is emitted with a phi, **no terminator**, and no
  continuation; the epilogue `ret` is skipped.

Scope: only the last-arm-diverges shape. The all-diverging path
(`:10682`, `phi_incoming.is_empty()`) sets `true` deliberately and stays.

**Fix.** Capture `self.fun.terminated` on entry; before emitting the merge
block restore it. Mirror `emit_if`'s save/restore discipline (the member-body
path already does exactly this around a nested body). Do not touch the
all-diverging path.

## W7-2 · `checkfree` asks the prover

Today `subscript_proven` (`src/typechecker/mod.rs:5566`) discharges the two
halves with **structural** matchers — `is_upper_bound_fact` (`:5588`) and
`index_is_non_negative_fact` (`:5619`) — which require the guard to name the
index expression as written. Consequence: `when i > 0 { when i <= xs.Count#() {
xs[i - 1] } }` is rejected, because no fact names `i - 1`.

The prover already handles it. Measured against
`callgate::prove_precondition` (facts → goal):

| facts | goal | verdict |
|---|---|---|
| `i > 0`, `i <= n` | `i - 1 < n` | Proven |
| `i > 0` | `0 <= i - 1` | Proven |
| `i <= n - 1` | `i < n` | Proven |
| `i <= n` | `i < n` | Unproven — no off-by-one leak |
| `i >= 0` | `i < n` | Unproven |
| `i < ys.Count#()` | `i < xs.Count#()` | Unproven — collection identity held |
| `i <= ys.Count#()`, `i > 0` | `i - 1 < xs.Count#()` | Unproven |

**Fix.** Replace the two structural matchers with goals handed to
`callgate::prove_precondition` against the site's captured fact list:
upper `index < <recv>.Count#()`, lower `0 <= index`, tried for both count
spellings (`xs.Count#()` and `Count#(xs)` — they are distinct symbols to the
prover). Keep `IndexIn` as the alternative lower-half discharge: its
construction rule supplies `0 <= i` and nothing more, so it must not be allowed
to satisfy the upper half.

Structural equality still proves, because `prove_conjunct_core`'s first move is
equality on the conjunct — so existing behaviour for exact-spelling guards is
preserved, not replaced.

## W7-3 · #149 — one contract policy for every entry

`install_contract_gate_from_config()` (`src/pipeline.rs:1012`) has one caller,
`check_source_for` (`:1049`, the conformance sweep). `check_source_with_options`
(`:992`) and `compile_source` (`src/compile.rs:249`) never install it, so
`policy: deny` is enforced only inside `cargo test`. The policy is a
process-global `AtomicU8` (`src/config_tuning.rs:569`).

Cost observed by the filing session: one full CI round-trip (≈21 min) to learn
something `brievc check` could have said in 40 ms.

**Blast radius, measured.** Installing makes **6 of 83** tier1 fixtures newly
deny: `constants_gate`, `data_verify`, `divergent_loop_attrs`,
`rating_population_gate`, `server_loop_smoke`, `unix_socket_loopback_smoke`.
Tier2: 0. Examples and benchmarks already pass under deny.

**Fix.** Install in the CLI entries that perform acceptance — `check`
(both `--validate` and plain), `build`, `run`, `test` — plus `compile_to_typed`
(the `brievc test` block path), so one policy governs every entry. Then make
the six fixtures honest: prove their call sites. An exemption is allowed only
where the fixture exists to exercise the *absence* of a proof, and must be
explicit and documented — never a silent root exclusion.

## W7-4 · #150 — reproduce before believing

Filed: an obj-method postcondition projects onto the call result; the identical
postcondition on a free defn does not.

**Refuted in isolation on `552f5e144`.** A `DenyGate` probe — free
`claim(sink: Buf, count: Int) [count >= 0 && sink.len + count <= sink.cap]
[term.len == sink.len + count && term.cap == sink.cap] -> Buf`, caller proving
`ext.len == buf.len + got` — **passes**. An independent investigation reached
the same negative result in isolated, cross-module and method-replaced forms.

What genuinely differs between the two spellings is **contract-key resolution**
(`src/typechecker/mod.rs:4121-4140`): `Expr::MethodCall` tries `Type::name`
then the bare key; `Expr::Call` tries the bare key only. Members are registered
only as `"Type::member"`, and `Trait` members are never registered at all. A
callee whose contract lives under a qualified/member/trait key therefore gets
no post facts — and the pre-gate silently skips (`:2211`), which is the worst
shape a gate can fail in.

**Action.** Reproduce inside the #147 branch context with the W7-3 gate live
(`claim` lives in `lib/std/net/bytes.bv`, its caller in
`lib/std/net/evented.bv`). If the key gap is the cause, fix key resolution
generally (typed/param-0 fallback for `Expr::Call`; register trait members). If
it does not reproduce, close the issue with the probe as evidence — the #144
precedent.

## Invariants

| # | Property | Pinned by |
|---|---|---|
| 1 | A value `match` whose **last** arm diverges compiles, and the code after the match executes | `diverging_last_arm_keeps_continuation` (codegen) + the #148 repro run end-to-end |
| 2 | A diverging arm in **any** position leaves the same observable output | `diverging_arm_position_matrix` (first/middle/last) |
| 3 | All arms diverging still emits `unreachable` and no phi | `all_arms_diverging_emits_unreachable` |
| 4 | An enclosing `term`/`endprogram` after a diverging-arm match is not double-emitted or dropped | `diverging_arm_then_term_emits_once` |
| 5 | Interpreter and native agree on every diverging-arm fixture | tier1 golden | 
| 6 | `xs[i - 1]` under `when i > 0 { when i <= xs.Count#() { … } }` is discharged | `checkfree_accepts_offset_index_guard` |
| 7 | Same offset with the guard bounding **another** collection is rejected | `checkfree_rejects_offset_mismatched_collection` |
| 8 | `i <= xs.Count#()` alone does not discharge `xs[i]` (exclusive vs inclusive) | `checkfree_rejects_inclusive_upper_guard` |
| 9 | Every previously-passing `checkfree` test still passes, unchanged | the 15 existing tests |
| 10 | `IndexIn` still supplies the lower half only | `checkfree_index_type_supplies_lower_bound` |
| 11 | `checkfree` remains analysis-only — no IR delta | `tests/checkfree_emission.rs` |
| 12 | `brievc check`, `build`, `run` and the sweep agree on contract denial | `contract_gate_parity_across_entries` |
| 13 | A fixture that must see a denial is not accidentally exempted | `nonexempt_fixture_denies_under_cli` |
| 14 | No fixture is exempted silently — every exemption is named in the config or the test | review + the config parse test |
| 15 | #150 either reproduces (and a test pins the fix) or is closed with the probe recorded | `free_defn_post_projects_fields` (keep-as-regression) |

## Tests to write

- `src/backend/llvm/tests.rs` (or the codegen test module): invariants 1–4.
- `tests/tier1/diverging_arm_match.bv` + `.golden`: invariant 5.
- `src/typechecker/mod.rs` tests: invariants 6–8, plus mutation checks
  (dropping the collection identity check must fail #7; dropping the exclusive
  test must fail #8).
- `tests/contract_gate_parity.rs`: invariants 12–13.
- `src/typechecker/mod.rs` (kept from the probe): invariant 15.

## Documentation

- `BUGS.md`: #148's stale-flag mechanism (the issue names the wrong cause and a
  reader will otherwise chase the phi).
- `docs/architecture/backend-contracts.md`: the `terminated` invariant —
  whoever owns it must restore it.
- SPEC §15.4b: the offset discharge, and that a bound is still not an index.
- `docs/plans/2026-11-20-checkfree-obligation.md`: follow-up A marked landed,
  B ruled out (no total construction source), C re-scoped (no monomorph pass
  exists in the tree).
- `src/pipeline.rs` doc comment corrected: the parity claim it makes today is
  false until W7-3 lands.

## Non-goals

- Merging PR #147 without the owner's explicit word.
- A true index type (`in` = index): needs a fallible constructor; no total
  construction source exists for a growable `List`.
- SMT delegation for `checkfree`: the SMT backend emits `BitVec 64` only, no
  linear integer arithmetic — the in-process prover is the stronger engine here.
