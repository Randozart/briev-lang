# Value-Summary and Word-Model Admission (2026-11-15)

**Status**: in progress. **Re-baselined 2026-11-16** after PRs #104/#105 — the
numbers in §1 were taken before the masked-offset kernel, the post→pre call
gate and the `Load#` width model landed.

**Supersedes**: the "value summaries for `Store#`/`Load#`" item in
`2026-11-15-remaining-verification-work.md` §6 — that diagnosis was half
right (summaries are the mechanism) and half wrong (the biggest "summary"
classes are memory operations that cannot soundly be summarized).

## 0. Current baseline (2026-11-16, commit `424c69ed`, `brievc` release build)

`bash scripts/verify_bodies_corpus.sh lib/std` (92 files):

```
Files:            92
PROVEN:           81        (files with no failure bucket — NOT obligations)
UNPROVEN:         5
NO_POSTCONDITION: 0
SKIPPED:          14685
VIOLATED:         0
ERR:              10        (codegen/link errors, not proof failures)
```

Obligation-level tallies (`scripts/verify-bodies-census.sh`, 92 files):

```
proven 925 | unproven 5 | skipped 14528 | violated 0 | build errors 2
```

The whole unproven set is now two goals:

| defn | obligations | goal | blocker |
|---|---|---|---|
| `ip_v4` | 3 | `term >= 0 && term <= 4294967295` | needs disjoint-bit Or / shift reasoning |
| `sha1_padded_len` | 2 | `term >= msg_len + 9`, `term % 64 == 0` | needs modular arithmetic |

**`mantissa_val` and `exponent_val` are gone from this table** — both are
proven now (PR #104: masked-offset interval + literal-shift folding; PR #104
also taught the call gate to use a callee's postcondition, and the
`float_format` NaN/Inf guard is spelled `when exp8 > 254` so its fall-through
fact is the bound the callee needs).

## 0.1 The census script was under-reporting

`scripts/verify-bodies-census.sh` built its header first and then patched it
with `sed -i "s/\"unproven\": 0/.../"`. That rewrites **every** match in the
document, so each file's `"unproven": 0` became the grand total: the full run
claimed "5 unproven in all 92 files" while every file individually reported 0.
The script now buffers per-file entries and assembles the document once, and it
counts a failed build as its own `error` field instead of silently attributing
the previous file's report to it. **Any census number produced before this fix
should be treated as unreliable at per-file granularity.**

## 1. Measured baseline (full `lib/std`, 92 files)

One pass of `scripts/verify_bodies_corpus.sh --full lib/std` at commit
`a754b25fd`:

```
Files:            92
PROVEN:           0
UNPROVEN:         187
NO_POSTCONDITION: 0
SKIPPED:          14690
VIOLATED:         0
ERR:              9
```

Unproven breakdown (all four are documented, not new):

| defn | obligations | blocker |
|---|---|---|
| `mantissa_val` | 91 | caller facts unprovable at the declared edge (no mask-bound reasoning in the call-site checker) |
| `exponent_val` | 91 | same |
| `ip_v4` | 3 | needs shift / disjoint-bit-Or reasoning |
| `sha1_padded_len` | 2 | needs modular arithmetic reasoning |

### Skip-reason distribution (full corpus)

| reason | count | class |
|---|---|---|
| `type Custom("String")` outside word model | 4 830 | type: string is a heap/iterable value, not a word |
| call to `Load#` — no value summary | 2 094 | **opaque memory read** |
| generic or variadic declaration | 2 007 | declaration shape |
| `type Custom("Float")` outside word model | 638 | type: IEEE-754 not a signed word |
| call to `coll_min` — no value summary | 546 | collection helper (returns a collection) |
| call to `Store#` — no value summary | 455 | **opaque memory write** |
| `type Custom("Bool")` outside word model | **369** | type: Bool is a 1/8-bit value, not a 64-bit word |
| division/remainder nonzero-divisor rule | 366 | expression: non-constant divisor |
| `type Tuple([Int, Int])` outside word model | 273 | type: multi-field result |
| call to `write_all` / `trunc` / `SysCall#` / `Asm#` / `Argc#` / … | 182 each | opaque FFI / intrinsics |
| global state (`stdout_fill`, `STDOUT_CAP`, `PAGE_SIZE`) | 91 each | state-free model excludes globals |

## 2. What the previous plan got wrong

`2026-11-15-remaining-verification-work.md` §6 named "value summaries for
`Store#`/`Load#` helpers" as the single highest-value follow-up because 16 k
skipped bodies dwarf the ~200 unproven ones. Measuring the skip *reasons*
splits that 16 k into two very different populations:

1. **Summarizable callees** — ordinary defns the body prover could inline if
   it had a value summary for them (`coll_min` 546, `write_all` 273,
   `trunc` 182, `when_hi`/`task_status`/`task_done`/`stdout_flush`/
   `stdout_byte` 91 each). These are real, and the existing
   `build_summaries` fixpoint already collects what it can — the residual
   cases are helpers whose own bodies call yet-unsurmarized things, and the
   fixpoint correctly refuses to guess.
2. **Unsummable operations** — `Store#(addr, value, width)`, `SysCall#`,
   `Asm#`, `Argc#`. These write memory at an address expression, or enter the
   host. Their results are not a function of their arguments in any model the
   prover has. A "summary" here would be a fabricated relation and would turn
   "skipped" into false proofs. The sound disposition is to **keep skipping
   them and say so** — which the diagnostic already does, and this plan records
   as doctrine.

   **`Load#` is the one exception, and it is not a summary (PR #105).**
   `Load#(addr, w)` emits `load i{8w}` and then *zero-extends*, so its value
   lies in `[0, 2^(8w)-1]` — a *range* that follows from the emitter, not a
   relation over the arguments. The prover states that range for literal
   widths 1..=3 only: at width 4 the load equals the word width on wasm32, so
   zero-extension is the identity and the value may read negative, making
   `[0, 2^32-1]` a false proof. The atom is keyed by AST node so two identical
   loads stay distinct (otherwise `a - b` would fold to 0 across a store). This
   is an intrinsic-model exception, documented so it is not mistaken for
   licence to summarize `Store#`.

So the honest statement is: `Load#`/`Store#` (2 549 skips) are not waiting on
summaries; they are waiting on a **memory model** (pointer state,
aliasing, width). That is a workstream of its own, not a summary table.

## 3. What is admitted, and why

The admission gate is `BodyTypeModel::resolve` (`src/analysis/verify_bodies.rs`):
a type is admitted iff its protocol category is `"Int"` **and** its LLVM
storage resolves to `i64`. Everything else returns `ScalarModel::Unsupported`,
and the body prover skips the defn with the "outside the signed 64-bit word
model" reason.

Two classes are *already* fully supported inside the prover and are blocked
only by this one gate:

### 3.1 Bool (369 skips)

- `SymbolicValue::bool_literal` evaluates `Expr::Bool` to `Literal(0|1, "bool")`.
- The SMT encoder (`briev-verify/src/smt/encoding.rs`) maps
  `Expr::Bool(b)` to the native SMT-LIB Boolean `(= #b1 #b1)` /
  `(= #b0 #b1)`; `==`/`!=` on operands of the same sort encode as
  `(= a b)` regardless of sort, and `Not` encodes as `(not a)`.
- Bool-typed defns (`cstr_equals`, `task_done`, `env_key_match`,
  `briev_event_ready_impl`, …) therefore have bodies the encoder can already
  handle end-to-end. The only thing rejecting them is the type gate.
- **Decision**: admit `Bool` (category `"Bool"`, storage `i8` or `i1`) into
  the model. The symbolic and SMT layers need no change. This is a gate
  fix, not a capability fix — which is exactly the kind of change the gate
  should make.

### 3.2 `Int, Int` tuples (273 skips)

`sscale`, `round_all`, `float_rounded` return a two-field tuple. The prover's
value model is one scalar per named variable; a two-field result needs a
multi-field representation (two obligations, one per field, or a record
sort in SMT). **Deferred** — documented here as phase 2, not implemented this
round.

**Phase 2 landed 2026-11-18** (`briev-verify/src/verify.rs`; see
`docs/plans/2026-11-18-w4-pf-exp-tuple-induction.md`). The representation chosen
was per-field expressions, not a record sort in SMT:

- a value `Summary` may now be `Expr::Tuple(fields)` (`evaluate_result`), so a
  non-recursive tuple callee earns a per-field summary;
- a tuple callee's **declared** postcondition is consumed at the call site as
  one fact per field (`tuple_arity`, tuple-of-atoms in `rewrite_expr`,
  `term`/`term.N` mapping in `post_facts_for_atom`), so even an
  unsymmarisable (guard-dependent or recursive) callee contributes its
  tuple bounds;
- `eval_symbolic` folds constant negation, so negative bounds (`term.0 >= -1000`)
  are decidable by the interval verdict.

Result: `pf_exp` proved; `lib/std` `goals_unproven` 99 → 10.

### 3.3 String / Float / List / collection types

These are genuinely outside the word model: heap values, IEEE-754, and
iterables respectively. Admitting them is not a gate fix but a new theory
layer. **Deferred** (string is the biggest single class at 4 830; when it is
done it will dominate the corpus).

### 3.4 Memory operations (`Load#`, `Store#`) and host intrinsics

As §2(2): **soundly unsummable without a memory model.** Kept as skips,
documented as phase 3. The diagnostic message is already precise ("a call to
`Load#`, which has no value summary"); the doctrine is that no value summary
may be added for these names, because one would be unsound.

## 4. Phases

| phase | scope | expected payoff (measured) |
|---|---|---|
| **1 (this round)** | admit Bool at the type gate; verify SMT roundtrip; re-measure | ~369 skips → proven/skipped-for-other-reasons |
| 2 | multi-field result model (`Int, Int` tuples) | ~273 |
| 3 | memory model for `Load#`/`Store#` (pointer state, width, aliasing) | up to ~2 549 |
| 4 | string theory layer | up to ~4 830 |
| 5 | summarizable callees (`coll_min`, `trunc`, `write_all`, …) | up to ~1 500 |

Phase order is by *measured* payoff-to-risk, not by the earlier
assumption that memory ops were the cheap win.

## 5. Verification plan (phase 1)

Phase 1 splits into 1a (type gate, done), 1b (SMT encoding, done), and 1c
(Bool parameters, deferred):

### 5.1 Phase 1a — type-gate admission (DONE, commits `e9aeea8da`, `af24dbc27`)

1. Before/after `scripts/verify_bodies_corpus.sh --full lib/std` summaries.
   - Baseline (at `a754b25fd`): SKIPPED 14 690, UNPROVEN 187, PROVEN 0.
   - After: SKIPPED 14 685, UNPROVEN 192, PROVEN 0.
   - Delta: 5 fewer skips, 5 more unproven. The 5 Bool-typed defns that
     pass the type gate now reach the SMT admission gate and are reported as
     "unsupported or ill-sorted word-model contract" — an honest unproven
     verdict, not a false proof.
2. `cargo test -p briev-verify`: 50/50 pass.
3. `cargo test --lib`: 3804 pass, 1 pre-existing failure (`conformance_sweep`
   — caused by the `ascii_lower` precondition in `1504b3da6`, not by this
   change; verified by stashing the Bool commit and re-running).
4. Praetor on `briev-verify/src` and `src/analysis`: clean.

### 5.2 Phase 1b — SMT encoding (DONE, commit `edd80e024`)

The return type is threaded from `contract_target` (which reads the declared
`OutputType`) through `verify_supported` → `verify_one` → `smt_discharge` →
`build_vc_query` → `query_header`. Bool-typed defns declare `term` as
`Bool` in the SMT query; Int-typed defns keep `(_ BitVec 64)`.

The pre/postcondition gate is term-sort aware: Bool defns use
`supported_boolean_with_term_sort` (the SMT encoder with `term` declared
Bool), word-model defns keep `supported_word_postcondition` (which
special-cases `Field` expressions that the strict `expect` encoder rejects).

Measured before/after (full lib/std, 92 files):
  Baseline (a754b25fd):     SKIPPED 14 690, UNPROVEN 187
  Phase 1a (e9aeea8da):     SKIPPED 14 685, UNPROVEN 192
  Phase 1b (edd80e024):     SKIPPED 14 685, UNPROVEN 101

The 91 unproven reduction is because several Bool-typed defns that were
"unsupported or ill-sorted" are now genuinely PROVED by the SMT solver.
The remaining Bool defns that were "ill-sorted" are now either genuinely
unprovable (pre-state `term` unbound — the solver correctly reports
could-not-prove) or proven. No Bool defn is silently skipped.

`cargo test -p briev-verify`: 50/50 pass.

### 5.3 Phase 1c — Bool parameters (DEFERRED)

Bool-typed *parameters* (e.g. `defn f(b: Bool)`) still fail the SMT gate:
the encoder hard-codes every identifier as `BitVec 64`, and a Bool parameter
compared against a Bool literal is ill-sorted. Fixing this requires
threading parameter types (not just the return type) through the encoding
pipeline. The corpus payoff is small (a handful of Bool-param defns), so
this is deferred.

## 6. Conformance sweep status (RESOLVED 2026-11-16)

The sweep was failing with 387 sources, rooted in the
`exponent_val`/`mantissa_val` precondition tightenings in `4a24b3a21`: the
call-site checker neither propagated callee postconditions nor reasoned about
byte masks, so every call site failed and the failure cascaded through every
file importing `float_fmt.bv`.

**Both halves are fixed and merged:**

1. `post_facts_for_term` (PR #104) lets a caller use its callee's *declared*
   postcondition for an inline call term — the `ascii_lower(byte_at(...))`
   class.
2. `float_format`'s NaN/Inf guard is `when exp8 > 254` rather than
   `when exp8 == 0xFF`. Identical for a byte field, but the fall-through fact
   is then the upper bound `exponent_val`'s precondition needs (a disequality
   `exp8 != 255` is not a bound the gate can use).

`conformance_sweep_every_active_source_parses_and_checks` passes (393s).

## 7. Undo

Reverting the phase-1a commits (`e9aeea8da`, `af24dbc27`) and the phase-1b
commit (`edd80e024`) restores the gate and the SMT pipeline; nothing else
is touched. Phase 1c is not started.
