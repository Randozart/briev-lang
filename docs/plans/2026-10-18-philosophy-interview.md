# Philosophy Interview — 13 questions, verbatim answers, consistency verdict

**Date:** 2026-10-18
**Participants:** Randy Smits-Schreuder Goedheijt (language owner), opencode
(session agent).
**Purpose:** interrogate the owner's own architectural decisions point by
point so the README pillars could be rewritten with their honest basis —
claim, because, cost, falsifier — instead of doctrine-as-assertion.
**Method:** one question per pillar, delivered via structured prompts;
answers captured verbatim (lightly copy-edited for keyboard typos only;
meaning untouched).

---

## The interview

### P1a — "The compiler proves reachability and termination": what does the prover actually discharge today, and where is the honest prove/checks line?

> The idea is that contracts allow the programmer to state how they intend
> the code to run, like predicates. Many of our analysis tools use this
> information to check the code against the intent, but also implicitly use
> it as the shape of the graph.

### P1b — All-or-nothing transactions: what class of bug did this kill that you actually saw?

> None observed — preventive. The shape prevents the class from ever
> forming.

### P2a — Execution inferred: what does the compiler guarantee about node firing order, and what must the programmer just trust?

> It's much like predicates in Prolog and Dialog. It guarantees that which
> must follow, follows, through declaration alone.

### P2b — Is `seq` the confession that inference sometimes fails?

> Most software is not concerned with the shape or order of operations, so
> long as it's fast. But some software necessitates predictability. Things
> like memory layout, or some code that requires a function to run on the
> declared sequence despite the compiler being able to prove it can be
> emitted concurrently.

### P3a — LLVM's greatest wins came from ASSUMING (UB). You chose PROVING. Where did proof beat assumption — and is the #13/argmem miscompile the general law or the exception?

> UB is only needed, I think, because languages like C never properly
> allowed a programmer to codify their intent. UB, in that way, is a patch
> on incomplete design.

### P3b — What is the minimum declaration set for competitive codegen today? Is one-sided contracts actually enough?

> One-sided often is sufficient. Two-sided is so much harder for a
> programmer to get right, but pays off if done.

### P4a — Fused-attention: recognition lost 10×, derivation won 2.77×. General law or favorable anecdote? What is the falsifiable claim?

> I strive to never match algorithmic shapes in the compiler. If an
> algorithm works well, it must imply there is a reason it works well, and
> a general shape to be discovered.

### P4b — proof-vs-shape.md admits building recognition in Rust is the natural fast path. Is the honest pillar "we derive because we cannot trust ourselves to recognize"?

> If a language is to be trusted, it must be able to turn your algorithm
> into the best it is capable of. If we recognize particular shapes, we
> fail past and future computer scientists by penalising their algorithm
> with our ignorance.

(The owner selected the *technical/moral ceiling argument*, not the
psychological one — though the companion doc records both.)

### P5a — C's compiler has been smarter than C for 40 years and C won. Why is expressiveness closure different for Briev?

> C can't state intent.

### P5b — Has the year-two-algorithm acceptance test ever been run against a real unforeseen algorithm, or is it an unfalsified creed?

> Otto provided an algorithm from his university thesis and ran it against
> hand optimized C++. It was more stable and ran at roughly equal speed.
> This made me happy, because it proved my intent.

(The cpp-vs-briev experiment — `benchmarks/recommender_corpus/`,
`2026-10-16-cpp-vs-briev-contributor-findings.md`. The owner counts this as
the acceptance test's first live run: parity, not ceiling — the CPU lane is
the mature backend.)

### P6a — Keywords never buy speed, yet pragmas shipped wins for decades. What do you know that they don't — or what do you accept losing?

> I think those keywords are dirty, lazy abstractions. They require a
> programmer to be deeply familiar with the compiler to know what it does,
> and it's something a good compiler should know, or the language needs
> better syntax for. As such, I gave myself a rule: someone programming in
> Briev should never need to know obscure keywords to write optimized code,
> and if they use keywords, it's for correctness, never speed. In fact,
> anything handled directly by the compiler is disclosed as such.

### P7a — Which programs is Briev honestly FOR today, and which does it LOSE?

> I do not know. It started with native code, but I keep being surprised
> how cleanly the thesis holds up to different targets. This is something I
> will find out as we go.

### P7b — Is material independence on `.sbv` a result or a bet?

> We synthesised working logic in the past, but several overhauls
> deprecated this. It's still reachable, as the logic still applies.

### P8a — What PROOF does `when` unlock that `if` blocks — or is it purely cognitive friction?

> Genuinely, it's to encourage flat code in humans and LLMs alike. No
> deeply nested if/else chains. By renaming it `when`, it's decoupled from
> the baggage of `if`.

### P9a — No implicit concurrency (Rule 22): why ceremony instead of a safe sequential default?

> This is a disambiguation keyword, like plain vs. state let. It exists so
> there is a visual artifact of intent. And really, just that. Legible code
> that the compiler can better reason about, but humans as well. Concurrency
> ought to be explicit. It says "this node will fire while another node is
> running. Be aware of this."

---

## Consistency verdict

**Substantively consistent.** One law at every layer: *declare intent →
derive everything determinate → surface every ambiguity → disclose
everything the compiler does itself.* The pillar answers reinforce each
other: `when`-as-cognitive drops an overclaim; `seq`-as-intent matches
Rule 2; "C can't state intent" is derivation §1 compressed; one-sided
contracts is "doesn't need you correct, just right" made practical.

### The seven seams

1. **`async`/`sync<g>`: three answers to one question.** SPEC §2.4 calls
   them strategy keywords ("a constraint, never a selection");
   `keyword-taxonomy.md` §5 arms simultaneity under the ambiguity ERROR
   rule; the owner described them as disambiguation/legibility. Deeper:
   Rule 22 errors where the taxonomy's own soundness severity criterion
   says warn — serializing eligible pairs is sound, so by taxonomy §5 it
   should warn, not error; the owner's justification is legibility, which
   the taxonomy explicitly excludes ("severity is soundness, not taste").
   **Ruling: deferred.** README phrases functionally ("the compiler
   refuses to guess"); the class question needs a future design ruling.
2. **"Proves reachability and termination" vs P1a's answer.** The owner's
   own description (predicates + checking + graph shape) is softer than
   the README claim. **Ruling: prove-and-report framing** — "checks the
   program against your declared intent, proves what it can, reports what
   it can't, consumes every declared fact."
3. **"Recognition is the cardinal sin" vs live loans that win their A/Bs**
   (cooperative softmax kept at a measured 1.8–2.5× gap, dot at
   1.05–1.4×). Consistent only as discipline: disclosed, gated,
   debt-not-principle. The README must phrase recognition as an obligation
   plus gate mechanism, never as an achieved state.
4. **Universal phrasing vs "I do not know" (P7a).** Fixed by asserting the
   thesis while making the boundary empirical ("decided by benchmarks, not
   claims"). `.sbv` stated as a bet under test with a deprecated prior
   result.
5. **"One syntax, all materials" vs `.bad`/`.bld` deliberately different
   grammars.** Core syntax shared; auxiliary dialects are different tiers
   by design. TL;DR bullet scoped.
6. **"UB is a patch" vs Briev feeding LLVM's UB-lever metadata.** Resolved
   by capability-frontier framing: same levers, proven provenance (`#13`
   showed assumption lying; proof can't). Wording: "earn the claims,"
   never "reject the levers."
7. **Pragma refusal vs `^^^` overrides / authority postconditions.**
   Consistent via disclosed escape hatches. Wording: "nothing hidden,"
   never "no escapes."

### Where the answers changed the pillar text

- P1b: the all-or-nothing pillar now claims *prevention*, not war stories.
- P3a: the fuel pillar gained the UB-as-patch thesis and the earn-vs-assert
  distinction.
- P4a/P4b: the derivation pillar leads with the trust/moral argument; the
  loans sentence keeps it honest.
- P5b: the closure pillar now cites a real, once-run acceptance test
  (Otto's thesis algorithm) instead of an unfalsified creed.
- P7a: the thesis section gained the honest-boundary paragraph.
- P8a: the friction pillar now says *cognitive engineering, not a proof
  mechanism* — the proofs live in `match` exhaustiveness.
- P9a: concurrency pillar written functionally per the Seam-1 deferral.
