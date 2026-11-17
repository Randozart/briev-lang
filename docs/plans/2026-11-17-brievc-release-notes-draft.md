# `brievc` release notes — body-verification campaign (draft)

**Status:** DRAFT, awaiting a version decision. Nothing is tagged or published.
**Scope:** the `--verify-bodies` work from PRs #104–#112 (merged onto `main`).
**Artifact mechanism:** `scripts/package_release.sh <vX.Y.Z>` produces a
self-contained tree (compiler, stdlib, config, plugins, examples) plus a SHA-256
sum file. There are currently **no release tags and no GitHub releases** in this
repository, so this would be the first cut of the campaign rather than an update
to an existing one.

## Why cut it at all

`brievc build <file> --verify-bodies` is a user-facing surface, and its capability
more than doubled during the campaign:

| | before campaign | now |
|---|---|---|
| proven obligations (`lib/std`, 92 files) | 952 | **2192** |
| unproven | 187 | 100 (1 distinct defn) |
| violated | 0 | **0** |

A user on the previously pinned compiler cannot see any of that.

## What a user gains

**Decided exactly, not heuristically**

- **Mask intervals** — `x & M ∈ [0, M]` for a literal non-negative mask.
- **Masked-plus-offset** — `(x & M) + n ∈ [n, M+n]`.
- **Literal-shift folding** — `1 << 24` (and other pure-literal shift/bitwise
  forms) fold to their exact value, so a shift bound is encodable at all instead
  of being refused as ill-sorted.
- **`Load#` width ranges** — `Load#(addr, w) ∈ [0, 2^(8w)-1]` for literal `w` in
  `1..=3`, because the backend zero-extends. Deliberately stops at 3 bytes: at 4
  the load equals the word width on wasm32 and the value may read negative.
- **Bit composition** — `a | b >= 0` when both operands are; `(x & m) << k >= 0`
  only when the shifted maximum stays below the sign bit (checked, since wrapping
  shift can push a positive value negative).
- **Relational values** are self-identical atoms, so a Bool body that *is* its own
  postcondition decides structurally.

**Contracts as sources of truth, at call sites**

- A **callee's declared postcondition** is usable as a fact about an *inline* call
  term (both in the type checker and in the body prover). This is what reaches
  memory-reading accessors (`byte_at`, `byte_raw`) and recursive helpers
  (`pow2`, `pow10`), which can never earn a value summary.
- Admission stays conservative: declared posts only, one level, no `false` posts,
  no self-recursion, an atom per **call site** (two identical calls are not one
  value), and only contracts that bound the result against a literal.

**Handles and results**

- `String`/`Data` are admitted as **opaque word handles**; representation casts
  (`String ↔ Data ↔ Int`) are identity on the word, while **conversions**
  (`String as Int` parses, `Int as String` formats) are never modelled as
  identity — the emitter is the authority for that distinction (§15.6 of the
  spec, verified against emitted IR).
- `s.^Length` / `CharCount#(s)` are non-negative integer atoms, one per handle.
- **Tuple results** are proved per field (`term.N`; a bare `term` names component
  0), including a refutation when a field contradicts the body.

**Honest reporting**

- `unproven` and `skipped` are distinct verdicts and stay distinct; a skip says
  *nothing was proved here*, never *passed*.
- `--verify-report <path>` emits machine-readable JSON (`proven`, `unproven`,
  `skipped`, `violated`), and `--no-link` produces a byte-identical report without
  paying to link.
- Only `violated` fails a build.

## Known gaps, stated rather than hidden

| Gap | Why it stands |
|---|---|
| `ip_v4` ×3 | needs disjoint-bit OR reasoning |
| `sha1_padded_len` ×2 | needs modular arithmetic |
| `is_alpha_loop`, `is_numeric_loop` | postcondition `i == s.^Length` — counter-vs-length, needs length algebra |
| `pf_exp` | path returns a call to the **recursive** `pf_exp_digits`, whose post bounds only component 0; a precondition fix was tried, rebuilt into the prelude, measured, and does not work |
| `Store#`, `SysCall#`, `Asm#`, `Argc#` | opaque by doctrine — a value summary would be a fabricated relation |
| String **content**, string **length algebra** (`CharCount#(a + b)`), `Capacity#` | explicitly out of model |
| `Float` (822 skips), generics/variadic (2007) | need a theory layer, not a gate flip |

## Version: **0.2.0** (decided 2026-11-17)

Pre-1.0 semver: a minor bump is the honest step, because the verification surface
materially widened (opaque handles, length atoms, per-field tuple results, declared
contracts as call-term facts) rather than merely gaining fixes.

`Cargo.toml` and `briev-verify/Cargo.toml` both go 0.1.0 → 0.2.0; `brievc --version`
reports `brievc 0.2.0`.

### Artifact, measured

```
briev-v0.2.0.tar.gz
29bc29980f9662784338451b8d310872aaaf491f0d8ed63cd5ddc6723c753164
```

Acceptance-tested from the **extracted tarball** (not the build tree):

- `brievc --version` → `brievc 0.2.0`
- `byte_at` proves its `[term >= 0 && term <= 255]`
- `violated: 0`

Census on the packaged compiler over `lib/std` (92 files): **proven 2216**,
unproven 101 (one distinct defn, `pf_exp`), skipped 13308, **violated 0**, one
build error.
