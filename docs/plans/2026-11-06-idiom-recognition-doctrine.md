# Idiom recognition — doctrine amendment + first instance plan

**2026-11-06.** Status: ACTIVE.
**Authority:** operator decision (2026-11-06) — idiom recognition is
permitted, derivation-first. This plan amends
`docs/architecture/derivation-not-recognition.md` (§12) and sequences the
first instance.
**Reading order:** `derivation-not-recognition.md` §2, §7, §8, §11 (the
worked example), then §12 (the amendment this plan lands).

## Why this is defensible (the operator's argument, recorded)

The doctrine bans recognition of *what the algorithm is* — op sequences,
names, magic constants that would live silently in the code forever and
cause heisenbugs. It has never banned deriving from *structure*: §3's
declared-fact table (reduction topology, affine forms) and §7's admissible
matchers (the GEMM "flattened-2-D affine shape") are already
shape recognition, i.e. derivation. The new category — **idiom
recognition** — sits deliberately between them: a language-level
structural idiom (associative reduce, scan, transpose) whose semantics a
hardware primitive CONTAINS. Matching against *intent-shaped structure*
is not cargo cult; it is the same fact-derivation the general pass does,
accelerated. The rule that keeps it honest is primacy: **general
derivation remains the obligation; the idiom matcher is an accelerant on
the same facts, feeding the same machinery, retiring when derivation
catches up.**

## W1 — Doctrine amendment (this commit)

`derivation-not-recognition.md` gains §12:
- §12.1 the three categories (shape / idiom / algorithm) with the litmus:
  *idiom recognition matches intent-shaped structure that maps to hardware
  semantics; algorithm recognition matches cargo-culted sequences.*
- §12.2 the five gates: structure-not-name; hardware-consumer test;
  proof license (Rule 25); disclosure both directions (honest compiler);
  retirement gate + ledger row.
- §12.3 primacy of derivation: matcher output is a frontend plan input
  into the general machinery, never a backend arm (Rule 6); matcher
  lifetime = measured parity debt; "every hour on a matcher is an hour
  owed to the general pass."
- §12.4 what stays banned (op sequences, constants, identifiers,
  undisclosed firing, ceiling-by-coverage) — with ONE licensed exception:
  **the silicon carve-out (§12.6)**. When a consumer is silicon literally
  designed to execute an algorithm (fixed-function attention block, tensor
  core MMA datapath, hardware softmax, AES/CRC engine), matching that
  algorithm to ITS OWN interface is legal — the die is public, immutable
  semantics, so nothing can rot silently. Literal-build test, proof
  license, disclosure, ledger and retirement still apply; and the match
  targets that silicon's driver/ABI — it never licenses software kernel
  emission "because the algorithm is popular". Direct consequence: the
  PTL node's NPU (fixed-function AI blocks, enumerated by OpenVINO) is a
  legal algorithm-match target; the programmable iGPU is not.
- §12.5 enforcement: recognition-gate extension; ledger row mandatory;
  first instance named.

Merge-time item (NOT in this branch — AGENTS.md is shared with the main
line): Rule 23 gains a pointer line to §12. The main session applies it
when this branch merges.

## W2 — Ledger rows + mechanical gate

Extend the coverage ledger (§8) with idiom rows keyed
`(idiom × structural facts × hardware consumer) → pass → diagnostic →
parity test → retirement condition`. CI/recognition-gate invariant: an
idiom matcher without a row, or a row without a why-not diagnostic path,
is a gate failure.

## W3 — First instance: the reduction idiom

**Deliberately chosen because the general machinery already exists** (§3
row: `foreach` fold → reduction synthesis: tree / warp-slice / split).
The idiom extends the *fact derivation* to the hand-written form:

- **Detected shape:** an `accel` counted-loop body of the form
  `acc = acc ⊕ a[affine(k)]` — single accumulator scalar, loop variable
  bound to the work-item index, op ∈ {add, mul, min, max}, accumulator
  not otherwise read or written in the node.
- **Facts derived (not matched):** reduction topology (§3), op
  associativity class (from the primitive-op table + declared algebraic
  laws), numeric license (f32 default; f16-accumulate only when the
  numeric contract licenses — gate 3).
- **Lowering:** route into the EXISTING reduction synthesis (warp-slice /
  split / tree machinery + `GroupNonUniform*` ops on the SPIR-V lane).
  The matcher picks the same plan the `foreach` fold picks; it does not
  emit a private kernel.
- **Disclosure:** firing → `info: reduce-idiom (assoc add, f32) →
  reduction synthesis (warp-slice)`. Near-miss → named reason
  (`reduce-idiom not applied: accumulator read inside body`).
- **Tests:** analysis unit tests (detect / reject each near-miss class);
  IR test (GroupNonUniform ops present, no private kernel);
  PTL GPU gate (Vulkan lane, g0-style host reference, same-geometry);
  parity check vs the plain vectorized loop (retirement evidence).
- **Acceptance:** the hand-written accumulator loop and the `foreach`
  fold of the same reduction produce the same plan lane and comparable
  numbers; the matcher row enters the ledger with its retirement
  condition.

## W4 — Why-not diagnostics infrastructure

A single author-facing mechanism for idiom near-misses: which structural
fact failed, stated in house diagnostic style (what is wrong, the
relevant fact, the concrete fix). Feeds every future idiom.

## W5 — Candidate inventory (each needs its own plan instance)

Scan (inclusive/exclusive prefix → subgroup scan ops), transpose-in-GEMM
(→ ldmatrix-class load path), map-chain fusion (already partially
general via chain fusion). Listed for completeness; none approved
without its own gates + ledger row.

## Sequencing

W1 now. W3 is the next implementation session (it is the proof that the
amendment is real: the matcher feeds existing machinery and retires on
parity). W2/W4 land with W3 (a matcher without its ledger row or
why-not path must not ship). W5 strictly on demand.

## Non-goals

No backend recognition arms. No matcher may fire without disclosure. No
matcher survives general-machinery parity. No expansion of the idiom
inventory by convenience — the inventory is closed by review.
