# Intentional non-termination: opt-in loops and a pre-agreed stop authority

**Status:** design note. Nothing implemented, nothing measured yet (§7).

Randy's question: *should we let a program consciously opt in to an infinite loop,
with a pre-agreed safety button or safety thread?*

## 1. What exists today (read from source)

| Mechanism | What it is | Where |
|---|---|---|
| **Guaranteed-infinite detection** | a reactive txn whose pre-condition variables it never writes, and which no other txn writes, is a **hard error** — *"this txn will run forever"*. If another txn *might* unblock it, a warning instead. | `analysis/provenance.rs::check_convergence_safety` |
| **`halt;`** | a statement and a never-type: the program stops. A `wfi` spin on ARM/RISC-V — *"the canonical sleep-forever"* — and a trap on other triples. | SPEC §8.8.2 |
| **`endprogram`** | an explicit process boundary. Its presence means a `[true]` postcondition is *not* read as "fires forever". | `analysis/causality.rs` |
| **Watchdog** | `?[progress] within 10ms -> on_timeout()` — a runtime deadline plus a handler. Only its **wiring** is checked: `UnknownTrigger`, `NoHandler`, `NoConflict`, `HandlerDoesNotFalsify`, `HandlerRestoresPrecondition`. | SPEC §10.3, `analysis/watchdog.rs` |
| **`Convergence::Unprovable`** | a loop whose convergence cannot be shown still runs, as a *conservative* reactor loop. | `analysis/loop_shape.rs` |

Two details from that read matter more than the table:

- `check_convergence_safety` **returns early when the precondition is literally
  `true`** — so the one shape most likely to be a deliberate endless loop is
  exempt from the only infinite-loop check that exists.
- A `[true]` **postcondition** is what makes a node "fire forever" by the
  causality analysis, unless `endprogram` is present.

## 2. The gap: intent is indistinguishable from failure

Two very different things produce the same outcome:

1. *"I could not prove this loop converges"* → `Unprovable` → conservative reactor
   loop. A failure, silently tolerated.
2. *"This loop runs until the process is stopped"* → a server, a scheduler, a
   persistent kernel. A design decision, expressed the same way.

Because intent cannot be **declared**, case (2) is parsed as case (1), and the
compiler has nothing to demand of it. Nothing requires a deliberately-endless loop
to carry any stop authority at all.

This is the same honesty problem the rest of this line of work has been fixing:
*"we could not prove X" must not be indistinguishable from "X is meant to be
false."* The convergence ladder (§10.8) fixed it for termination; this is the
mirror case where no termination is intended.

## 3. Options

**A. A new keyword** (`forever node ...`). Matches `seq`/`approx` in shape.
*Against:* the SPEC's admission process states that every author-surface keyword
is permanent cost and additions must clear the full admission criteria. A keyword
buys nothing the existing surface cannot already say.

**B. Make the `[true]` postcondition the explicit opt-in.** It already *is* one in
effect — that is the shape causality treats as "fires forever". Declare it
explicitly and **require a stop authority** on it. Zero new syntax; `[true]` stops
meaning "unconstrained" and starts meaning "runs until stopped".

**C. Tie the watchdog's `progress` condition to a liveness obligation.** The
watchdog already names a progress condition; today only its wiring is checked.
Require that the condition is **not provably unreachable** from the loop head — a
reachability fact the prover can already compute — and report it next to the
variant's termination obligation. `variant` proves *it stops*; `liveness` asserts
*it keeps doing useful work*. Exact dual, no temporal logic needed (§5).

**D. A safety thread.** Honest answer: **not a language surface, a platform
contract.** On bare metal the stop authority is a hardware watchdog or an ISR; on
a host, a supervisor thread; on a GPU, the host. The language should name the
abstract requirement — *an external stop authority must exist and be reachable* —
and let each backend satisfy it. Threads in a language targeting bare metal and
GPUs would be the tail wagging the dog.

## 4. Recommendation

**B, plus C, with D as a backend contract.**

- A `[true]`-post reactive `node` or `txn` is a declared endless loop: **require a
  watchdog** on it, and make its absence an error that names both remedies (state
  a postcondition, or add a watchdog).
- Check the watchdog's `progress` condition is not provably unreachable, and
  report it as a liveness obligation per declaration.
- Leave "safety thread" to the backend: the language requires a stop authority;
  each target implements it (hardware watchdog / ISR / supervisor).

No new keyword, which respects the spec's own admission rules.

## 5. Honest limits

The compiler can establish that:

- a stop authority **exists and is wired correctly** (today's watchdog analysis);
- the progress condition is **not provably unreachable** (already computable);
- the loop **does not terminate by construction** (no variant can be given).

It **cannot** establish *"the handler eventually fires"*. That is a temporal
property, and the prover is a first-order decision procedure over one state, or one
step. Claiming liveness would be precisely the overreach that produced a false
proof earlier in this codebase (`ed2fc83d`), so the surface must say "a stop
authority exists", not "it will stop you".

## 6. Relationship to the convergence ladder (§10.8)

The ladder covers loops that are *meant* to end:

```
watchdog (runtime) → shape recognition (pattern) → variant (proof)
```

A declared endless loop sits beside that ladder rather than on it: it has **no
termination obligation** by construction, and in exchange owes a **stop
authority**. Two obligations appear where one used to:

| | loop meant to end | declared endless loop |
|---|---|---|
| termination | variant proves it | not required, not claimed |
| stop authority | optional | **required** |
| liveness / progress | implied by termination | asserted, must not be provably unreachable |

## 7. Sequence — measure before building

The population matters: a requirement introduced on `[true]`-post reactive
declarations could either be a small, principled rule or a flag day. Measure
first, in this order:

1. **How many declarations carry a `[true]` postcondition**, and of those, how many
   are `node` or reactive `txn` (genuinely endless) rather than one-shot work with
   an unconstrained postcondition? Only the former would be subject to the new
   requirement.
2. **How many already carry a watchdog?** If most do, the rule codifies existing
   practice and breaks nothing. If almost none do, the rule is a real change of
   contract and needs a migration story (`--warn` first, then error).
3. **How many `[true]` *preconditions* are there?** They are exempt from
   `check_convergence_safety`, so they are where the current silence is loudest.

Then, if the numbers are material: implement B as a warning, migrate the corpus,
then promote to an error. C follows as a reported obligation.

## 8. Open question for Randy

B makes `[true]` postcondition mean "runs until stopped" — a **semantic** change to
an existing surface, not just a new check. If `[true]` post should keep meaning
"unconstrained", then A (a keyword) is forced despite its cost, or the endless-loop
form needs its own shape. That is the one decision this note cannot make for you.

> **Resolved below (§9).** Randy chose: keep `[true]` as it is, add a `forever`
> keyword with an optional `until`, and treat `forever` as a *waiver* — the loop
> may terminate, but showing that it does is not required. §3–§5 are kept as the
> reasoning that led there; §9 is the design.

## 9. Decided design — supersedes §3–§5

> **Name decided (Randy, 2026-11-14): `unbounded`.** Earlier parts of this note
> say `forever`; read that as `unbounded` unless §9.7's naming analysis is what is
> being discussed. The criterion: the keyword must say "no termination
> obligation", not "never ends", because the semantics is a *waiver* — the loop
> may terminate, showing that it does is simply not required.

Three decisions, and they change the shape of the answer:

1. **The `[true]` postcondition keeps its current meaning.** No semantic change to
   existing surface, so option B is off the table.
2. **A new `forever` keyword**, with an optional **`until`** condition.
3. **`forever` is a waiver, not an assertion.** The loop *may* terminate. What is
   waived is the obligation to show *that* it terminates — not the possibility.

That third point is the whole design, and it is a better answer than mine was.

### 9.1 What `forever` waives, and what it charges

`forever` does not remove an obligation; it swaps one for another:

| | default loop | `forever` loop |
|---|---|---|
| termination | must be shown (shape or `variant`) | **not required, not checked** |
| `check_convergence_safety` hard error | applies | **waived** — `forever` *is* the declared answer to it |
| interruptibility | not required | **required**: every cycle in the body reaches a `yield` |
| `variant` | allowed | **rejected** — it would prove termination, contradicting the waiver |
| pre/post on entry and on every `term` path | checked | **unchanged** |

So an intentionally-endless reactive loop becomes expressible *and pays a price*:
it must be interruptible. That is the "pre-agreed safety button", expressed in the
language rather than bolted on, and it resolves the conflation found in §2, where
`Unprovable` and "intended to run forever" were indistinguishable. The
`check_convergence_safety` hard error stops firing on deliberate design and keeps
firing on accidents — which is what it was for.

### 9.2 Forms

```briev
// runs until something outside stops it; post checked on every `term` path
unbounded node server [ready][done] {
    ...
};

// runs until the declared condition; the exit path owes C ⟹ post
unbounded until [shutdown] node server [ready][done] {
    ...
};

// unchanged: a convergent node
node poll [ready][done] { ... };
```

`forever` sits in the **modifier prefix** — where `async` already sits for nodes
and `seq` sits for `foreach` — so it adds a modifier rather than a new declaration
shape. `until [C]` is optional; when present it yields a checkable exit obligation
(`C ∧ I ⟹ post`) and makes the loop a `while !C` in disguise: unconstrained on
termination, provable on exit.

### 9.3 Interruptibility, precisely

The first reading of "headroom to be interrupted" is wrong, and the correction
matters: **a node body is already interruptible between ticks** — the reactor is
the scheduler, and each firing is a scheduling point. So `forever node` on its own
needs nothing extra.

The danger is an **inner unbounded loop** inside the body: that is where a program
stops being interruptible. So the obligation is:

> every **cycle** in the declaration's control-flow graph passes through a
> `yield` — or the looping construct is bounded.

That is a structural, decidable property: reachability of a `yield` from every
back-edge. `yield` is already *"the explicit scheduling point of the concurrent
scheduler"* (SPEC §12), and the compiler already walks statement reachability
(`analysis/termination.rs`), so this costs a graph query, not a timing analysis.

A body with no inner loop trivially satisfies it and should not be forced to write
a no-op `yield`.

### 9.4 The deadline stays at runtime

Structural interruptibility says "a `yield` is reachable". It does **not** say "it
arrives quickly". Bounding latency needs timing analysis the compiler does not
have — and that is exactly what the **required** watchdog is for:

```briev
forever node server [ready][done] ![progress] within 10ms -> on_stall() { ... };
```

`!` (required) already exists as `WatchdogSpec::is_required`, and already demands a
handler that restores the precondition. So the split is honest:

- **compile time** — every cycle reaches a `yield`: interruptibility;
- **runtime** — the deadline is met, or the handler runs: bounded latency.

Recommended, not required: `forever` guarantees the *opportunity* to interrupt;
`![…]` guarantees a *reaction* when the opportunity is not taken.

### 9.5 Open questions

1. **Does `forever` demand a literal `yield`?** Recommend the cycle rule of §9.3
   only — requiring a token `yield` in a body with no inner loop would be noise.
2. **`until` placement.** `forever until [C] node …` reads well and keeps node
   modifiers in one place; the alternative is a contract clause beside `variant`.
3. **Scope.** Only `node` and reactive `txn`. A plain `defn` with an inner loop
   needs a different story and probably should not get one yet.
4. **Admission cost.** `forever` is an author-surface keyword, so it goes through
   the SPEC's admission process — permanent cost. `forever … until` is the natural
   pair, and the pair is what justifies admitting it.
5. **Interaction with §10.8's ladder.** A `forever` loop is deliberately *off* the
   ladder: no termination rung applies. It should be reported as such — "declared
   endless; interruptibility checked" — rather than appearing as `Unprovable`,
   which would look like a failure.

### 9.6 The yield obligation is reachability, never execution

Clarification (Randy): the rule demands a `yield` be **reachable** on every cycle,
not that it **execute**. A conditional yield is fine, and a yield whose guard is
provably false costs nothing at runtime.

That is what keeps verification free:

- the check **never inserts** a yield; it only asks whether one is reachable;
- a guarded yield whose guard is false leaves the iteration bounded, and control
  returns to the scheduler at the *tick* boundary anyway — the reactor is the
  interruption point, so no synthetic yield is needed;
- emitted code is therefore unchanged by the obligation.

Stated precisely:

> for every back-edge in the declaration's CFG, either a `yield` is reachable
> before that edge, or the loop is provably bounded.

A body with no inner loop satisfies the second disjunct trivially — which is also
why `forever node` with no inner loop should not be forced to write a token
`yield`.

### 9.7 Naming

Criterion: the keyword should say **"no termination obligation"**, not "never
ends". We chose a *waiver* — the loop may terminate — so a name asserting
permanence overclaims, and avoiding exactly that overclaim is the point of the
feature.

| Candidate | Verdict |
|---|---|
| `forever` | reads as an assertion of permanence, fighting the semantics. Already loaded in the SPEC as prose for `halt` ("sleep-forever", "wait forever"). Rust's `loop` is the precedent that this *can* work (it may `break`), but the word still argues with the rule. |
| `indeterminate` | names a *diagnostic state*, not an intent — and sits uncomfortably close to `Unprovable`, the very thing this feature separates from intent. |
| `perpetual` | same overclaim as `forever`, longer, uncommon in keyword vocabulary. |
| `rolling` | collides with numeric vocabulary this codebase uses (rolling sum / rolling window). |
| `steady` | "steady state" reads well, but that is a **precise electronics term** (steady state vs transient) and Briev targets electronics surfaces. Domain collision. |
| `ongoing` | no collision, honest, reads well in `ongoing until [C]`; three syllables. |
| **`unbounded`** | standard PL vocabulary for "no static bound" — exactly the claim being made, with no permanence implied. **Recommended.** |
| `open` | open-ended, but `Open` already exists in the AST. |

**Recommendation: `unbounded until [C] node …`.** It is the only candidate that is
both short-ish and literally accurate: no termination bound is required, and
nothing is asserted about whether the loop ends. `ongoing` is the softer
alternative if `unbounded` reads too much like a diagnostic; `forever` is
defensible on precedent alone but invites readers to believe a claim we
deliberately do not make.

Whichever word is chosen, the `until [C]` half stays: it is what turns the waiver
back into a checkable exit.
