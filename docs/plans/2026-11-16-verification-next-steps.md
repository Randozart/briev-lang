# Verification next steps — plan (post #104/#105)

**Status:** PLAN. Research done; concrete phases with measured payoff, risk and
gates. Nothing here implies weakening a specification.

## 0. Where the corpus stands (measured 2026-11-16, commit `424c69ed`)

Full `lib/std` (92 files), `brievc --release`:

```
proven 925 | unproven 5 | skipped 14528 | violated 0 | build errors 2
```

| metric | value |
|---|---|
| proven obligations | 925 |
| unproven obligations | **5** (only `ip_v4` ×3, `sha1_padded_len` ×2) |
| skipped | 14528 |
| violated | **0** |
| build/codegen errors | 2 (`cast_portable`, `stb_image`) |

Bounded CI corpus (`lib/std/*.bv` + `.smoke/*.bv`, 49 files): proven 478 /
unproven 0 / skipped 7673 / violated 0.

The older `2026-11-15-value-summaries.md` baseline (0 proven / 187 unproven /
14 690 skipped) is **stale**: PR #104 proved `mantissa_val` + `exponent_val`
and fixed the conformance sweep; PR #105 modeled `Load#` ranges. That document
now carries the re-baselined numbers.

**Instrument caveat (fixed this round):** `scripts/verify-bodies-census.sh`
patched its JSON header with `sed -i "s/\"unproven\": 0/.../"`, which rewrote
every per-file `"unproven": 0` into the grand total — the full run claimed
"5 unproven in each of 92 files". Entries are now buffered and the document
assembled once; failed builds are counted as `error` instead of inheriting the
previous file's report. Per-file census numbers from before this fix are
unreliable.

## 1. Skip stack on current main (bounded corpus, after #105)

| reason | count | class |
|---|---|---|
| `type String` outside word model | 2519 | type gate |
| generic/variadic declaration has no scalar model | 1061 | declaration shape |
| call to `Load#` — no value summary | 1001 | width-dependent intrinsic; see §3 |
| call to `coll_min` — no value summary | 288 | guarded-body summarization |
| call to `Store#` — no value summary | 240 | opaque memory write |
| call to `write_all` — no value summary | 141 | String-typed FFI |
| `task_status` / `SysCall#` / `sched_tail_block` / `Asm#` / `Argc#` | 96 each | opaque intrinsics |
| `trunc` / `pow2` / `pow10` / `pos_digit_count` / `neg_digit_count` | 48 each | recursive or looped helpers |
| `Float`-typed | 338 | type gate |
| division/remainder constant-divisor rule | 187 | expression rule |

## 2. The strategic insight: two different walls

The skip stack splits into two populations with different fixes:

1. **Type-gate admissions** — `String` (2519), `Float` (338), `Int,Int` tuples
   (144 on this corpus). The prover's symbolic + SMT layers already handle the
   *operations*; the type gate (`ScalarModel::admitted`, only `SignedWord{64}`
   or `Bool`) rejects the bodies before they reach those layers. Bool was
   admitted this way in phase 1 (369 skips → proven, gate fix only).
   **String is NOT the same shape**, and the first draft of this plan was wrong
   about it: measured, 19% of String-parameter bodies also need an integer
   *cast* (`(s as Data) as Int`), and those are precisely the byte-accessor
   leaves every parser sits on. String admission alone yields zero new proofs.
   See §Phase 2 for the measured breakdown and the paired slice.
2. **Opaque operations** — `Store#`, `SysCall#`, `Asm#`, `Argc#`,
   `sched_tail_block`. These enter the host or write memory at a computed
   address; a *value* summary would be a fabricated relation = false proofs.
   The plan's existing doctrine (§6 of `2026-11-15-value-summaries.md`) is
   right: **no value summary for these names, ever.** They stay skipped and the
   diagnostic says why. `Load#` is the one sound exception (width-bounded
   interval, done in #105) — and only for literal widths 1..=3, because a
   4-byte load is the word width on wasm32 (see #105's tests).

`coll_min` / `write_all` / `trunc` etc. are **neither** — they are ordinary
guarded (`coll_min`), FFI-adjacent (`write_all`), or recursive (`trunc`)
defns. The phrase "up to ~1 500 summarizable" from the stale plan **overstates**
what is cheap: the current `Summary` mechanism only accepts *unconditional
single-value* bodies, and the sound reason is documented at the struct
("a value that depends on a guard has no single expression to substitute").
`coll_min` has four `when` guards; `trunc` recurses on itself. Summarizing
either soundly is real design work (guarded summaries as intervals or
guard-forms), not a mechanical fix.

## 3. Phases, in measured payoff-to-risk order

### Phase 1 — Re-baseline the full corpus and fix the stale plan doc  (DONE this round)

- Measured the full `lib/std` corpus with the project's own runner (§0).
- Re-baselined `2026-11-15-value-summaries.md`: new numbers, §6 marked resolved,
  the `Load#` range exception recorded against the "no value summary" doctrine.
- Fixed the census script's summary-rewriting bug and added an `error` bucket so
  a failed build can never masquerade as a verification result.
- **Gate:** the numbers come from `verify_bodies_corpus.sh`; the fixed census now
  agrees with it at obligation level.

### Phase 2 — Admit `String`, but **only together with integer casts**  (measured)

#### Why the naive order fails (measured 2026-11-16)

Static scan of `lib/std`: **192 defns take a `String` parameter**. Of those:

| property | count |
|---|---|
| body contains an `as` cast | 37 (19%) |
| body uses `Load#` | 14 |
| neither cast nor `Load#`/`Store#` | 155 |

The 155 "neither" cases are almost all *callers* of the byte accessors
(`str_to_int` → `parse_int_ws` → `byte_at`, `pf_sign` → `byte_raw`, …), or they
read `s.^Length`.

The load-bearing leaves are exactly these:

```briev
defn str_len_bytes(s: String) [true][term >= 0] -> Int {
    term Load#((s as Data) as Int, 8);
};
defn byte_raw(s: String, i: Int) [true][term >= 0] -> Int {
    term Load#((s as Data) as Int + 8 + i, 1);
};
```

Both are blocked by **two** gates at once: the `String` type gate *and* the
`(s as Data) as Int` cast. Admitting `String` alone moves them from "type String
is outside the word model" to "an unsupported expression" — zero new proofs.

**General lesson:** the skip table's counts are conditional on the *first* gate
that fires. Fixing gate A re-attributes A's bodies to whatever gate B they were
really blocked on. PR #105 demonstrated it for `Load#` (1237 → 1001, totals
unchanged); the String bucket has the same shape.

#### The refined slice (2a)

1. `ScalarModel::OpaqueHandle` — admitted for `String` and `Data`: word-sized,
   **no value semantics**. Usable as an opaque atom; never ordered; never
   compared for content equality.
2. `Expr::Cast` restricted to a **pair-keyed whitelist** of representation casts:
   `String ↔ Data` and `Data ↔ Int` (both directions), as an identity on the word.
   It must be keyed on the *pair*, not on "both operands are pointer-sized",
   because the emitter proves the distinction (verified 2026-11-16):

   | Cast | IR | Kind |
   |---|---|---|
   | `(s as Data) as Int` | `ptrtoint`/`inttoptr`/`bitcast`, **no call** | representation |
   | `s as Int` | `ptrtoint` + `call @str_to_int` | **conversion** (parses) |
   | `n as String` | `call @int_to_str` | **conversion** (formats) |

   So `String → Int` directly is a parse and `Int → String` is a format; both
   stay unsupported, as do width-changing casts (`Int → Byte`) and
   `Float ↔ Int`.
3. Target the leaves (`str_len_bytes`, `byte_raw`, `byte_at`); verify their posts
   prove from the `Load#` range model, then let the existing summary fixpoint
   feed the parser chain (`pf_ws` / `pf_sign` / `str_to_int`).
4. `s.^Length` / `CharCount#` is a **third, separate gate** (a length fact on an
   opaque handle). Do not bundle it: `str_to_bool` and `truncate` need it, the
   byte accessors do not.

**Gate:** a `byte_at`-calling defn proves its contract; a String goal needing
content reasoning stays skipped with a reason, never guessed; `violated` stays 0.

### Phase 3 — Multi-field results (`Int, Int` tuples)  (273 full-corpus)

`sscale`/`float_rounded` return `(Int, Int)`; the value model is one scalar per
name, so the result needs field-split obligations (`term.0`/`term.1` already
exist in the contract syntax — `post_facts_components` already splits them).
Mechanical: verify one obligation per field, or a small record sort in SMT.

### Phase 4 — Bool parameters (phase-1c in the stale plan)  (small)

The encoder hard-codes every identifier as `BitVec 64`; a Bool parameter
compared against a Bool literal is ill-sorted. Thread parameter types through
the encoding pipeline. Payoff small (handful of defns), but it completes the
Bool work and removes a known sharp edge.

### Phase 5 — Guarded / interval summaries (design first, then build)  (up to ~1500)

`coll_min` (guarded), `trunc` (recursive) are *not* summarizable by the current
mechanism, and should not be forced. Options, in increasing ambition:

- 5a — **Interval summaries**: a guarded defn earns a summary that is a
  *contract-respecting bound* (e.g. `coll_min` → `[0, max(a, b)]`) rather than
  a single expression. Sound if the interval is proved from the body, never
  assumed.
- 5b — **Loop-unrolled summaries**: `pos_digit_count` (a loop) could get a
  summary proved by the existing loop-invariant machinery, then inlined by
  callers. Real work; the invariant must be found, not guessed.

**Not planned:** value summaries for `Store#`/`SysCall#`/`Asm#`/`Argc#`
(soundly unsurmmarizable, doctrine recorded).

## 4. What is NOT going to move the needle

- Extending the `Load#` width model past 3 bytes: the zero-extension bound is
  **target-dependent** at 4 bytes (wasm32 word width), so a wider claim needs
  target parameterization. Tested refusal (see #105) is the honest state.
- Casting (`as`): the word model doesn't model truncating casts; a cast layer
  is its own workstream and interacts with the width-dependence above.
- Chasing `proven` count on `[true]`-contract defns: proving `term >= 0` on a
  body that already documents `[true]` adds no safety value. Effort belongs on
  defns whose contracts bind (the mantissa_val class), not on count inflation.

## 5. Recommended sequence

1. **Phase 1** (re-baseline + doc repair) — half a session, unlocks honest
   targeting.
2. **Phase 2a** (String type-gate + length-only abstraction, bounded stdlib
   set) — the single highest-leverage *new capability*; the corpus says String
   is ~⅓ of all skips and it will only matter more as contracts migrate to
   arg-checking.
3. **Phase 3** (`Int,Int` tuples) — mechanical, small, completes the "multi-field
   contract" story that `post_facts_components` already started.
4. Evaluate **Phase 5a** (interval summaries) as the design follow-up, with its
   own doc, only after 2–3 land.

## 6. Open questions for the owner

- String content reasoning (beyond length) is out of scope for phase 2 — same
  boundary counsel draws. Confirm that's the desired line.
- The String abstraction introduces a *new* admitted scalar model; the
  `ScalarModel` enum is the single seam (`domain.rs`), but the resolver surface
  (`BodyTypeModel::resolve`) and the SMT header both touch it. Confirm the
  enum extension naming before implementation.
- Phase-5a's "contract-respecting interval summaries" touch the soundness
  doctrine in `2026-11-15-value-summaries.md` §6. If you'd rather keep the
  doctrine absolute, phase 5 closes entirely and the remaining unproven-heavy
  work is phase 2 + 3 only.