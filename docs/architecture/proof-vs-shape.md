# Proof vs Shape — the eternal compiler, the temporal language

**Date:** 2026-09-20
**Status:** active doctrine
**Complements:** `briev-capability-frontier.md` — that doc: the optimum must be
*expressible*; this doc: the optimum must not be *hardcoded*
**Rules fed:** Golden Rules 15, 23, 24

---

## The boundary

**The compiler keeps the eternal:**

- **Proof machinery** — aliasing, linearity, single-writer, independence,
  lifetime, dataflow. Properties of *any* program; forever true.
- **Rewrite rules licensed by proofs** — "a division by a loop-completed
  scalar may move past a linear consumer" is algebra, not softmax. Fusion
  from single-reader dead intermediates is topology, not attention.
- **General lowering machinery** — reduction synthesis from the program's
  own fold structure, warp slicing, placement from cost models. Mechanical
  consequences of structure, never recognitions of it.
- **Metaprogramming + expressiveness** — so any structure an author needs
  is expressible once, in the language, and expands into the full
  optimization pipeline as if hand-written.

**The language keeps the temporal:**

- Algorithm shapes — softmax today, its year-two replacement tomorrow.
  Current research will rename the hot kernel; a foundational language
  must not care.
- Composites and canonical bodies (`lib/std/`).
- Everything whose name rhymes with an application domain.

## The test

Delete every algorithm from stdlib. A researcher writes the year-two
algorithm in Briev with contracts. It reaches the hardware ceiling with
**zero compiler changes**.

Foundational = passes this test. cuBLAS fails it. A pattern-matching
compiler fails it slower.

## The detection boundary

- **Detect what proofs reach**: chain topology (RAW edges, single-reader
  intermediates), linearity of a fold, single-writer loop-completed
  denominators, deferral algebra.
- **Metaprogram what inference should not reach**: canonical kernel
  structures — pass decomposition, merge order, tile shapes. These are
  design decisions; they belong to authors, expressed once in stdlib.
- **The escape hatch is always open**: any author may invoke the composite
  explicitly (the cuBLAS move — you call it, the language makes the call
  fast). Narrow auto-detection can no longer silently cap capability,
  because the explicit path is universal.

## Existence proofs carry retirement gates

A hand-written kernel or hand-lowered emitter is legitimate engineering:
it proves the target is reachable and sets the numeric bar (the hand-PTX
flash kernel did exactly this: 123/94 µs). But it is temporal shape
knowledge sitting in the compiler's path, so it MUST be committed with a
retirement gate: it retires when the general machinery drives the plain
expanded composite to its numbers. The discipline applies to ourselves
before it calcifies — the deferred-region emitter's retirement is a
formal milestone, see `docs/plans/2026-09-20-metaprogrammed-composites.md`
(Stage 2).

## Worked example — the failure mode is natural

2026-09-20, mid-M3: the chain-fusion synthesis was written with
`Expr::Call("Max#")` / `Expr::Call("Exp#")` construction, a `-1e30`
identity, and 2-pass loop structure hand-built in Rust — while Rules
15/23 were in force and the two-tier doctrine was already documented.
The failure was caught by one question: "is this hardcoding?" The honest
answer: the detection half was proof-shaped and survived; the synthesis
half was the compiler learning what softmax IS, and it was deleted.

The lesson is NOT "that agent was careless." The lesson: the failure mode
is NATURAL — building the fused form in Rust is the fastest path to a
working demo, and every future optimizer will feel the same pull. Two
things stop it: the rule must be read every session (Golden Rule 24), and
the escape must be cheap — the metaprogramming layer must make the
language-side path EASIER than the Rust-side path.

## What this means for sequencing

1. Shape enters the language FIRST (declared composite); the compiler's
   general machinery consumes it. Never the reverse.
2. A structural matcher in the backend is a loan, not an asset. Every
   loan is recorded in a retirement ledger with its repayment gate.
3. When two designs both pass correctness, the one that deletes
   compiler-side shape knowledge wins — even when it is more work.
