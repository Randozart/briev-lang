# Term semantics, tails, output inference, observability warning, diagnostics blocks

**2026-10-16.** Confirms and supersedes the tired-session parked proposal
`docs/plans/2026-10-15-term-tails-proposal.md` (that file stays untouched as
history — timestamped records are never retroactively edited). Rested
decisions from the 2026-10-16 discussion; every item below was reasoned to a
close with the maintainer, not inferred.

## 1. P-gated bare `term;` — uniform (Proposal 1, confirmed + refined)

A bare `term;` is the item asking "am I done?": the postcondition is
evaluated AT the term site.

| Site | P met | P unmet |
|------|-------|---------|
| bare `term;` in a **Void defn** | return void | continue the body |
| bare `term;` in a **Void txn/node** | stop firing | the loop continues (this firing completes; the reactor re-fires next cycle) |
| bare `term;` in a **value** defn/txn | — | **typecheck error**: "term needs the value" |
| `term v;` in a defn | return v (unchanged) | n/a — defn value-terms are not P-gated |
| `term v;` in a txn/node | stop firing (store → gate, unchanged) | the loop continues (unchanged) |

Rules:

- No postcondition ⇒ P trivially met ⇒ unconditional return / stop firing.
- The P of a **Void** item cannot reference `term` (typecheck error) — a
  Void item's P is over observable state.
- `term` always addresses the nearest enclosing contract-bearing item
  (txn/node/defn). Inside a `when` guard or a match arm it targets that
  enclosing item — when/match are transparent; they have no term
  semantics of their own.
- Terminology note: the unmet path in a txn is "the loop continues", in
  SPEC's reactor words ("it fires again on the next reactor cycle") —
  not a new loop construct.

Rationale for the asymmetry (bare P-gated, value-term unconditional in
defns): `term v;` *answers* ("here is the value" — the question is
settled); bare `term;` *asks*. P-gating defn value-terms would risk
"P unmet → continue → body ends without a value", a path no body verifier
covers today. In txns, `term v;` already P-gates (store → gate →
exit-or-reloop); that behaviour is untouched.

### Divergence resolved

BUGS.md's open bare-`term` divergence (backend: defn bare-term lowers as
early-return, guard inversion; interpreter eval.rs:2447: continue) is
resolved BY this rule: both sides become P-conditional. The interpreter
test is the divergence entry.

## 2. Tail expressions — SPEC-as-written (Proposal 2, confirmed)

- Scope: **defn bodies and match-arm blocks only**. No tails in `when`
  (an execution block, never a value producer), none in txn/node bodies
  (convergence bodies spell `term` explicitly).
- Body-final expression without `;` = the block's implicit value — an
  **unconditional** value return, NOT P-gated (SPEC §11.3: "a bare tail
  expression produces a value").
- AST: a distinct `Statement::Tail(Expr)` parser-sugar node — never
  user-spelled, no keyword. Cannot desugar to `Term(Some(e))` because
  Term is now P-gated in txns and tail must not be.
- Value tail in a Void body = typecheck error (Rust `()` discipline).
- Mid-body missing `;` stays a syntax error.

## 3. Output-type inference (arrowless)

| Item | Rule |
|------|------|
| `defn` | `-> T` explicit; **or** arrowless + value term = return type inferred (unchanged — `fs_open` et al.); **or** arrowless + no value term = **Void** |
| `txn` | same hybrid (parser already shares `parse_output_type`; the inference path is verified for value txns and mirrored if missing; array.bv's arrowless accumulate txns stay Void) |
| `node` | always Void — no output surface |

SPEC: replace the stale "defns cannot be void" parenthetical (SPEC:2398)
with this rule set (Void defns exist today — `tests/dir_smoke.rs:97`
`-> Void`).

## 4. Observability warning — effectless Void defns

A Void defn whose transitive effect closure is empty — no observable
intrinsic (`Print#`…), no frgn call, no `out`-marked call, no
trap/halt/endprogram, no state write (if the purity probe finds state
writes legal in defns at all) — and which is not itself `out`/`export`/
`frgn`, gets a **warning** (not an error; liveness already silently drops
these — this surfaces the silence, warn-the-default per the keyword
taxonomy):

> what: this defn has no observable effect — calls to it are no-ops and
> it is never emitted. fix: delete it, give it an effect, or mark
> `out defn` if the call boundary must survive for an external reason.

`out` already exists (plan `2026-08-04-out-observability-and-native-
stdlib.md`, implemented) as the observability pin — the warning points at
it. Emission is composite (§5): one block listing every site.

Open fact (verified during implementation): can defns assign state?
Write-set machinery is txn/node-centric (premise: no); a 3-line probe
settles it; the effect set grows if the probe says legal.

## 5. Diagnostics block layer (`src/diagnostics.rs`)

Today: ~235 scattered `eprintln!` sites + `println!` info notes with no
central type and no verbosity flag — console clog. New core:

```
warning: <short what>
  what: ...
  why:  <proof / analysis, where one exists>
  fix:  <concrete fix>
  sites:
    path:line:col  <one-line context>
    path:line:col  <one-line context>
```

Emission policy (mechanical form of the maintainer's rule):

| Kind | Rule |
|------|------|
| Error | own block, printed immediately (still aborts — no error-recovery redesign) |
| Serious single warning (termination, soundness) | own block |
| Composite analysis (multi-site diagnosis) | collected during the pass, **one** block: summary + all sites |
| Info notes | composite only — one end-of-compile "notes (N)" block, never per-site stdout spam; `-q` flag deferred |

Wiring: collector owned by the pipeline (deterministic order, not
thread-local), flushed at compile end; errors print immediately. New
diagnostics born in blocks (termination warning retrofit; observability
warning born composite). Remaining ~230 sites migrate
MIGRATE-WHEN-TOUCHED (Rule 18); `errors.rs` gains the API mandate header.
Smoke tests that grep stderr warning text assert against the new format.

## 6. Rejected alternatives (record)

- **`ret` keyword** — `return` was excised 2026-08-13 (BUGS ledger, ~50
  sites, all backends). `ret` is that word under a new name, and invites
  the classic-return assumption in txn bodies — the one place `term`'s
  non-return flavour is load-bearing. Tails carry the classic path; one
  keyword stays.
- **when-tails / when-expressions** (Proposal 3) — `when` is an execution
  block, never a value producer.
- **`resident;` / `declare`** — already rejected in the lifecycle-laws
  plan (2026-10-15).
- **P-gated defn value-terms** — see §1 rationale.
- **Big-bang diagnostic sweep** — 235 sites; migrate when touched.

## 7. Landing order (one gated commit each)

1. This plan doc.
2. Diagnostics core + termination-warning retrofit.
3. Interpreter P-gated bare-term (+ the BUGS divergence test).
4. Backend: defn cond-P emission + txn bare-term `br %post` + IR/E2E
   tests.
5. Tails: AST node + parser + typecheck + dir_smoke `print_all` flip to
   the tail idiom (regression test for the divergence + tails).
6. Inference: arrowless-Void defn + txn inference mirror + array.bv
   corpus check.
7. Observability warning (born composite) + state-purity probe first.
8. Docs pass: SPEC (§9 term rule, §11 tail rule already written, :2398
   void rule, §8.8.1 `if`→`when` drift), execution-model note, BUGS
   ledger flip for the divergence, INDEX.md refresh.

Gates per commit: `cargo test --lib`, dir/fs/daily/mini_cli smokes,
`bash benchmarks/rbv_gate.sh`, Praetor baseline-diff on changed dirs.

## 8. Open facts (resolve in-implementation)

- Defn state-assignment legality (probe → §4 effect set).
- Arrowless + no-value-term defn: current parse/typecheck behaviour
  (error today vs already-Void) — make it Void either way.
- Txn arrowless inference: present or mirrored from the defn path.
- stderr-grepping smoke assertions: update to the block format when the
  retrofit lands.
