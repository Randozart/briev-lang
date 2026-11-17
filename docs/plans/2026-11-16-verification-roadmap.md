# Verification roadmap after Phase 2b

**Status:** PLAN (research done, nothing implemented).
**Base:** `52b1a4c9` (PR #108). Full `lib/std`, 92 files, release `brievc`:

```
proven 1105 | unproven 5 | skipped 14348 | violated 0 | errors 2
```

The 5 unproven are unchanged and understood: `ip_v4` ×3 (disjoint-bit OR),
`sha1_padded_len` ×2 (modular arithmetic).

## 1. Fresh skip distribution (measured this round, 14 503 skip lines)

| Reason | Lines | Note |
|---|---|---|
| a call to X, which has no value summary | **7976** | 55% — decomposed in §2 |
| generic or variadic declaration has no scalar model | 2007 | type parameters, not a gate flip |
| a handle-returning contract is outside the word model | 1631 | **added in phase 2a — too broad, see §3** |
| `Float` outside the word model | 822 | IEEE, separate theory |
| `Tuple([Int, Int])` outside the word model | 546 | see §4 |
| division/remainder needs a constant divisor | 457 | expression rule |
| global `G` outside the state-free model | 394 | state model, not a gate |
| `List<Int>` outside the word model | 184 | collection model |
| `Tuple([Int,Int,Int,Int])` | 95 | same as §4 |
| other custom types (`ByteBuf`, `Socket`, `Char`, `GrowBuf`) | ~210 | separate models |

**Lesson applied:** this table is *conditional on the first gate that fires*, and
two gates opened since the last tally (handles, lengths). The ranking changed
substantially — `String` dropped out of the top entirely, and the
"handle-returning contract" skip I introduced in 2a is now the third-largest
bucket, which means it is over-broad.

## 2. What the 7976 "no value summary" actually are

| Callee | Lines | Reality |
|---|---|---|
| `Load#` | 2276 | **Modelled only for literal widths 1..=3.** The rest are 8-byte word loads (`str_len_bytes` reads the header): no unsigned bound exists, so this is correct refusal, not a gap. |
| `write_all` | 546 | String param + syscall inside; opaque. |
| `coll_min` | 546 | **Guarded** body — not summarizable as one expression. |
| `Store#` | 455 | Opaque write. Doctrine: never summarised. |
| `byte_raw` | 455 | **Proves today** (`[term >= 0]`) — but callers still skip, because its result depends on *memory*, so it can never earn a value summary. |
| `byte_at` | 372 | Same: proved, unsummarizable. |
| `str_len_bytes` | 183 | Same. |
| `trunc` / `pow2` / `pow2b` / `pow10` | 182 / 91 / 91 / 91 | **Recursive** — a summary would be a guess. |
| `task_status` | 182 | Declares `[term >= 0 && term <= 4]`; body has a `Load#`. |
| `SysCall#`, `Asm#`, `Argc#`, `sched_tail_block` | 182 each | Host intrinsics. Never. |
| `when_hi`, `cstr_len`, `task_done`, `stdout_*` | 91 each | Contracted helpers, small bodies. |

**Key observation.** `byte_raw`, `byte_at`, `str_len_bytes`, `task_status`,
`when_hi`, `pow2`, `pow10` all *declare* postconditions, and the call gate
already knows how to use a declared postcondition for an inline call term
(`post_facts_for_term`, PR #104). The body prover does not — it requires a
*value summary* or refuses the body.

That is the real gap: **1010 lines from the three proved byte accessors alone**,
plus the recursive helpers and `task_status`, all of which are unreachable by
summaries *by design*.

## 3. Phase A (cheap, do first): narrow the handle-returning skip

Phase 2a skips any `String`-returning declaration with a non-trivial post,
because a post about a handle is a claim about text. But **~30 defns** post only
about the result's length:

```briev
out defn int_to_str(n: Int) [true][term.^Length >= 0] -> String { … };
out defn char_to_str(c: Int) [c >= 0][term.^Length >= 1] -> String { … };
```

Since phase 2b models `term.^Length` as a non-negative atom, `>= 0` is
immediate, and `>= 1` needs only the length fact to exist. The skip is over-broad
by exactly the cases the length model was built for.

**Change:** skip only when `term` appears *as a value* — i.e. anywhere except as
the base of `term.^Length` / `CharCount#(term)`. Walk the post; when a
`Reflect(base,"Length")` or `CharCount#(base)` is seen, do not treat `base`'s
`term` as a value mention.

**Payoff:** ~30 defns × the files that include the stdlib. **Risk:** low — the
cases that stay skipped are unchanged, and the newly-admitted ones can only prove
length claims that the atom already guarantees.

## 4. Phase B (biggest): contract atoms in the body prover

**Idea.** A call to a *contracted* defn becomes an opaque atom carrying its
declared postcondition's facts about that atom — the same shape as phase 2b's
length atoms, and the same rule the call gate already applies to preconditions.

Why this is the right move:

- It is how contract-based verification is supposed to work: contracts are the
  source of truth in this project, so a callee's declared post is a legitimate
  fact about the call *term*.
- It reaches the cases summaries never can: memory-reading accessors
  (`byte_at`, `byte_raw`) and recursive helpers (`pow2`, `pow10`, `trunc`),
  because it never needs the callee's *body* — only its declaration.
- It reuses `post_facts_for_term`, already written and tested in #104.

**Design.**

1. Build a `contracts: name → (params, post)` map alongside `sums` (same
   fixpoint input, no recursion needed).
2. Admission (`expr_reason`): a `Call` is admissible when the callee has
   **either** a value summary **or** an explicit non-trivial contract. The
   existing modelled-load / modelled-length cases stay.
3. `eval_symbolic`: a contracted call evaluates to an opaque atom keyed on the
   AST node (distinct per call site — the same discipline as `Load#`, and for the
   same reason: two identical calls may read memory twice).
4. Facts: the callee's post, with parameters substituted by the arguments and
   `term` replaced by the atom, folded into the path's bounds (via
   `post_facts_for_term` + `facts_of`).
5. `inline_body` must pass these calls through unchanged (like modelled `Load#`).

**Soundness conditions (non-negotiable).**

- Only *declared* postconditions are used. No body is inspected; a lying
  postcondition is a lying contract, which is the project's stated source of
  truth, and `--verify-bodies` checks bodies against it.
- No transitive expansion: one level, conjuncts only.
- Skip a post that folds to literal `false` (an inconsistent declaration must not
  poison unrelated obligations).
- No self-recursion expansion (a call to the enclosing defn contributes nothing).
- The atom is per *call site*, never per callee — this is what keeps two calls
  from being confused for one value.

**Payoff estimate:** ~1010 lines directly (the three byte accessors), plus the
recursive and `task_status`/`when_hi`/`cstr_len` families, and their callers.
Plausibly 1500–2500 skip lines, with `proven` rising correspondingly.

**Risk:** medium — this is a new proof rule. Guard rails: `violated` must stay
0 on the corpus, `unproven` must not grow (as in 2b, where I caught the encoder
regression), and tests must pin the five conditions above.

## 5. Phase C: `Int, Int` tuple results (546 + 95 lines)

`sscale`, `float_rounded`, `pf_num`, `pf_exp` return tuples and post about
`term.0` / `term.1`. `post_facts_components` already splits `term.N` for the call
gate, so the vocabulary exists.

**Change:**

1. `bind_output`: when the terminating expression is a `Tuple`, bind `term.N` for
   each element (and `term` to nothing).
2. `eval_symbolic`/`rel_verdict`: resolve `term.N` through the same identifier
   path as `term`.
3. Obligation shape: one verdict per field, conjoined — a wrong field must still
   refute.

**Risk:** low-medium; the main hazard is a field index off-by-one silently
proving the wrong field, so tests must pin that a deliberately wrong `term.1`
refutes.

## Phase C result (tuples) — measured 2026-11-17

```
proven   2012 -> 2192  (+180)
skipped 13436 -> 13166 (-270)
unproven   10 -> 100   (+90)
violated    0 -> 0
errors      2 -> 2
```

The `+90` unproven is **one defn**: `pf_exp`, reported once per file that pulls in
the prelude. Inspected, not a prover gap — its postcondition claims
`term.1 >= 0` while its precondition never states `i >= 0`:

```briev
defn pf_exp(s: String, i: Int, len: Int, acc: Int)
    [acc >= 0 && acc <= 1000]                      // no `i >= 0`
    [term.0 >= -1000 && term.0 <= 1000 && term.1 >= 0]
```

Contract-first says state the domain (`i >= 0`, `len >= 0`) rather than weaken the
post. That edit was **attempted, built into the prelude, and measured — and it does
not work**. Two findings, both empirical:

1. **The precondition is not the blocker.** With `i >= 0 && len >= 0` added and
   `brievc` rebuilt from the patched prelude, `pf_exp` still reports
   *"could not prove … path 1 of 6"*, unchanged.
2. **The blocking path returns a call, not a tuple.** Two of `pf_exp`'s paths end
   in `term pf_exp_digits(…)`, which *returns* a tuple, so there is no
   `term.0`/`term.1` to bind — and `pf_exp_digits` is recursive, so it earns no
   summary, while its declared post bounds only its first component
   (`[term >= -1000 && term <= 1000]`).

Closing this needs one of two things, neither available today: induction over the
recursive `pf_exp_digits`, or a callee post that bounds the second component
*and* is itself dischargeable. It is therefore recorded as an inspected,
understood gap — not a precondition fix, and emphatically not a weakened post.

Two further facts worth keeping: `lib/std/cast_lanes.bv` is **prelude-baked** into
`brievc` (a `.bv` edit is inert until the binary is rebuilt), and
`cast_portable.bv` carries its **own** `pf_exp` with a different contract
(`[true][term >= -1000 && term <= 1000]`) — so any edit must consider both lanes.

## 6. Phase D: generic / variadic declarations (2007 lines)

`defn map<K, V>`, `iter_*`. These are outside the model because their parameters
are type-parameterised, and the value model is one scalar per name.

This is **not** a gate flip: admitting them means deciding what a type parameter
means for the prover (erased? opaque? per-instantiation?). Almost all of them are
also collection-shaped, so their bodies would then hit the `List` gate anyway.
**Recommendation: defer**, and write a short design note when the collection
model is on the table. Not worth guessing now.

## 7. Phase E: HIR/AST defn-liveness drift (known bug, in `BUGS.md`)

56 names differ on `lib/std/os/dir.bv`; `HIR ⊆ AST`; `entries`' calls to
`dir_open`/`dir_close`/`dir_loop` are lost, so the subtree goes. Release ships
the HIR set for ownership/reclamation classification (emission is unaffected).

**Next step** (recorded in `BUGS.md`): bisect by trimming `entries` in place —
the ad-hoc fixtures I tried failed at the type gate, so use the real file.
Candidates in order: the `names <- …` transfer statement,
`Free#(x as Ptr<…>)`, and the `Result<List<String>, Int>` output type.

**Independent of verification** — separate subsystem, separate review. Can run in
parallel with phase B.

## 8. Phase F: release train (only after B/C land)

Order and reasons:

1. **`brievc`** — the vehicle. Everything gained (masked offsets, post→pre facts,
   handles, lengths, contract atoms) lives here.
2. ~~**`counsel-macros`**, then **`counsel`** — because counsel's static half pins
   a `brievc` release tarball by sha256~~ — **premise revoked 2026-11-17**.
   `counsel 0.5.0` (the version briev-lang pins) has **no `build.rs` and no
   `brievc` dependency**: its static half is pure Rust (`syn` + `serde_json`,
   feature `static`). The tarball pinning belonged to counsel **0.1.x** — the
   `~/.cache/counsel/brievc-linux-x86_64-v0.1.{0,2,3}.tar.gz` artifacts on disk
   are that era's leftovers.

   Consequence: **a counsel release would carry none of this work.** The body
   prover lives entirely in `brievc`; counsel's checker reads `#[pre]`/`#[post]`
   attributes over Rust source and never sees a `.bv` body. Releasing counsel is
   warranted only when counsel's own checker changes — it has not, and nothing in
   this campaign touched it.

   So the train reduces to **one artifact: a `brievc` release**, which stands on
   its own because `brievc build --verify-bodies` is the user-facing surface.
3. **`briev-verify`: hold.** Its public surface (`ScalarModel`, the admission
   path) changed three times in the last week, and its only consumer is
   `briev-lang` itself. Publishing locks in a shape we are actively moving;
   revisit once phases A–C settle.

Preconditions for cutting anything: `main` CI green (three PRs merged back to
back — verify), and `violated == 0` on the corpus at the tagged commit.

## 9. Order of work and gates

| Phase | Work | Gate |
|---|---|---|
| **0** | Confirm `main` CI green — one run at `52b1a4c9` passed; `main` has since moved to `aca2fe00` (in progress) | all gates pass |
| **A** | Narrow handle-returning skip | `violated` 0; `unproven` unchanged; `proven` up |
| **B** | Contract atoms in the body prover | `violated` 0; `unproven` unchanged; 5 tests for the soundness conditions |
| **C** | Tuple results | a wrong `term.1` still refutes; `violated` 0 |
| **E** | Liveness drift | oracle test green; `BUGS.md` entry closed |
| **D** | Generics | design note first; no code until it exists |
| **F** | Release train | green `main`, `violated` 0, pinned sha recorded |

Each phase is its own PR with a before/after corpus table, because "proven went
up" is only meaningful next to "violated stayed 0".

### Gate amendment after phase B (measured 2026-11-16)

"`unproven` must not grow" was too strict, and phase B showed why. An unproven
verdict means *examined, could not decide* — so widening the model legitimately
turns some skips into unproven ones. The rule is now:

1. `violated` stays **0** (a false proof is the only hard failure);
2. every new unproven is **inspected and named** in the PR — it must be a genuine
   "needs a theory we do not have", not a gate disagreement (the 2b encoder bug
   was exactly that, and this is how it was caught);
3. `proven` must rise by substantially more than `unproven` grows.

Phase B: `proven` +997, `unproven` +5, and the five are two distinct defns
(`is_alpha_loop`, `is_numeric_loop`, both `i == s.^Length`) plus three
pre-existing lines.

## 10. What this plan deliberately does not do

- **`Store#`/`SysCall#`/`Asm#`/`Argc#` summaries** — soundly impossible; the
  doctrine stands.
- **`Load#` beyond 3 bytes** — the zero-extension bound is target-dependent at
  word width.
- **Content reasoning for strings** — length only.
- **Float theory** — 822 lines, but IEEE semantics is a modelling project, not a
  gate.
- **Chasing `proven` on `[true]` contracts** — count inflation with no safety
  value.
