# Follow-up program — width coercion, payload unification, G2, hygiene

**Status:** PLAN / research verified against today's `main` (fresh experiments,
not memory). Nothing built or modified except this document.

**Supersedes the "ABI retirement" sketch** in
`2026-11-18-compiler-health-program.md` §12/§13: fresh evidence changes what the
work actually is. That sketch assumed 12 latent width mismatches needed
retiring. The fresh census found **zero** — and in chasing the discrepancy
found a *live, user-facing* defect the corpus cannot see, plus three open issues
whose fixes are partially landed.

---

## 0. What fresh investigation established (evidence, not recall)

### 0.1 The width-mismatch census is now ZERO

`check --validate` over all 322 corpus files (`lib`, `examples`, `tests/tier1`):
**zero** call-site signature mismatches under the shipped class-exact check.
The earlier "12 hits in 6 shapes" came from the *width-exact experimental
mode*, and every one of those sites is clean today:

- the five `json_parse_gate` / `json_stringify_gate` / `net_stack_helpers` /
  `string_match_cleanup_phi` / `test_stringbuilder` fixtures: `OK (emitted IR
  verified)`;
- `examples/apps/task_board/backend/main.bv` (`char_to_str` i32): clean;
- a fresh `Socket` handle round-trip (`sock_tcp` → `sock_close`): clean.

The `ptr`/`i64` handle alias and the `char_to_str` widening are handled
correctly on today's emitter. There is no "retirement" work — there is a
policy decision and one real bug (§1).

### 0.2 The live defect: literal Int at a Float parameter emits a raw i64 pass

```briev
defn fround(x: Float) [true][…] -> Float { term x; };
node entry [beginprogram][true] { let r: Float = fround(7); … };
```

- **Typechecker: accepts** — `try_coerce_via_parse` deliberately admits a
  Decimal literal at a Float target (`fround(7)` is intended to work; a
  non-literal `let n: Int = 7; fround(n)` is correctly rejected with
  `expected Float … found Int`).
- **Emitter: passes `%t8 = add i64 0, 7` straight into
  `call float @fround(i64 %t8)`** — no `sitofp`. The declared parameter is
  `float`; the ABI registers hold the integer bits; the callee reads garbage.
- **The emitted-module check catches it** (class mismatch: integer → floating),
  so the program cannot run — which is why no corpus benchmark sees it: every
  affected program is rejected at the gate instead of miscompiling.

So the severity is "programs that should work don't", not "wrong code" — and
the check already contains the full blast radius.

### 0.3 #135 still reproduces — #138 fixed emission, not typecheck

`Packed([4, 5])` for `enum Basket<T> { Packed(List<T>) }` still fails with
`expected List<T> for payload of 'Packed', found List<Int>` on today's main.
#138's contextual machinery fixed the *emission* panic; the *typechecker's*
variant-payload unification still cannot bind `T` from a `List<Int>` argument
against a declared `List<T>`. #134 (enum-held collection alias mutation,
interpreter vs native) is likely the same root — verify together.

### 0.4 G2 promotion scope is small

`term;` appears 174 times across the corpus; `term! ->` (the exit spelling)
appears **zero** times. The convergence checkpoint warning is currently the
only thing distinguishing them. Migrating 174 sites is mechanical but wide; the
promotion decision should follow the migration, not precede it.

### 0.5 Hygiene

- Local branches `pr136/138/139/140/141/142/143` are merged (content on main);
  `fix/call-contract-fact-scope` and `pr141` are superseded. Deletable.
- `fuzz/Cargo.lock` remains modified by convention (do not commit).
- Open issues: #137 (Float32 storage width in the interpreter — #142 explicitly
  scoped it out), #135, #134, #4 (macOS, external host), #2 (C static library
  feature).

---

## 1. W3-1 — Literal coercion must emit the conversion (P0)

**The contract-first statement:** the typechecker *promises* that a Decimal
literal coerces to a Float parameter. A promise the checker makes and the
emitter does not keep is the #133 class again — checker and emitter
disagreeing about a call. The fix is on the emitter side, because the
acceptance is deliberate design (literal flexibility), not a bug.

**Where:** the ExtCall/defn-call argument path. Precedent already exists:
`emit_expr.rs`'s ExtCall arm widens a native `Char` (i32) to the boxed i64 the
runtime helper expects ("2026-08-13: … must be widened"). The same
*adapt-argument-to-declared-parameter* step is missing for Int→Float at
general defn calls.

**Design:**
1. The checker already knows the coercion happened — `try_coerce_via_parse`
   returns `true`. Record it: the call site must emit the same conversion the
   target type demands (Int→Float = `sitofp i64 %arg to float`; Float→Int
   literal = `fptosi`; TaggedLiteral/Quoted forms per their lanes).
2. Minimal, provable shape: extend the existing call-argument adaptation (the
   same place `string_ptr`/`adapt_to_i64` decisions are made) with a
   *declared-parameter-type* lookup — the emitter already consults
   `defn_params` for state prefixes; the parameter type is one lookup away
   (`LlvmContext::state_prefix` precedent: one question, one owner).
3. After emission, run the emitted-module check as the oracle: the new fixture
   must pass `check --validate` (today it rejects), and the module must contain
   exactly one `sitofp` between the literal and the call.

**Fixture:** `/tmp/prcheck/fmis3.bv` (already written; add as
`tests/fixtures/literal_float_coercion.bv` + a test asserting both acceptance
and correct *value*: `fround(7)` must print `7`, not garbage — the value check
is what distinguishes a conversion from a bit-reinterpretation).

**Regression direction:** the existing
`fround`-with-`let n: Int` variable form must KEEP failing typecheck
(non-literal Int → Float is not admitted; that rejection is correct and its
diagnostic names the mismatch).

**Acceptance:** fixture green end-to-end; `pointer_trickery`,
`daily_use_smoke`, lib suite, determinism, corpus floor 286/36 unchanged;
emitted-module check reports zero mismatches on the corpus.

**Scope guard (Rule 23/15):** the conversion is driven by the *declared
parameter type vs the argument's emitted representation* — no literal-shape
special cases beyond what `try_coerce_via_parse` already admits. If the
emitter cannot know the declared type at a given call form, that call form is
the unit of work; do not guess from the literal's shape.

## 2. W3-2 — Enum payload type unification (#135, with #134 verified) (P1)

#135 reproduces today (§0.3). The fix site is the same family I repaired for
call arguments: `infer_variant_construction` already substitutes payload type
params (`subst`) and unifies unbound params against a contextual expectation —
but the context it sees is `current_output_type` / the explicit `expected`
argument, and the *payload's own* `List<T>` vs the argument's `List<Int>`
needs element-level unification of the free param `T` from the *declared
result type* `Basket<Int>` at `let values: Basket<Int> = make()` / the return
position.

**Steps:**
1. Write the failing test from #135's repro (already verified it fails).
2. Extend the payload construction path to unify free type params from the
   declared enum instantiation (`Basket<Int>` known at the binding/return
   site) — same shape as `infer_argument`'s expectation threading, one level
   deeper (into `subst` for the payload's element types).
3. Check #134 after: if alias mutation through an enum-held collection
   diverges interpreter-vs-native, it may be the same "payload types were
   free at construction" root; verify its repro on the fixed build before
   claiming the connection (do not assume — its repro may be independent).

**Acceptance:** #135's repro builds and runs; `contextual_enum_collections`
stays green; #134's repro re-tested and either fixed or explicitly
de-scoped with a comment on the issue.

## 3. W3-3 — G2 promotion, migration first (P2)

`term;` ×174, `term! ->` ×0. The warning already names both spellings.
Sequence: (a) sweep the corpus converting `when <cond> { … term; }` sites that
are *exits* to `term! ->`; (b) after the sweep reaches zero ambiguous sites,
flip the diagnostic from warning to error. Do (a) with a census script first —
the 174 need manual classification (exit vs checkpoint), which is the real
cost. This is deliberately last: it is wide, mechanical, and zero user
requests depend on it.

## 4. W3-4 — Hygiene (any gap between other work)

- Delete merged local branches (`pr136/138/139/140/142/143`,
  `fix/call-contract-fact-scope`, `pr141`, `pr139` after its merge lands
  remotely). Keep `fuzz/Cargo.lock` uncommitted.
- #137: after W3-1, re-check whether the Float32 storage-width gap can ride
  the same declared-parameter machinery (its scoping note in #142 said it
  needs storage-format metadata the Float atom lacks — do not silently absorb
  it; verify the metadata question first).
- #4 (macOS) and #2 (C static library) remain external-scope; unchanged.

## 5. Order and effort

| # | item | effort | risk |
|---|---|---|---|
| W3-1 | literal coercion emission | small-med | low (checked both directions by fixture) |
| W3-2 | payload unification (#135/#134) | medium | medium (typechecker core) |
| W3-4 | branch/docs hygiene | trivial | none |
| W3-3 | G2 sweep + promotion | large, mechanical | low per-step, wide |

W3-1 first: it is the only *live* defect (programs that should compile don't),
it is small, and the emitted-module check gives it a tight oracle. W3-2 second
— the machinery exists from the #138/#142 lineage. W3-3 is a background grind.

## 6. Deliberately not doing

- **Not** re-tightening `ir_verify` to width-exact: the corpus is clean, the
  literal-coercion design *requires* cross-width argument passage at exactly
  the sites `try_coerce_via_parse` admits, and width-exactness would re-red
  those until W3-1 teaches the emitter the conversion. Re-tighten only if W3-1
  ships with a full corpus pass and a policy note.
- **Not** assuming #134 is the same root as #135 without running its repro on
  the fixed build (Rule: verify claims in source, not in similarity).
- **Not** starting the G2 sweep before a census script exists — 174 manual
  classifications invites mistakes; the script is the deliverable.
