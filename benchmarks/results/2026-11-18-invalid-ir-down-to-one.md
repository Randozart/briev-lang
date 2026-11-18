# The invalid-IR census: 3 → 1, and the check is now a gate (2026-11-18)

Follows `2026-11-18-ir-verification-and-diagnostics.md` (the verifier + renderer) and
implements P1/P2 of `2026-11-18-gate-the-ir-check-and-advance.md`.

## The two `glue_gen` fixtures were the fixture's fault — and a compiler gap

`tests/fixtures/glue_gen/handwritten_glue.bv` declared, in ten lines:

```briev
proto C_String: String {
    axiom CastTo(String<UTF8>) = cstr_to_briev(#Lh);
    axiom CastFrom(String<UTF8>) = str_to_c(#Lh);
};
```

and imported nothing. `str_to_c`/`cstr_to_briev` live in `lib/glue/c.bv`, so the
casting graph emitted `call ptr @str_to_c(ptr …)` for a function the program never
defined — a module LLVM rejects. The generated twin had the same gap. Both now
`import "glue/c.bv";` and build clean; `tests/glue_gen_parity.rs` still passes, so the
pair still agrees.

**The compiler-side gap is the interesting half.** The emitted call was never
validated. A first attempt put the check at the type level ("every cast binding must
name a callable in the program") and **was rejected by the sweep**: it fired on
`lib/glue/c/protocols.bv` and `lib/glue/c/types.bv`, which legitimately *declare*
protocols and leave the bindings to whoever imports them (frontend errors 93 → 96).
That is the correct behaviour for a module.

The shipped check is at the IR level and has no false positives by construction:
**a call target with no `define` and no `declare`**. Modules that declare a protocol
and never cast emit no calls, so they are untouched; a program that does cast through
an undefined binding is caught with:

```
error: the compiler produced invalid IR for 'p.ll'
  why:  LLVM rejected the module: the module calls '@str_to_c' with no definition or declaration
  note: this is a compiler defect, not a mistake in your program
```

Only `call … @name(` targets count — a global reference or a function-pointer load is
not a missing callee (pinned by a test, because the type-level version proved how easy
that distinction is to get wrong).

## Census

Full tree, 8 workers, debug compiler, 21m35:

| | before | after |
|---|---|---|
| `files_valid` | 333 | **332** |
| `files_invalid_ir` | 3 | **1** |
| `files_frontend_error` | 93 | 96 → then **93** once the over-strict type gate was removed |

The one remaining entry is `examples/syntax/meta/defn-composite.bv` — the
loop-carried `Float` (BUGS.md), which is P4 and a different bug: valid-looking IR that
computes the wrong number is not what this check can see, and it never claimed to.

## The gate

`scripts/ir_verify_corpus.sh` now sweeps in parallel (`IR_VERIFY_JOBS`) and enforces
`tests/ir_verify_allow.txt`, a shrink-only list: the sweep exits 1 when the set of
invalid modules *differs* from it, so a new invalid module fails **and** a stale
exemption fails. The ratchet only applies to full-tree runs, since a partial sweep
would report every out-of-scope entry as stale (found by running it: `… tests` flagged
`defn-composite` as "no longer invalid").

`.github/workflows/ir-verify.yml`: `tests/` on pull requests, the whole tree on push
to main, per-file logs uploaded on failure.

## Verification

* `cargo nextest run --lib ir_verify` — 8 tests (adds the undefined-call check, its
  non-call distinction, and the declared-callee case).
* `cargo nextest run --test glue_gen_parity` — pass.
* `bash scripts/gate.sh quick` — pass.
* Full sweep — 1 known invalid file, allowance matched.
